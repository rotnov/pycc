# 2026-09-06 — checkpoint 14: issue #984, a non-`@property` `def __slots__`

Autopilot iteration 26, working from `origin/main` = `e77b4b13`
("fix(hir): reject a `@property` getter named `__slots__` (#980) (#983)") in
the worktree `.claude/worktrees/autopilot-2026-09-06-26` on branch
`autopilot/iter-2026-09-06-26`.

## Where the repository stood at this checkpoint

- `origin/main` = `e77b4b13`. PR #983 merged as that squash, closing #980. One
  codex review thread on it was resolved without a code change. #982 and #984
  were filed off its local deep-review round.
- Post-merge runs on `e77b4b13`: CI `34055421094` first failed on the #414
  nbody flake (`build-test-coverage`, 11.59x against the 12x floor), was rerun
  with `--failed`, and now reports `completed / success`; Pages `34055421089`
  success; Main history audit `34055421095` success; Status page freshness
  `34055421099` success. The predecessor commit `f8e9d2e3` was all-success
  (CI `34051578722`, Pages `34051578768`).
- `python3 scripts/manage_ci_bypass.py status` reports branch protection
  matching the documented baseline (`audit` + `ci-gate` required, admins
  enforced, conversation resolution required, zero approving reviews). No
  `[ci-bypass]` incident is open.
- No open pull requests at dispatch time. One opened while this branch's
  gates were running: [#985](https://github.com/rotnov/pycc/pull/985)
  (`feat/issue-977`, issue #977), from a separate concurrent session. It
  does not touch this branch's files, and `origin/main` was still
  `e77b4b13` at the pre-push race check. That PR carries its own
  `docs/sessions/2026-09-06-13-issue-977-instance-string-conversion.md`,
  so this checkpoint takes `NN=14` to keep the same-day sequence unique.

## What this checkpoint delivered

Issue #984: every non-`@property` `def __slots__` in a class body was still
falsely accepted (D-198 class). Seven spellings were re-measured as diverging
at `e77b4b13` against CPython 3.13.9 — a bare `def`, `@staticmethod`,
`@classmethod`, `@abstractmethod`, `@override`, and a plain `def` inside a
`@dataclass` body and inside a `Protocol` body. Each raised
`TypeError: '<carrier>' object is not iterable` at class creation while pycc
compiled and ran the program.

- Plan published as an issue comment on #984 under the D-143 delegated
  authorization, after two adversarial review rounds against the pinned
  `ievo:deep-reviewer`, both ending in concrete edits.
- `crates/pycc_hir/src/class/reserved_names.rs` gained
  `reject_reserved_method_name` (which subsumes #980's getter dispatch
  unchanged), `reject_reserved_protocol_method_name`, `method_slots_carrier`
  and `method_slots_message`. `PROPERTY_SLOTS_MESSAGE`, `slots_message`,
  `ClassBodyRoute` and `instantiation_protocol_message` keep their exact
  bodies; their doc comments were updated in the same commit, because three of
  them asserted facts the change falsifies.
- Two call sites, not one: `walk_class_body`'s method loop and
  `lower_protocol_class`'s own method walk. `lower_class` returns through the
  protocol lowering *before* the method loop, so a guard placed only in
  `body.rs` would have left `class P(Protocol): def __slots__` open. This was
  the largest single correction the planning round made to the issue's own
  premises.
- Two precedence boundaries were kept deliberately. A plain `def __new__` is
  still accepted (the instantiation-protocol half stays gated to
  `MethodKind::PropertyGetter`; #981 owns it, and its pin stays green). A
  `@__slots__.setter` with no preceding getter keeps its existing "requires a
  preceding `@property` getter" diagnostic, because CPython raises
  `NameError: name '__slots__' is not defined` there rather than a `TypeError`
  about a non-iterable object — the new message would have been false in every
  clause for it.
- D-236 gained a dated 2026-09-06 amendment note generalizing its type-naming
  rule from "the decorator fixes the bound type structurally" to "the
  **binding form** fixes it", which is what licenses the new message to name
  `function` for a decorator-free `def`. No new decision entry: no new
  generating rule and no new reserved name.
- Docs updated in the same change: `docs/TYPE_SYSTEM.md` (the `__slots__`
  bullet's account count, and the note that the protocol-name half stays
  `@property`-only pending #981), `docs/ROADMAP.md` (prose inside the existing
  `#910` paragraph, plus the new e2e file on its `Tests:` list),
  and the D-236 amendment. `docs/DIAGNOSTICS.md` and
  `docs/PYTHON_STANDARDS.md` are unaffected, verified at the convention level
  rather than skipped: no diagnostic code is added (it stays `C0001`) and no
  conformance-matrix row concerns `__slots__`.
- A scope note was posted on #982 recording the measured plain
  `def __qualname__(self)` divergence (CPython
  `TypeError: type __qualname__ must be a str, not function`, `pycc check`
  exit 0) as that issue's scope, so the next deep-review round does not re-file
  it as a fresh gap the way #984 was filed off #983.

## Follow-ups and loop state

The standing D-127 directive is "fix all opened issues", currently scoped to
milestone v0.4. #984 was the only non-excluded P-tier v0.4 issue, which is why
it outranked every unmarked candidate this round.

Next candidates, in the order the selection round ranked them:

- **#977** — `check` passes and then `pycc_codegen` panics at `lib.rs:1593`
  when printing an instance without `__repr__`. A compiler panic, and the
  strongest unmarked candidate; the P-tier rule deferred it this round only.
  **Now already in flight** as PR #985 from a concurrent session — treat it
  as taken and start the next loop from #979.
- **#979** — decision-bearing: reject-every-dunder versus
  model-as-class-attribute; needs an amendment to D-236's Alternatives.
- **#974**, then **#981** (needs its own design pass), then **#982**.
- Then #908, #952, #954, #927, #932, #798, #768, #606, #903, #893.
- #958 and #965 stay deferred as decision-bearing.

The P-tier convention in this tracker is the issue **title** prefix, not a
label.

## Where a fresh session should resume

Read this file, then `docs/sessions/2026-09-06-12-issue-980-property-slots.md`
for the predecessor checkpoint. The reserved-name guard's own module
documentation (`crates/pycc_hir/src/class/reserved_names.rs`, lines 1-120) is
the most current account of which class-body routes reach which message and
why no further match arm may be added under D-014's region gate; read it
before touching that family again.
