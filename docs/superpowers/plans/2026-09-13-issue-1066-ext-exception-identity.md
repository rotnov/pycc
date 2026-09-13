# Part D of #1038 (#1066): exception identity across the `ext` boundary

> **Provenance.** This plan was drafted under `.claude/skills/issue-to-plan/SKILL.md` and its
> step-7 adversarial loop ended at an **impasse** (five rounds, five concrete edits), which is
> that skill's stop condition: no sixth round, and step 8 publishes nothing. The analysis
> itself is sound and verified, so under `AGENTS.md`'s step-10 planning gate the plan is routed
> to its other documented home — a dated file here, linked from the issue — rather than to an
> `issue-to-plan` comment. The decomposition question was re-examined against the round-5
> finding: the class table is unobservable without the shim-side synthesis that reads it, so
> #1066 remains one plan, not a series.
>
> One open item, carried deliberately: the round-5 edit (the sparse-tag-space defect recorded
> in W2 and W4(f)) was applied to this draft but was not itself reviewed by a further round.
> The implementer verifies it empirically before relying on it.

### 0. Baseline

Planned by `issue-to-plan` against **`2c6d7710`** (`origin/main` tip at planning time).
**Open pull requests: none.** Nothing competes for the decision-log numbering space or for
the files below; re-resolve any derived number at PR-open time anyway.

**Decomposition judgment (step 5): #1066 is one plan, not a series.** Its parts — the class
table, the eager registration, and the shim-side synthesis — are not independent seams: the
accessor or the table alone is unobservable, and no test can assert anything until registration
and synthesis land with it. They must merge together for the tree to stay green, which is the
skill's bar for *not* decomposing.

---

### 1. Corrections to the issue's premises

The issue was written against an older tree. Three of its claims did not survive verification.

**C1 — "only the accessor and the shim-side class synthesis are missing" understates the gap.**
Measured on this host (CPython 3.13.9, a real `pycc build --ext` artifact): a user class that
*subclasses a builtin* flattens too. `class MyValueError(ValueError)` raised out of an export
arrives as plain `Exception`, and a host-side `except ValueError:` does **not** match it — while
inside pycc, `except ValueError:` catches `MyValueError` correctly. Name-only synthesis parented
on `PyExc_Exception` fixes `type(e).__name__` and still leaves that mismatch. The base chain is
knowable at compile time (`HirClassDef::bases` / `::mro`) but is **not** carried at runtime:
`PyExceptionObj` has a flat `u8 type_tag` and no parent pointer. Restoring identity therefore
requires a per-program table, not just an accessor.

**C2 — the proposed `pycc_rt_ext_pending_name()` accessor is not needed, and this plan does not
add it.** Once the artifact carries a tag-keyed table of synthesized classes, the shim resolves
the pending tag directly to a `PyObject *` class; the class *name* is a build-time input to the
generator, never a runtime lookup. Adding the accessor would create a second, redundant path and
a new Rust/C signature pair that nothing binds (see R3). The issue's premise that
`PyExceptionObj` already carries a usable non-null `name`/`name_len` is **correct** — it is just
not the mechanism this plan uses.

**C3 — "synthesize whenever a name is available" would regress the two PEP 654 group classes.**
The shim's `default:` arm today serves *three* distinct cases, not one: tag 0 (`Exception`), tags
23..=24 (`BaseExceptionGroup`/`ExceptionGroup`), and tags >= 26 (user classes). Tags 23..=24 land
there deliberately — the limited C API exposes no `PyExc_ExceptionGroup`, and PEP 654 requires
`(msg, exceptions)`, so a lone message cannot construct one. A synthesized stand-in would be a
fake `ExceptionGroup` that is not CPython's, which is strictly worse than today's honest
`Exception`. So the set of classes that get synthesized must be decided by **table membership**,
never by "a name is available" — and that membership is computed on the Rust side (W1), which is
where `FIRST_USER_EXCEPTION_TYPE_TAG` is derived. The C shim performs no tag arithmetic of its
own; a class absent from the table simply misses the lookup and keeps `PyExc_Exception`.

---

### 2. Recommended shape

**Deliver full identity (name, module attribute, and base chain) in one change.**

Emit a per-program user-exception-class table into the existing generated companion
`pycc_ext_exports.inc`, register the synthesized classes on the module object during
`Py_mod_exec`, and have `pycc_ext_raise_pending` look the pending tag up in that table.

Empirically verified in a throwaway extension module outside the tree, built against this host's
CPython 3.13.9 headers with `-DPy_LIMITED_API=0x030D0000`:

* `PyErr_NewException("mod.MyValueError", PyExc_ValueError, NULL)`, `PyErr_NewExceptionWithDoc`
  and `PyModule_AddObjectRef` all **compile and link under the limited-API floor** the shim
  already pins (`0x030D0000`).
* A **tuple** base works under the same floor: `PyErr_NewException("m.E", PyTuple_Pack(2, MyBase,
  PyExc_ValueError), NULL)` yields
  `E.__mro__ == (E, MyBase, ValueError, Exception, BaseException, object)`, and the raised
  instance is caught by `except ValueError:` *and* by `except m.MyBase:`. This is the mechanism
  W1 needs for multiple exception bases, which pycc does accept — `class E(MyBase, ValueError)`
  compiles and builds an `ext` artifact today (verified with `pycc build --ext` on this tree).
* Raising that synthesized class gives, on the host side: `except ValueError:` matches;
  `except m.MyValueError:` matches; `type(e).__name__ == "MyValueError"`;
  `type(e).__mro__` is `(MyValueError, ValueError, Exception, BaseException, object)`;
  `e.args == ('user defined boom',)`; and the class object is **identical across two raises**.

Three levels were considered:

* **L1, accessor + lazy name-only synthesis (what the issue describes).** Rejected. It fixes only
  `__name__`; `except ValueError:` still fails (C1), the class is never in the module dict so
  `except m.MyError:` cannot be written at all, and a fresh class per raise makes even
  `except type(prev_e):` unmatchable. Its only assertable acceptance test is cosmetic.
* **L2, table + eager registration, all classes parented on `PyExc_Exception`.** Rejected: L3
  differs from it by exactly one column in the same table and one lookup in the same generator.
* **L3, table + eager registration + base chain (recommended).**

The table must be **regenerated with every artifact and never persisted**: user tags are assigned
in `pycc_hir::program::finalize` in deterministic program order, which is stable for a given
source but **shifts on any source edit**. No tag value may be baked into a checked-in file, a
fixture, or the hand-written shim.

Placement follows the existing seam. `collect_exports` (`src/ext_build.rs:445`) already receives
the whole `&HirModule`, and `generate_exports_inc` (`:700`) already writes the generated
companion. It does, however, need **a new parameter**: today's signature is
`pub(crate) fn generate_exports_inc(module_name: &str, exports: &[ExtExport]) -> String`
(`src/ext_build.rs:700`), and neither parameter carries `class_defs` — `ExtExport` holds only a
name, params and a return type. W2 widens it to take the class table (or widens what
`collect_exports` returns). That touches the sole non-test call site, `src/main.rs:410`, and the
**22** test call sites in `src/ext_build_tests/generated_c.rs`. Keep the `src/main.rs` call **on
one line**: the comment at `src/main.rs:405-409` records that a multi-line `foo(\n..\n)?;` puts
the `?` early-return arm on its own never-executed line, which the diff-coverage checker then
reports as uncovered (D-242 rule 1). That is a measurable budget, not a vibe — `src/main.rs:410`
is **82 characters** and there is no `rustfmt.toml`, so `max_width` is **100**: a third argument
has **18 characters** including its `, ` separator before rustfmt wraps the call and reintroduces
the uncovered-line problem. A short binding (`&classes`) fits; if the chosen name does not, take
the alternative and widen what `collect_exports` returns, keeping arity at two. Note that the module reaching `plan_ext`
(`src/main.rs:388`) has been through `monomorphize`, which *extends* `class_defs`
(`crates/pycc_types/src/monomorphize.rs:2882`); a generic class's specialization is built with
`exception_type_tag: None` and `mro: vec![mangled_class]` (`:1862-1864`), so it is deliberately
untagged and never enters the table. The tags the table needs are present because `finalize` ran
before monomorphization, not merely because the whole module is in scope.

---

### 3. Work items, in dependency order

W5 and W6 are this change's documentation and decision-log deliverables; they are not optional
tails. Track each as its own item — a plan clause that directs a document be updated or a
follow-up issue be filed is an item, not prose accompanying the code changes.

**W1 — `src/ext_build.rs`: build the table.** Add a `UserExceptionClass { tag: u8, name: String,
base: ExceptionBase }` collector over `module.class_defs`, selecting exactly those whose
`exception_type_tag` is `Some(t)` with `t >= FIRST_USER_EXCEPTION_TYPE_TAG` (26). The `None`/
`Some(<26)` cases are **not** user classes — `crates/pycc_hir/src/class.rs:345-365` documents that
trap explicitly: the seven flat builtins carry `None` and the OSError family carries a fixed
`Some`.

**Exclude any class whose `mro` reaches `ExceptionGroup` or `BaseExceptionGroup`.**
`finalize` tags every class whose MRO reaches *any* of the 26 builtin names
(`crates/pycc_hir/src/program.rs:179-195`), group classes included — so `class G(ExceptionGroup)`
gets a tag >= 26 and would otherwise be synthesized as a fake group class parented on
`PyExc_Exception`, which is exactly what C3 rejects. An excluded class simply has no table entry
and falls through W3's miss arm to `PyExc_Exception`, preserving today's behaviour.

**Resolve bases from `def.bases`, not from the linearized `mro`.** Multiple exception bases are
legal here: the only multiple-inheritance rejection is the attribute-layout prefix check in
`crates/pycc_hir/src/class/mro.rs:236-253`, and exception class defs are seeded with
`attrs: Vec::new()` (`crates/pycc_hir/src/exception.rs:267`), so `class E(MyBase, ValueError)`
passes it. Natively that class *is* caught by `except ValueError:`, because `handler_type_tags`
matches on `def.mro.iter().any(...)`. Taking the first exception ancestor out of the *linearized*
`mro` would pick `MyBase` alone and drop `ValueError`, reintroducing the precise C1 mismatch this
plan exists to remove. So: filter `def.bases` to its exception-class entries, map each, and when
more than one survives pass a **`PyTuple`** as `PyErr_NewException`'s `base` argument (it accepts
a tuple) — W2 emits the tuple construction.

Filtering `def.bases` to exception classes **drops non-exception bases**, and that is a residual
to record rather than a detail to leave implicit. `class Mixin:` with only methods, then
`class E(Mixin, ValueError):`, passes `pycc check` today (verified, exit 0 — the attribute-layout
prefix check does not reject a method-only mixin). The synthesized host-side class would carry
`ValueError` alone, so `isinstance(e, Mixin)` diverges from native pycc. W5 records it beside the
group-class residual.

Map each individual base:
* a builtin exception base → look its name up with
  `BUILTIN_EXCEPTION_CLASSES.iter().position(|n| *n == base_name)`
  (`crates/pycc_hir/src/exception.rs:7`, `pub`), and use the array entry itself as the `PyExc_*`
  spelling. **Do not read `def.exception_type_tag` to classify a base**: the flat seven builtins
  (`Exception`, `ValueError`, `TypeError`, `KeyError`, `IndexError`, `ZeroDivisionError`,
  `RuntimeError`) carry `None`, since only the 16-member OSError family gets a seeded `Some`
  (`crates/pycc_hir/src/exception.rs:263`). The field cannot classify a base at all.
* another user class (tag >= 26) → that class's own table slot.

There is deliberately **no "group class base" arm**: W1's exclusion above removes every such class
by construction, so an arm for it would be an executable line no test can reach — a zero-hit
changed line, which fails `--require-changed-lines 100`. This matters concretely because
`BUILTIN_EXCEPTION_CLASSES` *does* contain `"BaseExceptionGroup"` and `"ExceptionGroup"`
(`crates/pycc_hir/src/exception.rs:40-41`), so a naive `PyExc_<name>` render would emit the
non-existent `PyExc_ExceptionGroup` and fail to compile. Structure the `position(...)` lookup so a
`None` is impossible by construction rather than handled by a defensive `else`/`unwrap_or` line
that nothing executes. The same trap applies to the base list itself: the exception-filtered
`def.bases` is **provably non-empty** — a class earns a tag only when its MRO reaches a builtin
exception class, which requires at least one exception base — so do not write an empty-slice
fallback arm. It would be a zero-hit changed line and fail the gate.

**Emit the table in ascending tag order.** That is *already* topological and needs no sort:
`finalize` assigns tags by iterating `hir.class_defs` in program order
(`crates/pycc_hir/src/program.rs:186`), and `resolve_mro` requires every base to be present in
`defined_classes` before its subclass is resolved (`crates/pycc_hir/src/class.rs:1088`), so a base
always precedes its subclass. Keep `class Deep(MyValueError)` as the test that *guards* this
property rather than as the driver of an unspecified sort.

**W2 — `src/ext_build.rs`: emit the two generated functions.** `generate_exports_inc` always
emits both, with empty bodies when the table is empty, so an artifact with no user exception
classes still links:
* `static PyObject *pycc_ext_user_exception_class(unsigned char tag)` — returns the cached class
  for a tag, or `NULL`.
* `static int pycc_ext_register_exception_classes(PyObject *module)` — calls
  `PyErr_NewException("<module_name>.<ClassName>", <base>, NULL)` per entry, stores it in a
  file-scope static cache, and registers it with `PyModule_AddObjectRef`. **The cache owns the
  strong reference** returned by `PyErr_NewException`; `PyModule_AddObjectRef` takes its own
  reference rather than stealing, so the module attribute and the cache each hold one and neither
  needs a compensating decref. If a cache slot is already non-`NULL`, **reuse it** instead of
  minting a new class: `docs/RUNTIME.md:504-512` records that deleting the `sys.modules` entry and
  re-importing is the one path that re-runs `Py_mod_exec`, and reuse both avoids leaking the first
  instance's class objects and keeps class identity stable across that divergence. Assign the
  cache slot **before** checking `PyModule_AddObjectRef`'s result, so a failing `AddObjectRef`
  does not leak the strong reference the cache is meant to own. On a mid-registration failure,
  leave the already-created classes cached (no decref, no `NULL`-ing) so a later import completes
  the remaining slots. Qualify the name with
  the module name (`generate_exports_inc` already takes `module_name`) so `__module__` is right;
  the prototype confirms this yields `<class 'mod.MyValueError'>`. Prefer emitting
  `PYCC_EXT_MODULE_NAME_STR ".MyError"` — C string-literal concatenation against the macro the
  `.inc` already defines — over re-interpolating the module name into every call, so the
  qualified name has one source of truth.

**Pinning the generated text is W2's first deliverable, and must be settled before any W4
assertion is written** — every W4 case asserts against `.inc` text, so none of them is writable
until the rendering exists. Fix verbatim: the cache's shape, the exact one-entry rendering of the
`PyErr_NewException` + `PyModule_AddObjectRef` pair, and the lookup's dispatch form.

**The tag space is sparse relative to the table, and the sizing rule must be stated explicitly.**
W1 excludes group-derived classes, but `finalize` still *consumes* tags for them: in the very
program W4(f) prescribes, `class G(ExceptionGroup)` takes tag 26 and `class E(ValueError)` takes
27, while the table holds one entry. A `static PyObject *cache[N];` indexed by
`tag - FIRST_USER_EXCEPTION_TYPE_TAG` with `N` = *entry count* would send `E` to index 1, fail the
bounds test, return `NULL`, and silently flatten `E` to `PyExc_Exception` — reintroducing exactly
the C1 mismatch this plan exists to remove, with no compile or link error. Either size the array
by **tag span** (`max_tag - FIRST_USER_EXCEPTION_TYPE_TAG + 1`, excluded tags left as `NULL`
holes) or drop the offset-index form and key entries by explicit tag. State which.

**W3 — `src/ext/pycc_ext_module.c`: three edits.**
* Add a forward declaration `static PyObject *pycc_ext_user_exception_class(unsigned char tag);`
  near the existing extern block. This is required: the `.inc` is included at line **623**, but
  `pycc_ext_raise_pending` is at line **98**. `pycc_ext_exec_module` is at line **637**, *after*
  the include, so the registration call needs no forward declaration.
* In `pycc_ext_raise_pending`'s `default:` arm, before falling back to `PyExc_Exception`,
  call `pycc_ext_user_exception_class(tag)` **unconditionally**; on a non-`NULL` result use it as
  `exc_type`, on `NULL` keep `PyExc_Exception`. Deliberately **no `tag >= 26` test in the C
  shim**: membership is already decided in W1 (only tags >=
  `FIRST_USER_EXCEPTION_TYPE_TAG`, group-derived classes excluded), so tags 0 and 23..=24 provably
  cannot be in the table and a miss returns `NULL` anyway. A literal `26` in the hand-written shim
  would be a new magic constant that **neither drift guard covers** — `ext_bridge.rs:183` pins
  only 0..=6 and 25, and `toolchain.rs:348`'s parser only pairs a decimal `case N:` with a
  `PyExc_` assignment — while `FIRST_USER_EXCEPTION_TYPE_TAG` is derived from
  `BUILTIN_EXCEPTION_CLASSES.len()` and #1063 has already moved it once (25→26). Keep the
  boundary on the Rust side, where it is derived.
  The lookup returns a **borrowed** pointer (the cache holds the strong reference), matching how
  every existing `exc_type` in that function is an immortal borrowed `PyExc_*` handed to
  `PyErr_SetObject` with no decref — so no refcount handling changes in this function.
  Rewrite the `default:` comment — its current text ("carrying its identity across the boundary
  needs the class *name*, which the bridge does not expose yet") becomes false with this change
  and **nothing tests it**. Keep the tags-23..=24 half of that comment intact and make the split
  explicit.
* In `pycc_ext_exec_module`, call `pycc_ext_register_exception_classes(module)` before
  `pycc_ext_module_exec()` and fail module init if it returns non-zero. This is the only place
  the module object is reachable (`m_size = 0`, so there is no module state). The file-scope
  static cache is licensed by the shim's existing `Py_MOD_MULTIPLE_INTERPRETERS_NOT_SUPPORTED`
  slot and its free-threading refusal — cite both in the comment rather than leaving it implicit.

**W4 — tests.**
* Non-ignored, in `src/ext_build_tests/generated_c.rs`: assert the generated `.inc` text for
  (a) a module with no user exception classes — asserting **both function definitions are still
  emitted, with empty bodies**, so the artifact links; a program whose only exception classes are
  builtins is the same case and the same assertion, (b) one class subclassing `Exception`,
  (c) one subclassing `ValueError`, asserting the `PyExc_ValueError` base,
  (d) `Deep(MyValueError)`, asserting the user-tag base and that `MyValueError`'s entry precedes
  `Deep`'s (see the caveat below on what this can and cannot prove),
  (e) `class E(MyBase, ValueError)` — asserting a `PyTuple` base carrying both, and
  (f) `class G(ExceptionGroup)` — asserted in **one program alongside `class E(ValueError)`**,
  so the test shows `E` present *and* `G` absent in the same `.inc`. This case must **also assert
  the cache declaration itself** — its size, and that `E`'s emitted entry sits at the index its
  own tag implies — because the `PyErr_NewException` line for `E` renders identically whether the
  array is sized 1 or 2, so a sparsity bug would otherwise escape every non-ignored test and
  surface only in the `#[ignore]`d end-to-end file the coverage job never runs. An absence-only assertion
  passes just as well when table emission is missing or broken outright, so pair every absence
  with a presence.
  Caveat on (d): `src/ext_build_tests/mod.rs:45`'s `module()` helper builds an `HirModule` with
  `class_defs: Vec::new()`, so a hand-built `HirClassDef` carries whatever tag and base order the
  test author writes — such a test cannot *prove* the ascending-order property, which lives in
  `finalize` plus `resolve_mro`. Either lower real source inside the non-ignored test (the bin
  crate already does this: `resolve_frontend` at `src/main.rs:1009`, used by the non-ignored
  `plan_ext` tests at `src/main.rs:1088`/`:1128`/`:1152`), or state plainly that the `#[ignore]`d
  end-to-end test is the only real guard for it. Do not leave a vacuous assertion standing as if
  it were the guard.
* Non-ignored, in `src/ext_build_tests/generated_c.rs`'s existing shim-text test at `:352`:
  assert the shim contains the forward declaration and the `pycc_ext_register_exception_classes`
  call. This is the cheapest guard available for R3.
* End-to-end, `#[ignore]`d, in a new `tests/issue_1066_ext_user_exceptions.rs` following
  `tests/issue_1063_overflow_error.rs:239` as the closest template: build a real artifact and
  assert from Python that `except m.MyError:`, `except ValueError:` (for a `ValueError`
  subclass), `type(e).__name__`, `__module__`, and `e.args` all behave, and that tags 23..=24
  still arrive as `Exception`, and that `class G(ExceptionGroup)` likewise still arrives as
  `Exception` rather than a synthesized fake group class (the C3 regression guards).

**W4b — fold in a one-line doc correction.** `crates/pycc_hir/src/class.rs:354-355` says the tag
"is assigned by `module::lower_all` (in its post-loop phase…)", which is stale: it is assigned by
`program::finalize` (`crates/pycc_hir/src/program.rs:169`, reached via `module.rs:126`). An
implementer following that comment looks in the wrong function. Correct it in this change.

**W5 — documentation.** Rewrite `docs/RUNTIME.md`'s sentence at **:468-473**. It currently
bundles the group classes with user classes in one clause ("a user-defined exception class and
the two PEP 654 group classes reach the caller as `Exception`"); that bundling must **split**,
since user classes no longer do and the group classes still do. Record the group-class residual
in the same register as the existing `int`/`float`/`str`/`tuple` subclass-identity narrowings at
`:480-507` — that paragraph is this repository's documentation-convention precedent for an
identity narrowing, and the convention, not a grep for existing mentions, is what decides the
documentation obligation here.
That paragraph's tail (`:504-512`) also states the `ext` module-state contract the W2 cache sits
inside — `m_size = 0`, process-static state, subinterpreters refused, and the
delete-from-`sys.modules`-and-reimport divergence. Say there that the synthesized classes are
registered as module attributes and that the cache reuses existing entries on a re-exec. Record
two residuals there: a user class deriving from a PEP 654 group class still arrives as
`Exception`, and a non-exception base (a method-only mixin) is dropped from the synthesized
class, so `isinstance(e, Mixin)` does not hold host-side.

**W6 — decision log.** This changes D-244's artifact surface (a module now exports synthesized
exception class attributes), so it gets a **single inserted amendment line** on
`docs/decisions/D-244-*.md`, of the form `- Amendment (YYYY-MM-DD): ...`. That file is `accepted`
and therefore **insert-only** under D-240; it already carries eight amendments. Do **not** open a
new decision record — registering classes on an existing artifact is not a new irreversible or
project-wide choice under D-242 rule 3. Follow the file's own convention: D-244's 2026-09-13
#1050 amendment records the rule-3 artifact addition *and* its identity narrowing in one line, so
this amendment states both the new module attributes **and** the three residuals W5 documents
(group-derived classes still flatten, a non-exception mixin base is dropped so
`isinstance(e, Mixin)` fails host-side, and instance state beyond `args` does not carry). Insert
it after the last existing `- Amendment` line (currently line 17) and before `- Context:`
(line 18), so D-240's verbatim-and-in-order check is trivially satisfied. Verify locally with
`python3 -B scripts/check_decision_immutability.py --base "$(git merge-base origin/main HEAD)" --head HEAD`.

---

### 4. Affected-site inventory

Every site that dispatches on "which exception class is this" — branch points, not mentions.

| Site | Needs a branch? |
|---|---|
| `src/ext/pycc_ext_module.c:98` `pycc_ext_raise_pending`, `default:` arm | **Yes** — W3. No tag arithmetic on the C side: membership is decided in W1, and a lookup miss leaves tags 0 and 23..=24 on `PyExc_Exception` (C3). |
| `src/ext/pycc_ext_module.c:637` `pycc_ext_exec_module` | **Yes** — W3, the registration call. |
| `src/ext_build.rs:445` `collect_exports` | **No** — it gates boundary *types* (`C0003`); an exception class is not a boundary type. Its `&HirModule` is reused by W1. |
| `src/ext_build.rs:700` `generate_exports_inc` | **Yes** — W2. |
| `src/ext_build.rs:904` wrapper epilogue | **No.** Its *position* is load-bearing (#1050: packing before the check reads uninitialized out-pointer storage) and is asserted by `generated_c.rs:126,259,584`. Do not move it. |
| `crates/pycc_rt/src/exception.rs:416` `pycc_rt_exception_type_matches` | **No.** Native-side matching is exact-tag-plus-catch-all by design (D-173); the hierarchy is expanded at compile time. |
| `crates/pycc_mir/src/exception.rs:183` `handler_type_tags` | **No.** Native matching is unchanged. It is `pub(super)` to `pycc_mir` and maps a name to its *subclass* tags — the inverse of what W1 needs — so `src/ext_build.rs` neither can nor should call it. W1 reads `HirClassDef::bases`/`mro` directly. |
| `crates/pycc_rt/src/ext_bridge.rs:183` drift guard | **No.** It pins tag *numbering* (0..=6, 25) only; nothing is renumbered. |
| `src/ext_build_tests/toolchain.rs:348` drift guard | **No, but verify.** Its parser pairs a decimal `case N:` with an `exc_type = PyExc_<X>;` line and asserts the seen set is exactly `1..=22` plus `25`. W3 adds no decimal `case`, so the set is unchanged. Note the parser's shape: an `exc_type = PyExc_...` assignment with no preceding `case` is silently skipped, so the new `default:`-arm code passes without touching it — confirm after the edit rather than assuming. |
| `src/main.rs:410` `generate_exports_inc` call | **Yes** — the widened signature's only non-test call site. Inside the diff-coverage denominator; keep it one line. |
| `src/ext_build_tests/generated_c.rs` (22 call sites) | **Yes**, mechanically — every existing `generate_exports_inc` call updates to the new signature. |
| `docs/RUNTIME.md:468-473` | **Yes** — W5, and the clause must split. |
| `crates/pycc_hir/src/program.rs:169` `finalize` tag assignment | **No.** Allocation is unchanged; W1 only reads the result. |

---

### 5. Gates, and how to check each locally

* **Diff coverage, `--require-changed-lines 100`** — the real constraint. The C shim is outside
  the lcov denominator entirely, so W3 is free. `scripts/check_diff_coverage.py` excludes
  `tests/*.rs` but **not** `src/*_tests/`, and the coverage job's `llvm-cov` runs **without**
  `--include-ignored` — so an `#[ignore]`d test earns no coverage while its lines stay in the
  denominator. Keep every new **non-test** Rust line (all of it in `src/ext_build.rs`) reachable
  from the non-ignored tests in W4; put only assertions in the `#[ignore]`d end-to-end file.
* `cargo test --workspace --no-fail-fast` — plain `cargo test --workspace` fails fast, and
  `-- --include-ignored` exits 101 on this host from a CPython 3.14.6-vs-3.14.7 oracle pin
  unrelated to this change.
* `python3 -B scripts/check_decision_immutability.py --base "$(git merge-base origin/main HEAD)" --head HEAD`
* `LANG=en_US.UTF-8 ruby scripts/check_roadmap_evidence.rb` — the `LANG` prefix is required; it
  exits 1 environmentally with `invalid byte sequence in US-ASCII` without it.
* `cargo doc --workspace --no-deps` if any public Rust API doc changes.
* **`docs/ROADMAP.md`: leave it alone if at all possible.** It is 173436 bytes against a
  173568-byte per-resource llms.txt budget (`site/llms-txt-context-manifest.json`, label
  "Roadmap") — about **132 bytes of headroom**. If an edit is unavoidable it must pay for itself
  in the same paragraph and be followed by `sh scripts/check-site.sh`.
* Capture a gate's own exit status (`cmd > log 2>&1; echo $?`), never a pipeline's. Run heavy
  test loops under an isolated scratch dir per `docs/TESTING.md`.

---

### 6. Risks

* **R1 — a synthesized class shadows a builtin name: impossible, no test needed.** The all-or-
  nothing shadow gate (`crates/pycc_hir/src/module.rs:201-204`) withholds seeding when a user
  class takes a builtin exception name, and `finalize` is then skipped entirely
  (`crates/pycc_hir/src/program.rs:169-171`). Verified: `class ValueError(Exception): pass` is
  rejected up front with ``error[C0001]: class `ValueError` inherits from unknown class
  `Exception` ``. No raisable builtin-named user class can reach the table.
* **R2 — `MAX_USER_EXCEPTION_CLASSES` is 230** (tags 26..=255), already diagnosed as `C0001` in
  `finalize`. The table inherits that bound; no new limit is introduced.
* **R3 — the shim's hand-written declarations are bound to nothing.** Nothing checks the shim's
  `extern` block against `pycc_rt`'s real Rust signatures; a mismatch fails only at **link**
  time, i.e. only inside `#[ignore]`d tests that the coverage job never runs. This plan avoids
  adding a new Rust↔C signature pair at all (C2), which **narrows** the exposure — it does not
  remove it: the forward declaration is still hand-written in the shim while the definition is
  emitted by `src/ext_build.rs`, so a spelling or parameter-type mismatch fails at C compile time
  in exactly the same blind spot. Mitigate concretely: have W4 assert
  the **same literal substring** —
  `static PyObject *pycc_ext_user_exception_class(unsigned char tag)` — in both the `SHIM_C` test
  and the generated-`.inc` test, or derive both from one shared `const`.
* **R4 — registration failure during `Py_mod_exec`.** Must propagate: return -1 and let module
  import fail loudly rather than importing a module whose `except m.MyError:` silently never
  matches. Both failure paths count — `PyErr_NewException` returning `NULL` *and*
  `PyModule_AddObjectRef` returning negative. The partial-state rule is in W2.
* **R5 — tag instability across source edits.** Covered by the "regenerate, never persist" rule
  in §2; the risk is a future change baking a tag into a fixture. The W4 tests assert on
  generated *text* shape, not on specific tag values, which keeps them honest.

---

### 7. Out of scope

* Tags 23..=24, the PEP 654 group classes — **and any user class deriving from them**, which W1
  excludes from the table for the same reason. They remain `Exception`, for C3's reason, and W5
  records that as a deliberate residual. A separate issue should track carrying
  `BaseExceptionGroup`/`ExceptionGroup` properly (it needs the `exceptions` array across the
  boundary, not just a class identity) — file it in `product-sprint-1` per D-192's
  milestone-at-filing rule.
* Non-exception bases (mixins) on an exception class — dropped from the synthesized host-side
  class, recorded as a residual in W5 rather than modelled.
* Exception *instance* attributes beyond `args` — a user class with `__init__` state does not
  carry that state across; only the class identity and the message do.
* `raise ... from ...` cause/context chaining across the boundary.
* Parts C (#1065) and any remaining #1038 parts.

---

### 8. What could not be verified in this environment

* Behaviour on **Windows and Linux CI legs**. Everything above was measured on macOS against
  CPython **3.13.9**. `PyErr_NewException` / `PyModule_AddObjectRef` are limited-API-stable, so
  the conclusion is portable by contract, but it is *observed* only on this host.
* The full `--include-ignored` suite, which exits 101 here for an unrelated reason (see §5).
