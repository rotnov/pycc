# 2026-09-02-07 -- Issue #729: tests/conformance.rs split into root plus cohorts

## Status: delivered

Worktree `.claude/worktrees/autopilot-2026-09-02-06`, branch
`claude/autopilot-2026-09-02-06`, started from `origin/main` at `50f1f61f`
and rebased onto `cfd858a1` after peer PR #878 (#868) merged mid-task.
[#729](https://github.com/rotnov/pycc/issues/729) (v0.4, P3) was delivered by
[PR #880](https://github.com/rotnov/pycc/pull/880), squash-merged as
`35f25169`; the issue is closed. Decision record
[D-221](../decisions/D-221-the-conformance-harness-is-the-root-file-plus-its.md).
Plan: <https://github.com/rotnov/pycc/issues/729#issuecomment-5512417181>.

## How this task was run

One autopilot iteration under the standing `fix all opened issues`
directive: D-021 preflight, `issue-select` (v0.4 membership first per
D-191), then `issue-implement` with `issue-to-plan` and the implementation
each dispatched into isolated agents (D-142/D-143), six D-068
`ievo:deep-reviewer` rounds, the `/harden batch` pass, and this file.

### Selection

#729 was the top in-scope survivor. The v0.4 P2s (#414, #585, #636) are
blocked or not closable by code; the other v0.4 P3s were excluded (#408 is
branch-protection work, #641 is evidence-only owned by #416's ubuntu-leg
floor work, #706 is blocked on unscheduled varargs). D-211 was not invoked
(no verified Accept-clause evidence). Non-milestone open-issue count 68
(> the D-192 ceiling of 20; no new non-milestone filings by selection). The
4:1 quota was unspent (#874/#875/#872/#871 all v0.4). No milestone
assignments and no stale closures this pass; the adversarial advisor round
was clean.

## What changed

- `tests/conformance.rs` (1276 lines) is now an 876-line root plus three
  cohesion-driven cohorts under `tests/conformance/` -- `classes.rs` (14
  tests), `exceptions.rs` (5), `numeric.rs` (8) -- as byte-identical moves,
  declared through a compile-bound `harness_modules!` macro and pinned by
  `every_harness_module_on_disk_is_declared`.
- The three harness text-readers (`tests/conformance_matrix_guard.rs`,
  `tests/conformance_oracle_guard.rs`, `scripts/check_conformance_breadth.py`)
  share one source definition: root first, then the sorted non-recursive
  `tests/conformance/*.rs` regular files, CRLF normalised (new
  `tests/harness_support/conformance_sources.rs`, `read_harness` in the
  Python checker; `--harness` default and `ci.yml` unchanged).
- Docs: `docs/TESTING.md`, `docs/PYTHON_STANDARDS.md`, `docs/DELIVERY_PLAN.md`,
  D-221, regenerated `docs/decisions/README.md`, cross-references in
  `tests/issue_603_*` / `issue_604_*`.

## D-068 review and harden batch

Rounds 1-5 each surfaced findings, fixed in place; round 6 was clean. Five
of the six rows in `.harden/findings/issue-729.jsonl` are one class: prose
or comments still locating the fixtures in the single root file, found one
per round because each fix's sweep grepped for the phrasing just corrected.
The batch pass recorded that as the fifth recurrence of
`documentation-sweep-stops-at-the-changed-file` and measured the #868
skill-sentence artefact at zero; the static-checker rung is filed as
[#879](https://github.com/rotnov/pycc/issues/879) (cross-cutting, no
milestone). The pile itself was first written in an invented schema and
caught by `scripts/check_harden_findings.py` (recorded under
`process-record-written-without-read-back`, nothing built), and the
missing `is_file()` guard seeds the new `duplicated-predicate-copy-drifts`
counter. The lesson is in `docs/AGENT_RETROSPECTIVE.md`.

## Known follow-ups

- #879 (the location-claim checker) is filed, unassigned, no milestone.
- The `tests/conformance.rs:699-701` overclaim noted by the #712 iteration
  no longer exists: it sat inside the stale PEP 698 block replaced in review
  round 1, so that unfiled follow-up is settled by #880.
- The 55 conformance tests that need the pinned CPython 3.14.7 oracle stay
  `#[ignore]`d on machines with 3.14.6, exactly as before the split.

## Where to resume

`docs/sessions/` newest-first, then `git log origin/main` for anything
after `35f25169`. #868/#878 (the peer task) merged as `cfd858a1` before
this PR; no other task was claimed at the time of writing.

## Paused autopilot

- Directive scope: open-ended (`fix all opened issues`); milestone scope
  v0.4 in effect.
- Last iteration outcome: #729 delivered (#880 -> `35f25169`).
- Next step: fresh D-021 baseline from `origin/main`, then re-enter
  `issue-select` (v0.4 first). #868 is delivered; no peer claim is known.
- In-run denylist: none.
- Known follow-ups not filed: none (the #712-era conformance overclaim is
  resolved; the checker promotion is filed as #879).
