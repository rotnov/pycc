//! Unit tests for the compiler-provided `__name__` binding (W0 of #882,
//! #1156): both seeding gates, every top-level binding form the shadowing
//! gate must recognize, and the seeded item itself.

use super::*;
use crate::module::lower_module;
use crate::pycc_parser_test_helper::parse;
use crate::{HirModule, ResolvedImports};

fn references(source: &str) -> bool {
    references_dunder_name(&parse(source))
}

fn binds(source: &str) -> bool {
    binds_dunder_name_at_top_level(&parse(source))
}

fn seed(source: &str, module_name: Option<&str>) -> Option<HirItem> {
    seed_item(&parse(source), module_name)
}

fn lower(source: &str, module_name: Option<&str>) -> HirModule {
    lower_module(&parse(source), &ResolvedImports::default(), module_name)
        .expect("test fixture must lower")
        .hir
}

/// The `str` literal the module's *first* item assigns to `__name__`, if its
/// first item is such an assignment at all. A seeded module has one by
/// construction; so does a module whose own first statement is
/// `__name__ = "..."`, which is why the shadowing test below checks the value
/// rather than only its presence.
fn first_dunder_name_value(hir: &HirModule) -> Option<&str> {
    match hir.items.first()? {
        HirItem::TopLevelStmt(HirStmt::Assign {
            target,
            value: HirExpr::StringLiteral(literal),
        }) if target == DUNDER_NAME => Some(literal.as_str()),
        _ => None,
    }
}

// -- gate 1: the reference scan -------------------------------------

#[test]
fn a_module_that_never_mentions_the_name_is_not_a_reference() {
    assert!(!references("x: int = 1\nprint(x)\n"));
}

#[test]
fn a_module_level_read_is_a_reference() {
    assert!(references("print(__name__)\n"));
}

#[test]
fn a_read_inside_a_function_body_is_a_reference() {
    assert!(references("def f() -> None:\n    print(__name__)\n"));
}

#[test]
fn a_read_nested_deep_inside_an_expression_is_a_reference() {
    assert!(references(
        "x: list[str] = [s for s in [__name__] if s != \"\"]\n"
    ));
}

#[test]
fn the_scan_stops_descending_once_the_name_is_found() {
    // The first statement matches; the remaining statements (and the
    // remaining children of the matching expression) must not change the
    // answer, which is what the early return in `visit_expr` guarantees.
    assert!(references(
        "print(__name__ + \"x\" + \"y\")\nz: int = 1\nprint(z)\n"
    ));
}

#[test]
fn an_attribute_named_dunder_name_is_not_a_bare_reference() {
    assert!(!references("import m\nprint(m.__name__)\n"));
}

// -- gate 2: the top-level binding scan -----------------------------

#[test]
fn a_module_that_binds_nothing_named_dunder_name_does_not_shadow() {
    assert!(!binds("x: int = 1\nprint(__name__)\n"));
}

#[test]
fn a_plain_assignment_binds() {
    assert!(binds("__name__ = \"custom\"\n"));
}

#[test]
fn a_tuple_unpacking_target_binds() {
    assert!(binds("__name__, other = (\"a\", \"b\")\n"));
}

#[test]
fn a_list_unpacking_target_binds() {
    assert!(binds("[__name__, other] = [\"a\", \"b\"]\n"));
}

#[test]
fn a_starred_unpacking_target_binds() {
    assert!(binds("first, *__name__ = [\"a\", \"b\"]\n"));
}

#[test]
fn an_attribute_assignment_target_does_not_bind() {
    assert!(!binds(
        "import m\nm.__name__ = \"custom\"\nprint(__name__)\n"
    ));
}

#[test]
fn an_annotated_assignment_binds() {
    assert!(binds("__name__: str = \"custom\"\n"));
}

#[test]
fn an_augmented_assignment_binds() {
    assert!(binds("__name__ += \"suffix\"\n"));
}

#[test]
fn a_function_definition_binds() {
    assert!(binds("def __name__() -> None:\n    pass\n"));
}

#[test]
fn an_async_function_definition_binds() {
    assert!(binds("async def __name__() -> None:\n    pass\n"));
}

#[test]
fn a_class_definition_binds() {
    assert!(binds("class __name__:\n    pass\n"));
}

#[test]
fn a_type_alias_binds() {
    assert!(binds("type __name__ = int\n"));
}

#[test]
fn a_for_loop_target_binds() {
    assert!(binds("for __name__ in [\"a\"]:\n    pass\n"));
}

/// A genuine `Stmt::For { is_async: true, .. }`, which is the node the gate's
/// doc comment claims to cover -- ruff carries `async` as a flag on the same
/// variant, so a plain `for` would not exercise the claim at all. A bare
/// top-level `async for` parses fine here; it is rejected later, at lowering,
/// by the D-148 context-validity check, and this gate runs before that.
#[test]
fn an_async_for_loop_target_binds() {
    assert!(binds("async for __name__ in [\"a\"]:\n    pass\n"));
}

#[test]
fn a_with_as_target_binds() {
    assert!(binds("with open(\"f\") as __name__:\n    pass\n"));
}

#[test]
fn a_with_item_without_an_as_target_does_not_bind() {
    assert!(!binds("with open(\"f\"):\n    print(__name__)\n"));
}

#[test]
fn an_import_alias_binds() {
    assert!(binds("import math as __name__\n"));
}

#[test]
fn an_unaliased_import_of_a_dotted_module_binds_only_its_root() {
    assert!(!binds("import os.path\nprint(__name__)\n"));
    assert!(binds("import __name__.sub\n"));
}

#[test]
fn a_from_import_alias_binds() {
    assert!(binds("from math import sqrt as __name__\n"));
}

#[test]
fn an_unaliased_from_import_of_the_name_binds() {
    assert!(binds("from m import __name__\n"));
}

#[test]
fn a_binding_inside_a_function_body_is_not_a_top_level_binding() {
    assert!(!binds(
        "def f() -> None:\n    __name__ = \"local\"\n    print(__name__)\n"
    ));
}

#[test]
fn a_binding_inside_a_class_body_is_not_a_top_level_binding() {
    assert!(!binds(
        "class C:\n    __name__: str = \"in-class\"\n\nprint(__name__)\n"
    ));
}

#[test]
fn an_unrelated_top_level_statement_does_not_bind() {
    assert!(!binds("if True:\n    pass\n\nprint(__name__)\n"));
}

// -- the seed itself -------------------------------------------------

#[test]
fn no_module_name_means_no_seed() {
    assert_eq!(seed("print(__name__)\n", None), None);
}

#[test]
fn no_reference_means_no_seed() {
    assert_eq!(seed("x: int = 1\n", Some("__main__")), None);
}

#[test]
fn a_top_level_binding_withholds_the_seed() {
    assert_eq!(
        seed("__name__ = \"custom\"\nprint(__name__)\n", Some("__main__")),
        None
    );
}

#[test]
fn a_referencing_unshadowed_module_is_seeded_with_the_supplied_name() {
    assert_eq!(
        seed("print(__name__)\n", Some("ext_mod")),
        Some(HirItem::TopLevelStmt(HirStmt::Assign {
            target: DUNDER_NAME.to_string(),
            value: HirExpr::StringLiteral("ext_mod".to_string()),
        }))
    );
}

// -- lowering wires the seed in as the module's first item ----------

#[test]
fn lowering_puts_the_seed_ahead_of_every_user_item() {
    let hir = lower("x: int = 1\nprint(__name__)\n", Some("__main__"));
    assert_eq!(first_dunder_name_value(&hir), Some("__main__"));
    assert_eq!(hir.items.len(), 3);
}

#[test]
fn lowering_without_a_module_name_seeds_nothing() {
    let hir = lower("x: int = 1\n", None);
    assert_eq!(first_dunder_name_value(&hir), None);
}

#[test]
fn lowering_a_shadowing_module_seeds_nothing() {
    let hir = lower("__name__ = \"custom\"\nprint(__name__)\n", Some("__main__"));
    // Two items, not three: the user's own assignment and the `print`. The
    // first item carries the user's value, so no seed was prepended.
    assert_eq!(first_dunder_name_value(&hir), Some("custom"));
    assert_eq!(hir.items.len(), 2);
}
