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
