# pycc — ahead-of-time compiler for typed Python

**A strict ahead-of-time (AOT) compiler that turns type-annotated Python 3.14 into standalone native binaries. Like `gcc`, but for Python.**

`pycc` is being built to take standard Python 3.14 source code, enforce every type annotation at compile time, and produce a fast, standalone native binary. No interpreter, no venv, no new language to learn — the design contract is that valid typed Python compiles and incorrect types do not.

Written in Rust (1.97+). Built to be extremely fast — both the compiler itself and the binaries it produces.

> Status: early design / pre-alpha. The supported-language roadmap lives in [`docs/PYTHON_STANDARDS.md`](./docs/PYTHON_STANDARDS.md).

[Project website](https://rotnov.github.io/pycc/) · [Specification](./docs/SPEC.md) · [Roadmap](./docs/ROADMAP.md)

## Why

Python tooling solves every piece of this separately, but no tool does the whole job the way `rustc` or `gcc` does:

| | Enforces types at compile time | Standalone binary | Plain Python syntax |
|---|---|---|---|
| **pycc (design target)** | ✅ hard compile error | ✅ single executable | ✅ standard CPython 3.14 |
| Codon | ✅ (via inference) | ✅ | ⚠️ Python-like dialect, own stdlib |
| Nuitka | ❌ | ✅ | ✅ |
| mypyc | ✅ | ❌ C extension, needs CPython | ✅ |
| Cython | ⚠️ optional | ❌ needs CPython | ⚠️ own dialect |
| mypy / pyright | ✅ | ❌ checker only | ✅ |

**pycc = strict types + native binaries + the Python you already write.**

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

## How it works

```
.py source ──► parser (Python 3.14 grammar) ──► strict type checker ──► typed IR ──► LLVM ──► native binary
                                                                                       │
                                                                          minimal runtime (str, list, dict, GC)
```

Design principles:

- **Standard Python only.** No new keywords, no dialect. Target grammar and semantics: CPython 3.14 (t-strings, deferred annotations per PEP 649/749, pattern matching, PEP 695 generics).
- **Types are the contract.** Public functions must be annotated; annotations are verified, then used for static dispatch and unboxed native representations. No interpreter loop in the output.
- **Fast above all.** Compiler in Rust 1.97+, zero-copy parsing, per-module parallel compilation, incremental caching. Goal: frontend + type check of a mid-size project in well under a second — compiling should feel like `ruff`, not like `webpack`.
- **Ownership under the hood.** Rust-style ownership and escape analysis *inferred* from standard Python — no new syntax. Locals that don't escape live on the stack, values with a single owner are moved instead of shared, reference counting only where sharing is proven. Goal: predictable memory, no tracing-GC pauses.
- **No GIL in output.** Compiled binaries have no interpreter and no GIL; `threading` maps to real OS threads (roadmap).

## What won't compile (by design)

`eval` / `exec`, runtime monkey-patching, dynamic attribute injection, untyped public APIs. Dynamic Python is great — but pycc targets the statically-typed subset you already write in production code. Every rejected construct gets a clear diagnostic, and that diagnostic is itself a tested guarantee (see below).

Semantics follow CPython 3.14 wherever they are statically expressible; every deliberate deviation will be documented in `docs/semantics.md`.

## Testing strategy

Two layers, both required for every release:

1. **Conformance suite.** Every language standard pycc supports maps to a PEP, and every PEP has its own test in `tests/conformance/` — compile, run, compare against CPython 3.14 output. Unsupported-by-design features get *negative* tests asserting the exact compile error. The full matrix: [`docs/PYTHON_STANDARDS.md`](./docs/PYTHON_STANDARDS.md).
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

Requires Rust 1.97+ (`rustup update stable`) and LLVM. `cargo build --release`. That's it — the compiler has no Python dependency.

## License

MIT
