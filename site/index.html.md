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

## Evidence hero inventory

The versioned [evidence-hero manifest](https://rotnov.github.io/pycc/evidence-heroes.json)
is the canonical machine-readable inventory. An unavailable state means the
explanatory page may exist, but its unique fixture-backed hero has not been
accepted; it never means a zero or a failed measurement.

<!-- evidence-hero: landing | landing-quick-start-v1 | native-build-output | all-Tier-1 | / -->
- **Home:** `all-Tier-1` — real source → build → native binary → stdout evidence.
<!-- evidence-hero: language | language-support-v1 | language-conformance | all-Tier-1 | /language-support/ -->
language-support-v1 — all-Tier-1: `pycc run tests/fixtures/pep_0526_var_annotations.py` (exit 0, empty stderr); `python3.14 tests/fixtures/pep_0526_var_annotations.py` (exit 0, empty stderr). Both stdout streams: `15\n`. One passing fixture does not establish full Python 3.14 compatibility. all-Tier-1 means platform coverage for this fixture, not whole-language acceptance. The displayed pycc run uses debug, not release. [Exact source, snapshots, SHA-256 identities, toolchain and five jobs](https://rotnov.github.io/pycc/language-support/).
<!-- evidence-hero: diagnostics | diagnostics-v1 | compiler-diagnostic | all-Tier-1 | /diagnostics/ -->
diagnostics-v1 — all-Tier-1: `pycc check tests/diagnostics/d0021_range_argument_type.py` (exit 1, empty stderr); `pycc check tests/diagnostics/d0021_range_argument_type.py --error-format json` (exit 1, empty stderr). Human stdout: T0021; JSON stdout includes ``"help":["pass an `int` value"]``. Human output has no help line. The type checker uses a placeholder 1:1, zero-length span; the caret does not precisely highlight the argument. Exact serialization for this fixture does not establish diagnostic-class correctness for all inputs. [Exact source, snapshots, SHA-256 identities, toolchain and five jobs](https://rotnov.github.io/pycc/diagnostics/).
<!-- evidence-hero: performance | performance-v1 | benchmark | unavailable | /performance/ -->
- **Performance:** `unavailable` — reproducible benchmark evidence is tracked in [#567](https://github.com/rotnov/pycc/issues/567).
<!-- evidence-hero: architecture | architecture-trace-v1 | compiler-pipeline-trace | unavailable | /architecture/ -->
- **Architecture:** `unavailable` — the explanatory page is live; its fixture-derived trace is tracked in [#566](https://github.com/rotnov/pycc/issues/566).
<!-- evidence-hero: status | status-snapshot-v1 | required-checks-snapshot | unavailable | /status/ -->
- **Status:** `unavailable` — the explanatory page is live; its commit-bound checks snapshot is tracked in [#566](https://github.com/rotnov/pycc/issues/566).
<!-- evidence-hero: comparison | comparison-sources-v1 | source-backed-comparison | unavailable | /python-aot-compilers/ -->
- **Comparison:** `unavailable` — the source-backed table is live; its shared commit-bound hero remains tracked in [#563](https://github.com/rotnov/pycc/issues/563).
<!-- evidence-hero: provenance | ai-provenance-v1 | authorship-attestation | unavailable | /ai-native/ -->
- **AI provenance:** `unavailable` — the policy page is live; its sanitized immutable hero record is tracked in [#217](https://github.com/rotnov/pycc/issues/217).

### Accepted landing evidence

- Evidence ID/kind/state: `landing-quick-start-v1` / `native-build-output` / `all-Tier-1`.
- Fixture: [`tests/fixtures/quick_start.py`](https://github.com/rotnov/pycc/blob/8324332d5ea713bd8a56f4d08bf7e0120757d66b/tests/fixtures/quick_start.py).
- Test: [`tests/quick_start.rs::quick_start_fixture_builds_and_prints_the_documented_sequence`](https://github.com/rotnov/pycc/blob/8324332d5ea713bd8a56f4d08bf7e0120757d66b/tests/quick_start.rs).
- Commands: `pycc build hello.py -o hello`, then `./hello`; debug profile, no extra compiler flags.
- Exact stdout: [`tests/fixtures/quick_start.expected.txt`](https://github.com/rotnov/pycc/blob/8324332d5ea713bd8a56f4d08bf7e0120757d66b/tests/fixtures/quick_start.expected.txt).

```text
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

- Canonical LF SHA-256: source `09f7f6732d6837a0e7a91298eea549b0bdcba77d4908839ce1878955f4a0f043`; test `cb7af27aab96bf468fea7ee973e00ae9d11c0054187e09f2e94e4d8b9199a766`; stdout `668cec0e1f32369a43d6f74b1e795fe37f1e46bdb8eb0d7712b35c7e95e173e6`.
- Revision/attestation: commit [`8324332`](https://github.com/rotnov/pycc/commit/8324332d5ea713bd8a56f4d08bf7e0120757d66b), [CI run 33198103510](https://github.com/rotnov/pycc/actions/runs/33198103510).
- Environment: all five Tier-1 targets (Linux x64/arm64, macOS x64/arm64, Windows x64), CPython 3.14.7, Rust 1.97.1, LLVM 22.
- Tier-1 jobs: [`macos-14 · aarch64-apple-darwin`](https://github.com/rotnov/pycc/actions/runs/33198103510/job/98940383105); [`macos-15-intel · x86_64-apple-darwin`](https://github.com/rotnov/pycc/actions/runs/33198103510/job/98940383070); [`ubuntu-latest · x86_64-unknown-linux-gnu`](https://github.com/rotnov/pycc/actions/runs/33198103510/job/98940382966); [`ubuntu-24.04-arm · aarch64-unknown-linux-gnu`](https://github.com/rotnov/pycc/actions/runs/33198103510/job/98940383014); [`windows-latest · x86_64-pc-windows-msvc`](https://github.com/rotnov/pycc/actions/runs/33198103510/job/98940382973).
- Limitations: This compiles only the implemented v0.1 subset: statement-form `if` and `while`, recursive calls, and `print`. It does not prove support for classes, exceptions, generators, or imports.

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

v0.1 through v0.3 are met and released as `v0.3.0`. The compiler is not
ready for production.

pycc is pre-alpha and is not ready for production. The repository contains an
implemented v0.1 frontend and native backend with documented gaps.
`pycc check` now parses and type-checks the v0.1 frontend: arithmetic,
comparisons, assignments, function calls and returns, recursion, control flow,
primitive values, `range`, and basic f-strings. It enforces public annotations,
rejects `Any`, infers private-helper signatures, and renders human or JSON
diagnostics. `pycc build` and `pycc run` compile that implemented surface through MIR,
LLVM, the host linker, and the native runtime. v0.1's acceptance criteria
are now met: `fib` and `mandelbrot-ascii` match pinned CPython output on all
five Tier-1 targets, `pycc check` clears its <75ms/1000 LOC throughput floor,
and diagnostic output matches the stable CLI specification's example.
Documented representation and lifetime gaps, the full multi-version
conformance matrix, differential fuzzing, and corpus testing remain
roadmap work.

- Strict v0.1 frontend and diagnostic snapshots
- v0.1 native backend with documented gaps
- v0.1 acceptance criteria met (conformance verified on all five Tier-1 targets)
- v0.2 acceptance criteria also met
- v0.3 acceptance criteria met and released as `v0.3.0`
- v0.4 is in progress. Cross-file project from imports have landed. Bare/submodule imports, namespace handling, broader project CLI behavior, and incremental compilation remain incomplete.

The landing page's code example uses only implemented v0.1 language features
(statement-form `if`, `while`, recursive calls, `print`) and matches the
`recursive_fibonacci_matches_the_well_known_sequence` conformance test.
Other examples and CLI commands on the website are design targets unless
explicitly identified as implemented behavior.

## Evidence pages

- [Language support](https://rotnov.github.io/pycc/language-support/): One fixture, independently compared with CPython, with explicit limits.
- [Diagnostics](https://rotnov.github.io/pycc/diagnostics/): Exact human and JSON output, help and span boundaries.

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
