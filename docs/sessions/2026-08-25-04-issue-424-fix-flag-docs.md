# Session handoff: issue #424 — `--fix` CLI flag doc claims corrected

- Date: 2026-08-25
- PR: [#789](https://github.com/rotnov/pycc/pull/789) (`docs/issue-424-diagnostics-fix-claim` -> `main`)
- Issue closed by this PR: #424 (verified via GraphQL `closingIssuesReferences`:
  `totalCount: 1`, node `424` — this PR closes exactly and only #424)

## Status

PR #789 is open and pushed; CI has not been observed to completion at the
time this file is written. All work described below is committed and
pushed to the PR branch; nothing is merged yet.

## What shipped

Issue #424 reported that `docs/DIAGNOSTICS.md` asserted a `pycc check --fix`
CLI flag as already implemented. Verified via `grep -rn 'fix' src/` that no
`--fix`/`Fix` flag exists anywhere in `src/cli.rs` or the rest of `src/` —
the issue was not stale.

Two commits on the branch:

1. `2f00d6e0` — reworded the three doc claims that presented `--fix` as
   already implemented:
   - `docs/DIAGNOSTICS.md`'s Quality-bar bullet
   - `docs/CLI_SPEC.md`'s `--fix` flag-table entry
   - `docs/TYPE_SYSTEM.md`'s Error-philosophy line
   All three now describe `--fix` as planned/not-yet-implemented, matching
   the framing `docs/AGENT_TOOLING.md:240` and `docs/ROADMAP.md:39` already
   used.

2. `5970d7fd` — follow-up after the D-068 `ievo:deep-reviewer` pass (and a
   second self-review round) caught two more issues of the same class:
   - `docs/CLI_SPEC.md:291`'s JSON schema still asserted a `fix{edits[]}?`
     field as current; the serializer (`crates/pycc_diag/src/lib.rs`'s
     `render_json`/`Diagnostic`) never emits a `fix` key. Reworded as
     planned/not-yet-emitted.
   - `docs/DIAGNOSTICS.md`'s quality-bar bullet had accidentally been
     reworded to make "must be idempotent + tested" itself conditional on
     `--fix` existing, weakening a standing requirement on the
     already-existing `fix{edits[]}` data model. Restored to a present-tense
     requirement, independent of the flag's implementation status.
   - `docs/CLI_SPEC.md`'s "currently rejected" wording for `--fix` could be
     misread as "pycc recognizes the flag specially and rejects it."
     Verified via `scripts/run_alpha_skill_evals.py` (asserts
     `"unexpected argument '--fix' found"`) that it is clap's generic
     unrecognized-argument error, and reworded to say so explicitly.

No code changes; docs-only.

## Verification run

- `ruby scripts/check_source_links_registry.rb` — passes (unaffected by
  this change).
- `python3 scripts/check_claude_reviewer_binding.py` — confirms a
  structurally verified `ievo@ievo-skills` install (`0.80.19`, `0.80.22`
  available).
- D-068 pinned `ievo:deep-reviewer` run against commit `2f00d6e0` — returned
  2 findings (1 warning: the `fix{edits[]}` claim; 1 note: missing session
  entry, addressed here) — both addressed in commit `5970d7fd` and this
  file.

## Known follow-ups

- None identified beyond this PR's own scope. No GitHub issue tracks
  `--fix` CLI-flag implementation itself; the reworded docs point at
  `docs/CLI_SPEC.md` generically rather than a specific tracking issue,
  since none exists.

## Where to resume

Nothing further to do on this task beyond normal PR review/merge by the
repository owner. If picking this back up: confirm CI is green on PR #789,
confirm no new review findings, then merge (this session did not merge, per
D-024 protected-main policy).
