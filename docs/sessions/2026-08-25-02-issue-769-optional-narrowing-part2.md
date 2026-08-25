# Session handoff: issue #769 (D-199, Part 2 of #747) — Optional[int] narrowing

- Status: implementation complete, all local gates green, branch pushed and
  PR opened against `main`. **Not merged** — the D-068 pinned local
  reviewer (`ievo:deep-reviewer`) could not be dispatched from this
  dispatched-subagent session (see below); merge is left for a session that
  can actually run `/ievo:deep-review`, per the process lesson already
  logged for issue #763/PR #770 (`docs/AGENT_RETROSPECTIVE.md`,
  "2026-08-24 — A dispatched subagent cannot satisfy D-068's local-reviewer
  dispatch requirement").
- Branch: `feat/issue-769-optional-narrowing`, based on `origin/main` at
  `7b3c4301`. Worktree: `/Users/denis/projects/pycc-worktrees/issue-769-optional-narrowing`.
- What shipped: flow-sensitive `Optional[int]` narrowing on a top-level
  `if name is None:` / `if name is not None:` test —
  - `pycc_hir::optional_none_test` / `pycc_hir::definitely_terminates`: a
    shared, environment-independent recognizer and a strict terminator
    predicate, consumed directly by `pycc_mir` (which cannot depend on
    `pycc_types`) and re-exported thinly by `pycc_types::narrow`.
  - `pycc_types`: overlay-based narrowing state on `Environment` (clone/
    discard join semantics, not mutating), applied at both `if` bodies and
    the early-return-narrows-the-continuation shape.
  - `pycc_mir`: a `$narrowed:{name}` scope-sentinel on the existing
    `scopes` stack, plus `narrowing_snapshot`/`restore_narrowing`/
    `lower_scoped_body` to recreate the checker's clone-and-discard
    semantics on MIR's single shared scope frame — this closes a
    narrowing-leak-past-nested-body soundness gap the original plan's MIR
    design did not fully specify (a deviation from the dispatched design,
    made and flagged per the dispatch's own "resolve autonomously and flag
    it" instruction).
  - `MirExpr::OptionalUnwrap` (read-side counterpart of `OptionalWrap`),
    lowered to a single borrowed `build_extract_value` in codegen — no
    retain at the unwrap site itself; the existing
    `retain_if_int_duplicate`/`int_value_is_a_duplicate_reference`
    duplicate-reference classification in `bigint_rc.rs` gained an
    `OptionalUnwrap` arm so a bigint payload duplicated out of a narrowed
    binding is still correctly refcounted.
  - `docs/decisions/D-199-optional-t-flow-sensitive-narrowing-part2.md`,
    `docs/TYPE_SYSTEM.md`, `docs/ROADMAP.md`, and
    `tests/fixtures/conformance-breadth-manifest.json` updated; the
    manifest's narrowing row flips to `proven` without promoting its
    existing `◐` subset marker.
  - Deliberately out of scope (documented in D-199/TYPE_SYSTEM.md/the
    manifest): compound conditions (`and`/`or`), narrowing to `None`
    itself, `raise` as a terminator alongside `return`, and any test more
    complex than a top-level `is`/`is not None` comparison.
- D-014 coverage gate — a genuine, now-resolved gap: after all functional
  work and unit/MIR tests, `cargo llvm-cov --workspace --fail-under-lines
  100 --fail-under-regions 100` initially failed at **1 missed region**
  (`crates/pycc_codegen/src/bigint_rc.rs:281`, the new `OptionalUnwrap`
  arm's `=> true`). Root-caused via `llvm-cov show` against the full
  67-object test-binary set (region-level misses do not appear in
  `--show-missing-lines`, and JSON exported with `report -p pycc_codegen`
  alone under-counts since Cargo compiles `pycc_codegen` with several
  distinct metadata hashes depending on which package links it): the one
  uncovered instantiation is shared by `issue_382_exceptions`,
  `slice1_codegen_depth`, and the `pycc` binary itself, and is only reached
  when a *bigint-valued* narrowed `Optional[int]` read is duplicated into a
  second binding (`retain_if_int_duplicate`'s call site is
  `MirStmt::Assign`, not a bare read). `tests/fixtures/pep_0604_union.py`
  exercises narrowing generally, but its own coverage of this exact
  scenario only reaches that instantiation through
  `tests/conformance.rs`'s CPython-oracle byte-for-byte comparison, which
  this environment skips (`ignored, requires a pinned python3.14 (CPython
  3.14.7) oracle on PATH`) — so the fixture edit alone did not close the
  gap here. Fixed by adding `tests/issue_769_optional_narrowing.rs`, an
  oracle-independent, self-contained integration test in the
  `tests/issue_770_optional_reassignment.rs` build-and-run pattern
  (bigint duplicate-binding, smallint mirror, and absent-optional cases),
  plus the same bigint duplicate-binding scenario in
  `pep_0604_union.py` for when the oracle *is* available. Final run: **0
  missed lines/regions/functions across 42836 regions**, `--fail-under-lines
  100 --fail-under-regions 100` exits 0.
- Exact commands run this session, with results:
  - `cargo test -p pycc_hir --lib` → 648 passed, 0 failed (mid-session
    checkpoint, before the full run below).
  - `cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions
    100 --no-clean` → exit 0 on the final run; full breakdown above.
  - `cargo test --workspace` → run separately (not just implied by the
    coverage build) per the coordinator's explicit request: 60
    `test result: ok` blocks, 0 failures anywhere, 0 filtered-out failures.
  - `ruby scripts/check_roadmap_evidence.rb` → "Roadmap evidence policy
    passed." (exit 0; a first bare invocation hit a `US-ASCII` encoding
    error from this shell's locale, not the script — re-ran with
    `LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8` and it passed cleanly).
  - `python3 scripts/generate_decisions_index.py docs/decisions
    docs/decisions/README.md --check` → "docs/decisions/README.md is up
    to date." (exit 0).
- D-068 local review gate — **not run, and could not be run from this
  session**: `Skill(skill: "ievo:deep-review")` refused outright
  (`disable-model-invocation: true`; the tool's own error text: *"Ask the
  user to run /ievo:deep-review themselves ... it is reserved for explicit
  user invocation"*), and this session has no `Task`/`Agent`-style
  dispatch tool to route around that block — the identical capability gap
  already logged for #763/PR #770. `check_claude_reviewer_binding.py`
  confirms the binding itself is structurally intact (`ievo@ievo-skills
  0.80.19 OK, 0.80.22 available`), so this is a dispatch-capability gap in
  this execution context, not a broken install. Per the established
  fallback: land everything else, state the outstanding gate at the top of
  the PR body, and leave `/ievo:deep-review` plus the merge itself for a
  session that can actually dispatch it.
- Environment note (transient, not a code defect): the host filesystem was
  at 96% capacity (~548 MiB free of 1.8 TiB) for part of this session,
  causing `StorageFull` failures in `tests/issue_146_bigint_release.rs`'s
  `peak_rss::*` tests during an early `cargo llvm-cov` attempt. Freed by
  `rm -rf target/debug` inside this worktree only (in-scope; `target/llvm-cov-target`
  was untouched and reused). Disk pressure eased further mid-session from
  ~548 MiB to tens of GiB free, consistent with the standing note about a
  concurrent background actor on this repository freeing space elsewhere;
  no other directory was touched.
- Scope note: five additional coverage-closing unit tests were added this
  session across `pycc_types/src/tests.rs`, `pycc_mir/src/tests/narrow.rs`,
  and `pycc_hir/src/tests.rs` for branches that real type-checked programs
  cannot reach (a wholly unbound narrowing target, an `is None` test on a
  non-Optional name, an early-return guard on a non-Optional name, a nested
  scoped body entered while already narrowed, and `definitely_terminates`'s
  full `&&`-chain truth table) but that the checker/MIR still code
  defensively — D-014's literal 100% bar caught these even though they are
  provably dead through any program that passes type-checking first.
- Next step for whoever picks this up: run `/ievo:deep-review` against the
  full committed range (merge base of `origin/main` through `HEAD` on
  `feat/issue-769-optional-narrowing`), address any actionable findings,
  then merge PR #769's pull request through the normal reviewed path once
  CI is green and D-068 is satisfied.
