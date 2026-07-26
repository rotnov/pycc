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

**Current state (through PR-5):** the diagram above is the v1.0 target. As of PR-5, `pycc_own` does not exist (deferred to v0.5, per DELIVERY_PLAN.md's crate scope), so there is no separate ownership-analysis stage and no THIR; `pycc_types` produces a checked HIR directly, and `pycc_mir`'s `MIR` is a typed *structural mirror* of HIR (D-048), not the ownership-annotated SSA form shown above -- LLVM codegen uses one `alloca` per local/parameter and relies on no optimization pass, matching this project's `--debug`-only v0.1 profile. The `optimizations` stage does not exist yet either. This is a deliberate, currently-accepted gap between the target architecture and today's implementation, not an unplanned deviation.

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

Bootstrap note: v0.1 may vendor `ruff_python_parser` to move fast; replaced by own parser before v0.6 (tracked in [DECISIONS.md](./DECISIONS.md) D-003).

## Performance requirements (compiler itself)

- `pycc check`: ≥ 300k LOC/s single-thread parse+check target; feel = ruff, not mypy.
- Parallelism: per-module `rayon` pipeline; codegen units like rustc CGUs.
- Incremental: salsa-style query graph; red-green re-validation; on-disk cache in `.pycc/`.
- Memory: arenas + interned strings/types; no `Rc<RefCell<…>>` in hot paths.
- Benchmarks in CI on every PR; >2% frontend regression blocks merge.

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
