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
