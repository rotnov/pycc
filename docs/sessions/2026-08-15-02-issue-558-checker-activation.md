# Issue #558 checker activation checkpoint

## Exact state

- Task branch: `codex/activate-ci-routing-checker`.
- Exact base and pre-commit head: merge commit
  `c79da70746a999886fd3ab7fd63bf8241f5f69c3` for Stage 1 pull request
  [#559](https://github.com/rotnov/pycc/pull/559).
- Issue [#558](https://github.com/rotnov/pycc/issues/558) is open, assigned to
  `rotnov`, and labeled `enhancement` plus `in progress`. GitHub closed it
  automatically when #559 merged despite the non-closing PR wording; it was
  immediately reopened because the three-stage activation is incomplete.
- Branch protection again exactly requires `audit` and `ci-gate`, with strict
  up-to-date checks, administrator enforcement, resolved conversations, and
  force pushes/deletions disabled. An unrelated invalid D-125 relaxation for
  PR #560 was restored from incident
  [#561](https://github.com/rotnov/pycc/issues/561); the incident is closed and
  an independent Gate 2 comparison returned `MATCH` for every protected field.

## Stage 2 candidate

- `scripts/check_roadmap_evidence.rb` is byte-identical to the base-owned
  `check_roadmap_evidence-d171.rb` successor, SHA-256
  `70b2b286cc022435f481f1fba6585204f30148b4a2aba139e20aaf7c02292f2d`.
- `scripts/test_check_roadmap_evidence.rb` is byte-identical to the base-owned
  `test_check_roadmap_evidence-d171.rb` successor, SHA-256
  `cf9a6578042bf7ce07cf475233cdbd14b8a98b3dd23d551f95ba8ac9aa1c97cc`.
- Those two manifest entries now self-source. The still-byte-unchanged live
  `.github/workflows/ci.yml` target is staged from base-owned `ci-d171.yml`,
  SHA-256
  `785e6415aea979ac67563589fb8295f2d7a9ce991f11d269d2054422a527e33f`.
- No live workflow or standalone agent workflow changes in this stage.
  `docs/TESTING.md` describes the checker as active and CI as staged.

## Verification

- `cargo doc --workspace --no-deps`, workspace/cross-target builds, release
  runtime build, workspace tests, clippy with `-D warnings`, and the 75 ms
  frontend-throughput check passed. The workspace run filtered only the known
  base-identical deleted-current-directory test that hangs on this macOS host;
  36 CPython 3.14.7 conformance tests and the slow nbody benchmark remained
  ignored by the default suite.
- CI-permission policy: 42 runs / 155 assertions; live D-171 roadmap policy:
  213 runs / 1,054 assertions; both production checkers passed.
- Python discovery: 852 passed / 6 skipped; focused classifier: 20 passed;
  agent policy/assets and both offline alpha client contracts passed.
- README coverage, replicated performance policy, monitoring shell tests,
  Codex/Claude marketplace validation, actionlint, safe workflow YAML parsing,
  complete manifest digests, and `git diff --check` passed.
- Two fresh independent deep-review dispatches made no progress for three
  bounded waits each and were interrupted per `AGENTS.md`. The documented
  inline 11-point fallback found no actionable issue in the exact staged diff.
- The exact isolated 100% coverage command still needs passwordless
  `sudo -u nobody`, unavailable on this host. CPython 3.14.7 oracle execution,
  other Tier-1 runners, Intel-native cross execution, paired artifact
  performance, Lighthouse, and the aggregate `ci-gate` remain authoritative
  CI-only checks.

## Concurrent work and next step

- Open PR #518 remains unrelated to this protected sequence.
- Open PR [#560](https://github.com/rotnov/pycc/pull/560) is now conflicting at
  head `946839e0024c80300786c9bf57f76274a5749541`; it overlaps the protected CI,
  checker, manifest, testing docs, and D-171 numbering. Its `audit` is failing
  and six P1 review threads remain unresolved. It must rebase, renumber its
  decisions, fix those findings, and use the post-#558 D-103 state rather than
  bypass its own candidate failure.
- After this checker-activation PR merges, start a fresh branch from its exact
  merge commit, copy only `ci-d171.yml` into the live CI path, return the CI
  manifest entry to self-source, remove only duplicate broad discovery from
  the standalone agent workflows, update active-state documentation, and
  deliver final Stage 3 with `Fixes #558`.
