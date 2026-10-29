# Introduction

**Zunesha** (`zou`) is a local-first, parallelized AI coding orchestrator.
Give it a feature request and it:

1. **plans** a DAG of tasks — with the built-in planner or any LLM CLI you
   point it at (`ZOU_PLANNER_CMD`);
2. spins up an **isolated git workspace** per task under `.zou/ws/<task_id>`;
3. runs **coding agents in parallel**, one per ready task, bounded by `--jobs`;
4. **validates** each result with your test/lint command and retries with the
   failure text until it passes (self-healing);
5. parks every passing task in **`REVIEW`** and merges *only what you approve*
   back into your working tree — resolving conflicts automatically when two
   approved diffs overlap.

Nothing touches your files until you approve it, dependencies are strictly
gated, and everything — state, daemon, dashboard — runs on `127.0.0.1`.

## The four rules

| Rule | What it means |
|------|---------------|
| **Never overwrite user code** | Agents work in isolated clones. The only write is the reviewed diff, applied on approval. |
| **Strict DAG execution** | A task cannot start until every prerequisite is `DONE`. |
| **Local first** | SQLite in `.zou/`, a localhost daemon, no cloud state. |
| **Human in the loop** | Every task stops in `REVIEW` until a person approves it. |

## 30-second tour

```sh
zou model                             # pick the OpenCode model once (user-wide)
cd /path/to/your-repo
zou init                              # creates .zou/ (+ .gitignore entry)
zou plan "add rate limiting"          # DAG of tasks; the first head is TODO
zou run                               # agents run the ready tasks in parallel
zou status                            # head is REVIEW, dependents still gated
zou diff  <TASK_ID>                   # what the agent changed, exactly
zou approve <TASK_ID>                 # merge it → DONE, next task promoted
```

Prefer a UI? `zou start` serves the dashboard on
<http://127.0.0.1:57804> with a Kanban board, a live dependency graph, streamed
agent logs, and the approve/reject flow. The same daemon also serves a
interactive pixel office at `/playground/`, and `zou tui` attaches a customizable
terminal UI with the same controls.

## Where to go next

- **[Getting started](getting-started.md)** — install, first plan, first
  approval, running the dashboard.
- **[Configuring agents & the planner](agents.md)** — plug in your coding agent,
  your test command, and a real LLM planner.
- **[Troubleshooting](troubleshooting.md)** — stuck tasks, ports, merge
  conflicts, planner errors.
- **[Full reference](reference.md)** — the complete README: CLI, lifecycle,
  API/WebSocket protocol, database schema, and the design notes for every
  feature.
