---
id: D-256
title: "Admit a foreign-callable `staticmethod` class attribute, re-read at each access"
status: accepted
---

## D-256: Admit a foreign-callable `staticmethod` class attribute, re-read at each access
- Status: accepted
- Context: [D-224](./D-224-restrict-class-level-attributes-to-scalar.md) makes every
  class-level attribute a scalar literal constant that never reaches `pycc_codegen`, and
  its scalar restriction is also what keeps [D-213](./D-213-defer-pep-487-full-invocation-reject-the.md)'s
  `__set_name__` deferral sound. The D-244 interop workload
  ([#1207](https://github.com/rotnov/pycc/issues/1207)) binds a CPython function as a
  class attribute, `exists = staticmethod(os.path.exists)`, and calls it through the
  class or an instance. [#1284](https://github.com/rotnov/pycc/issues/1284) asks for
  that shape. Part 1 ([#1345](https://github.com/rotnov/pycc/issues/1345)) admits it
  alone. A function body cannot yet read a module-level CPython object global
  ([#1333](https://github.com/rotnov/pycc/issues/1333)), so the attribute cannot be
  captured once into hidden storage.
- Amendment (2026-09-30): Part 1 of #1333 ([#1362](https://github.com/rotnov/pycc/issues/1362))
  admits a function-body read of a module-level CPython object global, so the premise
  above no longer holds. The decision is unchanged; the eager-capture alternative is now
  unblocked and remains Part 5 (#1349).
- Decision:
  - One non-literal class-attribute shape is admitted. It is the un-annotated
    `name = staticmethod(<ref>)`, where `<ref>` is a bare name or a dotted attribute
    chain. Its root must be an unconditional module-level foreign (CPython) import that
    precedes the class. The value is `ClassAttrValue::ForeignStatic`, typed
    `Ty::Object`. It has no storage.
  - A read or a call through the attribute is rewritten at the use site into the
    reference itself. That rewritten expression is type-checked and lowered by the
    existing object paths (`ObjAttrGet`, `ObjMethodCall`, `ObjCall`). `pycc_codegen` and
    `pycc_rt` do not change.
  - Every other non-literal shape gets its own `C0001`. A deferred shape names its
    tracking issue: a call, a name or attribute reference, a container, `classmethod(...)`,
    a pycc function root, a conditional import, the annotated spelling, and a dunder,
    class-private or container-dispatched name. Two shapes stay refused with no tracking
    issue, because CPython itself rejects or rebinds them: the wrong argument count or kind
    for `staticmethod(...)`, and a rebound `staticmethod`.
  - The MRO rule is positional. Both `pycc_types` and `pycc_mir` apply it through the
    shared `pycc_hir` helpers (`crates/pycc_hir/src/class/foreign_static.rs`).
    - A class-name read or call reaches the attribute when the attribute is the first
      class-level binding of the name in the MRO.
    - An instance read or call also requires that no class in the MRO has an instance
      slot of that name.
    - An instance read whose winner is a method, with a foreign attribute later in the
      MRO, is refused (#1350).
    - An instance access is refused when any subclass of the receiver's static class
      resolves the name to a different winner and either winner is the foreign attribute.
      A name the receiver's own class never binds keeps the ordinary unknown-attribute
      refusal instead.
      pycc resolves the member statically, so the subclass case would otherwise
      miscompile (#1337).
    - A non-name instance receiver and a `super()` receiver are refused (#1346).
    - The root must not be shadowed by a local of the enclosing function at the use
      site. A comprehension target with the root's name is not refused: the rewritten
      chain still reads the module-level import, which matches CPython's result there
      (measured at module and function scope).
  - D-213 stays sound. A CPython `staticmethod` object has no `__set_name__`, measured on
    3.14.7: `hasattr(staticmethod(len), '__set_name__')`, the same check on its type,
    and the same check on `os.path.exists` are all `False`. So this descriptor-valued
    attribute cannot trigger PEP 487's hook.
  - Two divergences from CPython are documented rather than refused, because neither is
    statically detectable. Both are removed by eager capture in Part 5
    ([#1349](https://github.com/rotnov/pycc/issues/1349)).
    - A missing attribute (`staticmethod(os.path.nope)`) raises `AttributeError` at the
      first access, not at import.
    - A later monkeypatch of the attribute chain is observed.
- Alternatives:
  - *Capture the callable eagerly at class creation into a hidden module global.* This
    matches CPython exactly, but a function body cannot read a module-level object global
    yet (#1333). It is deferred to Part 5 (#1349).
    - Amendment (2026-09-30): that blocker is gone since Part 1 of #1333 (#1362), which
      admits the read; the capture itself is still Part 5 (#1349).
  - *Give class attributes real per-class storage.* This is D-224's rejected
    alternative, and nothing about it has changed.
  - *Admit `staticmethod` of a pycc function, or `classmethod`.* A pycc callable has no
    CPython object to re-read, and its dispatch would have to join the method tables.
    Tracked by #1347.
- Consequences:
  - The shape the #1207 workload uses compiles with CPython's own results, with no new
    runtime entry point.
  - D-224's "every class attribute is a scalar literal constant that never reaches
    codegen" now has exactly one exception. Codegen still sees only the rewritten
    expression, never the attribute.
  - The attribute is not published on an `--ext` extension type, like every other class
    attribute.
  - Widening further (a pycc callable, eager capture, a non-name receiver) needs a new
    entry that re-examines D-213 again.
