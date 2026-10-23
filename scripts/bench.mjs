// Runtime + memory benchmarks for the orchestrator CLI and daemon.
//
// BusyBox `date` has no %N, so timings come from Node's hrtime and memory
// from /proc. Usage: node scripts/bench.mjs [path-to-zou]
import { execFileSync, spawn } from "node:child_process";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

// Absolute, so spawning with a per-bench `cwd` still finds the binary.
const ZOU = resolve(process.argv[2] ?? "target/debug/zou");

const run = (cmd, args, opts = {}) =>
  execFileSync(cmd, args, { stdio: "pipe", ...opts }).toString();

function git(dir, ...args) {
  run("git", args, { cwd: dir });
}

function scratchRepo() {
  const dir = mkdtempSync(join(tmpdir(), "zou-bench-"));
  git(dir, "init", "-q", ".");
  git(dir, "config", "user.name", "T");
  git(dir, "config", "user.email", "t@t");
  writeFileSync(join(dir, "a.txt"), "v1\n");
  git(dir, "add", "-A");
  git(dir, "commit", "-qm", "base");
  run(ZOU, ["init"], { cwd: dir });
  return dir;
}

function bench(label, n, fn, { warmup = true } = {}) {
  if (warmup) fn(); // warm-up (first call may pay one-time costs)
  const t0 = process.hrtime.bigint();
  for (let i = 0; i < n; i++) fn(i);
  const ms = Number(process.hrtime.bigint() - t0) / 1e6;
  console.log(`${label.padEnd(22)}: ${(ms / n).toFixed(1)}ms/op (${n} runs)`);
}

const proj = scratchRepo();

bench("init (idempotent)", 5, () => run(ZOU, ["init"], { cwd: proj }));
bench("plan (3 tasks)", 10, () => run(ZOU, ["plan", "add feature"], { cwd: proj }));
bench("status", 20, () => run(ZOU, ["status"], { cwd: proj }));
bench("work (new task ws)", 5, (i) => {
  // Plan one task, then create its workspace.
  run(ZOU, ["plan", `ws ${i}`], { cwd: proj });
  const id = run(ZOU, ["status"], { cwd: proj })
    .split("\n")
    .map((l) => l.trim())
    .find((l) => /^TODO\s+task-/.test(l))
    ?.split(/\s+/)[1];
  run(ZOU, ["work", id], { cwd: proj });
});

// Agent + run + approve loop on a fresh project, end to end. The agent
// writes its own file (workspace basename = task id) so approvals merge
// cleanly instead of conflicting, and `approve` exercises the full git apply
// path.
const p2 = scratchRepo();
const agentEnv = {
  ...process.env,
  ZOU_AGENT_CMD: 'sh -c "echo v2 > {workspace}/$(basename {workspace}).txt" {workspace}',
};
for (let i = 0; i < 5; i++) run(ZOU, ["plan", `wave ${i}`], { cwd: p2 });
bench(
  "run (5 agents, wave)",
  1,
  () => run(ZOU, ["run"], { cwd: p2, env: agentEnv }),
  { warmup: false },
);
const reviewIds = run(ZOU, ["status"], { cwd: p2 })
  .split("\n")
  .map((l) => l.trim())
  .filter((l) => /^REVIEW\s+task-/.test(l))
  .map((l) => l.split(/\s+/)[1]);
if (reviewIds.length !== 5) throw new Error(`expected 5 REVIEW tasks, got ${reviewIds.length}`);
let approveIdx = 0;
bench(
  "approve + git apply",
  5,
  () => run(ZOU, ["approve", reviewIds[approveIdx++]], { cwd: p2 }),
  { warmup: false },
);

// Daemon memory: idle, then under a 20-task run with streaming logs.
const p3 = scratchRepo();
for (let i = 0; i < 20; i++) run(ZOU, ["plan", `burst ${i}`], { cwd: p3 });
const slowAgent = join(tmpdir(), "bench-agent.sh");
writeFileSync(
  slowAgent,
  "#!/bin/sh\ni=0\nwhile [ $i -lt 20 ]; do echo \"line $i with realistic padding here\"; i=$((i+1)); done\nsleep 0.5\n",
);
const daemon = spawn(ZOU, ["start", "--port", "7434"], {
  cwd: p3,
  env: { ...process.env, ZOU_AGENT_CMD: `sh ${slowAgent} {workspace}` },
  stdio: "ignore",
});
const proc = (field) =>
  execFileSync("sh", [
    "-c",
    `awk '/${field}:/ {print $2}' /proc/${daemon.pid}/status`,
  ])
    .toString()
    .trim();

for (let i = 0; i < 50; i++) {
  try {
    const res = await fetch("http://127.0.0.1:7434/health");
    if (res.ok) break;
  } catch {
    // not up yet
  }
  await new Promise((r) => setTimeout(r, 200));
}
console.log(`daemon idle RSS      : ${(proc("VmRSS") / 1024).toFixed(1)}MB`);
await fetch("http://127.0.0.1:7434/api/run", { method: "POST" });
await new Promise((r) => setTimeout(r, 1500));
console.log(`daemon busy RSS      : ${(proc("VmRSS") / 1024).toFixed(1)}MB (20 agents streaming)`);
console.log(`daemon peak RSS      : ${(proc("VmHWM") / 1024).toFixed(1)}MB`);
daemon.kill();
