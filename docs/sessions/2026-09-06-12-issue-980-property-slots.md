# 2026-09-06-12 — issue #980, `@property def __slots__` (autopilot iteration 25)

## Overall status

D-127 autopilot loop, standing directive *fix all opened issues*, milestone
v0.4. Iteration 25 **implements** the fix for
[#980](https://github.com/rotnov/pycc/issues/980) on branch
`autopilot/iter-2026-09-06-25`, based on `origin/main = f8e9d2e3` (the squash of
PR [#978](https://github.com/rotnov/pycc/pull/978) for issue #975 / D-236). At
the moment this snapshot is written the work is **on that task branch and not
yet merged** — it is delivered through the pull request this commit opens, and
nothing below should be read as already on `main`.

## What this iteration changes

A `@property` getter named `__slots__` was a D-198 false acceptance: pycc
compiled and ran it, while CPython 3.13.9 (`v3.13.9:8183fa5e3f7`) raises
`TypeError: 'property' object is not iterable` while the `class` statement
itself executes, because `type.__new__` iterates `__slots__` at class creation.
The fix is a single new branch inside `reject_reserved_property_name`
(`crates/pycc_hir/src/class/reserved_names.rs`) carrying a *third, distinct*
`__slots__` message. Three things were deliberately not done, each recorded in
the published plan and in D-236's dated amendment note:

* No new decision entry. D-236 deferred this exact shape by name, so closing it
  fulfils the deferral rather than reversing an accepted decision; the closure
  is a dated 2026-09-06 amendment note on the Consequences bullet that carried
  the deferral. D-235 is untouched.
* No `ClassBodyRoute` parameter and no `Enum` arm on the new check. Only one
  class body reaches this route — a plain and a `@dataclass` body share the one
  `MethodKind::PropertyGetter` arm, and `lower_enum_class` rejects a method
  definition outright before it — so a route arm would be a dead match arm and
  would fail D-014's 100% region gate.
* No reuse of `slots_message`. D-154's "the instance layout is fixed at compile
  time from its `__init__`" is a false account of this failure, which is why
  #980 existed at all; the new string carries none of that language, and the
  tests pin its absence from both the unit and the end-to-end side.

## State inspected

* `origin/main` = `f8e9d2e3` at dispatch and re-verified before committing.
* No open pull requests at dispatch time.
* `python3 scripts/manage_ci_bypass.py status` — branch protection matches the
  documented baseline (`audit`, `ci-gate`, admins enforced, conversation
  resolution required).
* Post-merge workflow runs, each re-resolved with `gh run view` immediately
  before writing this file:
  * on `f8e9d2e3`: CI `34051578722` success, Pages `34051578768` success;
  * on `64a6ee08` (PR #971 for #944, merged by the concurrent actor): CI
    `34050969256` success, Status page freshness `34050969234` success, Main
    history audit `34050969230` success, Agent policy `34050969258` success,
    Agent assets `34050969246` success, Pages `34050969312` **cancelled** —
    superseded by the later `f8e9d2e3` Pages run, which succeeded;
  * on `28a1b194`: CI `34039995372` success (after a `--failed` rerun of the
    known #414 nbody flake), Pages `34039995455` success.

## Preceding checkpoint

PR #978 merged as `f8e9d2e3`, closing #975, after four `chatgpt-codex-connector`
review rounds and one `origin/main` merge — the concurrent automated actor had
merged PR #971 (issue #944) as `64a6ee08` mid-flight. See
`docs/sessions/2026-09-06-11-issue-975-reserved-dunder-class-attrs.md`.

## Issues filed on 2026-09-06

#977, #979, #980, #981, and — from this iteration —
[#982](https://github.com/rotnov/pycc/issues/982), the `__qualname__`
divergence split out of #980. #982 carries all three measured strings
(`@property def __qualname__` → `TypeError: type __qualname__ must be a str,
not property`; `__qualname__: int = 1` → `... not int`; `__qualname__: str =
"D"` agrees on both engines) so nobody re-derives them, and it is pinned in the
tree by `a_property_getter_named_qualname_is_left_to_issue_982`.

## Known follow-ups, in the loop's candidate order

Next after #980: **#979** (an `Enum` body's non-protocol dunder becomes a
member) — decision-bearing, since it must choose between reject-every-dunder
and model-as-class-attribute *and* correct D-236's own Alternatives record;
then **#977** (codegen panic on `print(instance)` without `__repr__`), **#974**
(a conservative false rejection), **#981** (needs a design pass: a fourth
message, a `MethodKind::Regular` call site that must leave `def __init__`
alone, and the `@staticmethod def __new__` / `@classmethod def
__init_subclass__` twins decided together), then #908, #952, #954, #927, #932,
#798, #768, #606, #903, #893. #958 and #965 stay deferred as decision-bearing.
The P-tier convention is a **title** prefix (`P1:`/`P2:`/`P3:`), not a label.

## Where a fresh session should resume

Read this file, then `docs/decisions/D-236-…md` (including its 2026-09-06
amendment note) and `crates/pycc_hir/src/class/reserved_names.rs`'s module
header — together they are the current map of which reserved class-attribute
name is closed on which route, and which shapes are still parked behind #979,
#981 and #982.
