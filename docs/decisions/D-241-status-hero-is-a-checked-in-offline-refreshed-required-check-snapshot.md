---
id: D-241
title: "Status hero is a checked-in, offline-refreshed required-check snapshot bound to one default-branch revision"
status: accepted
---

## D-241: Status hero is a checked-in, offline-refreshed required-check snapshot bound to one default-branch revision

- Status: accepted (Part 1 of #566, issue #1006, is the pull request that
  depends on it; it narrowly supersedes D-230's "Landing and the five
  unavailable heroes remain intact" consequence -- four heroes now remain
  `unavailable` -- and leaves everything else in D-230 in force)
- Context: D-186 made every public evidence hero a commit-bound, offline
  proof and left the Status page's hero explicitly `unavailable` until #566
  accepted a real artifact. The page is the one place the site claims the
  project's CI state, and the natural artifact is a live badge or a
  build-time provider fetch -- both of which D-186 rejects, because the
  Pages build runs on untrusted pull-request code and every merge moves
  `main`, so nothing fetched at build time is reproducible or reviewable.
  The required contexts on `main` are `ci-gate` (a post-merge workflow run
  on the merge commit, App id 15368) and `audit` (the D-172 workflow-policy
  check, which runs under `pull_request_target` on the pull request's head,
  never on the merge commit). A snapshot that names one default-branch
  revision therefore has three distinct subjects, one of which is a commit
  object a clean full-history checkout does not have.
- Decision: The Status hero is a checked-in, sanitized snapshot of the
  required checks for exactly one default-branch revision, refreshed
  offline by an agent and validated by the same hermetic gate as the other
  heroes. Concretely:
  - The `status` record in `site/evidence-heroes.json` (schema `2.1.0`) is
    the single source of truth; every projection (HTML hero, JSON-LD,
    Open Graph/X metadata, `site/index.html.md`, `site/llms.txt`) is
    validated against it. Its `snapshot.subjects` are three rows:
    `published-revision` (the merge commit on `main`, its tree, its parent
    count and the merged pull request's number, head SHA and head tree),
    `post-merge-ci-gate` (`ci-gate` on that commit) and `pre-merge-audit`
    (`audit` on the pull request head, because that is where the check
    runs). `environment.platforms` holds the five Tier-1 jobs of the same
    `ci-gate` run. The provenance fields the `*_OBSERVATIONS.json`
    convention carries (`collected_at`, `collection_method`, `sanitized`)
    live in the record's `attestation`, beside the roadmap milestone line
    at the subject and the required-context names.
  - Unknown is not green. `scripts/collect_status_snapshot.py` reads GitHub
    only through the read-only `gh api` endpoints (commits, pulls,
    check-runs, paginated), writes only the enumerated fields, and never
    writes `success` for an absent, incomplete, ambiguous or non-completed
    run: any such answer makes the state `unavailable`, and the collector
    then exits non-zero and leaves the manifest untouched. The validator
    rejects a non-`unavailable` record that carries any conclusion other
    than `success` where the state mapping requires it.
  - The subject side is proven from Git, the pull-request side from the
    record. `scripts/check_status_snapshot.py --verify-git` (run by
    `scripts/check-site.sh`, so in the full-history Pages checkout)
    requires the subject to be an ancestor of `HEAD`, to have exactly one
    parent, and to have the recorded tree; the association with the pull
    request head is then the record-internal equality
    `merged_pull_request.head_tree == repository.tree`, whose inputs the
    collector took from the provider at capture time. The validator never
    fetches `refs/pull/*`. A head-SHA swap with an identical tree is
    therefore undetectable offline; the record's `limitations` say so, and
    the collector is the trusted party for that one field, as D-230
    already trusts it for `tested_commit`.
  - Freshness is visible, not asserted. The subject SHA and the capture
    time are visible in the hero; the hero's milestone text is the
    `docs/ROADMAP.md` `**Current milestone:` line **at the subject
    commit**, proved from Git like the subject's tree, never against the
    working tree — a milestone-transition pull request therefore merges
    with the previous snapshot and the hero shows the prior milestone,
    labelled as the roadmap at that revision, until the next refresh (a
    window the currency bound below caps at 20 first-parent merges); the
    D-156/D-170 freshness gate keeps forcing a Status-page edit on every
    milestone-truth change; and the convention is that any pull request
    which edits `site/status/index.html` or the `status` record re-runs
    the collector against the then-current `origin/main` tip. That
    convention is backed by a diff-conditioned currency step on the Pages
    pull-request leg only (`check_status_snapshot.py --currency`), which
    requires the subject to be an ancestor of the base tip and at most 20
    first-parent merges behind it. It never runs on `push`, so it cannot
    turn `main` red on its own. Twenty is a reviewed constant: daily
    first-parent merge counts on `main` since 2026-08-25 range 2-27 with a
    median near 8, so 20 is roughly one to three days of merges and below
    the busiest observed day.
  - Required-CI coverage of the Git logic is shallow-safe.
    `scripts/test_check_status_snapshot.py` runs in the depth-1
    `governance` job against throwaway repositories; the real-record
    public-CLI controls live in the Pages-only
    `scripts/site_status_evidence_test.py`, which
    `scripts/test-check-site.sh` runs with full history.
- Alternatives: A live badge or a CI-time provider fetch -- rejected by
  D-186 (untrusted PR CI, non-reproducible). A second observation file
  `docs/STATUS_SNAPSHOT.json` -- rejected as duplication: the manifest is
  already the reviewed projection data, and a cross-file equality check
  adds no proof. Fetching `refs/pull/N/head` inside the validator to prove
  the head tree from Git -- rejected as a network dependency in the Pages
  build and a D-230 deviation. An unconditional wall-clock or
  commit-distance staleness gate -- rejected as a time bomb on `main`
  (the D-239 rollover reasoning); the diff-conditioned, PR-leg-only step
  fails exactly the pull requests that were supposed to refresh the
  snapshot. Requiring `subject == base` -- rejected as a rebase livelock
  under strict up-to-date branch protection, since each refresh moves the
  page's own pin. A `governance` step in `ci.yml` that shallow-fetches the
  two SHAs -- rejected: ancestry cannot be checked shallowly, and `ci.yml`
  edits are an audited workflow surface.
- Consequences: The Status page states a point in time ("main `<sha>`
  observed at `<UTC time>`"), never currency; later merges are not covered
  until the snapshot is refreshed, and the publishing commit is always a
  later descendant of the subject. Refreshing the snapshot is a
  same-pull-request obligation for any status-page or status-record edit,
  and the Pages PR leg fails when it is skipped for long enough. The
  collector and validator share one `expected_shape`/`derive_state`, so
  the collector cannot write a record the gate rejects; the record pins
  the collector's and the validator suite's canonical SHA-256, so editing
  either requires re-collecting. Schema `2.1.0` adds the status record shape and
  the `snapshot.subjects` form; D-186's landing adapter and D-230's
  execution heroes are unchanged. Four heroes (performance, architecture,
  comparison, provenance) remain explicitly `unavailable`; Part 2 of #566
  (#1007) owns the Architecture hero. No branch-protection, Pages
  settings, `ci.yml`, compiler-behavior or provider-account change is
  authorized by this decision.
