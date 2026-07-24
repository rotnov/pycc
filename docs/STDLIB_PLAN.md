# pycc Standard Library Plan

Principle: the stdlib subset is itself **typed Python compiled by pycc**, with Rust intrinsics only where unavoidable (syscalls, math primitives). Eating our own dogfood is the best conformance test. Identical surface on all Tier-1 platforms; platform differences live behind `os`/`platform` exactly as in CPython.

## Tier 0 — builtins (v0.1–v0.3, no import needed)

`print`, `len`, `range`, `enumerate`, `zip`, `map`, `filter`, `sum`, `min`, `max`, `abs`, `round`, `sorted`, `reversed`, `any`, `all`, `repr`, `str`/`int`/`float`/`bool` constructors, `isinstance`, `issubclass`, `hash`, `iter`/`next`, `open` (returns typed file objects), `input`, `divmod`, `pow`, `ord`/`chr`, `format`.

Excluded by design: `eval`, `exec`, `compile`, `globals`, `locals`, `vars`, `setattr`/`getattr` with dynamic names on non-interop objects (`E01xx` family).

## Tier 1 — native compiled modules

| Module | Version | Notes |
|---|---|---|
| `math`, `cmath` | v0.2 | intrinsics → LLVM/libm |
| `sys` | v0.2 | argv, exit, stdin/out/err, platform; no refcount APIs |
| `dataclasses`, `enum`, `typing` | v0.3 | typing = compile-time only, zero runtime cost |
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

## Tier 2 — via CPython interop escape hatch

Everything else (`tkinter`, `multiprocessing`, `ctypes`, …) reachable through `pycc.interop` (RUNTIME.md) — explicit, typed at the boundary, never silent.

## Compatibility policy

- Target surface: CPython 3.14 signatures + PEP 594 removals honored (no dead batteries).
- Every implemented function: signature test (typeshed cross-check) + behavior test (differential vs CPython 3.14, all Tier-1 platforms).
- Deviations (e.g. `re` engine corner cases, float repr edge cases) — listed per-module in `docs/semantics.md`, each with a negative/documented test. Undocumented deviation found by corpus bot = release blocker.
