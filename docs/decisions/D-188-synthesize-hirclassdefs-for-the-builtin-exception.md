---
id: D-188
title: "Synthesize HirClassDefs for the builtin exception classes"
status: accepted
---

## D-188: Synthesize HirClassDefs for the builtin exception classes
- Status: accepted
- Context: [D-173](D-173-per-thread-exception-state-with-explicit-check.md) gave
  the compiler seven builtin exception names (`Exception`, `ValueError`,
  `TypeError`, `KeyError`, `IndexError`, `ZeroDivisionError`,
  `RuntimeError`) recognized *by name only*, through
  `pycc_hir::is_builtin_exception_class`. No `HirClassDef` stood behind any
  of them, so they were absent from `HirModule::class_defs` and from
  `pycc_types::Environment::classes`. Three defects followed from that
  absence, each reproduced against the pre-change revision. A builtin
  exception could not be subclassed at all: `class MyError(ValueError):`
  was rejected with `C0001 class \`MyError\` inherits from unknown class
  \`ValueError\``, because base resolution consults the class table.
  Reaching the class table through a `Ty::Instance` naming one of the seven
  crashed the compiler: `except ValueError as e:` binds `e` to
  `Ty::Instance("ValueError")`, and `print(e.args)` then panicked in
  `class::expect_class` with `internal error: class \`ValueError\` has no
  registered HirClassDef -- Environment::classes was built from a different
  HirModule than the one this Ty::Instance came from`, an
  internal-compiler-error abort rather than a diagnostic. And
  `isinstance`/`issubclass` and annotations naming a builtin exception class
  had no definition to resolve against. What the absence did *not* cause is
  a silent miscompile: `e = ValueError("x")` was, and remains, rejected by
  `constraints.rs`'s `KNOWN_CALLABLE_BUILTINS` check with `C0001 call to
  builtin \`ValueError\` is valid Python but not implemented yet`, so no
  builtin-exception value ever reached MIR or codegen.
- Decision: HIR lowering synthesizes a real `HirClassDef` for each of the
  seven names and seeds them into `HirModule::class_defs` before any user
  statement is lowered, deriving both the set and the parent links from the
  existing `BUILTIN_EXCEPTION_CLASSES`/`builtin_exception_parent` pair so the
  hierarchy keeps exactly one source of truth. `Exception` carries a
  synthetic `__init__(self, message: str)`, emitted as an ordinary mangled
  `HirItem::Function`; the other six inherit it through their MRO. Four
  properties make that safe:
  - **All-or-nothing per module.** A module whose own top level binds any of
    the seven names is seeded with none of them, so every existing
    name-collision check in `lower_checked` applies to the synthetic
    definitions with no exemption, and a user's `class ValueError:` keeps its
    ordinary meaning.
  - **A provenance side table, not a flag.** `Environment` carries
    `synthetic_classes: Arc<HashSet<String>>`, maintained by `bind_class` --
    the sole mutator of `classes` -- rather than a field on `HirClassDef`.
    Who authored a definition is a fact about the environment, not part of a
    class's declared shape.
  - **Shadowing is redefined in terms of provenance.**
    `is_unshadowed_builtin_exception` used to read "absent from `classes`" as
    "not shadowed"; seeding inverts that. It now reads "present *and not
    synthetic*", so the pre-existing `except`/`raise` surface is unchanged.
  - **A synthetic class is still not a value.**
    `class::resolve_instantiation` rejects instantiating one with `C0001`,
    keyed on the side table rather than on the name. Seeding otherwise
    routes `ValueError("x")` into the ordinary instantiation path, whose
    MRO walk would then panic on the missing constructor item; the guard
    turns that into a diagnostic. It is a defense-in-depth guard rather
    than a user-visible message, because `KNOWN_CALLABLE_BUILTINS` rejects
    all seven names earlier in `constraints.rs` -- reconciling the two
    messages is left to Part 2. A user class shadowing one of the names
    stays instantiable.
  The constructor *body* is emitted only when some user class's MRO actually
  reaches a seeded class, so an ordinary module gains no dead function.
- Alternatives:
  - *Keep name-only recognition and special-case each consumer.* Rejected:
    it is the status quo, and every new consumer of the class table has to
    rediscover that these seven names are classes that are not in the class
    table.
  - *Add an `is_synthetic: bool` to `HirClassDef`.* Rejected: it would have
    to be threaded through every `HirClassDef` literal in the tree, and it
    encodes provenance into a structure that otherwise describes only a
    class's declared shape.
  - *Derive "synthetic" by counting, or by name alone.* Rejected: name alone
    cannot tell a synthetic `ValueError` from a user's own, which is exactly
    the distinction the instantiation guard and the shadowing predicate both
    need. The all-or-nothing seeding makes structural comparison against
    `builtin_exception_class_defs()` exact instead.
  - *Give `Exception` CPython's `__init__(self, *args)`.* Rejected: this
    compiler has no variadic-argument support, and the supported surface is
    exactly one `str` message. The divergence is recorded in
    `docs/RUNTIME.md`.
  - *Give the synthetic classes a `message` attribute slot.* Rejected: D-173
    propagates a raised exception through global runtime state, not through
    an allocated instance, so there is no storage a slot could name.
- Consequences: `class MyError(ValueError):` and `MyError("boom")` now
  type-check, `issubclass(MyError, Exception)` evaluates, and builtin
  exception classes are usable in annotations. `except ValueError as e:
  print(e.args)` now reports `T0044 class \`ValueError\` has no attribute
  named \`args\`` instead of aborting with an internal compiler error;
  giving the synthetic classes real attribute slots is out of scope here
  for the reason recorded in `docs/RUNTIME.md`. `e = ValueError("x")` is
  unchanged -- still `C0001`, from the same earlier check. Every module's
  `class_defs` grows by seven entries, so any consumer that reasons about
  `class_defs.len()` or its ordering must account for them; the module's own
  classes still come first, in source order. This extends D-173 and
  supersedes nothing. It does not by itself make user-defined exception
  classes raisable or catchable -- that is Part 2 of
  [#541](https://github.com/rotnov/pycc/issues/541), which will decide how a
  user class maps onto D-173's runtime tag space.
