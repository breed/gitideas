use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::Utc;
use serde_json::{json, Value};

use crate::api::AppState;
use crate::oauth;
use crate::types::IdeaType;

const PROTOCOL_VERSION: &str = "2024-11-05";

pub async fn mcp_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    // Authenticate via OAuth bearer token
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    match token {
        Some(t) if oauth::validate_oauth_token(&state, t).await => {}
        _ => {
            let url = &state.oauth.server_url;
            return (
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
                .into_response();
        }
    }

    // Handle JSON-RPC batch or single request
    if let Some(arr) = body.as_array() {
        // Batch request
        let mut responses = Vec::new();
        for req in arr {
            if let Some(resp) = handle_jsonrpc(&state, req).await {
                responses.push(resp);
            }
        }
        if responses.is_empty() {
            return StatusCode::NO_CONTENT.into_response();
        }
        return Json(Value::Array(responses)).into_response();
    }

    // Single request
    match handle_jsonrpc(&state, &body).await {
        Some(resp) => {
            let mut response = Json(resp).into_response();
            // Add session header
            response.headers_mut().insert(
                "Mcp-Session-Id",
                "gitideas-session".parse().unwrap(),
            );
            response
        }
        None => StatusCode::NO_CONTENT.into_response(), // Notification, no response
    }
}

async fn handle_jsonrpc(state: &AppState, req: &Value) -> Option<Value> {
    let id = req.get("id");
    let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let params = req.get("params").cloned().unwrap_or(json!({}));

    // Notifications (no id) get no response
    if id.is_none() {
        return None;
    }
    let id = id.unwrap().clone();

    let result = match method {
        "initialize" => handle_initialize(&params),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(handle_tools_list()),
        "tools/call" => handle_tools_call(state, &params).await,
        _ => Err(jsonrpc_error(-32601, "method not found")),
    };

    Some(match result {
        Ok(res) => json!({"jsonrpc": "2.0", "id": id, "result": res}),
        Err(err) => json!({"jsonrpc": "2.0", "id": id, "error": err}),
    })
}

fn handle_initialize(params: &Value) -> Result<Value, Value> {
    let _client_version = params
        .get("protocolVersion")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    Ok(json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {
            "tools": {}
        },
        "serverInfo": {
            "name": "gitideas",
            "version": env!("CARGO_PKG_VERSION")
        }
    }))
}

fn handle_tools_list() -> Value {
    json!({
        "tools": [
            {
                "name": "add",
                "description": "Add a new idea, todo, or memory entry to the git repository",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "type": {
                            "type": "string",
                            "enum": ["IDEA", "TODO", "MEMORY"],
                            "description": "The type of entry"
                        },
                        "subject": {
                            "type": "string",
                            "description": "Single-line subject (max 120 bytes)",
                            "maxLength": 120
                        },
                        "text": {
                            "type": "string",
                            "description": "The body text of the entry"
                        }
                    },
                    "required": ["type", "subject", "text"]
                }
            },
            {
                "name": "search",
                "description": "Search entries by subject, text, date range, or type. Returns newest entries first, up to 100KB of results.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "subject": {
                            "type": "string",
                            "description": "Search subjects only (case-insensitive substring)"
                        },
                        "text": {
                            "type": "string",
                            "description": "Search subjects and body text (case-insensitive substring)"
                        },
                        "type": {
                            "type": "string",
                            "enum": ["IDEA", "TODO", "MEMORY"],
                            "description": "Filter by entry type"
                        },
                        "after": {
                            "type": "string",
                            "description": "Only entries after this date (YYYY-MM-DD or YYYY-MM-DD-hh:mm)"
                        },
                        "before": {
                            "type": "string",
                            "description": "Only entries before this date (YYYY-MM-DD or YYYY-MM-DD-hh:mm)"
                        }
                    }
                }
            }
        ]
    })
}

async fn handle_tools_call(state: &AppState, params: &Value) -> Result<Value, Value> {
    let tool_name = params
        .get("name")
        .and_then(|n| n.as_str())
        .ok_or_else(|| jsonrpc_error(-32602, "missing tool name"))?;

    let args = params.get("arguments").cloned().unwrap_or(json!({}));

    match tool_name {
        "add" => tool_add(state, &args).await,
        "search" => tool_search(state, &args).await,
        _ => Err(jsonrpc_error(-32602, &format!("unknown tool: {}", tool_name))),
    }
}

async fn tool_add(state: &AppState, args: &Value) -> Result<Value, Value> {
    let type_str = args
        .get("type")
        .and_then(|v| v.as_str())
        .ok_or_else(|| jsonrpc_error(-32602, "missing 'type' argument"))?;
    let subject = args
        .get("subject")
        .and_then(|v| v.as_str())
        .ok_or_else(|| jsonrpc_error(-32602, "missing 'subject' argument"))?;
    let text = args
        .get("text")
        .and_then(|v| v.as_str())
        .ok_or_else(|| jsonrpc_error(-32602, "missing 'text' argument"))?;

    let idea_type = IdeaType::from_str(type_str)
        .ok_or_else(|| jsonrpc_error(-32602, "type must be IDEA, TODO, or MEMORY"))?;

    let now = Utc::now().format("%Y-%m-%d-%H:%M").to_string();

    // Acquire git lock
    let _guard = state.git_lock.lock().await;

    let (file, date) = crate::git::add_with_retry(&state.repo_path, idea_type, subject, text, &now)
        .await
        .map_err(|e| jsonrpc_error(-32000, &e.to_string()))?;

    Ok(json!({
        "content": [{
            "type": "text",
            "text": format!("Added {} entry: \"{}\" to {} ({})", idea_type, subject, file, date)
        }]
    }))
}

async fn tool_search(state: &AppState, args: &Value) -> Result<Value, Value> {
    use crate::types::SearchRequest;

    let idea_type = args
        .get("type")
        .and_then(|v| v.as_str())
        .and_then(IdeaType::from_str);

    let req = SearchRequest {
        subject: args.get("subject").and_then(|v| v.as_str()).map(String::from),
        text: args.get("text").and_then(|v| v.as_str()).map(String::from),
        after: args.get("after").and_then(|v| v.as_str()).map(String::from),
        before: args.get("before").and_then(|v| v.as_str()).map(String::from),
        idea_type,
    };

    let repo_path = state.repo_path.clone();
    let result = tokio::task::spawn_blocking(move || crate::search::search(&repo_path, &req))
        .await
        .map_err(|e| jsonrpc_error(-32000, &format!("search failed: {}", e)))?
        .map_err(|e| jsonrpc_error(-32000, &e.to_string()))?;

    if result.entries.is_empty() {
        return Ok(json!({
            "content": [{
                "type": "text",
                "text": "No entries found."
            }]
        }));
    }

    let mut output = String::new();
    for entry in &result.entries {
        output.push_str(&format!(
            "[{}] {} — {}\n",
            entry.date, entry.idea_type, entry.subject
        ));
        if !entry.text.is_empty() {
            for line in entry.text.lines() {
                output.push_str(&format!("  {}\n", line));
            }
        }
        output.push('\n');
    }
    output.push_str(&format!("{} result(s)", result.entries.len()));

    Ok(json!({
        "content": [{
            "type": "text",
            "text": output
        }]
    }))
}

fn jsonrpc_error(code: i64, message: &str) -> Value {
    json!({
        "code": code,
        "message": message
    })
}
