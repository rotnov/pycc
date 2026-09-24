//! #1313: a direct call of a CPython object, `MirExpr::ObjCall`.
//!
//! Kept out of the parent file, which is already past the ~1,000-line
//! threshold; `use super::*` reaches its private `entry_ir` helper.

use super::*;

/// `from itertools import product` followed by one discarded
/// `product(args)` call.
fn direct_call(args: Vec<MirExpr>) -> Vec<MirItem> {
    vec![
        MirItem::ForeignImport {
            local_name: "product".to_string(),
            module_path: "itertools".to_string(),
            from: Some(pycc_mir::FromImport {
                name: "product".to_string(),
                fromlist: vec!["product".to_string()],
                index: 0,
            }),
        },
        MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::ObjCall {
            callee: Box::new(MirExpr::Name {
                name: "product".to_string(),
                ty: Ty::Object,
            }),
            args,
        })),
    ]
}

/// The callee is the loaded module global itself, handed to the borrowing
/// entry point: no method lookup is emitted, and the consuming
/// `pycc_ext_obj_call` (whose name is a prefix of the borrowing one, hence
/// the `(` in the pattern) is never called, since it would steal the
/// global's only reference.
#[test]
fn a_direct_call_reaches_the_borrowing_shim_with_the_loaded_global() {
    let ir = entry_ir("foreign_direct_call_zero_arg", direct_call(Vec::new()));
    assert!(
        ir.contains(&format!("@{EXT_OBJ_CALL_BORROWED_SYMBOL}(")),
        "{ir}"
    );
    assert!(!ir.contains(&format!("@{EXT_OBJ_CALL_SYMBOL}(")), "{ir}");
    assert!(!ir.contains(EXT_OBJ_GETATTR_SYMBOL), "{ir}");
    assert!(!ir.contains(EXT_OBJ_PACK_INT_SYMBOL), "{ir}");
    assert!(
        ir.contains("i64 0"),
        "the arity is passed as a constant: {ir}"
    );
    assert!(
        ir.contains("%load = load ptr, ptr @pyglobal_product"),
        "the callee is the module global: {ir}"
    );
    assert!(
        ir.contains(&format!("@{EXT_OBJ_CALL_BORROWED_SYMBOL}(ptr %load,")),
        "the loaded global is the call's first operand: {ir}"
    );
}

/// Each admitted scalar argument is packed into the argument array before
/// the call, in order.
#[test]
fn a_direct_call_packs_each_argument() {
    let ir = entry_ir(
        "foreign_direct_call_args",
        direct_call(vec![
            MirExpr::IntLiteral(7),
            MirExpr::StringLiteral("ab".to_string()),
        ]),
    );
    assert!(ir.contains(EXT_OBJ_PACK_INT_SYMBOL), "{ir}");
    assert!(ir.contains(EXT_OBJ_PACK_STR_SYMBOL), "{ir}");
    assert!(ir.contains("foreign_call_arg_slot"), "{ir}");
    assert!(ir.contains("i64 2"), "the arity is two: {ir}");
    let pack_at = ir.find(EXT_OBJ_PACK_STR_SYMBOL).expect("asserted above");
    let call_at = ir
        .find(&format!("call ptr @{EXT_OBJ_CALL_BORROWED_SYMBOL}("))
        .unwrap_or_else(|| panic!("no borrowing call was emitted: {ir}"));
    assert!(pack_at < call_at, "{ir}");
}

/// A `NULL` result stops the module body on the module-exec failure edge,
/// as every other foreign producer does.
#[test]
fn a_failed_direct_call_returns_on_the_module_exec_failure_edge() {
    let ir = entry_ir(
        "foreign_direct_call_fail_edge",
        direct_call(vec![MirExpr::IntLiteral(1)]),
    );
    assert!(ir.contains("foreign_call_failed"), "{ir}");
    assert!(ir.contains("foreign_call_fail:"), "{ir}");
    assert!(ir.contains("foreign_call_cont:"), "{ir}");
    assert!(
        ir.contains(&format!("ret i64 {EXT_MODULE_EXEC_FAILED}")),
        "{ir}"
    );
}

/// A direct call runs arbitrary CPython code, so it can leave an exception
/// set.
#[test]
fn a_direct_call_can_set_an_exception() {
    let node = MirExpr::ObjCall {
        callee: Box::new(MirExpr::Name {
            name: "product".to_string(),
            ty: Ty::Object,
        }),
        args: Vec::new(),
    };
    assert!(crate::exception::expression_can_set_exception(&node));
}

/// The defensive arm in `foreign_attr::expect_object_pointer`, reached
/// through a direct call: `pycc_mir` builds `ObjCall` only over a
/// `Ty::Object` callee. The panic text is shared by every foreign-object
/// operation and deliberately names no node.
#[test]
#[should_panic(expected = "did not evaluate to a CPython object")]
fn a_non_object_callee_is_an_internal_error() {
    entry_ir(
        "foreign_direct_call_bad_callee",
        vec![MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::ObjCall {
            callee: Box::new(MirExpr::IntLiteral(1)),
            args: Vec::new(),
        }))],
    );
}
