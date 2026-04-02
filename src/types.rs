use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum IdeaType {
    Todo,
    Idea,
    Memory,
}

impl fmt::Display for IdeaType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IdeaType::Todo => write!(f, "TODO"),
            IdeaType::Idea => write!(f, "IDEA"),
            IdeaType::Memory => write!(f, "MEMORY"),
        }
    }
}

impl IdeaType {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "TODO" => Some(IdeaType::Todo),
            "IDEA" => Some(IdeaType::Idea),
            "MEMORY" => Some(IdeaType::Memory),
            _ => None,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct AddRequest {
    #[serde(rename = "type")]
    pub idea_type: IdeaType,
    pub subject: String,
    pub text: String,
    pub id: Option<String>,
    pub due: Option<String>,
    pub complete: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct SearchRequest {
    pub subject: Option<String>,
    pub text: Option<String>,
    pub after: Option<String>,
    pub before: Option<String>,
    #[serde(rename = "type")]
    pub idea_type: Option<IdeaType>,
    pub id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AddResponse {
    pub ok: bool,
    pub id: String,
    pub file: String,
    pub date: String,
}

#[derive(Debug, Serialize)]
pub struct EntryResponse {
    pub id: String,
    #[serde(rename = "type")]
    pub idea_type: IdeaType,
    pub date: String,
    pub subject: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub complete: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub entries: Vec<EntryResponse>,
}

#[derive(Debug)]
pub enum AppError {
    InvalidSubject(String),
    BodyTooLarge,
    GitError(String),
    ConflictRetryExhausted,
    IoError(std::io::Error),
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppError::InvalidSubject(msg) => write!(f, "{}", msg),
            AppError::BodyTooLarge => write!(f, "body exceeds 1MB"),
            AppError::GitError(msg) => write!(f, "git error: {}", msg),
            AppError::ConflictRetryExhausted => {
                write!(f, "conflict retry exhausted after 5 attempts")
            }
            AppError::IoError(e) => write!(f, "io error: {}", e),
        }
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::IoError(e)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = match &self {
            AppError::InvalidSubject(_) | AppError::BodyTooLarge => StatusCode::BAD_REQUEST,
            AppError::ConflictRetryExhausted => StatusCode::CONFLICT,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let body = serde_json::json!({
            "ok": false,
            "error": self.to_string(),
        });
        (status, axum::Json(body)).into_response()
    }
}
