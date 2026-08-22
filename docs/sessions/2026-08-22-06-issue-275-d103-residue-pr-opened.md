# 2026-08-22-06 — issue #275: retire the D-103 residue, pull request opened

## Baseline

- Default branch: `origin/main` at `a60d6d64b5a4e8aaf6fd41b5ae5a00850586a9d8`.
- Open pull requests at this checkpoint: none.
- Task branch `issue-275-retire-d103-residue`, three commits ahead of that exact tip,
  clean working tree.

## What this checkpoint delivers

Issue #275 asked for the retirement of the last residue of D-103, the
policy-successor stage-then-activate mechanism that [D-172](../decisions/D-172-nonblocking-property-based-ci-policy-audit.md)
superseded when PR #570 merged on 2026-08-17.

The decisive finding, established before any edit: `163bf49f` removed
`validate_policy_successor_transition` from `scripts/check_ci_permissions.rb`, and that
script no longer reads `tests/fixtures/policy-successor-manifest.json` at all. The manifest
is nonetheless still live, through a completely different mechanism —
`.github/workflows/workflow-policy.yml` parses it and materializes every listed `path` from
the head tree via its `findEntry` helper, which throws when a listed path is absent. So a
listed path still cannot be renamed, deleted, or moved without updating the manifest in the
same pull request, but *editing* a listed path's contents now needs no special handling at
all.

Three commits:

- `e78798ff` — delete 10 `tests/fixtures/policy-successors/` fixtures with zero remaining
  references anywhere in the tree.
- `131d424d` — stop `.claude/skills/issue-implement/SKILL.md`,
  `.claude/skills/issue-select/SKILL.md`, and `.claude/skills/ci-temporary-bypass/SKILL.md`
  from directing sessions to a checker that no longer exists; correct
  `docs/AGENT_TOOLING.md`; add Correction #5 to
  `docs/superpowers/plans/2026-08-01-issue-109-frontend-perf-gate-runner-move.md`; drop the
  three now-subjectless eval cases per client and the `manifest_transition_status` oracle
  they were the only consumers of.
- `de1e88c0` — fixes for the two findings the pinned reviewer raised (below).

### The per-file fixture determination

The 27 files under `tests/fixtures/policy-successors/` were resolved individually, not as a
class. 17 stay:

- 14 are manifest `path` entries, so deleting one breaks the required `audit` check.
- `ci-d211.yml` is not in the manifest but is read by exact path from
  `scripts/test_check_roadmap_evidence.rb`, which `workflow-policy.yml` itself runs.
- `check_roadmap_evidence-d171.rb` and its self-test are cited by exact path from
  `docs/superpowers/plans/2026-08-15-ci-feedback-routing.md`.

The remaining 10 have zero references and were deleted.

**A trap worth recording:** several of these fixtures share a basename with a real file
under `scripts/`. A basename `grep` produces dozens of false "still referenced" hits and
would have kept every one of them. The classification was redone with exact-path
`git grep -F -- "tests/fixtures/policy-successors/<name>"` searches.

## Review

D-068 pinned reviewer (`ievo@ievo-skills` 0.78.8 `deep-reviewer`), two rounds:

- Round 1 — 3 findings, no blockers. Two were actionable: a Scope cross-reference in
  `issue-select` that pointed at a step-4 bullet the same change had repurposed, so it
  resolved to nothing; and a manifest note that declared itself "not a deprioritization"
  while sitting inside a list headed "Drop or defer". A `[note]` observed that
  `issue-select` lacked the D-080 scoping sentence `issue-implement` carries — material,
  since `.github/workflows/ci.yml` is itself the manifest's first entry, so the text could
  read as overriding D-080's still-live two-pull-request digest cycle.
- Round 2 — 0 findings, all three fixes confirmed against the tree.

## Gates

All run by this session as sole writer, exit status captured directly: `validate_agent_assets.py`,
`test_validate_agent_assets.py` (142 tests), `validate_agent_policies.py`,
`check_ci_permissions.rb`, `check_roadmap_evidence.rb`, `test_check_roadmap_evidence.rb`,
`test_check_ci_permissions.rb`, `test_classify_ci_changes.py`,
`generate_decisions_index.py --check`, the full `scripts/` unittest suite, and
`run_alpha_skill_evals.py` for both clients — every one rc=0.

`scripts/classify_ci_changes.py` selects `Selection(compiler=False, pages=False, agent=True)`
for this file set, which is correct: no Rust changed.

## Deliberately not done

`AGENTS.md`'s standing owner bypass authorization (2026-08-14) still names "the recurring
D-103 manifest deadlock class". Narrowing an authorization the repository owner granted is
outside an implementation pull request's scope; it is reported in the pull-request body
instead.

## Paused autopilot

The standing `/next-milestone` directive (no arguments) remains in effect and is **paused**
at this checkpoint, not terminated.

- Active milestone: **v0.3**. Accept criterion — at least 37 `docs/PYTHON_STANDARDS.md`
  matrix rows at `◐` or better. `python3 scripts/check_conformance_breadth.py` reports 32
  evidence-backed rows at this baseline, so **v0.3 is not met** and the `issue-select` loop
  continues.
- Last iteration's outcome: #275 selected, implemented, reviewed clean, pull request opened.
- Next step: monitor `audit` and `ci-gate` on that pull request, merge under the guarded
  read-back pattern, then re-enter `issue-select` at step 1 with a fresh baseline.
- In-run denylist carried forward: **#20**, **#631**, **#604**. #604's original stop reason
  was lost across a context boundary and is recorded as unrecovered rather than
  reconstructed.

## Where to resume

Read this file, then the pull request for #275. The remaining v0.3 breadth gap is five rows:
four are expected from #542 and #543 (both gated on #541, whose Part 3 is #703), and the
fifth candidate is #719 (PEP 701, fixture-only).
