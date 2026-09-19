# pycc Standard Library Plan

Principle: the stdlib subset is itself **typed Python compiled by pycc**, with Rust intrinsics only where unavoidable (syscalls, math primitives). Eating our own dogfood is the best conformance test. Identical surface on all Tier-1 platforms; platform differences live behind `os`/`platform` exactly as in CPython.

## Tier 0 — builtins (v0.1–v0.3, no import needed)

`print`, `len`, `range`, `enumerate`, `zip`, `map`, `filter`, `sum`, `min`, `max`, `abs`, `round`, `sorted`, `reversed`, `any`, `all`, `repr`, `str`/`int`/`float`/`bool` constructors, `isinstance`, `issubclass`, `hash`, `iter`/`next`, `open` (returns typed file objects), `input`, `divmod`, `pow`, `ord`/`chr`, `format`.

Excluded by design: `eval`, `exec`, `compile`, `globals`, `locals`, `vars`, `setattr`/`getattr` with dynamic names on non-interop objects (`E01xx` family).

### `__name__` (W0 of [#882](https://github.com/rotnov/pycc/issues/882), [#1156](https://github.com/rotnov/pycc/issues/1156))

`__name__` is a compiler-provided module-level `str` binding, seeded as the module's first top-level statement. It is provided only when the module references the name and **no dependency of the program mentions the name at all** and the entry module itself does not bind the name `__name__` anywhere in its own module scope, because Part 1 of [#881](https://github.com/rotnov/pycc/issues/881) links every module into one flat namespace in which the seed and a user binding would be the same global. Module scope includes a binding nested inside a top-level compound statement, a `match` case capture, and a walrus, none of which is a function-body local. A dependency is held to the stricter test — any mention, a read as much as a binding, anywhere in the file — because linking places every dependency's top-level statements *ahead* of the entry module's seed, so a dependency's read runs before the seed stores anything. That includes a read reached indirectly, through a dependency's top-level call to one of its own functions, which no binding-only test can see. A user binding wins outright: nothing is seeded and every `__name__` resolves through the ordinary name path, exactly as before this change. A value-less annotation (`__name__: str`) is not such a binding — it only declares a type and emits no store, exactly as in CPython — so the seed survives it. A binding inside a function body is an ordinary local and shadows the module binding only within that function, matching CPython.

Deviation from CPython, deliberate and documented: in CPython a read that textually precedes a module-level `__name__ = ...` still sees the interpreter-provided module name. Here the seed is withheld for the whole module, so such a read resolves to the user's binding — in practice a `T0021` "name `__name__` is not defined" when the read precedes the assignment. This is fail-closed (a diagnostic, never a silently wrong value) and mirrors the all-or-nothing shape [D-188](decisions/D-188-synthesize-hirclassdefs-for-the-builtin-exception.md) already established for the builtin exception hierarchy.

The value depends on the build mode:

| Mode | `__name__` |
|---|---|
| `pycc build` / `pycc run` (native) | `"__main__"` — CPython-faithful for a module run as a script, which is what makes the `if __name__ == "__main__":` idiom take its true branch |
| `pycc build --ext` | the extension module's own name, i.e. the `module_name` `src/ext_output.rs` derives from `-o` and validates as an ASCII identifier |
| `pycc check` | `"__main__"`, so a program checks exactly as it builds |

Known gap, deliberate: only the program's **entry** module is given a name; a non-entry (imported project) module receives none. Part 1 of [#881](https://github.com/rotnov/pycc/issues/881) links every module of a program into one flat namespace, so per-module `__name__` globals would collide. Linking also places every dependency's top-level statements ahead of the entry module's seed, so a dependency's read would observe the global before the seed stored anything — which is why a dependency that *mentions* the name at all, however indirectly, withholds the seed program-wide. The observable consequences today, pinned by `tests/issue_1156_dunder_name.rs`: a dependency's function-body read is `T0021`, whether it is reached from the entry module or from the dependency's own top-level call; a dependency's own *top-level* read is `T0021`; a dependency's `if __name__ == "__main__":` is `T0021` rather than a guard that silently takes the entry module's name; a dependency's read is `T0021` when the entry module never references the name (nothing is seeded at all); and a dependency that never mentions the name leaves the entry module's seed intact. A dependency that binds `__name__` at its *own* top level is a different case and is not affected: the rule above withholds the seed program-wide, so the dependency's binding stays the program's only `__name__`, exactly as before this feature existed. Correct per-module values wait on #881's per-module namespaces.

No new HIR, MIR, type, or codegen node exists for `__name__`: the seed is an ordinary top-level `str` assignment, so `pycc_types`, `pycc_mir`, `pycc_codegen` and `pycc_rt` are untouched by this feature. `crates/pycc_hir/src/dunder_name.rs` owns the rule and both seeding gates.

## Tier 1 — native compiled modules

| Module | Version | Notes |
|---|---|---|
| `math` (`sqrt`, `pi` only) | v0.2 (PR-14, D-136); `import math as m` since #962 (D-231) | the actual shipped v0.2 subset: `math.sqrt(x: float) -> float` (calls the platform libm `sqrt`), `math.pi` (a compile-time `float` constant). Since [#962](https://github.com/rotnov/pycc/issues/962) (Part 1 of #883, [D-231](decisions/D-231-lower-stdlib-import-x-as-y-to-canonical-names-with-an.md)) every registered module can also be imported behind an alias (`import math as m`; `m.sqrt(x)`, `m.pi`): the alias is resolved at HIR lowering and the HIR carries the canonical `math.sqrt`/`math.pi`, so MIR and codegen never see it, and the type checker's receiver shadow check treats a local named after the alias exactly like one named `math`. `from math import sqrt as s` stays `C0001` until [#963](https://github.com/rotnov/pycc/issues/963). Every other `math` name (`floor`, `pow`, `e`, `cmath`, ...) stays unimplemented, rejected with the ordinary `C0002`/`C0001` import diagnostics like any other unrecognized stdlib symbol/module — growing this registry is ordinary follow-up work under the existing `pycc_std` pattern (`crates/pycc_std/src/lib.rs`), not a new design decision (D-136's own Consequences note) |
| `sys` | not started | originally planned for v0.2 alongside `math`; deferred out of PR-14's scope entirely (no `pycc_std::StdModule::Sys` variant exists) — `sys.exit` needs `NoReturn`-shaped divergence handling this compiler's type checker/MIR have no precedent for yet, and `sys.argv` needs a `list[str]`-from-process-args construction path that does not exist either (D-136 addendum in `docs/decisions/D-136-pycc-std-is-a-plain-data-crate-math-sys-symbols.md`). Revisit alongside a future PR that actually builds one of those two prerequisites |
| `dataclasses`, `enum`, `typing`, `abc` | v0.3 (partial) | typing = compile-time only, zero runtime cost. **Shipped so far:** only the marker symbols the class model actually implements, registered in `crates/pycc_std/src/lib.rs` — `enum.Enum`, `enum.StrEnum`, `enum.auto` (#892), `typing.Protocol`, `typing.runtime_checkable`, `typing.override`, `typing.dataclass_transform`, `typing.Final`, `typing.Annotated`, `typing.ClassVar`, `typing.cast`, `typing.TYPE_CHECKING`, `abc.ABC`, `abc.abstractmethod`, and `dataclasses.dataclass` (#579, #762, #767, #790). Each is a compile-time marker with no runtime component, and pycc already recognizes every one of them as a bare name without the import (the `Final`/`Annotated`/`Enum` precedent) — registering them buys import *resolution* instead. For `dataclasses.dataclass`, `typing.override`, and `typing.dataclass_transform` (#579), that resolution was required for CPython's own byte-for-byte conformance oracle: CPython evaluates decorators eagerly (unlike annotations, which PEP 649/749 defers), so the pinned oracle raised `NameError` on those fixtures' imports and they could not be registered in `tests/conformance.rs` at all without this. `typing.ClassVar` (#911) is registered as the third `AnnotationMarker`, stripped to its single type argument like `Final`/`Annotated`, but unlike them it is position-restricted: it is accepted only on a class-body attribute declaration and rejected with `C0001` anywhere else. `typing.Final` and `typing.Annotated` (#762) have no such conformance-fixture dependency — `tests/fixtures/pep_0591_final.py` and `pep_0593_annotated.py` deliberately omit the import, since PEP 649/749 deferred annotation evaluation means CPython's own oracle never evaluates the bare name either — their registration instead removes an artificial `C0002` for code that idiomatically imports `Final`/`Annotated`, independent of any conformance fixture. `typing.cast` (#767) is the one entry here that is *not* purely an import-resolution fix and not purely a marker: `cast(T, value)` is a real call expression, so `pycc_types` intercepts it by bare callee name (like `isinstance`/`issubclass`) and `pycc_mir` lowers it to its second argument alone, matching CPython's runtime no-op. Its registry entry carries a dedicated `StdSymbolKind::CastMarker` only so the bare `from typing import cast` resolves and so the qualified `typing.cast(...)` form gets an accurate diagnostic; the target type is restricted to a bare builtin-scalar or user-class name, with subscripted generics rejected as `C0001`. Because the call is erased with no conversion emitted, the target must also preserve the value's runtime representation, attribute layout, *and* method-dispatch behavior (D-198): a cast to the value's own type, or an up-cast to one of its class's MRO ancestors that crosses no method-override boundary (the nominal relationship is deliberately not verified for that subset, matching CPython's unchecked `cast`). A representation-changing target -- `cast(str, 5)`, and `cast(int, some_bool)` despite `bool` being a static subtype of `int` -- is rejected with `C0001` rather than miscompiled, and so is a genuine down-cast (`cast(Derived, base)`): erasure drops the checker-verified target type before MIR sees it, so accepting a down-cast unconditionally (the first version of this decision) reaches a `pycc_mir` panic or an out-of-bounds `pycc_rt` instance-slot abort instead. An up-cast that crosses a method-override boundary (`cast(Base, derived)` where `Derived` overrides a `Base` method) is rejected too: pycc resolves method calls statically from the cast result's declared type (no vtable), so accepting it unconditionally (the second version of this decision) would silently call `Base`'s implementation instead of CPython's dynamically-dispatched override, with no diagnostic and no crash. `typing.TYPE_CHECKING` (#790) is registered so its import resolves, but the substantive fix lives in `pycc_hir::stmt::is_type_checking_guard`: it recognizes the bare name or the qualified `typing.TYPE_CHECKING` spelling *syntactically*, as an `if`/`elif` test only, and constant-folds that branch to an empty dead body -- matching CPython's always-`False`-at-runtime semantics for the type-checker-only-import guard idiom. This check is **not import-gated and not shadow-aware**: it recognizes the bare name `TYPE_CHECKING` or the qualified `typing.TYPE_CHECKING` attribute access regardless of whether the module actually imports `typing`/`TYPE_CHECKING`, or whether the name is locally shadowed. A module that never imports `typing` but defines its own truthy module-level `TYPE_CHECKING` would silently diverge from CPython (its guarded body would run under CPython but is folded away here) -- flagged by the D-068 pinned reviewer on PR #791 and tracked by [#798](https://github.com/rotnov/pycc/issues/798), deliberately deferred out of #790/#791 because a precise fix requires threading a new import-availability flag through `pycc_hir`'s entire recursive statement-lowering descent (at least 8 function signatures across `stmt.rs`, `stmt/exception.rs`, `func.rs`, and `class.rs`). The fold is also no longer *total*: since [#905](https://github.com/rotnov/pycc/issues/905) the guarded body is re-walked before it is discarded and a post-parse context violation in it (`return` in a `finally`, `break`/`continue` with no loop, `yield` outside a function, `async for`) is still reported as `L0001`, matching CPython, which rejects those at compile time whether or not the branch ever runs; the walk reports nothing else, so a guarded body full of unimplemented constructs still compiles. `enum.StrEnum` and `enum.auto` (#892) are the two entries here that change observable *compilation* behavior rather than only import resolution: `StrEnum` is a second marker base name, recognized like `Enum`, whose members must all be `str`; `auto` is recognized syntactically inside an enum body (`RED = auto()`) and derives the member's value, so like every other marker it has no value of its own outside that position. Everything else in these modules — `dataclasses.field`/`asdict`/`replace`, `enum.IntEnum`/`Flag`/`IntFlag`, the rest of `typing` — stays unregistered and rejected with the ordinary `C0002` |
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
CPython-backed and keeps values typed at the generated boundary according to
[RUNTIME.md](./RUNTIME.md). In embedded mode it also bundles their pinned
runtime/package closure (D-128): runtime inclusion is automatic under the
default `auto` policy but never invisible in build metadata or `pycc.lock`,
while `allowlist` and `deny`/`--pure` provide stricter deployment policies.
The planned hosted `ext` mode is the exception to that bundling and to those
policies (D-244).

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
