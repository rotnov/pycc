# 2026-09-04 (08) — #918 Part 1: parameterized container annotations lower in every non-return position

## Overall status

Delivered as PR #930 (`feat/issue-918-container-annotations`, head `ebf48540`),
based on `origin/main` at `c639e682`. State at the time this snapshot was written:
open, not a draft, `MERGEABLE`, CI in progress. It carries `Fixes #918` and the
GraphQL `closingIssuesReferences` query reports `totalCount: 1` naming only #918.

`pycc_hir::func::annotation_to_ty` now lowers `list[T]`, `set[T]`, `dict[K, V]`
and `tuple[A, B, ...]` in parameter, local and module `AnnAssign`, PEP 695, and
legacy `TypeAlias` positions. Diagnostics: T0034 (list), T0036 (dict), T0038
(set), T0039 (tuple element), T0042 (`Ty::Param` type argument), C0001 (position
gates), and a new **T0053** covering arity plus the ellipsis forms.

The ellipsis check runs **before** the arity check, and that order is
load-bearing rather than stylistic: `tuple[int, ...]` has a legal arity of 2, so
an arity-first check accepts it and silently lowers `tuple[int, EllipsisType]`.
A regression test pins the order at `crates/pycc_hir/src/tests.rs`'s `a_variadic_ellipsis_type_argument_is_rejected_in_every_family`.

## What is deliberately not in this change

Return position. `lower_return_annotation` still rejects containers, and that is
tracked as **#925 ("Part 2 of #918: container-typed call results in
`pycc_codegen` and container return-type annotations")**, which stays open. #918
Part 1 is the annotation-lowering seam only.

Protocol **attributes** also still reject containers, and the justification is
unsatisfiability rather than an unimplemented path: every route that establishes
an instance slot restricts it to a scalar (D-154). A protocol **method's**
parameter lowers a container normally — verified compiling and running end to
end, and covered by
`crates/pycc_hir/src/tests.rs::a_container_annotation_lowers_in_a_protocol_method_parameter`.
Six documentation sites originally claimed the gate covered protocol *members*;
all six now say *attributes*.

## Decision record

`docs/decisions/D-228-lower-parameterized-container-type-annotations.md`. It was
authored as **D-227** and renumbered in `f81d3445` after PR #928 merged
mid-review and claimed D-227 for its own record. Commit `518acbc1`'s subject line
still says D-227; history was deliberately not rewritten, since the number is
correct as of that commit and the renumber is its own auditable commit.

## Follow-ups opened by this session

- **#929** (v0.4) — wire the existing `scripts/generate_decisions_index.py --check`
  into `ci.yml`'s `governance` job. The checker already implements fail-closed id
  uniqueness and index freshness and is wired to no gate at all
  (`grep -rn "generate_decisions_index" .github/workflows/` is empty), which is
  why the D-227 collision above reached review undetected. The issue body carries
  the reproduction and, importantly, the **D-080 cost**: `ci.yml` is pinned by
  whole-file SHA-256 in `check_roadmap_evidence.rb`'s
  `REVIEWED_PERF_CI_WORKFLOW_SHA256S`, so even a two-line step needs the
  two-sequential-PR staged-fixture procedure.

## Process artefacts

A `/harden batch` pass over `.harden/findings/issue-918.jsonl` (nine findings,
four rounds) clustered into seven classes. One ships a guard, delegated as #929;
six terminate at "build nothing, deliberately" — four because
`references/rule-audit.md` disqualifies a further textual artefact in topics
already at three or more files, two because the gap is compliance rather than
content. Two of those six (a gate sweep that omitted `cargo fmt`; a review brief
typed freehand rather than composed from `references/review-brief.md`) were
weighed against AGENTS.md's filing bar for process observations and deliberately
**not** filed: neither can cause an incorrect merge, so both are retrospective
lines. The full class table and its reasoning are in PR #930's body.

`.harden/incidents/` gains three new topics
(`whole-file-conflict-resolution-drops-a-hunk`, `gate-sweep-omits-a-required-check`,
`decision-number-taken-by-a-merge-mid-review`) and counters in four existing ones.
`docs/AGENT_RETROSPECTIVE.md` gains five entries.

The occurrence-4 discriminator that
`.harden/incidents/reviewer-flags-a-later-phase-deliverable/2026-09-02-issue-868.md`
pre-registered is **resolved**: grepping this session's transcript for
`review-brief` returns hits only from the 2026-09-02 session that authored the
template and from today's tracer report, so the template was never opened during
this session's review dispatch. Gap type is compliance, and the topic is now at
four files.

## Where a fresh session should resume

1. **#925 (Part 2, return position)** needs its own `issue-to-plan` run against
   the post-merge tree — not against this branch, since the Part 1 lowering it
   builds on only exists after #930 merges.
2. **#929** is the CI-governance follow-up and needs the two-PR staged shape;
   read its body before starting, the digest pin is the whole cost.
3. **#926** (a private helper returning a list literal panics in codegen) is
   adjacent and still open.

## Gates

Run from a single-writer baseline. Coverage was measured on `95e51835`:
`--fail-under-lines 100 --fail-under-regions 100` exit 0, TOTAL 52,483 lines and
34,354 regions with zero uncovered. `git diff --name-only 95e51835..HEAD --
crates src Cargo.toml Cargo.lock` is empty, so the Rust tree that run measured is
byte-identical to the head shipped here. Every other gate — fmt, clippy, build,
doc, the `scripts/` unittest suite, `check-site.sh`, roadmap evidence, README
milestone projection, CI permissions, status-page freshness, conformance breadth,
agent assets, decisions index `--check`, harden findings — was re-run on
`ebf48540` and returned 0, with each exit status captured directly rather than
through a pipeline.

Both Ruby checkers need `LC_ALL=en_US.UTF-8 RUBYOPT=-EUTF-8` in this shell; that
is a local locale artifact, not a repository defect.
