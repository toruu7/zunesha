#!/bin/sh
# End-to-end verification of the orchestrator against real git + a live daemon.
#
# Covers what unit tests can't: the CLI surface, the human-gated DAG walk,
# the Phase 8 self-healing validation loop, Phase 9 auto-merge conflict
# resolution, crash recovery, and live WebSocket streaming.
#
# Usage: sh scripts/e2e.sh          (from anywhere; needs a prior `cargo build`)
set -e
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
ZOU="${ZOU:-$ROOT/target/debug/zou}"
HERE="$ROOT/scripts"
AG=$(mktemp -d)                       # generated fake agents (volatile OK)
PORT=7435
DAEMON_PID=
DPID2=
say()  { printf '\n=== %s ===\n' "$1"; }
pass() { printf '  PASS %s\n' "$1"; }
fail() { printf '  FAIL %s\n' "$1"; exit 1; }
assert_contains() { case "$2" in *"$3"*) pass "$1";; *) printf '   haystack: %s\n' "$2"; fail "$1";; esac; }
cleanup() {
  [ -n "$DAEMON_PID" ] && kill "$DAEMON_PID" 2>/dev/null || true
  [ -n "$DPID2" ] && kill "$DPID2" 2>/dev/null || true
}
trap cleanup EXIT

# A fresh one-commit git repo with the orchestrator initialized.
new_repo() {
  D=$(mktemp -d); cd "$D"
  git init -q .
  git config user.name T
  git config user.email t@t
  printf 'v1\n' > a.txt
  git add -A
  git commit -qm base
  "$ZOU" init > /dev/null
}
# First id in the given status, trimmed of the counts block.
id_in() {
  "$ZOU" status | awk -v s="$1" '$1 == s && NF >= 3 && !seen { print $2; seen = 1 }'
}
count_in() {
  "$ZOU" status | awk -v s="$1" '$1 == s && NF >= 3' | wc -l
}

say "init refuses a non-git directory without --force"
NG=$(mktemp -d)
if OUT=$(cd "$NG" && "$ZOU" init 2>&1); then
  fail "init should refuse outside a git repo (got: $OUT)"
fi
assert_contains "explains why + suggests --force" "$OUT" "--force"

say "init --force works there, and is idempotent"
(cd "$NG" && "$ZOU" init --force > /dev/null)
OUT=$(cd "$NG" && "$ZOU" init --force 2>&1)
case "$OUT" in *initialized*) pass "second init stays calm";; *) fail "idempotent init output: $OUT";; esac
[ -f "$NG/.zou/state.db" ] && pass "state.db created" || fail "no state.db"
assert_contains ".gitignore updated" "$(cat "$NG/.gitignore")" ".zou/"

say "zou work + zou diff round-trip"
new_repo
"$ZOU" plan "touch a file" > /dev/null
T=$(id_in TODO)
"$ZOU" work "$T" --activate | sed 's/^/  /'
[ -d ".zou/ws/$T" ] && pass "workspace created" || fail "no workspace"
OUT=$("$ZOU" diff "$T")
case "$OUT" in *"no changes"*) pass "diff empty before agent work";; *) fail "expected empty diff: $OUT";; esac
printf 'v2\n' > ".zou/ws/$T/a.txt"
assert_contains "diff shows agent change" "$("$ZOU" diff "$T")" "+v2"
OUT=$("$ZOU" diff does-not-exist 2>&1 || true)
case "$OUT" in *"no workspace"*) pass "diff of unknown task is friendly";; *) fail "diff unknown: $OUT";; esac

say "zou run ignores a stuck IN_PROGRESS task held by someone else"
# Regression: the wave-drain used to wait on the *global* IN_PROGRESS count,
# so a phantom claim (dead daemon, `work --activate` demo) hung `zou run`.
new_repo
"$ZOU" plan "claimed elsewhere" > /dev/null
PH=$(id_in TODO)
"$ZOU" work "$PH" --activate > /dev/null   # phantom IN_PROGRESS: no worker owns it
"$ZOU" plan "my real work" > /dev/null      # a fresh ready head beside it
if ! timeout 30 env ZOU_AGENT_CMD="true {workspace}" "$ZOU" run > /dev/null 2>&1; then
  fail "zou run hung (or failed) with a phantom IN_PROGRESS task present"
fi
pass "run completes despite an unrelated stuck IN_PROGRESS task"
"$ZOU" recover > /dev/null
[ "$(count_in IN_PROGRESS)" = "0" ] \
  && pass "recover released the phantom back to TODO" || fail "phantom still IN_PROGRESS"
[ "$(count_in TODO)" -ge 1 ] && pass "task is runnable again" || fail "recover did not free it"

say "the human-gated chain: run --follow + approve, step by step"
new_repo
"$ZOU" plan "chain through every task" > /dev/null
# Agent: emits 3 lines; writes content unique to its workspace (the task id);
# also reports what Phase 10 put in its prompt (map section + a tree entry).
cat > "$AG/agent-chain.sh" <<'EOF'
#!/bin/sh
echo "ws-live-1"; echo "ws-live-2"; echo "ws-live-3"
printf 'v2\nfrom-task\n' > "$1/a.txt"
for a in "$@"; do P="$a"; done
echo "prompt-has-map=$(echo "$P" | grep -c 'Codebase map')"
echo "prompt-has-tree=$(echo "$P" | grep -c 'a.txt (1 line)')"
EOF
export ZOU_AGENT_CMD="sh $AG/agent-chain.sh {workspace}"

# --follow must terminate immediately: the rest of the chain waits for a human.
OUT=$("$ZOU" run --follow 2>&1)
case "$OUT" in
  *"awaiting approval"*) pass "--follow runs the ready wave and stops at the human gate";;
  *) fail "--follow output: $OUT";;
esac
[ "$(count_in REVIEW)" = "1" ] && pass "head task in REVIEW" || fail "expected exactly 1 REVIEW"
[ "$(count_in BACKLOG)" = "2" ] && pass "dependents still gated in BACKLOG" || fail "dependents not gated"
RID0=$(id_in REVIEW)
[ -f ".zou/ws/$RID0/CODEBASE_MAP.md" ] \
  && pass "workspace carries CODEBASE_MAP.md" || fail "codebase map missing from workspace"

# Walk the whole chain the way a user does: run, approve, repeat.
for _hop in 1 2 3; do
  "$ZOU" run > /dev/null
  RID=$(id_in REVIEW)
  [ -n "$RID" ] || fail "hop $_hop: nothing in REVIEW to approve"
  "$ZOU" approve "$RID" > /dev/null
done
[ "$(count_in DONE)" = "3" ] && pass "all 3 chained tasks reached DONE" || fail "not all 3 DONE"
assert_contains "final content merged through the chain" "$(cat a.txt)" "from-task"
grep -aq "prompt-has-map=1" .zou/state.db \
  && pass "agent prompts carried the codebase map" || fail "map missing from agent prompts"
grep -aq "prompt-has-tree=1" .zou/state.db \
  && pass "the map showed the workspace's files" || fail "map tree missing from prompts"
[ ! -f CODEBASE_MAP.md ] \
  && pass "the map never reaches the user's tree" || fail "CODEBASE_MAP.md leaked into the repo"
[ ! -d ".zou/ws" ] || [ -z "$(ls -A .zou/ws)" ] \
  && pass "all workspaces reaped after approvals" || fail "workspaces left behind"
unset ZOU_AGENT_CMD

say "self-healing: eval failures retry, park for a human, then resume"
new_repo
"$ZOU" plan "chain through every task" > /dev/null
# State lives OUTSIDE the workspace so marker files never enter the diff.
EVAL_STATE=$(mktemp -d)
export ZOU_EVAL_STATE="$EVAL_STATE"
# The agent NEVER fixes the code itself: validation fails 3× and the task
# parks. The template uses {workspace}, so the prompt is the LAST argument —
# attempts 2 and 3 must carry the failure text in it.
cat > "$AG/agent-heal.sh" <<'EOF'
#!/bin/sh
printf 'v2\nfrom-task\n' > "$1/a.txt"
echo "attempt-done"
for a in "$@"; do P="$a"; done
echo "prompt-mentions-assertion=$(echo "$P" | grep -c 'ASSERTION: FIXED missing')"
EOF
export ZOU_AGENT_CMD="sh $AG/agent-heal.sh {workspace}"
export ZOU_EVAL_CMD="sh -c 'test -f $EVAL_STATE/FIXED || { echo ASSERTION: FIXED missing; exit 1; }'"

"$ZOU" run > /dev/null
[ "$(count_in FAILED_NEEDS_HUMAN)" = "1" ] \
  && pass "3 failed validations park the head task" || fail "expected exactly 1 FAILED_NEEDS_HUMAN"
[ "$(count_in REVIEW)" = "0" ] && pass "nothing reaches REVIEW unvalidated" || fail "unvalidated task got to REVIEW"
grep -aq "prompt-mentions-assertion=1" .zou/state.db \
  && pass "the retry attempt got the errors in its prompt" || fail "retry prompt lacked the errors"
grep -aq "ASSERTION: FIXED missing" .zou/state.db \
  && pass "failure output kept for the human" || fail "failure output missing"

# A human investigates, fixes the underlying problem (here: creating
# $EVAL_STATE/FIXED), and frees the parked task (dashboard button = this API).
touch "$EVAL_STATE/FIXED"
PORT2=7436
"$ZOU" start --port $PORT2 > /dev/null 2>&1 &
DPID2=$!
i=0
until curl -sf "http://127.0.0.1:$PORT2/health" > /dev/null 2>&1; do
  i=$((i+1)); [ "$i" -gt 60 ] && fail "second daemon did not start"; sleep 0.25
done
RID=$(id_in FAILED_NEEDS_HUMAN)
code=$(curl -s -o /dev/null -w '%{http_code}' -X POST -H 'content-type: application/json' \
  -d '{"feedback":"dependency installed"}' "http://127.0.0.1:$PORT2/api/tasks/$RID/reject")
kill "$DPID2" 2>/dev/null || true; DPID2=
[ "$code" = "200" ] && pass "reject frees the parked task" || fail "reject returned $code"
[ "$(count_in TODO)" = "1" ] && pass "back in the queue" || fail "task did not return to TODO"

# FIXED exists now, so validation passes and the human-gated chain completes.
unset ZOU_EVAL_CMD
for _hop in 1 2 3; do
  "$ZOU" run > /dev/null
  RID=$(id_in REVIEW)
  [ -n "$RID" ] || fail "heal hop $_hop: nothing in REVIEW"
  "$ZOU" approve "$RID" > /dev/null
done
[ "$(count_in DONE)" = "3" ] && pass "chain completes after the human nudge" || fail "chain did not finish"
assert_contains "healed content merged" "$(cat a.txt)" "from-task"
unset ZOU_AGENT_CMD ZOU_EVAL_STATE

say "merge conflict: parallel tasks with overlapping diffs auto-resolve"
# Both heads run before either approves, so their diffs compete for a.txt.
new_repo
"$ZOU" plan "left side" > /dev/null
"$ZOU" plan "right side" > /dev/null
H1=$(id_in TODO)
H2=$("$ZOU" status | awk '$1 == "TODO" && NF >= 3 && $2 != "'"${H1}"'" { print $2; exit }')
# The fake agent writes content unique to its workspace — as the run agent it
# produces the conflicting edits, and as the Phase 9 resolution agent it
# overwrites the conflicted file (markers included) with "its" merged line.
# It also reports whether its prompt carried the strict conflict instruction.
cat > "$AG/agent-conflict.sh" <<'EOF'
#!/bin/sh
printf 'v2-%s\n' "$(basename "$1")" > "$1/a.txt"
for a in "$@"; do P="$a"; done
echo "prompt-mentions-conflict=$(echo "$P" | grep -c 'conflict markers')"
EOF
export ZOU_AGENT_CMD="sh $AG/agent-conflict.sh {workspace}"
"$ZOU" run > /dev/null
[ "$(count_in REVIEW)" = "2" ] && pass "both heads reviewed in parallel" || fail "expected 2 REVIEW"

"$ZOU" approve "$H1" > /dev/null
assert_contains "first approve merged its version" "$(cat a.txt)" "v2-$H1"

# Phase 9: the second approval must NOT fail — it rebases the workspace onto
# the current tree, moves the task to RESOLVING_CONFLICTS, and drives the
# resolution agent inline (still inside `zou approve`).
if OUT=$("$ZOU" approve "$H2" 2>&1); then
  pass "second approve queues auto-resolution instead of failing"
else
  printf '   output: %s\n' "$OUT"; fail "second approve should auto-resolve"
fi
assert_contains "conflict is announced" "$OUT" "merge conflict"
assert_contains "agent received the strict conflict prompt" "$OUT" "prompt-mentions-conflict=1"
assert_contains "resolution lands back at the human gate" "$OUT" "back in REVIEW"
assert_contains "conflicted approve wrote nothing to the tree" "$(cat a.txt)" "v2-$H1"
[ "$(count_in RESOLVING_CONFLICTS)" = "0" ] \
  && pass "resolution finished before approve returned" || fail "task stuck RESOLVING_CONFLICTS"
[ "$(count_in REVIEW)" = "1" ] && pass "reconciled task awaits second approval" || fail "expected 1 REVIEW"

# The reconciled diff is computed against the new base, so it applies cleanly.
"$ZOU" approve "$H2" > /dev/null
[ "$(count_in DONE)" = "2" ] && pass "re-approve after auto-resolution completes" || fail "stuck after auto-resolution"
assert_contains "auto-resolved content landed" "$(cat a.txt)" "v2-$H2"
[ -d ".zou/ws/$H2" ] && fail "resolution workspace left behind" || pass "workspace reaped after final approve"
unset ZOU_AGENT_CMD

say "live daemon WebSocket check (fresh project)"
new_repo
"$ZOU" plan "stream my logs" > /dev/null
# Slow-ish agent so log lines stream while the run is live; the daemon reads
# ZOU_AGENT_CMD from its launch environment.
cat > "$AG/agent-slow.sh" <<'EOF'
#!/bin/sh
for w in 1 2 3; do echo "ws-live-$w"; sleep 0.2; done
EOF
ZOU_AGENT_CMD="sh $AG/agent-slow.sh {workspace}" "$ZOU" start --port "$PORT" > /dev/null 2>&1 &
DAEMON_PID=$!
i=0; code=000
while [ "$i" -lt 20 ]; do
  code=$(curl -s --max-time 3 -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PORT/health" || true)
  [ "$code" = "200" ] && break
  i=$((i + 1)); sleep 1
done
[ "$code" = "200" ] || fail "daemon not healthy"
PORT=$PORT node "$HERE/ws-check.mjs" || fail "ws-check failed"

printf '\nE2E SWEEP PASSED\n'
