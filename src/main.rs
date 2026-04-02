mod api;
mod entry;
mod git;
mod search;
mod storage;
mod types;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::post;
use axum::Router;

use api::AppState;

fn parse_ini(content: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        // Skip section headers like [gitideas]
        if line.starts_with('[') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            map.insert(key.trim().to_string(), value.trim().to_string());
        }
    }
    map
}

async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let auth_header = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok());

    match auth_header {
        Some(value) if value.starts_with("Bearer ") => {
            let token = &value[7..];
            if token == state.auth_token {
                Ok(next.run(req).await)
            } else {
                Err(StatusCode::UNAUTHORIZED)
            }
        }
        _ => Err(StatusCode::UNAUTHORIZED),
    }
}

#[tokio::main]
async fn main() {
    let config_path = dirs::home_dir()
        .expect("could not determine home directory")
        .join(".config/gitideas.ini");

    let content = std::fs::read_to_string(&config_path).unwrap_or_else(|e| {
        eprintln!("error: could not read {}: {}", config_path.display(), e);
        std::process::exit(1);
    });

    let config = parse_ini(&content);

    let port: u16 = config
        .get("port")
        .expect("config missing 'port'")
        .parse()
        .expect("'port' must be a valid number");

    let token = config
        .get("token")
        .expect("config missing 'token'")
        .to_string();

    let repo_path = PathBuf::from(
        config
            .get("repo")
            .expect("config missing 'repo'"),
    );

    // Verify the repo path is a git repository
    let git_check = std::process::Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(&repo_path)
        .output();
    match git_check {
        Ok(output) if output.status.success() => {}
        _ => {
            eprintln!("error: {} is not a git repository", repo_path.display());
            std::process::exit(1);
        }
    }

    let state = Arc::new(AppState {
        git_lock: tokio::sync::Mutex::new(()),
        auth_token: token,
        repo_path,
    });

    let app = Router::new()
        .route("/add", post(api::add_handler))
        .route("/search", post(api::search_handler))
        .layer(middleware::from_fn_with_state(state.clone(), auth_middleware))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port))
        .await
        .expect("failed to bind to port");

    eprintln!("gitideas listening on 0.0.0.0:{}", port);
    axum::serve(listener, app).await.expect("server error");
}
