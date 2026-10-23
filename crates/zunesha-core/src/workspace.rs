//! Safe isolation of per-task work environments.
//!
//! Every task that starts work gets its own copy of the repository under
//! `.zou/ws/<task_id>`, on its own git branch. Agents never touch
//! the user's real working tree; their changes are captured as a diff (or a
//! branch) and only reach the main project after human approval — the single
//! write performed by [`crate::integrator`].
//!
//! Layout of a workspace:
//!
//! ```text
//! .zou/ws/<task_id>/
//! ├── <copy of the repo, respecting .gitignore>
//! ├── CODEBASE_MAP.md        # Phase 10 orientation tree; git-excluded
//! └── .git/                  # fresh repo; baseline commit; branch zou/<task_id>
//! ```

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use crate::codebase_map;
use crate::db::{Database, WorkspaceRecord};
use crate::error::{Error, Result};
use crate::paths::ZuneshaPaths;
use crate::task::TaskStatus;

/// Branch prefix for task workspaces.
pub const BRANCH_PREFIX: &str = "zou";

/// Identity used for the baseline commits created inside workspaces, so
/// agent activity never borrows the user's git identity.
const WORKSPACE_GIT_NAME: &str = "zunesha";
const WORKSPACE_GIT_EMAIL: &str = "zunesha@localhost";

/// Result of preparing a workspace for automatic conflict resolution (Phase 9).
///
/// Returned by [`WorkspaceManager::prepare_conflict_resolution`] after the
/// 3-way merge has been performed inside the workspace.
#[derive(Debug, Clone)]
pub struct ConflictPrep {
    /// Files left unmerged (carrying conflict markers) by the 3-way merge.
    /// Empty when the merge auto-resolved without overlap.
    pub conflicted_files: Vec<String>,
    /// The updated base commit (`U'`) the workspace now diffs against: a
    /// snapshot of the repository's content at conflict-detection time.
    pub new_base_commit: String,
    /// Whether the merge left conflict markers for the agent to resolve.
    pub has_markers: bool,
}

/// Creates, inspects, and tears down isolated task workspaces.
pub struct WorkspaceManager {
    paths: ZuneshaPaths,
}

impl WorkspaceManager {
    /// Bind a manager to an initialized installation.
    pub fn new(paths: ZuneshaPaths) -> Self {
        Self { paths }
    }

    /// The installation this manager operates on.
    pub fn paths(&self) -> &ZuneshaPaths {
        &self.paths
    }

    /// Spin up an isolated workspace for `task_id`.
    ///
    /// Steps:
    /// 1. Validate the task exists and is claimed (`TODO`) — a `DONE` or
    ///    `BACKLOG` task must not have live workspaces.
    /// 2. Copy the current repo state (tracked + untracked, excluding anything
    ///    `.gitignore`d, plus `.git` and `.zou`) into
    ///    `.zou/ws/<task_id>`.
    /// 3. `git init` there, write the Phase 10 `CODEBASE_MAP.md` (excluded
    ///    from this repo only, so it never enters the baseline or a diff),
    ///    commit the snapshot as a baseline, and check out a dedicated branch
    ///    `zou/<task_id>`.
    /// 4. Record the workspace in the database.
    ///
    /// Idempotent: if the workspace already exists and is recorded, it is
    /// returned unchanged (upgrading a pre-Phase 10 workspace with the map if
    /// it lacks one). The caller is responsible for moving the task to
    /// `IN_PROGRESS`.
    pub fn create(&self, db: &Database, task_id: &str) -> Result<WorkspaceRecord> {
        validate_task_id(task_id)?;

        let task = db.require_task(task_id)?;
        if !matches!(task.status, TaskStatus::Todo | TaskStatus::InProgress) {
            return Err(Error::Workspace(format!(
                "task {task_id} is {} (must be TODO or IN_PROGRESS to have a workspace)",
                task.status
            )));
        }

        // Idempotent short-circuit.
        if let Some(existing) = db.get_workspace(task_id)? {
            let ws = Path::new(&existing.path);
            if ws.is_dir() {
                // Self-heal: workspaces from before Phase 10 get the map on
                // first reuse (install is idempotent and cheap).
                if !ws.join(codebase_map::MAP_FILE).is_file() {
                    codebase_map::install(ws, self.paths.root())?;
                }
                return Ok(existing);
            }
            // Row without a directory (e.g. manual cleanup) — fall through and
            // rebuild, then refresh the row.
        }

        let ws_dir = self.paths.workspace(task_id);
        if ws_dir.exists() {
            // Stale directory from a previous run; start clean.
            std::fs::remove_dir_all(&ws_dir)?;
        }
        std::fs::create_dir_all(&ws_dir)?;

        // 2. Copy the repository snapshot.
        copy_repo_snapshot(self.paths.root(), &ws_dir)?;

        // 3. Fresh git repo with a baseline commit on a task branch.
        git(&ws_dir, &["init", "--quiet"])?;
        git(&ws_dir, &["config", "user.name", WORKSPACE_GIT_NAME])?;
        git(&ws_dir, &["config", "user.email", WORKSPACE_GIT_EMAIL])?;
        // Phase 10: orientation map, written before staging so the exclude
        // rule keeps it out of the baseline (scaffolding, never the code).
        codebase_map::install(&ws_dir, self.paths.root())?;
        git(&ws_dir, &["add", "--all"])?;
        // `--allow-empty` keeps this robust for (near-)empty repositories.
        git(
            &ws_dir,
            &[
                "commit",
                "--quiet",
                "--allow-empty",
                "-m",
                &baseline_message(task_id),
            ],
        )?;
        let base_commit = git_stdout(&ws_dir, &["rev-parse", "HEAD"])?;
        let branch = branch_name(task_id);
        git(&ws_dir, &["checkout", "--quiet", "-b", &branch])?;

        // 4. Persist the record.
        let record = WorkspaceRecord {
            task_id: task_id.to_string(),
            path: ws_dir.to_string_lossy().into_owned(),
            branch,
            base_commit,
        };
        db.upsert_workspace(&record)?;
        Ok(record)
    }

    /// Capture the work done in a task's workspace as a unified diff.
    ///
    /// Stages everything first (`git add --all`) so newly created files are
    /// included, then diffs the workspace against the recorded baseline
    /// commit. Returns an empty string when nothing has changed.
    ///
    /// `--binary` matters for integration: without it a changed binary file
    /// degrades to a `Binary files … differ` marker that `git apply` cannot
    /// replay, which would make the task unapprovable.
    pub fn generate_diff(&self, db: &Database, task_id: &str) -> Result<String> {
        let record = db
            .get_workspace(task_id)?
            .ok_or_else(|| Error::Workspace(format!("no workspace recorded for task {task_id}")))?;
        let ws_dir = Path::new(&record.path);
        if !ws_dir.is_dir() {
            return Err(Error::Workspace(format!(
                "workspace directory missing for task {task_id}: {}",
                record.path
            )));
        }

        // Make sure even untracked files show up in the diff.
        git(ws_dir, &["add", "--all"])?;
        // Phase 10: scaffolding must never reach the reviewed diff, even if
        // an agent force-added the map past the exclude rule.
        untrack_map(ws_dir)?;
        let output = git(
            ws_dir,
            &["diff", "--cached", "--binary", &record.base_commit, "--"],
        )?;
        // Raw bytes, deliberately unlike `git_stdout`: trimming would swallow
        // the patch's final newline (and any trailing whitespace on its last
        // line), which `git apply` then reads as a corrupt patch. `git diff`
        // prints nothing at all when there are no changes, so "empty output"
        // still means "no changes".
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Rebase a conflicted workspace onto the repository's current state so an
    /// agent can resolve the overlap (Phase 9 auto-merge).
    ///
    /// When an approved diff no longer applies cleanly, the repository has
    /// moved since the workspace was snapshotted. Rather than fail, this
    /// performs a real 3-way merge *inside the workspace* so the agent sees
    /// actual conflict markers and can reconcile them:
    ///
    /// 1. Commit the agent's pending work as `A` (a child of the recorded
    ///    base `B`). A pending `MERGE_HEAD` from an earlier round is completed
    ///    by this commit, keeping the invariant "HEAD descends from the base".
    /// 2. Build `U'` — a commit whose tree is the repository's **current**
    ///    content and whose parent is `B` — on a throwaway branch.
    /// 3. Merge `U'` into the task branch. Where `A` and `U'` both changed the
    ///    same region of `B`, git writes conflict markers into the working
    ///    tree; where only one side changed, that change is taken as-is.
    /// 4. Repoint the workspace record's `base_commit` at `U'`, so the diff
    ///    the human finally approves is computed against the *new* base and
    ///    applies cleanly to the repository.
    ///
    /// The user's real working tree is never touched: the merge happens
    /// entirely in `.zou/ws/<task_id>`. When the merge conflicts,
    /// `MERGE_HEAD` may remain (the agent resolves it); diff generation only
    /// runs `git add`/`git diff --cached`, which are unaffected by merge state.
    pub fn prepare_conflict_resolution(
        &self,
        db: &Database,
        task_id: &str,
    ) -> Result<ConflictPrep> {
        let record = db
            .get_workspace(task_id)?
            .ok_or_else(|| Error::Workspace(format!("no workspace recorded for task {task_id}")))?;
        let ws_dir = PathBuf::from(&record.path);
        if !ws_dir.is_dir() {
            return Err(Error::Workspace(format!(
                "workspace directory missing for task {task_id}: {}",
                record.path
            )));
        }
        let base = record.base_commit.clone();
        let branch = record.branch.clone();

        // 1. Ensure the agent's work is committed as `A` (child of `B`).
        git(&ws_dir, &["add", "--all"])?;
        // Same Phase 10 guard as diff generation: a force-added map must not
        // enter the commit (it would follow the task through the merge).
        untrack_map(&ws_dir)?;
        // `--allow-empty` keeps this robust when the workspace already matches
        // its base (e.g. a repeat conflict with no new edits); the merge base
        // is still correct either way, and it also completes a pending merge.
        git(
            &ws_dir,
            &[
                "commit",
                "--quiet",
                "--allow-empty",
                "-m",
                &format!("orchestrator: agent work before conflict resolution ({task_id})"),
            ],
        )?;

        // 2. Build `U'` (the repository's current content) with parent `B`.
        git(
            &ws_dir,
            &["checkout", "--quiet", "-B", "zou-user-base", &base],
        )?;
        clear_worktree_except_git(&ws_dir)?;
        copy_repo_snapshot(self.paths.root(), &ws_dir)?;
        // Phase 10: the rebuild wiped the map — regenerate it for the new
        // snapshot (the exclude entry survived in `.git/info/exclude`).
        codebase_map::install(&ws_dir, self.paths.root())?;
        git(&ws_dir, &["add", "--all"])?;
        git(
            &ws_dir,
            &[
                "commit",
                "--quiet",
                "--allow-empty",
                "-m",
                "orchestrator: updated base for conflict resolution",
            ],
        )?;
        let new_base = git_stdout(&ws_dir, &["rev-parse", "HEAD"])?;

        // 3. Merge `U'` into the task branch; conflicts leave markers in the
        //    working tree (and set MERGE_HEAD) for the agent to resolve.
        git(&ws_dir, &["checkout", "--quiet", &branch])?;
        let merged = git_allow_conflict(&ws_dir, &["merge", "--no-edit", "zou-user-base"])?;
        let has_markers = !merged.status.success();
        let conflicted_files = if has_markers {
            unmerged_files(&ws_dir)
        } else {
            Vec::new()
        };

        // 4. Repoint the recorded base at `U'` so the next diff is rebased.
        db.upsert_workspace(&WorkspaceRecord {
            base_commit: new_base.clone(),
            ..record
        })?;

        Ok(ConflictPrep {
            conflicted_files,
            new_base_commit: new_base,
            has_markers,
        })
    }

    /// Delete a task's workspace directory and database record.
    ///
    /// Called once a task's changes have been merged back into the
    /// repository — what [`crate::approve_task`] does after a successful
    /// integration. Deliberately *not* called on rejection: retries build on
    /// the previous attempt's diff instead of starting cold.
    pub fn remove(&self, db: &Database, task_id: &str) -> Result<()> {
        if let Some(record) = db.get_workspace(task_id)? {
            let ws_dir = Path::new(&record.path);
            if ws_dir.is_dir() {
                std::fs::remove_dir_all(ws_dir)?;
            }
        }
        db.delete_workspace(task_id)?;
        Ok(())
    }
}

/// Result of [`WorkspaceManager::create`], plus the branch/base info the
/// integrator needs later.
///
/// (Kept as a thin alias so callers can name the type without importing the
/// DB row type directly.)
pub type Workspace = WorkspaceRecord;

/// The git branch dedicated to a task.
pub fn branch_name(task_id: &str) -> String {
    format!("{BRANCH_PREFIX}/{task_id}")
}

/// Validate a task id before using it as a path component.
fn validate_task_id(task_id: &str) -> Result<()> {
    let ok = !task_id.is_empty()
        && !task_id.starts_with('.')
        && task_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if ok {
        Ok(())
    } else {
        Err(Error::Workspace(format!(
            "invalid task id {task_id:?}: use only [A-Za-z0-9._-] and do not start with '.'"
        )))
    }
}

fn baseline_message(task_id: &str) -> String {
    format!("orchestrator: baseline snapshot for task {task_id}")
}

/// Drop the orientation map from the workspace index if it got tracked.
///
/// `codebase_map::install` excludes `CODEBASE_MAP.md` from ordinary adds,
/// but an agent can still force it in (`git add -f`), and a tracked map
/// would leak into diffs, commits, and merges. `--ignore-unmatch` makes this
/// a cheap no-op in the normal untracked case; the working-tree file stays
/// (and stays ignored).
fn untrack_map(ws_dir: &Path) -> Result<()> {
    git(
        ws_dir,
        &[
            "rm",
            "--cached",
            "--quiet",
            "--ignore-unmatch",
            codebase_map::MAP_FILE,
        ],
    )?;
    Ok(())
}

/// Copy the repository's current state into `dest`, honoring `.gitignore`.
///
/// Source list comes from `git ls-files --cached --others --exclude-standard`:
/// tracked files plus untracked files that are not ignored. `.git` and
/// `.zou` are always excluded. Symlinks are replicated as symlinks
/// (on unix); the executable bit is preserved.
fn copy_repo_snapshot(repo_root: &Path, dest: &Path) -> Result<usize> {
    // In a plain git work tree this is the accurate list. Outside one (only
    // reachable via `zou init --force`) fall back to a manual walk that
    // skips `.git`/`.zou`.
    let files = match git_stdout_opt(
        repo_root,
        &[
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ],
    )? {
        Some(listing) => listing
            .split('\0')
            .filter(|s| !s.is_empty())
            .filter(|rel| !rel.starts_with(".zou/") && *rel != ".zou")
            .map(|s| s.to_string())
            .collect::<Vec<_>>(),
        None => collect_files_fallback(repo_root)?,
    };

    for rel in &files {
        let src = repo_root.join(rel);
        let dst = dest.join(rel);
        if !src.exists() && !is_symlink(&src) {
            continue; // e.g. deleted-but-still-listed path
        }
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        copy_one(&src, &dst)?;
    }
    Ok(files.len())
}

/// Copy a single file or symlink, preserving the executable bit.
fn copy_one(src: &Path, dst: &Path) -> Result<()> {
    let meta = std::fs::symlink_metadata(src)?;
    if meta.file_type().is_symlink() {
        copy_symlink(src, dst)?;
        return Ok(());
    }
    std::fs::copy(src, dst)?;
    std::fs::set_permissions(dst, meta.permissions())?;
    Ok(())
}

#[cfg(unix)]
fn copy_symlink(src: &Path, dst: &Path) -> Result<()> {
    let target = std::fs::read_link(src)?;
    match std::os::unix::fs::symlink(&target, dst) {
        Ok(()) => Ok(()),
        // Cross-device or dangling links can fail; degrade to copying the
        // target's contents when possible, else skip harmlessly.
        Err(_) if src.exists() => {
            std::fs::copy(src, dst)?;
            Ok(())
        }
        Err(_) => Ok(()),
    }
}

#[cfg(not(unix))]
fn copy_symlink(_src: &Path, _dst: &Path) -> Result<()> {
    Ok(())
}

fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

/// Fallback file walk for non-git (`--force`) installations: everything
/// except `.git`, `.zou`, and hidden VCS/metadata dirs.
fn collect_files_fallback(root: &Path) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut stack = vec![PathBuf::from("")];
    while let Some(rel) = stack.pop() {
        let dir = root.join(&rel);
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == ".git" || name == ".zou" {
                continue;
            }
            let child_rel = if rel.as_os_str().is_empty() {
                PathBuf::from(&name)
            } else {
                rel.join(&name)
            };
            let meta = std::fs::symlink_metadata(entry.path())?;
            if meta.is_dir() {
                stack.push(child_rel);
            } else {
                out.push(child_rel.to_string_lossy().into_owned());
            }
        }
    }
    Ok(out)
}

/// Run a git command, returning an error (with stderr) on failure.
fn git(dir: &Path, args: &[&str]) -> Result<Output> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| Error::Git(format!("failed to run `git {args:?}`: {e}")))?;
    if !output.status.success() {
        return Err(Error::Git(format!(
            "`git {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(output)
}

/// Run a git command and capture trimmed stdout.
fn git_stdout(dir: &Path, args: &[&str]) -> Result<String> {
    let output = git(dir, args)?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Like [`git_stdout`] but returns `Ok(None)` when the command fails (used to
/// probe "are we in a git repo?").
fn git_stdout_opt(dir: &Path, args: &[&str]) -> Result<Option<String>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| Error::Git(format!("failed to run `git`: {e}")))?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&output.stdout).into_owned()))
}

/// Run a git command, returning its output even on a non-zero exit.
///
/// Used for `git merge`, which legitimately exits non-zero when it conflicts.
/// Only spawn failures are errors; the caller inspects `output.status`.
fn git_allow_conflict(dir: &Path, args: &[&str]) -> Result<Output> {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| Error::Git(format!("failed to run `git {args:?}`: {e}")))
}

/// List files left unmerged after a conflicted merge (`git diff --diff-filter=U`).
///
/// Returns an empty list when the merge completed cleanly or git is unavailable.
fn unmerged_files(dir: &Path) -> Vec<String> {
    let Ok(output) = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["diff", "--name-only", "--diff-filter=U"])
        .stdin(std::process::Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| line.to_string())
        .collect()
}

/// Remove everything in `dir` **except** `.git`, so a fresh base snapshot can
/// be copied in without leaving stale files from the previous base.
///
/// This is what guarantees the rebuilt `U'` tree exactly matches the
/// repository: `copy_repo_snapshot` only ever copies/overwrites, never deletes.
fn clear_worktree_except_git(dir: &Path) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == std::ffi::OsStr::new(".git") {
            continue;
        }
        let path = entry.path();
        // symlink_metadata: a symlink to a directory must be removed as a link,
        // not followed and deleted through.
        let meta = std::fs::symlink_metadata(&path)?;
        if meta.is_dir() {
            std::fs::remove_dir_all(&path)?;
        } else {
            std::fs::remove_file(&path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::Task;

    /// Build a scratch repo with: a committed file, an untracked-but-not-ignored
    /// file, and an ignored file.
    fn scratch_repo() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let run = |args: &[&str]| {
            let st = Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .status()
                .unwrap();
            assert!(st.success(), "git {args:?} should succeed");
        };
        run(&["init", "--quiet"]);
        run(&["config", "user.email", "t@t.local"]);
        run(&["config", "user.name", "T"]);

        std::fs::write(root.join("app.rs"), "fn main() {}\n").unwrap();
        std::fs::write(root.join("README.md"), "# demo\n").unwrap();
        std::fs::write(root.join(".gitignore"), "build/\nsecret.txt\n").unwrap();
        std::fs::create_dir_all(root.join("build")).unwrap();
        std::fs::write(root.join("build").join("artifact.bin"), "junk").unwrap();
        std::fs::write(root.join("secret.txt"), "hunter2").unwrap();
        run(&["add", "--all"]);
        run(&["commit", "--quiet", "-m", "baseline"]);
        // Untracked but NOT ignored: must be copied into workspaces.
        std::fs::write(root.join("notes.md"), "scratch notes\n").unwrap();
        tmp
    }

    fn git_captured(dir: &Path, args: &[&str]) -> String {
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
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[test]
    fn create_workspace_copies_repo_respecting_gitignore() {
        let repo = scratch_repo();
        let db = Database::open_in_memory().unwrap();
        // Point the installation at the scratch repo root.
        let paths = ZuneshaPaths::new(repo.path());
        std::fs::create_dir_all(paths.workspaces_dir()).unwrap();
        let mgr = WorkspaceManager::new(paths);

        db.insert_task(&Task::new("t1", "t", "")).unwrap();
        db.set_status("t1", TaskStatus::Todo).unwrap();

        let record = mgr.create(&db, "t1").unwrap();
        let ws = Path::new(&record.path);

        // Tracked files copied.
        assert!(ws.join("app.rs").is_file());
        assert!(ws.join("README.md").is_file());
        // Untracked-but-not-ignored files copied.
        assert!(ws.join("notes.md").is_file());
        // Ignored files NOT copied.
        assert!(
            !ws.join("build").exists(),
            ".gitignore'd dir must not be copied"
        );
        assert!(
            !ws.join("secret.txt").exists(),
            ".gitignore'd file must not be copied"
        );
        // Control dir and VCS metadata not copied.
        assert!(!ws.join(".zou").exists(), "control dir must not leak");
        assert!(ws.join(".git").is_dir(), "workspace has its own git repo");

        // On its own branch with a baseline commit.
        assert_eq!(
            git_captured(ws, &["rev-parse", "--abbrev-ref", "HEAD"]),
            "zou/t1"
        );
        assert!(!record.base_commit.is_empty());
        assert_eq!(git_captured(ws, &["rev-parse", "HEAD"]), record.base_commit);
    }

    #[test]
    fn create_is_idempotent_and_recorded_in_db() {
        let repo = scratch_repo();
        let db = Database::open_in_memory().unwrap();
        let paths = ZuneshaPaths::new(repo.path());
        std::fs::create_dir_all(paths.workspaces_dir()).unwrap();
        let mgr = WorkspaceManager::new(paths);

        db.insert_task(&Task::new("t1", "t", "")).unwrap();
        db.set_status("t1", TaskStatus::Todo).unwrap();

        let first = mgr.create(&db, "t1").unwrap();
        let second = mgr.create(&db, "t1").unwrap();
        assert_eq!(first.path, second.path);
        assert_eq!(first.base_commit, second.base_commit);
        assert!(db.get_workspace("t1").unwrap().is_some());
    }

    #[test]
    fn create_rejects_wrong_status_and_missing_task() {
        let repo = scratch_repo();
        let db = Database::open_in_memory().unwrap();
        let paths = ZuneshaPaths::new(repo.path());
        std::fs::create_dir_all(paths.workspaces_dir()).unwrap();
        let mgr = WorkspaceManager::new(paths);

        // Missing task.
        assert!(mgr.create(&db, "ghost").is_err());

        // BACKLOG task cannot claim a workspace.
        db.insert_task(&Task::new("t1", "t", "")).unwrap();
        assert!(mgr.create(&db, "t1").is_err());

        // DONE task cannot claim a workspace either.
        // force: BACKLOG -> DONE shortcut for setup.
        db.force_set_status("t1", TaskStatus::Done).unwrap();
        assert!(mgr.create(&db, "t1").is_err());
    }

    #[test]
    fn create_rejects_path_traversal_task_ids() {
        let repo = scratch_repo();
        let db = Database::open_in_memory().unwrap();
        let paths = ZuneshaPaths::new(repo.path());
        std::fs::create_dir_all(paths.workspaces_dir()).unwrap();
        let mgr = WorkspaceManager::new(paths);

        for bad in ["../evil", "a/b", ".hidden", ""] {
            assert!(
                mgr.create(&db, bad).is_err(),
                "task id {bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn generate_diff_is_empty_then_captures_changes() {
        let repo = scratch_repo();
        let db = Database::open_in_memory().unwrap();
        let paths = ZuneshaPaths::new(repo.path());
        std::fs::create_dir_all(paths.workspaces_dir()).unwrap();
        let mgr = WorkspaceManager::new(paths);

        db.insert_task(&Task::new("t1", "t", "")).unwrap();
        db.set_status("t1", TaskStatus::Todo).unwrap();
        let record = mgr.create(&db, "t1").unwrap();
        let ws = Path::new(&record.path);

        // Baseline: nothing has changed yet.
        assert!(mgr.generate_diff(&db, "t1").unwrap().is_empty());

        // Simulate agent work: modify a tracked file, add a new one.
        std::fs::write(ws.join("app.rs"), "fn main() { println!(\"hi\"); }\n").unwrap();
        std::fs::write(ws.join("feature.rs"), "pub fn feature() {}\n").unwrap();

        let diff = mgr.generate_diff(&db, "t1").unwrap();
        assert!(diff.contains("app.rs"), "modified file must appear: {diff}");
        assert!(diff.contains("feature.rs"), "new file must appear: {diff}");
        assert!(
            diff.contains("+fn main() { println!(\"hi\"); }"),
            "hunk body: {diff}"
        );
        assert!(
            diff.contains("+pub fn feature() {}"),
            "new file hunk: {diff}"
        );
    }

    #[test]
    fn generate_diff_requires_workspace() {
        let repo = scratch_repo();
        let db = Database::open_in_memory().unwrap();
        let paths = ZuneshaPaths::new(repo.path());
        std::fs::create_dir_all(paths.workspaces_dir()).unwrap();
        let mgr = WorkspaceManager::new(paths);

        db.insert_task(&Task::new("t1", "t", "")).unwrap();
        db.set_status("t1", TaskStatus::Todo).unwrap();
        assert!(mgr.generate_diff(&db, "t1").is_err(), "no workspace yet");
    }

    #[test]
    fn remove_deletes_directory_and_record() {
        let repo = scratch_repo();
        let db = Database::open_in_memory().unwrap();
        let paths = ZuneshaPaths::new(repo.path());
        std::fs::create_dir_all(paths.workspaces_dir()).unwrap();
        let mgr = WorkspaceManager::new(paths);

        db.insert_task(&Task::new("t1", "t", "")).unwrap();
        db.set_status("t1", TaskStatus::Todo).unwrap();
        let record = mgr.create(&db, "t1").unwrap();
        assert!(Path::new(&record.path).is_dir());

        mgr.remove(&db, "t1").unwrap();
        assert!(!Path::new(&record.path).exists());
        assert!(db.get_workspace("t1").unwrap().is_none());
        // Removing again is a harmless no-op.
        mgr.remove(&db, "t1").unwrap();
    }

    #[test]
    fn branch_name_uses_prefix() {
        assert_eq!(branch_name("task-1-0"), "zou/task-1-0");
    }

    // --------------------------- Phase 9: prepare_conflict_resolution

    /// Scratch install with a task whose workspace already holds agent work
    /// (app.rs extended), parked in `IN_PROGRESS` — the state a workspace is
    /// in when its approval later hits a conflict.
    fn agent_workspace() -> (tempfile::TempDir, ZuneshaPaths, Database) {
        let repo = scratch_repo();
        let paths = ZuneshaPaths::new(repo.path());
        std::fs::create_dir_all(paths.workspaces_dir()).unwrap();
        let db = Database::open_in_memory().unwrap();
        db.insert_task(&Task::new("t1", "t", "")).unwrap();
        db.set_status("t1", TaskStatus::Todo).unwrap();
        db.set_status("t1", TaskStatus::InProgress).unwrap();
        WorkspaceManager::new(paths.clone())
            .create(&db, "t1")
            .unwrap();
        std::fs::write(
            paths.workspace("t1").join("app.rs"),
            "fn main() { println!(\"done\"); }\n",
        )
        .unwrap();
        (repo, paths, db)
    }

    /// `git apply --check` the given patch against the repository — the exact
    /// dry run [`crate::approve_task`] performs when the human approves.
    fn applies_cleanly(root: &Path, patch: &str) -> std::result::Result<(), String> {
        use std::io::Write as _;
        let mut child = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["apply", "--check", "-"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(patch.as_bytes())
            .map_err(|e| e.to_string())?;
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).to_string())
        }
    }

    #[test]
    fn prepare_rebases_onto_the_new_base_when_changes_do_not_overlap() {
        let (repo, paths, db) = agent_workspace();
        let before = db.get_workspace("t1").unwrap().unwrap();

        // The repository moves on without touching app.rs: README edited AND
        // notes.md deleted (deletions are why the old base must be cleared,
        // not merely overwritten).
        std::fs::write(repo.path().join("README.md"), "# demo (revised)\n").unwrap();
        std::fs::remove_file(repo.path().join("notes.md")).unwrap();

        let mgr = WorkspaceManager::new(paths.clone());
        let prep = mgr.prepare_conflict_resolution(&db, "t1").unwrap();

        // Non-overlapping sides merge on their own: no markers, no blame...
        assert!(!prep.has_markers, "auto-merge leaves nothing to resolve");
        assert!(prep.conflicted_files.is_empty());

        // ...but the workspace is still rebased onto the new snapshot.
        assert_ne!(prep.new_base_commit, before.base_commit);
        assert_eq!(
            db.get_workspace("t1").unwrap().unwrap().base_commit,
            prep.new_base_commit
        );
        let ws = paths.workspace("t1");
        assert_eq!(
            std::fs::read_to_string(ws.join("README.md")).unwrap(),
            "# demo (revised)\n",
            "the user's edit absorbed"
        );
        assert!(
            !ws.join("notes.md").exists(),
            "the user's deletion absorbed, not left stale"
        );
        assert_eq!(
            std::fs::read_to_string(ws.join("app.rs")).unwrap(),
            "fn main() { println!(\"done\"); }\n",
            "the agent's work survived the rebase"
        );
        assert!(
            !ws.join("secret.txt").exists(),
            "gitignore rules still respected in the rebuilt base"
        );
        // The user's own tree is never touched by the rebase: it is exactly
        // the state the user left it in (their edit applied, their deletion
        // applied, and app.rs — the contested file — untouched by them).
        assert_eq!(
            std::fs::read_to_string(repo.path().join("README.md")).unwrap(),
            "# demo (revised)\n"
        );
        assert!(
            !repo.path().join("notes.md").exists(),
            "their deletion stands"
        );
        assert_eq!(
            std::fs::read_to_string(repo.path().join("app.rs")).unwrap(),
            "fn main() {}\n"
        );

        // The rebased diff is what the next approval will replay — and it now
        // applies cleanly to the repository's current content.
        let diff = mgr.generate_diff(&db, "t1").unwrap();
        assert!(diff.contains("fn main()"), "agent work present: {diff}");
        applies_cleanly(repo.path(), &diff).expect("rebased diff applies cleanly");
    }

    #[test]
    fn prepare_leaves_markers_and_blames_the_overlapping_file() {
        let (repo, paths, db) = agent_workspace();
        let before = db.get_workspace("t1").unwrap().unwrap();

        // The user edits the very line the agent edited.
        let diverged = "fn main() { /* user was here */ }\n";
        std::fs::write(repo.path().join("app.rs"), diverged).unwrap();

        let mgr = WorkspaceManager::new(paths.clone());
        let prep = mgr.prepare_conflict_resolution(&db, "t1").unwrap();

        assert!(prep.has_markers, "overlapping edits must conflict");
        assert_eq!(prep.conflicted_files, vec!["app.rs".to_string()]);
        assert_ne!(prep.new_base_commit, before.base_commit);
        assert_eq!(
            db.get_workspace("t1").unwrap().unwrap().base_commit,
            prep.new_base_commit
        );

        // Real markers in the workspace — what the resolution agent reads...
        let merged = std::fs::read_to_string(paths.workspace("t1").join("app.rs")).unwrap();
        assert!(
            merged.contains("<<<<<<<") && merged.contains(">>>>>>>"),
            "standard conflict markers: {merged}"
        );
        assert!(
            merged.contains("user was here"),
            "the base side of the conflict is visible"
        );
        // ...and MERGE_HEAD stays open for the agent to complete.
        assert!(
            paths
                .workspace("t1")
                .join(".git")
                .join("MERGE_HEAD")
                .is_file(),
            "the merge is left in progress on purpose"
        );

        // The user's real tree is byte-identical to before the attempt.
        assert_eq!(
            std::fs::read_to_string(repo.path().join("app.rs")).unwrap(),
            diverged
        );
    }

    // --------------------------- Phase 10: codebase map

    #[test]
    fn create_installs_the_map_and_keeps_it_out_of_git() {
        let repo = scratch_repo();
        let db = Database::open_in_memory().unwrap();
        let paths = ZuneshaPaths::new(repo.path());
        std::fs::create_dir_all(paths.workspaces_dir()).unwrap();
        let mgr = WorkspaceManager::new(paths);

        db.insert_task(&Task::new("t1", "t", "")).unwrap();
        db.set_status("t1", TaskStatus::Todo).unwrap();
        let record = mgr.create(&db, "t1").unwrap();
        let ws = Path::new(&record.path);

        // The map exists and describes the snapshot, copied files included.
        let map = std::fs::read_to_string(ws.join(codebase_map::MAP_FILE)).unwrap();
        assert!(map.contains("# Codebase map"), "{map}");
        assert!(map.contains("README.md (1 line)"), "{map}");
        assert!(
            map.contains("notes.md"),
            "untracked-but-copied files too: {map}"
        );

        // Invisible to git: status clean, and absent from the baseline, so
        // the very first diff is empty.
        assert_eq!(git_captured(ws, &["status", "--porcelain"]), "");
        assert!(mgr.generate_diff(&db, "t1").unwrap().is_empty());

        // Even a force-add cannot smuggle the map into a reviewed diff —
        // the untrack guard drops it again (and the file stays on disk).
        let out = Command::new("git")
            .arg("-C")
            .arg(ws)
            .args(["add", "-f", codebase_map::MAP_FILE])
            .output()
            .unwrap();
        assert!(out.status.success());
        assert!(mgr.generate_diff(&db, "t1").unwrap().is_empty());
        assert!(ws.join(codebase_map::MAP_FILE).is_file());

        // install() is idempotent: content regenerated, one exclude entry.
        codebase_map::install(ws, repo.path()).unwrap();
        codebase_map::install(ws, repo.path()).unwrap();
        let exclude =
            std::fs::read_to_string(ws.join(".git").join("info").join("exclude")).unwrap();
        assert_eq!(
            exclude.matches(codebase_map::MAP_FILE).count(),
            1,
            "{exclude}"
        );

        // Self-heal: a recorded workspace that lost its map gets one back.
        std::fs::remove_file(ws.join(codebase_map::MAP_FILE)).unwrap();
        mgr.create(&db, "t1").unwrap();
        assert!(ws.join(codebase_map::MAP_FILE).is_file());
    }

    #[test]
    fn conflict_prep_regenerates_the_map_for_the_updated_snapshot() {
        let (repo, paths, db) = agent_workspace();

        // The repository moves on: a new file enters the snapshot, which the
        // old map (written at create time) cannot know about.
        std::fs::write(repo.path().join("fresh.rs"), "fn fresh() {}\n").unwrap();

        let mgr = WorkspaceManager::new(paths.clone());
        let prep = mgr.prepare_conflict_resolution(&db, "t1").unwrap();
        assert!(!prep.has_markers, "disjoint files merge without conflict");

        let ws = paths.workspace("t1");
        let map = std::fs::read_to_string(ws.join(codebase_map::MAP_FILE)).unwrap();
        assert!(
            map.contains("fresh.rs (1 line)"),
            "regenerated for the rebuilt snapshot: {map}"
        );

        // Still exactly one exclude entry (no duplicate appends across
        // rounds), and the map still never appears in the approved diff.
        let exclude =
            std::fs::read_to_string(ws.join(".git").join("info").join("exclude")).unwrap();
        assert_eq!(
            exclude.matches(codebase_map::MAP_FILE).count(),
            1,
            "{exclude}"
        );
        let diff = mgr.generate_diff(&db, "t1").unwrap();
        assert!(
            !diff.contains(codebase_map::MAP_FILE),
            "map must never appear in a diff: {diff}"
        );
        assert!(
            diff.contains("fn main()"),
            "agent work is still the only change: {diff}"
        );
    }
}
