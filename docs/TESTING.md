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

## Language-track regression assertions

Required CI runs `scripts/check_language_tracks.py` before importing
`scripts/test_check_language_tracks.py`. The registered assertions bind the
root invariant, README, roadmap, conformance matrix, testing contract, active
ADR, and public-site projections to the same v1.0/Python 3.14 scope. They also
preserve the cumulative 3.14 and planned 3.15 fixture ranges plus the
independent pinned 3.14 compatibility run.

The self-tests mutate each contract independently. They reintroduce the
permanent-3.14 wording, widen v1.0 claims to all of v1, make either language run
non-cumulative, remove the independent 3.14 run or superseding-ADR gate, and
remove either CI command. A registered mutation-test inventory prevents silent
test deletion or rebinding. It requires every named test exactly once and
hashes the complete executable self-test source, so a hollow method or
module-level unittest discovery override also requires explicit
re-registration. Hidden Markdown comments, quoted text, non-normative fenced
examples, and struck text cannot satisfy a required visible claim (the normative
`pycc.toml` example in `CLI_SPEC.md` is the sole fenced assertion).
The checker also hashes all visible normative Markdown in every registered
policy document, plus the normative `pycc.toml` example. Any new or edited
statement there therefore fails as unregistered regardless of wording; an
intentional edit updates the reviewable prose, exact assertions, mutation
cases, and canonical digest together. Public-site projections are checked by
exact required claims and mutation cases without hashing unrelated HTML,
metadata, or presentation.

The trusted-base workflow policy separately parses the head `ci.yml` as data.
It requires a dedicated `language-track-policy` job on every pull request, its
pinned credential-free candidate-merge checkout, and the unique unconditional
language-track step as the first executable step. The step uses an isolated
environment with the default shell and the exact guard-then-self-test command
sequence. The repository contract is therefore checked before Python imports
the head-controlled mutation suite. No preceding head-controlled command can
rewrite its inputs, and `ci-gate` requires the job to succeed. Deleting,
redirecting, or masking the
regular-CI invocation therefore fails the required `audit` check without
executing pull-request code. The prose digests and their
validator remain head-controlled regression tripwires, not a security boundary;
the trusted audit protects their invocation, while human review protects an
intentional simultaneous weakening of head tests, validator, and digests.

The initial rollout follows the repository's staged CI-evidence protocol. A
preparatory pull request adds the exact prospective `ci.yml` digest while
retaining the active digest. After that change merges to the default branch,
the activation pull request changes the workflow byte-for-byte to the reviewed
version and adds the semantic Psych-AST contract; its trusted-default-branch
`audit` authorizes the already-staged digest without executing head code. A
cleanup change retires the old digest. After activation, both the exact
workflow digest and the semantic contract protect every later pull request.

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

## Roadmap acceptance evidence

A checked acceptance item in [ROADMAP.md](./ROADMAP.md) is a release claim, not
a manually maintained status decoration. Every `[x]` task-list item must have
exactly one inline marker, including an item nested in a Markdown blockquote:

```markdown
<!-- roadmap-evidence: <registered-id> -->
```

`scripts/check_roadmap_evidence.rb` binds each registered identifier to the
complete roadmap heading path and claim it proves plus a deterministic
repository check. Missing, unknown, misplaced, or claim-mismatched markers
fail. Fenced code, including a fence nested in a blockquote or list container,
and HTML comments do not contribute rendered headings or task items. Raw HTML
blocks are rejected fail-closed instead of attempting to infer their rendered
contents. A task indented beneath a rendered list container is still checked,
while an unattached four-space-indented block is code. Setext headings are
rejected fail-closed; roadmap structure uses ATX `#` headings with no
tab-indented pseudo-headings. Rendered headings in blockquotes and list-item
bodies update the same complete heading path, and a checked task continuing an
empty list marker is still evidence-bearing. Adding a new evidence type starts
with a failing public-CLI mutation in
`scripts/test_check_roadmap_evidence.rb`; the checker implementation, marker,
and documented claim land together.

The initial `ci-build-test-coverage-100` evidence requires all of the following:

- the exact coverage claim in the v0.1 checklist;
- an unfiltered `pull_request` trigger;
- the unconditional, dependency-free, failure-propagating
  `build-test-coverage` job on the trusted runner;
- the exact pinned environment and setup-step prefix, with no earlier
  head-controlled script able to shadow the coverage executable;
- the named hard-coverage step using the default shell with no inherited run
  defaults;
- the exact command
  `run_isolated "$TRUSTED_COV" llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`
  inside a clean environment owned by the unprivileged `nobody` user.

That workflow proof is also an unconditional repository invariant. The trusted
checker validates it even while the roadmap claim is unchecked or absent, so a
pull request cannot remove every evidence marker and replace the required
coverage job with a successful no-op. The marker controls whether the delivery
claim may be shown as complete; it never controls whether hard coverage is
enforced.

The `ci-tier1-cross-compile` evidence binds an allowlist of exact reviewed
`ci.yml` byte digests that provide the five Tier-1 native targets, cross-host
build and execution proof, and aggregate `ci-gate`. Because a workflow can
preserve plausible job names while bypassing their commands, any future change
to that evidence is staged: first append the reviewed prospective digest while
retaining the current digest in the trusted checker, then change the workflow
and roadmap claim, and finally remove the superseded digest. A digest is
retired immediately if later repository requirements make that workflow
incomplete; a transition window is valid only while both versions satisfy the
current contract.

D-047 makes the earlier PR-4 prospective performance workflow obsolete and
defers a newly designed and reviewed gate to PR-6. The old candidate digest is
therefore retired instead of leaving stale workflow bytes authorized. The
structural cache-lifecycle validator remains fail-closed for any future
`frontend-perf-gate`, but PR-6 must still stage its exact new workflow bytes and
digest before activation.

The preceding staged authorization reviews the Python language-track workflow
before activation. Its inert
`scripts/fixtures/ci-language-track-policy.yml` snapshot adds a dedicated
`language-track-policy` job, runs the repository guard before any
head-controlled self-test import, and makes the job an explicit `ci-gate`
dependency. This activation replaces `ci.yml` with those exact reviewed bytes,
so the language-track job is active in this revision. No frontend-performance
workflow remains allowlisted: under D-047, PR-6 must stage a fresh exact
candidate that preserves every then-active required job before it can activate
the deferred performance gate.

The dedicated language-track job runs its repository guard and then its
self-tests immediately after the pinned checkout. The authority is the
required read-only `Workflow policy` job: it checks out the base revision,
downloads the head revision's workflows and `docs/ROADMAP.md` as non-executable
data, then runs the base revision's roadmap tests and checker against those
inputs. A pull request that replaces its own checker therefore cannot replace
the implementation that authorizes its checked roadmap markers. Because
`pull_request_target` binds `github.sha` to the pull request's base branch, an
activation pull request must target the protected default branch directly; a
non-default stacked base is not an acceptable trust context.

## CI privilege policy

Every GitHub Actions workflow declares an explicit workflow-level permission
baseline. The baseline may contain only read or `none` scopes, or
`permissions: {}`; a job that needs an elevated scope must opt in at job level
and satisfy the trust-boundary rules in `AGENTS.md`.

Regular CI runs both commands after the isolated hard-coverage boundary and
before the ordinary post-gate build:

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
It also enforces the language-track invocation contract described above for
the unique `ci.yml` in the audited workflow set.
The audited workflow set must contain `workflow-policy.yml`, and that file must
match an explicitly approved SHA-256 digest in the trusted checker. This makes
deletion, renaming, trigger replacement, or an extra executable step fail
closed. Updating the anchor is intentionally staged: first add the independently
reviewed prospective workflow as an inert fixture and add its exact digest
while the old anchor remains, then replace the active anchor byte-for-byte from
that fixture in a later pull request and retire the superseded digest in that
activation change.

The regular PR job runs this checker for fast feedback only; pull-request code
can change its own workflow. The authoritative `Workflow policy` workflow uses
`pull_request_target` on every pull request, checks out the pull request's base
commit through `github.sha`, downloads the head revision's workflow YAML and
`docs/ROADMAP.md` through the read-only GitHub API, and treats them as data. It
never checks out or executes pull-request code. The checkout is a trusted
default-branch anchor only when the pull request targets protected `main`;
security-sensitive activation PRs therefore target `main` directly even when
their preparatory authorization remains an unmerged dependency. A stacked
non-default base is useful for reviewing a diff but is not accepted as audit
evidence. Do not substitute another webhook payload ref for this constraint.
Job-level trusted-ref exceptions remain a review boundary: reviewers
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
Branch protection is strict and requires `ci-gate` and `audit`, bound to the
GitHub Actions app. `ci-gate` (D-032) is a single stable-named job in
`ci.yml` that fans in every job in that workflow (`build-test-coverage`,
`language-track-policy`, all four `native-build-test` Tier-1 legs,
`cross-compile-build`, `cross-compile-verify`) so branch protection enforces
the whole Tier-1 matrix through one required-check name that survives matrix
edits, rather than naming each matrix leg directly (whose GitHub-generated
name bakes in the matrix values and would go stale the moment an `os`/`target`
entry changes). The switch from directly requiring `build-test-coverage` to
requiring `ci-gate` happened once `ci-gate` existed on `main` (PR #19,
merged 2026-07-25) -- it was deliberately not done inside that same PR,
since flipping it earlier, while other branches were still open against a
`main` without this job, would have left those PRs waiting on a required
check they had no way to satisfy. Removing either required check, disabling
strict mode, accepting an `audit` context from another app, or dropping a
job from `ci-gate`'s `needs:` list is a policy regression; all later policy
changes are evaluated by the trusted checker from their base revision.

## Code coverage (D-014)

Distinct from the grammar-coverage gate in Meta below (which measures PEP/language-surface coverage): this is ordinary line/region coverage of pycc's own Rust source, gated on every PR from v0.1 on.

- Tool: `cargo llvm-cov` — a separately distributed cargo subcommand, **not** bundled with any rustup component. CI installs it explicitly and pinned (installer action or `cargo install cargo-llvm-cov --locked --version <pinned>`), plus the `llvm-tools-preview` rustup component it drives at runtime; a bare "install llvm-tools" fails with "no such command: llvm-cov" (caught by repo audit, issue #13). Independent of the Homebrew LLVM used by `inkwell` for codegen — versions don't need to match.
- Gate: `run_isolated "$TRUSTED_COV" llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`, run in CI on at least one Tier-1 target per PR. The explicit `llvm-cov` argument is required when invoking Cargo's subcommand binary directly. CI resolves and installs the trusted tool before executing repository code, then runs the cross-target `pycc_rt` prerequisite, workspace build, and coverage under `sudo -u nobody env -i` with isolated HOME, Cargo home, temp, and target directories. The workspace and runner-owned toolchain/binary are read-only to that user, so a build script or procedural macro cannot replace the executables or write GitHub command files. The checker pins the complete environment and step prefix through the hard-gate step; regular repository policy/test steps run only afterward. The x86_64 macOS runtime is built first so the cross-compilation test cannot skip its success path, then `cargo build --workspace` supplies the normal debug `pycc_rt` used by the remaining slice-0 tests. The pinned tool's version smoke check runs immediately before entering the boundary.
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
