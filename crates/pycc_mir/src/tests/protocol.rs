//! `runtime_checkable` protocol evaluation (`class::eval_isinstance_protocol`).
//!
//! Covers method and attribute members, resolution through a property and
//! through the MRO, every false result, and protocol-typed parameters and
//! annotated assignments.

use crate::*;
use pycc_hir::{HirClassDef, HirExpr, HirItem, HirModule, HirStmt, Ty};

// -- #380: eval_isinstance_protocol in compile-time isinstance -------

#[test]
fn isinstance_with_runtime_checkable_protocol_evaluates_to_true() {
    use pycc_hir::{HirClassDef, ProtocolMember};
    let proto_def = HirClassDef {
        name: "Drawable".to_string(),
        bases: Vec::new(),
        mro: vec!["Drawable".to_string()],
        attrs: Vec::new(),
        methods: Vec::new(),
        type_param: None,
        properties: Vec::new(),
        static_methods: Vec::new(),
        class_methods: Vec::new(),
        enum_members: Vec::new(),
        is_dataclass: false,
        dataclass_fields: Vec::new(),
        is_protocol: true,
        runtime_checkable: true,
        protocol_members: vec![ProtocolMember::Method {
            name: "draw".to_string(),
            param_tys: vec![],
            return_ty: Ty::None,
        }],
        abstract_methods: Vec::new(),
        is_abstract: false,
    };
    let class_def = HirClassDef {
        name: "Circle".to_string(),
        bases: Vec::new(),
        mro: vec!["Circle".to_string()],
        attrs: Vec::new(),
        methods: vec![("draw".to_string(), "Circle.draw".to_string())],
        type_param: None,
        properties: Vec::new(),
        static_methods: Vec::new(),
        class_methods: Vec::new(),
        enum_members: Vec::new(),
        is_dataclass: false,
        dataclass_fields: Vec::new(),
        is_protocol: false,
        runtime_checkable: false,
        protocol_members: Vec::new(),
        abstract_methods: Vec::new(),
        is_abstract: false,
    };
    let classes: HashMap<String, HirClassDef> = [
        ("Drawable".to_string(), proto_def),
        ("Circle".to_string(), class_def),
    ]
    .into_iter()
    .collect();
    let obj_ty = Ty::Instance(Box::new("Circle".to_string()));
    let proto = classes.get("Drawable").unwrap();
    assert!(eval_isinstance_protocol(&obj_ty, proto, &classes));
}

#[test]
fn isinstance_with_runtime_checkable_protocol_evaluates_to_false_for_missing_method() {
    use pycc_hir::{HirClassDef, ProtocolMember};
    let proto_def = HirClassDef {
        name: "Drawable".to_string(),
        bases: Vec::new(),
        mro: vec!["Drawable".to_string()],
        attrs: Vec::new(),
        methods: Vec::new(),
        type_param: None,
        properties: Vec::new(),
        static_methods: Vec::new(),
        class_methods: Vec::new(),
        enum_members: Vec::new(),
        is_dataclass: false,
        dataclass_fields: Vec::new(),
        is_protocol: true,
        runtime_checkable: true,
        protocol_members: vec![ProtocolMember::Method {
            name: "draw".to_string(),
            param_tys: vec![],
            return_ty: Ty::None,
        }],
        abstract_methods: Vec::new(),
        is_abstract: false,
    };
    let class_def = HirClassDef {
        name: "Circle".to_string(),
        bases: Vec::new(),
        mro: vec!["Circle".to_string()],
        attrs: Vec::new(),
        methods: vec![("foo".to_string(), "Circle.foo".to_string())],
        type_param: None,
        properties: Vec::new(),
        static_methods: Vec::new(),
        class_methods: Vec::new(),
        enum_members: Vec::new(),
        is_dataclass: false,
        dataclass_fields: Vec::new(),
        is_protocol: false,
        runtime_checkable: false,
        protocol_members: Vec::new(),
        abstract_methods: Vec::new(),
        is_abstract: false,
    };
    let classes: HashMap<String, HirClassDef> = [
        ("Drawable".to_string(), proto_def),
        ("Circle".to_string(), class_def),
    ]
    .into_iter()
    .collect();
    let obj_ty = Ty::Instance(Box::new("Circle".to_string()));
    let proto = classes.get("Drawable").unwrap();
    assert!(!eval_isinstance_protocol(&obj_ty, proto, &classes));
}

#[test]
fn isinstance_with_runtime_checkable_protocol_evaluates_to_false_for_non_instance() {
    use pycc_hir::{HirClassDef, ProtocolMember};
    let proto_def = HirClassDef {
        name: "Drawable".to_string(),
        bases: Vec::new(),
        mro: vec!["Drawable".to_string()],
        attrs: Vec::new(),
        methods: Vec::new(),
        type_param: None,
        properties: Vec::new(),
        static_methods: Vec::new(),
        class_methods: Vec::new(),
        enum_members: Vec::new(),
        is_dataclass: false,
        dataclass_fields: Vec::new(),
        is_protocol: true,
        runtime_checkable: true,
        protocol_members: vec![ProtocolMember::Method {
            name: "draw".to_string(),
            param_tys: vec![],
            return_ty: Ty::None,
        }],
        abstract_methods: Vec::new(),
        is_abstract: false,
    };
    let classes: HashMap<String, HirClassDef> =
        [("Drawable".to_string(), proto_def)].into_iter().collect();
    // Non-instance type should return false.
    let obj_ty = Ty::Int;
    let proto = classes.get("Drawable").unwrap();
    assert!(!eval_isinstance_protocol(&obj_ty, proto, &classes));
}

#[test]
fn isinstance_with_runtime_checkable_protocol_evaluates_to_false_for_unknown_class() {
    use pycc_hir::{HirClassDef, ProtocolMember};
    let proto_def = HirClassDef {
        name: "Drawable".to_string(),
        bases: Vec::new(),
        mro: vec!["Drawable".to_string()],
        attrs: Vec::new(),
        methods: Vec::new(),
        type_param: None,
        properties: Vec::new(),
        static_methods: Vec::new(),
        class_methods: Vec::new(),
        enum_members: Vec::new(),
        is_dataclass: false,
        dataclass_fields: Vec::new(),
        is_protocol: true,
        runtime_checkable: true,
        protocol_members: vec![ProtocolMember::Method {
            name: "draw".to_string(),
            param_tys: vec![],
            return_ty: Ty::None,
        }],
        abstract_methods: Vec::new(),
        is_abstract: false,
    };
    let classes: HashMap<String, HirClassDef> =
        [("Drawable".to_string(), proto_def)].into_iter().collect();
    // Unknown class should return false.
    let obj_ty = Ty::Instance(Box::new("Unknown".to_string()));
    let proto = classes.get("Drawable").unwrap();
    assert!(!eval_isinstance_protocol(&obj_ty, proto, &classes));
}

#[test]
fn isinstance_with_runtime_checkable_protocol_attribute_member() {
    use pycc_hir::{HirClassDef, ProtocolMember};
    let proto_def = HirClassDef {
        name: "HasX".to_string(),
        bases: Vec::new(),
        mro: vec!["HasX".to_string()],
        attrs: Vec::new(),
        methods: Vec::new(),
        type_param: None,
        properties: Vec::new(),
        static_methods: Vec::new(),
        class_methods: Vec::new(),
        enum_members: Vec::new(),
        is_dataclass: false,
        dataclass_fields: Vec::new(),
        is_protocol: true,
        runtime_checkable: true,
        protocol_members: vec![ProtocolMember::Attribute {
            name: "x".to_string(),
            ty: Ty::Int,
        }],
        abstract_methods: Vec::new(),
        is_abstract: false,
    };
    let class_def = HirClassDef {
        name: "Point".to_string(),
        bases: Vec::new(),
        mro: vec!["Point".to_string()],
        attrs: vec![("x".to_string(), Ty::Int)],
        methods: Vec::new(),
        type_param: None,
        properties: Vec::new(),
        static_methods: Vec::new(),
        class_methods: Vec::new(),
        enum_members: Vec::new(),
        is_dataclass: false,
        dataclass_fields: Vec::new(),
        is_protocol: false,
        runtime_checkable: false,
        protocol_members: Vec::new(),
        abstract_methods: Vec::new(),
        is_abstract: false,
    };
    let classes: HashMap<String, HirClassDef> = [
        ("HasX".to_string(), proto_def),
        ("Point".to_string(), class_def),
    ]
    .into_iter()
    .collect();
    let obj_ty = Ty::Instance(Box::new("Point".to_string()));
    let proto = classes.get("HasX").unwrap();
    assert!(eval_isinstance_protocol(&obj_ty, proto, &classes));
}

#[test]
fn isinstance_with_runtime_checkable_protocol_attribute_found_via_property() {
    // This exercises the `mro_def.properties.iter().any(...)` path
    // (line 2063) in the attribute presence check.
    use pycc_hir::{HirClassDef, PropertyDef, ProtocolMember};
    let proto_def = HirClassDef {
        name: "HasX".to_string(),
        bases: Vec::new(),
        mro: vec!["HasX".to_string()],
        attrs: Vec::new(),
        methods: Vec::new(),
        type_param: None,
        properties: Vec::new(),
        static_methods: Vec::new(),
        class_methods: Vec::new(),
        enum_members: Vec::new(),
        is_dataclass: false,
        dataclass_fields: Vec::new(),
        is_protocol: true,
        runtime_checkable: true,
        protocol_members: vec![ProtocolMember::Attribute {
            name: "x".to_string(),
            ty: Ty::Int,
        }],
        abstract_methods: Vec::new(),
        is_abstract: false,
    };
    let class_def = HirClassDef {
        name: "Point".to_string(),
        bases: Vec::new(),
        mro: vec!["Point".to_string()],
        attrs: Vec::new(),
        methods: Vec::new(),
        type_param: None,
        properties: vec![PropertyDef {
            name: "x".to_string(),
            getter: "get_x".to_string(),
            setter: None,
        }],
        static_methods: Vec::new(),
        class_methods: Vec::new(),
        enum_members: Vec::new(),
        is_dataclass: false,
        dataclass_fields: Vec::new(),
        is_protocol: false,
        runtime_checkable: false,
        protocol_members: Vec::new(),
        abstract_methods: Vec::new(),
        is_abstract: false,
    };
    let classes: HashMap<String, HirClassDef> = [
        ("HasX".to_string(), proto_def),
        ("Point".to_string(), class_def),
    ]
    .into_iter()
    .collect();
    let obj_ty = Ty::Instance(Box::new("Point".to_string()));
    let proto = classes.get("HasX").unwrap();
    assert!(eval_isinstance_protocol(&obj_ty, proto, &classes));
}

#[test]
fn isinstance_with_runtime_checkable_protocol_attribute_missing_returns_false() {
    // This exercises the `return false` path (line 2069) when the
    // attribute is not found in the class's MRO.
    use pycc_hir::{HirClassDef, ProtocolMember};
    let proto_def = HirClassDef {
        name: "HasX".to_string(),
        bases: Vec::new(),
        mro: vec!["HasX".to_string()],
        attrs: Vec::new(),
        methods: Vec::new(),
        type_param: None,
        properties: Vec::new(),
        static_methods: Vec::new(),
        class_methods: Vec::new(),
        enum_members: Vec::new(),
        is_dataclass: false,
        dataclass_fields: Vec::new(),
        is_protocol: true,
        runtime_checkable: true,
        protocol_members: vec![ProtocolMember::Attribute {
            name: "x".to_string(),
            ty: Ty::Int,
        }],
        abstract_methods: Vec::new(),
        is_abstract: false,
    };
    let class_def = HirClassDef {
        name: "NoX".to_string(),
        bases: Vec::new(),
        mro: vec!["NoX".to_string()],
        attrs: vec![("y".to_string(), Ty::Int)],
        methods: Vec::new(),
        type_param: None,
        properties: Vec::new(),
        static_methods: Vec::new(),
        class_methods: Vec::new(),
        enum_members: Vec::new(),
        is_dataclass: false,
        dataclass_fields: Vec::new(),
        is_protocol: false,
        runtime_checkable: false,
        protocol_members: Vec::new(),
        abstract_methods: Vec::new(),
        is_abstract: false,
    };
    let classes: HashMap<String, HirClassDef> = [
        ("HasX".to_string(), proto_def),
        ("NoX".to_string(), class_def),
    ]
    .into_iter()
    .collect();
    let obj_ty = Ty::Instance(Box::new("NoX".to_string()));
    let proto = classes.get("HasX").unwrap();
    assert!(!eval_isinstance_protocol(&obj_ty, proto, &classes));
}

#[test]
fn isinstance_with_runtime_checkable_protocol_attribute_missing_in_mro_returns_false() {
    // This exercises the `false` path (line 2065) when an MRO entry
    // is not found in the `classes` map.
    use pycc_hir::{HirClassDef, ProtocolMember};
    let proto_def = HirClassDef {
        name: "HasX".to_string(),
        bases: Vec::new(),
        mro: vec!["HasX".to_string()],
        attrs: Vec::new(),
        methods: Vec::new(),
        type_param: None,
        properties: Vec::new(),
        static_methods: Vec::new(),
        class_methods: Vec::new(),
        enum_members: Vec::new(),
        is_dataclass: false,
        dataclass_fields: Vec::new(),
        is_protocol: true,
        runtime_checkable: true,
        protocol_members: vec![ProtocolMember::Attribute {
            name: "x".to_string(),
            ty: Ty::Int,
        }],
        abstract_methods: Vec::new(),
        is_abstract: false,
    };
    // The class's MRO includes "Ghost" which is NOT in the classes
    // map, exercising the `else { false }` branch.
    let class_def = HirClassDef {
        name: "WithGhost".to_string(),
        bases: Vec::new(),
        mro: vec!["WithGhost".to_string(), "Ghost".to_string()],
        attrs: Vec::new(),
        methods: Vec::new(),
        type_param: None,
        properties: Vec::new(),
        static_methods: Vec::new(),
        class_methods: Vec::new(),
        enum_members: Vec::new(),
        is_dataclass: false,
        dataclass_fields: Vec::new(),
        is_protocol: false,
        runtime_checkable: false,
        protocol_members: Vec::new(),
        abstract_methods: Vec::new(),
        is_abstract: false,
    };
    let classes: HashMap<String, HirClassDef> = [
        ("HasX".to_string(), proto_def),
        ("WithGhost".to_string(), class_def),
    ]
    .into_iter()
    .collect();
    let obj_ty = Ty::Instance(Box::new("WithGhost".to_string()));
    let proto = classes.get("HasX").unwrap();
    assert!(!eval_isinstance_protocol(&obj_ty, proto, &classes));
}

#[test]
fn isinstance_with_runtime_checkable_protocol_method_missing_in_mro_returns_false() {
    // Exercises the `?`-None path in the method arm of
    // `eval_isinstance_protocol` when an MRO entry is not found in
    // the `classes` map. The attribute arm's equivalent path is
    // covered by `isinstance_with_runtime_checkable_protocol_attribute_missing_in_mro_returns_false`.
    use pycc_hir::{HirClassDef, ProtocolMember};
    let proto_def = HirClassDef {
        name: "HasDraw".to_string(),
        bases: Vec::new(),
        mro: vec!["HasDraw".to_string()],
        attrs: Vec::new(),
        methods: Vec::new(),
        type_param: None,
        properties: Vec::new(),
        static_methods: Vec::new(),
        class_methods: Vec::new(),
        enum_members: Vec::new(),
        is_dataclass: false,
        dataclass_fields: Vec::new(),
        is_protocol: true,
        runtime_checkable: true,
        protocol_members: vec![ProtocolMember::Method {
            name: "draw".to_string(),
            param_tys: vec![],
            return_ty: Ty::None,
        }],
        abstract_methods: Vec::new(),
        is_abstract: false,
    };
    // The class's MRO includes "Ghost" which is NOT in the classes
    // map, exercising the `?`-None path in the method arm's
    // `find_map` closure.
    let class_def = HirClassDef {
        name: "WithGhostMethod".to_string(),
        bases: Vec::new(),
        mro: vec!["WithGhostMethod".to_string(), "Ghost".to_string()],
        attrs: Vec::new(),
        methods: Vec::new(),
        type_param: None,
        properties: Vec::new(),
        static_methods: Vec::new(),
        class_methods: Vec::new(),
        enum_members: Vec::new(),
        is_dataclass: false,
        dataclass_fields: Vec::new(),
        is_protocol: false,
        runtime_checkable: false,
        protocol_members: Vec::new(),
        abstract_methods: Vec::new(),
        is_abstract: false,
    };
    let classes: HashMap<String, HirClassDef> = [
        ("HasDraw".to_string(), proto_def),
        ("WithGhostMethod".to_string(), class_def),
    ]
    .into_iter()
    .collect();
    let obj_ty = Ty::Instance(Box::new("WithGhostMethod".to_string()));
    let proto = classes.get("HasDraw").unwrap();
    assert!(!eval_isinstance_protocol(&obj_ty, proto, &classes));
}

#[test]
fn method_call_on_protocol_typed_parameter_resolves_via_class_def_of() {
    // This exercises the `Ty::Protocol(name) => name.as_str()` arm
    // in `class_def_of` (line 1855) by constructing a HIR module
    // with a protocol-typed parameter and a method call on it.
    let proto_ty = Ty::Protocol(Box::new("P".to_string()));
    let hir = HirModule {
        seeded_builtin_exception_classes: false,
        items: vec![
            HirItem::Function {
                name: "P.foo".to_string(),
                params: vec![("self".to_string(), proto_ty.clone())],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
            },
            HirItem::Function {
                name: "f".to_string(),
                params: vec![("x".to_string(), proto_ty)],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::MethodCall {
                    base: Box::new(HirExpr::Name("x".to_string())),
                    method: "foo".to_string(),
                    args: vec![],
                }))],
            },
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: vec![(
            "P".to_string(),
            HirClassDef {
                name: "P".to_string(),
                bases: Vec::new(),
                mro: vec!["P".to_string()],
                attrs: Vec::new(),
                methods: vec![("foo".to_string(), "P.foo".to_string())],
                type_param: None,
                properties: Vec::new(),
                static_methods: Vec::new(),
                class_methods: Vec::new(),
                enum_members: Vec::new(),
                is_dataclass: false,
                dataclass_fields: Vec::new(),
                is_protocol: true,
                runtime_checkable: false,
                protocol_members: Vec::new(),
                abstract_methods: Vec::new(),
                is_abstract: false,
            },
        )],
    };
    // Just verify it doesn't panic — the MIR build succeeds when
    // `class_def_of` resolves the protocol-typed expression.
    let _ = build(&hir);
}

#[test]
fn protocol_typed_annassign_binds_concrete_type() {
    // Covers the `else` branch (line 759) and the protocol-annotation
    // `bind_ty` path (line 769) in `lower_stmt`'s `AnnAssign` arm.
    // When the annotation is `Ty::Protocol("P")` and the value is
    // `Ty::Instance("C")`, the types don't match (so the `else`
    // branch is taken), and `bind_ty` uses the concrete type.
    let proto_ty = Ty::Protocol(Box::new("P".to_string()));
    let instance_ty = Ty::Instance(Box::new("C".to_string()));
    let hir = HirModule {
        seeded_builtin_exception_classes: false,
        items: vec![HirItem::TopLevelStmt(HirStmt::AnnAssign {
            target: "c".to_string(),
            annotation: proto_ty,
            value: Some(HirExpr::Call {
                callee: "C".to_string(),
                args: vec![],
            }),
            is_final: false,
        })],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: vec![
            (
                "P".to_string(),
                HirClassDef {
                    name: "P".to_string(),
                    bases: Vec::new(),
                    mro: vec!["P".to_string()],
                    attrs: Vec::new(),
                    methods: Vec::new(),
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    enum_members: Vec::new(),
                    is_dataclass: false,
                    dataclass_fields: Vec::new(),
                    is_protocol: true,
                    runtime_checkable: false,
                    protocol_members: Vec::new(),
                    abstract_methods: Vec::new(),
                    is_abstract: false,
                },
            ),
            (
                "C".to_string(),
                HirClassDef {
                    name: "C".to_string(),
                    bases: Vec::new(),
                    mro: vec!["C".to_string()],
                    attrs: Vec::new(),
                    methods: vec![("__init__".to_string(), "C.__init__".to_string())],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    enum_members: Vec::new(),
                    is_dataclass: false,
                    dataclass_fields: Vec::new(),
                    is_protocol: false,
                    runtime_checkable: false,
                    protocol_members: Vec::new(),
                    abstract_methods: Vec::new(),
                    is_abstract: false,
                },
            ),
        ],
    };
    let mir = build(&hir);
    // The MIR should bind `c` with the concrete `Instance("C")` type,
    // not the protocol type.
    assert!(mir.items.iter().any(|item| {
        matches!(item, MirItem::TopLevelStmt(MirStmt::Assign { target, value })
                if target == "c" && value.ty() == instance_ty)
    }));
}

#[test]
fn isinstance_with_protocol_target_goes_through_lower_isinstance() {
    // Covers line 1999: the `eval_isinstance_protocol` call inside
    // `lower_isinstance` when the target class is a protocol.  The
    // existing `isinstance_with_runtime_checkable_protocol_*` tests
    // call `eval_isinstance_protocol` directly; this test goes
    // through the full `build` → `lower_expr` → `lower_isinstance`
    // path.
    use pycc_hir::ProtocolMember;
    let proto_def = HirClassDef {
        name: "Drawable".to_string(),
        bases: Vec::new(),
        mro: vec!["Drawable".to_string()],
        attrs: Vec::new(),
        methods: Vec::new(),
        type_param: None,
        properties: Vec::new(),
        static_methods: Vec::new(),
        class_methods: Vec::new(),
        enum_members: Vec::new(),
        is_dataclass: false,
        dataclass_fields: Vec::new(),
        is_protocol: true,
        runtime_checkable: true,
        protocol_members: vec![ProtocolMember::Method {
            name: "draw".to_string(),
            param_tys: vec![],
            return_ty: Ty::None,
        }],
        abstract_methods: Vec::new(),
        is_abstract: false,
    };
    let class_def = HirClassDef {
        name: "Circle".to_string(),
        bases: Vec::new(),
        mro: vec!["Circle".to_string()],
        attrs: Vec::new(),
        methods: vec![
            ("__init__".to_string(), "Circle.__init__".to_string()),
            ("draw".to_string(), "Circle.draw".to_string()),
        ],
        type_param: None,
        properties: Vec::new(),
        static_methods: Vec::new(),
        class_methods: Vec::new(),
        enum_members: Vec::new(),
        is_dataclass: false,
        dataclass_fields: Vec::new(),
        is_protocol: false,
        runtime_checkable: false,
        protocol_members: Vec::new(),
        abstract_methods: Vec::new(),
        is_abstract: false,
    };
    let hir = HirModule {
        seeded_builtin_exception_classes: false,
        items: vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "c".to_string(),
                value: HirExpr::Call {
                    callee: "Circle".to_string(),
                    args: vec![],
                },
            }),
            HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                callee: "isinstance".to_string(),
                args: vec![
                    HirExpr::Name("c".to_string()),
                    HirExpr::Name("Drawable".to_string()),
                ],
            })),
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: vec![
            ("Drawable".to_string(), proto_def),
            ("Circle".to_string(), class_def),
        ],
    };
    let mir = build(&hir);
    // The isinstance call should be lowered to a BoolLiteral(true)
    // because Circle conforms to the Drawable protocol.
    assert!(mir.items.iter().any(|item| matches!(
        item,
        MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::BoolLiteral(true)))
    )));
}
