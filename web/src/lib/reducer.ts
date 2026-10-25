// Pure board-state reducer.
//
// Everything here is a plain function over plain data — no React, no sockets —
// so the merge/idempotency rules that keep the live view correct are unit
// testable. The store wires these to the WebSocket; tests drive them directly.

import type {
  ApiEvent,
  DependencyEdge,
  LogEvent,
  StatusCount,
  StreamKind,
  Task,
  TaskStatus,
} from "./types";
import { STATUSES } from "./types";

/** One line of agent output, keyed by its index within the run. */
export interface LogLine {
  seq: number;
  stream: StreamKind;
  line: string;
}

/** The slice of UI state that events mutate. */
export interface BoardState {
  tasks: Task[];
  logs: Record<string, LogLine[]>;
  /**
   * Dependency edges (`task_id` ← `depends_on`), carried by snapshots.
   * Immutable after planning, so only snapshots ever change them.
   */
  deps: DependencyEdge[];
}

export const EMPTY_STATE: BoardState = { tasks: [], logs: {}, deps: [] };

/**
 * Split a task's persisted `worker_logs` into indexed lines.
 *
 * The worker stores output as `line\nline\n`, so a trailing newline leaves a
 * final empty element we drop. Persisted lines carry no stream marker (the
 * worker interleaves stdout and stderr into one buffer), so they default to
 * `stdout` for styling; only live-streamed lines keep their true stream.
 */
export function splitLogs(text: string): LogLine[] {
  if (!text) return [];
  const lines = text.split("\n");
  if (lines[lines.length - 1] === "") lines.pop();
  return lines.map((line, seq) => ({ seq, stream: "stdout" as StreamKind, line }));
}

/**
 * Merge streamed lines with a persisted prefix, keyed by `seq`.
 *
 * Same index always means the same line (both derive from one ordered stream),
 * so this is idempotent: re-merging the same data changes nothing. Lines we
 * already have win over persisted duplicates; persisted lines fill any we
 * missed. Result is always sorted and gap-tolerant.
 */
export function mergeLogLines(existing: LogLine[], persisted: LogLine[]): LogLine[] {
  const bySeq = new Map<number, LogLine>();
  for (const l of existing) bySeq.set(l.seq, l);
  for (const l of persisted) if (!bySeq.has(l.seq)) bySeq.set(l.seq, l);
  return [...bySeq.values()].sort((a, b) => a.seq - b.seq);
}

/** Insert-or-ignore a single streamed line (a duplicate `seq` is a no-op). */
function withLogLine(lines: LogLine[] | undefined, ev: LogEvent): LogLine[] {
  const base = lines ?? [];
  if (base.some((l) => l.seq === ev.seq)) return base;
  return [...base, { seq: ev.seq, stream: ev.stream, line: ev.line }].sort(
    (a, b) => a.seq - b.seq,
  );
}

/** Replace a task in place, or append it if new. */
function upsertTask(tasks: Task[], task: Task): Task[] {
  const idx = tasks.findIndex((t) => t.id === task.id);
  if (idx === -1) return [...tasks, task];
  const next = tasks.slice();
  next[idx] = task;
  return next;
}

/** Per-status counts, always zero-filled across all eight statuses. */
export function countsFor(tasks: Task[]): StatusCount[] {
  const tally = new Map<TaskStatus, number>();
  for (const t of tasks) tally.set(t.status, (tally.get(t.status) ?? 0) + 1);
  return STATUSES.map((status) => ({ status, count: tally.get(status) ?? 0 }));
}

/**
 * Apply one {@link ApiEvent}, returning the next state.
 *
 * Never mutates its input, so it's safe to call from React state updates and
 * from tests alike.
 */
export function applyEvent(state: BoardState, ev: ApiEvent): BoardState {
  switch (ev.type) {
    case "snapshot": {
      // Authoritative full state. Merge each task's persisted prefix in, and
      // drop logs for tasks that no longer exist.
      const logs: Record<string, LogLine[]> = {};
      for (const task of ev.tasks) {
        const persisted = splitLogs(task.worker_logs);
        // An empty buffer means the worker just reset it for a new run (the
        // approve-to-resolve path clears the DB before its first line lands,
        // with no status event to ride on) — drop the previous run's lines
        // instead of merging them into the fresh one.
        logs[task.id] =
          persisted.length === 0
            ? []
            : mergeLogLines(state.logs[task.id] ?? [], persisted);
      }
      return { tasks: ev.tasks, logs, deps: ev.dependencies };
    }

    case "task_updated": {
      const prev = state.tasks.find((t) => t.id === ev.task.id);
      const tasks = upsertTask(state.tasks, ev.task);

      // Entering IN_PROGRESS means a fresh run: the worker cleared its buffer
      // and restarted `seq` at 0, so drop the previous attempt's lines rather
      // than mixing two runs together.
      const freshRun =
        ev.task.status === "IN_PROGRESS" && prev?.status !== "IN_PROGRESS";
      const base = freshRun ? [] : state.logs[ev.task.id] ?? [];
      const logs = {
        ...state.logs,
        [ev.task.id]: mergeLogLines(base, splitLogs(ev.task.worker_logs)),
      };
      return { tasks, logs, deps: state.deps };
    }

    case "log": {
      const logs = {
        ...state.logs,
        [ev.task_id]: withLogLine(state.logs[ev.task_id], ev),
      };
      return { ...state, logs };
    }
  }
}
