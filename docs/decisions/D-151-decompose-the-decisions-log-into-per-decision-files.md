---
id: D-151
title: "Decompose docs/DECISIONS.md into docs/decisions/, one file per decision, generated index"
status: accepted
---

# D-151: Decompose docs/DECISIONS.md into docs/decisions/, one file per decision, generated index

- Status: accepted
- Context: `docs/DECISIONS.md` had grown to 1532 lines and 140 index rows, becoming the
  exact kind of shared, ever-growing, must-read-then-append tail
  [D-130](./D-130-decompose-the-session-handoff-log-into-per.md) already
  moved `docs/SESSION_LOG.md` away from. Unlike that file, this one is read by ID/topic
  lookup — 35 measured inbound `#d-xxx` anchor references across 10 currently-live files
  — not chronologically, so D-130's own no-index conclusion did not transfer directly.
  The hand-maintained index had also already silently drifted before this migration even
  started: this migration's own preflight (re-verified against the live tree immediately
  before running it, not assumed from any earlier plan draft) found D-136, D-137, D-138,
  D-139, and D-150 each had a long-form entry with no matching index-table row. A sixth
  instance of the identical gap, D-148, was independently found live by
  `rotnov/pycc#363` (filed by the `ultra-review` skill's own first real dogfood run) and
  had already been fixed by a backfill in D-149's own merge (#362) before this migration
  started, so it needed no further fix here — the other five did.
- Decision: one file per decision under `docs/decisions/D-XXX-<slug>.md`, split
  mechanically from the original file's real entry boundaries and verified
  byte-for-byte before anything was deleted (`scripts/migrate_decisions_log.py`).
  `docs/decisions/README.md`'s index table is a generated artifact
  (`scripts/generate_decisions_index.py`), never hand-edited — a missing index row for
  a new decision becomes structurally impossible rather than merely less likely, the
  direct fix for the exact class of gap this migration's own preflight found five live
  instances of (plus the sixth, already-fixed instance `rotnov/pycc#363` found).
  Every inbound anchor reference across 10 currently-live files (including
  historical/dated ones, since a dangling anchor is a mechanical defect regardless of
  the surrounding file's mutability) was rewritten to the new per-file target
  (`scripts/rewrite_decisions_references.py`); bare, non-anchored mentions were updated
  only in 15 currently-live governance documents, leaving `docs/sessions/`,
  `docs/superpowers/plans/`, `docs/superpowers/specs/`, and
  `docs/AGENT_RETROSPECTIVE.md` untouched, matching D-130's own explicit precedent that
  historical narrative is not rewritten. A handful of those 15 files' bare-mention
  sentences needed a further, non-mechanical fix beyond the regex substitution itself:
  several described *adding an entry to* the generated `docs/decisions/README.md`
  (impossible now that it is generated, never hand-edited) rather than creating a new
  file under `docs/decisions/` and regenerating the index, and several others cited a
  specific decision's own note or correction (e.g. "D-116 point 4 correction note",
  "D-080 Staging note") as if that content lived in the generic index rather than in
  that decision's own file — both shapes were corrected by hand during this same
  migration, redirecting to the individual decision file whenever specific content was
  being cited. `docs/SPEC.md`'s own giant parenthetical enumerating every decision's
  topic — the most visible symptom of the problem this migration solves — is replaced
  with a short pointer to the generated index. `docs/DECISIONS.md` is deleted outright,
  no stub, matching D-130's treatment of `SESSION_LOG.md`.
- Alternatives: keep D-130's own no-index shape (`ls`-only discovery) — rejected, this
  file's real read pattern is ID/topic lookup, not chronological, and dropping the
  index would regress an actively-used workflow. Range-grouped files
  (`D001-050.md`, ...) — rejected, arbitrary boundaries tracking nothing real about the
  decisions, and each range bucket still grows without bound. Topic-grouped files —
  rejected, most decisions here span more than one area, forcing a subjective grouping
  call per entry. A hand-maintained index — rejected, reintroduces the exact
  shared-mutable-tail conflict this migration exists to remove.
- Consequences: `scripts/generate_decisions_index.py --check` is wired into CI as a
  new required-check step, so a decision file added without regenerating the index
  fails the build rather than silently drifting. Every skill, `AGENTS.md`, and spec
  document's own reference to a specific decision now resolves to that decision's own
  file directly, no anchor needed. A future decision is added by creating
  `docs/decisions/D-1NN-<slug>.md` directly in this format and regenerating the index —
  never by re-creating a single growing file. `.claude/skills/ultra-review/SKILL.md` and
  `docs/superpowers/plans/2026-08-05-ultra-review-skill.md` were not in this migration's
  own file list because they live only on unmerged PR #357 (`CONFLICTING` against
  `main` for an unrelated reason, re-verified immediately before this entry was
  written) — that PR carries 5 of its own `docs/DECISIONS.md#d-xxx`
  anchor references that become dead links the moment this migration merges. PR #357's
  own rebase needs to apply the same anchor rewrite to its changed files before it can
  merge cleanly after this one. `rotnov/pycc#363` remains open as filed (its own
  suggested one-line fix was superseded by this structural migration rather than
  applied literally) — closing it with a citation to this entry is a natural follow-up
  but is left to a separate pass rather than folded into this migration's own scope.
