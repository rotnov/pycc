# Incident: a review criterion stated as a diff against a base revision

**Date:** 2026-09-04
**Topic:** reviewer-cannot-decide-a-base-diff-criterion
**Verdict:** pending — open, no artefact built
**Source:** `/harden batch .harden/findings/issue-923.jsonl`, finding 4

## Symptom

The #923 review brief asked the pinned reviewer to confirm that `docs/ROADMAP.md:41` was
unchanged byte-for-byte against the base revision. `ievo:deep-reviewer` has Read and
Grep and no Bash or git, so it cannot read a base revision at all. It behaved correctly:
it reported the claim as circumstantial rather than asserting it, and the dispatching
session closed it directly — `git show 12650781:docs/ROADMAP.md | sed -n 41p` and the
worktree copy both hash to
`547d6b9366dff12ae7cda69af74ee37247d118d265f823fa939db86371796510`, and no diff hunk
covers line 41.

## Gap type

**Content**, at the dispatch fork — in the brief, not at the reviewer. Any criterion
phrased as a comparison against a base revision is structurally unverifiable by a
reviewer with no revision access, and must be pre-resolved by the dispatching session.

## Why nothing is built

**The upstream route is closed by project policy, not by cost.** AGENTS.md's pinned
review loop (D-068 / D-155) requires loading the reviewer only from a structurally
verified `ievo@ievo-skills` install confirmed by
`scripts/check_claude_reviewer_binding.py`, and explicitly forbids substituting a
reviewer definition from the branch, index, or working tree. A local fork granting the
reviewer Bash or git would break that binding and weaken the gate it protects.

**The dispatching-side artefact is reachable and declined at this occurrence.** Stated
concretely for whoever picks this up: *when a review brief states a criterion as
"unchanged since base", the dispatching session pre-resolves it and hands the result in
as a stated fact, or drops the criterion from the brief* — a Local-skill termination,
genuinely buildable. Declined at one occurrence of `info` severity: the reviewer
self-labelled the claim circumstantial exactly as designed, and the orchestrator closed
it with a single `git show | sha256` pair. The system already produced the cheapest
possible outcome.

Revisit if a base-diff criterion is ever *closed as satisfied* on the reviewer's
circumstantial word.

## Adjacent topics, checked and distinct

- `reviewer-hypothesis-refuted-on-verification` (5 files) — there the reviewer asserted
  and was wrong; here it correctly declined to assert.
- `subagent-fabricated-evidence` (3 files) — the mirror image: that topic is an agent
  narrating an inspection it did not perform. This is the opposite behaviour, and is
  that topic's artefact working.

**fixture:** n/a — no artefact built
**artifact:** none — counter seed, verdict pending
**verify:** n/a — no artefact built
