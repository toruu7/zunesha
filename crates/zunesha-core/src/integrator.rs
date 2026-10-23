//! Landing approved work in the user's repository — the one deliberate write.
//!
//! Every other component works inside `.zou/ws/<task_id>`; nothing an
//! agent does reaches the real project until a human approves the task.
//! [`Integrator`] performs that single write by replaying the approved diff
//! onto the working tree with `git apply`, and [`approve_task`] wraps it in
//! the full `REVIEW → DONE` workflow the API and CLI both drive.
//!
//! Design choices, in order of importance:
//!
//! * **The reviewed diff is exactly what lands.** The patch is regenerated
//!   from the workspace at approval time (falling back to the copy stored on
//!   the task row if the workspace is gone), so the bytes the reviewer saw in
//!   the dashboard are the bytes applied — no re-generation behind their back,
//!   no fuzzy auto-merging of changes nobody reviewed.
//! * **All-or-nothing.** A `git apply --check` dry run happens first; a patch
//!   that does not apply cleanly writes nothing. That conflict now kicks off
//!   **Phase 9 automatic resolution**: the workspace — which is its own git
//!   repo, so this works even for `--force` plain-directory installs — is
//!   rebased onto the repository's current content with real conflict markers
//!   and the task moves to `RESOLVING_CONFLICTS` for an agent to reconcile
//!   ([`MergeOutcome::ConflictResolutionQueued`]). Only if that rebase itself
//!   fails does it surface as [`Error::MergeConflict`] (API → `409`, CLI →
//!   non-zero exit) with the task left in `REVIEW` for manual reconciliation.
//! * **Idempotent.** If the approved changes are already in the tree (a prior
//!   attempt merged them but a later state write failed, or the human applied
//!   the patch themselves), a reverse dry run detects that and the approval
//!   completes without touching a file.
//! * **No commits, no index writes.** Only files in the working tree change:
//!   the user's index, `HEAD`, and history stay theirs to manage (`git diff`,
//!   `git add`, `git commit` whenever they are ready).
//! * **Plain directories work too.** Outside a git repo (`zou init --force`)
//!   `git apply` still resolves patch paths against the current directory, so
//!   the merge behaves the same either way.
//!
//! [`Error::MergeConflict`]: crate::error::Error::MergeConflict

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde::Serialize;

use crate::dag::promote_ready;
use crate::db::Database;
use crate::error::{Error, Result};
use crate::paths::ZuneshaPaths;
use crate::task::{Task, TaskStatus};
use crate::workspace::WorkspaceManager;

/// What became of a task's approved diff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MergeOutcome {
    /// Nothing to land: the diff is empty (the agent made no changes).
    NoChanges,
    /// The approved changes are present in the working tree — applied by this
    /// run, or already there from an earlier one.
    Applied {
        /// Files the patch touches, as reported by `git apply --numstat`.
        files: Vec<String>,
    },
    /// The approved diff conflicted with the changed repository. The merge was
    /// **aborted** (nothing written to the working tree); instead the workspace
    /// was rebased onto the repository's current content with conflict markers
    /// and the task moved to `RESOLVING_CONFLICTS`, where an agent resolves the
    /// overlap before the result re-enters validation and returns to `REVIEW`.
    ConflictResolutionQueued {
        /// Files involved in the conflict: those the 3-way merge left with
        /// conflict markers, or — when it auto-resolved — those whose patch
        /// failed to apply.
        files: Vec<String>,
    },
}

/// Merges approved task diffs back into the main repository.
///
/// One instance per installation; cheap to construct.
pub struct Integrator {
    paths: ZuneshaPaths,
}

impl Integrator {
    /// Bind an integrator to an initialized installation.
    pub fn new(paths: ZuneshaPaths) -> Self {
        Self { paths }
    }

    /// Apply `task_id`'s approved diff to the main working tree.
    ///
    /// Deliberately does **not** change task state — that is
    /// [`approve_task`]'s job — so the merge itself can be tested and reasoned
    /// about in isolation. On conflict returns [`Error::MergeConflict`] with
    /// the files git blamed; nothing is written in that case.
    pub fn integrate(&self, db: &Database, task_id: &str) -> Result<MergeOutcome> {
        let patch = self.approved_patch(db, task_id)?;
        if patch.trim().is_empty() {
            return Ok(MergeOutcome::NoChanges);
        }
        apply_patch(self.paths.root(), task_id, &patch)
    }

    /// The patch to land: live from the workspace when it still exists,
    /// otherwise the reviewed copy stored on the task row.
    ///
    /// The two are byte-identical in normal operation (the worker regenerates
    /// the stored copy from the same workspace at the end of each run); the
    /// fallback exists so an approved task survives workspace cleanup.
    fn approved_patch(&self, db: &Database, task_id: &str) -> Result<String> {
        let mgr = WorkspaceManager::new(self.paths.clone());
        match db.get_workspace(task_id)? {
            Some(record) if Path::new(&record.path).is_dir() => mgr.generate_diff(db, task_id),
            _ => Ok(db.require_task(task_id)?.diff),
        }
    }
}

/// Result of [`approve_task`].
#[derive(Debug)]
pub struct ApproveOutcome {
    /// The task as stored after the transition — `DONE` on a clean land, or
    /// `RESOLVING_CONFLICTS` when the diff conflicted and auto-resolution was
    /// queued.
    pub task: Task,
    /// Tasks the DAG engine unlocked as a consequence (empty on conflict).
    pub promoted: Vec<String>,
    /// What happened to the approved diff.
    pub integration: MergeOutcome,
}

/// The human-in-the-loop gate: land the approved diff, then finish the task.
///
/// Ordering is the safety property. The merge runs first, while the task is
/// still `REVIEW`, because it is the only step that writes to the user's
/// repository — a conflict therefore aborts the write with **no** change to
/// the tree. That conflict now triggers **Phase 9**: instead of failing, the
/// workspace is rebased onto the repository's current content (a 3-way merge
/// `RESOLVING_CONFLICTS`, and the caller is handed
/// [`MergeOutcome::ConflictResolutionQueued`] to spawn a resolution agent. The
/// task then re-enters `EVALUATING` and, once green, returns to `REVIEW` so a
/// human approves the now-clean diff.
///
/// Only once the code is in the tree does the task move to `DONE`, unlock its
/// dependents, and drop the workspace it no longer needs (its diff stays on
/// the task row for the dashboard).
///
/// A failure *after* the merge (a state write, cleanup) leaves the task in
/// `REVIEW` with the changes already applied; approving again detects that
/// via a reverse dry run and completes normally.
pub fn approve_task(paths: &ZuneshaPaths, task_id: &str) -> Result<ApproveOutcome> {
    let db = Database::open(&paths.state_db())?;
    let task = db.require_task(task_id)?;
    if task.status != TaskStatus::Review {
        return Err(Error::InvalidTransition {
            task_id: task_id.to_string(),
            from: task.status,
            to: TaskStatus::Done,
        });
    }

    // 1. Merge — may return MergeConflict, leaving the repository untouched.
    match Integrator::new(paths.clone()).integrate(&db, task_id) {
        Ok(integration) => {
            // 2. Finish the lifecycle and unlock the DAG.
            db.set_status(task_id, TaskStatus::Done)?;
            let promoted = promote_ready(&db)?;

            // 3. The workspace earned its keep; its content is now in the
            //    repository.
            WorkspaceManager::new(paths.clone()).remove(&db, task_id)?;

            Ok(ApproveOutcome {
                task: db.require_task(task_id)?,
                promoted,
                integration,
            })
        }

        // Phase 9: the diff no longer applies cleanly. Rebase the workspace
        // onto the repository's current content (3-way merge, conflict markers
        // left in the working tree) and route the task into
        // RESOLVING_CONFLICTS so an agent reconciles the overlap. Nothing has
        // been written to the real working tree.
        Err(Error::MergeConflict { files, detail, .. }) => {
            match WorkspaceManager::new(paths.clone()).prepare_conflict_resolution(&db, task_id) {
                Ok(prep) => {
                    db.set_status(task_id, TaskStatus::ResolvingConflicts)?;
                    Ok(ApproveOutcome {
                        task: db.require_task(task_id)?,
                        promoted: Vec::new(),
                        integration: MergeOutcome::ConflictResolutionQueued {
                            // Prefer where the markers actually landed; fall back
                            // to the patch-failure blame if the merge auto-resolved.
                            files: if prep.conflicted_files.is_empty() {
                                files
                            } else {
                                prep.conflicted_files
                            },
                        },
                    })
                }
                // If the rebase itself fails (git missing, workspace corrupt),
                // surface the original conflict — the task stays in REVIEW and the
                // human reconciles manually, exactly as before Phase 9.
                Err(_) => Err(Error::MergeConflict {
                    task_id: task_id.to_string(),
                    files,
                    detail,
                }),
            }
        }

        Err(e) => Err(e),
    }
}

/// Replay `patch` onto `root` — check first, then apply, never partially.
fn apply_patch(root: &Path, task_id: &str, patch: &str) -> Result<MergeOutcome> {
    // File list first: purely informational, and computing it before any
    // write keeps every failure path side-effect free.
    let files = changed_files(root, patch);

    // Forward dry run — the normal path.
    let check = git_apply(root, &["--check"], patch)?;
    if check.status.success() {
        let applied = git_apply(root, &[], patch)?;
        if !applied.status.success() {
            // Only reachable if the tree changed between check and apply.
            return Err(Error::Git(format!(
                "`git apply` failed after a clean dry run: {}",
                stderr_of(&applied)
            )));
        }
        return Ok(MergeOutcome::Applied { files });
    }

    // Reverse dry run — the approved changes are already present.
    if git_apply(root, &["--reverse", "--check"], patch)?
        .status
        .success()
    {
        return Ok(MergeOutcome::Applied { files });
    }

    // A real conflict. The forward dry run wrote nothing, so the repository
    // is exactly as the user left it.
    let detail = stderr_of(&check);
    Err(Error::MergeConflict {
        task_id: task_id.to_string(),
        files: conflict_files(&detail),
        detail,
    })
}

/// Run `git apply` in `root`, feeding `patch` on stdin.
///
/// Piping the patch in (rather than via a path) means no temporary file is
/// ever created. A git that bails out early closes the pipe, so a failed
/// write is tolerated — its exit status decides the outcome.
///
/// The patch is forced to end in a newline first: a truncated final line
/// makes git report `corrupt patch`, and every invocation (check, apply,
/// numstat) must see byte-identical input.
fn git_apply(root: &Path, flags: &[&str], patch: &str) -> Result<Output> {
    let mut body = String::with_capacity(patch.len() + 1);
    body.push_str(patch);
    if !body.ends_with('\n') {
        body.push('\n');
    }

    let mut child = Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("apply")
        .args(flags)
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Error::Git(format!("failed to run `git apply`: {e}")))?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(body.as_bytes());
        // Dropping the pipe signals EOF to git.
    }

    child
        .wait_with_output()
        .map_err(|e| Error::Git(format!("`git apply` did not run to completion: {e}")))
}

/// Files the patch touches, straight from git's own parser.
///
/// Best-effort: an unparseable patch simply yields an empty list rather than
/// failing an approval that already succeeded.
fn changed_files(root: &Path, patch: &str) -> Vec<String> {
    let output = match git_apply(root, &["--numstat"], patch) {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        // "<added>\t<deleted>\t<path>"; binary patches report "-\t-\t<path>".
        .filter_map(|line| {
            let path = line.rsplit('\t').next()?.trim_matches('"');
            (!path.is_empty() && path != "-").then(|| path.to_string())
        })
        .collect()
}

/// File names git blamed for a failed dry run, in first-seen order.
///
/// Understands both shapes git emits: `error: patch failed: <path>:<line>`
/// and `error: <path>: <reason>`.
fn conflict_files(stderr: &str) -> Vec<String> {
    let mut files: Vec<String> = Vec::new();
    for line in stderr.lines() {
        let rest = match line.strip_prefix("error: ") {
            Some(rest) => rest.trim(),
            None => continue,
        };
        let path = if let Some(after) = rest.strip_prefix("patch failed: ") {
            // "<path>:<line>" — strip the trailing line number.
            match after.rsplit_once(':') {
                Some((p, n)) if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => p,
                _ => after,
            }
        } else if let Some((p, _)) = rest.split_once(": ") {
            p
        } else {
            continue;
        };
        if !path.is_empty() && !files.iter().any(|f| f.as_str() == path) {
            files.push(path.to_string());
        }
    }
    files
}

/// Trimmed stderr of a finished git invocation, for error messages.
fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::Task;

    /// Run git in `dir`, panicking with stderr on failure.
    fn git_ok(dir: &Path, args: &[&str]) -> String {
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

    /// A scratch installation: either a real git repo (with `app.rs` and
    /// `readme.md` committed) or a plain directory for `--force` installs.
    fn scratch(with_git: bool) -> (tempfile::TempDir, ZuneshaPaths, Database) {
        let tmp = tempfile::tempdir().unwrap();
        let paths = ZuneshaPaths::new(tmp.path());
        std::fs::create_dir_all(paths.workspaces_dir()).unwrap();

        std::fs::write(paths.root().join("app.rs"), "fn main() {}\n").unwrap();
        std::fs::write(paths.root().join("readme.md"), "# demo\n").unwrap();

        if with_git {
            git_ok(paths.root(), &["init", "--quiet"]);
            git_ok(paths.root(), &["config", "user.email", "t@t.local"]);
            git_ok(paths.root(), &["config", "user.name", "T"]);
            git_ok(paths.root(), &["add", "--all"]);
            git_ok(paths.root(), &["commit", "--quiet", "-m", "baseline"]);
        }

        let db = Database::open(&paths.state_db()).unwrap();
        (tmp, paths, db)
    }

    /// Insert a task, give it a workspace, and park it in `REVIEW`.
    ///
    /// Returns the workspace record so tests can act on its directory.
    fn reviewed_task(paths: &ZuneshaPaths, db: &Database, id: &str) -> crate::db::WorkspaceRecord {
        db.insert_task(&Task::new(id, "Add the feature", ""))
            .unwrap();
        db.set_status(id, TaskStatus::Todo).unwrap();
        db.set_status(id, TaskStatus::InProgress).unwrap();
        let record = WorkspaceManager::new(paths.clone()).create(db, id).unwrap();
        // InProgress -> Evaluating -> REVIEW (Phase 8 gates review on a
        // passing validation).
        db.set_status(id, TaskStatus::Evaluating).unwrap();
        db.set_status(id, TaskStatus::Review).unwrap();
        record
    }

    /// Simulate agent work inside a workspace: edit `app.rs`, add `feature.rs`.
    fn agent_work(ws: &Path) {
        std::fs::write(ws.join("app.rs"), "fn main() { println!(\"done\"); }\n").unwrap();
        std::fs::write(ws.join("feature.rs"), "pub fn feature() {}\n").unwrap();
    }

    const AGENT_APP: &str = "fn main() { println!(\"done\"); }\n";

    #[test]
    fn approves_clean_merge_into_git_repo_and_unlocks_dependents() {
        let (_tmp, paths, db) = scratch(true);
        let record = reviewed_task(&paths, &db, "t1");
        agent_work(Path::new(&record.path));

        // A second task waiting on t1, plus a user edit that must survive.
        db.insert_task(&Task::new("t2", "Follow-up", "")).unwrap();
        db.add_dependency("t2", "t1").unwrap();
        std::fs::write(paths.root().join("readme.md"), "# demo (wip)\n").unwrap();

        let outcome = approve_task(&paths, "t1").unwrap();

        // The approved changes landed in the real repository...
        assert_eq!(
            std::fs::read_to_string(paths.root().join("app.rs")).unwrap(),
            AGENT_APP
        );
        assert_eq!(
            std::fs::read_to_string(paths.root().join("feature.rs")).unwrap(),
            "pub fn feature() {}\n"
        );
        match &outcome.integration {
            MergeOutcome::Applied { files } => {
                assert!(files.contains(&"app.rs".to_string()), "{files:?}");
                assert!(files.contains(&"feature.rs".to_string()), "{files:?}");
            }
            other => panic!("expected Applied, got {other:?}"),
        }

        // ...uncommitted, next to the user's own uncommitted edit.
        let status = git_ok(paths.root(), &["status", "--porcelain"]);
        assert!(
            status.contains("M app.rs"),
            "modified file tracked: {status}"
        );
        assert!(
            status.contains("?? feature.rs"),
            "new file tracked: {status}"
        );
        assert!(
            status.contains("M readme.md"),
            "user's dirty file left alone: {status}"
        );
        assert_eq!(
            std::fs::read_to_string(paths.root().join("readme.md")).unwrap(),
            "# demo (wip)\n"
        );

        // Task finished, dependents unlocked, workspace cleaned up.
        assert_eq!(outcome.task.status, TaskStatus::Done);
        assert_eq!(outcome.promoted, vec!["t2".to_string()]);
        assert!(db.get_workspace("t1").unwrap().is_none());
        assert!(!Path::new(&record.path).exists());
    }

    #[test]
    fn conflict_queues_auto_resolution_and_leaves_repository_untouched() {
        let (_tmp, paths, db) = scratch(true);
        let record = reviewed_task(&paths, &db, "t1");
        agent_work(Path::new(&record.path));
        let original_base = db.get_workspace("t1").unwrap().unwrap().base_commit;

        // The user edits the same lines after the workspace was snapshotted.
        let diverged = "fn main() { /* user was here */ }\n";
        std::fs::write(paths.root().join("app.rs"), diverged).unwrap();

        // Phase 9: no error — the conflict queues auto-resolution instead.
        let outcome = approve_task(&paths, "t1").unwrap();
        match &outcome.integration {
            MergeOutcome::ConflictResolutionQueued { files } => {
                assert!(files.contains(&"app.rs".to_string()), "blamed: {files:?}");
            }
            other => panic!("expected ConflictResolutionQueued, got {other:?}"),
        }

        // Repository byte-identical to before the attempt (nothing written).
        assert_eq!(
            std::fs::read_to_string(paths.root().join("app.rs")).unwrap(),
            diverged
        );
        assert!(!paths.root().join("feature.rs").exists());

        // Task routed into RESOLVING_CONFLICTS; workspace kept for the agent.
        assert_eq!(
            db.require_task("t1").unwrap().status,
            TaskStatus::ResolvingConflicts
        );
        let ws = db.get_workspace("t1").unwrap().unwrap();
        assert!(Path::new(&ws.path).is_dir());
        // The workspace was rebased onto the new base (U').
        assert_ne!(ws.base_commit, original_base);

        // Real conflict markers now sit in the workspace's copy of app.rs.
        let merged = std::fs::read_to_string(Path::new(&ws.path).join("app.rs")).unwrap();
        assert!(
            merged.contains("<<<<<<<"),
            "workspace holds conflict markers: {merged}"
        );
    }

    #[test]
    fn conflict_auto_resolution_works_even_when_the_root_has_no_git() {
        // The workspace is its own git repo, so the 3-way rebase runs there
        // regardless of whether the root is one (`zou init --force`).
        let (_tmp, paths, db) = scratch(false);
        let record = reviewed_task(&paths, &db, "t1");
        agent_work(Path::new(&record.path));
        let diverged = "fn main() { /* user was here */ }\n";
        std::fs::write(paths.root().join("app.rs"), diverged).unwrap();

        let outcome = approve_task(&paths, "t1").unwrap();
        assert!(
            matches!(
                outcome.integration,
                MergeOutcome::ConflictResolutionQueued { .. }
            ),
            "queued, not applied: {:?}",
            outcome.integration
        );
        assert_eq!(
            db.require_task("t1").unwrap().status,
            TaskStatus::ResolvingConflicts
        );
        // Root untouched; markers live only in the workspace.
        assert_eq!(
            std::fs::read_to_string(paths.root().join("app.rs")).unwrap(),
            diverged
        );
        let ws = db.get_workspace("t1").unwrap().unwrap();
        let merged = std::fs::read_to_string(Path::new(&ws.path).join("app.rs")).unwrap();
        assert!(merged.contains("<<<<<<<"), "markers: {merged}");
        let _ = record;
    }

    #[test]
    fn a_second_conflict_rebases_again_on_top_of_the_first_resolution() {
        // The subtle repeat path: conflict -> resolve -> REVIEW -> the
        // repository moves AGAIN -> approve -> conflict #2. The workspace's
        // first merge is still open (MERGE_HEAD), which step 1 of the rebase
        // must complete before rebasing onto the newest snapshot.
        let (_tmp, paths, db) = scratch(true);
        let record = reviewed_task(&paths, &db, "t1");
        agent_work(Path::new(&record.path));

        // Conflict #1.
        std::fs::write(paths.root().join("app.rs"), "fn main() { /* v1 */ }\n").unwrap();
        let outcome = approve_task(&paths, "t1").unwrap();
        assert!(matches!(
            outcome.integration,
            MergeOutcome::ConflictResolutionQueued { .. }
        ));

        // The resolution agent merges both sides and validation is green, so
        // the task is back at REVIEW (as the Phase 9 worker leaves it).
        let ws = Path::new(&record.path);
        std::fs::write(
            ws.join("app.rs"),
            "fn main() { /* v1 */ println!(\"done\"); }\n",
        )
        .unwrap();
        db.set_status("t1", TaskStatus::Evaluating).unwrap();
        db.set_status("t1", TaskStatus::Review).unwrap();

        // The repository moves again before the human gets to it.
        let moved_on = "fn main() { /* v2 */ }\n";
        std::fs::write(paths.root().join("app.rs"), moved_on).unwrap();

        let outcome = approve_task(&paths, "t1").unwrap();
        match &outcome.integration {
            MergeOutcome::ConflictResolutionQueued { files } => {
                assert!(files.contains(&"app.rs".to_string()), "blamed: {files:?}")
            }
            other => panic!("second conflict also queues auto-resolution: {other:?}"),
        }
        assert_eq!(
            db.require_task("t1").unwrap().status,
            TaskStatus::ResolvingConflicts
        );

        // Root untouched; the new markers compare against the NEWEST base.
        assert_eq!(
            std::fs::read_to_string(paths.root().join("app.rs")).unwrap(),
            moved_on
        );
        let merged = std::fs::read_to_string(ws.join("app.rs")).unwrap();
        assert!(merged.contains("<<<<<<<"), "markers again: {merged}");
        assert!(
            merged.contains("/* v2 */"),
            "conflict is against the latest repository state: {merged}"
        );
        assert!(
            merged.contains("println!(\"done\")"),
            "the first resolution's work is preserved: {merged}"
        );
    }

    #[test]
    fn conflict_message_names_the_task_and_the_way_out() {
        let err = Error::MergeConflict {
            task_id: "t7".into(),
            files: vec!["src/lib.rs".into()],
            detail: "error: patch failed: src/lib.rs:1\n".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("t7"), "{msg}");
        assert!(msg.contains("src/lib.rs"), "{msg}");
        assert!(msg.contains("REVIEW"), "says where the task ends up: {msg}");
        assert!(msg.contains("patch failed"), "keeps git's reason: {msg}");
    }

    #[test]
    fn already_applied_changes_are_detected_and_finish_normally() {
        let (_tmp, paths, db) = scratch(true);
        let record = reviewed_task(&paths, &db, "t1");
        agent_work(Path::new(&record.path));

        // The human reconciles by taking the agent's version verbatim.
        std::fs::write(paths.root().join("app.rs"), AGENT_APP).unwrap();
        std::fs::write(paths.root().join("feature.rs"), "pub fn feature() {}\n").unwrap();

        let outcome = approve_task(&paths, "t1").unwrap();
        assert!(
            matches!(outcome.integration, MergeOutcome::Applied { .. }),
            "reverse dry run recognises applied work: {:?}",
            outcome.integration
        );
        assert_eq!(outcome.task.status, TaskStatus::Done);
    }

    #[test]
    fn approval_without_changes_completes_and_cleans_up() {
        let (_tmp, paths, db) = scratch(true);
        let record = reviewed_task(&paths, &db, "t1");
        // Workspace exists, but the agent never changed anything.

        let outcome = approve_task(&paths, "t1").unwrap();
        assert_eq!(outcome.integration, MergeOutcome::NoChanges);
        assert_eq!(outcome.task.status, TaskStatus::Done);
        assert!(outcome.promoted.is_empty());
        assert!(!Path::new(&record.path).exists(), "workspace still reaped");
        assert_eq!(
            std::fs::read_to_string(paths.root().join("app.rs")).unwrap(),
            "fn main() {}\n"
        );
    }

    #[test]
    fn approval_requires_review_status() {
        let (_tmp, paths, db) = scratch(true);
        db.insert_task(&Task::new("t1", "Add the feature", ""))
            .unwrap();
        db.set_status("t1", TaskStatus::Todo).unwrap();

        let err = approve_task(&paths, "t1").unwrap_err();
        assert!(
            matches!(err, Error::InvalidTransition { .. }),
            "expected InvalidTransition, got {err}"
        );
        assert_eq!(db.require_task("t1").unwrap().status, TaskStatus::Todo);
    }

    #[test]
    fn falls_back_to_stored_diff_when_workspace_is_gone() {
        let (_tmp, paths, db) = scratch(true);
        let record = reviewed_task(&paths, &db, "t1");
        agent_work(Path::new(&record.path));

        // Capture the reviewed diff, then lose the workspace entirely.
        let mgr = WorkspaceManager::new(paths.clone());
        let reviewed = mgr.generate_diff(&db, "t1").unwrap();
        assert!(!reviewed.is_empty());
        mgr.remove(&db, "t1").unwrap();
        db.set_diff("t1", &reviewed).unwrap();

        let outcome = approve_task(&paths, "t1").unwrap();
        assert!(matches!(outcome.integration, MergeOutcome::Applied { .. }));
        assert_eq!(
            std::fs::read_to_string(paths.root().join("app.rs")).unwrap(),
            AGENT_APP
        );
        assert_eq!(outcome.task.status, TaskStatus::Done);
    }

    #[test]
    fn merges_in_a_plain_directory_without_git() {
        let (_tmp, paths, db) = scratch(false);
        let record = reviewed_task(&paths, &db, "t1");
        agent_work(Path::new(&record.path));

        let outcome = approve_task(&paths, "t1").unwrap();
        assert!(matches!(outcome.integration, MergeOutcome::Applied { .. }));
        assert_eq!(
            std::fs::read_to_string(paths.root().join("app.rs")).unwrap(),
            AGENT_APP
        );
        assert!(paths.root().join("feature.rs").is_file());
        assert_eq!(outcome.task.status, TaskStatus::Done);
        assert!(!Path::new(&record.path).exists());
    }

    #[test]
    fn integrate_is_separable_from_the_status_transition() {
        let (_tmp, paths, db) = scratch(true);
        let record = reviewed_task(&paths, &db, "t1");
        agent_work(Path::new(&record.path));

        // The raw merge does not touch lifecycle state.
        let outcome = Integrator::new(paths.clone()).integrate(&db, "t1").unwrap();
        assert!(matches!(outcome, MergeOutcome::Applied { .. }));
        assert_eq!(db.require_task("t1").unwrap().status, TaskStatus::Review);
        assert!(db.get_workspace("t1").unwrap().is_some());
        assert_eq!(
            std::fs::read_to_string(paths.root().join("app.rs")).unwrap(),
            AGENT_APP
        );

        // Second call: changes already present, still reported as applied.
        let again = Integrator::new(paths.clone()).integrate(&db, "t1").unwrap();
        assert!(matches!(again, MergeOutcome::Applied { .. }));
        let _ = record;
    }

    #[test]
    fn conflict_files_parses_both_git_error_shapes() {
        let stderr = "error: patch failed: a.txt:1\nerror: a.txt: patch does not apply\n\
                      error: patch failed: b/c.txt:12\nerror: b/c.txt: patch does not apply\n";
        assert_eq!(conflict_files(stderr), vec!["a.txt", "b/c.txt"]);

        // Deletions/creations report plain `path: reason` lines.
        let stderr = "error: gone.txt: No such file or directory\n\
                      error: exists.txt: already exists in working directory\n";
        assert_eq!(conflict_files(stderr), vec!["gone.txt", "exists.txt"]);

        // Noise that carries no path is ignored rather than invented.
        assert!(conflict_files("fatal: not a git repository").is_empty());
    }

    #[test]
    fn merge_outcome_serialises_for_the_api() {
        let applied = MergeOutcome::Applied {
            files: vec!["a.txt".into()],
        };
        assert_eq!(
            serde_json::to_value(applied).unwrap(),
            serde_json::json!({ "status": "applied", "files": ["a.txt"] })
        );
        assert_eq!(
            serde_json::to_value(MergeOutcome::NoChanges).unwrap(),
            serde_json::json!({ "status": "no_changes" })
        );
        assert_eq!(
            serde_json::to_value(MergeOutcome::ConflictResolutionQueued {
                files: vec!["a.txt".into()]
            })
            .unwrap(),
            serde_json::json!({ "status": "conflict_resolution_queued", "files": ["a.txt"] })
        );
    }
}
