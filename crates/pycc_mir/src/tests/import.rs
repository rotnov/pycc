//! Foreign-import lowering (`MirItem::ForeignImport`, Part 1 of #1026).
//!
//! PR 1b carried the same information as a `MirModule::foreign_imports`
//! side table; #1080's own ordering requirement replaced it with an item
//! spliced into `MirModule::items` at the position the `import` statement
//! occupied, so codegen emits the import call in source order rather than
//! hoisted. These tests pin both halves: a compile-time-only binding
//! contributes no item at all, and a foreign binding contributes exactly
//! one, at its recorded index.

use crate::*;
use pycc_diag::Span;
use pycc_hir::{HirModule, ImportBinding, ProjectBindingKind};

fn module_with_imports(imports: Vec<ImportBinding>) -> HirModule {
    HirModule {
        seeded_builtin_exception_classes: false,
        items: Vec::new(),
        type_aliases: Vec::new(),
        imports,
        class_defs: Vec::new(),
    }
}

/// A module whose body is `n` `print`-free top-level statements, each a
/// distinct assignment, so an item's index is readable off its target.
fn module_with_stmts(n: usize, imports: Vec<ImportBinding>) -> HirModule {
    let items = (0..n)
        .map(|index| {
            HirItem::TopLevelStmt(pycc_hir::HirStmt::Assign {
                target: format!("v{index}"),
                value: pycc_hir::HirExpr::IntLiteral(index as i64),
            })
        })
        .collect();
    HirModule {
        items,
        ..module_with_imports(imports)
    }
}

fn foreign(local_name: &str, item_index: usize) -> ImportBinding {
    ImportBinding::Foreign {
        local_name: local_name.to_string(),
        module_path: local_name.to_string(),
        item_index,
        span: Span::new(0, 0),
    }
}

/// The `local_name` of every `ForeignImport` in `items`, paired with its
/// index, so a test can state ordering rather than merely membership.
fn foreign_items(module: &MirModule) -> Vec<(usize, String)> {
    module
        .items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| match item {
            MirItem::ForeignImport { local_name, .. } => Some((index, local_name.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn a_module_without_imports_lowers_no_foreign_import_item() {
    let mir = build(&module_with_imports(Vec::new()));
    assert!(foreign_items(&mir).is_empty());
}

#[test]
fn drops_the_stdlib_module_and_symbol_bindings() {
    let hir = module_with_imports(vec![
        ImportBinding::Module {
            local_name: "math".to_string(),
            module: pycc_std::StdModule::Math,
        },
        ImportBinding::Symbol {
            local_name: "sqrt".to_string(),
            module: pycc_std::StdModule::Math,
            symbol: pycc_std::resolve_symbol(pycc_std::StdModule::Math, "sqrt")
                .expect("`math.sqrt` is a registered stdlib symbol"),
        },
    ]);
    assert!(foreign_items(&build(&hir)).is_empty());
}

#[test]
fn drops_a_project_binding() {
    let hir = module_with_imports(vec![ImportBinding::Project {
        local_name: "helper".to_string(),
        module_path: "pkg.helper".to_string(),
        kind: ProjectBindingKind::Function,
    }]);
    assert!(foreign_items(&build(&hir)).is_empty());
}

#[test]
fn a_foreign_import_lands_at_its_recorded_position() {
    // `v0 = 0` / `import numpy` / `v1 = 1`: the import is recorded at
    // index 1 and must land between the two statements, never hoisted.
    let hir = module_with_stmts(2, vec![foreign("numpy", 1)]);
    let mir = build(&hir);
    assert_eq!(foreign_items(&mir), vec![(1, "numpy".to_string())]);
    assert_eq!(mir.items.len(), 3);
}

#[test]
fn a_leading_foreign_import_lands_first() {
    let hir = module_with_stmts(2, vec![foreign("numpy", 0)]);
    assert_eq!(foreign_items(&build(&hir)), vec![(0, "numpy".to_string())]);
}

#[test]
fn a_trailing_foreign_import_lands_last() {
    let hir = module_with_stmts(2, vec![foreign("numpy", 2)]);
    assert_eq!(foreign_items(&build(&hir)), vec![(2, "numpy".to_string())]);
}

#[test]
fn two_foreign_imports_straddling_a_statement_keep_their_order() {
    // `import a` / `v0 = 0` / `import b`: recorded at 0 and 1, and the
    // running insertion offset is what puts `b` after the statement
    // rather than immediately after `a`.
    let hir = module_with_stmts(1, vec![foreign("a", 0), foreign("b", 1)]);
    let mir = build(&hir);
    assert_eq!(
        foreign_items(&mir),
        vec![(0, "a".to_string()), (2, "b".to_string())]
    );
    assert!(matches!(mir.items[1], MirItem::TopLevelStmt(_)));
}

// ---------------------------------------------------------------------
// Part 2 of #1026 (#1081): attribute loads on the bound object.
//
// The two tests below are the only place the `HirExpr::AttrGet` ->
// `MirExpr::ObjAttrGet` lowering is exercised from real HIR. The codegen
// crate's own tests hand-build `MirExpr::ObjAttrGet` (they are about what
// LLVM receives, not about how the node is produced), and the integration
// tests stop at `pycc check`, which never lowers. Lowering is also where
// the module-scope `Ty::Object` bind in `build` is proved: without it,
// `lookup` panics on the `numpy` read rather than producing a base.
// ---------------------------------------------------------------------

/// A module that imports `numpy` at index 0 and then evaluates and
/// discards `base_expr` as its single statement.
fn module_with_discarded(base_expr: pycc_hir::HirExpr) -> HirModule {
    HirModule {
        items: vec![HirItem::TopLevelStmt(pycc_hir::HirStmt::ExprStmt(
            base_expr,
        ))],
        ..module_with_imports(vec![foreign("numpy", 0)])
    }
}

/// The lowered form of the single top-level statement in `hir`.
fn only_discarded_expr(hir: &HirModule) -> MirExpr {
    let mir = build(hir);
    mir.items
        .iter()
        .find_map(|item| match item {
            MirItem::TopLevelStmt(MirStmt::ExprStmt(expr)) => Some(expr.clone()),
            _ => None,
        })
        .expect("the module has exactly one top-level statement")
}

fn attr_get(base: pycc_hir::HirExpr, attr: &str) -> pycc_hir::HirExpr {
    pycc_hir::HirExpr::AttrGet {
        base: Box::new(base),
        attr: attr.to_string(),
    }
}

#[test]
fn an_attribute_load_on_a_foreign_object_lowers_to_obj_attr_get() {
    // `import numpy` / `numpy.pi`. The base resolves to `Ty::Object`
    // through the module-scope bind, so the load takes the string-keyed
    // path instead of `class_def_of`, which has no `HirClassDef` to find.
    let hir = module_with_discarded(attr_get(pycc_hir::HirExpr::Name("numpy".to_string()), "pi"));
    let MirExpr::ObjAttrGet { base, attr, ty } = only_discarded_expr(&hir) else {
        panic!("expected an `ObjAttrGet`");
    };
    assert_eq!(attr, "pi");
    assert_eq!(ty, Ty::Object);
    assert!(matches!(*base, MirExpr::Name { ref name, ty: Ty::Object } if name == "numpy"));
}

#[test]
fn a_chained_attribute_load_stays_opaque_at_every_level() {
    // `numpy.linalg.norm`: the inner load's own result is `Ty::Object`, so
    // the outer load re-enters the same arm rather than falling through to
    // the slot-resolving path. This is what makes the node's `ty` field
    // load-bearing rather than decorative.
    let hir = module_with_discarded(attr_get(
        attr_get(pycc_hir::HirExpr::Name("numpy".to_string()), "linalg"),
        "norm",
    ));
    let MirExpr::ObjAttrGet { base, attr, .. } = only_discarded_expr(&hir) else {
        panic!("expected an outer `ObjAttrGet`");
    };
    assert_eq!(attr, "norm");
    let MirExpr::ObjAttrGet { attr: inner, .. } = *base else {
        panic!("expected an inner `ObjAttrGet`");
    };
    assert_eq!(inner, "linalg");
}

// ---------------------------------------------------------------------
// PR 2b of #1081: method calls on the bound object.
//
// Same rationale as the two attribute-load tests above -- this is the
// only place the `HirExpr::MethodCall` -> `MirExpr::ObjMethodCall`
// lowering runs on real HIR. `pycc_codegen`'s `foreign_call.rs` tests
// hand-build the node, and the integration tests either stop at
// `pycc check` (which never lowers) or need a hosting interpreter.
// ---------------------------------------------------------------------

fn method_call(
    base: pycc_hir::HirExpr,
    method: &str,
    args: Vec<pycc_hir::HirExpr>,
) -> pycc_hir::HirExpr {
    pycc_hir::HirExpr::MethodCall {
        base: Box::new(base),
        method: method.to_string(),
        args,
    }
}

#[test]
fn a_method_call_on_a_foreign_object_lowers_to_obj_method_call() {
    // `import numpy` / `numpy.sqrt(2.0)`. The `Ty::Object` base makes the
    // arm above `class_def_of` fire: a foreign object has no
    // `HirClassDef`, so every later branch in that arm would fail to
    // resolve a mangled symbol.
    let hir = module_with_discarded(method_call(
        pycc_hir::HirExpr::Name("numpy".to_string()),
        "sqrt",
        vec![pycc_hir::HirExpr::FloatLiteral(2.0)],
    ));
    let expr = only_discarded_expr(&hir);
    // The node carries no `ty` field, so `ty()` is the only way to ask --
    // and answering `Ty::Object` unconditionally is the contract that
    // replaces the field.
    assert_eq!(expr.ty(), Ty::Object);
    let MirExpr::ObjMethodCall { base, method, args } = expr else {
        panic!("expected an `ObjMethodCall`");
    };
    assert_eq!(method, "sqrt");
    assert_eq!(args.len(), 1);
    assert!(matches!(args[0], MirExpr::FloatLiteral(f) if f == 2.0));
    assert!(matches!(*base, MirExpr::Name { ref name, ty: Ty::Object } if name == "numpy"));
}

#[test]
fn a_method_call_on_a_foreign_attribute_keeps_the_load_as_its_base() {
    // `numpy.linalg.norm(1)`: the base is itself an `ObjAttrGet`, which is
    // what proves the arm keys on the base's *type* rather than on the
    // base being a bare name. Zero-or-more arguments also means the
    // argument vector is lowered element by element rather than reused.
    let hir = module_with_discarded(method_call(
        attr_get(pycc_hir::HirExpr::Name("numpy".to_string()), "linalg"),
        "norm",
        Vec::new(),
    ));
    let MirExpr::ObjMethodCall { base, method, args } = only_discarded_expr(&hir) else {
        panic!("expected an `ObjMethodCall`");
    };
    assert_eq!(method, "norm");
    assert!(args.is_empty());
    let MirExpr::ObjAttrGet { attr, .. } = *base else {
        panic!("expected the base to stay an `ObjAttrGet`");
    };
    assert_eq!(attr, "linalg");
}

// ---------------------------------------------------------------------
// PR 3a of #1082 (Part 3 of #1026): `len` on the bound object.
//
// Same rationale as the four tests above -- this is the only place the
// `HirExpr::Call { callee: "len" }` -> `MirExpr::ObjLen` split runs on
// real HIR. `pycc_codegen`'s `foreign_len.rs` tests hand-build the node
// because they are about what LLVM receives, and the integration tests
// either stop at `pycc check` (which never lowers) or need a hosting
// interpreter. Without a test here the split itself is unexercised.
// ---------------------------------------------------------------------

fn call(callee: &str, args: Vec<pycc_hir::HirExpr>) -> pycc_hir::HirExpr {
    pycc_hir::HirExpr::Call {
        callee: callee.to_string(),
        args,
    }
}

#[test]
fn len_of_a_foreign_object_lowers_to_obj_len() {
    // `import numpy` / `len(numpy)`. The argument's `Ty::Object` is what
    // diverts this away from the ordinary scalar `len` lowering below it,
    // so the operand type -- not the callee name alone -- is the test's
    // subject.
    let hir = module_with_discarded(call(
        "len",
        vec![pycc_hir::HirExpr::Name("numpy".to_string())],
    ));
    let expr = only_discarded_expr(&hir);
    // The node carries no `ty` field for the same reason `ObjMethodCall`
    // does not: `ty()` answering `Ty::Int` unconditionally is the contract.
    assert_eq!(expr.ty(), Ty::Int);
    let MirExpr::ObjLen { base } = expr else {
        panic!("expected an `ObjLen`");
    };
    assert!(matches!(*base, MirExpr::Name { ref name, ty: Ty::Object } if name == "numpy"));
}

#[test]
fn len_of_a_foreign_attribute_keeps_the_load_as_its_base() {
    // `len(numpy.linalg)`: the operand is an `ObjAttrGet` rather than a
    // bare name, which proves the split keys on the lowered argument's
    // type and not on the argument being a module binding.
    let hir = module_with_discarded(call(
        "len",
        vec![attr_get(
            pycc_hir::HirExpr::Name("numpy".to_string()),
            "linalg",
        )],
    ));
    let MirExpr::ObjLen { base } = only_discarded_expr(&hir) else {
        panic!("expected an `ObjLen`");
    };
    let MirExpr::ObjAttrGet { attr, .. } = *base else {
        panic!("expected the base to stay an `ObjAttrGet`");
    };
    assert_eq!(attr, "linalg");
}

#[test]
fn len_of_a_foreign_object_ignores_a_module_level_len_definition() {
    // The regression PR 3a's original guard caused: a module-level
    // `def len(...)` put `$fn:len` in scope, the guard declined the
    // `ObjLen` split, and `len(<object>)` reached codegen as an ordinary
    // `Call` whose `Scalar::Object` argument tripped `expect_list_pointer`.
    // `pycc_types::check` resolves `len` as the reserved builtin either
    // way, so the lowering must not second-guess it. The divergence from
    // CPython -- that the shadow is ignored rather than refused -- is
    // #1098, not this node's concern.
    let hir = HirModule {
        items: vec![
            HirItem::Function {
                name: "len".to_string(),
                params: vec![("x".to_string(), Ty::Int)],
                return_ty: Ty::Int,
                body: vec![pycc_hir::HirStmt::Return(Some(pycc_hir::HirExpr::Name(
                    "x".to_string(),
                )))],
            },
            HirItem::TopLevelStmt(pycc_hir::HirStmt::ExprStmt(call(
                "len",
                vec![pycc_hir::HirExpr::Name("numpy".to_string())],
            ))),
        ],
        ..module_with_imports(vec![foreign("numpy", 0)])
    };
    let MirExpr::ObjLen { base } = only_discarded_expr(&hir) else {
        panic!("expected an `ObjLen` despite the shadowing `def len`");
    };
    assert!(matches!(*base, MirExpr::Name { ref name, ty: Ty::Object } if name == "numpy"));
}
