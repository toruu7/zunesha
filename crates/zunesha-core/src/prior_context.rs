//! Prior working-tree context: work another agent already left behind
//! (Phase 12).
//!
//! A workspace is a snapshot of the user's checkout, including uncommitted
//! tracked edits and untracked files. Those changes often come from another
//! coding agent (Claude Code, Cursor, Copilot, a previous Zunesha run) and are
//! invisible to the new agent: its workspace baseline is clean, so `git diff`
//! shows only the new task's work.
//!
//! [`generate`] captures the source checkout's status, diff, untracked file
//! list, and known agent instruction files as a bounded Markdown document.
//! [`install`] writes it into the workspace as `PRIOR_AGENT_CONTEXT.md` and
//! excludes it from that workspace's git, exactly like `CODEBASE_MAP.md`, so
//! it never reaches a baseline, diff, or approval. The planner and worker
//! append it to their prompts; the agent is told to build on the existing
//! work rather than revert or redo it.
//!
//! Capturing is deliberately bounded and conservative: diffs and instructions
//! are capped, untracked files are listed but never read (they can carry
//! secrets), and binary files are skipped. The agent can still open those
//! files in the workspace when the task needs them.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::error::Result;

/// File name of the context document inside a workspace.
pub const CONTEXT_FILE: &str = "PRIOR_AGENT_CONTEXT.md";

/// Hard cap on the generated document. The prompt already carries the 8 KB
/// codebase map, so prior context stays in the same reference-material budget.
const MAX_BYTES: usize = 32 * 1024;

/// Cap for `git status --short`.
const MAX_STATUS_BYTES: usize = 4 * 1024;

/// Cap for the uncommitted diff body.
const MAX_DIFF_BYTES: usize = 18 * 1024;

/// Cap for the untracked file list.
const MAX_UNTRACKED_BYTES: usize = 3 * 1024;

/// Only this many untracked paths are listed.
const MAX_UNTRACKED_FILES: usize = 200;

/// Cap for all instruction-file content together.
const MAX_INSTRUCTION_BYTES: usize = 10 * 1024;

/// Cap for one instruction file.
const MAX_INSTRUCTION_FILE_BYTES: usize = 4 * 1024;

/// Only this many instruction files are included.
const MAX_INSTRUCTION_FILES: usize = 8;

/// Cap for a single file read (an instruction file bigger than this is cut).
const MAX_READ_BYTES: u64 = 64 * 1024;

/// Bridge files left by common coding agents, in prompt order. These are
/// instructions/notes, not generated session logs.
const INSTRUCTION_FILES: &[&str] = &[
    "AGENTS.md",
    "CLAUDE.md",
    "GEMINI.md",
    "CODEX.md",
    ".cursorrules",
    ".windsurfrules",
    ".clinerules",
    ".github/copilot-instructions.md",
    ".claude/CLAUDE.md",
    ".codex/instructions.md",
];

/// Directories whose markdown-ish children are agent rules/instructions.
const INSTRUCTION_DIRS: &[&str] = &[".cursor/rules", ".github/instructions"];

/// Generate the context document for `root`, or `None` when there is nothing
/// to say (clean git checkout and no instruction files).
///
/// Best-effort: a missing git, unreadable file, or failed command degrades to
/// the remaining sections instead of failing workspace creation.
pub fn generate(root: &Path) -> Option<String> {
    let mut sections: Vec<String> = Vec::new();
    let mut has_work = false;

    if is_git_repo(root) {
        let status =
            git_output(root, &["status", "--short", "--untracked-files=all"]).unwrap_or_default();
        let status = status
            .lines()
            .filter(|line| {
                let path = line.get(3..).unwrap_or("");
                !is_control_path(path)
            })
            .collect::<Vec<_>>()
            .join("\n");
        let status = status.trim_end();
        if !status.is_empty() {
            has_work = true;
            let (body, truncated) = truncate(status, MAX_STATUS_BYTES);
            let mut section = String::from("## Working tree status\n\n```text\n");
            section.push_str(body.trim_end());
            section.push_str("\n```");
            if truncated {
                section.push_str("\n\n(status truncated)");
            }
            sections.push(section);
        }

        let head = git_output(root, &["rev-parse", "--verify", "HEAD"]);
        let diff_args: &[&str] = if head.is_some() {
            &[
                "diff",
                "--no-ext-diff",
                "--no-color",
                "--unified=3",
                "HEAD",
                "--",
            ]
        } else {
            // An initialized-but-never-committed repo has no HEAD to diff
            // against; staged files still carry useful context.
            &[
                "diff",
                "--no-ext-diff",
                "--no-color",
                "--unified=3",
                "--cached",
                "--",
            ]
        };
        let diff = git_output(root, diff_args).unwrap_or_default();
        if !diff.trim().is_empty() {
            has_work = true;
            let (body, truncated) = truncate(diff.trim_end(), MAX_DIFF_BYTES);
            let mut section =
                String::from("## Uncommitted changes (diff against HEAD)\n\n```diff\n");
            section.push_str(&body);
            section.push_str("\n```");
            if truncated {
                section.push_str(
                    "\n\n(diff truncated; inspect the files in the workspace for the rest)",
                );
            }
            sections.push(section);
        }

        let untracked =
            git_output(root, &["ls-files", "--others", "--exclude-standard"]).unwrap_or_default();
        let paths: Vec<&str> = untracked
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !is_control_path(line))
            .collect();
        if !paths.is_empty() {
            has_work = true;
            sections.push(untracked_section(root, &paths));
        }
    }

    let docs = instruction_docs(root);
    if !docs.is_empty() {
        sections.push(docs);
    }

    if !has_work && sections.is_empty() {
        return None;
    }

    let mut out = String::new();
    out.push_str("# Prior agent context\n\n");
    out.push_str(
        "This context was captured from the source checkout before this task's \
         isolated workspace was created. It may include work from another AI agent \
         (Claude, Cursor, Copilot, …) or from the user. The workspace snapshot \
         already contains these changes. Treat them as existing work: build on \
         them, do not revert or redo them, and call out any conflict with the task.\n",
    );
    if !has_work {
        out.push_str(
            "\nNo uncommitted changes were detected; known agent instruction files \
             are included below.\n",
        );
    }
    for section in sections {
        out.push('\n');
        out.push_str(&section);
        out.push('\n');
    }

    let (mut out, truncated) = truncate(out.trim_end(), MAX_BYTES);
    if truncated {
        out.push_str("\n\n(context truncated)");
    }
    Some(out)
}

/// Write the context document into `workspace` and keep it out of that
/// workspace's git. Idempotent; does nothing when [`generate`] returns `None`.
pub fn install(workspace: &Path, root: &Path) -> Result<()> {
    let Some(text) = generate(root) else {
        return Ok(());
    };
    std::fs::write(workspace.join(CONTEXT_FILE), text)?;

    let exclude = workspace.join(".git").join("info").join("exclude");
    let existing = std::fs::read_to_string(&exclude).unwrap_or_default();
    if !existing.lines().any(|line| line.trim() == CONTEXT_FILE) {
        use std::io::Write as _;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&exclude)?;
        writeln!(file, "{CONTEXT_FILE}")?;
    }
    Ok(())
}

/// The installed context for prompt injection, if the workspace has one.
///
/// Workspaces from before Phase 12 simply have no document; callers degrade
/// to a context-less prompt. Trailing whitespace is trimmed so callers can
/// append it directly after `"\n\n"`.
pub fn read(workspace: &Path) -> Option<String> {
    let text = std::fs::read_to_string(workspace.join(CONTEXT_FILE)).ok()?;
    let text = text.trim_end();
    (!text.is_empty()).then(|| text.to_string())
}

/// Render the untracked-file list. Names only: untracked files are the most
/// likely place for secrets, and the agent can read them in the workspace.
fn untracked_section(root: &Path, paths: &[&str]) -> String {
    let shown = paths.len().min(MAX_UNTRACKED_FILES);
    let mut body = String::new();
    for path in &paths[..shown] {
        let size = std::fs::metadata(root.join(path))
            .map(|meta| format!("{} bytes", meta.len()))
            .unwrap_or_else(|_| "unreadable".to_string());
        body.push_str(&format!("- {path} ({size})\n"));
    }
    if paths.len() > shown {
        body.push_str(&format!(
            "- … and {} more untracked file(s)\n",
            paths.len() - shown
        ));
    }
    let (body, truncated) = truncate(body.trim_end(), MAX_UNTRACKED_BYTES);
    let mut section = String::from("## Untracked files\n\n");
    section.push_str(&body);
    if truncated {
        section.push_str("\n\n(list truncated)");
    }
    section
}

/// Whether the workspace snapshot still contains every path the source
/// checkout now reports as changed (and all instruction files).
///
/// Used before self-healing an old workspace: injecting context for changes
/// the workspace does not actually contain would tell the agent to build on
/// work that is not there. A mismatch is safe to ignore — the task keeps its
/// old snapshot; removing the workspace or a conflict rebase will refresh it.
pub fn snapshot_matches(workspace: &Path, root: &Path) -> bool {
    source_paths(root)
        .into_iter()
        .chain(instruction_paths(root))
        .all(|rel| same_file(&root.join(&rel), &workspace.join(&rel)))
}

/// Read known agent instruction files into one Markdown section.
fn instruction_docs(root: &Path) -> String {
    let candidates = instruction_paths(root);
    let mut body = String::new();
    let mut included = 0;
    for rel in candidates {
        if included >= MAX_INSTRUCTION_FILES || body.len() >= MAX_INSTRUCTION_BYTES {
            break;
        }
        let Some(text) = read_instruction(root.join(&rel)) else {
            continue;
        };
        let heading = rel.to_string_lossy().replace('\\', "/");
        let block = format!("### {heading}\n\n{text}\n\n");
        if body.len() + block.len() > MAX_INSTRUCTION_BYTES {
            let remaining = MAX_INSTRUCTION_BYTES.saturating_sub(body.len());
            let (piece, _) = truncate(&block, remaining);
            body.push_str(&piece);
            body.push_str("\n\n(instruction files truncated)");
            break;
        }
        body.push_str(&block);
        included += 1;
    }

    if body.trim().is_empty() {
        return String::new();
    }
    format!("## Agent instruction files\n\n{}", body.trim_end())
}

/// The known instruction files present in `root`, sorted and deduplicated.
fn instruction_paths(root: &Path) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = INSTRUCTION_FILES.iter().map(PathBuf::from).collect();
    for dir in INSTRUCTION_DIRS {
        let Ok(entries) = std::fs::read_dir(root.join(dir)) else {
            continue;
        };
        let mut in_dir: Vec<PathBuf> = entries
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                if matches!(ext, "md" | "mdc" | "txt") && path.is_file() {
                    path.strip_prefix(root).ok().map(PathBuf::from)
                } else {
                    None
                }
            })
            .collect();
        in_dir.sort();
        candidates.extend(in_dir);
    }
    candidates.sort();
    candidates.dedup();
    candidates
        .into_iter()
        .filter(|rel| root.join(rel).is_file())
        .collect()
}

/// Zunesha's own control directory and git metadata are never user context.
fn is_control_path(path: &str) -> bool {
    path == ".zou" || path.starts_with(".zou/") || path == ".git" || path.starts_with(".git/")
}

/// Paths the source checkout reports as changed: staged/unstaged tracked
/// changes (including both sides of a rename) and untracked files.
fn source_paths(root: &Path) -> Vec<PathBuf> {
    if !is_git_repo(root) {
        return Vec::new();
    }
    let mut out = Vec::new();
    let head = git_output(root, &["rev-parse", "--verify", "HEAD"]);
    let args: &[&str] = if head.is_some() {
        &["diff", "--name-status", "-z", "HEAD", "--"]
    } else {
        &["diff", "--name-status", "-z", "--cached", "--"]
    };
    if let Some(text) = git_output(root, args) {
        let mut parts = text.split('\0').filter(|part| !part.is_empty());
        while let Some(status) = parts.next() {
            if status.starts_with('R') || status.starts_with('C') {
                if let Some(old) = parts.next() {
                    out.push(PathBuf::from(old));
                }
                if let Some(new) = parts.next() {
                    out.push(PathBuf::from(new));
                }
            } else if let Some(path) = parts.next() {
                out.push(PathBuf::from(path));
            } else {
                break;
            }
        }
    }
    if let Some(text) = git_output(root, &["ls-files", "--others", "--exclude-standard", "-z"]) {
        out.extend(
            text.split('\0')
                .filter(|part| !part.is_empty() && !is_control_path(part))
                .map(PathBuf::from),
        );
    }
    out.sort();
    out.dedup();
    out
}

/// Compare two paths by content (symlinks by target), treating "both absent"
/// as equal and a file/directory mismatch as different.
fn same_file(a: &Path, b: &Path) -> bool {
    let (ma, mb) = match (std::fs::symlink_metadata(a), std::fs::symlink_metadata(b)) {
        (Ok(ma), Ok(mb)) => (ma, mb),
        (Err(_), Err(_)) => return true,
        _ => return false,
    };
    if ma.file_type().is_symlink() || mb.file_type().is_symlink() {
        return std::fs::read_link(a).ok() == std::fs::read_link(b).ok();
    }
    if ma.is_dir() || mb.is_dir() {
        return ma.is_dir() == mb.is_dir();
    }
    if ma.len() != mb.len() {
        return false;
    }
    match (std::fs::read(a), std::fs::read(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Read one instruction file as UTF-8-ish text, skipping binaries and cutting
/// absurdly large files. Per-file output is capped by the caller's budget.
fn read_instruction(path: PathBuf) -> Option<String> {
    let meta = std::fs::metadata(&path).ok()?;
    if !meta.is_file() || meta.len() > MAX_READ_BYTES {
        return None;
    }
    let bytes = std::fs::read(&path).ok()?;
    if bytes.iter().take(8 * 1024).any(|&b| b == 0) {
        return None; // binary
    }
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let (text, truncated) = truncate(text.trim_end(), MAX_INSTRUCTION_FILE_BYTES);
    Some(if truncated {
        format!("{text}\n\n(truncated)")
    } else {
        text
    })
}

/// First `max` bytes on a char boundary, with a flag when cut.
fn truncate(text: &str, max: usize) -> (String, bool) {
    if text.len() <= max {
        return (text.to_string(), false);
    }
    let mut cut = max;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    (format!("{}…", &text[..cut]), true)
}

fn is_git_repo(root: &Path) -> bool {
    git_output(root, &["rev-parse", "--is-inside-work-tree"])
        .map(|out| out.trim() == "true")
        .unwrap_or(false)
}

/// Run git in `root`, returning lossy stdout on success.
fn git_output(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn repo() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        git(tmp.path(), &["init", "--quiet"]);
        git(tmp.path(), &["config", "user.email", "t@t.local"]);
        git(tmp.path(), &["config", "user.name", "T"]);
        std::fs::write(tmp.path().join("app.rs"), "fn main() {}\n").unwrap();
        git(tmp.path(), &["add", "--all"]);
        git(tmp.path(), &["commit", "--quiet", "-m", "baseline"]);
        tmp
    }

    #[test]
    fn clean_repo_without_instructions_has_no_context() {
        let tmp = repo();
        assert!(generate(tmp.path()).is_none());
    }

    #[test]
    fn captures_status_diff_untracked_files_and_instructions() {
        let tmp = repo();
        let root = tmp.path();
        std::fs::write(root.join("app.rs"), "fn main() { println!(\"prior\"); }\n").unwrap();
        std::fs::write(root.join("plan.md"), "next steps\n").unwrap();
        std::fs::write(root.join("CLAUDE.md"), "# Claude notes\n\nUse tabs.\n").unwrap();

        let text = generate(root).expect("context");
        assert!(text.contains("# Prior agent context"), "{text}");
        assert!(text.contains("## Working tree status"), "{text}");
        assert!(text.contains("## Uncommitted changes"), "{text}");
        assert!(
            text.contains("+fn main() { println!(\"prior\"); }"),
            "{text}"
        );
        assert!(text.contains("## Untracked files"), "{text}");
        assert!(text.contains("plan.md"), "{text}");
        assert!(text.contains("## Agent instruction files"), "{text}");
        assert!(text.contains("Use tabs."), "{text}");
        assert!(
            !text.contains("next steps"),
            "untracked file contents must not be copied: {text}"
        );
    }

    #[test]
    fn instructions_are_available_without_git_changes() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("AGENTS.md"), "Run the fast tests.\n").unwrap();
        let text = generate(tmp.path()).expect("instruction-only context");
        assert!(text.contains("AGENTS.md"), "{text}");
        assert!(text.contains("Run the fast tests."), "{text}");
        assert!(
            text.contains("No uncommitted changes were detected"),
            "{text}"
        );
    }

    #[test]
    fn install_is_excluded_from_git_and_readable_from_the_workspace() {
        let tmp = repo();
        let root = tmp.path();
        std::fs::write(root.join("app.rs"), "fn main() { println!(\"prior\"); }\n").unwrap();
        std::fs::write(root.join("CLAUDE.md"), "Use tabs.\n").unwrap();

        let ws = tempfile::tempdir().unwrap();
        git(ws.path(), &["init", "--quiet"]);
        install(ws.path(), root).unwrap();

        assert!(ws.path().join(CONTEXT_FILE).is_file());
        let text = read(ws.path()).expect("readable context");
        assert!(text.contains("Use tabs."), "{text}");

        // Like the codebase map, the scaffold is invisible to this repo.
        let status = Command::new("git")
            .arg("-C")
            .arg(ws.path())
            .args(["status", "--porcelain"])
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&status.stdout).trim().is_empty());
        let exclude =
            std::fs::read_to_string(ws.path().join(".git").join("info").join("exclude")).unwrap();
        assert_eq!(exclude.matches(CONTEXT_FILE).count(), 1, "{exclude}");

        // install is idempotent (no duplicate exclude entries).
        install(ws.path(), root).unwrap();
        let exclude =
            std::fs::read_to_string(ws.path().join(".git").join("info").join("exclude")).unwrap();
        assert_eq!(exclude.matches(CONTEXT_FILE).count(), 1, "{exclude}");
    }

    #[test]
    fn snapshot_matches_tracks_source_and_workspace_content() {
        let tmp = repo();
        let root = tmp.path();
        let ws = tempfile::tempdir().unwrap();
        std::fs::write(root.join("app.rs"), "fn main() { /* prior */ }\n").unwrap();
        std::fs::write(root.join("CLAUDE.md"), "Use tabs.\n").unwrap();
        std::fs::copy(root.join("app.rs"), ws.path().join("app.rs")).unwrap();
        std::fs::copy(root.join("CLAUDE.md"), ws.path().join("CLAUDE.md")).unwrap();

        assert!(snapshot_matches(ws.path(), root));
        // The source moved on: the workspace no longer contains it.
        std::fs::write(root.join("app.rs"), "fn main() { /* newer */ }\n").unwrap();
        assert!(!snapshot_matches(ws.path(), root));
        // Restore the match, then change the workspace instead.
        std::fs::write(root.join("app.rs"), "fn main() { /* prior */ }\n").unwrap();
        std::fs::write(ws.path().join("app.rs"), "fn main() { /* agent */ }\n").unwrap();
        assert!(!snapshot_matches(ws.path(), root));
    }

    #[test]
    fn output_is_bounded() {
        let tmp = repo();
        let root = tmp.path();
        let huge = "x".repeat(100 * 1024);
        std::fs::write(root.join("app.rs"), &huge).unwrap();
        std::fs::write(root.join("CLAUDE.md"), "#".repeat(20 * 1024)).unwrap();
        let text = generate(root).expect("context");
        assert!(text.len() <= MAX_BYTES + 32, "len={}", text.len());
    }
}
