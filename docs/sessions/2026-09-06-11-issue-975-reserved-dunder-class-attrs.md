# 2026-09-06 (11) — #975: class attributes named after the instantiation protocol

Autopilot iteration 24, under the standing D-127 directive *fix all opened issues*, milestone v0.4.

## Overall status

`origin/main` is **`28a1b194`** ("feat(hir): support `ClassVar` in a `@dataclass` body (#913) (#976)"),
which merged PR #976 and closed #913. The previous default-branch commit was `00b0f5a0` (#969 / PR #973).

This checkpoint is the branch `autopilot/iter-2026-09-06-24`, based on `28a1b194`, carrying the #975 fix.
The work is **not merged**. This file is written as part of the commit that completes the change; the branch
is pushed and a pull request opened immediately afterwards, and **merging and CI watching belong to the
orchestrating session**, not to this one. If you are reading this and no pull request exists from this
branch, the push is the missing step.

### Post-merge workflow runs

Re-resolved with `gh run view <id> --json status,conclusion` immediately before committing this file:

| commit | run | workflow | result |
|---|---|---|---|
| `28a1b194` | 34039995372 | CI | **failure** |
| `28a1b194` | 34039995455 | Pages | success |
| `28a1b194` | 34039995323 | Main history audit | success |
| `28a1b194` | 34039995300 | Status page freshness | success |
| `00b0f5a0` | 34032584932 | CI | success |
| `00b0f5a0` | 34032584927 | Pages | success |

**The CI failure on `28a1b194` is the known #414 flake, not a regression from #913.** The only failing job
is `native-build-test (ubuntu-latest, x86_64-unknown-linux-gnu)`, and the only failing test is
`nbody_release_binary_meets_required_speedup_over_cpython`:

> nbody wall-clock speedup ratio 18.42x is below the required 20x gate (cpython wall median 2.5057s,
> pycc --release wall median 0.1360s; CPU-time ratio 18.43x)

`ci-gate` then failed as the aggregate. [#414](https://github.com/rotnov/pycc/issues/414)
("P2: nbody-speedup and frontend-throughput CI gates flaking beyond their own historical noise envelopes")
is open and already tracks exactly this. No new issue was filed. A fresh session should not read this as a
`28a1b194` defect, and should not treat it as a D-024 governance incident — branch protection is intact.

`python3 scripts/manage_ci_bypass.py status` reports **branch protection matches the documented baseline**
(`strict`, contexts `audit` + `ci-gate`, `enforce_admins`, 0 required approvals, conversation resolution
required, no force pushes or deletions). No `[ci-bypass]` incident is open.

## In flight

- **This branch, `autopilot/iter-2026-09-06-24`** — #975. See below.
- **PR [#971](https://github.com/rotnov/pycc/pull/971)** (#944, `feat/issue-944`,
  "fix(hir): report the enum-call C0001 at the call expression"): **OPEN**, not a draft, `MERGEABLE`,
  head **`0e88a674`** — note this moved from `5f450ae5` during this session, so any earlier snapshot of its
  head is stale. It claims decision number **D-233** and session file `2026-09-06-08-*`, neither of which is
  in the tree; that is why this file is `-11-` (01–07, 09, 10 are on main, 08 is reserved by #971) and why
  #975's ADR is **D-236** rather than D-233.
  It adds a *new* `crates/pycc_hir/src/class/enum_call.rs` and does **not** touch
  `class/enum_class.rs`, `class/attrs.rs` or `class/body.rs`. The only overlap with this branch is the
  `mod` block in `crates/pycc_hir/src/class.rs` (#971 adds `mod enum_call;`, this branch adds
  `mod reserved_names;` — an adjacent-line textual conflict, resolved by keeping both) plus the usual
  documentation files.

## What #975 delivered

A class attribute named after Python's instantiation or class-creation protocol was a
[D-198](../decisions/D-198-cast-erasure-limits-cast-to-representation.md)-class **false acceptance**: pycc
compiled and ran programs CPython rejects with `TypeError: 'int' object is not callable`.

The issue's premises were corrected before implementation (the published plan is
[issue #975 comment 5560273212](https://github.com/rotnov/pycc/issues/975#issuecomment-5560273212)):

- The issue's set `__init__`/`__eq__`/`__repr__` was wrong **in both directions**. `__eq__` and `__repr__`
  do not diverge outside a dataclass; `__new__` and `__init_subclass__` do, and were missing.
- Three *spellings* diverge identically (`ClassVar[int] = 8`, `int = 8`, bare `= 8`), so the guard is not
  `ClassVar`-gated.
- Three *routes* reach it: a plain class body, a `@dataclass` body (closing two gaps D-235's own set
  missed), and — discovered during review, not in the issue — an `Enum` member list.
- The issue's suggested "move the collision check after synthesis" fix was foreclosed by D-235's
  Alternatives and its pinned four-deep diagnostic precedence.

Implementation: `reject_reserved_class_attr_name` moved out of `class/attrs.rs` (already past the
~1000-line decomposition bar) into a new `crates/pycc_hir/src/class/reserved_names.rs`, gained the
protocol name set, and is now called from three sites. New ADR
[D-236](../decisions/D-236-reject-a-class-attribute-named-after-the-instantiation.md) supersedes **D-235's
generating rule only** — D-235's six-name set and every one of its messages are unchanged, and the two sets
are deliberately disjoint so `__init__` in a `@dataclass` body still reports D-235's message.

## Known follow-ups

- **`print(non_dataclass_instance)` panics in `crates/pycc_codegen/src/lib.rs` with no diagnostic.**
  Reproduced *without* any `ClassVar`, so it is pre-existing and unrelated to #975. Deliberately left out
  of this change; the orchestrating session files it separately.
- **A `@dataclass` field with a plain (non-`ClassVar`) dunder annotation** was investigated and is **not** a
  defect: `@dataclass class P: __init__: int` produces byte-identical output under pycc and CPython. The
  with-default variant (`__new__: int = 8`) is already a conservative `C0001` rejection ("dataclass field
  defaults are not supported yet") where CPython raises `TypeError` — a suboptimal *message*, not a false
  acceptance. Recorded so a future session does not re-open it as a hazard.
- **#414's nbody flake** keeps failing `ci-gate` on main; see the run table above.

## Autopilot loop state

Standing directive: *fix all opened issues*, milestone **v0.4**.

- Next candidates, in order: **#974**, then #908, #952, #954, #927, #932, #798, #768, #606, #903, #893.
  #974 is the natural next pick — it is the `Derived.LIMIT` → `T0044` asymmetry that
  `docs/TYPE_SYSTEM.md`'s class-attribute section already names as tracked. It was deferred behind #975
  because it is a conservative *false rejection* (never a mis-compile) while #975 was a false *acceptance*,
  and because it spans `pycc_types` and `pycc_mir` in lockstep with `pycc_mir/src/expr.rs`, which #971 also
  touches.
- Decision-bearing, deliberately deferred: **#958**, **#965**.
- **#944 excluded** while #971 is open.
- **#336 is the only `P`-labelled open v0.4 issue** (`P3`). It stays excluded: it needs a privileged Pages
  deploy job plus repository Pages source settings (maintainer-only authority, a standing issue-select
  exclusion), and its premise is partly stale because `site/status/index.html` now exists. Re-triage rather
  than implement as written.
- **The P-tier convention in this repository is the issue *title prefix* (`P2:`/`P3:`), not a label.**
  #414, #416, #585, #636, #641, #408 and #706 are P-tier *by title* and carry no `P*` label — verified with
  `gh issue view`. A selection pass that filters on labels alone will mis-rank them as unmarked. #414 above
  is a worked example.

## Where to resume

1. `git fetch --prune origin` and re-read `origin/main`; this snapshot is anchored at `28a1b194`.
2. Find the #975 pull request opened from `autopilot/iter-2026-09-06-24`
   (`gh pr list --repo rotnov/pycc --head autopilot/iter-2026-09-06-24`) — it needs CI watched and merging,
   neither of which this session did.
3. Re-check #971's state and head before assuming anything about D-233 or session file `-08-`.
4. Then #974.
