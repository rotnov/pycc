# pycc Runtime Specification

`pycc_rt` owns the runtime linked into every binary. It never depends on libpython and
must not introduce platform-visible behavior differences (cross-platform is a hard
requirement — see ARCHITECTURE.md).

The v0.1 bootstrap is deliberately narrower than the final runtime: the `pycc_rt` Rust
crate embeds a target-independent C implementation of the two available ABI helpers
(`print(i64)` and `NameError`). The compiler materializes that source and passes it to
the same target-aware Clang driver as the generated object. The driver always receives
LLVM's exact effective target triple; MSVC targets additionally select bundled LLD,
so a `cc` executable that happens to resolve to MinGW cannot consume an MSVC object.
`PYCC_CLANG_<NORMALIZED_TARGET>` and then `PYCC_CLANG` provide explicit,
Clang-compatible tool overrides; an unavailable driver is a clean compile error. This
makes the runtime a real packaged dependency without looking up Cargo's profile- or
target-specific static-library artifacts.

The bootstrap writes protocol output as exact bytes. On Windows it switches the C
stdout/stderr descriptors to binary mode before emitting `\n`, preventing the CRT
from translating line feeds to `\r\n`; the same fixtures assert byte-identical stdout
and stderr on every Tier-1 host.

This exception ends before the heap/object runtime starts. The object model,
exceptions, allocator, and later facilities specified below are implemented in the
pure-Rust `pycc_rt` static library. Adding another C bootstrap helper beyond the v0.1
surface requires an explicit decision record rather than silently growing a second
runtime implementation.

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
