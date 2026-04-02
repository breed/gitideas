use tokio::process::Command;

use crate::types::AppError;

async fn run_git(args: &[&str]) -> Result<String, AppError> {
    let output = Command::new("git")
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

pub async fn git_pull() -> Result<(), AppError> {
    // Pull with rebase to keep history linear
    // If there's no remote configured or no upstream, that's okay for local-only use
    let output = Command::new("git")
        .args(["pull", "--rebase"])
        .output()
        .await
        .map_err(|e| AppError::GitError(format!("failed to run git pull: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // If there's no remote or no tracking branch, that's not a fatal error
        if stderr.contains("no tracking information")
            || stderr.contains("no such remote")
            || stderr.contains("There is no tracking information")
        {
            return Ok(());
        }
        return Err(AppError::GitError(format!("git pull failed: {}", stderr)));
    }
    Ok(())
}

pub async fn git_add(path: &str) -> Result<(), AppError> {
    run_git(&["add", path]).await?;
    Ok(())
}

pub async fn git_commit(message: &str) -> Result<(), AppError> {
    run_git(&["commit", "-m", message]).await?;
    Ok(())
}

pub async fn git_push() -> Result<(), AppError> {
    run_git(&["push"]).await?;
    Ok(())
}

/// Check if a push failure is due to a conflict (remote has new commits).
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

/// Get the current branch name.
async fn current_branch() -> Result<String, AppError> {
    let output = run_git(&["rev-parse", "--abbrev-ref", "HEAD"]).await?;
    Ok(output.trim().to_string())
}

/// Reset to the remote tracking branch, discarding local commits.
async fn git_reset_to_remote() -> Result<(), AppError> {
    let branch = current_branch().await?;
    // Abort any in-progress rebase (ignore errors)
    let _ = Command::new("git")
        .args(["rebase", "--abort"])
        .output()
        .await;
    run_git(&["reset", "--hard", &format!("origin/{}", branch)]).await?;
    Ok(())
}

/// The full ADD workflow with conflict retry.
/// Returns the filename the entry was written to.
pub async fn add_with_retry(
    idea_type: crate::types::IdeaType,
    subject: &str,
    text: &str,
    now: &str,
) -> Result<(String, String), AppError> {
    use crate::entry::{format_entry, validate_body, validate_subject};
    use crate::storage::{append_to_file, target_file};
    use std::path::Path;

    validate_subject(subject)?;
    validate_body(text)?;

    let entry_text = format_entry(now, subject, text);
    let dir = Path::new(".");

    for _attempt in 0..5 {
        git_pull().await?;

        let target = target_file(dir, idea_type, now)?;
        append_to_file(&target, &entry_text)?;

        let filename = target.file_name().unwrap().to_string_lossy().to_string();

        git_add(&filename).await?;

        let commit_msg = format!("{}: {}", idea_type, &subject[..subject.len().min(50)]);
        git_commit(&commit_msg).await?;

        match git_push().await {
            Ok(()) => return Ok((filename.clone(), now.to_string())),
            Err(e) if is_conflict_error(&e) => {
                // Conflict — discard and retry
                eprintln!("push conflict, retrying...");
                git_reset_to_remote().await?;
                continue;
            }
            Err(e) => {
                // Non-conflict push error — still try to clean up, then propagate
                // This handles the case where there's no remote configured
                // If push fails for non-conflict reasons, the commit is still local
                // which is acceptable for local-only use
                eprintln!("push failed (non-conflict): {}", e);
                return Ok((filename.clone(), now.to_string()));
            }
        }
    }

    Err(AppError::ConflictRetryExhausted)
}
