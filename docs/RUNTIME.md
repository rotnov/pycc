# pycc Runtime Specification

`pycc_rt` — the static library linked into every binary. Pure Rust, no libpython, no platform-visible behavior differences (cross-platform is a hard requirement — see ARCHITECTURE.md).

## Object model

- Scalars (`int` i64-path, `float`, `bool`, `None`) — unboxed, never touch the runtime.
- Heap objects: 16-byte header `{ rc: u32 (thread-local) | AtomicU32 (shared), type_id: u32, flags }` + payload. `flags` marks: cycle-tracked, shareable, has-finalizer.
- `str`: immutable UTF-8, `{len, hash-cache}` + bytes; small-string optimization ≤ 22 bytes inline. Codepoint indexing via lazily built offset index (amortized O(1), see D-007).
- `list[T]`: growable vec of unboxed `T` where `T` is scalar/struct — `list[int]` is literally `Vec<i64>`-shaped, SIMD-friendly.
- `dict[K, V]`: insertion-ordered swiss table (CPython 3.7+ order semantics).
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
