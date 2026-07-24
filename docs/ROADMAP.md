# pycc Roadmap

Milestone = shippable + demo-able. Acceptance criteria are binary; a milestone isn't done until they're green on **all Tier-1 platforms** (Linux x64/arm64, macOS x64/arm64, Windows x64).

## Current delivery status

Last reviewed against default-branch commit `fab1232` on 2026-07-24. This section counts only behavior merged to `main`; an open pull request is work in flight, not implemented behavior.

**Current milestone: v0.1 — in progress.** The first end-to-end vertical slice works on the primary macOS arm64 host, but v0.1 is not yet shippable.

| Area | Status on `main` | Evidence and remaining gap |
|---|---|---|
| Compiler pipeline | Partial | The workspace contains the driver plus `pycc_ast`, `pycc_parser`, `pycc_hir`, `pycc_types`, `pycc_mir`, `pycc_codegen`, `pycc_rt`, and `pycc_diag`. [`tests/slice0.rs`](../tests/slice0.rs) proves source → parser → HIR → type-check passthrough → MIR → LLVM object → host linker → native executable. |
| Language surface | Slice only | Module-level `print(<i64 literal>)`, zero-argument function definitions, and explicit zero-argument calls work. Arithmetic, variables, arguments and return values, control flow, recursion with values, floats/strings/bools, `range`, and f-strings remain v0.1 work. Unsupported valid Python can still reach explicit slice-limit panics in [`pycc_hir`](../crates/pycc_hir/src/lib.rs). |
| Type system | Stub | The parser preserves annotations, but [`pycc_types::check`](../crates/pycc_types/src/lib.rs) is still a no-op passthrough. `T0001` strictness and local inference have not landed. |
| CLI | Partial | `build`, `run`, and `version` have working slice-level paths. `check`, `test`, `explain`, `init`, and `clean` report “not yet implemented”; the broader flags and project-mode contracts in [CLI_SPEC.md](./CLI_SPEC.md) remain planned. |
| Diagnostics | Partial | Parser failures become `L0001` compile diagnostics and the diagnostic data types exist. Stable spans, the shipped-feature registry, screenshot parity, JSON output, and diagnostics for unsupported valid programs remain open. |
| Portability | Primary host only on `main` | The active CI compiler job runs on macOS arm64. The five-target Tier-1 matrix, bundled-linker cross-compilation, and cross-host execution evidence required by v0.1 are not yet merged. |
| Quality gates | Partial but enforced | Unit and slice-level end-to-end tests pass, and CI enforces 100% Rust line and region coverage on every PR. The conformance harness, diagnostic snapshots, frontend performance gate, five-target conformance, fuzzing, and corpus layers remain planned according to [TESTING.md](./TESTING.md). |

### v0.1 acceptance checklist

- [ ] `fib` and `mandelbrot-ascii` compile and match CPython output on all five Tier-1 targets.
- [ ] `pycc check` processes 1k LOC in under 50 ms.
- [ ] The error demonstration matches the stable [CLI specification](./CLI_SPEC.md) output.
- [ ] The five-target native CI matrix and one cross-host compilation path are live on `main`.
- [x] The 100% line and region coverage gate is required and green for the current slice.

The next delivery slices remain the sequence defined in [DELIVERY_PLAN.md](./DELIVERY_PLAN.md): Tier-1 CI and cross-compilation, frontend depth with real strict typing and the performance gate, full v0.1 codegen/runtime breadth, the conformance testkit and named demos, then the final v0.1 acceptance pass.

## v0.1 — "hello, binary"

Functions, `int`/`float`/`str`/`bool`, arithmetic, comparisons, `if`/`while`/`for`+`range`, f-strings (basic), `print`, module-level code, recursion. Frontend: strict annotations (`T0001`), local inference. Backend: LLVM debug builds, vendored parser allowed.

**Accept:** `fib`, `mandelbrot-ascii` compile & match CPython output on 5 targets; `pycc check` on 1k LOC < 50 ms; error demo screenshot-parity with CLI_SPEC.md; CI matrix live; 100% line+region coverage gate green on every crate (D-014, see TESTING.md).

## v0.2 — collections & generics

`list`/`dict`/`set`/`tuple` + literals, comprehensions, slicing, methods; PEP 585/695 generics via monomorphization; `--release` profile (LTO); `pycc.toml`.

**Accept:** corpus Tier-1 (`tomli`, `packaging`, `more-itertools`) compiles; nbody ≥ 20× CPython; conformance ≥ 25 PEPs green.

## v0.3 — classes & pattern matching

Classes, inheritance+C3, `@property`, dataclasses, enums, protocols, `match` (634) with exhaustiveness, exceptions (`try/except/finally`, chains).

**Accept:** conformance ≥ 45 PEPs; diagnostics registry fully implemented for shipped features; `pycc explain` live.

## v0.4 — projects & incremental

Multi-file, imports, namespace packages (420), incremental cache, parallel codegen, `os`/`pathlib`/`json`/`datetime` native.

**Accept:** corpus Tier-2 (`black`, `isort`, `attrs`, `click`) ≥ 80% files compile; incremental rebuild of 10k LOC < 200 ms; cross-compile demo mac→windows.exe in README gif.

## v0.5 — generators & ownership v1

Generators/`yield from` as state machines, iterator protocol, `itertools`/`functools`; ownership: escape analysis + move semantics + RC elision live; `--memstats`.

**Accept:** RC-elision ≥ 70% on corpus mean; fuzzing layer-4 running continuously; zero known miscompiles open > 7 days.

## v0.6 — threads without GIL

`threading`/`queue`, Shareable/move checks (`O03xx`), cycle collector, own parser replaces vendored (D-003 resolved).

**Accept:** thread-safety negative tests; 8-core scaling demo ≥ 6× on embarrassingly-parallel bench; race detector (TSan CI job) clean.

## v0.7 — interop escape hatch

`pycc.interop.cpython`, typed boundary (`I04xx`), `[interop] allow` config; `unittest`/`logging`/`argparse`.

**Accept:** demo: compiled app calls numpy through the hatch; boundary-cost benchmark published; pure-mode (`allow = []`) guarantees no libpython dependency.

## v0.8 — corpus at scale + bot

Corpus Tier-3 (`mypy`, `httpx`, `rich`) tracked; corpus-bot auto-issues live; `socket`/`http.client`; compression stack incl. PEP 784 zstd.

**Accept:** bot files/dedupes/closes issues autonomously for 30 days without human cleanup; ≥ 90 PEPs green.

## v0.9 — async & packaging

`asyncio` subset on state machines; `pycc build --lib` C-ABI; binary size diet; signing/notarization docs per OS.

**Accept:** async echo-server demo; `--lib` consumed from Rust and C in CI.

## v1.0 — spec freeze

PYTHON_STANDARDS matrix: every row ✅ or explicitly `rejected-by-design` with negative test; semantics deviations doc complete; benchmarks vs CPython/Nuitka/Codon/mypyc published; diagnostics/JSON formats frozen (semver).

**Accept:** corpus Tier-1..3 green 3 releases in a row; fuzzer finds 0 mismatches for 30 consecutive days; docs site.

## Post-1.0 (parking lot)

LSP server · `wasm32-wasi` target · PGO/BOLT pipeline · Cranelift debug backend (D-002) · pip-installable wheels of compiled modules · REPL via cranelift-jit (yes, ironic).
