# pycc — ahead-of-time compiler for typed Python

[![CI](https://github.com/rotnov/pycc/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/rotnov/pycc/actions/workflows/ci.yml)
[![test coverage: 100%](https://img.shields.io/badge/test%20coverage-100%25-brightgreen)](./docs/TESTING.md)

**A strict ahead-of-time (AOT) compiler that turns type-annotated Python 3.14—the v1.0 language target—into autonomous native deployment artifacts. Like `gcc`, but for Python.**

`pycc` is being built to take standard Python 3.14 source code, enforce every type annotation at compile time, and produce a fast, autonomous artifact. Native and `--pure` builds are standalone binaries without CPython; planned permitted interop dependencies are bundled with a pinned CPython runtime instead of requiring an installed interpreter or venv. There is no new language to learn — the design contract is that valid typed Python compiles and incorrect types do not.

Written in Rust (1.97+). Built to be extremely fast — both the compiler itself and the binaries it produces.

> Status: pre-alpha. `pycc check` now parses and type-checks the v0.1
> frontend surface, enforcing public annotations, rejecting `Any`, inferring
> private-helper signatures, and rendering human or JSON diagnostics.
> `pycc build` and `pycc run` compile that implemented v0.1 surface through
> MIR, LLVM, the host linker, and the native runtime. The compiler remains
> pre-alpha: documented representation and lifetime gaps are still roadmap
> work, but v0.1's own acceptance criteria are now met -- `fib` and
> `mandelbrot-ascii` match pinned CPython output on all five Tier-1 targets,
> `pycc check` clears its <75ms/1000 LOC throughput floor, and diagnostic
> output matches [`docs/CLI_SPEC.md`](./docs/CLI_SPEC.md)'s example.
> v0.2's own acceptance criteria are now also met: a hand-authored
> `list`/`dict`/`set`/`tuple` + comprehensions/slicing/methods/generics corpus
> matches CPython output on all five Tier-1 targets, `--release` clears its
> nbody-vs-CPython speedup floor on every target, and conformance covers ≥15
> [`docs/PYTHON_STANDARDS.md`](./docs/PYTHON_STANDARDS.md) matrix rows
> (17 distinct PEPs); see [`docs/ROADMAP.md`](./docs/ROADMAP.md) for the full,
> per-bullet evidence.
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

**pycc is an experiment in autonomous, AI-native software development: the project writes itself.** AI agents create its specifications, code, tests, documentation, reviews, and release automation. Humans set goals and constraints; not a single line of project code is handwritten by a human. This is a maintainer attestation supported by the public Git history, pull requests, agent instructions, review artifacts, and session handoff logs — not a fact provable from Git author metadata alone, which identifies the accountable human maintainer rather than the code generator.

“AI-native” describes the authorship workflow. pycc is an AOT compiler for
typed Python, not an AI or machine-learning compiler.

## Why

Python compiler projects make different tradeoffs between compatibility,
static semantics, output artifacts, and runtime dependencies. pycc is testing
one particular combination; this is a model comparison, not a benchmark or a
claim that pycc is ready to replace released tools:

| | Enforces types at compile time | Native executable without CPython | Standard Python input |
|---|---|---|---|
| **pycc (v1.0 design target)** | ✅ hard compile error | ✅ native/`--pure`; CPython bundled only for permitted interop | ✅ CPython 3.14 target |
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

## Quick start

```console
$ cat hello.py
def fib(n: int) -> int:
    if n < 2:
        return n
    return fib(n - 1) + fib(n - 2)

i = 0
while i < 11:
    print(fib(i))
    i = i + 1
$ pycc check hello.py
$ pycc build hello.py -o hello
$ ./hello
0
1
1
2
3
5
8
13
21
34
55
```

`pycc check` parses and type-checks the file, enforcing public annotations
and rejecting `Any`. No output means no errors. Type errors are compile
errors, Rust-style. For example, appending `print(fib("5"))` to the file
above and re-running `pycc check hello.py` prints:

<!-- #197: generated from tests/diagnostics/quick_start_type_error.expected.txt -->
```
error[T0021]: argument 1 of `fib` expects `int`, got `str`
 --> hello.py:1:1
  |
1 | def fib(n: int) -> int:
  | ^ argument 1 of `fib` expects `int`, got `str`
```

That block is the compiler's real output, not an illustration: it is
generated from `tests/diagnostics/quick_start_type_error.expected.txt`,
which `tests/diagnostics_test.rs` checks against the real binary's live
output on every test run, with only the file path substituted. The test
verifies the fixture rather than rewriting it: a renderer change fails
the test, and the fixture is then updated by hand from the new output.
Every `T0xxx` diagnostic's span
is currently the `Span::new(0, 0)` placeholder (`line 1, column 1`, a
one-character caret) regardless of where the real error is, and the caret
label always repeats the diagnostic's full message rather than an
independent short label -- both are current, real behavior, not an
aspirational target (D-043).

`pycc build` and `pycc run` compile the implemented v0.1 surface through
MIR, LLVM, and the native runtime — see the status block above and
[`docs/CLI_SPEC.md`](./docs/CLI_SPEC.md) for the full command reference.
The quick-start source above uses only implemented v0.1 language features
(statement-form `if`, `while`, recursive calls, `print`) and matches the
`recursive_fibonacci_matches_the_well_known_sequence` conformance test in
`tests/slice1_codegen_depth.rs`.

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

```text
.py source ──► parser ──► strict type checker ──► typed IR ──► LLVM ──► native binary + minimal pycc runtime
                                                                │
                                                                └─ planned permitted interop: autonomous bundle
                                                                   + pinned CPython/package/native-library closure
```

Design principles:

- **Standard Python only.** No pycc-specific keywords or dialect. The v1.0 target grammar and semantics are CPython 3.14 (t-strings, deferred annotations per PEP 649/749, pattern matching, PEP 695 generics); later standard Python levels use separate versioned gates.
- **Types are the contract.** Public functions must be annotated; annotations
  are verified, then used for static dispatch and unboxed native
  representations. Native pycc execution has no interpreter loop; only the
  planned CPython-backed boundary executes package operations in its bundled
  interpreter (D-128).
- **Fast above all.** Compiler in Rust 1.97+, zero-copy parsing. Goal: frontend + type check of a mid-size project in well under a second — compiling should feel like `ruff`, not like `webpack`. Per-module parallel compilation and incremental caching are planned for v0.4.
- **Ownership under the hood (planned v0.5).** Rust-style ownership and escape analysis *inferred* from standard Python — no new syntax. Locals that don't escape live on the stack, values with a single owner are moved instead of shared, reference counting only where sharing is proven. Goal: predictable memory, no tracing-GC pauses.
- **No pycc-wide GIL (planned v0.6).** Native pycc execution has no interpreter or GIL, and
  `threading` maps to real OS threads. The planned embedded CPython
  boundary retains CPython's own GIL only while it executes CPython-backed
  operations (D-128).

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
2. **Real-world corpus (planned).** CI would compile well-typed open-source projects (`black`, `packaging`, `attrs`, `mypy`, ...) and run their own test suites against the compiled artifacts — the same way ruff validates against a real-repo ecosystem. Pass rate per project would be tracked release to release. This is not yet implemented; no corpus workflow, pinned corpus inputs, or pass-rate dashboard exists on current `main`.
3. **Ecosystem bot (planned).** A scheduled job would pick popular PyPI/GitHub projects, compile them with the latest pycc, and auto-file a structured issue *in this repo* for every new incompatibility: minimized repro, diagnostic, PEP reference. When pycc uncovers a genuine type bug in an upstream project, we report it upstream — manually and curated, never bot-spammed. This is not yet implemented; no scheduled bot workflow exists on current `main`.

## Roadmap

**Current status (pre-alpha):** v0.1 and v0.2 acceptance criteria are both
met. For v0.1: `fib` and `mandelbrot-ascii` match pinned CPython output on
all five Tier-1 targets (Linux x64/arm64, macOS x64/arm64, Windows x64),
`pycc check` clears its <75ms/1000 LOC throughput floor, diagnostic output
matches the CLI specification, the five-target native CI matrix and one
cross-host compilation path are live, the 100% line/region coverage gate is
required and green, and the README coverage badge is bound to the enforced
CI coverage thresholds. v0.2's container/generics corpus and `--release`
speedup floor are likewise met on all five Tier-1 targets. v0.3 (classes,
dataclasses, protocols, pattern matching, exceptions) is the current
delivery milestone, in progress. Later capabilities — modules/imports,
stdlib subset, generators, ownership & escape analysis, GIL-free threads,
the ecosystem bot, and transparent CPython interop — remain planned. The
compiler is still pre-alpha: documented representation and lifetime gaps
are roadmap work, not production readiness.

The complete, commit-relative milestone status — every acceptance bullet,
its roadmap-evidence identifier, and the per-target evidence — lives in
[`docs/ROADMAP.md`](./docs/ROADMAP.md), the sole milestone status owner.
This section is a validated projection of that document, verified by
`scripts/check_readme_milestone_projection.rb` in `pages.yml`; it is not
an independently maintained checklist. Cross-platform is not a roadmap
item — Linux, macOS and Windows (x64 + arm64) are CI-gated from v0.1,
including `pycc build --target` cross-compilation. Full spec:
[`docs/SPEC.md`](./docs/SPEC.md).

## Building from source

Requires Rust 1.97+ (`rustup update stable`) and LLVM 22.1.1.
`cargo build --release`. The compiler itself has no Python dependency.

## License

MIT

## Citation

If you reference pycc in your work, please cite this repository. A
machine-readable [`CITATION.cff`](./CITATION.cff) (CFF 1.2.0) is provided
in the repository root; GitHub renders a "Cite this repository" panel
from it. The citation identity uses the exact `rotnov/pycc` repository
URL to avoid collision with unrelated same-named projects. Authorship is
attributed to a collective AI-agents entity that truthfully describes the
autonomous development model; the human maintainer is not listed as a
software author. Release-bound fields (version, date-released, DOI) are
omitted until the release lifecycle becomes coherent.
