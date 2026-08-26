---
id: D-202
title: "PEP 654 `except*`/ExceptionGroup: six deliberate simplifications"
status: accepted
---

## D-202: PEP 654 `except*`/ExceptionGroup: six deliberate simplifications
- Status: accepted
- Context: Part 3 of #382 (#542) adds PEP 654's `except*` clauses and the
  `ExceptionGroup`/`BaseExceptionGroup` builtin exception types on top of
  pycc's D-173 check-and-branch exception model (no platform unwinding;
  generated code explicitly checks a thread-local pending-exception state and
  branches) and D-189's per-class compile-time type tag. CPython's own PEP
  654 semantics are considerably richer than what a single-pass AOT compiler
  with pycc's existing exception representation can support without a much
  larger redesign: real `ExceptionGroup` objects are dynamically
  constructible with an arbitrary iterable of members and preserve each
  member's own class identity through arbitrarily deep splitting and
  re-merging, `BaseExceptionGroup` is a `BaseException`-only construct that
  cannot mix `BaseException`-only members with `Exception` members, and a new
  exception raised inside an `except*` clause is chained into a *derived*
  exception group alongside the clause's own still-unhandled siblings,
  visible to any enclosing `except*`. Implementing all of that exactly would
  require a real polymorphic list-backed group object, a materialized
  exception instance carrying its own dynamic subclass identity (which
  doesn't exist yet -- see D-189's own note that `except ... as e:` binding a
  user class waits on Part 3 of #541, #703), and a redesign of the
  check-and-branch model's single "current pending exception" slot into
  something that can represent a partially-handled group. None of that is
  needed to give #542 a real, testable, spec-referenced `except*` surface
  that composes with the rest of the exception model landed so far -- this
  decision records six narrower simplifications, each satisfying today's
  literal, non-dynamic-membership use of exception groups exactly, that make
  that surface implementable inside the existing model. Each is independently
  revisitable without disturbing the others once a materialized exception
  instance (#703) and/or a genuinely dynamic group object become available.
- Decision:
  1. **`BaseExceptionGroup`'s hierarchy parent is `Exception`, not a separate
     `BaseException`-only branch.** `BUILTIN_EXCEPTION_CLASSES` (now 25
     entries, tags `0..=24`; see D-194 for the array-index-as-tag mechanism
     this extends) adds `ExceptionGroup` and `BaseExceptionGroup` as two more
     fixed-tag compiler-defined classes, both reachable from `Exception`
     through the ordinary MRO-containment machinery
     `pycc_mir::exception::handler_type_tags` already has. pycc has no
     `BaseException`-rooted-but-not-`Exception`-rooted hierarchy at all
     today (`KeyboardInterrupt`, `SystemExit`, etc. are unimplemented), so a
     faithful two-branch MRO would introduce a second root with no other
     member to justify the complexity. `except* SomeError:` and
     `except BaseExceptionGroup:` therefore behave identically to every
     other builtin class already in the flat/tree hierarchy.
  2. **`ExceptionGroup`/`BaseExceptionGroup` construction accepts only a
     string message and a *literal* list of members
     (`pycc_types::exception::check_exception_group_operand`).** A
     non-`HirExpr::ListLiteral` second argument (a bound name, a call result,
     a comprehension) is `T0021`. This mirrors D-105/T0021's existing
     `list[T]`-annotation restriction rather than inventing new dynamic-list
     support solely for this one construction site, and keeps
     `pycc_mir::exception::MirExceptionValue::ConstructedGroup`'s member list
     a fixed-size, compile-time-known array that codegen can build with a
     stack allocation instead of a heap-backed dynamic list type pycc
     doesn't have yet.
  3. **Every group member must be an *existing* exception value, never a
     fresh constructor-call expression
     (`pycc_types::exception::check_exception_group_member_operand`).** This
     is narrower than what a plain top-level `raise` operand accepts: `raise
     SomeError("msg")` is fine, but `ExceptionGroup("multi", [SomeError("msg")])`
     is `T0021`. `pycc_mir::exception::lower_exception_value`'s
     `ConstructedGroup` arm lowers each member through ordinary expression
     lowering (`lower_expr`), which has no way to construct a *new* exception
     object -- only `lower_exception_value` itself (used for the group's own
     top-level `raise` operand, and for a plain, non-group `raise
     SomeError(...)`) knows how to do that. This was discovered as a real
     type-checker/codegen mismatch during #542's own test-writing: the
     type checker initially accepted a fresh-constructor member (by
     delegating to the general `check_raise_operand`) and codegen then
     panicked trying to lower it (`internal error: constructor
     'Exception.__init__' should have been registered as an ordinary user
     function`). A pre-existing doc comment on `MirExceptionValue::
     ConstructedGroup` already stated the intended design -- "a group's
     members are themselves exceptions raised or caught earlier, never
     freshly constructed inline" -- so this fix enforces an intent that was
     already recorded but not yet enforced, at the type-check boundary
     rather than by attempting to widen MIR/codegen to support in-place
     member construction.
  4. **A reconstructed subgroup handed to a matching `except*` clause, or
     re-raised as the final unmatched remainder, is always tagged and named
     as plain `ExceptionGroup`, never the original raised object's dynamic
     subclass.** `pycc_rt_exception_group_partition` (the runtime function
     `emit_try_star` calls once per clause) always allocates its
     `matched_out`/`rest_out` groups with the caller-supplied
     `group_type_tag`/`group_name` -- `emit_try_star` always passes the fixed
     `EXCEPTION_GROUP_TYPE_TAG`/`"ExceptionGroup"` constant, because pycc has
     no user-subclassable `ExceptionGroup` today (there is no mechanism for
     a user class to inherit from a builtin exception *and* itself be
     iterated/partitioned as a group) and no materialized-instance identity
     to preserve even if there were. CPython preserves the original group's
     exact subclass (and re-invokes its `derive()` for a user override)
     through every split; pycc's simplification is a strict loss of that
     identity, acceptable because no test or supported program can construct
     an observably different subclass of `ExceptionGroup` in the first
     place.
  5. **A new exception raised inside an `except*` clause's body propagates
     directly to the statement's `finally`, rather than being merged back
     into the group's still-unmatched remainder the way CPython's
     derived-exception-group chaining would.** `emit_try_star` pushes
     `finally_bb` onto `rt.exceptions.targets` for the duration of each
     clause body, exactly as `emit_try` does for an ordinary `except`
     clause -- so a raise inside the clause body takes the same control-flow
     path an ordinary handler's raise already takes, reusing tested
     primitives instead of inventing an new, untested merge-semantics path
     purely to chain the new exception alongside the statement's remaining
     unmatched members.
  6. **A group member must not itself be an `ExceptionGroup`/
     `BaseExceptionGroup`-typed value -- nested groups are rejected with
     `T0021` at the type-check boundary
     (`pycc_types::exception::check_exception_group_member_operand`).**
     `pycc_rt_exception_group_partition` matches each member only by its own
     top-level `type_tag`, with no recursion into a member's own
     `exceptions`/`exceptions_len` array when that member is itself a group
     -- unlike CPython's PEP 654 `split()`, which does recurse into nested
     groups. Without this rejection, an `except* ... as eg:` binding (whose
     type is unconditionally `ExceptionGroup`, per simplification 1's
     hierarchy) would type-check as an ordinary member of a freshly
     constructed outer group, building a group the runtime cannot partition
     correctly and silently losing the inner exceptions on a subsequent
     `except*` dispatch. This was found by the pinned local reviewer during
     #542's own review pass and fixed by extending the type-checker rule
     that simplification 3 already established for fresh constructor-call
     members, rather than teaching the runtime partition function to
     recurse.
- Alternatives:
  - Modeling `BaseException` as a real second root, so
    `BaseExceptionGroup`/`KeyboardInterrupt`/etc. are not conflated with
    `Exception`: rejected for #542 specifically because pycc has no other
    `BaseException`-only class today, so the extra branch would exist for
    exactly one class pair with no test surface distinguishing it from the
    chosen simplification; revisit alongside whatever issue first adds a
    real non-`Exception` `BaseException` subclass.
  - Accepting a dynamic (non-literal) member list by lowering it as a
    runtime-built dynamic array: rejected because pycc has no heap-backed
    dynamic list-of-exception-pointers type to lower it into yet, and
    building one solely for this call site would be a materially larger
    change than #542's own scope.
  - Widening MIR/codegen so a fresh constructor-call member could be
    lowered in place (allocating the new exception object inline as part of
    building the group): rejected as unnecessary scope growth once the
    narrower type-checker fix produces a working, spec-compliant surface for
    every literal-member-list use of group construction; a name-bound
    existing exception is one extra statement away from any test that
    wants a fresh one.
  - Preserving original subclass identity through
    `pycc_rt_exception_group_partition` by threading the raised object's own
    tag/name into each reconstructed subgroup: rejected because pycc has no
    way for a program to construct or observe a genuine `ExceptionGroup`
    subclass in the first place, so the extra runtime plumbing would have no
    reachable test.
  - Merging a clause-body raise into the group's remainder (CPython's
    derived-exception-group semantics): rejected as a materially larger
    codegen change (a new merge-and-continue control-flow shape distinct
    from every other statement's exception propagation) for a difference
    that is only observable once a program both raises from inside an
    `except*` clause and inspects the resulting group's shape -- a
    combination no accepted conformance fixture exercises.
  - Teaching `pycc_rt_exception_group_partition` to recurse into a member
    that is itself a group, matching CPython's `split()`: rejected as
    unnecessary scope growth once the narrower type-checker rejection
    produces a working, spec-compliant surface for every literal-member-list
    group construction that does not itself nest groups; nothing in this
    PR's scope needs a program to build a group whose member is another
    group.
- Consequences: `except*`/`ExceptionGroup`/`BaseExceptionGroup` land as a
  real, tested, spec-referenced surface for the common case -- catching and
  raising literal groups of existing exceptions -- without requiring a
  materialized exception instance, a dynamic list type, or a `BaseException`
  second root, all of which remain open work tracked elsewhere (#703, D-105).
  The six simplifications are all deliberately observable narrowings (a
  `T0021` diagnostic, or a fixed reconstructed identity) rather than silent
  behavioral divergences from CPython: every rejected program produces a
  clear compile-time diagnostic instead of a wrong runtime result, and every
  accepted program's runtime behavior for group re-tagging and clause-body
  raises is documented here and in `docs/RUNTIME.md` rather than left
  implicit. Revisiting any one of the six (a real `BaseException` root, a
  dynamic member list, in-place member construction, subclass-preserving
  partition, CPython-style derived-group chaining, or recursive nested-group
  partitioning) is additive: none requires reverting the other five.
