# pycc Testing Specification

Testing *is* the spec enforcement mechanism: [PYTHON_STANDARDS.md](./PYTHON_STANDARDS.md) defines what must work; this file defines how we prove it — on every Tier-1 platform, every commit.

## Layers

| Layer | Location | What it proves |
|---|---|---|
| 1. Unit (Rust) | per-crate `#[cfg(test)]` | lexer/parser/checker/MIR internals |
| 2. Conformance | `tests/conformance/pyXY/` | each supported language level compiles and runs its cumulative fixture set; `stdout ==` that level's pinned CPython oracle |
| 3. Diagnostics | `tests/diagnostics/` | rejected constructs fail with the exact code + span (insta-style snapshots) |
| 4. Differential fuzzing | `tests/fuzz/` | generated typed-Python programs: pycc binary output ≡ CPython output; crashes/mismatches auto-minimized |
| 5. Runtime property tests | `pycc_rt` proptest | str/list/dict/RC/cycle-collector invariants |
| 6. Corpus (OSS projects) | nightly CI | real code compiles and its own test suite passes |
| 7. Benchmarks | `benches/` + pyperformance subset | compiler speed + generated-code speed |

## Conformance harness (`pycc_testkit`)

- Each test = single `.py` file, header comment: PEP, category, min pycc milestone.
- Runner: for each supported language level, select that configuration's
  cumulative fixture range and pinned oracle → compile (`--debug` and
  `--release` both, once `--release` exists — see below) → execute → diff. The
  v1.0 Python 3.14 run covers `py30/` through `py314/` against CPython 3.14.6.
  After the v1.x adoption gate opens, the Python 3.15 run covers `py30/`
  through `py315/` against a pinned current Python 3.15 patch; the separate
  Python 3.14 compatibility run remains required. Outputs are recorded and
  re-recorded on oracle patch bumps.
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

## CI privilege policy

Every GitHub Actions workflow declares an explicit workflow-level permission
baseline. The baseline may contain only read or `none` scopes, or
`permissions: {}`; a job that needs an elevated scope must opt in at job level
and satisfy the trust-boundary rules in `AGENTS.md`.

CI runs both commands before the build:

```sh
ruby scripts/test_check_ci_permissions.rb
ruby scripts/check_ci_permissions.rb
```

The deterministic checker rejects a workflow with no top-level `permissions`
declaration, duplicate declarations, scalar shortcuts such as `read-all`, or a
top-level write/OIDC scope. For every trigger, including `workflow_call`, it
also rejects jobs with job-level write/OIDC permissions, secret references,
inherited secrets, or environment access unless they have the exact
`github.event_name == 'push' && github.ref == 'refs/heads/main'` guard. It
discovers both `.yml` and `.yaml` files under `.github/workflows/` and parses
them through Ruby's standard-library `Psych` YAML AST so quoted/spaced keys,
null values, and duplicates cannot bypass the policy. YAML merge keys and
aliases are rejected conservatively because the checker must not infer a
less-privileged expanded job than GitHub executes.
The audited workflow set must contain `workflow-policy.yml`, and that file must
match an explicitly approved SHA-256 digest in the trusted checker. This makes
deletion, renaming, trigger replacement, or an extra executable step fail
closed. Updating the anchor is intentionally staged: first add the independently
reviewed prospective digest while the old anchor remains, then change the
anchor in a later pull request, and remove the retired digest afterward.

The regular PR job runs this checker for fast feedback only; pull-request code
can change its own workflow. The authoritative `Workflow policy` workflow uses
`pull_request_target` on every pull request, checks out the trusted base commit,
downloads only the head revision's workflow YAML through the read-only GitHub
API, and treats it as data. It never checks out or executes pull-request code,
so the check can remain required without path-filtered runs getting stuck as
pending. Its checkout uses `github.sha`, which `pull_request_target` defines as
the latest commit on the base branch; do not substitute the webhook payload's
potentially stale `pull_request.base.sha`. Job-level trusted-ref exceptions
remain a review boundary: reviewers
must verify the event, actor where relevant, ref, trusted commit, environment,
and every artifact/cache/output boundary, with a focused negative-event test
whenever practical.

Bootstrap exception: the pull request that first adds `Workflow policy` cannot
run that workflow from the base revision because it does not exist there yet.
That one change requires the regular checker, `actionlint`, independent deep
review, and manual inspection of the pinned action SHAs before merge.

The bootstrap is complete. On 2026-07-24, the first post-merge
[`pull_request_target` run](https://github.com/rotnov/pycc/actions/runs/30129743650)
checked out the trusted policy implementation from base commit
`107eccf4d6d4161c26f7257de538cad974bed913`, passed all 31 checker tests and
70 assertions, and audited all five workflow files at the triggering
[PR #35](https://github.com/rotnov/pycc/pull/35) head as non-executable data.
Branch protection is strict and currently requires `build-test-coverage` and
`audit`, bound to the GitHub Actions app. `ci-gate` (D-032) is a single
stable-named job in `ci.yml`, added in the same pull request that landed the
five-target Tier-1 matrix, that fans in every job in that workflow
(`build-test-coverage`, all four `native-build-test` Tier-1 legs,
`cross-compile-build`, `cross-compile-verify`) so branch protection can enforce
the whole matrix through one required-check name that survives matrix edits,
rather than naming each matrix leg directly (whose GitHub-generated name bakes
in the matrix values and would go stale the moment an `os`/`target` entry
changes). Branch protection's required check is switched from
`build-test-coverage` to `ci-gate` **once `ci-gate` exists on `main`** --
flipping it earlier, while other branches are still open against a `main`
without this job, would leave those PRs waiting on a required check they have
no way to satisfy. Until that switch happens, `native-build-test`,
`cross-compile-build`, and `cross-compile-verify` failing or staying pending
does not by itself block a merge -- a real, tracked gap, not a silent one.
Removing either required check, disabling strict mode, accepting an `audit`
context from another app, or (once switched) dropping a job from `ci-gate`'s
`needs:` list is a policy regression; all later policy changes are evaluated
by the trusted checker from their base revision.

## Code coverage (D-014)

Distinct from the grammar-coverage gate in Meta below (which measures PEP/language-surface coverage): this is ordinary line/region coverage of pycc's own Rust source, gated on every PR from v0.1 on.

- Tool: `cargo llvm-cov` — a separately distributed cargo subcommand, **not** bundled with any rustup component. CI installs it explicitly and pinned (installer action or `cargo install cargo-llvm-cov --locked --version <pinned>`), plus the `llvm-tools-preview` rustup component it drives at runtime; a bare "install llvm-tools" fails with "no such command: llvm-cov" (caught by repo audit, issue #13). Independent of the Homebrew LLVM used by `inkwell` for codegen — versions don't need to match.
- Gate: `cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`, run in CI on at least one Tier-1 target per PR. Run `cargo build --workspace` first: the slice-0 end-to-end tests link the normal debug build of `pycc_rt`, matching the CI sequence. Without that prerequisite the coverage command fails at the link step before it can measure coverage. A version-print smoke step runs before the gate so a broken/missing install fails loudly rather than silently.
- Test code itself (`tests/`, `*_tests.rs`, `tests.rs`) is excluded from the denominator automatically — the gate measures product code exercised by tests, not tests covering themselves.
- Exemptions are whole-file only, via `--ignore-filename-regex` (no per-function opt-out exists on stable Rust — see D-014). Each exemption needs a named entry here:

  | File pattern | Reason |
  |---|---|
  | *(none yet)* | — |

  An uncovered file with no entry in this table is a review-blocking finding, not a gap to wave through.

- **Practical notes on what actually shows up as a coverage gap** (learned building the first few v0.1 crates — verified directly against `cargo llvm-cov`'s HTML report, not assumed):
  - A hand-written `match { expected => ..., _ => panic!("...") }` — in test code or production code — creates its *own* region for the `_`/catch-all arm. If nothing ever exercises that arm, it's a gap, even though the arm is real and reachable. In tests, prefer `#[derive(Debug, PartialEq)]` on the type under test plus `assert_eq!(actual, expected)` over a manual match-and-panic assertion — it needs no catch-all arm at all.
  - **`.expect()`/`.unwrap()` do *not* have this problem**: their internal panic branch lives inside libcore/libstd, outside the calling crate's instrumented regions, so a call that always succeeds in every test still reads as 100% covered. This is the right choice for an operation that's genuinely infallible given the caller's own invariants (see `pycc_codegen::compile_to_object`'s five `.expect()`s on IR-construction/target-machine-creation operations that no input can make fail once `Target::from_triple` has already validated the requested triple).
  - **A closure passed to a combinator (`.map_err(|e| ...)`, `.and_then(...)`, etc.) is tracked as its own function/region and *does* need to actually run** — if the `Result`/`Option` it's attached to never takes that branch across the whole test suite, the closure body shows as a missed region even though the call site's own line is "covered." Reserve `Result`-returning `.map_err(...)` for failure modes a test can actually trigger (e.g. a bad output path); use `.expect(...)` for the rest instead of threading a `Result` no real input can produce.
  - **A function generic over `impl Fn(..)` (dependency-injection for testability — e.g. passing in a fake filesystem-existence check) gets monomorphized once per distinct closure type**, and each monomorphized copy is tracked *separately*: a copy that's only ever called with an always-true fake never executes that copy's error branches, and that reads as a real gap even though the *production* closure (or a different test's fake) exercises them. Fix: take a plain `fn(..) -> ..` pointer instead of `impl Fn(..)` when every caller's closure is non-capturing (as is typical for this kind of fake) — one concrete function pointer type means one compiled body, so coverage from every caller (production and every test) accumulates on the same counters. Only reach for `impl Fn`/`Box<dyn Fn>` when a caller genuinely needs to capture state; don't default to it for simple fakes.
  - **A test that skips itself when an optional local prerequisite is missing (e.g. `tests/slice0.rs`'s cross-compilation test, which skips unless a `--target`'s `pycc_rt` has already been built locally) makes the coverage gate depend on incidental developer-machine state, not on the test suite itself.** A dev machine that accumulated that prerequisite from earlier manual testing shows 100%; a fresh CI runner that never built it sees the test skip and the branch it alone exercises reads as a gap — caught exactly this way when `build-test-coverage`'s CI job (a clean checkout) showed 3 missed regions/1 missed line in `src/main.rs` that a local run right before pushing did not, and reproduced precisely by moving the local prerequisite build aside and rerunning. Fix: give the coverage-gated CI job whatever setup makes the skip-guard's precondition always true there (here, building that one cross-target's `pycc_rt` in the same job), so the gate never rides on whether *this specific* environment happens to have accumulated the right state.

## Meta

Every bug that reaches `main` gets a permanent regression test named after the issue (`tests/regress/issue_1234.py`). Coverage gate: conformance suite must touch 100% of implemented grammar productions (grammar-coverage instrumentation in the parser).
