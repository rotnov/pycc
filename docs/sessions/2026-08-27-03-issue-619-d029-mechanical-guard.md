# 2026-08-27-03: Issue #619 — automate D-029's LLVMString guard

## Status

Delivered. This session's autopilot dispatch ran the `issue-select` skill
from a clean baseline (`origin/main` at `2817b1a6b76e1f9ea646b6e32587936096941eaf`,
the same commit `docs/sessions/2026-08-27-02-issue-246-tier1-target-parity.md`
recorded), selected issue #619 as the next candidate, and delivered it
through `issue-implement`'s pipeline in this same session.

## What #619 was

"P2: Guard D-029's LLVMString rule mechanically instead of in ADR prose."
D-029 already had a mechanical test
(`crates/pycc_codegen/src/tests.rs::every_inkwell_llvm_string_call_routes_through_a_d029_wrapper`,
added while resolving `.harden/incidents/platform-wrapper-bypassed-by-new-code/incident.md`)
that scans the crate's own sources for three D-029 protections. What was
missing: the checking logic itself had never been exercised against a
deliberate violator except by a one-time hand-run proof, pasted into the
incident file as a shell transcript (`verify: manual`, `Fixture: None`).
Nothing would catch a future edit to the checking logic silently losing
its ability to recognize a real violation.

## What changed

- `crates/pycc_codegen/src/tests.rs`: extracted the three checks into a
  pure function `d029_violations(sources: &[(&str, &str)],
  expected_triple_call_sites: usize) -> Vec<String>` — same needles, same
  conditions, same messages as the pre-extraction inline `assert_eq!`
  calls (verified via `git diff`, not just re-derived by hand). The real
  test now calls it against the crate's actual sources and asserts the
  result is empty.
- `crates/pycc_codegen/src/tests/d029_guard.rs` (new file): after the PR
  was opened, GitHub's automated review flagged that adding the
  extracted function plus ~130 lines of fixtures directly to `tests.rs`
  (already 14,720+ lines) violated AGENTS.md's decomposability rule,
  which requires decomposing the touched part into a cohesion-driven
  submodule rather than growing an already far-over-threshold file
  further. Moved `d029_violations` and the new `d029_violations_tests`
  module into this dedicated submodule (`mod d029_guard;` declared in
  `tests.rs`, `pub(super)` visibility on the function); the guard test
  itself stays in `tests.rs` alongside this crate's other `#[test]`
  functions and calls `d029_guard::d029_violations`. `d029_violations_tests`
  has seven tests: a compliant baseline, the five cases (A bare verify
  call, B unwrapped printer call, C — the same A-shaped verify violation
  planted in a second source entry, proving the scan isn't limited to
  one file, D a new triple call site, E a correctly wrapped call on a
  different receiver) previously proven only by hand in the incident
  file, plus one proving the tripwire count is caller-supplied rather
  than hardcoded.
- `docs/decisions/D-029-llvm-s-llvmdisposemessage-crashes-on-windows-for.md`:
  Consequences paragraph now points at the guard test and at #619's
  automated proof by name, instead of ending in prose-only obligation
  language. After the same automated review round flagged that the
  paragraph's "no longer prose-only" claim overclaimed coverage of "any
  future `LLVMString`-returning API" when the guard only recognizes the
  three specific, already-used APIs by call spelling, narrowed the
  wording to scope the claim to those three APIs and state explicitly
  that a not-yet-used `LLVMString`-returning API would still evade the
  guard and remains a prose-only obligation.
- `.harden/incidents/platform-wrapper-bypassed-by-new-code/incident.md`:
  added an "Update (2026-08-27): automated (#619)" section recording that
  `Fixture: None` and `verify: manual` are both now false — the fixture
  and automated verify path exist as of this change — while keeping the
  original hand-run transcript as the historical record of the first
  proof. Forward-pointed the original `## Verify` section to the update
  so a reader stopping there isn't misled. Updated to reference the
  guard's final `tests::d029_guard::d029_violations`/
  `tests::d029_guard::d029_violations_tests` location after the
  decomposition fix above.

## Verification

- `cargo test -p pycc_codegen d029` — 8/8 passing (the original guard
  test plus the 7 new `d029_violations_tests`).
- `cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`
  — 100.00%/100.00% (47141 regions, 30457 lines, zero missed), matching
  exactly the invocation CI's coverage job runs. A per-crate
  (`-p pycc_codegen`) run shows a pre-existing 1-region gap in
  `bigint_rc.rs` unrelated to this change (present identically on the
  base commit before this PR) that another workspace test binary covers
  at the workspace level; not a regression.
- `cargo doc -p pycc_codegen --no-deps` — clean.
- `python3 scripts/generate_decisions_index.py docs/decisions docs/decisions/README.md --check`
  — up to date, no regeneration needed (content-only ADR edit).
- `ruby scripts/check_roadmap_evidence.rb` — passes (unrelated to this
  change; ROADMAP.md was not touched). Note: this sandbox has `LANG`/
  `LC_ALL` unset, which makes the script fail with an unrelated
  `invalid byte sequence in US-ASCII` Ruby encoding error; reproduced
  identically on the pre-change base commit, so it is a pre-existing
  sandbox-locale issue, not something this PR introduced. Setting
  `LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8` makes it pass cleanly.
- D-068 pinned reviewer (`ievo:deep-reviewer`): ran once before opening
  the PR. Findings: no P0/P1s. Two P2s — (1) doc-drift in the
  `d029_violations_tests` module comment, whose case-C fixture had
  actually used case B's violation shape (unwrapped printer call)
  instead of case A's (bare verify call), contradicting both the
  incident file's own case-C transcript and the module comment's claim
  of A-E letter correspondence; fixed by changing the fixture to plant
  an unwrapped verify call, matching the incident file exactly — (2) no
  session-log entry existed yet for this work; this file is that fix.
  One optional/non-blocking note: the incident file's original `##
  Verify` section could mislead a reader who stops there before the
  later `## Update`; added a one-line forward-pointer. The reviewer
  could not itself run `git diff` (no Bash in its isolated context) to
  confirm the extraction preserved the original checks' conditions
  verbatim, so this session confirmed that directly via `git diff
  crates/pycc_codegen/src/tests.rs` before treating the finding set as
  final — the diff shows only the assert-to-push/pure-function
  restructuring, with identical needles, conditions, and messages.
  Given the fixes were small and mechanical (one fixture snippet change,
  one docs addition, one new file), the reviewer was not re-run a second
  time; `cargo test -p pycc_codegen d029` was re-run after the case-C fix
  and still passes 8/8.
- GitHub's own automated review on the opened PR (#826) raised two
  further findings that blocked `mergeStateStatus` (unresolved
  conversation threads) despite every CI check being green: a P1 for the
  AGENTS.md decomposability violation and a P2 for the ADR's overclaimed
  guard coverage, both described above under "What changed" and both
  fixed in this same PR before merge. `cargo test -p pycc_codegen d029`
  (8/8) and `cargo llvm-cov --workspace --fail-under-lines 100
  --fail-under-regions 100` (100.00%/100.00%) were re-run after these
  fixes and both still pass.

## Issue-select context (for the next session)

Selected #619 over the following same-priority (P2, v0.4-scoped)
survivors after the milestone-triage/staleness/blocker screens found
nothing to reassign or close: #824 (has open "decide whether..." design
questions, resolvable but not as clean a single-session scope), #618
(moderate-size new diagnostic across ~12 call sites), #636 (explicitly
blocked pending D-124's refcounting model landing), #638 (architecturally
consequential exception-unwinding work needing a plan), #24 and #614/#712
(D-080-deprioritized: all three touch the manifest-protected `ci.yml`,
forcing the two-PR stage-then-activate cycle onto what would otherwise be
a single-PR task), #573/#585/#676/#693/#707/#714 (substantive
semantics/runtime feature work, all larger in scope than #619), and #733
(a meta-issue about issue-select's own scoring whose cited v0.3 evidence
went stale once v0.3 completed on 2026-08-26 — recorded as stale-premise,
not closed, since its general claim is otherwise undisproven). #619 was
the smallest, fully unblocked, most mechanically-scoped v0.4 P2 candidate
requiring no maintainer judgment call. D-192's non-milestone ceiling (72
open non-milestone issues against a cap of 20) is in force and blocks new
non-milestone filings, but does not affect this milestone-scoped pick.

No `docs/ROADMAP.md` or `docs/DELIVERY_PLAN.md` changes were needed — this
PR moves no acceptance criterion and changes no milestone sequencing.

## Next steps for a fresh session

Loop back to `issue-select` for the next v0.4 candidate. #20 (the only P1)
stays legitimately blocked on #631's `ci.yml` two-PR digest cycle; #631
itself needs that same D-080 workflow before either can proceed.
