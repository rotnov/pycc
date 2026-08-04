# 2026-07-27 — v0.1's acceptance checklist is fully green; PR-7 (final v0.1 buffer slice) complete

**Status:** all five bullets in `docs/ROADMAP.md`'s "v0.1 acceptance
checklist" are now `[x]`, each with a valid `roadmap-evidence` marker. Per
`docs/ROADMAP.md`'s own binary milestone definition ("a milestone isn't done
until they're green on all Tier-1 platforms"), **v0.1 is complete** — this is
the final PR (`docs/DELIVERY_PLAN.md`'s PR-7 "buffer: close whatever's left"
row) in the v0.1 delivery plan's PR-1 through PR-7 sequence. Verified default
branch commit at this checkpoint:
[`611c4a5`](https://github.com/rotnov/pycc/commit/611c4a523cef555dc68da133ae07fb17ee5ee302)
(merge of [PR #176](https://github.com/rotnov/pycc/pull/176)).

**What shipped this session (PR-6 was already recorded merged at `a21918d`
in a prior entry; this entry covers PR-7 only):**

- [PR #175](https://github.com/rotnov/pycc/pull/175) ("PR-7a", merged at
  `22c522d`): registered three new `roadmap-evidence` identifiers in
  `scripts/check_roadmap_evidence.rb` --
  `conformance-fib-mandelbrot-tier1`, `check-throughput-1k-loc-50ms`,
  `cli-spec-diagnostic-match` -- with failing public-CLI mutation tests
  added first, per `AGENTS.md`'s requirement. Deliberately did not check any
  `docs/ROADMAP.md` box: `.github/workflows/workflow-policy.yml`'s `audit`
  job always runs the *base* branch's copy of the checker under
  `pull_request_target`, so a single PR that both registers a new ID and
  cites it in a checked box can never pass its own audit. `docs/TESTING.md`
  now documents this stage/activate split as a general rule for adding any
  new evidence identifier, not just a one-off for this PR. Two review
  findings were resolved before merge through two different outcomes: a
  genuine contradiction between two adjacent `docs/TESTING.md` sentences was
  fixed; a second finding -- a gap where the ci.yml digest proves invocation
  but not the content of the files it invokes -- was adjudicated via a
  reasoned review-thread reply rather than fixed with code (see below).
- [PR #176](https://github.com/rotnov/pycc/pull/176) ("PR-7b", merged at
  `611c4a5`): checked the three remaining boxes citing those IDs, and swept
  every human- and LLM-readable project surface (`docs/ROADMAP.md`'s
  "Current milestone" line, `README.md`'s status blurb, and five `site/`
  pages plus their hardcoded validator assertions in `scripts/check-site.sh`/
  `scripts/test-check-site.sh`) to stop describing "final v0.1 acceptance" as
  pending. Two review findings were fixed before merge, both overclaim/
  staleness bugs in the site copy (one wrongly generalized a "verified on
  all five Tier-1 targets" qualifier to a claim that isn't proven on all
  five; one left a page's `<meta name="description">`/JSON-LD description
  stale after its visible body text changed).

**A design gap was found and deliberately not fixed in PR-7a**, adjudicated
via review-thread reply rather than code: the three new evidence IDs'
underlying claims are proven only by CI *invoking* the right test/script
paths (via the existing `ci.yml` digest pin, for two of the three), not by
verifying those files' *content* still asserts real behavior. A future PR
could silently gut `tests/conformance.rs`'s assertions or weaken
`scripts/check_frontend_throughput.rb` without tripping anything. The
correct fix — embedding `shasum`/diff steps inside `ci.yml` itself,
mirroring the existing `PAIRED_PERF_CHECKER_SHA256` pattern — needs its own
`ci.yml` stage-then-activate digest cycle and was out of scope for a
hash-registration PR; it's tracked as a standalone follow-up (see the
autonomous background-task chip raised this session, "Content-pin
roadmap-evidence's backing test/script files"). See
`docs/AGENT_RETROSPECTIVE.md`'s 2026-07-27 entry for why a broader version of
this fix (reading those files directly from `check_roadmap_evidence.rb`'s
`root` argument) was rejected: `workflow-policy.yml`'s `audit` job only
provisions `docs/ROADMAP.md` and `.github/workflows/*.yml` into its sandbox,
so any check reading another path would break the audit for every future PR,
not just the one adding it.

**What's next:** v0.1's acceptance checklist does not include cutting an
actual release tag -- `docs/ROADMAP.md`'s Distribution row and
`docs/DISTRIBUTION.md` track "Tier-1 installation evidence plus a release
tag" as a separate, still-open concern, independent of milestone completion.
The next roadmap-level work is v0.2 ("collections & generics", see
`docs/ROADMAP.md`'s v0.2 section and `docs/DELIVERY_PLAN.md`'s milestone
table) -- unentered as of this checkpoint; its own brainstorm/plan cycle has
not started. Two other pending follow-up task chips from this session remain
open and untouched: pinning `actions/setup-python` in `ci.yml` to a commit
SHA, and fixing a locale-dependent crash in
`scripts/check_roadmap_evidence.rb`/its test suite under a POSIX/C locale
(both pre-existing or previously-flagged, neither blocking v0.1).
