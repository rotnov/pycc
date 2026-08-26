# 2026-08-26-07 -- Issue #809: widen Optional[T] codegen to float/bool

## Status: delivered by the pull request that carries this file

Worktree: `/Users/denis/projects/pycc-worktrees/issue-809-optional-float-bool`,
branch `feat/issue-809-optional-float-bool`, based on
`origin/main` at `aed29cd02b47e3edb33c6e8776a1e68c0f65fc7c`. Implements
[#809](https://github.com/rotnov/pycc/issues/809) (Part 3 of
[#747](https://github.com/rotnov/pycc/issues/747), following D-197/#763
Part 1 and D-201/#769 Part 2): widens `Optional[T]`/`T | None` codegen to
accept `T` in `{int, float, bool}`, not just `int`. `Optional[str]` and
other refcounted/pointer inner types, and general `A | B` unions, stay out
of scope. Full 7-item plan from the issue's pre-reviewed comment; decision
record [D-204](../decisions/D-204-widen-optional-t-codegen-to-float-and-bool-inner.md).

## What changed

- `crates/pycc_hir/src/func.rs`: widened the `T0049` diagnostic gate from
  `T == int` to `T` in `{int, float, bool}`.
- `crates/pycc_codegen/src/lib.rs`: fixed two real bugs found while
  widening codegen.
  1. `declare_module_globals`'s and `default_value_for_type`'s
     `Ty::Optional` arms unconditionally used `tag_smallint_const` (an
     `i64` D-141-encoded constant) for the payload field regardless of
     inner type -- an LLVM constant-type mismatch for `Ty::Float`/`Ty::Bool`
     struct fields (`f64`/`i8`), causing an "invalid number of bytes" LLVM
     crash for any `Optional[float]`/`Optional[bool]` module global. Fixed
     by special-casing `Ty::Int` and recursing into `default_value_for_type`
     for other inner types.
  2. `truthy`'s `Scalar::Optional` arm and the `MirExpr::OptionalUnwrap`
     codegen arm both always assumed a `Ty::Int` payload. Fixed to dispatch
     on the payload's actual LLVM type (`FloatValue` -> float truthiness,
     `i8`-wide `IntValue` -> `Ty::Bool`, other `IntValue` -> the existing
     `Ty::Int` path via `pycc_rt_int_truthy`) / the declared inner `Ty`.
  Also confirmed and left as-is: `storage_slot_at_entry` deliberately
  leaves `Optional[Float]`/`Optional[Bool]` locals' allocas uninitialized
  (their payloads are never refcounted, so this is harmless, unlike
  `Optional[Int]`, which the same function does initialize).
- `crates/pycc_codegen/src/tests.rs`: 13 new unit tests -- 11 for the
  `Optional[float]`/`Optional[bool]` feature itself (assignment,
  narrowing, truthiness, function returns, the exceptional-exit default
  value path, and the `Optional[bool]`/bare-`None`-placeholder LLVM
  struct-type-collision finding below), plus 2 added in a later round to
  close pre-existing coverage gaps unrelated to this feature (see below).
- `crates/pycc_codegen/src/bigint_rc.rs`: re-verified (not changed in
  substance) that its `OptionalUnwrap`-related bigint-refcount guards are
  already correctly `Ty::Int`-scoped -- 2 new pinning tests confirm no
  bigint refcount calls are emitted for a narrowed `Optional[float]`/
  `Optional[bool]` unwrap.
- `tests/fixtures/pep_0604_union.py`: extended with `float | None`/
  `bool | None` coverage (presence, narrowing, reversed operand order,
  truthiness, function returns, function-locals). Verified byte-for-byte
  identical against `python3.14` (3.14.6; the sandbox's pinned-oracle
  version check requires 3.14.7 exactly, a pre-existing environment
  mismatch unrelated to this change -- worked around by manually diffing
  compiled-binary output against the interpreter directly, in both debug
  and release profiles).
- Docs: `docs/DIAGNOSTICS.md`, `docs/TYPE_SYSTEM.md`,
  `docs/PYTHON_STANDARDS.md`, `docs/ROADMAP.md` (two passages, corrected
  again in the D-068 fix round below),
  `tests/fixtures/conformance-breadth-manifest.json` (corrected in the
  D-068 fix round below), and the new
  `docs/decisions/D-204-widen-optional-t-codegen-to-float-and-bool-inner.md`.

## The Ty::Bool struct-collision risk

The issue's plan flagged a risk: `Optional[bool]`'s real runtime
representation is an anonymous LLVM struct `{i8, i8}` (payload + present
flag), and `MirExpr::NoneLiteral`'s own placeholder struct (built before
any target type is known) is *also* `{i8, i8}`. Anonymous LLVM struct
types are uniqued per-`Context` by field-type list, so these are literally
the same `StructType` value. Empirically confirmed harmless: every
`coerce_scalar_to_type` call site discriminates on the requested `Ty`,
never by introspecting the `StructValue`'s LLVM type alone, so the
coincidence cannot cause misdispatch. Pinned by
`optional_bool_none_placeholder_and_real_absent_value_are_the_same_llvm_struct_type`
(asserts the type equality directly) and
`optional_bool_absent_value_truthiness_and_narrowed_unwrap_are_both_correct`
(an end-to-end program proving both directions still behave correctly).
Recorded in D-204.

## A pre-existing 100%-coverage gate gap, found and closed

After the feature work was otherwise complete, `cargo llvm-cov --workspace
--fail-under-lines 100 --fail-under-regions 100` failed with 2 missed
regions / 1 missed line, both in `crates/pycc_codegen/src/lib.rs`, in code
this branch's diff never touched (confirmed via `git blame`: introduced by
commit `2c99da6c6` on 2026-08-18, an unrelated exception-handling merge).
Closed with two new tests rather than a `docs/TESTING.md` exemption, since
both gaps are real, reachable-by-construction branches:

1. `a_non_none_function_whose_try_raise_finally_body_always_terminates_compiles_cleanly`
   exercises the `exception::block_always_terminates(body) == true` branch
   of the non-`None`-return-type fallthrough guard
   (`builder.build_unreachable()`), using the exact `try: raise ...
   finally: cleanup` shape that function's own doc comment names as its
   motivating example.
2. `truthy_of_an_optional_with_a_pointer_payload_panics_as_an_internal_error`
   pins `truthy`'s `Scalar::Optional` arm's defensive `other =>
   panic!(...)` catch-all (the shape `Optional[str]` would need, which
   `T0049` rejects before codegen), via a hand-built `Scalar::Optional`
   carrying a pointer-typed payload -- mirroring the existing
   `truthiness_of_a_list_value_panics_honestly` pattern of calling `truthy`
   directly with a hand-built `Scalar`.

## D-068 review and fix round

Dispatched the pinned `ievo:deep-reviewer` against the full committed
range (merge-base `aed29cd0` through HEAD). No blockers; 3 findings, all
addressed:

1. **Doc drift (fixed):** `tests/fixtures/conformance-breadth-manifest.json`'s
   PEP 604 entry still said `Optional[T]` was proven/not-proven on an
   `int`-only basis, unwidened by this PR. Added `proven` entries for
   `Optional[float]`/`Optional[bool]` (presence, truthiness, function
   returns, and narrowing), and narrowed the `not_proven` entry from "any
   `T` other than `int`" to "any `T` outside `{int, float, bool}`". No
   status-marker flip (the row stays `◐`; a `core` gap remains under
   D-177) and no `scripts/check_conformance_breadth.py` count change --
   re-ran the checker after editing to confirm.
2. **Doc drift (fixed):** six comments/doc passages wrote "#809 (Part 2 of
   #747)" where #769/D-201 already owns Part 2 and #809/D-204 is Part 3.
   Fixed all six in place (`crates/pycc_codegen/src/lib.rs:1197,1745`,
   `crates/pycc_codegen/src/bigint_rc.rs:1468`,
   `crates/pycc_hir/src/tests.rs:891,902`, plus one instance already
   correctly attributed to #769 was left untouched). `cargo check
   --workspace` confirmed clean after the comment-only edits.
3. **Process (this file):** this session file is that missing
   `docs/sessions/` handoff entry.

## Gate results

- `cargo doc --workspace --no-deps`: clean (pre-existing unrelated
  warnings only: `pycc_scratch` and a `pycc_types` private-intra-doc-link
  warning, both present before this branch).
- `cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions
  100`: **EXIT 0**, TOTAL 47072/47072 regions, 30409/30409 lines, both
  100.00%.
- `cargo clippy --workspace --all-targets -- -D warnings`: **EXIT 0**
  (pre-existing, unrelated `slice1_codegen_depth.rs` rustc lint warnings
  only, not touched by this branch).
- `cargo test --workspace`: **EXIT 0**, all suites green (`pycc_codegen`'s
  own lib suite: 381 passed, 0 failed).
- `ruby scripts/check_roadmap_evidence.rb` and
  `ruby scripts/test_check_roadmap_evidence.rb`: both pass (needed
  `LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8` in this sandbox -- a pre-existing
  environment quirk, not caused by this branch; no new `[x]` roadmap
  checkboxes were added by this task, only corrective prose, so no new
  `roadmap-evidence` identifier was needed).
- `python3 scripts/generate_decisions_index.py docs/decisions
  docs/decisions/README.md --check`: up to date.
- D-068 pinned `ievo:deep-reviewer`: one round, 3 findings (all doc-drift
  or process, no code defects), all addressed above.

## Where to resume

Nothing outstanding from this task beyond opening the pull request,
watching CI, and merging -- which happens outside this worktree/session
per this project's dispatch model. No further findings pending.
