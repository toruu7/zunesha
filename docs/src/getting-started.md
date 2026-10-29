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

If `~/.local/bin` is not already on PATH, the installer adds an idempotent
entry to your shell startup files and prints the reload command. Open a new
terminal (or run `exec "$SHELL" -l`) before running `zou`; pass
`--no-modify-path` to leave startup files untouched.

While the repository is private, run the installer from a checkout instead
(the raw URL above is not reachable without credentials):

```sh
sh install.sh --source --repo "$PWD"
```

Both routes install the dashboard with the binary.

### Windows

Use the PowerShell installer; it installs `zou.exe` and the dashboard under
`%LOCALAPPDATA%\Programs\Zunesha` and adds the `bin` directory to your user
PATH:

```powershell
irm https://raw.githubusercontent.com/toruu7/zunesha/master/install.ps1 -OutFile "$env:TEMP\install-zou.ps1"
powershell -ExecutionPolicy Bypass -File "$env:TEMP\install-zou.ps1"
```

From a checkout:

```powershell
.\install.ps1
.\install.ps1 -Version v0.9.0
.\install.ps1 -Source -Repo https://github.com/you/zunesha
```

The prebuilt path needs no Rust, Node, or Git. A source fallback needs Git,
Rust (MSVC toolchain), and Node.js 22+. Open a new terminal after installation
so PATH picks up `%LOCALAPPDATA%\Programs\Zunesha\bin`, then run `zou --version`.
The binary is unsigned, so SmartScreen may warn on first run.

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
`--uninstall`; run `sh install.sh --help` for the full list. `install.ps1`
mirrors those options on Windows (`-ReleaseBase`, `-Tarball`, `-Prefix`,
`-CargoProfile`, `-NoDashboard`, `-Uninstall`). The daemon locates
`<prefix>/web/out` by walking up from its own binary, so no environment
variables are needed.

## Update

Check for a newer release and install it:

```sh
zou update           # check, then prompt before updating
zou update --check   # report only
zou update --yes     # no prompt
```

The install records its source in `<prefix>/share/zou/install.json`.
`zou update` compares the running version with the highest `vX.Y.Z` Git tag
in that source, says `up to date` when there is nothing newer, and otherwise
runs the newer tag's `install.sh` (or `install.ps1` on Windows) after
confirmation. Use `--repo <checkout-or-url>` (or `ZOU_UPDATE_REPO`) if the
source moved, and restart `zou start` after updating. Windows updates need
`git` on PATH to compare tags. See
[versioning and release steps](reference.md#versioning) for how to publish a
new version.

## Choose a model

The default agent is OpenCode. Pick a model once — the choice is saved
user-wide and applies to every project:

```sh
zou model
```

It lists the ids reported by `opencode models`, prompts for a selection, and
stores the result in `${XDG_CONFIG_HOME:-~/.config}/zou/config.json`.
`zou model --list`, `--show`, and `--clear` manage it non-interactively. If
you already have a `ZOU_AGENT_CMD` export, that continues to override the
saved model.

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
- The review modal separates the final **Output** (styled Markdown) from the
  technical **Logs** (tool calls, stderr, and `[eval]` output), so analysis-only
  tasks still show a result even when they change no files.

The CLI and the daemon share `.zou/state.db`, so they can run side by side. One
caveat: on boot the daemon **resumes tasks a crash left claimed**, which would
collide with a live `zou run` in the same project — start it with
`zou start --no-resume` in that case.

## The terminal UI

```sh
zou tui                 # attach to the daemon at 127.0.0.1:57804
zou tui --start         # spawn a daemon first if none is reachable
zou tui --theme midnight
```

The TUI is a client of the same REST + WebSocket API: task list, logs, agent
output, diffs, plan/run/approve/reject/recover, a command palette, and a model
picker. It is customizable through `~/.config/zou/tui.toml` or a project
`.zou/tui.toml` — themes, keymaps, status-bar items, layout, mouse, and colors
are re-read while it runs. See [Terminal UI](tui.md) for the complete command,
key, and configuration reference.

## The pixel office

`http://127.0.0.1:57804/playground/` is an interactive pixel office: each task
is an agent working, waiting for review, idle, asleep in the backlog, done, or
blocked on a human. Click a desk or task to open the inspector, where you can
read the styled Markdown **Output**, the live **Logs**, the **Diff**, and the
task metadata. The same inspector can plan a feature, run ready tasks, run one
task, approve, reject with feedback, recover a stuck task, and refresh. The page
uses the full task snapshot and `/ws` stream; the normal dashboard build also
builds it.

## What lands on disk

```text
.zou/
├── state.db        # tasks, dependencies, workspaces, logs (SQLite)
└── ws/<task_id>/   # one git clone + agent work per task, reaped on approval
```

Approved diffs are applied to your working tree (never committed for you);
the repository's own history is yours to write.
