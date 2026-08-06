---
id: D-049
title: "Performance-transition tests use an inert pre-split workflow fixture"
status: accepted
---

## D-049: Performance-transition tests use an inert pre-split workflow fixture

- Status: accepted (corrects D-048's transition-fixture inventory; D-048's activation semantics and three-phase lifecycle are unchanged)
- Context: D-048 checked in byte-exact activation and steady-state fixtures, but its shell-level bootstrap tests simulated the predecessor-workflow API response by reading the live `.github/workflows/ci.yml`. Those tests passed only while the pre-split workflow was active. Replacing the live file byte-for-byte with the activation fixture made the same tests feed the activation digest back as its own predecessor, so the three legitimate bootstrap cases failed before the activation pull request could reach CI. A transition test cannot use the state it is designed to replace as its supposedly historical input.
- Decision: preserve the trusted pre-split workflow bytes as inert `tests/fixtures/d48-pre-split-ci.yml`, bind that file's SHA-256 to `PRE_SPLIT_PERF_CI_WORKFLOW_SHA256`, and make the bootstrap test helper use it by default. Explicit negative tests may still supply a different file to prove that an activation or unrelated predecessor fails closed. The active workflow remains unchanged in this corrective staging change.
- Alternatives: derive the predecessor from the live workflow (rejected because activation changes that state); reconstruct a minimal synthetic workflow with the same digest (impossible because the proof is over exact bytes); read an old Git commit during tests (rejected because it adds Git history and network/repository-shape dependencies to a hermetic unit test); weaken the predecessor check (rejected because the exact trusted digest is the activation boundary).
- Consequences: the corrective staging tests pass both before and after the activation replacement, and the fixture proves exactly which retired bytes the bootstrap accepts. The pre-split fixture, its digest, and the activation-only shell tests are transitional evidence: the D-048 cleanup removes them together with every bootstrap branch after the first exact-main artifact exists.

