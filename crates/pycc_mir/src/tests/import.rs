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
    let stmt = mir
        .items
        .iter()
        .find_map(|item| match item {
            MirItem::TopLevelStmt(MirStmt::ExprStmt(expr)) => Some(expr.clone()),
            _ => None,
        })
        .expect("the module has exactly one top-level statement");
    stmt
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
