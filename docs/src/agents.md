# Configuring agents & the planner

Zunesha shells out for everything model-shaped: the **agent** that writes code,
the **planner** that decomposes a request, and the **validation** command that
decides whether a task may reach review. All three are configured with
environment variables read **at launch** (so set them before `zou run` /
`zou start`).

| Variable | Purpose | Default |
|---|---|---|
| `ZOU_AGENT_CMD` | Coding agent command (overrides the saved provider/model) | `opencode run --format json {prompt}` |
| `ZOU_AGENT_TIMEOUT_SECS` | Wall-clock limit for one agent attempt | `1800` |
| `ZOU_SANDBOX` | `guard` (validate + use `bwrap` when available) or `off` | `guard` |
| `ZOU_CONFIG_FILE` | User-wide config file path | `${XDG_CONFIG_HOME:-~/.config}/zou/config.json` |
| `ZOU_PLANNER_CMD` | Planning command (prompt on stdin, JSON on stdout) | built-in three-step chain |
| `ZOU_EVAL_CMD` | Test/lint command run in each workspace | auto-detect |
| `ZOU_PLANNER_TIMEOUT_SECS` | Wall-clock limit for one plan | `120` |
| `ZOU_EVAL_TIMEOUT_SECS` | Wall-clock limit for one validation run | `600` |
| `ZOU_DASHBOARD_DIR` | Built dashboard directory | auto-discover `web/out` |
| `ZOU_PORT` | Port for `zou start` (`--port` wins) | `57804` |
| `NEXT_PUBLIC_API_BASE` | Daemon origin when the UI runs under `next dev` | `127.0.0.1:57804` |

Commands are parsed with POSIX quoting (single/double quotes, backslash
escapes), so spaces in arguments are fine.

## Choosing a model (`zou model`)

The default agent is OpenCode. Instead of exporting a full command every
session, save the model once:

```sh
zou model              # list `opencode models`, prompt, save the choice
zou model --list       # just show the available ids
zou model --show       # show the saved model and config path
zou model --clear      # forget it and return to the built-in command
zou model opencode/glm-5.3-flash-free   # save a known id directly
```

The choice is user-wide (`${XDG_CONFIG_HOME:-~/.config}/zou/config.json`), so
it applies to every project. It resolves to:

```text
opencode run --auto --format json --model <model> {prompt}
```

`ZOU_AGENT_CMD` still wins when set; unset it to use the saved model. A running
`zou start` reads the setting at launch, so restart the daemon after changing
it.

### Switching to Google Antigravity

The provider and model are stored separately, so switching does not erase the
other provider's saved model:

```sh
zou agent                    # show active provider + resolved command
zou agent antigravity        # switch (keeps the OpenCode model saved)
zou agent antigravity --model gemini-3-pro
zou model                    # list `agy models`, save a choice
zou agent opencode           # switch back; the old model is still there
```

The built-in Antigravity command is:

```text
agy -p {prompt} --output-format stream-json --dangerously-skip-permissions --print-timeout 30m
```

`--dangerously-skip-permissions` is required for unattended work. The safety
boundary is Zunesha's isolated workspace, symlink validation, optional
`bwrap` jail, per-attempt timeout, cancellation, and human approval gate.
`agy` must be installed and authenticated; Zunesha does not manage its auth.
Its `stream-json` events are parsed into the technical **Logs** transcript
(tool calls) and the final Markdown **Output**, like OpenCode's JSON stream.
`ZOU_AGENT_CMD` overrides both providers.

The `--format json` flag is what lets Zunesha show the agent's final Markdown
response in the dashboard's **Output** tab instead of only the raw transcript.
The built-in command includes it automatically.

## Straw Hat Crew autonomous agents (`zou crew`)

Instead of sending every task to a generic agent with identical instructions,
Zunesha features the **Straw Hat Pirates crew** — five specialized autonomous
agents whose distinct personalities and technical philosophies tackle different
stages of your development pipeline:

| Crew Member | Role | Specialty & Focus |
|---|---|---|
| 👒 **Monkey D. Luffy** | Captain | **Feature Driver**: High-agency, pragmatic implementation of new capabilities. Writes direct, end-to-end working code without getting bogged down in analysis paralysis. |
| ⚔️ **Roronoa Zoro** | Combatant | **Technical Debt Slayer**: Relentless refactoring, performance optimization, compiler cleanliness (zero warnings), and cutting away redundant code. |
| 🗺️ **Nami** | Navigator | **Architect & Planner**: System mapping, contracts, schema design, intuitive interfaces, and structured dependency routing. |
| 🎯 **Usopp** | Sniper | **Edge-Case & Test Scout**: Anticipating failure modes, boundary condition handling, regression tests, and sniper error elimination before review. |
| 🍳 **Sanji** | Cook | **Polish & Review Presentation**: UX aesthetics, formatting, ergonomics, delightful outputs, and crafting git diffs that are clean for human review. |

### Delegated planning

When decomposing a feature request into a task DAG (whether using the built-in
planner or an external `ZOU_PLANNER_CMD`), Nami maps out the DAG and assigns
each task to the most suitable crew member (`task.crew_member`).

When a worker claims that task, the crew member's persona instructions are
injected directly into the agent's prompt, steering its behavior toward their
specialty.

### Configuring crew models & providers (zero hardcoding)

No model names are hardcoded. Every crew member can use their own distinct model
and provider, or automatically fall back to your default model:

```sh
# View the crew roster, active models, and specialties:
zou crew

# Configure a specific crew member:
zou crew zoro --model anthropic/claude-3.7-sonnet --provider opencode
zou crew nami --model gemini-2.5-pro --provider antigravity
zou crew luffy --model opencode/deepseek-v4.1-flash-free

# Reset a member back to the global default:
zou crew zoro --reset
```

#### Configuration file & environment variables

Crew settings are stored in `${XDG_CONFIG_HOME:-~/.config}/zou/config.json`:

```json
{
  "agent": {
    "provider": "opencode",
    "model": "opencode/deepseek-v4.1-flash-free"
  },
  "crew": {
    "zoro": {
      "provider": "opencode",
      "model": "anthropic/claude-3.7-sonnet"
    },
    "nami": {
      "provider": "antigravity",
      "model": "gemini-2.5-pro"
    }
  }
}
```

Environment variables provide per-shell overrides:
- `ZOU_CREW_<MEMBER>_MODEL` (e.g. `ZOU_CREW_ZORO_MODEL=anthropic/claude-3.7-sonnet`)
- `ZOU_CREW_<MEMBER>_PROVIDER` (e.g. `ZOU_CREW_NAMI_PROVIDER=antigravity`)

### Interactive Pixel Office Playground

The web UI includes an interactive pixel-art control room served at `/playground`:
- **Procedural Pixel-Art Sprites**: Distinct sprites for Luffy (straw hat, red vest, scar), Zoro (green hair, earrings, haramaki), Nami (orange hair, Log Pose), Usopp (goggles, dungarees, long nose), and Sanji (blonde hair, lit cigarette, black suit).
- **Character Desk Props**: Luffy's roast bone meat 🍖 and Jolly Roger terminal, Zoro's 3-katana rack and sake gourd 🍶, Nami's weather radar and mikan tangerines 🍊, Usopp's slingshot and inventor toolbox, and Sanji's coffee cup ☕ and chef pan.
- **Standby Roster**: When the task queue is empty, the office renders the 5 Straw Hats on standby ready for the next plan.

## The agent (`ZOU_AGENT_CMD`)

The agent runs **inside the task's workspace**, with an isolated git clone as
its working directory. The prompt is the task's title and description plus the
repository map. If you saved a model with `zou model`, the default command
already includes `--auto --format json --model <id>`.

Placeholders:

- `{prompt}` — the task prompt
- `{workspace}` — the workspace path

If the template never mentions `{prompt}`, it is appended as the final
argument (`opencode run {prompt}` relies on that). If it never mentions
`{workspace}`, none is passed — the working directory is already the
workspace.

```sh
ZOU_AGENT_CMD='opencode run --auto --format json {prompt}' zou run
ZOU_AGENT_CMD='aider --message "{prompt}"' zou run
ZOU_AGENT_CMD='./scripts/my-agent.sh {workspace}' zou start
```

Anything that edits files and exits 0 is a valid agent. It does **not** need
to commit: Zunesha captures the workspace diff itself.

The dashboard shows two views of an agent run: **Logs** is the technical
transcript (tool calls, stderr, `[eval]` output), and **Output** is the final
assistant response rendered as Markdown. Agents that emit OpenCode-compatible
JSON `text` events (or Antigravity `stream-json` results) populate Output; a
custom plain-text command still works, with its output visible in Logs.

## Prompt attachments

Prompts can carry images and arbitrary files. The web UI and playground accept
the picker, drag-and-drop, and pasted images; the CLI and TUI take paths:

```sh
zou plan "match the attached mock" --attach mock.png --attach spec.pdf
zou tui                 # A, or :attach <path>, before planning
```

Uploads are validated and written under `.zou/attachments/<id>/`, then an
`## Attachments` section with absolute paths is appended to the planner prompt
and to every resulting task description. The agent reads the files with its
normal file tool; images should be inspected directly. The workspace stays a
clean git clone (attachments are not copied into it). Under `bwrap` the host
is read-only, so attachments are readable but not modifiable.

Limits are enforced by both client and daemon: **10** files, **8 MiB** each,
**24 MiB** total. Invalid or oversized uploads fail with HTTP `400` before the
planner runs. Rejecting a task accepts attachments too, so a screenshot can
explain the requested fix.

## Validation (`ZOU_EVAL_CMD`)

After the agent exits 0, the task moves to `EVALUATING` and this command runs
in the workspace. On failure the output (tail-capped) is appended to the
prompt and the agent retries in the same workspace; after **3** failures the
task parks in `FAILED_NEEDS_HUMAN`.

```sh
ZOU_EVAL_CMD='sh -c "cargo test && npm test"' zou start
```

When unset, Zunesha auto-detects, first match wins:

1. `package.json` with a `test` script → `npm test --silent`
2. `Cargo.toml` → `cargo test --quiet`
3. `Makefile` with a `test:` target → `make test`
4. nothing → validation is skipped (exit 0 goes straight to `REVIEW`)

Mixed repositories should set the command explicitly — detection picks
exactly one runner by design.

## The planner (`ZOU_PLANNER_CMD`)

Planning turns one prompt into a DAG. Set `ZOU_PLANNER_CMD` to any command that

- reads the **planning prompt on stdin** (feature request + repository map +
  the schema), and
- prints **one JSON object on stdout**.

stderr is ignored for the plan; a non-zero exit fails the plan.

```sh
# CLIs that read stdin and answer on stdout:
ZOU_PLANNER_CMD='llm -m claude-sonnet-4' zou plan "add rate limiting"
ZOU_PLANNER_CMD='ollama run qwen2.5-coder' zou plan "add rate limiting"

# Any CLI whose prompt is an argument: wrap it.
cat > scripts/plan-with-claude.sh <<'EOF'
#!/bin/sh
exec claude -p "$(cat)"
EOF
ZOU_PLANNER_CMD='sh scripts/plan-with-claude.sh' zou start
```

The command runs with the **repository root as its working directory**, so
relative paths in the prompt are real. `ZOU_PLANNER_TIMEOUT_SECS` bounds one
run.

### Output schema

```json
{"tasks":[
  {"key":"schema","title":"Add the rate-limit table",
   "description":"Migration and model for per-key counters.","depends_on":[]},
  {"key":"middleware","title":"Enforce the limit",
   "description":"Token bucket in the request path, keyed by API key.","depends_on":["schema"]},
  {"key":"docs","title":"Document the limits",
   "description":"README section + API reference.","depends_on":["middleware"]}
]}
```

- `key` — short, unique in the plan, 1–64 chars of `[A-Za-z0-9._-]`
- `title` — the card label (capped at 200 chars)
- `description` — what the agent reads; make it stand alone (capped at 8 KB)
- `depends_on` — keys that must be `DONE` first; omit or `null` for parallel

The parser is forgiving: markdown fences and surrounding prose are fine (the
first JSON value wins), `dependsOn` works as an alias, a bare array works, and
a missing description falls back to the title.

Validation is strict, and **nothing is written until the whole plan passes**:

- unique keys,
- every dependency refers to a task in the same plan,
- no self-dependencies,
- an acyclic graph (checked with Kahn's algorithm),
- at most 32 tasks.

A bad plan fails `zou plan` with the reason (and `POST /api/plan` answers
`400`) — no half-built DAG is ever stored.

## Per-project configuration

Because variables are read at launch, a project-local `.envrc` (direnv) or a
shell alias works well. Use this when one project needs a different agent
command than the user-wide model selected with `zou model`:

```sh
# .envrc — everything a repo needs
export ZOU_AGENT_CMD='opencode run --auto --format json --model opencode/glm-5.3-flash-free {prompt}'
export ZOU_PLANNER_CMD='llm -m claude-sonnet-4'
export ZOU_EVAL_CMD='sh -c "cargo test && cargo clippy -- -D warnings"'
```

Then `zou run` and `zou start` pick them up with no flags.
