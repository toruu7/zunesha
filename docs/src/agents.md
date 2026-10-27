# Configuring agents & the planner

Zunesha shells out for everything model-shaped: the **agent** that writes code,
the **planner** that decomposes a request, and the **validation** command that
decides whether a task may reach review. All three are configured with
environment variables read **at launch** (so set them before `zou run` /
`zou start`).

| Variable | Purpose | Default |
|---|---|---|
| `ZOU_AGENT_CMD` | Coding agent command (overrides the saved model) | `opencode run {prompt}` |
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
opencode run --auto --model <model> {prompt}
```

`ZOU_AGENT_CMD` still wins when set; unset it to use the saved model. A running
`zou start` reads the setting at launch, so restart the daemon after changing
it.

## The agent (`ZOU_AGENT_CMD`)

The agent runs **inside the task's workspace**, with an isolated git clone as
its working directory. The prompt is the task's title and description plus the
repository map. If you saved a model with `zou model`, the default command
already includes `--auto --model <id>`.

Placeholders:

- `{prompt}` — the task prompt
- `{workspace}` — the workspace path

If the template never mentions `{prompt}`, it is appended as the final
argument (`opencode run {prompt}` relies on that). If it never mentions
`{workspace}`, none is passed — the working directory is already the
workspace.

```sh
ZOU_AGENT_CMD='opencode run --auto {prompt}' zou run
ZOU_AGENT_CMD='aider --message "{prompt}"' zou run
ZOU_AGENT_CMD='./scripts/my-agent.sh {workspace}' zou start
```

Anything that edits files and exits 0 is a valid agent. It does **not** need
to commit: Zunesha captures the workspace diff itself.

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
export ZOU_AGENT_CMD='opencode run --auto --model opencode/glm-5.3-flash-free {prompt}'
export ZOU_PLANNER_CMD='llm -m claude-sonnet-4'
export ZOU_EVAL_CMD='sh -c "cargo test && cargo clippy -- -D warnings"'
```

Then `zou run` and `zou start` pick them up with no flags.
