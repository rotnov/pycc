# 2026-09-05 — #921: diagnose a call to an enum class instead of panicking

## Previous checkpoint's outcome

The iteration before this one delivered
[#937](https://github.com/rotnov/pycc/issues/937) (the PEP 563 row flip):
PR [#939](https://github.com/rotnov/pycc/pull/939) merged by squash as
`a275fbaa` at 2026-09-05T15:24:10Z, issue #937 is CLOSED, and its branch was
deleted. Every post-merge `main` run for `a275fbaa` completed with conclusion
`success`: CI
[33974688875](https://github.com/rotnov/pycc/actions/runs/33974688875), Pages
[33974688874](https://github.com/rotnov/pycc/actions/runs/33974688874), Main
history audit
[33974688847](https://github.com/rotnov/pycc/actions/runs/33974688847), and
Status page freshness
[33974688869](https://github.com/rotnov/pycc/actions/runs/33974688869).
The concurrent PR [#940](https://github.com/rotnov/pycc/pull/940) was a
duplicate of #937's residual documentation drift, left to its author with a
comment; it has since merged on its own as `2c92a74b` (2026-09-05T15:56:25Z),
carrying the README `39 rows/40 PEPs` correction this iteration was asked to
fold in, so that correction is delivered by #940 and not repeated here.
[#935](https://github.com/rotnov/pycc/pull/935) (site evidence) merged as
`cf7cc40f` at 2026-09-05T16:10:45Z. `autopilot/iter-2026-09-05-10` was cut
from `a275fbaa` and rebased onto `cf7cc40f` (then `origin/main`, with no open
pull request left) before this snapshot was written; the rebase was clean.

## Overall status

Implemented [#921](https://github.com/rotnov/pycc/issues/921) on
`autopilot/iter-2026-09-05-10` per the plan published on the issue: a
Rust change across `pycc_hir`, `pycc_types` and `pycc_mir` plus tests and
documentation. One pull request, body carrying `Fixes #921`; the
orchestrating session watches CI and merges.

## What the change is

`Color()` and `Color(1)` on an `Enum`/`StrEnum` class panicked in
`pycc_types::class::resolve_instantiation` (`pycc check`) and would have
panicked identically in `pycc_mir`'s `Instantiate` lowering (`pycc build`):
an enum class early-returns before `ensure_init` (D-225) and has no
`__init__` in its MRO. Both spellings are now one `C0001` at the call
expression:

```
error[C0001]: calling an enum class (`Color(...)`) is not supported yet -- refer to a member by name (`Color.<MEMBER>`) instead
 --> tests/diagnostics/c0001_enum_class_call_value.py:10:9
   |
10 |     c = Color(1)
   |         ^^^^^^^^ calling an enum class (`Color(...)`) is not supported yet -- refer to a member by name (`Color.<MEMBER>`) instead
```

- `HirClassDef.is_enum`: the enum's provenance marker, set only by
  `lower_enum_class` regardless of member count (a docstring-only enum has an
  empty `enum_members`), `false` at every other construction site.
- `crates/pycc_hir/src/class/enum_call.rs`: an AST-level scan
  (`pycc_ast::visitor::Visitor`, `visit_expr` only) run by `lower_module`
  once per top-level item right after that item is lowered, so the collected
  list stays in loop order (D-217 rule 3). The name set is the syntactic
  pre-collection of the module's own enum classes plus every `is_enum` class
  known to the module state (an enum pulled in by a project import), minus
  the currently poisoned names (D-219). Keyword calls (`Color(value=1)`) are
  skipped so the existing keyword `C0001` stays the only diagnostic.
- `resolve_instantiation` keeps a span-less guard on `is_enum` with the same
  message (shared `pycc_hir::enum_class_call_message`); the two "no
  `__init__` in the MRO" panics are reworded to state the now-true
  invariant and keep their `should_panic` tests.
- Fixtures `tests/diagnostics/c0001_enum_class_call_{no_args,value}`;
  ten unit tests in `enum_call.rs`; one guard unit test in `binding.rs`;
  three end-to-end tests in `tests/issue_379_enum.rs` (`check` on four
  spellings, `build` proving the MIR panic is unreachable, and a two-file
  project import of a docstring-only enum).
- `docs/DIAGNOSTICS.md` records the classification (the zero-argument form is
  deliberately `C0001`, not `T0021`, because pycc models no enum constructor
  to report an arity error against); `docs/ROADMAP.md` and
  `docs/TYPE_SYSTEM.md` edit existing prose only; D-225 gains a dated
  follow-up note.

Gates run locally on the rebased tree: `cargo fmt --all -- --check`,
`cargo clippy --workspace --all-targets -- -D warnings`,
`cargo test --workspace`, the CI coverage sequence ending in
`cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`
(100.00% lines, 100.00% regions), `cargo doc --workspace --no-deps`,
`python3 -m unittest discover -s scripts -p 'test_*.py'`,
`RUBYOPT="-E UTF-8" ruby scripts/check_roadmap_evidence.rb`,
`ruby scripts/check_status_page_freshness.rb origin/main` (no signal),
`RUBYOPT="-E UTF-8" bash scripts/check-site.sh`,
`python3 scripts/check_conformance_breadth.py`,
`ruby scripts/check_ci_permissions.rb`, and
`python3 scripts/generate_decisions_index.py --check` (index unchanged).

## Deviations from the plan

- The README status-blurb correction (plan item 8) was not applied here:
  #940 merged first and carries it.
- The branch was rebased from `a275fbaa` onto `cf7cc40f` after #940 and #935
  merged mid-implementation, so the pull request opens against the current
  `main`; no file overlapped.
- Hand-built enum `HirClassDef` literals in existing unit tests
  (`pycc_types`, `pycc_mir`, `pycc_codegen`) got `is_enum: true` rather than
  `false`, since they model enum classes; every non-enum literal got `false`.
- `ruby scripts/check_readme_milestone_projection.rb` and
  `check_roadmap_evidence.rb` need `RUBYOPT="-E UTF-8"` on this machine
  (the same locale artefact the 04 snapshot recorded).
- No `.harden/findings/issue-921.jsonl` was added.

## Known follow-ups

- [#934](https://github.com/rotnov/pycc/issues/934) — `pycc_types::check`
  accepts a protocol-returning function whose HIR `pycc_mir` cannot lower
  (panic at `crates/pycc_mir/src/expr.rs:876`): the next soundness candidate
  of the same "panic instead of diagnostic" class.
- [#889](https://github.com/rotnov/pycc/issues/889) — string-literal and
  attribute-qualified annotations.
- [#882](https://github.com/rotnov/pycc/issues/882) — the remaining
  `pycc_std` typing-surface widening.
- A `from __future__` import inside a function body keeps the generic
  block-import `C0001` (recorded in D-229, not locked in by a test).
- Enum value lookup itself (`Color(1)` compiling, with CPython's `TypeError`
  and `ValueError` outcomes becoming `T0021`), `Color(value=1)` (#884), and
  member equality (#908) stay out of scope.
- Two order-dependent limits of the scan are documented on
  `reject_enum_class_calls`: an enum imported *after* a `def` that calls it
  falls through to the span-less `pycc_types` guard (renders at `1:1`), and
  a call inside a `def` that precedes a failing `class E(Enum): pass` is
  reported rather than suppressed as a cascade.

## Paused autopilot

- Directive scope: open-ended (`/goal fix all opened issues`).
- Active milestone: `v0.4` (Accept unmet).
- Last iteration outcome: #939 merged (`a275fbaa`), #937 closed.
- This iteration: #921 implemented; PR open, awaiting CI and merge by the
  orchestrating session.
- Next step: re-enter `issue-select` for `v0.4` after this PR lands.
- Denylist: empty.

## Where to resume

`crates/pycc_hir/src/class/enum_call.rs` is the whole spanned mechanism and
its doc comment states the two scope limits; `lower_module`'s loop in
`crates/pycc_hir/src/module.rs` is the one call site. The `is_enum` field
lives on `HirClassDef` in `crates/pycc_hir/src/class.rs`, the span-less guard
in `crates/pycc_types/src/class/binding.rs`. If value lookup is ever
implemented, delete the scan and the guard together and move the CPython
arity/`ValueError` outcomes to `T0021` per `docs/DIAGNOSTICS.md`.
