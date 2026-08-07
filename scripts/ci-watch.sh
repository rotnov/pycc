#!/usr/bin/env sh
set -eu

# Poll one or more GitHub pull requests every $POLL_INTERVAL seconds
# (default 10) and emit exactly one line per PR the moment it reaches a
# terminal state -- merged/closed, merge conflicts, a stale (behind-base)
# branch, one or more failed/timed-out/startup-failed/cancelled checks
# (every such check is named, not just the first), or fully green + CLEAN
# (ready to merge).
# Silent between polls: no per-poll spam, one line per PR's actual outcome.
# Exits once every PR passed on the command line has reached a terminal
# state.
#
# When every reported failing check is CANCELLED (no genuine FAILURE,
# TIMED_OUT, or STARTUP_FAILURE among them), the line adds a hint that this
# is often a
# partial-rerun or GitHub Actions infra artifact rather than a code defect
# -- see `issue-implement`'s "Attribute CI failures before reacting" step
# for how to act on it (a full, non-`--failed` rerun, not a diff
# investigation).
#
# Intended for `Monitor`-style background polling (see
# .claude/skills/autopilot-async-monitoring/SKILL.md) instead of a fixed
# wakeup interval, so a real terminal event (conflict, stale branch,
# failed check, ready-to-merge) is reported the moment it happens rather
# than up to a full wakeup interval later.
#
# Usage: ci-watch.sh <repo> <pr-number> [<pr-number> ...]
# Example: ci-watch.sh rotnov/pycc 324 325

if [ "$#" -lt 2 ]; then
  echo "usage: ci-watch.sh <repo> <pr-number> [<pr-number> ...]" >&2
  exit 2
fi

repo=$1
shift
prs="$*"
poll_interval=${POLL_INTERVAL:-10}

resolved_file=$(mktemp "${TMPDIR:-/tmp}/pycc-ci-watch.XXXXXX")
trap 'rm -f "$resolved_file"' EXIT HUP INT TERM

is_resolved() {
  grep -qx "$1" "$resolved_file" 2>/dev/null
}

mark_resolved() {
  echo "$1" >>"$resolved_file"
}

all_resolved() {
  for pr in $prs; do
    is_resolved "$pr" || return 1
  done
  return 0
}

poll_once() {
  for pr in $prs; do
    is_resolved "$pr" && continue

    data=$(gh pr view "$pr" -R "$repo" --json state,mergeStateStatus,mergeable,statusCheckRollup 2>&1) || {
      echo "PR #$pr: gh pr view failed: $data"
      continue
    }

    state=$(echo "$data" | jq -r '.state')
    if [ "$state" != "OPEN" ]; then
      echo "PR #$pr: $state"
      mark_resolved "$pr"
      continue
    fi

    mergeable=$(echo "$data" | jq -r '.mergeable')
    if [ "$mergeable" = "CONFLICTING" ]; then
      echo "PR #$pr: CONFLICTS -- merge base has diverged, needs a rebase/resolve"
      mark_resolved "$pr"
      continue
    fi

    merge_state=$(echo "$data" | jq -r '.mergeStateStatus')
    if [ "$merge_state" = "BEHIND" ]; then
      echo "PR #$pr: STALE -- branch is behind base, needs an update"
      mark_resolved "$pr"
      continue
    fi

    failed_checks=$(echo "$data" | jq -c '[.statusCheckRollup[]? | select(.conclusion=="FAILURE" or .conclusion=="TIMED_OUT" or .conclusion=="CANCELLED" or .conclusion=="STARTUP_FAILURE")]')
    failed_count=$(echo "$failed_checks" | jq 'length')
    if [ "$failed_count" != "0" ]; then
      failed_list=$(echo "$failed_checks" | jq -r 'map("\(.name) (\(.conclusion))") | join(", ")')
      non_cancelled=$(echo "$failed_checks" | jq '[.[] | select(.conclusion!="CANCELLED")] | length')
      if [ "$non_cancelled" = "0" ]; then
        echo "PR #$pr: CHECK FAILED -- $failed_list -- all CANCELLED, no genuine FAILURE/TIMED_OUT among them; often a partial-rerun or GitHub Actions infra artifact, not a code defect -- consider a full (non --failed) rerun of the affected workflow run(s) before investigating the diff"
      else
        echo "PR #$pr: CHECK FAILED -- $failed_list"
      fi
      mark_resolved "$pr"
      continue
    fi

    pending=$(echo "$data" | jq -r '[.statusCheckRollup[]? | select(.status!="COMPLETED")] | length')
    if [ "$pending" = "0" ] && [ "$merge_state" = "CLEAN" ]; then
      echo "PR #$pr: READY -- all checks green, CLEAN, mergeable"
      mark_resolved "$pr"
      continue
    fi
  done
}

while true; do
  poll_once
  all_resolved && break
  sleep "$poll_interval"
done
