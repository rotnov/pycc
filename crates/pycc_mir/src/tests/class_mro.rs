//! MRO walks and `class::mro_attrs`.
//!
//! Covers attribute deduplication and type override across the MRO, plus
//! the internal-error panics raised when the MRO names a class the `classes`
//! side table has no entry for.

use crate::*;
use pycc_hir::{HirExpr, HirItem, HirModule, HirStmt, Ty};

// -- #432: MRO internal-error panic tests --------------------------------
//
// Every test below bypasses `pycc_types::check` (which would reject
// this) with a hand-built HIR whose class's MRO references a class the
// `classes` map has no entry for -- exactly the "MRO and classes side
// table disagree" scenario each MRO-walk's own doc comment names as
// unreachable from any real `check`-validated program, mirroring this
// file's own established convention for internal-consistency panics.

/// Builds a HIR module with a `Derived` class whose MRO lists
/// `["Derived", "Ghost"]`, where `Ghost` is deliberately absent from
/// `class_defs`. The `body_fn` closure receives the `self`-typed
/// parameter name and returns the function body that triggers the
/// specific MRO walk to test -- using a function parameter (not
/// `Instantiate`) avoids the `mro_attr_count` → `mro_attrs` panic
/// firing first, so the `AttrGet`/`AttrSet`/`MethodCall` MRO walk
/// panic is the one actually reached.
fn ghost_mro_param_module(body: Vec<HirStmt>) -> HirModule {
    use pycc_hir::HirClassDef;
    let self_ty = Ty::Instance(Box::new("Derived".to_string()));
    // The `__init__` body is deliberately empty (`return` only) -- an
    // `AttrSet` on `self` would itself walk the MRO and panic on the
    // absent `Ghost` entry before the `use_derived` function body (the
    // one that actually tests `AttrGet`/`MethodCall`) is ever lowered.
    let init = HirItem::Function {
        name: "Derived.__init__".to_string(),
        params: vec![("self".to_string(), self_ty.clone())],
        return_ty: Ty::None,
        body: vec![HirStmt::Return(None)],
    };
    // A function whose parameter is typed `Derived` -- the MRO walk
    // for `AttrGet`/`AttrSet`/`MethodCall` on this parameter triggers
    // the ghost-class panic without going through `Instantiate`.
    let user_fn = HirItem::Function {
        name: "use_derived".to_string(),
        params: vec![("d".to_string(), self_ty.clone())],
        return_ty: Ty::None,
        body,
    };
    HirModule {
        seeded_builtin_exception_classes: false,
        items: vec![init, user_fn],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: vec![(
            "Derived".to_string(),
            HirClassDef {
                class_attrs: Vec::new(),
                exception_type_tag: None,
                name: "Derived".to_string(),
                bases: vec!["Ghost".to_string()],
                mro: vec!["Derived".to_string(), "Ghost".to_string()],
                attrs: vec![("x".to_string(), Ty::Int)],
                methods: vec![("__init__".to_string(), "Derived.__init__".to_string())],
                type_param: None,
                properties: Vec::new(),
                static_methods: Vec::new(),
                class_methods: Vec::new(),
                enum_members: Vec::new(),
                is_enum: false,
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
#[should_panic(
    expected = "pycc_mir: internal error: class `Ghost` in MRO has no registered HirClassDef"
)]
fn attr_set_with_a_ghost_class_in_the_mro_panics_with_an_internal_error() {
    let hir = ghost_mro_param_module(vec![
        HirStmt::AttrSet {
            base: HirExpr::Name("d".to_string()),
            attr: "x".to_string(),
            value: HirExpr::IntLiteral(42),
        },
        HirStmt::Return(None),
    ]);
    let _ = build(&hir);
}

#[test]
#[should_panic(
    expected = "pycc_mir: internal error: class `Ghost` in MRO has no registered HirClassDef"
)]
fn attr_get_with_a_ghost_class_in_the_mro_panics_with_an_internal_error() {
    let hir = ghost_mro_param_module(vec![
        HirStmt::ExprStmt(HirExpr::AttrGet {
            base: Box::new(HirExpr::Name("d".to_string())),
            attr: "x".to_string(),
        }),
        HirStmt::Return(None),
    ]);
    let _ = build(&hir);
}

#[test]
#[should_panic(
    expected = "pycc_mir: internal error: class `Ghost` in MRO has no registered HirClassDef"
)]
fn method_call_with_a_ghost_class_in_the_mro_panics_with_an_internal_error() {
    let hir = ghost_mro_param_module(vec![
        HirStmt::ExprStmt(HirExpr::MethodCall {
            base: Box::new(HirExpr::Name("d".to_string())),
            method: "f".to_string(),
            args: vec![],
        }),
        HirStmt::Return(None),
    ]);
    let _ = build(&hir);
}

#[test]
#[should_panic(
    expected = "pycc_mir: internal error: class `Ghost` in MRO has no registered HirClassDef"
)]
fn mro_attrs_with_a_ghost_class_in_the_mro_panics_with_an_internal_error() {
    // `Instantiate` calls `mro_attr_count` → `mro_attrs`, which walks
    // the full MRO. The `__init__` MRO walk (using `?`, not panicking)
    // finds `Derived.__init__` first and returns early, so
    // `mro_attr_count` is reached -- and its `mro_attrs` call panics on
    // the absent `Ghost` entry.
    use pycc_hir::HirClassDef;
    let self_ty = Ty::Instance(Box::new("Derived".to_string()));
    let init = HirItem::Function {
        name: "Derived.__init__".to_string(),
        params: vec![("self".to_string(), self_ty.clone())],
        return_ty: Ty::None,
        body: vec![
            HirStmt::AttrSet {
                base: HirExpr::Name("self".to_string()),
                attr: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            },
            HirStmt::Return(None),
        ],
    };
    let hir = HirModule {
        seeded_builtin_exception_classes: false,
        items: vec![
            init,
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "d".to_string(),
                value: HirExpr::Call {
                    callee: "Derived".to_string(),
                    args: vec![],
                },
            }),
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: vec![(
            "Derived".to_string(),
            HirClassDef {
                class_attrs: Vec::new(),
                exception_type_tag: None,
                name: "Derived".to_string(),
                bases: vec!["Ghost".to_string()],
                mro: vec!["Derived".to_string(), "Ghost".to_string()],
                attrs: vec![("x".to_string(), Ty::Int)],
                methods: vec![("__init__".to_string(), "Derived.__init__".to_string())],
                type_param: None,
                properties: Vec::new(),
                static_methods: Vec::new(),
                class_methods: Vec::new(),
                enum_members: Vec::new(),
                is_enum: false,
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

#[test]
#[should_panic(expected = "pycc_mir: internal error: no `__init__` found in class `C`'s MRO")]
fn instantiate_with_no_init_in_the_mro_panics_with_an_internal_error() {
    // #432: `Instantiate`'s `__init__` MRO walk uses `?` (not
    // panicking) when a class is not in the `classes` map. Including
    // `Ghost` in the MRO (but not in `class_defs`) exercises that `?`
    // arm -- `C` is found but has no `__init__`, then `Ghost` is not
    // found, so `find_map` returns `None` and the `unwrap_or_else`
    // panic fires.
    use pycc_hir::HirClassDef;
    let hir = HirModule {
        seeded_builtin_exception_classes: false,
        items: vec![HirItem::TopLevelStmt(HirStmt::Assign {
            target: "c".to_string(),
            value: HirExpr::Call {
                callee: "C".to_string(),
                args: vec![],
            },
        })],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: vec![(
            "C".to_string(),
            HirClassDef {
                class_attrs: Vec::new(),
                exception_type_tag: None,
                name: "C".to_string(),
                bases: vec!["Ghost".to_string()],
                mro: vec!["C".to_string(), "Ghost".to_string()],
                attrs: vec![],
                methods: vec![("f".to_string(), "C.f".to_string())],
                type_param: None,
                properties: Vec::new(),
                static_methods: Vec::new(),
                class_methods: Vec::new(),
                enum_members: Vec::new(),
                is_enum: false,
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

#[test]
fn mro_attrs_deduplicates_a_redeclared_attribute_across_the_mro() {
    // #432: a derived class that re-declares an attribute of the same
    // name as a base class "wins" (its declaration appears first in the
    // MRO). `mro_attrs`'s `seen` set skips the base class's duplicate
    // entry, so the flat layout has exactly one slot for `x`, not two.
    // This exercises the `if seen.insert(..)` false branch (the
    // already-seen skip), which is otherwise uncovered when no test
    // re-declares an attribute across the MRO.
    use pycc_hir::HirClassDef;
    let self_ty = Ty::Instance(Box::new("Derived".to_string()));
    let init = HirItem::Function {
        name: "Derived.__init__".to_string(),
        params: vec![("self".to_string(), self_ty.clone())],
        return_ty: Ty::None,
        body: vec![
            HirStmt::AttrSet {
                base: HirExpr::Name("self".to_string()),
                attr: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            },
            HirStmt::Return(None),
        ],
    };
    let base_init = HirItem::Function {
        name: "Base.__init__".to_string(),
        params: vec![(
            "self".to_string(),
            Ty::Instance(Box::new("Base".to_string())),
        )],
        return_ty: Ty::None,
        body: vec![
            HirStmt::AttrSet {
                base: HirExpr::Name("self".to_string()),
                attr: "x".to_string(),
                value: HirExpr::IntLiteral(0),
            },
            HirStmt::Return(None),
        ],
    };
    let hir = HirModule {
        seeded_builtin_exception_classes: false,
        items: vec![
            base_init,
            init,
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "d".to_string(),
                value: HirExpr::Call {
                    callee: "Derived".to_string(),
                    args: vec![],
                },
            }),
            // AttrGet on `d.x` triggers `mro_attrs`, which walks the
            // MRO and deduplicates `x` (declared in both `Derived`
            // and `Base`).
            HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::AttrGet {
                base: Box::new(HirExpr::Name("d".to_string())),
                attr: "x".to_string(),
            })),
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: vec![
            (
                "Base".to_string(),
                HirClassDef {
                    class_attrs: Vec::new(),
                    exception_type_tag: None,
                    name: "Base".to_string(),
                    bases: Vec::new(),
                    mro: vec!["Base".to_string()],
                    attrs: vec![("x".to_string(), Ty::Int)],
                    methods: vec![("__init__".to_string(), "Base.__init__".to_string())],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    enum_members: Vec::new(),
                    is_enum: false,
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
                "Derived".to_string(),
                HirClassDef {
                    class_attrs: Vec::new(),
                    exception_type_tag: None,
                    name: "Derived".to_string(),
                    bases: vec!["Base".to_string()],
                    mro: vec!["Derived".to_string(), "Base".to_string()],
                    attrs: vec![("x".to_string(), Ty::Int)],
                    methods: vec![("__init__".to_string(), "Derived.__init__".to_string())],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    enum_members: Vec::new(),
                    is_enum: false,
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
    // The Instantiate node should allocate exactly 1 slot (not 2),
    // because `x` is deduplicated across the MRO.
    let instantiate = mir
        .items
        .iter()
        .find_map(|item| match item {
            MirItem::TopLevelStmt(MirStmt::Assign {
                value: MirExpr::Instantiate(inst),
                ..
            }) => Some(inst),
            _ => None,
        })
        .expect("expected an Instantiate node");
    assert_eq!(instantiate.attr_count, 1);
}

#[test]
fn mro_attrs_overrides_type_for_a_redeclared_attribute_with_a_different_type() {
    // #432: when a derived class re-declares an attribute with a
    // different type than the base, the most-derived declaration's
    // type wins (pass 2 of `mro_attrs`). This exercises the
    // `result[idx].1 = ty.clone()` line in the second pass.
    use pycc_hir::HirClassDef;
    let self_ty = Ty::Instance(Box::new("Derived".to_string()));
    let init = HirItem::Function {
        name: "Derived.__init__".to_string(),
        params: vec![("self".to_string(), self_ty.clone())],
        return_ty: Ty::None,
        body: vec![
            HirStmt::AttrSet {
                base: HirExpr::Name("self".to_string()),
                attr: "x".to_string(),
                value: HirExpr::FloatLiteral(1.0),
            },
            HirStmt::Return(None),
        ],
    };
    let base_init = HirItem::Function {
        name: "Base.__init__".to_string(),
        params: vec![(
            "self".to_string(),
            Ty::Instance(Box::new("Base".to_string())),
        )],
        return_ty: Ty::None,
        body: vec![
            HirStmt::AttrSet {
                base: HirExpr::Name("self".to_string()),
                attr: "x".to_string(),
                value: HirExpr::IntLiteral(0),
            },
            HirStmt::Return(None),
        ],
    };
    let hir = HirModule {
        seeded_builtin_exception_classes: false,
        items: vec![
            base_init,
            init,
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "d".to_string(),
                value: HirExpr::Call {
                    callee: "Derived".to_string(),
                    args: vec![],
                },
            }),
            // AttrGet on `d.x` triggers `mro_attrs`, which walks the
            // MRO and overrides `x`'s type from Int (Base) to Float
            // (Derived) in the second pass.
            HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::AttrGet {
                base: Box::new(HirExpr::Name("d".to_string())),
                attr: "x".to_string(),
            })),
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: vec![
            (
                "Base".to_string(),
                HirClassDef {
                    class_attrs: Vec::new(),
                    exception_type_tag: None,
                    name: "Base".to_string(),
                    bases: Vec::new(),
                    mro: vec!["Base".to_string()],
                    attrs: vec![("x".to_string(), Ty::Int)],
                    methods: vec![("__init__".to_string(), "Base.__init__".to_string())],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    enum_members: Vec::new(),
                    is_enum: false,
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
                "Derived".to_string(),
                HirClassDef {
                    class_attrs: Vec::new(),
                    exception_type_tag: None,
                    name: "Derived".to_string(),
                    bases: vec!["Base".to_string()],
                    mro: vec!["Derived".to_string(), "Base".to_string()],
                    attrs: vec![("x".to_string(), Ty::Float)],
                    methods: vec![("__init__".to_string(), "Derived.__init__".to_string())],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    enum_members: Vec::new(),
                    is_enum: false,
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
    // The AttrGet node should have type Float (Derived's declaration
    // wins), not Int (Base's declaration).
    let attr_get = mir
        .items
        .iter()
        .find_map(|item| match item {
            MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::AttrGet { ty, .. })) => {
                Some(ty.clone())
            }
            _ => None,
        })
        .expect("expected an AttrGet node");
    assert_eq!(
        attr_get,
        Ty::Float,
        "re-declared attribute should use the most-derived type (Float)"
    );
}

#[test]
fn class_level_attributes_occupy_no_instance_slot_in_the_mro_layout() {
    // #911 (Part 1 of #885): a class-level attribute is a compile-time
    // constant folded at every read -- it must never reach the flat
    // `mro_attrs` slot layout, in this class or in an inheriting one.
    // The e2e suite can only observe that the *values* read back
    // correctly, which a layout that over-allocates would still satisfy;
    // this asserts the slot count itself, which is the actual D-154
    // contract (`Instantiate` allocates exactly `mro_attr_count` words).
    use pycc_hir::{ClassAttrValue, HirClassDef};
    use std::collections::HashMap;

    fn class_def(name: &str, bases: Vec<String>, mro: Vec<String>) -> HirClassDef {
        HirClassDef {
            name: name.to_string(),
            bases,
            mro,
            attrs: Vec::new(),
            methods: Vec::new(),
            properties: Vec::new(),
            static_methods: Vec::new(),
            class_methods: Vec::new(),
            type_param: None,
            enum_members: Vec::new(),
            is_enum: false,
            class_attrs: Vec::new(),
            is_dataclass: false,
            dataclass_fields: Vec::new(),
            is_protocol: false,
            runtime_checkable: false,
            protocol_members: Vec::new(),
            abstract_methods: Vec::new(),
            is_abstract: false,
            exception_type_tag: None,
        }
    }

    let mut base = class_def("Base", Vec::new(), vec!["Base".to_string()]);
    base.attrs = vec![("w".to_string(), Ty::Int)];
    base.class_attrs = vec![("LIMIT".to_string(), Ty::Int, ClassAttrValue::Int(8))];
    let mut derived = class_def(
        "Derived",
        vec!["Base".to_string()],
        vec!["Derived".to_string(), "Base".to_string()],
    );
    derived.attrs = vec![("h".to_string(), Ty::Int)];
    derived.class_attrs = vec![
        (
            "KIND".to_string(),
            Ty::Str,
            ClassAttrValue::Str("d".to_string()),
        ),
        ("SCALE".to_string(), Ty::Float, ClassAttrValue::Float(1.5)),
        ("DEBUG".to_string(), Ty::Bool, ClassAttrValue::Bool(false)),
    ];

    let mut classes: HashMap<String, HirClassDef> = HashMap::new();
    classes.insert("Base".to_string(), base.clone());
    classes.insert("Derived".to_string(), derived.clone());

    // `Base` declares one instance attribute and one class attribute.
    assert_eq!(
        crate::class::mro_attrs(&base, &classes),
        vec![("w".to_string(), Ty::Int)]
    );
    assert_eq!(crate::class::mro_attr_count(&base, &classes), 1);
    // `Derived` inherits `Base`'s single slot and adds one of its own --
    // neither its own three class attributes nor `Base`'s inherited one
    // widen the layout.
    assert_eq!(
        crate::class::mro_attrs(&derived, &classes),
        vec![("w".to_string(), Ty::Int), ("h".to_string(), Ty::Int)]
    );
    assert_eq!(crate::class::mro_attr_count(&derived, &classes), 2);
    // The class attributes themselves survive the clone into `classes`
    // unchanged (`ClassAttrValue` is structurally comparable).
    assert_eq!(classes["Derived"].class_attrs, derived.class_attrs);
    assert_ne!(
        ClassAttrValue::Int(8),
        ClassAttrValue::Float(8.0),
        "{:?}",
        ClassAttrValue::Bool(true)
    );
}
