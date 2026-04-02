use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{Query, State};
use tracing::{info, warn};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Json;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::api::AppState;

// --- OAuth state stored in AppState ---

pub struct OAuthState {
    pub clients: Mutex<HashMap<String, RegisteredClient>>,
    pub auth_codes: Mutex<HashMap<String, AuthCode>>,
    pub access_tokens: Mutex<HashMap<String, TokenInfo>>,
    pub server_url: String,
}

impl OAuthState {
    pub fn new(server_url: String) -> Self {
        Self {
            clients: Mutex::new(HashMap::new()),
            auth_codes: Mutex::new(HashMap::new()),
            access_tokens: Mutex::new(HashMap::new()),
            server_url,
        }
    }
}

pub struct RegisteredClient {
    pub client_name: String,
    pub redirect_uris: Vec<String>,
}

pub struct AuthCode {
    pub client_id: String,
    pub redirect_uri: String,
    pub code_challenge: String,
    pub created: Instant,
}

pub struct TokenInfo {
    pub created: Instant,
    pub expires_in: Duration,
}

fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::rng().fill(&mut buf[..]);
    buf.iter().map(|b| format!("{:02x}", b)).collect()
}

// --- Well-Known Endpoints ---

pub async fn protected_resource_metadata(
    State(state): State<Arc<AppState>>,
) -> Json<serde_json::Value> {
    let url = &state.oauth.server_url;
    Json(serde_json::json!({
        "resource": format!("{}/mcp", url),
        "authorization_servers": [url],
        "bearer_methods_supported": ["header"],
    }))
}

pub async fn authorization_server_metadata(
    State(state): State<Arc<AppState>>,
) -> Json<serde_json::Value> {
    let url = &state.oauth.server_url;
    Json(serde_json::json!({
        "issuer": url,
        "authorization_endpoint": format!("{}/oauth/authorize", url),
        "token_endpoint": format!("{}/oauth/token", url),
        "registration_endpoint": format!("{}/oauth/register", url),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code"],
        "token_endpoint_auth_methods_supported": ["none"],
        "code_challenge_methods_supported": ["S256"],
        "service_documentation": format!("{}", url),
    }))
}

// --- Dynamic Client Registration (POST /oauth/register) ---

#[derive(Deserialize)]
pub struct RegisterRequest {
    pub client_name: Option<String>,
    pub redirect_uris: Vec<String>,
}

pub async fn register_client(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RegisterRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    if req.redirect_uris.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "redirect_uris required"})),
        );
    }

    let client_id = random_hex(16);
    let client_name = req.client_name.unwrap_or_else(|| "MCP Client".to_string());

    let client = RegisteredClient {
        client_name: client_name.clone(),
        redirect_uris: req.redirect_uris.clone(),
    };

    state.oauth.clients.lock().await.insert(client_id.clone(), client);
    info!(client_id = %client_id, client_name = %client_name, "oauth client registered");

    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "client_id": client_id,
            "client_name": client_name,
            "redirect_uris": req.redirect_uris,
            "token_endpoint_auth_method": "none",
        })),
    )
}

// --- Authorization Endpoint (GET /oauth/authorize) ---

#[derive(Deserialize)]
#[allow(dead_code)]
pub struct AuthorizeQuery {
    pub response_type: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub code_challenge: String,
    pub code_challenge_method: String,
    pub state: Option<String>,
    pub scope: Option<String>,
}

pub async fn authorize_page(
    State(state): State<Arc<AppState>>,
    Query(q): Query<AuthorizeQuery>,
) -> Response {
    if q.response_type != "code" {
        return (
            StatusCode::BAD_REQUEST,
            "unsupported response_type",
        )
            .into_response();
    }
    if q.code_challenge_method != "S256" {
        return (
            StatusCode::BAD_REQUEST,
            "code_challenge_method must be S256",
        )
            .into_response();
    }

    let clients = state.oauth.clients.lock().await;
    let client = match clients.get(&q.client_id) {
        Some(c) => c,
        None => {
            return (StatusCode::BAD_REQUEST, "unknown client_id").into_response();
        }
    };
    if !client.redirect_uris.contains(&q.redirect_uri) {
        return (StatusCode::BAD_REQUEST, "redirect_uri not registered").into_response();
    }
    let client_name = client.client_name.clone();
    drop(clients);

    let html = format!(
        r#"<!DOCTYPE html>
<html>
<head><title>gitideas - Authorize</title>
<style>
  body {{ font-family: system-ui, sans-serif; max-width: 420px; margin: 60px auto; padding: 0 20px; }}
  h1 {{ font-size: 1.3em; }}
  input[type=password] {{ width: 100%; padding: 8px; margin: 8px 0; box-sizing: border-box; }}
  button {{ padding: 10px 24px; margin-top: 8px; cursor: pointer; }}
  .client {{ color: #0066cc; font-weight: bold; }}
</style>
</head>
<body>
<h1>Authorize access to gitideas</h1>
<p><span class="client">{client_name}</span> wants to access your gitideas server.</p>
<form method="POST" action="/oauth/authorize">
  <input type="hidden" name="response_type" value="{response_type}">
  <input type="hidden" name="client_id" value="{client_id}">
  <input type="hidden" name="redirect_uri" value="{redirect_uri}">
  <input type="hidden" name="code_challenge" value="{code_challenge}">
  <input type="hidden" name="code_challenge_method" value="{code_challenge_method}">
  <input type="hidden" name="state" value="{state}">
  <label for="token">Enter the password from your gitideas.ini config file:</label>
  <input type="password" name="token" id="token" required>
  <p style="font-size: 0.85em; color: #666;">This is the <code>token</code> value in <code>~/.config/gitideas.ini</code> on the server.</p>
  <button type="submit">Authorize</button>
</form>
</body>
</html>"#,
        client_name = html_escape(&client_name),
        response_type = html_escape(&q.response_type),
        client_id = html_escape(&q.client_id),
        redirect_uri = html_escape(&q.redirect_uri),
        code_challenge = html_escape(&q.code_challenge),
        code_challenge_method = html_escape(&q.code_challenge_method),
        state = html_escape(&q.state.unwrap_or_default()),
    );

    Html(html).into_response()
}

// --- Authorization Endpoint (POST /oauth/authorize) ---

#[derive(Deserialize)]
#[allow(dead_code)]
pub struct AuthorizeForm {
    pub response_type: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub code_challenge: String,
    pub code_challenge_method: String,
    pub state: Option<String>,
    pub token: String,
}

pub async fn authorize_submit(
    State(state): State<Arc<AppState>>,
    axum::Form(form): axum::Form<AuthorizeForm>,
) -> Response {
    // Verify the token matches the configured one
    if form.token != state.auth_token {
        warn!(client_id = %form.client_id, "oauth authorization failed: bad token");
        return Html(
            r#"<!DOCTYPE html><html><body>
            <h1>Authorization Failed</h1>
            <p>Wrong password. Enter the <code>token</code> value from <code>~/.config/gitideas.ini</code> on the server.</p>
            <p><a href="javascript:history.back()">Go back and try again</a></p>
            </body></html>"#,
        )
        .into_response();
    }

    // Verify client exists
    let clients = state.oauth.clients.lock().await;
    if !clients.contains_key(&form.client_id) {
        return (StatusCode::BAD_REQUEST, "unknown client_id").into_response();
    }
    drop(clients);

    // Generate authorization code
    let code = random_hex(32);
    info!(client_id = %form.client_id, "oauth authorization granted");

    state.oauth.auth_codes.lock().await.insert(
        code.clone(),
        AuthCode {
            client_id: form.client_id,
            redirect_uri: form.redirect_uri.clone(),
            code_challenge: form.code_challenge,
            created: Instant::now(),
        },
    );

    // Redirect back to client
    let mut redirect_url = form.redirect_uri;
    redirect_url.push_str(if redirect_url.contains('?') { "&" } else { "?" });
    redirect_url.push_str(&format!("code={}", code));
    if let Some(st) = form.state {
        if !st.is_empty() {
            redirect_url.push_str(&format!("&state={}", st));
        }
    }

    Redirect::to(&redirect_url).into_response()
}

// --- Token Endpoint (POST /oauth/token) ---

#[derive(Deserialize)]
pub struct TokenRequest {
    pub grant_type: String,
    pub code: String,
    pub redirect_uri: String,
    pub client_id: String,
    pub code_verifier: String,
}

#[derive(Serialize)]
struct TokenResponse {
    access_token: String,
    token_type: String,
    expires_in: u64,
}

pub async fn token_exchange(
    State(state): State<Arc<AppState>>,
    axum::Form(req): axum::Form<TokenRequest>,
) -> Response {
    if req.grant_type != "authorization_code" {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "unsupported_grant_type"})),
        )
            .into_response();
    }

    // Look up and remove the authorization code (single use)
    let auth_code = state.oauth.auth_codes.lock().await.remove(&req.code);
    let auth_code = match auth_code {
        Some(c) => c,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "invalid_grant", "error_description": "invalid or expired code"})),
            )
                .into_response();
        }
    };

    // Verify code hasn't expired (5 minutes)
    if auth_code.created.elapsed() > Duration::from_secs(300) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "invalid_grant", "error_description": "code expired"})),
        )
            .into_response();
    }

    // Verify client_id and redirect_uri match
    if auth_code.client_id != req.client_id || auth_code.redirect_uri != req.redirect_uri {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "invalid_grant"})),
        )
            .into_response();
    }

    // PKCE: verify code_verifier against stored code_challenge
    let mut hasher = Sha256::new();
    hasher.update(req.code_verifier.as_bytes());
    let computed_challenge = URL_SAFE_NO_PAD.encode(hasher.finalize());

    if computed_challenge != auth_code.code_challenge {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "invalid_grant", "error_description": "PKCE verification failed"})),
        )
            .into_response();
    }

    // Issue access token
    let access_token = random_hex(32);
    let expires_in = Duration::from_secs(3600);

    state.oauth.access_tokens.lock().await.insert(
        access_token.clone(),
        TokenInfo {
            created: Instant::now(),
            expires_in,
        },
    );

    info!(client_id = %req.client_id, "oauth token issued");

    Json(TokenResponse {
        access_token,
        token_type: "Bearer".to_string(),
        expires_in: expires_in.as_secs(),
    })
    .into_response()
}

// --- Token Validation (used by MCP middleware) ---

pub async fn validate_oauth_token(state: &AppState, token: &str) -> bool {
    let tokens = state.oauth.access_tokens.lock().await;
    if let Some(info) = tokens.get(token) {
        return info.created.elapsed() < info.expires_in;
    }
    false
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
