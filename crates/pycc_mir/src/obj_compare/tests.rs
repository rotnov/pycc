//! Lowering of comparisons and `isinstance` with a CPython object operand
//! (Part 1 of #1371), and of membership in and slices of one (Part 2b),
//! exercised from real HIR with `numpy` bound as a foreign import.

use crate::*;
use pycc_diag::Span;
use pycc_hir::{CmpOpKind, HirExpr, HirItem, HirModule, HirStmt, ImportBinding};

fn numpy() -> HirExpr {
    HirExpr::Name("numpy".to_string())
}

fn numpy_attr(attr: &str) -> HirExpr {
    HirExpr::AttrGet {
        base: Box::new(numpy()),
        attr: attr.to_string(),
    }
}

/// The lowered form of `expr` evaluated and discarded as the single
/// statement of a module that imports `numpy` first.
fn lower_discarded(expr: HirExpr) -> MirExpr {
    let hir = HirModule {
        items: vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(expr))],
        imports: vec![ImportBinding::Foreign {
            local_name: "numpy".to_string(),
            module_path: "numpy".to_string(),
            from: None,
            site: pycc_hir::ForeignImportSite::Item(0),
            span: Span::new(0, 0),
        }],
        seeded_builtin_exception_classes: false,
        type_aliases: Vec::new(),
        class_defs: Vec::new(),
    };
    build(&hir)
        .items
        .into_iter()
        .find_map(|item| match item {
            MirItem::TopLevelStmt(MirStmt::ExprStmt(expr)) => Some(expr),
            _ => None,
        })
        .expect("the module has exactly one top-level statement")
}

fn compare(op: CmpOpKind, left: HirExpr, right: HirExpr) -> HirExpr {
    HirExpr::Compare {
        op,
        left: Box::new(left),
        right: Box::new(right),
    }
}

fn isinstance(value: HirExpr, class: HirExpr) -> HirExpr {
    HirExpr::Call {
        callee: "isinstance".to_string(),
        args: vec![value, class],
    }
}

#[test]
fn a_rich_comparison_with_an_object_operand_is_an_object_valued_obj_compare() {
    for (left, right) in [
        (numpy_attr("pi"), HirExpr::IntLiteral(1)),
        (HirExpr::FloatLiteral(1.5), numpy_attr("pi")),
    ] {
        let lowered = lower_discarded(compare(CmpOpKind::Lt, left, right));
        assert!(
            matches!(
                lowered,
                MirExpr::ObjCompare {
                    op: CmpOpKind::Lt,
                    ..
                }
            ),
            "{lowered:?}"
        );
        assert_eq!(lowered.ty(), Ty::Object);
    }
}

#[test]
fn an_identity_test_on_an_object_is_a_bool_valued_obj_compare() {
    for op in [CmpOpKind::Is, CmpOpKind::IsNot] {
        let lowered = lower_discarded(compare(op, numpy_attr("pi"), HirExpr::NoneLiteral));
        let MirExpr::ObjCompare { right, .. } = &lowered else {
            panic!("expected an `ObjCompare`: {lowered:?}");
        };
        assert_eq!(**right, MirExpr::NoneLiteral);
        assert_eq!(lowered.ty(), Ty::Bool);
    }
}

#[test]
fn a_native_comparison_keeps_its_native_node() {
    let lowered = lower_discarded(compare(
        CmpOpKind::Eq,
        HirExpr::IntLiteral(1),
        HirExpr::IntLiteral(2),
    ));
    assert!(matches!(lowered, MirExpr::Compare { .. }), "{lowered:?}");
}

#[test]
fn isinstance_on_an_object_is_a_run_time_test_never_a_folded_constant() {
    let lowered = lower_discarded(isinstance(
        numpy_attr("pi"),
        HirExpr::Name("float".to_string()),
    ));
    assert!(
        matches!(
            lowered,
            MirExpr::ObjIsInstance {
                class: ObjIsInstanceClass::Builtin(ObjBuiltinClass::Float),
                ..
            }
        ),
        "{lowered:?}"
    );
    assert_eq!(lowered.ty(), Ty::Bool);

    let lowered = lower_discarded(isinstance(numpy_attr("pi"), numpy_attr("ndarray")));
    let MirExpr::ObjIsInstance {
        class: ObjIsInstanceClass::Object(class),
        ..
    } = &lowered
    else {
        panic!("expected an object-class `ObjIsInstance`: {lowered:?}");
    };
    assert!(matches!(**class, MirExpr::ObjAttrGet { ref attr, .. } if attr == "ndarray"));
}

#[test]
fn every_builtin_class_name_maps_to_its_own_shim_selector() {
    let codes: Vec<u64> = ["int", "str", "float", "bool"]
        .into_iter()
        .map(|name| ObjBuiltinClass::from_name(name).expect(name).shim_code())
        .collect();
    assert_eq!(codes, [0, 1, 2, 3]);
    assert_eq!(ObjBuiltinClass::from_name("list"), None);
}

/// A walrus can hide in either operand of a comparison, and in either
/// argument of `isinstance`; a binding missed there is a local codegen
/// never allocates storage for.
#[test]
fn named_expr_bindings_are_collected_from_every_operand() {
    let walrus = |name: &str, value: HirExpr| HirExpr::NamedExpr {
        name: name.to_string(),
        value: Box::new(value),
    };
    let mut out = Vec::new();
    lower_discarded(compare(
        CmpOpKind::Eq,
        walrus("a", numpy_attr("pi")),
        walrus("b", HirExpr::IntLiteral(1)),
    ))
    .collect_named_expr_bindings(&mut out);
    lower_discarded(isinstance(
        walrus("c", numpy_attr("pi")),
        walrus("d", numpy_attr("ndarray")),
    ))
    .collect_named_expr_bindings(&mut out);
    lower_discarded(isinstance(
        walrus("e", numpy_attr("pi")),
        HirExpr::Name("int".to_string()),
    ))
    .collect_named_expr_bindings(&mut out);
    let names: Vec<&str> = out.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, ["a", "b", "c", "d", "e"]);
}

fn slice(
    base: HirExpr,
    start: Option<HirExpr>,
    stop: Option<HirExpr>,
    step: Option<HirExpr>,
) -> HirExpr {
    HirExpr::Slice {
        base: Box::new(base),
        start: start.map(Box::new),
        stop: stop.map(Box::new),
        step: step.map(Box::new),
    }
}

/// Part 2b of #1371: membership in an object container is its own
/// bool-valued node, item first, with `not in` as `negate`.
#[test]
fn membership_in_an_object_container_is_a_bool_valued_obj_contains() {
    for (op, expected_negate) in [(CmpOpKind::In, false), (CmpOpKind::NotIn, true)] {
        let lowered = lower_discarded(compare(op, HirExpr::IntLiteral(1), numpy_attr("pi")));
        let MirExpr::ObjContains {
            negate,
            item,
            container,
        } = &lowered
        else {
            panic!("expected an `ObjContains`: {lowered:?}");
        };
        assert_eq!(*negate, expected_negate);
        assert_eq!(**item, MirExpr::IntLiteral(1));
        assert!(matches!(**container, MirExpr::ObjAttrGet { .. }));
        assert_eq!(lowered.ty(), Ty::Bool);
    }
}

/// Part 2b of #1371: a slice of an object is its own object-valued node,
/// whatever bounds are present; a native base keeps `MirExpr::Slice`.
#[test]
fn a_slice_of_an_object_is_an_object_valued_obj_slice() {
    let lowered = lower_discarded(slice(
        numpy_attr("pi"),
        Some(HirExpr::IntLiteral(1)),
        None,
        Some(HirExpr::IntLiteral(2)),
    ));
    let MirExpr::ObjSlice {
        start, stop, step, ..
    } = &lowered
    else {
        panic!("expected an `ObjSlice`: {lowered:?}");
    };
    assert_eq!(start.as_deref(), Some(&MirExpr::IntLiteral(1)));
    assert_eq!(stop.as_deref(), None);
    assert_eq!(step.as_deref(), Some(&MirExpr::IntLiteral(2)));
    assert_eq!(lowered.ty(), Ty::Object);

    let native = lower_discarded(slice(
        HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1)]),
        None,
        Some(HirExpr::IntLiteral(1)),
        None,
    ));
    assert!(matches!(native, MirExpr::Slice { .. }), "{native:?}");
}

/// A walrus can hide in either operand of a membership test and in the
/// base or any bound of an object slice.
#[test]
fn named_expr_bindings_are_collected_from_membership_and_slice_operands() {
    let walrus = |name: &str, value: HirExpr| HirExpr::NamedExpr {
        name: name.to_string(),
        value: Box::new(value),
    };
    let mut out = Vec::new();
    lower_discarded(compare(
        CmpOpKind::In,
        walrus("a", HirExpr::IntLiteral(1)),
        walrus("b", numpy_attr("pi")),
    ))
    .collect_named_expr_bindings(&mut out);
    lower_discarded(slice(
        walrus("c", numpy_attr("pi")),
        Some(walrus("d", HirExpr::IntLiteral(1))),
        Some(walrus("e", HirExpr::IntLiteral(2))),
        Some(walrus("f", HirExpr::IntLiteral(1))),
    ))
    .collect_named_expr_bindings(&mut out);
    let names: Vec<&str> = out.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, ["a", "b", "c", "d", "e", "f"]);
}

/// Part 2d of #1371: a list display the empty-container pre-pass resolved
/// to an object slot lowers to its own object-valued node, elements in
/// source order, and a walrus inside an element still binds its target.
#[test]
fn an_object_list_display_is_an_object_valued_obj_list() {
    let lowered = lower_discarded(HirExpr::ObjectList(vec![
        HirExpr::IntLiteral(1),
        HirExpr::StringLiteral("a".to_string()),
        numpy_attr("pi"),
    ]));
    let MirExpr::ObjList { elements } = &lowered else {
        panic!("expected an `ObjList`: {lowered:?}");
    };
    assert_eq!(elements.len(), 3);
    assert_eq!(elements[0], MirExpr::IntLiteral(1));
    assert!(matches!(elements[2], MirExpr::ObjAttrGet { .. }));
    assert_eq!(lowered.ty(), Ty::Object);
    assert_eq!(
        lower_discarded(HirExpr::ObjectList(Vec::new())).ty(),
        Ty::Object
    );

    let mut out = Vec::new();
    lower_discarded(HirExpr::ObjectList(vec![HirExpr::NamedExpr {
        name: "n".to_string(),
        value: Box::new(HirExpr::IntLiteral(3)),
    }]))
    .collect_named_expr_bindings(&mut out);
    assert_eq!(out, [("n".to_string(), Ty::Int)]);
}
