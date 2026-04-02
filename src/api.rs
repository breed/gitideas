use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use chrono::Utc;

use tracing::{debug, info};

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
    let id = req.id.unwrap_or_else(crate::entry::generate_id);

    // Acquire lock — only one ADD at a time since git ops aren't concurrent-safe
    let _guard = state.git_lock.lock().await;

    let (file, date, id) = crate::git::add_with_retry(
        &state.repo_path,
        req.idea_type,
        &id,
        &req.subject,
        &req.text,
        req.due.as_deref(),
        req.complete.as_deref(),
        &now,
    )
    .await?;

    info!(id = %id, subject = %req.subject, r#type = %req.idea_type, "add");

    Ok(Json(AddResponse {
        ok: true,
        id,
        file,
        date,
    }))
}

pub async fn search_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SearchRequest>,
) -> Result<Json<SearchResponse>, AppError> {
    debug!(?req, "search");

    let repo_path = state.repo_path.clone();
    let result = tokio::task::spawn_blocking(move || search::search(&repo_path, &req))
        .await
        .map_err(|e| AppError::GitError(format!("search task failed: {}", e)))??;

    info!(results = result.entries.len(), "search");

    Ok(Json(result))
}
