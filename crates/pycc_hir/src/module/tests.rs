//! Unit tests for the module-level walk's per-item diagnostic collection
//! and cascade suppression (Part 2 of #864, #867, D-219).
//!
//! Kept here rather than in `crates/pycc_hir/src/tests.rs` (tracked by
//! #663 as oversized). Every test asserts on codes, messages, spans, and
//! order of the *whole* `lower_all` list; the first entry is always also
//! what `lower_checked` reports, which is D-217's byte-stable first
//! diagnostic.

use super::*;
use crate::pycc_parser_test_helper::parse;

/// Byte span of the `occurrence`-th (0-based) occurrence of `needle` in
/// `source`, in the form `Diagnostic::span` carries.
fn span_of(source: &str, needle: &str, occurrence: usize) -> Span {
    let start = source
        .match_indices(needle)
        .nth(occurrence)
        .map(|(offset, _)| offset)
        .unwrap_or_else(|| panic!("`{needle}` occurrence {occurrence} not found in fixture"));
    Span::new(start as u32, (start + needle.len()) as u32)
}

fn lower_all_err(source: &str) -> Vec<Diagnostic> {
    lower_all(&parse(source)).expect_err("fixture must fail to lower")
}

fn assert_c0001(diagnostic: &Diagnostic, message: &str, span: Span) {
    assert_eq!(diagnostic.code, "C0001");
    assert_eq!(diagnostic.message, message);
    assert_eq!(diagnostic.span, Some(span));
}

const CLASS_BODY_GAP: &str = "a class body statement must be a method definition (`def ...`) -- no \
                              other statement kind is supported yet";
const IMPORT_OS_GAP: &str = "import of module `os` is not supported yet";

#[test]
fn issue_864_reproduction_reports_both_class_body_gaps_in_source_order() {
    let source = "class A:\n    x: int = 1\nclass B:\n    y: str = \"a\"\ndef f(a: int) -> int:\n    \
                  return a + \"s\"\n";
    let diagnostics = lower_all_err(source);
    assert_eq!(diagnostics.len(), 2);
    assert_c0001(
        &diagnostics[0],
        CLASS_BODY_GAP,
        span_of(source, "x: int = 1", 0),
    );
    assert_c0001(
        &diagnostics[1],
        CLASS_BODY_GAP,
        span_of(source, "y: str = \"a\"", 0),
    );
    // The type error on line 6 is the type checker's, which does not run
    // after an HIR failure (D-219 decision A).
    let first = lower_checked(&parse(source)).unwrap_err();
    assert_eq!(first, diagnostics[0]);
}

#[test]
fn three_unsupported_imports_report_three_diagnostics_and_nothing_else() {
    let source = "import os\nimport sys\nimport re\ndef f() -> int:\n    return 1\n";
    let diagnostics = lower_all_err(source);
    assert_eq!(diagnostics.len(), 3);
    assert_c0001(
        &diagnostics[0],
        IMPORT_OS_GAP,
        span_of(source, "import os", 0),
    );
    assert_c0001(
        &diagnostics[1],
        "import of module `sys` is not supported yet",
        span_of(source, "import sys", 0),
    );
    assert_c0001(
        &diagnostics[2],
        "import of module `re` is not supported yet",
        span_of(source, "import re", 0),
    );
}

#[test]
fn an_import_gap_and_a_later_unrelated_gap_are_both_reported() {
    let source = "import os\ndef h(*args: int) -> int:\n    return 0\n";
    let diagnostics = lower_all_err(source);
    assert_eq!(diagnostics.len(), 2);
    assert_c0001(
        &diagnostics[0],
        IMPORT_OS_GAP,
        span_of(source, "import os", 0),
    );
    assert_c0001(
        &diagnostics[1],
        "`*args` is not supported yet",
        span_of(source, "(*args: int)", 0),
    );
}

#[test]
fn every_cascade_of_a_skipped_class_is_silent_and_transitive() {
    let source = "class A:\n    x: int = 1\n\
                  class B(A):\n    def __init__(self) -> None:\n        self.v = 1\n\
                  def g(a: A) -> int:\n    return 1\n\
                  x: A = A()\n\
                  type Alias = A\n\
                  Y: TypeAlias = A\n\
                  class D:\n    def __init__(self) -> None:\n        self.v = 1\n    def m(self) -> A:\n        return A()\n\
                  class C(B):\n    def __init__(self) -> None:\n        self.v = 1\n\
                  class E(Alias):\n    def __init__(self) -> None:\n        self.v = 1\n";
    let diagnostics = lower_all_err(source);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_c0001(
        &diagnostics[0],
        CLASS_BODY_GAP,
        span_of(source, "x: int = 1", 0),
    );
}

#[test]
fn non_cascade_shapes_after_a_skipped_import_stay_reported() {
    // `os` is import-bound, so it is never poisonable (correction 7): the
    // bare annotation is a genuine gap that exists with or without the
    // import, the attribute annotation fails on its own shape, and the
    // value-position reference is not an HIR lookup at all.
    let source = "import os\n\
                  def h(p: os) -> int:\n    return 1\n\
                  def h2(p: os.PathLike) -> int:\n    return 1\n\
                  def h3() -> int:\n    return os.getpid()\n";
    let diagnostics = lower_all_err(source);
    assert_eq!(diagnostics.len(), 3, "{diagnostics:#?}");
    assert_c0001(
        &diagnostics[0],
        IMPORT_OS_GAP,
        span_of(source, "import os", 0),
    );
    assert_c0001(
        &diagnostics[1],
        &unknown_annotation_name_message("os"),
        span_of(source, "os", 1),
    );
    assert_eq!(diagnostics[2].code, "C0001");
    assert!(
        diagnostics[2]
            .message
            .starts_with("only a bare name type annotation is supported so far"),
        "{}",
        diagnostics[2].message
    );
    assert_eq!(diagnostics[2].span, Some(span_of(source, "os.PathLike", 0)));
}

#[test]
fn a_skipped_def_does_not_poison_its_name() {
    let source = "def A(*args: int) -> int:\n    return 0\ndef g(a: A) -> int:\n    return 1\n";
    let diagnostics = lower_all_err(source);
    assert_eq!(diagnostics.len(), 2, "{diagnostics:#?}");
    assert_c0001(
        &diagnostics[0],
        "`*args` is not supported yet",
        span_of(source, "(*args: int)", 0),
    );
    assert_c0001(
        &diagnostics[1],
        &unknown_annotation_name_message("A"),
        span_of(source, "A", 1),
    );
}

#[test]
fn a_failed_from_import_of_a_marker_base_does_not_silence_the_class_using_it() {
    // Round-1 false-suppression class: `Enum` is resolved by spelling before
    // any table lookup, so `class Color(Enum)` lowers and only the import's
    // own `C0002` is reported.
    let source = "from enum import Enum, auto\nclass Color(Enum):\n    RED = 1\n";
    let diagnostics = lower_all_err(source);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_eq!(diagnostics[0].code, "C0002");
    assert_eq!(
        diagnostics[0].message,
        "module `enum` has no importable symbol named `auto`"
    );
    assert_eq!(
        diagnostics[0].span,
        Some(span_of(source, "from enum import Enum, auto", 0))
    );
}

#[test]
fn a_failed_from_import_of_final_does_not_silence_a_final_annotation() {
    let source = "from typing import Final, Optional\ny: Final[int] = 1\n";
    let diagnostics = lower_all_err(source);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_eq!(diagnostics[0].code, "C0002");
    assert_eq!(
        diagnostics[0].message,
        "module `typing` has no importable symbol named `Optional`"
    );
}

#[test]
fn a_folded_type_checking_block_is_not_a_cascade() {
    let source = "from typing import TYPE_CHECKING\nimport os\nif TYPE_CHECKING:\n    q: os = 1\n\
                  def f() -> int:\n    return 1\n";
    let diagnostics = lower_all_err(source);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_c0001(
        &diagnostics[0],
        IMPORT_OS_GAP,
        span_of(source, "import os", 0),
    );
}

#[test]
fn a_rejected_alias_colliding_with_a_valid_class_does_not_poison_the_class() {
    let source = "class A:\n    def __init__(self) -> None:\n        self.v = 1\ntype A = int\n\
                  def f(a: A) -> int:\n    return 1\n";
    let diagnostics = lower_all_err(source);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_c0001(
        &diagnostics[0],
        "type alias `A` collides with a class of the same name already defined in this module",
        span_of(source, "type A = int", 0),
    );
}

#[test]
fn a_successful_class_rebinding_un_poisons_the_name() {
    let source = "class A:\n    x: int = 1\n\
                  class A:\n    def __init__(self) -> None:\n        self.v = 1\n    def m(self) -> A:\n        return self\n\
                  def f(a: A) -> int:\n    return 1\n";
    let diagnostics = lower_all_err(source);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_c0001(
        &diagnostics[0],
        CLASS_BODY_GAP,
        span_of(source, "x: int = 1", 0),
    );
}

#[test]
fn a_twice_rejected_then_accepted_class_leaves_no_duplicate_poison_behind() {
    let source = "class A:\n    x: int = 1\n\
                  class A:\n    y: int = 2\n\
                  class A:\n    def __init__(self) -> None:\n        self.v = 1\n\
                  def f(a: A) -> int:\n    return 1\n";
    let diagnostics = lower_all_err(source);
    assert_eq!(diagnostics.len(), 2, "{diagnostics:#?}");
    assert_c0001(
        &diagnostics[0],
        CLASS_BODY_GAP,
        span_of(source, "x: int = 1", 0),
    );
    assert_c0001(
        &diagnostics[1],
        CLASS_BODY_GAP,
        span_of(source, "y: int = 2", 0),
    );
}

#[test]
fn a_skipped_def_leaves_no_binding_for_a_later_class_to_collide_with() {
    let source = "def A(*args: int) -> int:\n    return 0\n\
                  class A:\n    def __init__(self) -> None:\n        self.v = 1\n";
    let diagnostics = lower_all_err(source);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_c0001(
        &diagnostics[0],
        "`*args` is not supported yet",
        span_of(source, "(*args: int)", 0),
    );
}

#[test]
fn a_rejected_alias_after_a_valid_class_un_poisons_nothing_and_reports_once() {
    // `type X = A` after a skipped `A` is a silent cascade that poisons `X`;
    // a later `class X:` that lowers un-poisons it, so `def f(x: X)` lowers.
    let source = "class A:\n    x: int = 1\ntype X = A\n\
                  class X:\n    def __init__(self) -> None:\n        self.v = 1\n\
                  def f(x: X) -> int:\n    return 1\n";
    let diagnostics = lower_all_err(source);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_c0001(
        &diagnostics[0],
        CLASS_BODY_GAP,
        span_of(source, "x: int = 1", 0),
    );
}

#[test]
fn a_valueless_legacy_alias_declaration_poisons_nothing() {
    // `X: TypeAlias` without a value is lowered as an ordinary annotated
    // assignment (no alias is recorded), so its own rejection must not poison
    // `X`: the later `def f(a: X)` is a genuine gap, not a cascade.
    let source = "X: TypeAlias\n\
                  def f(a: X) -> int:\n    return 1\n";
    let diagnostics = lower_all_err(source);
    assert_eq!(diagnostics.len(), 2, "{diagnostics:#?}");
    assert_c0001(
        &diagnostics[0],
        &unknown_annotation_name_message("TypeAlias"),
        span_of(source, "TypeAlias", 0),
    );
    assert_c0001(
        &diagnostics[1],
        &unknown_annotation_name_message("X"),
        span_of(source, "X", 1),
    );
}

#[test]
fn poisonable_name_per_statement_kind() {
    let cases: &[(&str, Option<&str>)] = &[
        ("class C:\n    pass\n", Some("C")),
        ("type X = int\n", Some("X")),
        ("X: TypeAlias = int\n", Some("X")),
        // Valueless legacy spelling binds no alias, so it is not poisonable.
        ("X: TypeAlias\n", None),
        // ruff drops the parentheses, so the target is still a `Name`.
        ("(x): TypeAlias = int\n", Some("x")),
        // Target is not a `Name`.
        ("a.b: TypeAlias = int\n", None),
        ("x[0]: TypeAlias = int\n", None),
        // Annotation is a `Name` other than `TypeAlias`.
        ("X: Final = 1\n", None),
        // Annotation is not a `Name`.
        ("X: list[int] = []\n", None),
        ("import os\n", None),
        ("from math import sqrt\n", None),
        ("def f() -> int:\n    return 1\n", None),
        ("x = 1\n", None),
        ("x: int = 1\n", None),
        ("print(1)\n", None),
    ];
    for (source, expected) in cases {
        let module = parse(source);
        assert_eq!(poisonable_name(&module.body[0]), *expected, "{source}");
    }
}

#[test]
fn cascade_name_round_trips_both_message_builders() {
    let annotation = unsupported(unknown_annotation_name_message("Foo"), 0..3);
    assert_eq!(cascade_name(&annotation), Some("Foo"));
    let base = unsupported(unknown_base_message("Derived", "Base"), 0..3);
    assert_eq!(cascade_name(&base), Some("Base"));
    // The real producers, end to end.
    let diagnostics = lower_all_err("def f(a: Foo) -> int:\n    return 1\n");
    assert_eq!(cascade_name(&diagnostics[0]), Some("Foo"));
    let diagnostics = lower_all_err("class D(Base):\n    def m(self) -> int:\n        return 1\n");
    assert_eq!(cascade_name(&diagnostics[0]), Some("Base"));
}

#[test]
fn cascade_name_rejects_every_other_diagnostic_shape() {
    // Each parser step fails on its own input so every region is exercised.
    let cases: &[(&'static str, &str)] = &[
        // Annotation prefix ok, suffix fails; base prefix fails.
        ("C0001", "type annotation `x` foo"),
        // Annotation prefix fails; base prefix ok, infix split fails (the
        // real generic-base message from `validate_bases`).
        (
            "C0001",
            "class `B` cannot inherit from generic class `A` -- generic classes as bases are not \
             supported yet",
        ),
        // Base prefix and infix ok, suffix fails.
        (
            "C0001",
            "class `B` inherits from unknown class `A` -- trailing junk",
        ),
        // Neither prefix.
        ("C0001", CLASS_BODY_GAP),
        // Not a `C0001` at all, even with a cascade-shaped message.
        ("T0044", "type annotation `x` is not supported yet"),
        (
            "C0002",
            "module `enum` has no importable symbol named `auto`",
        ),
    ];
    for (code, message) in cases {
        let diagnostic = Diagnostic::error(code, message.to_string(), Span::new(0, 1));
        assert_eq!(cascade_name(&diagnostic), None, "{code}: {message}");
    }
}

#[test]
fn post_loop_phases_are_skipped_when_anything_was_collected() {
    // The hierarchy is seeded (the module references `ValueError`), one
    // class is rejected: exactly that diagnostic, and no `Exception.__init__`
    // seeding or tag assignment is attempted on the partial table.
    let source = "def f() -> None:\n    raise ValueError(\"x\")\n\
                  class E(ValueError):\n    def __init__(self) -> None:\n        self.v = 1\n\
                  class A:\n    x: int = 1\n";
    let diagnostics = lower_all_err(source);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_c0001(
        &diagnostics[0],
        CLASS_BODY_GAP,
        span_of(source, "x: int = 1", 0),
    );
}

#[test]
fn a_shadowing_class_that_fails_to_lower_still_takes_the_shadow_gate() {
    // The shadow gate is a whole-module scan decided before the loop: with
    // `ValueError` shadowed, nothing is seeded, so the second (valid)
    // `class ValueError` does not collide with a synthetic entry -- it
    // lowers and un-poisons the name, leaving exactly one diagnostic. Had
    // the gate looked at what actually lowered, the seeded synthetic class
    // would have made the rebinding a second "defined more than once".
    let source = "class ValueError:\n    x: int = 1\n\
                  class ValueError:\n    def __init__(self) -> None:\n        self.v = 1\n\
                  def f(e: ValueError) -> int:\n    return 1\n";
    let diagnostics = lower_all_err(source);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_c0001(
        &diagnostics[0],
        CLASS_BODY_GAP,
        span_of(source, "x: int = 1", 0),
    );
}

#[test]
fn lower_checked_is_the_first_element_view_of_lower_all() {
    let fixtures = [
        "import os\nimport sys\n",
        "class A:\n    x: int = 1\nclass B:\n    y: str = \"a\"\n",
        "def f(a: Foo) -> int:\n    return 1\n",
        "def h(*args: int) -> int:\n    return 0\n",
    ];
    for source in fixtures {
        let module = parse(source);
        let all = lower_all(&module).expect_err("fixture must fail to lower");
        assert!(!all.is_empty());
        assert_eq!(lower_checked(&module).unwrap_err(), all[0], "{source}");
    }
}

#[test]
fn a_clean_module_lowers_through_lower_all() {
    let module = parse(
        "class A:\n    def __init__(self) -> None:\n        self.v = 1\ndef f(a: A) -> int:\n    return 1\n",
    );
    let hir = lower_all(&module).expect("module must lower");
    assert_eq!(hir.class_defs.len(), 1);
    assert_eq!(hir.class_defs[0].0, "A");
}
