//! Task model shared across every phase of the orchestrator.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::Error;

/// Lifecycle of an orchestrator task.
///
/// Tasks flow forward through these states as the DAG engine, the worker pool
/// (Phase 4 + the Phase 8 self-healing loop) and the integrator (Phase 7 +
/// Phase 9 auto-merge) advance them:
///
/// ```text
/// BACKLOG ─▶ TODO ─▶ IN_PROGRESS ─▶ EVALUATING ─▶ REVIEW ─▶ DONE
///    ▲          ▲        │  ▲           │       (reject) │
///    │          │        │  └─retry─────┘        │       │
///    │          └────────┴─────── (agent failed / rejected)
///    │                             │  (retries exhausted)   │
///    └─── (prereq reopened)        ▼                        │ (merge conflict)
///                          FAILED_NEEDS_HUMAN ──(reject)──▶ TODO
///                                                            │
///                          RESOLVING_CONFLICTS ◀─────────────┘
///                                    │  agent resolves markers, then
///                                    └─▶ EVALUATING ──▶ REVIEW (re-approve)
/// ```
///
/// `EVALUATING` is the self-healing gate added in Phase 8: after an agent
/// exits 0, the daemon runs the project's test/lint command in the workspace;
/// failures send the task back to `IN_PROGRESS` (up to 3 times) with the error
/// output appended to its prompt, and exhausted retries park the task in
/// `FAILED_NEEDS_HUMAN` for a human.
///
/// `RESOLVING_CONFLICTS` is the auto-merge gate added in Phase 9: when an
/// approved diff no longer applies cleanly (the repository moved since the
/// workspace was snapshotted), the merge is aborted, the workspace is updated
/// to the new base with conflict markers, and an agent resolves them — then the
/// result re-enters `EVALUATING` and, on a green run, returns to `REVIEW` so a
/// human approves the now-clean diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TaskStatus {
    /// Waiting on prerequisites (dependencies not yet `DONE`).
    Backlog,
    /// All prerequisites are done; ready for a worker to pick up.
    Todo,
    /// A worker agent is actively running in an isolated workspace.
    InProgress,
    /// The agent finished; the project's test/lint command is now running in
    /// the workspace (Phase 8 self-healing loop).
    Evaluating,
    /// Worker finished *and* passed validation; waiting for human review.
    Review,
    /// An approved diff conflicted with the changed repository; an agent is
    /// resolving the merge conflict markers in the workspace (Phase 9).
    ResolvingConflicts,
    /// Approved and merged back into the main project.
    Done,
    /// Validation failed 3 times (or a conflict could not be resolved); parked
    /// for a human to inspect, fix, or send back to the queue.
    FailedNeedsHuman,
}

impl TaskStatus {
    /// Every status, in pipeline order.
    pub const ALL: [TaskStatus; 8] = [
        TaskStatus::Backlog,
        TaskStatus::Todo,
        TaskStatus::InProgress,
        TaskStatus::Evaluating,
        TaskStatus::Review,
        TaskStatus::ResolvingConflicts,
        TaskStatus::Done,
        TaskStatus::FailedNeedsHuman,
    ];

    /// The canonical string stored in the database.
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskStatus::Backlog => "BACKLOG",
            TaskStatus::Todo => "TODO",
            TaskStatus::InProgress => "IN_PROGRESS",
            TaskStatus::Evaluating => "EVALUATING",
            TaskStatus::Review => "REVIEW",
            TaskStatus::ResolvingConflicts => "RESOLVING_CONFLICTS",
            TaskStatus::Done => "DONE",
            TaskStatus::FailedNeedsHuman => "FAILED_NEEDS_HUMAN",
        }
    }
    /// Whether a direct transition from `self` to `next` is legal.
    ///
    /// This is the policy that enforces the "Strict DAG Execution" and
    /// "Human in the Loop" architecture rules at the storage layer, so no
    /// caller can silently demote a `DONE` task or skip a stage. Legal moves:
    ///
    /// * forward along the pipeline
    ///   (`BACKLOG→TODO→IN_PROGRESS→EVALUATING→REVIEW→DONE`)
    /// * `EVALUATING→IN_PROGRESS` – validation failed; the self-healing loop
    ///   re-runs the agent with the error output appended
    /// * `EVALUATING→FAILED_NEEDS_HUMAN` – retries exhausted
    /// * `REVIEW→TODO` – human rejected the work; it retries
    /// * `REVIEW→RESOLVING_CONFLICTS` – the approved diff conflicted with the
    ///   changed repository; an agent resolves the markers (Phase 9)
    /// * `RESOLVING_CONFLICTS→EVALUATING` – the agent finished resolving; the
    ///   result is validated
    /// * `EVALUATING→RESOLVING_CONFLICTS` – validation of a resolved conflict
    ///   failed; the agent re-resolves with the errors in its prompt
    /// * `RESOLVING_CONFLICTS→FAILED_NEEDS_HUMAN` – the conflict (or its
    ///   validation) could not be resolved automatically
    /// * `RESOLVING_CONFLICTS→TODO` – a worker crashed/was interrupted
    ///   mid-resolution; release the claim (`zou recover`)
    /// * `FAILED_NEEDS_HUMAN→TODO` – human sent a parked task back to the queue
    /// * `IN_PROGRESS|EVALUATING→TODO` – a worker crashed/was interrupted;
    ///   release the claim (`zou recover`)
    /// * `TODO→BACKLOG` – a prerequisite was reopened, so the task is blocked again
    /// * same→same – an idempotent no-op
    ///
    /// Anything else (notably anything out of `DONE`) requires an explicit
    /// force. [`crate::Database::set_status`] enforces this;
    /// [`crate::Database::force_set_status`] bypasses it for tests and manual
    /// repair.
    pub fn can_transition_to(self, next: TaskStatus) -> bool {
        use TaskStatus::*;
        if self == next {
            return true;
        }
        matches!(
            (self, next),
            (Backlog, Todo)
                | (Todo, InProgress)
                | (InProgress, Evaluating)
                | (Evaluating, Review)
                | (Evaluating, InProgress)
                | (Evaluating, FailedNeedsHuman)
                | (Review, Done)
                | (Review, Todo)
                | (Review, ResolvingConflicts)
                | (ResolvingConflicts, Evaluating)
                | (Evaluating, ResolvingConflicts)
                | (ResolvingConflicts, FailedNeedsHuman)
                | (ResolvingConflicts, Todo)
                | (InProgress, Todo)
                | (Evaluating, Todo)
                | (FailedNeedsHuman, Todo)
                | (Todo, Backlog)
        )
    }
}

impl fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for TaskStatus {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "BACKLOG" => Ok(TaskStatus::Backlog),
            "TODO" => Ok(TaskStatus::Todo),
            "IN_PROGRESS" => Ok(TaskStatus::InProgress),
            "EVALUATING" => Ok(TaskStatus::Evaluating),
            "REVIEW" => Ok(TaskStatus::Review),
            "RESOLVING_CONFLICTS" => Ok(TaskStatus::ResolvingConflicts),
            "DONE" => Ok(TaskStatus::Done),
            "FAILED_NEEDS_HUMAN" => Ok(TaskStatus::FailedNeedsHuman),
            other => Err(Error::UnknownTaskStatus(other.to_string())),
        }
    }
}

/// A single unit of work tracked by the orchestrator.
///
/// Mirrors the `tasks` table exactly. `created_at`/`updated_at` are filled in
/// by SQLite (`datetime('now')`) and are only meaningful on rows that came
/// out of the database.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub description: String,
    pub status: TaskStatus,
    /// Failed validation attempts in the current review cycle (Phase 8).
    /// Capped at [`MAX_EVAL_RETRIES`]; at the cap the task parks in
    /// [`TaskStatus::FailedNeedsHuman`]. Reset when a human sends a parked
    /// task back to the queue.
    pub retry_count: u32,
    /// Captured worker stdout/stderr, the technical transcript (populated in
    /// Phase 4).
    pub worker_logs: String,
    /// The agent's final assistant response, rendered as Markdown in the
    /// dashboard. Captured from structured agent output when available; empty
    /// for agents that only emit a plain transcript.
    pub agent_output: String,
    /// Captured `git diff` of the task's workspace (populated in Phase 3).
    pub diff: String,
    /// Optional crew member assigned to this task (e.g. "luffy", "zoro", "nami", "usopp", "sanji").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crew_member: Option<String>,
    /// `YYYY-MM-DD HH:MM:SS` (UTC), set by the database.
    pub created_at: String,
    /// `YYYY-MM-DD HH:MM:SS` (UTC), set by the database.
    pub updated_at: String,
}

/// Failed evaluations allowed before a task parks in
/// [`TaskStatus::FailedNeedsHuman`] and asks a human.
pub const MAX_EVAL_RETRIES: u32 = 3;

impl Task {
    /// Create a new task in the `BACKLOG` status.
    ///
    /// Timestamps start empty and are populated when the row is read back
    /// from the database.
    pub fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            description: description.into(),
            status: TaskStatus::Backlog,
            retry_count: 0,
            worker_logs: String::new(),
            agent_output: String::new(),
            diff: String::new(),
            crew_member: None,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    /// Assign a crew member to this task.
    pub fn with_crew(mut self, crew_member: impl Into<String>) -> Self {
        self.crew_member = Some(crew_member.into());
        self
    }

    /// Return an emoji badge for the assigned crew member.
    pub fn crew_emoji(&self) -> &'static str {
        match self
            .crew_member
            .as_deref()
            .map(|s| s.to_ascii_lowercase())
            .as_deref()
        {
            Some("luffy") => "👒",
            Some("zoro") => "⚔️",
            Some("nami") => "🗺️",
            Some("usopp") => "🎯",
            Some("sanji") => "🍳",
            _ => "🏴‍☠️",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_roundtrips_through_str() {
        for status in TaskStatus::ALL {
            let parsed: TaskStatus = status.as_str().parse().unwrap();
            assert_eq!(parsed, status);
            assert_eq!(parsed.to_string(), status.as_str());
        }
    }

    #[test]
    fn all_statuses_have_distinct_canonical_strings() {
        let mut seen = std::collections::HashSet::new();
        for status in TaskStatus::ALL {
            assert!(seen.insert(status.as_str()));
        }
        assert_eq!(seen.len(), 8);
    }

    #[test]
    fn unknown_status_is_an_error() {
        let err = "WIP".parse::<TaskStatus>().unwrap_err();
        assert!(matches!(err, Error::UnknownTaskStatus(s) if s == "WIP"));
    }

    #[test]
    fn new_task_starts_in_backlog() {
        let task = Task::new("t1", "Title", "Description");
        assert_eq!(task.status, TaskStatus::Backlog);
        assert_eq!(task.retry_count, 0, "no validation failures yet");
        assert!(task.worker_logs.is_empty());
        assert!(task.agent_output.is_empty());
        assert!(task.diff.is_empty());
        assert!(task.created_at.is_empty());
    }

    #[test]
    fn legal_transitions_are_allowed_and_illegal_ones_rejected() {
        use TaskStatus::*;
        // Forward pipeline moves.
        assert!(Backlog.can_transition_to(Todo));
        assert!(Todo.can_transition_to(InProgress));
        assert!(InProgress.can_transition_to(Evaluating), "agent done");
        assert!(Evaluating.can_transition_to(Review), "validation passed");
        assert!(Review.can_transition_to(Done));
        // Self-healing loop (Phase 8).
        assert!(
            Evaluating.can_transition_to(InProgress),
            "failed validation re-runs the agent"
        );
        assert!(
            Evaluating.can_transition_to(FailedNeedsHuman),
            "retries exhausted park the task"
        );
        // Auto-merge conflict resolution (Phase 9).
        assert!(
            Review.can_transition_to(ResolvingConflicts),
            "conflict at approve kicks off resolution"
        );
        assert!(
            ResolvingConflicts.can_transition_to(Evaluating),
            "agent resolved the markers; validate the result"
        );
        assert!(
            Evaluating.can_transition_to(ResolvingConflicts),
            "failed validation re-resolves the conflict"
        );
        assert!(
            ResolvingConflicts.can_transition_to(FailedNeedsHuman),
            "an unresolvable conflict parks the task"
        );
        assert!(
            ResolvingConflicts.can_transition_to(Todo),
            "crash during resolution releases the claim"
        );
        // Human-in-the-loop and recovery moves.
        assert!(Review.can_transition_to(Todo), "reject sends work back");
        assert!(
            FailedNeedsHuman.can_transition_to(Todo),
            "human can send a parked task back to the queue"
        );
        assert!(
            InProgress.can_transition_to(Todo),
            "crash releases the claim"
        );
        assert!(
            Evaluating.can_transition_to(Todo),
            "crash during validation releases the claim too"
        );
        assert!(
            Todo.can_transition_to(Backlog),
            "reopened prereq blocks again"
        );
        // Idempotent no-op.
        assert!(Done.can_transition_to(Done));

        // Illegal moves: skipping stages, demoting finished work, regressions.
        assert!(
            !Backlog.can_transition_to(InProgress),
            "must pass through TODO"
        );
        assert!(!Backlog.can_transition_to(Review));
        assert!(!Backlog.can_transition_to(Done));
        assert!(!Todo.can_transition_to(Review), "no skipping to REVIEW");
        assert!(!Todo.can_transition_to(Done));
        assert!(
            !Todo.can_transition_to(Evaluating),
            "validation only runs after an agent"
        );
        assert!(
            !InProgress.can_transition_to(Review),
            "code must pass validation before REVIEW"
        );
        assert!(!InProgress.can_transition_to(Done));
        assert!(
            !Evaluating.can_transition_to(Done),
            "no skipping human review"
        );
        assert!(
            !FailedNeedsHuman.can_transition_to(InProgress),
            "parked tasks go through the queue, not straight back to work"
        );
        assert!(!FailedNeedsHuman.can_transition_to(Review));
        assert!(!FailedNeedsHuman.can_transition_to(Done));
        // Conflict resolution can't skip validation or jump straight to DONE.
        assert!(
            !ResolvingConflicts.can_transition_to(Done),
            "a resolved conflict still needs validation + review"
        );
        assert!(
            !ResolvingConflicts.can_transition_to(Review),
            "resolution must pass through EVALUATING"
        );
        assert!(!ResolvingConflicts.can_transition_to(InProgress));
        // A conflict can't be entered from anywhere but REVIEW.
        assert!(!Todo.can_transition_to(ResolvingConflicts));
        assert!(!InProgress.can_transition_to(ResolvingConflicts));
        assert!(!Backlog.can_transition_to(ResolvingConflicts));
        assert!(!Done.can_transition_to(Backlog), "DONE is terminal");
        assert!(!Done.can_transition_to(Todo));
        assert!(!Done.can_transition_to(Review));
    }
}
