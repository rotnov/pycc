---
id: D-046
title: "Only successful main pushes publish the frontend-performance baseline"
status: superseded
---

## D-046: Only successful main pushes publish the frontend-performance baseline

- Status: accepted (PR-4 Codex review correction; supersedes D-042's per-run publication lifecycle)
- Context: GitHub Actions caches are scoped by ref. A `pull_request` run writes to its merge ref, and later runs of that pull request search that scope before the default branch. Publishing a baseline from every successful PR head would therefore make the next head compare with the same PR's previous head. Several individually sub-2% regressions could ratchet beyond 2% relative to the last merged code without any one comparison failing.
- Decision: split the gate into `frontend-perf-measure` and `frontend-perf-gate`. The measurement job executes PR code and uploads only Criterion's `estimates.json`. The gate job has no source build: it sparse-checks out only the Ruby comparator and its tests, verifies their reviewed SHA-256 digests, restores the `frontend-perf-main-v1-` namespace, downloads the measurement as untrusted JSON data, self-tests the comparator, and applies the threshold. The new namespace deliberately excludes every baseline written by the earlier PR-local lifecycle. Pull requests never promote or save; only a successful `push` on `refs/heads/main` may publish the next canonical baseline. Both jobs are explicit `ci-gate` dependencies. The first run before any canonical baseline exists remains the one permitted bootstrap: the PR reports no baseline, and the successful post-merge `main` run publishes it.
- Alternatives: let each PR head publish and compare incrementally (rejected because it weakens the cumulative merge threshold); use a privileged PR workflow to overwrite the `main` cache (rejected because low-trust PR code must not write trusted shared state); store baselines as repository artifacts or commits (deferred because the main-scoped cache already supplies the required canonical comparison without write credentials).
- Consequences: repeated updates to one PR always compare against the latest successfully published `main` measurement rather than a PR-local predecessor. Untrusted compiler/build code cannot modify the comparator that interprets its measurement. A failed comparison cannot advance either PR-local or canonical state. If the post-merge `main` run fails or is cancelled, it leaves the previous canonical baseline intact and the failure remains a release-blocking signal.

