# pycc Roadmap

Milestone = shippable + demo-able. Acceptance criteria are binary; a milestone isn't done until they're green on **all Tier-1 platforms** (Linux x64/arm64, macOS x64/arm64, Windows x64).

## v0.1 — "hello, binary"

Functions, `int`/`float`/`str`/`bool`, arithmetic, comparisons, `if`/`while`/`for`+`range`, f-strings (basic), `print`, module-level code, recursion. Frontend: strict annotations (`T0001`), local inference. Backend: LLVM debug builds, vendored parser allowed.

**Accept:** `fib`, `mandelbrot-ascii` compile & match CPython output on 5 targets; `pycc check` on 1k LOC < 50 ms; error demo screenshot-parity with CLI_SPEC.md; CI matrix live.

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
