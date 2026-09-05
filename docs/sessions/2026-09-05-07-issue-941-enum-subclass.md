# 2026-09-05 — #941: reject subclassing an enum class with `C0001`

## Previous checkpoint's outcome

Two iterations closed since the last snapshot.

- Iteration 09 delivered [#937](https://github.com/rotnov/pycc/issues/937)
  (flip the PEP 563 row): PR [#939](https://github.com/rotnov/pycc/pull/939)
  merged by squash as `a275fbaa` at 2026-09-05T15:24:10Z and #937 is CLOSED.
- Iteration 10 implemented [#921](https://github.com/rotnov/pycc/issues/921)
  as PR [#943](https://github.com/rotnov/pycc/pull/943), but the concurrent
  automated actor's PR [#942](https://github.com/rotnov/pycc/pull/942) for
  the same issue merged first as `e6781b2c` at 2026-09-05T17:04:42Z and
  closed #921. #943 was closed as superseded; its one surviving improvement
  (the enum-call `C0001` renders at `1:1` instead of on the call expression)
  was filed as [#944](https://github.com/rotnov/pycc/issues/944). The process
  mistake is journaled in `docs/AGENT_RETROSPECTIVE.md`.

Every post-merge `main` run for `e6781b2c` concluded `success`: Main history
audit [33979823367](https://github.com/rotnov/pycc/actions/runs/33979823367),
Status page freshness
[33979823315](https://github.com/rotnov/pycc/actions/runs/33979823315), CI
[33979823317](https://github.com/rotnov/pycc/actions/runs/33979823317), and
Pages [33979823414](https://github.com/rotnov/pycc/actions/runs/33979823414).
`autopilot/iter-2026-09-05-11` was cut from `e6781b2c`, which was still
`origin/main` when this snapshot was written.

## Overall status

Implemented [#941](https://github.com/rotnov/pycc/issues/941) on
`autopilot/iter-2026-09-05-11`: one code seam in `pycc_hir`, unit and CLI
tests, and the documentation that described the gap. One pull request, body
carrying `Fixes #941`; the orchestrating session watches CI and merges.
The issue was reconfirmed at `e6781b2c` before the first edit (`pycc check`
exit 0, `pycc run` aborting with `pycc_rt: invalid encoded int word 0x0`) for
the issue's program and its `StrEnum`-base, grand-subclass, and
cross-module-import variants; the reconfirmation comment on #941 doubled as
the claim, and the issue and open-PR list were re-checked before the first
edit, at the first commit, and before the push.

## What the change is

`crates/pycc_hir/src/class/mro.rs::validate_bases` now rejects a base whose
`HirClassDef::is_enum` (the #921 provenance flag) is set, in the same loop
and with the same class-header span as its unknown-base, generic-base, and
circular-inheritance siblings. Two wordings, selected by
`enum_members.is_empty()`: an enum with members names CPython's own
`TypeError: <enum 'Foo'> cannot extend <enum 'Color'>`; a member-less
docstring-only enum (#744), which CPython does allow extending, is "not
supported yet". Because the check runs at HIR lowering, D-225's `ensure_init`
never synthesizes the empty constructor whose unfilled `value`/`name` slots
caused the runtime abort, and a grand-subclass is stopped at the first class
that names the enum.

- Unit tests in `mro.rs`'s tests module: both wordings, the `StrEnum` base,
  the grand-subclass, an enum listed after an ordinary base, and the negative
  (an ordinary base beside an enum class still lowers). Each asserts the span
  starts at the class header.
- `tests/issue_941_enum_subclass.rs`: the issue's program under `check` and
  `build` (exit 1, `error[C0001]`, `<file>:8:1`, the class-header source
  line, no `panicked`/`internal error`/`pycc_rt:`, no artifact), the
  `StrEnum`, grand-subclass, and member-less variants, and an enum imported
  from a second project module (#881 harness shape).
- `docs/DIAGNOSTICS.md` (new C0001 case beside the #921 enum-call entry),
  `docs/TYPE_SYSTEM.md` (`enum.Enum` row), `docs/ROADMAP.md` (#379
  paragraph's "tracked by #941" sentence became "Fixed by #941"; the PEP 435
  `✅` gap list drops subclassing), and `class.rs`'s `is_enum` doc comment
  drops its "tracked by" pointer.

Gates run locally against this branch: `cargo fmt --all -- --check`,
`cargo clippy --workspace --all-targets -- -D warnings`,
`cargo test --workspace` (all suites 0 failed), the CI coverage sequence
(`cargo build --target x86_64-apple-darwin -p pycc_rt`,
`cargo build --workspace`, `cargo build --release -p pycc_rt`,
`cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`:
TOTAL regions 52849/0 missed = 100.00%, lines 34670/0 missed = 100.00%),
`python3 -m unittest discover -s scripts -p 'test_*.py'` (996 tests, OK),
`check_roadmap_evidence.rb`, `check_status_page_freshness.rb origin/main`
(no signal), `check-site.sh`, `check_conformance_breadth.py`,
`check_readme_milestone_projection.rb`, and
`generate_decisions_index.py --check`.

## Deviations from the plan

- No separate `issue-to-plan` comment: a single code seam, mechanically
  scoped (D-021 step 10); the reconfirmation comment carries the plan.
- No `tests/diagnostics/` byte-exact fixture: #921's `C0001` at `e6781b2c`
  uses a CLI integration test only (`tests/issue_921_enum_call.rs`), so this
  change follows that convention.
- `cargo test --workspace` was run without `--include-ignored`: the local
  oracle is CPython 3.14.6 and the conformance tests require 3.14.7.
- `check_status_page_freshness.rb` did not fire on the #379 paragraph edit,
  so `site/status/index.html` and its four pins are untouched.

## Known follow-ups

- [#944](https://github.com/rotnov/pycc/issues/944) — the enum-call `C0001`
  (#921) renders at `1:1`; carry the call expression's span.
- [#934](https://github.com/rotnov/pycc/issues/934) — `pycc_types::check`
  accepts a protocol-returning function whose HIR `pycc_mir` cannot lower
  (MIR panic on protocol-typed method dispatch).
- [#889](https://github.com/rotnov/pycc/issues/889) — string-literal and
  attribute-qualified annotations.
- [#882](https://github.com/rotnov/pycc/issues/882) — the remaining
  `pycc_std` typing-surface widening.
- Extending a member-less enum (`class Base(Enum): "doc"` then
  `class Color(Base): RED = 1`), which CPython allows, is now an explicit
  `C0001` rather than a runtime abort; it has no tracking issue of its own
  and is recorded in the PEP 435 gap list in `docs/ROADMAP.md`.

## Paused autopilot

- Directive scope: open-ended (`/goal fix all opened issues`).
- Active milestone: `v0.4` (Accept unmet).
- Last iteration outcome: #921 closed by the concurrent actor's #942
  (`e6781b2c`); #943 closed superseded; #944 filed.
- This iteration: #941 implemented; PR open, awaiting CI and merge by the
  orchestrating session.
- Next step: re-enter `issue-select` for `v0.4` after this PR lands.
- Denylist: empty.

## Where to resume

`validate_bases` in `crates/pycc_hir/src/class/mro.rs` is the whole code
change; its tests module and `tests/issue_941_enum_subclass.rs` lock both
wordings and the span. If a later slice implements member-less enum
extension, the `enum_members.is_empty()` arm is the one to remove, and
`docs/TYPE_SYSTEM.md`'s `enum.Enum` row plus the #379 paragraph in
`docs/ROADMAP.md` are the prose that must change with it.
