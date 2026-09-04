---
id: D-226
title: "Infer an un-annotated class attribute's type from its literal"
status: accepted
---

## D-226: Infer an un-annotated class attribute's type from its literal
- Status: accepted
- Context: [#911](https://github.com/rotnov/pycc/issues/911) (Part 1 of
  [#885](https://github.com/rotnov/pycc/issues/885)) made the annotated
  spelling `X: int = 1` a compile-time constant in a class body, under
  [D-224](./D-224-restrict-class-level-attributes-to-scalar.md)'s scalar-only
  restriction. [#910](https://github.com/rotnov/pycc/issues/910) is Part 2: the
  un-annotated spelling `X = 1`, which is what Python code overwhelmingly
  writes for a class-level constant, still hit the class-body catch-all and was
  rejected with `C0001`. The two spellings mean the same thing in CPython, and
  a user who writes the annotated form only to satisfy the compiler is being
  taxed by an implementation detail. The question the change had to answer is
  where the type comes from when there is no annotation to read it from.
- Decision: **infer the type from the literal, then reuse Part 1's path
  unchanged.** A new `infer_class_attr_ty` in
  `crates/pycc_hir/src/class/attrs.rs` maps the right-hand side — after
  unwrapping a unary `+`/`-` on a number — to its natural `Ty`: an integer
  literal to `Ty::Int`, a float literal to `Ty::Float`, `True`/`False` to
  `Ty::Bool`, a string literal to `Ty::Str`, and anything else (a complex
  literal, `~1`, `not True`, a name, a call, a container display) to `None`,
  which becomes the same "must be initialized with a literal" `C0001` the
  annotated spelling already emits. The inferred `Ty` is then handed to the
  **untouched** `class_attr_value`, so the literal extraction, the duplicate
  check, the collision check, the `T0044` write rejection, and the MIR-level
  constant fold are byte-for-byte the code Part 1 shipped. Because inference
  can only ever produce `int`/`float`/`bool`/`str`, the scalar-only invariant
  documented on `fn lower_class_attr` holds for the new spelling *by
  construction* rather than by a check — there is no non-scalar `Ty` an
  un-annotated class attribute can reach. Inside a `@dataclass` body a bare
  assignment deliberately stays `C0001`
  ([#378](https://github.com/rotnov/pycc/issues/378)): there it is a
  class-level default for a field, not a constant, and silently compiling it as
  a constant would diverge from Python's own model.
- Alternatives:
  - *Refactor `class_attr_value` to take an `Option<&Ty>` and let it infer when
    the annotation is absent.* This reads as the smaller diff, but it is worse
    on both axes that matter here. Its per-arm diagnostic strings are pinned by
    #911's fixtures, so every message would have to grow an
    annotation-present/absent split; and the extra branch inside each arm
    doubles the regions the D-014 100%-region gate has to cover, for behaviour
    that is identical once the type is known. Infer-first keeps one code path
    and one set of messages.
  - *Add a `Ty::Param` branch to the inference.* Rejected outright: #911
    already rejects `Ty::Param` for a class attribute, so such a branch would
    be unreachable and would turn the region gate red.
  - *Infer from a wider expression grammar — a constant-folded arithmetic
    expression, a reference to an earlier class attribute.* That is a constant
    evaluator, not an inference rule, and it would let the type depend on
    evaluation order inside the class body. Out of scope; `X = 1 + 1` stays
    `C0001` and says so.
  - *Leave the un-annotated form unsupported and document the annotated form as
    the required spelling.* This is what the tree did. It is a compiler-shaped
    demand on ordinary Python source, and #910 is the report that it is wrong.
- Consequences: the un-annotated spelling reaching the class-attribute path
  makes two pre-existing holes reachable, and both are closed in the same
  change rather than left silent. First, `reject_class_attr_collisions`
  previously compared a class attribute only against the instance-attribute and
  `@property` tables, silently skipping methods; measured against CPython 3.14
  at the base commit, `f = 2` alongside `def f(self)` printed `2` where CPython
  prints a bound method, and `class B(A): f: int = 2` over `class A: def f(self)`
  let `b.f()` dispatch to `A.f` and print `1` where CPython raises
  `TypeError: 'int' object is not callable`. The check now also covers the
  `methods`, `static_methods`, and `class_methods` tables of the class itself
  **and of every base in its MRO**, and its message names the method table.
  Second, `__slots__ = (...)` in a class body would have been bound as an
  ordinary constant and silently ignored, since the D-154 instance layout is
  already fixed at compile time from `__init__`; it is now rejected with an
  explanatory `C0001` in both spellings, checked before the right-hand side is
  examined so the two report identically. The class-body catch-all is reworded
  and split: outside a `@dataclass` it now names both attribute spellings as
  accepted, and five tests that were riding on `x = 1` as a convenient
  "unsupported yet" vehicle are re-vehicled onto `async def`, the
  furthest-scheduled unsupported construct. Reversing this decision means
  re-rejecting a spelling users will by then have written, so treat it as
  irreversible.
