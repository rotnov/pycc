---
id: D-237
title: "Reject string conversion of a non-dataclass, non-exception instance and of a protocol-typed value with C0001"
status: accepted
---

## D-237: Reject string conversion of a non-dataclass, non-exception instance and of a protocol-typed value with `C0001`

- Status: accepted
- Context:
  Exactly two surfaces hand a value to `pycc_codegen`'s `to_str`: a `print()`
  argument and an f-string interpolation (the `str()` builtin is already
  `C0001` through `KNOWN_CALLABLE_BUILTINS`). `to_str` renders scalars and
  panics on a `Scalar::Instance`. An instance-typed value avoids that panic
  only through one of two MIR rewrites, both applied at those two sites with
  the exception rewrite first: `rewrite_instance_to_repr` (fires only when the
  value's *own* class has `is_dataclass == true`) and
  `rewrite_exception_to_message` (fires only when `exception_type_tag` resolves
  -- the flat seven builtin exception names *by name*, everything else through
  the class table's D-189 tag).

  Before this decision `pycc_types` placed no restriction on either surface,
  so [#977](https://github.com/rotnov/pycc/issues/977)'s program passed `pycc
  check` and panicked in `pycc build`. Re-verified at `f8e9d2e3` against
  CPython 3.14.6 (the pinned 3.14.7 oracle was not on PATH in the delivering
  session; `object.__repr__`'s form is unchanged between them), every one of
  these shapes passed `check`:

  | program | pycc at `f8e9d2e3` | CPython 3.14.6 |
  |---|---|---|
  | `class C` with `__init__`; `print(C(1))` (the issue) | codegen panic | `<__main__.C object at 0x...>` |
  | `class C: pass`; `print(C())` (D-225 implicit constructor) | codegen panic | `<__main__.C object at 0x...>` |
  | plain class with a user `__repr__`; `print(C(1))` | codegen panic | `C()` |
  | `f"{a}"`, `print("x", a, 1)`, module-level `print(a)`, `print(self)` | codegen panic | `<__main__.C object at 0x...>` |
  | `Enum` member: `print(Color.RED)`, `f"{Color.RED}"` | codegen panic | `Color.RED` |
  | non-dataclass subclass of a dataclass: `class Q(P): pass`; `print(Q(1))` | codegen panic | `Q(x=1)` |
  | plain generic class instance `Box[int](1)` | codegen panic | `<__main__.Box object at 0x...>` |
  | `Protocol`-typed parameter `def show(x: Shape): print(x)`, called with a dataclass and a plain class | codegen panic | `Sq(s=2)` then `<__main__.Plain object at 0x...>` |
  | `class ValueError: ...`; `print(ValueError())` | **null-pointer dereference at runtime** | `<__main__.ValueError object at 0x...>` |
  | `@dataclass class ValueError: x: int`; `print(ValueError(1))` (likewise `Exception`, `TypeError` under `f"{v}"`) | **misaligned-pointer abort at runtime** | `ValueError(x=1)` |
  | `class MyErr(Exception): def __init__(self) -> None: return`; `print(MyErr())` | **null-pointer dereference at runtime** | `` (empty message) |
  | `@dataclass class ExceptionGroup: x: int` + `except* ValueError as eg: print(eg)` (or `f"{eg}"`) | **SIGSEGV** | the group's `repr` |
  | `@dataclass class BaseExceptionGroup: x: int` or `class OSError: LIMIT: int = 1`, + `except* ValueError as eg: print(eg)` | codegen panic | the group's `repr` |

  Only a protocol-typed *parameter* reaches the checker as `Ty::Protocol`: a
  protocol-annotated local is bound to its inferred concrete class by D-040's
  sticky representation. The `except*` binding is typed
  `Ty::Instance("ExceptionGroup")` unconditionally, whatever the module
  contains. The shadow gate (`crates/pycc_hir/src/module.rs`) withholds the
  seeded builtin class table for all 25 names when the module binds any one
  of them, and `Environment::is_synthetic_class` (D-188) is the crate's
  established provenance test for "did the user shadow this name".

  This is the [D-198](D-198-cast-erasure-limits-cast-to-representation.md)
  "check passes, backend fails" class that
  [D-236](D-236-reject-a-class-attribute-named-after-the-instantiation.md)
  also frames. `docs/DIAGNOSTICS.md`'s `C0001` rule (a versioned capability
  diagnostic naming the construct in Python terms) governs the rejection;
  no accepted decision states a general "reject rather than approximate"
  rule, so this entry records the choice for this instance on its merits.

- Decision:
  One fail-closed predicate, `pycc_types::string_conversion::reject_unrenderable`,
  applied in `infer_expr_in` at the `HirExpr::FString` arm (per
  interpolation) and at the `print` arm (over every inferred argument). It is
  a conservative under-approximation of what the two MIR rewrites render,
  decided by **name provenance first, shape second**:

  | type of the argument / interpolation | verdict |
  |---|---|
  | `Ty::Instance(c)`, `c` one of the 25 builtin exception names, `env.is_synthetic_class(c)` (seeded) | accept |
  | `Ty::Instance(c)`, `c` one of the flat seven, not synthetic, absent from the class table (seeding withheld) | accept -- MIR still resolves the flat seven by name |
  | `Ty::Instance(c)`, `c` any builtin exception name, present but user-authored (plain **or `@dataclass`**) | reject `C0001` (divergence 1) |
  | `Ty::Instance(c)`, `c` a non-flat builtin exception name (`OSError` family, `Base`/`ExceptionGroup`), not synthetic, absent | reject `C0001` -- no name fallback, so no rewrite |
  | `Ty::Instance(c)`, `c` not a builtin exception name, present, `is_dataclass` | accept |
  | `Ty::Instance(c)`, `c` not a builtin exception name, present, not a dataclass -- including a user exception class carrying a D-189 tag | reject `C0001` (divergence 2) |
  | `Ty::Instance(c)`, `c` not a builtin exception name, absent (only `pycc_types::check_function`'s empty class table) | reject `C0001` |
  | `Ty::Protocol(_)` | reject `C0001` with its own message |
  | everything else | unchanged |

  The two divergences from MIR's own resolution are deliberate. A user class
  under any of the 25 builtin names is rejected whatever its shape because
  MIR resolves the name before the shape: for a flat-seven name it rewrites
  the plain instance to `ExceptionMessage` (the null-dereference and the
  misaligned-pointer abort above), and for `ExceptionGroup` the `except*`
  binding is typed by that name unconditionally, so a user `@dataclass`
  under it gets the dataclass `__repr__` applied to an exception object (the
  SIGSEGV). A user exception class carrying a D-189 tag is rejected because
  MIR's class-table fallback would rewrite its plain `PyInstanceObj` to
  `ExceptionMessage` and abort. The predicate decides the 25 names by
  provenance and every other name by shape, never by a tag.

  Messages: ``string conversion of a `C` instance as a `print()` argument is
  not supported yet; `print()` and f-string interpolation can render only a
  `@dataclass` instance or a caught builtin exception`` (help: ``print the
  instance's attributes individually, or declare `C` with `@dataclass` to get
  a synthesized `__repr__` ``), with `an f-string interpolation` at the other
  site; and ``string conversion of a value typed as protocol `Shape` as a
  `print()` argument is not supported yet; the concrete class is not known at
  the conversion site``. One message template keyed on the class or protocol
  name; no enum-member special case. The help alone has a second form for a
  class under one of the 25 builtin exception names, which the predicate
  rejects by name before its shape is consulted, so `@dataclass` cannot make
  it renderable: ``print the instance's attributes individually, or rename
  `ValueError` so it no longer shadows the builtin exception `ValueError`;
  adding `@dataclass` does not make a class under a builtin exception name
  renderable``.

  The gate judges the *expression*, not only its type: `pycc_mir` erases
  `cast(T, v)` to `v` and its `__repr__` rewrite keys on the erased value's
  own class, so a representation-preserving upcast from a non-dataclass
  subclass to a `@dataclass` base (`print(cast(Base, d))` with `class
  Derived(Base): pass`) passes `check_cast`, reads as `Base` to a type-only
  gate, and still reaches codegen's `to_str` panic as a `Derived`.
  `reject_unrenderable_expr` therefore re-judges the value under every
  erased `cast` (nested casts included; a user `def cast` is an ordinary
  call and is not looked through), naming the erased class in the message.
  Binding the cast first (`b: Base = cast(Base, d); print(b)`) is a
  different, pre-existing codegen failure (`local type drifted`) outside
  this decision.

  A generic `@dataclass` (`@dataclass class Box[T]: n: int`) must render in
  every monomorphized specialization. `check` infers `print(Box[int](1))`
  against the origin class and accepts it; `build` re-infers the rewritten
  call against the `0gen_Box__T_int` specialization, whose `HirClassDef`
  `instantiate_generic_class_methods` used to build with `is_dataclass:
  false` and an empty `dataclass_fields`, so the gate rejected under `build`
  a program `check` had accepted (and, before the gate, `pycc_mir`'s
  `rewrite_instance_to_repr` skipped the specialization for the same reason
  and codegen panicked). The specialization now carries the origin's
  `is_dataclass` and its `dataclass_fields` substituted exactly as `attrs`
  are; its `methods` already carried the mangled synthesized `__repr__`, so
  the MIR rewrite renders it through the origin's name (`Box(n=1)`), as
  CPython does. Reported at this crate's conventional
  `(0, 0)` span (rendered `1:1`): `pycc_types` carries no expression spans,
  and D-233's HIR-level syntactic scan cannot apply because the value's
  *type* is unknown at HIR. No mirror in the solver pass: every body is
  walked by the check pass after it, and `monomorphize` delegates to
  `infer_expr_in`, so the check pass alone covers every path. No
  `crates/pycc_hir` class file changes. The `Scalar::Instance` arm of
  `to_str` stays as defence-in-depth; only comments change in `pycc_mir`
  and `pycc_codegen`.

- Alternatives:
  - *Model CPython's default `<module.Class object at 0x...>` rendering.*
    The address is not stable even across CPython runs, so it can never be a
    byte-for-byte conformance target; it would also need module-name
    plumbing into `pycc_rt` and a new runtime formatter for a rendering no
    correct program depends on.
  - *Honour a user-defined `__repr__` on a plain class now.* The MIR rewrite
    deliberately refuses a non-dataclass `__repr__` (arity and return type
    unverified). A correct positive path must type-check `__repr__`'s
    signature, extend D-236's reserved-name machinery, and change
    `rewrite_instance_to_repr` -- a HIR+MIR feature across three seams,
    independently mergeable after this rejection. The `userrepr` tests pin
    the current rejection so that later change is a visible flip.
  - *Mirror MIR's resolution exactly (name-first).* Would accept `class
    ValueError: ...; print(ValueError())` and preserve the null dereference.
    *Shape-first* (`is_dataclass` before the name) would accept the shadowing
    dataclass and preserve its abort. Provenance is the correct test.
  - *Reject in HIR lowering with real spans.* Impossible: whether `print(x)`
    is fine depends on `x`'s type, unknown at HIR.
  - *Reject in `pycc_mir`.* Too late: a MIR panic is not a diagnostic, and
    `pycc check` never runs MIR.
  - *Accept `Ty::Protocol` and rely on monomorphization.* Keeps a
    `check`-passes/`build`-panics program whenever any call site passes a
    non-dataclass.
  - *Remove the codegen panic arm.* Kept: it is defence-in-depth and its unit
    test hand-builds a `Scalar::Instance`.
  - *Also teach the constraint solver the class table.* When the concrete
    pass rejects a program that *calls* a user class declared under a builtin
    exception name (`print(ValueError(1))`), `check_all_keyed` falls through
    to the solver, whose `ConstraintEnvironment` has no class table and
    classifies that call as ``call to builtin `ValueError` ``; under D-220's
    solver-first merge that pre-existing message is the one `pycc check`
    prints for those shapes (still `C0001`, still naming the class). Fixing
    the solver's classification is an independent seam (every
    `ConstraintEnvironment` literal, and it already misreports a solver-path
    module that instantiates a user class shadowing a builtin name at
    `f8e9d2e3`), so it is left to its own change; the predicate's own
    verdict on those shapes is pinned directly in its unit tests. The
    rename help reaches `pycc check` only for a shape without such a call,
    for instance `print(self)` inside a method of the shadowing class, which
    is the shape the public-CLI test pins.
  - *Resolve a `0gen_`-prefixed specialization through its origin class in
    the predicate.* Rejected in favour of carrying the dataclass metadata on
    the specialization itself: `pycc_mir`'s `__repr__` and `__eq__` rewrites
    and `pycc_types`'s `==`/`!=` acceptance all key on the specialization's
    own `is_dataclass`, so a predicate-only fix would leave `build` panicking
    where `check` passes -- the exact split this decision exists to close.

- Consequences:
  - Programs that `check`ed clean and panicked in `build` (or aborted at
    runtime) now fail `check` with `C0001`; `pycc run` reports the diagnostic
    and never reaches the backend.
  - The `Protocol` rejection is stricter than a per-call-site analysis could
    allow: `show(Sq(2))` alone would have run.
  - The one program class that rendered at `f8e9d2e3` and is newly rejected
    is a user `@dataclass` under a non-flat builtin exception name
    (`FileNotFoundError`, `ExceptionGroup`, ...) instantiated and printed
    directly (`FileNotFoundError(x=3)`). Accepted because the name must be
    decided before the shape, and the shape-first form re-opens the
    `except*` SIGSEGV for the same name; pinned by a test so the loss stays
    visible.
  - The checker is stricter than MIR for the two divergence families. A
    future "honour user `__repr__`" slice, or Part 3 of #541 (#703, real
    exception instances), must widen the checker first and revisit MIR's
    name-first and class-table resolution rather than the checker alone.
  - Adjacent runtime defect observed and not fixed here: rendering a caught
    builtin exception *twice* (`print(e); print(e)` or `print(e); print(f"{e}")`)
    prints the message once and then aborts in `pycc_rt`'s `pycc_rt_str_decref`
    (a second decref of the message string), for the flat seven and the
    `OSError` family alike. The positive tests here render each caught
    exception exactly once. Recorded in the session file for `issue-select`.
