import { describe, expect, it } from "vitest";

import {
  applyEvent,
  countsFor,
  mergeLogLines,
  splitLogs,
  type LogLine,
} from "./reducer";
import type { ApiEvent, Task, TaskStatus } from "./types";

// --- fixtures -------------------------------------------------------------

function task(overrides: Partial<Task> & { id: string }): Task {
  return {
    title: `Task ${overrides.id}`,
    description: "",
    status: "TODO" as TaskStatus,
    retry_count: 0,
    worker_logs: "",
    diff: "",
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    ...overrides,
  };
}

function line(seq: number, text: string, stream: "stdout" | "stderr" = "stdout"): LogLine {
  return { seq, stream, line: text };
}

// --- splitLogs ------------------------------------------------------------

describe("splitLogs", () => {
  it("returns [] for empty input", () => {
    expect(splitLogs("")).toEqual([]);
  });

  it("indexes lines and drops the trailing newline artifact", () => {
    // The worker stores `line\nline\n`, so a naive split leaves a final "".
    expect(splitLogs("a\nb\nc\n")).toEqual([line(0, "a"), line(1, "b"), line(2, "c")]);
  });

  it("handles a single line", () => {
    expect(splitLogs("only\n")).toEqual([line(0, "only")]);
  });
});

// --- mergeLogLines --------------------------------------------------------

describe("mergeLogLines", () => {
  it("is idempotent: re-merging the same data changes nothing", () => {
    const persisted = [line(0, "a"), line(1, "b"), line(2, "c")];
    const once = mergeLogLines([], persisted);
    const twice = mergeLogLines(once, persisted);
    expect(twice).toEqual(once);
  });

  it("lets existing (streamed) lines win over duplicate seq", () => {
    const existing = [line(0, "a"), { seq: 1, stream: "stderr" as const, line: "B" }];
    const persisted = [line(0, "a"), line(1, "b")];
    const merged = mergeLogLines(existing, persisted);
    // seq 1 keeps the streamed stderr version, not the persisted stdout one.
    expect(merged[1]).toEqual({ seq: 1, stream: "stderr", line: "B" });
  });

  it("fills gaps from persisted data", () => {
    const existing = [line(0, "a"), line(2, "c")]; // seq 1 dropped in transit
    const persisted = [line(0, "a"), line(1, "b"), line(2, "c")];
    expect(mergeLogLines(existing, persisted).map((l) => l.line)).toEqual([
      "a",
      "b",
      "c",
    ]);
  });

  it("sorts by seq regardless of insertion order", () => {
    const merged = mergeLogLines([line(3, "d"), line(0, "a")], [line(2, "c"), line(1, "b")]);
    expect(merged.map((l) => l.seq)).toEqual([0, 1, 2, 3]);
  });
});

// --- countsFor ------------------------------------------------------------

describe("countsFor", () => {
  it("zero-fills all eight statuses in pipeline order", () => {
    const counts = countsFor([task({ id: "t1", status: "REVIEW" })]);
    expect(counts.map((c) => c.status)).toEqual([
      "BACKLOG",
      "TODO",
      "IN_PROGRESS",
      "EVALUATING",
      "REVIEW",
      "RESOLVING_CONFLICTS",
      "DONE",
      "FAILED_NEEDS_HUMAN",
    ]);
    expect(counts.find((c) => c.status === "REVIEW")?.count).toBe(1);
    expect(counts.find((c) => c.status === "TODO")?.count).toBe(0);
  });
});

// --- applyEvent -----------------------------------------------------------

describe("applyEvent", () => {
  it("snapshot seeds logs from persisted worker_logs", () => {
    const t = task({ id: "t1", worker_logs: "x\ny\n" });
    const ev: ApiEvent = { type: "snapshot", tasks: [t], counts: [] };
    const next = applyEvent({ tasks: [], logs: {} }, ev);
    expect(next.logs.t1.map((l) => l.line)).toEqual(["x", "y"]);
  });

  it("snapshot drops logs for tasks that no longer exist", () => {
    const state = {
      tasks: [task({ id: "t1" }), task({ id: "t2" })],
      logs: { t1: [line(0, "a")], t2: [line(0, "b")] },
    };
    const ev: ApiEvent = { type: "snapshot", tasks: [task({ id: "t1" })], counts: [] };
    const next = applyEvent(state, ev);
    expect(next.logs.t1).toBeDefined();
    expect(next.logs.t2).toBeUndefined();
  });

  it("log events append and are deduped by seq", () => {
    let state = applyEvent(
      { tasks: [], logs: {} },
      { type: "snapshot", tasks: [task({ id: "t1", status: "IN_PROGRESS" })], counts: [] },
    );
    const log = (seq: number, text: string): ApiEvent => ({
      type: "log",
      task_id: "t1",
      stream: "stdout",
      line: text,
      seq,
    });
    state = applyEvent(state, log(0, "first"));
    state = applyEvent(state, log(1, "second"));
    // Duplicate seq 0 must be a no-op.
    state = applyEvent(state, log(0, "first-again"));
    expect(state.logs.t1.map((l) => l.line)).toEqual(["first", "second"]);
  });

  it("entering IN_PROGRESS resets logs so two runs never mix", () => {
    // Simulate run 1 finishing (REVIEW) with its output…
    let state = applyEvent(
      { tasks: [], logs: {} },
      {
        type: "snapshot",
        tasks: [task({ id: "t1", status: "REVIEW", worker_logs: "run1-line\n" })],
        counts: [],
      },
    );
    expect(state.logs.t1.map((l) => l.line)).toEqual(["run1-line"]);

    // …then run 2 starts. The task_updated carries empty worker_logs.
    state = applyEvent(state, {
      type: "task_updated",
      task: task({ id: "t1", status: "IN_PROGRESS", worker_logs: "" }),
    });
    expect(state.logs.t1).toEqual([]);

    // Live lines from run 2 stream in.
    state = applyEvent(state, {
      type: "log",
      task_id: "t1",
      stream: "stdout",
      line: "run2-line",
      seq: 0,
    });
    expect(state.logs.t1.map((l) => l.line)).toEqual(["run2-line"]);
  });

  it("does not reset logs when a task_updated merely refreshes an IN_PROGRESS task", () => {
    let state = applyEvent(
      { tasks: [], logs: {} },
      {
        type: "snapshot",
        tasks: [task({ id: "t1", status: "IN_PROGRESS", worker_logs: "" })],
        counts: [],
      },
    );
    state = applyEvent(state, {
      type: "log",
      task_id: "t1",
      stream: "stdout",
      line: "mid-run",
      seq: 0,
    });
    // A second IN_PROGRESS update (e.g. timestamp refresh) must keep the line.
    state = applyEvent(state, {
      type: "task_updated",
      task: task({ id: "t1", status: "IN_PROGRESS", worker_logs: "" }),
    });
    expect(state.logs.t1.map((l) => l.line)).toEqual(["mid-run"]);
  });

  it("merges a task_updated's persisted logs without dropping streamed ones", () => {
    let state = applyEvent(
      { tasks: [], logs: {} },
      { type: "snapshot", tasks: [task({ id: "t1", status: "IN_PROGRESS" })], counts: [] },
    );
    // Stream line 2 but "miss" line 1 (dropped event).
    state = applyEvent(state, {
      type: "log",
      task_id: "t1",
      stream: "stdout",
      line: "b",
      seq: 1,
    });
    // task_updated flushes lines 0..1 to worker_logs.
    state = applyEvent(state, {
      type: "task_updated",
      task: task({ id: "t1", status: "IN_PROGRESS", worker_logs: "a\nb\n" }),
    });
    expect(state.logs.t1.map((l) => l.line)).toEqual(["a", "b"]);
  });

  it("an empty persisted buffer in a snapshot drops the previous run's logs", () => {
    // Phase 9: the approve-to-resolve path clears worker_logs with no status
    // event to ride on, so the snapshot carrying the reset must not merge the
    // old run into the fresh one (they'd collide at seq 0).
    let state = applyEvent(
      { tasks: [], logs: {} },
      {
        type: "snapshot",
        tasks: [task({ id: "t1", status: "REVIEW", worker_logs: "old-run-line\n" })],
        counts: [],
      },
    );
    expect(state.logs.t1).toHaveLength(1);

    state = applyEvent(state, {
      type: "snapshot",
      tasks: [task({ id: "t1", status: "RESOLVING_CONFLICTS", worker_logs: "" })],
      counts: [],
    });
    expect(state.logs.t1).toEqual([]);

    // The fresh run's lines stream in from seq 0.
    state = applyEvent(state, {
      type: "log",
      task_id: "t1",
      stream: "stdout",
      line: "resolving-line",
      seq: 0,
    });
    expect(state.logs.t1.map((l) => l.line)).toEqual(["resolving-line"]);
  });
});
