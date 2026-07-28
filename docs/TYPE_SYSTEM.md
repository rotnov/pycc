# pycc Type System Specification

The contract: **surface syntax is standard Python typing** (PEP 484 → 695/696/742/649, full list in [PYTHON_STANDARDS.md](./PYTHON_STANDARDS.md)); **checking is strict** — what mypy calls `--strict` is pycc's only mode; **types drive codegen** — every check result is also a representation decision.

## Strictness rules

1. Every public function/method: parameters and return type annotated, else `T0001`.
2. Locals and private helpers: inferred (Hindley-Milner-flavored local inference; annotations always win).
3. `Any` does not exist in pure pycc code — it is a compile error (`T0002`) except at declared interop boundaries (see RUNTIME.md § interop).
4. No implicit `Optional`, no implicit numeric narrowing **or widening** (D-086 — includes `int` at a `float`-annotated boundary; write `float(x)`), no untyped containers (`x = []` requires inferable or annotated element type).
5. Unreachable code after exhaustive `match` / `Never` is verified (`assert_never` pattern supported).
6. `==`/`!=` require operands to be comparable under the same numeric-like-or-`str` grouping ordering operators use (`int`/`float`/`bool` interchangeably, or `str`/`str`) — looser than the exact-type rule assignment/parameter/return boundaries enforce (rule 4), but still strict enough to reject genuinely incompatible pairs. Heterogeneous equality across categories (`1 == "1"`) is `T0021`, not `bool`, matching `mypy --strict`'s own `comparison-overlap` check (D-086). Ordering operators (`<`/`>`/`<=`/`>=`) use this identical grouping but are always rejected across it when CPython itself would raise `TypeError` at runtime for the pair.

### v0.1 local inference

- A module-level function whose name starts with `_` is a private helper under
  D-038. Missing parameter and return annotations create inference variables;
  explicit annotations remain fixed constraints.
- The v0.1 solver links those variables through call arguments, local names,
  assignments, returns, `range` operands, and arithmetic expressions. The
  resulting helper signature is monomorphic within the module. Conflicting
  call-site constraints are `T0021`; conflicting inferred returns are `T0022`.
- An unconstrained parameter or return variable is rejected with `T0021` and
  an instruction to add an annotation. It never silently becomes `Any` or
  `None`; only a helper with no value-returning path infers `None`.
- Function-local names are classified before the body is checked. Parameters
  are local from entry; every assignment target and `for` target anywhere in
  the implemented nested control-flow grammar is local throughout that
  function. A read before the local has been bound is `T0021` and never falls
  back to a same-named module global. A name with no local binding form may
  still resolve to a module global. Call targets follow the same lookup before
  builtin or function-registry resolution: an unbound local target is `T0021`,
  while a bound local or parameter from the current primitive subset cannot be
  called by falling through to a same-named function. Control-flow join state
  remains the separate definite-assignment work tracked in #118.
- The first assignment fixes a local variable's inferred type. Later
  assignments must be compatible or produce `T0023`; assigning `bool` to an
  `int` binding preserves the `int` representation. D-074 carries that
  decision through MIR and code generation at assignment, argument, return,
  and `range` representation boundaries.
- Python numeric semantics apply during inference: `bool` is an `int`
  subtype, mixed `int`/`float` arithmetic promotes to `float`, and true
  division `/` always returns `float` even for two integer operands.

## Types and representations

| Python type | Static semantics | Native representation |
|---|---|---|
| `int` | arbitrary precision (CPython-true) | `i64` + overflow promotion to heap bigint — see D-001 |
| `float` | IEEE 754 double | `f64`, unboxed |
| `bool` | subtype of `int` | `i8`, unboxed; `i1` is transient control-flow state only — see D-061/D-074 |
| `str` | immutable Unicode | UTF-8 heap, small-string opt — see D-007 |
| `bytes` / `bytearray` | per CPython | raw buffer |
| `None` | unit | LLVM `void` for returns; canonical `i8 0` carrier for the v0.1 user-function parameter ABI and parameter entry slots; `T \| None` = nullable/tagged repr |
| `tuple[A, B]` | fixed heterogeneous | inline struct (stack when non-escaping) |
| `list[T]` / `set[T]` / `dict[K, V]` | homogeneous, invariant | native vec / swiss-table (insertion-ordered dict) |
| `class` | nominal | struct; fields fixed at compile time (`__slots__` semantics implicit) |
| `Protocol` | structural | static dispatch via monomorphization; vtable only for explicit `dyn`-like use |
| unions `A \| B` | tagged | discriminant + payload; niche optimization for `T \| None` |
| `Callable[...]` | first-class functions | fn pointer / closure struct |
| `enum.Enum` | per CPython | integer discriminant + const table |

## Generics

- Monomorphization (Rust model): `list[int]` and `list[str]` are distinct compiled types; bounds expressed via `Protocol` constraints.
- PEP 695 syntax (`def f[T](x: T) -> T`, `type Alias[T] = ...`) and legacy `TypeVar` both supported; PEP 696 defaults honored.
- Variance: inferred per PEP 695 rules; containers invariant, as in the typing spec.
- Code-size control: polymorphic-by-vtable fallback for cold generic code under `--opt-size` (compiler-internal, semantics unchanged).

## Narrowing & flow typing

Flow-sensitive checker on HIR control-flow graph: `isinstance`, `is None`, truthiness of `Optional`, `match` patterns (PEP 634 — with exhaustiveness checking), `TypeGuard` (647), `TypeIs` (742), walrus bindings, `assert`.

## Annotation semantics

Per PEP 649/749 (3.14): annotations are lazily evaluated code — pycc evaluates them **statically at compile time**; string annotations and `from __future__ import annotations` files accepted. Runtime introspection of `__annotations__` is supported for dataclass-style use, computed at compile time.

## Python 3.15 typing preview (post-v1.0)

The preview does not expand the v1 Python 3.14 contract. The v1.x language
upgrade in ROADMAP.md adds four type-system obligations: singleton-value typing
and `is` narrowing for `sentinel()` (PEP 661), typed `TypedDict` extra items
(PEP 728), `TypeForm` (PEP 747), and disjoint-base reasoning (PEP 800). Each is
tracked by its own `py315/` conformance row in PYTHON_STANDARDS.md.

## Class model (compiled subset)

Supported: single + multiple inheritance with C3 MRO resolved at compile time, `@property`, `classmethod`/`staticmethod`, `__init_subclass__`/`__set_name__` executed at compile time when statically evaluable, dataclasses (557) and `dataclass_transform` (681), `@override` (698) enforced, dunder protocol methods (`__len__`, `__iter__`, `__enter__`…) → static dispatch.

Rejected (negative-tested, see DIAGNOSTICS.md): runtime class mutation, dynamic `type()` creation, metaclasses with runtime side effects beyond the statically evaluable subset (`E0105`), custom `__getattr__` catch-alls on non-interop types (`E0102`).

## Error philosophy

Rust-grade messages: primary span + labels, expected/found diff, suggestion machine-applicable where safe (`pycc check --fix` for trivial ones), `pycc explain T0021` long-form. Every diagnostic documented + tested. Full registry: [DIAGNOSTICS.md](./DIAGNOSTICS.md).
