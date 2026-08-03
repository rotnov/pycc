# 2026-07-30 — D-100 composes D-099 (merged to `main`) with PR-8's own D-091

**Authoritative checkpoint:** refreshed default `main` is
[`3bd05f3`](https://github.com/rotnov/pycc/commit/3bd05f3), which merged both
D-099 staging ([PR #227](https://github.com/rotnov/pycc/pull/227)) and D-099
activation ([PR #228](https://github.com/rotnov/pycc/pull/228)) — an
independent, unrelated concurrent change (Windows vcpkg binary cache for
D-027's libxml2 build, closing issue #225) that landed while this v0.2 PR-8
branch (`feat/v0-2-pr8-release-profile-pycctoml-nbody`) was still open. D-099's
own activation retired PR-8's D-091 digest to audit-only status in
`main`'s `scripts/check_roadmap_evidence.rb`, which made `workflow-policy.yml`'s
base-owned audit job fail on every PR-8 push regardless of anything PR-8 itself
changed (that job runs `main`'s copy of the checker against PR-8's `ci.yml` as
data).

**What this session did:** merged `main` into the PR-8 branch and recorded
D-100, composing D-091's changes (release-mode `pycc_rt` build step,
relaxed `frontend-perf-measure` manifest classification) with D-099's Windows
vcpkg cache into one new reviewed digest
(`tests/fixtures/d100-compose-d91-d99-ci.yml`,
`D100_COMPOSE_D91_D99_CI_WORKFLOW_SHA256`) — the two touch disjoint regions of
`ci.yml` and composed with a clean `git merge` (no conflicts inside `ci.yml`
itself; only surrounding docs/scripts needed manual resolution). Also
corrected a stale `docs/ROADMAP.md`/`docs/TESTING.md` claim that issue #109
"stays open" — it closed 2026-07-26 on repeated changed-source PR/main
evidence from merged PRs #51 and #132.

**Also resolved during this reconciliation:** a real, locally-reproduced
`frontend-perf-gate` investigation that initially suspected D-090's `toml`/
`serde` dependency addition was regressing `pycc check`'s own startup speed
(~5-8% measured locally, spawning the actual CLI binary) — but the gate's own
`benches/check_bench.rs` never spawns that binary at all; it calls
`pycc_parser`/`pycc_hir`/`pycc_types` in-process on a fixed fixture, none of
which PR-8 touched. The CI-reported 4.4961% delta on that specific gate is
most likely measurement noise (unconfirmed either way — the investigation
was superseded by the D-099/D-100 reconciliation before a fresh, genuinely
independent remeasurement was obtained). A quick mitigation attempt (dropping
`toml`'s `display` feature, hand-formatting `pycc init`'s scaffolded
`pycc.toml` instead of using `toml::to_string`) was tried and reverted: it
saved only ~0.05% of binary size since both `toml`'s `parse` and `display`
features depend on the same underlying `toml_edit` parser, so it did not
address the (unrelated, since-reframed) regression theory at all.

**Resolved once CI ran on the D-100 merge commit** (`34759559`, the genuinely
fresh predecessor/candidate pair D-100's merge created): `frontend-perf-gate`
passed cleanly, confirming the earlier 4.4961% reading was measurement noise
as suspected, not a real regression from anything PR-8 changed. The
`ubuntu-24.04-arm` nbody leg also passed. Every required job on that run
(`build-test-coverage`, all five `native-build-test` legs, both
`cross-compile-*` jobs, `frontend-perf-measure`/`frontend-perf-gate`,
`ci-gate`) succeeded on the first attempt.

**A second, distinct process mistake surfaced next:** D-100's own digest was
registered only on the PR-8 branch itself, not on `main` — but
`workflow-policy.yml`'s base-owned `audit` job runs *`main`'s own copy* of
`scripts/check_roadmap_evidence.rb` against the PR head's `ci.yml`, so it
correctly failed with "does not match a reviewed active-or-staged performance
CI workflow" regardless of what PR-8's own branch contained. This is exactly
the stage-then-activate two-phase pattern D-090/D-091/D-099 each already
followed, which D-100's own initial Alternatives section wrongly argued could
be skipped since everything happened inside PR-8's own branch. Fixed by
opening a separate stage PR, [#231](https://github.com/rotnov/pycc/pull/231)
(`chore/stage-d100-compose-d91-d99`), which registered
`D100_COMPOSE_D91_D99_CI_WORKFLOW_SHA256` alongside active D-099 on `main`
without touching live `ci.yml` — merged clean (all Tier-1 legs, coverage,
cross-compile, perf gate, and the base-owned `audit` itself all passed).
D-100's own decision text carries an appended Update note recording this
correction, per this file's own edit-don't-rewrite policy.

**Note for the next session:** `gh run rerun` on the previously-failed
`Workflow policy` run did *not* pick up `main`'s newly-merged state — GitHub
reuses the original run's already-resolved base-ref checkout rather than
re-resolving it fresh. A genuinely fresh `pull_request_target` evaluation
needs an actual new `synchronize` event (a real push to the PR-8 branch), not
a rerun. This session pushed this same session-log update to trigger that;
check whether `Workflow policy` passed on that push before assuming PR-8 is
unblocked.
