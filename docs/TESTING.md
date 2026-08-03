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

**PR-9 status (2026-07-30):** the `pycc_testkit` crate above remains unbuilt — D-102 extended the existing flat `tests/conformance.rs` integration test in place instead (11 fixtures at the time: the 2 pre-existing plus 9 new PEP fixtures), judging that PR-9's own needs (compile both profiles, run, diff against CPython) were still fully covered by that file's existing helper and didn't justify a new workspace crate. The "matrix file is updated by CI, not by hand" policy above has no automation behind it yet (verified: nothing currently writes `PYTHON_STANDARDS.md`'s status column); D-102's accepted interim policy is to flip a row by hand only once its fixture is observed green on a real, already-completed CI run across all 5 Tier-1 targets in both profiles — never speculatively. Building the real `pycc_testkit` crate and CI-owned status automation both remain deferred to whenever the v1.0-scale, multi-language-level harness this section describes is actually needed.

**PR-10 status (2026-07-31):** one more fixture added the same way (12 total now) — `pep_0585_builtin_generics_matches_cpython_3_14_6_byte_for_byte`, exercising `list[int]`'s literal/`.append()`/indexing/`len()`/iteration slice through the same `run_conformance_fixture_with_profile` helper D-102 established; no change to this section's harness shape. This branch's CI ([run 30608030517](https://github.com/rotnov/pycc/actions/runs/30608030517)) has since observed the new fixture passing on all 5 Tier-1 targets, in both profiles — per the same D-102 policy `PYTHON_STANDARDS.md`'s PEP 585 row is flipped to `✅` on that evidence (see `ROADMAP.md`'s v0.2 section and `DELIVERY_PLAN.md`'s PR-10 row).

The current frontend also keeps focused differential sources under
`tests/diagnostics/` when CPython's runtime behavior defines why strict pycc
must reject a program before code generation. In
`d0021_unbound_local.py`, CPython 3.14 raises `UnboundLocalError`, while
`pycc check` reports `T0021`; byte-exact human and version-1 JSON snapshots
lock the public diagnostic. This focused oracle case does not replace the
planned multi-version conformance harness.

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

The `conformance-fib-mandelbrot-tier1` and `check-throughput-1k-loc-50ms`
evidence bind the same reviewed `ci.yml` digest as `ci-tier1-cross-compile`,
so proving the digest is reviewed and current also proves these two steps
execute for real: the fib/mandelbrot-ascii byte-for-byte CPython differential
(`tests/conformance.rs`, D-085/D-080) runs via `cargo test -- --include-ignored`
in both `build-test-coverage` and every `native-build-test` matrix leg, i.e.
on all five Tier-1 targets, while the `pycc check` <50ms/1000 LOC
absolute-throughput-floor step (`scripts/check_frontend_throughput.rb`,
D-079/D-084) runs only inside `build-test-coverage` (one target) -- unlike
the tier1 claim above, this roadmap item's own wording does not assert the
floor holds on every target, only that it holds. The
`cli-spec-diagnostic-match` evidence instead binds
the `cli_spec_example` diagnostic-snapshot test (`tests/diagnostics_test.rs`,
D-083), which runs inside the same digest-pinned, 100%-coverage-gated
`cargo test`/`cargo llvm-cov` step `ci-build-test-coverage-100`'s evidence
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

The active `.github/workflows/ci.yml` is byte-identical to
`tests/fixtures/d112-ubuntu-frontend-perf-ci.yml` (D-112: `frontend-perf-measure`/
`frontend-perf-gate` moved from `macos-14` to `ubuntu-latest`, confirmed by
five real shadow-measurement runs before activation -- see `docs/DECISIONS.md`).
Its performance jobs keep D-062's comparator logic and five-replicate/`>2%`
contract unchanged, but their runner and LLVM install step differ from the
D-062 fixture, so job content is no longer byte-identical to it. The checker
allowlist currently accepts both active D-112 and D-100 (a deliberate D-103
coexist window pending a later, separate round that retires D-100), while
structural mutation tests exercise the active fixed-replicate and cache
boundaries plus D-100's, D-091's, D-099's, and D-056's retained audit
fixtures. The retired D-051, D-056, D-062, D-080, D-084, pre-D-100 D-091, and
pre-D-100 D-099 whole-file digests remain historical audit evidence, but the
public policy rejects them.
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
`REVIEWED_PERF_CI_WORKFLOW_SHA256S` now coexists `[D100, D112, D114]`;
`.github/workflows/ci.yml` matches D-114's shape. Issue #296 tracks
lowering the threshold back toward 2.0% once this one-time cost is
absorbed into every future baseline.

The byte-exact activation retired the D-048 workflow digest and fixture. No
administrative bootstrap is required because each run of the active workflow
(D-114, formerly D-112, formerly D-100) uses D-062's embedded contract to
measure both sides of its own comparison. D-054's one-shot
staging recovery is historical audit
evidence only; normal `audit` plus `ci-gate` protection was restored before this
activation branch was created and is not encoded in repository configuration.
A pull request that changes a bound manifest, local build script, lockfile,
toolchain, Cargo configuration, or benchmark source must first stage a
reviewed transition for that benchmark contract; the gate intentionally does
not guess whether such a change affects only product code or also the
measurement harness.

Regular CI runs the self-tests and repository checker after the hard coverage
step for fast feedback; placing a head-controlled script before that step would
violate the trusted setup sequence. The authority is the required read-only
`Workflow policy` job: it checks out the base revision, downloads the head
revision's workflows and `docs/ROADMAP.md` as non-executable data, then runs the
base revision's roadmap tests and checker against those inputs. A pull request
that replaces its own checker therefore cannot replace the implementation that
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
It reads the trusted base's complete
`tests/fixtures/policy-successor-manifest.json`, downloads every protected
policy executable and repository input plus the candidate's proposed next
manifest sources as non-executable `100644` data, and compares active targets
only with the base-staged sources and SHA-256 values. A candidate manifest
cannot authorize a target changed in that same pull request or remove any
already protected target. Under D-103, a legitimate policy-bundle update uses two
merges: first stage proposal files and the next complete manifest while active
targets remain unchanged; only a later pull request may activate those bytes,
after they are part of the trusted base. The later activation may reset each
source to its now-active target for the next cycle. This keeps future checker,
self-test, workflow-input, and fixture revisions under the same base-owned
transition boundary as initial activation instead of trusting whatever code a
pull request would make authoritative after merge. It then runs the base-owned
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

Activation copied the reviewed fixture byte-for-byte to
`workflow-policy.yml`, proved the required `audit` run, and retired the older
roadmap-only digest plus the one-use bridge. The active policy checker and its
self-test are the exact deterministic steady-state variants staged by the base;
their transition-only call and tests are absent. Every policy executable,
self-test, workflow input, and transitive repository fixture is bound by the
complete D-103 successor manifest. On every pull request, the base-owned audit
downloads those inputs as non-executable Git data, requires `100644 blob` modes,
rejects root or nested `.gitattributes`, and runs the isolated Python auditor
against the trusted base ledger. A candidate cannot replace a checker, change
one protected target while authorizing it from its own manifest, or shrink the
bundle in one merge. Future protected-bundle changes therefore retain the same
two-merge proposal-then-activation protocol.

The regular PR job runs this checker for fast feedback only; pull-request code
can change its own workflow. The authoritative `Workflow policy` workflow uses
`pull_request_target` on every pull request, checks out the trusted base commit,
downloads the head revision's workflows, search evidence, complete successor
manifest, and every protected target/source through the read-only GitHub API,
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
