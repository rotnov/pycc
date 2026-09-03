# Recurrence 4: `poisonable_names`' sibling arms, three repair rounds

**Date:** 2026-09-03
**Topic:** new-case-misses-branching-sites (see `incident.md` for the class
definition and the shipped `rule` artefact in
`.claude/skills/issue-to-plan/SKILL.md` step 3, with its `no baseline
(twice)` arena verdict — **do not re-run that arena**; this file records a
further occurrence and ships a *separate*, project-local static guard for
the one dispatch site involved)
**Verdict:** profit — escalated a rung from the topic's existing `rule` to a
required-CI test; proven `verify: manual` against both historical violators
(see below)
**Batch:** `.harden/findings/issue-898.jsonl`, findings 4, 6 and 7
(correctness; two blockers and one P1), rounds 2, 3 and 5

## Symptom

`crates/pycc_hir/src/module.rs`'s `poisonable_names` answers one question per
top-level statement kind: "if this statement fails, what names would it have
bound?". #898 made imports answerable for the first time (D-222 amends D-219
rule 3). The answer was then wrong three times, once per repair round, each
repair scoped to exactly the site the previous round reported:

- round 2 — the `Stmt::ImportFrom` arm returned the *source-side* name rather
  than the locally bound one, so an aliased import both double-reported and
  suppressed a genuine diagnostic;
- round 3 — there was no `Stmt::Import` arm at all, so every rejected plain
  `import` double-reported;
- round 5 — the `Stmt::ImportFrom` arm still short-circuited to "no names" for
  any `pycc_std`-resolvable module, so a failing stdlib from-import (unknown
  symbol, `as` alias, wildcard) double-reported.

This is `incident.md`'s own predicted failure mode verbatim: "because a repair
is itself scoped to the site that was just found missing, the same omission
reproduces for every site still missing." Three consecutive rounds is the
strongest intra-batch evidence this topic has recorded.

## Root cause

The arms hand-mirror `import::lower_import_stmt`'s success conditions, and
nothing tied a mirror to its original. Each round fixed one mirror by reading
one branch of the original. No gate compared the two functions, and the
existing artefact — prose in a planning skill — operates a step earlier than
the code and had already been measured at `no baseline`.

## Termination point

`precommit`/CI tier (a test in the required suite), not the governance text.
Gap classification: **absence** — the invariant "a shape that lowers poisons
nothing, and every shape that fails poisons what it would have bound" was
stated in comments and in D-222 but asserted nowhere.

## Artefact

`crates/pycc_hir/src/module/tests.rs`: `IMPORT_SHAPES`, a corpus with one row
per rejection branch of `lower_import_stmt` plus the accepting shapes around
them, and `a_failing_import_poisons_and_a_lowering_one_does_not`, which
derives the expected answer by *calling the original* (`lower_all(...).is_ok()`)
rather than restating it, so the two functions can no longer drift apart for
any shape in the corpus.

Scope is honest: it binds this dispatch pair, not every dispatch site in the
repository. That is the ladder's own criterion — the failure is detected by a
static command, so it belongs at the static rung for the site that failed, and
the topic's general prose artefact stays where it is.

## Verify (`verify: manual` — a static gate, not an agent-behaviour artefact)

Both historical violators were reconstructed in the worktree and the guard
rejected each, then accepted the clean tree:

- round-3 violator (delete the `Stmt::Import` arm):
  `` assertion `left == right` failed: `import os` lowers=false but poisons [] `` — FAILED
- round-5 violator (restore the `is_project_import` early return):
  `` assertion `left == right` failed: `from math import sqrt, pi` lowers=false but poisons [] `` — FAILED
- restored: `test result: ok. 1 passed`

## Effect on the termination point

None on `incident.md`'s shipped rule, which is deliberately untouched. This
occurrence raises the topic's counter to 4 and records that the class's
*general* rung has now failed twice more; the per-site static rung is the
response, and a future occurrence at a different dispatch site should reach
for the same rung there rather than rewording the planning prose again.
