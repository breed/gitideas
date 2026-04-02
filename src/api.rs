use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use chrono::Utc;

use crate::search;
use crate::types::{AddRequest, AddResponse, AppError, SearchRequest, SearchResponse};

pub struct AppState {
    pub git_lock: tokio::sync::Mutex<()>,
    pub auth_token: String,
    pub repo_path: PathBuf,
    pub oauth: crate::oauth::OAuthState,
}

pub async fn add_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AddRequest>,
) -> Result<Json<AddResponse>, AppError> {
    let now = Utc::now().format("%Y-%m-%d-%H:%M").to_string();

    // Acquire lock — only one ADD at a time since git ops aren't concurrent-safe
    let _guard = state.git_lock.lock().await;

    let (file, date) =
        crate::git::add_with_retry(&state.repo_path, req.idea_type, &req.subject, &req.text, &now)
            .await?;

    Ok(Json(AddResponse {
        ok: true,
        file,
        date,
    }))
}

pub async fn search_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SearchRequest>,
) -> Result<Json<SearchResponse>, AppError> {
    let repo_path = state.repo_path.clone();
    // Search runs without the git lock — reads are safe concurrently
    // Run blocking file I/O on a blocking thread
    let result = tokio::task::spawn_blocking(move || search::search(&repo_path, &req))
        .await
        .map_err(|e| AppError::GitError(format!("search task failed: {}", e)))??;

    Ok(Json(result))
}
