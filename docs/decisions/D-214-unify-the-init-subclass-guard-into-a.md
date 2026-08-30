---
id: D-214
title: "Unify the __init_subclass__ guard into a single unconditional nearest-ancestor lookup"
status: accepted
---

## D-214: Unify the `__init_subclass__` guard into a single unconditional nearest-ancestor lookup

- Status: accepted
- Context:
  [D-213](D-213-defer-pep-487-full-invocation-reject-the.md) fixed the
  soundness gap where a subclass that does **not** define its own
  `__init_subclass__` was never checked against an inherited, side-effecting
  base hook. Its own Consequences section forward-declared a further gap,
  filed as [#854](https://github.com/rotnov/pycc/issues/854): the guard's
  `if`/`else if` structure in `crates/pycc_hir/src/class.rs` (then at
  ~2076-2134) validated the *current class's own* `__init_subclass__` body
  whenever both the current class and some base defined the hook, and only
  fell through to the corrected ancestor lookup when the current class had
  no override of its own. This is backwards relative to CPython: `type.__new__`
  invokes `super(new_cls, new_cls).__init_subclass__(**kwargs)`, whose MRO
  lookup starts immediately *after* `new_cls` — `new_cls`'s own definition is
  never the one CPython invokes at `new_cls`'s own creation, only when
  something later subclasses it. So a subclass with a trivial override and a
  side-effecting ancestor hook was silently accepted (the ancestor's real
  invocation target was never checked), and a subclass with a side-effecting
  override and a trivial ancestor hook was wrongly rejected (its own,
  never-invoked body was checked instead of the ancestor's legal one).

  A second, independent blind spot compounded this: `MethodKind::ClassMethod`
  registers a `@classmethod`-decorated method into a separate `class_methods`
  table, never into `methods`. Both HIR-table-based checks in the pre-fix
  guard (the own-hook test and the base-has-hook gate) queried `methods`/
  `cd.methods` only, so a `@classmethod`-decorated `__init_subclass__` was
  invisible to either check on either side.

- Decision:
  Replace the `if`/`else if` structure with a single, unconditional lookup
  that runs identically for every class regardless of what it defines
  itself: find the nearest ancestor in `mro.iter().skip(1)` that has a
  `base_class_asts` entry defining `__init_subclass__` in its raw AST (a
  scan by `Stmt::FunctionDef` variant and name, which is decorator-agnostic
  and therefore closes the `@classmethod` blind spot structurally, not via a
  second dedicated check), and validate *that ancestor's* body against the
  current class's own creation site (`def.range`). The current class's own
  `__init_subclass__` definition, if any, is no longer inspected by this
  guard at all — it becomes ordinary method-body content until and unless
  something later subclasses it, at which point the same unconditional
  lookup runs again from that grandchild's perspective.

  `validate_init_subclass_body` drops its `inherited: bool` parameter: after
  unification there is exactly one message variant, since the validated body
  is now always the nearest ancestor's, never the current class's own. Its
  signature also drops the generic `<R> where std::ops::Range<u32>: From<R>`
  in favor of a concrete `std::ops::Range<u32>` parameter, since exactly one
  call site remains (`def.range`) after unification — retaining unused
  generality here would itself be a dead-abstraction finding. `pycc_ast`
  deliberately does not re-export `ruff_text_size::TextRange` (`pycc_hir`
  depends only on `pycc_ast`, never on `ruff_text_size`, per the documented
  boundary in `crates/pycc_hir/src/expr.rs`), so the parameter is spelled as
  `std::ops::Range<u32>` rather than a named `pycc_ast` range type; the call
  site converts via `.into()`, relying on `ruff_text_size`'s own
  `From<TextRange> for Range<u32>` impl plus `core`'s blanket `From<T> for T`
  — `pycc_hir`'s own source never names `TextRange`.

  **A further consequence discovered empirically while implementing this
  change, beyond what the posted implementation plan anticipated:** because
  the guard now runs unconditionally at *every* class's own creation against
  *that class's own* nearest ancestor, a class sitting directly under a
  side-effecting, introspectable ancestor hook is illegal **at its own
  creation**, regardless of what it overrides the hook with. There is no way
  to legally construct a three-level chain `Grandchild(D(B))` where `B`'s
  hook is side-effecting and `D`'s own creation succeeds only because `D`
  overrides the hook — `D`'s own creation is checked against `D`'s own
  nearest ancestor (`B`), independent of `D`'s override, so `B` being
  side-effecting already rejects `D` on its own. This does not contradict
  D-213's own "Alternatives" reasoning (that a grandparent's side-effecting
  hook, shadowed by a statically-evaluable parent override, is never actually
  invoked by CPython and must not be rejected at a *grandchild's* creation):
  that reasoning is about which ancestor a grandchild's own lookup finds
  first, and remains correct. It is a genuinely separate statement about a
  class's *own* legality that D-213 did not make and the posted #854
  implementation plan's own Test E did not account for — that plan's Test E
  fixture (`B` side-effecting, `D`'s own override trivial, `Grandchild(D)`
  with no override, "expect accepted") is unrealizable: `D`'s own creation
  would already be rejected by `B`'s side-effecting body before `Grandchild`
  is ever lowered, exactly per the plan's own stated truth table ("B
  side-effecting / D trivial → reject"). This was caught by an actual test
  run returning `Err` where the plan predicted `Ok`, confirmed by a second,
  independent review pass (this session's advisor tool) before revising the
  test rather than the production code. The test was replaced with two
  fixtures that are both realizable and still discriminate the intended
  "nearest ancestor, not any ancestor" property: a reject-direction fixture
  (`grandchild_validates_parents_own_override_not_grandparents_hook`, `B`
  trivial / `D`'s own override side-effecting / `Grandchild(D)` no override
  → the grandchild is rejected because its nearest ancestor `D`'s own
  override, not `B`'s trivial grandparent hook, is what a would-be linear
  chain's shadowing property would need to demonstrate), and a genuine
  nearest-vs-farther discriminator via multiple inheritance rather than a
  linear chain (`multiple_inheritance_nearest_mro_hook_wins_over_farther_side_effecting_one`,
  `class D(M, B)` with C3 MRO `[D, M, B, object]`, `M` trivial / `B`
  side-effecting / `D` no override of its own → accepted, since `skip(1)`
  must reach `M` before `B`).

- Alternatives:
  - *Keep Gap 2 (the `@classmethod` blind spot) as a separate, dedicated
    multi-table check, as the issue's own fix sketch proposed.* Rejected:
    the surviving ancestor-lookup mechanism after unifying Gap 1 already
    scans raw AST by `Stmt::FunctionDef` shape and name, which was already
    decorator-agnostic before this change. A second check auditing "however
    many method-kind tables a class carries" would be dead code checking a
    condition the unification already handles structurally. Two regression-
    lock unit tests pin this instead of new production code.
  - *Preserve Test E's original fixture from the posted plan and treat the
    test failure as a production-code bug to fix instead.* Considered and
    rejected after tracing CPython's actual `super(new_cls, new_cls)`
    semantics by hand and confirming the rejection is correct: `D`'s own
    creation genuinely does invoke `B`'s side-effecting hook in real
    CPython, so pycc rejecting `D` here is the *correct* behavior, not a
    bug the unification introduced. Changing the production code to accept
    this fixture would reintroduce the exact class of gap D-213 and this
    entry both close.
  - *Attribute the diagnostic to the ancestor's own definition site
    (`fd.range`) instead of the current class's creation site (`def.range`).*
    Rejected for the same reason as D-213's own equivalent alternative: the
    ancestor's own definition is legal standalone; the failure is specific
    to the obligation *this* class's creation places on it.

- Consequences:
  - This entry supersedes D-213's Consequences bullet describing `#854` as
    "left for a follow-up change" and its statement that the fix
    "intentionally covers only the case where the subclass does not define
    its own [override]" — both gaps #854 tracked are now closed by this
    unification, and #854 is closed by the pull request this entry ships
    with.
  - `validate_init_subclass_body`'s `inherited: bool` parameter (a D-213
    consequence) is removed; the function now takes a concrete
    `std::ops::Range<u32>` instead of a generic range parameter, and emits
    exactly one diagnostic message variant.
  - The pre-existing `if` branch and the `base_has_init_subclass` gate in
    `crates/pycc_hir/src/class.rs` are deleted outright; the guard is now a
    single unconditional block reached on every class lowering, not only
    when the current class itself defines the hook.
  - `docs/ROADMAP.md` and `docs/TYPE_SYSTEM.md`'s PEP 487 descriptions are
    updated in the same pull request to describe the corrected, unconditional
    nearest-ancestor rule and drop the "separate, still-open gap (#854)"
    framing.
  - `docs/PYTHON_STANDARDS.md` is unchanged: the PEP 487 row stays `☐` with
    no fixture, per D-213 (`__init_subclass__`/`__set_name__` remain
    recognition-only; full invocation semantics are still deferred).
  - A class directly beneath a side-effecting, introspectable ancestor hook
    is now illegal at its own creation regardless of its own override's
    shape — this is a new, intentional consequence of unconditional
    unification (see Decision above), not merely a restatement of D-213's
    existing inherited-hook rule.
