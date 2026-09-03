//! Unit tests for whole-program linking (#898, Part 1 of #881, D-222):
//! the equivalence with the single-file `lower_all` path, the flat
//! namespace's collision rule, the cross-module seeding reconciliation,
//! and the program-wide exception-tag assignment.
//!
//! Kept here rather than in `crates/pycc_hir/src/tests.rs`, which #663
//! already tracks as oversized.

use super::*;
use crate::pycc_parser_test_helper::parse;
use crate::{HirItem, ResolvedImports, lower_all, lower_module};

/// Lowers `source` as one standalone module named `display_path`.
fn input(display_path: &str, source: &str) -> LinkInput {
    LinkInput {
        display_path: display_path.to_string(),
        module: lower_module(&parse(source), &ResolvedImports::default())
            .expect("a fixture module must lower"),
    }
}

/// The full program pipeline over `inputs`.
fn link_and_finalize(inputs: Vec<LinkInput>) -> Result<HirModule, Vec<(usize, Diagnostic)>> {
    let linked = link(inputs)?;
    finalize(linked).map_err(|diagnostics| {
        diagnostics
            .into_iter()
            .map(|diagnostic| (0, diagnostic))
            .collect()
    })
}

fn first_error(inputs: Vec<LinkInput>) -> (usize, Diagnostic) {
    let mut errors = link_and_finalize(inputs).expect_err("these inputs must be rejected");
    assert_eq!(errors.len(), 1, "link reports exactly the first problem");
    errors.remove(0)
}

/// The exception type tag of every class in `hir`, in program order,
/// restricted to the user classes (the synthetic builtins carry none).
fn user_tags(hir: &HirModule) -> Vec<(String, Option<u8>)> {
    hir.class_defs
        .iter()
        .filter(|(name, _)| !is_builtin_exception_class(name))
        .map(|(name, def)| (name.clone(), def.exception_type_tag))
        .collect()
}

const POINT: &str = "class Point:\n    def __init__(self, x: int) -> None:\n        self.x = x\n";

const SEEDING: &str =
    "class MyError(ValueError):\n    pass\n\n\ndef main() -> None:\n    raise MyError(\"x\")\n";

#[test]
fn linking_one_unseeded_module_equals_lower_all() {
    let source = "def helper(x: int) -> int:\n    return x + 1\n\n\nvalue = 3\n";
    let linked = link_and_finalize(vec![input("a.py", source)]).expect("must link");
    let single = lower_all(&parse(source)).expect("must lower");
    assert_eq!(linked, single);
    assert!(!linked.seeded_builtin_exception_classes);
}

#[test]
fn linking_one_seeded_module_equals_lower_all() {
    // The seeded synthetic classes are stripped and re-appended by `link`;
    // the result must still be byte-identical to the single-file path,
    // including the trailing synthetic block and the tag numbering.
    let linked = link_and_finalize(vec![input("a.py", SEEDING)]).expect("must link");
    let single = lower_all(&parse(SEEDING)).expect("must lower");
    assert_eq!(linked, single);
    assert!(linked.seeded_builtin_exception_classes);
}

#[test]
fn items_are_concatenated_in_input_order() {
    let linked = link_and_finalize(vec![
        input("dep.py", "def first() -> int:\n    return 1\n"),
        input("main.py", "def second() -> int:\n    return 2\n"),
    ])
    .expect("must link");
    let names: Vec<&str> = linked
        .items
        .iter()
        .map(|item| match item {
            HirItem::Function { name, .. } => name.as_str(),
            HirItem::TopLevelStmt(_) => "<stmt>",
        })
        .collect();
    assert_eq!(names, vec!["first", "second"]);
}

#[test]
fn type_aliases_and_imports_are_unioned() {
    let linked = link_and_finalize(vec![
        input("dep.py", "type Meters = int\nimport math\n"),
        input("main.py", "type Seconds = float\n"),
    ])
    .expect("must link");
    let aliases: Vec<&str> = linked
        .type_aliases
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    assert_eq!(aliases, vec!["Meters", "Seconds"]);
    assert_eq!(linked.imports.len(), 1);
}

#[test]
fn two_modules_defining_one_function_name_collide() {
    let (index, diagnostic) = first_error(vec![
        input("dep.py", "def helper() -> int:\n    return 1\n"),
        input("main.py", "def helper() -> int:\n    return 2\n"),
    ]);
    assert_eq!(
        index, 1,
        "the collision is reported at the later definition"
    );
    assert_eq!(diagnostic.code, "C0001");
    assert!(
        diagnostic
            .message
            .contains("top-level name `helper` is already defined by `dep.py`"),
        "unexpected message: {}",
        diagnostic.message
    );
    assert!(
        diagnostic
            .message
            .contains("a separate namespace per module is not supported yet"),
        "unexpected message: {}",
        diagnostic.message
    );
}

#[test]
fn a_top_level_loop_variable_collides_across_modules() {
    // A name bound by a top-level statement is a definition just like a
    // `def`: the loop target `total` is reported at the statement span.
    let source = "total = 0\nfor total in range(2):\n    print(total)\n";
    let (index, diagnostic) = first_error(vec![input("dep.py", source), input("main.py", source)]);
    assert_eq!(index, 1);
    assert_eq!(diagnostic.code, "C0001");
    assert!(
        diagnostic
            .message
            .contains("top-level name `total` is already defined by `dep.py`"),
        "unexpected message: {}",
        diagnostic.message
    );
}

#[test]
fn a_module_rebinding_its_own_name_does_not_collide_with_itself() {
    // `own` dedupes within one input, so `x = 1; x = 2` in a single module
    // is still legal -- only a name owned by an *earlier* input collides.
    let linked = link_and_finalize(vec![
        input("dep.py", "x = 1\nx = 2\n"),
        input("main.py", "y = 3\n"),
    ])
    .expect("must link");
    assert_eq!(linked.items.len(), 3);
}

#[test]
fn a_class_defined_in_two_modules_collides() {
    let (index, diagnostic) = first_error(vec![input("dep.py", POINT), input("main.py", POINT)]);
    assert_eq!(index, 1);
    assert!(
        diagnostic
            .message
            .contains("top-level name `Point` is already defined by `dep.py`"),
        "unexpected message: {}",
        diagnostic.message
    );
}

#[test]
fn two_seeded_modules_keep_exactly_one_synthetic_class_set() {
    let other = "class OtherError(ValueError):\n    pass\n\n\ndef other() -> None:\n    raise OtherError(\"y\")\n";
    let linked = link_and_finalize(vec![input("dep.py", other), input("main.py", SEEDING)])
        .expect("must link");
    assert!(linked.seeded_builtin_exception_classes);
    let synthetic = linked
        .class_defs
        .iter()
        .filter(|(name, _)| is_builtin_exception_class(name))
        .count();
    assert_eq!(synthetic, builtin_exception_class_defs().len());
}

#[test]
fn exception_tags_are_assigned_across_seeded_modules_in_program_order() {
    let other = "class OtherError(ValueError):\n    pass\n\n\ndef other() -> None:\n    raise OtherError(\"y\")\n";
    let linked = link_and_finalize(vec![input("dep.py", other), input("main.py", SEEDING)])
        .expect("must link");
    assert_eq!(
        user_tags(&linked),
        vec![
            (
                "OtherError".to_string(),
                Some(FIRST_USER_EXCEPTION_TYPE_TAG)
            ),
            (
                "MyError".to_string(),
                Some(FIRST_USER_EXCEPTION_TYPE_TAG + 1)
            ),
        ]
    );
}

#[test]
fn an_unseeded_module_links_with_a_seeded_one() {
    let plain = "def helper() -> int:\n    return 1\n";
    let linked = link_and_finalize(vec![input("dep.py", plain), input("main.py", SEEDING)])
        .expect("must link");
    assert!(linked.seeded_builtin_exception_classes);
    assert_eq!(
        user_tags(&linked),
        vec![("MyError".to_string(), Some(FIRST_USER_EXCEPTION_TYPE_TAG))]
    );
}

#[test]
fn two_modules_that_never_name_a_builtin_exception_seed_nothing() {
    let linked = link_and_finalize(vec![
        input("dep.py", "def helper() -> int:\n    return 1\n"),
        input("main.py", "def main() -> None:\n    print(helper())\n"),
    ])
    .expect("must link");
    assert!(!linked.seeded_builtin_exception_classes);
    assert!(linked.class_defs.is_empty());
}

#[test]
fn a_module_shadowing_a_builtin_exception_cannot_link_with_a_seeded_one() {
    let shadowing = "helper = 1\n\n\ndef ValueError() -> int:\n    return 1\n";
    let (index, diagnostic) =
        first_error(vec![input("main.py", SEEDING), input("dep.py", shadowing)]);
    assert_eq!(index, 1, "reported against the shadowing module");
    assert_eq!(diagnostic.code, "C0001");
    assert!(
        diagnostic.message.contains(
            "module `dep.py` defines `ValueError`, which `main.py` uses as the builtin exception"
        ),
        "unexpected message: {}",
        diagnostic.message
    );
    assert!(
        diagnostic.span.expect("a span is attached").start > 0,
        "reported at the definition"
    );
}

#[test]
fn a_shadow_that_binds_nothing_is_reported_at_the_module_start() {
    // `ValueError: int` is an annotation with no value: the AST shadow scan
    // sees it, but it binds nothing at runtime, so `lower_module` records no
    // definition span for it and `definition_span` falls back to the module
    // start.
    let (index, diagnostic) = first_error(vec![
        input("main.py", SEEDING),
        input("dep.py", "ValueError: int\n"),
    ]);
    assert_eq!(index, 1);
    let span = diagnostic.span.expect("a span is attached");
    assert_eq!(span.start, 0);
    assert_eq!(span.end, 0);
}

#[test]
fn two_shadowing_modules_link_without_seeding() {
    // No input seeded, so the cross-module shadow gate never runs and the
    // program is simply un-seeded, exactly as each module was alone.
    let linked = link_and_finalize(vec![
        input("dep.py", "ValueError = 1\n"),
        input("main.py", "TypeError = 2\n"),
    ])
    .expect("must link");
    assert!(!linked.seeded_builtin_exception_classes);
}

#[test]
fn the_exception_class_budget_is_program_wide() {
    // Neither module alone exceeds the limit; together they do.
    let half = MAX_USER_EXCEPTION_CLASSES / 2 + 1;
    let dep = many_exception_classes("D", half);
    let main = many_exception_classes("M", half);
    let (_, diagnostic) = first_error(vec![input("dep.py", &dep), input("main.py", &main)]);
    assert_eq!(diagnostic.code, "C0001");
    assert!(
        diagnostic.message.contains(&format!(
            "program declares more than {MAX_USER_EXCEPTION_CLASSES} exception classes"
        )),
        "unexpected message: {}",
        diagnostic.message
    );
}

fn many_exception_classes(prefix: &str, count: usize) -> String {
    let mut source = String::new();
    for index in 0..count {
        source.push_str(&format!(
            "class {prefix}{index}(Exception):\n    pass\n\n\n"
        ));
    }
    source.push_str(&format!(
        "def {}() -> None:\n    raise {prefix}0(\"x\")\n",
        prefix.to_lowercase()
    ));
    source
}
