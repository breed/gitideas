use std::collections::HashMap;
use std::process;

fn parse_ini(content: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') || line.starts_with('[')
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
    token: String,
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

    let port = config
        .get("port")
        .expect("config missing 'port'");
    let token = config
        .get("token")
        .expect("config missing 'token'")
        .to_string();
    let host = config
        .get("host")
        .map(|s| s.as_str())
        .unwrap_or("127.0.0.1");

    Config {
        base_url: format!("http://{}:{}", host, port),
        token,
    }
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

    let resp = client
        .post(format!("{}/add", config.base_url))
        .bearer_auth(&config.token)
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
                // Bare argument — treat as text search for convenience
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

    let resp = client
        .post(format!("{}/search", config.base_url))
        .bearer_auth(&config.token)
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

        println!("\x1b[1;36m[{}]\x1b[0m \x1b[1;33m{}\x1b[0m \x1b[1m{}\x1b[0m", date, etype, subj);
        if !text.is_empty() {
            for line in text.lines() {
                println!("  {}", line);
            }
        }
        println!();
    }

    println!("{} result(s)", entries.len());
}
