# 2026-08-22-02 — Part 2 of #541 implemented, PR #715 opened

## Overall status

Issue [#702](https://github.com/rotnov/pycc/issues/702) ("Part 2 of #541:
raising and catching user exception classes") is implemented and delivered
as [PR #715](https://github.com/rotnov/pycc/pull/715), open against `main`
and not merged. The branch is `feat/702-raise-catch-user-exceptions`,
based on `60315e3b` (the `origin/main` tip at the time this task started),
six commits, head `2979e57b`. Its verified `closingIssuesReferences` is
`totalCount: 1`, naming exactly #702 — #541 stays open for Part 3
([#703](https://github.com/rotnov/pycc/issues/703)).

## What landed on the branch

- `crates/pycc_hir` — `HirClassDef::exception_type_tag: Option<u8>`;
  source-order tag assignment from `7..=255` for every user class whose MRO
  reaches a builtin exception class; `FIRST_USER_EXCEPTION_TYPE_TAG` and
  `MAX_USER_EXCEPTION_CLASSES` (249); `C0001` past the cap.
- `crates/pycc_types` — `check_raise_operand` accepts a user exception
  class **structurally**, on the `HirExpr::Call` shape. The pre-existing
  `matches!(&ty, Ty::Instance(..))` predicate is byte-identical to base and
  must stay that way (see D-189's rejected alternative). `except` accepts
  user classes; `as` bindings on them and own-`__init__` classes are
  `C0001`.
- `crates/pycc_mir` — `MirExceptionValue::Constructed` carries the tag and
  class name; `MirExceptHandler::exc_type_tag` is a sorted `Option<Vec<u8>>`
  computed by `handler_type_tags`.
- `crates/pycc_rt` — `PyExceptionObj` carries `name`/`name_len`; the
  tag-to-name `match` is gone; `pycc_rt_exception_alloc` and
  `raise_builtin` widened.
- `crates/pycc_codegen` — name constant emission and an `or`-chain over the
  handler's tag set.
- Docs — `docs/decisions/D-189-*.md` (accepted), regenerated decisions
  index, `docs/RUNTIME.md`, `docs/TYPE_SYSTEM.md`, `docs/ROADMAP.md`'s
  exception entry (the conformance-count headline at line 183 is
  untouched), and an `docs/AGENT_RETROSPECTIVE.md` entry.
- Tests — `tests/issue_702_user_exceptions.rs` (14),
  `crates/pycc_hir/src/exception/tag_tests.rs` (7),
  `crates/pycc_types/src/exception/user_class_tests.rs` (13), new MIR and
  codegen cases, and a three-class hierarchy appended to
  `tests/fixtures/pep_3110_exceptions.py` (verified byte-identical to
  CPython 3.14).

## In flight

CI on PR #715. At the time this entry was written the run
([32546976318](https://github.com/rotnov/pycc/actions/runs/32546976318) and
its siblings) had `classify-changes` and `status-page-freshness` green and
everything else pending. The PR is not to be merged by the session that
opened it.

Locally every gate was run capturing its own exit status: coverage 100.00%
lines / 100.00% regions (rc=0), clippy rc=0, fmt rc=0, `cargo doc` rc=0,
`check_conformance_breadth.py` rc=0 (32 evidence-backed rows, unchanged),
`cargo bench --bench check_bench` rc=0 with "No change in performance
detected". The full test run has 49 failures, all the local CPython-oracle
version mismatch (`found "Python 3.14.6"`), which reproduce identically at
the untouched base.

## Known follow-ups

- [#703](https://github.com/rotnov/pycc/issues/703) — Part 3 of #541.
  Materializing a real exception instance is what unblocks `except ... as
  e` on a user class, a raisable class with its own `__init__`, and
  `raise <bound value>`. All three are `C0001`/`T0021` today by design.
- [#704](https://github.com/rotnov/pycc/issues/704) (retargeted, P2) —
  partially shadowing a builtin exception name still panics in
  `crates/pycc_types/src/class/binding.rs`. Part 2 does not close it; the
  original repro now reports `T0044`, so the issue was narrowed rather than
  a duplicate filed.
- [#714](https://github.com/rotnov/pycc/issues/714) (P2) — binding a user
  exception subclass as a value compiles and then aborts at runtime with
  `NameError: 'Exception.__init__'`. Distinct from #711, which is a
  compile-time panic.
- No conformance matrix row flips in #715. Once its fixture is observed
  green on a completed `main` run across all five Tier-1 targets in both
  profiles, D-102's hand-flip path applies to PEP 3110's row, and
  `docs/ROADMAP.md`'s conformance headline moves with it — not before.

## Where to resume

Read `docs/decisions/D-189-assign-user-exception-classes-a-compile-time.md`
first; it carries the memory-safety reasoning that constrains any further
work on this path. Then `crates/pycc_types/src/exception.rs` for the
structural raise gate, and `crates/pycc_mir/src/exception.rs` for the tag
lookup and handler tag sets.
