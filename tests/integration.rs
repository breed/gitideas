use std::net::TcpListener;
use std::process::{Child, Command};
use std::time::Duration;
use tempfile::TempDir;

struct TestServer {
    child: Child,
    port: u16,
    token: String,
    _dir: TempDir,
    _home_dir: TempDir,
}

impl TestServer {
    fn start() -> Self {
        let dir = TempDir::new().unwrap();

        // Initialize a git repo in the temp dir
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

        // Bind to port 0 to get an OS-assigned free port, then release it
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let token = "test-token-12345".to_string();

        // Create a fake home dir with config file
        let home_dir = TempDir::new().unwrap();
        let config_dir = home_dir.path().join(".config");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            config_dir.join("gitideas.ini"),
            format!(
                "port = {}\ntoken = {}\nrepo = {}\n",
                port,
                token,
                dir.path().display()
            ),
        )
        .unwrap();

        let binary = env!("CARGO_BIN_EXE_gitideas");
        let child = Command::new(binary)
            .env("HOME", home_dir.path())
            .spawn()
            .expect("failed to start server");

        // Give the server a moment to start
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

#[tokio::test]
async fn test_auth_required() {
    let server = TestServer::start();
    let client = reqwest::Client::new();

    // No token
    let resp = client
        .post(&server.url("/add"))
        .json(&serde_json::json!({"type": "IDEA", "subject": "test", "text": "body"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);

    // Wrong token
    let resp = client
        .post(&server.url("/add"))
        .bearer_auth("wrong-token")
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

    // Add an entry
    let resp = client
        .post(&server.url("/add"))
        .bearer_auth(&server.token)
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
    assert!(body["file"].as_str().unwrap().starts_with("IDEA."));

    // Search for it
    let resp = client
        .post(&server.url("/search"))
        .bearer_auth(&server.token)
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

    // Add two entries
    client
        .post(&server.url("/add"))
        .bearer_auth(&server.token)
        .json(&serde_json::json!({"type": "IDEA", "subject": "Alpha idea", "text": "body one"}))
        .send()
        .await
        .unwrap();
    client
        .post(&server.url("/add"))
        .bearer_auth(&server.token)
        .json(&serde_json::json!({"type": "TODO", "subject": "Beta todo", "text": "body two"}))
        .send()
        .await
        .unwrap();

    // Search by subject
    let resp = client
        .post(&server.url("/search"))
        .bearer_auth(&server.token)
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

    client
        .post(&server.url("/add"))
        .bearer_auth(&server.token)
        .json(&serde_json::json!({"type": "IDEA", "subject": "Idea one", "text": "contains unique_keyword here"}))
        .send()
        .await
        .unwrap();
    client
        .post(&server.url("/add"))
        .bearer_auth(&server.token)
        .json(&serde_json::json!({"type": "IDEA", "subject": "Idea two", "text": "nothing special"}))
        .send()
        .await
        .unwrap();

    let resp = client
        .post(&server.url("/search"))
        .bearer_auth(&server.token)
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

    // Subject too long
    let long_subject = "x".repeat(121);
    let resp = client
        .post(&server.url("/add"))
        .bearer_auth(&server.token)
        .json(&serde_json::json!({"type": "IDEA", "subject": long_subject, "text": "body"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    // Invalid type
    let resp = client
        .post(&server.url("/add"))
        .bearer_auth(&server.token)
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

    for t in ["IDEA", "TODO", "MEMORY"] {
        client
            .post(&server.url("/add"))
            .bearer_auth(&server.token)
            .json(&serde_json::json!({"type": t, "subject": format!("A {}", t), "text": "body"}))
            .send()
            .await
            .unwrap();
    }

    // Search by type
    let resp = client
        .post(&server.url("/search"))
        .bearer_auth(&server.token)
        .json(&serde_json::json!({"type": "TODO"}))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let entries = body["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["type"], "TODO");
}

// --- MCP + OAuth Tests ---

#[tokio::test]
async fn test_mcp_requires_auth() {
    let server = TestServer::start();
    let client = reqwest::Client::new();

    // No auth → 401 with WWW-Authenticate header
    let resp = client
        .post(&server.url("/mcp"))
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "1.0"}
            }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
    let www_auth = resp.headers().get("www-authenticate").unwrap().to_str().unwrap();
    assert!(www_auth.contains("oauth-protected-resource"));
}

#[tokio::test]
async fn test_mcp_with_bearer_token() {
    let server = TestServer::start();
    let client = reqwest::Client::new();

    // Direct bearer token should work (backward compat)
    let resp = client
        .post(&server.url("/mcp"))
        .bearer_auth(&server.token)
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "1.0"}
            }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["result"]["serverInfo"]["name"], "gitideas");
    assert!(body["result"]["capabilities"]["tools"].is_object());
}

#[tokio::test]
async fn test_mcp_tools_list() {
    let server = TestServer::start();
    let client = reqwest::Client::new();

    let resp = client
        .post(&server.url("/mcp"))
        .bearer_auth(&server.token)
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }))
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

    // Add via MCP
    let resp = client
        .post(&server.url("/mcp"))
        .bearer_auth(&server.token)
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "add",
                "arguments": {
                    "type": "IDEA",
                    "subject": "MCP test idea",
                    "text": "Added via MCP protocol"
                }
            }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let text = body["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("MCP test idea"));

    // Search via MCP
    let resp = client
        .post(&server.url("/mcp"))
        .bearer_auth(&server.token)
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "search",
                "arguments": {
                    "subject": "MCP test"
                }
            }
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

    // 1. Discover protected resource metadata
    let resp = client
        .get(&server.url("/.well-known/oauth-protected-resource"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let meta: serde_json::Value = resp.json().await.unwrap();
    assert!(meta["authorization_servers"][0].as_str().is_some());

    // 2. Discover authorization server metadata
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

    // 3. Register client
    let resp = client
        .post(&server.url("/oauth/register"))
        .json(&serde_json::json!({
            "client_name": "Test MCP Client",
            "redirect_uris": ["http://localhost:9999/callback"]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let reg: serde_json::Value = resp.json().await.unwrap();
    let client_id = reg["client_id"].as_str().unwrap().to_string();

    // 4. PKCE: generate code_verifier and code_challenge
    let code_verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(code_verifier.as_bytes());
    let code_challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(hasher.finalize());

    // 5. Submit authorization (POST directly, simulating form submit)
    let resp = client
        .post(&server.url("/oauth/authorize"))
        .form(&[
            ("response_type", "code"),
            ("client_id", &client_id),
            ("redirect_uri", "http://localhost:9999/callback"),
            ("code_challenge", &code_challenge),
            ("code_challenge_method", "S256"),
            ("state", "test-state"),
            ("token", &server.token),
        ])
        .send()
        .await
        .unwrap();
    // Should redirect with code
    assert_eq!(resp.status(), 303);
    let location = resp.headers().get("location").unwrap().to_str().unwrap();
    assert!(location.starts_with("http://localhost:9999/callback?code="));
    assert!(location.contains("state=test-state"));

    // Extract code from redirect URL
    let url = reqwest::Url::parse(location).unwrap();
    let code = url.query_pairs().find(|(k, _)| k == "code").unwrap().1.to_string();

    // 6. Exchange code for token
    let resp = client
        .post(&server.url("/oauth/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", "http://localhost:9999/callback"),
            ("client_id", &client_id),
            ("code_verifier", code_verifier),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let token_resp: serde_json::Value = resp.json().await.unwrap();
    let access_token = token_resp["access_token"].as_str().unwrap();
    assert_eq!(token_resp["token_type"], "Bearer");

    // 7. Use access token with MCP
    let resp = client
        .post(&server.url("/mcp"))
        .bearer_auth(access_token)
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "oauth-test", "version": "1.0"}
            }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["result"]["serverInfo"]["name"], "gitideas");
}

use base64::Engine;
