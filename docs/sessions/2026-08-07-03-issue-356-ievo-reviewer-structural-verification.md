# 2026-08-07 checkpoint 03 — issue #356: stop exact-pinning the Claude-side ievo reviewer

## Status

PR [#399](https://github.com/rotnov/pycc/pull/399) opened against `main`, targeting
`f4b39789eb316c56fc5ec676db1a33465e37ae1a` (the commit that landed
[#358](https://github.com/rotnov/pycc/pull/358), "Fix #22: preserve Python execution
order for function definitions"). Not yet merged. Fixes
[#356](https://github.com/rotnov/pycc/issues/356).

## What this checkpoint delivers

Issue #356 reported that D-068's local pinned-reviewer loop verifies an exact
`sha` recorded in `.claude/settings.json` but does not actually bind to it: on a
real, non-isolated machine the installed `ievo@ievo-skills` plugin resolves to
whatever commit last won registration in Claude Code's machine-global marketplace
registry (`~/.claude/plugins/known_marketplaces.json`), not the pinned one. Only an
isolated `CLAUDE_CONFIG_DIR` ever actually observed the pinned commit.

The fix drops the exact-commit pin as a deliberate design change, not a patch:

- `.claude/settings.json`'s `ievo-skills` plugin source no longer carries a `sha`/
  `ref`; `scripts/validate_agent_assets.py` now rejects either key outright.
- `scripts/check-claude-marketplace.sh` (CI-safe) verifies structural
  well-formedness — non-empty deep-review entrypoint and deep-reviewer agent
  artifacts, semver-shaped plugin manifest `version` — instead of an exact
  SHA-256 digest.
- New `scripts/check_claude_reviewer_binding.py` is the local, non-CI half: hard
  fails only when no structurally intact install can be found at all for the
  current project (or a fallback user-scope install); otherwise prints an
  advisory freshness note against the latest upstream tag that never blocks
  dispatch.
- New decision `docs/decisions/D-155-stop-exact-pinning-the-claude-side-ievo-reviewer.md`
  supersedes `docs/decisions/D-068-use-a-pinned-local-reviewer-as-the-required.md`
  (single-word `status: superseded` in frontmatter, matching the repository's
  existing convention — confirmed against D-046/D-047/D-097/D-106 rather than
  inventing a hyphenated status value as the originally published plan proposed).
- `docs/AGENT_TOOLING.md`, `AGENTS.md`, and `.claude/skills/issue-implement/SKILL.md`
  rewritten to describe the structural/advisory model in place of the old
  exact-digest-pin language throughout (pin table, update process, local review
  workflow, rollback, review-loop step).

## Bug found during implementation

Running `check_claude_reviewer_binding.py` against this machine's real
`~/.claude/plugins/installed_plugins.json` (not just self-consistent test
fixtures) surfaced that the file's real schema wraps plugin entries under a
`"plugins"` key (`{"version": 2, "plugins": {"<key>": [...]}}`), not the flat
top-level mapping the script initially assumed. Fixed, with a regression test
(`test_installed_plugins_file_without_plugins_wrapper_fails_closed`) guarding the
schema shape directly.

## This session's own D-068/D-155 review loop

Because this fix changes how the D-068 review loop itself binds its reviewer, the
loop's own dispatch was the first live test of the new mechanism: with no
isolated `CLAUDE_CONFIG_DIR`, `check_claude_reviewer_binding.py` confirmed a real,
structurally intact `ievo@ievo-skills` install for this project (0.78.8 installed,
0.78.13 available), and the pinned `ievo:deep-reviewer` agent was dispatched
directly against the full branch diff. First round: 5 non-blocking findings (1
warning, 4 notes) — a wording inconsistency between two sibling error messages in
`check-claude-marketplace.sh`, a stale "Pinned repository agent tooling."
description string, a real edge-case bug (an `installed_plugins.json` entry with
a missing/empty `installPath` defaulted to `Path("") -> Path('.')`, whose
`is_dir()` is almost always `True`, so the binder produced a misleading
"structurally incomplete install" instead of "NOT FOUND"), and a hardcoded
personal machine path in a test fixture. All four confirmed and fixed with
regression tests; one (generic "pinned reviewer" wording surviving elsewhere in
`docs/AGENT_TOOLING.md`, matching `AGENTS.md`'s still-current "Local pinned
review loop" heading) was reviewed and left unchanged as a deliberate,
defensible terminology choice, not stale exact-pin framing. A scoped re-review
confirmed all four fixes and found no new issue in the fix commit itself. The
loop is closed.

## Local gate evidence

- `python3 -m unittest discover -s scripts -p 'test_*.py'` — 543 tests pass
  (6 environment-gated skips), up from 541 before this branch's two new
  regression tests.
- `python3 scripts/validate_agent_assets.py` — valid.
- `bash scripts/check-claude-marketplace.sh` — passes live against this
  machine's real Claude Code install.
- `python3 scripts/check_claude_reviewer_binding.py` — reports a real,
  structurally intact install.
- `python3 scripts/generate_decisions_index.py docs/decisions docs/decisions/README.md --check`
  — up to date.

## Known follow-ups (out of scope for this PR)

- The originally published plan for #356 noted PR [#357](https://github.com/rotnov/pycc/pull/357)
  ("Add ultra-review: periodic evidence-gated review that files prioritized
  issues") is based on a stale, pre-D-151-decomposition tree — it still touches
  the now-nonexistent monolithic `docs/DECISIONS.md`. That PR needs its own
  rebase/reconciliation once #356 merges; not addressed here.
- After #356 merges, this session's standing task is to resume the D-068/D-155-gated
  "ultra review" for PR #357.

## Where to resume

If this session ends before PR #399 merges: check its state
(`gh pr view 399 --repo rotnov/pycc`), monitor CI per D-078, resolve any review
threads, and merge once every required gate is green and the diff has been
re-read end to end. After merge, resume the ultra-review skill against PR #357
per the note above.
