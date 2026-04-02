use crate::entry::parse_entries;
use crate::storage::list_data_files;
use crate::types::{AppError, EntryResponse, SearchRequest, SearchResponse};

const MAX_RESULT_BYTES: usize = 100 * 1024; // 100KB

/// Normalize a date string for comparison.
/// If only YYYY-MM-DD is provided, append time component.
fn normalize_date(date: &str, is_before: bool) -> String {
    if date.len() == 10 {
        // YYYY-MM-DD without time
        if is_before {
            format!("{}-23:59", date)
        } else {
            format!("{}-00:00", date)
        }
    } else {
        date.to_string()
    }
}

pub fn search(req: &SearchRequest) -> Result<SearchResponse, AppError> {
    let files = list_data_files(std::path::Path::new("."), req.idea_type)?;

    let after = req.after.as_deref().map(|d| normalize_date(d, false));
    let before = req.before.as_deref().map(|d| normalize_date(d, true));
    let subject_query = req.subject.as_deref().map(|s| s.to_lowercase());
    let text_query = req.text.as_deref().map(|s| s.to_lowercase());

    let mut results = Vec::new();
    let mut accumulated_bytes: usize = 0;

    for file_info in &files {
        if accumulated_bytes >= MAX_RESULT_BYTES {
            break;
        }

        let content = std::fs::read_to_string(&file_info.path)
            .map_err(|e| AppError::IoError(e))?;
        let entries = parse_entries(&content);

        // Iterate in reverse: entries are appended chronologically,
        // so the last entry is the newest
        for entry in entries.into_iter().rev() {
            if accumulated_bytes >= MAX_RESULT_BYTES {
                break;
            }

            // Date range filters
            if let Some(ref after_date) = after {
                if entry.date.as_str() < after_date.as_str() {
                    continue;
                }
            }
            if let Some(ref before_date) = before {
                if entry.date.as_str() > before_date.as_str() {
                    continue;
                }
            }

            // Subject filter: case-insensitive substring on subject only
            if let Some(ref sq) = subject_query {
                if !entry.subject.to_lowercase().contains(sq.as_str()) {
                    continue;
                }
            }

            // Text filter: case-insensitive substring on subject + body
            if let Some(ref tq) = text_query {
                let haystack = format!("{}\n{}", entry.subject, entry.body).to_lowercase();
                if !haystack.contains(tq.as_str()) {
                    continue;
                }
            }

            let entry_size = entry.date.len() + entry.subject.len() + entry.body.len();
            accumulated_bytes += entry_size;

            results.push(EntryResponse {
                idea_type: file_info.idea_type,
                date: entry.date,
                subject: entry.subject,
                text: entry.body,
            });
        }
    }

    Ok(SearchResponse { entries: results })
}
