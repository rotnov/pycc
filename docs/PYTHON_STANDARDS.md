# Python Language Standards → pycc Conformance Matrix

Every language standard (PEP) that defines Python up to **3.14**, mapped to a conformance test. This file is the single source of truth for what pycc must implement. A feature is "supported" only when its test passes; a feature is "rejected by design" only when its *negative* test asserts the exact diagnostic.

**Conventions**

- Test path: `tests/conformance/pyXY/pep_NNNN_slug.py` — compiled by pycc, executed, stdout compared against CPython 3.14 running the same file.
- Negative tests: `tests/diagnostics/dNNNN_slug.py` — must *fail to compile* with the documented error code.
- Category: `syntax` · `typing` · `sem` (semantics/data model) · `import` · `rt` (runtime) — `rt`-only PEPs may need design docs instead of codegen.
- Status: ☐ planned · ⚙ in progress · ✅ passing.

Reference: [PEP index](https://peps.python.org/), [What's New in Python](https://docs.python.org/3/whatsnew/).

## Python 3.0–3.2 (foundations)

| PEP | Feature | Cat | Test | St |
|---|---|---|---|---|
| [238](https://peps.python.org/pep-0238/) | True division `/` vs `//` | sem | `py30/pep_0238_division.py` | ☐ |
| [3102](https://peps.python.org/pep-3102/) | Keyword-only arguments | syntax | `py30/pep_3102_kwonly.py` | ☐ |
| [3104](https://peps.python.org/pep-3104/) | `nonlocal` | syntax | `py30/pep_3104_nonlocal.py` | ☐ |
| [3105](https://peps.python.org/pep-3105/) | `print()` as function | syntax | `py30/pep_3105_print.py` | ☐ |
| [3107](https://peps.python.org/pep-3107/) | Function annotations | typing | `py30/pep_3107_annotations.py` | ☐ |
| [3115](https://peps.python.org/pep-3115/) | Metaclasses (`metaclass=`) | sem | `py30/pep_3115_metaclass.py` | ☐ |
| [3119](https://peps.python.org/pep-3119/) | ABCs, `isinstance` hooks | sem | `py30/pep_3119_abc.py` | ☐ |
| [3129](https://peps.python.org/pep-3129/) | Class decorators | syntax | `py30/pep_3129_class_deco.py` | ☐ |
| [3131](https://peps.python.org/pep-3131/) | Non-ASCII identifiers | syntax | `py30/pep_3131_unicode_ids.py` | ☐ |
| [3132](https://peps.python.org/pep-3132/) | Extended unpacking `a, *b = ...` | syntax | `py30/pep_3132_unpack.py` | ☐ |
| [3135](https://peps.python.org/pep-3135/) | Zero-argument `super()` | sem | `py30/pep_3135_super.py` | ☐ |
| — | `str`/`bytes` split, all-new-style classes | sem | `py30/str_bytes_model.py` | ☐ |

## Python 3.3

| PEP | Feature | Cat | Test | St |
|---|---|---|---|---|
| [380](https://peps.python.org/pep-0380/) | `yield from` | syntax | `py33/pep_0380_yield_from.py` | ☐ |
| [409](https://peps.python.org/pep-0409/) | `raise ... from None` | sem | `py33/pep_0409_from_none.py` | ☐ |
| [414](https://peps.python.org/pep-0414/) | `u''` literals | syntax | `py33/pep_0414_u_literal.py` | ☐ |
| [420](https://peps.python.org/pep-0420/) | Namespace packages | import | `py33/pep_0420_ns_packages.py` | ☐ |
| [3151](https://peps.python.org/pep-3151/) | `OSError` hierarchy | sem | `py33/pep_3151_oserror.py` | ☐ |

*n/a for codegen: PEP 393 (flexible str storage — internal; pycc uses its own UTF-8 representation, documented in `docs/semantics.md`).*

## Python 3.4

| PEP | Feature | Cat | Test | St |
|---|---|---|---|---|
| [435](https://peps.python.org/pep-0435/) | `enum` | sem | `py34/pep_0435_enum.py` | ☐ |
| [443](https://peps.python.org/pep-0443/) | `functools.singledispatch` | sem | `py34/pep_0443_singledispatch.py` | ☐ |
| [451](https://peps.python.org/pep-0451/) | Module spec import model | import | `py34/pep_0451_modspec.py` | ☐ |
| [3156](https://peps.python.org/pep-3156/) | `asyncio` event loop model | rt | `py34/pep_3156_asyncio.py` | ☐ |

## Python 3.5

| PEP | Feature | Cat | Test | St |
|---|---|---|---|---|
| [448](https://peps.python.org/pep-0448/) | Unpacking generalizations `*`/`**` | syntax | `py35/pep_0448_unpack_gen.py` | ☐ |
| [461](https://peps.python.org/pep-0461/) | `%` formatting for bytes | sem | `py35/pep_0461_bytes_fmt.py` | ☐ |
| [465](https://peps.python.org/pep-0465/) | `@` matrix-multiply operator | syntax | `py35/pep_0465_matmul.py` | ☐ |
| [484](https://peps.python.org/pep-0484/) | **Type hints** — pycc's cornerstone | typing | `py35/pep_0484_type_hints.py` | ☐ |
| [492](https://peps.python.org/pep-0492/) | `async` / `await` syntax | syntax | `py35/pep_0492_async_await.py` | ☐ |

## Python 3.6

| PEP | Feature | Cat | Test | St |
|---|---|---|---|---|
| [487](https://peps.python.org/pep-0487/) | `__init_subclass__`, `__set_name__` | sem | `py36/pep_0487_init_subclass.py` | ☐ |
| [498](https://peps.python.org/pep-0498/) | f-strings | syntax | `py36/pep_0498_fstrings.py` | ☐ |
| [515](https://peps.python.org/pep-0515/) | Underscores in numeric literals | syntax | `py36/pep_0515_underscores.py` | ☐ |
| [525](https://peps.python.org/pep-0525/) | Async generators | syntax | `py36/pep_0525_async_gen.py` | ☐ |
| [526](https://peps.python.org/pep-0526/) | Variable annotations `x: int = 0` | typing | `py36/pep_0526_var_annotations.py` | ☐ |
| [530](https://peps.python.org/pep-0530/) | Async comprehensions | syntax | `py36/pep_0530_async_comp.py` | ☐ |

## Python 3.7

| PEP | Feature | Cat | Test | St |
|---|---|---|---|---|
| [553](https://peps.python.org/pep-0553/) | `breakpoint()` | rt | `py37/pep_0553_breakpoint.py` | ☐ |
| [557](https://peps.python.org/pep-0557/) | **dataclasses** | sem | `py37/pep_0557_dataclasses.py` | ☐ |
| [560](https://peps.python.org/pep-0560/) | `__class_getitem__` typing support | typing | `py37/pep_0560_class_getitem.py` | ☐ |
| [562](https://peps.python.org/pep-0562/) | Module `__getattr__` | sem | `py37/pep_0562_mod_getattr.py` | ☐ |
| [563](https://peps.python.org/pep-0563/) | `from __future__ import annotations` (superseded by 649) | typing | `py37/pep_0563_lazy_annotations.py` | ☐ |
| — | `dict` insertion order guaranteed | sem | `py37/dict_order.py` | ☐ |

## Python 3.8

| PEP | Feature | Cat | Test | St |
|---|---|---|---|---|
| [544](https://peps.python.org/pep-0544/) | `Protocol` — structural typing | typing | `py38/pep_0544_protocol.py` | ☐ |
| [570](https://peps.python.org/pep-0570/) | Positional-only params `/` | syntax | `py38/pep_0570_pos_only.py` | ☐ |
| [572](https://peps.python.org/pep-0572/) | Walrus `:=` | syntax | `py38/pep_0572_walrus.py` | ☐ |
| [586](https://peps.python.org/pep-0586/) | `Literal` | typing | `py38/pep_0586_literal.py` | ☐ |
| [589](https://peps.python.org/pep-0589/) | `TypedDict` | typing | `py38/pep_0589_typeddict.py` | ☐ |
| [591](https://peps.python.org/pep-0591/) | `Final` | typing | `py38/pep_0591_final.py` | ☐ |
| — | f-string `=` debug specifier | syntax | `py38/fstring_eq.py` | ☐ |

## Python 3.9

| PEP | Feature | Cat | Test | St |
|---|---|---|---|---|
| [584](https://peps.python.org/pep-0584/) | `dict \| dict` union | sem | `py39/pep_0584_dict_union.py` | ☐ |
| [585](https://peps.python.org/pep-0585/) | Builtin generics `list[int]` | typing | `py39/pep_0585_builtin_generics.py` | ☐ |
| [593](https://peps.python.org/pep-0593/) | `Annotated` | typing | `py39/pep_0593_annotated.py` | ☐ |
| [614](https://peps.python.org/pep-0614/) | Relaxed decorator grammar | syntax | `py39/pep_0614_decorators.py` | ☐ |
| [617](https://peps.python.org/pep-0617/) | PEG parser — **pycc's grammar reference** | syntax | (covered by whole suite) | ☐ |

## Python 3.10

| PEP | Feature | Cat | Test | St |
|---|---|---|---|---|
| [604](https://peps.python.org/pep-0604/) | Union syntax `int \| str` | typing | `py310/pep_0604_union.py` | ☐ |
| [612](https://peps.python.org/pep-0612/) | `ParamSpec` | typing | `py310/pep_0612_paramspec.py` | ☐ |
| [613](https://peps.python.org/pep-0613/) | `TypeAlias` | typing | `py310/pep_0613_typealias.py` | ☐ |
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
| [673](https://peps.python.org/pep-0673/) | `Self` | typing | `py311/pep_0673_self.py` | ☐ |
| [675](https://peps.python.org/pep-0675/) | `LiteralString` | typing | `py311/pep_0675_literalstring.py` | ☐ |
| [681](https://peps.python.org/pep-0681/) | `dataclass_transform` | typing | `py311/pep_0681_dc_transform.py` | ☐ |

## Python 3.12

| PEP | Feature | Cat | Test | St |
|---|---|---|---|---|
| [688](https://peps.python.org/pep-0688/) | `Buffer` type | typing | `py312/pep_0688_buffer.py` | ☐ |
| [692](https://peps.python.org/pep-0692/) | `Unpack[TypedDict]` for `**kwargs` | typing | `py312/pep_0692_kwargs.py` | ☐ |
| [695](https://peps.python.org/pep-0695/) | **`type` statement + generic syntax** `class C[T]` | typing | `py312/pep_0695_generics.py` | ☐ |
| [698](https://peps.python.org/pep-0698/) | `@override` | typing | `py312/pep_0698_override.py` | ☐ |
| [701](https://peps.python.org/pep-0701/) | Formalized f-string grammar | syntax | `py312/pep_0701_fstring_grammar.py` | ☐ |
| [709](https://peps.python.org/pep-0709/) | Comprehension inlining semantics | sem | `py312/pep_0709_comp_inline.py` | ☐ |

*n/a: PEP 684 (per-interpreter GIL — pycc binaries have no GIL at all).*

## Python 3.13

| PEP | Feature | Cat | Test | St |
|---|---|---|---|---|
| [594](https://peps.python.org/pep-0594/) | "Dead batteries" stdlib removals | rt | `py313/pep_0594_removals.py` | ☐ |
| [696](https://peps.python.org/pep-0696/) | TypeVar defaults | typing | `py313/pep_0696_typevar_defaults.py` | ☐ |
| [702](https://peps.python.org/pep-0702/) | `@deprecated` | typing | `py313/pep_0702_deprecated.py` | ☐ |
| [703](https://peps.python.org/pep-0703/) | Free-threading (experimental) | rt | superseded by 779 test | ☐ |
| [742](https://peps.python.org/pep-0742/) | `TypeIs` — narrowing | typing | `py313/pep_0742_typeis.py` | ☐ |

*n/a: PEP 744 (JIT — pycc is AOT), new REPL.*

## Python 3.14 ← primary target

| PEP | Feature | Cat | Test | St |
|---|---|---|---|---|
| [649](https://peps.python.org/pep-0649/)/[749](https://peps.python.org/pep-0749/) | **Deferred annotations** — pycc reads annotations exactly per 3.14 semantics | typing | `py314/pep_0649_deferred_ann.py` | ☐ |
| [734](https://peps.python.org/pep-0734/) | Subinterpreters in stdlib | rt | `py314/pep_0734_interpreters.py` | ☐ |
| [750](https://peps.python.org/pep-0750/) | **Template strings (t-strings)** | syntax | `py314/pep_0750_tstrings.py` | ☐ |
| [758](https://peps.python.org/pep-0758/) | `except A, B:` without parentheses | syntax | `py314/pep_0758_except_noparens.py` | ☐ |
| [765](https://peps.python.org/pep-0765/) | No `return`/`break`/`continue` in `finally` | sem | `py314/pep_0765_finally.py` | ☐ |
| [779](https://peps.python.org/pep-0779/) | Free-threading officially supported → pycc: GIL-free native threads | rt | `py314/pep_0779_threads.py` | ☐ |

*n/a: PEP 768 (remote debugging interface), 784 (zstd in stdlib — will ship in pycc stdlib subset), 741 (C API config), 761 (release signing), 776 (Emscripten).*

## Rejected by design → negative tests

Each entry must produce the documented error code, tested in `tests/diagnostics/`:

| Construct | Error | Test |
|---|---|---|
| `eval` / `exec` / `compile` | `E0100` | `d0100_eval.py` |
| Monkey-patching (attribute assignment on foreign classes/modules) | `E0101` | `d0101_monkeypatch.py` |
| Untyped public function signature | `T0001` | `d_t0001_untyped_pub.py` |
| Dynamic attribute injection on non-`dict`-like objects | `E0102` | `d0102_dyn_attr.py` |
| `type()` three-arg dynamic class creation | `E0103` | `d0103_dyn_class.py` |
| Wildcard `import *` | `E0104` | `d0104_star_import.py` |

## Real-world corpus (integration tests)

Nightly CI compiles pinned revisions of open-source projects and runs their own test suites against pycc-built artifacts. New incompatibilities auto-file issues in the pycc repo (minimized repro + PEP reference + diagnostic).

| Tier | Projects | Gate |
|---|---|---|
| 1 — small, well-typed | `tomli`, `packaging`, `more-itertools` | must fully pass before v0.2 |
| 2 — medium | `black`, `isort`, `attrs`, `click` | tracked pass-rate |
| 3 — large / typed | `mypy`, `httpx`, `rich` | tracked pass-rate |
| 4 — stretch | `fastapi` + `pydantic` stack | aspirational |

Rules: pinned commits for reproducibility; per-project pass-rate dashboard; regressions block release; genuine upstream type bugs found by pycc are reported upstream manually (curated, never bot-spammed).
