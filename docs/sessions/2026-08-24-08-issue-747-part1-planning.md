# 2026-08-24 — Planning Part 1 of #747 (PEP 604 unions), retrospective PR merged

## Status

Merged: PR #764 (docs-only — two `docs/AGENT_RETROSPECTIVE.md` entries).
No code changed in this session. Issue #763 ("Part 1 of #747: `Ty::Optional`
representation, `T | None` parsing, `is`/`is not` on `None`, and
`Optional[int]` codegen") was filed in the v0.3 milestone with a full
implementation plan posted as a comment, plus a follow-up correction
comment. Implementation of #763 has **not** started — it is left for a
separate `issue-implement` session/cycle.

Base commit at the end of this session: `origin/main` @ `77396d92`
(includes this session's merge of PR #764).

## What happened

Ran `issue-to-plan` against issue #747 per D-021 step 10 and AGENTS.md's
decomposition rule. The issue itself already called for decomposition
into dependency-ordered sub-issues rather than being planned as one unit.

The plan went through three corrected drafts before publication, each
correction recorded per D-127 (self-resolved via independent-reviewer
consultation, not by asking the repository owner):

1. **First correction (before publishing):** a representation-only Part 1
   (just `Ty::Optional` + parsing, codegen deferred to Part 2) was
   initially drafted, then judged unmergeable and revised to include real
   `Optional[int]` codegen in the same PR — reasoning at the time cited
   both a D-014 coverage-gate argument and `docs/DELIVERY_PLAN.md`
   precedent (PR #236, PR #305).
2. **Second correction (before publishing):** the plan's narrowing work
   item ("minimal `is None` narrowing") turned out to silently assume
   `is`/`is not` comparisons already had HIR lowering to attach to. Direct
   source verification (`crates/pycc_hir/src/lib.rs`'s `CmpOpKind` enum has
   no `Is`/`IsNot` variant; `docs/DELIVERY_PLAN.md` row 11 confirms both are
   rejected today via generic `C0001`) showed this compiler has zero
   `is`/`is not` support at all. The plan was split into item 5a (build
   real, tightly-scoped `is`/`is not` support) and 5b (narrow on top of
   it), and the issue body/plan comment named this as an independently
   precedent-setting seam.
3. **Third correction (after publishing, via external review on PR #764):**
   `chatgpt-codex-connector`'s review of the retrospective entry pointed
   out that the D-014 coverage argument from correction 1 was itself
   overstated — `crates/pycc_hir/src/lib.rs:62-77` shows PR #236 actually
   landed `Ty::Dict`/`Ty::Set`/`Ty::Tuple` representation-only, ahead of
   PR #305's codegen, directly contradicting the "every prior `Ty`-shape
   change ships codegen in the same PR" claim. This was verified against
   source, judged correct, and both `docs/AGENT_RETROSPECTIVE.md` and a
   follow-up comment on issue #763 were corrected: Part 1's conclusion
   (ship real `Optional[int]` codegen) still holds, but for the
   independently sufficient reason already in the plan —
   `scripts/check_conformance_breadth.py` only counts a fixture that
   actually compiles and runs byte-for-byte against CPython, which a
   clean-diagnostic-only slice cannot produce — not the coverage-gate
   argument, which was dropped.

## Where things stand

- **Issue #747** (parent, PEP 604 unions): stays open.
- **Issue #763** ("Part 1 of #747"), v0.3 milestone: open, not started.
  Plan comment: https://github.com/rotnov/pycc/issues/763#issuecomment-5399214284
  (initial plan) and https://github.com/rotnov/pycc/issues/763#issuecomment-5399287089
  (correction). No sub-issue for "Part 2" exists yet — per the plan and
  `issue-to-plan`'s own workflow, it is filed only after Part 1 merges and
  the tree reflects it.
- **PR #764**: merged (documentation-only, two retrospective entries plus
  one correction). `closingIssuesReferences.totalCount` was confirmed `0`
  before merge — it does not close #747 or #763.
- **Conformance-breadth numbers**: unchanged at 35 rows / 36 PEPs (nothing
  in this session touched `tests/fixtures/conformance-breadth-manifest.json`
  or `docs/PYTHON_STANDARDS.md`). Issue #763's plan projects 36/37 once
  implemented. **v0.3's Accept criteria (≥37 rows / ≥39 PEPs) are not met
  and will not be met by #763 alone** — 36/37 still leaves a gap of at
  least 1 row / 2 PEPs. At least one more PEP-moving issue is needed after
  #763 for v0.3 to close, independent of anything else in flight.

## For the next session

- Pick up #763 via `issue-implement` in a clean context (not a continuation
  of this planning session's own context, which has already been
  compacted once). The plan comment on #763 is self-contained and does not
  assume this session's transcript.
- Re-verify #763's plan against the tree at that time — `git fetch`,
  re-check open PRs (especially anything touching `crates/pycc_hir/src/lib.rs`'s
  `CmpOpKind`/`Ty` enums or `crates/pycc_hir/src/func.rs`'s
  `annotation_to_ty`), and re-resolve the next free `T0xxx` diagnostic code
  and `D-1xx` ADR number at that point — both were left as "re-resolve at
  PR-open time" in the plan, not fixed.
- After #763 merges and the conformance counters move to 36/37, continue
  the v0.3 issue-select loop for at least one more PEP-moving issue; the
  milestone cannot close on #763 alone.
