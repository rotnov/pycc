# 2026-09-04-03 -- Issue #911: `ClassVar` registration and annotated scalar class attributes

## Status: pull request open, not merged, CI not yet observed

Base: `origin/main` at `ef991f70` -- re-fetched immediately before this file was
committed and unchanged since the branch was cut.
Branch: `autopilot/iter-2026-09-04-03`. Head is the reviewer-fix commit that
carries this revision of this file; `9811c6c4` is its parent.
Pull request: [#917](https://github.com/rotnov/pycc/pull/917), open, `MERGEABLE`,
not a draft, one closing reference (`Fixes #911`), verified through
`gh pr view 917 --json closingIssuesReferences`.

This session did **not** merge and did **not** wait on CI: the orchestrating
session owns the CI watch, the diff review, and the merge.

## What this change delivers

Issue #911 is Part 1 of #885 (milestone v0.4), implemented against the plan
published as [comment 5539081675](https://github.com/rotnov/pycc/issues/885#issuecomment-5539081675).

- `typing.ClassVar` is registered in `crates/pycc_std/src/lib.rs` as a third
  `StdSymbolKind::AnnotationMarker` (after `Final`/`Annotated`), so
  `from typing import ClassVar` no longer fails with `C0002`. No peer-owned
  import-series file was touched -- the symbol resolves generically through
  `pycc_std::resolve_module`/`resolve_symbol`.
- `ClassVar` is position-restricted to a class-body attribute declaration.
  `pycc_hir::class::body::strip_class_var` removes the wrapper there;
  `func::annotation_to_ty` rejects both the bare and the subscripted spelling
  in every other position rather than silently accepting a meaningless one.
- `HirClassDef` gains `class_attrs: Vec<(String, Ty, ClassAttrValue)>`, with
  `ClassAttrValue` covering `Int`/`Float`/`Bool`/`Str`. An annotated class-body
  attribute with a literal initializer becomes a compile-time constant that
  `pycc_mir` folds at every read -- through the class name and through an
  instance -- so it occupies no instance slot and shifts no other attribute's
  slot index (D-154). `pycc_codegen` and `pycc_rt` are untouched.
- A class attribute colliding with an instance slot or an `@property` -- the
  class's own, or any inherited through its MRO, in either declaration order --
  is rejected with `C0001` after the whole class-body walk (at the `AnnAssign`
  site `attrs` is still empty). The mirror-image shape, a subclass `__init__`
  declaring an instance slot whose name an *ancestor* holds as a class
  attribute, is rejected one layer later by `pycc_types`'
  `check_attr_set`/`lookup_class_attr_through_mro` with `T0044`, which points at
  the offending assignment; both directions carry a test.
- Per AGENTS.md's decomposability rule (D-185), the class-body walk was extracted
  first, in its own commit, from `crates/pycc_hir/src/class.rs` (5,212 lines) into
  `crates/pycc_hir/src/class/body.rs` (935 lines). `class.rs` lands at 4,401 lines,
  so #548 stays open.

## The #585 invariant

The plan resolves the `__set_name__` interaction by *rejection*: a class-level
attribute is restricted to the scalar slot types (`int`/`float`/`bool`/`str`),
and `Ty::Param(_)` is rejected too, so a descriptor-valued class attribute
remains a `C0001` and `__set_name__`'s precondition never arises. #885 removed
the blanket ban that #585 cites as the cause, so without this guard pycc would
have gained a new silent CPython divergence.

That is recorded as **D-224**
(`docs/decisions/D-224-restrict-class-level-attributes-to-scalar.md`), written
into `pycc_hir`'s source as a named comment, and `docs/TYPE_SYSTEM.md`'s
`__set_name__` clause was rewritten -- it previously attributed the
untriggerability to the wrong cause. The implementer comment the plan calls for
was posted on #585
([comment 5539945470](https://github.com/rotnov/pycc/issues/585#issuecomment-5539945470)).
The same constraint binds Part 2 (#910), which must accept a literal right-hand
side only.

## Commits (merge base `ef991f70` .. `HEAD`)

- `7cf046d8` Extract the class-body walk into `class/body.rs` (D-185)
- `99873243` Add the `HirClassDef::class_attrs` field and `ClassAttrValue` (#911)
- `8643e819` Fold annotated scalar class attributes at compile time (#911)
- `9811c6c4` Record the #911 checkpoint and the `llvm-cov` instantiation lesson
- `HEAD` Address the pinned reviewer's two findings (#911)

The branch was force-pushed once after the PR was opened, to rewrite `7cf046d8`'s
message: the phrase "does **not** close #548" contains a GitHub closing keyword
immediately before an issue reference, which had added #548 to the PR's closing
references. Both the body and the commit message were reworded; the PR now
reports exactly one closing reference.

## Gates

All green, re-run after the reviewer fixes; every exit status was
captured directly rather than through a pipe. Full numbers are in the PR's own
[gate-results comment](https://github.com/rotnov/pycc/pull/917#issuecomment-5539948852).
Headline: `cargo test --workspace` 4,412 passed across 76 suites, and
`cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`
exited 0 at 51,824 regions / 2,293 functions / 33,840 lines, zero missed,
100.00% on all three.

`tests/conformance.rs`'s oracle tests are unconditionally `#[ignore]`d locally;
CI runs them with `--include-ignored` against the pinned CPython 3.14.7, so the
local skips are not evidence they pass.

## The coverage gate, and the hours it cost

The gate was red for five full runs at 99.94% lines / 99.85% regions while every
merged `llvm-cov` view reported exactly one uncovered region. This is
`llvm-cov`'s per-instantiation summary accounting, already documented at
`docs/TESTING.md:1082`: each crate here is compiled twice (its own `--cfg test`
unit-test binary, and plainly as a dependency of the `pycc` binary that the
`tests/` suite drives as a subprocess), and the file summary takes the maximum
covered-region count per instantiation group rather than a union -- so new code
reached only by end-to-end tests never satisfies the gate.

The fix was crate-local unit tests at each owning seam --
`pycc_hir::tests` (via `lower_checked`), `pycc_types::tests` (via
`check_and_resolve`), and `pycc_mir::tests::class_attr` (via `build`) --
mirroring the end-to-end cases. No threshold was lowered and no
`--ignore-filename-regex` entry was added. A retrospective entry was written
because the diagnosis was already in the repository and was not consulted.

## Pinned local reviewer (D-068)

The iEvo `deep-reviewer` reviewed the full committed range from the merge base
through `HEAD` in a fresh context, with all new files staged. Verdict: ready to
commit, no P0/P1, two actionable findings, both fixed in this branch's head commit:

1. `reject_class_attr_collisions`'s doc comment claimed both directions were
   checked; only one is. The comment now states the checked direction exactly
   and names `check_attr_set` as the guard for the reverse one, the companion
   comment in `pycc_types::class` was corrected the same way (it had credited
   HIR for a rejection it does not perform), and
   `a_subclass_init_writing_an_inherited_class_attribute_name_is_rejected` pins
   the shape so a future narrowing of that MRO walk cannot silently reintroduce
   a miscompile. Keeping `T0044` here rather than unifying to `C0001` is
   deliberate: there is a single offending statement with a specific reason, so
   the write-site diagnostic is the better one.
2. A stray run of internal spaces inside the `ClassVar` diagnostic string in
   `func.rs` (a literal that had been hand-wrapped without a continuation).
   Reformatted to the sibling arm's backslash-continuation style; no fixture
   pins the full text.

## Known follow-ups

Filed in milestone v0.4, all linked to #885, each with a test in
`tests/issue_911_class_attrs.rs` pinning today's diagnostic:

- **#912** a class with no `__init__` -- this is why #885's own headline snippet
  (`class Config:\n    MAX: ClassVar[int] = 100`) still does not compile;
- **#913** `ClassVar` inside a `@dataclass` body;
- **#914** a class attribute satisfying a `Protocol` attribute member;
- **#915** `super().CLASS_CONST`;
- **#916** `Final[...]` on a class-body attribute declaration;
- **#910** Part 2 of #885, un-annotated class-body assignments.

**#910 has a fixture dependency worth knowing before starting it.** Three test
fixtures assert that an *un-annotated* class-body assignment is rejected with
`C0001`, and this change converted them from annotated to un-annotated form to
keep that intent alive: `tests/diagnostics/c0001_issue_864_repro.py`,
`tests/diagnostics/c0001_hir_cascade_suppressed.py`, and seven unit tests in
`crates/pycc_hir/src/module/tests.rs`. When #910 accepts `X = 1`, all of them
stop reporting the diagnostic they assert and will need new fixtures.

## Where a fresh session should look to resume

1. `gh pr view 917 --repo rotnov/pycc` for the current state, then the CI checks
   for its current head. The PR was opened without waiting on CI.
2. The plan comment on #885 (comment 5539081675) is the authoritative
   specification for this work item and for #910.
3. `docs/decisions/D-224-...` for the scalar-restriction invariant, and
   `docs/TYPE_SYSTEM.md`'s class-attribute section for the user-facing contract
   and its recorded Part 1 limitations.
