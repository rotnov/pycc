# pycc Testing Specification

Testing *is* the spec enforcement mechanism: [PYTHON_STANDARDS.md](./PYTHON_STANDARDS.md) defines what must work; this file defines how we prove it — on every Tier-1 platform, every commit.

## Layers

| Layer | Location | What it proves |
|---|---|---|
| 1. Unit (Rust) | per-crate `#[cfg(test)]` | lexer/parser/checker/MIR internals |
| 2. Conformance | fixtures: `tests/conformance/pyXY/` *(planned; today flat at `tests/fixtures/`)*; harness: `tests/conformance.rs` + `tests/conformance/*.rs` | each supported language level compiles and runs its cumulative fixture set; `stdout ==` that level's pinned CPython oracle |
| 3. Diagnostics | `tests/diagnostics/` | rejected constructs fail with the exact code + span (insta-style snapshots) |
| 4. Differential fuzzing *(planned)* | `tests/fuzz/` *(not yet created)* | generated typed-Python programs: pycc binary output ≡ CPython output; crashes/mismatches auto-minimized |
| 5. Runtime property tests | `pycc_rt` proptest | str/list/dict/RC/cycle-collector invariants |
| 6. Corpus (OSS projects) *(planned)* | nightly CI *(not yet live)* | real code compiles and its own test suite passes |
| 7. Benchmarks | `benches/` + pyperformance subset | compiler speed + generated-code speed |
| 8. Hosted `ext` boundary | `tests/issue_1067_neg004_ext_conformance.rs`, plus the other end-to-end `ext` harnesses (`tests/issue_1036_ext_wiring.rs`, `tests/issue_1048_ext_scalars.rs`, `tests/issue_1049_ext_str.rs`, `tests/issue_1050_ext_tuple.rs`, `tests/issue_1063_overflow_error.rs`, `tests/issue_1066_ext_user_exceptions.rs`, `tests/issue_1112_ext_memoryview.rs`, `tests/issue_1113_ext_buffer_index.rs`, `tests/issue_1114_numpy_oracle.rs`, `tests/issue_1142_ext_buffer_store.rs` and `tests/issue_1292_import_error.rs`) | a built CPython extension module refuses every non-conforming host call exactly as [D-244](./decisions/D-244-add-a-hosted-cpython-extension-module-artifact-mode.md) rule 7 states, on an installed interpreter |
| 9. Embedded executable | `tests/issue_1223_embedded_executable.rs`, `tests/issue_1242_locked_closure.rs`, `tests/issue_1286_windows_embedded_executable.rs`, `tests/issue_1296_windows_locked_closure.rs`, plus the unit tests under `src/embed/` and `src/lock/build_tests.rs` | a plain build of a standard-library-only program bundles CPython 3.14 and matches CPython 3.14.7 byte-for-byte, relocated and under a shadowing `PYTHONPATH`; on a Windows host, the stub `OUT` plus the program DLL match CPython 3.14.7 for standard-library programs (D-253) and for a pure-Python locked closure (#1296); a third-party root runs from its `pycc.lock` closure in `OUT.pycc/closure/`; every refusal keeps its reason ([D-248](./decisions/D-248-embedded-executable-artifact-layout-and-bridge-split.md), [D-249](./decisions/D-249-pycc-lock-schema-environment-resolver-and-update-command.md)) |
| 10. Interop policy | `tests/issue_1224_interop_policy.rs`, the `i0402_*` snapshots under `tests/diagnostics/`, plus `src/interop_policy/tests.rs` | `--interop-policy`, `--pure` and `[interop]` admit or reject each CPython-backed root alike in `check`, `build` and `run`, a rejection is `I0402` on every host and precedes any `I0403` (D-128) |

Layers 4 and 6 are planned and not yet implemented on current `main`; no
`tests/fuzz/` directory or nightly corpus workflow exists. Their table rows
describe the target architecture, not a live system — see the dedicated
planned sections below for the current status of each.

Layer 2's `tests/conformance/pyXY/` location is the same kind of row: it
describes the eventual v1.0-scale, language-level-selecting harness, not what
runs today. Every conformance fixture currently lives **flat** at
`tests/fixtures/pep_NNNN_slug.py` and is run by the conformance harness (`tests/conformance.rs` plus its `tests/conformance/*.rs` cohort files, see below)
(D-102); no `pyXY/` directory exists anywhere in this repository, and the only
subdirectory under `tests/fixtures/` is `policy-successors/`. This is already
recorded for the PR-13 fixtures below, and it is the settled convention rather
than an accident of those two files.

The harness's own Rust sources are a different thing from that fixture tree.
`tests/conformance.rs` is the crate root, and its cohort submodules live at
`tests/conformance/*.rs` (`classes.rs`, `exceptions.rs`, `imports.rs`,
`numeric.rs`),
`#[path]`-declared from the root's `harness_modules!` block; a new fixture's
test goes in the cohort that owns its semantics. Per
[D-221](./decisions/D-221-the-conformance-harness-is-the-root-file-plus-its.md),
the **conformance harness sources** are the root file followed by every direct
`tests/conformance/*.rs` in sorted file-name order, and that set is the one
contract all three text-readers of the harness share: the two Rust guards
include `tests/harness_support/conformance_sources.rs`, and
`scripts/check_conformance_breadth.py`'s `read_harness` mirrors it. The root's
`every_harness_module_on_disk_is_declared` test fails on a cohort file that
exists on disk but is not declared, so a file cannot be read by the guards
without also being compiled. The still-planned `pyXY/` fixture directories are
invisible to that non-recursive `*.rs` rule by design.

`docs/PYTHON_STANDARDS.md`'s matrix follows the same split, and
`tests/conformance_matrix_guard.rs` is what now holds the line (see
[D-175](./decisions/D-175-scope-the-conformance-matrix-fixture-guard-to-green.md)):
a `☐` row's `pyNN/`-prefixed path is a *planned* path for a fixture nobody has
authored yet and is deliberately not asserted to exist, while an evidence-backed
row — `◐` or `✅` — claims its fixture passes and so must cite a path that
resolves under `tests/fixtures/` and is registered in the conformance harness
(`tests/conformance.rs` plus `tests/conformance/*.rs`). The guard also runs the
inverse direction — every `tests/fixtures/pep_*.py` must be registered in the
conformance harness or carry an allowlist entry recording why it is not.

That guard proves a green row's fixture *exists and runs*; it deliberately
says nothing about how much of the PEP that fixture exercises. Breadth is a
separate contract, recorded per row in
`tests/fixtures/conformance-breadth-manifest.json` and validated by
`scripts/check_conformance_breadth.py` (see
[D-176](./decisions/D-176-declare-per-row-conformance-breadth-in-a-validated.md)).
Every evidence-backed row declares what its fixtures actually prove (`proven`,
each item citing one of that row's own fixtures as evidence) and what the PEP
contains that they do not (`not_proven`, each item giving a reason, the issue
tracking it where one exists, and a required `kind`). The checker parses
`docs/PYTHON_STANDARDS.md` with exactly the guard's own rules, so the two
cannot disagree about which rows are evidence-backed or which fixtures a row
cites, and it requires a bijection between manifest entries and those rows in
both directions — flipping a row to `◐`/`✅` without declaring its breadth
fails, and so does a manifest entry for a row that is no longer evidence-backed.

`kind` is what separates the two evidence statuses. A gap classified `core` is
a category of the PEP that is simply not implemented or not exercised; one
classified `out-of-scope` is a deliberate, permanent non-goal for pycc. The
checker enforces
[D-177](./decisions/D-177-scope-matrix-acceptance-to-proven-semantics.md)'s
rule in both directions — **any `core` gap forces `◐`; `✅` requires zero** —
so a row's marker is derived mechanically from its classified gaps rather than
chosen at review time, and a narrow fixture cannot be promoted to whole-PEP
acceptance. The manifest is at `manifest_version: 2`, which is what makes
`kind` required. `scripts/test_check_conformance_breadth.py` is the mutation
self-test for all of this; both run without `cargo`:

```
python3 scripts/check_conformance_breadth.py
python3 scripts/test_check_conformance_breadth.py
```

Since [#595](https://github.com/rotnov/pycc/issues/595) the breadth contract is
a merge gate rather than a locally-runnable convention: the `governance` job
runs `scripts/check_conformance_breadth.py` directly, and `ci-gate` — the
required branch-protection check — requires `governance` unconditionally, on
every pull request rather than only compiler-classified ones. That placement is
deliberate: a matrix row is edited by documentation-only changes as often as by
compiler ones, so a gate that ran only under the compiler classification would
miss exactly the pull requests most likely to promote a row past its manifest
entry. It also means no new branch-protection entry was needed, and none should
be added — registering the step as its own required context would create a
second, independently-editable gate for the same contract.
`scripts/test_check_conformance_breadth.py`'s `CiWiringTest` binds both halves:
it fails if the step is removed from `governance`, if `governance` leaves
`ci-gate`'s fan-in, or if the step is made advisory with `continue-on-error`.

The same checker also binds `docs/ROADMAP.md`'s prose to the matrix, which is
[#623](https://github.com/rotnov/pycc/issues/623)'s third completion criterion.
The roadmap states the conformance totals in a bold
`**Conformance progress (...)**` headline and quotes the checker's own summary
line; both used to be maintained by hand and both had drifted. The checker now
parses that headline — the evidence-backed total, the required-row target, the
derived gap, and the whole-PEP count — and fails when any of them disagrees
with the matrix, when the headline contradicts itself, or when the quoted
summary is no longer the string the checker prints. It is deliberately
fail-closed: a headline that is missing, duplicated, or reworded past the
parse is a failure rather than a silent pass, because a guard that quietly
stops guarding when someone rewrites the paragraph is exactly the drift #623
describes. Anchoring on the bold span rather than the paragraph is what keeps
the superseded totals the paragraph narrates historically from being read as
the current claim. The path is overridable with `--roadmap` alongside
`--matrix`, `--manifest` and `--harness`; CI runs the checker with its
defaults, so the roadmap binding is enforced by the same `governance` step and
needs no separate job.

Both of those guards reason about which fixtures a row cites and how much of
the PEP they cover. Neither says anything about what the registered tests
actually compare, and until [#224](https://github.com/rotnov/pycc/issues/224)
nothing did: `tests/conformance.rs`'s shared helper returns the compiled pycc
binary's stdout as its first tuple element and the pinned CPython oracle's as
its second, and a helper returning `(pycc_output.stdout.clone(),
pycc_output.stdout)` would still build pycc, still launch `python3.14`, still
assert both processes exited zero — and turn every differential test into
`assert_eq!(x, x)` with every gate green. `tests/conformance_oracle_guard.rs`
holds that line. It reads the harness sources as text (so it needs neither
LLVM nor the oracle interpreter, and is a genuine negative control rather than
a second copy of the tests it guards) and requires: the shared helper launches
the oracle and asserts both processes succeeded; its returned tuple's first
element derives from the pycc process alone and its second from the oracle
alone; `run_conformance_fixture` still delegates to it; and every
`*_matches_cpython_3_14_7_byte_for_byte` test binds a helper result pair,
compares exactly that pair in a top-level `assert_eq!`, and exercises both
profiles unless it is one of the two named in the guard's own
`DEBUG_ONLY_TESTS`. It also requires every registered `pep_*.py` fixture to be
run by some `#[test]`, so a fixture cannot satisfy the matrix guard while no
test executes it. The guard's own tests apply each mutation — assertion
deleted, made tautological, both sides pycc, oracle process removed, oracle
exit status ignored, comparison moved behind `if false` or into a comment, a
profile dropped — to a synthetic harness and to the real one, and require the
guard to reject each.

The focused D-094 release regression in `pycc_codegen` observes the same
production codegen helper immediately before object emission. Debug codegen
reports no applied pass pipeline and retains both a used and an unused runtime
declaration; release codegen reports the exact `default<O3>` pipeline, retains
the used declaration, and removes the unused one. This distinguishes a real
pass-pipeline run from the aggressive target-machine setting alone; differing
object bytes are not sufficient evidence that `Module::run_passes` executed.

Issue #242's permanent regression source lives at
`tests/regress/issue_242.py` and is executed through the public `pycc build`
path by `tests/slice1_codegen_depth.rs`. Companion controls cover both
function-local and module-global `None` assignment slots, the independent
`set.add()` result route, and a module-global read before assignment that must
trap rather than treating the canonical zero carrier as initialized. The
existing D-072 should-panic unit test remains the negative control that
`print()` itself is still rejected as a nested expression.

## Conformance harness (`pycc_testkit`)

- Each test = single `.py` file, header comment: PEP, category, min pycc milestone.
- Runner *(planned shape; see the flat-layout note under **Layers** for what
  actually runs today)*: for each supported language level, select that
  configuration's cumulative fixture range and pinned oracle → compile
  (`--debug` and `--release` both, once `--release` exists — see below) →
  execute → diff. The
  v1.0 Python 3.14 run covers `py30/` through `py314/` against CPython 3.14.7.
  After the v1.x adoption gate opens, the Python 3.15 run covers `py30/`
  through `py315/` against a pinned current Python 3.15 patch; the separate
  Python 3.14 compatibility run remains required. Outputs are recorded and
  re-recorded on oracle patch bumps.
- A PEP flips to ✅ in PYTHON_STANDARDS.md **only** when green on all Tier-1 targets in both profiles. The matrix file is updated by CI, not by hand (no automation backs this yet — see the PR-9 status note below for D-102's accepted interim by-hand policy). Whichever way a row is flipped, `tests/conformance_matrix_guard.rs` requires the newly-green row to cite a real flat fixture that is registered in the conformance harness (`tests/conformance.rs` plus its `tests/conformance/*.rs` cohort files).
- **v0.1 exception:** `--release`/LTO doesn't exist until v0.2 (see ROADMAP.md), so the "both profiles" rule only binds from v0.2 on. Every v0.1 PEP/feature flips to ✅ on `--debug` alone; nothing in v0.1 is held to a `--release` bar that has nothing to build against (see DELIVERY_PLAN.md, "Debug/release conformance").

**Python 3.14.7 oracle transition (activated 2026-08-15):** the three-round
D-103 sequence first staged the exact reviewed workflow and checker successors,
then activated the checker, and finally advanced the live Tier-1 workflow and
conformance contract to Python 3.14.7. `PYTHON_STANDARDS.md` and `ROADMAP.md`
move with the activation. This patch-level oracle refresh does not expand
D-012's Python 3.14 language level.

**PR-9 status (2026-07-30):** the `pycc_testkit` crate above remains unbuilt — D-102 extended the existing flat `tests/conformance.rs` integration test in place instead (11 fixtures at the time: the 2 pre-existing plus 9 new PEP fixtures), judging that PR-9's own needs (compile both profiles, run, diff against CPython) were still fully covered by that file's existing helper and didn't justify a new workspace crate. The "matrix file is updated by CI, not by hand" policy above has no automation behind it yet (verified: nothing currently writes `PYTHON_STANDARDS.md`'s status column); D-102's accepted interim policy is to flip a row by hand only once its fixture is observed green on a real, already-completed CI run across all 5 Tier-1 targets in both profiles — never speculatively. Building the real `pycc_testkit` crate and CI-owned status automation both remain deferred to whenever the v1.0-scale, multi-language-level harness this section describes is actually needed.

**PR-10 status (2026-07-31):** one more fixture added the same way (12 total now) — `pep_0585_builtin_generics_matches_cpython_3_14_6_byte_for_byte`, exercising `list[int]`'s literal/`.append()`/indexing/`len()`/iteration slice through the same `run_conformance_fixture_with_profile` helper D-102 established; no change to this section's harness shape. This branch's CI ([run 30608030517](https://github.com/rotnov/pycc/actions/runs/30608030517)) has since observed the new fixture passing on all 5 Tier-1 targets, in both profiles — per the same D-102 policy `PYTHON_STANDARDS.md`'s PEP 585 row is flipped to `✅` on that evidence (see `ROADMAP.md`'s v0.2 section and `DELIVERY_PLAN.md`'s PR-10 row).

**PR-13 Task 5 status (2026-08-04):** two more fixtures added the same way (19 dual-profile `*_matches_cpython_3_14_6_byte_for_byte` tests in `tests/conformance.rs` now) — `pep_0695_generics_matches_cpython_3_14_6_byte_for_byte` (a one-type-parameter generic function called at 3 sites across `int` and `str`, exercising call-site monomorphization) and `pep_0613_typealias_matches_cpython_3_14_6_byte_for_byte` (a legacy `X: TypeAlias = int` alias used as a parameter/return annotation). Both fixtures live flat at `tests/fixtures/` — the brief's literal `py312/`/`py310/` subdirectory paths do not exist anywhere in this repo and were not created; every existing PEP-numbered fixture is flat, and these two follow that real convention instead (see `docs/PYTHON_STANDARDS.md`'s PEP 695/613 rows, corrected in this same patch to the real flat paths). The `pep_0613_typealias.py` fixture deliberately omits `from typing import TypeAlias`: pycc has no `Stmt::Import`/`Stmt::ImportFrom` support at all (confirmed by compiling the import against this exact fixture — `error[C0001]: statement kind not supported yet`), and the pinned CPython 3.14.6 oracle defers annotation evaluation by default (PEP 649/749), so the bare `TypeAlias` name is never evaluated at runtime on either side; both fixtures were verified locally in both `--debug` and `--release` against the pinned oracle and pass byte-for-byte. Per D-102's "never speculatively" policy, `PYTHON_STANDARDS.md`'s PEP 695 and PEP 613 rows stay `☐` in this patch — flipping them to `✅` requires this branch's own CI run observed green on all 5 Tier-1 targets in both profiles, which does not exist yet; that flip (with the run-URL citation, matching PR-10's own row-flip format above) is deferred to this PR's final docs sweep once real CI evidence exists.

The current frontend also keeps focused differential sources under
`tests/diagnostics/` when CPython's runtime behavior defines why strict pycc
must reject a program before code generation. In
`d0021_unbound_local.py`, CPython 3.14 raises `UnboundLocalError`, while
`pycc check` reports `T0021`; byte-exact human and version-1 JSON snapshots
lock the public diagnostic. This focused oracle case does not replace the
planned multi-version conformance harness.

### CPython oracle exception list

Byte-for-byte conformance against the pinned CPython oracle stays the rule for
semantics — values, output order, exit codes, exception types
([D-242](decisions/D-242-product-mode-the-delivery-process-informs-rather-than-blocks.md) rule 4). The only text a
`*_matches_cpython_*_byte_for_byte` test may exempt from the comparison is
listed here, one row per exemption, naming the inessential text (traceback
formatting, warning wording) and why. Anything not listed stays byte-exact;
an exemption applied in a test without a row here is a review-blocking
finding.

| Fixture/test | Exempted text | Reason |
|---|---|---|
| *(none yet)* | — | — |

## Website execution evidence preparation

`tests/site_evidence.rs` binds the proposed Language and Diagnostics
transcripts to the real CLI from the repository root. The non-oracle
`language_command_matches_canonical_stdout` test runs exactly
`pycc run tests/fixtures/pep_0526_var_annotations.py` and checks exit 0,
empty stderr, and `tests/fixtures/pep_0526_var_annotations.expected.txt`.
This is the default debug `run` path, not a release run. The unchanged
`pep_0526_var_annotations_matches_cpython_3_14_7_byte_for_byte` conformance
test separately covers both build profiles; one fixture proves neither all
of PEP 526 nor full Python 3.14 compatibility (D-177).

`language_commands_match_cpython_3_14_7_and_canonical_stdout` independently
executes the same pycc command and
`python3.14 tests/fixtures/pep_0526_var_annotations.py`, after requiring
exactly `Python 3.14.7` from the oracle's successful, stderr-free version
probe. It checks both exit codes, both stderr streams, both stdout streams
against the canonical snapshot, and the independent pycc/CPython comparison.
It is ignored by default under D-080: the existing Tier-1
`cargo test --workspace -- --include-ignored` steps execute it after oracle
setup, while the isolated coverage step needs no oracle. Windows uses
the existing `python3.14.exe` alias. Only the D-082 CR-before-LF translation
is removed from CPython output; pycc output is never normalized. Checked-out
text snapshots receive the same narrow Git-autocrlf normalization.
Non-oracle controls exercise a missing executable and a real non-oracle
version process; synthetic version-probe cases reject wrong patches,
malformed bytes, extra whitespace, nonzero exit, and stderr. These controls
are not evidence of a successful CPython run and never enable a fallback.

`diagnostics_commands_match_human_and_json_snapshots` executes exactly
`pycc check tests/diagnostics/d0021_range_argument_type.py` and the same
command with `--error-format json`. Both must return exit 1, empty stderr,
and the existing `.expected.txt`/`.expected.json` bytes on stdout. The
test also checks the actual JSON code, message, span and help fields against
the human transcript's code/message/location: the span remains the 1:1,
zero-length placeholder, JSON help is populated, and human output has no
help line (D-043, D-152). The existing diagnostics test remains unchanged.

Run the focused ordinary suite with `cargo test --test site_evidence`; with
the pinned oracle installed, use
`cargo test --test site_evidence -- --include-ignored`. Use the isolated
TMPDIR procedure below for repeated runs. The preserved preparation commit is
`0d94ad8f30b27131a5da381a034d55165558e56a`; successful CI run `33969157527`
executed the proof on all five Tier-1 targets. Language and Diagnostics now
publish these accepted historical executions under D-230. The website gate
remains offline: `scripts/site_execution_evidence_test.py` mutates the public
`check-site.sh` inputs to verify closed nested fields, preserved Git blobs,
provenance, command/status/output identity, visible transcripts, H1s and
limitations. The shared parser omits exactly one final newline from each
snapshot pane, never arbitrary whitespace. Page-cohort mutation tests cover
both new canonical routes; Lighthouse and narrow-width no-JS/JS checks are
browser evidence, not compiler conformance or field measurements. Run these
historical-blob controls through `scripts/test-check-site.sh` in a full-history
checkout, as the Pages workflow does. They are deliberately outside shallow
governance's `test_*.py` discovery; `scripts/test_site_execution_wiring.py`
checks the shell invocation, Pages validation invocation and full-history
checkout without reading Git objects. The Pages push and pull-request path
filters enumerate the ten direct execution-evidence dependencies exactly once
per event; wiring controls reject independent removals and duplicates.
Permissions, checkout depth, thresholds, required checks and `ci.yml`
classification remain unchanged.
The current Language/Status directive note follows D-229 and the annotation
semantics owner separately from the immutable D-230 transcript. Positive and
mutation controls preserve its module-prologue, no-binding, three core annotation
gaps and partial PEP 563 acceptance boundaries after integrating issues #919 and
#937. Public
CLI mutations also reject each new hero's limitations or source transcript
wrapped in `noscript`: evidence must be visible with JavaScript enabled too.

The Architecture hero adds a third per-page contract module under
[D-243](./decisions/D-243-architecture-hero-is-a-checked-in-re-derivable-compiler-pipeline-trace.md)
(Part 2 of #566). `tests/architecture_trace.rs` is the only party that
re-derives the pipeline: it drives `tests/fixtures/quick_start.py` through the
public crate APIs (`pycc_parser`, `pycc_hir`, `pycc_types`, `pycc_mir`) and a
native build, byte-compares every stage against the artifacts checked in under
`tests/fixtures/architecture-trace/`, asserts the native exit status and exact
stdout, and carries a negative control that rejects both a byte-mutated
artifact and one stage's artifact substituted for another's. It runs under
`cargo test --workspace` on all five Tier-1 targets. The separate guarantee
that a stage carrying no evidence may not be presented as implemented is a
record-level property owned by the validator, not by re-derivation, and is
proved through the public CLI by `scripts/site_pipeline_evidence_test.py`
described below. `scripts/site_pipeline_evidence.py` owns the record shape, the closed
eight-stage vocabulary, the derived state and the visible projection, and never
runs the compiler. Its controls are split by cost: the fast pure-function,
record-internal and wiring cases live in `scripts/test_site_pipeline_wiring.py`
and are discovered by `unittest discover -s scripts -p 'test_*.py'` in the
depth-1 `governance` job, while the slow public-CLI mutation battery lives in
`scripts/site_pipeline_evidence_test.py`, is invoked explicitly by
`scripts/test-check-site.sh` beside its Part 1 siblings, and is deliberately
not named `test_*`. Every rejection there is paired with a positive control
against the shipped record, so a mutation cannot pass for the wrong reason.
The Pages push and pull-request path filters enumerate
`scripts/site_pipeline_evidence.py`,
`scripts/site_pipeline_evidence_test.py`,
`scripts/test_site_pipeline_wiring.py`, `tests/architecture_trace.rs`,
`tests/architecture_manifest.rs` and `tests/fixtures/architecture-trace/**`
exactly once per event; the wiring
controls reject independent removals and duplicates. Regenerate the artifacts
with `PYCC_ARCHITECTURE_TRACE_OUT=tests/fixtures/architecture-trace cargo test
--test architecture_trace regeneration`; the parser, HIR and MIR artifacts are
Rust `Debug` renderings and are expected to churn whenever those types change. `tests/architecture_manifest.rs` carries the Rust-side manifest facts
(state, kind, page path, stable links, required-field presence) and re-runs
nothing; it is a separate integration test because the D-230 language and
diagnostics records pin `tests/site_evidence.rs` byte-for-byte to a preserved
source blob, so a case added there fails the site gate.

## Differential fuzzing (planned)

A generator would produce well-typed programs (type-directed generation — always compile-clean), weighted toward: arithmetic edges (overflow → bigint promotion paths), string unicode edges, collection aliasing, control-flow + exceptions, match patterns. Mismatch → auto-minimize (creduce-style) → auto-file issue with repro. This would run continuously on a dedicated runner. No fuzzing harness exists on current `main`.

## Corpus: open-source projects as integration tests (planned)

Tiers and gates in PYTHON_STANDARDS.md § Real-world corpus. Planned mechanics:

- Pinned commit per project; `pycc build` the package, run its pytest suite against the compiled artifact (test files themselves compiled where possible; interop fallback allowed and measured).
- Per-project dashboard: % files compiled, % tests passed, RC-elision rate, binary size, speed vs CPython on the project's own benchmarks.
- Regression vs previous release = release blocker.

No open-source-package corpus workflow, pinned package inputs, or per-project pass-rate dashboard exists on current `main`. The separate competitive-programming corpus below is a different corpus with a different shape, and does exist.

## Corpus: competitive-programming stdin/stdout programs (informational)

A second, narrower corpus, vendored and running today. It measures one thing:
how much real single-file stdin/stdout Python compiles unchanged, and how fast
the result runs against CPython. That measurement is retained informational
telemetry, not a contract. It carried `product-sprint-1`'s acceptance until the
2026-09-12 redirection
([D-244](./decisions/D-244-add-a-hosted-cpython-extension-module-artifact-mode.md));
`docs/ROADMAP.md`'s `product-sprint-1` section now owns that sprint's
acceptance, and "Hosted `ext` benchmark protocol" below is the methodology that
measures it. Do not read this section as a statement of what the sprint must
achieve.

- **Inputs.** `tests/corpus/codecontests/` — a pinned subset of the DeepMind
  CodeContests dataset (CC BY 4.0; see that directory's `NOTICE` and `LICENSE`),
  checked in as plain files rather than a submodule so a clean clone measures
  offline. 200 steering problems under `problems/`, 100 under `holdout/`. Each
  problem is one PYTHON3 solution of at most 100 lines, importing only an
  explicit stdlib allowlist, plus its public and private test cases packed into
  one `tests.json`. Every vendored file's sha256 is in `manifest.json`.
  Regenerate with `python3 scripts/select_codecontests_corpus.py` (needs
  `pyarrow` and network access; deliberately not a repository dependency).
- **Holdout discipline.** The holdout set is excluded from the default
  denominator and reported only under `--include-holdout`. Do not read it when
  deciding what to implement — it exists to show that gains on the steering set
  generalise.
- **Reading the holdout rate.** Neither the report nor its JSON splits the
  denominator by set, so the holdout rate comes from two runs — one default, one
  `--include-holdout`. Subtract their `compiled` **counts**, never their rates:
  the default run's denominator is 200 and the merged run's is 300, so a
  difference of rates is not the gap between the two sets at all. The holdout
  rate is `(merged compiled − default compiled) / 100`, and the steering rate is
  the default run's own. The method is valid only when neither run reports
  `INCOMPLETE`: `manifest.json` orders all 200 steering records before all 100
  holdout records, so a run that exhausts `--max-seconds` drops holdout problems
  first, and the two runs would then cover different evaluated subsets.
- **Metric.** `python3 scripts/check_corpus_compile_rate.py` reports
  `compiled N/M`, `matched K/N`, the median speedup against CPython, and the
  diagnostic classes that stopped the failures, as a first/any tally. Each
  problem is built with `pycc build --release`: the reported speedup is a
  statement about the shipping profile, so timing an unoptimized build would
  measure something this report never claims. `matched` compares the binary's
  output against the expected output on every case after one narrow
  normalization and
  byte for byte otherwise — CRLF and CR become LF, and a run of trailing
  newlines collapses to exactly one, because the dataset's recorded outputs and
  a program's own final newline disagree about trailing whitespace often enough
  that a byte-exact rule would report correct programs as mismatched. Output
  that is empty stays empty and does not match a blank line. Speedup is measured
  on each problem's largest vendored case, best of three runs, and counts only
  problems whose CPython wall time reaches 200 ms — below that the ratio
  measures process startup, so those problems are reported as excluded rather
  than folded in. A matched problem whose timing runs disagree with the case is
  reported as dropped, for the same reason: every matched problem is accounted
  for in the report rather than quietly missing from the sample count.
  A program whose imports are all standard-library roots builds as an
  embedded executable ([D-248](./decisions/D-248-embedded-executable-artifact-layout-and-bridge-split.md)),
  detected by its `<binary>.pycc/PYCC-BUNDLE` marker; it keeps its compile and
  match verdicts but is reported as `embedded K/N` and left out of the
  speedup median, whose ratio describes native code rather than interpreter
  start-up. The CI job therefore installs CPython 3.14.7, sets
  `PYCC_PYTHON=python3.14`, and times the CPython baseline with `python3.14`.
  `--json` writes the same data machine-readably. The script
  reads the corpus, writes nothing inside it, and performs no network I/O.
- **Gate status: reporting only.** CI's `corpus-compile-rate` job is
  non-blocking by omission from `ci-gate`'s `needs`, not by
  `continue-on-error`. The metric exits 0 for every measurement outcome,
  including a zero compile rate and exhausting its own `--max-seconds` budget
  (which prints `INCOMPLETE: n of M evaluated`). That budget is checked at every
  phase boundary, not only between problems, so the run overshoots it by at most
  one build rather than by a whole problem's builds, case runs and timing legs;
  a problem abandoned before its outcome is final is left out of the tallies
  entirely, and one abandoned after its cases have been checked keeps its
  correctness verdict and counts its lost sample as dropped. It exits non-zero
  only when the harness is broken: a missing or corrupt manifest entry, a
  problem with no `solution.py` or no `tests.json` manifest entry, an unreadable
  corpus, a malformed `tests.json` payload — including a case that is not an
  object or whose `input`/`output` is not a string — a corpus over its own byte
  budget, a declared `steering_count`/`holdout_count` that disagrees with the
  manifest's own per-set records, a `pycc` binary that is absent or not
  executable, a scratch directory that cannot be created under `RUNNER_TEMP`,
  an unwritable `--json` output path, or either of the two ways a toolchain
  rather than a program can be what failed: a build that exits 2, which
  `docs/CLI_SPEC.md` reserves for an invalid invocation or a broken environment
  and which this script's own fixed invocation therefore narrows to the
  environment, and every evaluated build failing while emitting no
  `error[CODE]` diagnostic at all, which is what a linker driver that runs and
  then fails looks like. A single undiagnosed failure stays a tallied failure
  class, since one such problem is a compiler defect worth reporting. A red job
  therefore means the measurement could not be taken, never that the score was
  low. Every long step in that job -- the LLVM install, the release build, the
  measurement itself -- carries its own bound, so the job-level
  `timeout-minutes` is a backstop rather than the first thing to fire: a
  job-level timeout runs no further steps, which would skip the report upload
  even though it is guarded by `if: always()`.

## Embedded executable tests (Part 1 of #1028, #1223)

- **Interpreter-free, non-ignored.** `src/embed/`'s unit tests drive a fake
  interpreter layout (a `Python.h` holding one `#error` line and, on macOS,
  throwaway dylibs and bundles built with `cc`), so probing, bundle
  assembly, the Mach-O relocation worklist and the compile step all run on
  the coverage host, which has no `python3.14`. The build stops
  deterministically inside `cc`, exactly as the `--ext` wiring tests do.
  `tests/issue_1223_embedded_executable.rs`'s non-ignored tests pin each
  `I0403` reason, the unmarked-sidecar and missing-interpreter failures, and
  that a program with no CPython import links no `Py*` symbol (#1045).
- **Hosted, `#[ignore]`d.** The `*_matches_cpython_3_14_7_byte_for_byte`
  tests (and `tests/issue_1081_foreign_method_call.rs`'s embedded `gc`
  test) build a real embedded executable, so they need `python3.14` on
  `PATH` to be CPython 3.14.7, or `PYCC_PYTHON` naming one; the Tier-1
  non-Windows legs run them under `--include-ignored`, and the `gc` test runs
  on the Windows leg too. `tests/issue_1286_windows_embedded_executable.rs`
  is their Windows counterpart (D-253): its `#[ignore]`d tests run on the
  Windows leg against `python3.14.exe`, where a missing CPython 3.14.7 or
  `llvm-readobj` fails them instead of skipping, and cover the oracle
  program, relocation under a scrubbed `PATH`, the `DLLs\` extension
  modules, `pycc run`, `sys.exit(3)`, the missing-DLL exit 121, the sidecar's
  file set, the stub's system-DLL-only imports (`KERNEL32`, `ntdll`), and
  the in-tree PE import reader's agreement with `llvm-readobj --coff-imports`
  on every bundled image (#1305). Each takes its CPython
  oracle from the bundle's `PYCC-BUNDLE` marker and asserts it is 3.14.7.
  They cover the synthetic oracle program, relocation, `PYTHONPATH`
  isolation, `pycc run`, `sys.exit(3)`, and the freshness of
  `EMBEDDABLE_STDLIB_ROOTS` against `sys.stdlib_module_names`. The oracle
  program prints nothing the D-248 deviations touch and raises nothing, so
  no row is added to the CPython oracle exception list.
- **Locked closure (#1242).** `tests/issue_1242_locked_closure.rs`'s
  non-ignored tests pin the missing-lock refusal (before any interpreter
  runs), a stale lock, `pycc check` needing no lock, and a current
  multi-module lock passing every lock check, through a `sh` fake
  interpreter; `src/lock/build_tests.rs`, `src/embed/lock_tests.rs` and
  `src/embed/closure_tests.rs` cover each lock, interpreter and copy
  refusal, and `src/embed/macos_closure_tests.rs` each closure-image
  relocation arm on `cc`-built images. Its `#[ignore]`d oracle locks a
  `venv --without-pip` holding the test-authored `tinypkg`/`tinydep`,
  builds, moves the venv away, and compares the run with CPython 3.14.7.
  An import under a module-level `try`/`except ImportError` (#1290) is
  pinned the same way: a non-ignored test locks it absent and installed
  and reports a newly installed package as stale, and an `#[ignore]`d
  oracle compares the fallback and the installed run with CPython 3.14.7.
- **Real static archive (#1273).** `tests/issue_1273_real_static_archive.rs`'s
  `#[ignore]`d tests probe the interpreter's `LIBPL/LIBRARY`. When it is a
  genuine ar archive, they build `--static-libpython` executables and
  compare them with CPython 3.14.7: `_json`, `math`, `_random` and `_ssl`
  loaded from `lib-dynload`, and a locked closure. When it is not, they
  assert D-251's refusal instead; on a Linux GitHub Actions leg, whose
  `actions/setup-python` interpreter ships the archive, a missing archive
  fails the test.
- **Bounds.** Every spawn of a built embedded executable uses
  `Command::output()`, whose stdin is null; a manual run should be
  time-bounded with stdin closed, e.g.
  `perl -e 'alarm 60; exec @ARGV' ./app </dev/null` on macOS.

## Hosted `ext` boundary conformance harness (NEG-004, #1067)

[D-244](./decisions/D-244-add-a-hosted-cpython-extension-module-artifact-mode.md)
names NEG-004 as the CPython-driven conformance harness for the hosted `ext`
mode. It is `tests/issue_1067_neg004_ext_conformance.rs`: one `--ext` artifact,
built through the public CLI, then every refusal shape the host call boundary
can produce, plus the properties a single refusal cannot show on its own --
that a refused call left no partial effect behind, that the export is still
correct on the next conforming call, and that a call with two non-conforming
arguments names argument 1.

- **Scope.** [D-244](./decisions/D-244-add-a-hosted-cpython-extension-module-artifact-mode.md)
  rule 7 is what this harness checks. Rule 4's pinned CPython oracle is *not*
  involved: it scopes to conforming calls -- what a compiled function computes
  -- while a refusal is pycc's own contract at the boundary. There is no
  oracle comparison anywhere in this file, and `### CPython oracle exception
  list` above gains no row from it.
- **Loader, not oracle.** The interpreter here only has to import the artifact,
  so it is any CPython at or above rule 1's stable-ABI floor. `PYCC_PYTHON`
  selects it and defaults to `python3`. Both supported versions must agree on
  every message asserted, so the harness is expected to pass unchanged under
  `PYCC_PYTHON=python3.14`; a shape on which they disagree does not belong in
  it as an exact assertion.
- **`PYCC_PYTHON` is read twice.** `src/ext_build.rs` probes it at build time
  for the development headers, and every `ext` test reads it at run time as the
  loader. Building against one version and loading on another is
  `PYCC_PYTHON_INCLUDE`'s job; `PYCC_PYTHON` cannot express that split.
- **Assertion convention.** Text pycc authors is asserted **exactly**. Text
  CPython authors is asserted by exception *type* plus a substring both
  supported versions share, with a comment naming which half authored it --
  pinning CPython's own wording would make a pycc test fail on a CPython patch
  release. Exactly two shapes are CPython-authored today: the keyword-argument
  refusal, which `METH_FASTCALL` dispatch emits before the generated wrapper is
  entered (and which qualifies the name with the module), and the lone-surrogate
  `UnicodeEncodeError` from `PyUnicode_AsUTF8AndSize`.
- **Where it runs.** Every test in the file is
  `#[ignore = "requires a CPython 3.13+ with development headers on PATH"]`,
  because an installed CPython with headers is a property of the machine. CI
  runs it on the Tier-1 `native-build-test` legs, which run the suite with
  `-- --include-ignored` against the 3.13 stable-ABI floor, and once more in
  `build-test-coverage` -- not in that job's `llvm-cov` step, which runs
  without that flag, but in the plain `cargo test --workspace --
  --include-ignored` step that follows it, after CPython 3.14.7 is installed.
  The two are the harness's two supported loaders, so removing either leg
  drops a version this file claims to agree. It earns no line coverage either
  way, because only the `llvm-cov` step is measured: the
  closed-set completeness guard that *is* inside
  `scripts/check_diff_coverage.py`'s denominator is
  `src/ext_build_tests/refusal_completeness.rs`, which is not `#[ignore]`d.
  That guard closes the set along both axes: a new `BoundaryCarrier` variant
  fails to compile against its wildcard-free refusal-arm `match`, and a new
  `Ty` variant -- or an existing one newly admitted past `boundary_carrier`'s
  own `_ => None` -- fails to compile against, or flips an assertion in, its
  wildcard-free admissibility table. One axis alone would let the other drift.
- **Cost.** Each `#[test]` in the file costs one full `pycc build` on every
  Tier-1 leg, so the shapes share a single artifact and a single script rather
  than taking one test each.

## Hosted `ext` benchmark protocol (product-sprint-1)

`docs/ROADMAP.md`'s `product-sprint-1` acceptance turns on one measured ratio,
and
[D-244](./decisions/D-244-add-a-hosted-cpython-extension-module-artifact-mode.md)
rule 6 states that ratio's threshold and its publication rule. This section
fixes the methodology only. It is written before the artifact exists on
purpose: a protocol chosen after a result is seen decides the bet instead of
measuring it, and the same implementation can otherwise pass or fail depending
on how it was timed. The reference codebase is proprietary, so nothing here or
in the report names it or reproduces its source ([D-244](./decisions/D-244-add-a-hosted-cpython-extension-module-artifact-mode.md)
rule 6); only numbers are published.

- **Subject.** One function — the reference codebase's hot loop — byte-identical
  across all three arms. Compiling unchanged is part of the claim, so an arm
  that edits the source to make it compile has failed rather than scored.
  "Byte-identical" is checked, not asserted: the SHA-256 of the subject module's
  bytes is committed as `subject_sha256` in the pre-registration record, and the
  runner verifies it before any arm is built, then reuses the bytes it read so
  the path cannot be edited between arms. Only the digest is committed or
  published — a SHA-256 is a number, while a path or a source line would name
  the proprietary codebase, which [D-244](./decisions/D-244-add-a-hosted-cpython-extension-module-artifact-mode.md)
  rule 6 forbids. The field is `null` in the committed record until the run that
  publishes numbers registers the real digest; both the runner and
  `scripts/check_roadmap_evidence.rb` refuse a `null`, so registering it is that
  run's first action, in a stage commit of its own, and no roadmap box can cite
  this protocol's evidence before then.
  **Annotation-only additions (2026-09-24,
  [D-252](./decisions/D-252-admit-annotation-only-additions-to-a-kill-criterion-subject.md)).**
  On the owner's directive, the kill criterion's subject may carry type
  annotations *added* where none existed -- on a parameter, a return or a local
  -- and nothing else: no statement, expression, default, import or existing
  annotation changes. The addition is published as an exact diff with both
  module digests, `subject_sha256` is the digest of the annotated bytes that all
  three arms then share, and the result is labelled "annotated", never
  "unchanged". The **compile-unchanged count** below is not affected by this.
- **Workload admissibility.** The Subject bullet fixes *which* function is the
  subject once a workload is chosen, but it presumes such a function exists.
  [D-247](./decisions/D-247-pre-register-a-workload-admissibility-predicate-for-the-kill-criterion.md)
  supplies the missing predicate, and it is checked against a profile of the
  candidate workload *before* that workload is adopted. A workload is an
  admissible subject source only when one function in that profile satisfies all
  four clauses: it is the workload's **own** source rather than a dependency's
  or the interpreter's; it is Python rather than a C extension; it is
  loop-bearing by **statement** loops, a comprehension alone not qualifying;
  and it accounts for at least
  **20%** of the profile's summed self time. Exportability is deliberately not
  part of the predicate — whether the subject can be reached from a host is
  already governed by
  [D-244](./decisions/D-244-add-a-hosted-cpython-extension-module-artifact-mode.md)
  rule 1 and by
  [D-038](./decisions/D-038-a-leading-underscore-marks-a-top-level-function.md).
  A workload that fails any clause is refused, and the refusal is a statement
  about that workload, never a result for the compiler.
- **Arms.** Three: the pinned CPython interpreter, Cython, and pycc's `ext`
  artifact. All three run back to back in one session on one machine, on the
  same OS and power profile, with no other timed work on the box. Which machine
  that is, is committed alongside the generator, the seed, and the digest below
  and restated in the report: a run whose arms were split across machines, or
  whose machine was chosen after a result was seen, is inadmissible for the same
  reason the **Input** bullet gives. The runner compares the host's own answers
  against the committed identity before it times anything, and refuses a
  mismatch: `model_identifier`, `cpu`, `cores`, `memory_bytes` and the composed
  `os` string are what the host reports for itself, so those five are verified
  exactly. `power_profile` is not derivable from anything the host exposes and
  remains an operator assertion, restated in the report rather than checked —
  claiming otherwise would report a verification that does not happen.
- **Versions.** The interpreter is the conformance oracle's pinned CPython (see
  "Python 3.14.7 oracle transition" above), GIL-enabled, and it is both the
  baseline arm and the host that imports the `ext` artifact — the loader and the
  comparison are the same interpreter. Cython is pinned at 3.1.6, in
  pure-Python mode with `annotation_typing` enabled and its generated C compiled
  `-O2`. The pycc arm is built `--release`, the shipping profile. The
  interpreter arm is additionally an optimized, non-debug build: a
  `--with-pydebug` or otherwise unoptimized CPython is slower by a multiple,
  which inflates the ratio on its own and can manufacture a passing result out of
  the baseline alone, so a timing taken against one is inadmissible whatever it
  reports. The report carries `python3 -VV` and
  `sysconfig.get_config_var('CONFIGURE_ARGS')` for the interpreter actually used,
  so that build is checkable rather than asserted. Those flags must *positively*
  report `--enable-optimizations`: an ordinary `./configure && make` build denies
  none of the markers above and optimizes nothing, so refusing only the named
  debug markers would admit exactly the slow baseline this bullet rules out.
  A free-threaded build is refused for a second reason of its own: the `ext`
  artifact is built against the GIL-enabled stable ABI and its module
  initializer rejects a free-threaded host outright, so the `ext` arm cannot run
  there at all. The runner therefore refuses an interpreter whose `-VV` banner
  carries the `free-threading build` marker, rather than letting the arm fail
  later at import.
  Every one of these versions is restated in the report, because a later run that
  changes one is a different experiment, and every restatement is checked in one
  of three ways. Five are pinned to a literal value and are compared against it
  -- CPython `3.14.7` in `cpython`, Cython `3.1.6` in `cython`, the Cython build
  mode `pure-python` in `cython_mode`, the C optimization level `-O2` in
  `cython_c_optimization`, and the `release` pycc profile in `pycc_profile`. One
  is a directive rather than a version string: `cython_annotation_typing` must
  be the JSON boolean `true` exactly, so a string `"true"`, a `1` and an absent
  key are all refused -- the protocol pins the directive as enabled, not as
  merely mentioned. The Cython build settings are bound as tightly as the
  version is because that arm's median is the denominator of one of the two
  published ratios: a run that built it in a different mode, with the directive
  off, or without `-O2` measured something the protocol does not describe. The
  remaining two, the `-VV` banner and the `CONFIGURE_ARGS` string, have no
  single pinned value, since they differ per build; what is checked of them is
  the properties this bullet fixes -- the banner reports the pinned CPython and
  is not a `free-threading build`, and the flags carry none of the inadmissible
  markers and positively report `--enable-optimizations`.
- **Input.** Size alone does not pin this workload: the measured loop branches
  on its data, so two evaluators who generate different inputs of the same size
  can reach opposite verdicts on the same implementation. The workload is
  therefore produced by a generator committed with the benchmark, from a fixed
  integer seed, serialized to one file that all three arms read verbatim; the
  run records the generator's path, the seed, and the SHA-256 of that file. Its
  size is the one D-244's Context records for the probe. The generator, the
  seed, and the resulting digest are committed **before** any run may be
  scored, and a timing whose inputs were not already committed when it ran is
  inadmissible -- otherwise an evaluator could try workloads until one clears
  5x and then designate that one as the protocol's first run, which is input
  selection after the result rather than before it. D-244 rule 6's kill
  criterion is evaluated only against runs reporting that committed digest. A
  run that changes the generator, the seed, or the digest is a different
  experiment: report it as a protocol change, never as a comparison against an
  earlier ratio.
- **The pre-registration record is bound to git.** Everything above turns on the
  record predating the run, and a record read from a file the run itself could
  have written proves nothing: a scored invocation could otherwise point
  `--pre-registration` at a freshly generated JSON file whose digests describe
  an input chosen after its performance was seen. The runner therefore reads
  `--pre-registration` as raw bytes and refuses unless they equal the bytes git
  has at `HEAD` for `scripts/bench_hosted_ext_precommit.json`. Bytes are what is
  compared, not the path: bytes equal to the committed blob are the committed
  record wherever they were read from, and bytes that differ are not it even at
  the right path. The binding is exactly that — this record's content is the
  committed content. Whether the rest of the checkout is clean is deliberately
  outside it, since an ordinary working tree carries modifications this record
  does not own, and a refusal on those would not distinguish a rewritten record
  from ordinary development.
- **Correctness precondition.** An arm that is faster and wrong scores nothing:
  it is reported as a failure, never as a ratio. The conformance harness's
  comparison is not the one that decides that here --
  `run_conformance_fixture_with_profile` in `tests/conformance.rs` builds and
  runs whole programs, requires both processes to exit successfully, and diffs
  their stdout, so it cannot see a value returned to a caller and cannot tell
  two exception types apart; an arm that returned a wrong number or raised the
  wrong error would pass it unchanged. The comparison is instead made in the
  host interpreter, where all three arms already return a Python object on the
  same committed input, and it sits outside the **Timing boundary** below
  because the clock stops when the call returns. Against the CPython arm, each
  arm must return the same type; integer and boolean results must be equal
  exactly, with no tolerance; a float result must agree within a tolerance
  committed alongside the generator, the seed, and the digest **before** any run
  may be scored, on the same grounds the **Input** bullet gives -- a tolerance
  chosen after the difference is seen is selection after the result, and the
  arms are not bit-comparable in principle, since `-O2` on the Cython arm and
  LLVM on the `--release` pycc arm may both contract `a*b - c*d` to an FMA where
  the interpreter does not. A divergence beyond that committed tolerance is a
  failure, not a ratio. A non-finite float result -- a NaN or an infinity, on
  either arm -- is a failure too, never an agreement: a tolerance comparison
  against a NaN is false whatever the difference, so a silently non-finite arm
  would otherwise pass the precondition by arithmetic rather than by agreeing. An arm that raises where the CPython arm returns, or
  raises a different exception type, has failed. The two exception types are
  compared as classes rather than by name, so an arm that raises its own
  namesake of the class the CPython arm raised has failed as well. The
  committed tolerance itself must be a finite, non-negative number: JSON admits
  `1e999`, which would parse as an infinity that admits every divergence, so a
  record whose tolerance cannot bound anything is refused before any arm runs. This precondition applies to
  every call whose duration is reported, not only to the warm-up: each timed
  invocation's outcome and its post-call argument state are validated against
  that arm's own warm-up under the same committed tolerance before the duration
  is admitted into the median. An arm that answers correctly once and then
  raises, or returns something else, on all seven measured calls would otherwise
  publish an entirely ordinary-looking median. The arguments must also be
  unchanged after the call, or changed exactly as the CPython arm changes them,
  so that an arm cannot be faster for having clobbered the committed input. That
  holds on the failing path as well: the arguments are compared as they stand
  after the call whether it returned or raised, so an arm cannot clobber its
  input and then hide behind raising the same exception type the CPython arm
  raises.
- **Warm-up.** One untimed full run per arm before any timed run, so page
  faults, dynamic-loader work, and the interpreter's own caches are paid outside
  the measurement. For the `ext` arm that untimed run also pays the import and
  the first call through each export wrapper.
- **Replicates and statistic.** Seven timed runs per arm; the arm's figure is
  the median of its seven, and each speedup is a ratio of medians. Report the
  per-arm minimum and maximum alongside it. Never a mean — one descheduled run
  moves a mean and leaves a median where it was.
- **Timing boundary.** The clock runs inside the host interpreter:
  `time.perf_counter_ns()` immediately before the call into the function under
  test and immediately after it returns. The export wrappers' argument unpacking
  and result packing are therefore inside the boundary, because a caller pays
  them, while import, module load, argument construction, and building the
  artifact are outside it for every arm. Argument construction happens outside
  the boundary *for every invocation*, not once per arm: the arguments are
  rebuilt from the committed input before the warm-up and before each of the
  seven replicates. The **Correctness precondition** bullet permits an arm to
  mutate its arguments as long as every arm mutates them identically, so reusing
  one argument tuple would let the warm-up change the workload every timed call
  then sees, and let each replicate run on data the previous one had already
  modified. Cross-arm comparison is unaffected, because the construction is
  deterministic from the committed input. "The committed input" is the open file
  handle the digest was taken through, never the path: the runner verifies
  `input_sha256` through a handle it then keeps, and every rebuild seeks that
  handle back to zero rather than reopening `--input`. A path checked once and
  reopened per invocation is a different object as soon as it is replaced in
  between, and all three arms would then agree with each other on data nobody
  committed while the report still carried the pre-registered digest.
- **Reporting.** The report publishes the three medians, their minima and
  maxima, both ratios (versus CPython and versus Cython), the replicate count,
  the machine and OS, and the pinned versions above -- `cpython`, `cpython_vv`,
  `cpython_configure_args`, `cython`, `cython_mode`,
  `cython_annotation_typing`, `cython_c_optimization` and `pycc_profile`, under
  a `versions` object. The threshold those numbers
  are judged against is D-244 rule 6's and is not restated here. The report is
  one JSON document at `docs/benchmarks/hosted-ext-product-sprint-1.json`, and
  it restates the `input_sha256`, the `subject_sha256`, the
  `compile_unchanged_denominator`, the `compile_unchanged_set_sha256` and the
  machine identity committed in `scripts/bench_hosted_ext_precommit.json`, and
  carries the `compile_unchanged_count` the next bullet defines -- a
  report that does not carry the committed values is a different experiment, not
  this one's result. That count must be positive whenever the hot-function
  acceptance item is claimed, since that item asserts a reference function did
  compile unchanged; the numbers-published item publishes the count whatever it
  is, zero included. That path
  and every field above are what `scripts/check_roadmap_evidence.rb` requires
  before either `product-sprint-1` roadmap box may cite its evidence
  identifier, so an absent report, a missing field, a digest that does not match
  the pre-registration record, a restated version that contradicts its pin, or a
  speedup below D-244 rule 6's threshold all fail the roadmap evidence gate
  rather than passing silently. An unchecked box claims nothing and so validates
  nothing: the checker collects an evidence identifier only from a checked item,
  and these requirements are what checking the box must satisfy. The speedup that threshold
  judges is the ratio derived from the published medians, not the ratio the
  report prints for itself: the printed ratio is checked for consistency with
  those medians and rounding is tolerated there, so judging the threshold
  against it would let a rounded-up number clear a bar the measurement does
  not.
- **The compile-unchanged count.** `product-sprint-1`'s second acceptance item
  is a count, not a timing: how many of the reference codebase's annotated
  functions compile unchanged as part of an `ext` artifact, over how many were
  attempted. A function counts only with its source byte-identical to the
  original; both numbers are published, never the ratio alone. The denominator is
  fixed before the evaluation, not after it: the number of annotated functions to
  be attempted and a digest over that exact set are committed **before** any
  compile may be scored, on the same grounds the **Input** bullet gives for a
  timing — otherwise an evaluator could attempt functions until the count reads
  well and then designate those as the set. What is committed is that count and
  that digest, never a list of function names: the reference codebase is
  proprietary and only numbers are published (D-244 rule 6).
- **Implementation.** `scripts/bench_hosted_ext.py` runs this protocol and
  enforces the refusals above; `scripts/gen_hosted_ext_input.py` is the
  committed generator; `scripts/bench_hosted_ext_precommit.json` is the
  pre-registration record the **Input**, **Arms** and **Correctness
  precondition** bullets require; and `scripts/enumerate_annotated_functions.py`
  derives the compile-unchanged denominator and its digest, stating its
  enumeration predicate in full; and `scripts/check_roadmap_evidence.rb` binds
  the roadmap's two `product-sprint-1` acceptance items to the published report
  the **Reporting** bullet fixes. Two environment variables configure a run, and
  are defined here rather than in [CLI_SPEC.md](./CLI_SPEC.md) because neither
  is a `pycc` command-line variable: `PYCC_BENCH_SUBJECT` is the path to the
  subject function's module, outside this repository, and `PYCC_BENCH_PYTHON`
  is the interpreter to time against, falling back to `PYCC_PYTHON`.


### Status: the replacement workload is selected and its annotated subject is blocked on compiler gaps and a boundary question

This subsection was titled "the protocol has no admissible subject" until
2026-09-23; D-244's 2026-09-17 amendment for #1116 cites it by that title.
From 2026-09-23 to 2026-09-24 it was titled "the replacement workload is
selected and the criterion is recorded as not met".

**Current state (2026-09-24).** On 2026-09-24 the repository owner directed
that the row (b) outcome below is a chicken-and-egg result and that the missing
annotation should be added.
[D-252](./decisions/D-252-admit-annotation-only-additions-to-a-kill-criterion-subject.md)
records that directive: a kill-criterion subject may carry annotation-only
additions, published as a diff, and the result is labelled "annotated". For
`lark` the whole addition is one annotation, applied to a copy of the pinned
`f79772cd` tree:

```diff
--- lark/parsers/lalr_parser_state.py
+++ lark/parsers/lalr_parser_state.py
@@ -64,7 +64,7 @@
             deepcopy(self.value_stack) if deepcopy_values else copy(self.value_stack),
         )
 
-    def feed_token(self, token: Token, is_end=False) -> Any:
+    def feed_token(self, token: Token, is_end: bool = False) -> Any:
         state_stack = self.state_stack
         value_stack = self.value_stack
         states = self.parse_conf.states
```

The module's SHA-256 is
`419d76a780adbdcbc5008288aa4783049620114de7b509650490d85dbe35f65b` before the
addition and `4335a1995da91fa264b3f16ebbb0c882863d0aba5d7d219205752fbf8a8680d9`
after it. With the subject fully annotated, row (b) no longer governs and row
(c) does: what blocks the subject is worked until 2026-10-22, and whatever is
still open then is the recorded miss. Two kinds of blocker stand. The first is
the compiler gaps in the subject's import closure. The second is a boundary
question: the subject's own `-> Any` return, which is inferred (not observed)
to meet `T0002`, as explained below.

Row (c) also requires each gap to be filed in `product-sprint-1`. Here is how
that stands:

- The gaps that had no open issue were filed there on 2026-09-24 as #1278,
  #1279, #1280, #1282, #1283 and #1284.
- #882, already in `product-sprint-1`, covers `collections` and the `typing`
  symbols.
- The boundary question is #1285, also in `product-sprint-1`.
- Four gaps are covered by pre-existing issues in the `v0.4` milestone (#884,
  #886, #887, #889). They are therefore **not** filed in `product-sprint-1` as
  row (c) literally requires. They are cross-referenced here rather than
  re-milestoned, because each of them is broader than this workload's use.
  Whether they count as worked for this deadline is recorded at the deadline.

The annotated module was compiled with `pycc build <module> -o <out>.abi3.so
--ext` at pycc `20c2c76d` (release build, `PYCC_PYTHON` the uv CPython
3.14.7). The build exits 1. The same command on the unedited module fails with
the same diagnostics: the two outputs are identical apart from the directory
name. The annotation was not the binding constraint. A row (a) shim entry
module that only imports `ParserState` fails identically too. pycc stops at the
first failing module of the subject's import closure. That module is
`lark/utils.py`, which `lark/lexer.py` imports, and it carries 18 errors. The
first is

```
error[C0001]: import of module `itertools` is not supported yet
 --> lark/utils.py:3:1
```

Only two of the 18 (`T0001` on the unannotated parameters of
`combine_alternatives` and `bfs_all_unique`) could be fixed by annotation-only
additions, and D-252 admits additions only to the subject module, not to its
import closure. The other 16 could not be fixed that way either, so no further
edit was made:

| Diagnostic in `lark/utils.py` | Count | Nearest open issue |
|---|---|---|
| `C0001` import of `itertools` / `collections` not supported yet | 0 (was 1 each) | #1278 admits an unaliased, top-level `from X import a, b` of an undotted foreign module, so `from itertools import product` (line 3) and `from collections import deque` (line 4) now compile, each binding CPython's own object; #882 still tracks a native `collections`. The same `pycc build <module> -o <out>.abi3.so --ext` command (release build of the #1278 change on top of `main` at `17189dde`, on the unedited subject module) reports 13 errors, all still in `lark/utils.py`; the first is now the `C0002` for `typing.Callable` (line 5). The `product` use site (`return list(product(*lists))`, line 300) still reports only the `T0001` on `lists` in that run. Separate probes built with the same command (one with `lists` annotated, one per construct) measure the blockers behind it: the starred argument (`C0001 a starred expression`, which is what the annotated copy of line 300 reports first), `list(...)` (`C0001 call to builtin list`), and the direct call of an `object`-typed name, which [#1313](https://github.com/rotnov/pycc/issues/1313) admits in a module body below the import with positional scalar arguments. Line 300 sits inside a function body, where [#1316](https://github.com/rotnov/pycc/issues/1316) now admits a read of `product`, a direct call included: a release build of the #1316 branch (on top of `main` at `5770ebf0`) compiles a probe whose function body returns `len(product(a, a))` or `str(product(a, a))`, which was `I0404` before. The whole-module run is unchanged at 13 errors, byte for byte, because the `T0001` on `lists` still masks the site, and the use site stays blocked on the starred argument (`C0001 a starred expression`), `list(...)` of an object (`C0001 call to builtin list`) and the `list[list[int]]` parameter type (`T0034`), each measured by its own probe. [#1325](https://github.com/rotnov/pycc/issues/1325) admits binding an `object` to a *module-level* name and a bare-name `for` over it: a debug build of the #1325 branch (on top of `main` at `cb05ff2e`) builds a probe `x = product("ab", "c")` followed by `for t in x: print(str(t))` with `--ext`, and importing it prints CPython's own `('a', 'c')` and `('b', 'c')`. It does not reach line 300, which is inside a function body: the same build's probe binding `p = product(a, a)` in a function body is still `I0404` ("binding a CPython object to a name"), which [#1333](https://github.com/rotnov/pycc/issues/1333) tracks. [#1340](https://github.com/rotnov/pycc/issues/1340) admits printing the object itself: a debug build of the #1340 branch (on top of `main` at `21dcf2ce`) builds the same probe with `print(t)` in place of `print(str(t))`, and `tests/issue_1340_print_object.rs` pins its output against CPython 3.14.7; that adds nothing toward line 300 |
| `C0002` `typing` has no importable `Callable` / `Generic` | 1 each | #882 |
| `C0001` only a single module per `import` statement (`import sys, re`) | 1 | #1280, closed: `import sys, re` is now accepted; the same `pycc build <module> -o <out>.abi3.so --ext` command (release build at the #1280 branch head `54a0fa93`, on the unedited subject module, which fails identically) reports 17 errors, all still in `lark/utils.py` |
| `C0001` `import` inside a block body (module-level `try`/`if`) | 0 (was 3) | #1282; #1291 (Part 1) admits an undotted foreign import in a module-level `if`/`try` body, so `import regex` (line 120) and `import atomicwrites` (line 303) now compile. Both sit in `try: ... except ImportError:`; that is compile-time only for this workload, since lark still stops at `lark/utils.py`, but since #1293 such a handler runs when the module is absent, as in CPython (`tests/issue_1293_import_bridge.rs`). The same `pycc build <module> -o <out>.abi3.so --ext` command (release build of the #1291 change on top of `main` at `cb2ed87a`, which includes #1292's `ImportError` builtins, on the unedited subject module) reports 15 errors, all still in `lark/utils.py` |
| `C0001` import of module `re._parser` (`import re._parser as sre_parse`, line 126, in the `if` body) | 1 | #1138 (dotted foreign submodules) and #1282, whose third occurrence this is: before #1291 it was the block-body `C0001`, and the dotted name now fails on its own |
| `C0001` attribute-expression annotation (`logging.Logger`) | 1 | #889 (v0.4) |
| `C0001` keyword call arguments (`TypeVar("_T", bound=...)`) | 1 | #884 (v0.4) |
| `C0001` `@dataclass` with options | 1 | #887 (v0.4) |
| `C0001` attribute-form base class | 1 | #886 (v0.4) |
| `C0001` class inherits from builtin type `frozenset` | 1 | #1319 (Part 2 of #1283) carries support; #1318 (Part 1) only made the message honest -- it now names the builtin type instead of calling `frozenset` an unknown class -- and #1283 stays open. `fzset` is defined only in `lark/utils.py` (line 319) and used in `lark/parsers/grammar_analysis.py` and `lark/parsers/lalr_analysis.py`. The same `pycc build <module> -o <out>.abi3.so --ext` command (release builds of `main` at `d59fde73` and of the #1318 change on top of it, on the unedited subject module from the local 1.3.1 archive) reports 13 errors at both; only this line's text differs. #1319 carries the support in three parts: Part 1 ([#1326](https://github.com/rotnov/pycc/issues/1326)) ships the native `frozenset[int]` value type the subclass will build on, and leaves this line's diagnostic unchanged -- the same command still reports 13 errors, all in `lark/utils.py` |
| `C0001` class attribute initialised with a non-literal | 1 | #1284 |
| `T0002` `Any` outside a declared interop boundary | 2 | none: internal `Any` use inside a dependency, refused by design; unlike the subject's own `-> Any` (#1285) it is not at the timed boundary |
| `T0001` unannotated public parameter | 2 | annotation-fixable, but outside D-252's scope (a closure module) |

This list is a lower bound. `lark/lexer.py`, `lark/common.py`,
`lark/parsers/lalr_analysis.py`, `lark/exceptions.py` and the subject module
itself were never reached. A separate probe compiled only the subject module's
own first two lines, `from copy import deepcopy, copy` and `from typing import
Dict, Any, Generic, List`. That probe is not the workload. It was refused with
``C0001 import of module `copy` is not supported yet`` (#1279) and ``C0002 module
`typing` has no importable symbol named `Dict` ``. Since #1278 the `copy` line
compiles as a foreign import, and the probe's only remaining error is the
`typing.Dict` `C0002`. The subject's own `-> Any`
return would also meet `T0002` (#1285); that is an inference, since
compilation never reached the subject. Replacing it is not an addition, so D-252 does
not admit that edit. That makes it a second blocker, independent of the
import-closure gaps: closing every gap in the table would still leave the
subject refused, unless pycc comes to admit an `Any` return on a method that
the host reaches only through a shim. That is a D-244 boundary question, not a
missing feature.

No timing was taken, and none can be until the subject compiles. The
pre-registration commit and the protocol report remain unwritten. Two points
bear on the ≥ 5× expectation even once every gap closes:

- Nearly every operation in the loop body is on an object pycc does not own.
  The body subscripts a `dict` of `dict`s keyed by the generic `StateT` and the
  foreign `Token.type`. It calls arbitrary Python callbacks from a `dict`, tests
  `action is Shift`, appends to untyped stacks and returns `Any`.
- The `ext` boundary admits only scalars and small tuples. A row (a) shim would
  therefore have to build the `ParserState` and `Token` inside the timed call.

**State on 2026-09-23 (row (b); superseded 2026-09-24 by D-252).**
The replacement-workload selection that
[#1207](https://github.com/rotnov/pycc/issues/1207) pre-registered under
[D-247](./decisions/D-247-pre-register-a-workload-admissibility-predicate-for-the-kill-criterion.md)
ran on 2026-09-23, and [its result comment](https://github.com/rotnov/pycc/issues/1207#issuecomment-5791727061) is the outcome record. Step 0
refused no candidate. `idna` 3.20 was refused under clause (iv): its rank-1
function by self time was own-Python-with-statement-loop at 11.5%, 11.4% and
11.6% across the three runs. `lark` 1.3.1 was admissible, so it is the
workload and the stop rule ended the selection: its subject is
`ParserState.feed_token` in `lark/parsers/lalr_parser_state.py`, at 21.2%,
21.9% and 21.2% of summed self time. That subject's signature is
`feed_token(self, token: Token, is_end=False) -> Any`, and `is_end` is not
annotated, so #1207's pre-registered outcome row (b) applies: **criterion not
met: subject not compilable unchanged (not fully annotated)**. Row (a) (a
method, measured through a shim) also applies but is moot. Row (b) records the
miss without a timed run, so no timing exists or will be taken: no
pre-registration commit for `lark` follows,
`scripts/bench_hosted_ext_precommit.json` still carries the refused reference
workload's fields with `subject_sha256` `null`, and no
`docs/benchmarks/hosted-ext-product-sprint-1.json` report will be written.
D-244 rule 6's kill criterion is therefore recorded as not met. The selection's
instruments are committed as they ran under `scripts/workload_selection/`,
whose README lists the two harness deviations the result comment discloses.

The selection made one correction that applies to this protocol's own text.
The **Versions** bullet, `scripts/bench_hosted_ext.py` and #1207's section 5
all read `sysconfig.get_config_var('CONFIGURE_ARGS')`. No CPython build defines that
variable: it is `None` on 3.14.6, 3.13.9, 3.9.6 and 3.14.7, so the runner as
written refuses every interpreter. The variable CPython defines is
`CONFIG_ARGS`. The selection checked that variable instead, and the uv 3.14.7
build it used carries `--enable-optimizations` there. The bullet and the
runner are left as they are, because no run follows.

The rest of this subsection is the record that led to the selection.
Nothing in the protocol above is amended by this subsection. The protocol has been
amended exactly once since it was committed: the **Workload admissibility**
bullet, added on 2026-09-22 by
[D-247](./decisions/D-247-pre-register-a-workload-admissibility-predicate-for-the-kill-criterion.md),
which supplies a predicate the protocol presumed rather than revising how a
chosen subject is measured. (That count held until 2026-09-24, when
[D-252](./decisions/D-252-admit-annotation-only-additions-to-a-kill-criterion-subject.md)
added the Subject bullet's **Annotation-only additions** paragraph, a second
amendment.) `subject_sha256` is still `null`; the record's
only amended field is `machine.os`, re-pinned on 2026-09-21 and recorded there
as `machine_os_amendment` (prerequisite 3 below), and no other field has
changed.
This records why no run has been scored against it, so that a later session
does not re-derive the same findings. It was corrected on 2026-09-17, when the
count prerequisite 2 reports was measured rather than asserted, and again on
2026-09-18, when a re-measurement against `f7f8748c` corrected that count
itself. A second correction the same day withdrew the parameter-shape
conclusion that first re-measurement reached: it characterized only the
boundary-admissible subset, which is selected by the very property it was
being read for. A third, the same day again, narrowed that second one: its
replacement claim — that the remaining work is compiler-side rather than
workload-side — overstated the scan in the opposite direction, because the
scan's array-like spelling list counted `list[int]` as a carrier. Both
blockers stand. A fourth, again the same day, narrowed the third: the second
blocker was called a property of the workload that no compiler change reaches,
but every scan behind it enumerated module-level functions only — the same
predicate the export rule uses — so it could not see methods. Re-run over the
bodies of public classes, exactly one public method takes a numpy-array
parameter and contains loops, and it is unreachable only because of where it
is defined. That is a fifth boundary gap (#1131), not a workload property.
A fifth correction, on 2026-09-22, settles the question the closing
paragraph below left open — what the protocol takes as its subject — by
pre-registering an admissibility predicate for the *workload* rather than
revising the Subject bullet:
[D-247](./decisions/D-247-pre-register-a-workload-admissibility-predicate-for-the-kill-criterion.md),
whose operational form is the **Workload admissibility** bullet above. Under
it the reference workload is refused, so prerequisite 2 is no longer a
prerequisite of this workload's run at all: there is no run to prepare here.
The replacement D-247 admits was then selected under #1207, with the outcome
stated at the head of this subsection.
Each correction is dated in place below; none is deleted.

A scored run needs one function that is byte-identical across the three arms
and that the `ext` arm can actually export. Two readings of what one replicate
times are possible, and the boundary blocks only one of them:

- **The sweep reading** — a replicate is one call that consumes the whole
  committed input — needs the subject to read the ~128 MB file itself.
  `pycc` has no `open`: a sweep-shaped subject compiled with `pycc build
  --ext` is rejected with ``error[C0001]: call to builtin `open` is valid
  Python but not implemented yet``, and no file-reading builtin is registered
  in `crates/pycc_std`.
- **The per-record reading** — a replicate is one call per record, with one
  record's fields handed in as arguments and the host looping over the
  records — **is** admissible at the current boundary. The `ext` boundary
  admits `int`, `float`, `bool`, `str` and fixed-arity tuples of
  `int`/`float`/`bool` as parameters and returns those or `None`, and a
  triangle plus its query point is eight `float` arguments returning a
  `bool` — exactly the shape the measurement below already exports and
  times. What the boundary rejects is handing a record, or the whole input,
  across as a *container*: `bytes` with `C0001`, `list[float]` with `T0034`,
  `tuple[float, ...]` with `T0053`, and a `list[int]` parameter with `C0003`.
  That the input holds 2,000,000 records does not make one record's signature
  inadmissible; it only means the host, not the subject, does the iterating.

Three prerequisites were recorded here; the first is now met and two
remain. The first two are stated per reading — they are not the same for
the sweep and the per-record shapes — and the third is independent of both:

1. **The sweep reading needs the boundary to be able to carry the committed
   input in one call.** That is the buffer-protocol bridge (#1027), which
   gives a subject a `memoryview` parameter whose elements compile to native
   loads — the one signature shape that expresses "2,000,000 triangles plus
   their query points" as a single call's argument. #1027 in turn depends on
   foreign imports (#1026). The carrier half is met: Part 1
   ([#1112](https://github.com/rotnov/pycc/issues/1112)) admitted the parameter
   and the boundary that carries it, and Part 2
   ([#1113](https://github.com/rotnov/pycc/issues/1113)) added the element load
   itself — `b[i]`, a bounds-checked native `float` load — so the committed
   input does now cross in one call and its elements do compile to native
   loads. The length half is met as well, by
   [#1116](https://github.com/rotnov/pycc/issues/1116): `len(b)` answers the
   buffer's element count as an `int` instead of the `C0001` capability gap it
   was, so a subject can now write the `for i in range(len(b)):` loop that
   reads the whole buffer — the shape this protocol's sweep reading actually
   needs — and the whole committed input crosses the boundary in one call.
   This prerequisite is therefore met, for the sweep reading as well as the
   per-record one the boundary already admitted; what still refuses a scored
   run is prerequisites 2 and 3, not the bridge.
   Annotated 2026-09-18 ([#1129](https://github.com/rotnov/pycc/issues/1129)):
   the carrier was widened, not extended. The boundary's first refusal arm
   moved from `PyMemoryView_Check` to `PyObject_CheckBuffer` and the bare name
   `ndarray` became a second spelling of the same parameter type, so a
   conforming array now reaches the admitted slot without the
   `memoryview(a.reshape(-1))` a host had to write at the call site. That
   removes a wrapper; it does not change what this prerequisite reports, which
   was already met, and it does not bear on prerequisite 2 — no count below is
   re-measured by it, and none is restated here.
   Annotated 2026-09-20 (Part 1 of [#1142](https://github.com/rotnov/pycc/issues/1142)):
   the buffer gained an element **store**, `b[i] = v`, so a subject may now
   write its result back through a buffer parameter instead of only reading
   one. This prerequisite reports on the *ingress* of the committed input,
   which was already met, so the store does not change it either; it is
   recorded here because the same annotation's absence would leave the
   operation set above reading as two operations when it is three. No count
   under prerequisite 2 is re-measured by it, and none is restated here — the
   denominator enumerates module-level functions by signature, and a store in
   a body changes no signature.
2. **Either reading needs an admissible subject to exist at all**, and none
   does. Reported as counts, so that nothing about the proprietary codebase is
   published beyond them ([D-244](./decisions/D-244-add-a-hosted-cpython-extension-module-artifact-mode.md) rule 6). Of the **133** module-level fully
   annotated functions the pre-registered denominator enumerates, **101** pass
   [D-244](./decisions/D-244-add-a-hosted-cpython-extension-module-artifact-mode.md) rule 1's export test, **23** of those also have a signature the current
   `ext` boundary admits, and **0** of those 23 compile and export under `pycc
   build --ext` (verified by importing the built artifact, not inferred from the
   build's exit status).

   This corrects the figure recorded on 2026-09-17, which read **3**. The drop
   is not a regression: pycc rebuilt at `16cc340b` — the last commit before
   #1116/#1121/#1054/#1125 — rejects those same three subjects with the same
   diagnostic counts as `f7f8748c` does, and `921c548a` (project imports
   resolved across files) landed 2026-09-03, two weeks before that triage. The
   earlier count came from compiling subjects in extracted or isolated form
   rather than through their containing module.

   Two further corrections follow from the re-measurement. First, the failures
   are overwhelmingly **not in the subjects**: 22 of the 23 have zero
   diagnostics in their own line range, and the dominant families are
   dependency-level import gaps (`C0001` import of module, `C0002` no
   importable symbol). Second, that histogram measures **what pycc reports
   first**, not the full set of capability gaps — `54738cca` emits one
   diagnostic per top-level item and aborts before lowering a body whose
   transitive dependency already failed, so the depth behind the first wall is
   unmeasured and the family list must not be read as a queue with an end. The
   full inventory, with templated messages, synthetic reproducers and the
   tracker mapping, is published on
   [#1039](https://github.com/rotnov/pycc/issues/1039#issuecomment-5724843390).

   What fails the **Subject** bullet's other half is now measured directly, and
   it is independent of compilation: across all 23 admissible subjects the
   parameters are drawn only from `str` and `int` or absent entirely, with
   **zero `memoryview` and zero `tuple` parameters**, and the only three that
   contain loops take **zero parameters**. No *admissible* subject can consume
   the committed input — 2,000,000 `float` triangles — whatever the compiler
   learns next. Timing one of the parameterless or `str`-taking functions would
   be the post-hoc subject substitution the **Input** bullet exists to forbid,
   and an exported `str` parameter leaks one object per call
   ([D-244](./decisions/D-244-add-a-hosted-cpython-extension-module-artifact-mode.md)'s 2026-09-13 amendment), so a long timed loop would be
   corrupted as well as meaningless.

   **Corrected on 2026-09-18: that measurement characterizes the wrong subset,
   and the conclusion drawn from it was wrong.** The 23 are selected *by*
   boundary-admissibility, so a function whose parameter is annotated with any
   array spelling the boundary does not admit is filtered out at the 101 → 23
   step before parameter shapes are ever counted — the absence of a carrier
   among the 23 is partly tautological. Re-scanning parameter annotations across
   the full pre-registered denominator instead: **10 of the 133** take at least
   one array-like parameter, **6** of those are public, undecorated, module-level
   and fully annotated (so they pass rule 1's export test and sit inside the
   101), and **3** contain loops. None spells its carrier `memoryview`; the
   spellings are `NDArray[np.floating[Any]]` (11 occurrences across the
   denominator), `npt.NDArray[...]` (5), `np.ndarray` (1), `NDArray[np.floating]`
   (1) and `NDArray[np.bool_]` (1).

   So the asymmetry is **not** that pycc has the buffer-parameter shape and the
   workload has no function that uses it. The workload has such functions; the
   boundary does not admit the way they spell it. Four separate gaps stand
   between them, each verified against `5e96f065` with a synthetic reproducer
   and each tracked: the aliased import form `import numpy as np` is rejected
   while the bare `import numpy` is accepted (import aliasing, #883; since
   #1291 `import numpy as np` binds `np` to the CPython module object as a
   foreign import, and the census was not re-measured); `from
   numpy.typing import NDArray` is rejected (`C0001`, the #882 family); an
   attribute-form annotation `np.ndarray` is rejected (#889), as is a
   subscripted one — `T0044`, the annotated class defining no
   `__class_getitem__` (#1130); and `ndarray` is not registered as a buffer
   carrier at all, which is the part #1027 delivered for `memoryview` only
   (#1129). `def f(a:
   memoryview, n: int) -> float` builds clean under `pycc build --ext` at the
   same commit, which is what isolates the gap to the spelling rather than to
   the carrier machinery.

   The operative consequence for this protocol is unchanged — prerequisite 2 is
   still unmet, because a subject must *compile* as well as be shaped right, and
   these six sit behind the same import wall as everything else.

   **Second correction, the same day: "the remaining work is compiler-side, not
   workload-side" overstates it in the other direction, and the same scan
   refutes it.** The 10 rows were selected by an `ARRAYISH` spelling list that
   included `list[int]` and `list[float]`, which are already-compiled scalar
   sequences (D-105) and not buffer carriers at all. Re-counting the 6 public
   rows by carrier kind and by loop content separately: **4** take a genuine
   numpy-array parameter and **all 4 are loop-free** (4-11 calls over 16-78
   lines — thin wrappers that delegate into numpy/scipy C code, so compiling one
   would time nothing); **2** are loop-bearing but their array-like parameters
   are `list[int]`, which the boundary's carrier gaps do not touch; **0** are
   public, take a numpy-array parameter and contain a loop; and **0** take a
   buffer-protocol parameter of any spelling. The loop-bearing row that returns
   an `NDArray` returns one — it does not take one.

   So neither the original claim nor the first correction is right. Both
   blockers stand, and they are independent: the compiler does not admit the
   way the workload spells an array parameter (#883, the #882 family, #889,
   #1130, #1129), **and** closing every one of those gaps would still yield no
   scorable subject, because no public function in the denominator both takes
   an array and contains a loop. The first is tracked and closeable.

   **Third correction, the same day again: the second blocker is not a
   workload property either.** Calling it one rested on a scan that enumerated
   module-level `def`s — the same predicate D-244 rule 1 uses to define the
   export set — so it was blind to methods by construction, which is the same
   defect as reading a population property off a set selected by that very
   property. Re-running the identical predicates over the bodies of public
   classes gives, numbers only per D-244 rule 6: **1485** methods in public
   classes, **20** public and fully annotated, **8** of those loop-bearing,
   **1** taking a numpy-array parameter, **1** taking a numpy-array parameter
   *and* loop-bearing, **0** taking a buffer-protocol parameter. That single
   row is 434 lines with 9 loops over 83 calls — by size and loop content the
   most plausible scoring subject found anywhere in the workload, and it is
   inadmissible purely because D-244 rule 1 exports module-level functions
   only. That is a compiler-side gap like the other four, now tracked as
   #1131, and closing it is not sufficient on its own: the row spells its
   array parameter as a PEP 604 union, which #747 tracks as entirely
   unimplemented. Prerequisite 2 stays unmet either way, and the denominator
   claim above is unchanged — it is a statement about module-level functions,
   which is what the export rule selects today.

   **Fourth correction (2026-09-18): two of the four gaps above have moved,
   and a third was never stated correctly.** (a) The subscripted-annotation
   clause — "as is a subscripted one — `T0044`, the annotated class defining
   no `__class_getitem__` (#1130)" — is false as of
   [#1130](https://github.com/rotnov/pycc/issues/1130): a subscripted
   annotation whose base resolves to a nameable type is now accepted with the
   type argument erased, so that gap is closed. Nothing downstream changes,
   because every one of the 19 occurrences also trips at least one of the
   remaining gaps. (b) Clause 4 — "`ndarray` is not registered as a buffer
   carrier at all" — was already stale when it was written: commit
   `4d5a9677` ([#1129](https://github.com/rotnov/pycc/issues/1129)) registered
   `ndarray` as a second spelling of the carrier, and the dated note at
   prerequisite 1 disclaimed any bearing on prerequisite 2 rather than
   correcting it. What remains is that `NDArray` — the capitalized
   `numpy.typing` spelling all 18 subscripted occurrences use — is not
   registered, tracked as a follow-up to #1130. (c) The clause "`from
   numpy.typing import NDArray` is rejected (`C0001`, the #882 family)"
   misattributes the refusal: numpy is not stdlib, so it comes from the
   foreign-import path, not the `pycc_std` registry #882 widens. Correcting
   that clause is not #1130's business and it is left as written here; the
   same follow-up issue carries it. **The gap enumeration repeated in the
   second correction above as "(#883, the #882 family, #889, #1130, #1129)"
   is stale in the same two ways** and is read through this note rather than
   restated there.

   **Fifth correction (2026-09-18): the two clauses the fourth correction
   diagnosed but declined to fix are corrected here, and one of its own
   closing claims has gone stale.** (a) Clause 4 of the gap list — "`ndarray`
   is not registered as a buffer carrier at all" — is superseded: commit
   `4d5a9677` ([#1129](https://github.com/rotnov/pycc/issues/1129)) registered
   `ndarray`, and [#1134](https://github.com/rotnov/pycc/issues/1134) has now
   registered `NDArray`, the capitalized `numpy.typing` spelling 18 of the 19
   occurrences use, as a third source spelling of the same carrier. Read
   clause 4 as closed. (b) The fourth correction's own closing sentence —
   "What remains is that `NDArray` … is not registered, tracked as a
   follow-up to #1130" — is the claim #1134 retires. (c) The clause "`from
   numpy.typing import NDArray` is rejected (`C0001`, the #882 family)" is
   corrected as diagnosed: numpy is not stdlib, so that refusal comes from
   the **foreign-import** path and not from the `pycc_std` registry
   [#882](https://github.com/rotnov/pycc/issues/882) widens. Widening
   `pycc_std` would not admit it, and no #882-family issue closes that gap.
   Read the clause as naming the foreign-import path wherever the `#882`
   attribution appears above.

   **What this does not change: the count stays 19, and the numerator does
   not move.** Registering `NDArray` makes none of the 19 array-parameter
   occurrences compile. Each of them still needs a name binding that does
   not exist: `from numpy.typing import NDArray` (the foreign-import path,
   per (c) above), `import numpy as np`
   ([#883](https://github.com/rotnov/pycc/issues/883); since
   [#1291](https://github.com/rotnov/pycc/issues/1291) this binding exists
   as a foreign import, but an array parameter still needs a spelling the
   boundary admits, and the census was not re-measured), or an attribute-form
   base `np.ndarray` ([#889](https://github.com/rotnov/pycc/issues/889)).
   Registering the name is necessary, not sufficient — a reader must not
   infer any progress on this prerequisite from it. The operative
   consequence is unchanged: prerequisite 2 stays unmet, for both of the
   independent blockers the third correction states.

   **Sixth correction (2026-09-19, [#1143](https://github.com/rotnov/pycc/issues/1143)):
   the export rule no longer selects module-level functions only, so two of
   the third correction's clauses are superseded.** PR 1 of #1143 widens
   D-244 rule 1 to export a public `@staticmethod` and a public
   `@classmethod` of a public non-exception class, published as
   `mod.Class.method` on a `PyType_FromSpec` type object. (a) The clause "it
   is inadmissible purely because D-244 rule 1 exports module-level functions
   only" is superseded: the row's method kind was measured, numbers only per
   D-244 rule 6, and it is a **`@staticmethod`**, so the widened rule 1 now
   admits it into the export set. (b) The closing clause "the denominator
   claim above is unchanged — it is a statement about module-level functions,
   which is what the export rule selects today" is superseded in its stated
   *reason*, not in its effect: the export rule no longer selects module-level
   functions only, but the denominator is fixed by **pre-registration** rather
   than by tracking the export rule. `scripts/bench_hosted_ext_precommit.json`
   pins it at **133** with a digest over that exact set, the enumeration
   predicate in `scripts/enumerate_annotated_functions.py` is unchanged by
   this pull request, and re-deriving it on this host reproduces both the
   count and the digest. Changing it would itself be a pre-registration
   change, with the "chosen after a result is seen" hazard the protocol's
   preamble names. The denominator therefore stays **133**.

   **What this does not change: the sufficiency clause still holds, and
   prerequisite 2 stays unmet.** The third correction's own clause — "closing
   it is not sufficient on its own: the row spells its array parameter as a
   PEP 604 union, which [#747](https://github.com/rotnov/pycc/issues/747)
   tracks as entirely unimplemented" — is unaffected and was re-verified
   against the row's current signature: three of its four parameters name
   user-defined classes and the fourth is the PEP 604 union, so the row is in
   the export set and is a `C0003`, not a scorable compile. **This correction
   does not narrow [#1039](https://github.com/rotnov/pycc/issues/1039)'s
   prerequisite 2**, which stays unmet for exactly the reasons the third
   through fifth corrections state. A **seventh correction follows with PR 2
   of #1143**; no PR 2 figure appears here.

   **Seventh correction (2026-09-21, re-measured at `0638dc9c`): both named
   remedies landed and the census did not move by a single row.** #1143's PR 2
   became #1146 and merged at `b1de5499` (the `Ty::Instance` receiver carrier),
   and #1170 merged at `0638dc9c` (the `memoryview` return arm, Part 2b of
   #1142). The 2026-09-19 comment on #1039 named exactly those two as what
   stood between the array-taking, loop-bearing rows and a carryable signature,
   cheapest first, and singled out the return arm as sufficient on its own for
   four of the six. Re-running the census at `0638dc9c` with the same predicate
   refutes that: the denominator re-derives to `count=133` with a digest
   byte-identical to the committed `compile_unchanged_set_sha256`, and every
   row of the table is unchanged, including the module-level carryable count,
   which is 23 under both the old and the new rule. **Scorable subjects: 0.**
   The measurement, with each row's exact refusal reproduced empirically on a
   synthetic shape, is published on
   [#1039](https://github.com/rotnov/pycc/issues/1039#issuecomment-5767793679).
   Two conditions the earlier corrections never named are what the four
   buffer-in/buffer-out rows hit first, and neither is tracked by any issue
   those corrections cite: a **method** may not return a buffer (the admission
   is module-level-only), and a returned buffer must be **artifact-owned** (a
   parameter, a slice of one, and an intra-artifact call's result are each
   `C0001`). The measurement above stands as taken; the first of those two
   conditions no longer holds going forward, because
   [#1174](https://github.com/rotnov/pycc/issues/1174) widened the admission
   to a public method of a public class. The second is unchanged, and this
   count was never re-measured under the new rule, so the scorable-subject
   figure is the one the old rule produced. `Self` is also not a separate gap — it produces the same `C0003`
   named-type-return message as any other named type. Prerequisite 2 stays
   unmet, now for those two reasons rather than for the ones the third through
   sixth corrections state.

3. **The pre-registered machine pin no longer matched this host**, which
   would have refused a scored run on its own even with a subject in hand. The
   **Arms** bullet binds the run to the five fields
   `scripts/bench_hosted_ext_precommit.json` records, and the runner's
   `assert_hosts_the_arms` compares each as an exact string. Four still matched
   (`Mac15,9`, `Apple M3 Max`, 16 cores, 137438953472 bytes); the fifth did
   not — the record pinned `macOS 26.5.1 (build 25F80)` and this host composes
   `macOS 27.0 (build 26A428)`.

   **Resolved 2026-09-21 by re-pinning the record**, recorded in it as
   `machine_os_amendment`. Re-pinning is itself a pre-registration change and
   carries the "chosen after a result is seen" hazard the protocol's preamble
   names, so the decision was *when* rather than *whether*: it was made while
   the seventh correction above had just measured **zero** scorable subjects,
   so no arm had been built and no result of any kind existed that could have
   influenced it. Deferring it until a subject exists would have put the
   amendment on the wrong side of that hazard even if its content were
   identical. It is the same physical machine, updated in place: the other four
   fields are unchanged and still match what the host reports, so this
   re-pin does not relocate the run. The alternative — running on a host that
   still matches — was rejected because no such host exists here; the pinned
   OS was an in-place upgrade of the only machine available, not a second one.

One thing did hold and does not need re-verifying. The denominator was
re-derived at commit `8ad29658` with the same command and subtree the record
names, and returned `count=133` with a digest byte-identical to the committed
`compile_unchanged_set_sha256`. There is no drift in the "compiles unchanged"
denominator.

The operative blocker was therefore prerequisite 2, for both readings, joined
by prerequisite 3. **Annotated 2026-09-22 ([D-247](./decisions/D-247-pre-register-a-workload-admissibility-predicate-for-the-kill-criterion.md)):** neither is operative any longer, because this workload is refused and has no run to prepare. Neither has an issue tracking it, because neither
is straightforwardly a compiler gap: the **Subject** bullet requires a
byte-identical function that both compiles *and* consumes the committed
input, from a codebase that currently offers none, so closing it could
equally mean revising what the protocol takes as its subject — and a subject
revised now, with the 3.41x figure from #1114 already in hand, decides the
bet instead of measuring it. That was left open here on 2026-09-18 and is
settled as of 2026-09-22 by
[D-247](./decisions/D-247-pre-register-a-workload-admissibility-predicate-for-the-kill-criterion.md):
the protocol keeps the Subject bullet as written and gains a predicate on the
*workload* instead, under which this reference workload yields no subject and
is refused. #1039 therefore resumed on an admissible replacement workload, not
on landing #1027; that replacement's outcome is stated at the head of this
subsection. The compilation diagnostics are the actionable residue: they
are ordinary capability gaps, dominated by dependency-level import families and
tracked chiefly by [#882](https://github.com/rotnov/pycc/issues/882), sized
against a real codebase — but read them with prerequisite 2's cascade-suppression
caveat, which is why their count is not a remaining total.

The protocol's **Input** bullet still forbids choosing a different workload
*because* of these obstacles, and the **Workload admissibility** bullet does
not relax that: a replacement is admissible only through the predicate
[D-247](./decisions/D-247-pre-register-a-workload-admissibility-predicate-for-the-kill-criterion.md)
fixed in advance, never because an obstacle was met. What that bullet does
change is which obstacle is operative. This workload yields no subject at
all, so the committed generator, seed, `input_sha256` and denominator describe
a run that cannot be scored: a replacement workload replaces its input too, so
those fields — `generator_path`, `seed`, `input_sha256` and every
`compile_unchanged_*` field, the set digest included — are re-registered in
their own pre-registration commit when an admissible workload is adopted, ahead
of any run. `subject_sha256` is not part of that commit: it keeps the **Subject**
bullet's own rule, registered as the scoring run's first action in a stage
commit of its own. Until then nothing here is reshaped by an obstacle. The
workload #1207 adopted never reached that commit: its row (b) outcome was
recorded without a run, so these fields still describe the refused reference
workload. Since 2026-09-24 row (c) governs that workload instead
([D-252](./decisions/D-252-admit-annotation-only-additions-to-a-kill-criterion-subject.md)),
and its annotated subject does not compile yet, so the commit has still not
happened.

#### What the boundary costs, measured (not part of the protocol)

Everything in this subsection is an ad-hoc measurement taken **outside** the
protocol above, so that the reading is a number rather than an expectation.
It is not pre-registered, carries none of the protocol's disciplines — no
replicates, no median, no committed input, no correctness precondition — and
is not evidence for either `product-sprint-1` roadmap box or for D-244 rule
6's kill criterion. Only a run of the protocol itself can be those things.
The method, stated so the numbers can be read for what they are: two
`--release` `--ext` exports, timed against the same function written in
Python, on the same interpreter and machine, 1,000,000 calls each, wall time
divided by the call count.

| Subject | `ext` | CPython | Ratio |
| --- | --- | --- | --- |
| `noop(float) -> float` | 29.1 ns/call | 15.5 ns/call | 0.53x |
| `point_in_triangle(8 floats) -> bool` | 76.0 ns/call | 164.0 ns/call | 2.16x |

The first row isolates the boundary: an export that does nothing is **slower
than a Python-level call**, by about 29 ns of wrapper. The wrapper is already
`METH_FASTCALL`, so that cost is argument unboxing, result boxing, and the
runtime's per-call pending-exception check, not a calling convention that can
be swapped. That finding is filed against #1031.

The second row is the shape the per-record reading would use: eight `float`
arguments and a `bool` return, the same arity a triangle-plus-query-point
record has. At that arity the export is about **2.16x** faster per call than
the same function written in Python, boundary cost included. Both rows are
single observations of specific signatures, not a model: no asymptote or
body-only ratio is derived from them here, because subtracting the
one-argument row from the eight-argument row leaves unmatched
argument-binding and return-path costs on both sides of the quotient and the
resulting figure would isolate nothing. Neither row is evidence about D-244
rule 6's 5x bar in either direction — only a run of the protocol above can be
that.

#### What the buffer path costs, measured (not part of the protocol)

A second ad-hoc measurement, taken the same way and outside the protocol for
the same reason: #1027 closed the `memoryview` carrier (#1112, #1113, #1114),
and the question that follows is what a loop over a real third-party array
costs once the boundary is crossed only once. Unlike the per-call subsection
above, this one does carry replicates, a median, and a correctness
precondition — all three arms had to agree exactly before any ratio was
recorded — but it is still not pre-registered, its input is not committed, and
it is not evidence for either `product-sprint-1` roadmap box or for D-244 rule
6's kill criterion. Only a run of the protocol above can be those things.

The method: subject `dot9(b: memoryview, n: int) -> float`, summing
`b[i * 9 + 0] * b[i * 9 + 1]` over `n` rows. Input
`numpy.random.default_rng(20260917).random((200_000, 9))`, flattened
zero-copy with `.reshape(-1)` and handed over as a `memoryview` — 1-D,
C-contiguous, format `'d'`, 1,800,000 elements. `time.perf_counter_ns()`
around a single `dot9(v, n)` call, seven replicates, median reported with
min and max. Three whole process invocations of those seven replicates were
taken, and the table below is the third. The first is discarded as cold-start:
its pycc median was 5.74 ms (2.36x) against CPython and Cython medians within
3% of every later run, so the outlier is in the arm that had just been built
and first loaded. Invocations two and three agree to within 1% on every arm,
which is the whole basis for reporting a single run rather than pooling them.
Three arms on one machine and one interpreter: CPython running
the subject's own source through `exec`, pycc built with `--ext --release`,
and Cython 3.1.6 in pure-Python mode
(`cythonize("buf_oracle.py", compiler_directives={"annotation_typing": True,
"language_level": "3"})`, built with `CFLAGS="-O2"`). All three printed the
identical `repr` `49910.61395118853`, which is the precondition the ratios
below rest on. Machine: Apple M3 Max, macOS 27.0, arm64, CPython 3.13.9,
numpy 2.4.2. No measurement script is committed — the numbers are a reading
taken once, not a gate.

| Arm | Median | Min | Max | Versus CPython |
| --- | --- | --- | --- | --- |
| CPython 3.13.9 (`exec`'d source) | 13.17 ms | 12.46 ms | 13.45 ms | 1.00x |
| Cython 3.1.6 (pure-Python mode) | 14.74 ms | 13.67 ms | 15.09 ms | 0.89x |
| pycc `--ext --release` | 3.86 ms | 3.82 ms | 4.01 ms | 3.41x |

Two things are worth reading off this and nothing more. The compiled loop is
about **3.41x** faster than the same source under CPython, and the per-call
boundary cost the subsection above isolates is amortized here across the
400,000 element reads this loop actually performs — two per row over 200,000
rows, not one per element of the 1,800,000-element view — rather than paid
per read; the crossing happens once. And
Cython in pure-Python mode is **slower than CPython** on this shape, because
`b: memoryview` types the parameter as the Python `memoryview` object, so each
`b[i]` is still a Python-level subscript plus a boxed `float`; reaching
Cython's own fast path would mean annotating `double[:]`, which is a different
source file and therefore a different subject. Both readings are of this one
shape at this one size. A loop with a different element count, a different
number of reads per row, or a different amount of arithmetic between reads
will give a different number, so no unlabelled "speedup" is derivable from
this table. And neither reading is evidence about either
`product-sprint-1` Accept box or D-244 rule 6's 5x bar in either direction —
only a run of the protocol above can be that.


## CPython interop matrix (v0.7)

D-128's transparent interop contract is partly implemented: the embedded
executable for standard-library roots (#1223, D-248) and the policy surface
(#1224), `pycc lock` (#1241, D-249), bundling the locked closure (#1242),
its native libraries outside the interpreter (#1243), and macOS relative
references outside a closure payload (#1259) exist, and so does Windows
embedding of standard-library roots (#1286, D-253) and of a pure-Python
locked closure (#1296, `tests/issue_1296_windows_locked_closure.rs`), and a
Windows closure holding a native image is covered by #1306's clang-built
extension with a native (that file's tests (d), (e) and (f) on the Windows
leg; `src/embed/native_windows_closure_tests.rs` and
`src/embed/windows_lock_tests.rs` on every host). The v0.7 implementation cannot mark its roadmap acceptance complete
until all of the following run on every Tier-1 target. Each bullet names the
tests that cover it now, or the owner of what is still missing.

- unchanged source fixtures containing both `import numpy as np` and
  `from numpy import array` build and run under the default `auto` policy
  without a separately installed Python. *Pending:* `from numpy import
  array` binds the CPython object `numpy.array` since #1278 and
  `import numpy as np` binds a foreign module since #1291, and a plain
  `import numpy` embeds from `pycc.lock` (#1242), but calling `array(...)`
  directly is still `I0404` and no test yet runs the combined fixture from a
  lock;
- the produced `pycc.lock` and deployment bundle select the exact intended
  CPython, package, and native-library artifacts and never consult ambient
  `site-packages` at runtime. *Partly covered:* the lock's closure, integrity
  and byte stability (`tests/issue_1241_pycc_lock.rs`, the `src/lock/` unit
  tests); the build consuming it, every lock and interpreter refusal, the
  copy's digest, case-collision and permission checks, and the macOS
  closure-image relocation arms (`tests/issue_1242_locked_closure.rs`, whose
  `#[ignore]`d oracle runs a `tinypkg`/`tinydep` closure from the sidecar
  after its venv is moved away; `src/lock/build_tests.rs`,
  `src/embed/closure_tests.rs`, `src/embed/lock_tests.rs`,
  `src/embed/macos_closure_tests.rs`, `src/embed/macho_tests.rs`); the
  native libraries a closure needs, locked, compared and copied
  (`tests/issue_1243_native_libraries.rs`, whose `#[ignore]`d oracle runs a
  `cc`-built extension linking two outside libraries after both are moved
  away; the Linux ELF walk over synthetic images in
  `src/embed/native_linux_tests.rs`, `src/embed/elf_tests.rs` and
  `src/embed/lock_tests.rs`); macOS `@rpath`/`@loader_path` references
  outside a closure payload, kept, rebound, vendored or refused
  (`tests/issue_1259_relative_natives.rs`, whose `#[ignore]`d oracle runs
  one extension per bundling case after the venv and every outside
  directory are moved away; `src/embed/macho_host_tests.rs` over a fake
  host probe, `src/embed/macos_relative_tests.rs` and
  `src/embed/macos_closure_tests.rs` over `cc`-built images). The bundle
  carries no `site-packages` (D-248);
- `allowlist` accepts an allowed direct import root, covers its submodules and
  pinned transitive closure, and emits `I0402` for an otherwise-resolvable
  unlisted direct root. *Covered:* acceptance and the unlisted root
  (`tests/issue_1224_interop_policy.rs`, the `interop_allowlist/` snapshots
  in `tests/diagnostics/`); the closure is bundled from `pycc.lock`
  (`tests/issue_1242_locked_closure.rs`). *Pending:* submodules (a dotted
  CPython-backed import is `C0001` today);
- CLI policy precedence covers every usable branch: explicit `auto` and
  `deny` each override the other and a configured `allowlist`; explicit
  `allowlist` with its configured roots accepts an allowed root and emits
  `I0402` for an unlisted root. A CLI switch *to* `allowlist` from configured
  `auto` or `deny` has no permitted stored roots because non-empty `allow` is
  invalid under those policies, so it deterministically rejects every
  CPython-backed direct root with `I0402` rather than borrowing a stale list.
  *Covered:* `tests/issue_1224_interop_policy.rs` and
  `src/interop_policy/tests.rs`;
- `check`, `build`, `run`, and the eventual `test` compilation path apply the
  same effective policy and select the same success or policy diagnostic for
  an equivalent import graph. *Covered* for `check`, `build` and `run`
  (`tests/issue_1224_interop_policy.rs`). *Pending:* `pycc test`, which is
  not implemented;
- invalid policy enum values, a non-empty `[interop].allow` outside
  `allowlist`, and every `--pure` plus explicit `--interop-policy`
  combination fail as bad invocations (exit 2) instead of depending on
  argument order or silently ignoring stale configuration. *Covered:*
  `tests/issue_1224_interop_policy.rs` and `src/cli.rs`'s parser tests;
- `deny` and its `--pure` shorthand both reject the same CPython-backed fixture,
  while a native pycc import remains accepted and a successful pure artifact
  has no CPython/libpython dependency. *Covered:* the `i0402_*` snapshots in
  `tests/diagnostics/` and `tests/issue_1224_interop_policy.rs`; the
  no-`Py*`-symbol check runs on the non-Windows legs, while the Windows leg
  proves only that the build succeeds with no `OUT.pycc/` sidecar; and
- the boundary benchmark publishes copied scalar/container marshalling and
  supported zero-copy buffer transfers separately, so compatibility does not
  hide the cost model.

Each negative case requires both human and versioned JSON diagnostic snapshots.
The automatic and allowlist cases must also exercise target-specific native
package artifacts rather than passing only with a pure-Python stand-in
(*pending:* #1225). Admission on a Windows host is delivered for
standard-library roots by #1286 (`tests/issue_1224_interop_policy.rs`'s
admission tests run on every host) and for other roots with a pure-Python
locked closure by #1296 (`tests/issue_1296_windows_locked_closure.rs`); a
closure holding a native image is delivered by #1306 (that file's tests (d),
(e) and (f)).

## The bot (planned)

A planned GitHub Action (`corpus-bot`) would work as follows. No `corpus-bot` workflow exists on current `main`.

1. Nightly: run corpus + a rotating slice of top-PyPI packages (by download count) in compile-only mode.
2. New failure → fingerprint (diagnostic code + normalized span + package) → dedupe → auto-file issue **in the pycc repo**: minimized repro, diagnostic output, PEP link, dashboard delta. Labels: `corpus`, `regression`/`gap`.
3. Fix confirmed → bot closes the issue with the passing run linked.
4. Upstream bugs pycc finds (genuine type errors in the project): bot drafts the report, human reviews and files — never automated spam.

## Benchmarks

- Compiler: `pycc check` LOC/s, cold + incremental build times; tracked per-commit (criterion + CI history), >2% regression fails PR.
- Generated code: pyperformance subset + fib/nbody/spectral-norm vs CPython 3.14, Nuitka, Codon, mypyc; published table per release. Honesty rule: publish losses too.

D-129 completes D-126's evidence phase with five observations from every Tier-1
leg. Each of the five `--release` pycc launches and five pinned-CPython launches
still records both elapsed wall-clock time and CPU time consumed by that exact
child process. Unix obtains per-child usage from `wait4`; Windows reads the
waited process handle through `GetProcessTimes`. CPU-time variance was not
consistently better: it was effectively identical on the motivating Ubuntu
x86_64 leg and 44.2% worse on Windows. The median wall-clock ratio therefore
continues to enforce the existing 20x/12x/15x/18x target-specific floors, while
the CPU-time ratio and all four medians remain non-gating telemetry written on
both passes and failures.

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
`scripts/test_check_roadmap_evidence.rb`; the checker implementation and
documented claim land together in that same pull request, but the inline
marker on an actually-checked roadmap item lands separately, in a later pull
request, because registering a new identifier is itself staged the same way
a digest change is (see `ci-tier1-cross-compile` below):
`.github/workflows/workflow-policy.yml`'s `audit` job checks out
`scripts/check_roadmap_evidence.rb` from the base branch under
`pull_request_target`, so a single pull request that both registers a new
identifier and checks a roadmap item citing it can never pass its own
audit — the checker that runs is always the base branch's prior version,
which does not yet know the new identifier. Register first, with every
roadmap checkbox that will cite it left unchecked; merge; only then open a
second pull request that checks the box.

The coverage evidence — `ci-diff-coverage-100` ("Every compiler-relevant
pull request keeps 100% line coverage of its added and modified Rust lines,
and total line and region coverage is reported by CI."), which under
[D-242](decisions/D-242-product-mode-the-delivery-process-informs-rather-than-blocks.md)
replaces the retired whole-workspace identifier `ci-build-test-coverage-100`
(still registered; no roadmap item cites it), together with
`readme-diff-coverage-badge-bound` ("The README coverage badge percentage is
bound to ci.yml's enforced --require-changed-lines threshold.") — requires
all of the following named properties of
`.github/workflows/ci.yml`, checked by `coverage_gate_present?`:

- the exact claim text in the v0.1 checklist;
- an unfiltered `pull_request` trigger;
- the unconditional, dependency-free, failure-propagating
  `build-test-coverage` job on the trusted runner, with the exact trusted
  workflow environment and no inherited run defaults;
- pre-gate steps drawn only from the pinned checkout with
  `persist-credentials: false` and the enumerated trusted setup commands
  (`TRUSTED_COVERAGE_SETUP_COMMANDS`), in any order, none repeated, none
  carrying `shell`, `if`, `env`, or `with`, so no earlier head-controlled
  script can shadow the coverage executable;
- the gate step, named exactly
  `Coverage gate — 100% of changed lines, totals reported (D-242)`, using the
  default shell, unconditional, with exactly the two-key `env` that supplies
  the diff base (`PR_BASE_SHA`, `PUSH_BASE_SHA`);
- a gate script that starts with `set -euo pipefail`, contains each of
  `REQUIRED_COVERAGE_GATE_LINES` exactly once and in order — the changed-line
  diff `git diff -U0 --no-color --no-renames "$COVERAGE_BASE_SHA" HEAD >
  "$RUNNER_TEMP/coverage-changed.diff"` that writes the gate's denominator
  (no pathspec, no rename detection), the trusted
  `cargo-llvm-cov` path and its pinned `--locked --version` install, `nobody`
  ownership of the isolated root, the sanitized `PATH`, the three-line
  `run_isolated` definition (`sudo -u nobody env -i "${ISOLATED_ENV[@]}" "$@"`),
  the `llvm-cov --workspace --lcov` export, the `llvm-cov report
  "${WORKSPACE_PACKAGE_FLAGS[@]}"` table, and the
  `python3 -B scripts/check_diff_coverage.py … --require-changed-lines 100`
  gate — assigns `TRUSTED_COV=` and defines `run_isolated()` exactly once,
  and contains none of the forbidden fragments (`exit 0`, `|| true`, `|| :`,
  `set +e`, `trap `).

The legacy D91 byte-exact shape (the `Hard coverage gate — 100% lines +
regions (D-014)` step with its reviewed script and pinned setup-step prefix)
stays accepted permanently as a strictly stricter historical form, so
`ci.yml` satisfies the checker in either shape. **Threat model:** the audit
proves *shape* — unprivileged sandbox, trusted binary, workspace denominator,
pinned changed-line denominator (the diff command itself, so no pathspec or
rename detection can shrink the measured line set), gate command line,
threshold — not byte identity; a hostile edit placed
between the required lines is caught by the D-068 full-diff review that
precedes every merge, which is the compensating control. Privileged jobs keep
every `scripts/check_ci_permissions.rb` check unchanged. Adding a required
line, a trusted setup command, or an evidence identifier is the one case that
still needs the staged registration above, because the base-owned checker
must know it before a head workflow uses it.

That workflow proof is also an unconditional repository invariant. The trusted
checker validates it even while the roadmap claim is unchecked or absent, so a
pull request cannot remove every evidence marker and replace the required
coverage job with a successful no-op. The marker controls whether the delivery
claim may be shown as complete; it never controls whether hard coverage is
enforced.

## README badge-to-gate contract (issue #211)

The README displays two badges at the top of the file:

1. **CI badge** — a GitHub Actions workflow badge
   (`[![CI](https://github.com/rotnov/pycc/actions/workflows/ci.yml/badge.svg?branch=main)]`).
   It is bound to the `ci.yml` workflow and the `main` branch.
2. **Coverage badge** — a Shields static badge
   (`[![diff coverage: 100%](https://img.shields.io/badge/diff%20coverage-100%25-brightgreen)]`).
   Its percentage is bound to `ci.yml`'s `--require-changed-lines`
   threshold (D-242).

`scripts/check_readme_coverage_badge.rb` validates both badges locally
without network access. It verifies:

- the coverage badge's alt-text percentage and URL percentage agree with each
  other and with ci.yml's `--require-changed-lines` threshold;
- the coverage badge links to `./docs/TESTING.md`;
- the CI badge references `actions/workflows/ci.yml` (not another workflow),
  uses `branch=main` (not another branch), and links to `rotnov/pycc`;
- the CI badge's clickable link target matches its badge image source (same
  repo and workflow);
- the README contains exactly one coverage badge and exactly one CI badge
  (no duplicates);
- the coverage step in `build-test-coverage` has no `if:` condition or
  `continue-on-error` that could skip it or suppress its failures;
- `ci-gate`'s `needs` list includes `build-test-coverage`.

The validator runs in both `ci.yml` (inside the `build-test-coverage` job, which
contributes to `ci-gate`) and `pages.yml`, so deletion from either workflow is
detected. Its test suite (`scripts/test_check_readme_coverage_badge.rb`)
includes negative tests for every mutation: wrong percentage, missing badge,
alt/URL disagreement, CI threshold mismatch, missing CI threshold, lines/regions
disagreement, badge not linking to TESTING.md, missing CI badge, CI badge with
wrong branch, CI badge with wrong workflow, CI badge with wrong repository, CI
badge link target pointing to wrong repo or workflow, duplicate coverage badge,
duplicate CI badge, skippable coverage step (both `if:` condition and
`continue-on-error`), and ci-gate without build-test-coverage dependency.

## README testing-claims validator (issue #214)

`scripts/check_readme_testing_claims.rb` parses the README's "Testing
strategy" section and verifies that every claimed testing mechanism is
consistent with actual repository evidence. It complements
`scripts/check_readme_claims.rb` (which checks text patterns) by
cross-referencing claims against real workflow files and test directories.

The validator checks four invariants:

1. **Current mechanisms have evidence.** A mechanism presented as current
   (e.g. the conformance suite) must have backing evidence — either a test
   file (`tests/conformance.rs`) or a workflow that references it. A
   current claim without evidence is a false live claim.
2. **Planned mechanisms are explicitly marked.** A planned mechanism
   (corpus, ecosystem bot, differential fuzzing) must carry an explicit
   `(planned)` marker or "not yet implemented" language in the same
   paragraph as any mention.
3. **No contradictory present-tense claims.** A present-tense deployment
   verb ("CI compiles", "a scheduled job picks", "runs continuously") for
   a planned mechanism without a planned marker in the same paragraph is
   rejected — it reads as live but no backing evidence exists.
4. **Roadmap checklist consistency.** The roadmap's ecosystem-bot
   checklist item must be unchecked `[ ]` while the testing section marks
   the bot as planned; a checked roadmap item alongside a planned testing
   claim is an inconsistency.

The validator runs in `pages.yml` alongside
`scripts/check_readme_claims.rb`. Its test suite
(`scripts/test_check_readme_testing_claims.rb`) includes mutation tests
for every invariant: current mechanism without evidence, present-tense
corpus/bot/fuzzing claims without planned markers, mechanism mentioned
without any planned marker, roadmap bot checked while testing says
planned, and positive controls for planned markers that legitimize
mentions.

The `ci-tier1-cross-compile` evidence is enforced by D-172's base-owned
property audit. It requires the exact five-target Tier-1 matrix, the macOS
arm64-to-Intel cross-host build and native execution proof, immutable Actions,
read-only checkout credentials, event-default candidate checkouts for required
workspace jobs, fail-closed D-171 routing, and the aggregate
`ci-gate` truth table. Required jobs cannot inherit an alternate job-level
`env`, `defaults`, or `container`; the native matrix and Pages jobs retain
their reviewed runner bindings; and the Intel and Pages proof commands cannot
be made conditional. The audit binds the commands and dependencies that
produce those properties instead of authorizing a complete `ci.yml` byte
digest. Historical workflow digests remain immutable regression fixtures only;
they cannot force a later pull request to activate their bytes.

D-100 composes D-091
(release-mode `pycc_rt` build step, relaxed `frontend-perf-measure` manifest
classification) with D-099 (Windows vcpkg binary cache) into one reviewed
digest; the live workflow matched this fixture until D-112 activated (see
below), and D-100's own digest remains reviewed audit evidence. D-099 activated on `main` independently of PR-8's own work, briefly
retiring D-091's own digest before D-100 composed the two -- both D-091's and
D-099's pre-composition digests remain reviewed pre-D-100 audit fixtures, no
longer publicly authorized on their own. D-099 is the retired D-084 workflow
plus only a Windows vcpkg binary-cache boundary for D-027's libxml2 build,
which D-100 carries forward unchanged. It resolves the hosted image's
exact vcpkg commit before restoring, then
uses an exact key containing that commit, the hosted `ImageVersion`,
`LLVM_VERSION`, runner OS and architecture, and `x64-windows-static-md`; it
deliberately provides no prefix `restore-keys`. The image version rotates the
immutable outer key if MSVC or another vcpkg ABI input changes without a vcpkg
commit change. Pull requests may restore the default branch's cache but the
separate save action is guarded to exact `push` plus `refs/heads/main` and an
exact-key miss. Both action entrypoints use the immutable reviewed
`actions/cache` v6.1.0 commit. The fixture tests prove that removing these three
cache steps produces the retired D-084 workflow semantics exactly, that the
restore/save keys and paths are paired, and that removing either the hosted
image or LLVM component of the key is rejected by the public checker.

The `conformance-fib-mandelbrot-tier1` and `check-throughput-1k-loc-75ms`
evidence is checked as part of the same active D-171 structure. The
base-owned validator requires the command that runs the fib/mandelbrot-ascii
byte-for-byte CPython differential (`tests/conformance.rs`, D-085/D-080) via
`cargo test -- --include-ignored`
in both `build-test-coverage` and every `native-build-test` matrix leg, i.e.
on all five Tier-1 targets, while the `pycc check` <75ms/1000 LOC
absolute-throughput-floor step (`scripts/check_frontend_throughput.rb`,
D-079/D-084) runs only inside `build-test-coverage` (one target) -- unlike
the tier1 claim above, this roadmap item's own wording does not assert the
floor holds on every target, only that it holds. The
`cli-spec-diagnostic-match` evidence instead binds
the `cli_spec_example` diagnostic-snapshot test (`tests/diagnostics_test.rs`,
D-083), which runs inside the same structurally required, 100%-coverage-gated
`cargo test`/`cargo llvm-cov` step `ci-diff-coverage-100`'s evidence
already requires -- none of these three add a second, evidence-ID-specific
repository check beyond the roadmap claim/section binding every evidence ID
gets, since the underlying capability proof is already exhaustively covered
by the two structural checks the other evidence IDs already established.

The historical D-048 workflow established the split trust boundary:
`frontend-perf-measure` executed pull-request benchmark code and uploaded only
Criterion estimates as untrusted data, while `frontend-perf-gate` executed the
hash-verified main-owned comparator against an exact successful-main artifact.
D-051/D-053 introduced the paired-runner version of that isolation while
retiring the cross-run artifact dependency, the D-048 digest, and its fixture.
D-056 retained the same boundary, and D-062 keeps it while changing only the
fixed sample plan and comparator. Artifact and checkout actions remain immutable
reviewed pins.

The active `.github/workflows/ci.yml` uses D-171 change-aware routing while its
performance jobs retain D-112's Ubuntu runner, D-062's five-replicate
comparator, and D-114's current `>7%` threshold. D-172 validates their exact
predecessor/candidate bindings, source-aware comparator, artifact identities,
routing dependency, and aggregate result branches as named properties. The
retired D-051, D-056, D-062, D-080, D-084, pre-D-100 D-091, pre-D-100 D-099,
D-100, D-112, and D-114 whole-workflow digests remain historical mutation-test
evidence; none authorizes the live workflow.
The D-048 steady-state, pre-split, and activation fixtures, their digests, and
their bootstrap tests are absent.
The retired D-048 mean comparator and its standalone test are absent too;
references to those paths in the historical D-042/D-044 decisions describe
the repository state when those decisions were accepted, not active tooling.

The paired lifecycle is fail-closed and predecessor-owned without an external
baseline. Both performance jobs remain exact literal-success dependencies of
`ci-gate`; the trusted checker validates their complete shapes and the aggregate
fan-in. Each run derives the predecessor exclusively from
`pull_request.base.sha` or `push.before`, rejects missing, zero, or unsupported
event inputs, and measures both revisions inside that same run. There is no
missing-evidence exception, reusable bootstrap, repository variable, cache
fallback, older convenient SHA, or failed-run artifact path.

D-051/D-053 removed between-runner timing; D-056 added trusted executable-input
identity; D-062 retains those provenance controls and keeps the 2% threshold for
changed executable inputs. The active measurement job
resolves `pull_request.base.sha` or `push.before`, checks out that exact
predecessor and `github.sha` into separate directories, verifies both
revisions, and rejects drift in the bound benchmark-definition and
build-configuration contract: `benches/`, the root `Cargo.toml` and
`Cargo.lock`, both root Rust toolchain filenames, root `.cargo/`, every
workspace-member `Cargo.toml`, and every tracked local `build.rs`. It
benchmarks both revisions on one hosted runner using separate Cargo target
directories. It performs exactly five complete predecessor runs and uploads
the fixed `round-1.json` through `round-5.json` set through the pinned v4
artifact action before candidate code executes, closing the same-user
background-process race that a local hash-then-copy sequence would leave open.
It then performs and uploads the same fixed five-run set for the candidate. The
active gate checks out and hash-verifies the dedicated fixed-replicate comparator
and its tests from the exact predecessor, validates
the distinct numeric artifact identities returned by the trusted upload steps,
downloads both same-run inputs by those exact IDs rather than replaceable
names, flattens each single-ID download into its own exact destination,
requires exactly ten regular files under the two exact revision directories
with no symlinks, extra files, or extra directories, and
remains an exact `ci-gate` dependency. Missing or zero predecessor SHAs,
unsupported events, a mutable action, revision mismatch, removal of any bound
contract path or local-manifest/build-script binding, shared target state,
candidate execution before the sealed predecessor upload, a broad artifact
upload, a missing, repeated, or non-numeric artifact identity, a name-based
download, a non-flat artifact download, a changed fixed sample count, any
missing round, an extra file or directory, a symlink, a skippable comparison,
or a mixed old/new job pair fails closed in focused tests.

Median point estimates are deliberate rather than a threshold relaxation. A
local paired validation with identical Rust and benchmark code produced a
`-2.94%` mean difference after the predecessor sample accumulated 15 severe
high outliers, while the medians differed by `-0.56%`. The merge threshold
remains greater than 2%, and the comparator remains isolated and digest-bound.

D-056 introduced the source-aware rule after the earlier paired gate still
produced a `+3.14%` false failure for identical executable inputs in main run
[30198852753](https://github.com/rotnov/pycc/actions/runs/30198852753), followed
by a `+0.86%` pass for the same unchanged-input class in run
[30199477003](https://github.com/rotnov/pycc/actions/runs/30199477003). The
reviewed [`d56-source-aware-ci.yml`](../tests/fixtures/d56-source-aware-ci.yml)
kept both measurements and every D-051 provenance control. Before candidate
code runs, it classifies the complete `src/` and `crates/` trees as identical
or changed; the existing contract independently binds every benchmark,
manifest, lockfile, toolchain, Cargo configuration, and local build script.
The D-056 rule treats a timing delta as non-blocking environment
telemetry only for the exact `true` identity, while any changed executable
input keeps the same greater-than-2% failure. Boolean validation, complete-path
classification, step ordering, output propagation, comparator binding, and
the unchanged failure path have focused positive and negative tests.

This identity rule remained current through D-100 (its performance-job
content stayed byte-identical to the reviewed D-062 fixture, while the
whole-file digest changed for later conformance, throughput-floor, and
vcpkg-cache steps), and D-112 inherits the same classifier logic unchanged --
only the runner and LLVM install step differ (see above). D-051, D-056,
D-062, D-080, D-084, pre-D-100 D-091, and pre-D-100 D-099 are retained as
audit fixtures and have public-CLI rejection tests; active D-112 has positive
and mutation tests, and its live bytes must remain exact, while D-100 has
moved into the same retained-fixture category pending its own retirement
round.

D-062 addresses the residual single-observation defect tracked in #109 without
changing D-056's identity rule or threshold. PR run `30200982922` and immediate
post-merge main run `30201385971` measured the same changed-source pair at
`+0.10%` and `+3.66%` respectively, even though every provenance and artifact
check succeeded. D-056 correctly leaves such a pair in the blocking `false`
path, so D-062's contract -- carried unchanged from D-100 into the now-active
D-112 workflow -- fixes that path's sample plan before execution: five full
Criterion runs for the exact predecessor, immutable upload of all five JSON
files, then five full candidate runs. Exact `true` remains non-blocking
telemetry. The gate requires the exact
`round-1.json` through `round-5.json` set in both artifact directories, rejects
symlinks and extras, extracts every per-run median, and applies the unchanged
greater-than-2% rule to the median of each five-value set. No retry count or
sample can depend on an observed result. Predecessor-first sealing remains
mandatory; alternating execution was rejected because candidate code could
leave a process that influences a later supposedly trusted predecessor sample.
The fixed-replicate comparator independently rejects missing directories or
rounds, extra samples, symlinks, malformed/root-shape/median-shape JSON,
non-numeric, non-positive, or non-finite estimates, and invalid thresholds.
A synthetic isolated extreme outlier passes only when the other four samples
keep the aggregate within 2%; three regressed samples make the median fail. An
exact `true` passes even for an extreme delta. An identical-tree local 5+5 run
retained all samples and measured aggregate medians `7068.84 ns -> 7054.06 ns`
(`-0.21%`). Byte-exact activation proves the reviewed jobs execute, and repeated
changed-source PR/main runs from merged PRs [#51](https://github.com/rotnov/pycc/pull/51)
and [#132](https://github.com/rotnov/pycc/pull/132) later validated the
blocking aggregate without result selection, closing #109 (2026-07-26): a
changed-input `>2%` failure is a real, validated gate result, not
presumptively known-noise.

**Update (2026-08-03, D-114):** the `>2%` threshold described throughout
this section's own history is no longer the live value — `frontend-perf-gate`
now requires `>7.0%` before failing, raised via a corrected six-round D-103
propose/activate sequence to accommodate v0.2 PR-10's real, one-time
`Ty`-migration cost (D-109), not runner noise. Every historical `2%`
reference above still accurately describes what that specific decision
changed at the time; only the currently-active threshold has moved.
The active D-171 workflow preserves D-114's threshold and D-062's measurement
properties without being authorized by `REVIEWED_PERF_CI_WORKFLOW_SHA256S`.
Issue #296 tracks
lowering the threshold back toward 2.0% once this one-time cost is
absorbed into every future baseline.

The historical byte-exact activation retired the D-048 workflow digest and
fixture. No administrative bootstrap is required because each compiler-selected
run of the active D-171 workflow uses D-062's embedded contract to
measure both sides of its own comparison. D-054's one-shot
staging recovery is historical audit
evidence only; normal `audit` plus `ci-gate` protection was restored before this
activation branch was created and is not encoded in repository configuration.
A pull request that changes a bound manifest, local build script, lockfile,
toolchain, Cargo configuration, or benchmark source must preserve the
source-aware benchmark properties and pass the base-owned audit in that same
pull request; the gate intentionally does not guess whether such a change
affects only product code or also the measurement harness.

Regular CI runs the self-tests and repository checker in the always-run
`governance` job for fast feedback, independently of whether compiler-heavy
coverage is selected. The authority is the required read-only `Workflow
policy` job: it checks out the base revision, downloads the head revision's
workflows and `docs/ROADMAP.md` as non-executable data, then runs the base
revision's roadmap tests and checker against those inputs. A pull request that
replaces its own checker therefore cannot replace the implementation that
authorizes its checked roadmap markers.

### Agent hook lifecycle

The required macOS Python discovery run includes
`scripts/test_manage_ievo_hooks.py`. A Windows-only Rust integration harness runs
that lifecycle suite plus `scripts/test_validate_agent_policies.py` inside the
required native Windows matrix, so native reparse-point, advisory-lock, and DOS 8.3
branches execute in CI without otherwise modifying D-100's byte-pinned
workflow. Its isolated
synthetic repository covers the
complete D-077/D-081 lifecycle: shared Claude entries plus pre-existing local state are
localized without duplicates; unrelated settings and hooks survive; both Claude and
Codex hook scripts execute successfully with a no-op payload; upstream tracked-shim
ignore exceptions are removed; repeated localize and disable operations are stable;
and disable removes every exact iEvo entry before its generated targets while
preserving the tracked shared-intent flag. The full lifecycle also verifies that the
tracked view returns to clean after upstream-style enable mutations are normalized.
It preserves unrelated empty hook groups/events and makes refreshed shared metadata
win over stale duplicate local metadata. Separate negative cases prove a missing
target, incomplete or conflicting corrections-only intent, malformed local JSON, an
unsupported reference to a managed target (including lexical, case, quoted-fragment,
line-continuation, POSIX within-component backslash/expansion, and Windows-shell
expansion aliases), an
applicable wildcard (including POSIX bracket-class)/brace/extglob alias, a PowerShell
backtick/constant-expression or cmd caret alias, an LF/CRLF Windows continuation or
multiline substitution alias, a Windows DOS 8.3-shaped path component, an unignored/force-tracked local
configuration, a directory-shaped script target, a force-tracked vendor descendant,
or a vendor traversal failure fails before mutation or target deletion.
Additional race/error cases cover an inaccessible vendor root, an active advisory
lock, harmless recovery from an orphaned lock file, absolute/relative linked-worktree
gitdir lock resolution and malformed metadata, symlink/junction gitdir components and
lock entries, the root-local non-git fallback, a configuration edit observed
before its replacement, a generated script changed between snapshots, a vendor entry
inserted between initial validation and removal, an ancestor relocated and replaced
with a symlink after snapshots, and successful deepest-first removal of a nested vendor
tree without touching an unrelated sibling. The Windows-only junction regression
proves that reparse redirection blocks both smoke execution and disable deletion
before either can touch the external tree; the native 8.3 short-path regression
covers lexical aliasing. Platform-neutral
mount simulations prove that neither a
mounted configuration ancestor nor a mounted generated-hook ancestor can redirect
writes or deletion outside the worktree. The raw `--root` CLI argument itself is
covered separately from every path *underneath* an already-accepted root
(D-113, the #169 follow-up that supersedes D-081's original leaf-only fix):
a symlinked leaf and a symlink anywhere in `--root`'s
ancestor chain are each rejected through the real CLI before `main()` ever resolves
the argument, with a Windows junction sibling for both the leaf and the ancestor
case; a mounted `--root` leaf is proven directly against the new
ancestor-walk function in-process, since a real mount point cannot be created
portably in a test. A direct, non-CLI call to
`disable()` with a symlinked root argument proves `ensure_root_is_a_real_directory`'s
own, narrower contract for library callers independently of the CLI-boundary check.
Matching duplicate values for all intent
fields are accepted by localize, check, disable, and the policy parser, while missing
or conflicting values fail closed before every lifecycle mutation.
`scripts/validate_agent_policies.py` additionally requires both
`.claude/settings.local.json` and `.codex/hooks.json` to remain ignored in the real
tracked checkout.

## CI privilege policy

Every GitHub Actions workflow declares an explicit workflow-level permission
baseline. The baseline may contain only read or `none` scopes, or
`permissions: {}`; a job that needs an elevated scope must opt in at job level
and satisfy the trust-boundary rules in `AGENTS.md`.

Regular CI runs both commands in the always-run, read-only `governance` job:

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
reviewed prospective workflow as an inert fixture and add its exact digest
while the old anchor remains, then replace the active anchor byte-for-byte from
that fixture in a later pull request and retire the superseded digest in that
activation change.

The active search-ledger audit is pinned by the reviewed fixture
`tests/fixtures/workflow-policy-search-ledger.yml` with SHA-256
`f8d60936438c48362d0a5dc11ee709c9dd5354c3f697038bc36b620c266f0688`.
It keeps the existing read-only `pull_request_target` boundary, additionally
downloads the head search ledger, query registry, checkpoint file, and roadmap
as non-executable data. Before materialization it requires every workflow and
required evidence path returned by the Git tree API to be an exact
`100644 blob`; symlinks, executable files, trees, and submodules cannot reuse
approved bytes while breaking the next trusted checkout. The steady-state
successor also rejects every root or nested `.gitattributes` entry on every
pull request before checkout rules can rewrite a byte-identical workflow.
It reads the trusted base's
`tests/fixtures/policy-successor-manifest.json` as a bounded inventory of
historical policy inputs and downloads candidate workflows and protected inputs
as non-executable `100644` data. D-172 supersedes D-103's forced exact-byte
two-merge transition for general CI and checker changes: the base-owned checker
validates named permissions, Action pins, checkout credentials, trusted-event
guards, D-171 routing, Tier-1 coverage, and aggregate-gate properties in one
pull request. The candidate manifest cannot authorize its own bytes, and no
manifest entry forces a later activation. The separately pinned
`workflow-policy.yml` trust anchor retains its own staged update boundary. The
audit then runs the base-owned
`check_search_visibility_audit.py` against the checked-out base ledger. The
audit rejects a rewritten history prefix, invalid checkpoints, mutable surface
or activation contracts, incorrect rank deltas, and replay metadata whose
types, ranges, result-count relationships, or corpus digest are invalid. Its
trusted clock also rejects future-dated evidence, and each timestamp/query
pair must identify exactly one observation. Existing base measurements are an
immutable registry prefix, and history rows are accepted only after the exact
Markdown table header and delimiter. That header must be the first section
content or follow a CommonMark blank-line block boundary containing only ASCII
spaces or tabs; explanatory prose, Unicode whitespace, and non-CommonMark line
separators cannot merge with the header into a GFM paragraph while the raw rows
remain auditable. A table-like line without the canonical
unindented leading pipe fails instead of becoming visible, unaudited evidence.
Every accepted history line also has exactly one leading and one trailing
boundary pipe, so repeated pipes cannot add an empty rendered column while the
parser silently strips it. Every delimiter cell has the GFM minimum of three
hyphens, preserving the table rendering that gives the evidence its column
meaning. `Current interpretation` begins with an exact, machine-checked table
projected from the latest accepted row for every GitHub query. A valid append,
replay record, and checkpoint cannot merge while that public latest-state view
still reports the previous snapshot.
GitHub query text rejects every qualifier except an `in:description` metadata
diagnostic or a single `topic:` diagnostic. Syntax-sensitive identities
normalize ASCII case and repeated whitespace before classifying Boolean
operators and preserving punctuation, phrases, qualifiers, exclusions,
Boolean syntax, and term order as distinct;
active product queries cannot split one acquisition intent through spelling
variants. Raw GitHub queries also reject HTML tag/comment forms that the
fail-closed ledger parser cannot later project. A closed intent/KPI matrix
keeps product, category-version, and task-output queries in acquisition;
brand, metadata, topic, and competitive queries in diagnostics; and authorship
in excluded evidence. Unknown or crossed pairings fail before they can alter
the KPI denominator. A complete REST response must
contain exactly `min(api_total, 50)` rows; a shorter result list cannot support
an organic `>50` claim, and `incomplete_results=true` cannot produce any rank
row. Both measurements and checkpoints preserve their trusted base list as an
immutable prefix before new entries are accepted. The mutation suite treats
only rows already present in the trusted base prefix as legacy; every append
requires replay metadata regardless of its claimed timestamp. Once prose or a
blank line ends the history table, a later pipe row cannot resume it. Section
lookup normalizes CommonMark ATX whitespace and closing hashes so a visually
equivalent duplicate history heading cannot hide a second table. A later
top-level ATX H1 or H2 ends the history section; blockquoted, list-contained,
and indented-continuation headings remain inside the audited section, so they
cannot hide a resumed forged table. A table below a new top-level heading
cannot remain bound to the canonical H2. The one canonical H2 itself must start
at column zero and be top-level: blockquoted,
list-contained, and indented list-continuation versions remain visible for
duplicate detection but cannot own the outside table. The sole owner must equal
the canonical normalized word sequence; broader
containment matching is used only to reject prefixed/suffixed lookalike H2s.
Raw HTML comment
delimiters are forbidden anywhere in the ledger because a multiline inline
comment can otherwise hide the canonical table without changing its source
rows or checkpoint. Raw HTML tags and other CommonMark raw constructs are
rejected from their opening syntax wherever they occur, without depending on
parsing a closing `>` through quoted attributes; text before
`<details title="<">`, processing instructions, declarations, or CDATA cannot
bypass the audit and hide the unchanged table. Literal rank
values such as `>50` remain valid because they are not tags. Heading
identity is compared after HTML character-reference decoding. Inline markup
marker characters are forbidden even intraword or entity-encoded, rather than
being stripped into a canonical title the renderer does not produce. After
ATX closing-marker parsing, the canonical owner title must equal the prescribed
plain source text exactly; punctuation, entities, backslashes, and repeated
spaces cannot disappear through word tokenization.
Invisible Unicode format
and mark characters are rejected after entity decoding. Every machine-ledger
heading is ASCII-only, so Cyrillic, Greek, and other homoglyphs cannot evade
canonical lookalike detection. The machine ledger permits only ATX headings:
every visible standalone Setext or thematic underline is rejected before
section lookup, including quoted, list-contained, lazy-continuation, and nested
forms. This fail-closed grammar avoids joining source lines across CommonMark
container ancestry and avoids both soft and hard title newlines that GitHub
renders as `<br>`.
Because this evidence file is a data ledger rather than
general documentation, inline
links or HTML are forbidden in headings, and fenced, GitHub display-math, or
raw-HTML blocks are rejected fail-closed anywhere in the document. They cannot
turn the canonical table into rendered code/math or an unaudited Markdown/HTML
surface; ATX syntax behind four-space or tab code indentation is rejected for
the same reason. The checkpoint
schema also requires a non-boolean JSON integer version before accepting
version `1`. The GitHub surface and every measurement require exact non-boolean
integers for the top-50 result window and `per_page`; Python's numeric equality
cannot admit JSON floats such as `50.0` into authoritative replay data. The
reviewed bootstrap imported the 22 GitHub rows that predate the registry, the
initial registry, both history checkpoints, and their roadmap projection. The
audit's one-time initialization path accepts only those exact reviewed 108-row
and 130-row digests plus the byte-exact registry, ledger, and checkpoint files.
Its schema validation covers the registry version, semantic identity versions,
both surface contracts, query lifecycle/KPI/alias rules, one-way
identity-preserving retirement, unambiguous backtick projection,
provider-scoped legacy-history bounds, unprojectable raw HTML, Unicode controls,
pipes, line separators, and the retired `AI-native compiler` authorship
diagnostic. Google retirement remains activation/clock-bounded until the
registry gains a Google snapshot series.

The historical activation copied the reviewed fixture byte-for-byte to
`workflow-policy.yml`, proved the required `audit` run, and retired the older
roadmap-only digest plus the one-use bridge. That trust-anchor workflow remains
exactly pinned. Under D-172, its base-owned checker downloads candidate
workflows and protected inputs as non-executable Git data, requires `100644
blob` modes, rejects root or nested `.gitattributes`, and runs the isolated
Python auditor against the trusted base ledger. General CI/checker changes are
authorized by the named policy properties in one pull request; the successor
manifest remains historical bounded-input inventory, not a future-byte
activation mechanism.

The regular PR job runs this checker for fast feedback only; pull-request code
can change its own workflow. The authoritative `Workflow policy` workflow uses
`pull_request_target` on every pull request, checks out the trusted base commit,
downloads the head revision's workflows, search evidence, successor-manifest
inventory, and protected inputs through the read-only GitHub API,
and treats them as data. It never checks out or executes
pull-request code, so the check can remain required without path-filtered runs
getting stuck as pending. Its checkout uses `github.sha`, which
`pull_request_target` defines as the latest commit on the base branch; do not
substitute the webhook payload's potentially stale `pull_request.base.sha`.
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
`ci.yml` that fans in every job in that workflow (`build-test-coverage`, all
four `native-build-test` Tier-1 legs, `cross-compile-build`,
`cross-compile-verify`, `frontend-perf-measure`, and
`frontend-perf-gate`) so branch protection enforces the whole Tier-1 matrix
and performance invariant through one required-check name that survives
matrix edits, rather than naming each generated context directly. The switch
from directly requiring `build-test-coverage` to
requiring `ci-gate` happened once `ci-gate` existed on `main` (PR #19,
merged 2026-07-25) -- it was deliberately not done inside that same PR,
since flipping it earlier, while other branches were still open against a
`main` without this job, would have left those PRs waiting on a required
check they had no way to satisfy. Removing either required check, disabling
strict mode, accepting an `audit` context from another app, or dropping a
job from `ci-gate`'s `needs:` list is a policy regression; all later policy
changes are evaluated by the trusted checker from their base revision.

## CI temporary-bypass lifecycle (D-125)

`scripts/test_manage_ci_bypass.py` covers `scripts/manage_ci_bypass.py`'s
`status`/`relax`/`restore`/`restore_to_baseline` lifecycle at 100% line
coverage, run via `python3 -m coverage run -m pytest
test_manage_ci_bypass.py` from `scripts/`. Every `CiBypassError`-raising
branch has a dedicated test: a `gh` failure, an already-open `[ci-bypass]`
incident (refuses to stack), a check that isn't currently failing or isn't
a required check, a missing or unreadable `--evidence` file, an unparseable
snapshot or Expiry timestamp, a `gh issue create` whose output has no
parseable issue number, drift after `restore` or `restore_to_baseline`, and
`restore`'s CLI wiring rejecting `--incident`/`--to-baseline` given together
or neither given with no prior `state.json` to fall back to.

`status()` compares the normalized full 7-field protection snapshot against
`BASELINE_PROTECTION`, not just the required-checks list, so DRIFT tests
cover both a `required_status_checks`-only mismatch and a mismatch confined
to another field (e.g. `enforce_admins`). A realistic GitHub review-protection
fixture includes the response-only `url` field and proves that metadata is
absent from status, incident, and restore/readback snapshots, while one
parameterized regression changes each of the four effective review-policy
fields and proves every change still reports DRIFT. Additional regressions
prove that an effective or unclassified extra field is preserved and reports
DRIFT rather than being mistaken for metadata. A separate test preserves `None`
when pull-request reviews are disabled, and legacy-incident regressions prove
that snapshots already persisted with `url` still explain live drift and
restore cleanly through the normalized readback. Separately, `status()` also
detects a `[ci-bypass]` incident that is open past its own recorded expiry
with no restore recorded -- DRIFT even when protection itself currently
matches baseline -- and the combined case where both conditions hold at
once; an incident whose body has no parseable Expiry line is skipped rather
than crashing the check. `status()` also recognizes when the observed drift
is fully explained by a currently open, unexpired incident's own recorded
pre-relax snapshot and relaxed check (an in-progress relaxation, reported
`ok`, not release-blocking DRIFT) -- with dedicated tests for the case where
an open incident does *not* explain the observed drift (must still report
DRIFT, never blanket-suppressed just because an incident happens to be
open) and where the incident's body has no parseable snapshot or "Check
relaxed" line (skipped, not crashed). Two more tests isolate the exact
mutants an independent review found surviving an earlier version of this
suite: one where `contexts` matches the incident's prediction exactly but
`enforce_admins` also drifted (must still report DRIFT, proving the
comparison is the full dict, not just `contexts`), and one where the
incident names the wrong check (`ci-gate` named as relaxed while `audit`
is the one actually missing -- must still report DRIFT, proving
`check_name` itself is what's compared, not merely presence/absence of
any context).

Authenticating an incident's author matters differently depending on what
trusting the wrong one would cause, and the tests are organized around
that split. `status()`'s live-incident branch is the one place a forged
issue's content could *suppress* a safety signal (blind the only automated
DRIFT detector, indefinitely, using only `BASELINE_PROTECTION` -- a public
literal in this file -- and a far-future Expiry), so it requires the
issue's author to match `get_authenticated_login()` before an incident may
suppress DRIFT; a dedicated regression test reproduces that exact exploit
(same check, same snapshot, unexpired, but authored by `"attacker"`) and
asserts DRIFT is still reported, plus a test that the lookup is cached
(one `gh api user` call even across multiple open issues in the loop).
`find_open_bypass_issue`'s and `restore_to_baseline`'s stacking guards are
deliberately left unauthenticated -- a forged issue there only makes the
tool refuse and escalate to a human, the correct fail-closed outcome, not
a suppression risk.

`relax()` refuses `ci-gate` before making any `gh` call at all -- it
reflects the candidate's own build/test/coverage result, never external
repository state, and the skill's documented exclusion is enforced here in
code, not left to prose alone.

`create_incident_issue()` refuses to create an issue whenever its fully
assembled body contains this mechanism's own snapshot-marker text more
than once -- `parse_snapshot_from_body` reads the *first* occurrence of
the marker, and the function's own genuine marker is always last, so
marker-shaped text in `--evidence` (influenced by CI failure text, which
can itself be influenced by a PR's own content) or in `--reason` (which
also lands directly in the issue title) would otherwise be parsed as
authoritative on a later `restore`, even inside an issue that is correctly
titled and authored. Checking the assembled body once catches both fields
-- and any field added later -- rather than enumerating them individually;
dedicated tests inject the marker through each field separately and prove
`relax()` refuses before ever calling `gh issue create`.

`restore()`'s `get_incident_body()` only trusts an incident's embedded
snapshot when the issue's title starts with `[ci-bypass]` *and* its author
matches the currently authenticated `gh` actor (`get_authenticated_login()`)
-- closing the gap where a public issue forging both the title and the
`<!-- ci-bypass-snapshot -->` marker, opened by anyone else, could otherwise
have its snapshot applied to branch protection by a later `restore
--incident`. Both rejections (title, author) have dedicated tests, including
one proving the author check is never reached when the title check already
failed, one proving no `PATCH`/comment/close call happens on an author
mismatch, and one proving a `null` GitHub `author` (e.g. a deleted account)
fails closed as `CiBypassError` rather than an uncaught `TypeError`.

`restore()` itself adds two more predicates as defense in depth beyond that
check, for a body that was edited after creation or an incident that
predates it: the snapshot's `contexts` must equal `BASELINE_CONTEXTS`
*exactly*, and `strict` must be `true`. An earlier version of the first
predicate only required `NEVER_RELAXABLE_CHECKS` to be a subset of
`contexts` -- which a snapshot dropping `audit` while keeping `ci-gate`
present would still have passed, permanently un-requiring `audit` (the
`pull_request_target` trust anchor `AGENTS.md` calls "never permanently
remove or downgrade") while `restore` reported success. Three dedicated
tests cover this predicate: dropping `ci-gate`, dropping `audit` while
keeping `ci-gate` (the exact regression case above), and adding an extra
context beyond baseline (which would permanently wedge every future PR on
a check that can never report). A fourth test covers `strict != true`
alone. Each proves no `PATCH`/comment/close call happens on rejection.

`relax()`'s TOCTOU re-check -- `find_open_bypass_issue` called once before
any work starts and again immediately before the mutating `PATCH`, narrowing
(not eliminating) the window where a concurrent session's relax could stack
underneath this one -- has its own test: the first call reports no open
incident, the second reports a different one that appeared in between, and
`relax()` must abort before the `PATCH` with the other incident's number and
a manual-cleanup pointer to the incident it already created, without ever
calling `patch_required_status_checks` or writing `state.json`.

## Code coverage (D-014, narrowed by D-242)

Distinct from the grammar-coverage gate in Meta below (which measures PEP/language-surface coverage): this is ordinary line/region coverage of pycc's own Rust source, measured on every compiler-relevant pull request selected by the fail-closed classifier and every push to `main`.

**The gate ([D-242](decisions/D-242-product-mode-the-delivery-process-informs-rather-than-blocks.md) rule 1):** the merge invariant is 100% line
coverage of the Rust lines the pull request adds or modifies. It is computed
by `scripts/check_diff_coverage.py`, which joins the `cargo llvm-cov
--workspace --lcov` export against a zero-context unified diff of the change
(its base commit to `HEAD` in CI; the merge base with `origin/main` locally):

- `--lcov PATH` (required) is the LCOV export; `DA:` records give per-line
  hit counts (the maximum across duplicate records), `LF`/`LH` the totals.
- `--diff PATH` (required) is a `git diff -U0` output, possibly empty; a
  C-quoted path (`+++ "b/…"`) fails closed with exit 2.
- `--root DIR` (default: the current directory) is stripped from absolute
  `SF:` paths so they match the diff's repository-relative paths.
- `--require-changed-lines N` (0–100, default 100) is the threshold.
- A diff path counts as a *changed source file* exactly when cargo-llvm-cov
  0.8.7 instruments it by default (`src/report.rs:919-923`): it ends in
  `.rs`, is not `build.rs`, has no `tests`, `examples`, or `benches`
  directory component, and its basename does not match
  `^(tests\.rs|[0-9a-zA-Z_-]+[_-]tests\.rs)$`. So `src/expr/tests.rs`,
  `class/enum_call_tests.rs`, and `tests/x.rs` are ignored while
  `src/foo_test.rs` and `src/testsuite.rs` count.
- For each changed source file, every added or modified line with a `DA:`
  record is counted; lines with no record (comments, braces, signatures) are
  ignored; a changed source file with no LCOV record at all fails the gate
  closed ("no coverage data for changed source file"), the same fail-closed
  policy as the exemption table below.
- Output: per-file uncovered lines, `changed lines: N, covered: M (P%)`, and
  `workspace lines: LH/LF (P%)`; exit 0 when P ≥ threshold (a change with no
  counted lines passes), 1 otherwise, 2 on a usage or parse error.

Total workspace line coverage (the LCOV totals line) and the per-crate
region table (`run_isolated "$TRUSTED_COV" llvm-cov report
"${WORKSPACE_PACKAGE_FLAGS[@]}"`) are printed in the same job as a report,
never as a threshold. **What the diff gate cannot see:** coverage lost on
*unchanged* lines — a deleted test that orphans an old path — never fails
the gate; the `report` TOTAL row and the totals line are the only signal for
that drift, and the D-068 review reads them. Locally:

```
cargo llvm-cov --workspace --lcov --output-path target/coverage.lcov
git diff -U0 --no-color --no-renames "$(git merge-base origin/main HEAD)" HEAD > target/changed.diff
python3 -B scripts/check_diff_coverage.py --lcov target/coverage.lcov --diff target/changed.diff --root "$PWD" --require-changed-lines 100
cargo llvm-cov report
```

### A runtime function an integration test links is measured from that binary

`llvm-cov`'s export keeps one record per function across the objects it is
given, so a `pycc_rt` function that an integration-test binary under
`crates/pycc_rt/tests/` also links is reported from *that* binary's copy and
the lib-test binary's counters for it are discarded. A line exercised only by
a `#[cfg(test)] mod tests` unit test in `crates/pycc_rt/src/lib.rs` then reads
as uncovered in the `--workspace` export the gate consumes, even though
`cargo llvm-cov --lib -p pycc_rt` shows it covered and the unit test passes.
`pycc_rt_buffer_f64_alloc` is one such function (`crates/pycc_rt/tests/buffer_live_views.rs`
links it), which is why its refusal arms are asserted there as well as in the
unit tests.

When a changed `pycc_rt` line is covered by `-p pycc_rt --lib` but uncovered
by `--workspace`, this is the first thing to check; it is not flakiness. The
one-run diagnostic is a workspace export with a test-name filter
(`cargo llvm-cov --workspace --lcov --output-path … -- <filter>`): if the
function's lines export as `0` while its tests are listed as `ok`, its
counters are being shadowed, and the assertion must be made from an
integration test to be visible to the gate.

**Threat model:** `scripts/check_roadmap_evidence.rb` audits the coverage
job by named properties (see "Roadmap acceptance evidence" above), which
proves *shape* — unprivileged sandbox, trusted binary, workspace denominator,
pinned changed-line denominator (the `git diff -U0 --no-color --no-renames
"$COVERAGE_BASE_SHA" HEAD` command is itself a required line, so neither a
pathspec nor rename detection can shrink the measured line set), gate command
line, threshold — not byte identity; a hostile edit placed
between the required lines is caught by the D-068 full-diff review that
precedes every merge, which is the compensating control.

**D-171 change-aware scheduling (active 2026-08-15):** The active workflow is
validated by D-172's base-owned named-property audit rather than by equality to
the historical `ci-d171.yml` fixture or a live whole-file digest.
An always-run classifier may skip a heavy category only for an exact reviewed path set;
unknown, empty, malformed, or unsupported input selects the complete topology,
and the required `ci-gate` fails unless every selected job succeeds and every
unselected job is skipped. Pushes to `main` always run the full topology.
Every compiler input also selects the offline alpha skill evals: those evals
execute the freshly built compiler and bind its current diagnostics and D-072
backend boundary, so they are part of the compiler contract rather than an
agent-assets-only check.

Two documents under `docs/` are classified as compiler inputs rather than cheap
prose, because a Rust test reads each one as data: `docs/DIAGNOSTICS.md`, read by
the diagnostics-registry tests, and — since
[#591](https://github.com/rotnov/pycc/issues/591) — `docs/PYTHON_STANDARDS.md`,
read by `tests/conformance_matrix_guard.rs`. Left in the `docs/` bucket they
classify as an empty selection, so the change most likely to break each guard —
editing the very document it asserts on — is the change that skips it, and the
breakage surfaces on the later `main` push run instead of on the pull request
that caused it. That is a deliberate exception to the classifier's
docs-are-cheap rule and not a relaxation of it: a pull request that promotes a
conformance matrix row is asserting that the row's fixtures pass, so running the
compiler jobs on it verifies the claim being made rather than adding unrelated
cost. `scripts/test_classify_ci_changes.py`'s
`test_every_input_the_conformance_matrix_guard_reads_selects_compiler` binds all
three of that guard's inputs, so returning the matrix to the cheap-docs bucket
fails the classifier's own self-test.

This active scheduling change supersedes only D-014's instruction to execute
coverage on literally every pull request: coverage remains mandatory for every
compiler-relevant pull request selected by the classifier and every push to
`main`. It does not change the coverage threshold, the full-workspace
denominator, the isolated `nobody`
sandbox, tool pins, or whole-file exemption policy. Compiler-relevant changes
still require the complete Tier-1 native matrix, cross-compilation build and
verification, and paired frontend performance gates; Pages-relevant changes
still require both Pages gates and their existing budgets. `audit` and the
fail-closed `ci-gate` remain required.

- Tool: `cargo llvm-cov` — a separately distributed cargo subcommand, **not** bundled with any rustup component. CI installs it explicitly and pinned (installer action or `cargo install cargo-llvm-cov --locked --version <pinned>`), plus the `llvm-tools-preview` rustup component it drives at runtime; a bare "install llvm-tools" fails with "no such command: llvm-cov" (caught by repo audit, issue #13). Independent of the Homebrew LLVM used by `inkwell` for codegen — versions don't need to match.
- Gate (D-242 product-mode shape): `run_isolated "$TRUSTED_COV" llvm-cov --workspace --lcov --output-path "$ISOLATED_ROOT/coverage.lcov"` exports the whole-workspace LCOV, `run_isolated "$TRUSTED_COV" llvm-cov report "${WORKSPACE_PACKAGE_FLAGS[@]}"` prints the informational per-crate table, and `python3 -B scripts/check_diff_coverage.py --lcov "$ISOLATED_ROOT/coverage.lcov" --diff "$RUNNER_TEMP/coverage-changed.diff" --root "$GITHUB_WORKSPACE" --require-changed-lines 100` (run as the runner user, outside the sandbox, against the `git diff -U0 --no-color --no-renames "$COVERAGE_BASE_SHA" HEAD` the step wrote first) is the threshold; run in CI on at least one Tier-1 target per PR. The explicit `llvm-cov` argument is required when invoking Cargo's subcommand binary directly. CI resolves the exact event base (`PR_BASE_SHA`/`PUSH_BASE_SHA`, validated as a full non-zero SHA and fetched by hash) and installs the trusted tool before executing repository code, then runs the cross-target `pycc_rt` prerequisite, workspace build, and coverage under `sudo -u nobody env -i` with isolated HOME, Cargo home, temp, and target directories. The workspace and runner-owned toolchain/binary are read-only to that user, so a build script or procedural macro cannot replace the executables or write GitHub command files. The checker audits the job by named properties (setup steps, gate `env`, required gate lines in order, the `run_isolated` sandbox definition) rather than a byte-exact prefix; no head-controlled policy step runs earlier in this coverage job, while the separate always-run governance job remains unprivileged. The x86_64 macOS runtime is built first so the cross-compilation test cannot skip its success path, then `cargo build --workspace` supplies the normal debug `pycc_rt` used by the remaining slice-0 tests. The pinned tool's version smoke check runs immediately before entering the boundary.
- Test code itself (`tests/`, `examples/`, `benches/`, `build.rs`, `tests.rs`, `<name>_tests.rs`, `<name>-tests.rs` — exactly the basename regex `^(tests\.rs|[0-9a-zA-Z_-]+[_-]tests\.rs)$` stated in the "Code coverage" section's file predicate) is excluded from the denominator automatically by cargo-llvm-cov's default exclusion (`src/report.rs:919-923` in 0.8.7), and `scripts/check_diff_coverage.py` applies the identical predicate to the diff — the gate measures product code exercised by tests, not tests covering themselves, and a test-only pull request has no changed source lines and passes.
- Exemptions are whole-file only, via `--ignore-filename-regex` (no per-function opt-out exists on stable Rust — see D-014). Each exemption needs a named entry here:

  | File pattern | Reason |
  |---|---|
  | *(none yet)* | — |

  An uncovered file with no entry in this table is a review-blocking finding, not a gap to wave through.

- **Practical notes on what actually shows up as a coverage gap** (learned building the first few v0.1 crates — verified directly against `cargo llvm-cov`'s HTML report, not assumed):
  - A hand-written `match { expected => ..., _ => panic!("...") }` — in test code or production code — creates its *own* region for the `_`/catch-all arm. If nothing ever exercises that arm, it's a gap, even though the arm is real and reachable. In tests, prefer `#[derive(Debug, PartialEq)]` on the type under test plus `assert_eq!(actual, expected)` over a manual match-and-panic assertion — it needs no catch-all arm at all.
  - **`.expect()`/`.unwrap()` do *not* have this problem**: their internal panic branch lives inside libcore/libstd, outside the calling crate's instrumented regions, so a call that always succeeds in every test still reads as 100% covered. This is the right choice for an operation that's genuinely infallible given the caller's own invariants (see `pycc_codegen::compile_to_object`'s `.expect()`s on IR-construction operations, and `pycc_codegen`'s `target_machine::target_machine_for` on target-machine creation — operations that no input can make fail once `Target::from_triple` has already validated the requested triple).
  - **A closure passed to a combinator (`.map_err(|e| ...)`, `.and_then(...)`, etc.) is tracked as its own function/region and *does* need to actually run** — if the `Result`/`Option` it's attached to never takes that branch across the whole test suite, the closure body shows as a missed region even though the call site's own line is "covered." Reserve `Result`-returning `.map_err(...)` for failure modes a test can actually trigger (e.g. a bad output path); use `.expect(...)` for the rest instead of threading a `Result` no real input can produce.
  - **A function generic over `impl Fn(..)` (dependency-injection for testability — e.g. passing in a fake filesystem-existence check) gets monomorphized once per distinct closure type**, and each monomorphized copy is tracked *separately*: a copy that's only ever called with an always-true fake never executes that copy's error branches, and that reads as a real gap even though the *production* closure (or a different test's fake) exercises them. Fix: take a plain `fn(..) -> ..` pointer instead of `impl Fn(..)` when every caller's closure is non-capturing (as is typical for this kind of fake) — one concrete function pointer type means one compiled body, so coverage from every caller (production and every test) accumulates on the same counters. Only reach for `impl Fn`/`Box<dyn Fn>` when a caller genuinely needs to capture state; don't default to it for simple fakes.
  - **The per-file summary takes the *maximum* covered-region count over each function's instantiations, then sums those per-function maxima — it is neither a union nor a sum across compilations.** LLVM's `RegionCoverageInfo::merge` (`CoverageSummaryInfo.h`) does `Covered = max(Covered, RHS.Covered); NumRegions = max(NumRegions, RHS.NumRegions)`, and instantiations are grouped by the function's *definition location* (file, line, column) — which is how generic monomorphizations and the plain-vs-`--cfg test` compilations of the same crate group together. A crate in this workspace is compiled at least twice: once plainly (linked into the `pycc` binary and therefore into every integration test under `tests/`) and once with `--cfg test` for its own unit-test binary. Consequence: **a function whose regions are covered by *different* instantiations still reports a deficit of `NumRegions - max(Covered over instantiations)`**. To reach 100% for a function, one single instantiation must cover all of its regions — "the integration suite covers the rest" does not compose. Diagnosed exactly this way on [#603](https://github.com/rotnov/pycc/issues/603): new `HirExpr::UnaryOp` arms in `pycc_hir`, `pycc_mir`, and `pycc_types` were exercised end to end by `tests/issue_603_unary_general_operand.rs` but by no inline `mod tests` case, and `build-test-coverage` failed. **Every merged view hides this** — `--show-missing-lines`, `--lcov`, `report --text` without `--show-instantiations`, and the JSON `files[].segments` array all report the per-source-range *union* and show the lines as covered; aggregating the JSON region counts per source range likewise reports zero missed, which is what made the first fix look complete when it was not. The diagnostic that actually reproduces the gate's number: take `cargo llvm-cov --workspace --json`, group `data[].functions[]` by `min((r[0], r[1]) for r in regions_in_target_file)` (the definition location), and for each group compute `max(len(regions))` minus `max(count of regions with r[4] > 0)`; the sum over groups equals the summary's missed-region count exactly. Fix: add inline unit tests in the crate's own `mod tests` for any arm an integration test is the only thing reaching.
  - **A test that skips itself when an optional local prerequisite is missing (e.g. `tests/slice0.rs`'s cross-compilation test, which skips unless a `--target`'s `pycc_rt` has already been built locally) makes the coverage gate depend on incidental developer-machine state, not on the test suite itself.** A dev machine that accumulated that prerequisite from earlier manual testing shows 100%; a fresh CI runner that never built it sees the test skip and the branch it alone exercises reads as a gap — caught exactly this way when `build-test-coverage`'s CI job (a clean checkout) showed 3 missed regions/1 missed line in `src/main.rs` that a local run right before pushing did not, and reproduced precisely by moving the local prerequisite build aside and rerunning. Fix: give the coverage-gated CI job whatever setup makes the skip-guard's precondition always true there (here, building that one cross-target's `pycc_rt` in the same job), so the gate never rides on whether *this specific* environment happens to have accumulated the right state.
  - **A skip guard that hard-codes an artifact path is the same hazard one level down.** `tests/slice0.rs`'s cross-target guards used to ask whether `<manifest dir>/target/<triple>/debug/libpycc_rt.a` existed. Under the coverage job that path is only correct because the job symlinks `$GITHUB_WORKSPACE/target` at its isolated `CARGO_TARGET_DIR`; anywhere else that redirects the target directory, the guard reports "not available", the test skips, and the branch it alone exercises reads as a gap — a green test run and a red gate, from the same tree. Since [#629](https://github.com/rotnov/pycc/issues/629) every such lookup resolves through `pycc_codegen::artifact_layout::resolve_cargo_target_root` (D-183), which honors `CARGO_TARGET_DIR` and `CARGO_BUILD_TARGET_DIR`, so the guard answers correctly wherever Cargo actually wrote the artifact.

### Mechanical gate: no hard-coded Cargo target-directory paths

`tests/target_dir_literals.rs` is an ordinary workspace `#[test]`, not a CI
step, and therefore runs inside the existing required `cargo test` and
coverage jobs. It fails if any `.rs` file under `src/`, `crates/`, `tests/`,
or `benches/` contains a `target/`-shaped path or a `.join("target")` in
non-comment code, directing the author to
`pycc_codegen::artifact_layout::resolve_cargo_target_root` instead.

Three construction constraints, each load-bearing:

- Its needles are assembled with `concat!`, so the gate does not match its
  own source and cannot be made permanently red by its own existence.
- It scans a written-out directory allowlist rather than walking the
  workspace root: under the coverage job the root contains a `target`
  symlink into the isolated artifact tree, and a recursive walk from there
  would descend into build output. A companion test asserts the scan still
  finds this workspace's Rust sources, so a renamed directory fails loudly
  instead of silently scanning nothing.
- It does not shell out to `git ls-files`: the coverage job runs as
  `nobody` under `env -i` with no `git` on `PATH`.

The matcher is a pure function over one file's text, unit-tested with both
an offending and a clean sample. That is deliberate under D-014: driven
only from the real tree, the "offender found" arm would never execute —
because the tree is, and must stay, clean — and would read as a missed
region.

A CI step would have been the more obvious home, but `.github/workflows/ci.yml`
edits that need new checker knowledge of the changed shape hit D-172's
base-owned `audit` job, which validates the PR head's `ci.yml` against the
pre-PR (`main`) checker script rather than the PR's own updated one (D-103's
manifest-hash staging is retired; this is a narrower, still-live constraint
identified by [D-203](decisions/D-203-narrow-the-d-091-bench-manifest-tail-check-to.md)).
Adding a step that a frozen checker constant must recognize would force the
same D-203-precedented two-PR coexist-then-retire cycle for a check a
workspace test enforces identically.

It is a lexical gate and makes no claim beyond that: it catches the literal
shape that caused #629, not every conceivable way to reconstruct an
artifact path.

### Mechanical gate: Tier-1 target list stays consistent across the binary and docs

`tests/tier1_target_parity.rs` closes #246: the Tier-1 target set/order
(`src/main.rs::TIER1_TARGETS`) is independently duplicated in
`docs/ARCHITECTURE.md`'s "Cross-platform (hard requirement)" table,
`tests/slice0.rs`'s hardcoded snapshot of `pycc version --verbose`'s exact
output, and `docs/CLI_SPEC.md`'s illustrative transcript. `slice0.rs`
already pins the binary's real output against its own literal, but nothing
tied that literal, or the binary's real output, back to either
documentation source — a mutation that changed the code and the
`slice0.rs` snapshot together, while leaving both docs untouched, passed
every existing test.

This test is a deterministic parity validator, not a generator: it
re-derives the Tier-1 list from the binary's real `pycc version --verbose`
stdout and from both documentation sources independently of `slice0.rs`'s
hardcoded literal, and asserts all three name the same five targets in the
same order. Both `include_str!`-loaded documentation texts are normalized
with `.replace("\r\n", "\n")` before any marker search or line split,
exactly as `tests/diagnostics_test.rs` normalizes its own fixture
comparisons — a Windows checkout's `core.autocrlf` text conversion would
otherwise turn a literal `\n`-terminated search marker into a byte
sequence the raw checked-out text never contains, panicking the test on
this repository's own required Windows CI leg.

`docs/ARCHITECTURE.md` and `docs/CLI_SPEC.md` are test inputs for this
guard, not only prose, so `scripts/classify_ci_changes.py`'s `COMPILER_FILES`
lists both explicitly — the same treatment `docs/PYTHON_STANDARDS.md`
already got for `tests/conformance_matrix_guard.rs` (issue #591). Without
that entry, a pull request editing only one of these two documents to
introduce a Tier-1 mismatch would classify as docs-only and skip the
compiler jobs that run this guard, surfacing the mismatch only on the
later `main` push instead of on the pull request itself.

## Scratch directories (issue #781, Part 1 of #779)

#779 found ~384 ad hoc `std::env::temp_dir().join(...)` call sites across 36
tracked test files (plus two production call sites in `src/main.rs`), each
building its own scratch directory by hand: no shared cleanup, so a panic
partway through a test left the directory behind, and no collision-safe
naming, so two call sites in the same test binary could pick the same name
and collide (the exact defect `crates/pycc_codegen/src/tests_support.rs`'s
`TempTestDir`/`tempfile_dir` had before Part 1 — its `pycc_codegen_test_{label}_{pid}`
naming used only the process ID, so two call sites sharing a `label` in the
same test binary raced on the same directory).

## Accepted-decision immutability (issue #1000, Part 2 of #77)

`AGENTS.md` has always said that an accepted decision is never rewritten, but
nothing enforced it: PR #74 appended a parenthetical to the accepted
Consequences text of D-032 and merged with every required check green, and
Part 1 of #77 (#999, PR #1001) restored the line by hand.
`scripts/check_decision_immutability.py` (D-240) is the guard.

**What it compares.** Only the event's own `base..head` range, two-dot:
`git diff --name-status --no-renames <base> <head> -- docs/decisions/`,
restricted to paths matching `docs/decisions/D-<n>-<slug>.md` (anchored, no
`/` in the basename, so `README.md`, `TEMPLATE.md` and a file inside a
directory named like a decision are ignored). History is never re-judged: a
replay of the rule over the 35 first-parent commits in `7ae33ae1..4b317abe`
that modified an existing decision file would have failed 19 of them
(including Part 1's own restoring merge, which is itself an in-place
replacement), and none of those are retrofitted -- a pull request answers
only for the lines it removes or changes.

**The rule.** A file is frozen when its base frontmatter says `accepted` or
`superseded`; `proposed` files, files new at the head (`A`), and base files
whose frontmatter does not parse are unconstrained. For a frozen file, in
order: deletion (or, under `--no-renames`, the `D` half of a rename) is a
violation; the head frontmatter must parse and say `accepted` or
`superseded`, and `superseded` never returns to `accepted`; a D-151
index-only stub (the base's ninth line, 0-based `splitlines()` index 8, is
exactly `Index-only: no long-form entry recorded yet.` *and* no base line
starts with `- Status:` -- both, so the marker cannot be smuggled into a
long-form entry and exploited later; positional, so a marker at any other
index is not a stub and the file stays under the strict walk) may replace
its five stub body lines while its frontmatter and any later lines stay
frozen -- but only when the replaced stub body itself carries a well-formed
body status line (`- Status: accepted` or `- Status: superseded`, optionally
annotated): the first such head line after the frontmatter's closing blank,
with every frozen line after the stub reappearing in order after it, so a
status line placed after the frozen tail or inside the frontmatter unlocks
nothing; a head without one in that region keeps the stub body frozen and
fails the walk with an `index-only stub replaced without a long-form entry:
no '- Status: accepted' or '- Status: superseded' line in the replaced stub
body` hint, so deleting the stub body or replacing it with prose is a
violation;
otherwise an exact greedy subsequence walk over `splitlines()` (no
`keepends`, so a trailing-newline-only change passes) requires every base
line to reappear in the head verbatim and in order, except that the
frontmatter `status:` line (line 4) may be replaced by `status: accepted` or
`status: superseded` and the *first* body `- Status:` line by a line
starting with `- Status: accepted` or `- Status: superseded` (an annotation
such as `superseded by D-NNN` may follow after whitespace) -- the
status-transition and narrowing-annotation shapes. A replacement with any
other value (`proposed`, `rejected`, blank) fails and names the offending
head line. Any other diff status for
a frozen path (`T` for a symlink replacement, `C`, `R`, `U`, `X`) fails
closed. Blobs are decoded with `errors="surrogateescape"`, so a non-UTF-8
byte is a line mismatch, not a traceback. `difflib.SequenceMatcher` opcodes
are deliberately not the verdict (they misreport pure insertions as
deletions on repetitive ADR-like lines); `difflib` only prints context.

**Event plumbing.** With no flags the script reads `GITHUB_EVENT_NAME`: on
`pull_request` the base is `pull_request.base.sha` and the head `GITHUB_SHA`
(the `refs/pull/N/merge` commit whose first parent is `base.sha`, so two-dot
is exact); on `push` the base is `before` and an all-zero `before` is a skip;
any other event, or none, exits 2. The governance checkout is depth 1, so a
missing revision is shallow-fetched once (`git fetch --no-tags --depth=1
origin <rev>`), the `ensure_revision_available` shape from
`scripts/check_site_pin_merge_currency.rb`. Exit codes: 0 passed or skipped,
1 violation, 2 usage or plumbing error. Locally, use the merge base rather
than `origin/main` (a two-dot diff against a base that has since merged an
inserted amendment would report `main`'s own insertions as removals):

```
python3 -B scripts/check_decision_immutability.py \
  --base "$(git merge-base origin/main HEAD)" --head HEAD
```

**Binding.** The step `Check accepted-decision immutability (issue 1000)`
(deliberately no `#` in the name: an unquoted `#` in a YAML scalar starts a
comment) runs in `.github/workflows/ci.yml`'s `governance` job, which the
required `ci-gate` context needs unconditionally;
`scripts/test_check_decision_immutability.py`'s `CiWiringTest` fails if the
step, the `needs`, or the `ci-gate` truth-table entry is removed or the job
is made advisory. The step is also listed in `D171_GOVERNANCE_POLICY_STEPS`
in `scripts/check_roadmap_evidence.rb`, with the
`tests/fixtures/policy-successors/ci-d171.yml` fixture and
`D171_CHANGE_AWARE_CI_WORKFLOW_SHA256` rotated in the same pull request (the
#936 precedent), so the base-owned `audit` rejects a later head that drops,
conditions, or replaces it. During the pull request that adds the step, the
base-owned `audit` cannot yet see it -- unlisted steps are invisible to the
base checker, which was verified to accept the head's `ci.yml` with the new
step present, with its `run` replaced, and with `if: always()` added -- so
the head-controlled `ci-gate` is the only enforcement until the merge, after
which `audit` binds it too.

**Evidence.** `scripts/test_check_decision_immutability.py` replays the
literal PR #74 append on D-032's current bytes (`base line 15 removed or
changed`), rewords, deletes, renames, symlinks and status-regresses frozen
files, fills in the real D-001 and D-005 stubs, rejects a stub whose body is
deleted or replaced without a `- Status: accepted`/`superseded` line in the
replaced body (a decoy after D-005's frozen tail, or between its tail lines,
unlocks nothing) and a status replacement carrying any other value, and runs
every current
`docs/decisions/D-*.md` against itself; its plumbing tests build two-commit
throwaway repositories and drive `main` with explicit revisions, fake
`pull_request`/`push` events, and a depth-1 clone that must trigger exactly
one shallow fetch. Local Ruby suites need a UTF-8 locale
(`LC_ALL=en_US.UTF-8`); CI's runner already has one.

## Multi-file (project-import) tests

A multi-file program's layout is part of what is under test, so a project
fixture is *built at run time* inside a `pycc_scratch::ScratchDir`, never
checked in under `tests/fixtures/` (which `tests/conformance.rs` treats as a
flat, single-file corpus by D-221). `src/modules.rs`'s unit tests and
`tests/issue_881_project_imports.rs` follow that shape: write the package
tree, invoke the CLI with an absolute path into the scratch directory, and
assert with `contains` on the parts of the render that are stable. Byte-exact
whole-output snapshots stay in `tests/diagnostics/`, whose fixtures are
repo-relative single files run from the repository root, so their rendered
paths are machine-independent.

`pycc_scratch::ScratchDir` (new workspace crate `crates/pycc_scratch`,
`crates/pycc_scratch/src/lib.rs`) is the one correct way to obtain a scratch
directory in this repository going forward: `ScratchDir::new(category:
&str) -> io::Result<Self>` creates a directory named
`pycc_{category}_{pid}_{nanos}_{seq}` under `std::env::temp_dir()`
(process ID, full epoch nanoseconds, and a per-process atomic sequence
number — collision-safe both within a process and, in practice, across
process restarts) and derefs to `Path`; `Drop` removes the directory tree
unconditionally, including while a panic unwinds through the owning stack
frame. See the crate's own doc comments for the exact naming-format
stability commitment.

`scripts/check_scratch_dir_usage.py` (self-tested by
`scripts/test_check_scratch_dir_usage.py`, wired into the `governance` CI
job) is the enforcement mechanism: it fails on any tracked `.rs` file that
contains more raw `temp_dir().join(...)` occurrences than a checked-in
snapshot allowlist records for it, so new test code cannot add a fresh raw
call site, and an already-listed file cannot grow past what it already had
when Part 1 merged. It is a textual pattern match, not a data-flow
analysis — see the script's own docstring for the accepted scope
limitation.

**This section describes the state of the tree, not just the pattern going
forward.** Part 1 (#781) added `pycc_scratch` and the lint above, and
retired `crates/pycc_codegen/src/tests_support.rs`'s
`TempTestDir`/`tempfile_dir` wrapper (never part of the `ALLOWLIST`
backlog — it already wrapped the raw pattern behind its own helper, so the
lint never saw it) directly onto `ScratchDir`; `tests_support.rs` no longer
exists in the tree. Part 2
([#782](https://github.com/rotnov/pycc/issues/782)) then migrated every
`ALLOWLIST`-tracked test call site onto `ScratchDir` —
`tests/quick_start.rs` last, since the public site's versioned
evidence-hero contract (`docs/WEBSITE.md`, enforced by
`scripts/check-site.sh` via `site/evidence-heroes.json`) pinned that
file's exact bytes and its migration therefore rode a full re-attestation
of the landing hero. Part 3
([#783](https://github.com/rotnov/pycc/issues/783)) finished the job by
migrating `src/main.rs`'s two production call sites — the temp object
`try_build` emits before linking and `run`'s built executable — onto
caller-owned `ScratchDir`s (`pycc_scratch` moved from the root manifest's
`[dev-dependencies]` to `[dependencies]`): `main()`'s `Command::Build` arm
and `run()` each create one via `src/main.rs::create_scratch`, inject the
object path into `try_build`, and drop the directory — removing its
contents — on every exit path once the linker (and, for `run`, the awaited
child process) is done with the files inside. An unusable system temp
directory now fails fast at scratch creation as a CLI_SPEC.md exit-2
environment error, before any frontend work. The snapshot allowlist in
`scripts/check_scratch_dir_usage.py` is now **empty** — its terminal
state, and #779's completeness signal for Parts 2/3: every tracked `.rs`
file except the `pycc_scratch` implementation itself is held to zero raw
occurrences. `tests/slice0.rs`'s
`successful_build_and_run_leave_no_temporary_files_behind`
(`#[cfg(unix)]`-gated; Windows can transiently fail to delete a
just-exited executable, and `ScratchDir::drop` deliberately ignores
removal errors, so an unconditional assertion would flake the required
`windows-latest` leg) is the leak-regression proof that one successful
`pycc build` plus one successful `pycc run` return a controlled temp
directory to empty.

Part 4 ([#784](https://github.com/rotnov/pycc/issues/784), design in
D-209) adds the defense-in-depth half `Drop` cannot provide — cleanup
after a process that died without unwinding. Every root now contains a
`pycc_scratch::LOCK_FILE_NAME` marker held under an exclusive OS advisory
lock (`std::fs::File::lock`) for the handle's lifetime — kernel-released
even on SIGKILL, so a successful `try_lock` probe from another process is
exact, PID-reuse-immune proof the creator is dead. Before creating their
own root, `pycc build`/`pycc run` (via `src/main.rs::create_scratch`) run
`pycc_scratch::sweep_stale_roots()`: a silent, best-effort pass over the
OS temp directory that deletes only entries which fully parse as
`pycc_{category}_{pid}_{nanos}_{seq}`, are real directories (not
symlinks), exceed the age floor for their class (1 h for lock-bearing
roots, 24 h for roots without an observable marker: pre-Part-4 legacy
ones, or a root killed before its lock file was created; both measured
against the directory's mtime), and hold no live lock — under budgets (10,000
entries / 512 deletions / 250 ms) that bound the caller's added latency.
The sweep's regression tests live in three places:
`crates/pycc_scratch/src/sweep.rs`'s unit tests cover the ownership-parse
accept/reject table, every keep/delete class, each budget, and the error
folds hermetically (an injected sweep root and `now` — portable to the
`windows-latest` leg, with deletion-success assertions
`#[cfg(not(windows))]`-gated per the same Windows delete-contention
class the previous paragraph documents);
`crates/pycc_scratch/src/lib.rs`'s unit tests pin the lock-file
lifecycle (held while alive, released by drop, creation-failure cleanup);
and `tests/slice0.rs`'s
`build_sweeps_a_provably_stale_scratch_root_and_spares_everything_else`
(`#[cfg(unix)]`-gated) proves the production wiring end to end through a
spawned `pycc build` against a redirected temp directory.

### Operational TMPDIR guidance (issue #785, Part 5 of #779)

Part 5 ([#785](https://github.com/rotnov/pycc/issues/785)) is the
operational complement to Parts 1–4: guidance for the runs and directories
the in-tree mechanisms cannot reach on their own. It is operational
practice, not a CI gate — the behavior underneath it is enforced by
`crates/pycc_scratch/src/lib.rs`'s unit tests (normal-`Drop` and
panic-unwind removal, parallel-creation uniqueness, lock-file lifecycle),
`crates/pycc_scratch/src/sweep.rs`'s keep/delete-class and budget tests,
and `tests/slice0.rs`'s two `#[cfg(unix)]` e2e tests named above.

**Run high-volume test loops under an isolated, auto-cleaned `TMPDIR`.**
High-volume or repeated full-suite test runs — and any agent-driven fix
loop that runs `cargo test` many times — should run under an isolated,
freshly created, automatically cleaned temp directory (#779 requirement
8's own wording; the `trap` below is what makes an interrupted loop still
clean up). The block fails closed: a failed test run or any `pycc_*`
entry in the isolated root preserves the directory as evidence and ends
with a non-zero status instead of deleting it. The recipe is for Unix,
where `std::env::temp_dir()` resolves `TMPDIR`; on native Windows set
`TMP` and `TEMP` to the fresh directory instead and remove it afterward
the same way (derived from the standard library's documentation, not
locally observed):

```sh
iso="$(mktemp -d)"                       # never name it with a pycc_ prefix
trap 'rm -rf "$iso"' EXIT                # interrupt safety net (requirement 8: auto-cleaned)
ok=1
TMPDIR="$iso" cargo test --workspace || ok=0
[ "$(find "$iso" -maxdepth 1 -name 'pycc_*' | wc -l)" -eq 0 ] || ok=0    # pycc leak check: any hit fails
ls -A "$iso"                             # informational: toolchain leftovers are not pycc leaks
if [ "$ok" -eq 1 ]; then
  rm -rf "$iso"; trap - EXIT             # clean up now; don't rely on the trap in an interactive shell
else
  echo "test failure or pycc leak -- preserving $iso for inspection"
  trap - EXIT; false                     # keep the evidence and propagate a non-zero status
fi
```

This works because every pycc scratch root is created via
`pycc_scratch::ScratchDir` under `std::env::temp_dir()`, which on Unix
resolves `TMPDIR` (observed: the `#[cfg(unix)]` e2e tests in
`tests/slice0.rs` drive spawned `pycc` binaries under a redirected
`TMPDIR`) and on Windows resolves `TMP`/`TEMP` (derived from the standard
library's documentation, not locally observed). Never give the isolated
directory itself a `pycc_*` name: a directory whose name fully parses as
`pycc_{category}_{pid}_{nanos}_{seq}` inside a shared temp directory is,
by the D-201/D-209 contract, pycc property and eligible for sweeping.

**Pin `CARGO_TARGET_DIR` per worktree; keep `TMPDIR` ephemeral.** The two
variables answer different questions and must not be given the same
lifetime. `TMPDIR` holds the run's *test scratch*, and the fail-closed
`pycc_*` leak check above only works on a directory that starts empty --
so it is freshly created and removed per run, exactly as the recipe says.
`CARGO_TARGET_DIR` holds the *instrumented build*, which is pure cache:
it participates in no leak check, and `cargo llvm-cov` removes the
workspace's own artifacts and every `.profraw` at the start of each run
unless `--no-clean` is passed, so a reused directory cannot carry a prior
run's coverage into the next one. Point it at one stable directory per
worktree instead:

```sh
cov="$HOME/.cache/pycc-coverage/$(basename "$PWD")/target"   # stable: outside the worktree, outside any swept temp root
iso="$(mktemp -d)"; trap 'rm -rf "$iso"' EXIT                # ephemeral: as above
TMPDIR="$iso" CARGO_TARGET_DIR="$cov" cargo llvm-cov --workspace \
  --lcov --output-path "$iso/coverage.lcov" > "$iso/cov.log" 2>&1; echo $?
```

Measured on an M-series Mac: two consecutive runs against one pinned
directory reported identical figures (changed lines and workspace total
alike), and the directory stayed at 3.3 GB with its `.profraw` count flat
across runs rather than growing by a further 3.3 GB each time.

The old shape wrote the instrumented build under the ephemeral directory
too, and it leaked for a mechanical reason worth stating: `trap ... EXIT`
fires when *that shell* exits, and an agent-driven loop runs each command
in a separate shell, so the trap has always already fired by the time the
next command starts. Sessions therefore substituted a stable hand-named
directory per attempt -- and nothing removed those, because the documented
cleanup lived in a trap that no longer applied. One incident accumulated
roughly 36 GB across twelve abandoned coverage target directories and
about 18,000 `.profraw` files. A pinned directory removes the incentive:
re-running the gate overwrites it instead of minting another one.

Do not put the pinned directory under the working tree (Cargo artifacts
are not repository content), under a swept temp root (it would be deleted
and rebuilt, which is the cost this avoids), or share one across
worktrees (two concurrent `cargo` processes on one lock present as a
hang, not an error).

**What the automatic sweep cannot serve.** Two cases are permanently
outside the Part 4 sweep's reach, and this guidance is their remedy:

- *Entry-budget starvation*: a temp directory holding ≥ 10,000 persistent
  foreign entries can exhaust the sweep's 10,000-entry budget on every
  pass before it reaches any stale pycc root (a D-209 accepted edge
  case). The remedy is exactly the isolated-`TMPDIR` rule above, which
  also keeps new pycc roots out of the crowded directory in the first
  place.
- *Pre-Part-1 legacy roots*: names that do not fully parse as
  `pycc_{category}_{pid}_{nanos}_{seq}` (e.g. the retired
  `pycc_codegen_test_{label}_{pid}` format) are never deleted by the
  sweep — D-209 deliberately treats them as unattributable, so manual
  cleanup must not assume every `pycc_*` name is this repository's
  either. One-time manual cleanup: after confirming no pycc,
  `cargo test`, or CI process is running on the machine, list the
  candidates first
  (`find "${TMPDIR:-/tmp}" -maxdepth 1 -name 'pycc_*' -mtime +1`),
  review the listing, and delete only the entries matching this
  repository's known retired formats — `pycc_codegen_test_{label}_{pid}`
  directories, `pycc_obj_{pid}.o` files, and `pycc_run_{pid}`
  directories — leaving anything else (another tool's `pycc_*` name) in
  place. Never a blanket temp-directory wipe, and never an unreviewed
  `rm -rf` over the whole `pycc_*` glob.

**The anti-pattern, named.** Periodic global `/tmp` deletion is not the
fix and must not be relied on as one. Creation-site RAII (Parts 1–3) plus
the bounded sweep (Part 4) are the fix; this section is the operational
complement for the cases they cannot reach.

**The reusable verification protocol** (#779's closing "Verification"
section, restated as a repeatable procedure — under the isolated-`TMPDIR`
protocol, "no new pycc-owned temp roots remain" becomes "the isolated
root contains zero `pycc_*` entries after each run"; toolchain
intermediates under `TMPDIR` are recorded as informational, never
failed on):

```sh
# 1. Baseline: the set, count, and size of pycc-owned roots in the default
#    temp dir (keep the listing itself — the report records the set, not
#    only the count).
tmp="${TMPDIR:-/tmp}"
find "$tmp" -maxdepth 1 -name 'pycc_*'
find "$tmp" -maxdepth 1 -name 'pycc_*' | wc -l
find "$tmp" -maxdepth 1 -name 'pycc_*' -exec du -sk {} + 2>/dev/null \
  | awk '{s+=$1} END {print s+0 " KiB"}'
# 2-3. Three consecutive full-suite runs, each asserted free of pycc-owned
#      leftovers (toolchain intermediates are recorded, not failed on).
iso="$(mktemp -d)"
trap 'rm -rf "$iso"' EXIT                # interrupt safety net only
leaked=0
for i in 1 2 3; do
  TMPDIR="$iso" cargo test --workspace \
    || { echo "run $i: cargo test FAILED -- preserving $iso for inspection"; trap - EXIT; leaked=1; break; }
  if [ "$(find "$iso" -maxdepth 1 -name 'pycc_*' | wc -l)" -eq 0 ]; then
    echo "run $i: clean (non-pycc entries, informational: $(ls -A "$iso" | wc -l))"
  else
    # Keep the evidence: disarm cleanup and preserve the directory.
    echo "run $i: PYCC LEAK -- preserving $iso"; ls -la "$iso"
    trap - EXIT; leaked=1; break
  fi
done
[ "$leaked" -eq 0 ] && { rm -rf "$iso"; trap - EXIT; }
# 4. Re-run the repository-required gates (coverage, clippy, the scripts
#    unittest suite, and the governance checkers) — #779's Verification
#    step 4 is part of the protocol, not an extra.
# 5. Repeat the baseline commands: set, count, and size must be unchanged
#    (or lower).
```

A `pycc_*` entry left in the isolated root by any run is a live
regression of Parts 1–4: stop, preserve the evidence, and file it against
#779's area rather than working around it.

The #779-closing verification was executed on 2026-08-29 at the commit
delivering this section (macOS arm64): all three consecutive
`cargo test --workspace` runs under one fresh isolated `TMPDIR` exited 0
with 69/69 green `test result:` blocks each and zero entries of any kind
left in the isolated root after every run; the default per-user temp
directory's `pycc_*` set, count, and size (14,677 entries, 5,135,936
KiB — all but 19 of them pre-Parts 1–3 legacy-format leftovers outside
the sweep's authority, a live instance of both sweep-unreachable cases
above) were unchanged afterward. The full measured report lives in the
`docs/sessions/` entry for issue #785 and in the closing comment on
#779.

## Meta

Every bug that reaches `main` gets a permanent regression test named after the issue (`tests/regress/issue_1234.py`). Coverage gate: conformance suite must touch 100% of implemented grammar productions (grammar-coverage instrumentation in the parser).
