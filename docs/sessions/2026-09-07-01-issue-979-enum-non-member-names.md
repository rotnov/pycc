# 2026-09-07 — checkpoint 01: issue #979, `Enum`-body non-member names

Autopilot iteration 27, working from `origin/main` = `3ba4a027`
("fix(pycc_types): reject unrenderable instance and protocol string conversion
with C0001 (#977) (#985)") in the worktree
`.claude/worktrees/autopilot-2026-09-06-27` on branch
`autopilot/iter-2026-09-06-27`.

## Where the repository stood at this checkpoint

- The iteration opened against `origin/main` = `edc454ba` (PR #986, issue #984;
  one codex review thread resolved). Post-merge runs on `edc454ba` were all
  success: CI `34059876911`, Pages `34059876849`, Main history audit
  `34059876841`, Status page freshness `34059876866`.
- **`origin/main` moved to `3ba4a027` mid-iteration**, while the #979 plan
  comment was being posted: PR #985 (owner's `feat/issue-977`) merged, closing
  #977 and claiming `docs/decisions/D-237-*`. This branch was fast-forwarded to
  it, `D-238` was re-verified free, and a short baseline-update comment was
  added under the plan on #979 rather than editing the published plan.
- Post-merge runs on `3ba4a027`: CI `34109350822` success, Main history audit
  `34109350909` success, Status page freshness `34109351783` success, **Pages
  `34109350781` failure**. That failure is not caused by this branch — see
  "Known follow-ups" below.
- `python3 scripts/manage_ci_bypass.py status` reports branch protection
  matching the documented baseline (`audit` + `ci-gate` required, admins
  enforced, conversation resolution required, zero approving reviews). No
  `[ci-bypass]` incident is open.
- No open pull requests remained after #985 merged, until this branch's own.

## What this checkpoint delivered

Issue #979 (v0.4): in an `Enum` body, an assignment CPython's
`enum._EnumDict.__setitem__` keeps *out* of the member list was lowered here as
an ordinary member — a D-198 false acceptance. Fixed by rejecting the whole
non-member family with `C0001`, recorded as
[D-238](../decisions/D-238-reject-enum-body-assignments-cpython-keeps-out-of-the.md).

- The plan is published as a comment on
  [#979](https://github.com/rotnov/pycc/issues/979) (D-143 delegated
  authorization) and went through two adversarial rounds with the pinned local
  reviewer, each ending in concrete edits.
- Scope is deliberately wider than the issue text. The issue proposed a
  dunder-only predicate; eleven shapes measured against CPython 3.13.9 show all
  three of `_EnumDict`'s sibling branches diverging, including `__x = 1`
  (name-mangled, not a dunder) and `_order_ = 'B'` beside `B = 'b'` (which slips
  past the value-kind mismatch that accidentally rejected the same name beside
  `B = 2`). D-238 carries the full table.
- The local deep review of the staged diff found a **blocker** that widened the
  predicate again, and it is the one substantive deviation from the published
  plan. `_EnumDict._is_private` matches the *raw* dict key against the literal
  `_<ClassName>__` prefix, so a source name already spelled `_C__x` inside
  `class C(Enum)` is kept out of the member list even though nothing mangled it
  — a surviving D-198 false acceptance that a shape-only "two leading
  underscores" test can never see. Confirmed against CPython 3.13.9 before
  fixing. The predicate now has four arms rather than three, and
  `ClassBodyRoute::Enum` carries the enclosing class's name so the fourth can be
  class-name-keyed; `_C__x` inside `class D(Enum)` stays an ordinary member and
  has its own accepting test in both the unit and the end-to-end file. The same
  round corrected two documentation claims (`__order__` is popped out of the
  class dict entirely rather than left as a class attribute; `_x__` and
  `_foo___` are a third over-rejected family, excluded from `_is_sunder` by its
  `name[-2] != '_'` condition).
- The guard is the third and last check inside
  `reject_reserved_class_attr_name`, gated to `ClassBodyRoute::Enum`. That
  ordering is load-bearing: `__slots__`, `__init__`, `__new__` and
  `__init_subclass__` are all dunder-shaped, so a shape check placed first would
  silently repoint their pinned messages. A test pins the ordering from the
  other side.
- The predicate is a documented superset of CPython's own set: it never
  under-rejects, and over-rejects every sunder-shaped name (including the ten
  `_EnumDict` allowlists), names matching only the `__`-prefix-and-suffix shape
  (`__`, `___`, `____`, `___x___`), and names with one leading underscore and
  two or more trailing ones (`_x__`, `_foo___`, `_C__x__`). All three families
  are measured and recorded in D-238 rather than asserted.
- Two tests that pinned the old acceptance were inverted, prose included, not
  deleted: `an_enum_member_named_after_an_unreserved_dunder_is_rejected` in
  `crates/pycc_hir/src/tests/reserved_dunder_class_attrs.rs` and in
  `tests/issue_975_reserved_dunder_class_attrs.rs`.

All local gates pass on this branch, including `cargo llvm-cov --workspace
--fail-under-lines 100 --fail-under-regions 100` at 100.00% lines and 100.00%
regions (55768 lines, 36935 regions, 0 missed).

## Known follow-ups

- **[#987](https://github.com/rotnov/pycc/issues/987) — Pages is red on `main`**
  (filed this checkpoint, v0.4). `ruby scripts/check_sitemap_lastmod.rb` fails at
  `3ba4a027`: #985 stamped the `/status/` sitemap `lastmod` as `2026-09-06` while
  its squash merge landed on `2026-09-07`, so `deploy` and `notify-indexnow` are
  skipped and the site is not being published. Reproduced locally. Deliberately
  **not** folded into the #979 PR — that would mix an unrelated `site/status/`
  four-pin rotation into a `pycc_hir` compiler fix. `ci-gate` and `audit` are
  unaffected, so merges are not blocked; publishing is.
- **`docs/ROADMAP.md` is at its llms.txt per-resource budget.** After this
  change the Roadmap document is 168948 bytes against a 168960-byte budget
  (issue #207) — 12 bytes of headroom. The #979 paragraph had to be rewritten
  four times to fit. The next roadmap prose addition of any size will fail
  `scripts/check-site.sh`, and the fix will have to be a real trim or a budget
  decision rather than more compression.
- Deferred from D-238's Alternatives: porting CPython's three predicates exactly,
  so the recognized sunders and the short `__`-shaped names stay accepted. For
  the sunders that would trade a documented over-rejection for a fresh false
  acceptance; for the short shapes it is achievable but pathological. Narrowing
  later is compatible.

## D-127 autopilot loop state

- Standing directive: `fix all opened issues`, milestone v0.4.
- Next candidates after #979: **#987** (new, blocks publishing), then #974,
  #981 (needs a design pass — the callable-vs-non-callable distinction D-236's
  messages do not describe), #982, then #908, #952, #954, #932, #798, #768,
  #606, #903, #893.
- #927 stays blocked on #918's Part 1. #958 and #965 remain deferred as
  decision-bearing.
- The P-tier convention is the issue **title** prefix, not a label.

## Where a fresh session should resume

Read this file, then `docs/decisions/D-238-*.md` for the rule and its measured
evidence, and `crates/pycc_hir/src/class/reserved_names.rs`'s module docs for
the three name sets that now share one guard and why their check order is
load-bearing. The #979 pull request is open and unmerged at this checkpoint;
confirm its state before assuming either outcome.
