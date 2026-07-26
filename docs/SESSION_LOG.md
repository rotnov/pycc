# Agent Session Log

A running handoff log for autonomous agent sessions working toward the
version 0.1 delivery goal (see `docs/DELIVERY_PLAN.md`
for the PR-1 through PR-7 breakdown this tracks against). Distinct from
`docs/AGENT_RETROSPECTIVE.md`: this file is "what state is the work in and
what's next," not "what went wrong." Newest entry first. Entries are
snapshots, not a byte-for-byte transcript — write enough for a fresh
session (human or agent) to resume without re-deriving context from git
history alone, not a full narrative.

---

## 2026-07-26 — PR #132 latest Codex findings repaired; final review loop pending

**Snapshot evidence:** the isolated task worktree is based on published PR
[#132](https://github.com/rotnov/pycc/pull/132) head
`d30e6a6c787de39e7e761d44d44cbf3e6cad3353`; exact default branch
`origin/main@78f5dcc0c3fd7c88fdc87e716e294fb0fc5cdb53` remains the task base already
integrated into that head. The remote advanced from `50e36e8` while this repair
was uncommitted; the work was stashed, the branch fast-forwarded, and the
remote's independent `None`-call-return plus comparison-test fixes were
preserved in the conflict resolution. It advanced again from `d9d31e2` to
`d30e6a6` with a Unix-only exit-101 repair and documentation of the `None`
parameter as an open gap; that runtime regression test remains, while D-075
supersedes the documented-gap approach and D-076 generalizes the exit mapping
to every unsuccessful child on every platform. The exact `@codex review` for
`50e36e8` produced two new actionable P2 threads: a valid `None`-typed parameter reached
`ty_to_basic_type`'s backend panic, and a generated-program abort was converted
to exit 1 instead of CLI_SPEC.md's portable runtime-failure code 101.

**Local repair:** D-075 gives `None`-typed user-function parameters a canonical
LLVM `i8 0` unit carrier while retaining LLVM `void` returns. Parameter name
reads, `return value`, `print(value)`, f-string interpolation, and passing a
`None`-returning call into a `None` parameter now compile and run end to end;
D-072's explicit nested-`print()` boundary and general `None` assignment gap
remain unchanged. D-076 maps every unsuccessful generated child of `pycc run`
to 101 without changing compiler-owned build or invocation failures. The type,
runtime, CLI, roadmap, historical implementation-plan scope note, and ADRs are
updated with the implementation.

**Local evidence:** focused regressions and the complete 123-test codegen,
57-test slice-0 suite active on the local Darwin host, and 30-test slice-1
suite pass. The exact hard command
`cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`
passes with 16,318/16,318 regions and 11,696/11,696 lines. Clippy, fresh Rust
API docs, and roadmap-evidence checks pass. The first independent deep-review
pass found one documentation-inventory blocker: the accepted D-075/D-076
sections were absent from DECISIONS.md's summary table and SPEC.md's ADR map.
Both indexes now include the decisions. The follow-up pass found two stale
direct-call-only `None` descriptions in the historical plan and code comments;
those descriptions now include D-075's parameter-carried paths. The next pass
found and corrected the same stale wording in the runtime API comment plus the
pre-integration slice-0 count in this snapshot. The final independent
deep-review verification is clean across all 11 checklist areas. Commit/push,
thread resolution, a new one-per-head `@codex review`, and remote CI remain
required before merge.

**Monitoring scope correction:** PR #119 and PR #125 are historical governance
evidence only, not live monitoring targets. Continue monitoring the current
PR #132 plus newly opened PRs and newly merged default-branch commits.

## 2026-07-26 — PR #132 final-head performance-gate repair validated locally

**Remote evidence:** [PR #132](https://github.com/rotnov/pycc/pull/132) head
`5a9741e1b6761c58eefb7a85e1f7906a4dbdea19` passed workflow policy, agent
policy/assets, 100% coverage, Pages, all five native/cross target legs, and the
replicated measurement job. The predecessor-owned isolated gate then correctly
blocked the changed-input classification: predecessor replicate medians
`6201.39, 5973.44, 6078.99, 6044.88, 6399.96 ns` (aggregate `6078.99 ns`)
versus candidate `6179.26, 6119.92, 6209.22, 6263.44, 6250.65 ns` (aggregate
`6209.22 ns`), or `+2.1423%` against the unchanged hard `2.00%` threshold.
The run was not retried or waived. All fourteen prior review threads remain
resolved. The exact `@codex review` request for that head completed with two
new actionable findings: exceptional `float` powers could silently return
infinity/NaN, and the roadmap omitted reachable arithmetic failure boundaries.

**Local repair:** the measured parse/lower/check path had no PR-local parser,
HIR, types, or benchmark-fixture change, but the complete `src/`/`crates/`
classifier deliberately treated the backend diff as changed executable input.
Rather than weakening or re-running the gate, `pycc_types::check` now builds the
concrete function environment directly instead of creating and then cloning a
temporary signature table. Call validation retains the existing behavior of
inferring every argument before arity/type diagnostics while storing up to four
types inline and allocating only for wider calls. Focused regressions cover the
wide-call fallback, its error path, and diagnostic-order preservation.

The review repair now rejects zero-to-negative, negative-base/fractional, and
finite-overflow `float` powers explicitly until Python exceptions and complex
results exist. Floor-dividing the minimum tagged integer by `-1` promotes the
out-of-range quotient to bigint instead of aborting. Runtime and slice-level
regressions cover the successful promotion plus each reachable `float`-power
failure class and the supported non-finite real-result domains; `RUNTIME.md`
and the commit-relative roadmap enumerate the
remaining bigint-to-float, bigint-operand arithmetic, and negative-`int`-power
boundaries.

**Local evidence:** the same-tree Criterion baseline comparison moved the point
estimate from `6.0009 µs` to `5.7464 µs`; Criterion estimated `-3.2494%` with
`p = 0.00` and reported an improvement. The exact hard command
`cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`
passes with 16,038/16,038 regions and 11,501/11,501 lines. The coverage run
includes 159 `pycc_types` tests, 62 `pycc_rt` tests, and 28 slice-level tests.
Clippy with `-D warnings`, fresh workspace Rust documentation, formatting,
roadmap-evidence validation (99 runs / 432 assertions plus the production
checker), and diff checks are green after the final review repair.

**Review evidence at handoff:** independent iEvo reviews found that the initial
finite-domain guards over-rejected non-finite float-power operands and that the
diagnostic-order regression exercised an internal helper but not the changed
public `check` path. Both repairs are staged with focused assertions. The exact
final staged diff is reviewed again after this entry; the pull request and
commit history remain the authoritative outcome.

**Concurrent-head evidence:** while the reviewed repair was staged, the remote
PR head advanced through `f0226542e7601b3a82883ae82c74e45ed5fa3549`. The
reviewed local repair was first preserved as
`fcd865613c46b70bb7cbaf4b72b11929e897d5fb`, then the remote head was merged.
The resolution retains its exact-fibonacci oracle, Linux/libm end-to-end test,
and refreshed architecture/CLI text while preserving the independently
reviewed finite-only float-power guards and the now-implemented floor-division
promotion in the roadmap.

**Required next steps:** commit and push the reviewed repair, then request exact
`@codex review` once for that commit. Re-run every required gate and merge only
if the new fixed five-replicate performance result, coverage, aggregate
`ci-gate`, and review state are all green.

## 2026-07-26 — PR #132 concurrent merge and all live review fixes validated locally

**Snapshot evidence:** local task branch `codex/fix-pr132-review-0764` is at
`c461edac12d0f4fc1e1fd3c464f22dc892ef6555`, which already combines review-fix
patch `0f19f225f81ebca5166708cec74b010d2d47336e` with exact default branch
`origin/main@78f5dcc0c3fd7c88fdc87e716e294fb0fc5cdb53`. A staged merge with
`c63de02be35321b4a8b66821fb5cd04774056558` is in progress. A final fetch left
the remote default branch unchanged and showed published
[PR #132](https://github.com/rotnov/pycc/pull/132) at
`5ff10f1ecd619bde410dfbf2ad3997f0d382cfeb`, a merge whose only parents are that
same `c63de02` and `origin/main@78f5dcc`; it contains no unique non-merge commit.
GitHub reports the PR open, non-draft, and blocked on conversations. All required
checks on `5ff10f1` are green. Fourteen review threads are unresolved, eight of
them non-outdated; all eight describe behavior covered by the staged local tree.

**Validated local merge:** functions see completed module bindings; globals
and maybe-bound non-parameter locals carry runtime initialization flags;
parameters remain initialized and reassignable; local allocations dominate
their uses; accepted `bool`→`int` boundaries use the tagged representation; a
`for` uses hidden SSA induction state so empty ranges, post-loop targets,
negative steps, and body reassignment match Python; two-return merges are
terminated; and `None` in an f-string renders as `None` while malformed
`None`-typed non-call interpolation fails explicitly. The newest numeric fixes
promote an out-of-range product of two smallints, implement CPython's adjusted
float divmod algorithm (including signed zero and the `1.0 // 0.1 == 9.0`
case), and route true division through a zero-divisor guard. Multiplication with
an already-promoted bigint operand remains the documented boundary.

**Local evidence:** the exact hard command
`cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`
passed with 15,844/15,844 regions and 11,391/11,391 lines. Workspace tests,
Clippy with `-D warnings`, fresh `cargo doc`, site checks and mutation
self-tests, 220 Python policy tests, Ruby CI-permission and roadmap-evidence
suites, agent policy/assets validation, Codex/Claude alpha-skill evals, both
marketplace checks, and `git diff --check` passed. A final independent pinned
iEvo review found one non-blocking conflict-resolution artifact: imported test
names and comments still described allocation helpers removed by the merged
implementation. Those descriptions now cover the actual module-global and
preclassified function-local storage paths, the focused 119-test codegen suite
passes, and the required follow-up deep review is clean with no findings. The
known iEvo stale-catalog defect remains deduplicated in
upstream [`ievo-ai/skills#459`](https://github.com/ievo-ai/skills/issues/459);
no new confirmed iEvo defect was found.

**Required next steps:** commit the independently reviewed staged `c63de02`
merge, then record `5ff10f1` as an additional merge parent without
replacing the independently reviewed resolution (the remote merge has no
unique non-merge input). Push normally to `feat/v0-1-pr5-codegen-depth`, resolve
only threads verified against the resulting remote head, and request the
user-required exact `@codex review` once for that new head. Merge only after the
new head's required CI is green and no actionable thread remains. Monitor
current open PRs, new merges, current checks, and current review threads; PR
#119/#125 references are historical governance records, not live monitoring
targets.

## 2026-07-26 — PR #132 blocked on `frontend-perf-gate`; likely order/thermal drift, not a real regression; escalated to the user

**Snapshot evidence:** head `1ae1b3c` (fifth merge round). CI run
[30205958740](https://github.com/rotnov/pycc/actions/runs/30205958740):
every job passed except `frontend-perf-gate`, which failed with
`FAIL: pycc check replicated frontend median regressed 6.1931% (threshold: 2.00%)` —
previous replicate medians `6686.28, 6777.54, 7088.44, 7228.73, 7185.40 ns`,
current replicate medians `8498.72, 7405.25, 7527.44, 8023.83, 7353.21 ns`.
(A first attempt on this same head also failed
`frontend-perf-measure` on a plain DNS lookup failure fetching the Rust
toolchain — pure infra, correctly retried via `gh run rerun --failed`,
unrelated to the finding below.)

**Why this looks like the gate's own known order/thermal-drift gap, not
a PR-5 regression:**
1. `git diff 45545bb...HEAD -- crates/pycc_parser crates/pycc_hir crates/pycc_types benches/ Cargo.toml Cargo.lock rust-toolchain.toml`
   is empty, and neither `pycc_hir` nor `pycc_types`'s `Cargo.toml`
   depends on `pycc_mir`/`pycc_codegen`/`pycc_rt`. The exact code path
   this benchmark measures (`pycc_parser::parse` → `pycc_hir::lower_checked`
   → `pycc_types::check` over a fixed fib/print fixture, all in-process,
   no CLI subprocess spawn) is byte-for-byte identical to the predecessor.
   A real algorithmic regression in the measured path is not possible.
2. The two five-value sets show complete separation: every one of the 5
   current replicates (min `7353.21`) is slower than every one of the 5
   previous replicates (max `7228.73`). Under random per-round noise that
   has roughly a 1-in-252 (~0.4%) chance; a full separation like this
   points to a systematic effect, not noise scattered around a stable
   mean. The measurement order is fixed (all 5 predecessor rounds run
   first, then all 5 candidate rounds) — D-062's own text still runs
   sequentially, and D-056's own text already names "order and thermal
   drift inside one hosted runner" as a gap neither D-051 nor D-062
   removes (interleaving was explicitly rejected on trust-boundary
   grounds: candidate code must not run before the predecessor upload is
   sealed).

**Why not just retry:** D-051/D-056/D-062 all explicitly reject
"rerun until one pair passes" as selection bias, and this session already
burned five merge-conflict rounds while `main` kept advancing during
each CI wait — a retry is also a bet that the same host-level order
effect doesn't recur, not a fix for anything PR-5 controls. Widening the
gate's classifier or changing its measurement order is the concurrent
actor's byte-exact-reviewed CI-workflow domain, not something this PR
can or should patch as a side effect of shipping compiler work.

**Escalated to the user** with the two facts above, and three options:
one documented probe re-run, a D-054-style audited exception for this
one merge, or pausing/adjusting the gate's classifier so this stops
recurring. Full CI check list at
[the failed run](https://github.com/rotnov/pycc/actions/runs/30205958740).
No action taken on the gate itself pending that decision; the fifth
merge round's local verification (tests/clippy/doc/100% coverage/evals)
already passed before this push.

## 2026-07-26 — PR #51 performance repair integrated with current main

**Snapshot evidence:** the containing merge integrates local performance-repair
parent `a7f048d` with refreshed `main@128285fbfbcfaa29b1a6c8fa81da4d84bae8d67f`.
[PR #51](https://github.com/rotnov/pycc/pull/51) remained open and non-draft at
remote head `c1e855590a23307bcd8472979ff37f8bbfd0f8d9` before this local integration
was pushed. That remote head ran required CI as run `30206099702` from
active-D-062 `main@45545bb057f5cd9e8712610c6137f53ef56d3aae`.
Immediately before preparing the follow-up commit, a fetch confirmed
`origin/main` still at `128285fbfbcfaa29b1a6c8fa81da4d84bae8d67f`; GitHub
still reported the old remote head as open, non-draft, and dirty, with one
unresolved P1 review thread.

**Gate result:** trusted audit, agent checks, 100% coverage, Linux/macOS,
cross-compile, and the 5+5 measurement job passed. The isolated comparator
correctly blocked the changed-source candidate at `+10.7215%`: predecessor
aggregate median `7964.08 ns`, candidate `8817.95 ns`. This was not retried or
waived. The benchmark does not execute the changed root CLI sources, but it
exposed an existing redundant type-checker walk that could be removed without
changing the gate.

**Repair:** `pycc_types::check` now constructs already-concrete
function signatures directly and reserves constraint collection for modules
that contain real `Ty::Infer` signatures; a failed concrete validation falls
back to the historical solver-first order so diagnostic selection is stable.
The workspace coverage gate passes at 100% lines and regions, including
explicit fast-path, diagnostic-parity, solver-path, and collector edge cases;
workspace clippy, Rust documentation, roadmap evidence, and agent-asset checks
also pass. An initial local Criterion comparison improved from about `7.15 µs`
to `5.85 µs` (`−18.0%`); a later run after the diagnostic-order fallback
measured `6.99 µs` (about `−2.3%` from the same original observation). This
single-host evidence is noisy and is not selected as the gate result; the next
fixed 5+5 CI comparison remains authoritative.

**Pre-merge review repair:** the unresolved thread correctly found that valid
but unsupported Python could still panic during HIR lowering, aborting the
pre-commit batch with exit 101. The follow-up converts every user-reachable HIR
capability rejection to a spanned `C0001` diagnostic, keeps only an internal
parser-invariant assertion, and proves both exact CLI rendering and continued
multi-file checking after an unsupported construct. The workspace coverage
gate passes at 100% lines and regions; clippy, Rust documentation, roadmap
evidence, and agent-asset checks pass as well.

**Local review:** the exact pinned staged-diff reviewer found no implementation,
contract, security, test, or documentation defect in the repair; its only
finding was that the previous handoff text still listed that now-completed
review as pending. This paragraph replaces that stale instruction.
The subsequent full-range review found that adding a direct `ruff_text_size`
dependency would violate D-062's byte-identical manifest/lock precondition and
block CI before measurement. The fix keeps `Cargo.toml` and `Cargo.lock`
identical to the predecessor and exposes byte ranges through the existing
`pycc_ast` facade instead; an exhaustive facade test covers every upstream
statement and expression variant at 100% line and region coverage.

**Where to resume:** commit and push the verified repair, repeat exact-revision
`pre-commit try-repo`, and resolve the P1 thread only after the remote head
contains the verified fix. Treat the new CI run as new candidate evidence, not
a rerun of the failed head, and merge only if every required check is green
with no unresolved actionable review thread.

## 2026-07-26 — PR #51 pre-commit hook awaiting final CI and merge

**Snapshot evidence:** the checked-out `codex/pre-commit-hook` branch was at
commit `171eceb` with a clean tree before integrating refreshed
`origin/main@841048ec37e20d85a5a0406778f9ec8b66224b04`. The integration was in
progress with its documentation conflicts resolved but not yet committed at
this snapshot. [PR #51](https://github.com/rotnov/pycc/pull/51) is open and no
longer a draft.

**Overall status:** the pull request publishes `pycc-check` from the main
repository as a serial, read-only `language: rust` pre-commit hook; extends
`pycc check` to aggregate diagnostics across native input paths and supported
source encodings; and replaces required asynchronous GitHub review comments
with the immutable pinned local-review loop. D-067 and D-068 record the two
project-wide choices after confirming PR #132's reconciled D-057…D-061 and
D-070…D-073 allocations.

**Validation already observed:** the Rust workspace tests, clippy, generated
API documentation, agent-policy and marketplace checks, roadmap checks, and
100% line/region coverage passed before the latest default-branch integration.
An isolated `pre-commit try-repo` install selected exact revision `10a0502`
and passed `pycc check`; the final merged revision still needs the same check,
the pinned full-range local review, required pull-request CI, and normal
protected-branch merge.

**Where to resume:** finish and review the `841048e` merge, rerun affected
checks, push `codex/pre-commit-hook`, wait for every required PR #51 check and
conversation to clear, then merge normally and verify the post-merge
`main-history-audit`. Do not request `@codex review`; D-068 makes that external
service optional rather than a required gate.

## 2026-07-26 — PR #138 merged; D-062 blocks PR #132

**Delivered state:** [PR #137](https://github.com/rotnov/pycc/pull/137)
merged as `45545bb057f5cd9e8712610c6137f53ef56d3aae`. Post-merge CI run
`30205599108` passed the hard 100% line/region gate, all Tier-1 legs, the
cross-target proof, both frontend-performance jobs, and aggregate `ci-gate`;
the exact merge also passed agent-assets, agent-policy, Pages, and
main-history-audit. [PR #138](https://github.com/rotnov/pycc/pull/138) then
merged the PR-state-first monitoring rule as
`fb5d483daa9f9fd18914a0ceeee1b8448edd1421`. Post-merge run `30206232849`
and its agent and main-history workflows all completed successfully by the
2026-07-26T14:40:20Z inspection checkpoint, including 100% coverage, both
performance jobs, every Tier-1 leg, and aggregate `ci-gate`.

**D-062 evidence:** run `30205599108` retained exact predecessor artifact
`8632975406` and candidate artifact `8632990263`. Their five per-run medians
aggregate to `6924.73 ns -> 7077.93 ns` (`+2.2123%`). The trusted classifier
proved the executable inputs identical, so D-056's retained rule correctly
treated that delta as non-blocking environment telemetry; evaluating the same
evidence as changed inputs would fail the unchanged hard `>2%` gate. This
verifies D-062's delivered identical-input path, but does not close
[#109](https://github.com/rotnov/pycc/issues/109): repeated changed-source PR
and post-merge evidence is still required without result selection.

**Current work at the same checkpoint:** [PR #132](https://github.com/rotnov/pycc/pull/132)
was open at `1ae1b3c90749836aeaa340ad0d8a067dc605d464` while current `main` was
`fb5d483daa9f9fd18914a0ceeee1b8448edd1421`; GitHub reported it conflicting
and `DIRTY`. Its first performance attempt failed before collecting timing
because rustup hit a DNS lookup error. The permitted no-result rerun completed
all fixed samples, but D-062 then blocked the changed-input comparison at
`6686.28, 6777.54, 7088.44, 7228.73, 7185.40 ns` versus
`8498.72, 7405.25, 7527.44, 8023.83, 7353.21 ns`: aggregate medians
`7088.44 ns -> 7527.44 ns`, or `+6.1931%`. Exact artifacts `8633194749`
and `8633213046` retain that evidence. Coverage, audit, cross-target, and every
native platform check passed, but `frontend-perf-gate` and `ci-gate` failed;
do not rerun or select another timing result for this head. Nine current Codex
threads and one outdated thread also remain unresolved. Refresh the branch from
current `main`, verify and fix every confirmed thread with regression coverage,
investigate the retained performance evidence without result selection, resolve
only fixed or proven-obsolete threads, then obtain green required checks and a
Codex review for the final exact head before considering merge.

## 2026-07-26 — Third D-062 collision resolved; PR #132 re-pushed, awaiting CI

**Snapshot evidence:** direct work on `feat/v0-1-pr5-codegen-depth`,
merging `origin/main` at `841048e` (PR #128, which added this file and
`docs/AGENT_RETROSPECTIVE.md` under D-066) into this branch and pushing
the result as commit `1b68e21` (superseded by a second merge commit
resolving the immediately-following conflict described below). Local
`cargo test --workspace`, `cargo clippy --workspace --all-targets`,
`cargo doc --workspace --no-deps`, and
`cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`
all passed (100.00% lines and regions across every crate) before pushing.

**What changed since the entry below:** the prior entry's "Known
follow-up required before PR-5 merges" predicted a colliding tail between
this branch's D-062 (str-leak correction) and `main`'s new D-062
(fixed-replicate perf-gate stabilization). Resolved by keeping D-057–061
as `main` had already reserved them, ceding D-062 (and `main`'s
subsequently added D-066, this file's own decision) to `main`'s
decisions, and renumbering this branch's remaining four entries — str-leak
correction, the renumbering-record itself, the `print()`-nested-expression
boundary, and the `RelocMode::PIC` fix — to D-070 through D-073, a gap
ahead of `main`'s reach chosen so future `main` advances stop colliding
with this branch's own IDs before it merges. The renumbering-record entry
(now D-071) was also frozen to a single dense table row instead of a full
section, since three collisions made it the highest-churn entry in the
file for no technical content. A second, smaller conflict round
immediately followed (`main` advanced again mid-resolution, touching the
same `ROADMAP.md`/`SPEC.md` table rows this branch had just edited); it
required no further ID changes, only combining both sides' additive text.

**Known follow-up required before PR-5 merges:** re-check
`gh pr view 132 --json mergeable,mergeStateStatus` and `gh pr checks 132`
after this push lands, since `main` has advanced during every prior
verification window on this branch. Re-verify the live ADR tail
immediately before picking any new ID; later IDs are candidates, not
reservations, and this has now happened four times.

## 2026-07-26 — PR #138 opened for PR-state-first CI monitoring

**Snapshot evidence:** ready-for-review
[PR #138](https://github.com/rotnov/pycc/pull/138) was opened from
`codex/check-pr-state-before-ci` at head
`61163e35f67af30a5b3dc24b988abc9f3c1eb9a3`, based on
`main@45545bb057f5cd9e8712610c6137f53ef56d3aae`. A live state query reported
the PR `OPEN`, non-draft, and `MERGEABLE`; `mergeStateStatus=BLOCKED`
reflected required checks still in progress rather than a merge conflict.
The containing change is not yet merged.

**Delivered scope if merged:** `.ievo/evolution/project.md` will require
agents to inspect pull-request lifecycle and mergeability before waiting for
CI, and `docs/AGENT_RETROSPECTIVE.md` records the PR #132 incident that
motivated the rule. No compiler behavior, supported platform, roadmap
acceptance evidence, or delivery sequencing changes.

**Required next steps:** push the session-log snapshot as the final PR head,
rerun focused local validation, confirm the PR is still open and mergeable,
request Codex review through the retry guard, and monitor required CI plus
all review surfaces. Merge only after required checks are green and no
actionable review thread remains; then verify the post-merge `main` run and
history audit.
## 2026-07-26 — D-062 activation PR #137 green; refresh onto current main in progress

**Snapshot evidence:** draft [PR #137](https://github.com/rotnov/pycc/pull/137)
at head `b6f5a29d4c56d65d88d82120595bbc04343c6f25` was based on
`e433b849ef1083c0af7aa6da6c022a6e0661dc9f`. Its first complete CI run
`30204610811` and every required check passed, Codex reported no major
issues for that exact head, and the PR had no review threads. While the
run completed, PR #128 advanced `main` to
`841048ec37e20d85a5a0406778f9ec8b66224b04`; the activation worktree is
therefore integrating that exact default-branch commit before review and
merge. This containing snapshot includes the resolved documentation from
that integration but does not itself claim that PR #137 has merged.

**Performance-gate state:** PR #137 activates the staged D-062 workflow
byte-for-byte and retires D-056's live workflow digest while preserving
its identical-input telemetry rule. The first unselected 5+5 PR artifacts
are `frontend-perf-previous` ID `8632698165` and
`frontend-perf-current` ID `8632713088`, retained for 90 days. Their five
per-run medians aggregate to `6869.56 ns -> 6967.66 ns` (`+1.4281%`). The
exact base/head diff contains no `src/` or `crates/` change, so the trusted
classifier reports identical executable inputs; the comparator also falls
within 2% if evaluated as changed. This validates byte-exact execution and
fixed evidence handling, but does not exercise the blocking changed-input
path.

**Required next steps:** finish the non-force merge of current `main`,
rerun focused local policy/site checks, push the new head, run the review
retry checker before requesting Codex review for that SHA, and merge only
after the repeated required checks are green with no unresolved threads.
Then verify the post-merge `main` CI and history audit. Keep issue #109
open until repeated changed-source PR and post-merge runs validate D-062's
blocking aggregate without result selection. PR #132 is a likely future
changed-source observation only after it rebases; its unrelated draft
D-062 through D-065 ADR range currently collides with `main`, and the
renumbering finding is recorded on that PR. **Update from the entry
above:** by the time PR #137 merged (as `45545bb`), PR #132 had already
resolved that collision (D-057–061 kept, D-062 ceded to `main`, remaining
four entries moved to D-070–073) — this entry's last sentence is
superseded, kept verbatim below as the historical record it actually was
at the time it was written.

## 2026-07-26 — PR-5 integration and PR loop pending; PR-6/PR-7 not started

**Snapshot evidence:** read-only inspection of
`feat/v0-1-pr5-codegen-depth` at commit `c70ac56`; its worktree was clean.
The branch is not merged and has no open pull request as of this snapshot.

**Overall status:** PR-1 through PR-4 are merged to `main`. The default-
branch snapshot is `619d232` (the merge of PR #130) and includes the later
infrastructure, governance, performance-gate, and agent-tooling changes.
PR-5 remains in progress on branch `feat/v0-1-pr5-codegen-depth`,
following that branch's complete 11-task version of
`docs/superpowers/plans/2026-07-25-pr5-codegen-depth.md`; the version in
the containing `main` snapshot has only Tasks 1–2.

**PR-5 task status:** all 11 planned tasks have implementation commits.
The observed head follows Task 11's end-to-end fixture/documentation sweep
and its review fixes with a commit that adds the top-level-return
terminator guard and clears the recorded deferred minors. Current-`main`
integration, ADR renumbering, full current-base validation, PR creation,
and the PR review loop have not yet completed.

**Known follow-up required before PR-5 merges:** integrate the latest
`main` without overwriting any newer local work. Published PR #132 now
carries D-057 through D-065, while current `main` owns a conflicting
D-062 and this journal uses D-066. PR #132 must reconcile that colliding
tail before merge. Re-check the live ADR tail immediately before editing;
later IDs are candidates, not reservations. Full detail is in
`docs/AGENT_RETROSPECTIVE.md`.

**After PR-5 merges:** PR-6 (conformance and acceptance benchmarking —
`pycc_testkit`, `fib`/`mandelbrot-ascii` vs. pinned CPython on all 5
Tier-1 targets, the `pycc check` <50ms/1k-LOC benchmark, and exact
diagnostic-output acceptance) and PR-7 (buffer to close whatever's left
against the v0.1 acceptance checklist) have not been started. The paired
frontend regression gate is already active and required through
`ci-gate` under D-056 in the containing commit; D-051/D-053 remain the
retained paired-provenance controls, not the current workflow/comparator
authorization. The gate is not deferred PR-6 work. PR-6 is the first point
the full pipeline runs end-to-end on all five Tier-1 platforms — treat it
as the highest-uncertainty remaining slice, not a formality.

**PR-5 recovery boundary:** the local-only state above is historical, not
the current recovery path. A later read-only check found the snapshot commit
in the ancestry of published branch
`origin/feat/v0-1-pr5-codegen-depth`, with observed remote head
`453e7dd9b23effe0390770d8ad7c264c33150bdd` and open
[PR #132](https://github.com/rotnov/pycc/pull/132) based on
`main@6ec86a8e89c7775f9f41a9aa9b12a1a2660952de`. A clean clone can recover
the work without any machine-local path:

```sh
git fetch --prune origin main feat/v0-1-pr5-codegen-depth
git rev-parse origin/main origin/feat/v0-1-pr5-codegen-depth
git merge-base --is-ancestor \
  c70ac5696ff908770350a587ed87210cd6edd80b \
  origin/feat/v0-1-pr5-codegen-depth
git log --oneline --decorate \
  origin/main..origin/feat/v0-1-pr5-codegen-depth
```

The exact historical snapshot remains
`c70ac5696ff908770350a587ed87210cd6edd80b`. If the published head differs
from the observed head above, treat the remote and PR as newer state: inspect
them before acting and never reset, force-push, or overwrite an existing
owner's local worktree to recreate this older snapshot.

**Where to look to resume:**
- Read [PR #132](https://github.com/rotnov/pycc/pull/132) and compare its
  current remote head with the observed head above. If an existing PR-5
  worktree is present, run `git status --short --branch` there before any
  mutation; never use this snapshot to overwrite newer local work.
- `docs/DELIVERY_PLAN.md` — PR breakdown and autonomy policy.
- `docs/ROADMAP.md` — current delivery status and the v0.1 acceptance
  checklist (source of truth for what's actually done vs. claimed).
- `git show origin/feat/v0-1-pr5-codegen-depth:docs/superpowers/plans/2026-07-25-pr5-codegen-depth.md`
  — the branch's complete active plan, task-by-task, if PR-5 is not merged
  yet; do not mistake the shorter `main` copy for the whole plan.
- `git log --oneline origin/main..origin/feat/v0-1-pr5-codegen-depth`
  for the published commit-by-commit state.
