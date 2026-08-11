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

# --- Fixture 4: a PR that is pending on poll 1, then green+CLEAN on poll 2
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
[ "$poll_count" = "2" ] || fail "expected exactly 2 polls before READY, got $poll_count"

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

echo "ci-watch.sh: valid"
