//! Emission for `MirItem::ForeignImport` (Part 1 of #1026, PR 1c of #1080)
//! and for `MirStmt::ForeignImport`, a foreign import nested in a
//! module-level `if`/`try` block (#1291).
//!
//! Cohesion-driven carve out of `lib.rs` under AGENTS.md's decomposability
//! rule: everything that knows how a foreign `import numpy` becomes machine
//! code lives here -- the module global its local name gets, the extern
//! declaration of the shim's import helper, and the call emitted at the
//! import's own source position.
//!
//! **Ordering.** The call is emitted from `compile_to_object`'s single
//! source-order `for item in &mir.items` loop, at the position
//! `pycc_mir::build` spliced the item into. It is deliberately *not* a
//! hoisted prologue: CPython runs a module body statement by statement, so a
//! `print(...)` written above an `import` runs first, and D-244 rule 3 binds
//! the artifact to CPython's observable behaviour. `MirItem::ForeignImport`
//! exists precisely so that ordering is structural rather than a convention
//! this file would have to re-derive. A nested import is a statement, so
//! `emit_stmt` emits it where its block runs; its failure edge returns from
//! the same entry point, skipping any enclosing `except`/`finally` (the
//! #1096 edge, `docs/RUNTIME.md`).
//!
//! **Ownership** (`docs/RUNTIME.md`). The module object is imported exactly
//! once, during `pycc_ext_module_exec`, into a module-level global, and is
//! never released: a CPython module object lives in `sys.modules` for the
//! interpreter's lifetime anyway, and the generated artifact has no
//! teardown hook to release it from. Contrast the `Ty::Str` exit-time decref
//! loop in `compile_to_object`, which is `!options.ext`-guarded for the
//! same reason -- there is no "program exit" in a hosted extension module.
//!
//! Part 2 of #1026 kept that rule but removed its old justification: a
//! `Py_DECREF` path used to be *unreachable* because no operation on the
//! bound name was implemented at all, and now `foreign_attr.rs` implements
//! one. The leak here is still once per process; the one an attribute load
//! introduces is trip-count-linear, and `docs/RUNTIME.md` carries the
//! consequence for D-244 rule 6 benchmarking.
//!
//! **Native mode.** A foreign import reaches codegen only with `ext` set:
//! either `--ext`, or an embedded build (D-248), which compiles its module
//! with `ext` set. Every other build has no interpreter to import into, and
//! the driver refuses it with `I0403` after typed HIR. This file
//! therefore *ignores* a `ForeignImport` item when `options.ext` is false
//! rather than asserting -- an assertion here would be an unreachable line
//! under the 100%-changed-line coverage gate, and the driver gate is where
//! the refusal is actually tested.

use super::*;
use inkwell::builder::Builder;

/// Declares the shim's `PyObject *pycc_ext_obj_import(const char *)` once
/// per module, returning the existing declaration on every later call.
fn obj_import_fn<'ctx>(
    context: &'ctx Context,
    module: &inkwell::module::Module<'ctx>,
) -> FunctionValue<'ctx> {
    if let Some(existing) = module.get_function(EXT_OBJ_IMPORT_SYMBOL) {
        return existing;
    }
    let ptr = context.ptr_type(inkwell::AddressSpace::default());
    module.add_function(
        EXT_OBJ_IMPORT_SYMBOL,
        ptr.fn_type(&[ptr.into()], false),
        None,
    )
}

/// Emits the import call for one foreign import binding (a
/// [`MirItem::ForeignImport`], or one pair of a `MirStmt::ForeignImport`)
/// and stores the resulting module object into `slot`, `local_name`'s
/// module global. `local_name` also names the module-path string global;
/// two imports binding the same name get two strings, which LLVM names
/// apart by emission order.
///
/// A `NULL` return means CPython raised (`ModuleNotFoundError` being the
/// expected one): the exception is already set by the shim, so the entry
/// point returns [`EXT_MODULE_EXEC_FAILED`] immediately -- the
/// `Py_mod_exec` slot's failure convention -- and the module body's
/// remaining statements never run.
pub(super) fn emit<'ctx>(
    context: &'ctx Context,
    builder: &Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    entry_fn: FunctionValue<'ctx>,
    slot: &StorageSlot<'ctx>,
    local_name: &str,
    module_path: &str,
) {
    let import = obj_import_fn(context, module);
    let name = builder
        .build_global_string_ptr(module_path, &format!("pycc_foreign_module_{local_name}"))
        .expect("build_global_string_ptr should not fail")
        .as_pointer_value();
    let imported = builder
        .build_call(import, &[name.into()], "foreign_import")
        .expect("build_call should not fail for pycc_ext_obj_import")
        .try_as_basic_value()
        .expect_basic("pycc_ext_obj_import returns PyObject *")
        .into_pointer_value();
    let failed = builder
        .build_is_null(imported, "foreign_import_failed")
        .expect("build_is_null should not fail");
    let fail_bb = context.append_basic_block(entry_fn, "foreign_import_fail");
    let cont_bb = context.append_basic_block(entry_fn, "foreign_import_cont");
    builder
        .build_conditional_branch(failed, fail_bb, cont_bb)
        .expect("build_conditional_branch should not fail");
    builder.position_at_end(fail_bb);
    builder
        .build_return(Some(
            &context
                .i64_type()
                .const_int(EXT_MODULE_EXEC_FAILED as u64, true),
        ))
        .expect("build_return should not fail");
    builder.position_at_end(cont_bb);
    builder
        .build_store(slot.ptr, imported)
        .expect("build_store should not fail for a foreign module global");
    // Every module global carries a separate definite-assignment flag; the
    // import is the binding's assignment, so raise it exactly as an
    // ordinary top-level `Assign` does.
    if let Some(initialized) = slot.initialized {
        builder
            .build_store(initialized, context.i8_type().const_int(1, false))
            .expect("build_store should not fail for a foreign module init flag");
    }
}

/// Emits a [`pycc_mir::MirStmt::ForeignImport`]: one import per binding,
/// in order, into the module-exec entry point `builder` is emitting into.
/// No `options.ext` guard is needed: a foreign binding reaches codegen only
/// in an `ext` build, because the driver refuses every other build with
/// `I0403`, and `expect_module_exec_entry` pins the entry point.
pub(super) fn emit_stmt<'ctx>(
    context: &'ctx Context,
    builder: &Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    locals: &HashMap<String, StorageSlot<'ctx>>,
    bindings: &[(String, String)],
) {
    let entry_fn = crate::foreign_attr::expect_module_exec_entry(builder);
    for (local_name, module_path) in bindings {
        emit(
            context,
            builder,
            module,
            entry_fn,
            &locals[local_name],
            local_name,
            module_path,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CompileOptions, EXT_MODULE_EXEC_SYMBOL, compile_to_object_with_observer};
    use inkwell::values::AnyValue;
    use pycc_mir::{MirExpr, MirItem, MirModule, MirStmt, Ty};

    /// `print(<n>)` as a top-level statement, with `n` chosen distinct per
    /// call site so the emitted literal identifies which statement's code a
    /// given point in the IR belongs to.
    fn print_int(n: i64) -> MirItem {
        MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
            callee: "print".to_string(),
            args: vec![MirExpr::IntLiteral(n)],
            ty: Ty::None,
        }))
    }

    /// The LLVM text of the module-exec entry point after compiling `items`
    /// as an `ext` object.
    ///
    /// Compiles all the way to an object file, exactly as
    /// `ext_thunk_tests.rs` does, so LLVM's verifier runs over the blocks
    /// `emit` appends before any assertion here is believed.
    fn entry_ir(label: &str, items: Vec<MirItem>) -> String {
        let dir = pycc_scratch::ScratchDir::new(label).expect("failed to create scratch dir");
        let mut ir = String::new();
        let mut observer = |module: &inkwell::module::Module<'_>, _: Option<&'static str>| {
            if let Some(entry) = module.get_function(EXT_MODULE_EXEC_SYMBOL) {
                // D-029: an `LLVMString` reaches an owned `String` through
                // the crate's own wrapper, never through its `Drop`.
                ir = crate::llvm_string_to_owned(entry.print_to_string());
            }
        };
        compile_to_object_with_observer(
            &MirModule {
                items,
                ..Default::default()
            },
            &dir.join(format!("{label}.o")),
            &CompileOptions {
                ext: true,
                ..CompileOptions::default()
            },
            Some(&mut observer),
        )
        .expect("ext codegen should succeed");
        assert!(!ir.is_empty(), "no {EXT_MODULE_EXEC_SYMBOL} was emitted");
        ir
    }

    /// The #1080 ordering obligation, stated at the emission layer.
    ///
    /// `crates/pycc_mir/src/tests/import.rs` pins that the *item* lands at
    /// its source position; that is a statement about the IR the MIR
    /// builder produces, not about the machine code codegen emits from it.
    /// This is the other half: a module body statement written above an
    /// `import` really is emitted above the import call, as CPython would
    /// run it (D-244 rule 3), and a statement written below it below.
    ///
    /// Position is read off the two `print` calls rather than off block
    /// names: the import's own `foreign_import_fail`/`foreign_import_cont`
    /// blocks are appended to the function in emission order regardless of
    /// where the call sits, so a block-order assertion would hold just as
    /// well for the hoisted prologue this replaced. It is read off the
    /// emitted `pycc_rt_int_to_str` calls rather than off the literals
    /// themselves because an `int` reaches the IR tag-encoded (`111` prints
    /// as `i64 223`), which is a representation detail this test has no
    /// business pinning.
    #[test]
    fn a_statement_above_a_foreign_import_is_emitted_above_the_import_call() {
        let ir = entry_ir(
            "foreign_import_order",
            vec![
                print_int(111),
                MirItem::ForeignImport {
                    local_name: "numpy".to_string(),
                    module_path: "numpy".to_string(),
                },
                print_int(222),
            ],
        );
        const PRINTED: &str = "pycc_rt_int_to_str";
        let before = ir.find(PRINTED).expect("the statement above the import");
        let after = ir.rfind(PRINTED).expect("the statement below the import");
        assert_ne!(before, after, "both statements are emitted: {ir}");
        let import = ir
            .find(EXT_OBJ_IMPORT_SYMBOL)
            .expect("the foreign import call");
        assert!(before < import, "{ir}");
        assert!(import < after, "{ir}");
    }

    /// Two foreign imports in one module share one extern declaration of
    /// the shim helper.
    ///
    /// `obj_import_fn` declares `pycc_ext_obj_import` lazily and returns
    /// the existing `FunctionValue` on every later call; declaring it
    /// twice would be an LLVM module-verifier error (a redefinition), so
    /// the second import is what proves the early return is taken rather
    /// than merely present. Both calls are still emitted, in source order.
    #[test]
    fn a_second_foreign_import_reuses_the_one_extern_declaration() {
        let mut declared = 0usize;
        let ir = entry_ir(
            "foreign_import_twice",
            vec![
                MirItem::ForeignImport {
                    local_name: "numpy".to_string(),
                    module_path: "numpy".to_string(),
                },
                MirItem::ForeignImport {
                    local_name: "scipy".to_string(),
                    module_path: "scipy".to_string(),
                },
            ],
        );
        let mut rest = ir.as_str();
        while let Some(at) = rest.find(EXT_OBJ_IMPORT_SYMBOL) {
            declared += 1;
            rest = &rest[at + EXT_OBJ_IMPORT_SYMBOL.len()..];
        }
        assert_eq!(declared, 2, "one call site per import: {ir}");
        let numpy = ir.find("numpy").expect("the first module name");
        let scipy = ir.find("scipy").expect("the second module name");
        assert!(numpy < scipy, "{ir}");
    }

    /// The gate `src/foreign_import.rs` exists to make coverable: a build
    /// that neither passes `--ext` nor embeds CPython (D-248; an embedded
    /// build compiles with `ext` set) has no interpreter to import into,
    /// the driver has already refused with `I0403`, and this arm emits
    /// nothing rather than asserting.
    #[test]
    fn a_native_build_emits_no_import_call_for_a_foreign_import_item() {
        let dir = pycc_scratch::ScratchDir::new("foreign_import_native").expect("scratch");
        let mut symbols = Vec::new();
        let mut observer = |module: &inkwell::module::Module<'_>, _: Option<&'static str>| {
            symbols = module
                .get_functions()
                .map(|f| f.get_name().to_string_lossy().into_owned())
                .collect();
        };
        compile_to_object_with_observer(
            &MirModule {
                items: vec![MirItem::ForeignImport {
                    local_name: "numpy".to_string(),
                    module_path: "numpy".to_string(),
                }],
                ..Default::default()
            },
            &dir.join("native.o"),
            &CompileOptions::default(),
            Some(&mut observer),
        )
        .expect("native codegen should succeed");
        assert!(
            !symbols.iter().any(|s| s == EXT_OBJ_IMPORT_SYMBOL),
            "{symbols:?}"
        );
    }
}
