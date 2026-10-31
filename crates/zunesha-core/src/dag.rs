//! The DAG engine: promotes tasks whose prerequisites are satisfied.
//!
//! The engine is the heartbeat of task flow. After any status change (a task
//! finishing, being approved, being rejected) the daemon calls
//! [`promote_ready`] to move newly-unblocked `BACKLOG` tasks to `TODO`.
//!
//! The rule is intentionally simple and strict:
//! a `BACKLOG` task becomes `TODO` **iff every task it depends on (directly
//! or transitively) is `DONE`**. Because `DONE` is the only status that
//! satisfies prerequisites, transitivity falls out of iterating the check to
//! a fixed point — which [`promote_ready`] does in a single pass, since a
//! task promoted to `TODO` in this pass cannot itself be `DONE`.

use crate::db::Database;
use crate::error::{Error, Result};
use crate::paths::ZuneshaPaths;
use crate::task::TaskStatus;
use crate::workspace::WorkspaceManager;

/// Scan every `BACKLOG` task and promote to `TODO` the ones whose
/// prerequisites are all `DONE`.
///
/// Returns the ids that were promoted, in creation order. Safe to call
/// repeatedly; a task is only ever promoted once (subsequent calls see it as
/// `TODO`, not `BACKLOG`).
///
/// Promotions cascade within one pass: if `t1` is `DONE` and both `t2` and
/// `t3` (which depends on `t2`) are `BACKLOG`, only `t2` is promoted — `t3`
/// waits for the next call, after `t2` reaches `DONE`.
pub fn promote_ready(db: &Database) -> Result<Vec<String>> {
    let backlog = db.list_tasks_by_status(TaskStatus::Backlog)?;
    let mut promoted = Vec::new();

    for task in backlog {
        if db.all_dependencies_done(&task.id)? {
            db.set_status(&task.id, TaskStatus::Todo)?;
            promoted.push(task.id);
        }
    }

    Ok(promoted)
}

/// The set of `BACKLOG` tasks that are currently blocked, with the ids of
/// their unfinished prerequisites. Useful for the dashboard's "why is this
/// waiting?" affordance.
pub fn blocked_tasks(db: &Database) -> Result<Vec<(String, Vec<String>)>> {
    let backlog = db.list_tasks_by_status(TaskStatus::Backlog)?;
    let mut blocked = Vec::new();

    for task in backlog {
        if db.all_dependencies_done(&task.id)? {
            continue;
        }
        let unfinished = unfinished_dependencies(db, &task.id)?;
        blocked.push((task.id, unfinished));
    }

    Ok(blocked)
}

/// Release `IN_PROGRESS`, `EVALUATING` and `RESOLVING_CONFLICTS` tasks back to
/// `TODO` — crash recovery.
///
/// A daemon killed mid-run, or an interrupted `zou run`, leaves its claimed
/// tasks stuck in `IN_PROGRESS` (mid-validation in `EVALUATING`, or
/// mid-auto-merge in `RESOLVING_CONFLICTS`) forever: they can never be
/// re-run, reviewed, or rejected, and their dependents stay blocked in
/// `BACKLOG`. This moves them back to `TODO` so [`promote_ready`] can treat
/// the plan normally again.
///
/// With `task_id`, only that task is released (and a missing or non-stuck
/// task is an error, so a typo can't silently "recover" the wrong thing);
/// with `None`, every stuck task is released at once.
///
/// This stays **manual** for the CLI (`zou recover` / `POST /api/recover`):
/// a CLI run and the daemon can legitimately coexist on one machine, and
/// without a heartbeat there is no safe way to distinguish a live worker
/// from a dead one automatically. Only the human can know the worker is gone.
/// (At *daemon boot* the previous process is gone by construction, so
/// [`crate::WorkerPool::resume_stuck`] re-runs these tasks automatically —
/// Phase 11.)
///
/// The task's workspace is kept, so the retry (and its review) builds on
/// whatever the crashed agent already produced — the same contract as a
/// rejection. A released task restarts from a fresh agent attempt; its
/// persisted `retry_count` still counts against the 3-failure cap.
///
/// Releasing a `RESOLVING_CONFLICTS` task re-queues it as a *normal* attempt:
/// the workspace keeps whatever state the interrupted resolution left (its
/// base already points at the repository snapshot, so the diff stays
/// correctly rebased), and validation + human review still gate anything
/// reaching the real tree.
pub fn recover_stuck(db: &Database, task_id: Option<&str>) -> Result<Vec<String>> {
    /// Statuses that mean "a worker owns this task right now".
    const STUCK: [TaskStatus; 3] = [
        TaskStatus::InProgress,
        TaskStatus::Evaluating,
        TaskStatus::ResolvingConflicts,
    ];

    // For a targeted release, verify the task exists and is actually stuck
    // before touching anything; an unknown id or wrong status is an error.
    if let Some(id) = task_id {
        let task = db.require_task(id)?;
        if !STUCK.contains(&task.status) {
            return Err(Error::InvalidTransition {
                task_id: id.to_string(),
                from: task.status,
                to: TaskStatus::Todo,
            });
        }
    }

    let mut released = Vec::new();
    for status in STUCK {
        for task in db.list_tasks_by_status(status)? {
            if task_id.is_some_and(|id| task.id != id) {
                continue;
            }
            db.set_status(&task.id, TaskStatus::Todo)?;
            released.push(task.id);
        }
    }

    Ok(released)
}

/// Remove a task completely from the orchestrator.
///
/// This cleans up the task's workspace directory (if any), removes its row
/// from the database (which cascades to dependency edges, runs, and workspace
/// records), and promotes any downstream `BACKLOG` tasks that are now unblocked.
///
/// Returns `Ok(true)` if the task was found and removed, or `Ok(false)` if it
/// did not exist.
pub fn remove_task(paths: &ZuneshaPaths, db: &Database, task_id: &str) -> Result<bool> {
    if db.get_task(task_id)?.is_none() {
        return Ok(false);
    }

    // Clean up the task workspace directory and DB record if present.
    let mgr = WorkspaceManager::new(paths.clone());
    let _ = mgr.remove(db, task_id);

    // Also remove the workspace directory directly in case no DB workspace record existed.
    let ws_dir = paths.workspace(task_id);
    if ws_dir.is_dir() {
        let _ = std::fs::remove_dir_all(&ws_dir);
    }

    // Delete the task (cascades dependencies, runs, workspaces in SQLite).
    let removed = db.delete_task(task_id)?;
    if removed {
        // Any tasks in BACKLOG that were blocked only by this task may now be ready.
        let _ = promote_ready(db);
    }
    Ok(removed)
}

/// Prerequisites of `task_id` that are not yet `DONE`.
fn unfinished_dependencies(db: &Database, task_id: &str) -> Result<Vec<String>> {
    let mut unfinished = Vec::new();
    for dep_id in db.dependencies_of(task_id)? {
        let dep = db.require_task(&dep_id)?;
        if dep.status != TaskStatus::Done {
            unfinished.push(dep_id);
        }
    }
    Ok(unfinished)
}

/// Render a read-only macro roadmap of the task DAG for the agent working on `current_task_id`.
///
/// Returns `None` if there are no other tasks in the project (single-task plan).
/// If there are multiple tasks, formats a markdown summary showing completed work,
/// the current task, its direct prerequisites, downstream tasks waiting on it,
/// and remaining upcoming tasks.
pub fn render_roadmap(db: &Database, current_task_id: &str) -> Result<Option<String>> {
    let all_tasks = db.list_tasks()?;
    if all_tasks.len() <= 1 {
        return Ok(None);
    }

    let mut out = String::new();
    out.push_str("## Feature Roadmap\n");
    out.push_str(
        "This project consists of multiple planned tasks. You are working on one part of this graph.\n\
         Keep completed foundations and downstream dependents in mind:\n",
    );

    let current_deps: std::collections::HashSet<String> =
        db.dependencies_of(current_task_id)?.into_iter().collect();
    let current_dependents: std::collections::HashSet<String> =
        db.dependents_of(current_task_id)?.into_iter().collect();

    for task in &all_tasks {
        if task.id == current_task_id {
            out.push_str(&format!("- [>] **CURRENT**: {} (`{}`)\n", task.title, task.id));
            if !current_deps.is_empty() {
                let mut dep_names: Vec<String> = current_deps
                    .iter()
                    .filter_map(|id| {
                        db.get_task(id)
                            .ok()
                            .flatten()
                            .map(|t| format!("`{}` ({}, `{}`)", t.title, t.status, t.id))
                    })
                    .collect();
                dep_names.sort();
                out.push_str(&format!(
                    "  * Direct prerequisites: {}\n",
                    dep_names.join(", ")
                ));
            }
            if !current_dependents.is_empty() {
                let mut dep_names: Vec<String> = current_dependents
                    .iter()
                    .filter_map(|id| {
                        db.get_task(id)
                            .ok()
                            .flatten()
                            .map(|t| format!("`{}` ({}, `{}`)", t.title, t.status, t.id))
                    })
                    .collect();
                dep_names.sort();
                out.push_str(&format!(
                    "  * Downstream tasks waiting on your work: {}\n",
                    dep_names.join(", ")
                ));
            }
        } else {
            let marker = match task.status {
                TaskStatus::Done => "[x]",
                TaskStatus::InProgress | TaskStatus::Evaluating => "[*]",
                _ => "[ ]",
            };
            out.push_str(&format!(
                "- {} {}: {} (`{}`)\n",
                marker, task.status, task.title, task.id
            ));
        }
    }

    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::Task;

    /// Build a chain t1 <- t2 <- t3 where `t2` depends on `t1` and `t3`
    /// depends on `t2`. All tasks start in `BACKLOG`.
    fn chain_db() -> Database {
        let db = Database::open_in_memory().unwrap();
        for (id, title) in [("t1", "one"), ("t2", "two"), ("t3", "three")] {
            db.insert_task(&Task::new(id, title, "")).unwrap();
        }
        db.add_dependency("t2", "t1").unwrap();
        db.add_dependency("t3", "t2").unwrap();
        db
    }

    fn status_of(db: &Database, id: &str) -> TaskStatus {
        db.get_task(id).unwrap().unwrap().status
    }

    #[test]
    fn head_of_chain_is_promoted_first() {
        let db = chain_db();
        let promoted = promote_ready(&db).unwrap();
        // Only t1 (no prerequisites) is ready.
        assert_eq!(promoted, vec!["t1".to_string()]);
        assert_eq!(status_of(&db, "t1"), TaskStatus::Todo);
        assert_eq!(status_of(&db, "t2"), TaskStatus::Backlog);
        assert_eq!(status_of(&db, "t3"), TaskStatus::Backlog);
    }

    #[test]
    fn completing_task_one_unlocks_task_two() {
        // This is the Phase 2 deliverable: proving that as Task 1 is
        // manually updated to DONE, Task 2 automatically transitions from
        // BACKLOG to TODO.
        let db = chain_db();
        promote_ready(&db).unwrap(); // t1 -> TODO

        // Nothing new unlocks while t1 is merely TODO.
        assert!(promote_ready(&db).unwrap().is_empty());
        assert_eq!(status_of(&db, "t2"), TaskStatus::Backlog);

        // Mark t1 DONE: the very next engine pass promotes t2.
        // (force: TODO -> DONE is a policy-violating shortcut used only to
        // set up the scenario quickly.)
        db.force_set_status("t1", TaskStatus::Done).unwrap();
        let promoted = promote_ready(&db).unwrap();
        assert_eq!(promoted, vec!["t2".to_string()]);
        assert_eq!(status_of(&db, "t2"), TaskStatus::Todo);
        // t3 still waits on t2.
        assert_eq!(status_of(&db, "t3"), TaskStatus::Backlog);
    }

    #[test]
    fn chain_unlocks_end_to_end() {
        let db = chain_db();
        for expected_ready in ["t1", "t2", "t3"] {
            let promoted = promote_ready(&db).unwrap();
            assert_eq!(promoted, vec![expected_ready.to_string()]);
            // force: TODO -> DONE shortcut for fast setup.
            db.force_set_status(expected_ready, TaskStatus::Done)
                .unwrap();
        }
        // Everything done: no further promotions possible.
        assert!(promote_ready(&db).unwrap().is_empty());
    }

    #[test]
    fn diamond_requires_both_branches_done() {
        //   t1
        //  /  \
        // t2  t3
        //  \  /
        //   t4
        let db = Database::open_in_memory().unwrap();
        for id in ["t1", "t2", "t3", "t4"] {
            db.insert_task(&Task::new(id, id, "")).unwrap();
        }
        db.add_dependency("t2", "t1").unwrap();
        db.add_dependency("t3", "t1").unwrap();
        db.add_dependency("t4", "t2").unwrap();
        db.add_dependency("t4", "t3").unwrap();

        // t1 ready immediately.
        assert_eq!(promote_ready(&db).unwrap(), vec!["t1".to_string()]);
        // force: TODO -> DONE shortcut for fast setup.
        db.force_set_status("t1", TaskStatus::Done).unwrap();

        // Both branches unlock together.
        let mut promoted = promote_ready(&db).unwrap();
        promoted.sort();
        assert_eq!(promoted, vec!["t2".to_string(), "t3".to_string()]);

        // Completing only ONE branch must NOT unlock t4.
        db.force_set_status("t2", TaskStatus::Done).unwrap();
        assert!(promote_ready(&db).unwrap().is_empty());
        assert_eq!(status_of(&db, "t4"), TaskStatus::Backlog);

        // Completing the second branch unlocks t4.
        db.force_set_status("t3", TaskStatus::Done).unwrap();
        assert_eq!(promote_ready(&db).unwrap(), vec!["t4".to_string()]);
    }

    #[test]
    fn promotion_is_idempotent() {
        let db = chain_db();
        let first = promote_ready(&db).unwrap();
        let second = promote_ready(&db).unwrap();
        assert_eq!(first, vec!["t1".to_string()]);
        assert!(second.is_empty(), "t1 is already TODO, not BACKLOG");
    }

    #[test]
    fn in_flight_work_is_not_touched() {
        // A task already claimed by a worker (IN_PROGRESS) or awaiting review
        // must never be demoted or re-promoted by the engine.
        let db = chain_db();
        // force: BACKLOG -> IN_PROGRESS shortcut (policy would require TODO first).
        db.force_set_status("t1", TaskStatus::InProgress).unwrap();
        assert!(promote_ready(&db).unwrap().is_empty());
        assert_eq!(status_of(&db, "t1"), TaskStatus::InProgress);
    }

    #[test]
    fn blocked_tasks_reports_unfinished_prereqs() {
        let db = chain_db();
        promote_ready(&db).unwrap(); // t1 -> TODO

        let blocked = blocked_tasks(&db).unwrap();
        assert_eq!(blocked.len(), 2);
        let t2 = blocked.iter().find(|(id, _)| id == "t2").unwrap();
        assert_eq!(t2.1, vec!["t1".to_string()], "t2 waits on unfinished t1");
        let t3 = blocked.iter().find(|(id, _)| id == "t3").unwrap();
        assert_eq!(t3.1, vec!["t2".to_string()]);
    }

    #[test]
    fn recover_stuck_releases_every_in_progress_task() {
        let db = chain_db();
        promote_ready(&db).unwrap(); // t1 -> TODO
                                     // Two claimed workers "crash" (daemon killed mid-run).
        db.force_set_status("t1", TaskStatus::InProgress).unwrap();
        db.force_set_status("t2", TaskStatus::InProgress).unwrap();

        let released = recover_stuck(&db, None).unwrap();
        assert_eq!(released, vec!["t1".to_string(), "t2".to_string()]);
        assert_eq!(status_of(&db, "t1"), TaskStatus::Todo);
        assert_eq!(status_of(&db, "t2"), TaskStatus::Todo);
        assert_eq!(status_of(&db, "t3"), TaskStatus::Backlog, "untouched");
        assert!(
            recover_stuck(&db, None).unwrap().is_empty(),
            "nothing stuck remains"
        );
    }

    #[test]
    fn recover_stuck_can_release_a_single_task() {
        let db = chain_db();
        promote_ready(&db).unwrap();
        db.force_set_status("t1", TaskStatus::InProgress).unwrap();
        db.force_set_status("t2", TaskStatus::InProgress).unwrap();

        assert_eq!(
            recover_stuck(&db, Some("t2")).unwrap(),
            vec!["t2".to_string()]
        );
        assert_eq!(status_of(&db, "t2"), TaskStatus::Todo);
        assert_eq!(
            status_of(&db, "t1"),
            TaskStatus::InProgress,
            "the other stuck task is left alone"
        );
    }

    #[test]
    fn recover_stuck_rejects_unknown_or_unstuck_tasks() {
        let db = chain_db();
        promote_ready(&db).unwrap(); // t1 -> TODO, t2/t3 stay BACKLOG

        assert!(matches!(
            recover_stuck(&db, Some("nope")),
            Err(Error::TaskNotFound(_))
        ));
        assert!(matches!(
            recover_stuck(&db, Some("t3")),
            Err(Error::InvalidTransition { .. })
        ));
        assert_eq!(status_of(&db, "t3"), TaskStatus::Backlog, "no side effects");
    }

    #[test]
    fn recover_stuck_releases_tasks_mid_validation_too() {
        // Phase 8: a daemon killed while the test command was running leaves
        // the task in EVALUATING — it must be releasable like IN_PROGRESS.
        let db = chain_db();
        promote_ready(&db).unwrap();
        db.force_set_status("t1", TaskStatus::InProgress).unwrap();
        db.force_set_status("t1", TaskStatus::Evaluating).unwrap();
        db.force_set_status("t2", TaskStatus::InProgress).unwrap();
        db.set_retry_count("t1", 2).unwrap();

        assert_eq!(
            recover_stuck(&db, None).unwrap(),
            vec!["t2".to_string(), "t1".to_string()],
            "both worker-owned statuses released (IN_PROGRESS batch, then EVALUATING)"
        );
        assert_eq!(status_of(&db, "t1"), TaskStatus::Todo);
        assert_eq!(status_of(&db, "t2"), TaskStatus::Todo);
        // The retry budget survives recovery: the cap is per task, not per run.
        assert_eq!(db.require_task("t1").unwrap().retry_count, 2);

        // Targeted release works for EVALUATING as well.
        db.force_set_status("t1", TaskStatus::InProgress).unwrap();
        db.force_set_status("t1", TaskStatus::Evaluating).unwrap();
        assert_eq!(
            recover_stuck(&db, Some("t1")).unwrap(),
            vec!["t1".to_string()]
        );
    }

    #[test]
    fn recover_stuck_releases_an_interrupted_conflict_resolution() {
        // Phase 9: a daemon killed while the auto-merge agent ran leaves the
        // task in RESOLVING_CONFLICTS — equally worker-owned, equally stuck.
        let db = chain_db();
        promote_ready(&db).unwrap();
        db.force_set_status("t1", TaskStatus::InProgress).unwrap();
        db.force_set_status("t1", TaskStatus::Evaluating).unwrap();
        db.force_set_status("t1", TaskStatus::Review).unwrap();
        db.force_set_status("t1", TaskStatus::ResolvingConflicts)
            .unwrap();

        // Targeted release accepts it...
        assert_eq!(
            recover_stuck(&db, Some("t1")).unwrap(),
            vec!["t1".to_string()]
        );
        assert_eq!(status_of(&db, "t1"), TaskStatus::Todo);

        // ...and so does the bulk sweep.
        db.force_set_status("t1", TaskStatus::InProgress).unwrap();
        db.force_set_status("t1", TaskStatus::Evaluating).unwrap();
        db.force_set_status("t1", TaskStatus::Review).unwrap();
        db.force_set_status("t1", TaskStatus::ResolvingConflicts)
            .unwrap();
        assert_eq!(recover_stuck(&db, None).unwrap(), vec!["t1".to_string()]);
        assert_eq!(status_of(&db, "t1"), TaskStatus::Todo);
    }

    #[test]
    fn recovered_task_can_finish_and_unblock_dependents() {
        // The full crash-recovery round trip: claimed -> stuck -> released ->
        // retried -> approved -> dependents promoted again.
        let db = chain_db();
        promote_ready(&db).unwrap(); // t1 -> TODO
        db.force_set_status("t1", TaskStatus::InProgress).unwrap();

        assert_eq!(recover_stuck(&db, None).unwrap(), vec!["t1".to_string()]);
        assert_eq!(status_of(&db, "t2"), TaskStatus::Backlog);

        // Retry succeeds; the human approves; the DAG moves on.
        db.force_set_status("t1", TaskStatus::Done).unwrap();
        assert_eq!(promote_ready(&db).unwrap(), vec!["t2".to_string()]);
    }

    #[test]
    fn remove_task_cleans_workspace_and_unblocks_dependents() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = ZuneshaPaths::new(tmp.path());
        std::fs::create_dir_all(paths.dir()).unwrap();
        let db = Database::open(&paths.state_db()).unwrap();

        // Build a 2-task chain: t1 -> t2
        db.insert_task(&Task::new("t1", "First", "")).unwrap();
        db.insert_task(&Task::new("t2", "Second", "")).unwrap();
        db.add_dependency("t2", "t1").unwrap();

        promote_ready(&db).unwrap();
        assert_eq!(status_of(&db, "t1"), TaskStatus::Todo);
        assert_eq!(status_of(&db, "t2"), TaskStatus::Backlog);

        // Create a workspace for t1 on disk
        let ws_dir = paths.workspace("t1");
        std::fs::create_dir_all(&ws_dir).unwrap();
        std::fs::write(ws_dir.join("test.txt"), "hello").unwrap();
        assert!(ws_dir.is_dir());

        // Removing non-existent task returns false
        assert!(!remove_task(&paths, &db, "nonexistent").unwrap());

        // Remove t1
        assert!(remove_task(&paths, &db, "t1").unwrap());

        // t1 is gone from DB
        assert!(db.get_task("t1").unwrap().is_none());
        // Workspace directory is deleted
        assert!(!ws_dir.exists());
        // t2 was unblocked and promoted to TODO!
        assert_eq!(status_of(&db, "t2"), TaskStatus::Todo);
    }

    #[test]
    fn render_roadmap_returns_none_for_single_task() {
        let db = Database::open_in_memory().unwrap();
        db.insert_task(&Task::new("solo", "Solo Task", "")).unwrap();
        assert!(render_roadmap(&db, "solo").unwrap().is_none());
    }

    #[test]
    fn render_roadmap_formats_multi_task_dag_foresight() {
        let db = chain_db();
        promote_ready(&db).unwrap(); // t1 -> TODO
        db.force_set_status("t1", TaskStatus::Done).unwrap();
        promote_ready(&db).unwrap(); // t2 -> TODO
        db.force_set_status("t2", TaskStatus::InProgress).unwrap();

        let roadmap = render_roadmap(&db, "t2").unwrap().expect("roadmap rendered");
        assert!(roadmap.contains("## Feature Roadmap"), "{roadmap}");
        assert!(roadmap.contains("- [x] DONE: one (`t1`)"), "{roadmap}");
        assert!(roadmap.contains("- [>] **CURRENT**: two (`t2`)"), "{roadmap}");
        assert!(
            roadmap.contains("Direct prerequisites: `one` (DONE, `t1`)"),
            "{roadmap}"
        );
        assert!(
            roadmap.contains("Downstream tasks waiting on your work: `three` (BACKLOG, `t3`)"),
            "{roadmap}"
        );
        assert!(roadmap.contains("- [ ] BACKLOG: three (`t3`)"), "{roadmap}");
    }
}
