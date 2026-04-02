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
    pub date: String,
    pub subject: String,
    pub body: String,
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
    // Also reject DEL (0x7F) and C1 control characters (U+0080..U+009F)
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
    // Try sequential combinations to find one not in the body.
    // With 32 emojis and 4 positions, there are 32^4 = 1,048,576 combos.
    for a in EMOJI_POOL {
        for b in EMOJI_POOL {
            for c in EMOJI_POOL {
                for d in EMOJI_POOL {
                    let delim: String =
                        format!("-----{}{}{}{}", a, b, c, d);
                    if !body.contains(&delim) {
                        return delim;
                    }
                }
            }
        }
    }
    // Fallback — should never happen with 1M+ combos vs 1MB max body
    "-----🔥😀🍕🎸".to_string()
}

pub fn format_entry(date: &str, subject: &str, body: &str) -> String {
    let delimiter = generate_delimiter(body);
    format!(
        "date: {}\nsubject: {}\n{}\n{}\n{}\n",
        date, subject, delimiter, body, delimiter
    )
}

enum ParseState {
    ExpectDate,
    ExpectSubject { date: String },
    ExpectDelimiter { date: String, subject: String },
    ReadBody {
        date: String,
        subject: String,
        delimiter: String,
        body_lines: Vec<String>,
    },
}

pub fn parse_entries(content: &str) -> Vec<Entry> {
    let mut entries = Vec::new();
    let mut state = ParseState::ExpectDate;

    for line in content.lines() {
        state = match state {
            ParseState::ExpectDate => {
                if let Some(date) = line.strip_prefix("date: ") {
                    ParseState::ExpectSubject {
                        date: date.to_string(),
                    }
                } else {
                    ParseState::ExpectDate
                }
            }
            ParseState::ExpectSubject { date } => {
                if let Some(subject) = line.strip_prefix("subject: ") {
                    ParseState::ExpectDelimiter {
                        date,
                        subject: subject.to_string(),
                    }
                } else {
                    // Malformed, reset
                    ParseState::ExpectDate
                }
            }
            ParseState::ExpectDelimiter { date, subject } => {
                if line.starts_with("-----") && line.chars().count() > 5 {
                    ParseState::ReadBody {
                        date,
                        subject,
                        delimiter: line.to_string(),
                        body_lines: Vec::new(),
                    }
                } else {
                    ParseState::ExpectDate
                }
            }
            ParseState::ReadBody {
                date,
                subject,
                delimiter,
                mut body_lines,
            } => {
                if line == delimiter {
                    entries.push(Entry {
                        date,
                        subject,
                        body: body_lines.join("\n"),
                    });
                    ParseState::ExpectDate
                } else {
                    body_lines.push(line.to_string());
                    ParseState::ReadBody {
                        date,
                        subject,
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
    fn test_format_and_parse_roundtrip() {
        let date = "2026-04-01-14:30";
        let subject = "Test idea";
        let body = "This is the body of the idea.\nWith multiple lines.";

        let formatted = format_entry(date, subject, body);
        let entries = parse_entries(&formatted);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].date, date);
        assert_eq!(entries[0].subject, subject);
        assert_eq!(entries[0].body, body);
    }

    #[test]
    fn test_parse_multiple_entries() {
        let entry1 = format_entry("2026-04-01-14:30", "First", "Body one");
        let entry2 = format_entry("2026-04-01-15:00", "Second", "Body two");
        let content = format!("{}{}", entry1, entry2);

        let entries = parse_entries(&content);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].subject, "First");
        assert_eq!(entries[1].subject, "Second");
    }

    #[test]
    fn test_parse_entry_with_dashes_in_body() {
        let body = "Some text\n-----not a delimiter\nMore text";
        let formatted = format_entry("2026-04-01-14:30", "Test", body);
        let entries = parse_entries(&formatted);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].body, body);
    }

    #[test]
    fn test_delimiter_avoids_body_content() {
        // Body containing the first possible delimiter
        let first_delim = format!("-----{}{}{}{}", EMOJI_POOL[0], EMOJI_POOL[0], EMOJI_POOL[0], EMOJI_POOL[0]);
        let body = format!("text with {} inside", first_delim);
        let delim = generate_delimiter(&body);
        assert!(!body.contains(&delim));
        assert_ne!(delim, first_delim);
    }
}
