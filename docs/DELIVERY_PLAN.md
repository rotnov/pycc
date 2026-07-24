# pycc Delivery Plan

How the roadmap in [ROADMAP.md](./ROADMAP.md) actually gets built: milestone decomposition, environment baseline, v0.1 execution strategy, and the autonomy policy that lets agents proceed without stopping for questions. Companion to [SPEC.md](./SPEC.md); written under D-013 (AI-first development).

## Program-level decomposition

Every ROADMAP.md milestone is its own sub-project: its own brainstorm → plan → implementation cycle, entered only when the previous milestone's acceptance criteria are green. Only v0.1 is detailed below (see [Delivery mechanics](#delivery-mechanics) for why); v0.2+ get the same treatment when reached, since their design will be shaped by what v0.1–v0.4 teach us.

| Milestone | New crates / major additions | Depends on | Rough PR count |
|---|---|---|---|
| v0.1 hello, binary | `pycc`, `pycc_lexer`/`pycc_parser`/`pycc_ast` (vendored `ruff_python_parser`, D-003), `pycc_hir`, `pycc_types`, `pycc_mir`, `pycc_codegen`, `pycc_rt`, `pycc_diag`, `pycc_testkit`, CI matrix on 5 Tier-1 targets | — | ~7 (detailed below) |
| v0.2 collections & generics | `pycc_std` created (`math`, `sys`), monomorphization in `pycc_types`/`pycc_mir`, `--release`/LTO profile, `pycc.toml` | v0.1 | ~5-6 |
| v0.3 classes & pattern matching | class model, dataclasses, protocols, `match` w/ exhaustiveness, diagnostics registry grows | v0.2 | ~6 |
| v0.4 projects & incremental | multi-file/import resolution in `pycc_hir`, salsa-style incremental cache, `os`/`pathlib`/`json`/`datetime` | v0.3 | ~6-7 |
| v0.5 generators & ownership v1 | new crate `pycc_own` (escape analysis, move semantics, RC elision), generators as state machines | v0.4 | ~6-7 |
| v0.6 threads without GIL | Shareable/move checks in `pycc_own`, cycle collector, own parser replaces vendored `ruff_python_parser` (D-003 resolved) | v0.5 | ~6 |
| v0.7 interop escape hatch | `pycc.interop.cpython` typed boundary | v0.6 | ~4-5 |
| v0.8 corpus at scale + bot | `corpus-bot` GitHub Action, `socket`/`http.client`/compression stdlib | v0.7 | ~4 |
| v0.9 async & packaging | `asyncio` subset on state machines, `--lib` C-ABI | v0.8 | ~4-5 |
| v1.0 spec freeze | PYTHON_STANDARDS matrix complete, semantics deviations doc, benchmarks published, diagnostics/JSON frozen semver | v0.9 | ~3-4 |

## Environment baseline (verified, not assumed)

Verified empirically on the primary dev host (macOS, aarch64-apple-darwin) before committing to any of the below:

| Requirement | Finding | Consequence |
|---|---|---|
| Rust | `rustc 1.97.1` (stable, updated via `rustup update stable`; matches README's "1.97+" exactly) | Toolchain pinned via `rust-toolchain.toml` inside the repo — **not** a global `rustup default` change, since this machine has other toolchains (incl. a `solana` one) that must stay untouched |
| LLVM | `22.1.1` (single Homebrew keg; the `llvm@17`..`llvm@22` opt-paths are stale symlinks to the same keg, not distinct installs) | `inkwell = "0.9"` with feature `llvm22-1` — clean match, no version fudging needed |
| CPython oracle | `python3.14` → `3.14.3` at `/opt/homebrew/bin/python3.14` | Satisfies TESTING.md's differential requirement (`stdout == CPython 3.14 stdout`) |
| Local linker | Apple clang 21 / Xcode CLT `ld64` | Sufficient for the first vertical slice on native host; bundled `lld` for cross-compilation is wired when `--target` work starts, not before |
| crates.io | Reachable (a bare `curl -I` 403s on crates.io's anti-bot filter — mundane, not a sandbox restriction; a real UA gets 200) | `cargo build` can fetch `ruff_python_parser`, `inkwell`, `rayon`, `mimalloc` |
| `gh` CLI | Authenticated, `repo`+`workflow` scopes | Can open PRs and push `.github/workflows` |

Only macOS is locally verifiable. Linux x64/arm64 and Windows MSVC exist only via CI — so CI must be wired right after the first local slice works, not as a v0.1 finishing touch (see PR-3 below).

## v0.1 crate scope

Not all 11 crates in [ARCHITECTURE.md](./ARCHITECTURE.md) are needed on day one.

**Built now:** `pycc` (CLI/driver), `pycc_lexer`/`pycc_parser`/`pycc_ast` (thin wrapper over vendored `ruff_python_parser`), `pycc_hir`, `pycc_types` (T0001 strictness + local inference over the v0.1 type subset only), `pycc_mir`, `pycc_codegen` (LLVM via `inkwell`), `pycc_rt` (minimal runtime), `pycc_diag`, `pycc_testkit`.

**Deliberately deferred:** `pycc_own` (ownership/escape analysis is a v0.5 item — semantics-preserving, perf-only, so v0.1 just uses RC/heap without ownership inference), `pycc_std` (real importable modules start v0.2; the Tier-0 builtins v0.1 needs — `print`, `range`, etc. — live as intrinsics directly in `pycc_rt`/`pycc_codegen`).

## v0.1 execution strategy: thin vertical slice, then breadth

Three approaches were weighed: (A) thin end-to-end slice first, then grow features; (B) parallel crate teams integrated at the end; (C) fully lock every crate interface up front, then build sequentially.

**A was chosen.** B reproduces the "horizontal slicing" anti-pattern the repo's own `tdd` skill warns against — interfaces designed independently by separate workstreams diverge at integration time, and a compiler pipeline's crates have real, not incidental, sequential dependencies. C is excess upfront design for a project whose own TESTING.md philosophy is "prove it with a test," not "prove it on paper" — and the biggest unknown (does LLVM/inkwell/linking/CI actually work end to end on 5 platforms) is exactly the thing A de-risks first instead of discovering three weeks into frontend work.

1. **Slice 0**: `def main() -> None: print(42)` compiled through the entire pipeline to a running native binary on the local host. Proves crate wiring before anything else.
2. **CI matrix** on all 5 Tier-1 targets immediately after slice 0 — not at the end. From this point, CI is the source of truth for autonomous progress, not local macOS runs.
3. **TDD red-green, vertical slices** for feature growth: one conformance test per feature, written first, then the implementation — arithmetic → comparisons → `if`/`while`/`for`+`range` → functions/recursion → basic f-strings.
4. **Parallelism** only where genuinely independent (CI tooling, `pycc_diag` registry scaffolding, `docs/semantics.md`) via subagents; the pipeline core stays sequential because its dependencies are real.

### Rough PR breakdown

| PR | Content |
|---|---|
| 1 | Workspace scaffold (all crate stubs), `rust-toolchain.toml` (1.97.1), CLI skeleton, CI skeleton **with the `cargo llvm-cov --fail-under-lines 100 --fail-under-regions 100` gate wired in from the first commit** (D-014). New ADRs appended to DECISIONS.md: LLVM 22.1/inkwell 0.9 pin, vendored `ruff_python_parser` version |
| 2 | Slice 0: parser → HIR → types (passthrough) → MIR → LLVM codegen → link; "hello binary" runs locally |
| 3 | CI matrix live on all 5 Tier-1 targets for slice 0 |
| 4 | Frontend depth: full v0.1 grammar, real T0001 + local inference, first diagnostic codes with snapshot tests |
| 5 | Codegen depth: full v0.1 feature set (int/float/str/bool, arithmetic, control flow, recursion, f-strings); runtime fleshed out (overflow→bigint per D-001, small-string opt per D-007) |
| 6 | `pycc_testkit`: fib + mandelbrot-ascii vs. CPython 3.14.3 on all 5 targets × 2 profiles; `pycc check` benchmark <50ms/1k LOC; diagnostic output matches CLI_SPEC.md's example byte-for-byte |
| 7 | Buffer: close whatever's left so all v0.1 ROADMAP.md acceptance bullets are simultaneously green |

## Autonomy policy ("no questions" mechanics)

**Spec is law.** Where an existing doc already decided something, follow it literally. Where the spec is genuinely silent (as with the exact LLVM/inkwell version above), the agent picks the most conservative option that is *actually available in this environment* — never a hypothetical — records it as a new dated entry in [DECISIONS.md](./DECISIONS.md) using its existing template, and continues without stopping.

This resolves the one real tension with the merged `tdd` skill, which asks to confirm test seams with the user before writing tests: the approved plan (this document + the writing-plans output that follows it) enumerates the seams per crate up front. Approval of the plan **is** the seam confirmation; implementation agents do not re-ask.

This policy governs architectural/implementation forks only. Standing safety rules (force-push, branch deletion, rewriting published history, etc.) are unaffected and still pause for explicit confirmation.

## Delivery mechanics

This plan is committed to the current branch (`claude/project-overview-53ef3d`, currently identical to `main`) and opened as a docs-only PR. Every subsequent PR in the table above is its own feature branch off `main`, merged only once CI is green on all Tier-1 targets and that PR's slice of the v0.1 acceptance criteria is demonstrably met.

## Testing scope for v0.1

Of TESTING.md's 7 layers: Layer 1 (per-crate unit tests) from the start, Layer 2 (`pycc_testkit` conformance harness) as early as slice 0, Layer 3 (diagnostic snapshot tests) as soon as `pycc_diag` exists, Layer 5 (runtime property tests) in minimal form for the v0.1 runtime subset. **Out of scope for v0.1**: Layer 4 (differential fuzzing), Layer 6 (OSS corpus), Layer 7 (benchmarks vs. Nuitka/Codon/mypyc) — these start at v0.2 per ROADMAP.md.

Cutting across all of these: the D-014 coverage gate (`cargo llvm-cov --fail-under-lines 100 --fail-under-regions 100`) applies to every crate from PR-1 on — it is not a v0.1-specific item but a standing requirement, wired into the CI skeleton before any crate has a chance to accumulate untested code. Each task in the implementation plan below writes its test alongside its code for exactly this reason.

## Scope honesty

v0.1 is realistically a multi-PR, multi-session effort — real Rust/LLVM compilation and 5-target CI have minutes-scale iteration loops, not seconds. Work proceeds PR by PR, autonomously, opening each PR as it's ready, without stopping for questions, for as long as a session productively runs.
