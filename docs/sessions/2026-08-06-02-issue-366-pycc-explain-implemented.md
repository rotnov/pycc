# 2026-08-06-02: Issue #366 (`pycc explain`, Part 1 of #338) implemented, PR open

## Status

Implementation complete, locally gated green, D-068 pinned review clean (1 round, 0 findings).
Pull request about to open against `origin/main` @ `3694448`. Not yet merged.

## What happened

Third iteration of the standing v0.3 autopilot loop, continuing directly after #141 (PR #362)
and #361 (PR #365) both merged.

1. **Selection**: after #361 merged, the only remaining P2 issue (#142) stayed deprioritized
   (collision with two other-actor PRs still touching `crates/pycc_types/src/lib.rs`), #359/
   #354 stayed blocked, #336/#335 are hard exclusions (release/publish lifecycle), #337 is
   D-103-deprioritized (manifest-protected `ci.yml`). #338 ("Make diagnostics LLM-legible:
   implement `pycc explain`, add a structured help field") was the only survivor and directly
   closes one of v0.3's own Accept-criteria conjuncts (`pycc explain` live).
2. **Decomposition** (before planning, not left to `issue-to-plan`): #338 bundled two
   independent seams — a new CLI subcommand (own explanation-data-source decision) and a core
   `Diagnostic` struct field change (JSON schema, fixture regeneration). Split into #366 (Part
   1, this PR) and #367 (Part 2, not started). #338 stays open until both close.
3. **issue-to-plan** (dispatched, 4 substantive review rounds — including catching and fixing a
   real error in the plan's own first fix, per this session's discipline of treating even the
   planning stage's own drafts as fallible): settled the explanation-data-source question as a
   coverage-gate decision (hand-authored `const` table over a runtime markdown parser, mirroring
   `pycc_std::REGISTRY`'s existing precedent), found the "both SKILL.md files need editing"
   premise was false (the Codex mirror is a thin pointer, parity-checked only at the frontmatter
   level), and tracked a live three-way ADR-numbering collision across three other open PRs
   (#360, #357, #368) through to a re-resolved target number.
4. **Implementation** (dispatched): new `crates/pycc_diag/src/explain.rs` (42-entry
   hand-authored explanation table, grep-verified against real emission sites, not just doc
   prose), a drift-guard test parsing `docs/DIAGNOSTICS.md` at test time to catch future
   completeness gaps, `--format human|json` on `Command::Explain` (deliberately distinct from
   `check`'s `--error-format`). **PR #360 merged mid-implementation** (adding diagnostic code
   `T0041` and initially claiming, then not actually landing, ADR `D-150`) — the implementer
   rebased, added the new `T0041` entry, and renumbered the ADR from the plan's stale `D-151`
   snapshot back down to the now-actually-free `D-150`.
5. **Local gates, all green**: full `cargo test --workspace` pass, clippy clean, **100.00%**
   lines/regions coverage, `cargo doc` clean, `validate_agent_assets.py` valid,
   `check_roadmap_evidence.rb` passed.
6. **D-068 pinned review**: 1 round, 0 findings — the reviewer independently re-verified every
   specific claim (all 42 explanation entries against real source call sites, the drift-guard's
   bidirectionality and generic severity-normalization, JSON schema field distinctness, the
   plain-stderr-even-in-JSON-mode error path, the full ADR-renumbering cross-reference set) and
   found no discrepancies. No fix round was needed.

## Known follow-ups (not blockers for this PR)

- #367 (Part 2 of #338: `Diagnostic.help` field + JSON schema extension) is open, milestone
  v0.3, not started — #338 itself stays open until it lands too.
- Same open-PR landscape note as #141/#361: #357 still independently claims ADR `D-147` on its
  own unmerged branch.

## Where to resume

If this session ends before the PR merges: task branch `feat/issue-366-pycc-explain` in
worktree `.claude/worktrees/issue-366-pycc-explain`, ahead of `origin/main` (`3694448`),
working tree clean, not yet pushed. Push it, open the PR (`Fixes #366`), and resume at
`issue-implement`'s own step 7 (monitor) / step 8 (merge). The standing v0.3 autopilot
directive continues after this issue merges — re-enter `issue-select` step 1 with a fresh
baseline (task #9 in this session's task list carries the standing-directive context forward
across a compaction boundary).
