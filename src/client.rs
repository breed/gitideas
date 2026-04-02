use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::process;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::Rng;
use sha2::{Digest, Sha256};

fn parse_ini(content: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty()
            || line.starts_with('#')
            || line.starts_with(';')
            || line.starts_with('[')
        {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            map.insert(key.trim().to_string(), value.trim().to_string());
        }
    }
    map
}

struct Config {
    base_url: String,
}

fn load_config() -> Config {
    let config_path = dirs::home_dir()
        .expect("could not determine home directory")
        .join(".config/gitideas.ini");

    let content = std::fs::read_to_string(&config_path).unwrap_or_else(|e| {
        eprintln!("error: could not read {}: {}", config_path.display(), e);
        process::exit(1);
    });

    let config = parse_ini(&content);

    let port = config.get("port").expect("config missing 'port'");
    let host = config
        .get("host")
        .map(|s| s.as_str())
        .unwrap_or("127.0.0.1");

    Config {
        base_url: format!("http://{}:{}", host, port),
    }
}

fn token_cache_path() -> std::path::PathBuf {
    dirs::home_dir()
        .expect("could not determine home directory")
        .join(".config/gitideas-oauth-token")
}

fn load_cached_token() -> Option<String> {
    std::fs::read_to_string(token_cache_path())
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn save_cached_token(token: &str) {
    let path = token_cache_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, token);
    // Restrict permissions on the token file
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
}

async fn get_access_token(config: &Config, client: &reqwest::Client) -> String {
    // Try cached token first
    if let Some(token) = load_cached_token() {
        // Verify it still works
        let resp = client
            .post(format!("{}/search", config.base_url))
            .bearer_auth(&token)
            .json(&serde_json::json!({"before": "1970-01-01"}))
            .send()
            .await;
        if let Ok(r) = resp {
            if r.status() != 401 {
                return token;
            }
        }
    }

    eprintln!("Authenticating with gitideas server...");

    // 1. Generate PKCE
    let mut verifier_bytes = [0u8; 32];
    rand::rng().fill(&mut verifier_bytes);
    let code_verifier = URL_SAFE_NO_PAD.encode(verifier_bytes);
    let code_challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(code_verifier.as_bytes()));

    // 3. Start local callback server
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let callback_port = listener.local_addr().unwrap().port();
    let redirect_uri = format!("http://127.0.0.1:{}/callback", callback_port);

    // 2. Register client with the actual redirect URI
    let resp = client
        .post(format!("{}/oauth/register", config.base_url))
        .json(&serde_json::json!({
            "client_name": "gitideas-client",
            "redirect_uris": [&redirect_uri]
        }))
        .send()
        .await
        .unwrap();
    let reg: serde_json::Value = resp.json().await.unwrap();
    let client_id = reg["client_id"].as_str().unwrap().to_string();

    // 4. Build authorize URL and open browser
    let auth_url = format!(
        "{}/oauth/authorize?response_type=code&client_id={}&redirect_uri={}&code_challenge={}&code_challenge_method=S256&state=cli",
        config.base_url,
        urlencoded(&client_id),
        urlencoded(&redirect_uri),
        urlencoded(&code_challenge),
    );

    eprintln!("Opening browser for authorization...");
    eprintln!("If the browser doesn't open, visit: {}", auth_url);
    let _ = open_browser(&auth_url);

    // 5. Wait for callback
    let (stream, _) = listener.accept().unwrap();
    let mut reader = BufReader::new(&stream);
    let mut request_line = String::new();
    reader.read_line(&mut request_line).unwrap();

    // Parse the code from the callback URL: GET /callback?code=...&state=... HTTP/1.1
    let path = request_line.split_whitespace().nth(1).unwrap_or("");
    let query = path.split_once('?').map(|(_, q)| q).unwrap_or("");
    let params: HashMap<String, String> = query
        .split('&')
        .filter_map(|p| p.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();

    let code = params.get("code").cloned().unwrap_or_default();

    // Send a nice response to the browser
    let html = "<html><body><h2>Authorization successful!</h2><p>You can close this tab.</p></body></html>";
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        html.len(),
        html
    );
    let mut writer = stream;
    let _ = writer.write_all(response.as_bytes());
    let _ = writer.flush();

    if code.is_empty() {
        eprintln!("error: no authorization code received");
        process::exit(1);
    }

    // 6. Exchange code for token
    let resp = client
        .post(format!("{}/oauth/token", config.base_url))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", &redirect_uri),
            ("client_id", &client_id),
            ("code_verifier", &code_verifier),
        ])
        .send()
        .await
        .unwrap_or_else(|e| {
            eprintln!("error: token exchange failed: {}", e);
            process::exit(1);
        });

    if !resp.status().is_success() {
        let body: serde_json::Value = resp.json().await.unwrap_or_default();
        eprintln!(
            "error: token exchange failed: {}",
            body["error_description"]
                .as_str()
                .or(body["error"].as_str())
                .unwrap_or("unknown error")
        );
        process::exit(1);
    }

    let token_resp: serde_json::Value = resp.json().await.unwrap();
    let access_token = token_resp["access_token"]
        .as_str()
        .unwrap()
        .to_string();

    save_cached_token(&access_token);
    eprintln!("Authenticated successfully.");
    access_token
}

fn urlencoded(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{:02X}", b),
        })
        .collect()
}

fn open_browser(url: &str) -> Result<(), std::io::Error> {
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open").arg(url).spawn()?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).spawn()?;
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", url])
            .spawn()?;
    }
    Ok(())
}

fn usage() -> ! {
    eprintln!("Usage:");
    eprintln!("  gitideas-client add <type> <subject>");
    eprintln!("    type: IDEA, TODO, or MEMORY");
    eprintln!("    reads body text from stdin");
    eprintln!();
    eprintln!("  gitideas-client search [options]");
    eprintln!("    --subject <query>    search subjects");
    eprintln!("    --text <query>       search subjects and body text");
    eprintln!("    --type <type>        filter by type (IDEA, TODO, MEMORY)");
    eprintln!("    --after <date>       entries after date (YYYY-MM-DD or YYYY-MM-DD-hh:mm)");
    eprintln!("    --before <date>      entries before date");
    process::exit(1);
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        usage();
    }

    match args[1].as_str() {
        "add" => cmd_add(&args[2..]).await,
        "search" => cmd_search(&args[2..]).await,
        _ => usage(),
    }
}

async fn cmd_add(args: &[String]) {
    if args.len() < 2 {
        eprintln!("error: add requires <type> and <subject>");
        eprintln!("Usage: gitideas-client add <type> <subject>");
        process::exit(1);
    }

    let idea_type = &args[0];
    match idea_type.as_str() {
        "IDEA" | "TODO" | "MEMORY" => {}
        _ => {
            eprintln!("error: type must be IDEA, TODO, or MEMORY");
            process::exit(1);
        }
    }

    let subject = &args[1];

    // Read body from stdin
    let mut text = String::new();
    if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        eprintln!("Enter body text (Ctrl+D to finish):");
    }
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut text).unwrap_or_else(|e| {
        eprintln!("error reading stdin: {}", e);
        process::exit(1);
    });
    let text = text.trim_end().to_string();

    if text.is_empty() {
        eprintln!("error: body text is empty");
        process::exit(1);
    }

    let config = load_config();
    let client = reqwest::Client::new();
    let token = get_access_token(&config, &client).await;

    let resp = client
        .post(format!("{}/add", config.base_url))
        .bearer_auth(&token)
        .json(&serde_json::json!({
            "type": idea_type,
            "subject": subject,
            "text": text,
        }))
        .send()
        .await
        .unwrap_or_else(|e| {
            eprintln!("error: could not connect to server: {}", e);
            process::exit(1);
        });

    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap_or_else(|e| {
        eprintln!("error: invalid response: {}", e);
        process::exit(1);
    });

    if status.is_success() {
        println!(
            "Added to {} ({})",
            body["file"].as_str().unwrap_or("?"),
            body["date"].as_str().unwrap_or("?")
        );
    } else {
        eprintln!(
            "error: {}",
            body["error"].as_str().unwrap_or("unknown error")
        );
        process::exit(1);
    }
}

async fn cmd_search(args: &[String]) {
    let mut subject: Option<&str> = None;
    let mut text: Option<&str> = None;
    let mut idea_type: Option<&str> = None;
    let mut after: Option<&str> = None;
    let mut before: Option<&str> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--subject" => {
                i += 1;
                subject = Some(args.get(i).map(|s| s.as_str()).unwrap_or_else(|| {
                    eprintln!("error: --subject requires a value");
                    process::exit(1);
                }));
            }
            "--text" => {
                i += 1;
                text = Some(args.get(i).map(|s| s.as_str()).unwrap_or_else(|| {
                    eprintln!("error: --text requires a value");
                    process::exit(1);
                }));
            }
            "--type" => {
                i += 1;
                idea_type = Some(args.get(i).map(|s| s.as_str()).unwrap_or_else(|| {
                    eprintln!("error: --type requires a value");
                    process::exit(1);
                }));
            }
            "--after" => {
                i += 1;
                after = Some(args.get(i).map(|s| s.as_str()).unwrap_or_else(|| {
                    eprintln!("error: --after requires a value");
                    process::exit(1);
                }));
            }
            "--before" => {
                i += 1;
                before = Some(args.get(i).map(|s| s.as_str()).unwrap_or_else(|| {
                    eprintln!("error: --before requires a value");
                    process::exit(1);
                }));
            }
            other => {
                text = Some(other);
            }
        }
        i += 1;
    }

    let mut body = serde_json::Map::new();
    if let Some(v) = subject {
        body.insert("subject".into(), serde_json::Value::String(v.into()));
    }
    if let Some(v) = text {
        body.insert("text".into(), serde_json::Value::String(v.into()));
    }
    if let Some(v) = idea_type {
        body.insert("type".into(), serde_json::Value::String(v.into()));
    }
    if let Some(v) = after {
        body.insert("after".into(), serde_json::Value::String(v.into()));
    }
    if let Some(v) = before {
        body.insert("before".into(), serde_json::Value::String(v.into()));
    }

    let config = load_config();
    let client = reqwest::Client::new();
    let token = get_access_token(&config, &client).await;

    let resp = client
        .post(format!("{}/search", config.base_url))
        .bearer_auth(&token)
        .json(&serde_json::Value::Object(body))
        .send()
        .await
        .unwrap_or_else(|e| {
            eprintln!("error: could not connect to server: {}", e);
            process::exit(1);
        });

    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap_or_else(|e| {
        eprintln!("error: invalid response: {}", e);
        process::exit(1);
    });

    if !status.is_success() {
        eprintln!(
            "error: {}",
            body["error"].as_str().unwrap_or("unknown error")
        );
        process::exit(1);
    }

    let entries = match body["entries"].as_array() {
        Some(e) => e,
        None => {
            println!("No entries found.");
            return;
        }
    };

    if entries.is_empty() {
        println!("No entries found.");
        return;
    }

    for entry in entries {
        let date = entry["date"].as_str().unwrap_or("?");
        let etype = entry["type"].as_str().unwrap_or("?");
        let subj = entry["subject"].as_str().unwrap_or("?");
        let text = entry["text"].as_str().unwrap_or("");

        println!(
            "\x1b[1;36m[{}]\x1b[0m \x1b[1;33m{}\x1b[0m \x1b[1m{}\x1b[0m",
            date, etype, subj
        );
        if !text.is_empty() {
            for line in text.lines() {
                println!("  {}", line);
            }
        }
        println!();
    }

    println!("{} result(s)", entries.len());
}
