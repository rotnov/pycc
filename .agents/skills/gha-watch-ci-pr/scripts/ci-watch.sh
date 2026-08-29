#!/usr/bin/env sh
set -eu

# Poll one or more GitHub pull requests every $POLL_INTERVAL seconds
# (default 10) and emit exactly one line per PR the moment it reaches a
# terminal state -- merged/closed, merge conflicts, a stale (behind-base)
# branch, one or more failed/timed-out/startup-failed/cancelled checks
# (every such check is named, not just the first), all checks completed
# with none failing but still blocked by something other than checks (e.g.
# an unresolved required review or conversation thread), or fully green +
# CLEAN (ready to merge).
# Silent between polls: no per-poll spam, one line per PR's actual outcome.
# Exits once every PR passed on the command line has reached a terminal
# state.
#
# False-terminal protection: an empty statusCheckRollup (GitHub Actions
# has not started yet, or a gap between two chained workflows) looks
# exactly like "all checks completed", so it is never treated as terminal;
# after $EMPTY_NOTE_POLLS consecutive empty polls (default 30) one
# non-terminal NOTE line surfaces the possibility that Actions never
# started, and polling continues. The NOTE fires once per
# consecutive-empty streak (the counter resets on any non-empty poll), so
# it can recur if the rollup empties again later. The READY and BLOCKED verdicts
# additionally require the same qualifying observation on two consecutive
# polls, so a momentary all-complete gap before the next workflow's checks
# are created cannot resolve the watch early. When the base branch's
# required status contexts are readable (they may not be, e.g. without
# push access or branch protection), the verdicts are further bound to
# them: no READY/BLOCKED while any required context is still absent from
# the rollup, which closes the gap even when a chained workflow takes
# longer than two polls to register. The binding is itself bounded: a
# required context that stays missing for $REQ_MISS_POLLS consecutive
# polls (default 30) is more likely one the rollup reports under a shape
# this script does not match (e.g. a legacy commit-status context) than a
# workflow still registering, so the script emits one non-terminal NOTE,
# disables the binding for the rest of the watch, and falls back to the
# two-consecutive-poll confirmation alone -- a silent, permanent verdict
# suppression is exactly the failure mode this watcher exists to avoid.
# A head change (new push) resets the
# confirmation state, since everything observed so far described the old
# head. CHECK FAILED, MERGED/CLOSED,
# CONFLICTS, and STALE remain immediate -- those states do not regress on
# their own.
#
# When every reported failing check is CANCELLED (no genuine FAILURE,
# TIMED_OUT, or STARTUP_FAILURE among them), the line adds a hint that this
# is often a
# partial-rerun or GitHub Actions infra artifact rather than a code defect
# -- see `issue-implement`'s "Attribute CI failures before reacting" step
# for how to act on it (a full, non-`--failed` rerun, not a diff
# investigation).
#
# Intended for `Monitor`-style background polling (see this skill's
# SKILL.md) instead of a fixed wakeup interval, so a real terminal event
# (conflict, stale branch, failed check, ready-to-merge) is reported the
# moment it happens rather than up to a full wakeup interval later.
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
empty_note_polls=${EMPTY_NOTE_POLLS:-30}
req_miss_polls=${REQ_MISS_POLLS:-30}

state_dir=$(mktemp -d "${TMPDIR:-/tmp}/pycc-ci-watch.XXXXXX")
trap 'rm -rf "$state_dir"' EXIT HUP INT TERM
resolved_file="$state_dir/resolved"

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

    data=$(gh pr view "$pr" -R "$repo" --json state,mergeStateStatus,mergeable,statusCheckRollup,baseRefName,headRefOid 2>&1) || {
      echo "PR #$pr: gh pr view failed: $data"
      continue
    }

    state=$(echo "$data" | jq -r '.state')
    if [ "$state" != "OPEN" ]; then
      echo "PR #$pr: $state"
      mark_resolved "$pr"
      continue
    fi

    head_oid=$(echo "$data" | jq -r '.headRefOid // ""')
    prev_head=$(cat "$state_dir/head_$pr" 2>/dev/null || echo "")
    if [ -n "$prev_head" ] && [ "$head_oid" != "$prev_head" ]; then
      # New push: every observation so far described the old head, so the
      # verdict confirmation, empty-streak, and required-context-missing
      # counters start over.
      rm -f "$state_dir/cand_$pr" "$state_dir/empty_$pr" "$state_dir/reqmiss_$pr"
    fi
    echo "$head_oid" >"$state_dir/head_$pr"

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

    total=$(echo "$data" | jq -r '[.statusCheckRollup[]?] | length')
    if [ "$total" = "0" ]; then
      # No checks reported at all: Actions has not started, or the rollup
      # is momentarily empty between chained workflows. Indistinguishable
      # from "all checks passed", so never a terminal verdict. Surface a
      # one-time non-terminal NOTE if it persists, then keep polling.
      empties=$(cat "$state_dir/empty_$pr" 2>/dev/null || echo 0)
      empties=$((empties + 1))
      echo "$empties" >"$state_dir/empty_$pr"
      if [ "$empties" = "$empty_note_polls" ]; then
        echo "PR #$pr: NOTE -- statusCheckRollup still empty after $empties polls; GitHub Actions may not have started for this head (not a terminal state, still watching)"
      fi
      rm -f "$state_dir/cand_$pr"
      continue
    fi
    rm -f "$state_dir/empty_$pr"

    pending=$(echo "$data" | jq -r '[.statusCheckRollup[]? | select(.status!="COMPLETED")] | length')
    if [ "$pending" != "0" ]; then
      rm -f "$state_dir/cand_$pr"
      continue
    fi

    # Bind the verdict to the base branch's required status contexts when
    # they are readable: during a between-workflow gap a required check can
    # be "expected" by GitHub but absent from the rollup entirely, and two
    # consecutive polls of that state look complete. Unreadable protection
    # (no push access, no branch protection) leaves the cache empty and
    # skips the binding.
    if [ ! -f "$state_dir/req_$pr" ]; then
      base=$(echo "$data" | jq -r '.baseRefName // ""')
      gh api "repos/$repo/branches/$base/protection/required_status_checks/contexts" 2>/dev/null |
        jq -r 'if type=="array" then .[] | strings else empty end' 2>/dev/null >"$state_dir/req_$pr" || : >"$state_dir/req_$pr"
    fi
    missing=0
    missing_ctx=""
    while IFS= read -r ctx; do
      [ -n "$ctx" ] || continue
      present=$(echo "$data" | jq -r --arg n "$ctx" '[.statusCheckRollup[]? | select(.name==$n and .status=="COMPLETED")] | length')
      if [ "$present" = "0" ]; then
        missing=1
        missing_ctx=$ctx
        break
      fi
    done <"$state_dir/req_$pr"
    if [ "$missing" = "1" ]; then
      # Bound the binding itself: a required context that never matches
      # (e.g. one the rollup reports as a legacy commit-status entry
      # rather than a check run) would otherwise suppress READY/BLOCKED
      # forever -- the silent-stall failure mode this watcher exists to
      # avoid. After $REQ_MISS_POLLS consecutive missing polls, note it
      # once, drop the binding for this watch, and rely on the
      # two-consecutive-poll confirmation alone.
      reqmiss=$(cat "$state_dir/reqmiss_$pr" 2>/dev/null || echo 0)
      reqmiss=$((reqmiss + 1))
      echo "$reqmiss" >"$state_dir/reqmiss_$pr"
      if [ "$reqmiss" = "$req_miss_polls" ]; then
        echo "PR #$pr: NOTE -- required context '$missing_ctx' still not completed in the rollup after $reqmiss polls; disabling the required-context binding for this watch and relying on the two-consecutive-poll confirmation (not a terminal state, still watching)"
        : >"$state_dir/req_$pr"
      fi
      rm -f "$state_dir/cand_$pr"
      continue
    fi
    rm -f "$state_dir/reqmiss_$pr"

    if [ "$merge_state" = "CLEAN" ]; then
      verdict=READY
    else
      verdict=BLOCKED
    fi
    prev=$(cat "$state_dir/cand_$pr" 2>/dev/null || echo "")
    if [ "$prev" != "$verdict" ]; then
      # First qualifying observation: a completed-but-momentary gap before
      # the next workflow's checks are created looks identical, so require
      # the same verdict on two consecutive polls before reporting it.
      echo "$verdict" >"$state_dir/cand_$pr"
      continue
    fi
    if [ "$verdict" = "READY" ]; then
      echo "PR #$pr: READY -- all checks green, CLEAN, mergeable"
    else
      echo "PR #$pr: BLOCKED -- all checks completed with no failures, but mergeStateStatus=$merge_state (not CLEAN) -- often an unresolved required review or conversation thread; check the PR directly for the reason"
    fi
    mark_resolved "$pr"
  done
}

while true; do
  poll_once
  all_resolved && break
  sleep "$poll_interval"
done
