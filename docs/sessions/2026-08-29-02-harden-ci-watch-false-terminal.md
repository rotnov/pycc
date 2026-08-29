# 2026-08-29-02 — ci-watch.sh false-terminal fix + hand-rolled-poll incident hardening

## Status

Delivered by the pull request that carries this file (branch
`chore/harden-ci-watch-false-terminal`, based on `main` commit `8afc1d66`,
the #828 merge). Small non-milestone change under the D-192 filing bar:
referenced as a checklist item of umbrella issue #806 (agent tooling), no
dedicated issue.

## What this PR delivers

A `/harden` outcome for the 13-hour CI-wait stall (owner-reported): the
session watched CI through a hand-rolled `while`/`sleep` loop over
`gh pr checks` in a background Bash call instead of the repository watcher,
and the loop died silently. Two artefact halves:

1. **Watcher repair (repo-side, this PR):**
   `.claude/skills/gha-watch-ci-pr/scripts/ci-watch.sh` no longer treats an
   empty `statusCheckRollup` as terminal (one non-terminal NOTE after
   `EMPTY_NOTE_POLLS` consecutive empty polls, default 30, then keeps
   watching), and `READY`/`BLOCKED` require the same verdict on two
   consecutive polls — additionally bound, when the base branch's required
   status contexts are readable, to every required context being present
   and completed in the rollup, with a head change resetting the
   confirmation. `CHECK FAILED`, `MERGED`/`CLOSED`, `CONFLICTS`, and
   `STALE` stay immediate. `test-ci-watch.sh` gains fixtures 9–14
   (empty-rollup regression, between-workflow gap, one-time NOTE,
   per-streak NOTE re-fire, required-context binding across a multi-poll
   gap, head-change reset) and fixture 4's expected poll count moves
   2 → 3 for the confirmation. Both scripts are mirrored byte-identically
   to `.agents/skills/gha-watch-ci-pr/scripts/` (the Codex copies are real
   files, not forwarding stubs — a review finding). Semantics prose
   updated in `gha-watch-ci-pr/SKILL.md` and
   `autopilot-async-monitoring/SKILL.md` (frontmatter descriptions
   unchanged).
2. **Deny hook (machine-local, not in this PR):**
   `~/.claude/hooks/deny-handrolled-ci-poll.py` wired as a
   `PreToolUse`/`Bash` hook in `~/.claude/settings.json` denies commands
   combining a `gh` CI query with a `while`/`until` loop and a `sleep`;
   `ci-watch.sh` invocations are allowlisted. Kept machine-local per
   D-023/D-025. A verbatim copy plus violator/clean replay payloads lives
   in the incident fixture directory so the proof is reproducible.

Incident record: second dated entry in
`.harden/incidents/ad-hoc-ci-polling-instead-of-skill/` (the first entry,
`incident.md`, arrived via PR #829 from a parallel session on the same
complaint — that one shipped the advisory one-shot nudge; this entry ships
the complementary blocking rung and the content fix). Two retrospective
entries added to `docs/AGENT_RETROSPECTIVE.md` (transport substitution;
the #828 gate-inventory lesson the D-068 review of #828 asked to record).

## Verification

- `sh .claude/skills/gha-watch-ci-pr/scripts/test-ci-watch.sh` →
  `ci-watch.sh: valid` (11 fixtures).
- Hook fixture replay: 2 violators → exit 2, 5 clean payloads → exit 0;
  wiring proven live (the first in-session smoke test after installation
  was itself intercepted).
- `python3 scripts/validate_agent_assets.py` clean at commit time.

## In flight / follow-ups

- #779 scratch-dir migration: Parts #782 (merged via #828) done; **#783**
  (2 production sites in `src/main.rs`, empties the
  `check_scratch_dir_usage.py` allowlist) is next, then **#784** (bounded
  stale scratch-root cleanup) and **#785** (TMPDIR guidance + closing
  verification). Parent #779 closes only after all parts.
- Optional: tick the checklist item on umbrella #806 once this PR merges.

## Where to resume

Start from #783 via `issue-to-plan`/`issue-implement`. The watcher fix is
in the tree after this merge, so PR watching goes back to plain
`Monitor` + `ci-watch.sh` with no one-shot re-verification caveat.
