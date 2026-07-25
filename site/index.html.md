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

pycc is pre-alpha and is not ready for production. The repository contains a
first end-to-end vertical slice through parsing, intermediate representations,
LLVM code generation, linking, and execution. Broad Python language coverage,
diagnostics, runtime behavior, and production readiness remain roadmap work.

Examples and CLI commands on the website are design targets unless explicitly
identified as implemented behavior.

## Evidence pages

- [Current implementation status](https://rotnov.github.io/pycc/status/):
  working language and CLI surface, enforced CI and coverage, missing v0.1
  behavior, and the next planned delivery slice.
- [Compiler architecture](https://rotnov.github.io/pycc/architecture/):
  implemented Rust and LLVM stages, current crate boundaries, planned
  typed-Python pipeline, and platform model.
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
