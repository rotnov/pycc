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
| `L0001` | error | syntax error (span + expected set) *(also reused, without an "expected set", for a post-parse context violation caught during HIR lowering -- `break`/`continue` outside a loop, `async for` outside an async function (D-148), `yield`/`yield from` outside a function (D-149), and `return`/`break`/`continue` inside a `finally` block that would exit it (PEP 765, D-193, Part 1 of #543) -- since CPython classifies all of these as `SyntaxError` (a `SyntaxWarning` as of CPython 3.14 for the `finally` case specifically) too; `T0024` ("`return` outside a function") is the closest existing precedent for this same "keyword valid only in a specific context" family but lives under `T0xxx` instead, an accepted, deliberately unfixed inconsistency -- see D-148)* |
| `L0002` | error | Python version mismatch (feature needs 3.14 level) |
| `T0001` | error | public function missing annotation |
| `T0002` | error | `Any` outside interop boundary |
| `T0003` | error | untyped empty container needs annotation |
| `T0021` | error | name resolution (including an unbound local), operand, call, or inference type mismatch |
| `T0022` | error | return type mismatch |
| `T0023` | error | incompatible assignment |
| `T0024` | error | `return` outside a function |
| `T0025` | error | annotated-assignment initializer incompatible with its declared annotation |
| `T0026` | error | a later plain or annotated assignment is incompatible with an earlier value-less declaration (`x: int` with no `= ...`) |
| `T0030` | error | non-exhaustive `match` (missing cases listed) |
| `T0031` | error | `@override` without matching base method |
| `T0032` | error | list literal elements must all share one type |
| `T0033` | error | value does not support list/dict/set operations (subscript, slicing, item assignment, `for`, `.append()`, `.pop()`, `.get()`, `.add()`, `len()`), or `len()` called with the wrong number of arguments |
| `T0034` | error | list codegen only supports `list[int]` in v0.2 (D-105) |
| `T0035` | error | dict literal key/value pairs must all share one key type and one value type |
| `T0036` | error | dict codegen only supports `dict[str, int]` in v0.2 (D-122) |
| `T0037` | error | set literal elements must all share one type |
| `T0038` | error | set codegen only supports `set[int]` in v0.2 (D-122) |
| `T0039` | error | tuple element type not compiled yet (int/bool/float only, D-116) |
| `T0040` | error | tuple index must be a non-negative literal integer within range (D-116) |
| `T0041` | error | local name may not be bound on every path reaching this use (definite-assignment tracking, issue #118 Part 1, D-147) |
| `T0042` | error | generic-function shape or call-site instantiation rejected beyond the PR-13 v0.2 thin slice (D-133/D-134): more than one PEP 695 type parameter; a type parameter used in a container position (`list[T]`); a call site whose occurrences of the type parameter resolve to different concrete types; a call site whose argument for the type parameter is not `int`/`float`/`bool`/`str`; a generic function's body calling itself or any other generic function (recursive generic instantiation is outside D-134's single-call-site monomorphization slice); also reused (D-135) for a generic `type` alias declaring its own type parameters |
| `T0043` | error | attribute access, attribute assignment, or method call on a value that is not a class instance (D-154, Part 1 of #375) |
| `T0044` | error | class has no attribute or method with the given name (D-154, Part 1 of #375); a mismatched instantiation/method call-site argument list reuses `T0021` instead, exactly like an ordinary function call; also reported (PEP 560, #610/#611) when `ClassName[...]` is used — in value or annotation position — on a class that defines no `__class_getitem__` anywhere in its MRO and is not a PEP 695 generic class, matching CPython's `TypeError: type 'C' is not subscriptable` |
| `T0045` | error | cannot reassign a `Final` name (PEP 591, #383) — variable-level annotations only (module-level and function-local); `Final` on parameters or class-body attributes is out of scope |
| `T0046` | error | class does not conform to a protocol (PEP 544 structural typing, #380, D-166) — a concrete class is missing one or more of a protocol's required methods or attributes, or a method/attribute has an incompatible signature/type |
| `T0047` | error | `super()` does not proxy instance attributes (#587) — an attribute established by `self.<attr> = ...` is reachable only through `self`, matching CPython's `super` object, which proxies class-level attributes and descriptors along the MRO but not the instance's own attributes |
| `T0048` | error | general union type annotation is not supported yet (PEP 604, D-197, #763, Part 1 of #747) — a `X \| Y` annotation is accepted only in the exact `T \| None`/`None \| T` shape (a 2-operand union where one side is literally `None`); any other shape, including a longer `A \| B \| None` chain or a 2-operand union where neither side is `None`, is rejected |
| `T0049` | error | `Optional[T]` annotation is not supported yet for this `T` (PEP 604, D-197, #763, Part 1 of #747) — `T \| None` type-checks only for `T == int` in this PR; every other inner type is a recognized but out-of-scope shape, distinct from `T0048`'s rejection of the general-union shape itself |
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
| `E0108` | error | import cycle in top-level init |
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
primary span. `pycc_types` also uses it for calls to known Python 3.14
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
by-design rejection.

## Quality bar

- Every error: primary span, ≥1 label, expected/found where applicable, help with a suggestion when one is safe. Populated for arity/type-mismatch, missing-annotation, and literal-index-constraint families as of D-152 (`docs/decisions/D-152-populate-diagnostic-help-for-arity-type.md`); still `None`/empty for name-resolution, capability-limitation, and ambiguous-conflict diagnostics, and human-format output never renders it.
- Suggestions marked machine-applicable are applied by `pycc check --fix` and must be idempotent + tested.
- Message text changes are allowed; codes and JSON structure are not (corpus bot fingerprints on code + span shape).
- Diagnostics on all Tier-1 platforms byte-identical (paths normalized).
