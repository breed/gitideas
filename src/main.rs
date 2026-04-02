mod api;
mod entry;
mod git;
mod search;
mod storage;
mod types;

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::post;
use axum::Router;

use api::AppState;

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
    let port: u16 = std::env::args()
        .nth(1)
        .expect("Usage: gitideas HTTP_PORT")
        .parse()
        .expect("PORT must be a valid number");

    let token =
        std::env::var("GITIDEAS_TOKEN").expect("GITIDEAS_TOKEN environment variable must be set");

    // Verify we're in a git repo
    let git_check = std::process::Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .output();
    match git_check {
        Ok(output) if output.status.success() => {}
        _ => {
            eprintln!("error: current directory is not a git repository");
            std::process::exit(1);
        }
    }

    let state = Arc::new(AppState {
        git_lock: tokio::sync::Mutex::new(()),
        auth_token: token,
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
