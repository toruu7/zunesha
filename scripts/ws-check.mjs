// Live WebSocket verification for the orchestrator daemon.
//
// Connects to /ws, triggers a run over REST, and asserts the event sequence:
// snapshot first, then task_updated (IN_PROGRESS → REVIEW) and log lines with
// dense 0-based seq numbers. Used by scripts/e2e.sh against a running daemon.
const PORT = process.env.PORT ?? "7432";
const BASE = `http://127.0.0.1:${PORT}`;

function fail(msg, extra) {
  console.error(`FAIL ${msg}`, extra ?? "");
  process.exit(1);
}
function pass(msg) {
  console.log(`  PASS ${msg}`);
}

const events = [];
const ws = new WebSocket(`ws://127.0.0.1:${PORT}/ws`);
const deadline = setTimeout(() => finish(), 15000);
let finished = false;

function finish() {
  if (finished) return;
  finished = true;
  clearTimeout(deadline);
  try {
    ws.close();
  } catch {
    // already closed
  }

  const snapshot = events.find((e) => e.type === "snapshot");
  if (!snapshot) fail("no snapshot event received on connect");
  if (!Array.isArray(snapshot.tasks) || snapshot.tasks.length === 0)
    fail("snapshot has no tasks", JSON.stringify(snapshot).slice(0, 200));
  pass(`snapshot on connect (${snapshot.tasks.length} tasks, ${snapshot.counts.length} count buckets)`);

  // The DAG view renders from the snapshot's edge list (Phase 11): the
  // planner's 3-task chain must arrive as 2 prerequisite → dependent edges.
  const deps = snapshot.dependencies;
  if (!Array.isArray(deps) || deps.length !== 2)
    fail("snapshot should carry the 2 dependency edges", JSON.stringify(deps));
  for (const e of deps) {
    if (typeof e.task_id !== "string" || typeof e.depends_on !== "string")
      fail("malformed dependency edge", JSON.stringify(e));
  }
  pass(`snapshot carries the dependency DAG (${deps.length} edges)`);

  const updated = events.filter((e) => e.type === "task_updated");
  const statuses = new Set(updated.map((e) => e.task?.status));
  if (!statuses.has("IN_PROGRESS")) fail("no IN_PROGRESS task_updated", [...statuses]);
  if (!statuses.has("REVIEW")) fail("no REVIEW task_updated", [...statuses]);
  pass(`task_updated stream covers IN_PROGRESS → REVIEW (${updated.length} events)`);

  const logs = events.filter((e) => e.type === "log");
  if (logs.length < 3) fail(`expected >=3 log lines, got ${logs.length}`);
  const lines = logs.map((l) => l.line);
  if (!lines.some((l) => l.includes("ws-live-1")) || !lines.some((l) => l.includes("ws-live-3")))
    fail("missing expected log lines", lines);
  pass(`log streaming: ${logs.length} lines received live`);

  const seqs = logs.filter((l) => l.task_id === logs[0].task_id).map((l) => l.seq);
  for (let i = 0; i < seqs.length; i++)
    if (seqs[i] !== i) fail(`seq not dense/0-based: ${JSON.stringify(seqs)}`);
  pass(`log seq dense & 0-based: ${JSON.stringify(seqs)}`);

  console.log("WS LIVE CHECKS PASSED");
  process.exit(0);
}

ws.addEventListener("open", async () => {
  const res = await fetch(`${BASE}/api/run`, { method: "POST" });
  if (!res.ok) fail(`/api/run -> ${res.status}`);
  pass("POST /api/run accepted");
});

ws.addEventListener("message", (raw) => {
  let ev;
  try {
    ev = JSON.parse(raw.data);
  } catch {
    return fail("non-JSON WS frame", String(raw.data).slice(0, 120));
  }
  events.push(ev);
  // Early exit once we have both transitions and all three log lines.
  const hasAll =
    events.some((e) => e.type === "task_updated" && e.task?.status === "REVIEW") &&
    events.filter((e) => e.type === "log").length >= 3;
  if (hasAll) setTimeout(finish, 300);
});

ws.addEventListener("error", () => fail("WebSocket error"));
