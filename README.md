# Zunesha (zou) — Local-first AI Coding Orchestrator

[![docs](https://img.shields.io/badge/docs-live-black?logo=vercel&logoColor=white)](https://zunesha-docs.vercel.app)

A local-first, parallelized AI coding orchestrator (in the spirit of *fold*).
You give it a feature request; it plans a **DAG of tasks**, spins up **isolated
workspaces**, runs **AI coding agents in parallel**, collects their **diffs**,
and — after **human approval** — merges approved work back into your project.

* **Backend / daemon / CLI:** Rust (`clap`, `tokio`, `axum`, `rusqlite`)
* **Frontend:** Next.js (App Router) + TypeScript + Tailwind + Zustand
* **State:** SQLite inside a hidden `.zou/` folder in the target repo

## Architecture rules

1. **Never overwrite user code directly.** All agent work happens in isolated
   workspaces under `.zou/ws/<task_id>`. The single exception is the
   integration step: the *reviewed* diff, applied to your working tree only
   when you approve (see [Integration](#integration-merging-approved-work-phase-7)).
2. **Strict DAG execution.** A task cannot start until every prerequisite is
   `DONE`.
3. **Local first.** Everything (state, dashboard, daemon) runs on localhost.
4. **Human in the loop.** Tasks pause in `REVIEW` until you approve them.

## Documentation

The full docs are an [mdBook](https://rust-lang.github.io/mdBook/) —
tutorials, agent/planner configuration, troubleshooting, and this README as
the complete reference. **Live at <https://zunesha-docs.vercel.app>**.

```sh
sh scripts/build-docs.sh        # fetch the pinned mdBook, build → docs/book/
cargo install mdbook --locked   # or install it once…
mdbook serve docs               # …and preview at http://localhost:3000
```

Every push builds the book in CI, and `master` is deployed by **Vercel's Git
integration** (`vercel.json` runs `scripts/build-docs.sh`, which fetches the
pinned mdBook binary). No secrets to configure: the project is linked to this
repository, so docs edits go live automatically. `mdbook serve docs` still
works locally with no setup.

## Usage

### Install

```sh
curl -fsSL https://raw.githubusercontent.com/toruu7/zunesha/master/install.sh | sh
```

The script tries the **prebuilt tarball** for your OS/arch first (binary +
dashboard), then falls back to a **source build** (`git clone` + `cargo
build`) on any miss. Every URL is a flag, so mirrors, forks, and local
checkouts work identically — flags go after `sh -s --`:

### Windows (native, no WSL)

Windows uses the PowerShell installer and the `zou-windows-amd64.zip` release:

```powershell
irm https://raw.githubusercontent.com/toruu7/zunesha/master/install.ps1 -OutFile "$env:TEMP\install-zou.ps1"
powershell -ExecutionPolicy Bypass -File "$env:TEMP\install-zou.ps1"
```

Or from a checkout:

```powershell
.\install.ps1
.\install.ps1 -Version v0.10.0
.\install.ps1 -Source -Repo https://github.com/you/zunesha
```

The installer puts `zou.exe` in `%LOCALAPPDATA%\Programs\Zunesha\bin`, the
dashboard next to it in `%LOCALAPPDATA%\Programs\Zunesha\web\out`, and adds the
`bin` directory to your **user** PATH. Open a new terminal, then run:

```powershell
zou --version
zou model
zou init
zou start
```

If the prebuilt zip is missing (for example an older tag), the script falls
back to a source build. That path needs Git, Rust (MSVC toolchain), and Node.js
22+; the prebuilt path needs none of them. The binary is unsigned, so
SmartScreen may warn on first run.

`zou update` uses the same PowerShell installer on Windows. It still needs
`git` to compare release tags; install Git for Windows if you want in-place
updates. Re-run the installer if you prefer a clean install.

### Unix

```sh
# …or run the installer from a checkout:
sh install.sh

# Build from a fork (skip the prebuilt; add --repo "$PWD" for a local checkout):
curl -fsSL https://raw.githubusercontent.com/toruu7/zunesha/master/install.sh \
  | sh -s -- --source --repo https://github.com/you/zunesha

# Pin a prebuilt release instead of "latest":
curl -fsSL https://raw.githubusercontent.com/toruu7/zunesha/master/install.sh \
  | sh -s -- --release-base https://github.com/toruu7/zunesha/releases/download --version v0.2.0

# Fully local:
sh install.sh --tarball file:///tmp/rel/zou-linux-amd64.tar.gz --prefix ~/.local
```

Releases are built by `.github/workflows/release.yml` on every `v*` tag
(`zou-linux-amd64`, `zou-macos-arm64`, `zou-macos-amd64`,
`zou-windows-amd64`; each archive holds the binary + `web/out`). While this
repository is **private**, the public URLs above are not readable — run the
installer from a checkout instead (`sh install.sh --source --repo "$PWD"` or
`.\install.ps1 -Source -Repo .`), or point `--repo` / `--release-base` at a
mirror you can authenticate to.

| Flag | Default | Purpose |
|------|---------|---------|
| `--prefix DIR` | `~/.local` | Binary → `DIR/bin/zou`, dashboard → `DIR/web/out`. |
| `--repo URL` | placeholder | Git repo for source builds. |
| `--release-base URL` | placeholder | Base for prebuilt tarballs (`[/VERSION/]zou-<os>-<arch>.tar.gz`). |
| `--version TAG` | — | Release tag (path segment for the tarball, or clone branch). |
| `--tarball URL` | — | Full tarball URL override. |
| `--source` | off | Skip the prebuilt and build from `--repo` instead. |
| `--profile NAME` | `release` | cargo profile for source builds (`dev` = much faster). |
| `--no-dashboard` | off | Daemon-only install. |
| `--no-modify-path` | off | Leave shell startup files alone; print the PATH line. |
| `--uninstall` | — | Remove the binary, dashboard, and managed PATH entry. |

The daemon finds the dashboard by walking up from the executable, so
`<prefix>/web/out` needs no environment variables. If `<prefix>/bin` is not
on PATH, the script adds an idempotent entry to the startup file(s) for your
shell — `~/.bashrc` plus the login file for bash, `~/.zshrc` and
`~/.zprofile` for zsh, `config.fish` for fish, or `~/.profile` otherwise —
and prints the command to reload the shell. Pass `--no-modify-path` to leave
startup files untouched. `--uninstall` removes the managed PATH entry too.
Source builds skip the dashboard with a warning when node/npm are absent.

### Updates

After installing, `zou update` checks the release tags in the source recorded
by the installer and compares them with the running binary:

```sh
zou update           # check; prompt before installing a newer release
zou update --check   # only report (no prompt, no install)
zou update --yes     # install without prompting
```

If the installed version is current it prints `✔ up to date`; when a newer
tag exists it shows the old/new versions and asks before rebuilding. A
running daemon keeps the old binary, so restart `zou start` afterwards.

The installer records its source in
`<prefix>/share/zou/install.json` (for example `~/.local/share/zou/install.json`).
If that checkout moved, override it:

```sh
zou update --repo /path/to/zunesha        # local checkout
zou update --repo git@github.com:you/zunesha.git
# or set ZOU_UPDATE_REPO once
```

`zou update` uses the same installer and prefix as the original install
(`install.sh` on Unix, `install.ps1` on Windows); pass `--prefix DIR` or
`--no-dashboard` to adjust the update.

### Versioning

Zunesha uses semantic versioning with one version shared by the Rust
workspace and dashboard. Every push that changes the project bumps
`[workspace.package].version` and gets a matching `vX.Y.Z` tag:

- **major** — breaking changes;
- **minor** — new features or behavior;
- **patch** — fixes, docs, tests, and housekeeping.

```sh
sh scripts/bump-version.sh minor   # major|minor|patch, or an explicit X.Y.Z
git add Cargo.toml Cargo.lock web/package.json web/package-lock.json
git commit -m "release: v0.10.0"
git tag v0.10.0
git push origin master v0.10.0     # tag triggers the release workflow
```

The script keeps `Cargo.toml`, `Cargo.lock`, `web/package.json`, and
`web/package-lock.json` in sync. `zou update` and
`.github/workflows/release.yml` both key off the tags.

### Install with cargo

```sh
cargo install --git https://github.com/toruu7/zunesha.git zunesha-cli
# or over SSH:
cargo install --git git@github.com:toruu7/zunesha.git zunesha-cli
```

This installs the **binary only** — Rust packages cannot ship the static
dashboard. Build it once from a checkout and point the daemon at it (or use
`install.sh`, which bundles both):

```sh
git clone https://github.com/toruu7/zunesha.git
cd zunesha/web && npm install && npm run build
export ZOU_DASHBOARD_DIR="$PWD/out"
```

A release build straight from a checkout is just `cargo build --release`
(the binary lands in `target/release/zou`).

### Build (from a checkout)

```sh
# Backend → ./target/debug/zou (add to PATH, or `cargo build --release`)
cargo build

# Dashboard: a static export the daemon serves (optional but recommended)
cd web && npm install && npm run build && cd ..
```

### Quick start

```sh
zou model                     # one-time: pick the OpenCode model (saved user-wide)
# or switch providers: zou agent antigravity && zou model
cd /path/to/your-repo
zou init                      # creates .zou/ (+ .gitignore entry)
zou plan "add rate limiting"  # 3-task chain (built-in) or a real DAG via ZOU_PLANNER_CMD
zou plan "match this mock" --attach mock.png  # images/specs are copied into .zou/attachments/
zou status                    # per-status counts + task ids
zou crew                      # view Straw Hat crew members & assigned models

# --- drive it from the terminal ---
zou run                       # agent runs on every ready (TODO) task, in parallel
zou status                    # head is now REVIEW; dependents still gated
zou diff <TASK_ID>            # inspect exactly what the agent changed
zou approve <TASK_ID>         # merge the reviewed diff → DONE, next task promoted
zou cancel <TASK_ID>          # stop a running agent + process tree, release task
zou remove <TASK_ID>          # remove a task and its workspace cleanly
# repeat run → diff → approve until the chain is DONE (rejecting a task —
# sending it back with feedback for another attempt — is a dashboard action)
git diff                       # the approved changes are in your working tree

# --- or drive it from the dashboard ---
zou start                     # http://127.0.0.1:57804
zou tui                       # attach the terminal UI to that daemon
# or let the TUI spawn the daemon when it is not running:
zou tui --start
```

The daemon serves the Kanban board: type a prompt in the plan bar, hit **Run**,
watch agent output stream live, then open a `REVIEW` card and **Approve** or
**Reject** it. The header names the repository being served and shows a
spinner for every running task; `zou run` mirrors that spinner in the terminal.
Same state as the CLI — they share `.zou/state.db` and can run side by side.

### Configure the agent

The default agent is OpenCode, executed with the task's workspace as the
working directory. Pick a model once and Zunesha remembers it for every
project:

```sh
zou model            # list `opencode models`, prompt, save the choice
zou model --list     # non-interactive list
zou model --show     # show the saved model and config path
zou model --clear    # forget it, back to the built-in default
```

`zou model` writes `${XDG_CONFIG_HOME:-~/.config}/zou/config.json` and uses:

```text
opencode run --auto --format json --model <model> {prompt}
```

The setting is read when `zou run` / `zou start` launch, so restart a running
daemon after changing it. `ZOU_AGENT_CMD` is an explicit per-shell override
and wins over the saved model; without either, the built-in command is
`opencode run --format json "<prompt>"`.

#### Google Antigravity

Zunesha can drive Google Antigravity's `agy` CLI instead of OpenCode. The
provider and its model are stored independently, so switching back does not
lose the other provider's saved choice:

```sh
zou agent                    # show the active provider + resolved command
zou agent antigravity        # switch provider (keeps OpenCode's model saved)
zou agent antigravity --model gemini-3-pro
zou model                    # lists `agy models` for the active provider
zou agent opencode           # switch back; its previous model is still there
```

Antigravity runs headless in the task workspace:

```text
agy -p {prompt} --output-format stream-json --dangerously-skip-permissions --print-timeout 30m
```

`--dangerously-skip-permissions` is required for unattended work; Zunesha's
own workspace isolation, symlink guard, timeout, and human approval gate are
the safety boundary. `stream-json` events are split into the technical **Logs**
transcript (tool calls) and the final Markdown **Output**, exactly like
OpenCode's JSON stream. `ZOU_AGENT_CMD` still overrides both providers.

#### Straw Hat autonomous agent crew (`zou crew`)

Rather than treating every task with generic, one-size-fits-all prompts, Zunesha
organizes execution around the **Straw Hat Pirates crew** — five autonomous
specialists with distinct technical directives:

* 👒 **Monkey D. Luffy** (*Captain / Feature Driver*): Bold, high-agency implementation. Direct problem solving that writes working code end-to-end without analysis paralysis.
* ⚔️ **Roronoa Zoro** (*Combatant / Technical Debt Slayer*): Relentless refactoring, performance optimization, compiler cleanliness (zero warnings), and cutting away dead code.
* 🗺️ **Nami** (*Navigator / Planner & Architect*): System topology mapping, data schemas, clean interfaces, contracts, and dependency graph routing.
* 🎯 **Usopp** (*Sniper / Edge-Case & Test Scout*): Failure mode anticipation, boundary condition tests, regression suites, and hunting down edge cases.
* 🍳 **Sanji** (*Cook / Review & Polish*): Ergonomics, UI/UX aesthetics, documentation, intuitive outputs, and crafting git diffs designed for human review.

##### Delegated planning & zero-hardcoded models

When planning with `zou plan` (built-in or `ZOU_PLANNER_CMD`), Nami tags each task
with its assigned crew member. When claimed, that member's persona prompt is
injected automatically into the workspace.

No model names are hardcoded. Each crew member can run with their own provider
and model (falling back to your global default model if unconfigured):

```sh
zou crew                                                      # inspect active roster
zou crew zoro --model anthropic/claude-3.7-sonnet --provider opencode  # configure a member
zou crew nami --model gemini-2.5-pro --provider antigravity   # assign Antigravity
zou crew zoro --reset                                         # return to default model
```

Settings persist in `${XDG_CONFIG_HOME:-~/.config}/zou/config.json` under `crew.<member>`.
Per-shell environment overrides are supported via `ZOU_CREW_<MEMBER>_MODEL` and
`ZOU_CREW_<MEMBER>_PROVIDER`.

##### Interactive Pixel Office Playground

Open `http://127.0.0.1:57804/playground` for the interactive pixel control room:
procedurally rendered pixel-art characters (Luffy's straw hat and red vest, Zoro's
katanas and earrings, Nami's Log Pose, Usopp's goggles, Sanji's cigarette) sitting
at desks with custom props (roast bone meat 🍖, katana rack & sake gourd 🍶,
weather cyclone radar 🍊, gadget blueprints, and porcelain espresso ☕). The
office shows all 5 Straw Hats on standby ready for work when the board is idle.

Use `ZOU_AGENT_CMD` when you want a different agent CLI or extra flags.
`{prompt}` and `{workspace}` are substituted after parsing (if the template
never mentions `{prompt}`, it is appended as the final argument):

```sh
ZOU_AGENT_CMD='aider --message "{prompt}"' zou run
ZOU_AGENT_CMD='./scripts/my-agent.sh {workspace}' zou start
ZOU_AGENT_CMD='opencode run --auto --format json {prompt}' zou run --jobs 5
```

A custom agent command still streams its output into Logs. To populate the
separate Markdown Output tab, it must emit OpenCode-compatible `text` events
(the built-in and saved-model commands already do).

The spec is parsed with full POSIX quoting (single/double quotes, backslash
escapes). The daemon reads it **at launch**, so set it before `zou start`.

### Prompt attachments (images and files)

Prompts can carry screenshots, mockups, specs, logs, or any other file. The
dashboard and playground support the file picker, drag-and-drop, and pasted
images; the CLI and TUI use paths:

```sh
zou plan "match the attached mock" --attach mock.png --attach api-spec.pdf
zou tui                       # A (or :attach <path>) before planning
```

Uploads are validated and written under `.zou/attachments/<id>/`. The planner
prompt and every resulting task description get an `## Attachments` section
with absolute paths, so the agent can read them with its normal file tool
(images should be inspected directly). The task workspace remains a clean git
clone; attachments are not copied into it.

Limits are enforced in both the browser and daemon: at most **10** files,
**8 MiB** each, **24 MiB** total. Rejected uploads return HTTP `400` without
starting a planner run. Under the strong Linux sandbox the host is read-only,
so attachments stay readable but cannot be modified by the agent.

### Configure the planner (LLM-backed)

`zou plan` turns a feature request into a DAG of tasks. The default is a
deterministic mock (a fixed 3-task chain); set **`ZOU_PLANNER_CMD`** to any
command that reads the planning prompt on **stdin** and prints one JSON object
on **stdout** — an LLM CLI, a wrapper script, anything:

```sh
# any CLI that reads stdin and answers on stdout works:
ZOU_PLANNER_CMD='llm -m claude-sonnet-4' zou plan "add rate limiting"
ZOU_PLANNER_CMD='ollama run qwen2.5-coder' zou plan "add rate limiting"
ZOU_PLANNER_CMD='sh scripts/plan-with-my-api.sh' zou start   # daemon reads it at launch
```

The prompt Zunesha sends contains the feature request, any **prior-agent
context** from uncommitted work (Phase 12), a **bounded repository map** (the
same tree agents get, so file paths are real) and the output schema. The
model must answer with **one JSON object**:

```json
{"tasks":[
  {"key":"schema","title":"Add the rate-limit table",
   "description":"Migration and model for per-key counters.","depends_on":[]},
  {"key":"middleware","title":"Enforce the limit",
   "description":"Token bucket in the request path, keyed by API key.","depends_on":["schema"]}
]}
```

* `key` — short unique slug, referenced by other tasks' `depends_on`
* `title` / `description` — what the agent sees; write descriptions that stand alone
* `depends_on` — keys that must be `DONE` first; omit for parallel work

Nothing is written until the draft passes validation: **unique keys, known
dependencies, no self-edges, an acyclic graph, at most 32 tasks**. A bad answer
fails `zou plan` with the reason (and the daemon answers `400`), so a confused
model can never leave a half-built DAG behind. Markdown fences and surrounding
prose are tolerated (the first JSON value wins), `dependsOn` is accepted as an
alias, `null`/missing `depends_on` means "no prerequisites", and a missing
description falls back to the title. `ZOU_PLANNER_TIMEOUT_SECS` caps one run
(default `120`); the command runs with the repo root as its working directory.

### Configure validation (self-healing loop)

After an agent exits 0, the daemon runs the project's **test/lint command**
inside the workspace before the task reaches `REVIEW` (Phase 8). The command
is resolved in this order:

1. **`ZOU_EVAL_CMD`** — an explicit command line, parsed with the same POSIX
   quoting as `ZOU_AGENT_CMD` (`{workspace}` is substituted; the working
   directory is the workspace either way):

   ```sh
   ZOU_EVAL_CMD='sh -c "cargo test && npm test"' zou start
   ```

2. **Auto-detection** from the workspace files — first match wins:
   * `package.json` with a `test` script → `npm test --silent`
   * `Cargo.toml` → `cargo test --quiet`
   * `Makefile` with a `test:` target → `make test`
   * nothing detectable → validation is skipped (exit-0 goes straight to `REVIEW`)

When validation fails, the task flips back to `IN_PROGRESS` automatically and
the agent re-runs in the same workspace with the failing output appended to
its prompt ("The code failed validation with these errors: … Please fix it.").
After **3 failures** the task parks in `FAILED_NEEDS_HUMAN`; open it in the
dashboard and either fix the code yourself or **Send back to queue** (which
resets the retry budget). The whole loop — tests, error capture, retry —
holds the worker's concurrency slot, so `--jobs` still bounds total load.

Validation output streams live into the task's logs, prefixed `[eval]`.
`ZOU_EVAL_TIMEOUT_SECS` caps one validation run (default `600`); a hung test
suite is killed and counts as a failure rather than wedging the daemon.

### Crash recovery

A killed daemon or interrupted `zou run` (Ctrl-C, crash, reboot) leaves tasks
stuck `IN_PROGRESS` (mid-agent), `EVALUATING` (mid-validation), or
`RESOLVING_CONFLICTS` (mid-auto-merge): they can't
be re-run or reviewed, and their dependents stay blocked.

**Restarting the daemon resumes them automatically** (Phase 11): `zou start`
detects the previous process's claims, releases each back to `TODO`, and
re-runs it in the kept workspace — announced on boot:

```
resumed 1 interrupted task(s): task-1729000000000000000-0
```

For claims a *live* worker may own — e.g. a concurrent `zou run` — release
them explicitly instead:

```sh
zou start --no-resume    # boot without touching existing claims
zou recover             # release every stuck task
zou recover <TASK_ID>   # release just one (errors if unknown or not stuck)
```

Or in the dashboard: open the stuck task → **Release stuck task**. The
workspace is kept, so the retry continues from whatever the crashed agent
already produced (the persisted retry counter still counts against the
3-failure cap). Nothing in the state can tell a live agent from a dead one —
that is why the manual path stays available and the boot-time resume can be
switched off.

### Environment variables

| Variable              | Read by          | Purpose                                        |
|-----------------------|------------------|------------------------------------------------|
| `ZOU_AGENT_CMD`      | CLI / daemon     | Agent command template; overrides the saved provider/model (see above). |
| `ZOU_AGENT_TIMEOUT_SECS` | CLI / daemon | Wall-clock limit for one agent attempt (default `1800`). |
| `ZOU_SANDBOX`        | CLI / daemon     | `guard` (default: validate + use `bwrap` when available) or `off`. |
| `ZOU_CONFIG_FILE`    | CLI / daemon     | Override the user-wide config path (default `${XDG_CONFIG_HOME:-~/.config}/zou/config.json`). |
| `ZOU_UPDATE_REPO`    | `zou update`     | Source checkout or Git URL to check instead of the install record. |
| `ZOU_PLANNER_CMD`    | CLI / daemon     | Planning command: prompt on stdin, JSON plan on stdout; unset = built-in planner (see above). |
| `ZOU_PLANNER_TIMEOUT_SECS` | CLI / daemon | Per-plan time limit (default `120`).          |
| `ZOU_EVAL_CMD`       | CLI / daemon     | Validation command template; unset = auto-detect (see above). |
| `ZOU_EVAL_TIMEOUT_SECS` | CLI / daemon  | Per-validation time limit (default `600`).     |
| `ZOU_DASHBOARD_DIR`  | daemon at start  | Built dashboard dir; else auto-discovers `web/out`. |
| `ZOU_PORT`           | `zou start`      | Port to bind (default `57804`); `--port` wins over it. |
| `NEXT_PUBLIC_API_BASE`| `next dev`/build | Daemon origin when the dashboard runs under Next's dev server. |

Ports and caps: `zou start --host --port --jobs` (`--port` falls back to
`ZOU_PORT`, default `57804`), `zou run --jobs`.

## Repository layout

```
zunesha/
├── Cargo.toml                      # cargo workspace + build profiles
├── install.sh                      # Unix installer (prebuilt → source)
├── install.ps1                     # Windows installer (zip → source)
├── docs/                           # mdBook site (guides + this README as reference)
├── .github/workflows/              # CI, release tarballs, docs deployment
├── crates/
│   ├── zunesha-core/          # state, DAG, workspaces, workers, merge
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── codebase_map.rs      # Phase 10: workspace map + prompt injection
│   │       ├── dag.rs               # promote_ready / blocked_tasks / recover_stuck
│   │       ├── db.rs                # SQLite schema + store (lifecycle policy)
│   │       ├── error.rs             # shared error type
│   │       ├── eval.rs              # validation command + self-healing loop helpers
│   │       ├── git.rs               # git detection helpers
│   │       ├── init.rs              # `zou init` implementation
│   │       ├── integrator.rs        # approval-time merge (the only write)
│   │       ├── paths.rs             # `.zou/` layout
│   │       ├── planner.rs           # built-in planner + external ZOU_PLANNER_CMD plans
│   │       ├── prior_context.rs     # Phase 12: prior work + instruction files
│   │       ├── task.rs              # TaskStatus lifecycle enum
│   │       ├── worker.rs            # bounded tokio agent pool + eval loop
│   │       └── workspace.rs         # isolated clones + diff capture
│   ├── zunesha-api/            # Axum REST + WebSocket server
│   │   └── src/
│   │       ├── lib.rs               # routes, state, static dashboard
│   │       ├── handlers.rs          # REST endpoints
│   │       ├── events.rs            # ApiEvent + snapshots
│   │       ├── playground.rs        # compact office projection (legacy/lightweight clients)
│   │       └── ws.rs                # live event stream
│   └── zunesha-cli/            # `zou` binary
│       └── src/
│           ├── main.rs              # CLI entrypoint
│           └── tui.rs               # ratatui terminal client
└── web/                             # Next.js dashboard → static `web/out`
    └── playground/                  # Vite pixel office → `web/out/playground`
```

## Target-repo layout (created by `zou init`)

```
<your-repo>/
└── .zou/
    ├── state.db      # SQLite: tasks, dependencies, workspaces, run journal
    ├── attachments/  # prompt uploads (bounded; readable by agents)
    └── ws/           # one isolated workspace per task
```

## CLI

```
zou init    [--path <PATH>] [--force]   # bootstrap .zou/ + SQLite
zou plan    "<prompt>" [--attach FILE] [--path <PATH>]  # plan a feature into a task DAG (built-in, or ZOU_PLANNER_CMD)
zou work    <TASK_ID> [--activate]      # spin up isolated workspace for a task
zou diff    <TASK_ID>                   # show the task workspace's diff
zou run     [--jobs N] [--follow]       # run the agent on ready tasks, in parallel
zou cancel  <TASK_ID>                   # stop a running agent + its process tree; release the task
zou remove  <TASK_ID> [--yes]           # remove a task and its workspace; unblock the DAG (alias: delete, rm)
zou events  <TASK_ID>                   # print the structured run/audit timeline
zou approve <TASK_ID>                   # merge reviewed work into the repo (auto-resolves conflicts, Phase 9)
zou recover [TASK_ID]                   # release stuck IN_PROGRESS/EVALUATING/RESOLVING_CONFLICTS tasks (crash recovery)
zou agent   [opencode|antigravity] [--model ID]  # show/switch the agent provider
zou model   [MODEL] [--list|--show|--clear]  # choose/save the active provider's model
zou crew    [MEMBER] [--model ID] [--provider P] [--reset]  # configure/view Straw Hat crew member models
zou update  [--check] [--yes] [--repo URL|PATH] [--prefix DIR] [--no-dashboard]
                                        # check release tags; prompt before updating
zou status  [--path <PATH>]             # per-status counts + task list
zou start   [--host H] [--port N] [--jobs N] [--no-resume]  # serve the local API + dashboard (port: --port or ZOU_PORT)
zou tui     [--url URL] [--start] [--theme NAME] [--no-mouse]
                                        # attach the interactive terminal UI (or start a daemon first)
```

`init` refuses to run outside a git work tree unless `--force` is passed, and
appends `.zou/` to the repo's `.gitignore` so Zunesha's state never
shows up in `git status`. `init` is idempotent — re-running preserves state.

## Task lifecycle & DAG engine

Tasks flow through eight statuses:

```text
BACKLOG ──▶ TODO ──▶ IN_PROGRESS ──▶ EVALUATING ──▶ REVIEW ──▶ DONE
   ▲          ▲         │  ▲            │        (reject) │
   │          │         │  └──retry─────┘             │   │
   │          └─────────┴───── (agent failed / rejected / sent back)
   │                            │ (3 failed validations)
   └── (prereq reopened)        ▼
                        FAILED_NEEDS_HUMAN ──(reject)──▶ TODO
```

`EVALUATING` is the Phase 8 self-healing gate: the project's test/lint command
runs in the workspace, failures re-run the agent with the error output in its
prompt (up to 3), and exhausted retries park the task for a human.

`RESOLVING_CONFLICTS` is the Phase 9 auto-merge gate, reached from `REVIEW`
when approval hits a merge conflict (nothing is written to your tree):

```text
REVIEW ──approve over a conflict──▶ RESOLVING_CONFLICTS ──agent ok──▶ EVALUATING ──tests green──▶ REVIEW
                                        │  ▲               │             (approve again)
                         (agent gave up)│  └──retry────────┘
                                        ▼    (≤3 failed tests)
                                 FAILED_NEEDS_HUMAN
```

The parked exit also covers a *third* failed validation during resolution.
A worker owns `RESOLVING_CONFLICTS` just like `IN_PROGRESS`/`EVALUATING`, so
`zou recover` releases a resolution that crashed mid-run too — see
[Auto-merge conflict resolution](#auto-merge-conflict-resolution-phase-9).

The **DAG engine** ([`dag::promote_ready`]) is the heartbeat: after any status
change it scans `BACKLOG` tasks and promotes to `TODO` the ones whose
prerequisites are all `DONE`. Because `DONE` is the only status that satisfies
a prerequisite, chains and diamonds resolve naturally. The engine never touches
in-flight work (`IN_PROGRESS`/`EVALUATING`/`REVIEW`/`RESOLVING_CONFLICTS`/
`FAILED_NEEDS_HUMAN`).

Dependency edges are guarded against cycles: `add_dependency` refuses any edge
that would close a loop, so the graph is a true DAG by construction.

The **planner** turns a prompt into a DAG. By default it is a deterministic
mock (Phase 2): a fixed three-step chain (`Analyze → Implement → Test &
document`). Set `ZOU_PLANNER_CMD` to hand planning to a real LLM — see
[Configure the planner](#configure-the-planner-llm-backed). Either way the
draft is validated (unique keys, known dependencies, acyclic, ≤ 32 tasks)
before the first task row is written.

### Integrity guarantees

Two invariants are enforced at the **storage layer**, so no later phase can
inadvertently violate them:

- **Lifecycle policy** — `Database::set_status` refuses any transition that
  `TaskStatus::can_transition_to` forbids (no stage-skipping, no demoting a
  `DONE` task). Legal moves are the forward pipeline plus the recovery/human
  moves: `REVIEW→TODO` (reject), `FAILED_NEEDS_HUMAN→TODO` (human frees a
  parked task), `EVALUATING→IN_PROGRESS` (self-healing retry),
  `IN_PROGRESS|EVALUATING|RESOLVING_CONFLICTS→TODO` (worker crashed),
  `REVIEW→RESOLVING_CONFLICTS` (approve hit a conflict, Phase 9),
  `RESOLVING_CONFLICTS↔EVALUATING` (resolve → validate → retry),
  `TODO→BACKLOG` (prereq reopened). `force_set_status` exists for tests and
  manual repair only.
- **Concurrency safety** — every connection sets WAL mode *and* a
  `busy_timeout`. Without the timeout, concurrent writers (daemon + CLI + web
  API, which is the normal case from Phase 4 on) silently lose writes to
  `SQLITE_BUSY`. An 8-writer × 50-write stress test goes from **130/400 rows
  written** to **400/400** with the timeout in place.

`Database` also exposes `update_task` (title/description; the reject flow
appends human feedback here) and `delete_task` (cascades dependency edges and
the workspace record).

## Isolated workspaces (Phase 3)

When a task starts work, `WorkspaceManager::create` snapshots the repository
into `.zou/ws/<task_id>`:

- **Copy source** comes from `git ls-files --cached --others --exclude-standard`
  — tracked files plus untracked files that are not ignored, so `.gitignore`
  is honored exactly. `.git` and `.zou` never leak into a workspace.
- **Own git repo**: the snapshot is committed as a *baseline* (using a
  dedicated `zunesha <zunesha@localhost>` identity, never the
  user's) and checked out on branch `zou/<task_id>`.
- **`generate_diff`** stages everything (`git add --all`, so brand-new files
  are included) and diffs the workspace against the baseline commit. This is
  what the review UI will show.
- **`CODEBASE_MAP.md`** (Phase 10) — the orientation tree handed to the
  agent, excluded via the workspace's *own* `.git/info/exclude` so it never
  shows in `git status`, the baseline, or any diff; see
  [Codebase map (Phase 10)](#codebase-map-phase-10).
- **`PRIOR_AGENT_CONTEXT.md`** (Phase 12) — a bounded record of uncommitted
  work and agent instruction files found in the source checkout, excluded
  the same way and injected into planner/worker prompts; see
  [Prior agent context (Phase 12)](#prior-agent-context-phase-12).
- Task ids are validated (`[A-Za-z0-9._-]`, no leading `.`) so a malicious id
  can never escape the `ws/` directory.

The user's real working tree is never modified: the smoke test confirms zero
dirt in `git status` after a full plan → work → edit → diff cycle.

## Agent orchestration (Phase 4)

`zou run` drives a bounded `WorkerPool` (tokio) that executes the AI agent on
every ready (`TODO`) task:

- **Concurrency cap** — a `tokio::sync::Semaphore` limits simultaneous agents
  (default 3, `--jobs N`). Tasks claim themselves (`IN_PROGRESS`) immediately,
  then wait for a slot; a marker-file stress test observes the true in-flight
  count and asserts it never exceeds the cap.
- **Isolated execution** — each agent runs as its own process *inside*
  `.zou/ws/<task_id>` (never the user's tree), with piped
  stdin/stdout/stderr. The child is pinned with `PWD`/`INIT_CWD`, and parent
  agent-session markers (`OPENCODE*`, `CLAUDECODE`, `CLAUDE_CODE_*`) are
  stripped, so starting `zou` from inside another agent cannot retarget the
  child at the caller's checkout.
- **Live log capture** — both streams are read line-by-line, broadcast on a
  `tokio::sync::broadcast` channel (the daemon relays this to the dashboard),
  and periodically flushed to the task's `worker_logs` column. OpenCode's
  `--format json` events are translated into that transcript; the final
  assistant message is also stored separately as Markdown in `agent_output`,
  so analysis-only tasks still show a readable result.
- **Exit handling** — a non-zero agent exit sends the task back to `TODO` for
  a human-guided retry; a zero exit starts the Phase 8 validation gate:
  `IN_PROGRESS → EVALUATING` → test command → `REVIEW` (pass) or back to
  `IN_PROGRESS` with the errors appended (fail, up to 3 times) — see
  [Self-healing validation (Phase 8)](#self-healing-validation-phase-8).
- **Agent command** — defaults to `opencode run --format json "<prompt>"`.
  `zou model` saves
  `opencode run --auto --format json --model <id> {prompt}` user-wide;
  `zou agent antigravity` switches to the `agy` headless command;
  `ZOU_AGENT_CMD` overrides all of them (parsed with full POSIX-style quoting
  — single/double quotes, backslash escapes — and `{prompt}` / `{workspace}`
  placeholders). A custom command that does not emit OpenCode's JSON events
  still works; its full output remains in Logs. Antigravity's `stream-json`
  events are parsed into the same separate Logs/Output split.
- **Containment** — before every attempt the worker canonicalizes the
  workspace and rejects symlinks that resolve outside it. On Linux, when
  `bwrap` is usable, the agent runs with the host filesystem read-only and
  only the workspace writable (plus a private `/tmp` and narrow agent cache
  dirs). This is a policy layer, not a claim that a hostile local process is
  fully contained; `ZOU_SANDBOX=off` disables the OS jail but keeps symlink
  validation. Logs and final output pass through secret redaction before they
  are streamed or persisted.
- **Timeout & cancellation** — each attempt has a wall-clock limit
  (`ZOU_AGENT_TIMEOUT_SECS`, default 30 minutes). `zou cancel <id>` (or the
  dashboard/TUI/playground cancel action) wakes the worker immediately and
  kills the whole process tree (`kill -TERM`/`-KILL` on the process group on
  Unix, `taskkill /T /F` on Windows). The task returns to `TODO` with a note
  explaining the timeout or cancellation; its workspace is kept.
- **Run journal** — every attempt writes a `runs` row plus append-only
  `run_events` (`run_started`, `agent_started`, `agent_finished`,
  `validation_*`, `status_changed`, `run_interrupted`, `run_finished`).
  `zou events <id>` and `GET /api/tasks/:id/events` replay the latest run for
  debugging, including runs that failed during workspace setup. Journal
  writes are best-effort and never take down the agent.
- **Prompt shape** — task title + description (including any
  `## Validation failure` tail from Phase 8), the conflict instruction when
  running in Phase 9's resolution mode, prior-agent context when the source
  checkout had uncommitted work (Phase 12), then the codebase map appended
  last (Phase 10 — reference material rides at the end; see
  [Codebase map (Phase 10)](#codebase-map-phase-10) and
  [Prior agent context (Phase 12)](#prior-agent-context-phase-12)).

Because `rusqlite::Connection` is not `Sync`, each worker opens its *own*
database connection rather than sharing one — safe now that Phase 3's audit
added WAL mode and a `busy_timeout`.

## Self-healing validation (Phase 8)

Never show a human code that doesn't compile or pass tests. After an agent
exit 0, the worker moves the task to `EVALUATING` and runs the project's
test/lint command inside the task's workspace (`ZOU_EVAL_CMD`, else
auto-detected — see [Configure validation](#configure-validation-self-healing-loop)):

- **Pass** → `REVIEW` with the diff captured, as before. If the task carried a
  stale failure section from an earlier retry, it's stripped first, so the
  reviewer sees the clean task text.
- **Fail** → the combined output (stderr+stdout, tail-capped at 8 KB) is
  appended to the task description as a `## Validation failure` section
  ("The code failed validation with these errors: … Please fix it."), the
  `retry_count` increments, and the same worker re-runs the agent in the same
  workspace — the next attempt's prompt contains the errors. Only the latest
  failure is kept in the prompt.
- **3 failures** → the task parks in `FAILED_NEEDS_HUMAN`. A human inspects
  the logs/diff, fixes the repo, or rejects it back to the queue (which resets
  the retry budget for a fresh cycle).
- **Timeout / spawn error** → counts as a failed validation (with the reason
  in the output), never a wedged worker; the eval subprocess is killed.
- Eval output streams into the same live log stream as the agent, prefixed
  `[eval]`, with a continuous `seq` across phases.

Everything happens inside the worker's concurrency slot: a task with two
retries occupies one slot the whole time, so `--jobs` still bounds total load.

## Auto-merge conflict resolution (Phase 9)

A reviewed diff can go stale while it waits for you: the repository may move
(a dependency bump, a teammate's merge, your own edit) so the patch no longer
applies. Before Phase 9 that meant a `409`, a manual rebase, and a retry.
Now the overlap is reconciled without you:

1. **Detect** — approval still starts with a `git apply --check` dry run. On
   failure *nothing* is written to your tree.
2. **Rebase the workspace** (`WorkspaceManager::prepare_conflict_resolution`)
   — inside the task's own git workspace (so this works even for
   `--force` plain-directory installs): the agent's work is committed, the
   workspace is rebuilt from the repository's *current* content, and the two
   are merged for real. Overlapping edits leave standard conflict markers;
   non-overlapping changes auto-merge silently. The recorded base commit is
   repointed at the new snapshot either way.
3. **Hand it to an agent** — the task moves `REVIEW → RESOLVING_CONFLICTS`
   and a worker runs in that workspace with the strict instruction:
   *"There are Git merge conflict markers in the code. Resolve them by keeping
   the new logic but ensuring it integrates correctly with the updated base
   code."* (The dashboard's approve handler and `zou approve` both spawn this
   agent automatically.)
4. **Re-enter the gate** — the result goes through the same
   `EVALUATING` validation loop as any run: green validation lands back in
   `REVIEW` for a *second* human approval (the merged-only-after-approval rule
   is untouched — the diff you approve then applies cleanly because it is now
   computed against the new base). A failed validation retries in place
   (`EVALUATING → RESOLVING_CONFLICTS`, up to 3); an agent that gives up, or
   a third failure, parks the task in `FAILED_NEEDS_HUMAN` with the markers
   still in the workspace for manual repair.

Only the pathological case — the rebase itself failing (git missing, a
corrupt workspace) — still surfaces as `409 merge conflict …` with the task
left in `REVIEW`, exactly like before Phase 9.

## Codebase map (Phase 10)

An agent dropped into a fresh workspace needs to know *what is there* before
it edits anything. Phase 10 gives it that orientation up front, in every
prompt, without a single tool call:

1. **Generate** — the `codebase_map` module walks the repository with the
   `ignore` crate (the engine behind ripgrep): `.gitignore` rules are
   honored inside a git repository and inert in plain `--force` directories —
   exactly mirroring what the workspace snapshot contains — hidden files are
   included, `.git` / `.zou` are skipped, and empty directories are
   pruned (the snapshot never copies them). Readable files get line counts;
   binaries, symlinks, and files over 512 KB show a size or label instead.
   The document itself is bounded — 8 KB, a 10,000-entry walk cap, and a
   16 MB read budget — so a pathological repository can neither slow
   workspace creation nor crowd the task out of a prompt:

   ````text
   # Codebase map

   Repository tree of this workspace's snapshot; gitignore rules apply when
   the source is a git repository. Generated scaffolding, git-excluded: it
   never enters a commit or a diff.

   ```text
   .
   ├── src/ (2 files · 3 lines)
   │   ├── lib.rs (2 lines)
   │   └── mod.rs (1 line)
   └── README.md (1 line)
   ```

   1 directory · 3 files · 4 lines
   ````

2. **Write it into the workspace** — `CODEBASE_MAP.md` lands in
   `.zou/ws/<task_id>` before the baseline commit, excluded through
   the workspace's *own* `.git/info/exclude` (your global and user exclude
   rules are untouched). It never enters `git status`, the baseline, a diff,
   or an approval; conflict prep regenerates it after rebuilding the
   snapshot, and a defensive `git rm --cached --ignore-unmatch` in diff
   generation and conflict prep drops it again even if an agent force-added
   it. Your real tree never sees the file.
3. **Inject it into the prompt** — the worker reads the installed map once
   per run and appends it *last* to every prompt: normal attempts and
   conflict-resolution runs alike (task text first, reference material
   last). Workspaces from before Phase 10 run map-less and get one on their
   next `create`.

The e2e sweep asserts the full path: the file exists in a live workspace,
the agent's prompt contains both the map heading and a tree entry, and
`CODEBASE_MAP.md` never appears in your repository after approvals.

## Prior agent context (Phase 12)

A repository is often already mid-change when you point Zunesha at it:
Claude Code, Cursor, Copilot, or a previous Zunesha run may have left
uncommitted edits, new files, and instruction notes. The workspace snapshot
contains that work, but the fresh baseline makes it invisible to `git diff`,
so a new agent can accidentally revert or duplicate it.

Phase 12 captures that state before the baseline commit:

- **Working-tree status and diff** — `git status --short` plus a unified diff
  against `HEAD` (staged and unstaged tracked changes together). Untracked
  files are listed by path and size, but their contents are deliberately not
  copied into the prompt (they are the most likely place for secrets); the
  agent can read them in the workspace when the task needs them.
- **Agent instruction files** — known bridge files are included as text:
  `AGENTS.md`, `CLAUDE.md`, `GEMINI.md`, `CODEX.md`, `.cursorrules`,
  `.windsurfrules`, `.clinerules`,
  `.github/copilot-instructions.md`, `.claude/CLAUDE.md`,
  `.codex/instructions.md`, plus markdown in `.cursor/rules/` and
  `.github/instructions/`. Binary and oversized files are skipped.
- **Bounded** — the document is capped at 32 KB (4 KB status, 18 KB diff,
  3 KB untracked list, 10 KB instructions), so reference material never
  crowds out the task. Truncation is called out in the document.
- **Scaffolding, not code** — `PRIOR_AGENT_CONTEXT.md` is written into the
  workspace before the baseline and excluded through `.git/info/exclude`,
  exactly like the codebase map. It never enters `git status`, a baseline,
  a diff, or an approval; conflict prep regenerates it for the rebuilt
  snapshot, and the defensive `git rm --cached` guard drops it even if an
  agent force-adds it. Your real tree never sees the file.

The planner receives it too: a plan for a partially implemented request is
asked to cover only the remaining delta instead of redoing completed work.
At task time the context is appended to the prompt just before the codebase
map, with the explicit instruction to build on the existing work, not
revert or redo it. Workspaces created before Phase 12 get the document on
their next `zou run`/`zou work` only when their snapshot still contains every
change the source checkout reports; a workspace that has diverged is left
alone (remove it, or let conflict prep rebuild it) rather than being told to
build on work it does not contain. A source checkout with no uncommitted
changes and no instruction files simply produces no document.

## Local API & WebSockets (Phase 5)

`zou start` binds an Axum server on `127.0.0.1:57804` (override with
`--host` / `--port`) that bridges the Rust state to the dashboard. It owns a
`WorkerPool`, so the API can start agents in parallel (capped at `--jobs`).

| Method | Path                     | Purpose                                    |
|--------|--------------------------|--------------------------------------------|
| GET    | `/health`                | Liveness probe.                            |
| GET    | `/api/status`            | Per-status counts + concurrency cap.       |
| GET    | `/api/tasks`             | Every task, the dependency DAG (`dependencies`), and per-status counts. |
| GET    | `/api/tasks/:id`         | One task (transcript, agent output, and diff included). |
| POST   | `/api/plan`              | `{"prompt": "...", "attachments": [{"name","media_type","data"}]}` → DAG of tasks (built-in or `ZOU_PLANNER_CMD`; `400` when the plan or an upload is invalid). `data` is raw base64 or a `data:` URL; body limit 40 MiB (the 24 MiB decoded attachment cap plus base64/JSON overhead). |
| POST   | `/api/run`               | Start agents on all ready (`TODO`) tasks.  |
| POST   | `/api/tasks/:id/run`     | Start the agent on one ready task.         |
| POST   | `/api/tasks/:id/cancel`  | Stop a live agent + process tree and release the task → `TODO`; `409` when this daemon does not own it. |
| GET    | `/api/tasks/:id/events`  | Latest run row + up to 500 decoded audit events. |
| POST   | `/api/tasks/:id/approve` | Merge approved diff → repo; `REVIEW→DONE` (a conflict queues auto-resolution, `REVIEW→RESOLVING_CONFLICTS`). |
| POST   | `/api/tasks/:id/reject`  | `{"feedback": "...", "attachments": [...]}` → back to `TODO` (from `REVIEW` or `FAILED_NEEDS_HUMAN`; resets the retry budget). |
| POST   | `/api/recover`           | Release stuck `IN_PROGRESS`/`EVALUATING`/`RESOLVING_CONFLICTS` tasks → `TODO`; `{"task_id": "..."}` for one, `{}` for all. |
| WS     | `/ws`                    | Live snapshots + streaming agent logs.     |

Key behaviors:

- **Human-in-the-loop gate** — `approve`/`reject` only work on `REVIEW` tasks;
  anything else returns `409 Conflict` (the lifecycle policy in Phase 2 is
  enforced at the storage layer, and the API maps `InvalidTransition → 409`,
  `TaskNotFound → 404`, `NotInitialized → 503`).
- **Approve merges first, then transitions.** The approved diff is applied to
  the working tree *while the task is still `REVIEW`*; only a clean merge
  proceeds to `REVIEW → DONE` + the DAG engine (the response's `promoted` lists
  what unlocked, `integration` reports what landed). When the diff no longer
  applies, **Phase 9 takes over**: nothing is written, the workspace is rebased
  onto the repository's current content with conflict markers, the response's
  `integration` is `{"status": "conflict_resolution_queued", "files": [...]}`,
  and the handler *immediately spawns the conflict-resolution agent* — the task
  returns to `REVIEW` after validation for a second approval. Only if the
  rebase itself fails does `409` `merge conflict …` surface (see
  [Auto-merge conflict resolution](#auto-merge-conflict-resolution-phase-9)).
- **Reject** appends the reviewer's feedback under a `## Reviewer feedback`
  heading on the task description (the next agent attempt sees why) and sends
  it `REVIEW → TODO`, leaving the workspace intact so the retry builds on the
  previous diff. Optional attachments are stored and referenced the same way
  as plan uploads.
- **Cancel** is deliberately in-memory: it only stops a worker owned by the
  current daemon. A task claimed by a dead process is released by
  `/api/recover` or boot resume instead.
- **Audit** — `/api/tasks/:id/events` returns the latest run and its decoded
  timeline. Events survive restarts; the endpoint is read-only.
- **Validation errors** — `SandboxViolation`, `Attachment`, invalid plans, and
  invalid transitions map to `400`/`409` with a JSON `error` message, never an
  HTML page.
- **WebSocket protocol** — every message is an `ApiEvent`, tagged by `type`:
  `snapshot` (full board — tasks, the `dependencies` edge list the DAG view
  renders, and counts; sent on connect and after every human mutation),
  `task_updated` (one row), and `log` (a live agent line). A client that falls
  behind the bounded (256-slot) event ring is not disconnected and not left
  with a silent gap: the server pushes it a fresh `snapshot` immediately, and
  the reducer merges snapshots idempotently (persisted `worker_logs` fill in
  any skipped lines). Dependency edges are immutable after planning, so only
  snapshots carry them.
- **Crash recovery** — on boot the daemon releases stuck `IN_PROGRESS` /
  `EVALUATING` / `RESOLVING_CONFLICTS` tasks back to `TODO` and re-runs them
  (opt out with `zou start --no-resume`); `POST /api/recover` does the release
  manually for claims a live worker may own (see [Usage](#crash-recovery)).
- **Routing** — unmatched `/api/*` paths return a JSON `404` (never the HTML
  dashboard fallback); other unmatched GETs serve `web/out` when it exists,
  otherwise a placeholder page.

Each handler opens its own short-lived database connection (same pattern as
the worker pool), so blocking SQLite work is pushed onto `spawn_blocking` to
keep the async runtime responsive.

## Dashboard (Phase 6)

`web/` is a Next.js (App Router, TypeScript, Tailwind v4, Zustand) single-page
app that the daemon serves as a **static export** — no Node server at runtime.

```sh
cd web && npm install && npm run build   # emits web/out, served by `zou start`
```

The Axum server auto-discovers `web/out` (override with `ZOU_DASHBOARD_DIR` or
`zou start --dashboard <dir>`); without a build it serves a placeholder page.

- **Board** — a fixed 4×2 desktop Kanban (`BACKLOG → TODO → IN_PROGRESS →
  EVALUATING` on the first row, `REVIEW → RESOLVING_CONFLICTS → DONE →
  FAILED_NEEDS_HUMAN` on the second), driven by WebSocket
  `snapshot`/`task_updated` events. Click a card to open the review modal.
  `FAILED_NEEDS_HUMAN` cards open with a **Send back to queue** action (with
  optional feedback, resetting the retry budget). On phones the board becomes
  a horizontally snapping column rail; controls and the modal go full-width,
  and the header collapses to a compact form. Tablet/desktop layouts are
  unchanged.
- **Attachments** — the plan bar accepts files, drag-and-drop, and pasted
  images, shows bounded chips with previews, and sends base64 in
  `POST /api/plan`. The review modal accepts attachments with rejection
  feedback. Limits are enforced client-side for instant feedback and again in
  the daemon.
- **Cancel** — live tasks expose **Cancel agent** (process-tree kill) beside
  the existing **Release stuck task** recovery path.
- **DAG view** (Phase 11) — a `board | dag` toggle in the header switches to an
  interactive [@xyflow/react](https://reactflow.dev) graph of the plan: nodes are
  tasks with the same status accents, edges are the dependency list drawn
  prerequisite → dependent, laid out left-to-right by dependency depth (a small
  pure layout in `web/src/lib/dag.ts`, no layout library). Pan/zoom/drag freely,
  click a node to open the same review modal — the graph repaints live as
  statuses change but never jumps while you're looking at it.
- **Review modal** — task description, the agent's **Output** rendered as
  styled Markdown, a colourised unified diff, a live technical **Logs** tab,
  and the human-in-the-loop controls: **Approve** (merges the diff into your
  working tree, then `REVIEW → DONE` + DAG promotion) or **Reject** (feedback
  appended to the description, task returns to `TODO` for a guided retry).
  Analysis-only tasks show their answer instead of an empty diff. Approving
  over a conflict (Phase 9) *keeps the modal open* on the task: an orange
  banner explains that the workspace was rebased, and the auto-merge agent's
  output streams live into the logs tab. Hard failures (the `409` fallback)
  are echoed *inside* the modal — the page-level banner sits behind its
  full-screen overlay.
- **Live logs** — the store merges streamed `log` events with each task's
  persisted `worker_logs` by `seq` (a 0-based line index), so the view is
  seamless across reconnects and never duplicates or drops lines. A claim
  (`TODO → IN_PROGRESS`, the start of a new run) resets the buffer so two
  runs never mix — as does an approval that starts a resolution run (an empty
  persisted buffer in a snapshot means "fresh run", not "no output yet");
  `[eval]` lines during `EVALUATING` append to the same stream. A running
  task also opens a **live logs dock** at the bottom of the board (and the
  review modal defaults to Logs while a worker owns the task); the header
  button toggles the dock for any selected task.
- **Planning** — a prompt bar posts to `/api/plan`; the Run button starts every
  ready (`TODO`) task.

Client logic that can silently break lives in `web/src/lib/reducer.ts` (pure,
unit-tested with Vitest) rather than inside components, so the merge and
idempotency rules are exercised by `npm test` in `web/`.

## Terminal UI

`zou tui` is a first-class client of the same REST + WebSocket protocol as the
dashboard. It attaches to an already-running daemon by default, so the browser
and terminal can watch and control the same run at once:

```sh
zou start                 # in one shell
zou tui                   # in another

zou tui --start           # or let the TUI spawn `zou start` when needed
zou tui --theme midnight  # one-off theme override
```

The layout has a task list, a detail pane (`logs`, `output`, `diff`, `task`),
and a status bar. Agent output stays on its own **output** tab, separate from
the live log/tool transcript. All actions go through the public API:

| Key | Action |
|-----|--------|
| `j` / `k`, `↑` / `↓` | select task |
| `tab` | cycle detail tabs |
| `l` / `o` / `d` / `i` | logs / output / diff / task |
| `p` | plan a feature |
| `A` | attach a file to the next plan (repeatable) |
| `r` / `R` | run all ready tasks / run selected task |
| `a` / `x` | approve / reject with feedback |
| `C` | cancel the selected running task (confirmation) |
| `c` | recover a stuck task |
| `g` | refresh from REST |
| `m` | model picker for the active provider (saves user-wide) |
| `f` | filter tasks |
| `t` | cycle theme |
| `:` | command palette (`:run`, `:attach <path>`, `:clear-attachments`, `:theme solarized`, …) |
| `?` | help |
| `q` / `Ctrl-C` | quit |

Customize it with `~/.config/zou/tui.toml` or a project-local
`.zou/tui.toml` (the project file wins):

```toml
theme = "midnight"          # paper | midnight | solarized | high-contrast | mono
mouse = true
follow_logs = true
layout = "auto"             # auto | split | stack
statusbar = ["project", "connection", "running", "review", "theme"]

[keys]
quit = "q"
plan = "p"
attach = "A"
run = "r"
run_one = "R"
approve = "a"
reject = "x"
cancel = "C"
recover = "c"
command = ":"

[colors]
accent = "#ffb000"
success = "#00dd88"
danger = "#ff5a5a"
```

The config is re-read every two seconds while the TUI is open, so theme,
keymap, status-bar, layout, and colour edits apply without restarting. `--theme`
still wins for that session. `layout = "auto"` stacks the panes below 100
columns, so narrow terminals and phone SSH sessions stay usable. The model
picker (`m`) lists `opencode models` or `agy models` for the active provider
and writes `${XDG_CONFIG_HOME:-~/.config}/zou/config.json`; restart a running
daemon after changing it. See [Terminal UI](docs/src/tui.md) for every key,
palette command, key-spec syntax, status-bar item, and theme.

## Playground (pixel office control room)

`zou start` also serves an interactive pixel office at
<http://127.0.0.1:57804/playground/>. Every task is an agent at a desk; the
office animates the lifecycle at a glance:

| Task status | Office state |
|-------------|--------------|
| `IN_PROGRESS`, `EVALUATING`, `RESOLVING_CONFLICTS` | working |
| `REVIEW` | waiting (review bubble) |
| `TODO` | idle |
| `BACKLOG` | sleeping |
| `DONE` | done / off duty |
| `FAILED_NEEDS_HUMAN` | error (needs-human bubble) |

It is a real control room, not a read-only monitor. Select a desk or task to
inspect its **Output** (styled Markdown), **Logs** (live stream), **Diff**, and
**Task** metadata. The inspector has the same lifecycle controls as the
dashboard and TUI: plan a feature, attach images/files, run all ready tasks,
run one task, approve, reject with feedback (plus attachments), cancel a
running agent, recover a stuck task, and refresh. Plan/run/reject/cancel
actions are confirmed, and errors stay visible in a dismissible banner.
Attachments support the picker, drag-and-drop, and pasted images.

On narrow screens the rail, office, and inspector stack; the topbar stays
compact, controls become full-width tap targets, and the reject modal becomes
a bottom sheet with safe-area padding.

The page consumes the full `GET /api/tasks` snapshot and the `/ws` stream, so
logs and output arrive live. The office renderer is adapted from
[pixel-agents-openclaw](https://github.com/isnardokun/pixel-agents-openclaw)
(MIT); the attribution and license live in `web/playground/`.

The normal `npm run build` emits both the dashboard and the playground:

```sh
cd web && npm run build   # Next export + Vite playground → web/out/playground
```

## Integration: merging approved work (Phase 7)

`integrator::approve_task` is the **only** code path that writes into your
repository, and it runs solely during an approval (API `POST /approve`, CLI
`zou approve`). It exists to make one promise true: *the diff you reviewed is
exactly the code that lands.*

1. **Fetch the patch** — regenerate it from the task's workspace (the reviewed
   copy stored on the task row is the fallback if the workspace is gone).
2. **Dry run** — `git apply --check` against the working tree. Git's
   `apply` is all-or-nothing, and the preflight means a non-applicable patch
   writes *nothing at all*. It also decides *how* the approval finishes: a
   conflict hands the task to **Phase 9 auto-resolution** (rebase the
   workspace onto the current content, agent reconciles the markers, back to
   `REVIEW` — see
   [Auto-merge conflict resolution](#auto-merge-conflict-resolution-phase-9));
   only a *failed rebase* (git missing, workspace corrupt) returns `409` /
   a non-zero exit with the files git blamed, leaving the task in `REVIEW`
   for manual reconciliation.
3. **Apply** — a clean dry run is followed by the real `git apply`, touching
   only working-tree files: no commits, no index writes, no history. Review the
   result with `git diff` and commit whenever you're ready. Unrelated uncommitted
   changes are untouched, and non-overlapping edits elsewhere in the same file
   apply fine (git matches hunks by context, at an offset if needed).
4. **Finish** — the task flips `REVIEW → DONE`, the DAG engine unlocks
   dependents, and the (now redundant) workspace is removed; the diff stays on
   the task row for the dashboard.

Conflict recovery (in practice only the rare rebase failure reaches you) —
pick any of:

- **Reconcile and re-approve.** Bring the files to a state where the reviewed
  diff applies (apply it by hand, or take the agent's version wholesale).
  Approving again first tries a normal dry run, then a *reverse* dry run — so
  work that is already present is detected and the approval simply completes
  instead of failing twice.
- **Reject with feedback** so the agent retries against its previous attempt's
  diff (the workspace survives rejection for exactly this reason).

Design notes: diffs carry `--binary`, so changed binaries replay as well as
text (a plain `git diff` would emit an unappliable marker); outside a git repo
(`zou init --force`) the same `git apply` path runs relative to the project
root; and overlapping-but-different changes are never silently landed — they
either go through the explicit, human-gated Phase 9 resolution above or fail
outright, because landing content nobody reviewed would break the promise at
the top of this section.

## Database schema (Phase 1)

```sql
tasks (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    description TEXT,
    status TEXT,           -- BACKLOG | TODO | IN_PROGRESS | EVALUATING | REVIEW | RESOLVING_CONFLICTS | DONE | FAILED_NEEDS_HUMAN
    retry_count INTEGER,   -- failed validations in the current cycle (Phase 8)
    worker_logs TEXT,      -- captured agent + [eval] transcript (Phase 4/8)
    agent_output TEXT,     -- final assistant response, Markdown (Output tab)
    diff TEXT              -- captured git diff (Phase 3)
)

dependencies (
    task_id TEXT,
    depends_on_id TEXT,
    PRIMARY KEY (task_id, depends_on_id)
)

workspaces (                              -- added in Phase 3
    task_id TEXT PRIMARY KEY,
    path TEXT,           -- .zou/ws/<task_id>
    branch TEXT,         -- zou/<task_id>
    base_commit TEXT     -- baseline for diffs
)

runs (                                    -- added in v0.10.0
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    mode TEXT NOT NULL,           -- normal | resolve_conflicts
    state TEXT NOT NULL,          -- RUNNING | final task status | ERROR
    started_at TEXT NOT NULL,
    ended_at TEXT,
    error_class TEXT,
    detail TEXT
)

run_events (                              -- added in v0.10.0
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    seq INTEGER NOT NULL,
    kind TEXT NOT NULL,           -- run_started | agent_started | …
    payload TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL,
    UNIQUE (run_id, seq)
)
```

Databases created before Phase 8 are migrated transparently on first open
(the `tasks` table is rebuilt to add `retry_count` and the new status
whitelist; all rows, dependency edges, and workspace records are preserved).

## Build & test

```sh
cargo build
cargo test          # core + API integration suites
cargo clippy --all-targets
cargo fmt --check

# Frontend
cd web && npm install
npm test            # reducer unit tests (Vitest)
npm run lint
npm run build       # static export → web/out

# End-to-end (needs a built `zou`; exercises git, the daemon, WebSockets)
sh scripts/e2e.sh
sh scripts/test-install.sh   # installer PATH + uninstall (fake tarball)
node scripts/bench.mjs   # runtime + memory benchmarks
```

## Performance & memory

Build profiles (root `Cargo.toml`) balance the three goals — fast iteration,
fast runtime, small footprint:

- **Dev builds** keep our crates at `opt-level = 0` with `line-tables-only`
  debug info (accurate panic/backtrace lines without full debuginfo's
  compile + link cost) while *dependencies* — SQLite included — compile at
  `opt-level = 2`. Deps build once and stay cached, so iteration speed is
  untouched while dev binaries run at release speed on DB paths (verified by
  a same-machine A/B: dev ≈ release within noise on every op).
- **Release** uses `lto = "thin"`, `codegen-units = 1`, `strip = "symbols"` —
  a slower cold build in exchange for a 4.5 MB binary. `panic = "unwind"` is
  kept on purpose: a panicking worker task must fail *that task*, not abort
  the daemon.

Measured on this machine (debug profile, `scripts/bench.mjs`):

| Metric                               | Value |
|--------------------------------------|-------|
| Incremental rebuild after a change   | **~10 s** (was ~23 s before profiles) |
| Cold `cargo build --workspace`       | ~231 s (one-time: all deps at O2) |
| Cold `cargo build --release`         | ~550 s → 4.5 MB stripped binary |
| `zou plan` (3 tasks) / `zou status`| ~28 ms / ~16 ms |
| `zou approve` incl. `git apply`     | ~135 ms |
| Daemon RSS idle                      | 3.8 MB (dev) / **2.4 MB** (release) |
| Daemon RSS, 20 agents streaming      | 5.8 MB (dev) / **4.6 MB** (release) |

Per-op latency is dominated by process + `git` subprocess spawns (each task
also pays one workspace `git clone`, ~60 ms warm); Zunesha's own
overhead is single-digit milliseconds. Memory stays flat under load: the
event bus is a 256-slot broadcast ring (a lagged client gets an immediate
fresh snapshot instead of accumulating backlog), logs flush to SQLite every
400 ms, and workers hold short-lived database connections — no resident
connection pool.

## Roadmap

| Phase | Scope                                              | Status |
|-------|----------------------------------------------------|--------|
| 1     | CLI foundation, `init`, SQLite schema              | ✅ done |
| 2     | DAG engine, state transitions, planner seam        | ✅ done |
| 3     | Workspace + git manager (isolated clones, diffs)   | ✅ done |
| 4     | Agent process orchestration (tokio worker pool)    | ✅ done |
| 5     | Axum REST API + WebSockets on `localhost:57804`    | ✅ done |
| 6     | Next.js dashboard (Kanban, live logs, review)      | ✅ done |
| 7     | Integration: merge approved diffs back             | ✅ done |
| 8     | Self-healing test loop (`EVALUATING`, retries)     | ✅ done |
| 9     | Auto-merge conflict resolution (`RESOLVING_CONFLICTS`) | ✅ done |
| 10    | Codebase map injected into agent prompts           | ✅ done |
| 11    | Interactive DAG view + automatic crash resumption  | ✅ done |
| 12    | Prior-agent context from uncommitted work          | ✅ done |
| 13    | Customizable terminal UI (ratatui, themes, keymaps) | ✅ done |
| 14    | Interactive pixel-office control room              | ✅ done |
| 15    | **v0.10.0 Safe by Default**: containment, timeout/cancel, run journal, Antigravity provider, prompt attachments, mobile layouts | ✅ done |
| 16    | v0.11.0 Recoverable: persistent sessions, requeue with feedback, stronger restart semantics | next |
| 17    | v0.12.0 Measurable: evals, cost/token accounting, policy checks | planned |
| 18    | v0.13.0 Coordinated: multi-agent handoffs, live steering protocol | planned |
| 19    | v1.0.0 Everywhere: stable interfaces, desktop/mobile parity, remote workers | planned |

Beyond the blueprint: a real **external planner** (`ZOU_PLANNER_CMD` — any LLM
CLI that reads a prompt on stdin and prints a JSON plan; see
[Configure the planner](#configure-the-planner-llm-backed)), a
**docs site** (mdBook: guides + this README as the reference — see
[Documentation](#documentation)), and an optional **Google Antigravity**
provider (`zou agent antigravity`).
