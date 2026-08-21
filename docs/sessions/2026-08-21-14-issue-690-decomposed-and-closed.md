# 2026-08-21-14 — #690 decomposed and closed; v0.3's fifth conformance row now has a tracker

**Baseline:** `origin/main` = `7f0257712c644afac33ec10be0ca2bb332f8808c` (the squash merge of PR #699).
**Open pull requests:** none.

## What landed

[PR #699](https://github.com/rotnov/pycc/pull/699) — a one-paragraph `docs/ROADMAP.md` correction,
merged, closing [#690](https://github.com/rotnov/pycc/issues/690). The paragraph previously ended
"the remaining 5-row gap is tracked by #690 … and is to be decomposed once each row's blocker is
known"; it now records the decomposition instead of promising it.

Alongside it, three tracker writes:

- [#698](https://github.com/rotnov/pycc/issues/698) filed (milestone v0.3) — a feasibility pass
  over the unassessed `☐` matrix rows, which is what actually sources v0.3's fifth conformance row.
- #690's title corrected from "6-row" to "5-row", reconciling it with `docs/ROADMAP.md`.
- Two comments on #690 carrying the evidence below, plus a third correcting the first.

## The finding, and the limit of the finding

An exhaustive parse of every status-bearing row in `docs/PYTHON_STANDARDS.md` — 95 rows, 63 of them
`☐` — extracted every backticked `.py` path each row cites and tested it under `tests/fixtures/`,
under `tests/`, and bare. **Zero hits.** No `☐` row cites a fixture that already exists.

This matters because the seven-row jump recorded earlier in that same roadmap paragraph came
exactly from that category: fixtures already authored and already passing, with the matrix simply
not updated. That well is empty; every remaining row needs implementation work.

What is *not* established is the stronger claim an earlier comment of mine published: that no `☐`
row is reachable as a *bounded* change. That was asserted over the whole set from a small subset.
D-153's own feasibility pass assessed 11 PEPs (8 still `☐`), and this session assessed 2 more —
**53 of the 63 rows have never been assessed by anyone**, and closing that gap is #698's whole
purpose.

The 2 assessed here, both refuted as cheap:

- **PEP 3115 (metaclasses)** — `crates/pycc_hir/src/class.rs:1026` rejects every class-header
  keyword argument with `C0001`; `crates/pycc_diag/src/explain.rs:804` states outright that `E0105`
  "has nothing to guard today".
- **The unnumbered `str`/`bytes` split row** — pycc has no `bytes` type. The sole `Bytes`
  occurrence in `crates/pycc_types` is the builtin name string `"BytesWarning"`
  (`lib.rs:506`); `Bytes` appears nowhere in `pycc_hir`, `pycc_mir`, or `pycc_codegen`.

No ADR was opened. D-153's Decision 2 already permits reaching the count from outside its itemized
set — the itemization sums to exactly 37, so any member that later proved unreachable (PEP 487's
recognition-only surface, #585) leaves it short, and Decision 2 anticipates that by naming eight
outside rows as welcome contributors. Superseding D-153 would re-litigate what it already settled.

## Process note worth carrying forward

The pinned local reviewer caught a real factual error in #699 **before** merge: the roadmap text
said D-153's feasibility pass "covered 8 candidates" when it assessed 11 (3 have since flipped via
PR-23). Corrected in the ROADMAP, in #698's body, and by a follow-up comment on #690 — which also
made the "53" figure exact rather than a lower bound.

This is the same error class as PR #691's wrong `core` gap, already in
`docs/AGENT_RETROSPECTIVE.md`: a derivation published as complete while covering only part of its
domain. It occurred three times this session. The reviewer catching the third instance pre-merge is
the only reason it did not become a fourth retrospective entry.

## v0.3 status

**Not met.** 32 of the required 37 rows are at `◐` or better; the gap is 5 rows, now fully tracked:
four via [#542](https://github.com/rotnov/pycc/issues/542) (PEP 654) and
[#543](https://github.com/rotnov/pycc/issues/543) (PEPs 3151, 765, 758), both gated on
[#541](https://github.com/rotnov/pycc/issues/541); the fifth via #698.

## Paused autopilot

- **Directive:** `/next-milestone` invoked with no arguments — adopt the first `## vX.Y` roadmap
  section whose Accept bullet is unmet on verified evidence, then loop through
  `.claude/skills/issue-select/SKILL.md` until the milestone's criteria are met.
- **Active milestone:** v0.3. Accept criteria **not** met (32 of 37).
- **Last iteration's outcome:** #690 implemented and closed via PR #699; #698 filed as its
  decomposition.
- **Next step:** re-enter `issue-select` step 1 from `7f025771`. The substantive milestone work is
  the exception chain — #541 first, which unblocks #542 and #543 and supplies four of the five
  remaining rows.
- **In-run denylist (must carry forward):**
  - **#20** and **#631** — deprioritized per #20's own last comment (the CI `cargo build`/`cargo
    test` ordering workaround).
  - **#604** — denylisted earlier in this run; **the original stop reason was not recovered**
    across a context boundary and is recorded here as unrecovered rather than reconstructed. A
    session that reselects it should re-derive its standing from scratch rather than trusting this
    denylist entry as a judgment.

## Loose ends, unchanged from the previous checkpoint

- #676, #677, #685, #687; D-171's stale lines 8 and 12; the 2026-08-01 issue-109 plan doc's line 50
  / Task 5; the orphaned `tests/fixtures/policy-successors/`; `src/project_config.rs:116`'s citation
  of a nonexistent test gap; a mechanical CI guard over declared `closingIssuesReferences`.
- **New, and directly relevant to the guard already listed above:** PR #700's own
  `closingIssuesReferences` reported one declared closure that was never intended. Editing the
  pull-request body left the count at 1; rewriting the *commit message* — which contained
  "closed via PR #699" — took it to 0. AGENTS.md's rule describes the pattern as a closing keyword
  "immediately followed by an issue reference", and this phrase has two words between the keyword
  and the reference, so the rule as written understates the trap. The attribution to #690 rather
  than #699 is unexplained; recorded here as a measured observation, not a mechanism. Anyone
  sharpening the rule should establish the mechanism first rather than generalizing from this one
  data point.
- **New:** running `python3 scripts/check_source_links_live.py` locally rewrites the tracked
  `site/python-aot-compilers/source-link-registry.json`. Reverted here rather than committed as an
  unrelated change; whether that write is intended for local runs is unexamined.
- **`/harden` candidates:** `issue-implement` step 4 still describes D-103's retired exact-byte
  gate as live; `issue-select` step 5's priority-marker ordering; `.harden/` being gitignored means
  the batch journal never reaches review; the append-only-flag-at-creation fix belongs upstream;
  and the thrice-repeated "partial derivation published as complete" class now warrants its own
  guard.
