# 2026-07-30 — v0.2 PR-8 merged (D-101 lowers the `ubuntu-24.04-arm` nbody floor to 18x)

**Authoritative checkpoint:** `main`'s tip is
[`23a106c`](https://github.com/rotnov/pycc/commit/23a106c) — [PR #188](https://github.com/rotnov/pycc/pull/188)
(v0.2 PR-8), squash-merged, carrying every commit from `a48d243` (the
second merge of `origin/main` needed after stage PR #231 itself introduced
a new, differently-worded D-100 entry to `main`'s own
`docs/DECISIONS.md`/`docs/ROADMAP.md`/`docs/SPEC.md`/`docs/TESTING.md`/
`scripts/check_roadmap_evidence.rb`/`scripts/test_check_roadmap_evidence.rb`,
resolved by keeping PR-8 branch's own fuller D-100 text throughout) through
D-101's own final commit. The final pre-merge CI run on PR-8's head was
fully green (all five `native-build-test` legs including
`ubuntu-24.04-arm`, `build-test-coverage`, both `cross-compile-*` jobs,
`frontend-perf-measure`/`frontend-perf-gate`, `ci-gate`) — see the "Merged."
paragraph below for the exact run and merge commit.

**`ubuntu-24.04-arm`'s nbody-gate history took two passes to read correctly.**
The first pass (5 observations: 3 failures clustered at
19.92x/19.88x/19.86x, 2 passes at unrecorded ratios) concluded the tight fail
band was a censored-left-tail artifact — the assertion only ever prints a
ratio on failure — not a measured ceiling, since a ~40% pass rate alongside
that tight a cluster looked inconsistent with a genuine sub-20x plateau.
Committed as `5923c61` (unconditional `$GITHUB_STEP_SUMMARY` ratio reporting
plus a `docs/ROADMAP.md` follow-up item, no floor change) and pushed. The
very next fresh CI run on that commit produced a 6th observation: a 4th
failure at 19.90x, landing inside the *same* 19.86x-19.92x band rather than
scattering — exactly the additional evidence the first pass said would be
needed before a floor decision was defensible, and it flipped the
conclusion. **D-101** now lowers `ubuntu-24.04-arm`'s floor to 18x (real
margin below the worst observed 19.86x, well above D-096's 15x since this
leg's own plateau sits ~2 points higher than Windows'), with no mechanism
proposed — unlike D-095/D-096, this rests on CI evidence alone with no
local Linux aarch64 hardware to corroborate. The `$GITHUB_STEP_SUMMARY`
instrumentation from the first pass stays: it is now cited by D-101 itself
as what makes future observations (including passes) usable if this floor
ever needs revisiting. The `docs/ROADMAP.md` follow-up item was rewritten,
not left alongside the superseded reasoning — it now points at D-101 and its
own open mechanism question, matching D-095's/D-096's own follow-up
entries.

**Merged.** The pinned `ievo:deep-reviewer` reviewed the full committed range
(`a4c0f28`..head, the exact merge-base with `main`) and found no
correctness, security, or test-drift blockers — one actionable doc-drift
note (the v0.2 design spec's nbody-gate exceptions parenthetical still only
listed D-095/D-096, one commit after D-101 added a third exception), fixed
in the same PR before merge. [PR #188](https://github.com/rotnov/pycc/pull/188)
squash-merged as
[`23a106c`](https://github.com/rotnov/pycc/commit/23a106c) with a fully
green final CI run (all five `native-build-test` legs, `build-test-coverage`,
both `cross-compile-*` jobs, `frontend-perf-measure`/`frontend-perf-gate`,
`ci-gate`), `mergeStateStatus: CLEAN`, and no unresolved review threads.

Note for whoever picks up PR-9: PR-8 consumed D-090 through D-101 plus three
separate `main` merges before landing, and every one of the last five
blockers was CI-infrastructure reconciliation (concurrent D-099 activation, a
self-inflicted stage-PR merge conflict, and a two-pass nbody-flakiness
investigation that only resolved once a 6th CI observation arrived) rather
than PR-8's own compiler work (`pycc.toml` parsing, `--release`/LTO wiring,
the nbody fixture and harness). Worth a deliberate look before PR-9 at
whether future CI-gate/digest decisions should be split into their own PRs
rather than absorbed into whichever feature PR happens to be open when they
occur. Next up per `docs/DELIVERY_PLAN.md`'s v0.2 breakdown: PR-9 (real
per-PEP conformance harness).
