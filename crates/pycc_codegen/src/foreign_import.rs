//! Emission for `MirItem::ForeignImport` (Part 1 of #1026, PR 1c of #1080)
//! and for `MirStmt::ForeignImport`, a foreign import nested in a
//! module-level `if`/`try` block (#1291).
//!
//! Cohesion-driven carve out of `lib.rs` under AGENTS.md's decomposability
//! rule: everything that knows how a foreign `import numpy` becomes machine
//! code lives here -- the module global its local name gets, the extern
//! declaration of the shim's import helpers, and the call emitted at the
//! import's own source position. Since #1278 that includes `from numpy
//! import pi`, which binds the module's attribute through
//! `pycc_ext_obj_import_from` instead of the module through
//! `pycc_ext_obj_import`, and the `MirItem::ForeignImport` arm of
//! `compile_to_object`'s item loop, which is only a call to [`emit_item`].
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
use crate::ext::EXT_OBJ_IMPORT_FROM_SYMBOL;
use inkwell::builder::Builder;
use pycc_mir::FromImport;

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

/// One foreign import binding as codegen emits it: the module global it
/// stores into (`local_name`), the module, and -- for one name of a
/// `from X import a, b` (#1278) -- that name's place in the statement's
/// fromlist.
#[derive(Clone, Copy)]
pub(super) struct ForeignBinding<'a> {
    pub(super) local_name: &'a str,
    pub(super) module_path: &'a str,
    pub(super) from: Option<&'a FromImport>,
}

/// Emits a top-level [`MirItem::ForeignImport`] at its own position in
/// `compile_to_object`'s source-order item loop, or nothing at all in a
/// build without `ext` (see the module doc's **Native mode**). `def_iter`
/// in that loop is deliberately not advanced: it pairs with
/// `MirItem::Function` items only.
pub(super) fn emit_item<'ctx>(
    context: &'ctx Context,
    builder: &Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    entry_fn: FunctionValue<'ctx>,
    ext: bool,
    module_globals: &BTreeMap<String, StorageSlot<'ctx>>,
    binding: ForeignBinding<'_>,
) {
    if ext {
        emit(
            context,
            builder,
            module,
            entry_fn,
            &module_globals[binding.local_name],
            binding,
        );
    }
}

/// Declares the shim's `PyObject *pycc_ext_obj_import_from(const char *,
/// const char *const *, long long, long long)` once per module (#1278).
fn obj_import_from_fn<'ctx>(
    context: &'ctx Context,
    module: &inkwell::module::Module<'ctx>,
) -> FunctionValue<'ctx> {
    if let Some(existing) = module.get_function(EXT_OBJ_IMPORT_FROM_SYMBOL) {
        return existing;
    }
    let ptr = context.ptr_type(inkwell::AddressSpace::default());
    let i64t = context.i64_type();
    module.add_function(
        EXT_OBJ_IMPORT_FROM_SYMBOL,
        ptr.fn_type(&[ptr.into(), ptr.into(), i64t.into(), i64t.into()], false),
        None,
    )
}

/// Emits the `pycc_ext_obj_import_from` call for one name of a
/// `from X import a, b` (#1278) and returns the new reference it yields.
///
/// The whole fromlist is passed on every name's call: CPython's
/// `IMPORT_NAME` receives all of it before its first `IMPORT_FROM`, so a
/// package submodule listed later in the statement is already imported
/// when an earlier name is fetched. Each name is a private string global
/// (`pycc_foreign_attr_{local}`), and the list is a private constant array
/// of pointers to them (`pycc_foreign_fromlist_{local}`).
fn emit_import_from_call<'ctx>(
    context: &'ctx Context,
    builder: &Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    local_name: &str,
    module_name: PointerValue<'ctx>,
    from: &FromImport,
) -> PointerValue<'ctx> {
    let ptr = context.ptr_type(inkwell::AddressSpace::default());
    let names: Vec<PointerValue<'ctx>> = from
        .fromlist
        .iter()
        .map(|name| {
            builder
                .build_global_string_ptr(name, &format!("pycc_foreign_attr_{local_name}"))
                .expect("build_global_string_ptr should not fail")
                .as_pointer_value()
        })
        .collect();
    let array_ty = ptr.array_type(names.len() as u32);
    let fromlist = module.add_global(
        array_ty,
        None,
        &format!("pycc_foreign_fromlist_{local_name}"),
    );
    fromlist.set_initializer(&ptr.const_array(&names));
    fromlist.set_constant(true);
    fromlist.set_linkage(Linkage::Private);
    let i64t = context.i64_type();
    builder
        .build_call(
            obj_import_from_fn(context, module),
            &[
                module_name.into(),
                fromlist.as_pointer_value().into(),
                i64t.const_int(from.fromlist.len() as u64, false).into(),
                i64t.const_int(from.index as u64, false).into(),
            ],
            "foreign_import",
        )
        .expect("build_call should not fail for pycc_ext_obj_import_from")
        .try_as_basic_value()
        .expect_basic("pycc_ext_obj_import_from returns PyObject *")
        .into_pointer_value()
}

/// Emits the import call for one foreign import binding (a
/// [`MirItem::ForeignImport`], or one pair of a `MirStmt::ForeignImport`)
/// and stores the resulting object into `slot`, the binding's module
/// global. `pycc_ext_obj_import(module_path)` binds the module for `import
/// X`; `pycc_ext_obj_import_from` binds the named attribute for `from X
/// import n` (#1278). The local name also names the module-path string
/// global; two imports binding the same name get two strings, which LLVM
/// names apart by emission order.
///
/// A `NULL` return means CPython raised (`ModuleNotFoundError`, or
/// `ImportError` for a missing name): the exception is already set by the
/// shim, so the entry point returns [`EXT_MODULE_EXEC_FAILED`] immediately
/// -- the `Py_mod_exec` slot's failure convention -- and the module body's
/// remaining statements never run.
pub(super) fn emit<'ctx>(
    context: &'ctx Context,
    builder: &Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    entry_fn: FunctionValue<'ctx>,
    slot: &StorageSlot<'ctx>,
    binding: ForeignBinding<'_>,
) {
    let ForeignBinding {
        local_name,
        module_path,
        from,
    } = binding;
    let name = builder
        .build_global_string_ptr(module_path, &format!("pycc_foreign_module_{local_name}"))
        .expect("build_global_string_ptr should not fail")
        .as_pointer_value();
    let imported = match from {
        None => builder
            .build_call(
                obj_import_fn(context, module),
                &[name.into()],
                "foreign_import",
            )
            .expect("build_call should not fail for pycc_ext_obj_import")
            .try_as_basic_value()
            .expect_basic("pycc_ext_obj_import returns PyObject *")
            .into_pointer_value(),
        Some(from) => emit_import_from_call(context, builder, module, local_name, name, from),
    };
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
/// `I0403`, and `expect_module_exec_entry` pins the entry point. A block
/// import is never a from-import (#1278; see `MirStmt::ForeignImport`).
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
            ForeignBinding {
                local_name,
                module_path,
                from: None,
            },
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
                    from: None,
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
                    from: None,
                },
                MirItem::ForeignImport {
                    local_name: "scipy".to_string(),
                    module_path: "scipy".to_string(),
                    from: None,
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

    /// `if <test>:` wrapping a block foreign import (#1291) of `bindings`.
    fn if_block_import(bindings: &[(&str, &str)]) -> MirItem {
        MirItem::TopLevelStmt(MirStmt::If {
            test: MirExpr::BoolLiteral(true),
            body: vec![MirStmt::ForeignImport {
                bindings: bindings
                    .iter()
                    .map(|(local, module)| ((*local).to_string(), (*module).to_string()))
                    .collect(),
            }],
            orelse: vec![],
        })
    }

    /// #1291: a foreign import nested in a module-level `if` is emitted in
    /// the branch that runs it, with the same failure edge as a top-level
    /// one, and its name is stored to a module global that
    /// `collect_module_bindings` declared.
    #[test]
    fn a_block_foreign_import_is_emitted_inside_its_branch() {
        let ir = entry_ir(
            "foreign_import_block",
            vec![
                print_int(111),
                if_block_import(&[("colorsys", "colorsys")]),
                print_int(222),
            ],
        );
        let import = ir.find(EXT_OBJ_IMPORT_SYMBOL).expect("the import call");
        let then_block = ir.find("if_then:").expect("the `if` branch block");
        assert!(then_block < import, "{ir}");
        assert!(ir.contains("foreign_import_fail"), "{ir}");
        assert!(ir.contains("ret i64 -1"), "{ir}");
        assert!(
            ir.contains("store ptr %foreign_import, ptr @pyglobal_colorsys"),
            "the module global is stored: {ir}"
        );
    }

    /// Two bindings of one node are emitted in source order, one call each.
    #[test]
    fn two_bindings_of_one_block_import_are_emitted_in_order() {
        let ir = entry_ir(
            "foreign_import_block_two",
            vec![if_block_import(&[("sys", "sys"), ("re", "re")])],
        );
        assert_eq!(ir.matches(EXT_OBJ_IMPORT_SYMBOL).count(), 2, "{ir}");
        let sys = ir.find("@pyglobal_sys").expect("the first global");
        let re = ir.find("@pyglobal_re").expect("the second global");
        assert!(sys < re, "{ir}");
    }

    /// The identical pair #1291 admits (`import numpy` in both arms of an
    /// `if`/`else`) requests the module-path global
    /// `pycc_foreign_module_numpy` twice. LLVM uniquifies the second name,
    /// so each emission must still pass a global holding its own path
    /// string: both calls are emitted, and every module-path global the
    /// module declares holds `numpy`.
    #[test]
    fn an_identical_pair_in_both_arms_emits_two_imports_of_its_own_path() {
        let import = || MirStmt::ForeignImport {
            bindings: vec![("numpy".to_string(), "numpy".to_string())],
        };
        let dir = pycc_scratch::ScratchDir::new("foreign_import_block_pair").expect("scratch");
        let mut ir = String::new();
        let mut observer = |module: &inkwell::module::Module<'_>, _: Option<&'static str>| {
            if module.get_function(EXT_MODULE_EXEC_SYMBOL).is_some() {
                ir = crate::llvm_string_to_owned(module.print_to_string());
            }
        };
        compile_to_object_with_observer(
            &MirModule {
                items: vec![MirItem::TopLevelStmt(MirStmt::If {
                    test: MirExpr::BoolLiteral(true),
                    body: vec![import()],
                    orelse: vec![import()],
                })],
                ..Default::default()
            },
            &dir.join("pair.o"),
            &CompileOptions {
                ext: true,
                ..CompileOptions::default()
            },
            Some(&mut observer),
        )
        .expect("ext codegen should succeed");
        let calls: Vec<&str> = ir
            .lines()
            .filter(|line| line.contains(&format!("call ptr @{EXT_OBJ_IMPORT_SYMBOL}(")))
            .collect();
        assert_eq!(calls.len(), 2, "{ir}");
        assert!(
            calls[0].ends_with("(ptr @pycc_foreign_module_numpy)"),
            "{ir}"
        );
        assert!(
            calls[1].ends_with("(ptr @pycc_foreign_module_numpy.1)"),
            "{ir}"
        );
        let globals: Vec<&str> = ir
            .lines()
            .filter(|line| line.starts_with("@pycc_foreign_module_numpy"))
            .collect();
        assert_eq!(globals.len(), 2, "{ir}");
        for global in globals {
            assert!(global.contains("c\"numpy\\00\""), "{global}");
        }
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
                    from: None,
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

    /// The whole-module LLVM text after compiling `items` as an `ext`
    /// object, verified by the same object emission `entry_ir` runs.
    fn module_ir(label: &str, items: Vec<MirItem>) -> String {
        let dir = pycc_scratch::ScratchDir::new(label).expect("scratch");
        let mut ir = String::new();
        let mut observer = |module: &inkwell::module::Module<'_>, _: Option<&'static str>| {
            if module.get_function(EXT_MODULE_EXEC_SYMBOL).is_some() {
                ir = crate::llvm_string_to_owned(module.print_to_string());
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

    /// The item `from itertools import product, chain` lowers to for
    /// `fromlist[index]`.
    fn from_item(index: usize) -> MirItem {
        let fromlist = vec!["product".to_string(), "chain".to_string()];
        MirItem::ForeignImport {
            local_name: fromlist[index].clone(),
            module_path: "itertools".to_string(),
            from: Some(pycc_mir::FromImport {
                name: fromlist[index].clone(),
                fromlist,
                index,
            }),
        }
    }

    /// #1278: each name of a from-import calls `pycc_ext_obj_import_from`
    /// with the module, the statement's whole fromlist, its length and the
    /// name's own index; the helper is declared once, the plain import
    /// helper not at all, and each result takes the usual failure edge and
    /// is stored to its own module global.
    #[test]
    fn a_from_import_passes_the_whole_fromlist_and_its_own_index() {
        let ir = module_ir("foreign_import_from", vec![from_item(0), from_item(1)]);
        let calls: Vec<&str> = ir
            .lines()
            .filter(|line| line.contains(&format!("call ptr @{EXT_OBJ_IMPORT_FROM_SYMBOL}(")))
            .collect();
        assert_eq!(calls.len(), 2, "{ir}");
        assert!(
            calls[0].ends_with(
                "(ptr @pycc_foreign_module_product, ptr @pycc_foreign_fromlist_product, \
                 i64 2, i64 0)"
            ),
            "{ir}"
        );
        assert!(
            calls[1].ends_with(
                "(ptr @pycc_foreign_module_chain, ptr @pycc_foreign_fromlist_chain, i64 2, i64 1)"
            ),
            "{ir}"
        );
        assert_eq!(
            ir.matches(&format!("declare ptr @{EXT_OBJ_IMPORT_FROM_SYMBOL}("))
                .count(),
            1,
            "{ir}"
        );
        assert!(!ir.contains(&format!("@{EXT_OBJ_IMPORT_SYMBOL}(")), "{ir}");
        let fromlist = ir
            .lines()
            .find(|line| line.starts_with("@pycc_foreign_fromlist_product "))
            .expect("the fromlist global");
        assert!(
            fromlist.contains("private constant [2 x ptr]"),
            "{fromlist}"
        );
        for name in ["product", "chain"] {
            assert!(
                ir.lines()
                    .any(|line| line.starts_with("@pycc_foreign_attr_")
                        && line.contains(&format!("c\"{name}\\00\""))),
                "the `{name}` string: {ir}"
            );
        }
        assert!(
            ir.lines()
                .any(|line| line.starts_with("@pycc_foreign_module_product ")
                    && line.contains("c\"itertools\\00\"")),
            "the module string names the module, not the name: {ir}"
        );
        assert_eq!(
            ir.lines()
                .filter(|line| line.starts_with("foreign_import_fail"))
                .count(),
            2,
            "one failure edge per name: {ir}"
        );
        assert!(
            ir.contains("store ptr %foreign_import, ptr @pyglobal_product"),
            "{ir}"
        );
        assert!(ir.contains("@pyglobal_chain"), "{ir}");
    }
}
