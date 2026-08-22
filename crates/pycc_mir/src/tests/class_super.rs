//! `super()` lowering.
//!
//! Covers direct calls to the base `__init__`, base methods, and base
//! property getters, plus the internal-error panics raised when a bare
//! `super()` or a `super()` base outside a method body reaches MIR.

use crate::*;
use pycc_hir::{HirClassDef, HirExpr, HirItem, HirModule, HirStmt, Ty};

// #433: a bare `HirExpr::Super` should never reach MIR lowering —
// HIR lowering rejects a standalone `super()` with C0001, and
// `super().method()`/`super().attr` are handled by the special-case
// blocks before recursing into `lower_expr` for the base. This test
// bypasses the type checker with a hand-built HIR to exercise the
// panic arm, matching this file's own established internal-error-test
// convention.
#[test]
#[should_panic(expected = "pycc_mir: internal error: a bare `HirExpr::Super` reached MIR lowering")]
fn bare_super_reaching_mir_panics_with_an_internal_error() {
    let hir = HirModule {
        seeded_builtin_exception_classes: false,
        items: vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Super))],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let _ = build(&hir);
}

// #449: `current_class.expect(...)` panic paths in the Super-base blocks
// of `HirExpr::AttrGet` and `HirExpr::MethodCall`. A `HirExpr::Super` as
// the base of an `AttrGet` or `MethodCall` at top level (outside a method
// body, where `current_class` is `None`) should never reach MIR lowering
// — HIR lowering rejects it with C0001. These tests bypass the type
// checker with a hand-built HIR to exercise the panic arms, matching this
// file's own established internal-error-test convention.

#[test]
#[should_panic(
    expected = "pycc_mir: internal error: `HirExpr::Super` reached lower_expr outside a method body"
)]
fn super_attr_get_outside_method_panics_with_an_internal_error() {
    let hir = HirModule {
        seeded_builtin_exception_classes: false,
        items: vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::AttrGet {
            base: Box::new(HirExpr::Super),
            attr: "x".to_string(),
        }))],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let _ = build(&hir);
}

#[test]
#[should_panic(
    expected = "pycc_mir: internal error: `HirExpr::Super` reached lower_expr outside a method body"
)]
fn super_method_call_outside_method_panics_with_an_internal_error() {
    let hir = HirModule {
        seeded_builtin_exception_classes: false,
        items: vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(
            HirExpr::MethodCall {
                base: Box::new(HirExpr::Super),
                method: "f".to_string(),
                args: vec![],
            },
        ))],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let _ = build(&hir);
}

// #433: super() MIR lowering tests.

/// Helper: builds a minimal two-class HIR module where `B.__init__`
/// calls `super().__init__()` and `B.greet` calls `super().greet()`.
fn super_module() -> HirModule {
    let self_a = Ty::Instance(Box::new("A".to_string()));
    let self_b = Ty::Instance(Box::new("B".to_string()));
    HirModule {
        seeded_builtin_exception_classes: false,
        items: vec![
            // A.__init__
            HirItem::Function {
                name: "A.__init__".to_string(),
                params: vec![("self".to_string(), self_a.clone())],
                return_ty: Ty::None,
                body: vec![HirStmt::AttrSet {
                    base: HirExpr::Name("self".to_string()),
                    attr: "x".to_string(),
                    value: HirExpr::IntLiteral(1),
                }],
            },
            // A.greet
            HirItem::Function {
                name: "A.greet".to_string(),
                params: vec![("self".to_string(), self_a.clone())],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::AttrGet {
                    base: Box::new(HirExpr::Name("self".to_string())),
                    attr: "x".to_string(),
                }))],
            },
            // B.__init__ — calls super().__init__()
            HirItem::Function {
                name: "B.__init__".to_string(),
                params: vec![("self".to_string(), self_b.clone())],
                return_ty: Ty::None,
                body: vec![HirStmt::ExprStmt(HirExpr::MethodCall {
                    base: Box::new(HirExpr::Super),
                    method: "__init__".to_string(),
                    args: vec![],
                })],
            },
            // B.greet — calls super().greet()
            HirItem::Function {
                name: "B.greet".to_string(),
                params: vec![("self".to_string(), self_b)],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::MethodCall {
                    base: Box::new(HirExpr::Super),
                    method: "greet".to_string(),
                    args: vec![],
                }))],
            },
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: vec![
            (
                "A".to_string(),
                HirClassDef {
                    exception_type_tag: None,
                    name: "A".to_string(),
                    bases: vec![],
                    mro: vec!["A".to_string()],
                    attrs: vec![("x".to_string(), Ty::Int)],
                    methods: vec![
                        ("__init__".to_string(), "A.__init__".to_string()),
                        ("greet".to_string(), "A.greet".to_string()),
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
                },
            ),
            (
                "B".to_string(),
                HirClassDef {
                    exception_type_tag: None,
                    name: "B".to_string(),
                    bases: vec!["A".to_string()],
                    mro: vec!["B".to_string(), "A".to_string()],
                    attrs: Vec::new(),
                    methods: vec![
                        ("__init__".to_string(), "B.__init__".to_string()),
                        ("greet".to_string(), "B.greet".to_string()),
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
                },
            ),
        ],
    }
}

#[test]
fn super_init_lowers_to_direct_call_to_base_init() {
    let hir = super_module();
    let mir = build(&hir);
    let init = mir.items.iter().find_map(|item| match item {
        MirItem::Function { name, body, .. } if name == "B.__init__" => body.first(),
        _ => None,
    });
    assert_eq!(
        init,
        Some(&MirStmt::ExprStmt(MirExpr::Call {
            callee: "A.__init__".to_string(),
            args: vec![MirExpr::Name {
                name: "self".to_string(),
                ty: Ty::Instance(Box::new("B".to_string())),
            }],
            ty: Ty::None,
        }))
    );
}

#[test]
fn super_method_lowers_to_direct_call_to_base_method() {
    let hir = super_module();
    let mir = build(&hir);
    let greet = mir.items.iter().find_map(|item| match item {
        MirItem::Function { name, body, .. } if name == "B.greet" => body.first(),
        _ => None,
    });
    assert_eq!(
        greet,
        Some(&MirStmt::Return(Some(MirExpr::Call {
            callee: "A.greet".to_string(),
            args: vec![MirExpr::Name {
                name: "self".to_string(),
                ty: Ty::Instance(Box::new("B".to_string())),
            }],
            ty: Ty::Int,
        })))
    );
}

#[test]
#[should_panic(expected = "is not a property on any class after `B` in its MRO")]
fn super_attr_get_naming_an_instance_attr_panics_with_an_internal_error() {
    // #587: `super().x` where `x` is an instance attribute is rejected
    // by `pycc_types::class::resolve_super_attr_get` with `T0047`, so
    // this HIR shape cannot reach `pycc_mir` through the real pipeline.
    // Constructing it directly exercises the arm's panic-on-inconsistency
    // guard, which replaced the slot-read lowering this test previously
    // asserted (`super_attr_lowers_to_attr_get_with_self_base`).
    let self_a = Ty::Instance(Box::new("A".to_string()));
    let self_b = Ty::Instance(Box::new("B".to_string()));
    let hir = HirModule {
        seeded_builtin_exception_classes: false,
        items: vec![
            HirItem::Function {
                name: "A.__init__".to_string(),
                params: vec![("self".to_string(), self_a)],
                return_ty: Ty::None,
                body: vec![HirStmt::AttrSet {
                    base: HirExpr::Name("self".to_string()),
                    attr: "x".to_string(),
                    value: HirExpr::IntLiteral(42),
                }],
            },
            HirItem::Function {
                name: "B.get_x".to_string(),
                params: vec![("self".to_string(), self_b)],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::AttrGet {
                    base: Box::new(HirExpr::Super),
                    attr: "x".to_string(),
                }))],
            },
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: vec![
            (
                "A".to_string(),
                HirClassDef {
                    exception_type_tag: None,
                    name: "A".to_string(),
                    bases: vec![],
                    mro: vec!["A".to_string()],
                    attrs: vec![("x".to_string(), Ty::Int)],
                    methods: vec![("__init__".to_string(), "A.__init__".to_string())],
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
            (
                "B".to_string(),
                HirClassDef {
                    exception_type_tag: None,
                    name: "B".to_string(),
                    bases: vec!["A".to_string()],
                    mro: vec!["B".to_string(), "A".to_string()],
                    attrs: Vec::new(),
                    methods: vec![("get_x".to_string(), "B.get_x".to_string())],
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
    build(&hir);
}

#[test]
fn super_property_lowers_to_call_to_base_getter() {
    use pycc_hir::PropertyDef;
    let self_a = Ty::Instance(Box::new("A".to_string()));
    let self_b = Ty::Instance(Box::new("B".to_string()));
    let hir = HirModule {
        seeded_builtin_exception_classes: false,
        items: vec![
            HirItem::Function {
                name: "A.__init__".to_string(),
                params: vec![("self".to_string(), self_a.clone())],
                return_ty: Ty::None,
                body: vec![
                    HirStmt::AttrSet {
                        base: HirExpr::Name("self".to_string()),
                        attr: "_val".to_string(),
                        value: HirExpr::IntLiteral(0),
                    },
                    HirStmt::Return(None),
                ],
            },
            HirItem::Function {
                name: "A.val".to_string(),
                params: vec![("self".to_string(), self_a)],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::AttrGet {
                    base: Box::new(HirExpr::Name("self".to_string())),
                    attr: "_val".to_string(),
                }))],
            },
            HirItem::Function {
                name: "B.get_val".to_string(),
                params: vec![("self".to_string(), self_b)],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::AttrGet {
                    base: Box::new(HirExpr::Super),
                    attr: "val".to_string(),
                }))],
            },
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: vec![
            (
                "A".to_string(),
                HirClassDef {
                    exception_type_tag: None,
                    name: "A".to_string(),
                    bases: vec![],
                    mro: vec!["A".to_string()],
                    attrs: vec![("_val".to_string(), Ty::Int)],
                    methods: vec![("__init__".to_string(), "A.__init__".to_string())],
                    type_param: None,
                    properties: vec![PropertyDef {
                        name: "val".to_string(),
                        getter: "A.val".to_string(),
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
                },
            ),
            (
                "B".to_string(),
                HirClassDef {
                    exception_type_tag: None,
                    name: "B".to_string(),
                    bases: vec!["A".to_string()],
                    mro: vec!["B".to_string(), "A".to_string()],
                    attrs: Vec::new(),
                    methods: vec![("get_val".to_string(), "B.get_val".to_string())],
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
    let get_val = mir.items.iter().find_map(|item| match item {
        MirItem::Function { name, body, .. } if name == "B.get_val" => body.first(),
        _ => None,
    });
    assert_eq!(
        get_val,
        Some(&MirStmt::Return(Some(MirExpr::Call {
            callee: "A.val".to_string(),
            args: vec![MirExpr::Name {
                name: "self".to_string(),
                ty: Ty::Instance(Box::new("B".to_string())),
            }],
            ty: Ty::Int,
        })))
    );
}
