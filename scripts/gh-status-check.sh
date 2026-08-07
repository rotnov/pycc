#!/usr/bin/env sh
set -eu

# Check (or, with --watch, poll) the public GitHub status page
# (https://www.githubstatus.com) for the operational status of one or more
# named components (default: "Actions") and any unresolved incidents naming
# them. This is corroborating evidence for issue-implement's "Attribute CI
# failures before reacting" step and for ci-watch.sh's all-CANCELLED infra
# hint -- it does not replace reading the actual failing job's own logs, it
# only tells you whether GitHub itself is reporting trouble at the same
# time. A component name matches by case-insensitive substring, so "Actions"
# also matches "Actions" any regional variants GitHub may report.
#
# Usage:
#   gh-status-check.sh [component-substring ...]
#   gh-status-check.sh --watch [component-substring ...]
#
# Without --watch: fetches once, prints one line per matched component that
# is not "operational" plus one line per unresolved incident naming a
# matched component, or a single "operational, no unresolved incidents"
# line when there is nothing to report. Exits 0 regardless of what it
# finds -- this script is informational only and never fails a build.
#
# With --watch: polls every $POLL_INTERVAL seconds (default 30). Emits a
# line immediately for whatever the first poll finds (mirroring
# ci-watch.sh's immediate first-poll report), then a line only when the
# matched state changes, and exits once every matched component is
# "operational" with no unresolved incidents naming it. A poll that fails
# to reach the status page reports once (not on every retry) and stays
# silent again once it starts succeeding, matching ci-watch.sh's own
# per-poll error surfacing. Intended for the `Monitor` tool exactly like
# ci-watch.sh, for when a confirmed active incident is worth waiting out
# rather than repeatedly re-running CI into it.

STATUS_URL=${GH_STATUS_URL:-https://www.githubstatus.com/api/v2/summary.json}

watch=0
case "${1:-}" in
  --watch) watch=1; shift ;;
esac

components="$*"
[ -n "$components" ] || components="Actions"
poll_interval=${POLL_INTERVAL:-30}

# Print one line per matched component whose status isn't "operational",
# and one line per unresolved incident naming a matched component. Empty
# output means every named component is operational with nothing
# unresolved against it.
report() {
  summary=$1
  for c in $components; do
    printf '%s' "$summary" | jq -r --arg c "$c" '
      .components[]
      | select(.name | ascii_downcase | contains($c | ascii_downcase))
      | select(.status != "operational")
      | "GitHub status -- \(.name): \(.status)"
    '
    printf '%s' "$summary" | jq -r --arg c "$c" '
      .incidents[]?
      | select(.status != "resolved" and .status != "postmortem")
      | select(any(.components[]?; .name | ascii_downcase | contains($c | ascii_downcase)))
      | "GitHub incident -- \(.name) [\(.impact)/\(.status)] since \(.started_at // .created_at)"
    '
  done
}

fetch_and_report() {
  summary=$(curl -fsS "$STATUS_URL") || return 1
  report "$summary"
  return 0
}

if [ "$watch" = "0" ]; then
  if ! lines=$(fetch_and_report); then
    echo "gh-status-check: failed to reach $STATUS_URL"
    exit 0
  fi
  if [ -z "$lines" ]; then
    echo "GitHub status: $components -- operational, no unresolved incidents"
  else
    printf '%s\n' "$lines"
  fi
  exit 0
fi

prev=""
first=1
unreachable_reported=0
while true; do
  if ! lines=$(fetch_and_report); then
    if [ "$unreachable_reported" = "0" ]; then
      echo "gh-status-check: failed to reach $STATUS_URL"
      unreachable_reported=1
    fi
    sleep "$poll_interval"
    continue
  fi
  unreachable_reported=0

  if [ -z "$lines" ]; then
    if [ "$first" = "1" ] || [ -n "$prev" ]; then
      echo "GitHub status: $components -- operational, no unresolved incidents (clear)"
    fi
    exit 0
  fi

  if [ "$first" = "1" ] || [ "$lines" != "$prev" ]; then
    printf '%s\n' "$lines"
  fi
  prev=$lines
  first=0
  sleep "$poll_interval"
done
