# Fixture: deny-handrolled-ci-poll hook proof payloads

Replay evidence for the 2026-08-29 recurrence entry beside this directory
(`2026-08-29-handrolled-loop-and-watcher-repair.md`). The live hook is
machine-local (`~/.claude/hooks/deny-handrolled-ci-poll.py`, wired as a
`PreToolUse`/`Bash` hook in `~/.claude/settings.json`, deliberately not part
of this repository per D-023); `deny-handrolled-ci-poll.py` here is a
verbatim copy so the proof is reproducible from a clean checkout.

Replay:

```sh
for f in violator-*.json; do
  python3 deny-handrolled-ci-poll.py <"$f"; echo "$f exit=$?"   # expect 2
done
for f in clean-*.json clean-5-malformed.txt; do
  python3 deny-handrolled-ci-poll.py <"$f"; echo "$f exit=$?"   # expect 0
done
```

- `violator-1-incident-loop.json` — the exact loop shape from the incident
  (background `while true; gh pr checks; sleep`).
- `violator-2-until-view.json` — `until gh pr view ...; sleep` variant.
- `clean-1-oneshot-checks.json` — a one-shot status check (legitimate; the
  advisory nudge from this topic's first entry covers it instead).
- `clean-2-sanctioned-watcher.json` — the sanctioned `ci-watch.sh`
  invocation (allowlisted).
- `clean-3-logfile-until.json` — a poll loop that does not query CI.
- `clean-4-non-bash.json` — a non-Bash tool call.
- `clean-5-malformed.txt` — unparseable input (the hook must fail open).

The watcher-repair half of the entry is proven by the skill's own harness,
`.claude/skills/gha-watch-ci-pr/scripts/test-ci-watch.sh`, which the
`agent-assets` workflow runs in CI — not duplicated here.
