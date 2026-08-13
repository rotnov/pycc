# Python Language Standards → pycc Conformance Matrix

Every language standard (PEP) that defines Python up to the v1.0 target,
**3.14**, mapped to a conformance test. Accepted Python 3.15 standards are
tracked separately as a post-v1.0 preview. This file is the single source of
truth for what pycc must implement. A feature is "supported" only when its test
passes; a feature is "rejected by design" only when its *negative* test asserts
the exact diagnostic.

**Conventions**

- Test path: `tests/conformance/pyXY/pep_NNNN_slug.py`. Every supported
  language level runs all fixture directories through that level and compares
  them with its pinned oracle. The v1.0 Python 3.14 run therefore covers
  `py30/` through `py314/` against CPython 3.14.6. After the v1.x gate opens,
  the Python 3.15 run covers `py30/` through `py315/` against its pinned
  current 3.15 patch, while the independent 3.14 compatibility run remains
  required. This describes the eventual v1.0-scale, language-level-selecting
  harness; per [D-102](./decisions/D-102-extend-tests-conformance-rs-for-pr-9-s-9-new-pep.md)
  the fixtures PR-9 added live flat at `tests/fixtures/pep_NNNN_slug.py`
  instead, run directly by `tests/conformance.rs` — the `pyXY/` tree and its
  language-level selection do not exist yet.
- Negative tests: `tests/diagnostics/dNNNN_slug.py` — must *fail to compile* with the documented error code.
- Category: `syntax` · `typing` · `sem` (semantics/data model) · `import` · `rt` (runtime) — `rt`-only PEPs may need design docs instead of codegen.
- Status: ☐ planned · ⚙ in progress · ✅ passing.

Reference: [PEP index](https://peps.python.org/), [What's New in Python](https://docs.python.org/3/whatsnew/).

## Upstream release baseline

Last reviewed against the official Python release pages, release-schedule PEPs,
and What's New documents on **2026-07-24**:

| Track | Upstream checkpoint | pycc consequence |
|---|---|---|
| v1 stable oracle | Python **3.14.6** final, released 2026-06-10 | Conformance recordings and CI oracle setup are pinned to 3.14.6 (moved from 3.14.3 in PR-6); re-record outputs again whenever the oracle version next changes. |
| Post-v1 preview | Python **3.15.0b4**, released 2026-07-18 | Feature-frozen since 3.15.0b1; track only Final/Accepted Standards Track PEPs below until 3.15.0 final. |

D-012 remains unchanged: v1 accepts exactly Python 3.14. The 3.15 rows do not
expand the v1 grammar or acceptance gate. Promoting them into a supported
language level requires Python 3.15 final, the post-v1 roadmap gate, and a new
ADR that supersedes D-012.

The gate-opening change must also add a machine-readable supported-language
registry consumed by project-configuration validation and `pycc_testkit`. For
each accepted language level, the registry binds its configuration value,
cumulative fixture range, and pinned CPython oracle. CI must reject drift
between that registry, this matrix, and the conformance jobs. Until then,
preview rows are planning inventory only and do not make Python 3.15 a
supported input level.

For each newly observed upstream release:

1. Update this baseline and [ROADMAP.md](./ROADMAP.md) in the same change.
2. For a 3.14 maintenance release, review What's New and the changelog for
   observable semantic changes, then advance the pinned differential oracle.
3. For a 3.15 prerelease after feature freeze, add newly Final/Accepted
   Standards Track PEPs to the preview section; Draft, Deferred, and Rejected
   proposals are not implementation commitments.
4. Do not flip conformance statuses by hand; CI still owns the status column.
   [D-102](./decisions/D-102-extend-tests-conformance-rs-for-pr-9-s-9-new-pep.md)
   is the one accepted interim exception: no automation backs this column
   today, so PR-9's 9 rows were flipped by hand, only after each fixture was
   observed passing on a real, already-completed CI run across all 5 Tier-1
   targets in both build profiles. Building the real automation remains a
   tracked `docs/ROADMAP.md` follow-up.

## Python 3.0–3.2 (foundations)

| PEP | Feature | Cat | Test | St |
|---|---|---|---|---|
| [238](https://peps.python.org/pep-0238/) | True division `/` vs `//` | sem | `py30/pep_0238_division.py` | ✅ |
| [3102](https://peps.python.org/pep-3102/) | Keyword-only arguments | syntax | `py30/pep_3102_kwonly.py` | ☐ |
| [3104](https://peps.python.org/pep-3104/) | `nonlocal` | syntax | `py30/pep_3104_nonlocal.py` | ☐ |
| [3105](https://peps.python.org/pep-3105/) | `print()` as function | syntax | `py30/pep_3105_print.py` | ✅ |
| [3107](https://peps.python.org/pep-3107/) | Function annotations | typing | `py30/pep_3107_annotations.py` | ✅ |
| [3115](https://peps.python.org/pep-3115/) | Metaclasses (`metaclass=`) | sem | `py30/pep_3115_metaclass.py` | ☐ |
| [3119](https://peps.python.org/pep-3119/) | ABCs, `isinstance` hooks | sem | `py30/pep_3119_abc.py` | ☐ |
| [3129](https://peps.python.org/pep-3129/) | Class decorators | syntax | `py30/pep_3129_class_deco.py` | ☐ |
| [3131](https://peps.python.org/pep-3131/) | Non-ASCII identifiers | syntax | `py30/pep_3131_unicode_ids.py` | ✅ |
| [3132](https://peps.python.org/pep-3132/) | Extended unpacking `a, *b = ...` | syntax | `py30/pep_3132_unpack.py` | ☐ |
| [3135](https://peps.python.org/pep-3135/) | Zero-argument `super()` | sem | `py30/pep_3135_super.py` | ☐ |
| — | `str`/`bytes` split, all-new-style classes | sem | `py30/str_bytes_model.py` | ☐ |

## Python 3.3

| PEP | Feature | Cat | Test | St |
|---|---|---|---|---|
| [380](https://peps.python.org/pep-0380/) | `yield from` | syntax | `py33/pep_0380_yield_from.py` | ☐ |
| [409](https://peps.python.org/pep-0409/) | `raise ... from None` | sem | `py33/pep_0409_from_none.py` | ☐ |
| [414](https://peps.python.org/pep-0414/) | `u''` literals | syntax | `py33/pep_0414_u_literal.py` | ✅ |
| [420](https://peps.python.org/pep-0420/) | Namespace packages | import | `py33/pep_0420_ns_packages.py` | ☐ |
| [3151](https://peps.python.org/pep-3151/) | `OSError` hierarchy | sem | `py33/pep_3151_oserror.py` | ☐ |

*n/a for codegen: PEP 393 (flexible str storage — internal; pycc uses its own UTF-8 representation, documented in `docs/semantics.md`).*

## Python 3.4

| PEP | Feature | Cat | Test | St |
|---|---|---|---|---|
| [435](https://peps.python.org/pep-0435/) | `enum` | sem | `pep_0435_enum.py` | ☐ |
| [443](https://peps.python.org/pep-0443/) | `functools.singledispatch` | sem | `py34/pep_0443_singledispatch.py` | ☐ |
| [451](https://peps.python.org/pep-0451/) | Module spec import model | import | `py34/pep_0451_modspec.py` | ☐ |
| [3156](https://peps.python.org/pep-3156/) | `asyncio` event loop model | rt | `py34/pep_3156_asyncio.py` | ☐ |

## Python 3.5

| PEP | Feature | Cat | Test | St |
|---|---|---|---|---|
| [448](https://peps.python.org/pep-0448/) | Unpacking generalizations `*`/`**` | syntax | `py35/pep_0448_unpack_gen.py` | ☐ |
| [461](https://peps.python.org/pep-0461/) | `%` formatting for bytes | sem | `py35/pep_0461_bytes_fmt.py` | ☐ |
| [465](https://peps.python.org/pep-0465/) | `@` matrix-multiply operator | syntax | `py35/pep_0465_matmul.py` | ☐ |
| [484](https://peps.python.org/pep-0484/) | **Type hints** — pycc's cornerstone | typing | `py35/pep_0484_type_hints.py` | ✅ |
| [492](https://peps.python.org/pep-0492/) | `async` / `await` syntax | syntax | `py35/pep_0492_async_await.py` | ☐ |

## Python 3.6

| PEP | Feature | Cat | Test | St |
|---|---|---|---|---|
| [487](https://peps.python.org/pep-0487/) | `__init_subclass__`, `__set_name__` | sem | `py36/pep_0487_init_subclass.py` | ☐ |
| [498](https://peps.python.org/pep-0498/) | f-strings | syntax | `py36/pep_0498_fstrings.py` | ✅ |
| [515](https://peps.python.org/pep-0515/) | Underscores in numeric literals | syntax | `py36/pep_0515_underscores.py` | ✅ |
| [525](https://peps.python.org/pep-0525/) | Async generators | syntax | `py36/pep_0525_async_gen.py` | ☐ |
| [526](https://peps.python.org/pep-0526/) | Variable annotations `x: int = 0` | typing | `py36/pep_0526_var_annotations.py` | ✅ |
| [530](https://peps.python.org/pep-0530/) | Async comprehensions | syntax | `py36/pep_0530_async_comp.py` | ☐ |

## Python 3.7

| PEP | Feature | Cat | Test | St |
|---|---|---|---|---|
| [553](https://peps.python.org/pep-0553/) | `breakpoint()` | rt | `py37/pep_0553_breakpoint.py` | ☐ |
| [557](https://peps.python.org/pep-0557/) | **dataclasses** | sem | `py37/pep_0557_dataclasses.py` | ☐ |
| [560](https://peps.python.org/pep-0560/) | `__class_getitem__` typing support | typing | `py37/pep_0560_class_getitem.py` | ☐ |
| [562](https://peps.python.org/pep-0562/) | Module `__getattr__` | sem | `py37/pep_0562_mod_getattr.py` | ☐ |
| [563](https://peps.python.org/pep-0563/) | `from __future__ import annotations` (superseded by 649) | typing | `py37/pep_0563_lazy_annotations.py` | ☐ |
| — | `dict` insertion order guaranteed (`dict[str, int]`, D-123) | sem | `dict_order.py` | ✅ |

## Python 3.8

| PEP | Feature | Cat | Test | St |
|---|---|---|---|---|
| [544](https://peps.python.org/pep-0544/) | `Protocol` — structural typing | typing | `py38/pep_0544_protocol.py` | ☐ |
| [570](https://peps.python.org/pep-0570/) | Positional-only params `/` | syntax | `py38/pep_0570_pos_only.py` | ✅ |
| [572](https://peps.python.org/pep-0572/) | Walrus `:=` | syntax | `py38/pep_0572_walrus.py` | ☐ |
| [586](https://peps.python.org/pep-0586/) | `Literal` | typing | `py38/pep_0586_literal.py` | ☐ |
| [589](https://peps.python.org/pep-0589/) | `TypedDict` | typing | `py38/pep_0589_typeddict.py` | ☐ |
| [591](https://peps.python.org/pep-0591/) | `Final` | typing | `py38/pep_0591_final.py` | ✅ |
| — | f-string `=` debug specifier | syntax | `py38/fstring_eq.py` | ☐ |

## Python 3.9

| PEP | Feature | Cat | Test | St |
|---|---|---|---|---|
| [584](https://peps.python.org/pep-0584/) | `dict \| dict` union | sem | `py39/pep_0584_dict_union.py` | ☐ |
| [585](https://peps.python.org/pep-0585/) | Builtin generics: `list[int]` (D-105), `dict[str, int]`/`set[int]` (D-121/D-122, PR-11a), and `tuple[...]` (D-115/D-116, PR-11b) all ship real codegen — this row's `✅` reflects all four fixtures' own CI-observed, all-5-Tier-1-target evidence ([PR #305](https://github.com/rotnov/pycc/pull/305)). `tuple[...]`'s own literal construction, `t[k]` literal-index reads, and both module-global and function-local storage are covered; passing or returning a tuple *value* across a function boundary from real, unannotated Python source does not work yet even though the codegen layer already supports it, for two independent reasons: `pycc_types`' signature-inference solver is scalar-only (D-116 point 4's correction note), and `pycc_codegen`'s `emit_expr` has no `MirExpr::Call` result-dispatch arm for a container-typed return either (D-116's own further correction note) — deferred alongside iteration, unpacking assignment, and an annotation syntax (see `docs/ROADMAP.md`) | typing | `pep_0585_builtin_generics.py`, `dict_order.py`, `pep_0585_set_int.py`, `tuple_heterogeneous.py` | ✅ |
| [593](https://peps.python.org/pep-0593/) | `Annotated` | typing | `py39/pep_0593_annotated.py` | ✅ |
| [614](https://peps.python.org/pep-0614/) | Relaxed decorator grammar | syntax | `py39/pep_0614_decorators.py` | ☐ |
| [617](https://peps.python.org/pep-0617/) | PEG parser — **pycc's grammar reference** | syntax | (covered by whole suite) | ☐ |

## Python 3.10

| PEP | Feature | Cat | Test | St |
|---|---|---|---|---|
| [604](https://peps.python.org/pep-0604/) | Union syntax `int \| str` | typing | `py310/pep_0604_union.py` | ☐ |
| [612](https://peps.python.org/pep-0612/) | `ParamSpec` | typing | `py310/pep_0612_paramspec.py` | ☐ |
| [613](https://peps.python.org/pep-0613/) | `TypeAlias` | typing | `pep_0613_typealias.py` | ✅ |
| [626](https://peps.python.org/pep-0626/) | Precise line numbers (debug info) | rt | `py310/pep_0626_lineno.py` | ☐ |
| [634](https://peps.python.org/pep-0634/)–636 | **Structural pattern matching** | syntax | `py310/pep_0634_match.py` | ☐ |
| [647](https://peps.python.org/pep-0647/) | `TypeGuard` | typing | `py310/pep_0647_typeguard.py` | ☐ |
| — | Parenthesized context managers | syntax | `py310/paren_with.py` | ☐ |

## Python 3.11

| PEP | Feature | Cat | Test | St |
|---|---|---|---|---|
| [646](https://peps.python.org/pep-0646/) | `TypeVarTuple` — variadic generics | typing | `py311/pep_0646_typevartuple.py` | ☐ |
| [654](https://peps.python.org/pep-0654/) | `except*` + `ExceptionGroup` | syntax | `py311/pep_0654_except_star.py` | ☐ |
| [655](https://peps.python.org/pep-0655/) | `Required` / `NotRequired` | typing | `py311/pep_0655_required.py` | ☐ |
| [657](https://peps.python.org/pep-0657/) | Fine-grained error locations | rt | (drives pycc diagnostics UX) | ☐ |
| [673](https://peps.python.org/pep-0673/) | `Self` as method return/param annotation (#387 Part 1) — resolves to the class's own instance type at HIR-lowering time | typing | `pep_0673_self.py` | ☐ |
| [675](https://peps.python.org/pep-0675/) | `LiteralString` | typing | `py311/pep_0675_literalstring.py` | ☐ |
| [681](https://peps.python.org/pep-0681/) | `dataclass_transform` | typing | `py311/pep_0681_dc_transform.py` | ☐ |

## Python 3.12

| PEP | Feature | Cat | Test | St |
|---|---|---|---|---|
| [688](https://peps.python.org/pep-0688/) | `Buffer` type | typing | `py312/pep_0688_buffer.py` | ☐ |
| [692](https://peps.python.org/pep-0692/) | `Unpack[TypedDict]` for `**kwargs` | typing | `py312/pep_0692_kwargs.py` | ☐ |
| [695](https://peps.python.org/pep-0695/) | **`type` statement + generic functions** `def f[T](x: T) -> T` (v0.2 scope per D-088 — no class support exists yet) | typing | `pep_0695_generics.py` | ✅ |
| [695](https://peps.python.org/pep-0695/) | Generic classes `class C[T]` (#387 Part 3) — single type param, scalar-only instantiation, monomorphized at compile time | typing | `pep_0695_generic_classes.py` | ☐ |
| [698](https://peps.python.org/pep-0698/) | `@override` | typing | `py312/pep_0698_override.py` | ☐ |
| [701](https://peps.python.org/pep-0701/) | Formalized f-string grammar | syntax | `py312/pep_0701_fstring_grammar.py` | ☐ |
| [709](https://peps.python.org/pep-0709/) | Comprehension inlining semantics -- pycc has no bytecode/frame model to "inline" the way CPython's own PEP 709 change does; this row instead verifies the one CPython-observable, statically-testable guarantee PEP 709 depends on: a comprehension's own loop variable does not leak into an enclosing same-named binding (D-117/D-120) | sem | `pep_0709_comp_inline.py` | ✅ |

*n/a for native execution: PEP 684 configures CPython interpreter GILs, while
pycc-native code has no GIL. A planned embedded CPython interop boundary keeps
the pinned interpreter's own GIL only for CPython-backed operations (D-128).*

## Python 3.13

| PEP | Feature | Cat | Test | St |
|---|---|---|---|---|
| [594](https://peps.python.org/pep-0594/) | "Dead batteries" stdlib removals | rt | `py313/pep_0594_dead_battery.py` + `py313/pep_0594_dead_battery_rejected.py` (D-138; both fixtures observed passing across all 5 Tier-1 targets on [PR #328](https://github.com/rotnov/pycc/pull/328)'s final green CI run) | ✅ |
| [696](https://peps.python.org/pep-0696/) | TypeVar defaults | typing | `py313/pep_0696_typevar_defaults.py` | ☐ |
| [702](https://peps.python.org/pep-0702/) | `@deprecated` | typing | `py313/pep_0702_deprecated.py` | ☐ |
| [703](https://peps.python.org/pep-0703/) | Free-threading (experimental) | rt | superseded by 779 test | ☐ |
| [742](https://peps.python.org/pep-0742/) | `TypeIs` — narrowing | typing | `py313/pep_0742_typeis.py` | ☐ |

*n/a: PEP 744 (JIT — pycc is AOT), new REPL.*

## Python 3.14 ← v1.0 target

| PEP | Feature | Cat | Test | St |
|---|---|---|---|---|
| [649](https://peps.python.org/pep-0649/)/[749](https://peps.python.org/pep-0749/) | **Deferred annotations** — pycc reads annotations exactly per 3.14 semantics; self-referential class-name annotations in own methods work (#387 Part 2) | typing | `pep_0649_deferred_ann.py` | ☐ |
| [734](https://peps.python.org/pep-0734/) | Subinterpreters in stdlib | rt | `py314/pep_0734_interpreters.py` | ☐ |
| [750](https://peps.python.org/pep-0750/) | **Template strings (t-strings)** | syntax | `py314/pep_0750_tstrings.py` | ☐ |
| [758](https://peps.python.org/pep-0758/) | `except A, B:` without parentheses | syntax | `py314/pep_0758_except_noparens.py` | ☐ |
| [765](https://peps.python.org/pep-0765/) | No `return`/`break`/`continue` in `finally` | sem | `py314/pep_0765_finally.py` | ☐ |
| [779](https://peps.python.org/pep-0779/) | Free-threading officially supported → pycc: GIL-free native threads | rt | `py314/pep_0779_threads.py` | ☐ |

*n/a: PEP 768 (remote debugging interface), 784 (zstd in stdlib — will ship in pycc stdlib subset), 741 (C API config), 761 (release signing), 776 (Emscripten).*

## Python 3.15 preview (post-v1.0)

These rows reflect the feature-frozen 3.15.0b4 surface. They are planning
inputs for the post-v1.0 language-level upgrade and are not part of the Python
3.14 v1 acceptance gate.

| PEP | Feature | Cat | Test | St |
|---|---|---|---|---|
| [661](https://peps.python.org/pep-0661/) | `sentinel()` builtin, singleton-value typing, and `is` narrowing | typing | `py315/pep_0661_sentinel.py` | ☐ |
| [686](https://peps.python.org/pep-0686/) | UTF-8 mode becomes the default encoding behavior | rt | `py315/pep_0686_utf8_default.py` | ☐ |
| [728](https://peps.python.org/pep-0728/) | `TypedDict` with typed extra items | typing | `py315/pep_0728_typeddict_extra_items.py` | ☐ |
| [747](https://peps.python.org/pep-0747/) | `TypeForm` annotations for type expressions | typing | `py315/pep_0747_typeform.py` | ☐ |
| [791](https://peps.python.org/pep-0791/) | `math.integer` integer-specific mathematics | rt | `py315/pep_0791_math_integer.py` | ☐ |
| [798](https://peps.python.org/pep-0798/) | `*`/`**` unpacking in comprehensions and generator expressions | syntax | `py315/pep_0798_unpacking_comprehensions.py` | ☐ |
| [799](https://peps.python.org/pep-0799/) | Standard `profiling` package | rt | `py315/pep_0799_profiling.py` | ☐ |
| [800](https://peps.python.org/pep-0800/) | Disjoint bases in the type system | typing | `py315/pep_0800_disjoint_bases.py` | ☐ |
| [810](https://peps.python.org/pep-0810/) | Explicit `lazy import` / `lazy from` syntax and deferred import semantics | import | `py315/pep_0810_lazy_imports.py` | ☐ |
| [814](https://peps.python.org/pep-0814/) | Builtin immutable `frozendict` | sem | `py315/pep_0814_frozendict.py` | ☐ |
| [829](https://peps.python.org/pep-0829/) | Auditable package-startup `.start` files | import | `py315/pep_0829_package_startup.py` | ☐ |

*n/a for native Python semantics: PEP 782 (`PyBytesWriter` C API), 788
(interpreter-finalization C API guards), 793/803/820 (extension entry points,
free-threaded stable ABI, and unified C slots), and 831 (CPython frame-pointer
build policy). Revisit the C API items only for the v0.7+ CPython interop
boundary.*

## Rejected by design → negative tests

Each entry must produce the documented error code, tested in `tests/diagnostics/`:

| Construct | Error | Test |
|---|---|---|
| `eval` / `exec` / `compile` | `E0100` | `d0100_eval.py` |
| Monkey-patching (attribute assignment on foreign classes/modules) | `E0101` | `d0101_monkeypatch.py` |
| Untyped public function signature | `T0001` | `d0001_missing_public_annotation.py` |
| `Any` outside a compiler-classified CPython interop boundary (planned v0.7; the current v0.1 snapshot retains its legacy “declared boundary” wording) | `T0002` | `d0002_any_forbidden.py` |
| Dynamic attribute injection on non-`dict`-like objects | `E0102` | `d0102_dyn_attr.py` |
| `type()` three-arg dynamic class creation | `E0103` | `d0103_dyn_class.py` |
| Wildcard `import *` | `E0104` | `d0104_star_import.py` |

## Real-world corpus (integration tests, planned)

Planned nightly CI would compile pinned revisions of open-source projects and run their own test suites against pycc-built artifacts. New incompatibilities would auto-file issues in the pycc repo (minimized repro + PEP reference + diagnostic). No corpus workflow exists on current `main`.

| Tier | Projects | Gate |
|---|---|---|
| 1 — small, well-typed | `tomli`, `packaging`, `more-itertools` | milestone gate not yet assigned — empirically shown unreachable at v0.2 (these packages' non-test source needs stdlib modules scheduled through v0.8, plus several `STDLIB_PLAN.md` does not schedule at all; see D-088). v0.2 uses a hand-authored container/generics corpus instead (D-088). Re-verifying this tier's real milestone against actual import surfaces, the same way D-088 verified it for v0.2, is tracked in #183 |
| 2 — medium | `black`, `isort`, `attrs`, `click` | tracked pass-rate |
| 3 — large / typed | `mypy`, `httpx`, `rich` | tracked pass-rate |
| 4 — stretch | `fastapi` + `pydantic` stack | aspirational |

Rules: pinned commits for reproducibility; per-project pass-rate dashboard; regressions block release; genuine upstream type bugs found by pycc are reported upstream manually (curated, never bot-spammed).
