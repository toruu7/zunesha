# Getting started

## Prerequisites

- **git** — Zunesha snapshots and merges through git.
- **Rust** (stable toolchain; the workspace uses edition 2021) to build from
  source — prebuilt binaries need nothing.
- **Node 20+ / npm** — only to build the dashboard (`web/`).
- **An agent CLI** — the default is `opencode`; any program that accepts a
  prompt works (see [Configuring agents & the planner](agents.md)).

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/toruu7/zunesha/master/install.sh | sh
# binary → ~/.local/bin/zou
# dashboard → ~/.local/web/out (found automatically by the daemon)
```

While the repository is private, run the installer from a checkout instead
(the raw URL above is not reachable without credentials):

```sh
sh install.sh --source --repo "$PWD"
```

Both routes install the dashboard with the binary.

`cargo install --git https://github.com/toruu7/zunesha.git zunesha-cli` works
too, but installs the binary only — see the
[reference](reference.md#install-with-cargo) for the dashboard step.

Or build in place:

```sh
cargo build --release                    # → target/release/zou
cd web && npm install && npm run build   # → web/out, served by the daemon
```

`install.sh` also understands prebuilt release tarballs (`--release-base`,
`--tarball`), a custom `--prefix`, `--profile`, `--no-dashboard`, and
`--uninstall`; run `sh install.sh --help` for the full list. The daemon
locates `<prefix>/web/out` by walking up from its own binary, so no
environment variables are needed.

## Initialize your project

```sh
cd /path/to/your-repo
zou init
```

`init` creates `.zou/` (SQLite state + workspaces), refuses to run outside a
git work tree unless `--force` is passed, and appends `.zou/` to the repo's
`.gitignore` so the orchestrator's state never shows up in `git status`. It is
idempotent — re-running preserves existing state.

## Plan

```sh
zou plan "add rate limiting"
zou status
```

With no planner configured you get a deterministic three-task chain
(*analyze → implement → test & document*): the head is `TODO`, the rest are
gated in `BACKLOG`. To get a real decomposition, point `ZOU_PLANNER_CMD` at an
LLM CLI — see
[the planner](agents.md#the-planner-zou_planner_cmd).

## Run, review, approve

```sh
zou run                 # every ready task, in parallel (--jobs N to change the cap)
zou status              # the head is REVIEW; dependents are still gated
zou diff <TASK_ID>      # the exact change, from the task's workspace
zou approve <TASK_ID>   # merge it into your working tree → DONE, next task promoted
```

Approval is the **only** write to your repository, and it is applied
uncommitted — `git diff` shows it, `git status` is clean of `.zou/`. Rejecting
happens in the dashboard: **Reject** with feedback sends the task back to
`TODO`, and the agent's next prompt carries your note.

Repeat *run → diff → approve* until the chain is `DONE`. `zou run --follow`
walks wave by wave and stops at each human gate for you.

## The dashboard

```sh
zou start               # http://127.0.0.1:57804
```

- **Board** — eight status columns, live via WebSocket.
- **DAG view** — an interactive graph of the plan: pan/zoom/drag, click a node
  to open the review modal. Edges are prerequisite → dependent, laid out by
  dependency depth.
- **Plan bar** — type a feature request to plan from the UI, then **Run**.
- Agent output streams into the task's log while it runs; validation output is
  prefixed `[eval]`.

The CLI and the daemon share `.zou/state.db`, so they can run side by side. One
caveat: on boot the daemon **resumes tasks a crash left claimed**, which would
collide with a live `zou run` in the same project — start it with
`zou start --no-resume` in that case.

## What lands on disk

```text
.zou/
├── state.db        # tasks, dependencies, workspaces, logs (SQLite)
└── ws/<task_id>/   # one git clone + agent work per task, reaped on approval
```

Approved diffs are applied to your working tree (never committed for you);
the repository's own history is yours to write.
