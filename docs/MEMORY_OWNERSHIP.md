# pycc Memory & Ownership Specification

The Rust-inspired core of pycc: ownership is **inferred, never written**. Source stays standard Python; the compiler proves who owns what.

## Goals

1. No tracing GC, no stop-the-world pauses.
2. Deterministic, predictable memory behavior; C-like performance for non-shared data.
3. Data-race freedom across native pycc threads, checked at compile time (there is no pycc-wide GIL — safety must come from the type system).

## Ownership model (inferred on MIR)

| Analysis | Result |
|---|---|
| Escape analysis | value never escapes scope → **stack allocation**, no heap at all |
| Uniqueness analysis | single owner at each point → **move semantics**; assignments/returns transfer, no refcount traffic |
| Borrow inference | function parameters default to **borrows** (non-escaping refs) — zero RC ops on call |
| Sharing detection | proven multi-owner → **reference counting** (non-atomic within a thread, atomic only when value crosses threads) |
| RC elision | pairs of inc/dec removed when lifetimes provably nest (Lobster/Koka-style) |

Python semantics preserved: aliasing of mutable objects (`b = a; b.append(...)` visible via `a`) — uniqueness analysis only *optimizes* when no live alias exists; it never changes observable behavior.

## Cycles

RC alone leaks cycles. Decision (D-004): lightweight incremental **trial-deletion cycle collector** over suspected objects (Bacon-Rajan), runs on allocation pressure; types proven acyclic at compile time (most dataclasses, all immutables) are exempt from tracking — expected majority case. `gc` module surface: `gc.collect()` triggers cycle collection; tuning APIs are no-ops.

## Deviations from CPython (documented, negative-tested)

- `__del__` timing: not tied to refcount reaching zero at a specific line; runs at collection. `E0106` warns on `__del__` relying on ordering.
- `sys.getrefcount`, `id()` stability across moves: `id()` returns stable logical id; `getrefcount` unavailable (`E0107`).
- `weakref`: supported on RC-managed objects.

## Native threading without a pycc-wide GIL

Native pycc execution has no GIL; `threading.Thread` = OS thread. A planned
CPython interop boundary owns CPython's GIL only while executing CPython-backed
operations and does not weaken the rules below for native values (D-114).
Safety model (the "fishечка"):

- Compiler classifies every type: **`Shareable`** (deeply immutable: `int`, `str`, `frozen` dataclasses, `tuple` of Shareable…) or **`ThreadLocal`**.
- A value crossing a thread boundary (thread target args, closures captured by threads, `queue.Queue[T]` payloads) must be `Shareable`, or ownership must **move** (sender provably loses access — checked by uniqueness analysis), else `O0301`.
- Shared mutable state requires explicitly synchronized types: `threading.Lock`-guarded containers get checked lock discipline (`O0302` on unguarded access) — standard Python objects, standard Python code, Rust-style guarantees.
- Atomics: `int`/`bool` fields annotated `Final`-mutable patterns are out of scope v1; use locks.

## Async

`async def` compiles to state machines (no interpreter frames). Single-threaded executor v1 (asyncio-surface subset); ownership rules identical — futures capture moves or Shareables.

## Observability

`pycc build --memstats` reports per-function: stack vs heap allocations, RC ops before/after elision, cycle-tracked types. The corpus CI (TESTING.md) tracks RC-elision rate as a regression metric — the number that keeps us honest about "Rust-like, not refcounted-Python-like".
