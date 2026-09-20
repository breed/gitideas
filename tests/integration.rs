use std::net::TcpListener;
use std::process::{Child, Command};
use std::time::Duration;
use tempfile::TempDir;

use base64::Engine;

struct TestServer {
    child: Child,
    port: u16,
    token: String, // the configured password for OAuth authorize
    _dir: TempDir,
    _home_dir: TempDir,
}

impl TestServer {
    fn start() -> Self {
        Self::start_with(&[], "")
    }

    /// Start with extra config lines and extra environment variables.
    fn start_with(env: &[(&str, &str)], extra_config: &str) -> Self {
        let dir = TempDir::new().unwrap();

        Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "--allow-empty", "-m", "init"])
            .current_dir(dir.path())
            .output()
            .unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let token = "test-token-12345".to_string();

        let home_dir = TempDir::new().unwrap();
        let config_dir = home_dir.path().join(".config");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            config_dir.join("gitideas.ini"),
            format!(
                "port = {}\ntoken = {}\nrepo = {}\n{}",
                port,
                token,
                dir.path().display(),
                extra_config
            ),
        )
        .unwrap();

        let binary = env!("CARGO_BIN_EXE_gitideas");
        let child = Command::new(binary)
            .env("HOME", home_dir.path())
            .envs(env.iter().copied())
            .spawn()
            .expect("failed to start server");

        std::thread::sleep(Duration::from_millis(500));

        TestServer {
            child,
            port,
            token,
            _dir: dir,
            _home_dir: home_dir,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{}", self.port, path)
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Perform the full OAuth flow programmatically and return an access token.
async fn get_oauth_token(server: &TestServer, client: &reqwest::Client) -> String {
    let no_redirect = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    // Register
    let resp = client
        .post(&server.url("/oauth/register"))
        .json(&serde_json::json!({
            "client_name": "test",
            "redirect_uris": ["http://localhost:9999/callback"]
        }))
        .send()
        .await
        .unwrap();
    let reg: serde_json::Value = resp.json().await.unwrap();
    let client_id = reg["client_id"].as_str().unwrap().to_string();

    // PKCE
    let code_verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    let code_challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(sha2::Sha256::digest(code_verifier.as_bytes()));

    // Authorize
    let resp = no_redirect
        .post(&server.url("/oauth/authorize"))
        .form(&[
            ("response_type", "code"),
            ("client_id", client_id.as_str()),
            ("redirect_uri", "http://localhost:9999/callback"),
            ("code_challenge", code_challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("state", "test"),
            ("token", server.token.as_str()),
        ])
        .send()
        .await
        .unwrap();
    let location = resp.headers().get("location").unwrap().to_str().unwrap();
    let url = reqwest::Url::parse(location).unwrap();
    let code = url
        .query_pairs()
        .find(|(k, _)| k == "code")
        .unwrap()
        .1
        .to_string();

    // Exchange
    let resp = client
        .post(&server.url("/oauth/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", "http://localhost:9999/callback"),
            ("client_id", client_id.as_str()),
            ("code_verifier", code_verifier),
        ])
        .send()
        .await
        .unwrap();
    let token_resp: serde_json::Value = resp.json().await.unwrap();
    token_resp["access_token"].as_str().unwrap().to_string()
}

use sha2::Digest;

// --- REST API Tests (now use OAuth) ---

#[tokio::test]
async fn test_auth_required() {
    let server = TestServer::start();
    let client = reqwest::Client::new();

    // No token → 401
    let resp = client
        .post(&server.url("/add"))
        .json(&serde_json::json!({"type": "IDEA", "subject": "test", "text": "body"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);

    // Wrong token → 401
    let resp = client
        .post(&server.url("/add"))
        .bearer_auth("wrong-token")
        .json(&serde_json::json!({"type": "IDEA", "subject": "test", "text": "body"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);

    // Raw configured token no longer works → 401
    let resp = client
        .post(&server.url("/add"))
        .bearer_auth(&server.token)
        .json(&serde_json::json!({"type": "IDEA", "subject": "test", "text": "body"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn test_add_and_search() {
    let server = TestServer::start();
    let client = reqwest::Client::new();
    let token = get_oauth_token(&server, &client).await;

    let resp = client
        .post(&server.url("/add"))
        .bearer_auth(&token)
        .json(&serde_json::json!({
            "type": "IDEA",
            "subject": "My test idea",
            "text": "This is the body of the idea."
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], true);
    assert!(body["file"].as_str().unwrap().starts_with("IDEA/IDEA."));

    let resp = client
        .post(&server.url("/search"))
        .bearer_auth(&token)
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let entries = body["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["subject"], "My test idea");
    assert_eq!(entries[0]["text"], "This is the body of the idea.");
    assert_eq!(entries[0]["type"], "IDEA");
}

#[tokio::test]
async fn test_search_by_subject() {
    let server = TestServer::start();
    let client = reqwest::Client::new();
    let token = get_oauth_token(&server, &client).await;

    client
        .post(&server.url("/add"))
        .bearer_auth(&token)
        .json(&serde_json::json!({"type": "IDEA", "subject": "Alpha idea", "text": "body one"}))
        .send()
        .await
        .unwrap();
    client
        .post(&server.url("/add"))
        .bearer_auth(&token)
        .json(&serde_json::json!({"type": "TODO", "subject": "Beta todo", "text": "body two"}))
        .send()
        .await
        .unwrap();

    let resp = client
        .post(&server.url("/search"))
        .bearer_auth(&token)
        .json(&serde_json::json!({"subject": "alpha"}))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let entries = body["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["subject"], "Alpha idea");
}

#[tokio::test]
async fn test_search_by_text() {
    let server = TestServer::start();
    let client = reqwest::Client::new();
    let token = get_oauth_token(&server, &client).await;

    client
        .post(&server.url("/add"))
        .bearer_auth(&token)
        .json(&serde_json::json!({"type": "IDEA", "subject": "Idea one", "text": "contains unique_keyword here"}))
        .send()
        .await
        .unwrap();
    client
        .post(&server.url("/add"))
        .bearer_auth(&token)
        .json(&serde_json::json!({"type": "IDEA", "subject": "Idea two", "text": "nothing special"}))
        .send()
        .await
        .unwrap();

    let resp = client
        .post(&server.url("/search"))
        .bearer_auth(&token)
        .json(&serde_json::json!({"text": "unique_keyword"}))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let entries = body["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["subject"], "Idea one");
}

#[tokio::test]
async fn test_validation_errors() {
    let server = TestServer::start();
    let client = reqwest::Client::new();
    let token = get_oauth_token(&server, &client).await;

    let long_subject = "x".repeat(121);
    let resp = client
        .post(&server.url("/add"))
        .bearer_auth(&token)
        .json(&serde_json::json!({"type": "IDEA", "subject": long_subject, "text": "body"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    let resp = client
        .post(&server.url("/add"))
        .bearer_auth(&token)
        .json(&serde_json::json!({"type": "INVALID", "subject": "test", "text": "body"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 422);
}

#[tokio::test]
async fn test_multiple_types() {
    let server = TestServer::start();
    let client = reqwest::Client::new();
    let token = get_oauth_token(&server, &client).await;

    for t in ["IDEA", "TODO", "MEMORY"] {
        client
            .post(&server.url("/add"))
            .bearer_auth(&token)
            .json(&serde_json::json!({"type": t, "subject": format!("A {}", t), "text": "body"}))
            .send()
            .await
            .unwrap();
    }

    let resp = client
        .post(&server.url("/search"))
        .bearer_auth(&token)
        .json(&serde_json::json!({"type": "TODO"}))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let entries = body["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["type"], "TODO");
}

// --- MCP Tests (OAuth only) ---

#[tokio::test]
async fn test_mcp_requires_auth() {
    let server = TestServer::start();
    let client = reqwest::Client::new();

    let resp = client
        .post(&server.url("/mcp"))
        .json(&serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": {"name": "test", "version": "1.0"}}
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
    let www_auth = resp.headers().get("www-authenticate").unwrap().to_str().unwrap();
    assert!(www_auth.contains("oauth-protected-resource"));

    // Raw configured token no longer works
    let resp = client
        .post(&server.url("/mcp"))
        .bearer_auth(&server.token)
        .json(&serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": {"name": "test", "version": "1.0"}}
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn test_mcp_with_oauth_token() {
    let server = TestServer::start();
    let client = reqwest::Client::new();
    let token = get_oauth_token(&server, &client).await;

    let resp = client
        .post(&server.url("/mcp"))
        .bearer_auth(&token)
        .json(&serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": {"name": "test", "version": "1.0"}}
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["result"]["serverInfo"]["name"], "gitideas");
}

#[tokio::test]
async fn test_mcp_tools_list() {
    let server = TestServer::start();
    let client = reqwest::Client::new();
    let token = get_oauth_token(&server, &client).await;

    let resp = client
        .post(&server.url("/mcp"))
        .bearer_auth(&token)
        .json(&serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let tools = body["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 2);
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"add"));
    assert!(names.contains(&"search"));
}

#[tokio::test]
async fn test_mcp_tool_add_and_search() {
    let server = TestServer::start();
    let client = reqwest::Client::new();
    let token = get_oauth_token(&server, &client).await;

    let resp = client
        .post(&server.url("/mcp"))
        .bearer_auth(&token)
        .json(&serde_json::json!({
            "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": {"name": "add", "arguments": {"type": "IDEA", "subject": "MCP test idea", "text": "Added via MCP"}}
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["result"]["content"][0]["text"].as_str().unwrap().contains("MCP test idea"));

    let resp = client
        .post(&server.url("/mcp"))
        .bearer_auth(&token)
        .json(&serde_json::json!({
            "jsonrpc": "2.0", "id": 4, "method": "tools/call",
            "params": {"name": "search", "arguments": {"subject": "MCP test"}}
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let text = body["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("MCP test idea"));
    assert!(text.contains("1 result(s)"));
}

#[tokio::test]
async fn test_oauth_full_flow() {
    let server = TestServer::start();
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    // Discover metadata
    let resp = client
        .get(&server.url("/.well-known/oauth-protected-resource"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let resp = client
        .get(&server.url("/.well-known/oauth-authorization-server"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let auth_meta: serde_json::Value = resp.json().await.unwrap();
    assert!(auth_meta["authorization_endpoint"].as_str().is_some());
    assert!(auth_meta["token_endpoint"].as_str().is_some());
    assert!(auth_meta["registration_endpoint"].as_str().is_some());

    // Register → Authorize → Token exchange → Use
    let token = get_oauth_token(&server, &client).await;

    // Verify the token works on all endpoints
    let resp = client
        .post(&server.url("/mcp"))
        .bearer_auth(&token)
        .json(&serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": {"name": "test", "version": "1.0"}}
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let resp = client
        .post(&server.url("/search"))
        .bearer_auth(&token)
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_robots_txt_disallows_all() {
    let server = TestServer::start();
    let client = reqwest::Client::new();
    let resp = client.get(&server.url("/robots.txt")).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("text/plain"));
    let body = resp.text().await.unwrap();
    assert_eq!(body, "User-agent: *\nDisallow: /\n");
}

#[tokio::test]
async fn test_web_index_without_google_config() {
    let server = TestServer::start();
    let client = reqwest::Client::new();
    let resp = client.get(&server.url("/")).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(body.contains("not configured"));
    assert!(body.contains("noindex"));

    // sign-in endpoints are absent when not configured
    let resp = client.get(&server.url("/auth/google")).send().await.unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn test_web_add_requires_session() {
    let server = TestServer::start();
    let client = reqwest::Client::new();

    // no cookie
    let resp = client
        .post(&server.url("/web/add"))
        .json(&serde_json::json!({"type": "IDEA", "subject": "x", "text": "y"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);

    // forged cookie
    let resp = client
        .post(&server.url("/web/add"))
        .header("cookie", "gitideas_session=bWVAZXhhbXBsZS5jb20:1700000000:00")
        .json(&serde_json::json!({"type": "IDEA", "subject": "x", "text": "y"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);

    // an OAuth bearer token is not accepted as a web session either
    let token = get_oauth_token(&server, &client).await;
    let resp = client
        .post(&server.url("/web/add"))
        .header("cookie", format!("gitideas_session={}", token))
        .json(&serde_json::json!({"type": "IDEA", "subject": "x", "text": "y"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

/// Minimal stand-in for Google's token endpoint: returns an unsigned id_token
/// carrying the given email. Runs until the returned handle is dropped.
async fn mock_google_token_endpoint(email: &str, client_id: &str) -> (String, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/token", listener.local_addr().unwrap());
    let claims = serde_json::json!({
        "iss": "https://accounts.google.com",
        "aud": client_id,
        "exp": chrono::Utc::now().timestamp() + 300,
        "email": email,
        "email_verified": true,
    });
    let id_token = format!(
        "{}.{}.sig",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#),
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims.to_string())
    );
    let body = serde_json::json!({"id_token": id_token, "access_token": "x", "token_type": "Bearer"}).to_string();
    let handle = tokio::spawn(async move {
        loop {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 8192];
            let _ = sock.read(&mut buf).await;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = sock.write_all(resp.as_bytes()).await;
        }
    });
    (url, handle)
}

const GOOGLE_CONFIG: &str = "google_client_id = cid\ngoogle_client_secret = cs\nemail = Owner@Example.com\n";

/// Drive the sign-in flow against the mock and return the callback response.
async fn google_sign_in(server: &TestServer) -> reqwest::Response {
    let no_redirect = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let resp = no_redirect.get(&server.url("/auth/google")).send().await.unwrap();
    assert_eq!(resp.status(), 303);
    let location = resp.headers().get("location").unwrap().to_str().unwrap().to_string();
    assert!(location.starts_with("https://accounts.google.com/o/oauth2/v2/auth?"));
    let state = reqwest::Url::parse(&location)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "state")
        .unwrap()
        .1
        .to_string();
    let state_cookie = resp
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    assert_eq!(state_cookie, format!("gitideas_oauth_state={}", state));

    no_redirect
        .get(&server.url(&format!("/auth/google/callback?code=abc&state={}", state)))
        .header("cookie", state_cookie)
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn test_google_login_allowed_email_can_add() {
    let (token_url, _mock) = mock_google_token_endpoint("owner@example.com", "cid").await;
    let server = TestServer::start_with(&[("GITIDEAS_GOOGLE_TOKEN_URL", token_url.as_str())], GOOGLE_CONFIG);
    let client = reqwest::Client::new();

    // sign-in page is shown when not signed in
    let body = client.get(&server.url("/")).send().await.unwrap().text().await.unwrap();
    assert!(body.contains("Sign in with Google"));

    let resp = google_sign_in(&server).await;
    assert_eq!(resp.status(), 303);
    assert_eq!(resp.headers().get("location").unwrap(), "/");
    let session = resp
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|v| v.to_str().unwrap())
        .find(|v| v.starts_with("gitideas_session="))
        .expect("session cookie")
        .to_string();
    assert!(session.contains("HttpOnly"));
    assert!(session.contains("SameSite=Lax"));
    let cookie = session.split(';').next().unwrap().to_string();

    // dialog is shown with the session
    let body = client
        .get(&server.url("/"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(body.contains("Sign out"));
    assert!(body.contains("<option value=\"MEMORY\">"));

    // and adding works through the web endpoint
    let resp = client
        .post(&server.url("/web/add"))
        .header("cookie", &cookie)
        .json(&serde_json::json!({"type": "MEMORY", "subject": "from web", "text": "dropped"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let j: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(j["ok"], true);
    assert!(j["file"].as_str().unwrap().starts_with("MEMORY/MEMORY."));

    // sign out clears the cookie
    let no_redirect = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let resp = no_redirect
        .post(&server.url("/web/logout"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 303);
    assert!(resp
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .contains("Max-Age=0"));
}

#[tokio::test]
async fn test_google_login_other_email_gets_go_away() {
    let (token_url, _mock) = mock_google_token_endpoint("intruder@example.com", "cid").await;
    let server = TestServer::start_with(&[("GITIDEAS_GOOGLE_TOKEN_URL", token_url.as_str())], GOOGLE_CONFIG);

    let resp = google_sign_in(&server).await;
    assert_eq!(resp.status(), 403);
    assert!(!resp
        .headers()
        .get_all("set-cookie")
        .iter()
        .any(|v| v.to_str().unwrap().starts_with("gitideas_session=")));
    let body = resp.text().await.unwrap();
    assert!(body.contains("go away!"));
}
