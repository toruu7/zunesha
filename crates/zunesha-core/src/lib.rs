//! Core library for the local-first AI coding orchestrator.
//!
//! This crate owns all state primitives: the SQLite-backed task store, the
//! task/dependency model, and the filesystem layout of the `.zou`
//! control directory that lives inside the user's target repository.
//!
//! Phase 1 scope:
//! * [`paths`] – resolution of the `.zou` directory layout.
//! * [`db`] – SQLite schema bootstrap and handle.
//! * [`task`] – `TaskStatus` enum shared across every later phase.
//! * [`git`] – lightweight git detection helpers.
//! * [`init`] – the `zou init` implementation.
//!
//! Phase 7 added [`integrator`] — the one component allowed to write into the
//! user's working tree, and only during a human approval.
//!
//! Phase 8 added [`eval`] — the project-validation step that gates `REVIEW`
//! and feeds failing test output back to the agent.
//!
//! Phase 9 added [`workspace::ConflictPrep`] — the 3-way merge that rebases a
//! conflicted workspace onto the repository's current content so an agent can
//! resolve markers automatically (see [`task::TaskStatus::ResolvingConflicts`]).
//!
//! Phase 10 added [`codebase_map`] — `CODEBASE_MAP.md`, a bounded repository
//! tree written into every workspace (git-excluded, so it never reaches a
//! diff) and appended to every agent prompt.
//!
//! Phase 12 added [`prior_context`] — `PRIOR_AGENT_CONTEXT.md`, a bounded
//! record of uncommitted work and instruction files another agent left in the
//! source checkout, injected into planner and worker prompts.

mod child_env;

pub mod attachments;
pub mod codebase_map;
pub mod config;
pub mod control;
pub mod dag;
pub mod db;
pub mod error;
pub mod eval;
pub mod git;
pub mod init;
pub mod integrator;
pub mod paths;
pub mod planner;
pub mod prior_context;
pub mod sandbox;
pub mod task;
pub mod worker;
pub mod workspace;

pub use attachments::{
    append_attachments, attachment_block, save_attachments, AttachmentUpload, SavedAttachment,
    MAX_ATTACHMENTS, MAX_ATTACHMENT_BYTES, MAX_TOTAL_ATTACHMENT_BYTES,
};
pub use config::{normalize_agent_model, normalize_model, AgentConfig, AgentProvider, Config};
pub use control::{RunControl, RunRegistry};
pub use dag::{blocked_tasks, promote_ready, recover_stuck};
pub use db::{Database, RunEvent, RunRecord, WorkspaceRecord};
pub use error::{Error, Result};
pub use eval::{EvalCommand, EvalOutcome};
pub use init::{init_project, InitReport};
pub use integrator::{approve_task, ApproveOutcome, Integrator, MergeOutcome};
pub use paths::ZuneshaPaths;
pub use planner::{plan, plan_with, plan_with_attachments, Planner, MAX_PLAN_TASKS};
pub use sandbox::{redact_secrets, SandboxMode, SandboxPolicy};
pub use task::{Task, TaskStatus, MAX_EVAL_RETRIES};
pub use worker::{AgentCommand, StreamKind, WorkerEvent, WorkerPool, DEFAULT_MAX_CONCURRENCY};
pub use workspace::{branch_name, ConflictPrep, Workspace, WorkspaceManager};
