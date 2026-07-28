# pycc — AOT compiler for typed Python to native binaries

> pycc is a fully AI-created, human-managed pre-alpha project building an
> ahead-of-time compiler for typed, standard Python 3.14 with Rust and LLVM.

## What pycc is

pycc is an open-source compiler project whose design target is to check Python
type annotations at compile time and emit a standalone native executable. It
uses standard Python syntax rather than introducing a Python-like dialect.

The intended compiler pipeline is:

1. Parse Python 3.14 source while preserving source spans.
2. Resolve names and enforce the typed contract in high-level IR.
3. Lower checked programs to typed mid-level IR.
4. Generate LLVM IR, emit object code, and link a native binary.

## Development model

pycc is an experiment in autonomous software development. AI agents create the
specifications, every line of project code, tests, documentation, reviews,
release automation, and process rules. A human only manages goals, constraints,
priorities, product decisions, and the definition of success.

No project code is handwritten by a human.

## Honest status

pycc is pre-alpha and is not ready for production. The repository contains an
implemented v0.1 frontend and native backend with documented gaps.
`pycc check` now parses and type-checks the v0.1 frontend: arithmetic,
comparisons, assignments, function calls and returns, recursion, control flow,
primitive values, `range`, and basic f-strings. It enforces public annotations,
rejects `Any`, infers private-helper signatures, and renders human or JSON
diagnostics. `pycc build` and `pycc run` compile that implemented surface through MIR,
LLVM, the host linker, and the native runtime. v0.1's own acceptance criteria
are now met: `fib` and `mandelbrot-ascii` match pinned CPython output on all
five Tier-1 targets, `pycc check` clears its <50ms/1000 LOC throughput floor,
and diagnostic output matches the stable CLI specification's example.
Documented representation and lifetime gaps, the full multi-version
conformance matrix, differential fuzzing, corpus testing, and production
readiness remain roadmap work.

Examples and CLI commands on the website are design targets unless explicitly
identified as implemented behavior.

## Evidence pages

- [Current implementation status](https://rotnov.github.io/pycc/status/):
  working language and CLI surface, enforced CI and coverage, missing v0.1
  behavior, and the next planned delivery slice.
- [Compiler architecture](https://rotnov.github.io/pycc/architecture/):
  implemented Rust and LLVM stages, current crate boundaries, planned
  typed-Python pipeline, and platform model.
- [Python AOT compiler comparison](https://rotnov.github.io/pycc/python-aot-compilers/):
  source-backed differences among pycc, LPython, Codon, Nuitka, mypyc, and
  Cython, including output artifacts, runtime models, and current positioning.
- [AI-native experiment](https://rotnov.github.io/pycc/ai-native/):
  who creates each project artifact, what the human manages, how the agent
  development loop works, and where its public audit evidence lives.

## Authoritative resources

- [Canonical website](https://rotnov.github.io/pycc/)
- [Source repository](https://github.com/rotnov/pycc)
- [Specification index](https://github.com/rotnov/pycc/blob/main/docs/SPEC.md)
- [Compiler architecture](https://github.com/rotnov/pycc/blob/main/docs/ARCHITECTURE.md)
- [Python standards matrix](https://github.com/rotnov/pycc/blob/main/docs/PYTHON_STANDARDS.md)
- [Roadmap](https://github.com/rotnov/pycc/blob/main/docs/ROADMAP.md)
- [MIT license](https://github.com/rotnov/pycc/blob/main/LICENSE)
