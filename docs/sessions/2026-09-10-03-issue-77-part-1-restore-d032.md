# 2026-09-10 (03) — issue #999: restore D-032 verbatim (Part 1 of #77)

## Status

Delivered by the pull request that carries this file. Base `c6623900`
(`origin/main` at branch time); branch `fix/issue-77-restore-d032-verbatim`,
one code commit `4028cbda` touching only
`docs/decisions/D-032-branch-protection-requires-one-aggregate-ci-gate.md`.

## What was restored

PR #74 appended ` (The switch has since happened -- see D-033, a new entry
rather than an edit to this one.)` to the `- Consequences:` line of the
already-accepted D-032. That sentence is removed. Evidence: diffing the
D-032 section of `docs/DECISIONS.md` at `2bffab2c` (the pre-PR-74 base)
against `tail -n +7` of the restored file prints exactly one hunk, `9a10`,
adding the trailing blank line the D-151 migration gave every decision file.
Before the revert the same diff also showed the parenthetical.

## Deliberately untouched

`D-033` (its claim that D-024's and D-032's text was "left exactly as
originally written" becomes true again with this revert; editing it would
be a fresh accepted-decision rewrite), `AGENTS.md`, `TEMPLATE.md`, the
generated `docs/decisions/README.md` (frontmatter unchanged, `--check`
green), `docs/ROADMAP.md`/`DELIVERY_PLAN.md` (no behavior or sequencing
change).

## Gates at HEAD

All nine `governance` steps reproduced locally, exit 0 (unittest 1028 OK,
6 skipped). D-068 review: one `ievo:deep-reviewer` round, zero findings.

## Follow-ups and merge order

- #1000 — Part 2 of #77: the accepted-decision immutability guard, its ADR
  and CI wiring. It must merge **after** this pull request: a base-vs-head
  guard would fail this revert's own PR. #77 stays open until #1000 closes.
- Plan for both parts, with Part 2's verified design inputs:
  https://github.com/rotnov/pycc/issues/77#issuecomment-5623620809

## Where to resume

`docs/decisions/D-032-*.md` line 15 and D-033 line 11 for the restored
state; #1000 for the next unit of work.
