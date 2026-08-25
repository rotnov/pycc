# pycc Standard Library Plan

Principle: the stdlib subset is itself **typed Python compiled by pycc**, with Rust intrinsics only where unavoidable (syscalls, math primitives). Eating our own dogfood is the best conformance test. Identical surface on all Tier-1 platforms; platform differences live behind `os`/`platform` exactly as in CPython.

## Tier 0 — builtins (v0.1–v0.3, no import needed)

`print`, `len`, `range`, `enumerate`, `zip`, `map`, `filter`, `sum`, `min`, `max`, `abs`, `round`, `sorted`, `reversed`, `any`, `all`, `repr`, `str`/`int`/`float`/`bool` constructors, `isinstance`, `issubclass`, `hash`, `iter`/`next`, `open` (returns typed file objects), `input`, `divmod`, `pow`, `ord`/`chr`, `format`.

Excluded by design: `eval`, `exec`, `compile`, `globals`, `locals`, `vars`, `setattr`/`getattr` with dynamic names on non-interop objects (`E01xx` family).

## Tier 1 — native compiled modules

| Module | Version | Notes |
|---|---|---|
| `math` (`sqrt`, `pi` only) | v0.2 (PR-14, D-136) | the actual shipped v0.2 subset: `math.sqrt(x: float) -> float` (calls the platform libm `sqrt`), `math.pi` (a compile-time `float` constant). Every other `math` name (`floor`, `pow`, `e`, `cmath`, ...) stays unimplemented, rejected with the ordinary `C0002`/`C0001` import diagnostics like any other unrecognized stdlib symbol/module — growing this registry is ordinary follow-up work under the existing `pycc_std` pattern (`crates/pycc_std/src/lib.rs`), not a new design decision (D-136's own Consequences note) |
| `sys` | not started | originally planned for v0.2 alongside `math`; deferred out of PR-14's scope entirely (no `pycc_std::StdModule::Sys` variant exists) — `sys.exit` needs `NoReturn`-shaped divergence handling this compiler's type checker/MIR have no precedent for yet, and `sys.argv` needs a `list[str]`-from-process-args construction path that does not exist either (D-136 addendum in `docs/decisions/D-136-pycc-std-is-a-plain-data-crate-math-sys-symbols.md`). Revisit alongside a future PR that actually builds one of those two prerequisites |
| `dataclasses`, `enum`, `typing`, `abc` | v0.3 (partial) | typing = compile-time only, zero runtime cost. **Shipped so far:** only the marker symbols the class model actually implements, registered in `crates/pycc_std/src/lib.rs` — `enum.Enum`, `typing.Protocol`, `typing.runtime_checkable`, `typing.override`, `typing.dataclass_transform`, `typing.Final`, `typing.Annotated`, `typing.cast`, `typing.TYPE_CHECKING`, `abc.ABC`, `abc.abstractmethod`, and `dataclasses.dataclass` (#579, #762, #767, #790). Each is a compile-time marker with no runtime component, and pycc already recognizes every one of them as a bare name without the import (the `Final`/`Annotated`/`Enum` precedent) — registering them buys import *resolution* instead. For `dataclasses.dataclass`, `typing.override`, and `typing.dataclass_transform` (#579), that resolution was required for CPython's own byte-for-byte conformance oracle: CPython evaluates decorators eagerly (unlike annotations, which PEP 649/749 defers), so the pinned oracle raised `NameError` on those fixtures' imports and they could not be registered in `tests/conformance.rs` at all without this. `typing.Final` and `typing.Annotated` (#762) have no such conformance-fixture dependency — `tests/fixtures/pep_0591_final.py` and `pep_0593_annotated.py` deliberately omit the import, since PEP 649/749 deferred annotation evaluation means CPython's own oracle never evaluates the bare name either — their registration instead removes an artificial `C0002` for code that idiomatically imports `Final`/`Annotated`, independent of any conformance fixture. `typing.cast` (#767) is the one entry here that is *not* purely an import-resolution fix and not purely a marker: `cast(T, value)` is a real call expression, so `pycc_types` intercepts it by bare callee name (like `isinstance`/`issubclass`) and `pycc_mir` lowers it to its second argument alone, matching CPython's runtime no-op. Its registry entry carries a dedicated `StdSymbolKind::CastMarker` only so the bare `from typing import cast` resolves and so the qualified `typing.cast(...)` form gets an accurate diagnostic; the target type is restricted to a bare builtin-scalar or user-class name, with subscripted generics rejected as `C0001`. Because the call is erased with no conversion emitted, the target must also preserve the value's runtime representation, attribute layout, *and* method-dispatch behavior (D-198): a cast to the value's own type, or an up-cast to one of its class's MRO ancestors that crosses no method-override boundary (the nominal relationship is deliberately not verified for that subset, matching CPython's unchecked `cast`). A representation-changing target -- `cast(str, 5)`, and `cast(int, some_bool)` despite `bool` being a static subtype of `int` -- is rejected with `C0001` rather than miscompiled, and so is a genuine down-cast (`cast(Derived, base)`): erasure drops the checker-verified target type before MIR sees it, so accepting a down-cast unconditionally (the first version of this decision) reaches a `pycc_mir` panic or an out-of-bounds `pycc_rt` instance-slot abort instead. An up-cast that crosses a method-override boundary (`cast(Base, derived)` where `Derived` overrides a `Base` method) is rejected too: pycc resolves method calls statically from the cast result's declared type (no vtable), so accepting it unconditionally (the second version of this decision) would silently call `Base`'s implementation instead of CPython's dynamically-dispatched override, with no diagnostic and no crash. `typing.TYPE_CHECKING` (#790) is registered so its import resolves, but the substantive fix lives in `pycc_hir::stmt::is_type_checking_guard`: it recognizes the bare name or the qualified `typing.TYPE_CHECKING` spelling *syntactically*, as an `if`/`elif` test only, and constant-folds that branch to an empty dead body -- matching CPython's always-`False`-at-runtime semantics for the type-checker-only-import guard idiom (see `docs/ROADMAP.md`'s #790 entry for the full rationale). Everything else in these modules — `dataclasses.field`/`asdict`/`replace`, `enum.IntEnum`/`auto`, the rest of `typing` — stays unregistered and rejected with the ordinary `C0002` |
| `os`, `os.path`, `pathlib` | v0.4 | full Windows/POSIX parity — CI-gated on all Tier-1 |
| `time`, `datetime` | v0.4 | |
| `json` | v0.4 | serde-grade native perf |
| `collections` (`deque`, `Counter`, `defaultdict`, `namedtuple`) | v0.4 | |
| `itertools`, `functools` (`partial`, `reduce`, `lru_cache`, `cache`) | v0.5 | |
| `io`, `struct`, `csv` | v0.5 | |
| `re` | v0.5 | `regex` crate engine; documented deviation list vs `sre` |
| `random`, `secrets`, `hashlib`, `base64`, `uuid` | v0.6 | |
| `subprocess`, `shutil`, `tempfile`, `glob` | v0.6 | |
| `threading`, `queue` | v0.6 | GIL-free semantics per MEMORY_OWNERSHIP.md |
| `argparse`, `logging` | v0.7 | |
| `unittest` (subset), `contextlib`, `abc`, `copy`, `pickle` (subset) | v0.7 | pickle: typed protocol-5 subset |
| `socket`, `ssl`, `http.client`, `urllib` (subset) | v0.8 | |
| `asyncio` (subset) | v0.9 | surface for state-machine async |
| `zlib`, `gzip`, `bz2`, `lzma`, `compression.zstd` (PEP 784) | v0.8 | rust crates underneath |

## Tier 2 — via transparent CPython interop

Everything else (`numpy`, `tkinter`, `multiprocessing`, `ctypes`, …) remains
ordinary standard-Python source (`import numpy`, not a required
`pycc.interop` rewrite). Planned v0.7 classifies these imports as
CPython-backed, bundles their pinned runtime/package closure, and keeps values
typed at the generated boundary according to [RUNTIME.md](./RUNTIME.md) and
D-128. Runtime inclusion is automatic under the default `auto` policy but
never invisible in build metadata or `pycc.lock`; `allowlist` and
`deny`/`--pure` provide stricter deployment policies.

## Compatibility policy

- Target surface: CPython 3.14 signatures + PEP 594 removals honored (no dead batteries).
- Every implemented function: signature test (typeshed cross-check) + behavior test (differential vs CPython 3.14, all Tier-1 platforms).
- Deviations (e.g. `re` engine corner cases, float repr edge cases) — listed per-module in `docs/semantics.md`, each with a negative/documented test. Undocumented deviation found by corpus bot = release blocker.

## Python 3.15 preview (post-v1.0)

The v1 surface remains CPython 3.14. The v1.x upgrade defined in ROADMAP.md
adds these feature-frozen 3.15 deltas:

| PEP | Surface | Plan |
|---|---|---|
| 661 | `sentinel()` builtin | Add to Tier 0 with identity, copy/pickle, repr, truthiness, and typing semantics covered by the PEP test. |
| 686 | UTF-8 mode by default | Make default text I/O, locale interaction, and environment overrides match the pinned 3.15 oracle. |
| 791 | `math.integer` | Add beside `math` in Tier 1. |
| 799 | `profiling` | Add the public package surface; native profiler integration may use pycc-specific internals without changing its API. |
| 814 | `frozendict` builtin | Add to Tier 0 with immutable mapping, hashing, and union semantics. |

PEP 810 lazy imports and PEP 829 package-startup files belong to the import
contract in PYTHON_STANDARDS.md rather than the module inventory here.
