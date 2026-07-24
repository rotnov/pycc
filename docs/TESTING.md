# pycc Testing Specification

Testing *is* the spec enforcement mechanism: [PYTHON_STANDARDS.md](./PYTHON_STANDARDS.md) defines what must work; this file defines how we prove it — on every Tier-1 platform, every commit.

## Layers

| Layer | Location | What it proves |
|---|---|---|
| 1. Unit (Rust) | per-crate `#[cfg(test)]` | lexer/parser/checker/MIR internals |
| 2. Conformance | `tests/conformance/pyXY/` | one test per PEP: compile with pycc, run, `stdout == CPython 3.14 stdout` |
| 3. Diagnostics | `tests/diagnostics/` | rejected constructs fail with the exact code + span (insta-style snapshots) |
| 4. Differential fuzzing | `tests/fuzz/` | generated typed-Python programs: pycc binary output ≡ CPython output; crashes/mismatches auto-minimized |
| 5. Runtime property tests | `pycc_rt` proptest | str/list/dict/RC/cycle-collector invariants |
| 6. Corpus (OSS projects) | nightly CI | real code compiles and its own test suite passes |
| 7. Benchmarks | `benches/` + pyperformance subset | compiler speed + generated-code speed |

## Conformance harness (`pycc_testkit`)

- Each test = single `.py` file, header comment: PEP, category, min pycc milestone.
- Runner: compile (`--debug` and `--release` both, once `--release` exists — see below) → execute → diff vs CPython 3.14 reference output (recorded, pinned CPython version; re-recorded on CPython patch bumps).
- A PEP flips to ✅ in PYTHON_STANDARDS.md **only** when green on all Tier-1 targets in both profiles. The matrix file is updated by CI, not by hand.
- **v0.1 exception:** `--release`/LTO doesn't exist until v0.2 (see ROADMAP.md), so the "both profiles" rule only binds from v0.2 on. Every v0.1 PEP/feature flips to ✅ on `--debug` alone; nothing in v0.1 is held to a `--release` bar that has nothing to build against (see DELIVERY_PLAN.md, "Debug/release conformance").

## Differential fuzzing

Generator produces well-typed programs (type-directed generation — always compile-clean), weighted toward: arithmetic edges (overflow → bigint promotion paths), string unicode edges, collection aliasing, control-flow + exceptions, match patterns. Mismatch → auto-minimize (creduce-style) → auto-file issue with repro. Runs continuously on a dedicated runner.

## Corpus: open-source projects as integration tests

Tiers and gates in PYTHON_STANDARDS.md § Real-world corpus. Mechanics:

- Pinned commit per project; `pycc build` the package, run its pytest suite against the compiled artifact (test files themselves compiled where possible; interop fallback allowed and measured).
- Per-project dashboard: % files compiled, % tests passed, RC-elision rate, binary size, speed vs CPython on the project's own benchmarks.
- Regression vs previous release = release blocker.

## The bot

GitHub Action (`corpus-bot`):

1. Nightly: run corpus + a rotating slice of top-PyPI packages (by download count) in compile-only mode.
2. New failure → fingerprint (diagnostic code + normalized span + package) → dedupe → auto-file issue **in the pycc repo**: minimized repro, diagnostic output, PEP link, dashboard delta. Labels: `corpus`, `regression`/`gap`.
3. Fix confirmed → bot closes the issue with the passing run linked.
4. Upstream bugs pycc finds (genuine type errors in the project): bot drafts the report, human reviews and files — never automated spam.

## Benchmarks

- Compiler: `pycc check` LOC/s, cold + incremental build times; tracked per-commit (criterion + CI history), >2% regression fails PR.
- Generated code: pyperformance subset + fib/nbody/spectral-norm vs CPython 3.14, Nuitka, Codon, mypyc; published table per release. Honesty rule: publish losses too.

## Code coverage (D-014)

Distinct from the grammar-coverage gate in Meta below (which measures PEP/language-surface coverage): this is ordinary line/region coverage of pycc's own Rust source, gated on every PR from v0.1 on.

- Tool: `cargo llvm-cov` — a separately distributed cargo subcommand, **not** bundled with any rustup component. CI installs it explicitly and pinned (installer action or `cargo install cargo-llvm-cov --locked --version <pinned>`), plus the `llvm-tools-preview` rustup component it drives at runtime; a bare "install llvm-tools" fails with "no such command: llvm-cov" (caught by repo audit, issue #13). Independent of the Homebrew LLVM used by `inkwell` for codegen — versions don't need to match.
- Gate: `cargo llvm-cov --fail-under-lines 100 --fail-under-regions 100`, run in CI on at least one Tier-1 target per PR. A version-print smoke step runs before the gate so a broken/missing install fails loudly rather than silently.
- Test code itself (`tests/`, `*_tests.rs`, `tests.rs`) is excluded from the denominator automatically — the gate measures product code exercised by tests, not tests covering themselves.
- Exemptions are whole-file only, via `--ignore-filename-regex` (no per-function opt-out exists on stable Rust — see D-014). Each exemption needs a named entry here:

  | File pattern | Reason |
  |---|---|
  | *(none yet)* | — |

  An uncovered file with no entry in this table is a review-blocking finding, not a gap to wave through.

- **Practical notes on what actually shows up as a coverage gap** (learned building the first few v0.1 crates — verified directly against `cargo llvm-cov`'s HTML report, not assumed):
  - A hand-written `match { expected => ..., _ => panic!("...") }` — in test code or production code — creates its *own* region for the `_`/catch-all arm. If nothing ever exercises that arm, it's a gap, even though the arm is real and reachable. In tests, prefer `#[derive(Debug, PartialEq)]` on the type under test plus `assert_eq!(actual, expected)` over a manual match-and-panic assertion — it needs no catch-all arm at all.
  - **`.expect()`/`.unwrap()` do *not* have this problem**: their internal panic branch lives inside libcore/libstd, outside the calling crate's instrumented regions, so a call that always succeeds in every test still reads as 100% covered. This is the right choice for an operation that's genuinely infallible given the caller's own invariants (see `pycc_codegen::compile_to_object`'s five `.expect()`s on native-target/IR-verification operations that no input to that function can make fail).
  - **A closure passed to a combinator (`.map_err(|e| ...)`, `.and_then(...)`, etc.) is tracked as its own function/region and *does* need to actually run** — if the `Result`/`Option` it's attached to never takes that branch across the whole test suite, the closure body shows as a missed region even though the call site's own line is "covered." Reserve `Result`-returning `.map_err(...)` for failure modes a test can actually trigger (e.g. a bad output path); use `.expect(...)` for the rest instead of threading a `Result` no real input can produce.
  - **A function generic over `impl Fn(..)` (dependency-injection for testability — e.g. passing in a fake filesystem-existence check) gets monomorphized once per distinct closure type**, and each monomorphized copy is tracked *separately*: a copy that's only ever called with an always-true fake never executes that copy's error branches, and that reads as a real gap even though the *production* closure (or a different test's fake) exercises them. Fix: take a plain `fn(..) -> ..` pointer instead of `impl Fn(..)` when every caller's closure is non-capturing (as is typical for this kind of fake) — one concrete function pointer type means one compiled body, so coverage from every caller (production and every test) accumulates on the same counters. Only reach for `impl Fn`/`Box<dyn Fn>` when a caller genuinely needs to capture state; don't default to it for simple fakes.

## Meta

Every bug that reaches `main` gets a permanent regression test named after the issue (`tests/regress/issue_1234.py`). Coverage gate: conformance suite must touch 100% of implemented grammar productions (grammar-coverage instrumentation in the parser).
