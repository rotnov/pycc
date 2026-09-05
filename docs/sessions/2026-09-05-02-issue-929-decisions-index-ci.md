# 2026-09-05-02 — issue #929: run `generate_decisions_index.py --check` in CI

## Status

Delivered in the pull request that carries this snapshot, branched from
`origin/main` at `4eca5e24` (#933 merge). No Rust changed; `cargo doc
--workspace --no-deps` exited 0 at the task base.

## What landed

- `.github/workflows/ci.yml`: one new step in the `governance` job, directly
  after the conformance-breadth step, running
  `python3 -B scripts/generate_decisions_index.py docs/decisions
  docs/decisions/README.md --check`. `governance` is already in `ci-gate`'s
  `needs:` at `contents: read`, so the required-check list is unchanged.
- `AGENTS.md` ("Keep documentation current") and
  `docs/REPOSITORY_GOVERNANCE.md` now say CI enforces the check.
- `.harden/incidents/decision-number-taken-by-a-merge-mid-review/incident.md`
  moved from `verdict: pending` to `shipped`, pointing at the CI step, with the
  manual both-direction verification recorded.
- `.harden/findings/issue-929.jsonl` holds the review rounds.

## Correction to the issue body

The issue (and the incident file it came from) claimed the edit needs the
D-080 two-pull-request staged-digest procedure because
`scripts/check_roadmap_evidence.rb` pins `ci.yml` by SHA-256. That is stale:
`validate_evidence` returns early through `d171_routed_workflow?` for any
workflow whose `jobs` contain `classify-changes` or `governance`, so the
`REVIEWED_PERF_CI_WORKFLOW_SHA256S` pin is never reached for the live
workflow. All four ruby gates exited 0 with the step added.

## Follow-ups

- None required for #929. `docs/ROADMAP.md` has no cell enumerating the
  individual `governance` steps, so it was not edited.

## Where to resume

Nothing in flight for this issue after the pull request merges. The
violator/clean proof commands are in the pull-request body.
