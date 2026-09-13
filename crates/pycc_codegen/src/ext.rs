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
