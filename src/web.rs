//! Browser-facing endpoints: robots.txt, Google sign-in at `/`, and the
//! drag-and-drop dialog for adding entries.
//!
//! Google sign-in is only enabled when `google_client_id`,
//! `google_client_secret` and `email` are all present in the config file.
//! After a successful sign-in with the configured email a stateless
//! HMAC-signed session cookie is issued; it is deliberately not a valid
//! OAuth bearer token (and vice versa).

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{AppendHeaders, Html, IntoResponse, Redirect, Response};
use axum::Json;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::Utc;
use hmac::Mac;
use rand::Rng;
use serde::Deserialize;
use sha2::Sha256;
use tracing::{info, warn};

use crate::api::AppState;
use crate::types::{AddRequest, AddResponse, AppError};

const SESSION_COOKIE: &str = "gitideas_session";
const STATE_COOKIE: &str = "gitideas_oauth_state";
const SESSION_TTL_SECS: i64 = 2_592_000; // 30 days
const STATE_TTL_SECS: i64 = 600; // 10 minutes

const GOOGLE_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

pub struct GoogleConfig {
    pub client_id: String,
    pub client_secret: String,
    /// The one email address allowed to use the web dialog.
    pub email: String,
}

fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::rng().fill(&mut buf[..]);
    buf.iter().map(|b| format!("{:02x}", b)).collect()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

// --- robots.txt ---

pub async fn robots_txt() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        "User-agent: *\nDisallow: /\n",
    )
}

// --- cookies ---

fn get_cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .map(|c| c.trim())
        .find_map(|c| c.strip_prefix(name)?.strip_prefix('='))
}

fn cookie_header(state: &AppState, name: &str, value: &str, max_age: i64) -> HeaderValue {
    let secure = if state.oauth.server_url.starts_with("https://") {
        "; Secure"
    } else {
        ""
    };
    HeaderValue::from_str(&format!(
        "{}={}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}{}",
        name, value, max_age, secure
    ))
    .expect("cookie header")
}

// --- session tokens ---

fn session_mac(state: &AppState, email: &str, issued_at: i64) -> hmac::Hmac<Sha256> {
    let mut mac = hmac::Hmac::<Sha256>::new_from_slice(state.auth_token.as_bytes())
        .expect("HMAC key");
    // "web:" domain prefix keeps session cookies distinct from OAuth tokens.
    mac.update(format!("web:{}:{}", email, issued_at).as_bytes());
    mac
}

fn issue_session(state: &AppState, email: &str) -> String {
    let issued_at = Utc::now().timestamp();
    let sig = hex::encode(session_mac(state, email, issued_at).finalize().into_bytes());
    // email is base64url-encoded so ':' can be used as separator safely.
    format!("{}:{}:{}", URL_SAFE_NO_PAD.encode(email), issued_at, sig)
}

/// Returns the signed-in email if the request carries a valid session cookie.
pub fn session_email(state: &AppState, headers: &HeaderMap) -> Option<String> {
    let cookie = get_cookie(headers, SESSION_COOKIE)?;
    let mut parts = cookie.splitn(3, ':');
    let (Some(email_b64), Some(issued_at_str), Some(sig_hex), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    let email = String::from_utf8(URL_SAFE_NO_PAD.decode(email_b64).ok()?).ok()?;
    let issued_at = issued_at_str.parse::<i64>().ok()?;
    let now = Utc::now().timestamp();
    if now < issued_at || now - issued_at > SESSION_TTL_SECS {
        return None;
    }
    let sig = hex::decode(sig_hex).ok()?;
    if session_mac(state, &email, issued_at).verify_slice(&sig).is_err() {
        return None;
    }
    // The allowed email can change between restarts; re-check on every request.
    let google = state.google.as_ref()?;
    if !google.email.eq_ignore_ascii_case(&email) {
        return None;
    }
    Some(email)
}

// --- GET / ---

pub async fn index(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if state.google.is_none() {
        return Html(page(
            "gitideas",
            "<h1>gitideas</h1><p>Google sign-in is not configured on this server.</p>",
        ))
        .into_response();
    }
    match session_email(&state, &headers) {
        Some(email) => Html(dialog_page(&email)).into_response(),
        None => Html(page(
            "gitideas - Sign in",
            r#"<h1>gitideas</h1>
<p>Sign in to add ideas, todos, memories and notes.</p>
<p><a class="button" href="/auth/google">Sign in with Google</a></p>"#,
        ))
        .into_response(),
    }
}

// --- GET /auth/google ---

pub async fn google_login(State(state): State<Arc<AppState>>) -> Response {
    let Some(google) = state.google.as_ref() else {
        return (StatusCode::NOT_FOUND, "google sign-in not configured").into_response();
    };
    let csrf = random_hex(16);
    let redirect_uri = format!("{}/auth/google/callback", state.oauth.server_url);
    let url = reqwest::Url::parse_with_params(
        GOOGLE_AUTH_URL,
        &[
            ("client_id", google.client_id.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
            ("response_type", "code"),
            ("scope", "openid email"),
            ("state", csrf.as_str()),
            ("prompt", "select_account"),
        ],
    )
    .expect("google auth url");

    (
        [(
            header::SET_COOKIE,
            cookie_header(&state, STATE_COOKIE, &csrf, STATE_TTL_SECS),
        )],
        Redirect::to(url.as_str()),
    )
        .into_response()
}

// --- GET /auth/google/callback ---

#[derive(Deserialize)]
pub struct GoogleCallbackQuery {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
}

#[derive(Deserialize)]
struct GoogleTokenResponse {
    id_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Deserialize)]
pub struct IdTokenClaims {
    pub iss: String,
    pub aud: String,
    pub exp: i64,
    pub email: Option<String>,
    #[serde(default)]
    pub email_verified: bool,
}

/// Decode the claims of a Google ID token. The token is obtained directly from
/// Google's token endpoint over TLS, so (per Google's guidance) the signature
/// does not need to be verified; we still check issuer, audience and expiry.
pub fn parse_id_token(id_token: &str, client_id: &str) -> Result<IdTokenClaims, String> {
    let payload = id_token
        .split('.')
        .nth(1)
        .ok_or("malformed id_token")?;
    let bytes = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|e| format!("id_token payload not base64url: {}", e))?;
    let claims: IdTokenClaims =
        serde_json::from_slice(&bytes).map_err(|e| format!("id_token claims: {}", e))?;
    if claims.iss != "https://accounts.google.com" && claims.iss != "accounts.google.com" {
        return Err(format!("unexpected issuer {}", claims.iss));
    }
    if claims.aud != client_id {
        return Err("id_token audience mismatch".to_string());
    }
    if claims.exp < Utc::now().timestamp() {
        return Err("id_token expired".to_string());
    }
    Ok(claims)
}

pub async fn google_callback(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<GoogleCallbackQuery>,
) -> Response {
    let Some(google) = state.google.as_ref() else {
        return (StatusCode::NOT_FOUND, "google sign-in not configured").into_response();
    };
    let clear_state = cookie_header(&state, STATE_COOKIE, "", 0);

    if let Some(err) = q.error {
        warn!(error = %err, "google sign-in error");
        return (
            StatusCode::BAD_REQUEST,
            [(header::SET_COOKIE, clear_state)],
            Html(page("gitideas", "<h1>Sign-in failed</h1><p><a href=\"/\">Try again</a></p>")),
        )
            .into_response();
    }

    let expected_state = get_cookie(&headers, STATE_COOKIE);
    match (&q.state, expected_state) {
        (Some(s), Some(e)) if s == e => {}
        _ => {
            warn!("google sign-in rejected: state mismatch");
            return (
                StatusCode::BAD_REQUEST,
                [(header::SET_COOKIE, clear_state)],
                "invalid state",
            )
                .into_response();
        }
    }
    let Some(code) = q.code else {
        return (StatusCode::BAD_REQUEST, "missing code").into_response();
    };

    let redirect_uri = format!("{}/auth/google/callback", state.oauth.server_url);
    // Overridable so tests can point the exchange at a local mock.
    let token_url =
        std::env::var("GITIDEAS_GOOGLE_TOKEN_URL").unwrap_or_else(|_| GOOGLE_TOKEN_URL.to_string());
    let client = reqwest::Client::new();
    let resp = client
        .post(&token_url)
        .form(&[
            ("code", code.as_str()),
            ("client_id", google.client_id.as_str()),
            ("client_secret", google.client_secret.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
            ("grant_type", "authorization_code"),
        ])
        .send()
        .await;

    let token: GoogleTokenResponse = match resp {
        Ok(r) => match r.json().await {
            Ok(t) => t,
            Err(e) => {
                warn!(error = %e, "google token response unreadable");
                return (StatusCode::BAD_GATEWAY, "google token exchange failed").into_response();
            }
        },
        Err(e) => {
            warn!(error = %e, "google token exchange failed");
            return (StatusCode::BAD_GATEWAY, "google token exchange failed").into_response();
        }
    };

    let Some(id_token) = token.id_token else {
        warn!(
            error = token.error.as_deref().unwrap_or("?"),
            description = token.error_description.as_deref().unwrap_or(""),
            "google token exchange returned no id_token"
        );
        return (StatusCode::BAD_GATEWAY, "google token exchange failed").into_response();
    };

    let claims = match parse_id_token(&id_token, &google.client_id) {
        Ok(c) => c,
        Err(e) => {
            warn!(error = %e, "google id_token rejected");
            return (StatusCode::BAD_GATEWAY, "invalid id_token").into_response();
        }
    };

    let email = claims.email.unwrap_or_default();
    if !claims.email_verified || !google.email.eq_ignore_ascii_case(&email) {
        warn!(email = %email, verified = claims.email_verified, "web login rejected: email not allowed");
        return (
            StatusCode::FORBIDDEN,
            [(header::SET_COOKIE, clear_state)],
            Html(page("go away!", "<h1>go away!</h1>")),
        )
            .into_response();
    }

    info!(email = %email, "web login");
    let session = issue_session(&state, &google.email);
    (
        AppendHeaders([
            (
                header::SET_COOKIE,
                cookie_header(&state, SESSION_COOKIE, &session, SESSION_TTL_SECS),
            ),
            (header::SET_COOKIE, clear_state),
        ]),
        Redirect::to("/"),
    )
        .into_response()
}

// --- POST /web/logout ---

pub async fn logout(State(state): State<Arc<AppState>>) -> Response {
    (
        [(
            header::SET_COOKIE,
            cookie_header(&state, SESSION_COOKIE, "", 0),
        )],
        Redirect::to("/"),
    )
        .into_response()
}

// --- POST /web/add ---

pub async fn web_add(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<AddRequest>,
) -> Result<Json<AddResponse>, Response> {
    let Some(email) = session_email(&state, &headers) else {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"ok": false, "error": "not signed in"})),
        )
            .into_response());
    };
    info!(email = %email, "web add");
    crate::api::do_add(&state, req)
        .await
        .map(Json)
        .map_err(AppError::into_response)
}

// --- HTML ---

const CSS: &str = r#"
  body { font-family: system-ui, sans-serif; max-width: 720px; margin: 40px auto; padding: 0 20px; color: #222; }
  h1 { font-size: 1.4em; }
  a.button, button { display: inline-block; padding: 10px 24px; background: #0066cc; color: #fff; border: 0; border-radius: 6px; text-decoration: none; cursor: pointer; font-size: 1em; }
  button:disabled { background: #999; cursor: default; }
  label { display: block; margin-top: 14px; font-weight: 600; }
  input[type=text], select, textarea { width: 100%; padding: 8px; margin-top: 4px; box-sizing: border-box; font: inherit; }
  textarea { min-height: 260px; font-family: ui-monospace, monospace; }
  .drop { margin-top: 14px; padding: 18px; border: 2px dashed #bbb; border-radius: 8px; text-align: center; color: #666; }
  .drop.over { border-color: #0066cc; background: #eef5ff; color: #0066cc; }
  .bar { display: flex; justify-content: space-between; align-items: center; }
  .bar form { margin: 0; }
  .bar button { background: #eee; color: #222; padding: 6px 12px; }
  .status { margin-top: 12px; min-height: 1.4em; }
  .status.ok { color: #1a7f37; }
  .status.err { color: #c00; }
  .hint { font-size: 0.85em; color: #666; }
"#;

fn page(title: &str, body: &str) -> String {
    format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><meta name=\"robots\" content=\"noindex\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>{}</title><style>{}</style></head><body>{}</body></html>",
        html_escape(title),
        CSS,
        body
    )
}

fn dialog_page(email: &str) -> String {
    let body = format!(
        r#"<div class="bar"><h1>gitideas</h1>
<form method="POST" action="/web/logout"><span class="hint">{email}</span> <button type="submit">Sign out</button></form></div>
<form id="add">
  <label for="type">Type</label>
  <select id="type" name="type">
    <option value="MEMORY" selected>MEMORY</option>
    <option value="IDEA">IDEA</option>
    <option value="TODO">TODO</option>
    <option value="NOTES">NOTES</option>
  </select>
  <label for="subject">Subject</label>
  <input type="text" id="subject" name="subject" maxlength="120" required autocomplete="off">
  <label for="text">Text</label>
  <textarea id="text" name="text" placeholder="Type, paste, or drop text and images here"></textarea>
  <div class="drop" id="drop">Drop files or paste images here. Images are embedded as base64; text files are appended.</div>
  <p class="hint">Markdown is preferred. Total size must stay under 1&nbsp;MB.</p>
  <button type="submit" id="submit">Add</button>
  <div class="status" id="status"></div>
</form>
<script>
(function() {{
  const form = document.getElementById('add');
  const text = document.getElementById('text');
  const subject = document.getElementById('subject');
  const drop = document.getElementById('drop');
  const status = document.getElementById('status');
  const submit = document.getElementById('submit');
  const MAX = 1048576;

  function setStatus(msg, cls) {{ status.textContent = msg; status.className = 'status ' + (cls || ''); }}

  function insert(s) {{
    const start = text.selectionStart, end = text.selectionEnd, v = text.value;
    const before = v.slice(0, start), after = v.slice(end);
    const pad = before.length && !before.endsWith('\n') ? '\n' : '';
    text.value = before + pad + s + after;
    const pos = (before + pad + s).length;
    text.setSelectionRange(pos, pos);
    text.focus();
  }}

  // If the subject is empty, use the first non-empty line of s, cut at 5 words.
  function suggestSubject(s) {{
    if (subject.value) return;
    const line = (s || '').split(/\r?\n/).map(l => l.trim()).find(l => l.length);
    if (!line) return;
    subject.value = line.split(/\s+/).slice(0, 5).join(' ').slice(0, 120);
  }}

  function insertText(t) {{
    suggestSubject(t);
    insert(t.endsWith('\n') ? t : t + '\n');
  }}

  function addFile(file) {{
    const name = file.name || 'pasted';
    const reader = new FileReader();
    if (file.type.startsWith('image/')) {{
      if (!subject.value) subject.value = name.slice(0, 120);
      reader.onload = () => insert('![' + name.replace(/[\[\]]/g, '') + '](' + reader.result + ')\n');
      reader.readAsDataURL(file);
    }} else {{
      reader.onload = () => insertText(reader.result);
      reader.readAsText(file);
    }}
  }}

  function handleTransfer(dt) {{
    let handled = false;
    if (dt.files && dt.files.length) {{
      for (const f of dt.files) addFile(f);
      handled = true;
    }} else if (dt.items) {{
      for (const it of dt.items) {{
        if (it.kind === 'file') {{ const f = it.getAsFile(); if (f) {{ addFile(f); handled = true; }} }}
      }}
    }}
    return handled;
  }}

  ['dragenter', 'dragover'].forEach(ev => document.addEventListener(ev, e => {{
    e.preventDefault(); drop.classList.add('over');
  }}));
  ['dragleave', 'drop'].forEach(ev => document.addEventListener(ev, e => {{
    if (ev === 'drop' || e.target === document.documentElement) drop.classList.remove('over');
  }}));
  document.addEventListener('drop', e => {{
    e.preventDefault();
    if (handleTransfer(e.dataTransfer)) return;
    const t = e.dataTransfer.getData('text/plain');
    if (t) insertText(t);
  }});

  document.addEventListener('paste', e => {{
    const cd = e.clipboardData;
    if (!cd) return;
    let hasFile = false;
    for (const it of cd.items) if (it.kind === 'file') hasFile = true;
    if (hasFile) {{ e.preventDefault(); handleTransfer(cd); return; }}
    const t = cd.getData('text/plain');
    if (e.target === subject) return;
    if (e.target === text) {{ suggestSubject(t); return; }}
    // plain text pasted outside the inputs goes into the text area
    e.preventDefault();
    if (t) insertText(t);
  }});

  form.addEventListener('submit', async e => {{
    e.preventDefault();
    const body = {{ type: document.getElementById('type').value, subject: subject.value.trim(), text: text.value }};
    if (new TextEncoder().encode(body.text).length > MAX) {{ setStatus('text exceeds 1 MB', 'err'); return; }}
    submit.disabled = true; setStatus('adding…');
    try {{
      const r = await fetch('/web/add', {{ method: 'POST', headers: {{ 'Content-Type': 'application/json' }}, body: JSON.stringify(body) }});
      if (r.status === 401) {{ location.reload(); return; }}
      const j = await r.json();
      if (r.ok && j.ok) {{
        setStatus('added ' + body.type + ' ' + j.id + ' to ' + j.file, 'ok');
        subject.value = ''; text.value = '';
      }} else {{
        setStatus(j.error || ('error ' + r.status), 'err');
      }}
    }} catch (err) {{
      setStatus('request failed: ' + err, 'err');
    }} finally {{
      submit.disabled = false;
    }}
  }});
}})();
</script>"#,
        email = html_escape(email),
    );
    page("gitideas", &body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state(google: Option<GoogleConfig>) -> AppState {
        AppState {
            git_lock: tokio::sync::Mutex::new(()),
            auth_token: "secret".to_string(),
            repo_path: std::path::PathBuf::from("/nonexistent"),
            oauth: crate::oauth::OAuthState::new("http://localhost:1".to_string()),
            google,
        }
    }

    fn google(email: &str) -> GoogleConfig {
        GoogleConfig {
            client_id: "cid".to_string(),
            client_secret: "cs".to_string(),
            email: email.to_string(),
        }
    }

    fn headers_with_cookie(v: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, HeaderValue::from_str(v).unwrap());
        h
    }

    #[test]
    fn session_round_trip() {
        let state = test_state(Some(google("Me@Example.com")));
        let s = issue_session(&state, "me@example.com");
        let h = headers_with_cookie(&format!("other=1; {}={}", SESSION_COOKIE, s));
        assert_eq!(session_email(&state, &h).as_deref(), Some("me@example.com"));
    }

    #[test]
    fn session_rejects_tampering_and_other_email() {
        let state = test_state(Some(google("me@example.com")));
        let s = issue_session(&state, "me@example.com");
        let mut bad = s.clone();
        bad.pop();
        bad.push('0');
        assert!(session_email(&state, &headers_with_cookie(&format!("{}={}", SESSION_COOKIE, bad))).is_none());

        let other = issue_session(&state, "someone@else.com");
        assert!(session_email(&state, &headers_with_cookie(&format!("{}={}", SESSION_COOKIE, other))).is_none());
        assert!(session_email(&state, &HeaderMap::new()).is_none());
    }

    #[test]
    fn session_cookie_is_not_a_bearer_token() {
        let state = test_state(Some(google("me@example.com")));
        let s = issue_session(&state, "me@example.com");
        let rt = tokio::runtime::Runtime::new().unwrap();
        assert!(!rt.block_on(crate::oauth::validate_oauth_token(&state, &s)));
    }

    fn make_id_token(claims: serde_json::Value) -> String {
        format!(
            "{}.{}.sig",
            URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#),
            URL_SAFE_NO_PAD.encode(claims.to_string())
        )
    }

    #[test]
    fn id_token_parsing() {
        let exp = Utc::now().timestamp() + 60;
        let good = make_id_token(serde_json::json!({
            "iss": "https://accounts.google.com", "aud": "cid", "exp": exp,
            "email": "me@example.com", "email_verified": true
        }));
        let c = parse_id_token(&good, "cid").unwrap();
        assert_eq!(c.email.as_deref(), Some("me@example.com"));
        assert!(c.email_verified);

        let wrong_aud = make_id_token(serde_json::json!({
            "iss": "https://accounts.google.com", "aud": "other", "exp": exp
        }));
        assert!(parse_id_token(&wrong_aud, "cid").is_err());

        let expired = make_id_token(serde_json::json!({
            "iss": "https://accounts.google.com", "aud": "cid", "exp": exp - 120
        }));
        assert!(parse_id_token(&expired, "cid").is_err());

        let bad_iss = make_id_token(serde_json::json!({
            "iss": "https://evil.example", "aud": "cid", "exp": exp
        }));
        assert!(parse_id_token(&bad_iss, "cid").is_err());
        assert!(parse_id_token("garbage", "cid").is_err());
    }
}
