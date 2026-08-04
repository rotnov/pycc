# v0.2 PR-14 (final v0.2 PR): `pycc_std`, stdlib-intrinsic imports, PEP-594 fixture, hand-authored corpus — implementation plan

Status: drafted, not yet implemented. Written against commit `8cac479` (this
branch, `feat/v0-2-pr14-stdlib-imports`, based on `origin/main` tip `1016d03`
at task start, 2026-08-04). Governing ADRs: D-136 (`pycc_std` shape), D-137
(import binding mechanism), D-138 (PEP-594 fixture), D-139 (container/generics
corpus) — see `docs/DECISIONS.md`. Governing scope ADR: D-088. Design
precedent: `print`/`len` hand-recognized intrinsics in
`pycc_types::infer_expr_in`/`collect_expr_constraints`
(`crates/pycc_types/src/lib.rs` around line 495).

**Before starting implementation:** re-run the D-021 preflight (fetch
`origin/main`, re-verify D-140/T0043 are still unclaimed, re-run
`cargo doc --workspace --no-deps`).

## Scope recap (D-088, D-136–D-139)

- New workspace member crate `pycc_std`: a plain data crate, no proc-macro, no
  build script. Exposes a static registry of stdlib symbols this compiler
  hand-recognizes: `math.sqrt`, `math.pi`, `math.floor`, `math.ceil`,
  `sys.argv` (read-only, `list[str]`-shaped — verify at implementation time
  whether `list[str]` construction from process args is actually reachable in
  `pycc_rt`'s current runtime surface; if not, scope `sys` down to
  `sys.exit(code: int) -> None` only, which needs no new runtime object, and
  record that narrowing as a plan-deviation note in the PR description and
  `docs/DECISIONS.md` D-136 addendum, not silently).
- `import math`, `import sys`, `from math import sqrt, pi, ...`, `from sys
  import ...` resolve as compile-time intrinsic bindings against that
  registry — module-qualified name access (`math.sqrt(x)`) for the plain
  `import` form, direct name binding for the `from ... import ...` form.
- Every other import shape (`import os`, `import cgi`, `from typing import
  X`, relative imports, `import math as m`) is a clean, generic `C0001`
  rejection — not a partial implementation, not a panic, not a silent no-op.
- PEP-594 fixture: `import cgi` (a real, removed-in-3.13 stdlib module, absent
  from `pycc_std`'s registry) asserts the same `C0001` rejection as any other
  unrecognized import, contrasted with a passing `math`/`sys` fixture that
  compiles and runs.
- Hand-authored container/generics differential corpus per D-139: one
  multi-feature fixture combining list/dict/set/tuple literals,
  comprehensions, slicing, container methods, and a generic function call,
  oracle-diffed against CPython 3.14.6 like every existing conformance
  fixture (D-078's harness, D-102's dual-profile extension) — not a new
  harness mechanism.
- Close every still-open `docs/ROADMAP.md` v0.2 acceptance bullet once the
  above lands (D-088's corrected ≥15-row conformance count, PEP 594 row,
  import-scope row).

## Task 1 — `pycc_std` crate skeleton

Files: `Cargo.toml` (workspace `members`), new `crates/pycc_std/Cargo.toml`,
`crates/pycc_std/src/lib.rs`.

- Add `"crates/pycc_std"` to the workspace `members` array in the root
  `Cargo.toml`, alongside the existing 8 members, in the same dependency-order
  position `pycc_hir` occupies relative to `pycc_ast` (before any crate that
  will depend on it).
- `crates/pycc_std/Cargo.toml`: no external dependencies beyond
  `workspace.package` inheritance (edition/rust-version) — this crate is pure
  data plus lookup functions, matching D-136's "no proc-macro, no build
  script, no dependency on any other workspace crate" design.
- `crates/pycc_std/src/lib.rs`: define
  ```rust
  pub enum StdModule { Math, Sys }
  pub enum StdSymbolKind { /* covers the concrete return-shape cases the
      registered symbols actually need, e.g. Function { arg_tys: &'static
      [ScalarKind], ret_ty: ScalarKind }, Constant { ty: ScalarKind } */ }
  pub struct StdSymbol { pub module: StdModule, pub name: &'static str, pub kind: StdSymbolKind }
  pub fn resolve_module(name: &str) -> Option<StdModule>;
  pub fn resolve_symbol(module: StdModule, name: &str) -> Option<StdSymbol>;
  ```
  Use a plain `const`/`static` slice of `StdSymbol` values and linear search
  inside `resolve_symbol` (the registry is under two dozen entries — no
  `HashMap`, no `lazy_static`/`OnceCell`, no macro-generated dispatch table;
  D-136's coverage-gate rationale: every arm here must be exercised by a
  direct unit test, and a linear scan over a small `const` slice keeps every
  branch triviailly testable without speculative generality). Do **not**
  define a `ScalarKind`/type representation duplicate here — reuse
  `pycc_hir::Ty` directly if `pycc_std` is allowed to depend on `pycc_hir`
  (check D-136's exact dependency-direction ruling at implementation time; if
  `pycc_std` must stay dependency-free per its "plain data crate" framing,
  define a minimal local scalar enum here and convert to `pycc_hir::Ty` at
  the `pycc_types` call site instead).
- Unit tests in the same file: every registered symbol resolves via
  `resolve_module`+`resolve_symbol`; an unregistered module name (`"os"`)
  returns `None` from `resolve_module`; an unregistered symbol name inside a
  registered module (`"math.tan"`, assuming `tan` is not in the chosen
  subset) returns `None` from `resolve_symbol`. This crate must itself reach
  100% line/region coverage (D-014) — every `StdModule`/`StdSymbolKind`
  variant needs at least one exercising test.

## Task 2 — HIR import statement handling (D-137)

File: `crates/pycc_hir/src/lib.rs`.

- `pycc_ast` already carries `Stmt::Import`/`Stmt::ImportFrom` (confirmed,
  inherited from the vendored parser — see the existing comment near line
  541 in this file documenting that `pycc_hir` has *no* handling for either
  variant today). Add a new `lower_stmt` arm for both.
- Add a new `HirStmt::ImportBinding` variant (or extend `HirModule` with a
  side-table, matching the `type_aliases: Vec<(String, Ty)>` shape PR-13
  added for aliases — pick whichever is more consistent with how
  `pycc_types`/`pycc_mir` already consume compile-time-only bindings; an
  import binding has zero runtime footprint for the `import math` form
  itself, same as a type alias, so the side-table shape is the better
  precedent to follow unless a concrete counter-example turns up during
  implementation).
- Lowering logic for `Stmt::Import`:
  - Exactly one dotted name, no `as` alias, resolving via
    `pycc_std::resolve_module` → record a module-import binding
    `(local_name, StdModule)` in the module-level side-table.
  - Anything else (multiple names in one `import` statement, an `as` alias,
    an unresolvable module name) → the existing generic `C0001` catch-all,
    reusing the existing `unsupported`-diagnostic helper at this file's
    `C0001` construction site (do not add a new diagnostic code for this —
    D-137 is explicit that every non-registry import shape stays `C0001`).
- Lowering logic for `Stmt::ImportFrom`:
  - Module name resolves via `pycc_std::resolve_module`, every imported name
    resolves via `pycc_std::resolve_symbol`, no `as` aliases, no relative
    (`from . import x`) level → record one `(local_name, StdModule, symbol)`
    binding per imported name in the side-table.
  - Any single unresolvable name inside an otherwise-valid `from math import
    ...` list, any `as` alias, any relative import, any unresolvable module
    → `C0001` for the whole statement (fail closed, not partial binding).
- Unit tests: `import math` binds `math` to `StdModule::Math`; `import cgi`
  is `C0001`; `from math import sqrt, pi` binds both names; `from math import
  sqrt, tan` (assuming `tan` unregistered) is `C0001` for the whole
  statement, not a partial bind of `sqrt`; `import math as m` is `C0001`;
  `from . import x` is `C0001`.

## Task 3 — type-checking: bind registry symbols to real `Ty`s (D-136/D-137)

File: `crates/pycc_types/src/lib.rs`.

- Depends on `pycc_std` (add the `Cargo.toml` dependency) and on Task 2's HIR
  side-table being threaded through into whatever `pycc_types` entry point
  consumes `HirModule` (verify exact current signature at implementation
  time — likely the same place `type_aliases` from PR-13 is threaded in).
- For the plain-`import` form (`math.sqrt(x)` at a call site,
  `math.pi` at a name/attribute-access site): extend
  `collect_expr_constraints`/`infer_expr_in`'s existing attribute-access and
  call-site handling (the same functions that special-case `print`/`len`
  around line 495) with a new branch: when the receiver of an attribute
  access or method-style call resolves (via the import side-table) to a
  bound `StdModule`, look up the symbol through `pycc_std::resolve_symbol`
  and produce its registered `Ty` directly — bypassing ordinary
  name-resolution/signature-inference entirely, matching `print`/`len`'s
  existing "hand-recognized intrinsic, not general dispatch" shape exactly
  (D-136's explicit design constraint).
- For the `from ... import ...` form: the imported names become ordinary
  local bindings whose `Ty` is fixed to the registry's recorded type up
  front (a function symbol binds to something callable with that fixed
  signature; a constant binds directly to its `Ty`) — reuse whatever
  existing mechanism resolves a name to a fixed non-inferred `Ty` (the alias
  table from PR-13 is again the closest existing precedent: a compile-time
  name→`Ty`/signature substitution, not a new inference pathway).
- Add a new diagnostic if a bound stdlib symbol is *called* with the wrong
  argument shape (arity or scalar type) — decide at implementation time
  whether this reuses an existing arity/type-mismatch code or needs a new
  `T0043`; record whichever is chosen in `docs/DIAGNOSTICS.md` (Task 6).
- Unit tests: `import math; print(math.sqrt(2.0))` type-checks with
  `math.sqrt`'s call producing `Ty::Float`; `from math import pi;
  print(pi)` type-checks with `Ty::Float`; calling `math.sqrt("x")` (wrong
  arg scalar) is a clean diagnostic, not a panic; referencing
  `math.tan` (unregistered symbol under a *bound* module) is a clean
  diagnostic distinct from the HIR-level `C0001` for a wholly unresolvable
  import (decide the exact code at implementation time — this is a
  bound-module/unbound-symbol case, which may deserve its own code separate
  from both `C0001` and `T0043`).

## Task 4 — MIR lowering + codegen for the concrete thin slice (D-136)

Files: `crates/pycc_mir/src/lib.rs`, `crates/pycc_codegen/src/lib.rs`,
`crates/pycc_rt` (only if `sys.argv` needs a new runtime object — see Task 1's
note; skip this sub-bullet entirely if `sys` is narrowed to `sys.exit` only,
since that needs only a libc `exit`/process-exit call, which likely already
exists somewhere in this codebase's existing exit-code handling — check
before adding a new runtime symbol).

- Pick the concrete thin slice actually reaching codegen, following this
  series' established pattern (D-105 through D-135 each shipped exactly one
  concrete case to real generated code, with everything else a clean
  diagnostic): `math.sqrt(x: float) -> float` calling into the platform
  `libm` `sqrt` (the same C ABI the existing scalar float codegen path
  already links against — verify whether `pycc_codegen`/`pycc_rt` already
  link `libm` for any existing float operation before adding a new link
  flag), plus `math.pi` as a compile-time float constant (no runtime call at
  all — just emit the literal).
- Extend `pycc_mir`'s existing `print`/`len`-style special-cased lowering
  (find the exact match arm dispatching on callee name, the same one Task 3
  in `pycc_types` mirrors) with a `math.sqrt`/`math.pi` arm producing either
  a new `MirExpr::NativeCall` variant (if none exists for calling a
  non-pycc-authored external symbol) or reusing an existing extern-call
  mechanism if `pycc_codegen` already has one for any other purpose — check
  before adding a new `MirExpr` variant.
- `pycc_codegen`: emit an LLVM `declare` for `sqrt(double) -> double` (or the
  correct libm symbol name/signature for this target) and a `call`
  instruction at the `math.sqrt` call site; emit the float-constant
  immediate for `math.pi`.
- Every other registered-but-not-yet-lowered symbol (if the registry ends up
  larger than this one lowered case — e.g. `math.floor`/`math.ceil` if kept
  in the Task 1 registry) needs its own lowering arm too, or must be removed
  from the registry until it has one: **the registry must never contain a
  symbol that type-checks successfully in Task 3 but then panics or silently
  produces wrong output in `pycc_mir`/`pycc_codegen`** — if time is short,
  narrow Task 1's registry down to exactly the symbols Task 4 actually
  lowers (`math.sqrt`, `math.pi`, `sys.exit`) rather than leaving
  type-checkable-but-unlowerable entries; record any such narrowing as a
  plan-deviation note.
- Unit/integration tests: a `.py` fixture calling `math.sqrt` and printing
  the result produces the correct value through the full compile-and-run
  pipeline (same pattern as existing end-to-end fixture tests elsewhere in
  this crate's test module).

## Task 5 — conformance fixtures (D-138, D-139)

- `tests/fixtures/<appropriate PEP dir>/pep_0594_dead_batteries.py`: `import
  cgi` (or whatever concrete removed module is chosen at implementation
  time — `cgi` is D-138's example, verify it is still the best choice once
  the exact registry/rejection message is finalized) asserts the compiler
  cleanly rejects with `C0001`, matching every other conformance fixture's
  existing pattern for asserting a *specific* diagnostic rather than mere
  non-crash (check how existing negative/rejection fixtures assert their
  expected diagnostic code — likely `tests/conformance.rs` or a sibling
  diagnostics-focused test file, not the CPython-oracle-diff path, since
  CPython does not reject `import cgi` the same way this compiler does;
  confirm this fixture belongs in a diagnostics-focused test list, not the
  oracle-diffed corpus).
- A second, passing fixture exercising `math`/`sys` end-to-end (the concrete
  `math.sqrt`/`math.pi` slice from Task 4), oracle-diffed against CPython
  3.14.6 in both `--debug` and `--release` per D-102's dual-profile
  convention, wired into `tests/conformance.rs`.
- D-139 hand-authored container/generics corpus: one fixture combining
  list/dict/set/tuple literals, at least one comprehension, slicing, at
  least one container method call, and a generic-function call (PR-13's
  thin slice) in a single realistic program, oracle-diffed against CPython
  3.14.6 on all 5 Tier-1 targets, same harness as every other corpus
  fixture — re-read D-139's own text in `docs/DECISIONS.md` in full before
  writing this fixture, since it is the authoritative source for exactly
  which features it must combine, not this plan.
- Re-verify D-088's itemized ≥15-row conformance count against
  `docs/PYTHON_STANDARDS.md`'s current state once these fixtures land; flip
  every row this PR's fixtures newly satisfy.

## Task 6 — documentation sweep (same PR, same commit as the code)

- `docs/STDLIB_PLAN.md`: record `pycc_std`'s actual shipped `math`/`sys`
  symbol subset (not the aspirational full stdlib schedule) and cross-link
  to D-136/D-137.
- `docs/PYTHON_STANDARDS.md`: flip every row this PR's fixtures newly
  satisfy; update the conformance count to match D-088's corrected ≥15
  target; add the PEP 594 row.
- `docs/DELIVERY_PLAN.md` row 14: replace with "Merged to `main`" prose
  mirroring rows 10–13's exact style (PR link, squash-merge commit, date,
  what shipped, what's explicitly deferred).
- `docs/ROADMAP.md`: verify and flip every v0.2 acceptance bullet honestly
  against D-088's corrected criteria — this is explicitly the last PR of
  v0.2, so this is the point where v0.2's overall shippability gets an
  honest final assessment, not just this PR's own row.
- `docs/DECISIONS.md`: graduate D-136–D-139 status fields if they carry a
  `proposed`/`accepted-pending-implementation` marker today; also record any
  plan-deviation narrowing from Tasks 1/4 (registry subset, `sys` scope) as
  an addendum note on D-136 rather than silently shipping a different
  design than what the ADR describes.
- `docs/DIAGNOSTICS.md`: register whatever diagnostic code(s) Task 3
  introduces (existing code reused, or new `T0043`).
- `docs/SPEC.md`: update only if a document is added/renamed/repurposed
  (unlikely for this PR — confirm once Tasks 1–5 are done).

## Task 7 — review and merge

- Run `cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions
  100` locally before requesting review, including the new `pycc_std` crate;
  add tests for every branch Tasks 1–4 introduce until clean (D-014, no
  exemptions expected).
- Run the pinned `ievo:deep-reviewer` (D-068) against the staged diff
  (`git add` every new untracked file — fixtures, the new crate — before
  requesting review, per the documented iEvo 0.70.1 `--working` omission,
  ievo-ai/skills#483).
- Open the PR against `main`; require the current CI check, resolved
  conversations, up-to-date branch (D-024); no separate approving review
  required (solo-maintainer branch protection).
- After merge: write a `docs/sessions/YYYY-MM-DD-NN-<slug>.md` checkpoint
  assessing v0.2 completeness honestly against `docs/ROADMAP.md`'s
  D-088-corrected criteria, regardless of whether it's fully done.

## Explicit non-goals (confirm still true at merge, not just at planning time)

- General/relative multi-file import resolution — stays v0.4 per
  `docs/ROADMAP.md`, unchanged by this PR.
- Any stdlib module beyond the chosen `math`/`sys` subset — no `os`, `sys`
  beyond the chosen symbols, `typing`, `collections`, etc.
- A general FFI/native-call mechanism — D-136 explicitly scopes this PR away
  from building one; `math.sqrt`'s libm call is a special case, not a
  reusable declare-any-external-symbol facility.
- Import aliasing (`import x as y`), star imports (`from x import *`) — both
  `C0001`.
