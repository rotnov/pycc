//! Typing of comparisons, identity tests and `isinstance` with a CPython
//! object operand (Part 1 of #1371).

use pycc_diag::Span;
use pycc_hir::ImportBinding;

fn check_foreign(source: &str) -> Result<(), Vec<pycc_diag::Diagnostic>> {
    let module = pycc_parser::parse(source).expect("test source must parse");
    let mut hir = pycc_hir::lower_checked(&module).expect("test source must lower");
    hir.imports.push(ImportBinding::Foreign {
        local_name: "numpy".to_string(),
        module_path: "numpy".to_string(),
        from: None,
        site: pycc_hir::ForeignImportSite::Item(0),
        span: Span::new(0, 0),
    });
    crate::check_all(&hir).map(|_| ())
}

fn assert_admitted(source: &str) {
    if let Err(diagnostics) = check_foreign(source) {
        panic!("must type-check: {source}\n{diagnostics:?}");
    }
}

fn assert_refused(source: &str, code: &str, phrase: &str) {
    let diagnostics = check_foreign(source).expect_err(source);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?} for {source}");
    assert_eq!(diagnostics[0].code, code, "{diagnostics:?} for {source}");
    assert!(
        diagnostics[0].message.contains(phrase),
        "{diagnostics:?} for {source}"
    );
}

#[test]
fn identity_between_two_objects_and_against_none_is_admitted() {
    for source in [
        "b = numpy.pi is numpy.e\n",
        "b = numpy.pi is not numpy.e\n",
        "b = numpy.pi is None\n",
        "b = None is not numpy.pi\n",
        "def f() -> bool:\n    o = numpy.pi\n    return o is None\n",
        "def f() -> bool:\n    o = numpy.pi\n    p = numpy.e\n    return o is not p\n",
    ] {
        assert_admitted(source);
    }
}

#[test]
fn identity_against_a_native_value_is_refused_either_way_round() {
    for source in [
        "x = 1\nb = numpy.pi is x\n",
        "x = 1\nb = x is not numpy.pi\n",
    ] {
        assert_refused(source, "I0404", "identity against a `int` value");
    }
}

#[test]
fn identity_between_two_native_values_keeps_its_c0001() {
    assert_refused(
        "x = 1\ny = 2\nb = x is y\n",
        "C0001",
        "comparison operator not supported yet: Is",
    );
}

#[test]
fn identity_of_a_native_value_against_none_keeps_its_t0021() {
    assert_refused(
        "x = 1\nb = x is None\n",
        "T0021",
        "cannot compare `int` and `None`",
    );
}

#[test]
fn a_rich_comparison_with_an_object_or_scalar_operand_is_admitted() {
    for source in [
        "if numpy.pi == numpy.e:\n    print(1)\n",
        "if numpy.pi != 1:\n    print(1)\n",
        "if 1.5 < numpy.pi:\n    print(1)\n",
        "if numpy.pi <= True:\n    print(1)\n",
        "if numpy.pi > \"a\":\n    print(1)\n",
        "while numpy.pi >= 3:\n    print(1)\n",
        "r = numpy.pi == 1\nprint(r)\n",
        "def f() -> None:\n    o = numpy.pi\n    r = o == numpy.e\n    print(r)\n",
        "def f() -> bool:\n    o = numpy.pi\n    return bool(o < 2)\n",
    ] {
        assert_admitted(source);
    }
}

/// The result of a rich comparison involving an object is the object
/// CPython returns, not a `bool`: an annotation or return type of `bool`
/// is refused by the ordinary assignability rule.
#[test]
fn a_rich_comparison_on_an_object_is_an_object_not_a_bool() {
    for source in [
        "b: bool = numpy.pi == 1\n",
        "def f() -> bool:\n    o = numpy.pi\n    return o == 1\n",
    ] {
        let diagnostics = check_foreign(source).expect_err(source);
        assert!(
            diagnostics.iter().any(|d| d.message.contains("object")),
            "{diagnostics:?} for {source}"
        );
    }
}

#[test]
fn a_rich_comparison_with_an_unpackable_operand_is_refused() {
    assert_refused(
        "b = numpy.pi == [1]\n",
        "I0404",
        "comparing a CPython object with a `list[int]` value",
    );
    assert_refused(
        "b = numpy.pi == None\n",
        "I0404",
        "comparing a CPython object with a `None` value",
    );
}

#[test]
fn a_chained_comparison_with_an_object_operand_is_refused() {
    assert_refused(
        "b = 1 < numpy.pi < 4\n",
        "I0404",
        "a chained comparison with a CPython object operand",
    );
    // A chain without an object operand keeps its native typing.
    assert_admitted("b = 1 < 2 < 4\n");
}

#[test]
fn isinstance_with_an_object_first_argument_is_admitted_for_a_builtin_or_object_class() {
    for source in [
        "b = isinstance(numpy.pi, float)\n",
        "b = isinstance(numpy.pi, numpy.ndarray)\n",
        "b = isinstance(numpy.pi, numpy)\n",
        "def f() -> bool:\n    o = numpy.pi\n    return isinstance(o, str)\n",
        "b = isinstance(numpy.pi, list)\n",
        "b = isinstance(numpy.pi, dict)\n",
        "b = isinstance(numpy.pi, tuple)\n",
    ] {
        assert_admitted(source);
    }
}

/// Part 7 of #1371: a plain class compiled in this module is admitted as
/// the `isinstance` class argument -- a base, a subclass, and from a
/// method body with an object local, the lark `__eq__` shape.
#[test]
fn isinstance_of_an_object_against_a_plain_compiled_class_is_admitted() {
    for source in [
        "class C:\n    pass\n\n\nb = isinstance(numpy.pi, C)\n",
        "class B:\n    pass\n\n\nclass D(B):\n    pass\n\n\nb = isinstance(numpy.pi, B)\n",
        "class C:\n    def same(self) -> bool:\n        other = numpy.pi\n        \
         if not isinstance(other, C):\n            return False\n        \
         return True\n",
    ] {
        assert_admitted(source);
    }
}

/// Part 7 of #1371: the compiled classes that have no exported CPython type
/// object of their own kind are refused, each naming its kind.
#[test]
fn isinstance_of_an_object_against_a_special_compiled_class_is_refused() {
    for (source, phrase) in [
        (
            "class E(Exception):\n    pass\n\n\nb = isinstance(numpy.pi, E)\n",
            "against the pycc exception class `E`",
        ),
        (
            "from typing import Protocol\n\n\nclass P(Protocol):\n    \
             def m(self) -> int: ...\n\n\nb = isinstance(numpy.pi, P)\n",
            "against the pycc protocol `P`",
        ),
        (
            "from enum import Enum\n\n\nclass K(Enum):\n    A = 1\n\n\n\
             b = isinstance(numpy.pi, K)\n",
            "against the pycc enum `K`",
        ),
        (
            "class G[T]:\n    pass\n\n\nb = isinstance(numpy.pi, G)\n",
            "against the pycc generic class `G`",
        ),
    ] {
        assert_refused(source, "I0404", phrase);
    }
}

#[test]
fn isinstance_with_an_object_first_argument_refuses_other_class_arguments() {
    assert_refused(
        "b = isinstance(numpy.pi, (int, str))\n",
        "I0404",
        "against a tuple of classes",
    );
    assert_refused(
        "x = 3\nb = isinstance(numpy.pi, x)\n",
        "I0404",
        "against a `int` value",
    );
}

/// Part 2b of #1371: membership in an object container is admitted for
/// every packable item, and its result is a `bool` -- in a module body, a
/// function body, a `-> bool` return and an unannotated helper, the last of
/// which only the constraint solver's `Compare` arm types.
#[test]
fn membership_in_an_object_container_is_a_bool_for_every_packable_item() {
    for source in [
        "b: bool = 1 in numpy.pi\n",
        "b: bool = 1.5 not in numpy.pi\n",
        "b: bool = True in numpy.pi\n",
        "b: bool = 's' in numpy.pi\n",
        "b: bool = numpy.e not in numpy.pi\n",
        "def f(k: str) -> bool:\n    o = numpy.pi\n    return k in o\n",
        "def _h():\n    return 1 in numpy.pi\n\n\nb: bool = _h()\n",
    ] {
        assert_admitted(source);
    }
}

#[test]
fn membership_with_an_object_on_the_wrong_side_is_refused() {
    assert_refused(
        "b = [1] in numpy.pi\n",
        "I0404",
        "testing membership of a `list[int]` value in a CPython object",
    );
    assert_refused(
        "b = None not in numpy.pi\n",
        "I0404",
        "testing membership of a `None` value in a CPython object",
    );
    assert_refused(
        "xs = [1]\nb = numpy.pi in xs\n",
        "I0404",
        "testing membership of a CPython object in a `list[int]` value",
    );
}

/// No native membership test is lowered, so a native pair -- even two
/// `int`s, which the numeric comparison rule would otherwise admit --
/// keeps the HIR's `C0001` message. The type stage has no span for it, so
/// it is reported at `Span::new(0, 0)` (1:1) until Part 5 of #1371.
#[test]
fn membership_between_two_native_values_keeps_its_c0001() {
    let diagnostics = check_foreign("x = [1]\nb = 1 in x\n").expect_err("native pair");
    assert_eq!(
        diagnostics[0].span,
        Some(Span::new(0, 0)),
        "{diagnostics:?}"
    );
    assert_refused(
        "x = 1\ny = 2\nb = x in y\n",
        "C0001",
        "comparison operator not supported yet: In",
    );
    assert_refused(
        "x = 's'\ny = 's'\nb = x not in y\n",
        "C0001",
        "comparison operator not supported yet: NotIn",
    );
}
