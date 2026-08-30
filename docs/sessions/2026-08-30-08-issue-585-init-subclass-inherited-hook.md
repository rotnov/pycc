# Session handoff: issue #585 — reject an inherited side-effecting `__init_subclass__`

- Date: 2026-08-30
- Issue: [#585](https://github.com/rotnov/pycc/issues/585) (P2, PEP 487 recognition-only gap, milestone v0.4)
- Follow-up issue filed this session: [#854](https://github.com/rotnov/pycc/issues/854) (v0.4)
- Branch: `claude/issue-585-init-subclass-inherited-hook`
- Base commit: `499f3a35` (origin/main at task start, confirmed up to date)

## Status

Delivered on branch `claude/issue-585-init-subclass-inherited-hook`
(commits `297c2e45`, `646e1596`); PR not yet opened as of this writing.
Issue #585 is **narrowed, not closed**: completion criteria 1-2 are checked
off with an accurate note that criterion 2 is only *partially* satisfied
(see below); criteria 3-4 remain open, gated on a future
full-invocation-semantics issue.

## What was built

`crates/pycc_hir/src/class.rs`'s `__init_subclass__` static-evaluability
guard (from #435 Part B) only fired when the *derived* class also defined
`__init_subclass__`. A base class with a side-effecting `__init_subclass__`
was accepted unconditionally, and a subclass that inherited that hook
without overriding it was never checked against it — CPython invokes the
inherited hook at every subclass's creation; pycc silently ran nothing and
rejected nothing.

The fix: `lower_class` now re-validates an inherited, non-overridden
`__init_subclass__` against **the subclass's own creation site** (not the
base's definition site) when a base in the MRO defines one this crate can
still introspect. A base class that is never subclassed stays legal
regardless of its own hook's body shape — CPython never invokes it until a
subclass exists.

Implementation: rather than widen `HirClassDef` (155 construction sites
across 3 crates) with a new boolean field only needed within one lowering
pass, `lower_class` gained a new `base_class_asts: &[(String,
&pycc_ast::StmtClassDef)]` parameter threaded from `pycc_hir::lower_checked`
(2 real call sites: `lib.rs`'s per-statement loop, and `class/mro.rs`'s one
unit test). It pairs every already-lowered *user-authored* class with its
original AST so the inherited-hook check can re-walk the base's real method
body; a synthetic builtin-exception base (no AST counterpart) is naturally
absent and left unrestricted.

Full rationale, the rejected `HirClassDef`-field alternative, and the
`.find()`-vs-`.any()` MRO-walk alternative are recorded in
[D-213](../decisions/D-213-defer-pep-487-full-invocation-reject-the.md).

## What was deliberately NOT fixed in this PR (tracked as #854)

The D-068 pinned local reviewer (`ievo:deep-reviewer`) found a blocker in
the first draft of this change: the pre-existing (pre-#585) `if` branch,
which fires when the *current class itself* also defines `__init_subclass__`,
validates the **current class's own body** rather than the ancestor's. Per
CPython's real invocation mechanism —
`super(new_cls, new_cls).__init_subclass__(**kwargs)` in
`type_new_init_subclass` looks up the hook starting immediately *after*
`new_cls` in its own MRO — a class's own `__init_subclass__` definition is
**never** the one CPython invokes at that class's own creation, regardless
of whether the class also overrides the hook. So this program is currently
still accepted (should be rejected):

```python
class B:
    def __init_subclass__(cls):
        print("side effect")

class D(B):
    def __init_subclass__(cls):
        pass  # D's own override -- CPython never invokes this at D's own creation
```

I independently re-derived the CPython mechanism to confirm the reviewer's
premise was correct, then consulted this session's advisor tool on how to
scope the fix. The advisor's key finding: unifying the two branches (making
the ancestor-hook check run unconditionally, dropping the own-body check)
would flip the expected outcome of several of #435's pre-existing tests in
*both* directions (a trivial override + side-effecting base becomes
rejected; a side-effecting override + trivial base becomes accepted) — a
real, intentional behavior change to #435's shipped contract, not a
mechanical cleanup, and it also surfaces a related gap (a
`@classmethod`-decorated `__init_subclass__` is invisible to the guard's
own-methods check entirely). Both were filed as
[#854](https://github.com/rotnov/pycc/issues/854) instead of being folded
into this PR, and this PR's own scope was corrected to match exactly what
it delivers: `docs/ROADMAP.md`, `docs/TYPE_SYSTEM.md`, and
[D-213](../decisions/D-213-defer-pep-487-full-invocation-reject-the.md)'s
Consequences section were all updated to describe the override case as a
known, separately-tracked remaining gap rather than implying full coverage.
Issue #585's own completion-criterion-2 checkbox note was corrected to say
"partially done" with a pointer to #854.

## Files changed

- `crates/pycc_hir/src/class.rs` — the fix, plus 2 new tests
  (`subclass_without_own_init_subclass_rejects_side_effecting_inherited_body`,
  `base_alone_never_subclassed_with_side_effecting_init_subclass_stays_legal`).
- `crates/pycc_hir/src/lib.rs` — threads `base_class_asts` through
  `lower_checked`.
- `crates/pycc_hir/src/class/mro.rs` — updated the one existing unit-test
  call site for `lower_class`'s new fifth parameter.
- `docs/decisions/D-213-defer-pep-487-full-invocation-reject-the.md` — new
  ADR, with a Consequences entry describing the #854 gap explicitly.
- `docs/decisions/README.md` — regenerated index.
- `docs/ROADMAP.md`, `docs/TYPE_SYSTEM.md` — updated the `__init_subclass__`
  descriptions to reflect the new rejection behavior, cross-reference
  D-213, and describe the still-open #854 gap accurately instead of
  overclaiming full coverage of "everywhere CPython would actually invoke
  the hook."

## Gate results

- `cargo test --workspace`: pass (0 failures, full suite).
- `cargo clippy --workspace --all-targets -- -D warnings`: pass (0 errors;
  pre-existing unrelated rustc string-escape warnings in
  `tests/slice1_codegen_depth.rs` and an unrelated `rustdoc` warning in
  `pycc_types::env` did not fail the build).
- `cargo doc --workspace --no-deps`: pass.
- `cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`:
  pass, 100.00% lines / 100.00% regions across the whole workspace. Getting
  to 100% required a `find_map` restructuring in `class.rs` (an earlier
  revision left one, then a different one, structurally-hard-to-hit line),
  resolved by simplifying the lookup and adjusting one test fixture's
  method order rather than adding a special-case test for a dead branch.
- D-068 pinned local reviewer (`ievo:deep-reviewer`), first pass: 4
  findings (1 blocker, 1 warning, 2 notes). The blocker (own-override case
  validates the wrong artifact) was resolved by narrowing this PR's
  documented scope and filing #854, per the reasoning above, rather than by
  changing the code — expanding the code fix itself would have been a
  second, differently-shaped behavior change requiring its own test-fixture
  pass, which #854 now owns. The warning (decorator-sensitivity, i.e. the
  same `@classmethod` gap) is captured in #854 too. The two notes (a
  functional test duplicate with its own traceability justification; a
  coverage-gap concern already resolved by the 100%-coverage gate result
  above) required no code changes.

## What a fresh session should know if resuming this area

- The reject-only fix in this PR is deliberately narrow: it closes only the
  case where the subclass has no override of its own. Do not describe #585
  or this PR as closing "the" `__init_subclass__` soundness gap — there are
  two mirror-image gaps, and this PR closes exactly one of them. #854 tracks
  the other.
- Do not attempt full PEP 487 invocation semantics under #585 — that is out
  of scope per D-213 and belongs to a *new* issue once pycc's
  class-creation model can run real side effects at that point (or a
  different resolution to the fixture-inertness problem in #580 is found).
- `base_class_asts` is `pub(crate)`-internal to `pycc_hir::class`'s
  `lower_class`; no other crate calls `lower_class` directly, so a future
  change to its signature (e.g. #854's fix) only needs to update the two
  call sites listed above.
- #854's fix will need to touch several of #435's pre-existing
  `__init_subclass__` tests, since unifying the two validation branches
  flips their expected outcome (see #854's body for the exact repro and
  which tests are affected). This is expected and intentional there — do
  not treat those test changes as regressions when #854 is picked up.
