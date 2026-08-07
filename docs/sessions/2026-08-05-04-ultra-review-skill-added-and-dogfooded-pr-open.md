# 2026-08-05 checkpoint: `ultra-review` skill added, dogfooded live, PR opened

## Status

A seventh alpha skill, `ultra-review`, is implemented, reviewed, dogfooded three
times against the live `rotnov/pycc` repository, and sits on branch
`feat/ultra-review-skill-design` in
[PR #357](https://github.com/rotnov/pycc/pull/357), which is **open, not
merged**. The merge decision was deliberately handed back to the maintainer
rather than taken autonomously.

Branch is rebased onto `origin/main` `e5f9302` (re-fetched and re-confirmed
immediately before this entry was committed; no concurrent-actor push landed
during the session).

## What the skill is

`ultra-review` re-reviews the codebase periodically for the drift class a single
pull request's own D-068 gate structurally cannot see — inconsistency that only
appears once several merges have accumulated, or a doc/config file nobody
happened to touch in the diff that introduced the drift. It reads a
GitHub-native checkpoint (a dedicated tracking issue, not a tracked file — this
project's ephemeral-worktree lifecycle ruled a checkpoint file out directly),
computes the diff since that checkpoint, dispatches the pinned D-068
deep-reviewer once against it, maps `blocker`/`warning`/`note` findings to
`P1`/`P2`/`P3` GitHub issues with milestone-at-filing and a dedup pass, and
files the survivors autonomously within a bounded evidence bar — mirroring
D-022's standing-authority precedent rather than `pycc-feedback`'s per-payload
gate. It mutates no tracked file and implements nothing itself.

The single-pass architecture was **not** the original design. An
originally-proposed second, broader architecture/roadmap-alignment pass was
built and empirically raced against the single pass on the same real
accumulated diff; an independent judge found the second pass's only genuinely
out-of-scope finding weak and non-blocking, while its other findings either
duplicated the single pass or were an outright false causal claim that
distracted from a real drift bug in the same file region. The bake-off and its
verdict are recorded in full in
`docs/superpowers/specs/2026-08-05-ultra-review-skill-design.md`; the decision
itself is [D-147](../DECISIONS.md).

## What was actually done this session

1. Six commits implementing the skill (design spec, plan, skill definition +
   Codex mirror + evals, registration in `scripts/run_alpha_skill_evals.py` and
   `scripts/validate_agent_assets.py`, documentation in `docs/AGENT_TOOLING.md`
   / `docs/ROADMAP.md` / `docs/SPEC.md` / D-147).
2. Full local gate set, each command's own exit status captured explicitly
   rather than read off a piped summary: 494 Python unit tests, both agent
   validators, both client eval suites, both Ruby roadmap-evidence checkers,
   the CI-permission checker, and the marketplace checker — all exit 0.
3. **Four D-068 review rounds** with the pinned deep-reviewer, fixing findings
   between each. Round 4 reported only two self-labeled non-blocking notes and
   "ready to commit as-is"; both were fixed anyway. Substantive findings across
   the loop: a plan reference to the retired `docs/SESSION_LOG.md` (D-130
   replaced it with this directory); the skill's step 5 citing `AGENTS.md`
   D-021 step 9's milestone-at-filing convention while inverting its default
   and omitting the `no milestone — cross-cutting` reasoning slot that step
   requires; a missing negative test for the new `ULTRA_REVIEW_CONTRACT`
   fail-closed branch; a design-spec `--limit 1` that would have made the
   skill's own duplicate-checkpoint race branch unreachable.
4. **Three live dogfood runs against `rotnov/pycc`** (below).
5. Filed [#356](https://github.com/rotnov/pycc/issues/356) for an unrelated
   governance defect found during the review loop (below); deliberately kept
   out of this PR's diff.
6. Pushed the branch and opened [PR #357](https://github.com/rotnov/pycc/pull/357)
   at head `e1d6a92`, then added this entry on top of it. Monitored CI to green.
   **Did not merge.**

## The three dogfood runs

**Run 1 — bootstrap.** Neither label existed. Created `ultra-review-checkpoint`
and `ultra-review`, then created checkpoint tracking issue
[#355](https://github.com/rotnov/pycc/issues/355) with the exact step-8 block
format, checkpoint set to the then-current default-branch tip `e5f9302`. Diff
range was consequently empty, so the run stopped there — the deliberate
one-time no-op the design specifies instead of a full historical sweep. No
other public write.

**Run 2 — empty-diff no-op.** Parsed the checkpoint back out of #355, computed
`e5f9302..e5f9302`, found it empty, reported "nothing new" and stopped. Verified
afterwards that #355's `updatedAt` still equalled its `createdAt` and that zero
`ultra-review`-labeled issues existed — the run performed no write at all,
which is exactly step 3's contract.

**Run 3 — re-verification after the fixes.** Re-ran the whole flow against the
corrected skill text; same clean empty-diff stop, still no writes.

Runs 2 and 3 are the two consecutive correct runs the plan's Task 5 Step 6
required.

**Two real defects the dogfooding found in the skill's own text, both fixed:**

- Step 2's bootstrap branch deferred the checkpoint body to step 8, but step
  8's block has two fields with no bootstrap meaning (`Reviewed at:` and
  `Last run: <N findings, M filed, K deduped>`) because no review has run yet.
  The live run had to invent them. Step 2 now fixes the bootstrap body
  literally.
- Step 2's label check used bare `gh label list`, which silently returns only
  the first 30 labels, and said nothing about `gh label create` exiting 1 on an
  already-existing label — an ensure-exists step whose success condition looks
  like a hard failure. Both are now stated, along with why the label check
  cannot be folded into the checkpoint search (`gh issue list --label
  <missing-label>` returns an empty list and exit 0, indistinguishable from "no
  checkpoint yet").

Neither defect would have been caught by the offline evals; both required the
live run. That is the argument for dogfooding a skill before merging it.

## Honest scope of what the live runs proved

The dogfood runs exercised the skill's steps 1–3 only: baseline, label ensure,
checkpoint bootstrap, checkpoint parse, and the empty-diff stop. Steps 4–8
(reviewer dispatch, triage, dedup, autonomous filing, checkpoint advance)
remain covered by the offline evals alone, because no commit landed on `main`
during the session to produce a non-empty range. The skill is **not**
end-to-end verified against live data, and no attempt was made to manufacture a
diff to claim otherwise. The first real merge onto `main` after this PR lands
will be the first genuine exercise of steps 4–8.

## Governance defect found and reported separately: #356

While verifying the D-068 reviewer binding before dispatch, the pinned reviewer
turned out to be **verified but not actually bound**. `docs/AGENT_TOOLING.md`
pins iEvo `v0.58.1` and records both artifact digests; those digests are correct
and `scripts/check-claude-marketplace.sh` passes — but it validates a copy it
installs into an isolated temporary config directory, while a real session
dispatches `v0.78.8` from `~/.claude/plugins/marketplaces/ievo-skills`. Root
cause is a marketplace-name collision: the machine-local `~/.claude/settings.json`
defines the same name `ievo-skills` as a plain GitHub source with
`autoUpdate: true`, and that definition wins over this repository's pinned
`git-subdir` definition.

Confirmed two independent ways at runtime: a direct probe (the dispatched
reviewer answered that its instruction set defines `coverage_caveats`, a field
introduced only in 0.78.8) and an output-template tell (a later round emitted a
`### Coverage` section, which exists only in 0.78.8's mandated report format).

This is a supply-chain control failure, not a review-quality one — 0.78.8 is a
strict superset of 0.58.1's reviewer (same 11 points, same `Read`/`Grep`-only
tool policy, plus untracked-file handling, secret-redaction-on-quote, and
markdown-injection containment), so no review performed under it was weaker.
But the project cannot currently claim its reviewer is pinned, and the drift
recurs on every upstream release because the winning definition auto-updates.

It is pre-existing and machine-local — it predates this branch, affects every
D-068 review run on this machine, and cannot be fixed from inside the
repository (the fix is in user-level settings). Filed as its own issue rather
than folded into this PR.

## What is NOT done

- **[PR #357](https://github.com/rotnov/pycc/pull/357) is open and not merged.**
  That was the explicit endpoint for this work; the merge decision belongs to
  the maintainer.
- #356 is filed and untouched — no fix attempted.
- The skill schedules no recurrence of its own. Wiring an actual recurring
  trigger (Claude Code Routines or a Codex cron equivalent) is a separate setup
  step, deliberately outside the skill's definition.
- Authenticated model-response evals remain deferred for `ultra-review`, the
  same standing gap every other alpha skill in this repository carries.

## Where a fresh session should resume

Read [PR #357](https://github.com/rotnov/pycc/pull/357)'s own state first — it
is the only thing in flight. If it has
merged, the next `ultra-review` run will be the first with a non-empty range
and therefore the first real exercise of steps 4–8; treat its output
skeptically and verify every filed issue against the evidence bar before
trusting it. Checkpoint issue #355 must stay open — it is the skill's only
persistent state.
