# 2026-08-22-01 — #701 (Part 1 of #541) merged; autopilot paused before Part 2

## Baseline

Default branch tip at the time of writing: `a24b721d` — the squash merge of
[#710](https://github.com/rotnov/pycc/pull/710), *"feat(hir): synthesize HirClassDefs for the
builtin exception classes (Part 1 of #541)"*. No open pull requests. All remote state below was
re-resolved immediately before this file was committed.

## Delivered

[#701](https://github.com/rotnov/pycc/issues/701) — closed by the merge, as intended.
[#541](https://github.com/rotnov/pycc/issues/541) stays **open**: Part 2
([#702](https://github.com/rotnov/pycc/issues/702)) and Part 3
([#703](https://github.com/rotnov/pycc/issues/703)) are still ahead, and the pull request's
`closingIssuesReferences` was verified to be exactly `{totalCount: 1, nodes: [{number: 701}]}`
immediately before merging so #541 could not be closed by accident.

The branch carried five commits on base `7116ed0d`:

1. `789afe51` — synthesize the `HirClassDef`s; `Environment::synthetic_classes`; D-188.
2. `1f6a3339` — rebind the status page's checked text and identity digest.
3. `91101c27` — seed the synthetic classes only into modules that reference them.
4. `0ea948ef` — decide provenance by record, not by shape.
5. `d1869a41`, `8d123246` — attribute the classes-table invariant to both mutators, then
   narrow `bind_synthetic_class` to `pub(crate)` so the restriction is compiler-enforced.

CI settled fully green on `8d123246`, including all five Tier-1 targets and the aggregate
`ci-gate`. Coverage held at 100.00% lines / 100.00% regions (0 missed of 40695 lines and 26340
regions).

## What the review loop actually cost, and what it caught

Four blocker-severity claims were raised across three reviewers. Two were **refuted by running
the code** (the `c0001_callable_builtin_*` fixtures pass; the paired doc-drift warning fell with
it). One was **confirmed and fixed** (`0ea948ef`): structural equality had been used as a
provenance proxy, so a user's own `class Exception:` with an `__init__` started being rejected —
a real regression against base. One was **confirmed but proven pre-existing** and routed to
[#711](https://github.com/rotnov/pycc/issues/711) rather than blocking Part 1: explicit
`Exception.__init__()` ICEs on both base and head, at different panic sites.

Two of my own hypotheses were likewise refuted by measurement and withdrawn: a "fixed per-module
seeding cost" framing (the real pathology was `is_builtin_exception_class_def` rebuilding the
whole table on every `bind_class` call), and a "superlinear cost" reading of the CI numbers (the
added cost is roughly linear; the *baseline* is what grows superlinearly, and the 43.73 → 84.0 ms
CI comparison was runner noise).

The perf regression itself — `frontend-perf-gate` reporting 252.3% — is the entry worth
remembering: it reached CI because `scripts/check_frontend_throughput.rb` is an **absolute floor**
(75 ms) that a 3× regression can sit comfortably under, and the benchmark that would have caught
it was not the gate consulted before pushing. Fixing it required removing ~30 µs against 856 ns of
headroom; the LazyLock cache alone was ~10× short, which forced the reference-gated per-module
seeding in `91101c27`.

## Follow-ups filed this session

- [#711](https://github.com/rotnov/pycc/issues/711) — P2, v0.3: `Exception.__init__()` ICEs.
  Distinct from #704 (`e.args` via `expect_class`).
- [#712](https://github.com/rotnov/pycc/issues/712) — P3, v0.3: two `ci.yml` comments claim
  `tests/conformance.rs` has two `#[ignore]`d tests; it has 48. Comment-accuracy only, nothing
  mis-gated. `ci.yml` is manifest entry 1, so even this needs a D-103 stage-then-activate cycle —
  bundle it with the next PR that already touches `ci.yml`.
- Carry-forward planner notes posted on #702 and #703 recording the four facts from Part 1 that
  bear on their plans: the `KNOWN_CALLABLE_BUILTINS` interception, the exact-length assertion that
  must be rewritten alongside any table edit, the record-not-shape provenance rule, and the
  perf-gate constraint on widening the seeding gate.

## Paused autopilot

The governing directive is `/next-milestone` invoked with no arguments, currently on **v0.3**.

- **v0.3 Accept: not met.** `docs/ROADMAP.md:181` requires ≥ 37 `PYTHON_STANDARDS.md` matrix rows
  at `◐` or better; the tree is at **32 of 37**. The headline count is bound to the matrix by
  `check_conformance_breadth.py`'s `check_roadmap_counts` and was verified untouched by every
  commit on this branch.
- **Next step:** Part 2 of #541 — [#702](https://github.com/rotnov/pycc/issues/702) — then Part 3
  ([#703](https://github.com/rotnov/pycc/issues/703)). #541 closes only when all three land. Four
  of the five missing rows come through #542 (PEP 654) and #543 (PEPs 3151, 765, 758), both gated
  on #541; the fifth comes from #698's ranked shortlist.
- **In-run denylist carried forward:** `#20`, `#631`, `#604`. #20 and #631 are deprioritized per
  #20's own last comment. **#604's original stop reason was not recovered** across a context
  boundary and is recorded as unrecovered rather than reconstructed — a fresh session should
  re-screen it from scratch rather than trusting this entry.

## Environment notes for a resuming session

- The SSH key is gone after a reboot. Use
  `git -c url."https://github.com/".insteadOf="git@github.com:" fetch/push origin`; `gh` is
  authenticated over HTTPS.
- In this worktree `git checkout main` fails — use `git checkout -B <branch> origin/main`. And
  `gh pr merge --delete-branch` fails; delete via
  `gh api -X DELETE repos/rotnov/pycc/git/refs/heads/<branch>` after reading back `state`.
- Two local failure classes are environmental and reproduce identically at base — do not change
  code to accommodate them: ~48 `tests/conformance.rs` / `tests/nbody_bench.rs` failures reading
  `conformance oracle must be exactly Python 3.14.7, found "Python 3.14.6"`, and two
  `scripts/test_check_pages_performance_budget.rb` cases about unexpected images.
