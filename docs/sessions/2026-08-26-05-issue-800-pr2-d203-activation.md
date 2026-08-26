# Session handoff: issue #800 PR-2 — D-203 activation

## Status

PR-2 (activation, the final part) of issue #800 (milestone v0.3),
implemented against `origin/main` at
`dd1d91151ca437b21a88687ca9a1e56ded036b42` (the PR #804 merge that
delivered PR-1's checker-side D-203 authorization; re-verified by
`git fetch --prune` at implementation start) on branch
`feat/issue-800-pr2-d203-activation`, from the reviewed three-round
implementation plan published at
<https://github.com/rotnov/pycc/issues/800#issuecomment-5427644875>
(§4 "PR-2 — activation"). This entry lands with PR-2's merge (D-192).
Merging PR-2 closes #800.

## What PR-2 delivers (everything CI-visible)

- `.github/workflows/ci.yml`, job `frontend-perf-measure`, step
  "Verify exact benchmark revisions": the final two-line tail `diff` is
  replaced by the D-203 filtered form — a three-line `# D-203:` shell
  comment plus the symmetric
  `grep -vxF 'pycc_scratch = { path = "crates/pycc_scratch" }'` filter on
  both `[dev-dependencies]`-onward tails. The replacement was spliced
  mechanically from `D203_VERIFY_REVISIONS_SCRIPT` in
  `scripts/check_roadmap_evidence.rb` (never retyped), then verified
  byte-identical post-dedent by parsing the YAML and comparing the step's
  `run` body against the dedented heredoc — `IDENTICAL` — and end-to-end
  by the checker run below.
- Root `Cargo.toml`: `pycc_scratch = { path = "crates/pycc_scratch" }`
  added directly under the `[dev-dependencies]` header — the exact line
  the filter tolerates, whose earlier attempts (`f79bb2b5`, `f9231e2f`)
  were reverted against the un-narrowed gate.
- `Cargo.lock`: regenerated (`cargo metadata`); the root `pycc` package
  gains the `pycc_scratch` dependency edge — the only hunk.
- `docs/decisions/D-201-shared-pycc-scratch-crate-and-lint-gate-for.md`:
  dated (2026-08-26) update appended to the blocker paragraph marking the
  D-091 tail blocker resolved by D-203 plus this activation; migration
  itself stays under #782.
- `scripts/check_scratch_dir_usage.py`: both ALLOWLIST blocker comment
  blocks (`tests/issue_150_zero_step_range.rs`,
  `tests/issue_769_optional_narrowing.rs`) reworded to past tense with a
  D-203 reference; counts and code unchanged.
- `docs/ROADMAP.md`: one dated D-203 sentence added to the quality-gates
  row (written against the post-#801 text, which had merged before this
  session branched). The sentence had to be condensed to 105 bytes:
  `main`'s ROADMAP already sat within 110 bytes of the D-200 264 KiB
  llms.txt aggregate budget (`scripts/check-site.sh`), #801's milestone
  text having consumed the ~7 KiB headroom D-200 bought. **After this PR
  the remaining margin is 5 bytes** — the next ROADMAP addition of any
  size trips the gate and will need a D-200-successor ADR raising
  `budget_kib` in `site/llms-txt-context-manifest.json` (see D-200 for
  the sizing rationale and the 2026-08-26 retrospective entry for the
  recurring pattern).
- `docs/REPOSITORY_GOVERNANCE.md`: the plan's optional one-clause mention
  of the D-203 tolerance added to the active-ci.yml frontend-perf
  paragraph. `docs/TESTING.md` re-verified: only historical digest
  narrative, no byte-exact-tail claim — no edit. `docs/SPEC.md`: no
  specification added/removed/repurposed — no edit.
- `docs/AGENT_RETROSPECTIVE.md`: new 2026-08-26 entry recording the PR-1
  delivery lesson — `ci-watch.sh` prints
  `BLOCKED -- all checks completed with no failures` when GitHub Actions
  has dispatched zero check suites (vacuously true over an empty check
  list), which the 2026-08-26 Actions outage triggered twice; confirm
  checks exist via `gh pr checks`/the check-suites API before acting, and
  re-fire lost `pull_request` events with close/reopen.

## Gates (all green at this snapshot, macOS local run, UTF-8 locale prefix)

- `ruby scripts/check_roadmap_evidence.rb`: passed — in this tree that is
  the live byte-identity proof that the narrowed ci.yml step matches
  `D203_VERIFY_REVISIONS_SCRIPT` (the lifecycle validator's D-203 branch
  accepts the measure job).
- `ruby scripts/test_check_roadmap_evidence.rb`: 237 runs,
  1222 assertions, 0 failures, 0 errors, ~27 s.
- `ruby scripts/check_ci_permissions.rb`: passed (10 files).
- `python3 scripts/check_scratch_dir_usage.py`: passed;
  `python3 scripts/test_check_scratch_dir_usage.py`: 12 tests, OK.
- `python3 -m unittest discover -s scripts -p 'test_*.py'`: 955 tests,
  OK (skipped=6), ~46 s.
- `python3 scripts/generate_decisions_index.py docs/decisions
  docs/decisions/README.md --check`: up to date (no ADR change this PR).
- `cargo check -q`: clean — the new dev-dependency resolves.
- `cargo test --workspace`: 64 test binaries, every result line ok,
  0 failures.
- D-021 preflight: `cargo doc --workspace --no-deps` green (one
  pre-existing `pycc_types` doc warning);
  `python3 scripts/manage_ci_bypass.py status` matched the documented
  baseline.
- CI expectation per the plan §7: this PR routes to `Selection.full()`
  (ci.yml changed) — all jobs including the 100% coverage gate run;
  `audit` uses the base checker, which since PR #804 contains the D-203
  branch, so the narrowed head ci.yml is accepted; `frontend-perf-measure`
  runs the narrowed step from the merge commit (previous=base without the
  line, current=merge with it → filtered diff clean), and because
  Cargo.toml/Cargo.lock change, the executable-input classifier triggers
  the real five-replicate comparison under the D-114 7% threshold.

## Pending — NOT delivered by this PR

- **PR #793 disposition** (under #782, not #800): rebase onto `main`,
  retarget from `feat/issue-781-scratch-dir`, drop the hunks the base
  already carries, keep the `src/main.rs`/`src/project_config.rs`
  migrations, lower the ALLOWLIST counts; also fold in `f9231e2f`'s
  `tests/issue_769_optional_narrowing.rs` allowlist debt. The D-203
  narrowing this PR activates is what unblocks that work.
- **Issue #803** (duplicate D-201/D-202 renumbering) remains open and
  separate; nothing here touches the duplicate files.

## Where to resume

After this PR merges, #800 is closed. The next scratch-dir work is #782
Batches B–D (start with PR #793's rebase/retarget per the plan's
post-merge hand-off note); the plan comment on #800 §4 "Post-merge"
carries the checklist.
