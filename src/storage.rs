use crate::types::IdeaType;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

const MAX_FILE_SIZE: u64 = 100 * 1024; // 100KB

const ALL_TYPES: &[IdeaType] = &[IdeaType::Idea, IdeaType::Todo, IdeaType::Memory];

/// A data file with its parsed type and date components.
#[derive(Debug)]
pub struct DataFile {
    pub path: PathBuf,
    pub idea_type: IdeaType,
    pub date_part: String,
}

/// Scan a single type subdirectory for data files.
fn scan_type_dir(dir: &Path, idea_type: IdeaType) -> io::Result<Vec<DataFile>> {
    let type_dir = dir.join(idea_type.to_string());
    let mut files = Vec::new();

    let entries = match fs::read_dir(&type_dir) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(files),
        Err(e) => return Err(e),
    };

    let prefix = idea_type.to_string();
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();

        // Parse as TYPE.DATE
        if let Some((type_str, date_part)) = name.split_once('.') {
            if type_str == prefix && date_part.len() == 16 {
                files.push(DataFile {
                    path: entry.path(),
                    idea_type,
                    date_part: date_part.to_string(),
                });
            }
        }
    }

    Ok(files)
}

/// List all data files, optionally filtered by type, sorted newest-first.
/// Files live in subdirectories: IDEA/, TODO/, MEMORY/
pub fn list_data_files(dir: &Path, filter_type: Option<IdeaType>) -> io::Result<Vec<DataFile>> {
    let mut files = match filter_type {
        Some(t) => scan_type_dir(dir, t)?,
        None => {
            let mut all = Vec::new();
            for &t in ALL_TYPES {
                all.extend(scan_type_dir(dir, t)?);
            }
            all
        }
    };

    files.sort_by(|a, b| b.date_part.cmp(&a.date_part));
    Ok(files)
}

/// Find the newest file for the given type. Returns None if no files exist.
pub fn find_newest_file(dir: &Path, idea_type: IdeaType) -> io::Result<Option<DataFile>> {
    let files = list_data_files(dir, Some(idea_type))?;
    Ok(files.into_iter().next())
}

/// Get the file to write to: the newest file if it's under 100KB, or a new file.
/// Creates the type subdirectory if it doesn't exist.
pub fn target_file(dir: &Path, idea_type: IdeaType, now: &str) -> io::Result<PathBuf> {
    let type_dir = dir.join(idea_type.to_string());

    if let Some(file) = find_newest_file(dir, idea_type)? {
        let meta = fs::metadata(&file.path)?;
        if meta.len() < MAX_FILE_SIZE {
            return Ok(file.path);
        }
    }

    fs::create_dir_all(&type_dir)?;
    Ok(type_dir.join(format!("{}.{}", idea_type, now)))
}

/// Append an entry string to the given file (creates if it doesn't exist).
pub fn append_to_file(path: &Path, content: &str) -> io::Result<()> {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(content.as_bytes())?;
    Ok(())
}

/// Return the relative path of a file within the repo (e.g., "IDEA/IDEA.2026-04-01-14:30").
pub fn relative_path(repo: &Path, full_path: &Path) -> String {
    full_path
        .strip_prefix(repo)
        .unwrap_or(full_path)
        .to_string_lossy()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_list_data_files_empty() {
        let dir = TempDir::new().unwrap();
        let files = list_data_files(dir.path(), None).unwrap();
        assert!(files.is_empty());
    }

    #[test]
    fn test_list_data_files_finds_files() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("IDEA")).unwrap();
        fs::create_dir_all(dir.path().join("TODO")).unwrap();
        fs::write(dir.path().join("IDEA/IDEA.2026-04-01-14:30"), "content").unwrap();
        fs::write(dir.path().join("TODO/TODO.2026-03-15-09:00"), "content").unwrap();
        fs::write(dir.path().join("random.txt"), "not a data file").unwrap();

        let files = list_data_files(dir.path(), None).unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].date_part, "2026-04-01-14:30");
    }

    #[test]
    fn test_list_data_files_filter_by_type() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("IDEA")).unwrap();
        fs::create_dir_all(dir.path().join("TODO")).unwrap();
        fs::write(dir.path().join("IDEA/IDEA.2026-04-01-14:30"), "content").unwrap();
        fs::write(dir.path().join("TODO/TODO.2026-03-15-09:00"), "content").unwrap();

        let files = list_data_files(dir.path(), Some(IdeaType::Idea)).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].idea_type, IdeaType::Idea);
    }

    #[test]
    fn test_find_newest_file() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("IDEA")).unwrap();
        fs::write(dir.path().join("IDEA/IDEA.2026-03-01-10:00"), "old").unwrap();
        fs::write(dir.path().join("IDEA/IDEA.2026-04-01-14:30"), "new").unwrap();

        let newest = find_newest_file(dir.path(), IdeaType::Idea).unwrap().unwrap();
        assert_eq!(newest.date_part, "2026-04-01-14:30");
    }

    #[test]
    fn test_target_file_uses_existing_small_file() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("IDEA")).unwrap();
        fs::write(dir.path().join("IDEA/IDEA.2026-04-01-14:30"), "small content").unwrap();

        let path = target_file(dir.path(), IdeaType::Idea, "2026-04-01-15:00").unwrap();
        assert!(path.to_string_lossy().contains("2026-04-01-14:30"));
    }

    #[test]
    fn test_target_file_creates_new_when_large() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("IDEA")).unwrap();
        let big_content = "x".repeat(100 * 1024 + 1);
        fs::write(dir.path().join("IDEA/IDEA.2026-04-01-14:30"), big_content).unwrap();

        let path = target_file(dir.path(), IdeaType::Idea, "2026-04-01-15:00").unwrap();
        assert!(path.to_string_lossy().contains("2026-04-01-15:00"));
    }

    #[test]
    fn test_target_file_creates_new_when_none_exist() {
        let dir = TempDir::new().unwrap();
        let path = target_file(dir.path(), IdeaType::Idea, "2026-04-01-14:30").unwrap();
        assert!(path.to_string_lossy().contains("IDEA/IDEA.2026-04-01-14:30"));
    }

    #[test]
    fn test_target_file_creates_subdir() {
        let dir = TempDir::new().unwrap();
        let path = target_file(dir.path(), IdeaType::Todo, "2026-04-01-14:30").unwrap();
        assert!(dir.path().join("TODO").is_dir());
        assert!(path.to_string_lossy().contains("TODO/TODO.2026-04-01-14:30"));
    }

    #[test]
    fn test_append_to_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.txt");
        append_to_file(&path, "hello ").unwrap();
        append_to_file(&path, "world").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "hello world");
    }

    #[test]
    fn test_relative_path() {
        let repo = Path::new("/home/user/repo");
        let full = Path::new("/home/user/repo/IDEA/IDEA.2026-04-01-14:30");
        assert_eq!(relative_path(repo, full), "IDEA/IDEA.2026-04-01-14:30");
    }
}
