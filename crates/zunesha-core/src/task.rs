//! Task model shared across every phase of the orchestrator.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::Error;

/// Why a task stopped without reaching `REVIEW` (or why a review needs
/// attention). Persisted as a stable machine-readable value so the CLI, TUI,
/// and dashboard can all render the same badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// Provider quota, free-usage limit, or HTTP 429.
    RateLimit,
    /// Context/token/step ceiling reached.
    TokenLimit,
    /// The agent exited non-zero for another reason.
    AgentError,
    /// The project's verification command failed after its retry budget.
    VerificationFailed,
    /// The reviewed diff no longer applies cleanly against the repository.
    MergeConflict,
    /// No stdout or workspace file activity within the idle timeout.
    HungProcess,
}

impl StopReason {
    /// Every reason, in a stable order for UIs and tests.
    pub const ALL: [StopReason; 6] = [
        StopReason::RateLimit,
        StopReason::TokenLimit,
        StopReason::AgentError,
        StopReason::VerificationFailed,
        StopReason::MergeConflict,
        StopReason::HungProcess,
    ];

    /// The canonical string stored in SQLite and sent over the API.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RateLimit => "rate_limit",
            Self::TokenLimit => "token_limit",
            Self::AgentError => "agent_error",
            Self::VerificationFailed => "verification_failed",
            Self::MergeConflict => "merge_conflict",
            Self::HungProcess => "hung_process",
        }
    }

    /// Short human label for badges and status bars.
    pub fn label(self) -> &'static str {
        match self {
            Self::RateLimit => "rate limit",
            Self::TokenLimit => "token limit",
            Self::AgentError => "agent error",
            Self::VerificationFailed => "verification failed",
            Self::MergeConflict => "conflict",
            Self::HungProcess => "hung",
        }
    }
}

impl fmt::Display for StopReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for StopReason {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "rate_limit" => Ok(Self::RateLimit),
            "token_limit" => Ok(Self::TokenLimit),
            "agent_error" => Ok(Self::AgentError),
            "verification_failed" => Ok(Self::VerificationFailed),
            "merge_conflict" => Ok(Self::MergeConflict),
            "hung_process" => Ok(Self::HungProcess),
            other => Err(Error::UnknownStopReason(other.to_string())),
        }
    }
}

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
///    │          └────────┴─────── (human retry)  │       │
///    │                             │                        │ (merge conflict)
///    └─── (prereq reopened)        ▼                        │
///                          NEEDS_INTERACTION ◀── failures  │
///                                    │                    │
///                          RESOLVING_CONFLICTS ◀───────────┘
///                                    │  agent resolves markers, then
///                                    └─▶ EVALUATING ──▶ REVIEW (re-approve)
/// ```
///
/// `NEEDS_INTERACTION` is the recovery state: a task that hit a rate limit,
/// token/step cap, runtime error, verification exhaustion, or an idle timeout
/// is parked with its worktree intact. It never silently falls back to `TODO`;
/// a human chooses Continue, Re-run with feedback, or Reset & Restart.
///
/// `FAILED_NEEDS_HUMAN` remains for databases/records created before the
/// recovery state machine; new failures use `NEEDS_INTERACTION`.
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
    /// Work stopped and needs a human decision. The worktree and partial
    /// commits are preserved; the stop reason says why.
    NeedsInteraction,
    /// An approved diff conflicted with the changed repository; an agent is
    /// resolving the merge conflict markers in the workspace (Phase 9).
    ResolvingConflicts,
    /// Approved and merged back into the main project.
    Done,
    /// Legacy parked state from before `NEEDS_INTERACTION`; still readable and
    /// recoverable for existing installations.
    FailedNeedsHuman,
}

impl TaskStatus {
    /// Every status, in pipeline order.
    pub const ALL: [TaskStatus; 9] = [
        TaskStatus::Backlog,
        TaskStatus::Todo,
        TaskStatus::InProgress,
        TaskStatus::Evaluating,
        TaskStatus::Review,
        TaskStatus::NeedsInteraction,
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
            TaskStatus::NeedsInteraction => "NEEDS_INTERACTION",
            TaskStatus::ResolvingConflicts => "RESOLVING_CONFLICTS",
            TaskStatus::Done => "DONE",
            TaskStatus::FailedNeedsHuman => "FAILED_NEEDS_HUMAN",
        }
    }

    /// Whether this state needs a human decision before work can continue.
    pub fn needs_attention(self) -> bool {
        matches!(
            self,
            TaskStatus::NeedsInteraction | TaskStatus::FailedNeedsHuman
        )
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
    /// * `IN_PROGRESS|EVALUATING|RESOLVING_CONFLICTS→NEEDS_INTERACTION` –
    ///   classified stop; partial work is preserved
    /// * `REVIEW→TODO` – human rejected the work; it retries
    /// * `REVIEW→RESOLVING_CONFLICTS` – the approved diff conflicted with the
    ///   changed repository; an agent resolves the markers (Phase 9)
    /// * `RESOLVING_CONFLICTS→EVALUATING` – the agent finished resolving; the
    ///   result is validated
    /// * `EVALUATING→RESOLVING_CONFLICTS` – validation of a resolved conflict
    ///   failed; the agent re-resolves with the errors in its prompt
    /// * `NEEDS_INTERACTION|FAILED_NEEDS_HUMAN→TODO` – human sent a parked
    ///   task back to the queue (Continue, feedback, or hard reset)
    /// * `IN_PROGRESS|EVALUATING|RESOLVING_CONFLICTS→TODO` – a worker crashed
    ///   or was interrupted; release the claim (`zou recover`)
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
                | (InProgress, NeedsInteraction)
                | (Evaluating, NeedsInteraction)
                // Legacy parked state remains reachable for installations and
                // tests that predate the recovery state machine.
                | (InProgress, FailedNeedsHuman)
                | (Evaluating, FailedNeedsHuman)
                | (ResolvingConflicts, FailedNeedsHuman)
                | (Review, Done)
                | (Review, Todo)
                | (Review, ResolvingConflicts)
                | (ResolvingConflicts, Evaluating)
                | (Evaluating, ResolvingConflicts)
                | (ResolvingConflicts, NeedsInteraction)
                | (ResolvingConflicts, Todo)
                | (InProgress, Todo)
                | (Evaluating, Todo)
                | (NeedsInteraction, Todo)
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
            "NEEDS_INTERACTION" => Ok(TaskStatus::NeedsInteraction),
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
    /// Capped at [`MAX_EVAL_RETRIES`]; reset when a human resumes the task.
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
    /// Classified reason the last attempt stopped, if any.
    #[serde(default)]
    pub stop_reason: Option<StopReason>,
    /// True when a human must look before this task can move forward. This is
    /// also set on `REVIEW` when a merge conflict is detected proactively.
    #[serde(default)]
    pub requires_attention: bool,
    /// Tail of the last agent transcript, kept separately from the full logs
    /// so a recovery prompt and the dashboard badge have concise context.
    #[serde(default)]
    pub last_log_tail: String,
    /// Compact handoff summary (modified files + exported signatures) used by
    /// downstream DAG tasks and by Continue retries.
    #[serde(default)]
    pub handoff_summary: String,
    /// `YYYY-MM-DD HH:MM:SS` (UTC), set by the database.
    pub created_at: String,
    /// `YYYY-MM-DD HH:MM:SS` (UTC), set by the database.
    pub updated_at: String,
}

/// Failed evaluations allowed before a task parks in
/// [`TaskStatus::NeedsInteraction`] and asks a human.
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
            stop_reason: None,
            requires_attention: false,
            last_log_tail: String::new(),
            handoff_summary: String::new(),
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
            Some("chopper") => "🩺",
            Some("robin") => "📜",
            Some("franky") => "⚙️",
            Some("brook") => "🎻",
            Some("jinbe") => "🌊",
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
    fn stop_reasons_roundtrip_through_str() {
        for reason in StopReason::ALL {
            let parsed: StopReason = reason.as_str().parse().unwrap();
            assert_eq!(parsed, reason);
        }
    }

    #[test]
    fn all_statuses_have_distinct_canonical_strings() {
        let mut seen = std::collections::HashSet::new();
        for status in TaskStatus::ALL {
            assert!(seen.insert(status.as_str()));
        }
        assert_eq!(seen.len(), 9);
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
        assert_eq!(task.stop_reason, None);
        assert!(!task.requires_attention);
        assert!(task.last_log_tail.is_empty());
        assert!(task.handoff_summary.is_empty());
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
            Evaluating.can_transition_to(NeedsInteraction),
            "retries exhausted park the task"
        );
        // Recovery state machine.
        assert!(InProgress.can_transition_to(NeedsInteraction));
        assert!(ResolvingConflicts.can_transition_to(NeedsInteraction));
        assert!(NeedsInteraction.can_transition_to(Todo));
        assert!(FailedNeedsHuman.can_transition_to(Todo));
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
            ResolvingConflicts.can_transition_to(Todo),
            "crash during resolution releases the claim"
        );
        // Human-in-the-loop and recovery moves.
        assert!(Review.can_transition_to(Todo), "reject sends work back");
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
            !NeedsInteraction.can_transition_to(InProgress),
            "parked tasks go through the queue, not straight back to work"
        );
        assert!(!NeedsInteraction.can_transition_to(Review));
        assert!(!NeedsInteraction.can_transition_to(Done));
        assert!(
            !FailedNeedsHuman.can_transition_to(InProgress),
            "legacy parked tasks also go through the queue"
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
