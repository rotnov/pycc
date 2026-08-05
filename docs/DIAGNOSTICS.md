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
| `L0001` | error | syntax error (span + expected set) |
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
| `T0042` | error | generic-function shape or call-site instantiation rejected beyond the PR-13 v0.2 thin slice (D-133/D-134): more than one PEP 695 type parameter; a type parameter used in a container position (`list[T]`); a call site whose occurrences of the type parameter resolve to different concrete types; a call site whose argument for the type parameter is not `int`/`float`/`bool`/`str`; a generic function's body calling itself or any other generic function (recursive generic instantiation is outside D-134's single-call-site monomorphization slice); also reused (D-135) for a generic `type` alias declaring its own type parameters |
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

`C0001` is a versioned capability diagnostic, not a rejected-by-design
language rule. A construct stops producing it when the corresponding roadmap
slice is implemented; the code remains reserved so older compiler output keeps
an unambiguous meaning. HIR lowering uses it for valid Python outside the
currently implemented frontend subset, with the unsupported AST node as the
primary span.

## Quality bar

- Every error: primary span, ≥1 label, expected/found where applicable, help with a suggestion when one is safe.
- Suggestions marked machine-applicable are applied by `pycc check --fix` and must be idempotent + tested.
- Message text changes are allowed; codes and JSON structure are not (corpus bot fingerprints on code + span shape).
- Diagnostics on all Tier-1 platforms byte-identical (paths normalized).
