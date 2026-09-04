---
id: D-224
title: "Restrict class-level attributes to scalar compile-time constants"
status: accepted
---

## D-224: Restrict class-level attributes to scalar compile-time constants
- Status: accepted
- Context: [#885](https://github.com/rotnov/pycc/issues/885) asks for class-level
  attributes (`class Config: MAX: int = 100`), and its Part 1
  ([#911](https://github.com/rotnov/pycc/issues/911)) delivers the annotated form
  plus `typing.ClassVar` registration. Two existing constraints bound the design.
  [D-154](./D-154-instance-attribute-slot-storage-is-one-i64-word.md) gives an
  instance attribute exactly one `i64` word, with no representation for a heap
  object, tuple, `None`, or class instance — the same reason a dataclass field is
  already restricted to `int`/`float`/`bool`/`str`.
  [D-213](./D-213-defer-pep-487-full-invocation-reject-the.md)/[#585](https://github.com/rotnov/pycc/issues/585)
  defers PEP 487's `__set_name__`, and until now the reason it could not fire was
  simply that class-level attribute assignments did not exist at all. Landing them
  removes that reason, so the deferral needs a new, explicitly-implemented
  precondition rather than an accidental one.
- Decision: a class-level attribute is a **compile-time constant**, not a storage
  location. Its annotation must resolve to a scalar slot type — `int`, `float`,
  `bool`, or `str` — and its initializer must be a literal of that type (an `int`
  literal widens under a `float` annotation; a unary `+`/`-` on a numeric literal is
  folded). Anything else is rejected with `C0001` at HIR-lowering time. The value is
  carried in `HirClassDef::class_attrs` and folded into a MIR literal at every read
  site; it consumes no instance slot, never enters `mro_attrs`/`mro_attr_count`, and
  reaches neither `pycc_codegen` nor `pycc_rt`. Every write to it is rejected with
  `T0044`, and a read is restricted to a plain-name base so folding cannot discard a
  side-effecting base expression. A generic type parameter (`Ty::Param`) is rejected
  even though it is a scalar slot type elsewhere, because a type parameter has no
  constant value to fold. The scalar restriction is written into the code as the
  named `__set_name__` invariant: a descriptor is not a scalar, so a
  descriptor-valued class attribute — CPython's only trigger for the hook — cannot
  be constructed, and D-213's deferral stays sound by rejection rather than by
  absence.
- Alternatives:
  - *Give a class attribute real storage (a per-class static)*. This would admit
    non-scalar values eventually, but it needs a class-object representation in
    `pycc_rt` that does not exist, re-opens `__set_name__` immediately, and makes
    the write paths meaningful — a far larger change than #885 Part 1, with no
    consumer waiting on it.
  - *Accept any annotation and reject only at the fold site*. This pushes a
    capability error from HIR to MIR, past the type checker, and produces a
    diagnostic without a useful span. It also leaves `__set_name__`'s precondition
    implicit in `pycc_mir` rather than stated where the class model is built.
  - *Keep the blanket ban on class-body assignments and defer #885 entirely*. This
    keeps D-213 sound for free but leaves the single most common Python class idiom
    unsupported, which is the whole point of #885.
- Consequences: the accepted surface is narrow and its narrowness is load-bearing
  rather than incidental — widening it later (a non-scalar class attribute, a
  descriptor) requires re-examining D-213 in the same change, which is exactly the
  coupling this entry makes explicit. In exchange, the feature costs nothing at run
  time: no allocation, no slot, no codegen change, and reads are free. Four
  limitations ship as tracked follow-ups rather than silent gaps: `ClassVar` inside
  a `@dataclass` body, `Final[...]` on a class-body attribute, class attributes
  satisfying `Protocol` members, and `super().CLASS_CONST`. Un-annotated class-body
  assignments (`X = 1`) stay unsupported as #885's Part 2.
