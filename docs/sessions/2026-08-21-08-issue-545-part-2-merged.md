# 2026-08-21-08 — issue #545 Part 2 merged, autopilot paused mid-loop

## Repository state at the moment this entry was committed

- `origin/main` = `40e49875e7e311f8296e4f101dc8c084316788da` (squash merge of PR #679).
- Open pull requests: none.
- Branch protection at the documented baseline: `{"admins":true,"checks":["ci-gate","audit"],"strict":true}`.
  No D-024 or D-125 incident is open.
- `tests/fixtures/policy-successor-manifest.json`: 49 targets, 0 mid-transition, so no
  run-wide `audit` block is in effect.

## Delivered this checkpoint

**PR #679 — `refactor(codegen): extract rt_fns.rs from the crate root (Part 2 of #545)`**,
merged as `40e49875`.

`crates/pycc_codegen/src/lib.rs` went from 8,190 to 7,759 lines. `struct RtFns<'ctx>` and
`declare_rt_functions` moved verbatim into the new `crates/pycc_codegen/src/rt_fns.rs`
(448 lines). Visibility widened only to `pub(super)` — 66 markers, matching what the three
existing sibling carve-outs in the crate already use — and no new bare `pub` appeared.
Coverage did not move: workspace TOTAL 100.00% regions / functions / lines with zero missed,
`rt_fns.rs` itself at 416/416 regions and 278/278 lines.

Per D-185 the issue is **not** closed: the file is still far above AGENTS.md's ~1,000-line
threshold. #545 carries a narrowing comment renumbering the remaining production seams against
`40e49875` — scalar tagging (354–657), collection builders, numeric coercion (968), string
emission (1145/1268) — any of which is a viable Part 3.

Two documentation commits rode in the same branch: a new `docs/AGENT_RETROSPECTIVE.md` entry,
and a follow-up correcting two of that entry's own claims after review found them ungrounded.

## Also written this checkpoint

- **#275 narrowed** ([comment](https://github.com/rotnov/pycc/issues/275#issuecomment-5369612232)).
  The blocking diagnostic `candidate protected policy target <path> lacks a base-staged
  successor` is absent from `scripts/` at the current tip and was present at `163bf49f^`;
  `scripts/check_ci_permissions.rb` is now 268 lines with zero `successor`/`source_path`
  occurrences. Residual is documentation only: the 2026-08-01 issue-109 plan document still
  narrates the retired validator as live. One claim from the screening pass was **refuted**
  before publication — `D100_COMPOSE_D91_D99_CI_WORKFLOW_SHA256` is not inert, it is read by
  live checks at `scripts/check_roadmap_evidence.rb:2214` and `:2558`.

## Milestone status

**v0.3 is not met.** `python3 scripts/check_conformance_breadth.py` reports
`31 evidence-backed rows, all declared (2 accepted as whole-PEP, 29 subset)` against the
roadmap's `≥ 37` requirement. Six more rows are needed. The diagnostics-registry and
`pycc explain` conjuncts of the same Accept bullet have not been separately re-verified at
this tip. `docs/ROADMAP.md:183` still states "29 of the required 37", which is stale in the
other direction — tracked by open #623.

## Paused autopilot

The standing directive is `/next-milestone` with no arguments, delegating to `issue-select`.
The loop is **paused mid-iteration**, not terminated.

- **Directive scope:** loop milestones; adopt the first `## vX.Y` roadmap section whose
  Accept bullet is unmet on independently verified evidence; hand off to `issue-select`.
- **Active milestone:** v0.3, not met (above).
- **Last iteration's outcome:** #545 Part 2 selected and merged.
- **Next step:** re-enter `issue-select` at step 1 with a fresh baseline from `40e49875`.
- **In-run denylist (must carry forward):** #20, #631, #604.

### Selection state a resuming session can reuse rather than re-derive

A staleness screen over the P1 tier was run this session against `be7268ef`. Its conclusions,
each of which a resuming session should treat as evidence to re-check rather than accept:

- **#82** — still current, premise reproduced empirically (a `[toolchain] path` override in
  the PR-controlled `rust-toolchain.toml` redirects `rustup which cargo`, and `ci.yml` resolves
  the trusted binaries before `cd "$RUNNER_TEMP"`). Fully autonomous, two-PR route via
  `REVIEWED_COVERAGE_SCRIPTS` rather than the trust-anchor digest. The strongest remaining
  candidate.
- **#14** — still current, but end-to-end proof needs GitHub-hosted Windows/Linux runners.
- **#44** — still current, and its acceptance criteria are positively forbidden by a passing
  test plus accepted ADR D-172. Taking it means superseding an accepted decision.
- **#45** — partially resolved; the checkout ref is fixed, the bare `pull_request_target:`
  trigger is not. Small, but touches the trust anchor's own digest allowlist.
- **#53** — wider than filed: the same missing-Codex-fallback defect affects three adapters,
  and no repository skill carries such a fallback, so a fix must establish the pattern.
- **#558** — deliberately held open for a 20-merge comparison whose window has now elapsed;
  the residual is a measurement and a comment, not implementation.
- **#162** — narrowed to Part 4 (#397). A second pass also found that the README comparison
  table's ✅/❌ glyphs are bound by nothing: `scripts/check-site.sh` strips them before
  comparison, and for the `mypy / pyright` row two of three cells have no validated content
  at all. That is a live correctness gap and is **not** currently tracked by its own issue.

## Follow-ups noticed but not filed

- `docs/decisions/D-171-change-aware-ci-gate-scheduling.md` lines 8 and 12 both describe the
  CI workflow successor as staged and not yet active; the live `ci.yml` contradicts this.
- `docs/superpowers/plans/2026-08-01-issue-109-frontend-perf-gate-runner-move.md` line 50 and
  its Task 5 narrate the retired manifest validator as live.
- `crates/../src/project_config.rs:116` cites a "W3 accepted `cfg(unix)` test gap" for which no
  acceptance record exists under `docs/`.
- Decomposition tracking issues carry stale line counts in their titles, all measured at this
  tip: #545 says 17,665 (now 7,759), #544 says 31,673 (`crates/pycc_types/src/lib.rs` is now
  34,300 — it grew), #549 says 4,701 (`crates/pycc_types/src/class.rs` is now 4,614). Title
  edits are outside `issue-implement`'s authorized writes.
- #162 and #397 both have no milestone.
