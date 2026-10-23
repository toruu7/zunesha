//! A mock "planner" that turns a feature prompt into a DAG of tasks.
//!
//! Phase 2 ships a deliberately simple planner: given a prompt string it
//! creates three tasks in a fixed dependency chain (1 → 2 → 3). This is
//! enough to exercise the whole pipeline — DAG engine, workspaces, workers,
//! review loop — before a real LLM-backed planner replaces it in a later
//! phase. The public signature ([`plan`]) is what the daemon and the `/api/plan`
//! endpoint will call, so the swap will be transparent.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::dag;
use crate::db::Database;
use crate::error::Result;
use crate::task::Task;

/// Monotonic counter so ids generated within the same nanosecond stay unique.
static ID_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Plan a feature from `prompt`, inserting the resulting tasks and their
/// dependency edges into the database, then run one engine pass so the
/// ready tasks land in `TODO`.
///
/// Returns the created task ids in chain order (prerequisite-first).
///
/// The mock planner always produces the same shape:
///
/// ```text
/// task-1 (analyze & design) ──▶ task-2 (implement) ──▶ task-3 (test & document)
/// ```
pub fn plan(db: &Database, prompt: &str) -> Result<Vec<String>> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        // A planner with nothing to plan should be a loud no-op rather than
        // silently creating empty tasks.
        return Ok(Vec::new());
    }

    let steps = [
        ("Analyze & design", plan_step_description(prompt, 1)),
        ("Implement the change", plan_step_description(prompt, 2)),
        ("Test & document", plan_step_description(prompt, 3)),
    ];

    let mut ids = Vec::with_capacity(steps.len());
    let mut previous: Option<String> = None;

    for (title, description) in steps {
        let id = next_task_id();
        db.insert_task(&Task::new(&id, format!("{title}: {prompt}"), &description))?;

        // Chain: each new task depends on the one before it.
        if let Some(ref prev) = previous {
            db.add_dependency(&id, prev)?;
        }
        previous = Some(id.clone());
        ids.push(id);
    }

    // Promote whatever is immediately ready (the head of the chain).
    dag::promote_ready(db)?;

    Ok(ids)
}

/// Description for step N of the mock plan.
fn plan_step_description(prompt: &str, step: u8) -> String {
    match step {
        1 => format!("Understand the request and sketch an implementation plan for: {prompt}"),
        2 => format!("Implement the core of: {prompt}"),
        _ => format!("Add tests and docs covering: {prompt}"),
    }
}

/// Generate a unique task id: `task-<unix_nanos>-<counter>`.
fn next_task_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("task-{nanos}-{n}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::TaskStatus;

    fn status_of(db: &Database, id: &str) -> TaskStatus {
        db.get_task(id).unwrap().unwrap().status
    }

    #[test]
    fn plan_creates_a_three_task_chain() {
        let db = Database::open_in_memory().unwrap();
        let ids = plan(&db, "add a dark mode toggle").unwrap();

        assert_eq!(ids.len(), 3);
        assert_eq!(db.task_count().unwrap(), 3);
        assert_eq!(db.dependency_count().unwrap(), 2, "a 3-chain has 2 edges");

        // ids[1] depends on ids[0]; ids[2] depends on ids[1].
        assert_eq!(db.dependencies_of(&ids[1]).unwrap(), vec![ids[0].clone()]);
        assert_eq!(db.dependencies_of(&ids[2]).unwrap(), vec![ids[1].clone()]);
        // The head has no prerequisites.
        assert!(db.dependencies_of(&ids[0]).unwrap().is_empty());
    }

    #[test]
    fn plan_head_is_ready_and_rest_are_backlog() {
        let db = Database::open_in_memory().unwrap();
        let ids = plan(&db, "add a dark mode toggle").unwrap();

        assert_eq!(status_of(&db, &ids[0]), TaskStatus::Todo);
        assert_eq!(status_of(&db, &ids[1]), TaskStatus::Backlog);
        assert_eq!(status_of(&db, &ids[2]), TaskStatus::Backlog);
    }

    #[test]
    fn plan_tasks_reference_the_prompt() {
        let db = Database::open_in_memory().unwrap();
        let ids = plan(&db, "add a dark mode toggle").unwrap();
        for id in &ids {
            let task = db.require_task(id).unwrap();
            assert!(
                task.title.contains("dark mode toggle")
                    || task.description.contains("dark mode toggle"),
                "prompt should appear in task text: {task:?}"
            );
        }
    }

    #[test]
    fn empty_prompt_plans_nothing() {
        let db = Database::open_in_memory().unwrap();
        let ids = plan(&db, "   ").unwrap();
        assert!(ids.is_empty());
        assert_eq!(db.task_count().unwrap(), 0);
    }

    #[test]
    fn two_plans_produce_distinct_ids() {
        let db = Database::open_in_memory().unwrap();
        let a = plan(&db, "feature A").unwrap();
        let b = plan(&db, "feature B").unwrap();
        assert_eq!(db.task_count().unwrap(), 6);
        for id_a in &a {
            assert!(!b.contains(id_a), "ids must be globally unique");
        }
    }

    #[test]
    fn plan_integrates_with_dag_engine_end_to_end() {
        // Full Phase 2 loop: plan, then walk the whole chain to DONE using
        // only engine promotions.
        let db = Database::open_in_memory().unwrap();
        let ids = plan(&db, "ship the feature").unwrap();

        for id in &ids {
            // The engine must have made this task ready by now.
            assert_eq!(status_of(&db, id), TaskStatus::Todo, "{id} should be TODO");
            // force: TODO -> DONE shortcut for fast walkthrough.
            db.force_set_status(id, TaskStatus::Done).unwrap();
            // Promote the next link before looping.
            dag::promote_ready(&db).unwrap();
        }

        let counts = db.task_counts_by_status().unwrap();
        let done = counts.iter().find(|(s, _)| *s == TaskStatus::Done).unwrap();
        assert_eq!(done.1, 3);
    }
}
