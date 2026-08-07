# pycc Type System Specification

The contract: **surface syntax is standard Python typing** (PEP 484 → 695/696/742/649, full list in [PYTHON_STANDARDS.md](./PYTHON_STANDARDS.md)); **checking is strict** — what mypy calls `--strict` is pycc's only mode; **types drive codegen** — every check result is also a representation decision.

## Strictness rules

1. Every public function/method: parameters and return type annotated, else `T0001`.
2. Locals and private helpers: inferred (Hindley-Milner-flavored local inference; annotations always win).
3. `Any` does not exist in pure pycc code — it is a compile error (`T0002`) except at compiler-classified CPython interop boundaries (see RUNTIME.md § interop). Planned v0.7 creates that boundary behind ordinary standard-Python imports; it does not require a pycc-specific import spelling (D-128).
4. No implicit `Optional`, no implicit numeric narrowing **or widening** (D-086 — includes `int` at a `float`-annotated boundary; `float(x)` for `x: int | float | bool` is now a real callable builtin conversion, D-086's own remedy for that boundary, correctly deferring to a user-defined `float` of the same name if one exists — `int(...)` remains unimplemented and out of scope, and a bigint-valued `int` argument aborts, the same pre-existing limitation every other numeric promotion to `float` already has), no untyped containers (`x = []` requires inferable or annotated element type).
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
- An initialized scalar-annotated local with no earlier representation binding
  binds its target to the declaration type, while the initializer keeps its
  independently inferred type and is checked directionally afterward. A
  compatible re-declaration retains the first representation binding.
  Scalar declaration bounds act only as deferred fallbacks for
  otherwise-unresolved initializer variables: all bounds for the same
  inference root are aggregated and the most-specific compatible type wins,
  so `bool` evidence is never widened to `int` merely because an annotation or
  helper body was visited first. A value-less annotation still creates no
  initialized binding; that separate state-model limitation is tracked by
  [#245](https://github.com/rotnov/pycc/issues/245).
  Non-scalar annotated targets receive local-only unresolved solver bindings;
  an inference root joined to one cannot resolve an otherwise-inferred private
  parameter or return as a container, even through hard call evidence. Existing
  container inference from explicit function-signature evidence is unchanged.
- An unconstrained parameter or return variable is rejected with `T0021` and
  an instruction to add an annotation. It never silently becomes `Any` or
  `None`; only a helper with no value-returning path infers `None`.
- D-146 (#239): the solver now carries `Ty::List` as a destructured element-
  type carrier for homogeneous scalar-element list literals. The `ListLiteral`
  arm produces `Some(Ok(Ty::List(...)))` when all elements share an exact-equal
  private-solver scalar type; the `Subscript` and `ListPop` arms destructure
  that carrier to extract the scalar element type for scalar return-type
  inference (e.g. `def _first(): xs = [1]; return xs[0]` infers `int`). The
  carrier is never unified — `unify_terms` and `merge_inferred_types` are
  unchanged — and `dict`/`set`/`tuple` remain in the scalar-only gap.
- Function-local names are classified before the body is checked. Parameters
  are local from entry; every assignment target and `for` target anywhere in
  the implemented nested control-flow grammar is local throughout that
  function. A read before the local has been bound is `T0021` and never falls
  back to a same-named module global. A name with no local binding form may
  still resolve to a module global. Call targets follow the same lookup before
  builtin or function-registry resolution: an unbound local target is `T0021`,
  while a bound local or parameter from the current primitive subset cannot be
  called by falling through to a same-named function. Definite-assignment
  tracking (issue #118 Part 1, D-147) extends this to control-flow joins: a
  name assigned in only one branch of an `if`/`elif` (no `else`), or only in a
  `while`/`for` body (which may execute zero times), is *maybe* bound after the
  construct. Reading a maybe-bound name is `T0041` (possibly-unbound read),
  distinct from `T0021` (never bound). An unconditional assignment on the
  current path after the join upgrades a maybe-bound name back to definitely
  bound, so `if c: x = 1` followed by `x = 2` makes `x` readable.
- The first assignment fixes a local variable's inferred type. Later
  assignments must be compatible or produce `T0023`; assigning `bool` to an
  `int` binding preserves the static `int` representation and, per D-141, the
  source object's runtime `False`/`True` identity. D-074/D-141 carry that
  decision through MIR and code generation at assignment, argument, return,
  container-value, and `range` boundaries; range consumes the numeric value
  and produces ordinary int objects rather than forwarding bool identity.
- Python numeric semantics apply during inference: `bool` is an `int`
  subtype, mixed `int`/`float` arithmetic promotes to `float`, and true
  division `/` always returns `float` even for two integer operands.
- A value-less annotation (`x: int` with no `= ...`) declares `x`'s static
  type without binding a value: a premature read is still `T0021`, exactly as
  an ordinary unannotated local would be. The declared type is retained and
  becomes the sticky representation the first time a later plain or
  annotated assignment reaches `x` — incompatible with that declared type is
  `T0026`, distinct from `T0023`'s "previously inferred" wording, since
  nothing was ever assigned, only declared. A repeated value-less
  declaration for the same still-unassigned name is itself checked against
  the first one (first-declaration-wins) and rejected with `T0026` on
  mismatch (issue #245). This general-checker rule is independent of the
  private-helper solver's own separate, still-open limitation described
  above.

## Types and representations

| Python type | Static semantics | Native representation |
|---|---|---|
| `int` | arbitrary precision (CPython-true); accepts `bool` as a subtype at checked boundaries | one int-compatible `i64`: odd smallint, exact `2`/`6` bool-identity marker, or aligned heap-bigint pointer — see D-061/D-141 |
| `float` | IEEE 754 double | `f64`, unboxed |
| `bool` | subtype of `int` | standalone `i8`, unboxed; exact `2`/`6` marker only after crossing an `int` boundary; `i1` is transient control-flow state only — see D-061/D-074/D-141 |
| `str` | immutable Unicode | UTF-8 heap, small-string opt — see D-007 |
| `bytes` / `bytearray` | per CPython | raw buffer |
| `None` | unit | LLVM `void` for returns; canonical `i8 0` carrier for the v0.1 user-function parameter ABI plus parameter, local-assignment, and module-assignment storage; the MIR/static `Ty::None` tag keeps that carrier distinct from `False`; `T \| None` = nullable/tagged repr |
| `tuple[A, B]` | fixed heterogeneous | inline struct (stack when non-escaping) |
| `list[T]` / `set[T]` / `dict[K, V]` | homogeneous, invariant | native vec / swiss-table (insertion-ordered dict) |
| `class` | nominal | struct; fields fixed at compile time (`__slots__` semantics implicit) |
| `Protocol` | structural | static dispatch via monomorphization; vtable only for explicit `dyn`-like use |
| unions `A \| B` | tagged | discriminant + payload; niche optimization for `T \| None` |
| `Callable[...]` | first-class functions | fn pointer / closure struct |
| `enum.Enum` | per CPython | integer discriminant + const table |

## Generics

- Monomorphization (Rust model): `list[int]` and `list[str]` are distinct compiled types; bounds expressed via `Protocol` constraints. **Current state (through PR-10, D-105):** this is the v1.0 target model, not yet fully landed — v0.2 ships real codegen for exactly `list[int]`; every other `list[T]` (`list[str]`, `list[float]`, `list[bool]`, nested `list[list[T]]`) type-checks (`Ty::List(Box<Ty>)` is already fully general) but is rejected before codegen with `T0034`, a clean diagnostic rather than a runtime panic. **Current state (through PR-11a, D-121/D-122):** `dict[str, int]` and `set[int]` now also ship real codegen (a dense, insertion-ordered array with linear-scan lookup — D-121 — not yet the swiss table this section's own v1.0 target describes); every other `dict`/`set` key/element combination type-checks (`Ty::Dict`/`Ty::Set` are already fully general per D-089) but is rejected before codegen with `T0036`/`T0038`, mirroring `list[int]`'s own `T0034` gate. **Current state (through PR-11b, D-115/D-116):** `tuple[...]` now also ships real codegen for exactly `int`/`bool`/`float` elements (any mix, any arity ≥ 1) — a fixed-arity LLVM struct-of-scalar-fields held as an SSA aggregate value, not a heap object (D-115); every other element type (`Ty::Str`, or any nested container) type-checks structurally (`Ty::Tuple(Box<Vec<Ty>>)` is already fully general) but is rejected before codegen with `T0039`, mirroring `T0034`/`T0036`/`T0038`. `t[k]` requires a literal, non-negative, in-range integer index (`T0040`) rather than merely an `int`-typed one, since a heterogeneous tuple's element type at position `k` is only knowable when `k` is known at compile time. `for x in t:` iteration, tuple-unpacking assignment (`a, b = t`), and a `tuple[...]` annotation syntax are deferred (`docs/ROADMAP.md`). Passing or returning a tuple value across a function boundary is implemented at the codegen layer but not yet reachable from real, unannotated Python source, for two independent reasons: `pycc_types`' private-helper signature-inference solver is scalar-only by construction, a pre-existing limitation shared by `list`/`dict`/`set` (D-116 point 4's correction note); and, even if that solver gap closed, `pycc_codegen`'s own `emit_expr` has no dedicated `MirExpr::Call` result-dispatch arm for a container-typed return either — it panics for `Ty::List`/`Ty::Dict`/`Ty::Set`/`Ty::Tuple` alike (D-116's own further correction note). Full per-element-type monomorphization for `list`/`dict`/`set`/`tuple` remains the v1.0 target this section describes.
- **Current state (through PR-13, D-133/D-134/D-135):** `def f[T](...) -> ...` now ships real codegen for exactly **one type parameter**, called from any number of call sites, each independently monomorphized by substituting `Ty::Param(Box<String>)` (D-133) with that call site's own concrete `Ty`. Instantiation is scalar-only, matching `pycc_types`' existing scalar-only signature-inference solver: `T` may resolve to `Ty::Int`/`Ty::Float`/`Ty::Bool`/`Ty::Str` only (D-134). Two or more type parameters, a type parameter used in a container position (`def f[T](x: list[T])`), a call-site argument type outside the four scalars above, inconsistent occurrences of `T` across a signature, and a generic function calling itself or another generic function (direct or mutual recursion) are all rejected pre-codegen with `T0042` rather than reaching codegen or panicking. The `type X = <expr>` statement and a legacy `X: TypeAlias = <expr>` assignment are compile-time-only name-to-`Ty` substitution inside `pycc_types`/`pycc_hir` — no `HirItem`/`MirItem`, no runtime representation, and no new `Ty` variant is produced for the alias itself (D-135); a generic alias (`type Alias[T] = list[T]`) is out of scope for v0.2 and is rejected pre-codegen with the same `T0042`. Multiple type parameters, container-position type parameters, generic aliases, and PEP 696 defaults all remain v1.0-target follow-ups (`docs/ROADMAP.md`), not yet implemented.
- PEP 695 syntax (`def f[T](x: T) -> T`, `type Alias[T] = ...`) and legacy `TypeVar` both supported; PEP 696 defaults honored. **This bullet describes the v1.0 target model, not current behavior** — see the v0.2 thin-slice paragraph immediately above for what actually ships today.
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

**Current state (through PR-15, D-154):** this is the v1.0 target model above, not yet landed — PR-15 ships real codegen for exactly a **single class, no inheritance**: instance attributes established by a first assignment inside `__init__` (each `self.<attr> = <expr>` establishes one `i64`-slot attribute, in source order, from a bare reference to one of `__init__`'s own always-annotated parameters or a scalar int/float/bool/str literal — `list`/`dict`/`set`/`tuple`- and instance-typed attributes are rejected pre-codegen, not yet representable in a slot word; the attribute-slot pre-scan only considers `__init__`'s own top-level body statements, with no recursion into a nested `if`/`while`/`for`, so a `self.x = 1` written only inside such a nested block establishes no slot at all — a later read of `self.x` then fails with `T0044` rather than an immediate, self-explanatory error at the assignment site itself), plain instance methods taking `self` (mangled `<ClassName>.<method_name>`, resolved to a compile-time-known function pointer — static dispatch, no vtable), attribute read/write (`base.attr` / `base.attr = value`), method calls (`base.method(...)`), and instantiation (`ClassName(...)`, calling the mangled `__init__`). The runtime instance object is an opaque heap object accessed only through FFI accessors, not a `#[repr(C)]` direct-layout struct (D-154's own ADR). `@property`, `classmethod`/`staticmethod`, inheritance (single or multiple), C3 MRO, dataclasses, `@override`, dunder protocol dispatch, and a generic class (`class C[T]`) all remain v1.0-target follow-ups (`docs/ROADMAP.md`), not yet implemented — a bare attribute/method access on a non-instance value is `T0043`, an unknown attribute/method name is `T0044`, and any other class-body statement shape (a `def` other than `__init__`/a plain method, or a non-`self.<attr> = ...` top-level statement) is `C0001`. A method literally named `append`, `pop`, `get`, or `add` is also rejected with `C0001` at class-definition time, since `pycc_hir`'s own hand-recognized container-method call syntax for those exact four names is resolved before any type information is available and would otherwise silently misroute a call to the user's own method into the wrong (container) code path. A class name colliding with an already-defined top-level function, type alias, or import name is likewise rejected with `C0001`, in either definition order. Class-body execution order and redefinition/rebind semantics beyond this single-pass model are issue #386's own scope, not decided here.

## Error philosophy

Rust-grade messages: primary span + labels, expected/found diff, suggestion machine-applicable where safe (`pycc check --fix` for trivial ones), `pycc explain T0021` long-form. Every diagnostic documented + tested. Full registry: [DIAGNOSTICS.md](./DIAGNOSTICS.md).
