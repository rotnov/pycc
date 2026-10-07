//! #1476 (Part 3 of #1387): an `isinstance(o, C)` guard narrows an `object`
//! name back to a native type for the reads it dominates
//! (`crate::narrow`), on both walkers.
//!
//! A fully annotated module is decided by the check phase alone; a module
//! holding one unannotated private helper also runs the constraint solver,
//! whose per-function verdict wins (D-220), so each shape is asserted
//! under both.

use super::*;

fn diagnostics(source: &str) -> Vec<pycc_diag::Diagnostic> {
    let parsed = pycc_parser::parse(source).expect("test fixture must parse");
    let mut resolved = pycc_hir::ResolvedImports::default();
    resolved.set_ext_module(true);
    let hir = pycc_hir::lower_module(&parsed, &resolved, None)
        .map(|lowered| lowered.hir)
        .unwrap_or_else(|diagnostics| panic!("{source}\nmust lower: {diagnostics:#?}"));
    check_all(&hir).err().unwrap_or_default()
}

/// One unannotated private helper and a call that resolves it: present, the
/// module runs the constraint solver as well as the check phase.
const SOLVER: &str = "def _id(x):\n    return x\n\n\ndef use() -> int:\n    return _id(1)\n\n\n";

fn checks(source: &str) {
    for prefix in ["", SOLVER] {
        let source = format!("{prefix}{source}");
        let diagnostics = diagnostics(&source);
        assert!(diagnostics.is_empty(), "{source}\n{diagnostics:#?}");
    }
}

fn refused(source: &str, code: &str, message: &str) {
    for prefix in ["", SOLVER] {
        let source = format!("{prefix}{source}");
        let diagnostics = diagnostics(&source);
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == code && d.message.contains(message)),
            "{source}\nexpected {code} {message:?}, got {diagnostics:#?}"
        );
    }
}

/// Any diagnostic at all: the shape is refused, whatever names it.
fn rejected(source: &str) {
    for prefix in ["", SOLVER] {
        let source = format!("{prefix}{source}");
        assert!(
            !diagnostics(&source).is_empty(),
            "{source}\nexpected a refusal"
        );
    }
}

#[test]
fn each_scalar_guard_admits_native_operations_on_the_narrowed_name() {
    checks(
        "def f(o: object) -> int:\n    if isinstance(o, int):\n        return o + 1\n    return 0\n",
    );
    checks(
        "def f(o: object) -> float:\n    if isinstance(o, float):\n        return o * 2.0\n    return 0.0\n",
    );
    checks(
        "def f(o: object) -> bool:\n    if isinstance(o, bool):\n        return not o\n    return False\n",
    );
    checks(
        "def f(o: object) -> str:\n    if isinstance(o, str):\n        return o + \"!\"\n    return \"\"\n",
    );
}

#[test]
fn a_compiled_class_guard_admits_its_fields_and_methods() {
    checks(
        "class C:\n    def __init__(self, v: int) -> None:\n        self.v = v\n\n    \
         def twice(self) -> int:\n        return self.v * 2\n\n\n\
         def f(o: object) -> int:\n    if isinstance(o, C):\n        return o.v + o.twice()\n    return 0\n",
    );
}

#[test]
fn outside_the_guard_the_name_is_still_an_object() {
    refused(
        "def f(o: object) -> int:\n    if isinstance(o, int):\n        pass\n    return o + 1\n",
        "T0021",
        "operator Add is not defined for `object` and `int`",
    );
    // The `else` branch of a positive guard is not narrowed.
    rejected(
        "def f(o: object) -> int:\n    if isinstance(o, int):\n        return 0\n    else:\n        return o + 1\n",
    );
}

#[test]
fn a_negated_guard_narrows_the_else_branch_and_the_continuation() {
    checks(
        "def f(o: object) -> int:\n    if not isinstance(o, int):\n        return 0\n    else:\n        return o + 1\n",
    );
    checks(
        "def f(o: object) -> int:\n    if not isinstance(o, int):\n        return 0\n    return o + 1\n",
    );
    // A body that does not terminate narrows nothing after it.
    rejected(
        "def f(o: object) -> int:\n    if not isinstance(o, int):\n        pass\n    return o + 1\n",
    );
}

#[test]
fn rebinding_the_name_ends_the_narrowing() {
    rejected(
        "def f(o: object, p: object) -> int:\n    if isinstance(o, int):\n        o = p\n        return o + 1\n    return 0\n",
    );
}

#[test]
fn a_loop_that_rebinds_the_name_is_not_narrowed_on_entry() {
    rejected(
        "def f(o: object, p: object) -> int:\n    if isinstance(o, int):\n        while o:\n            \
         x = o + 1\n            o = p\n    return 0\n",
    );
}

#[test]
fn a_shadowed_class_name_or_isinstance_does_not_narrow() {
    rejected(
        "def isinstance(a: object, b: object) -> bool:\n    return True\n\n\n\
         def f(o: object) -> int:\n    if isinstance(o, int):\n        return o + 1\n    return 0\n",
    );
}

#[test]
fn unnarrowable_classes_and_shapes_keep_the_object() {
    let shapes = [
        // A tuple of classes.
        "def f(o: object) -> int:\n    if isinstance(o, (int, str)):\n        return o + 1\n    return 0\n",
        // A container class.
        "def f(o: object) -> int:\n    if isinstance(o, list):\n        return len(o) + o\n    return 0\n",
        // A compound test.
        "def f(o: object, b: bool) -> int:\n    if isinstance(o, int) and b:\n        return o + 1\n    return 0\n",
        // An enum.
        "from enum import Enum\n\n\nclass Color(Enum):\n    RED = 1\n\n\n\
         def f(o: object) -> int:\n    if isinstance(o, Color):\n        return o.value\n    return 0\n",
        // An exception class.
        "class Boom(Exception):\n    pass\n\n\n\
         def f(o: object) -> str:\n    if isinstance(o, Boom):\n        return o.upper()\n    return \"\"\n",
    ];
    for shape in shapes {
        rejected(shape);
    }
}

#[test]
fn a_first_binding_from_the_narrowed_name_is_an_object_slot() {
    // Both branches bind `y`, one from the narrowed name and one from the
    // object: the slot is `object` either way.
    checks(
        "def f(o: object, p: object) -> object:\n    if isinstance(o, str):\n        y = o\n    else:\n        y = p\n    return y\n",
    );
    // An annotated binding takes the narrowed value.
    checks(
        "def f(o: object) -> int:\n    if isinstance(o, int):\n        y: int = o\n        return y + 1\n    return 0\n",
    );
    // A non-bare read is native: `y` is an `int` slot, and a later object
    // rebinding is refused.
    rejected(
        "def f(o: object, p: object) -> int:\n    if isinstance(o, int):\n        y = o + 0\n        y = p\n        return y\n    return 0\n",
    );
}

#[test]
fn identity_and_a_nested_guard_see_the_object() {
    checks(
        "def f(o: object, p: object) -> bool:\n    if isinstance(o, str):\n        return o is p\n    return False\n",
    );
    checks(
        "def f(o: object) -> bool:\n    if isinstance(o, str):\n        return o is None or o is not None\n    return False\n",
    );
    checks(
        "def f(o: object) -> int:\n    if isinstance(o, int):\n        if isinstance(o, bool):\n            return 1\n        return o + 2\n    return 0\n",
    );
}

#[test]
fn an_inferred_return_and_an_object_return_both_take_the_narrowed_value() {
    checks(
        "def _inc(o: object):\n    if isinstance(o, int):\n        return o + 1\n    return 0\n\n\n\
         def g(o: object) -> int:\n    return _inc(o)\n",
    );
    checks(
        "def f(o: object) -> object:\n    if isinstance(o, str):\n        return o\n    return o\n",
    );
}

#[test]
fn an_unannotated_parameter_is_not_narrowed() {
    // `x` is a solver variable, not an `object` binding, so the guard does
    // not narrow it: `x + 1` constrains it to `int` as it would unguarded.
    checks(
        "def _g(x):\n    if isinstance(x, int):\n        return x + 1\n    return 0\n\n\n\
         def g() -> int:\n    return _g(2)\n",
    );
    rejected(
        "def _g(x):\n    if isinstance(x, int):\n        return x + 1\n    return 0\n\n\n\
         def g(o: object) -> int:\n    return _g(o)\n",
    );
}

#[test]
fn a_binding_named_like_the_class_or_a_maybe_bound_operand_does_not_narrow() {
    rejected(
        "def f(o: object, int: object) -> int:\n    if isinstance(o, int):\n        return o + 1\n    return 0\n",
    );
    rejected(
        "def f(p: object, b: bool) -> int:\n    if b:\n        o = p\n    if isinstance(o, int):\n        return o + 1\n    return 0\n",
    );
}

#[test]
fn a_guarded_branch_that_rebinds_the_name_still_joins() {
    checks(
        "def f(o: object, p: object) -> object:\n    if isinstance(o, int):\n        x = o + 1\n        o = p\n    return o\n",
    );
}

#[test]
fn a_maybe_bound_rebinding_ends_the_narrowing_after_the_if() {
    rejected(
        "def f(o: object, p: object, b: bool) -> int:\n    if not isinstance(o, int):\n        return 0\n    \
         if b:\n        o = p\n    return o + 1\n",
    );
}
