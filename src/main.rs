mod api;
mod entry;
mod git;
mod mcp;
mod oauth;
mod search;
mod storage;
mod types;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use tower_http::trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer};
use tracing::info;

use api::AppState;

fn parse_ini(content: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            map.insert(key.trim().to_string(), value.trim().to_string());
        }
    }
    map
}

async fn oauth_auth_middleware(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Response {
    let token = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    match token {
        Some(t) if oauth::validate_oauth_token(&state, t).await => next.run(req).await,
        _ => {
            let url = &state.oauth.server_url;
            (
                StatusCode::UNAUTHORIZED,
                [(
                    "WWW-Authenticate",
                    format!(
                        "Bearer resource_metadata=\"{}/.well-known/oauth-protected-resource\"",
                        url
                    ),
                )],
                "",
            )
                .into_response()
        }
    }
}

#[tokio::main]
async fn main() {
    // Initialize tracing. RUST_LOG controls verbosity:
    //   RUST_LOG=info (default), RUST_LOG=debug, RUST_LOG=gitideas=debug,tower_http=debug
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,tower_http=info".parse().unwrap()),
        )
        .init();

    let config_path = dirs::home_dir()
        .expect("could not determine home directory")
        .join(".config/gitideas.ini");

    let content = std::fs::read_to_string(&config_path).unwrap_or_else(|e| {
        tracing::error!("could not read {}: {}", config_path.display(), e);
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

    let repo_path = PathBuf::from(config.get("repo").expect("config missing 'repo'"));

    let host = config
        .get("host")
        .map(|s| s.as_str())
        .unwrap_or("127.0.0.1");

    let server_url = config
        .get("url")
        .cloned()
        .unwrap_or_else(|| format!("http://{}:{}", host, port));

    // Verify the repo path is a git repository
    let git_check = std::process::Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(&repo_path)
        .output();
    match git_check {
        Ok(output) if output.status.success() => {}
        _ => {
            tracing::error!("{} is not a git repository", repo_path.display());
            std::process::exit(1);
        }
    }

    info!(repo = %repo_path.display(), url = %server_url, "configuration loaded");

    let state = Arc::new(AppState {
        git_lock: tokio::sync::Mutex::new(()),
        auth_token: token,
        repo_path,
        oauth: oauth::OAuthState::new(server_url),
    });

    // REST API and MCP routes — all use OAuth auth
    let authed_routes = Router::new()
        .route("/add", post(api::add_handler))
        .route("/search", post(api::search_handler))
        .route("/mcp", post(mcp::mcp_handler).get(mcp::mcp_sse_handler).delete(mcp::mcp_delete_handler))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            oauth_auth_middleware,
        ));

    // OAuth routes (no auth required — they ARE the auth)
    let oauth_routes = Router::new()
        .route(
            "/.well-known/oauth-protected-resource",
            get(oauth::protected_resource_metadata),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(oauth::authorization_server_metadata),
        )
        .route("/oauth/register", post(oauth::register_client))
        .route(
            "/oauth/authorize",
            get(oauth::authorize_page).post(oauth::authorize_submit),
        )
        .route("/oauth/token", post(oauth::token_exchange));

    let app = Router::new()
        .merge(authed_routes)
        .merge(oauth_routes)
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().level(tracing::Level::INFO))
                .on_response(DefaultOnResponse::new().level(tracing::Level::INFO)),
        )
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port))
        .await
        .expect("failed to bind to port");

    info!(port, "gitideas listening");
    axum::serve(listener, app).await.expect("server error");
}
