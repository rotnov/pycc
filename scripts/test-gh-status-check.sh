#!/usr/bin/env sh
set -eu

repo_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
work_dir=$(mktemp -d "${TMPDIR:-/tmp}/pycc-gh-status-check-test.XXXXXX")

cleanup() {
  rm -rf "$work_dir"
}
trap cleanup EXIT HUP INT TERM

fail() {
  echo "FAIL: $1" >&2
  exit 1
}

# --- Fixture 1: everything operational, no incidents -----------------------
mkdir -p "$work_dir/fixture-clear/bin"
cat >"$work_dir/fixture-clear/bin/curl" <<'EOF'
#!/usr/bin/env sh
cat <<'JSON'
{"components":[{"name":"Actions","status":"operational"},{"name":"API Requests","status":"operational"}],"incidents":[]}
JSON
EOF
chmod +x "$work_dir/fixture-clear/bin/curl"

output=$(PATH="$work_dir/fixture-clear/bin:$PATH" "$repo_root/scripts/gh-status-check.sh")
[ "$output" = "GitHub status: Actions -- operational, no unresolved incidents" ] \
  || fail "expected clear one-liner, got: $output"

# --- Fixture 2: matched component degraded, no incidents -------------------
mkdir -p "$work_dir/fixture-degraded/bin"
cat >"$work_dir/fixture-degraded/bin/curl" <<'EOF'
#!/usr/bin/env sh
cat <<'JSON'
{"components":[{"name":"Actions","status":"degraded_performance"},{"name":"Pages","status":"operational"}],"incidents":[]}
JSON
EOF
chmod +x "$work_dir/fixture-degraded/bin/curl"

output=$(PATH="$work_dir/fixture-degraded/bin:$PATH" "$repo_root/scripts/gh-status-check.sh" Actions)
case "$output" in
  *"GitHub status -- Actions: degraded_performance"*) ;;
  *) fail "expected degraded-component line, got: $output" ;;
esac

# --- Fixture 3: unresolved incident naming a matched component -------------
mkdir -p "$work_dir/fixture-incident/bin"
cat >"$work_dir/fixture-incident/bin/curl" <<'EOF'
#!/usr/bin/env sh
cat <<'JSON'
{"components":[{"name":"Actions","status":"operational"}],"incidents":[{"name":"Actions is experiencing delays","status":"investigating","impact":"minor","started_at":"2026-08-07T10:00:00Z","components":[{"name":"Actions"}]}]}
JSON
EOF
chmod +x "$work_dir/fixture-incident/bin/curl"

output=$(PATH="$work_dir/fixture-incident/bin:$PATH" "$repo_root/scripts/gh-status-check.sh" Actions)
case "$output" in
  *"GitHub incident -- Actions is experiencing delays [minor/investigating] since 2026-08-07T10:00:00Z"*) ;;
  *) fail "expected incident line, got: $output" ;;
esac

# --- Fixture 4: an unmatched component/incident is not reported ------------
mkdir -p "$work_dir/fixture-unmatched/bin"
cat >"$work_dir/fixture-unmatched/bin/curl" <<'EOF'
#!/usr/bin/env sh
cat <<'JSON'
{"components":[{"name":"Actions","status":"operational"},{"name":"Pages","status":"major_outage"}],"incidents":[{"name":"Pages is down","status":"identified","impact":"major","started_at":"2026-08-07T09:00:00Z","components":[{"name":"Pages"}]}]}
JSON
EOF
chmod +x "$work_dir/fixture-unmatched/bin/curl"

output=$(PATH="$work_dir/fixture-unmatched/bin:$PATH" "$repo_root/scripts/gh-status-check.sh" Actions)
[ "$output" = "GitHub status: Actions -- operational, no unresolved incidents" ] \
  || fail "expected Pages outage to be filtered out by the Actions-only match, got: $output"

# --- Fixture 5: curl failure is reported but never a hard error ------------
mkdir -p "$work_dir/fixture-unreachable/bin"
cat >"$work_dir/fixture-unreachable/bin/curl" <<'EOF'
#!/usr/bin/env sh
exit 7
EOF
chmod +x "$work_dir/fixture-unreachable/bin/curl"

output=$(PATH="$work_dir/fixture-unreachable/bin:$PATH" "$repo_root/scripts/gh-status-check.sh")
case "$output" in
  *"failed to reach"*) ;;
  *) fail "expected an unreachable-status line, got: $output" ;;
esac

# --- Fixture 6: --watch reports the initial bad state, then exits once the
# next poll comes back clear ------------------------------------------------
mkdir -p "$work_dir/fixture-watch/bin"
counter_file="$work_dir/fixture-watch/counter"
echo 0 >"$counter_file"
cat >"$work_dir/fixture-watch/bin/curl" <<EOF
#!/usr/bin/env sh
n=\$(cat "$counter_file")
n=\$((n + 1))
echo "\$n" >"$counter_file"
if [ "\$n" = "1" ]; then
  cat <<'JSON'
{"components":[{"name":"Actions","status":"major_outage"}],"incidents":[]}
JSON
else
  cat <<'JSON'
{"components":[{"name":"Actions","status":"operational"}],"incidents":[]}
JSON
fi
EOF
chmod +x "$work_dir/fixture-watch/bin/curl"

output=$(PATH="$work_dir/fixture-watch/bin:$PATH" POLL_INTERVAL=1 "$repo_root/scripts/gh-status-check.sh" --watch Actions)
line_count=$(printf '%s\n' "$output" | wc -l | tr -d ' ')
[ "$line_count" = "2" ] || fail "expected exactly 2 lines from --watch (bad then clear), got $line_count: $output"
case "$output" in
  *"GitHub status -- Actions: major_outage"*) ;;
  *) fail "expected initial major_outage line, got: $output" ;;
esac
case "$output" in
  *"operational, no unresolved incidents (clear)"*) ;;
  *) fail "expected a closing clear line, got: $output" ;;
esac

echo "gh-status-check.sh: valid"
