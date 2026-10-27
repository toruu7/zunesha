# Troubleshooting

## `zou start` says the port is in use

Something else owns `127.0.0.1:57804` (another daemon, or an unrelated
service). Pick another port:

```sh
zou start --port 8080
ZOU_PORT=8080 zou start      # same, via the environment
```

The dashboard follows the daemon automatically — it is served from the same
origin. (If you are running the UI under `next dev`, point it at the new port
with `NEXT_PUBLIC_API_BASE=http://127.0.0.1:8080`.)

## The dashboard is a 404 / blank page

The daemon serves the **built** dashboard from `web/out`; it does not run
`next dev` for you. Either use a release/install that shipped the dashboard,
or build it:

```sh
cd web && npm install && npm run build && cd ..
```

If it lives somewhere unusual, pass `--dashboard <dir>` or set
`ZOU_DASHBOARD_DIR` before `zou start`.

## A task is stuck in `IN_PROGRESS` / `EVALUATING` / `RESOLVING_CONFLICTS`

That is what a crash (or Ctrl-C, or a reboot) leaves behind: the claim is
persisted, but the worker that owned it is gone.

- **Restart the daemon.** `zou start` releases claims left by a previous
  process and re-runs those tasks in their kept workspaces, announcing
  `resumed N interrupted task(s)`. This is the normal path.
- **Release one task manually** — `zou recover <TASK_ID>` (or `zou recover`
  for all), also available in the dashboard as **Release stuck task**.
- **If a live worker may own the claim** (for example a concurrent `zou run`),
  do not let the boot release it: `zou start --no-resume`. Nothing in the
  state can distinguish a live agent from a dead one, so this is a judgement
  call.

The workspace is kept across recovery, so the retry continues from whatever
the agent already produced, and the persisted retry counter still counts
towards the 3-failure cap.

## Tasks return to `TODO` and the log says the model endpoint is unavailable

The agent process exited non-zero before editing anything, so the task goes
back to `TODO` with no diff. This is usually a bad or temporarily unavailable
model, not a Zunesha error. Pick a different one:

```sh
zou model              # list `opencode models` and select one
zou model --show       # confirm the saved id
zou run                # retry the TODO tasks
```

If `ZOU_AGENT_CMD` is exported, it overrides the saved model — unset it (or
update it with the same `--model`) before retrying. Restart a running
`zou start` daemon after changing the setting. You can also verify a model by
hand in a scratch directory:

```sh
opencode run --auto --model opencode/deepseek-v4.1-flash-free "say OK"
```

## The agent keeps failing and the task is in `FAILED_NEEDS_HUMAN`

Three failed validations park a task on purpose. Open it in the dashboard:
the log shows the validation output, and **Send back to queue** gives it a
fresh retry budget (optionally with feedback that is added to the prompt).

Common causes:

- `ZOU_EVAL_CMD` doesn't match how this repo runs tests (try it by hand in
  the workspace shown in the log panel).
- The agent cannot see a dependency it needs — the task description is thin;
  reject with feedback, or improve the planner prompt.
- The validation command needs the network/credentials the daemon doesn't
  have.

## Approving says the diff does not apply (`MergeConflict`)

Your working tree changed after the task's workspace was snapshotted: the
approved diff no longer applies to the current files. Zunesha first tries an
**automatic resolution** (`RESOLVING_CONFLICTS`) when the conflict is between
two approved tasks. If the diff still cannot be applied:

1. `zou diff <TASK_ID>` to see exactly what the task wants to change.
2. Reconcile the overlapping files by hand (or apply the diff yourself).
3. Approve again — the diff is recomputed against your current tree — or
   reject with feedback to have the agent redo it.

Nothing is written when an approval conflicts; the task stays in `REVIEW`.

## The planner fails

| Message | Meaning |
|---|---|
| `produced no JSON` | The command printed prose (wrong binary? wrong flags?). |
| `not valid JSON` | Truncated or malformed answer — often a too-small model or a low token limit. |
| `cycle (tasks involved: …)` | The model made tasks depend on each other circularly. Tighten the prompt. |
| `unknown task \`x\`` | A `depends_on` refers to a key that is not in the plan. |
| `duplicate task key` | Two tasks share a `key`. |
| `timed out after 120s` | Raise `ZOU_PLANNER_TIMEOUT_SECS` (or use a faster model). |
| `exited with exit status: N` | The CLI itself failed; its stderr is quoted in the message. |

Remember: the plan must be on **stdout**, one JSON object, and the command
must not require a prompt argument (`zou` writes the prompt to stdin). If your
CLI takes the prompt as an argument, wrap it:
`ZOU_PLANNER_CMD='sh scripts/plan-with-my-cli.sh'` where the script ends in
`exec mycli -p "$(cat)"`.

An invalid plan writes nothing — fix the prompt/model and plan again.

## `zou` refuses to run / `not initialized`

`zou init` must have been run in this git work tree. From a subdirectory,
`zou` finds the enclosing project automatically; use `--path <dir>` to target
another checkout.

## Validation is slow or hangs

`ZOU_EVAL_TIMEOUT_SECS` (default `600`) kills a hung test run and counts it as
a failure. Very large suites may legitimately need longer; raise it before
`zou start`.

## I want a fresh start

Delete `.zou/` in the target repo. That removes all Zunesha state and
workspaces; your repository is untouched (task workspaces are ignored by the
repo — `.zou/` is in `.gitignore`).

## Git says the workspace commits are by `zunesha`

Workspace baseline/`diff` commits use the synthetic identity
`zunesha <zunesha@localhost>` so they never depend on (or pollute) your global
git config. Approval applies the reviewed diff to **your working tree** — it
is not committed for you.
