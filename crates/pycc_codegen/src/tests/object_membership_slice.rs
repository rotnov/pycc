//! Part 2b of #1371: `MirExpr::ObjContains` (`foreign_compare.rs`) and
//! `MirExpr::ObjSlice` (`foreign_call.rs`) emission, pinned on the LLVM IR
//! of an `ext` object.

use super::*;
use pycc_mir::CmpOpKind;

fn contains(negate: bool, item: MirExpr, container: MirExpr) -> MirExpr {
    MirExpr::ObjContains {
        negate,
        item: Box::new(item),
        container: Box::new(container),
    }
}

fn obj_slice(start: Option<MirExpr>, stop: Option<MirExpr>, step: Option<MirExpr>) -> MirExpr {
    MirExpr::ObjSlice {
        base: Box::new(numpy_pi()),
        start: start.map(Box::new),
        stop: stop.map(Box::new),
        step: step.map(Box::new),
    }
}

/// Compiles `exprs` as discarded module-body statements and hands the IR
/// to `check`.
fn module_ir(label: &str, exprs: Vec<MirExpr>, check: impl Fn(&str)) {
    compile_ext_items_checking_ir(
        label,
        with_foreign_numpy(
            exprs
                .into_iter()
                .map(|expr| MirItem::TopLevelStmt(MirStmt::ExprStmt(expr)))
                .collect(),
        ),
        check,
    );
}

/// Every packable item is boxed by its own packer (an object item by
/// `pack_object`), the container is passed borrowed, a negative answer
/// takes the failure edge, and `not in` is the same call negated.
#[test]
fn membership_is_one_contains_call_per_test_with_a_packed_item() {
    module_ir(
        "obj_contains",
        vec![
            contains(false, MirExpr::IntLiteral(1), numpy_pi()),
            contains(true, MirExpr::FloatLiteral(1.5), numpy_pi()),
            contains(false, MirExpr::BoolLiteral(true), numpy_pi()),
            contains(true, MirExpr::StringLiteral("k".to_string()), numpy_pi()),
            contains(false, numpy_pi(), numpy_pi()),
        ],
        |ir| {
            let calls = ir
                .lines()
                .filter(|line| line.contains("call i32 @pycc_ext_obj_contains("))
                .count();
            assert_eq!(calls, 5, "{ir}");
            for packer in [
                "@pycc_ext_obj_pack_int(",
                "@pycc_ext_obj_pack_float(",
                "@pycc_ext_obj_pack_bool(",
                "@pycc_ext_obj_pack_str(",
                "@pycc_ext_obj_pack_object(",
            ] {
                assert!(ir.contains(packer), "{packer}: {ir}");
            }
            assert!(ir.contains("foreign_contains_failed"), "{ir}");
            assert!(ir.contains("xor i8"), "{ir}");
            assert!(!ir.contains("DecRef"), "{ir}");
        },
    );
}

/// Each bound shape passes its own `present` mask -- bit 0 start, bit 1
/// stop, bit 2 step -- with `ptr null` for an absent bound, and a `NULL`
/// result takes the failure edge.
#[test]
fn a_slice_is_one_getslice_call_with_a_present_mask_per_bound_shape() {
    module_ir(
        "obj_slice",
        vec![
            obj_slice(None, None, None),
            obj_slice(Some(MirExpr::IntLiteral(1)), None, None),
            obj_slice(None, Some(MirExpr::IntLiteral(-2)), None),
            obj_slice(
                Some(MirExpr::IntLiteral(1)),
                None,
                Some(MirExpr::IntLiteral(2)),
            ),
            obj_slice(
                Some(numpy_pi()),
                Some(MirExpr::StringLiteral("a".to_string())),
                Some(MirExpr::FloatLiteral(1.5)),
            ),
        ],
        |ir| {
            for present in [0, 1, 2, 5, 7] {
                let needle = format!("i32 {present})");
                assert!(
                    ir.lines()
                        .any(|line| line.contains("@pycc_ext_obj_getslice(")
                            && line.contains(&needle)),
                    "{needle}: {ir}"
                );
            }
            assert!(
                ir.lines()
                    .any(|line| line.contains("@pycc_ext_obj_getslice(")
                        && line.contains("ptr null, ptr null, ptr null, i32 0)")),
                "{ir}"
            );
            assert!(ir.contains("foreign_slice_failed"), "{ir}");
            assert!(!ir.contains("DecRef"), "{ir}");
        },
    );
}

/// In a function body both nodes bridge their failure, and an `int`
/// temporary item or bound is protected across the later operands and
/// released afterwards.
#[test]
fn a_function_body_membership_or_slice_bridges_its_failure_with_int_temporaries() {
    let sum = || MirExpr::BinOp {
        op: pycc_mir::BinOpKind::Add,
        left: Box::new(MirExpr::IntLiteral(1)),
        right: Box::new(MirExpr::IntLiteral(2)),
        ty: Ty::Int,
    };
    compile_ext_items_checking_ir(
        "obj_contains_slice_function",
        with_foreign_numpy(vec![MirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![
                MirStmt::ExprStmt(contains(true, sum(), numpy_pi())),
                MirStmt::ExprStmt(obj_slice(Some(sum()), Some(sum()), None)),
                MirStmt::Return(None),
            ],
        }]),
        |ir| {
            assert!(ir.contains("@pycc_ext_obj_contains("), "{ir}");
            assert!(ir.contains("@pycc_ext_obj_getslice("), "{ir}");
            assert!(ir.contains("@pycc_ext_obj_error_bridge("), "{ir}");
        },
    );
}

/// `pycc_types` admits membership only in a CPython object, so a native
/// `Compare` with `in` is an internal error.
#[test]
#[should_panic(expected = "a native `in`/`not in` reached codegen")]
fn a_native_membership_compare_is_an_internal_error() {
    module_ir(
        "native_in",
        vec![MirExpr::Compare {
            op: CmpOpKind::In,
            left: Box::new(MirExpr::IntLiteral(1)),
            right: Box::new(MirExpr::IntLiteral(2)),
            ty: Ty::Bool,
        }],
        |_| {},
    );
}

/// `pycc_mir` lowers object membership to `ObjContains`, so an
/// `ObjCompare` carrying `in` is an internal error, never an identity test.
#[test]
#[should_panic(expected = "a membership test reached `ObjCompare`")]
fn an_obj_compare_carrying_membership_is_an_internal_error() {
    module_ir(
        "obj_compare_in",
        vec![MirExpr::ObjCompare {
            op: CmpOpKind::NotIn,
            left: Box::new(MirExpr::IntLiteral(1)),
            right: Box::new(numpy_pi()),
        }],
        |_| {},
    );
}
