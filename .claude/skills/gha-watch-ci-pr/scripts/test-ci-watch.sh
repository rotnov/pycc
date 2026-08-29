#!/usr/bin/env sh
set -eu

repo_root=$(CDPATH='' cd -- "$(dirname -- "$0")/../../../.." && pwd)
work_dir=$(mktemp -d "${TMPDIR:-/tmp}/pycc-ci-watch-test.XXXXXX")

cleanup() {
  rm -rf "$work_dir"
}
trap cleanup EXIT HUP INT TERM

fail() {
  echo "FAIL: $1" >&2
  exit 1
}

# --- Fixture 1: a PR whose first poll already reports two failed checks --
# (mixed genuine FAILURE + CANCELLED, not just the first check) ------------
mkdir -p "$work_dir/fixture-fail/bin"
cat >"$work_dir/fixture-fail/bin/gh" <<'EOF'
#!/usr/bin/env sh
cat <<'JSON'
{"state":"OPEN","mergeStateStatus":"BLOCKED","mergeable":"MERGEABLE","statusCheckRollup":[{"name":"agent-assets","status":"COMPLETED","conclusion":"FAILURE"},{"name":"ci-gate","status":"COMPLETED","conclusion":"CANCELLED"},{"name":"audit","status":"COMPLETED","conclusion":"SUCCESS"}]}
JSON
EOF
chmod +x "$work_dir/fixture-fail/bin/gh"

output=$(PATH="$work_dir/fixture-fail/bin:$PATH" POLL_INTERVAL=1 "$repo_root/.claude/skills/gha-watch-ci-pr/scripts/ci-watch.sh" owner/repo 42)
case "$output" in
  *"PR #42: CHECK FAILED -- agent-assets (FAILURE), ci-gate (CANCELLED)"*) ;;
  *) fail "expected failed-check line naming both checks, got: $output" ;;
esac
case "$output" in
  *"all CANCELLED"*) fail "mixed FAILURE+CANCELLED must not get the all-CANCELLED infra hint, got: $output" ;;
  *) ;;
esac

# --- Fixture 2: a PR that is CONFLICTING on the first poll ----------------
mkdir -p "$work_dir/fixture-conflict/bin"
cat >"$work_dir/fixture-conflict/bin/gh" <<'EOF'
#!/usr/bin/env sh
cat <<'JSON'
{"state":"OPEN","mergeStateStatus":"DIRTY","mergeable":"CONFLICTING","statusCheckRollup":[]}
JSON
EOF
chmod +x "$work_dir/fixture-conflict/bin/gh"

output=$(PATH="$work_dir/fixture-conflict/bin:$PATH" POLL_INTERVAL=1 "$repo_root/.claude/skills/gha-watch-ci-pr/scripts/ci-watch.sh" owner/repo 43)
case "$output" in
  *"PR #43: CONFLICTS"*) ;;
  *) fail "expected conflicts line, got: $output" ;;
esac

# --- Fixture 3: a PR that is BEHIND on the first poll ----------------------
mkdir -p "$work_dir/fixture-stale/bin"
cat >"$work_dir/fixture-stale/bin/gh" <<'EOF'
#!/usr/bin/env sh
cat <<'JSON'
{"state":"OPEN","mergeStateStatus":"BEHIND","mergeable":"MERGEABLE","statusCheckRollup":[]}
JSON
EOF
chmod +x "$work_dir/fixture-stale/bin/gh"

output=$(PATH="$work_dir/fixture-stale/bin:$PATH" POLL_INTERVAL=1 "$repo_root/.claude/skills/gha-watch-ci-pr/scripts/ci-watch.sh" owner/repo 44)
case "$output" in
  *"PR #44: STALE"*) ;;
  *) fail "expected stale line, got: $output" ;;
esac

# --- Fixture 4: a PR that is pending on poll 1, then green+CLEAN from
# poll 2 on -- READY needs the same verdict on two consecutive polls, so it
# resolves on poll 3
mkdir -p "$work_dir/fixture-ready/bin"
counter_file="$work_dir/fixture-ready/counter"
echo 0 >"$counter_file"
cat >"$work_dir/fixture-ready/bin/gh" <<EOF
#!/usr/bin/env sh
n=\$(cat "$counter_file")
n=\$((n + 1))
echo "\$n" >"$counter_file"
if [ "\$n" = "1" ]; then
  cat <<'JSON'
{"state":"OPEN","mergeStateStatus":"BLOCKED","mergeable":"MERGEABLE","statusCheckRollup":[{"name":"audit","status":"IN_PROGRESS","conclusion":null}]}
JSON
else
  cat <<'JSON'
{"state":"OPEN","mergeStateStatus":"CLEAN","mergeable":"MERGEABLE","statusCheckRollup":[{"name":"audit","status":"COMPLETED","conclusion":"SUCCESS"}]}
JSON
fi
EOF
chmod +x "$work_dir/fixture-ready/bin/gh"

output=$(PATH="$work_dir/fixture-ready/bin:$PATH" POLL_INTERVAL=1 "$repo_root/.claude/skills/gha-watch-ci-pr/scripts/ci-watch.sh" owner/repo 45)
case "$output" in
  *"PR #45: READY"*) ;;
  *) fail "expected ready line after two polls, got: $output" ;;
esac
poll_count=$(cat "$work_dir/fixture-ready/counter")
[ "$poll_count" = "3" ] || fail "expected exactly 3 polls before READY (green verdict confirmed on 2 consecutive polls), got $poll_count"

# --- Fixture 5: a merged PR is reported immediately and only once ---------
mkdir -p "$work_dir/fixture-merged/bin"
cat >"$work_dir/fixture-merged/bin/gh" <<'EOF'
#!/usr/bin/env sh
cat <<'JSON'
{"state":"MERGED","mergeStateStatus":"CLEAN","mergeable":"MERGEABLE","statusCheckRollup":[]}
JSON
EOF
chmod +x "$work_dir/fixture-merged/bin/gh"

output=$(PATH="$work_dir/fixture-merged/bin:$PATH" POLL_INTERVAL=1 "$repo_root/.claude/skills/gha-watch-ci-pr/scripts/ci-watch.sh" owner/repo 46)
[ "$output" = "PR #46: MERGED" ] || fail "expected exactly one MERGED line, got: $output"

# --- Fixture 6: two PRs tracked at once, each reported exactly once -------
mkdir -p "$work_dir/fixture-multi/bin"
cat >"$work_dir/fixture-multi/bin/gh" <<'EOF'
#!/usr/bin/env sh
# $1=pr $2=-R $3=repo $4=--json ... ; find the PR number from argv
for arg in "$@"; do
  case "$arg" in
    [0-9]*) pr="$arg" ;;
  esac
done
if [ "$pr" = "47" ]; then
  echo '{"state":"MERGED","mergeStateStatus":"CLEAN","mergeable":"MERGEABLE","statusCheckRollup":[]}'
else
  echo '{"state":"CLOSED","mergeStateStatus":"DIRTY","mergeable":"CONFLICTING","statusCheckRollup":[]}'
fi
EOF
chmod +x "$work_dir/fixture-multi/bin/gh"

output=$(PATH="$work_dir/fixture-multi/bin:$PATH" POLL_INTERVAL=1 "$repo_root/.claude/skills/gha-watch-ci-pr/scripts/ci-watch.sh" owner/repo 47 48)
line_count=$(printf '%s\n' "$output" | wc -l | tr -d ' ')
[ "$line_count" = "2" ] || fail "expected exactly 2 lines for 2 PRs, got $line_count: $output"
case "$output" in
  *"PR #47: MERGED"*) ;;
  *) fail "expected PR #47 MERGED line, got: $output" ;;
esac
case "$output" in
  *"PR #48: CLOSED"*) ;;
  *) fail "expected PR #48 CLOSED line, got: $output" ;;
esac

# --- Fixture 7: a PR whose only failing checks are all CANCELLED, with no
# genuine FAILURE/TIMED_OUT among them -- expects the infra-artifact hint --
mkdir -p "$work_dir/fixture-all-cancelled/bin"
cat >"$work_dir/fixture-all-cancelled/bin/gh" <<'EOF'
#!/usr/bin/env sh
cat <<'JSON'
{"state":"OPEN","mergeStateStatus":"BLOCKED","mergeable":"MERGEABLE","statusCheckRollup":[{"name":"agent-assets","status":"COMPLETED","conclusion":"CANCELLED"},{"name":"audit","status":"COMPLETED","conclusion":"CANCELLED"}]}
JSON
EOF
chmod +x "$work_dir/fixture-all-cancelled/bin/gh"

output=$(PATH="$work_dir/fixture-all-cancelled/bin:$PATH" POLL_INTERVAL=1 "$repo_root/.claude/skills/gha-watch-ci-pr/scripts/ci-watch.sh" owner/repo 49)
case "$output" in
  *"PR #49: CHECK FAILED -- agent-assets (CANCELLED), audit (CANCELLED) -- all CANCELLED"*) ;;
  *) fail "expected all-CANCELLED infra hint, got: $output" ;;
esac

# --- Fixture 8: a PR whose checks are all COMPLETED/SUCCESS but is BLOCKED
# by something other than checks (e.g. an unresolved required review
# thread) -- must be reported as terminal, not polled forever ------------
mkdir -p "$work_dir/fixture-blocked-clean-checks/bin"
cat >"$work_dir/fixture-blocked-clean-checks/bin/gh" <<'EOF'
#!/usr/bin/env sh
cat <<'JSON'
{"state":"OPEN","mergeStateStatus":"BLOCKED","mergeable":"MERGEABLE","statusCheckRollup":[{"name":"ci-gate","status":"COMPLETED","conclusion":"SUCCESS"},{"name":"audit","status":"COMPLETED","conclusion":"SUCCESS"}]}
JSON
EOF
chmod +x "$work_dir/fixture-blocked-clean-checks/bin/gh"

output=$(PATH="$work_dir/fixture-blocked-clean-checks/bin:$PATH" POLL_INTERVAL=1 "$repo_root/.claude/skills/gha-watch-ci-pr/scripts/ci-watch.sh" owner/repo 50)
case "$output" in
  *"PR #50: BLOCKED -- all checks completed with no failures, but mergeStateStatus=BLOCKED"*) ;;
  *) fail "expected a terminal BLOCKED line for all-green-but-not-CLEAN checks, got: $output" ;;
esac

# --- Fixture 9: an empty statusCheckRollup (Actions not started yet) must
# never be terminal -- regression for the 2026-08-29 false-BLOCKED incident
# (the pre-fix script resolved BLOCKED on the very first empty poll). The
# watch must ride through empty and pending polls to READY. --------------
mkdir -p "$work_dir/fixture-empty-rollup/bin"
counter_file_9="$work_dir/fixture-empty-rollup/counter"
echo 0 >"$counter_file_9"
cat >"$work_dir/fixture-empty-rollup/bin/gh" <<EOF
#!/usr/bin/env sh
n=\$(cat "$counter_file_9")
n=\$((n + 1))
echo "\$n" >"$counter_file_9"
if [ "\$n" -le 2 ]; then
  cat <<'JSON'
{"state":"OPEN","mergeStateStatus":"BLOCKED","mergeable":"MERGEABLE","statusCheckRollup":[]}
JSON
elif [ "\$n" = "3" ]; then
  cat <<'JSON'
{"state":"OPEN","mergeStateStatus":"BLOCKED","mergeable":"MERGEABLE","statusCheckRollup":[{"name":"audit","status":"IN_PROGRESS","conclusion":null}]}
JSON
else
  cat <<'JSON'
{"state":"OPEN","mergeStateStatus":"CLEAN","mergeable":"MERGEABLE","statusCheckRollup":[{"name":"audit","status":"COMPLETED","conclusion":"SUCCESS"}]}
JSON
fi
EOF
chmod +x "$work_dir/fixture-empty-rollup/bin/gh"

output=$(PATH="$work_dir/fixture-empty-rollup/bin:$PATH" POLL_INTERVAL=1 "$repo_root/.claude/skills/gha-watch-ci-pr/scripts/ci-watch.sh" owner/repo 51)
case "$output" in
  *"PR #51: READY"*) ;;
  *) fail "expected READY after empty-rollup polls, got: $output" ;;
esac
case "$output" in
  *"BLOCKED --"*) fail "empty statusCheckRollup must never produce a terminal BLOCKED, got: $output" ;;
  *) ;;
esac
poll_count=$(cat "$counter_file_9")
[ "$poll_count" = "5" ] || fail "expected exactly 5 polls (2 empty, 1 pending, 2 green) before READY, got $poll_count"

# --- Fixture 10: a momentary between-workflow gap (every enumerated check
# COMPLETED, but the next chained workflow's checks are not created yet and
# mergeStateStatus is still BLOCKED) must not emit a terminal BLOCKED -----
mkdir -p "$work_dir/fixture-workflow-gap/bin"
counter_file_10="$work_dir/fixture-workflow-gap/counter"
echo 0 >"$counter_file_10"
cat >"$work_dir/fixture-workflow-gap/bin/gh" <<EOF
#!/usr/bin/env sh
n=\$(cat "$counter_file_10")
n=\$((n + 1))
echo "\$n" >"$counter_file_10"
if [ "\$n" = "1" ]; then
  cat <<'JSON'
{"state":"OPEN","mergeStateStatus":"BLOCKED","mergeable":"MERGEABLE","statusCheckRollup":[{"name":"ci-gate","status":"COMPLETED","conclusion":"SUCCESS"}]}
JSON
elif [ "\$n" = "2" ]; then
  cat <<'JSON'
{"state":"OPEN","mergeStateStatus":"BLOCKED","mergeable":"MERGEABLE","statusCheckRollup":[{"name":"ci-gate","status":"COMPLETED","conclusion":"SUCCESS"},{"name":"audit","status":"IN_PROGRESS","conclusion":null}]}
JSON
else
  cat <<'JSON'
{"state":"OPEN","mergeStateStatus":"CLEAN","mergeable":"MERGEABLE","statusCheckRollup":[{"name":"ci-gate","status":"COMPLETED","conclusion":"SUCCESS"},{"name":"audit","status":"COMPLETED","conclusion":"SUCCESS"}]}
JSON
fi
EOF
chmod +x "$work_dir/fixture-workflow-gap/bin/gh"

output=$(PATH="$work_dir/fixture-workflow-gap/bin:$PATH" POLL_INTERVAL=1 "$repo_root/.claude/skills/gha-watch-ci-pr/scripts/ci-watch.sh" owner/repo 52)
case "$output" in
  *"PR #52: READY"*) ;;
  *) fail "expected READY after the between-workflow gap, got: $output" ;;
esac
case "$output" in
  *"BLOCKED --"*) fail "a single-poll all-complete gap must not produce a terminal BLOCKED, got: $output" ;;
  *) ;;
esac

# --- Fixture 11: a persistently empty statusCheckRollup surfaces exactly
# one non-terminal NOTE (after EMPTY_NOTE_POLLS consecutive empty polls),
# then the watch continues to a real terminal state ----------------------
mkdir -p "$work_dir/fixture-empty-note/bin"
counter_file_11="$work_dir/fixture-empty-note/counter"
echo 0 >"$counter_file_11"
cat >"$work_dir/fixture-empty-note/bin/gh" <<EOF
#!/usr/bin/env sh
n=\$(cat "$counter_file_11")
n=\$((n + 1))
echo "\$n" >"$counter_file_11"
if [ "\$n" -le 4 ]; then
  cat <<'JSON'
{"state":"OPEN","mergeStateStatus":"BLOCKED","mergeable":"MERGEABLE","statusCheckRollup":[]}
JSON
else
  cat <<'JSON'
{"state":"OPEN","mergeStateStatus":"CLEAN","mergeable":"MERGEABLE","statusCheckRollup":[{"name":"audit","status":"COMPLETED","conclusion":"SUCCESS"}]}
JSON
fi
EOF
chmod +x "$work_dir/fixture-empty-note/bin/gh"

output=$(PATH="$work_dir/fixture-empty-note/bin:$PATH" POLL_INTERVAL=1 EMPTY_NOTE_POLLS=3 "$repo_root/.claude/skills/gha-watch-ci-pr/scripts/ci-watch.sh" owner/repo 53)
case "$output" in
  *"PR #53: NOTE -- statusCheckRollup still empty after 3 polls"*) ;;
  *) fail "expected one non-terminal NOTE after 3 empty polls, got: $output" ;;
esac
note_count=$(printf '%s\n' "$output" | grep -c "NOTE --" || true)
[ "$note_count" = "1" ] || fail "expected exactly one NOTE line, got $note_count: $output"
case "$output" in
  *"PR #53: READY"*) ;;
  *) fail "expected the watch to continue past the NOTE to READY, got: $output" ;;
esac

# --- Fixture 12: the NOTE fires once per consecutive-empty streak -- a
# second streak after an intervening non-empty poll re-fires it ----------
mkdir -p "$work_dir/fixture-two-streaks/bin"
counter_file_12="$work_dir/fixture-two-streaks/counter"
echo 0 >"$counter_file_12"
cat >"$work_dir/fixture-two-streaks/bin/gh" <<EOF
#!/usr/bin/env sh
n=\$(cat "$counter_file_12")
n=\$((n + 1))
echo "\$n" >"$counter_file_12"
if [ "\$n" -le 2 ] || { [ "\$n" -ge 4 ] && [ "\$n" -le 5 ]; }; then
  cat <<'JSON'
{"state":"OPEN","mergeStateStatus":"BLOCKED","mergeable":"MERGEABLE","statusCheckRollup":[]}
JSON
elif [ "\$n" = "3" ]; then
  cat <<'JSON'
{"state":"OPEN","mergeStateStatus":"BLOCKED","mergeable":"MERGEABLE","statusCheckRollup":[{"name":"audit","status":"IN_PROGRESS","conclusion":null}]}
JSON
else
  cat <<'JSON'
{"state":"OPEN","mergeStateStatus":"CLEAN","mergeable":"MERGEABLE","statusCheckRollup":[{"name":"audit","status":"COMPLETED","conclusion":"SUCCESS"}]}
JSON
fi
EOF
chmod +x "$work_dir/fixture-two-streaks/bin/gh"

output=$(PATH="$work_dir/fixture-two-streaks/bin:$PATH" POLL_INTERVAL=1 EMPTY_NOTE_POLLS=2 "$repo_root/.claude/skills/gha-watch-ci-pr/scripts/ci-watch.sh" owner/repo 54)
note_count=$(printf '%s\n' "$output" | grep -c "NOTE --" || true)
[ "$note_count" = "2" ] || fail "expected one NOTE per consecutive-empty streak (2 streaks -> 2 NOTEs), got $note_count: $output"
case "$output" in
  *"PR #54: READY"*) ;;
  *) fail "expected the watch to continue past both streaks to READY, got: $output" ;;
esac

echo "ci-watch.sh: valid"
