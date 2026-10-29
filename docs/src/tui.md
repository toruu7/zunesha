# Terminal UI

`zou tui` is a thin terminal client for the same REST + WebSocket API as the
dashboard. It is useful when you want to stay in the terminal without giving up
the review workflow: task list, live logs, the agent's final Markdown answer,
the captured diff, plan/run/approve/reject/recover, themes, and a command
palette.

It does **not** run agents itself. The daemon (`zou start`) owns the worker
pool and the SQLite state, so the TUI can attach and detach without stopping a
run.

## Start or attach

```sh
zou start                 # terminal 1: API + dashboard + worker pool
zou tui                   # terminal 2: attach to 127.0.0.1:57804
```

If no daemon is running, let the TUI spawn one:

```sh
zou tui --start
```

The TUI waits for the daemon to become healthy, then attaches. It uses
`http://127.0.0.1:57804` by default.

```sh
zou tui --url http://127.0.0.1:57804
zou tui --theme midnight
zou tui --no-mouse
```

| Flag | Default | Meaning |
| --- | --- | --- |
| `--url URL` | `http://127.0.0.1:57804` | Daemon to attach to. |
| `--start` | off | Spawn `zou start` first if the daemon is not reachable. |
| `--theme NAME` | config or `paper` | One-run theme override. |
| `--no-mouse` | off | Disable mouse capture; useful in terminals or multiplexers that do not pass mouse events cleanly. |

`Ctrl-C` always quits the TUI. It does not stop a separately started daemon.

## Screen layout

The default layout is a task list on the left and a detail pane on the right.
On narrow terminals it stacks. The detail pane has four tabs:

- **Logs** — the full technical transcript: tool calls, stderr, and `[eval]`
  output. It streams live while a task runs.
- **Output** — the agent's final response, rendered as Markdown. This is kept
  separate from the transcript so an analysis task still has a readable answer.
- **Diff** — the unified diff captured when the task reached `REVIEW`.
- **Task** — id, status, retries, timestamps, and the task description.

The status bar shows the project, daemon connection, running count, review
count, and active theme. You can change which items appear.

## Keys

These are the built-in bindings. Every action can be rebound in the config
file; see [Configuration](#configuration).

### Navigation

| Key | Action |
| --- | --- |
| `j` / `↓` | Select the next task. |
| `k` / `↑` | Select the previous task. |
| `PageUp` / `PageDown` | Scroll the log pane. |
| `Tab` | Cycle the detail tab (Logs → Output → Diff → Task). |
| `Enter` | Open the selected task's Logs tab and resume following. |
| `Esc` | Clear the task filter. |
| `q` | Quit the TUI. |

### Task actions

| Key | Action |
| --- | --- |
| `p` | Plan a feature. Type the prompt and press `Enter`. |
| `r` | Run all ready (`TODO`) tasks. Asks for confirmation. |
| `R` | Run the selected task. Only `TODO` tasks can start. |
| `a` | Approve the selected `REVIEW` task. Asks for confirmation. |
| `x` | Reject the selected `REVIEW` or `FAILED_NEEDS_HUMAN` task with feedback. |
| `c` | Recover the selected stuck task, or all stuck tasks when nothing is selected. |
| `g` | Refresh the task list from the daemon. |

### Views and configuration

| Key | Action |
| --- | --- |
| `l` | Logs tab. |
| `o` | Output tab (styled Markdown). |
| `d` | Diff tab. |
| `i` | Task info tab. |
| `t` | Cycle the theme. |
| `m` | Open the model picker. |
| `f` | Edit the task filter. |
| `:` | Open the command palette. |
| `?` | Show the in-app key help. |

### Confirmation dialogs

`r`, `R`, `a`, and `c` open a confirmation prompt. Press `y` to proceed or
`n` / `Esc` to cancel. The prompt shows the task and the action so a stray
keypress cannot merge work.

## Command palette

Press `:` and type one of:

```text
:help
:plan <prompt>
:run
:run-one
:approve
:reject <feedback>
:recover
:refresh
:filter <text>
:logs
:output
:diff
:task
:theme [name]
:model
:quit
```

Commands without an argument act on the selected task where applicable. The
palette is a shortcut, not a second API: every command maps to the same action
as its key binding.

## Model picker

Press `m` to list the models reported by `opencode models`. Use `j` / `k` or the
arrow keys to move, `Enter` to save, and `Esc` to cancel.

The choice is written to `${XDG_CONFIG_HOME:-~/.config}/zou/config.json` and is
used by future `zou run` / `zou start` processes. A running daemon keeps the
model it started with; restart it after changing the model.

## Configuration

The TUI reads, in order:

1. `$XDG_CONFIG_HOME/zou/tui.toml` (or `~/.config/zou/tui.toml`)
2. `<project>/.zou/tui.toml`, which overrides the user-wide file

The file is re-read while the TUI runs, so theme, keymap, layout, and status-bar
changes apply without restarting. If the file is invalid, the TUI keeps the
defaults and shows the parse error in the status bar.

```toml
# ~/.config/zou/tui.toml or <project>/.zou/tui.toml
theme = "midnight"        # paper | midnight | solarized | high-contrast | mono
mouse = true
follow_logs = true
layout = "auto"           # auto | split | stack

# Left-to-right status-bar items. Unknown names are ignored.
statusbar = ["project", "connection", "running", "review", "theme"]

# Rebind actions. Key specs are case-sensitive for single characters.
[keys]
quit = "q"
help = "?"
plan = "p"
run = "r"
run_one = "R"
approve = "a"
reject = "x"
recover = "c"
refresh = "g"
logs = "l"
output = "o"
diff = "d"
task = "i"
theme = "t"
model = "m"
filter = "f"
command = ":"
up = "k"
down = "j"
page_up = "pageup"
page_down = "pagedown"
enter = "enter"
escape = "esc"
tab = "tab"

# Override individual theme colors. Use six-digit hex, with or without '#'.
[colors]
bg = "#0f121c"
fg = "#e2e8f0"
muted = "#7e8b9e"
accent = "#60a5fa"
signal = "#facc15"
success = "#4ade80"
danger = "#f87171"
border = "#475569"
highlight_bg = "#1e40af"
highlight_fg = "#eff6ff"
```

### Key syntax

A key spec is a single character, a named key, or a modifier plus a key:

```toml
[keys]
run = "ctrl+r"
approve = "alt+a"
help = "f1"
```

Supported modifiers are `ctrl` / `control`, `alt` / `meta`, and `shift` (for
single characters, prefer the uppercase character). Named keys include `enter`,
`esc`, `tab`, `backtab`, `backspace`, `delete`, `home`, `end`, `pageup`,
`pagedown`, `insert`, `up`, `down`, `left`, `right`, and `f1`–`f24`.

Unknown action names and invalid key specs are reported in the status bar and
ignored; the rest of the keymap still loads.

### Layout

- `auto` — split when the terminal is at least 100 columns wide, stack
  otherwise.
- `split` — always side by side.
- `stack` — always list above detail.

### Status bar

Supported items: `project`, `connection`, `running`, `review`, `theme`,
`filter` (only when a filter is active), and `counts`. `? help` is always
appended.

### Themes

| Theme | Character |
| --- | --- |
| `paper` | Warm off-white, ink, red accent. |
| `midnight` | Dark blue-grey, blue accent. |
| `solarized` | Solarized Dark. |
| `high-contrast` | Black/white/yellow for accessibility. |
| `mono` | Terminal-default colors, no color dependency. |

`t` cycles themes for the current session. `--theme` overrides the config for
one run. `[colors]` overrides apply on top of whichever theme is active.

## Typical review loop

1. `p` — describe the feature.
2. `r` — run the ready tasks.
3. Select a task and watch **Logs** while it works.
4. When the status becomes `REVIEW`, open **Output** and **Diff**.
5. `a` to approve, or `x` to reject with feedback.
6. Repeat until the DAG is `DONE`.

Approval merges the captured diff into your working tree; it does not create a
git commit. The repository's history remains yours to write.

## Troubleshooting

**`daemon did not become ready`** — start it manually with `zou start`, then
attach with `zou tui`. Check that the port is free and that `--url` matches the
daemon's `--host` / `--port`.

**Keys do nothing in tmux/screen** — try `zou tui --no-mouse`; some multiplexers
intercept mouse reporting. Also check that your terminal sends `PageUp` /
`PageDown` rather than mapping them to scrollback.

**Config changes are ignored** — the TUI prefers `<project>/.zou/tui.toml` over
the user-wide file. Check the status bar for a parse error, and confirm the
project file is the one you edited.

**Windows** — use Windows Terminal or another modern terminal with ANSI and
UTF-8 support. `zou tui --no-mouse` is a useful fallback if mouse capture is
unreliable. The TUI does not require WSL when `zou.exe` is installed natively.
