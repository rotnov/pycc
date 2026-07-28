# pycc — ahead-of-time compiler for typed Python

[![CI](https://github.com/rotnov/pycc/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/rotnov/pycc/actions/workflows/ci.yml)
[![test coverage: 100%](https://img.shields.io/badge/test%20coverage-100%25-brightgreen)](./docs/TESTING.md)

**A strict ahead-of-time (AOT) compiler that turns type-annotated Python 3.14—the v1.0 language target—into standalone native binaries. Like `gcc`, but for Python.**

`pycc` is being built to take standard Python 3.14 source code, enforce every type annotation at compile time, and produce a fast, standalone native binary. No interpreter, no venv, no new language to learn — the design contract is that valid typed Python compiles and incorrect types do not.

Written in Rust (1.97+). Built to be extremely fast — both the compiler itself and the binaries it produces.

> Status: pre-alpha. `pycc check` now parses and type-checks the v0.1
> frontend surface, enforcing public annotations, rejecting `Any`, inferring
> private-helper signatures, and rendering human or JSON diagnostics.
> `pycc build` and `pycc run` compile that implemented v0.1 surface through
> MIR, LLVM, the host linker, and the native runtime. The compiler remains
> pre-alpha: documented representation and lifetime gaps are still roadmap
> work, but v0.1's own acceptance criteria are now met -- `fib` and
> `mandelbrot-ascii` match pinned CPython output on all five Tier-1 targets,
> `pycc check` clears its <50ms/1000 LOC throughput floor, and diagnostic
> output matches [`docs/CLI_SPEC.md`](./docs/CLI_SPEC.md)'s example; see
> [`docs/ROADMAP.md`](./docs/ROADMAP.md).
> The frontend performance measurement and isolated greater-than-2% regression
> gate are required through `ci-gate` independently of that compiler sequence.
> The source-aware paired gate measures the exact predecessor and candidate
> sequentially on one hosted runner and seals the predecessor timing before
> candidate code runs. It classifies all repository-owned executable inputs
> before that execution: identical inputs keep the observed timing as
> non-blocking environment telemetry, while changed `src/` or `crates/` inputs
> use exactly five complete runs per revision, compare the median of their
> per-run medians, and retain the hard greater-than-2% regression block. All ten
> timing files are retained. Revision, benchmark-contract, executable-input
> identity, artifact-identity, exact file-set, and comparison drift fail closed.
> See the [current status](https://rotnov.github.io/pycc/status/) and
> [`docs/PYTHON_STANDARDS.md`](./docs/PYTHON_STANDARDS.md).

Python 3.14 is the v1.0 target, not a permanent language ceiling. Later
standard Python language levels may enter only through explicit versioned
conformance gates and superseding design decisions; pycc never adds its own
syntax or dialect.

[Project website](https://rotnov.github.io/pycc/) · [Current status](https://rotnov.github.io/pycc/status/) · [Architecture](https://rotnov.github.io/pycc/architecture/) · [Python AOT compiler comparison](https://rotnov.github.io/pycc/python-aot-compilers/) · [AI-native experiment](https://rotnov.github.io/pycc/ai-native/) · [Search visibility](./docs/SEARCH_VISIBILITY.md) · [Specification](./docs/SPEC.md) · [Roadmap](./docs/ROADMAP.md)

## The experiment

**pycc is an experiment in autonomous, AI-native software development: the project writes itself.** AI agents create its specifications, code, tests, documentation, reviews, and release automation. Humans set goals and constraints; not a single line of project code is handwritten by a human.

## Why

Python compiler projects make different tradeoffs between compatibility,
static semantics, output artifacts, and runtime dependencies. pycc is testing
one particular combination; this is a model comparison, not a benchmark or a
claim that pycc is ready to replace released tools:

| | Enforces types at compile time | Native executable without CPython | Standard Python input |
|---|---|---|---|
| **pycc (v1.0 design target)** | ✅ hard compile error | ✅ design target | ✅ CPython 3.14 target |
| LPython | ✅ typed subset | ✅ AOT executable | ⚠️ CPython-compatible subset |
| Codon | ✅ static language | ✅ | ⚠️ Python-like language with documented differences |
| Nuitka | ❌ | ❌ packages CPython runtime components | ✅ compatibility-focused |
| mypyc | ✅ strict compiled subset | ❌ CPython C extension | ✅ type-annotated Python subset |
| Cython | ⚠️ optional | ❌ extension or embedded CPython | ⚠️ Python superset |
| mypy / pyright | ✅ | ❌ checker only | ✅ |

See the [source-backed Python AOT compiler comparison](https://rotnov.github.io/pycc/python-aot-compilers/)
for the language, artifact, runtime, and positioning boundaries behind this
summary. No performance ranking is claimed without a shared reproducible
benchmark.

## Quick start (planned CLI)

```python
# hello.py
def fib(n: int) -> int:
    return n if n < 2 else fib(n - 1) + fib(n - 2)

def main() -> None:
    print(fib(35))
```

```console
$ pycc build hello.py -o hello
$ ./hello
9227465
```

Type errors are compile errors, Rust-style:

```
error[T0021]: argument 1 of `fib` expects `int`, got `str`
 --> hello.py:5:15
  |
5 |     print(fib("35"))
  |               ^^^^ expected `int`
  = help: did you mean `int("35")`?
```

## Pre-commit (experimental)

The repository publishes a `pycc-check` hook for the
[pre-commit](https://pre-commit.com/) framework. Pin it to a pycc release tag
or commit:

```yaml
repos:
  - repo: https://github.com/rotnov/pycc
    rev: <release-tag-or-commit>
    hooks:
      - id: pycc-check
```

The hook passes staged Python files to serial frontend-only `pycc check`
batches and never modifies them. At most one hook process runs at a time;
pre-commit may split a large path set to respect platform command-line limits.
This is currently a pre-alpha integration:
`pycc check` recognizes only the implemented v0.1 language slice, and
pre-commit's first `language: rust` installation builds the existing complete
`pycc` package, so LLVM 22.1.1 is still required even though checking does not
run code generation. See [`docs/DISTRIBUTION.md`](./docs/DISTRIBUTION.md) for
the exact contract and limitations.

## How it works

```
.py source ──► parser (Python 3.14 grammar) ──► strict type checker ──► typed IR ──► LLVM ──► native binary
                                                                                       │
                                                                          minimal runtime (str, list, dict, GC)
```

Design principles:

- **Standard Python only.** No pycc-specific keywords or dialect. The v1.0 target grammar and semantics are CPython 3.14 (t-strings, deferred annotations per PEP 649/749, pattern matching, PEP 695 generics); later standard Python levels use separate versioned gates.
- **Types are the contract.** Public functions must be annotated; annotations are verified, then used for static dispatch and unboxed native representations. No interpreter loop in the output.
- **Fast above all.** Compiler in Rust 1.97+, zero-copy parsing, per-module parallel compilation, incremental caching. Goal: frontend + type check of a mid-size project in well under a second — compiling should feel like `ruff`, not like `webpack`.
- **Ownership under the hood.** Rust-style ownership and escape analysis *inferred* from standard Python — no new syntax. Locals that don't escape live on the stack, values with a single owner are moved instead of shared, reference counting only where sharing is proven. Goal: predictable memory, no tracing-GC pauses.
- **No GIL in output.** Compiled binaries have no interpreter and no GIL; `threading` maps to real OS threads (roadmap).

## What won't compile (by design)

`eval` / `exec`, runtime monkey-patching, dynamic attribute injection, untyped public APIs. Dynamic Python is great — but pycc targets the statically-typed subset you already write in production code. Every rejected construct gets a clear diagnostic, and that diagnostic is itself a tested guarantee (see below).

Semantics follow the selected supported CPython language level wherever they
are statically expressible; v1.0 selects CPython 3.14. Every deliberate
deviation will be documented in `docs/semantics.md`.

## Testing strategy

The complete internal test architecture has seven layers, defined in
[`docs/TESTING.md`](./docs/TESTING.md). Three public compatibility mechanisms
are especially important:

1. **Conformance suite.** Every language standard pycc supports maps to a PEP, and every PEP has its own test in `tests/conformance/`. Each supported language level compiles and runs its cumulative fixture set, then compares output with that level's pinned CPython oracle. The v1 track uses CPython 3.14. Unsupported-by-design features get *negative* tests asserting the exact compile error. The full matrix: [`docs/PYTHON_STANDARDS.md`](./docs/PYTHON_STANDARDS.md).
2. **Real-world corpus.** CI compiles well-typed open-source projects (`black`, `packaging`, `attrs`, `mypy`, ...) and runs their own test suites against the compiled artifacts — the same way ruff validates against a real-repo ecosystem. Pass rate per project is tracked release to release.
3. **Ecosystem bot.** A scheduled job picks popular PyPI/GitHub projects, compiles them with the latest pycc, and auto-files a structured issue *in this repo* for every new incompatibility: minimized repro, diagnostic, PEP reference. When pycc uncovers a genuine type bug in an upstream project, we report it upstream — manually and curated, never bot-spammed.

## Roadmap

- [ ] **MVP:** functions, `int`/`float`/`str`/`bool`, arithmetic, `if`/`while`/`for`, `print` → Linux/macOS binary
- [ ] Collections (`list`/`dict`/`tuple`/`set`) + generics (PEP 585/695)
- [ ] Classes, dataclasses, protocols, pattern matching
- [ ] Modules, imports, multi-file projects, incremental builds
- [ ] Core stdlib subset (typed, compiled)
- [ ] Generators, comprehensions, t-strings (PEP 750)
- [ ] Exceptions incl. `except*` groups
- [ ] Ownership & escape analysis: move semantics, stack allocation, RC elision
- [ ] GIL-free threads
- [ ] Ecosystem bot: compile top PyPI packages nightly, auto-file incompatibility issues
- [ ] CPython interop escape hatch (call untyped third-party libs through an embedded interpreter)

Cross-platform is not a roadmap item — Linux, macOS and Windows (x64 + arm64) are CI-gated from v0.1, including `pycc build --target` cross-compilation. Full spec: [`docs/SPEC.md`](./docs/SPEC.md).

## Building from source

Requires Rust 1.97+ (`rustup update stable`) and LLVM 22.1.1.
`cargo build --release`. The compiler itself has no Python dependency.

## License

MIT
