//! D-244's hosted `ext` artifact mode, as codegen sees it.
//!
//! Everything here answers one question -- *who calls the synthetic
//! module-body entry point, and what does it return when the body raises* --
//! and nothing here touches LLVM. `native` mode's answer is "the process
//! loader, and `main` never comes back from an uncaught exception"; `ext`'s
//! is "the C shim's `Py_mod_exec` slot, and it returns `-1` with the pending
//! exception handed to CPython". The option struct that selects between them
//! and the two symbol-name predicates that implement the distinction live
//! together so the naming invariant cannot drift across a 9000-line file
//! (AGENTS.md's "Keep source files decomposable"; the tracker for the rest
//! of `lib.rs` is #545).
//!
//! #1050 added a second, closely related question -- *what shape does a
//! public export present to the generated C wrapper* -- and the answer is
//! the `pycc_ext_thunk_<name>` convention below. It lives here rather than
//! in `ext_thunk.rs` because the driver (`src/ext_build.rs`) renders C
//! against exactly this convention and must agree with it symbol for symbol
//! and width for width, while `ext_thunk.rs` is the LLVM emission that
//! implements it.

use pycc_mir::Ty;

/// Everything `compile_to_object` needs beyond the MIR and the output path.
///
/// Additive by construction: `compile_to_object` has hundreds of call sites
/// across this crate's own tests and the workspace's integration targets, so
/// `ext` (D-244's hosted CPython extension-module mode) arrives as a field on
/// a `Default`-constructible struct behind a second entry point rather than
/// as a new positional parameter on the existing one. `..Default::default()`
/// then keeps a later mode from churning those call sites either.
#[derive(Debug, Clone, Default)]
pub struct CompileOptions {
    /// `None` builds for the host's own default target; `Some(triple)`
    /// cross-compiles. Owned rather than borrowed so the struct can be
    /// stored and passed without threading a lifetime through every caller.
    pub target_triple: Option<String>,
    /// `true` runs LLVM's `"default<O3>"` pipeline (D-094).
    pub release: bool,
    /// `true` emits an object destined for a CPython extension-module
    /// artifact rather than a native executable (D-244). Two things change,
    /// both of them about *who calls the module body and what happens when
    /// it raises*: the synthetic module-body entry point is named
    /// [`EXT_MODULE_EXEC_SYMBOL`] instead of `main`, so the artifact exports
    /// no stray `main` and the fixed C shim's `Py_mod_exec` slot has a
    /// symbol to call; and an uncaught module-scope exception returns `-1`
    /// after handing the pending state to the host (see
    /// `pycc_rt_ext_pending_type`) instead of calling
    /// `pycc_rt_exception_print_and_exit`, which would terminate the
    /// interpreter process instead of failing the import.
    pub ext: bool,
}

/// The symbol the module body is emitted under in `ext` mode: the fixed C
/// shim's `Py_mod_exec` slot calls exactly this name, and it returns `0` on
/// success or `-1` with the pending exception already handed to CPython.
///
/// Deliberately *not* `main`: a CPython extension module that exports `main`
/// would collide with the host interpreter's own entry point.
pub const EXT_MODULE_EXEC_SYMBOL: &str = "pycc_ext_module_exec";

/// What [`EXT_MODULE_EXEC_SYMBOL`] returns when the module body raised: the
/// `Py_mod_exec` slot's own failure convention (`-1` with the exception
/// already set), which is *not* the per-export wrapper's (`NULL`).
pub const EXT_MODULE_EXEC_FAILED: i64 = -1;

/// The name the synthetic module-body entry point carries in each mode.
/// One function so the `add_function` call and the `MirStmt::Return`
/// invariant that pins the name can never drift apart.
pub(crate) fn entry_fn_name(ext: bool) -> &'static str {
    if ext { EXT_MODULE_EXEC_SYMBOL } else { "main" }
}

/// Whether `name` is the symbol the synthetic module-body entry point was
/// emitted under, in *either* mode. The `MirStmt::Return` invariant below
/// asks this rather than comparing against `main` directly: `ext` builds
/// rename that entry point (see [`CompileOptions::ext`]), and an invariant
/// that still only recognized `main` would stop firing there -- silently, and
/// exactly in the mode where a module-level `return` reaching codegen would
/// corrupt the `Py_mod_exec` slot's own return value.
pub(crate) fn is_module_entry_symbol(name: &[u8]) -> bool {
    name == b"main" || name == EXT_MODULE_EXEC_SYMBOL.as_bytes()
}

/// The prefix every scalar-only `ext` export thunk's symbol carries.
///
/// Kept beside [`EXT_MODULE_EXEC_SYMBOL`] for the same reason: the driver's
/// generated C declares this symbol by name, and a prefix spelled twice is a
/// link-time-deferred crash rather than a compile error (the `--ext` link
/// passes `-undefined dynamic_lookup` on Mach-O and `-Bsymbolic` on ELF, so
/// an undefined thunk resolves to nothing and dies at call time).
pub const EXT_THUNK_PREFIX: &str = "pycc_ext_thunk_";

/// The fixed C shim's module-import helper (Part 1 of #1026): it takes a
/// NUL-terminated module name and returns a *new* reference to the imported
/// module, or `NULL` with the CPython exception already set.
///
/// Kept here for exactly the reason [`EXT_THUNK_PREFIX`] is: the symbol is
/// defined in `src/ext/pycc_ext_module.c` and declared by LLVM in
/// `foreign_import.rs`, and the `--ext` link resolves an undefined symbol
/// lazily (`-undefined dynamic_lookup` on Mach-O, `-Bsymbolic` on ELF), so
/// a misspelling on either side is a crash at first call rather than a link
/// error. One constant, referenced by both sides, makes that impossible.
pub const EXT_OBJ_IMPORT_SYMBOL: &str = "pycc_ext_obj_import";

/// The fixed C shim's attribute-load helper (Part 2 of #1026): it takes a
/// borrowed `PyObject *` and a NUL-terminated attribute name, and returns a
/// *new* reference to the attribute's value, or `NULL` with the CPython
/// exception already set.
///
/// Spelled once here for exactly the reason [`EXT_OBJ_IMPORT_SYMBOL`]
/// directly above is: the symbol is defined in `src/ext/pycc_ext_module.c`
/// and declared by LLVM in `lib.rs`'s `MirExpr::ObjAttrGet` arm, and the
/// `--ext` link resolves an undefined symbol lazily, so a misspelling on
/// either side is a crash at first call rather than a link error.
pub const EXT_OBJ_GETATTR_SYMBOL: &str = "pycc_ext_obj_getattr";

/// The fixed C shim's method-call helper (Part 2 of #1026, PR 2b of #1081):
/// it takes a borrowed `PyObject *`, a NUL-terminated method name, an array
/// of `nargs` *owned* argument references, and returns a *new* reference to
/// the call's result, or `NULL` with the CPython exception already set. It
/// consumes every argument reference on every path.
///
/// Fused rather than "getattr then call" so the bound method object never
/// becomes a pycc value and the whole operation has one failure edge; the C
/// side's own comment carries the full rationale. Spelled once here for
/// exactly the reason [`EXT_OBJ_IMPORT_SYMBOL`] is.
pub const EXT_OBJ_CALL_SYMBOL: &str = "pycc_ext_obj_call";

/// The shim's `int` argument packer: a D-141 encoded int word in, a new
/// `PyObject *` reference out, or `NULL` with an `OverflowError` set for a
/// bigint (#1040). Borrows its argument -- see the C side's own comment.
pub const EXT_OBJ_PACK_INT_SYMBOL: &str = "pycc_ext_obj_pack_int";

/// The shim's `float` argument packer (`PyFloat_FromDouble`).
pub const EXT_OBJ_PACK_FLOAT_SYMBOL: &str = "pycc_ext_obj_pack_float";

/// The shim's `bool` argument packer (`PyBool_FromLong`, so the result is
/// one of the two interned singletons).
pub const EXT_OBJ_PACK_BOOL_SYMBOL: &str = "pycc_ext_obj_pack_bool";

/// The shim's `str` argument packer: a borrowed `PyStrObj` in, an
/// independent CPython `str` out.
pub const EXT_OBJ_PACK_STR_SYMBOL: &str = "pycc_ext_obj_pack_str";

/// The fixed C shim's `len` helper (Part 3 of #1026): it takes a borrowed
/// `PyObject *` and an out-pointer, writes the D-141 encoded `int` word for
/// `PyObject_Size(o)` through it and returns `0`, or returns `-1` with a
/// CPython exception already set. It does not touch the operand's refcount.
///
/// The `PyObject_Size` call and the `pycc_rt_ext_int_encode` call are fused
/// inside the shim deliberately: either can fail, and folding both into one
/// `-1` return lets codegen emit exactly *one* module-exec failure edge for
/// `len` instead of two. (The encode failure is unreachable for a real
/// container -- a length never leaves D-141's inline range -- so the second
/// branch exists only as defence in depth.) Spelled once here for exactly
/// the reason [`EXT_OBJ_IMPORT_SYMBOL`] is: the `--ext` link resolves an
/// undefined symbol lazily, so a misspelling is a crash at first call rather
/// than a link error.
pub const EXT_OBJ_LEN_SYMBOL: &str = "pycc_ext_obj_len";

/// The fixed C shim's truth-testing helper (Part 3 of #1026): it takes a
/// borrowed `PyObject *` and returns `1` for a truthy operand, `0` for a
/// falsy one, or `-1` with a CPython exception already set (`PyObject_IsTrue`
/// can run arbitrary `__bool__`/`__len__` code, so it really can raise). It
/// does not touch the operand's refcount.
///
/// Spelled once here for the same lazy-link reason as [`EXT_OBJ_LEN_SYMBOL`].
pub const EXT_OBJ_TRUTHY_SYMBOL: &str = "pycc_ext_obj_truthy";

/// The fixed C shim's subscript-load helper (Part 3 of #1026, PR 3b of
/// #1082): it takes a borrowed `PyObject *` and an *owned* key reference
/// produced by one of the `pycc_ext_obj_pack_*` helpers above, and returns a
/// *new* reference to `o[k]`, or `NULL` with the CPython exception already
/// set.
///
/// **It consumes the key on every path**, including the one where `o` or
/// `k` is itself `NULL`. That is deliberately the same arg-slot contract
/// [`EXT_OBJ_CALL_SYMBOL`] already imposes on the values the packers
/// produce, so the packer contract stays one rule rather than two: whatever
/// a packer creates is handed to a shim helper and is that helper's to
/// release. Folding the failed-packer test into the helper as well is what
/// leaves this operation with exactly *one* module-exec failure edge, for
/// the reason [`EXT_OBJ_LEN_SYMBOL`] records for its own fused encode.
///
/// Spelled once here for the same lazy-link reason as [`EXT_OBJ_LEN_SYMBOL`].
pub const EXT_OBJ_GETITEM_SYMBOL: &str = "pycc_ext_obj_getitem";

/// The fixed C shim's iterator-acquisition helper (Part 3 of #1026, PR 3c
/// of #1082): it takes a borrowed `PyObject *` and returns a *new*
/// reference to `iter(o)` -- `PyObject_GetIter` -- or `NULL` with the
/// CPython exception already set (a non-iterable operand raises
/// `TypeError` there, which is exactly the behaviour pycc wants to
/// surface).
///
/// The iterator is read once, in the loop preheader, and is never
/// released: one leaked reference per `for` statement, on the same
/// leak-only rule the rest of this boundary follows (#1092).
///
/// Spelled once here for the same lazy-link reason as [`EXT_OBJ_LEN_SYMBOL`].
pub const EXT_OBJ_GET_ITER_SYMBOL: &str = "pycc_ext_obj_get_iter";

/// The fixed C shim's iterator-advance helper (Part 3 of #1026, PR 3c of
/// #1082). Unlike every other helper here it is *three-valued*: given a
/// borrowed iterator and an out-parameter, it returns `1` having written a
/// *new* reference to the next item through `*out`, `0` on clean
/// exhaustion, or `-1` with a CPython exception already set.
///
/// **The discrimination lives in C, not in LLVM IR.** `PyIter_Next`
/// signals both exhaustion and failure with `NULL`, and only
/// `PyErr_Occurred()` tells the two apart; open-coding that in emitted IR
/// would put a second, independently-maintained copy of a CPython calling
/// convention into this crate. Collapsing the two into one value here is
/// what keeps exhaustion off the module-exec failure edge: a `for` loop
/// that simply ends is not a failure.
///
/// Each item written through `*out` is a new reference that is never
/// released, which is what makes the boundary's leak **trip-count-linear**
/// for a `for` loop (#1092, `docs/RUNTIME.md`).
///
/// Spelled once here for the same lazy-link reason as [`EXT_OBJ_LEN_SYMBOL`].
pub const EXT_OBJ_ITER_NEXT_SYMBOL: &str = "pycc_ext_obj_iter_next";

/// The fixed C shim's `float(o)` conversion helper (Part 4 of #1026, PR 4a
/// of #1083): it takes a borrowed `PyObject *` and a `double *`
/// out-parameter, writes the converted value and returns `0`, or returns
/// `-1` with the CPython exception already set.
///
/// **This is an explicit conversion, not an implicit boundary crossing.**
/// D-244 rule 7 keeps the type boundary closed at the *thunk export seam*,
/// where a value crosses implicitly and the annotation is the whole
/// contract. `float(o)` in user source names its destination type, so
/// running CPython's own `PyNumber_Float` protocol -- the operand's
/// `__float__`, `__index__` or string parse -- is precisely what the author
/// asked for. `docs/TYPE_SYSTEM.md`'s `object` row and the helper's own C
/// comment record the same distinction.
///
/// **Ownership.** The helper releases the temporary `PyNumber_Float`
/// produces on *every* exit, including the failing one, and nothing but a
/// `double` escapes into compiled code -- so unlike an attribute load or a
/// subscript, this operation adds nothing to the #1092 leak-only set.
///
/// Spelled once here for the same lazy-link reason as [`EXT_OBJ_LEN_SYMBOL`].
pub const EXT_OBJ_TO_FLOAT_SYMBOL: &str = "pycc_ext_obj_to_float";

/// The fixed C shim's `int(o)` conversion helper (Part 4 of #1026, PR 4b
/// of #1083): it takes a borrowed `PyObject *` and a `long long *`
/// out-parameter, writes a **D-141 encoded** integer word and returns `0`,
/// or returns `-1` with the CPython exception already set.
///
/// **This is an explicit conversion, not an implicit boundary crossing.**
/// The distinction [`EXT_OBJ_TO_FLOAT_SYMBOL`] records applies unchanged:
/// D-244 rule 7 closes the type boundary at the *thunk export seam*, and
/// `int(o)` in user source names its destination type, so running CPython's
/// own `PyNumber_Long` protocol is what the author asked for. It is
/// therefore *not* `pycc_ext_unpack_int_at`, whose `PyBool_Check` and
/// `PyLong_Check` guards exist precisely because that seam is closed.
///
/// **Overflow.** The encode is fused into the helper, exactly as
/// [`EXT_OBJ_LEN_SYMBOL`]'s is and for its reason (one failure edge rather
/// than two). A value outside pycc's inline-integer range
/// `[-2**62, 2**62-1]` raises `OverflowError` citing #1040 -- there is no
/// bigint path across this boundary.
///
/// **Ownership.** The helper releases the `PyNumber_Long` temporary on
/// *every* exit, including the `OverflowError` path that still holds it, so
/// this operation adds nothing to the #1092 leak-only set.
///
/// Spelled once here for the same lazy-link reason as [`EXT_OBJ_LEN_SYMBOL`].
pub const EXT_OBJ_TO_INT_SYMBOL: &str = "pycc_ext_obj_to_int";

/// The fixed C shim's `str(o)` conversion helper (Part 4 of #1026, PR 4b of
/// #1083): it takes a borrowed `PyObject *` and a `void **` out-parameter,
/// writes a pycc `PyStrObj *` at refcount 1 and returns `0`, or returns `-1`
/// with the CPython exception already set.
///
/// **This is an explicit conversion, not an implicit boundary crossing.**
/// See [`EXT_OBJ_TO_FLOAT_SYMBOL`]; `PyObject_Str` *is* `str()`, so no other
/// answer is defensible. Unlike `pycc_ext_unpack_str` it does not
/// `PyUnicode_Check` its operand -- refusing a non-`str` is exactly what an
/// explicit conversion must not do.
///
/// **Ownership.** The handle written through the out-parameter is produced
/// by `pycc_rt_str_from_literal`, the same call a `str` literal's own
/// emission uses, and arrives as compiled code's own reference -- so this
/// crate needs no new rule for it. On the C side the copy must complete
/// *before* the `PyObject_Str` result is released, because
/// `PyUnicode_AsUTF8AndSize` points into that result's buffer; the helper's
/// own comment records why that ordering is load-bearing. The temporary is
/// released on every exit, so nothing joins the #1092 leak-only set.
///
/// Spelled once here for the same lazy-link reason as [`EXT_OBJ_LEN_SYMBOL`].
pub const EXT_OBJ_TO_STR_SYMBOL: &str = "pycc_ext_obj_to_str";

/// The fixed C shim's fixed-arity all-`float` tuple unpack helper (Part 4
/// of #1026, PR 4c of #1083): it takes a borrowed `PyObject *`, the
/// declared arity and a `double *` out-array, writes that many converted
/// doubles and returns `0`, or returns `-1` with the CPython exception
/// already set.
///
/// **Strict container, converting elements.** The helper checks
/// `PyTuple_Check` with an *exact*-arity test and then converts each item
/// with `PyNumber_Float`. The two halves answer two different questions:
/// D-115/D-116 hold a tuple as a by-value LLVM struct of fixed width, so
/// there is no shape a `list`, a generator or a differently-sized tuple
/// could be written into -- while the elements' `float` is a type the
/// author wrote in the annotation, which makes running CPython's own
/// conversion protocol on them the same explicit-conversion case
/// [`EXT_OBJ_TO_FLOAT_SYMBOL`] records. It is therefore *not*
/// `pycc_ext_unpack_float_at`, whose `PyFloat_Check` refusal exists because
/// the thunk export seam is closed.
///
/// **The arity is a parameter, never a constant.** The admission rule is
/// any fixed arity with every element `float`, so neither this declaration
/// nor the shim may hard-code the three of `tuple[float, float, float]`.
///
/// **Ownership.** Each `PyNumber_Float` temporary is released inside the
/// same loop iteration that produced it, so the failing exit holds nothing
/// and this operation adds nothing to the #1092 leak-only set -- Part 4's
/// property, unchanged.
///
/// Spelled once here for the same lazy-link reason as [`EXT_OBJ_LEN_SYMBOL`].
pub const EXT_OBJ_UNPACK_FLOAT_TUPLE_SYMBOL: &str = "pycc_ext_obj_unpack_float_tuple";

/// The C-legal spelling of a possibly-dotted pycc name.
///
/// A method reaches MIR under a dotted name (`Grid.scale.static`), and two
/// of the places that name is used are *C identifiers*: the
/// `extern void *fnptr_<name>;` declaration `pycc::ext_build`'s
/// `wrapper_for` emits, and the `pycc_ext_wrap_<name>` /
/// `pycc_ext_thunk_<name>` symbols. `extern void *fnptr_Grid.scale.static;`
/// is not accepted by any C compiler, so the dotted spelling has to be
/// encoded -- and `src/ext_build.rs`'s rebind-dedup comment records that a
/// *duplicate* export name makes clang reject the generated `.inc`
/// outright, so the encoding has to be injective as well as legal.
///
/// The encoding: a name with no `.` is returned unchanged; otherwise the
/// result is `"0m"` followed by, for each `.`-separated segment in order,
/// the segment's byte length in decimal, then `"_"`, then the segment. So
/// `f` stays `f` and `Grid.scale.static` becomes `0m4_Grid5_scale6_static`.
///
/// **It is injective, and that is argued rather than fixture-tested.**
/// Every dot-free name reaching this function is either a Python identifier
/// or carries the compiler-generated `0gen_` prefix
/// (`pycc_types::monomorphize`), and neither can begin with `0m` -- the
/// premise is stated this way rather than as "Python identifiers cannot
/// begin with a digit", because a dot-free name like `0gen_make__T_int` is
/// a real identity-branch input that *does* begin with a digit. So the
/// identity branch's outputs never collide with a `0m...` output. Within
/// the `0m` branch the encoding is length-prefixed and therefore uniquely
/// decodable, so two distinct dotted names cannot mangle alike. `.` -> `_`
/// and `.` -> `__` both fail this argument, the second one silently: a
/// module-level `def Grid__scale__static` would collide with
/// `Grid.scale.static`.
///
/// Every use site prefixes the result (`fnptr_`, `fnname_`,
/// `pycc_ext_thunk_`, `pycc_ext_wrap_`), so the leading digit never starts
/// a C identifier. Because the dot-free case is the identity, every symbol
/// the compiler emitted before methods became exportable is byte-identical.
///
/// This is the one canonical implementation: `pycc::ext_build` calls it
/// rather than reimplementing it, exactly as it already calls
/// [`ext_thunk_symbol`].
#[must_use]
pub fn mangle_ext_name(name: &str) -> String {
    if !name.contains('.') {
        return name.to_string();
    }
    let mut out = String::from("0m");
    for segment in name.split('.') {
        out.push_str(&segment.len().to_string());
        out.push('_');
        out.push_str(segment);
    }
    out
}

/// The external symbol `name`'s scalar-only `ext` export thunk is emitted
/// under.
///
/// `name` is mangled through [`mangle_ext_name`] first, so a method's thunk
/// is a legal C identifier the generated `extern` declaration can name. The
/// mangling is the identity for a dot-free name, so every module-level
/// function's thunk symbol is unchanged.
#[must_use]
pub fn ext_thunk_symbol(name: &str) -> String {
    format!("{EXT_THUNK_PREFIX}{}", mangle_ext_name(name))
}

/// Whether `name` is a name D-244 rule 1 can export at all, disregarding
/// its signature.
///
/// The tests are exactly `pycc::ext_build::collect_exports`' own *lexical*
/// verdict, and this is their one canonical home so the two sides of the
/// seam cannot drift: a wrapper generated for a name codegen declined to
/// emit a thunk for links cleanly and crashes on the first call.
/// `src/ext_build_tests/exports.rs` carries a test pinning the two equal
/// over a shared table of names, because the drift is otherwise silent --
/// `wrapper_for` picks the thunk `extern` or the `fnptr_` `extern` from
/// [`ext_thunk_required`], so a disagreement emits the wrong C declaration
/// for a `tuple`-carrying method.
///
/// The public-name test is D-038's predicate, spelled out rather than
/// delegated to `pycc_hir::is_public_name` because this crate deliberately
/// does not depend on `pycc_hir` -- it sees only `pycc_mir`'s re-export of
/// `Ty`. The body there is `!name.starts_with('_')` and nothing else; if it
/// ever grows a case, this copy must grow with it.
///
/// **Dotted names are no longer refused wholesale.** A method reaches MIR
/// under `<Class>.<method>` and the suffixed spellings
/// `<Class>.<method>.static`, `<Class>.<method>.classmethod` and
/// `<Class>.<property>.setter` (`pycc_hir::class`'s mangling). This admits
/// the `.static` and `.classmethod` spellings and -- since #1145 -- the
/// bare `<Class>.<method>` one, with every segment public.
/// `<Class>.<property>.setter` stays refused: a `@property` is attribute
/// syntax on the host side, never a method table entry.
///
/// The bare spelling covers the three `MethodKind`s `Regular`,
/// `PropertyGetter` and `AbstractMethod` at once, and nothing here can tell
/// them apart. Admitting it is deliberate rather than an approximation:
/// widening this mirror is what makes it a **superset** of the driver's
/// admitted set again, and a superset is the safe direction. The driver
/// narrows the bare spelling back down with filters that read
/// `HirModule::class_defs`; leaving this side refusing it would instead
/// make `ext_thunk_required` answer `false` for a `tuple`-carrying instance
/// method, so the wrapper would emit the `fnptr_` cast form -- measured to
/// fault (SIGBUS) on aarch64-apple-darwin for an out-pointer signature.
///
/// **This verdict is purely lexical, and must stay so.** The function
/// receives a bare `&str` and this crate cannot see `pycc_hir`, so a
/// verdict that consulted `HirModule::class_defs` would have no
/// mirror-comparable form here and the parity test would stop being
/// well-formed. The driver layers its exception-class exclusion *on top of*
/// this verdict rather than inside it, which makes this mirror a
/// **superset** of the driver's admitted set: at worst a thunk is emitted
/// for a name no wrapper calls, which is dead code -- never the link error
/// the drift above would be.
///
/// A monomorphized generic specialization carries the `0gen_` prefix and
/// has no `fnptr_` global to dispatch through, so it stays refused; that
/// test is applied to the whole name *before* the split and takes
/// precedence, as cheap defense in depth. It is not a live hazard: the real
/// specialization shapes put the substitution suffix last
/// (`0gen_<Class>.<method>__<P>_<C>`), so a `0gen_` name's last segment is
/// never `static`.
#[must_use]
pub fn is_ext_exportable_name(name: &str) -> bool {
    if name.starts_with("0gen_") {
        return false;
    }
    let mut segments = name.split('.');
    // `str::split` always yields at least one segment, so the fallback is
    // unreachable rather than a second refusal path; an empty first segment
    // is refused on the next line either way, which is what the driver's
    // mirror does with its own empty-segment guard.
    let first = segments.next().unwrap_or("");
    if first.starts_with('_') || first.is_empty() {
        return false;
    }
    let Some(second) = segments.next() else {
        // A dot-free name: a module-level function, admitted by D-038's
        // predicate alone.
        return true;
    };
    if second.starts_with('_') || second.is_empty() {
        return false;
    }
    match segments.next() {
        // `<Class>.<method>` -- `Regular`, `PropertyGetter` or
        // `AbstractMethod`, indistinguishable here and all admitted as the
        // superset the driver narrows (#1145).
        None => true,
        Some(kind) => {
            // A fourth segment cannot arise: a class nested in a class or a
            // function is refused by `pycc_hir` (`stmt.rs`, `class.rs`), so
            // no `A.B.method` name exists. Refusing it is the fail-closed
            // reading rather than a reachable branch.
            segments.next().is_none() && (kind == "static" || kind == "classmethod")
        }
    }
}

/// The boundary slots a value of type `ty` occupies when it crosses the
/// `ext` seam: a `tuple`'s elements, in order, or the type itself.
///
/// D-116 fixes a tuple's arity and restricts its elements to `int`, `bool`
/// and `float` (`pycc_hir`'s `check_tuple_element_ty` rejects anything else
/// with `T0039`, and `tuple[int, ...]`/`tuple[()]` with `T0053`), so the
/// returned slice is always non-empty and never itself contains a tuple.
#[must_use]
pub fn ext_boundary_slots(ty: &Ty) -> &[Ty] {
    match ty {
        Ty::Tuple(elems) => elems.as_slice(),
        _ => std::slice::from_ref(ty),
    }
}

/// The thunk's own parameter list: every declared parameter flattened in
/// place to the scalars it occupies.
#[must_use]
pub fn ext_thunk_param_tys(param_tys: &[Ty]) -> Vec<Ty> {
    param_tys
        .iter()
        .flat_map(|ty| ext_boundary_slots(ty).iter().cloned())
        .collect()
}

/// The element types a `tuple` return is handed back through, one trailing
/// out-pointer each, appended after every flattened parameter. Empty for
/// every other return type, which stays a real return value.
///
/// Returning a tuple *by value* is not an option: pycc's own aggregate
/// calling convention is not the platform C struct ABI. Measured on
/// aarch64-apple-darwin, a pycc function returning `tuple[int, int, int,
/// int, int]` hands the five words back in `x0`-`x4` where clang's C ABI
/// would pass a hidden `sret` pointer -- so a C declaration of the compiled
/// function would disagree with it silently. The thunk exists precisely to
/// keep every aggregate on the LLVM side of the seam.
///
/// Part 2 of #1175 (#1179) adds the second producer of out-pointers, and it
/// is **not** a function of the declared return type: an export declared
/// `-> memoryview` carries three trailing `i64` out-slots only when its own
/// body returns a sub-range of a buffer parameter. `has_slice`, `start` and
/// `stop`, in that order. Keying this on `Ty::MemoryView` alone would flip
/// every existing buffer-returning export off the direct `fnptr_<name>`
/// cast path and onto the thunk path, changing generated C for exports this
/// change does not touch; hence the explicit per-body argument.
///
/// The third slot is load-bearing. `stop = -1` is not "absent", it is the
/// legal `b[0:-1]`, and `i64::MIN`/`i64::MAX` are legal and clamp
/// correctly, so no pair of `i64`s is available as an in-band
/// whole-view sentinel.
#[must_use]
pub fn ext_thunk_out_tys(return_ty: &Ty, returns_buffer_slice: bool) -> Vec<Ty> {
    match return_ty {
        Ty::Tuple(elems) => (**elems).clone(),
        _ if returns_buffer_slice => vec![Ty::Int, Ty::Int, Ty::Int],
        _ => Vec::new(),
    }
}

/// Whether `body` contains the admitted buffer sub-range `return`
/// (Part 2 of #1175, #1179).
///
/// The codegen-side half of a fact the driver computes independently from
/// HIR (`src/ext_build.rs`'s `ExtExport::returns_buffer_slice`). `ExtExport`
/// is a driver-only type that never reaches this crate, and the fact governs
/// both the compiled function's own LLVM signature and the generated C call
/// form, so each side must answer it from the IR it has. Because
/// `MirStmt::ReturnBufferSlice` exists precisely so the admitted shape has a
/// node of its own, this walk is an exact presence test rather than a second
/// copy of the admission rule -- the same mirror relationship
/// [`is_ext_exportable_name`] already has with the driver's own export
/// predicate, and pinned by the cross-crate parity test.
///
/// Exhaustive over every nested-body statement form, modelled on
/// `pycc_mir`'s own `set_frame_function`, so a `return b[i:j]` inside an
/// `if`, a loop or a `try` is found -- branching provenance is admitted and
/// must be.
#[must_use]
pub fn body_returns_buffer_slice(body: &[pycc_mir::MirStmt]) -> bool {
    use pycc_mir::MirStmt;
    body.iter().any(|stmt| match stmt {
        MirStmt::ReturnBufferSlice { .. } => true,
        MirStmt::If { body, orelse, .. } => {
            body_returns_buffer_slice(body) || body_returns_buffer_slice(orelse)
        }
        MirStmt::While { body, .. }
        | MirStmt::ForRange { body, .. }
        | MirStmt::ForList { body, .. }
        | MirStmt::ForObject { body, .. }
        | MirStmt::ForDict { body, .. }
        | MirStmt::ForSet { body, .. } => body_returns_buffer_slice(body),
        MirStmt::Seq(stmts) => body_returns_buffer_slice(stmts),
        MirStmt::Try {
            body,
            handlers,
            orelse,
            finalbody,
        }
        | MirStmt::TryStar {
            body,
            handlers,
            orelse,
            finalbody,
        } => {
            body_returns_buffer_slice(body)
                || handlers
                    .iter()
                    .any(|handler| body_returns_buffer_slice(&handler.body))
                || body_returns_buffer_slice(orelse)
                || body_returns_buffer_slice(finalbody)
        }
        MirStmt::ExprStmt(_)
        | MirStmt::Assign { .. }
        | MirStmt::NoOp
        | MirStmt::Unreachable
        | MirStmt::DictSet { .. }
        | MirStmt::BufferSet { .. }
        | MirStmt::ListCompAssign { .. }
        | MirStmt::DictCompAssign { .. }
        | MirStmt::SetCompAssign { .. }
        | MirStmt::Return(_)
        | MirStmt::AttrSet { .. }
        | MirStmt::Raise { .. }
        | MirStmt::RaiseFrom { .. }
        | MirStmt::Reraise => false,
    })
}

/// Whether a function needs a scalar-only export thunk emitted for it.
///
/// Only a signature that actually carries an aggregate does: a scalar-only
/// export's generated wrapper still reaches the compiled function through
/// the `fnptr_<name>` global directly, and emitting a thunk it would never
/// call would be dead weight in every artifact.
///
/// Part 2 of #1175 (#1179) adds one non-signature reason: an export whose
/// body returns a sub-range of a buffer parameter needs the three trailing
/// out-pointers [`ext_thunk_out_tys`] describes, so it can no longer be a
/// pure function of the declared signature. Every other `-> memoryview`
/// export keeps the direct cast path byte-for-byte, which is what the
/// `returns_buffer_slice` argument buys -- it is passed in rather than
/// inferred so the driver and codegen halves of the fact stay two
/// deliberately mirrored computations rather than three.
#[must_use]
pub fn ext_thunk_required(
    name: &str,
    param_tys: &[Ty],
    return_ty: &Ty,
    returns_buffer_slice: bool,
) -> bool {
    is_ext_exportable_name(name)
        && (param_tys.iter().any(|ty| matches!(ty, Ty::Tuple(_)))
            || matches!(return_ty, Ty::Tuple(_))
            || returns_buffer_slice)
}
