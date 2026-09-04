# 2026-09-04 (08) — #918 Part 1: parameterized container annotations lower in every non-return position

## Overall status

Delivered as PR #930 (`feat/issue-918-container-annotations`, head `c3b036f7`
plus this snapshot's own commit), based on `origin/main` at `c639e682`,
re-fetched and unmoved. State re-resolved immediately before this snapshot was
committed: open, not a draft, `MERGEABLE`, all six review threads answered and
resolved, CI green on every check that had reported. It carries `Fixes #918` and
the GraphQL `closingIssuesReferences` query reports `totalCount: 1` naming only
#918.

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
four rounds at the time of the pass; fifteen over six rounds now) clustered
into seven classes. One ships a guard, delegated as #929;
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
`docs/AGENT_RETROSPECTIVE.md` gains six entries.

A fifth review round on `b3f8f225` produced four further findings, all verified
empirically against a control before being accepted and all fixed in `76d9f544`
and `d1d22692`: a bare-container message that was not cascade-shaped, the same
message emitted from position-blind `annotation_to_ty` and therefore false in
the positions where the parameterized form is itself rejected, a PEP 695 type
parameter shadowed by the container dispatch, and a `tuple`-only ellipsis
advice offered for all four families. They are recorded as round-5 lines in the
findings pile rather than as new incident topics: the reviewer caught each at
zero cost, which is the class the pile already tracks. The position-blind one
also carries a lesson, in the retrospective's 2026-09-05 entry — the review
named four affected positions and an inventory of one file per position found a
fifth.

A sixth round on `48ed1ab6` produced two findings, both reproduced against a
control before being accepted and split by whether Part 1 introduced them. The
return-position diagnostic's enumeration of the positions that *do* work omitted
module-scope annotated assignments, which D-228 lowers — a false claim in a
message this change added, fixed in `c3b036f7` at every site that repeats the
list and now pinned by a test that lowers a container in each of the five named
positions. The second, a module binding shadowing a container builtin, is real
and reproduced but was filed as #932 rather than fixed: it is not a regression
of previously-correct behaviour, its consequence is bounded to accepting a
program CPython rejects at run time, and the fix needs plumbing and a scoping
decision the report does not settle. Both are round-6 lines in the findings
pile — fifteen findings over six rounds — with the refuted one carrying the
scope reasoning; neither opens an incident topic, since the reviewer caught each
at zero cost.

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
4. **#931** was filed from this task: `annotation_to_ty`'s subscript
   fallthrough discards every type argument when the base is not a recognized
   container, so `T[int]`, `int[str]` and `Self[int]` are all accepted silently.
   Pre-existing on `c639e682`, found while inventorying the bare-container
   advice, and deliberately out of scope here.
5. **#932** was also filed from this task: a module-level function, value or
   import binding named after a container builtin does not suppress the
   container dispatch, so `def list(...)` followed by `x: list[int]` still
   lowers as `Ty::List(Int)`. Newly reachable rather than pre-existing — before
   Part 1 every parameterized container annotation was rejected — but scoped
   out because the fix needs a module-binding set threaded through
   `annotation_to_ty`'s ~19 call sites plus a decision on which binding kinds
   and which orderings participate.

## Gates

Run from a single-writer baseline at `c3b036f7`, the head that ships here.
Coverage: `cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions
100` exit 0, TOTAL 52,528 lines / 2,359 functions / 34,381 regions, all 100.00%
with zero uncovered. `cargo fmt --all -- --check`, `cargo clippy --workspace
--all-targets -- -D warnings` and `cargo test --workspace` all exit 0 on the same
commit. Every doc and policy gate — decisions index `--check`, roadmap evidence,
README milestone projection, CI permissions, agent assets and policies,
`check-site.sh`, `check_status_page_freshness.rb c639e682 HEAD`, the conformance
breadth check, the `scripts/` unittest suite including the branch-protection
baseline, and the harden findings pile — returned 0 on that same head, with each
exit status captured directly rather than through a pipeline.

The earlier baseline at `d1d22692` was discarded rather than carried forward.
`git diff --name-only d1d22692..c3b036f7 -- crates src Cargo.toml Cargo.lock`
names three files, so the round-6 fix moved the Rust tree and the old measurement
no longer describes it. Two coverage runs were killed for the same reason before
this one: each had been started before a fix round landed, so each would have
measured a tree that was about to change.

One commit lands after the measurement: this snapshot itself, together with the
retrospective entry and the round-6 findings lines. All three are prose, so
`git diff --name-only c3b036f7..HEAD -- crates src Cargo.toml Cargo.lock` is
empty — executed, not asserted — and no gate that reads the Rust tree is
affected; the doc gates are re-run once more on the final head before the merge.

Both Ruby checkers need `LC_ALL=en_US.UTF-8 RUBYOPT=-EUTF-8` in this shell; that
is a local locale artifact, not a repository defect.
