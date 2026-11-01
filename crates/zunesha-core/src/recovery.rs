//! Failure classification and human-guided task recovery.
//!
//! Agent CLIs do not agree on how they report a quota wall: OpenCode may print
//! `free usage limit`, a provider may emit HTTP 429, and another may simply
//! exit non-zero after `max steps`. This module turns those signals into a
//! stable [`StopReason`] and keeps the last transcript tail, so a paused task
//! can be explained and resumed instead of silently reverting to `TODO`.
//!
//! Resuming is deliberately a human action:
//! * [`ResumeMode::Continue`] re-runs in the existing worktree with a short
//!   continuation instruction and the prior handoff summary.
//! * [`ResumeMode::Feedback`] appends the reviewer's prompt plus the current
//!   `git status` / `git diff --stat`.
//! * [`ResumeMode::HardReset`] removes the worktree and starts clean.
//!
//! All three move the task back to `TODO`; the caller then runs it. The
//! worktree is never deleted unless the human explicitly asks for a reset.

use std::path::Path;

use crate::db::Database;
use crate::error::{Error, Result};
use crate::paths::ZuneshaPaths;
use crate::sandbox::redact_secrets;
use crate::task::{StopReason, Task, TaskStatus};
use crate::workspace::WorkspaceManager;

/// Maximum transcript tail persisted with a failed task.
pub const LOG_TAIL_CHARS: usize = 6_000;

/// Maximum worktree-state block injected into a feedback retry.
const WORKTREE_STATE_CHARS: usize = 5_000;

/// How a human wants to resume a paused task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeMode {
    /// Continue from the current worktree without new instructions.
    Continue,
    /// Re-run with a targeted feedback prompt.
    Feedback,
    /// Discard the worktree and start from a fresh snapshot.
    HardReset,
}

impl ResumeMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Continue => "continue",
            Self::Feedback => "feedback",
            Self::HardReset => "hard_reset",
        }
    }
}

/// Classify an agent attempt from its exit status and transcript.
///
/// Quota/token signatures win over the generic non-zero exit because the
/// recovery action differs (wait for a reset / switch provider / summarize)
/// and the UI badge is more useful. A successful exit with a signature is also
/// classified: some CLIs exit 0 after printing a quota warning.
pub fn classify_agent_stop(exit_ok: bool, logs: &str) -> Option<StopReason> {
    if let Some(reason) = classify_log_text(logs) {
        return Some(reason);
    }
    (!exit_ok).then_some(StopReason::AgentError)
}

/// Find a quota/token signature in a transcript.
pub fn classify_log_text(text: &str) -> Option<StopReason> {
    let lower = text.to_ascii_lowercase();
    const RATE_LIMIT: &[&str] = &[
        "rate limit",
        "rate_limit",
        "ratelimit",
        "quota exceeded",
        "quota_exceeded",
        "free usage limit",
        "free-usage limit",
        "free usage",
        "usage limit reached",
        "insufficient_quota",
        "http 429",
        "status 429",
        "429 too many requests",
        "too many requests",
    ];
    if RATE_LIMIT.iter().any(|needle| lower.contains(needle)) {
        return Some(StopReason::RateLimit);
    }

    const TOKEN_LIMIT: &[&str] = &[
        "token limit",
        "token_limit",
        "max tokens",
        "maximum tokens",
        "context length",
        "context window",
        "maximum context",
        "max steps",
        "step limit",
        "steps exhausted",
        "token budget",
        "output token",
    ];
    if TOKEN_LIMIT.iter().any(|needle| lower.contains(needle)) {
        return Some(StopReason::TokenLimit);
    }
    None
}

/// Keep a bounded tail of a transcript, preferring whole lines.
pub fn log_tail(logs: &str, max_chars: usize) -> String {
    let redacted = redact_secrets(logs);
    let trimmed = redacted.trim_end();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let mut start = trimmed.len().saturating_sub(max_chars);
    while start < trimmed.len() && !trimmed.is_char_boundary(start) {
        start += 1;
    }
    let tail = &trimmed[start..];
    let tail = tail.split_once('\n').map(|(_, rest)| rest).unwrap_or(tail);
    format!("…\n{tail}")
}

/// Persist a classified stop on a task, preserving its worktree.
pub fn record_stop(db: &Database, task_id: &str, reason: StopReason, detail: &str) -> Result<()> {
    let task = db.require_task(task_id)?;
    let tail = log_tail(&task.worker_logs, LOG_TAIL_CHARS);
    let summary = if task.handoff_summary.trim().is_empty() {
        handoff_from_output(&task.agent_output, &task.description)
    } else {
        task.handoff_summary.clone()
    };
    db.set_recovery_state(
        task_id,
        Some(reason),
        true,
        &tail,
        &summary,
        detail,
    )?;
    Ok(())
}

/// A concise fallback handoff summary when no generated summary exists.
pub fn handoff_from_output(output: &str, description: &str) -> String {
    let output = output.trim();
    if !output.is_empty() {
        return truncate(output, 2_000);
    }
    let description = description.trim();
    if description.is_empty() {
        "No handoff summary was produced; inspect the worktree diff.".to_string()
    } else {
        format!(
            "No final agent response. Original task:\n{}",
            truncate(description, 1_500)
        )
    }
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut end = max_chars;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// Resume a paused/reviewed task in its existing worktree (or reset it).
///
/// The task is moved to `TODO` and its recovery flags are cleared; the caller
/// is responsible for spawning the worker. `feedback` is required for
/// [`ResumeMode::Feedback`] and ignored otherwise.
pub fn resume_task(
    paths: &ZuneshaPaths,
    task_id: &str,
    mode: ResumeMode,
    feedback: &str,
) -> Result<Task> {
    let db = Database::open(&paths.state_db())?;
    let task = db.require_task(task_id)?;
    if !matches!(
        task.status,
        TaskStatus::Review
            | TaskStatus::NeedsInteraction
            | TaskStatus::FailedNeedsHuman
            | TaskStatus::Todo
    ) {
        return Err(Error::InvalidTransition {
            task_id: task_id.to_string(),
            from: task.status,
            to: TaskStatus::Todo,
        });
    }
    if mode == ResumeMode::Feedback && feedback.trim().is_empty() {
        return Err(Error::Workspace(
            "feedback resume requires a non-empty message".into(),
        ));
    }

    let mgr = WorkspaceManager::new(paths.clone());
    if mode == ResumeMode::HardReset {
        mgr.remove(&db, task_id)?;
    }

    let workspace_state = if mode == ResumeMode::HardReset {
        "The previous worktree was discarded. Start from the base repository snapshot."
            .to_string()
    } else {
        mgr.worktree_state(&db, task_id)
            .unwrap_or_else(|_| "Worktree state unavailable.".to_string())
    };

    let mut description = task.description.trim_end().to_string();
    match mode {
        ResumeMode::Continue => {
            if !description.is_empty() {
                description.push_str("\n\n");
            }
            description.push_str("## Resume and complete\n");
            description.push_str(
                "Resume and complete the task from the current repository state and existing \
                 git diff in this worktree. Do not restart from scratch. Preserve all partial \
                 work, finish the remaining acceptance criteria, and summarize what changed.",
            );
            let summary = if task.handoff_summary.trim().is_empty() {
                handoff_from_output(&task.agent_output, &task.description)
            } else {
                task.handoff_summary.clone()
            };
            description.push_str("\n\n### Prior handoff summary\n");
            description.push_str(&truncate(&summary, 2_500));
            description.push_str("\n\n### Current worktree state\n");
            description.push_str(&workspace_state);
        }
        ResumeMode::Feedback => {
            if !description.is_empty() {
                description.push_str("\n\n");
            }
            description.push_str("## Human feedback\n");
            description.push_str(feedback.trim());
            description.push_str("\n\n### Current worktree state\n");
            description.push_str(&workspace_state);
            description.push_str(
                "\n\nIterate directly on the existing worktree changes. Do not discard the \
                 current diff unless the feedback explicitly asks for a reset.",
            );
        }
        ResumeMode::HardReset => {
            if !description.is_empty() {
                description.push_str("\n\n");
            }
            description.push_str("## Restart from base\n");
            description.push_str(
                "The previous worktree was reset at human request. Re-implement the task from \
                 the current base repository state, incorporating any feedback below.",
            );
            if !feedback.trim().is_empty() {
                description.push_str("\n\n");
                description.push_str(feedback.trim());
            }
        }
    }

    db.update_task(task_id, None, Some(&description))?;
    db.set_retry_count(task_id, 0)?;
    db.set_diff(task_id, "")?;
    db.clear_recovery_state(task_id)?;
    db.set_status(task_id, TaskStatus::Todo)?;
    db.require_task(task_id)
}

/// Read `git status --short` and `git diff --stat` from a task's worktree.
pub fn worktree_state(root: &Path, workspace: &Path) -> String {
    let status = git_output(workspace, &["status", "--short"]);
    let stat = git_output(workspace, &["diff", "--stat"]);
    let mut out = String::new();
    if !status.trim().is_empty() {
        out.push_str("git status --short:\n");
        out.push_str(status.trim());
        out.push('\n');
    }
    if !stat.trim().is_empty() {
        out.push_str("git diff --stat:\n");
        out.push_str(stat.trim());
        out.push('\n');
    }
    if out.is_empty() {
        out.push_str("No uncommitted changes in the worktree.\n");
    }
    let _ = root;
    truncate(&out, WORKTREE_STATE_CHARS)
}

fn git_output(dir: &Path, args: &[&str]) -> String {
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_quota_and_token_signatures() {
        assert_eq!(
            classify_log_text("provider said: free usage limit reached"),
            Some(StopReason::RateLimit)
        );
        assert_eq!(
            classify_log_text("HTTP 429 Too Many Requests"),
            Some(StopReason::RateLimit)
        );
        assert_eq!(
            classify_log_text("maximum context length exceeded"),
            Some(StopReason::TokenLimit)
        );
        assert_eq!(
            classify_log_text("reached max steps for this run"),
            Some(StopReason::TokenLimit)
        );
        assert_eq!(classify_log_text("compiler warning"), None);
    }

    #[test]
    fn non_zero_exit_without_signature_is_agent_error() {
        assert_eq!(
            classify_agent_stop(false, "segfault"),
            Some(StopReason::AgentError)
        );
        assert_eq!(classify_agent_stop(true, "done"), None);
        assert_eq!(
            classify_agent_stop(true, "quota exceeded"),
            Some(StopReason::RateLimit)
        );
    }

    #[test]
    fn log_tail_is_bounded_and_redacted() {
        let logs = format!("secret sk-abcdefghijklmnopqrstuvwxyz\n{}", "x".repeat(100));
        let tail = log_tail(&logs, 40);
        assert!(tail.chars().count() <= 42);
        assert!(!tail.contains("sk-abcdefghijklmnopqrstuvwxyz"));
    }
}
