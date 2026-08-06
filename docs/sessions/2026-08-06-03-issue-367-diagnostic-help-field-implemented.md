# 2026-08-06-03: Issue #367 (`Diagnostic.help` field, Part 2 of #338) implemented, PR open

## Status

Implementation complete, locally gated green, D-068 pinned review clean (2 rounds; a real
title/doc-scope finding fixed, a trivial follow-up title finding fixed). Rebased through an
unrelated concurrent `docs/DECISIONS.md` → `docs/decisions/` migration (PR #372, merged mid-task
by a separate process). Pull request about to open against `origin/main` @ `17ef4d4`. Not yet
merged.

## What happened

Fourth iteration of the standing v0.3 autopilot loop, continuing directly after #141, #361, and
#366 all merged.

1. **Selection**: #338 ("Make diagnostics LLM-legible") was decomposed this session (before
   planning, not left to `issue-to-plan`) into #366 (Part 1, `pycc explain`, already merged) and
   #367 (Part 2, this issue: a `Diagnostic.help` field + JSON schema population). #367 has no
   priority marker but was the only unblocked survivor in the v0.3 pool once #142 (P2) stayed
   collision-deferred against #358, which still touches `crates/pycc_types/src/lib.rs`.
2. **issue-to-plan** (dispatched): settled two open questions the issue itself left vague —
   the constructor/API shape (a `.with_help()` builder, zero signature changes at any of the 94
   production call sites, since all already go through `Diagnostic::error`/`::warning`) and,
   critically, exactly *which* of those 94 sites get a `help` value (46, under an auditable
   textual selection rule, enumerated in a full per-site table grouped into message-shape
   families with worked templates — not left as "decide at implementation time"). The plan's own
   2-round review caught and fixed two real defects in its own draft before publishing: an
   inverted variable-order bug in one help-text family (would have produced a false suggestion at
   10 call sites) and one misclassified call site inventing wording beyond its message.
3. **Implementation** (dispatched): followed the per-site table exactly — 44 sites in
   `crates/pycc_types/src/lib.rs`, 2 in `crates/pycc_hir/src/lib.rs`. Hit one unanticipated
   fallout: growing `Diagnostic` by one field pushed `clippy::result_large_err` over its
   threshold; fixed by boxing `FrontendFailure::Compile`'s field in `src/main.rs` (mechanical,
   no behavior change).
4. **Local gates, all green**: full `cargo test --workspace` pass, clippy clean, **100.00%**
   lines/regions coverage, `cargo doc` clean, roadmap-evidence check passed.
5. **D-068 pinned review**: round 1 found one real [warning] — the three normative docs
   (`CLI_SPEC.md`/`DIAGNOSTICS.md`/`ROADMAP.md`) described the populated families as only
   "arity/type-mismatch and missing-annotation," omitting a third family the diff also populated
   (3 `T0040` tuple-index sites) — plus a [note] that the tuple-index help text itself elaborated
   slightly beyond its own message. Both fixed. Round 2 confirmed both fixes, independently
   re-verified the 46-site count two ways, and found one more trivial instance of the same
   naming gap in the new ADR's own title (not caught by the first fix's file list). Fixed;
   reviewer judged the loop conclusible without a third round.
6. **Concurrent-actor rebase**: while this issue was mid-review, an unrelated PR (#372,
   `docs/DECISIONS.md` → `docs/decisions/`, one file per decision plus a generated index) merged
   to `main` — matching this repository's known pattern of other concurrent autonomous actors
   pushing to shared branches mid-session. Rebasing surfaced two real conflicts: `docs/DECISIONS.md`
   itself (deleted upstream, modified on this branch) and `docs/SPEC.md`'s decisions-log
   summary line (replaced upstream with a short pointer to `docs/decisions/README.md`, vs. this
   branch's now-obsolete giant one-line summary addition). Resolved by moving this issue's ADR
   entry into a new `docs/decisions/D-152-*.md` file (the migration's own `D-151` had already
   claimed the number this issue's plan/implementation used, so every `D-151` reference in this
   issue's own commits was renumbered to `D-152` during conflict resolution — including the
   review-round title fix, which needed re-applying to the new file), taking `SPEC.md`'s new
   simplified pointer line as-is, and regenerating `docs/decisions/README.md` via
   `scripts/generate_decisions_index.py`. Full gate suite re-run clean post-rebase.

## Known follow-ups (not blockers for this PR)

- `render_human` still has no `help:` line codepath — explicitly out of scope, a possible future
  "Part 3" the plan recommended filing but did not file itself (not yet tracked by any issue).
- #338 should close automatically once this PR merges (both #366 and #367 will be closed).

## Where to resume

If this session ends before the PR merges: task branch `feat/issue-367-diagnostic-help` in
worktree `.claude/worktrees/issue-367-diagnostic-help`, rebased onto `origin/main` (`17ef4d4`),
working tree clean, not yet pushed. Push it, open the PR (`Fixes #367`), and resume at
`issue-implement`'s own step 7 (monitor) / step 8 (merge). **Before merging, re-fetch and
re-verify `D-152` is still the correct/free number** — this branch was already renumbered once
mid-session due to a concurrent actor's own migration claiming `D-151` first; another PR could
claim `D-152` before this one merges. The standing v0.3 autopilot directive continues after this
issue merges — re-enter `issue-select` step 1 with a fresh baseline (task #9 in this session's
task list carries the standing-directive context forward across a compaction boundary).
