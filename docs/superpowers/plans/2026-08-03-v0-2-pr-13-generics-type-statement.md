# v0.2 PR-13: generic functions, `type` statement, legacy `TypeAlias` — implementation plan

Status: drafted, not yet implemented. Written against commit `a2527a6` (origin/main tip
at task start, 2026-08-03). Governing ADRs: D-133 (`Ty::Param` representation), D-134
(one-type-parameter/scalar-only thin slice, `T0042`), D-135 (alias mechanism, no `Ty`
representation) — see `docs/DECISIONS.md`. Design source: `docs/superpowers/specs/2026-07-28-v0-2-collections-generics-design.md`
§2/§4, `docs/DELIVERY_PLAN.md` row 13.

**Before starting implementation:** re-run the D-021 preflight (fetch origin/main,
re-verify no new D-number/T-code collided, re-run `cargo doc --workspace --no-deps`).
This repo has a documented history of concurrent agent sessions advancing both
counters between planning and execution.

## Scope recap

- Generic functions: exactly one type parameter, callable at any number of call
  sites, each independently monomorphized to one of `Ty::Int`/`Ty::Float`/`Ty::Bool`/`Ty::Str`.
  Multiple type parameters, a type parameter in a container position, and an
  inconsistent/unresolvable call site are rejected pre-codegen with `T0042`.
- `type X = <expr>` and legacy `X: TypeAlias = <expr>`: compile-time-only name-to-`Ty`
  binding, zero HIR/MIR/codegen footprint. A generic alias (`type Alias[T] = ...`)
  is rejected with `T0042` (shares the code with the generic-function rejections
  per D-134/D-135 — both are "generic shape beyond this PR's scope").
- Two conformance fixtures: `tests/fixtures/py312/pep_0695_generics.py` (PEP 695) and
  `tests/fixtures/py310/pep_0613_typealias.py` (PEP 613), wired into `tests/conformance.rs`
  per D-102's existing extension pattern (no new harness).

## Task 1 — `Ty::Param` representation (D-133)

File: `crates/pycc_hir/src/lib.rs`.

- Add a new variant to `pub enum Ty` immediately after `Infer`:
  ```rust
  /// A generic function's own type parameter (PEP 695, D-133), e.g. the `T`
  /// in `def f[T](x: T) -> T`. Distinct from `Ty::Infer`: this is resolved by
  /// call-site substitution (D-134), not by unification, and must never
  /// reach `pycc_mir` unsubstituted (same invariant `Ty::Infer` already
  /// holds, see the assertion near this enum's other pre-MIR checks).
  /// Boxed as `Box<str>` (a single pointer, matching `Tuple`'s `Box<Vec<Ty>>`
  /// shape) to keep `size_of::<Ty>()` at the D-109 16-byte ceiling.
  Param(Box<str>),
  ```
- Add a `#[test]` in the same file's test module asserting
  `std::mem::size_of::<Ty>() <= 16` (extend the existing D-109 size-regression
  test if one already exists under this name; if not, add
  `ty_size_stays_within_d109_ceiling`).
- `lower_function` (same file, currently rejecting `def.type_params.is_some()`
  unconditionally): change to accept **exactly one** type parameter and lower
  it to `Ty::Param(name)` everywhere it appears in the signature; two or more
  type parameters still hit `unsupported` (reuse the existing `Err(unsupported(...))`
  path with an updated message, since this is still "not implemented," not a
  `T0042` case — `T0042` is for the type-checked, resolvable-generic-shape
  rejections in `pycc_types`, not this frontend arity gate). Add unit tests:
  one type parameter accepted (asserts the lowered `HirItem::Function`'s
  param/return `Ty`s contain `Ty::Param("T".into())`), two type parameters
  still rejected with the existing message text.

## Task 2 — symbolic body check + call-site substitution (D-133/D-134)

File: `crates/pycc_types/src/lib.rs`.

- Add `fn check_generic_function(...)`: type-checks a generic function's body
  once, treating every occurrence of `Ty::Param(name)` as an opaque, self-
  consistent placeholder (no numeric/string-specific operations allowed on a
  bare `Ty::Param` value unless already generic-safe, e.g. `return x` where
  `x: T`). Reuses `collect_expr_constraints`/`infer_expr_in`'s existing
  traversal, adding `Ty::Param` as a value that unifies only with itself or
  another occurrence of the same name.
- Add `fn instantiate_generic_call(func: &HirItem, call: &ExprCall, arg_tys: &[Ty]) -> Result<Ty, Diagnostic>`:
  for a call site, resolve each `Ty::Param(name)` occurrence against the
  corresponding argument's concrete `Ty`; reject (new `T0042`) if (a) two
  occurrences of the same name disagree, (b) the resolved concrete `Ty` is
  not one of `Int`/`Float`/`Bool`/`Str`. On success, produce a substituted
  copy of the function body (a recursive `Ty`-tree rewrite plus a body-AST
  clone with only `Ty` annotations replaced — the underlying `HirExpr`/`HirStmt`
  nodes are untouched, only their associated `Ty`s change) tagged with a
  mangled name (e.g. `f__T_int`) for `pycc_mir` to treat as an ordinary,
  independent function per call site, mirroring D-105's monomorphization
  naming convention (verify exact existing mangling helper name/location for
  `list[int]` before reusing it — check `crates/pycc_mir/src` for the
  function that names `list[int]`'s specialized runtime entry points and
  follow the same convention here).
- Reject multiple type parameters and container-position type parameters at
  this layer too (defense in depth alongside Task 1's frontend arity gate),
  each with its own `T0042` snapshot test.
- Diagnostic: add `T0042` to `docs/DIAGNOSTICS.md`'s registry (message text,
  one example per rejection case: two type parameters, container-position
  type parameter, inconsistent call-site substitution, non-scalar call-site
  argument, generic `type` alias declaring type parameters).

## Task 3 — monomorphized call lowering into `pycc_mir`

File: `crates/pycc_mir/src/lib.rs` (verify exact existing entry point for
`list[int]`'s per-instantiation lowering, e.g. wherever D-105's monomorphized
function bodies are emitted, and follow the same per-call-site specialization
list/dispatch structure). Each concrete instantiation reaching this stage is,
by Task 2, already an ordinary concrete-`Ty` function — no new `MirExpr`
variant or codegen arm is needed for genericity itself, only registration of
one specialized MIR function per distinct (generic function, concrete type)
pair actually called, deduplicated so two call sites with the same concrete
type share one compiled specialization.

## Task 4 — `type` statement and legacy `TypeAlias` (D-135)

Files: `crates/pycc_hir/src/lib.rs` (lowering), `crates/pycc_types/src/lib.rs`
(alias table + annotation resolution).

- `lower_stmt`: add a `Stmt::TypeAlias` arm that evaluates the RHS as a type
  expression (reuse the existing annotation-to-`Ty` resolver used for
  parameter/return annotations) and records `(alias_name, Ty)` into a new
  module-level `HirModule` field, e.g. `type_aliases: Vec<(String, Ty)>`
  (populated in source order; emits zero `HirItem`s of its own — the
  `Stmt::TypeAlias` node is consumed, not lowered to a statement). Reject
  (existing `unsupported`, not `T0042`, since this is unconditionally
  unimplemented, matching Task 1's arity-gate precedent) any `type` statement
  whose own type-parameter list is non-empty — that is a generic alias,
  D-134/D-135's explicit out-of-scope case — with `T0042` instead of
  `unsupported`, since this shape *is* recognized and type-checked far enough
  to name precisely why it's rejected (unlike, e.g., `async def`, which is
  simply unrecognized syntax).
- Detect legacy `TypeAlias` form: an `AnnAssign` whose annotation expression
  is exactly the bare name `TypeAlias` (verify at implementation time whether
  this needs an import-aware check — real Python requires `from typing import
  TypeAlias`; decide whether to require the import be present, matching this
  compiler's existing import-handling scope, or accept the bare name
  unconditionally since PR-14 owns real import resolution — document whichever
  is chosen as a plan-deviation note, since the design doc's "same underlying
  alias mechanism" framing doesn't resolve this by itself).
- Annotation resolution (wherever parameter/return/variable annotations are
  currently turned into a `Ty`): before falling through to `C0001`, check the
  alias table for an exact name match and substitute its recorded `Ty`.
- Unit tests: `type X = int` resolves `X` to `Ty::Int` in a later annotation;
  legacy `X: TypeAlias = int` does the same; a generic `type X[T] = list[T]`
  is rejected with `T0042`; an unresolvable alias RHS still falls through to
  its existing diagnostic.

## Task 5 — conformance fixtures

- `tests/fixtures/py312/pep_0695_generics.py`: a generic function with one
  type parameter called at ≥2 call sites with two different scalar types
  (e.g. `int` and `str`), each printed, matching CPython 3.14.6 byte-for-byte
  in both `--debug` and `--release` (D-102's existing dual-profile
  convention).
- `tests/fixtures/py310/pep_0613_typealias.py`: a legacy `TypeAlias` used as a
  parameter annotation, exercised through a real call and printed result.
- Wire both into `tests/conformance.rs` per the existing per-fixture pattern
  (see PR-9/PR-10's own additions for the exact macro/registration shape to
  copy).
- Flip `docs/PYTHON_STANDARDS.md`'s PEP 695 and PEP 613 rows to `✅` with the
  PR/commit citation once fixtures pass in CI, following PR-10's own row-flip
  prose style exactly (see `docs/DELIVERY_PLAN.md` row 10's own description of
  how that row was updated).

## Task 6 — documentation sweep (same PR, same commit as the code)

- `docs/TYPE_SYSTEM.md` Generics section: replace the current aspirational
  "PEP 695 syntax ... and legacy `TypeVar` both supported" sentence with a
  precise v0.2-thin-slice description matching D-134/D-135 (one type
  parameter, scalar-only instantiation, alias is compile-time-only), in the
  same prose style as the existing `list[int]`-through-PR-11b paragraph
  above it (append, don't replace that paragraph — it's still accurate for
  containers).
- `docs/PYTHON_STANDARDS.md`: flip PEP 695/613 rows, update the conformance
  count (currently should read against whatever PR-12 left it at — re-verify
  the exact current count at implementation time rather than assuming a
  number here).
- `docs/DELIVERY_PLAN.md` row 13: replace with "Merged to `main`" prose
  mirroring rows 10/11/12's exact style (PR link, squash-merge commit, date,
  what shipped, what's explicitly deferred to `docs/ROADMAP.md`).
- `docs/ROADMAP.md`: v0.2 acceptance evidence section, add PR-13's fixtures
  and conformance-count contribution; record multiple type parameters,
  container-position type parameters, generic aliases, and PEP 696 defaults
  as explicit follow-ups (not silently dropped).
- `docs/DIAGNOSTICS.md`: `T0042` registry entry (already noted in Task 2).
- No `docs/SPEC.md` change expected (no doc added/renamed/repurposed) —
  confirm this holds once Tasks 1-5 are actually done.

## Task 7 — review and merge

- Run `cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`
  locally before requesting review; add tests for every branch Tasks 1-4
  introduce until it's clean (D-014, no exemptions expected — this is new
  frontend/type-checker logic, not a candidate for the `docs/TESTING.md`
  exemption table).
- Run the pinned `ievo:deep-reviewer` (D-068) against the staged diff
  (remember to `git add` untracked new fixture files before requesting
  review — the known iEvo 0.70.1 `--working` omission, ievo-ai/skills#483).
- Open the PR against `main`; require the current CI check, resolved
  conversations, up-to-date branch (D-024); no separate approving review
  required (solo-maintainer branch protection).

## Explicit non-goals (confirm still true at merge, not just at planning time)

- Generic classes (v0.3's class model doesn't exist, D-088).
- Multiple type parameters, container-position type parameters, PEP 696
  defaults, `TypeVar` bounds/constraints beyond bare `T` — all deferred to
  `docs/ROADMAP.md`.
- Generic type aliases (`type Alias[T] = ...`) — rejected with `T0042`, not
  silently ignored.
- PR-14's stdlib/import work — out of scope for this PR entirely.
