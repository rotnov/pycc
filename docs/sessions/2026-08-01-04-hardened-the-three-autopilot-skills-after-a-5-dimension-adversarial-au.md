# 2026-08-01 — Hardened the three autopilot skills after a 5-dimension adversarial audit

**Authoritative checkpoint:** `origin/main`'s tip is `26e415e` (same commit
the three skills merged at, below). This session's work lives on branch
`claude/issue-skill-hardening`, not yet merged — see its own pull request
once opened for the exact commit range and CI/review outcome.

**What happened:** while shipping issue #256 (`pycc init` rollback fix,
separate work, not part of this entry), a real gap surfaced live:
`issue-implement`'s "one CI re-run" instruction didn't distinguish a job
that gathers fresh data from one that only recomputes an already-uploaded
upstream artifact — the latter reproduces an identical failure on rerun by
construction, which is not evidence the failure "persisted." That prompted
a broader question: what else in the three skills has this shape? A
5-dimension parallel audit (33 agents: 5 finders, then an adversarial
verifier per candidate finding) read all three skills' full text against
`AGENTS.md`, `docs/DECISIONS.md`, `docs/AGENT_TOOLING.md`, and the eval/
oracle scripts. 28 candidate findings, 12 confirmed after independent
re-verification against the live text (16 refuted).

**Design correction worth recording:** the first design draft routed most
of the newly-found gaps to a full autopilot halt ("stop and report"). The
user pushed back directly ("не похоже это на автопилот цикл", "слишком
много стопов") — the standing directive is full autopilot, stop only on a
genuinely unsolvable problem. The design was reworked around a strict test:
would a *different* issue hit this same wall? Only "the pinned reviewer
cannot be bound" does (an environment failure, not an issue-specific one);
every other stop condition became **per-issue** — `issue-select`'s loop now
carries forward an in-run denylist of issues that hit a per-issue stop
condition, sets that one issue aside, and keeps working the rest of the
pool instead of halting or reselecting and re-failing it every iteration.

**14 fixes shipped** (design: `docs/superpowers/specs/2026-08-01-issue-autopilot-skill-hardening-design.md`;
plan: `docs/superpowers/plans/2026-08-01-issue-autopilot-skill-hardening.md`):
review-thread resolution now distinguishes bot-authored (self-resolvable)
from human-authored (reply only) threads; `issue-to-plan`'s delegation
exception is a closed, named list instead of an open self-declared class;
`issue-implement` reciprocally acknowledges `issue-select`'s
standing-directive closure authority (previously asserted only on
`issue-select`'s side); a two-tier issue-trust policy (owner-authored or
`approved`-labeled is trusted, otherwise an explicit security check is
required) across all three skills; the systemic/per-issue stop-condition
split above; a live re-check of the target issue's own state before
opening the PR and before merging; a bounded retry on a rejected merge;
`issue-to-plan` gained a 5-round review-loop cap it previously lacked
entirely; a concrete "at or near tip" definition replacing an undefined
proximity feel; a new capability letting `issue-implement` execute this
repository's established two-PR CI-digest stage-then-activate pattern
(D-080), chosen as full automation after the trust-anchor blast-radius risk
was raised and the user confirmed that choice explicitly; the informal
`P1:`/`P2:`/`P3:` priority-title-prefix convention promoted into D-111 (no
GitHub priority labels exist in this repository — live-verified, 85/104
open issues); `scripts/validate_agent_assets.py`'s structural eval-contract
check extended from 2 to all 5 alpha skills; and the previously-untestable
"Inconclusive" triage outcome now has real eval coverage and a
`triage_action` oracle that can actually distinguish it from "Still
current."

**Verified before this entry:** full local gate set green —
`scripts/test_run_alpha_skill_evals.py` (30 tests) and
`scripts/test_validate_agent_assets.py` (138 tests, 363 subtests) via
`python3 -m pytest`, both `validate_agent_assets.py`/
`validate_agent_policies.py`, `ruby scripts/check_roadmap_evidence.rb`, both
marketplace checkers, and `run_alpha_skill_evals.py --client claude`/
`--client codex` end to end against a locally built `pycc` binary — all
exit 0, checked directly (no pipe hiding a real exit code, per this
session's own earlier-learned lesson).

**Second audit pass, run before this entry:** 10 independent checkers
re-verified all 12 originally-confirmed findings against the actual
committed text, not the design's stated intent. 8 were genuinely resolved
outright; 2 were not, and 3 more were resolved but flagged thin regression
coverage — all 5 fixed and re-verified locally before this entry:

- **#1 not resolved on the first pass:** item 4's bot/human thread-resolution
  split never made it into step 7's own procedural text, which still said
  "resolve the thread afterwards" unconditionally — an agent following the
  workflow section (as opposed to the authorization summary) would have
  reproduced the exact original defect. Fixed, and bot detection tightened
  to a concrete signal (GitHub API author `type: Bot` / a known reviewer-bot
  login) instead of one named example.
- **#3 not resolved on the first pass:** the "at or near tip" evidence-bar
  fix landed only in `issue-select`; `issue-implement`'s own near-identical
  step 2 text — which the original finding's evidence explicitly cited —
  was untouched, so a direct `/issue-implement` invocation bypassing
  `issue-select`'s screen still hit the pre-fix behavior. Fixed.
- **#8, #11, #12:** resolved, but each had a real regression-coverage gap
  the checkers demonstrated empirically (deleting the fix's own text left
  every test green). All three closed with a pin or a targeted mutation
  test.

**Next session:** push the branch, open the pull request, D-078 monitoring,
merge, then resume the `issue-select` autopilot loop per the user's standing
directive.
