// Wire types mirroring the Rust API (`zunesha-core` / `zunesha-api`).
//
// These are kept structurally identical to the serde output of the daemon so a
// mismatch shows up as a TypeScript error (or a runtime `undefined`) rather
// than a silently-empty field. `TaskStatus` uses SCREAMING_SNAKE_CASE on the
// wire (see `TaskStatus`'s serde attr in task.rs).

/** Every status a task can hold, in pipeline order. */
export const STATUSES = [
  "BACKLOG",
  "TODO",
  "IN_PROGRESS",
  "EVALUATING",
  "REVIEW",
  "RESOLVING_CONFLICTS",
  "DONE",
  "FAILED_NEEDS_HUMAN",
] as const;

export type TaskStatus = (typeof STATUSES)[number];

/** Which stream a log line came from (snake_case on the wire). */
export type StreamKind = "stdout" | "stderr";

/** A single task row, exactly as the API serializes it. */
export interface Task {
  id: string;
  title: string;
  description: string;
  status: TaskStatus;
  /** Failed validations in the current review cycle (Phase 8, max 3). */
  retry_count: number;
  /** Full persisted worker transcript: tool activity, stderr, and `[eval]` output. */
  worker_logs: string;
  /** The agent's final Markdown response for the last run (may be empty). */
  agent_output: string;
  /** Unified diff captured when the task last reached REVIEW. */
  diff: string;
  created_at: string;
  updated_at: string;
}

/** The repository this daemon is working on. */
export interface ProjectInfo {
  /** Final path component, e.g. `saas-ops-hub`. */
  name: string;
  /** Absolute (or best-effort) repository root. */
  root: string;
}

/** Per-status count. Always emitted for all eight statuses (zero-filled). */
export interface StatusCount {
  status: TaskStatus;
  count: number;
}

/**
 * One edge of the task DAG: `task_id` may start only once `depends_on` is
 * DONE. Created with the plan and never mutated afterwards.
 */
export interface DependencyEdge {
  task_id: string;
  depends_on: string;
}

/**
 * One live log line.
 *
 * `seq` is the 0-based index of the line within the run. It lets the client
 * merge streamed lines with snapshot `worker_logs` idempotently: same index,
 * same line, no duplicates, and gaps (from a dropped event) stay visible.
 */
export interface LogEvent {
  task_id: string;
  stream: StreamKind;
  line: string;
  seq: number;
}

/** The single message protocol pushed over the WebSocket (and REST). */
export type ApiEvent =
  | {
      type: "snapshot";
      /** Which repository this daemon serves. */
      project: ProjectInfo;
      tasks: Task[];
      /** Dependency edges (`task_id` ← `depends_on`); see `dag.ts`. */
      dependencies: DependencyEdge[];
      counts: StatusCount[];
    }
  | { type: "task_updated"; task: Task }
  | ({ type: "log" } & LogEvent);

// --- REST response shapes -------------------------------------------------

export interface TasksResponse {
  project: ProjectInfo;
  tasks: Task[];
  /** Dependency edges (`task_id` ← `depends_on`), immutable after planning. */
  dependencies: DependencyEdge[];
  counts: StatusCount[];
}

export interface StatusResponse {
  project: ProjectInfo;
  counts: StatusCount[];
  max_concurrency: number;
  initialized: boolean;
}

/** What became of a task's approved diff when it was merged back. */
export type MergeOutcome =
  | { status: "no_changes" }
  | { status: "applied"; files: string[] }
  /** Conflict (Phase 9): workspace rebased, agent resolving it right now. */
  | { status: "conflict_resolution_queued"; files: string[] };

export interface ApproveResponse {
  task: Task;
  /** Tasks the DAG engine promoted to TODO as a consequence. */
  promoted: string[];
  /** Whether the diff was applied, had nothing to land, or was queued for auto-resolution. */
  integration: MergeOutcome;
}

export interface RecoverResponse {
  /** Tasks moved from IN_PROGRESS/EVALUATING/RESOLVING_CONFLICTS back to TODO (workspace kept). */
  released: string[];
}

export interface CancelResponse {
  task_id: string;
  cancelled: boolean;
}

export interface DeleteResponse {
  task_id: string;
  deleted: boolean;
}

/** A parsed API error (`{ error: string }` + HTTP status). */
export class ApiError extends Error {
  readonly status: number;

  constructor(status: number, message: string) {
    super(message);
    this.name = "ApiError";
    this.status = status;
  }
}
