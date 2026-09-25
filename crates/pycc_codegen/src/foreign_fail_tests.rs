//! #1316: a foreign `object` operation inside a function body.
//!
//! Every test compiles real MIR as an `ext` object -- LLVM's verifier runs
//! before any assertion here is believed -- and reads the LLVM text of the
//! user function `f` (mangled `pyfn_f`). The module-exec edge keeps its own
//! tests in `foreign_attr.rs`, `foreign_call.rs` and `foreign_len.rs`,
//! which pin that it is emitted unchanged.

use crate::{
    CompileOptions, EXT_MODULE_EXEC_SYMBOL, EXT_NAME_ERROR_SYMBOL, EXT_OBJ_ERROR_BRIDGE_SYMBOL,
    compile_to_object_with_observer,
};
use inkwell::values::AnyValue;
use pycc_mir::{BinOpKind, MirExceptHandler, MirExpr, MirItem, MirModule, MirStmt, Ty};

/// `import copy` -- the foreign module global every test reads.
fn import_copy() -> MirItem {
    MirItem::ForeignImport {
        local_name: "copy".to_string(),
        module_path: "copy".to_string(),
        from: None,
    }
}

fn copy_name() -> MirExpr {
    MirExpr::Name {
        name: "copy".to_string(),
        ty: Ty::Object,
    }
}

fn copy_boxed() -> Box<MirExpr> {
    Box::new(copy_name())
}

/// `def f(<params>) -> <return_ty>: <body>`, ahead of any `extra` items.
fn function(params: Vec<(String, Ty)>, return_ty: Ty, body: Vec<MirStmt>) -> MirItem {
    MirItem::Function {
        name: "f".to_string(),
        params,
        return_ty,
        body,
    }
}

/// The LLVM text of the named functions after compiling `items` as an
/// `ext` object, in the order asked for.
fn functions_ir(label: &str, items: Vec<MirItem>, names: &[&str]) -> Vec<String> {
    let dir = pycc_scratch::ScratchDir::new(label).expect("failed to create scratch dir");
    let mut irs = vec![String::new(); names.len()];
    let mut observer = |module: &inkwell::module::Module<'_>, _: Option<&'static str>| {
        for (slot, name) in irs.iter_mut().zip(names) {
            if let Some(function) = module.get_function(name) {
                *slot = crate::llvm_string_to_owned(function.print_to_string());
            }
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
    for (ir, name) in irs.iter().zip(names) {
        assert!(!ir.is_empty(), "no {name} was emitted");
    }
    irs
}

/// The LLVM text of `f` after compiling `import copy` plus `items`.
fn f_ir(label: &str, items: Vec<MirItem>) -> String {
    let mut all = vec![import_copy()];
    all.extend(items);
    functions_ir(label, all, &["pyfn_f"]).remove(0)
}

/// `f`, with one statement discarding `expr`.
fn discarding(expr: MirExpr) -> Vec<MirItem> {
    vec![function(
        Vec::new(),
        Ty::None,
        vec![MirStmt::ExprStmt(expr), MirStmt::Return(None)],
    )]
}

/// The body of the block labelled `label` in `ir`, up to its terminator.
fn block<'a>(ir: &'a str, label: &str) -> &'a str {
    let header = format!("\n{label}:");
    let start = ir
        .find(&header)
        .unwrap_or_else(|| panic!("no `{label}` block:\n{ir}"))
        + header.len();
    let rest = &ir[start..];
    let end = rest.find("\n\n").unwrap_or(rest.len());
    &rest[..end]
}

/// The `{label}_fail` block bridges CPython's exception and branches to
/// `f`'s own `exception_exit`, and nothing in `f` returns the module-exec
/// status.
fn assert_bridged_to_exit(ir: &str, label: &str) {
    let fail = block(ir, &format!("{label}_fail"));
    assert!(
        fail.contains(&format!("call i32 @{EXT_OBJ_ERROR_BRIDGE_SYMBOL}()")),
        "{label}_fail must bridge CPython's exception:\n{ir}"
    );
    assert!(
        fail.trim_end().ends_with("br label %exception_exit"),
        "{label}_fail must branch to the function's exception exit:\n{ir}"
    );
    assert!(!ir.contains("ret i64 -1"), "{ir}");
}

#[test]
fn a_foreign_attribute_load_in_a_function_bridges_to_the_exception_exit() {
    let ir = f_ir(
        "foreign_fail_attr",
        discarding(MirExpr::ObjAttrGet {
            base: copy_boxed(),
            attr: "deepcopy".to_string(),
            ty: Ty::Object,
        }),
    );
    assert_bridged_to_exit(&ir, "foreign_attr");
}

/// A method call has two failure points -- the lookup and the call -- and
/// each takes the function edge.
#[test]
fn a_foreign_method_call_in_a_function_bridges_both_of_its_failure_points() {
    let ir = f_ir(
        "foreign_fail_method",
        discarding(MirExpr::ObjMethodCall {
            base: copy_boxed(),
            method: "copy".to_string(),
            args: vec![MirExpr::IntLiteral(1)],
        }),
    );
    assert_bridged_to_exit(&ir, "foreign_call_lookup");
    assert_bridged_to_exit(&ir, "foreign_call");
}

#[test]
fn a_direct_foreign_call_in_a_function_bridges_to_the_exception_exit() {
    let ir = f_ir(
        "foreign_fail_direct_call",
        discarding(MirExpr::ObjCall {
            callee: copy_boxed(),
            args: Vec::new(),
        }),
    );
    assert_bridged_to_exit(&ir, "foreign_call");
}

#[test]
fn a_foreign_subscript_in_a_function_bridges_to_the_exception_exit() {
    let ir = f_ir(
        "foreign_fail_subscript",
        discarding(MirExpr::ObjSubscript {
            base: copy_boxed(),
            index: Box::new(MirExpr::IntLiteral(0)),
        }),
    );
    assert_bridged_to_exit(&ir, "foreign_subscript");
}

#[test]
fn a_foreign_len_in_a_function_bridges_to_the_exception_exit() {
    let ir = f_ir(
        "foreign_fail_len",
        discarding(MirExpr::ObjLen { base: copy_boxed() }),
    );
    assert_bridged_to_exit(&ir, "foreign_len");
}

/// The four explicit conversions share one emitter shape each; every one
/// takes the function edge.
#[test]
fn each_foreign_conversion_in_a_function_bridges_to_the_exception_exit() {
    for (callee, ty, label) in [
        ("float", Ty::Float, "foreign_to_float"),
        ("int", Ty::Int, "foreign_to_int"),
        ("str", Ty::Str, "foreign_to_str"),
        ("bool", Ty::Bool, "foreign_truthy"),
    ] {
        let ir = f_ir(
            &format!("foreign_fail_conv_{callee}"),
            discarding(MirExpr::Call {
                callee: callee.to_string(),
                args: vec![copy_name()],
                ty,
            }),
        );
        assert_bridged_to_exit(&ir, label);
    }
}

/// A condition-position truth test has no expression guard after it at
/// all, which is one of the two reasons the branch is immediate.
#[test]
fn a_foreign_condition_in_a_function_bridges_to_the_exception_exit() {
    let ir = f_ir(
        "foreign_fail_if",
        vec![function(
            Vec::new(),
            Ty::None,
            vec![
                MirStmt::If {
                    test: copy_name(),
                    body: vec![MirStmt::Return(None)],
                    orelse: Vec::new(),
                },
                MirStmt::Return(None),
            ],
        )],
    );
    assert_bridged_to_exit(&ir, "foreign_truthy");
}

/// Inside a `try`, the innermost exception target is the handler dispatch,
/// not the function's exit: the foreign failure is catchable.
#[test]
fn a_foreign_failure_inside_a_try_branches_to_the_handler_not_the_exit() {
    let ir = f_ir(
        "foreign_fail_try",
        vec![function(
            Vec::new(),
            Ty::None,
            vec![
                MirStmt::Try {
                    body: vec![MirStmt::ExprStmt(MirExpr::ObjAttrGet {
                        base: copy_boxed(),
                        attr: "missing".to_string(),
                        ty: Ty::Object,
                    })],
                    handlers: vec![MirExceptHandler {
                        exc_type_tag: None,
                        binding_name: None,
                        binding_ty: None,
                        body: vec![MirStmt::Return(None)],
                    }],
                    orelse: Vec::new(),
                    finalbody: Vec::new(),
                },
                MirStmt::Return(None),
            ],
        )],
    );
    let fail = block(&ir, "foreign_attr_fail");
    assert!(
        fail.contains(&format!("call i32 @{EXT_OBJ_ERROR_BRIDGE_SYMBOL}()")),
        "{ir}"
    );
    let terminator = fail.trim_end().lines().last().unwrap_or_default().trim();
    assert!(terminator.starts_with("br label %"), "{ir}");
    assert_ne!(terminator, "br label %exception_exit", "{ir}");
}

/// A live bigint temporary -- `a * b`, evaluated before the foreign `len`
/// on the right of `+` -- is released on the foreign failure edge before
/// the branch, exactly as `guard_statement_effects` releases it.
#[test]
fn a_foreign_failure_releases_a_pending_bigint_temporary_before_branching() {
    let ir = f_ir(
        "foreign_fail_bigint",
        vec![function(
            vec![("a".to_string(), Ty::Int), ("b".to_string(), Ty::Int)],
            Ty::Int,
            vec![MirStmt::Return(Some(MirExpr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(MirExpr::BinOp {
                    op: BinOpKind::Mul,
                    left: Box::new(MirExpr::Name {
                        name: "a".to_string(),
                        ty: Ty::Int,
                    }),
                    right: Box::new(MirExpr::Name {
                        name: "b".to_string(),
                        ty: Ty::Int,
                    }),
                    ty: Ty::Int,
                }),
                right: Box::new(MirExpr::ObjLen { base: copy_boxed() }),
                ty: Ty::Int,
            }))],
        )],
    );
    // The release is a null-checked call, so the unwind spans the fail
    // block and the release blocks it falls into, up to the one branch to
    // the exit.
    let start = ir
        .find("\nforeign_len_fail:")
        .unwrap_or_else(|| panic!("no foreign_len_fail block:\n{ir}"));
    let unwind = &ir[start..];
    let unwind = &unwind[..unwind
        .find("br label %exception_exit")
        .unwrap_or_else(|| panic!("the unwind must reach the exception exit:\n{ir}"))];
    let bridge = unwind
        .find(&format!("call i32 @{EXT_OBJ_ERROR_BRIDGE_SYMBOL}()"))
        .unwrap_or_else(|| panic!("the failure must be bridged:\n{ir}"));
    let release = unwind
        .find("call void @pycc_rt_bigint_release")
        .unwrap_or_else(|| panic!("the pending temporary must be released:\n{ir}"));
    assert!(bridge < release, "bridge first, then unwind:\n{ir}");
    assert!(!ir.contains("ret i64 -1"), "{ir}");
}

/// A function body reads a foreign global through its initialized flag,
/// because D-041 cannot prove call order; the unbound branch raises a
/// catchable `NameError` naming the local binding.
#[test]
fn an_unbound_foreign_global_read_in_a_function_raises_name_error() {
    let ir = f_ir(
        "foreign_fail_name_error",
        discarding(MirExpr::ObjAttrGet {
            base: copy_boxed(),
            attr: "deepcopy".to_string(),
            ty: Ty::Object,
        }),
    );
    let unbound = block(&ir, "global_unbound");
    assert!(
        unbound.contains(&format!(
            "@{EXT_NAME_ERROR_SYMBOL}(ptr @pycc_foreign_name_copy, i64 4)"
        )),
        "{ir}"
    );
    assert!(
        unbound.trim_end().ends_with("br label %exception_exit"),
        "{ir}"
    );
    assert!(!unbound.contains("llvm.trap"), "{ir}");
}

/// Only a foreign `object` read changes: an unbound `int` global read in a
/// function keeps its `llvm.trap`, and so does a module-body read, which
/// D-041 proves bound (it emits no flag check at all there).
#[test]
fn a_non_object_unbound_read_keeps_its_trap_and_the_module_body_is_unchanged() {
    let items = vec![
        import_copy(),
        MirItem::TopLevelStmt(MirStmt::Assign {
            target: "n".to_string(),
            value: MirExpr::IntLiteral(3),
        }),
        function(
            Vec::new(),
            Ty::Int,
            vec![MirStmt::Return(Some(MirExpr::Name {
                name: "n".to_string(),
                ty: Ty::Int,
            }))],
        ),
        MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::ObjAttrGet {
            base: copy_boxed(),
            attr: "deepcopy".to_string(),
            ty: Ty::Object,
        })),
    ];
    let irs = functions_ir(
        "foreign_fail_trap_kept",
        items,
        &["pyfn_f", EXT_MODULE_EXEC_SYMBOL],
    );
    let (f, entry) = (&irs[0], &irs[1]);
    let unbound = block(f, "global_unbound");
    assert!(unbound.contains("llvm.trap"), "{f}");
    assert!(!f.contains(EXT_NAME_ERROR_SYMBOL), "{f}");
    assert!(!entry.contains(EXT_NAME_ERROR_SYMBOL), "{entry}");
    assert!(!entry.contains(EXT_OBJ_ERROR_BRIDGE_SYMBOL), "{entry}");
    assert!(entry.contains("ret i64 -1"), "{entry}");
}
