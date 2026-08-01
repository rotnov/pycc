# pycc Architecture

Compiler pipeline, crate layout, and infrastructure decisions. Companion specs: [TYPE_SYSTEM.md](./TYPE_SYSTEM.md), [MEMORY_OWNERSHIP.md](./MEMORY_OWNERSHIP.md), [RUNTIME.md](./RUNTIME.md).

## Pipeline

```
source.py
   │  pycc_lexer      — zero-copy tokenizer (spans, no allocations per token)
   ▼
tokens
   │  pycc_parser     — recursive-descent per CPython 3.14 PEG grammar
   ▼
AST (lossless, spans)
   │  lowering        — desugar: comprehensions, decorators, with, t-strings…
   ▼
HIR (name-resolved, scoped)
   │  pycc_types      — strict type check + inference (see TYPE_SYSTEM.md)
   ▼
THIR (fully typed)
   │  pycc_own        — ownership / escape / thread-safety analysis
   ▼
MIR (typed SSA, ownership-annotated)
   │  optimizations   — inlining, RC elision, devirtualization, monomorphization
   ▼
LLVM IR  ──►  object code  ──►  lld  ──►  native binary (+ pycc_rt + pycc_std)
```

**Current state (through PR-5):** the diagram above is the v1.0 target. As of PR-5, `pycc_own` does not exist (deferred to v0.5, per DELIVERY_PLAN.md's crate scope), so there is no separate ownership-analysis stage and no THIR; `pycc_types` produces a checked HIR directly, and `pycc_mir`'s `MIR` is a typed *structural mirror* of HIR (D-057), not the ownership-annotated SSA form shown above -- LLVM codegen uses one `alloca` per local/parameter and relies on no optimization pass, matching this project's `--debug`-only v0.1 profile. The `optimizations` stage does not exist yet either. This is a deliberate, currently-accepted gap between the target architecture and today's implementation, not an unplanned deviation.

## Workspace crates (Rust 1.97+, edition 2024)

| Crate | Role |
|---|---|
| `pycc` | CLI driver, orchestration, incremental engine |
| `pycc_lexer` / `pycc_parser` / `pycc_ast` | Frontend. Grammar reference: CPython 3.14 `Grammar/python.gram` |
| `pycc_hir` | Desugared, name-resolved tree + scope graph |
| `pycc_types` | Type checker, inference, trait/protocol solver |
| `pycc_own` | Ownership, escape, Send/Sync-style thread checks |
| `pycc_mir` | Typed SSA IR + optimization passes |
| `pycc_codegen` | LLVM backend via `inkwell`; `--emit llvm-ir\|obj\|mir` |
| `pycc_rt` | Runtime staticlib (see RUNTIME.md) — pure Rust, `#![no_std]`-friendly core |
| `pycc_std` | Compiled stdlib subset (typed Python + Rust intrinsics, see STDLIB_PLAN.md) |
| `pycc_diag` | Diagnostics engine, error registry (see DIAGNOSTICS.md) |
| `pycc_testkit` | Conformance/differential test harness (see TESTING.md) |

The implemented v0.1 frontend currently uses `ruff_python_parser` to produce
the AST. `pycc_hir::lower_checked` preserves module statement order and lowers
primitive literals and annotations, assignments, arithmetic, comparisons,
calls, returns, `if`/`while`/`for`+`range`, and basic f-strings. A first
`list[int]` slice (D-105, PR-10) lowers list literals, read-only subscript
indexing (`base[index]`), a dedicated `.append()` call form, and
`for var in <bare-name-list>:` iteration through HIR (D-105's HIR-forms
task), `pycc_types` type-checking including `len()`'s call-dispatch (D-105's
type-checking task, `T0032`/`T0033`/`T0034`), `pycc_mir` lowering (D-105's
MIR-lowering task), and `pycc_codegen` (D-105's codegen task) against
`pycc_rt`'s `PyIntListObj` -- so `build`/`run` compiles and runs a
`list[int]` program end to end, at module scope or inside a private helper
(the two places D-105's first scope cut allows a `list[int]` value to live).
Only `list[int]` reaches codegen: `T0034` rejects every other element type
first. `pycc_codegen` owns the tagged/raw conversion at that runtime
boundary in both directions (D-106), and `list[T]` values are deliberately
never refcounted in v0.2, so their allocations leak for the process's
lifetime (D-107). Two *operations* on a `list[T]` still type-check and then
stop codegen with a "not supported yet" panic rather than compiling, because
v0.2 gives `list[T]` no `str(list)` or `bool(list)` meaning (D-107):
converting one to `str`, and using one as an `if`/`while` condition. The
string conversion is reachable from every context that needs one, which
today means both `print(xs)` and f-string interpolation (`f"{xs}"`) -- they
share a single conversion helper in `pycc_codegen`, so both fail identically.

A second slice (D-111/D-112, PR-11a) extends this same pattern to
`dict[str, int]` and `set[int]`: dict/set literals, `d[k]`/`d[k] = v`
(dict only, insert-or-update), `len(...)`, and `for k in d:`/`for x in s:`
iteration lower through the same HIR/type-checking/MIR/codegen path,
against `pycc_rt`'s `PyDictObj`/`PyIntSetObj` respectively -- so `build`/`run`
now also compiles and runs `dict[str, int]` and `set[int]` programs end to
end, not just `list[int]`. Exactly one key/element combination reaches
codegen per container (`T0036` for dict, `T0038` for set), mirroring
`list[int]`'s own `T0034` gate; every other combination type-checks but is
rejected before codegen. Both new containers stay leak-only in v0.2 (D-114),
matching `list[int]` (D-107), and neither ships a `str(...)`/`bool(...)`
conversion or (for `set`) a membership test -- `in` does not exist anywhere
in this compiler yet (D-113). `tuple[...]` remains unimplemented, pending
its own follow-up plan.

Function items carry their parameter and return types, while call
expressions retain only the bare callee name plus ordered argument
expressions; HIR does not yet assign binding identities or build and memoize a
call graph. Syntactically valid constructs outside that implemented HIR subset
return a spanned `C0001` capability diagnostic, so `pycc check` never turns an
unsupported statement or expression into an uncaught lowering panic.
`pycc_types::check` validates the lowered module against the
inferred signature table without cloning HIR. Compiler stages that need
concrete private-helper signatures use `pycc_types::check_and_resolve`, which
performs the same validation and returns HIR with those signatures
materialized.

`pycc check` stops after the check-only frontend pipeline. `build`/`run`
continue through `pycc_mir` and `pycc_codegen` into the full v0.1 language
surface as of PR-5 (arithmetic including `int` overflow-to-bigint, `if`/
`while`/`for`+`range`, functions with real parameters/return values and
recursion, `int`/`float`/`bool`/`str`, string concatenation, basic
f-strings, and type-aware `print`) -- the PR-4-era "only integer-literal
`print()` and zero-argument calls" boundary this paragraph used to describe
no longer exists (D-072 records `pycc_mir`'s own D-035 boundary panic
closing for real, and D-074 records the backend lexical-scope and
representation fixes needed to preserve it). See
[ROADMAP.md](./ROADMAP.md)'s "Language surface" row for the specific,
still-open gaps (unary operators, bigint-operand arithmetic beyond
overflow, etc.), not a broad "backend is narrower" statement.

Bootstrap note: v0.1 may vendor `ruff_python_parser` to move fast; replaced by own parser before v0.6 (tracked in [DECISIONS.md](./DECISIONS.md) D-003).

## Performance requirements (compiler itself)

- `pycc check`: ≥ 300k LOC/s single-thread parse+check target; feel = ruff, not mypy.
- Parallelism: per-module `rayon` pipeline; codegen units like rustc CGUs.
- Incremental: salsa-style query graph; red-green re-validation; on-disk cache in `.pycc/`.
- Memory: arenas + interned strings/types; no `Rc<RefCell<…>>` in hot paths.
- Benchmarks in CI on every PR; >2% frontend regression blocks merge.

The check-only frontend path validates the original HIR against its inferred
signature table without materializing a resolved HIR clone. Compiler stages
that need concrete private-helper signatures use `check_and_resolve` and pay
for that returned clone; `pycc check` does not construct and discard it.
When every declared function signature is already concrete, the validation-only
checker builds its function environment directly rather than materializing and
then cloning an intermediate signature table; the constraint-collection walk
is reserved for modules that contain an actual private-helper inference
variable. A concrete module that fails validation falls back to the historical
solver-first sequence so the selected diagnostic does not change when multiple
errors are present; valid concrete modules keep the single-pass fast path.
Call validation preserves its all-arguments-before-arity diagnostic order while
holding up to four inferred argument types in a stack buffer; wider calls use a
heap-backed fallback.
Per-function checking also shares the immutable module function registry
through an `Arc`-backed copy-on-write table. Function-local environments still
clone global bindings so parameter and assignment changes remain isolated, but
they no longer copy every registered function name and signature or clone the
registered parameter vector merely to read it.

## Cross-platform (hard requirement)

Tier 1 — build, test, CI-gated from MVP:

| Target | Notes |
|---|---|
| `x86_64-unknown-linux-gnu` / `aarch64-unknown-linux-gnu` | primary CI |
| `x86_64-apple-darwin` / `aarch64-apple-darwin` | primary dev platform |
| `x86_64-pc-windows-msvc` | MSVC ABI, from v0.1 — not an afterthought |

Rules:

- Runtime has **zero** platform-conditional behavior visible to user code (path/OS specifics live in `pycc_std` behind `os`/`pathlib` just like CPython).
- Cross-compilation: `pycc build --target <triple>` is currently proven for same-OS/cross-arch only (e.g. macOS x86_64⟷arm64, CI-gated) — cross-OS targets are not yet supported (see D-026). Linking goes through each host's own toolchain driver (system `cc`, or a bundled `clang` on Windows/Linux when a target is given), not a universally bundled linker.
- Static linking by default on Linux (musl optional), self-contained .exe on Windows, notarization-friendly binary on macOS.
- CI matrix runs the full conformance suite on all Tier-1 targets; a PEP test only counts as passing when it passes everywhere.
- Tier 2 (build, best-effort tests): `aarch64-pc-windows-msvc`, `x86_64-unknown-linux-musl`, `wasm32-wasi` (experiment).

## Debug info & tooling

- DWARF/PDB mapping machine code → original `.py` lines (PEP 626/657 fidelity): `gdb`/`lldb`/VS debuggers step through Python source.
- `--error-format=json` for editors; LSP server (`pycc lsp`) reuses the query engine post-v1.
- Deterministic builds: same input → bit-identical binary per target.
