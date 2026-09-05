# pycc Diagnostics Registry

Every code: stable forever, documented via `pycc explain`, covered by at least one test in `tests/diagnostics/`. Format contract in [CLI_SPEC.md](./CLI_SPEC.md).

## Code ranges

| Range | Domain |
|---|---|
| `C0xxx` | compiler-version capability gaps for valid Python |
| `L0xxx` | lexing / parsing / grammar |
| `T0xxx` | type checking |
| `O0xxx` | ownership / memory / threading |
| `E01xx` | unsupported dynamic Python (rejected by design) |
| `I04xx` | CPython interop boundary |
| `W1xxx` | warnings (deprecations, perf hints) |

## Initial registry

| Code | Severity | Message (short form) |
|---|---|---|
| `C0001` | error | valid Python construct is not implemented by this pycc version |
| `C0002` | error | recognized stdlib module, but the specific imported symbol is not registered (D-136), e.g. `from math import isnan` |
| `L0001` | error | syntax error (span + expected set) *(also reused, without an "expected set", for a post-parse context violation caught during HIR lowering -- `break`/`continue` outside a loop, `async for` outside an async function (D-148), `yield`/`yield from` outside a function (D-149), `return`/`break`/`continue` inside a `finally` block that would exit it (PEP 765, D-193, Part 1 of #543), `return`/`break`/`continue` inside an `except*` clause body (PEP 654, #795), and a `from __future__ import ...` CPython rejects -- an unknown feature name including `*` (`future feature <name> is not defined`), `braces` (`not a chance`), or a future import after the docstring-and-future-imports prologue (`from __future__ imports must occur at the beginning of the file`) (#919, D-229) -- since CPython classifies all of these as `SyntaxError` (a `SyntaxWarning` as of CPython 3.14 for the `finally` case specifically) too; `T0024` ("`return` outside a function") is the closest existing precedent for this same "keyword valid only in a specific context" family but lives under `T0xxx` instead, an accepted, deliberately unfixed inconsistency -- see D-148)* |
| `L0002` | error | Python version mismatch (feature needs 3.14 level) |
| `T0001` | error | public function missing annotation |
| `T0002` | error | `Any` outside interop boundary |
| `T0003` | error | untyped empty container needs annotation |
| `T0021` | error | name resolution (including an unbound local), operand, call, or inference type mismatch; also the project-import failures CPython itself rejects (D-222): a name the imported module does not define, a relative import with no parent package or climbing above the top-level package, and a relative target that resolves to no module |
| `T0022` | error | return type mismatch |
| `T0023` | error | incompatible assignment |
| `T0024` | error | `return` outside a function |
| `T0025` | error | annotated-assignment initializer incompatible with its declared annotation |
| `T0026` | error | a later plain or annotated assignment is incompatible with an earlier value-less declaration (`x: int` with no `= ...`) |
| `T0030` | error | non-exhaustive `match` (missing cases listed) |
| `T0031` | error | `@override` without matching base method |
| `T0032` | error | list literal elements must all share one type |
| `T0033` | error | value does not support list/dict/set operations (subscript, slicing, item assignment, `for`, `.append()`, `.pop()`, `.get()`, `.add()`, `len()`), or `len()` called with the wrong number of arguments |
| `T0034` | error | list codegen only supports `list[int]` in v0.2 (D-105) — gates both an inferred list-literal element type and a written `list[T]` annotation (D-228) |
| `T0035` | error | dict literal key/value pairs must all share one key type and one value type |
| `T0036` | error | dict codegen only supports `dict[str, int]` in v0.2 (D-122) — gates both an inferred dict-literal key/value type and a written `dict[K, V]` annotation (D-228) |
| `T0037` | error | set literal elements must all share one type |
| `T0038` | error | set codegen only supports `set[int]` in v0.2 (D-122) — gates both an inferred set-literal element type and a written `set[T]` annotation (D-228) |
| `T0039` | error | tuple element type not compiled yet (int/bool/float only, D-116) — gates both an inferred tuple-literal element type and each element of a written `tuple[A, B, ...]` annotation (D-228) |
| `T0040` | error | tuple index must be a non-negative literal integer within range (D-116) |
| `T0041` | error | local name may not be bound on every path reaching this use (definite-assignment tracking, issue #118 Part 1, D-147) |
| `T0042` | error | generic-function shape or call-site instantiation rejected beyond the PR-13 v0.2 thin slice (D-133/D-134): more than one PEP 695 type parameter; a type parameter used in a container position (`list[T]`); a call site whose occurrences of the type parameter resolve to different concrete types; a call site whose argument for the type parameter is not `int`/`float`/`bool`/`str`; a generic function's body calling itself or any other generic function (recursive generic instantiation is outside D-134's single-call-site monomorphization slice); also reused (D-135) for a generic `type` alias declaring its own type parameters |
| `T0043` | error | attribute access, attribute assignment, or method call on a value that is not a class instance (D-154, Part 1 of #375) |
| `T0044` | error | class has no attribute or method with the given name (D-154, Part 1 of #375); a mismatched instantiation/method call-site argument list reuses `T0021` instead, exactly like an ordinary function call; also reported (PEP 560, #610/#611) when `ClassName[...]` is used — in value or annotation position — on a class that defines no `__class_getitem__` anywhere in its MRO and is not a PEP 695 generic class, matching CPython's `TypeError: type 'C' is not subscriptable` |
| `T0045` | error | cannot reassign a `Final` name (PEP 591, #383) — variable-level annotations only (module-level and function-local); `Final` on parameters or class-body attributes is out of scope |
| `T0046` | error | class does not conform to a protocol (PEP 544 structural typing, #380, D-166) — a concrete class is missing one or more of a protocol's required methods or attributes, or a method/attribute has an incompatible signature/type |
| `T0047` | error | `super()` does not proxy instance attributes (#587) — an attribute established by `self.<attr> = ...` is reachable only through `self`, matching CPython's `super` object, which proxies class-level attributes and descriptors along the MRO but not the instance's own attributes |
| `T0048` | error | general union type annotation is not supported yet (PEP 604, D-197, #763, Part 1 of #747) — a `X \| Y` annotation is accepted only in the exact `T \| None`/`None \| T` shape (a 2-operand union where one side is literally `None`); any other shape, including a longer `A \| B \| None` chain or a 2-operand union where neither side is `None`, is rejected |
| `T0049` | error | `Optional[T]` annotation is not supported yet for this `T` (PEP 604, D-197, #763, Part 1 of #747; widened to `float`/`bool` by #809, Part 3 of #747) — `T \| None` type-checks only for `T` in `{int, float, bool}`; every other inner type (`str`, `list[...]`, a class instance, or another `Optional[...]`) is a recognized but out-of-scope shape, distinct from `T0048`'s rejection of the general-union shape itself |
| `T0050` | error | walrus assignment (`:=`) value type is not supported yet (PEP 572, #774) — a walrus value is currently restricted to the non-reference-counted scalar types `int`, `float`, `bool`, `None`, and `Optional` of those, since codegen's `MirExpr::NamedExpr` value-yielding read is only worked out for those types' refcount contract |
| `T0051` | error | an `int` literal in one of the 13 named runtime `int`-boundary positions D-179 left in D-141's inventory (a `range()` argument is bigint-capable and excluded, D-179) is out of range for D-061's 63-bit tagged smallint representation (issue #618, restoring `pycc check` as the catch point D-178/#148 knowingly moved to run time for the literal case) — a bigint reaching the same position through arithmetic is unaffected |
| `T0052` | error | cross-MRO attribute redeclaration with a differing declared type (issue #676, D-210) — two distinct classes in one class's own C3-linearized MRO each declare an instance attribute of the same name with a different type; rejected at class-definition time since pycc has no per-instance runtime type tag to safely coerce at the shared, statically-dispatched assignment site |
| `T0053` | error | a parameterized container type annotation (`list[T]`, `set[T]`, `dict[K, V]`, `tuple[A, B, ...]`) was written with the wrong number of type arguments (issue #918, D-228) — `list`/`set` take exactly one, `dict` exactly two, and `tuple` at least one; `tuple[()]` and any `...` type argument (the homogeneous-variadic `tuple[int, ...]`, and the ill-typed `list[...]`/`dict[str, ...]` spellings) are not supported yet and are rejected here |
| `O0201` | error | value used after move across scope boundary *(internal-only: never fires on legal Python — see note)* |
| `O0301` | error | non-Shareable value crosses thread boundary without move |
| `O0302` | error | lock-guarded field accessed without holding lock |
| `E0100` | error | `eval`/`exec`/`compile` not compilable |
| `E0101` | error | monkey-patching foreign class/module |
| `E0102` | error | dynamic attribute injection |
| `E0103` | error | dynamic `type()` class creation |
| `E0104` | error | wildcard import *(reserved; not currently emitted — a wildcard `from x import *` is rejected today by the same versioned `C0001` "not implemented yet" capability code every other unsupported import shape uses, not this by-design-rejection code; see D-137)* |
| `E0105` | error | metaclass with non-static side effects |
| `E0106` | warning→error | `__del__` relies on refcount timing |
| `E0107` | error | `sys.getrefcount` unavailable |
| `E0108` | error | import cycle in top-level init, reported as the chain of files that closes it (`import cycle: `a.py` -> `b.py` -> `a.py``), emitted by the driver's project-module loader since #898/D-222 |
| `I0401` | error | untyped value leaks across interop boundary |
| `I0402` | error | CPython-backed direct import root rejected by the effective v0.7 `allowlist` or `deny` policy (planned; not emitted by the current compiler) |
| `W1001` | warning | unreachable code |
| `W1002` | warning | boxed fallback in hot loop (`--memstats` hint) |

Note on `O0201`: legal Python never observes moves (see MEMORY_OWNERSHIP.md — optimization is semantics-preserving). The code exists for internal assertions and `--emit mir` tooling; if it ever fires on user code, that's a pycc bug, auto-reported.

Adding a new code here requires a matching `EXPLANATIONS` entry in
[`crates/pycc_diag/src/explain.rs`](../crates/pycc_diag/src/explain.rs)
(D-150) — that file's own test suite enforces the two stay in sync, so a
registry row added here without a matching entry fails CI immediately.

`C0001` is a versioned capability diagnostic, not a rejected-by-design
language rule. A construct stops producing it when the corresponding roadmap
slice is implemented; the code remains reserved so older compiler output keeps
an unambiguous meaning. HIR lowering uses it for valid Python outside the
currently implemented frontend subset, with the unsupported AST node as the
primary span and a message that names the rejected construct in Python terms
(`a tuple`, ``an `import` inside a function or block body``, `a call whose
callee is a call expression`); a message never renders an AST node's Rust
`Debug` form, and `tests/diagnostics_test.rs` scans every `.expected.txt`
fixture for the `NodeIndex(` marker such a dump would carry (issue #890).
It also covers the project-import shapes #898/D-222 recognizes but does not
implement yet: a bare `import <project module>` and a `from pkg import
<submodule>` (both bind a module namespace -- ``module namespace bindings
(`import geometry`) are not supported yet``), a PEP 420 namespace package as
an import's *terminal* segment (``namespace package `nspkg` (a directory
without `__init__.py`) is not supported yet``; an intermediate namespace
segment is fine), and a top-level name two linked modules both define
(``top-level name `helper` is already defined by `colA.py`; a separate
namespace per module is not supported yet``, lifted by Part 3 of #881). A
program that seeds the builtin exception classes in one module while another
shadows one of their names is rejected the same way. A protocol class in
return-annotation position (``a protocol class (`P`) as a return type
annotation is not supported yet -- a protocol type is currently supported in
parameter and variable positions only``,
[#934](https://github.com/rotnov/pycc/issues/934)) is rejected at the
annotation's own span, on a module-level function, a method, and a protocol
member declaration alike; the check runs after the annotation lowers, so a
nested `list[P]` or `P | None` still reports its own element-type gate first.
An import failure
CPython itself would raise on is `T0021`, not `C0001`. A `from __future__
import ...` is a compiler directive, not a module (#919, D-229): its nine no-op
features lower to nothing, a name CPython rejects is `L0001`, and the one
CPython-valid feature that changes the grammar, `barry_as_FLUFL`, is `C0001`
(``the `barry_as_FLUFL` future feature (`<>` in place of `!=`) is not
supported yet``), as is `from __future__ import x as y` under the generic
aliasing gap.

`pycc_types` also uses it for calls to known Python 3.14
callable builtins that this compiler version does not implement (e.g.
`ValueError("x")`, `Exception("msg")`, `int("5")`, `range(10)` as a
standalone call) -- these are valid Python, not name-resolution failures
(`T0021`), so they are classified as capability gaps. User-defined functions
always take priority: a `def ValueError(...)` is called correctly, not
classified as `C0001`. The same classification applies in both the final
validation pass and the private-helper inference path (issue #142). It is
also used for an *implemented* special-cased builtin called with an argument
shape this version does not support -- `isinstance(f(), int)` (a call as the
inspected operand), `cast(list[int], x)` (a subscripted generic as the
target type, issue #767), and a `cast` whose target type would change the
value's runtime representation, narrow its attribute layout, or cross a
method-override boundary that pycc's static (non-virtual) method dispatch
cannot see through
(`cast(str, 5)`, `cast(int, some_bool)`, a down-cast such as
`cast(Derived, base)`, or an up-cast such as `cast(Base, derived)` where
`Derived` overrides a method `Base` defines; D-198, issue #767) are the
current instances -- for the same reason:
the construct is valid Python that a later slice can implement, not a
by-design rejection. It is also used for a direct method call on a class
that HIR lowering synthesized for a seeded builtin exception (D-188) --
e.g. `except Exception as e: e.__init__("oops")`, or the same call reached
through a user subclass that inherits the method without overriding it
(issue #714) -- because a synthetic class's method-table entry does not
correspond to a real, callable function (D-173 propagates a raised
exception through global runtime state rather than a real allocated
instance); Part 1 of issue #737, issue #711. The same issue #714 also
covers a *construction*, not a dunder call: binding a fresh instantiation
of such a class to a name (`e = MyError("boom")`, for a user subclass
whose MRO reaches a builtin exception without overriding its inherited
constructor) reports `cannot instantiate exception class \`...\` as a
value` -- `raise MyError("boom")` stays the one supported construction
(Part 3 of issue #541). Calling an enum class (`Color()`, `Color(1)`)
reports `cannot call enum class \`...\`` from the same
`resolve_instantiation` ladder (issue #921): members are compile-time
singletons reached by name (`Color.RED`), and by-value lookup is the
not-yet-implemented construct; the zero-argument form is a CPython
`TypeError` too and is named as such rather than as unsupported. Naming
an enum class as a base (`class Foo(Color): pass`) is rejected from
`validate_bases` on the class header (issue #941): when the enum has
members the message names CPython's own `TypeError: <enum 'Foo'> cannot
extend <enum 'Color'>`, and when it is a member-less docstring-only enum
-- a shape CPython does allow extending -- the message reads
`cannot inherit from member-less enum class \`...\` -- ... not supported
yet`.

## Quality bar

- Every error: primary span, ≥1 label, expected/found where applicable, help with a suggestion when one is safe. Populated for arity/type-mismatch, missing-annotation, and literal-index-constraint families as of D-152 (`docs/decisions/D-152-populate-diagnostic-help-for-arity-type.md`). That is a standing contract on those families, not a snapshot of the tree D-152 measured: a diagnostic added to one of them later joins the populated set at its own introduction (`T0053`, D-228 decision 11, is the first such case). Still `None`/empty for name-resolution, capability-limitation, and ambiguous-conflict diagnostics, and human-format output never renders it.
- Suggestions marked machine-applicable must be idempotent + tested. Applying them automatically is intended for a planned `pycc check --fix` flag, which is not yet implemented (see `docs/CLI_SPEC.md`).
- Message text changes are allowed; codes and JSON structure are not (corpus bot fingerprints on code + span shape).
- Every diagnostic a pass can report for a file is emitted (JSON: one object per line). The *first* diagnostic for any input -- code, message, span, position -- was kept byte-identical across the three #864 parts (D-217 rule 2, discharged once D-219 and D-220 landed) so the existing fixtures could verify that collection and ordering changes perturbed nothing; that was a transition invariant, not a release-to-release promise: message text remains changeable under the bullet above, codes and JSON structure do not. The parser reports in ruff's discovery order, which is not always source order. HIR lowering reports one diagnostic per failing top-level item in source order and skips the item; an item that fails only because it names a class or type alias that itself failed to lower (a bare-name annotation or a base class) is skipped silently, with no diagnostic of any kind, so a later gap inside such an item surfaces only once the root cause is fixed (D-219). Type checking reports one diagnostic per failing function (#868, D-220): a pre-check failure (an incompatible redefinition or attribute redeclaration) is reported alone. Otherwise, if the private-helper solver's list is module-level (a failure in its top-level walk or in a post-body phase such as `propagate_binop_constraints`, and `monomorphize` after checking), that one diagnostic is reported alone and the annotation checker's list is dropped, because a post-body solver diagnostic cannot be matched by function to the checker's entry for the same error. Otherwise the solver's per-function diagnostics are reported in item order, then every checker entry -- per-function or module-level -- whose function the solver did not flag, in the checker's order; a function both phases flag is reported once, with the solver's text. If the solver passes, the checker's list against the solved signatures is reported on its own. Every type diagnostic still carries an empty span and renders at `:1:1` (D-043), so a per-function report is distinguished by its message, not its location, until spans reach `pycc_types`.
- Diagnostics on all Tier-1 platforms byte-identical (paths normalized).
