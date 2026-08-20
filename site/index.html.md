# pycc — AOT compiler for typed Python to native binaries

> pycc is a pre-alpha ahead-of-time compiler for typed, standard Python 3.14
> with an implemented native-binary path through Rust and LLVM. AI agents
> create it, and a human manages it.

Typed Python in. Autonomous artifacts out.

pycc is an open-source ahead-of-time compiler project for standard Python
3.14. Its design contract is to check annotations at compile time, then emit
a fast, autonomous deployment artifact—without inventing a new language.
Native and pure builds emit standalone executables; planned permitted CPython
interop emits a self-contained bundle with its pinned runtime. AI agents
create the entire project; a human only manages direction, priorities, and
constraints.

    $ pycc build hello.py -o hello

Compile typed Python to a native binary · pre-alpha.

## Design contract

- **3.14** — Standard Python target
- **Strict** — Compile-time types
- **Native** — Standalone output
- **No dialect** — Python syntax stays Python

## Development model

Built entirely by AI. Managed by a human.

pycc is an experiment in autonomous software development. AI agents create the
specifications, every line of project code, tests, documentation, reviews,
release automation, and process rules. A human only manages goals, constraints,
priorities, product decisions, and the definition of success.

No project code is handwritten by a human. This is a maintainer attestation
supported by the public Git history, pull requests, agent instructions,
review artifacts, and session handoff logs — not a fact provable from Git
author metadata alone, which identifies the accountable human maintainer
rather than the code generator.

"AI-native" describes the authorship and management model. pycc is not an AI
or machine-learning compiler.

## Why pycc

One tool should finish the whole job.

Type checkers stop before runtime. Packagers bundle an interpreter.
Typed-subset compilers trade compatibility breadth for static optimization.
Python-like compilers ask you to adopt a dialect.

pycc's design target combines hard type errors, standard Python syntax,
native execution, and autonomous permitted-interoperability bundles in one
compiler pipeline. Annotations are the contract—not optional hints.

## The intended position

Design targets, not release claims.

| Tool    | Static model     | Output artifact   | Language contract          |
| ------- | ---------------- | ----------------- | -------------------------- |
| pycc    | Hard annotations | Standalone target | CPython 3.14 target        |
| LPython | Typed subset     | AOT executable    | CPython-compatible subset  |
| Codon   | Static language  | Native code       | Python-like language       |
| Nuitka  | None required    | Executable        | Standard Python            |
| mypyc   | Strict subset    | C extension       | Standard Python subset     |

This table describes the intended position. The source-backed Python AOT
compiler comparison separates current output models and pycc's pre-alpha
status.

## Compiler pipeline

From source file to machine code.

1. Parse — CPython 3.14 grammar, source spans preserved.
2. Check — Resolve names and enforce the typed contract.
3. Lower — Build typed IR for optimization and ownership analysis.
4. Compile — Emit the native binary, or package the planned pinned CPython
   closure when permitted interop requires it.

## Honest status

The v0.1 and v0.2 frontend and native backend exist. The compiler is not
ready for production.

pycc is pre-alpha and is not ready for production. The repository contains an
implemented v0.1 frontend and native backend with documented gaps.
`pycc check` now parses and type-checks the v0.1 frontend: arithmetic,
comparisons, assignments, function calls and returns, recursion, control flow,
primitive values, `range`, and basic f-strings. It enforces public annotations,
rejects `Any`, infers private-helper signatures, and renders human or JSON
diagnostics. `pycc build` and `pycc run` compile that implemented surface through MIR,
LLVM, the host linker, and the native runtime. v0.1's own acceptance criteria
are now met: `fib` and `mandelbrot-ascii` match pinned CPython output on all
five Tier-1 targets, `pycc check` clears its <75ms/1000 LOC throughput floor,
and diagnostic output matches the stable CLI specification's example.
Documented representation and lifetime gaps, the full multi-version
conformance matrix, differential fuzzing, corpus testing, and production
readiness remain roadmap work.

- Strict v0.1 frontend and diagnostic snapshots
- v0.1 native backend with documented gaps
- v0.1 acceptance criteria met (conformance verified on all five Tier-1 targets)
- v0.2 acceptance criteria also met; v0.3's class model core has landed
- The full conformance matrix and the rest of v0.3 are next

The landing page's code example uses only implemented v0.1 language features
(statement-form `if`, `while`, recursive calls, `print`) and matches the
`recursive_fibonacci_matches_the_well_known_sequence` conformance test.
Other examples and CLI commands on the website are design targets unless
explicitly identified as implemented behavior.

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

## Follow the project

Follow an AI-built Python compiler from first slice to production.

A human manages the mission. AI agents execute, test, review, and evolve the
project in public under the MIT license.

## Authoritative resources

- [Canonical website](https://rotnov.github.io/pycc/)
- [Source repository](https://github.com/rotnov/pycc)
- [Specification index](https://github.com/rotnov/pycc/blob/main/docs/SPEC.md)
- [Compiler architecture](https://github.com/rotnov/pycc/blob/main/docs/ARCHITECTURE.md)
- [Python standards matrix](https://github.com/rotnov/pycc/blob/main/docs/PYTHON_STANDARDS.md)
- [Roadmap](https://github.com/rotnov/pycc/blob/main/docs/ROADMAP.md)
- [MIT license](https://github.com/rotnov/pycc/blob/main/LICENSE)
