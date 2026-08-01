# pycc Runtime Specification

`pycc_rt` — the static library linked into every binary. Pure Rust, no libpython, no platform-visible behavior differences (cross-platform is a hard requirement — see ARCHITECTURE.md).

## Object model

- Scalars (`int` i64-path, `float`, `bool`, `None`) are unboxed and require no heap allocation. **Current state (through PR-5):** `None` returns lower to LLVM `void`, while a `None` value crossing the user-function parameter ABI or its parameter entry slot uses a canonical `i8 0` unit carrier (D-075). `int`'s fast path additionally low-bit-tags its word (D-061) and every arithmetic/comparison/formatting operation on it is a `pycc_rt` function call rather than a raw LLVM instruction -- simplest-correct for a `--debug`-only, no-generated-code-perf-requirement v0.1 profile (see D-061's Alternatives); replacing these calls with direct LLVM-intrinsic codegen once a real perf bar exists (v0.2+) is a documented future item, not a v0.1 requirement. Add, subtract, a product of two smallints, and a smallint floor-division quotient outside the tagged range promote to the heap bigint representation. Operations that require converting an already-promoted bigint to `float` (including mixed bigint/float arithmetic), multiplication/floor-division/modulo/power with a bigint operand, and a negative `int` exponent remain explicit accepted failure boundaries. Float true division, floor division, modulo, and power also cross the runtime boundary: zero divisors fail explicitly until exceptions land, `//`/`%` share CPython's adjusted-remainder algorithm so rounding and signed-zero behavior are not delegated to naive LLVM division, and power rejects finite overflow plus domains that require Python exceptions or complex results instead of returning a silent infinity/NaN.
- Heap objects: no project-wide generic header exists. **Current state (through PR-10, D-105):** each heap object type defines its own header inline instead of sharing one common layout. The *refcounted* heap objects, `PyStrObj` and `PyIntListObj`, use just `rc: Cell<u32>` plus their own payload fields, with no `type_id`/`flags` anywhere in the actual runtime. A third heap-allocated type, `BigIntObj` (the heap bigint an overflowing `int` promotes to, D-001/D-061), carries no header at all — no `rc`, no `type_id`, no `flags` — since it is never refcounted or freed: its own doc comment (`crates/pycc_rt/src/lib.rs`, next to the struct itself, D-058) records this as a deliberate, narrower "simplest safe default" than `str`'s real refcounting (D-060) — a rare, overflow-only path with no v0.1 construct that could leak it in a hot loop the way an unbounded string-building loop could — not a fourth shape sharing the other two's refcounted-header convention. A shared/generic header with cycle-tracking, shareable, and has-finalizer flags remains a possible future design once a real consumer of those flags exists, not something any shipped object implements today. **Current state (through PR-11a, D-111/D-114):** `PyDictObj` and `PyIntSetObj` (`crates/pycc_rt/src/lib.rs`) join `PyStrObj`/`PyIntListObj` in that same `rc: Cell<u32>`-plus-payload shape — the refcounted set is four types now, not the two the D-105 sentence above counted at the time; `BigIntObj` remains the sole heap-allocated type with no header at all, unaffected by this count, and none of the four refcounted types shares a generic/common header either.
- `str`: immutable UTF-8, `{len, hash-cache}` + bytes; small-string optimization ≤ 22 bytes inline. Codepoint indexing via lazily built offset index (amortized O(1), see D-007). **Current state (through PR-5):** every `str` value is a pointer to a refcounted heap object (small-string bytes inline in that same allocation, per D-059). Every named local slot is preallocated, so reassignment decrefs the previous value even when the first lexical assignment is inside a loop; top-level completion also decrefs the final named value. Two memory-safe accepted leaks remain until `pycc_own` (v0.5) adds real lifetime tracking: an unbound temporary is never decrefed, and a `str` parameter/local is not decrefed at function return. See D-074.
- `list[T]`: growable vec of unboxed `T` where `T` is scalar/struct — `list[int]` is literally `Vec<i64>`-shaped, SIMD-friendly. **Current state (through PR-10, D-105):** only `list[int]` is actually implemented, as `PyIntListObj` (`crates/pycc_rt/src/lib.rs`) — `rc: Cell<u32>` plus a `Cell<Vec<i64>>` payload of raw, untagged `i64` elements (D-106, a deliberate exception to D-061's tagged-`int` rule, analogous to `bool`'s own untagged representation). `list[str]`/`list[float]`/`list[bool]`/nested `list[T]` are type-checked but rejected before codegen (`T0034`); refcounting is leak-only (no `pycc_rt_int_list_incref`/`_decref` call site exists yet, D-107); negative indices are rejected rather than treated as CPython's last-element addressing (D-108).
- `dict[K, V]`: insertion-ordered swiss table (CPython 3.7+ order semantics). **Current state (through PR-11a, D-111):** only `dict[str, int]` is actually implemented, as `PyDictObj` (`crates/pycc_rt/src/lib.rs`) -- a dense insertion-ordered array with linear-scan lookup, not yet a real hash table. `d[k] = v` performs insert-or-update; missing-key reads panic (no `KeyError` handling exists). Refcounting is leak-only (D-114), matching `list[int]`.
- `set[T]`: **Current state (through PR-11a, D-111/D-112):** only `set[int]` is implemented, as `PyIntSetObj` (`crates/pycc_rt/src/lib.rs`) -- structurally identical to `list[int]`'s own `PyIntListObj` except insertion dedups via a linear scan. Iteration order is this implementation's own insertion order, which is not guaranteed to match CPython's own hash-dependent set iteration order -- no conformance fixture asserts byte-for-byte agreement on set iteration output (D-113). No membership test (`in`) exists yet anywhere in this compiler.
- `tuple` typed: inline struct; classes: fixed-layout structs, fields resolved to offsets at compile time.

## Exceptions

Zero-cost on the happy path: LLVM/Itanium unwinding (SEH on Windows). `try/except/else/finally`, `except*` groups (PEP 654), full traceback with `.py` lines reconstructed from unwind tables + debug info. `raise ... from ...` chains preserved.

## Generators & iterators

Generators/`yield from` compile to resumable state machines (struct + resume fn) — no frames, no heap unless the generator escapes. `for` over known containers lowers to plain loops (no iterator protocol overhead when types are static).

## Allocator & startup

- mimalloc bundled on all Tier-1 targets; identical behavior everywhere.
- Startup: `main()` runs directly — no interpreter boot. Target: `hello` binary < 2 MB, < 5 ms cold start.
- Module init: top-level code of imported modules runs once, in deterministic import order, at process start (statically scheduled — import cycles are a compile error `E0108`).

## CPython interop escape hatch (v0.7+)

For the untyped world (numpy, requests…):

```python
from pycc.interop import cpython

np = cpython.import_module("numpy")   # type: cpython.Object
```

- Embeds a real CPython 3.14 (dynamically loaded, only if used; binary stays standalone otherwise).
- `cpython.Object` is the **only** `Any`-like type; conversions at the boundary are explicit and typed (`to_int()`, `from_list(xs)`), misuse is `I0401`.
- Interop calls hold that interpreter's GIL internally; pycc threads stay GIL-free outside the boundary.
- Cost model documented: boundary crossing is expensive by design; the escape hatch is a bridge, not a lifestyle.

## ABI & embedding

- `pycc build --lib` emits a C-ABI static/shared library + generated header: compiled Python callable from C/Rust/Go.
- Symbol naming stable per version; no global state beyond one runtime context (re-entrant embedding allowed).
