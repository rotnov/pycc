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
| `T0030` | error | non-exhaustive `match` (missing cases listed) |
| `T0031` | error | `@override` without matching base method |
| `O0201` | error | value used after move across scope boundary *(internal-only: never fires on legal Python — see note)* |
| `O0301` | error | non-Shareable value crosses thread boundary without move |
| `O0302` | error | lock-guarded field accessed without holding lock |
| `E0100` | error | `eval`/`exec`/`compile` not compilable |
| `E0101` | error | monkey-patching foreign class/module |
| `E0102` | error | dynamic attribute injection |
| `E0103` | error | dynamic `type()` class creation |
| `E0104` | error | wildcard import |
| `E0105` | error | metaclass with non-static side effects |
| `E0106` | warning→error | `__del__` relies on refcount timing |
| `E0107` | error | `sys.getrefcount` unavailable |
| `E0108` | error | import cycle in top-level init |
| `I0401` | error | untyped value leaks across interop boundary |
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
