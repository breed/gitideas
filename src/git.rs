use std::path::Path;

use tokio::process::Command;
use tracing::{debug, info, warn};

use crate::types::AppError;

async fn run_git(repo: &Path, args: &[&str]) -> Result<String, AppError> {
    debug!(cmd = %args.join(" "), "git");
    let output = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .await
        .map_err(|e| AppError::GitError(format!("failed to run git: {}", e)))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        Err(AppError::GitError(format!(
            "git {} failed: {}",
            args.join(" "),
            stderr
        )))
    }
}

async fn git_pull(repo: &Path) -> Result<(), AppError> {
    let output = Command::new("git")
        .current_dir(repo)
        .args(["pull", "--rebase"])
        .output()
        .await
        .map_err(|e| AppError::GitError(format!("failed to run git pull: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("no tracking information")
            || stderr.contains("no such remote")
            || stderr.contains("There is no tracking information")
        {
            debug!("git pull skipped: no remote tracking");
            return Ok(());
        }
        return Err(AppError::GitError(format!("git pull failed: {}", stderr)));
    }
    Ok(())
}

async fn git_add(repo: &Path, path: &str) -> Result<(), AppError> {
    run_git(repo, &["add", path]).await?;
    Ok(())
}

async fn git_commit(repo: &Path, message: &str) -> Result<(), AppError> {
    run_git(repo, &["commit", "-m", message]).await?;
    Ok(())
}

async fn git_push(repo: &Path) -> Result<(), AppError> {
    run_git(repo, &["push"]).await?;
    Ok(())
}

fn is_conflict_error(err: &AppError) -> bool {
    match err {
        AppError::GitError(msg) => {
            msg.contains("rejected")
                || msg.contains("fetch first")
                || msg.contains("non-fast-forward")
                || msg.contains("failed to push")
        }
        _ => false,
    }
}

async fn current_branch(repo: &Path) -> Result<String, AppError> {
    let output = run_git(repo, &["rev-parse", "--abbrev-ref", "HEAD"]).await?;
    Ok(output.trim().to_string())
}

async fn git_reset_to_remote(repo: &Path) -> Result<(), AppError> {
    let branch = current_branch(repo).await?;
    let _ = Command::new("git")
        .current_dir(repo)
        .args(["rebase", "--abort"])
        .output()
        .await;
    run_git(repo, &["reset", "--hard", &format!("origin/{}", branch)]).await?;
    Ok(())
}

/// The full ADD workflow with conflict retry.
/// Returns (file_path, date, id).
pub async fn add_with_retry(
    repo: &Path,
    idea_type: crate::types::IdeaType,
    id: &str,
    subject: &str,
    text: &str,
    due: Option<&str>,
    complete: Option<&str>,
    now: &str,
) -> Result<(String, String, String), AppError> {
    use crate::entry::{format_entry, validate_body, validate_subject};
    use crate::storage::{append_to_file, relative_path, target_file};

    validate_subject(subject)?;
    validate_body(text)?;

    let entry_text = format_entry(id, now, subject, due, complete, text);

    for attempt in 0..5u32 {
        git_pull(repo).await?;

        let target = target_file(repo, idea_type, now)?;
        append_to_file(&target, &entry_text)?;

        let rel_path = relative_path(repo, &target);

        git_add(repo, &rel_path).await?;

        let commit_msg = format!("{}: {}", idea_type, &subject[..subject.len().min(50)]);
        git_commit(repo, &commit_msg).await?;

        match git_push(repo).await {
            Ok(()) => {
                info!(id, %idea_type, file = %rel_path, "entry added");
                return Ok((rel_path.clone(), now.to_string(), id.to_string()));
            }
            Err(e) if is_conflict_error(&e) => {
                warn!(attempt, "push conflict, retrying");
                git_reset_to_remote(repo).await?;
                continue;
            }
            Err(e) => {
                warn!(%e, "push failed (non-conflict), keeping local commit");
                return Ok((rel_path.clone(), now.to_string(), id.to_string()));
            }
        }
    }

    Err(AppError::ConflictRetryExhausted)
}
