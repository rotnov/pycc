# Session handoff — issue #733 (issue-select critical-path escape)

## Status

This session ran the standing `/goal fix all opened issues` autopilot
directive for one issue (#733) and is delivering it through the pull request
this doc is committed into. At the point this snapshot was written, the
implementation and its gates were complete and the deep-review round had
returned actionable findings that were addressed in the same diff (see
below); the PR itself, its CI run, and the self-merge had not yet happened.
This section is re-verified against live PR/CI state immediately before
commit, per this repository's own session-log rule — if a later state
changed after this paragraph was drafted, the "PR and merge" section below
carries the authoritative outcome instead of this paragraph.

## What happened

Started fresh from `origin/main` tip `1cefc668` (the merge of PR #844, which
had just landed the #714 fix and its session doc) in an isolated worktree at
`/tmp/pycc-autopilot-run`. Ran `issue-select`:

- **Baseline**: no open pull requests; branch protection matches the
  documented baseline (`python3 scripts/manage_ci_bypass.py status`) — no
  D-024 governance incident to resolve first.
- **Milestone read**: `docs/ROADMAP.md`'s v0.3 section carries an
  "Update (2026-08-26): met." note backed by PR #797/#799 — v0.3 is complete.
  v0.4 ("projects & incremental") carries no such note and is the active
  milestone.
- **Non-milestone ceiling (D-192)**: 72 open issues carry no milestone,
  against the ceiling of 20. This run did **not** perform a full per-issue
  triage pass over all 72 — only a titles-only skim looking for an
  unambiguous v0.4 fit (multi-file/imports/incremental/`os`/`pathlib`/`json`/
  `datetime` keywords). None of the 72 titles read as an unambiguous v0.4-scope
  fit; the backlog is dominated by CI/governance/documentation/oversized-file
  decomposition work, which is process apparatus rather than product-milestone
  work. No milestone assignments were made this iteration. The ceiling stays
  in force at 72: **no new non-milestone issue may be filed by this or the
  next iteration** without one first closing. A full per-issue body read of
  the 72 remains undone — flagged here rather than silently skipped.
- **v0.4 candidate scoring**: sampled the v0.4-milestone P1/P2 backlog. #20
  (P1) is effectively gated — its only remaining scope (#631) requires the
  D-080 staged two-PR CI-workflow digest cycle, and the issue's own latest
  comment defers it. Among P2 survivors (#24, #414, #585, #636, #693, #707,
  #733, #834), #733 was selected: smallest verified blast radius (a
  skill-file prose amendment, no Rust/coverage surface), a currently-true,
  directly-reproduced premise, and a clean, itemized completion criteria
  list. #834 was the nearest rival but carries a live-code exception-edge fix
  in `bigint_rc.rs`, which historically drives the expensive part of this
  repo's D-014 100%-region coverage gate — a materially larger unit of work
  for the same priority tier.
- **Premise verification**: `grep -n -i "critical.path\|escape"
  .claude/skills/issue-select/SKILL.md` on the pre-change file showed exactly
  one match (the already-fixed, out-of-scope-vs-in-scope ordering rule from
  PR #727/#728, commit `02b51f8e`) — confirming the in-scope "evidence-bound
  critical-path escape" #733 asks for did not yet exist. Timestamp check
  confirmed #733 (filed 2026-08-22T11:05:37Z) describes a distinct, still-live
  gap from #727 (closed 2026-08-22T08:44:45Z): #727 fixed cross-scope
  ordering, #733 is about within-scope priority-marker ordering starving an
  unmarked critical-path issue.
- **Adversarial advisor round**: ran via this session's own advisor tool
  with the full peer set and #20/#631's deprioritization reasoning in
  context. The pick held. The advisor additionally required three things be
  settled before writing the fix, all resolved below.

## The fix

Added a new "evidence-bound critical-path escape" paragraph to
`.claude/skills/issue-select/SKILL.md` step 5, immediately after the
existing marker-then-size in-scope ordering rule. It lets an in-scope issue
outrank a higher-marked in-scope peer only with verified evidence that it
gates the active milestone's own Accept criterion (a named `docs/ROADMAP.md`
clause plus the specific completion path it unblocks, verified against the
current tree the same way step 6 verifies any candidate's premise). A bare
importance claim is explicitly insufficient. The escape never reaches
outside the scope and is a reportable event, matching the existing
"leaving the scope" reporting requirement; step 8's report checklist and the
`## Output` enumeration were both updated to name the escape-invocation
record alongside the existing scope-departure record.

Resolved the issue's three completion criteria explicitly:

1. **Skill-file amendment** — done, described above.
2. **Codex-entrypoint mirror** — checked `.agents/skills/issue-select/SKILL.md`:
   it is a thin pointer ("read `.claude/skills/issue-select/SKILL.md` ...
   completely and follow it as the canonical workflow"), naming only the two
   *arithmetic* gates (the D-192 ceiling and 4:1 quota) as call-outs easy to
   miss by skimming. The critical-path escape is not arithmetic and the
   wrapper's own stated scope is limited to those two named gates, so no
   Codex-side edit is needed — the pointer already gives full discovery of
   the new paragraph on both platforms. Verified `scripts/validate_agent_assets.py`
   has no prose-parity check between the two files (only structural checks:
   wrapper existence, canonical-path regex, and an exact eval-runner-set
   check unrelated to this prose) — confirmed via
   `python3 scripts/validate_agent_assets.py` and
   `python3 scripts/test_validate_agent_assets.py` both passing.
3. **ADR** — added. The first-pass rationale in this diff (D-111's marker
   semantics are untouched) was correct but incomplete: the deep-reviewer
   round below pointed out that the clause this change actually carves an
   exception into is D-191's own Decision text, "Inside each group (in-scope,
   and out-of-scope among themselves) the order remains marker-then-size" —
   and AGENTS.md requires superseding an accepted decision explicitly rather
   than silently narrowing it. Added
   [D-211](../decisions/D-211-evidence-bound-critical-path-escape-in-issue.md),
   which supersedes D-191's in-scope ordering clause only (the out-of-scope
   group's ordering, and D-191's membership-ranks-first rule, are untouched),
   and regenerated `docs/decisions/README.md` with
   `python3 scripts/generate_decisions_index.py docs/decisions docs/decisions/README.md`
   (verified fresh with `--check` afterward).

Also updated the offline eval machinery to match this project's own
precedent for step-5 ordering changes (D-191's own Consequences section):
added an eighth `issue-select` eval case exercising the escape (and its
inertness without evidence) to
`.claude/skills/issue-select/evals/evals.json`, registered the new runner
name `critical-path-escape-outranks-higher-marked-in-scope-peer` in both
`scripts/run_alpha_skill_evals.py`'s `EXPECTED_RUNNERS` and
`scripts/validate_agent_assets.py`'s mirror registry, extended
`issue_select_higher_ranked` with two further defaulted (inert-by-default)
parameters (`gates_milestone_accept`, `other_gates_milestone_accept`), and
pinned the literal "gates the active milestone's own Accept criterion" in
`ISSUE_SELECT_CONTRACT`.

## Gates run

- `python3 scripts/validate_agent_assets.py` — valid.
- `python3 scripts/test_validate_agent_assets.py` — 142 tests, all pass.
- `python3 scripts/test_run_alpha_skill_evals.py` — 47 tests, all pass after
  the second review round added two dedicated tests for the new escape
  (see "Second-round verdict" below); 45 passed before that round, already
  exercising `run_issue_select_case` and the new runner via the generic
  oracle test.
- `python3 scripts/run_alpha_skill_evals.py --client claude --pycc-bin
  target/debug/pycc` and the same with `--client codex` — both report
  "valid" after building a real `target/debug/pycc` (`cargo build -p pycc
  --bin pycc`), confirming the new eval case passes end to end on both
  client entrypoints, not just under the Python unit tests.
- `python3 scripts/generate_decisions_index.py docs/decisions
  docs/decisions/README.md` followed by the same command with `--check` —
  regenerated then confirmed fresh.
- `cargo doc --workspace --no-deps` — succeeds (one pre-existing,
  unrelated `rustdoc::private_intra_doc_links` warning in
  `crates/pycc_types/src/env.rs`, present before this change).
- Full `scripts/test_*.rb` suite run with `RUBYOPT="-E UTF-8"` — all green
  except `scripts/test_check_pages_performance_budget.rb`, which fails two
  tests (`test_resource_budget_fails_when_unexpected_image_added`,
  `test_resource_budget_fails_when_image_added_in_subdirectory`) on a clean
  `origin/main` checkout with **no** working-tree changes at all (verified by
  stashing this change and re-running). This is a pre-existing defect wholly
  unrelated to this diff (a Markdown skill file vs. a website
  performance-budget Ruby checker); it is not fixed here, and a background
  task was flagged (via this session's `spawn_task`) describing the
  reproduction and asking for a proper investigation/fix issue to be filed
  and implemented separately.
- No compiler-crate Rust source changed (only `scripts/run_alpha_skill_evals.py`
  and `scripts/validate_agent_assets.py`, both already covered above), so
  `clippy`/`cargo llvm-cov` carry no new compiler surface for this diff;
  CI's fail-closed classifier is the discriminator for whether the D-014
  coverage gate runs at all on a change like this.
- `ievo:deep-reviewer` run against the staged diff before commit. It
  returned four findings: one blocker (this session doc narrating unresolved
  future PR/merge state as completed fact) and three warnings (the ADR
  rationale addressing the wrong decision, missing eval/contract coverage
  for the new rule, and no step 8/`## Output` reporting slot for the
  escape). All four were addressed in this same diff before commit — see
  "The fix" above for the ADR and eval-coverage responses, the step 5 text
  for the step 8/Output update, and this section's own rewrite for the
  first finding. The reviewer was re-run against the updated staged diff
  after these fixes, since the fix round touched substantive new content
  (a new ADR file, new code in two scripts, a new eval case) rather than
  only the originally reviewed prose. **Second-round verdict:** all four
  original findings genuinely resolved (verified by hand-tracing the
  `issue_select_higher_ranked` key-tuple algebra for every call site); two
  new, narrower warnings surfaced — this session doc's own "see below"
  reference not resolving to recorded content (fixed by this parenthetical),
  and no dedicated unit test for the new parameters/pinned literal, unlike
  this file's own established convention. Both were fixed in the same diff:
  `scripts/test_run_alpha_skill_evals.py` gained
  `test_issue_select_critical_path_escape_outranks_higher_marked_peer` (four
  assertions: the escape firing, its inertness without evidence, its
  inertness across the scope boundary, and its inertness with no milestone
  scope at all) and
  `test_issue_select_eval_fails_when_the_critical_path_escape_text_is_missing`,
  mirroring the D-191-era tests immediately above them in that file. The
  full `scripts/test_run_alpha_skill_evals.py` suite was 47/47 green after
  adding them (up from 45/45 before this round).

## PR and merge

PR #845 ("Add an evidence-bound critical-path escape to issue-select step 5
(D-211)") was opened with `Fixes #733`, head commit `7f4584e7`. CI reached a
green/CLEAN/mergeable state (all required checks passing, no unresolved
review threads recorded at that point). Before this session reached its own
planned self-merge step, the repository owner (`rotnov`) merged the PR
directly at 2026-08-30T01:37:34Z, producing merge commit `77c2d6be` on
`main`. This is a legitimate D-127 owner intervention, not a self-merge by
this session, and it takes precedence over the agent's own merge plan.

After the merge, the automated `chatgpt-codex-connector` review left two
unresolved P1 threads on the now-closed PR:

- One claiming the `.claude/skills/issue-select/SKILL.md` step-5 ordering
  change should have updated `docs/DELIVERY_PLAN.md`/`docs/ROADMAP.md` under
  AGENTS.md's "execution order changes" rule. This is not actionable:
  AGENTS.md is explicit that `docs/DELIVERY_PLAN.md` "stays at its existing
  milestone/PR-level granularity ... and is never rewritten to hold
  per-issue detail," and D-211 changes only the per-issue selection order
  inside a milestone, a granularity that file is expressly forbidden to
  carry; `docs/ROADMAP.md`'s existing "Agent tooling" paragraph already
  describes the skill family generically and needs no D-211-specific detail.
  Separately, AGENTS.md's review-loop section states that asynchronous
  external review (including `@codex review`-style bot comments) is not a
  blocking gate under that section — it does not override branch
  protection's own required-conversation-resolution rule. No documentation
  update was made in response to this thread.
- One confirming that this file's own `## PR and merge` section still held
  stale placeholder text at merge time. That finding was correct; this
  paragraph is the correction, recorded in follow-up PR #846 — a
  documentation-only change against `main` carrying no `Fixes` keyword
  (`closingIssuesReferences.totalCount: 0`, verified) — since PR #845 was
  already closed and this file cannot be amended in place.

## Known follow-ups for the next session

- The D-192 non-milestone-issue triage pass (`issue-select` step 2) is still
  owed a full per-issue body read across the 72 open non-milestone issues;
  this session did only a titles-only skim and made no assignments.
- The pre-existing `scripts/test_check_pages_performance_budget.rb` failure
  (two tests) needs its own investigation/fix issue — flagged as a
  background-task suggestion this session, not yet filed as a tracked issue.
- `#24` (rustfmt CI gate) was not confirmed as D-080/D-103-gated (only
  suspected by analogy); a future selection pass should check whether it
  touches `.github/workflows/ci.yml` before scoring it.
- `#414`, `#585`, `#636`, `#693`, `#707`, `#834` — the other v0.4 P2 survivors
  — had only their raw bodies sampled, not fully read for staleness or blast
  radius, before this session settled on #733. A future pass should read them
  in full before scoring the next candidate.
