use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::Rng;

use crate::types::AppError;

const MAX_SUBJECT_BYTES: usize = 120;
const MAX_BODY_BYTES: usize = 1_048_576; // 1MB

const EMOJI_POOL: &[char] = &[
    '🔥', '😀', '🍕', '🎸', '🌈', '🚀', '💡', '🎯', '🌟', '🎪', '🏔', '🌊', '🦀', '🎭',
    '🍀', '🔮', '🎲', '🏆', '🎵', '⚡', '🌺', '🦊', '🐉', '🍄', '🧩', '💎', '🛸', '🎁',
    '🌻', '🦋', '🍉', '🧊',
];

#[derive(Debug, Clone)]
pub struct Entry {
    pub id: String,
    pub date: String,
    pub subject: String,
    pub due: Option<String>,
    pub complete: Option<String>,
    pub body: String,
}

/// Generate a random 72-bit ID encoded as URL-safe base64 (12 chars).
pub fn generate_id() -> String {
    let mut bytes = [0u8; 9]; // 72 bits = 9 bytes
    rand::rng().fill(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn validate_subject(subject: &str) -> Result<(), AppError> {
    if subject.len() > MAX_SUBJECT_BYTES {
        return Err(AppError::InvalidSubject(format!(
            "subject exceeds {} bytes",
            MAX_SUBJECT_BYTES
        )));
    }
    for byte in subject.as_bytes() {
        if *byte < 0x20 {
            return Err(AppError::InvalidSubject(
                "subject contains control characters".to_string(),
            ));
        }
    }
    for ch in subject.chars() {
        if ch == '\u{7F}' || ('\u{80}'..='\u{9F}').contains(&ch) {
            return Err(AppError::InvalidSubject(
                "subject contains control characters".to_string(),
            ));
        }
    }
    Ok(())
}

pub fn validate_body(text: &str) -> Result<(), AppError> {
    if text.len() > MAX_BODY_BYTES {
        return Err(AppError::BodyTooLarge);
    }
    Ok(())
}

pub fn generate_delimiter(body: &str) -> String {
    for a in EMOJI_POOL {
        for b in EMOJI_POOL {
            for c in EMOJI_POOL {
                for d in EMOJI_POOL {
                    let delim: String = format!("-----{}{}{}{}", a, b, c, d);
                    if !body.contains(&delim) {
                        return delim;
                    }
                }
            }
        }
    }
    "-----🔥😀🍕🎸".to_string()
}

pub fn format_entry(
    id: &str,
    date: &str,
    subject: &str,
    due: Option<&str>,
    complete: Option<&str>,
    body: &str,
) -> String {
    let delimiter = generate_delimiter(body);
    let mut header = format!("id: {}\ndate: {}\nsubject: {}\n", id, date, subject);
    if let Some(d) = due {
        header.push_str(&format!("due: {}\n", d));
    }
    if let Some(c) = complete {
        header.push_str(&format!("complete: {}\n", c));
    }
    format!("{}{}\n{}\n{}\n", header, delimiter, body, delimiter)
}

enum ParseState {
    ExpectHeader,
    ReadHeaders {
        headers: Vec<(String, String)>,
    },
    ReadBody {
        id: String,
        date: String,
        subject: String,
        due: Option<String>,
        complete: Option<String>,
        delimiter: String,
        body_lines: Vec<String>,
    },
}

pub fn parse_entries(content: &str) -> Vec<Entry> {
    let mut entries = Vec::new();
    let mut state = ParseState::ExpectHeader;

    for line in content.lines() {
        state = match state {
            ParseState::ExpectHeader => {
                if line.contains(": ") && !line.starts_with("-----") {
                    let mut headers = Vec::new();
                    if let Some((key, value)) = line.split_once(": ") {
                        headers.push((key.to_string(), value.to_string()));
                    }
                    ParseState::ReadHeaders { headers }
                } else {
                    ParseState::ExpectHeader
                }
            }
            ParseState::ReadHeaders { mut headers } => {
                if line.starts_with("-----") && line.chars().count() > 5 {
                    // Reached the delimiter — extract fields from headers
                    let mut id = None;
                    let mut date = None;
                    let mut subject = None;
                    let mut due = None;
                    let mut complete = None;

                    for (key, value) in &headers {
                        match key.as_str() {
                            "id" => id = Some(value.clone()),
                            "date" => date = Some(value.clone()),
                            "subject" => subject = Some(value.clone()),
                            "due" => due = Some(value.clone()),
                            "complete" => complete = Some(value.clone()),
                            _ => {}
                        }
                    }

                    match (id, date, subject) {
                        (Some(id), Some(date), Some(subject)) => ParseState::ReadBody {
                            id,
                            date,
                            subject,
                            due,
                            complete,
                            delimiter: line.to_string(),
                            body_lines: Vec::new(),
                        },
                        _ => ParseState::ExpectHeader, // Missing required fields
                    }
                } else if line.contains(": ") {
                    if let Some((key, value)) = line.split_once(": ") {
                        headers.push((key.to_string(), value.to_string()));
                    }
                    ParseState::ReadHeaders { headers }
                } else {
                    ParseState::ExpectHeader // Malformed
                }
            }
            ParseState::ReadBody {
                id,
                date,
                subject,
                due,
                complete,
                delimiter,
                mut body_lines,
            } => {
                if line == delimiter {
                    entries.push(Entry {
                        id,
                        date,
                        subject,
                        due,
                        complete,
                        body: body_lines.join("\n"),
                    });
                    ParseState::ExpectHeader
                } else {
                    body_lines.push(line.to_string());
                    ParseState::ReadBody {
                        id,
                        date,
                        subject,
                        due,
                        complete,
                        delimiter,
                        body_lines,
                    }
                }
            }
        };
    }

    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_subject_valid() {
        assert!(validate_subject("Hello world").is_ok());
        assert!(validate_subject("A simple idea with spaces").is_ok());
        assert!(validate_subject(&"x".repeat(120)).is_ok());
    }

    #[test]
    fn test_validate_subject_too_long() {
        assert!(validate_subject(&"x".repeat(121)).is_err());
    }

    #[test]
    fn test_validate_subject_control_chars() {
        assert!(validate_subject("hello\tworld").is_err());
        assert!(validate_subject("hello\nworld").is_err());
        assert!(validate_subject("hello\rworld").is_err());
        assert!(validate_subject("hello\x00world").is_err());
    }

    #[test]
    fn test_validate_body_valid() {
        assert!(validate_body("hello").is_ok());
        assert!(validate_body(&"x".repeat(MAX_BODY_BYTES)).is_ok());
    }

    #[test]
    fn test_validate_body_too_large() {
        assert!(validate_body(&"x".repeat(MAX_BODY_BYTES + 1)).is_err());
    }

    #[test]
    fn test_generate_delimiter_not_in_body() {
        let body = "some text with 🔥 emojis 😀";
        let delim = generate_delimiter(body);
        assert!(delim.starts_with("-----"));
        assert!(!body.contains(&delim));
    }

    #[test]
    fn test_generate_id_format() {
        let id = generate_id();
        assert_eq!(id.len(), 12); // 9 bytes → 12 base64 chars
        // Should be valid URL-safe base64
        assert!(URL_SAFE_NO_PAD.decode(&id).is_ok());
    }

    #[test]
    fn test_format_and_parse_roundtrip_minimal() {
        let id = generate_id();
        let date = "2026-04-01-14:30";
        let subject = "Test idea";
        let body = "This is the body.\nWith multiple lines.";

        let formatted = format_entry(&id, date, subject, None, None, body);
        let entries = parse_entries(&formatted);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, id);
        assert_eq!(entries[0].date, date);
        assert_eq!(entries[0].subject, subject);
        assert_eq!(entries[0].body, body);
        assert!(entries[0].due.is_none());
        assert!(entries[0].complete.is_none());
    }

    #[test]
    fn test_format_and_parse_roundtrip_with_due_and_complete() {
        let id = generate_id();
        let formatted = format_entry(
            &id,
            "2026-04-01-14:30",
            "Test",
            Some("2026-05-01"),
            Some("2026-04-15"),
            "Body text",
        );
        let entries = parse_entries(&formatted);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, id);
        assert_eq!(entries[0].due.as_deref(), Some("2026-05-01"));
        assert_eq!(entries[0].complete.as_deref(), Some("2026-04-15"));
    }

    #[test]
    fn test_parse_multiple_entries() {
        let entry1 = format_entry(&generate_id(), "2026-04-01-14:30", "First", None, None, "Body one");
        let entry2 = format_entry(&generate_id(), "2026-04-01-15:00", "Second", Some("2026-05-01"), None, "Body two");
        let content = format!("{}{}", entry1, entry2);

        let entries = parse_entries(&content);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].subject, "First");
        assert_eq!(entries[1].subject, "Second");
        assert!(entries[0].due.is_none());
        assert_eq!(entries[1].due.as_deref(), Some("2026-05-01"));
    }

    #[test]
    fn test_parse_entry_with_dashes_in_body() {
        let body = "Some text\n-----not a delimiter\nMore text";
        let formatted = format_entry(&generate_id(), "2026-04-01-14:30", "Test", None, None, body);
        let entries = parse_entries(&formatted);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].body, body);
    }

    #[test]
    fn test_delimiter_avoids_body_content() {
        let first_delim = format!(
            "-----{}{}{}{}",
            EMOJI_POOL[0], EMOJI_POOL[0], EMOJI_POOL[0], EMOJI_POOL[0]
        );
        let body = format!("text with {} inside", first_delim);
        let delim = generate_delimiter(&body);
        assert!(!body.contains(&delim));
        assert_ne!(delim, first_delim);
    }

    #[test]
    fn test_parse_backward_compat_no_id() {
        // Old format without id header should not parse (id is required)
        let old = "date: 2026-04-01-14:30\nsubject: Old entry\n-----🔥😀🍕🎸\nbody\n-----🔥😀🍕🎸\n";
        let entries = parse_entries(old);
        assert_eq!(entries.len(), 0);
    }
}
