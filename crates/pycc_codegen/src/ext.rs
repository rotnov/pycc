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

/// The external symbol `name`'s scalar-only `ext` export thunk is emitted
/// under.
#[must_use]
pub fn ext_thunk_symbol(name: &str) -> String {
    format!("{EXT_THUNK_PREFIX}{name}")
}

/// Whether `name` is a name D-244 rule 1 can export at all, disregarding
/// its signature.
///
/// The three tests are exactly `pycc::ext_build::collect_exports`' own, and
/// this is their one canonical home so the two sides of the seam cannot
/// drift: a wrapper generated for a name codegen declined to emit a thunk
/// for links cleanly and crashes on the first call.
///
/// The first test is D-038's public-name predicate, spelled out rather than
/// delegated to `pycc_hir::is_public_name` because this crate deliberately
/// does not depend on `pycc_hir` -- it sees only `pycc_mir`'s re-export of
/// `Ty`. The body there is `!name.starts_with('_')` and nothing else; if it
/// ever grows a case, this copy must grow with it. The other two are not
/// policy but representation: a method reaches MIR under its `Class.method`
/// name, and a monomorphized generic specialization carries the `0gen_`
/// prefix and has no `fnptr_` global to dispatch through.
#[must_use]
pub fn is_ext_exportable_name(name: &str) -> bool {
    !name.starts_with('_') && !name.contains('.') && !name.starts_with("0gen_")
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
#[must_use]
pub fn ext_thunk_out_tys(return_ty: &Ty) -> &[Ty] {
    match return_ty {
        Ty::Tuple(elems) => elems.as_slice(),
        _ => &[],
    }
}

/// Whether a function needs a scalar-only export thunk emitted for it.
///
/// Only a signature that actually carries an aggregate does: a scalar-only
/// export's generated wrapper still reaches the compiled function through
/// the `fnptr_<name>` global directly, and emitting a thunk it would never
/// call would be dead weight in every artifact.
#[must_use]
pub fn ext_thunk_required(name: &str, param_tys: &[Ty], return_ty: &Ty) -> bool {
    is_ext_exportable_name(name)
        && (param_tys.iter().any(|ty| matches!(ty, Ty::Tuple(_)))
            || matches!(return_ty, Ty::Tuple(_)))
}
