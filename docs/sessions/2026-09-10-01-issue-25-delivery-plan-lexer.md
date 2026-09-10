# 2026-09-10-01 — #25: drop `pycc_lexer` (and `pycc_testkit`) from the delivery plan's v0.1 crate row

## Overall status

Delivered as one docs-only pull request carrying `Fixes #25`. Baseline:
`origin/main` at `0cc4e0ce` ("Extract the constraint-collection tests into
their own module (part of #695) (#995)"). Branch
`fix/issue-25-delivery-plan-lexer`, worktree
`/Users/denis/projects/pycc-worktrees/issue-25`. One other open pull request at
the D-078 checkpoint: #996 (`pycc_types` `tests.rs` extraction, part of #695) —
no file overlap.

## What landed

`docs/DELIVERY_PLAN.md`, milestone table only:

- v0.1 row (line 11): `pycc_lexer` removed from the "New crates / major
  additions" list, replaced by an inline pointer — `no pycc_lexer until v0.6,
  D-017` — so the row agrees with D-017 (v0.1 creates `pycc_parser` +
  `pycc_ast` only) and with the file's own "Deliberately deferred" paragraph
  (line 44), which already deferred it.
- v0.6 row (line 16): `pycc_lexer created alongside it (D-017)` added next to
  "own parser replaces vendored `ruff_python_parser`", so the crate's creation
  is now scheduled where D-017 places it.
- Same v0.1 cell, surfaced by the D-068 review round: `pycc_testkit` was also
  still listed as a v0.1 addition although D-018 deferred it and D-085 replaced
  it with a plain `tests/conformance.rs` integration test (lines 44, 66, 150
  and 176 already said so; `Cargo.toml` has no such member). Dropped with a
  D-018/D-085 pointer — same defect class, same row, fixed in the same change.

Not edited, deliberately: `docs/ARCHITECTURE.md` (its crate table describes a
target state per D-017), `docs/superpowers/plans/*` (historical, already
marked as such, and already say `pycc_lexer` is not created for v0.1), and
`docs/ROADMAP.md`/`docs/SPEC.md` (neither mentions the crate).

## Evidence

Docs-only diff; the Rust gates (fmt/clippy/coverage) were not run locally —
CI's fail-closed classifier decides coverage selection for the pull request.
Local gates, exit status captured directly: `ruby
scripts/check_roadmap_evidence.rb` 0, `python3
scripts/generate_decisions_index.py docs/decisions docs/decisions/README.md
--check` 0, `python3 scripts/validate_agent_assets.py` 0, `python3
scripts/validate_agent_policies.py` 0, `python3 -m unittest discover -s
scripts` 0 (996 tests, 6 skipped), `ruby scripts/check_ci_permissions.rb` 0,
`scripts/check-site.sh` 0, `python3 -B scripts/check_harden_findings.py
.harden/findings/issue-25.jsonl` 0. `grep -rn pycc_lexer docs/` outside the
historical set returns only the two edited lines and D-017's own file.

## Process notes

- Workflow deviations, recorded per D-127: no formal `issue-to-plan` round
  (AGENTS.md D-021 step 10's single-file docs-correction exemption), and the
  edit was made directly in this session rather than through a dispatched
  implementer — a two-cell change has no context-growth case for D-142.
- Selection: `issue-select` advisor round clean; runners-up were #80, #260,
  #261 (v0.4).
- D-068 review: one round, two findings — the `pycc_testkit` P2 above (fixed)
  and a P3 asking for this session file (refuted: step 6 writes it after the
  loop). `/harden batch` clustered both into existing journal topics
  (`documentation-sweep-stops-at-the-changed-file`, recurrence 10;
  `reviewer-flags-a-later-phase-deliverable`, recurrence 5 — the brief was
  again typed freehand instead of from `references/review-brief.md`); build
  nothing in both, counters recorded under `.harden/incidents/`.

## Known follow-ups

None filed. The remaining `pycc_testkit` mentions in the file (lines 44, 66,
150, 176) are consistent with each other and with D-018/D-085.

## Where to resume

Re-enter `issue-select` step 1 from `origin/main` after this merge; the
denylist is empty. Issue #25 closes with the merge.
