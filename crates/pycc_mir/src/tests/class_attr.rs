//! Class instantiation, attribute access, and method-call lowering.
//!
//! Covers `Instantiate`, attribute slot indices for reads and writes,
//! method calls with `self` prepended, and the internal-error panics for
//! undeclared members, non-instance bases, read-only properties, and a
//! generic-class instantiation that should never reach MIR. The enum
//! member test belongs here because it is an `AttrGet` lowering path.

use crate::*;
use pycc_hir::{BinOpKind, HirClassDef, HirExpr, HirItem, HirModule, HirStmt, Ty};

// -- D-154 (Part 1 of #375): class instantiation, attribute access,
// method calls --------------------------------------------------------

/// A minimal `Point` class module: `__init__(self, x: int, y: int)`
/// sets both attributes from its own parameters; `bump(self) -> None`
/// reads and mutates `self.x`. Mirrors `pycc_types::class::tests`'s own
/// `point_module` fixture (same shape, this crate's own `HirModule`
/// literal convention).
fn point_module(extra_items: Vec<HirItem>) -> HirModule {
    let self_ty = Ty::Instance(Box::new("Point".to_string()));
    let init = HirItem::Function {
        name: "Point.__init__".to_string(),
        params: vec![
            ("self".to_string(), self_ty.clone()),
            ("x".to_string(), Ty::Int),
            ("y".to_string(), Ty::Int),
        ],
        return_ty: Ty::None,
        body: vec![
            HirStmt::AttrSet {
                base: HirExpr::Name("self".to_string()),
                attr: "x".to_string(),
                value: HirExpr::Name("x".to_string()),
            },
            HirStmt::AttrSet {
                base: HirExpr::Name("self".to_string()),
                attr: "y".to_string(),
                value: HirExpr::Name("y".to_string()),
            },
            HirStmt::Return(None),
        ],
    };
    let bump = HirItem::Function {
        name: "Point.bump".to_string(),
        params: vec![("self".to_string(), self_ty)],
        return_ty: Ty::None,
        body: vec![
            HirStmt::AttrSet {
                base: HirExpr::Name("self".to_string()),
                attr: "x".to_string(),
                value: HirExpr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(HirExpr::AttrGet {
                        base: Box::new(HirExpr::Name("self".to_string())),
                        attr: "x".to_string(),
                    }),
                    right: Box::new(HirExpr::IntLiteral(1)),
                },
            },
            HirStmt::Return(None),
        ],
    };
    let mut items = vec![init, bump];
    items.extend(extra_items);
    HirModule {
        seeded_builtin_exception_classes: false,
        items,
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: vec![(
            "Point".to_string(),
            HirClassDef {
                exception_type_tag: None,
                name: "Point".to_string(),
                bases: Vec::new(),
                mro: vec!["Point".to_string()],
                attrs: vec![("x".to_string(), Ty::Int), ("y".to_string(), Ty::Int)],
                methods: vec![
                    ("__init__".to_string(), "Point.__init__".to_string()),
                    ("bump".to_string(), "Point.bump".to_string()),
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
        )],
    }
}

#[test]
fn instantiation_lowers_to_a_dedicated_mir_node() {
    let hir = point_module(vec![HirItem::TopLevelStmt(HirStmt::Assign {
        target: "p".to_string(),
        value: HirExpr::Call {
            callee: "Point".to_string(),
            args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
        },
    })]);
    let mir = build(&hir);
    assert_eq!(
        mir.items.last(),
        Some(&MirItem::TopLevelStmt(MirStmt::Assign {
            target: "p".to_string(),
            value: MirExpr::Instantiate(Box::new(InstantiateExpr {
                ctor: "Point.__init__".to_string(),
                attr_count: 2,
                args: vec![MirExpr::IntLiteral(1), MirExpr::IntLiteral(2)],
                ty: Ty::Instance(Box::new("Point".to_string())),
            })),
        }))
    );
}

#[test]
fn a_method_call_lowers_to_an_ordinary_call_with_self_prepended() {
    let hir = point_module(vec![
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: "p".to_string(),
            value: HirExpr::Call {
                callee: "Point".to_string(),
                args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
            },
        }),
        HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::MethodCall {
            base: Box::new(HirExpr::Name("p".to_string())),
            method: "bump".to_string(),
            args: vec![],
        })),
    ]);
    let mir = build(&hir);
    assert_eq!(
        mir.items.last(),
        Some(&MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
            callee: "Point.bump".to_string(),
            args: vec![MirExpr::Name {
                name: "p".to_string(),
                ty: Ty::Instance(Box::new("Point".to_string())),
            }],
            ty: Ty::None,
        })))
    );
}

#[test]
fn a_method_call_with_arguments_lowers_each_argument_after_self() {
    // `point_module`'s own `bump` method takes no extra parameters, so
    // `a_method_call_lowers_to_an_ordinary_call_with_self_prepended`
    // above never exercises `MethodCall`'s own per-argument lowering
    // loop -- a standalone minimal `Counter.add(self, n: int) -> None`
    // fixture here does.
    let self_ty = Ty::Instance(Box::new("Counter".to_string()));
    let init = HirItem::Function {
        name: "Counter.__init__".to_string(),
        params: vec![("self".to_string(), self_ty.clone())],
        return_ty: Ty::None,
        body: vec![
            HirStmt::AttrSet {
                base: HirExpr::Name("self".to_string()),
                attr: "n".to_string(),
                value: HirExpr::IntLiteral(0),
            },
            HirStmt::Return(None),
        ],
    };
    let add = HirItem::Function {
        name: "Counter.add".to_string(),
        params: vec![
            ("self".to_string(), self_ty.clone()),
            ("n".to_string(), Ty::Int),
        ],
        return_ty: Ty::None,
        body: vec![HirStmt::Return(None)],
    };
    let hir = HirModule {
        seeded_builtin_exception_classes: false,
        items: vec![
            init,
            add,
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "c".to_string(),
                value: HirExpr::Call {
                    callee: "Counter".to_string(),
                    args: vec![],
                },
            }),
            HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::MethodCall {
                base: Box::new(HirExpr::Name("c".to_string())),
                method: "add".to_string(),
                args: vec![HirExpr::IntLiteral(5)],
            })),
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: vec![(
            "Counter".to_string(),
            HirClassDef {
                exception_type_tag: None,
                name: "Counter".to_string(),
                bases: Vec::new(),
                mro: vec!["Counter".to_string()],
                attrs: vec![("n".to_string(), Ty::Int)],
                methods: vec![
                    ("__init__".to_string(), "Counter.__init__".to_string()),
                    ("add".to_string(), "Counter.add".to_string()),
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
        )],
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items.last(),
        Some(&MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
            callee: "Counter.add".to_string(),
            args: vec![
                MirExpr::Name {
                    name: "c".to_string(),
                    ty: self_ty,
                },
                MirExpr::IntLiteral(5),
            ],
            ty: Ty::None,
        })))
    );
}

#[test]
fn an_attribute_read_lowers_to_a_slot_index() {
    let hir = point_module(vec![
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: "p".to_string(),
            value: HirExpr::Call {
                callee: "Point".to_string(),
                args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
            },
        }),
        HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::AttrGet {
                base: Box::new(HirExpr::Name("p".to_string())),
                attr: "y".to_string(),
            }],
        })),
    ]);
    let mir = build(&hir);
    assert_eq!(
        mir.items.last(),
        Some(&MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
            callee: "print".to_string(),
            args: vec![MirExpr::AttrGet {
                base: Box::new(MirExpr::Name {
                    name: "p".to_string(),
                    ty: Ty::Instance(Box::new("Point".to_string())),
                }),
                slot: 1,
                ty: Ty::Int,
            }],
            ty: Ty::None,
        })))
    );
}

#[test]
fn an_attribute_write_lowers_to_a_slot_index() {
    let hir = point_module(vec![
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: "p".to_string(),
            value: HirExpr::Call {
                callee: "Point".to_string(),
                args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
            },
        }),
        HirItem::TopLevelStmt(HirStmt::AttrSet {
            base: HirExpr::Name("p".to_string()),
            attr: "x".to_string(),
            value: HirExpr::IntLiteral(9),
        }),
    ]);
    let mir = build(&hir);
    assert_eq!(
        mir.items.last(),
        Some(&MirItem::TopLevelStmt(MirStmt::AttrSet {
            base: MirExpr::Name {
                name: "p".to_string(),
                ty: Ty::Instance(Box::new("Point".to_string())),
            },
            slot: 0,
            value: MirExpr::IntLiteral(9),
        }))
    );
}

#[test]
fn the_init_and_bump_methods_themselves_lower_with_self_typed_as_the_instance() {
    // Exercises `__init__`'s own body -- two `HirStmt::AttrSet`s whose
    // RHS is a bare parameter reference -- and `bump`'s own body -- an
    // `HirExpr::AttrGet` nested inside a `BinOp` -- end to end through
    // `build`, not just the module-scope exercises above. Direct value
    // comparisons against `mir.items[0]`/`[1]`, not a
    // `let PATTERN = .. else { panic!(..) }` destructure -- this file's
    // own established coverage-gate convention (see e.g.
    // `class_instantiation_...` tests elsewhere): a hand-written panic
    // arm never taken on the happy path is a permanently uncovered
    // region under D-014's 100%-region gate.
    let hir = point_module(vec![]);
    let mir = build(&hir);
    let self_ty = Ty::Instance(Box::new("Point".to_string()));
    assert_eq!(
        mir.items[0],
        MirItem::Function {
            name: "Point.__init__".to_string(),
            params: vec![
                ("self".to_string(), self_ty.clone()),
                ("x".to_string(), Ty::Int),
                ("y".to_string(), Ty::Int),
            ],
            return_ty: Ty::None,
            body: vec![
                MirStmt::AttrSet {
                    base: MirExpr::Name {
                        name: "self".to_string(),
                        ty: self_ty.clone(),
                    },
                    slot: 0,
                    value: MirExpr::Name {
                        name: "x".to_string(),
                        ty: Ty::Int,
                    },
                },
                MirStmt::AttrSet {
                    base: MirExpr::Name {
                        name: "self".to_string(),
                        ty: self_ty.clone(),
                    },
                    slot: 1,
                    value: MirExpr::Name {
                        name: "y".to_string(),
                        ty: Ty::Int,
                    },
                },
                MirStmt::Return(None),
            ],
        }
    );
    assert_eq!(
        mir.items[1],
        MirItem::Function {
            name: "Point.bump".to_string(),
            params: vec![("self".to_string(), self_ty.clone())],
            return_ty: Ty::None,
            body: vec![
                MirStmt::AttrSet {
                    base: MirExpr::Name {
                        name: "self".to_string(),
                        ty: self_ty.clone(),
                    },
                    slot: 0,
                    value: MirExpr::BinOp {
                        op: BinOpKind::Add,
                        left: Box::new(MirExpr::AttrGet {
                            base: Box::new(MirExpr::Name {
                                name: "self".to_string(),
                                ty: self_ty.clone(),
                            }),
                            slot: 0,
                            ty: Ty::Int,
                        }),
                        right: Box::new(MirExpr::IntLiteral(1)),
                        ty: Ty::Int,
                    },
                },
                MirStmt::Return(None),
            ],
        }
    );
}

#[test]
#[should_panic(expected = "expected an instance- or protocol-typed expression")]
fn attr_get_on_a_non_instance_base_panics_with_an_internal_error() {
    // Bypasses `pycc_types::check` (which would reject this) with a
    // hand-built `HirExpr::AttrGet` over an `int`-typed base, matching
    // this file's own established internal-error-test convention (see
    // e.g. `referencing_an_unbound_name_panics_with_an_internal_error`
    // above).
    let hir = HirModule {
        seeded_builtin_exception_classes: false,
        items: vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            }),
            HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::AttrGet {
                base: Box::new(HirExpr::Name("x".to_string())),
                attr: "y".to_string(),
            })),
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let _ = build(&hir);
}

#[test]
#[should_panic(expected = "class `Ghost` has no registered HirClassDef")]
fn attr_get_over_an_instance_typed_parameter_from_an_unregistered_class_panics_with_an_internal_error()
 {
    // `class_def_of`'s *second* panic (as opposed to
    // `attr_get_on_a_non_instance_base_panics_with_an_internal_error`
    // above, which exercises its first): `x`'s own declared parameter
    // type names `Ghost`, but `hir.class_defs` never registers it --
    // parameter binding itself never consults `classes` at all, so
    // this reaches `class_def_of`'s own `classes.get(..)` lookup with a
    // genuinely instance-typed expression pointing at a class this
    // module's own side table has no entry for.
    let ghost_ty = Ty::Instance(Box::new("Ghost".to_string()));
    let hir = HirModule {
        seeded_builtin_exception_classes: false,
        items: vec![HirItem::Function {
            name: "f".to_string(),
            params: vec![("x".to_string(), ghost_ty)],
            return_ty: Ty::None,
            body: vec![
                HirStmt::ExprStmt(HirExpr::AttrGet {
                    base: Box::new(HirExpr::Name("x".to_string())),
                    attr: "y".to_string(),
                }),
                HirStmt::Return(None),
            ],
        }],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let _ = build(&hir);
}

#[test]
#[should_panic(expected = "attribute `z` not declared on class `Point`")]
fn attr_get_of_an_undeclared_attribute_panics_with_an_internal_error() {
    let hir = point_module(vec![
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: "p".to_string(),
            value: HirExpr::Call {
                callee: "Point".to_string(),
                args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
            },
        }),
        HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::AttrGet {
            base: Box::new(HirExpr::Name("p".to_string())),
            attr: "z".to_string(),
        })),
    ]);
    let _ = build(&hir);
}

#[test]
#[should_panic(expected = "method `fly` not declared on class `Point`")]
fn method_call_of_an_undeclared_method_panics_with_an_internal_error() {
    let hir = point_module(vec![
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: "p".to_string(),
            value: HirExpr::Call {
                callee: "Point".to_string(),
                args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
            },
        }),
        HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::MethodCall {
            base: Box::new(HirExpr::Name("p".to_string())),
            method: "fly".to_string(),
            args: vec![],
        })),
    ]);
    let _ = build(&hir);
}

#[test]
#[should_panic(expected = "attribute `z` not declared on class `Point`")]
fn attr_set_of_an_undeclared_attribute_panics_with_an_internal_error() {
    let hir = point_module(vec![
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: "p".to_string(),
            value: HirExpr::Call {
                callee: "Point".to_string(),
                args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
            },
        }),
        HirItem::TopLevelStmt(HirStmt::AttrSet {
            base: HirExpr::Name("p".to_string()),
            attr: "z".to_string(),
            value: HirExpr::IntLiteral(1),
        }),
    ]);
    let _ = build(&hir);
}

// #377: a read-only `@property` (no setter) should never reach MIR
// lowering -- `pycc_types::check` rejects the assignment with `T0044`
// before MIR runs. This test bypasses the type checker with a hand-built
// HIR to exercise the panic arm, matching this file's own established
// internal-error-test convention (see e.g.
// `attr_set_of_an_undeclared_attribute_panics_with_an_internal_error`
// above).
#[test]
#[should_panic(expected = "pycc_mir: internal error: property `val` on class `Box` has no setter")]
fn attr_set_on_a_read_only_property_panics_with_an_internal_error() {
    use pycc_hir::{HirClassDef, PropertyDef};
    let self_ty = Ty::Instance(Box::new("Box".to_string()));
    let init = HirItem::Function {
        name: "Box.__init__".to_string(),
        params: vec![("self".to_string(), self_ty.clone())],
        return_ty: Ty::None,
        body: vec![
            HirStmt::AttrSet {
                base: HirExpr::Name("self".to_string()),
                attr: "_val".to_string(),
                value: HirExpr::IntLiteral(0),
            },
            HirStmt::Return(None),
        ],
    };
    let getter = HirItem::Function {
        name: "Box.val".to_string(),
        params: vec![("self".to_string(), self_ty.clone())],
        return_ty: Ty::Int,
        body: vec![HirStmt::Return(Some(HirExpr::AttrGet {
            base: Box::new(HirExpr::Name("self".to_string())),
            attr: "_val".to_string(),
        }))],
    };
    let hir = HirModule {
        seeded_builtin_exception_classes: false,
        items: vec![
            init,
            getter,
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "b".to_string(),
                value: HirExpr::Call {
                    callee: "Box".to_string(),
                    args: vec![],
                },
            }),
            HirItem::TopLevelStmt(HirStmt::AttrSet {
                base: HirExpr::Name("b".to_string()),
                attr: "val".to_string(),
                value: HirExpr::IntLiteral(42),
            }),
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: vec![(
            "Box".to_string(),
            HirClassDef {
                exception_type_tag: None,
                name: "Box".to_string(),
                bases: Vec::new(),
                mro: vec!["Box".to_string()],
                attrs: vec![("_val".to_string(), Ty::Int)],
                methods: vec![("__init__".to_string(), "Box.__init__".to_string())],
                properties: vec![PropertyDef {
                    name: "val".to_string(),
                    getter: "Box.val".to_string(),
                    setter: None,
                }],
                type_param: None,
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
        )],
    };
    let _ = build(&hir);
}

// PEP 695 (#387): `GenericClassInstantiate` should never reach MIR —
// `pycc_types::monomorphize` rewrites every such expression to an
// ordinary `HirExpr::Call` before MIR lowering runs. This test bypasses
// the type checker with a hand-built HIR to exercise the panic arm,
// matching this file's own established internal-error-test convention.
#[test]
#[should_panic(
    expected = "pycc_mir: internal error: `GenericClassInstantiate` for class `C` reached MIR lowering"
)]
fn generic_class_instantiate_reaching_mir_panics_with_an_internal_error() {
    let hir = HirModule {
        seeded_builtin_exception_classes: false,
        items: vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(
            HirExpr::GenericClassInstantiate {
                class: "C".to_string(),
                type_arg: Ty::Int,
                args: vec![HirExpr::IntLiteral(1)],
            },
        ))],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let _ = build(&hir);
}

#[test]
fn enum_member_attr_get_lowers_to_synthetic_global() {
    // #379: `Color.RED` lowers to `MirExpr::Name` reading the
    // synthetic `<Class>.<Member>.enum_member` global.
    let class_def = pycc_hir::HirClassDef {
        exception_type_tag: None,
        name: "Color".to_string(),
        bases: vec![],
        mro: vec!["Color".to_string()],
        attrs: vec![
            ("value".to_string(), Ty::Int),
            ("name".to_string(), Ty::Str),
        ],
        methods: vec![],
        properties: vec![],
        static_methods: vec![],
        class_methods: vec![],
        type_param: None,
        enum_members: vec![
            ("RED".to_string(), pycc_hir::EnumMemberValue::Int(1)),
            ("GREEN".to_string(), pycc_hir::EnumMemberValue::Int(2)),
        ],
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
        items: vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::AttrGet {
                base: Box::new(HirExpr::Name("Color".to_string())),
                attr: "RED".to_string(),
            }],
        }))],
        type_aliases: vec![],
        imports: vec![],
        class_defs: vec![("Color".to_string(), class_def)],
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items[0],
        MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
            callee: "print".to_string(),
            args: vec![MirExpr::Name {
                name: "Color.RED.enum_member".to_string(),
                ty: Ty::Instance(Box::new("Color".to_string())),
            }],
            ty: Ty::None,
        }))
    );
}
