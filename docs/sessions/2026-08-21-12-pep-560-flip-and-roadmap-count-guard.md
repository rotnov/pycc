# 2026-08-21-12 — PEP 560 flipped to `◐`, roadmap counts now guarded

Checkpoint taken after [#691](https://github.com/rotnov/pycc/pull/691) merged as
`dcf0c2a9`. Default branch at that commit; zero open pull requests.

## Overall status

v0.3 is **not met**. Its Accept criterion requires ≥ 37 conformance-matrix rows at
`◐` or better (D-153). `python3 scripts/check_conformance_breadth.py` on
`dcf0c2a9` reports:

```
conformance breadth: 32 evidence-backed rows, all declared (2 accepted as whole-PEP, 30 subset)
```

so the gap is **5 rows**, down from 6 at the start of this checkpoint's work.

## What landed

**#691 — PEP 560 flipped `☐` → `◐`, plus #623's third criterion.** Two changes
that are really one: the count moved, and the guard that keeps the roadmap honest
about it landed with it. Closed [#623](https://github.com/rotnov/pycc/issues/623).

The flip shipped no code. It records an observation that already existed: run
[32494747082](https://github.com/rotnov/pycc/actions/runs/32494747082) at
`82d63301`, all five Tier-1 jobs green, with
`tests/fixtures/pep_0560_class_getitem.py` registered at that commit. Two
interpretive questions had been blocking it, and both are now settled in
`docs/PYTHON_STANDARDS.md`'s new policy rule 9:

- `build-test-coverage` on `macos-14` **is** `aarch64-apple-darwin`'s Tier-1
  observation. It is the fifth target; `native-build-test`'s matrix supplies the
  other four. Both job families are gated on
  `needs.classify-changes.outputs.compiler == 'true'` and both reported `success`
  rather than `skipped`, so the classifier selected that run.
- D-102's "both build profiles" means the **fixture's** `pycc build` profile, not
  the Rust harness profile. Each conformance `#[test]` calls
  `run_conformance_fixture_with_profile(..., false)` then `(..., true)` in one
  body, so a single `cargo test --workspace -- --include-ignored` covers both.

Neither reading is new: policy rules 6, 7 and 8 already used exactly this
evidence basis for eleven prior hand-flips. Rule 9 writes it down so the next
session does not have to re-derive it.

The mark is `◐`, not `✅`, because the fixture reaches value-position `C[x]`
dispatch only. Annotation-position subscript gating is unimplemented and recorded
as the row's `core` gap in `tests/fixtures/conformance-breadth-manifest.json`.

#623's third criterion added a fail-closed guard: `check_conformance_breadth.py`
gains `--roadmap` and binds `docs/ROADMAP.md`'s conformance-progress headline to
what the matrix actually supports — total, whole-PEP count, subset count, the
internal `A of those N` restatement, and the derived gap against the required 37.
A headline that cannot be found or parsed fails, so rewording cannot quietly
disable it. Verified adversarially rather than asserted: mutating the total,
mutating the subset count, and deleting the headline each exit 1. It runs in CI
already — `.github/workflows/ci.yml`'s `governance` job invokes it with default
arguments, no `continue-on-error`, required through `ci-gate`.

## Issue-tracker changes

- **#623** — closed by #691.
- **#690** — narrowed, not closed. It remains the umbrella for the now-5-row gap.
  Its own premise that "no open issue tracks it" was corrected on the issue: four
  of the six gap-adjacent rows already have open v0.3 trackers (#586, #585, #542,
  #543). The premise was self-fulfilling — filing #690 created the umbrella it
  claimed was missing.
- **#586** — narrowed, not closed. Three of its four completion criteria are
  discharged by #610 and #691. What remains is annotation-position subscript
  gating, which is precisely what holds PEP 560's row at `◐`. Its title still
  describes the pre-#610 state and reads as stale.
- **Milestone triage** (`issue-select` step 2): only one milestone exists (`v0.3`),
  so triage reduces to in-scope versus cross-cutting. Assigned seven unambiguous
  matches — #23, #167, #246, #247, #249, #414, #416. Whole categories (public
  site, agent-tooling, CI-validator self-tests) were left unassigned deliberately:
  that is the repository's established pattern, not an oversight.

## Paused autopilot

The `/next-milestone` (no arguments) directive is **paused**, not finished.

- **Directive scope:** open-ended autopilot — adopt the first `## vX.Y` roadmap
  section whose Accept bullet is unmet on independently verified evidence, hand
  off to `issue-select`, loop until the milestone completes.
- **Active milestone:** v0.3, not met (32 of 37 rows).
- **Last iteration's outcome:** #690 selected, narrowed to its first concrete
  deliverable, delivered via #691; #623 closed as a bundled second deliverable.
- **Next step:** re-enter `issue-select` step 1 with a fresh baseline at
  `dcf0c2a9`. The most direct remaining path to v0.3's row count is the four open
  trackers named above; #586 is the narrowest, since its remaining half is a
  single gating rule and its fixture already exists.
- **In-run denylist (must carry forward):** **#20**, **#631**, **#604**.

## Known follow-ups

Carried forward from earlier checkpoints and still open: #558's elapsed-window
measurement; narrowing #162 to #397; #44's "downloaded but un-audited" gap; P1s
never screened (#259, #563, #565, #566, #569); stale decomposition-issue titles
(#545 says 17,665, now 7,759; #549 says 4,701, now 4,614); #641's title naming
only `macos-15-intel`; D-171's stale lines 8 and 12; the orphaned
`tests/fixtures/policy-successors/`; `src/project_config.rs:116`'s citation of a
nonexistent test gap; a mechanical CI guard over declared
`closingIssuesReferences`; #162 and #397 both still unassigned to a milestone; and
`crates/pycc_types/src/tests.rs` at 25,253 lines with no D-185 tracking issue.

## Where to resume

`docs/sessions/` sorted by filename — the date-then-`NN` prefix keeps lexical
order chronological. This entry supersedes `2026-08-21-11` as the latest state.
