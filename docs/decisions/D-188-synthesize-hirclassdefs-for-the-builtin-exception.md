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
  `HirItem::Function`; the other six inherit it through their MRO. Five
  properties make that safe:
  - **Seeded only into a module that references one of the names.** The
    frontend's per-item work is proportional to the size of the class table
    -- the projected class slice `annotation_to_ty` consumes, the
    name-collision checks, and `pycc_types`' per-function `bind_classes` --
    so seven unconditional entries cost every module, including one with no
    classes at all. Measured on `benches/check_bench.rs`'s class-free
    fixture, unconditional seeding tripled the whole `parse` +
    `lower_checked` + `check` time (7.85 us to 23.69 us), which the
    `frontend-perf-gate` correctly rejected. A module that never spells one
    of the seven cannot distinguish a seeded class table from an empty one,
    so it is seeded with none of them and the gate stays green (7.90 us,
    +0.6%). The reference scan is `pycc_ast::visitor::Visitor`, ruff's own
    generic AST walker, rather than a hand-rolled `Stmt`/`Expr` match: a
    spelling missed by a hand-rolled scan fails *silently*, as a spurious
    `C0001` or an internal-compiler-error abort. Overriding a single
    `visit_expr` makes every name-bearing position reachable by construction
    and keeps new upstream AST nodes covered automatically. Crucially,
    absence is not shadowing -- see the third property -- so the gate can
    only ever withhold definitions from a module that could not have used
    them.
  - **All-or-nothing per module.** A module whose own top level binds any of
    the seven names is seeded with none of them, so every existing
    name-collision check in `lower_checked` applies to the synthetic
    definitions with no exemption, and a user's `class ValueError:` keeps its
    ordinary meaning. This gate is deliberately *not* fused into the
    reference scan's single pass: shadowing is a property of a module's top
    level only, while a reference counts at any depth.
  - **A provenance side table, not a flag.** `Environment` carries
    `synthetic_classes: Arc<HashSet<String>>`, maintained by `bind_class` --
    the sole mutator of `classes` -- rather than a field on `HirClassDef`.
    Who authored a definition is a fact about the environment, not part of a
    class's declared shape.
  - **Shadowing is redefined in terms of provenance.**
    `is_unshadowed_builtin_exception` used to read "absent from `classes`" as
    "not shadowed"; seeding inverts that. It now reads "present *and not
    synthetic*", so the pre-existing `except`/`raise` surface is unchanged.
    Note the direction: `is_user_defined_class` is
    `classes.contains_key(name) && !is_synthetic_class(name)`, so an *absent*
    name is not user-defined and therefore reads as un-shadowed -- exactly its
    pre-#541 meaning. That is what makes the reference gate safe. The four
    paths that instead need a seeded class to be *present* -- base resolution,
    `class::expect_class` behind a `Ty::Instance`, `annotation_to_ty`
    projection, and `isinstance`/`issubclass` -- each require one of the seven
    to be spelled in the module, so the *reference* gate never withholds a
    definition from a module that could have reached one. The *shadow* gate
    still can, because it is all-or-nothing across the group: a module whose
    top level binds `Exception` and whose body writes
    `except ValueError as e: print(e.args)` is seeded with nothing and
    reaches `expect_class`'s internal-error panic, even though the user never
    redefined `ValueError`. That is a known open gap in this decision rather
    than a property of the reference gate -- it reproduces identically at the
    commit that first introduced the seeding, before the reference gate
    existed. Closing it is Part 2's, since it requires deciding what a
    partially shadowed builtin hierarchy means; the alternative available
    today, per-name seeding, was rejected above for making the hierarchy
    incoherent.
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
  - *Seed unconditionally and make the construction cheap.* Rejected on
    measurement. Caching the seven definitions in a `LazyLock` removed ~10.4
    us of the ~13.2 us the seeding added to `pycc_types::check` on
    `benches/check_bench.rs`'s fixture -- a real fix, kept -- but the
    irreducible remainder (seven `HirClassDef` clones per `bind_classes`, the
    per-item projected class slice) still left the fixture at 12.5 us against
    a 7% budget of 8.4 us, roughly ten times over. No amount of making the
    construction cheaper closes a gap that size; only not doing the work does.
  - *Fuse the reference scan into the shadow scan's existing pass.* Rejected:
    the two questions have different scope semantics. Shadowing is about
    top-level bindings only -- a `class ValueError:` nested inside a function
    shadows nothing at module scope -- while a reference counts at any depth.
    One pass answering both would have to either widen shadowing or narrow the
    reference scan, and both errors are silent.
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
  named \`args\`` instead of aborting with an internal compiler error, and
  attribute access on the bare class name (`ValueError.args`) reports the
  same `T0044` where it previously reported `T0021 name \`ValueError\` is
  not defined` -- both only for a module the two gates actually seed; a
  module whose top level binds any one of the seven names is seeded with
  none of them and the `expect_class` abort remains reachable there, as
  recorded under the shadowing property above;
  giving the synthetic classes real attribute slots is out of scope here
  for the reason recorded in `docs/RUNTIME.md`. `e = ValueError("x")` is
  unchanged -- still `C0001`, from the same earlier check. A module that references one of
  the seven names has its `class_defs` grow by seven entries, so any consumer
  that reasons about `class_defs.len()` or its ordering must account for them;
  the module's own classes still come first, in source order. A module that
  references none of them is unchanged, which is what keeps
  `frontend-perf-gate` and the D-084 absolute throughput budget green. The
  cost of the reference scan itself is one linear AST walk that short-circuits
  on the first match. This extends D-173 and
  supersedes nothing. It does not by itself make user-defined exception
  classes raisable or catchable -- that is Part 2 of
  [#541](https://github.com/rotnov/pycc/issues/541), which will decide how a
  user class maps onto D-173's runtime tag space.
