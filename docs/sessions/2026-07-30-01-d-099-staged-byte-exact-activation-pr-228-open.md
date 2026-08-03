# 2026-07-30 — D-099 staged; byte-exact activation PR #228 open

**Authoritative checkpoint:** refreshed default `main` is
[`b62f539e0655997d2e33c7b779d186f58df76d43`](https://github.com/rotnov/pycc/commit/b62f539e0655997d2e33c7b779d186f58df76d43),
the squash merge of staging
[PR #227](https://github.com/rotnov/pycc/pull/227). That commit adds the
reviewed D-099 workflow fixture and digest without changing live CI. Draft
activation [PR #228](https://github.com/rotnov/pycc/pull/228) is `OPEN` at
head `d1f7d7ef8a1f1d16afd4f6526fa9418c3b418330`, exact base `b62f539`,
`MERGEABLE`, and has no review threads. Its base-owned `audit`,
`agent-policy`, and `agent-assets` checks are successful; the remaining
required CI jobs are still running. This is a point-in-time snapshot before
this session-log commit advances the same pull request head.

**Activation scope:** PR #228 copies
`tests/fixtures/d99-vcpkg-libxml2-cache-ci.yml` byte-for-byte into live
`.github/workflows/ci.yml`, makes D-099 the sole publicly authorized
whole-workflow digest, and retains D-084 plus pre-D-099 D-091 only as rejected
audit evidence. D-062's fixed-replicate performance-job content remains
byte-identical inside D-099. Pull requests can restore the Windows vcpkg
binary archive but cannot publish it; only an exact-key miss on a trusted
`main` push can save. Local evidence includes `actionlint`, exact fixture/live
SHA-256 equality, 128 roadmap-policy tests (537 assertions), 33 permission
tests (87 assertions), the public policy checkers, `cargo doc`, and the full
workspace test suite. The immutable pinned iEvo reviewer reached a clean
11-point verdict after every finding was fixed.

**Adjacent live state:** v0.2 PR-8
[PR #188](https://github.com/rotnov/pycc/pull/188) remains `OPEN` at head
`d66982a0b4d615db6b37da197129254f67fbb1a0` but is now `CONFLICTING`/
`DIRTY` against the advanced default branch. Its pre-D-099 D-091 workflow
must not replace the activated cache: before PR-8 can resume, it needs a
separately staged and reviewed D-091+D-099 composed digest. Issue #225 does
not authorize modifying that PR-8 branch, so this task records the boundary
without resolving its conflict.

**Next:** push this log checkpoint, wait for PR #228's exact new head to pass
`audit`, hard 100% coverage, the Tier-1 matrix, performance gate, and aggregate
`ci-gate`, then mark it ready and merge through protected `main`. Inspect the
exact post-merge Windows job to prove the trusted save ran, then inspect a
later exact-key Windows run for a real cache hit and the resulting libxml2/job
duration before declaring #225 complete.
