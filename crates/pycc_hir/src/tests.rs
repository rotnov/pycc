//! Unit tests for the crate-root lowering surface.
//!
//! Extracted verbatim from `lib.rs`'s inline `mod tests` (issue #547): the
//! module is still a direct child of the crate root, so `use super::*` and the
//! private items it reaches resolve exactly as they did inline.

use super::*;
use std::collections::HashSet;
// `lower_comprehension_header`/`rename_name_in_expr` moved to `expr.rs`
// (issue #361, D-149) but the two tests below call them directly,
// bypassing the public `lower_checked` entry point -- unlike every other
// test in this module, so a `super::*` glob import alone does not reach
// them (the crate root neither defines nor re-exports them). See
// `expr.rs`'s own module doc comment for why these two stayed here
// instead of moving.
use crate::expr::{lower_comprehension_header, rename_name_in_expr};

fn assert_capability_error(source: &str, expected_message: &str, expected_span: Span) {
    let module = pycc_parser_test_helper::parse(source);
    let diagnostic = lower_checked(&module).unwrap_err();

    assert_eq!(diagnostic.code, "C0001");
    assert!(diagnostic.message.contains(expected_message));
    assert_eq!(diagnostic.span, Some(expected_span));
}

fn assert_capability_error_message(source: &str, expected_message: &str) {
    let module = pycc_parser_test_helper::parse(source);
    let diagnostic = lower_checked(&module).unwrap_err();

    assert_eq!(diagnostic.code, "C0001");
    assert!(diagnostic.message.contains(expected_message));
    assert!(diagnostic.span.is_some());
}

/// Sibling of `assert_capability_error_message` for the new `L0001`
/// context-invalidity diagnostic (issue #141, D-148) -- kept separate
/// rather than parameterizing the existing helper so every other
/// existing `C0001` call site stays untouched.
fn assert_context_invalid_error_message(source: &str, expected_message: &str) {
    let module = pycc_parser_test_helper::parse(source);
    let diagnostic = lower_checked(&module).unwrap_err();

    assert_eq!(diagnostic.code, "L0001");
    assert!(diagnostic.message.contains(expected_message));
    assert!(diagnostic.span.is_some());
}

#[test]
fn ty_name_returns_the_python_spelling_of_every_scalar_variant() {
    // The four recursive container variants (List/Dict/Set/Tuple) are
    // covered separately by `ty_name_describes_nested_container_types`
    // below, since their expected `.name()` output depends on a nested
    // `Ty` argument rather than being a fixed string per variant.
    assert_eq!(Ty::Int.name(), "int");
    assert_eq!(Ty::Float.name(), "float");
    assert_eq!(Ty::Bool.name(), "bool");
    assert_eq!(Ty::Str.name(), "str");
    assert_eq!(Ty::None.name(), "None");
    assert_eq!(Ty::Infer.name(), "<inferred>");
    assert_eq!(Ty::Param(Box::new("T".to_string())).name(), "T");
    assert_eq!(
        Ty::Instance(Box::new("MyClass".to_string())).name(),
        "MyClass"
    );
    // #380 (PR-20): `Ty::Protocol` name — covered here as a unit test
    // to avoid cargo-llvm-cov issue #276 (instantiation merging).
    assert_eq!(
        Ty::Protocol(Box::new("MyProto".to_string())).name(),
        "MyProto"
    );
}

#[test]
fn lowers_a_protocol_typed_parameter_annotation() {
    // #380 (PR-20): exercises `annotation_to_ty`'s `Ty::Protocol`
    // return path (line 1487) — a bare name matching a known protocol
    // class resolves to `Ty::Protocol`, not `Ty::Instance`. Covered
    // here as a unit test to avoid cargo-llvm-cov issue #276.
    let module = pycc_parser_test_helper::parse(
        "from typing import Protocol\n\
             class P(Protocol):\n    def f(self) -> int: ...\n\
             def _marker() -> None:\n    pass\n\
             def g(x: P) -> None:\n    pass\n",
    );
    let hir = lower_checked(&module).unwrap();
    // The function `g` should have a parameter typed as `Ty::Protocol`.
    // A `_marker` function precedes `g` so that the `None` arm of the
    // match is also exercised (protocol methods do not produce HIR
    // items, so without `_marker` the `None` arm would be unreachable).
    let func = hir.items.iter().find_map(|item| match item {
        HirItem::Function { name, params, .. } if name == "g" => Some(params),
        _ => None,
    });
    let params = func.expect("function `g` should exist");
    assert_eq!(params.len(), 1);
    assert_eq!(params[0].0, "x");
    assert_eq!(params[0].1, Ty::Protocol(Box::new("P".to_string())));
}

#[test]
fn ty_list_variant_is_structurally_comparable_and_not_copy() {
    let a = Ty::List(Box::new(Ty::Int));
    let b = Ty::List(Box::new(Ty::Int));
    let c = Ty::List(Box::new(Ty::Str));
    assert_eq!(a, b);
    assert_ne!(a, c);
    // `Ty` is `Clone` but deliberately not `Copy` (a `Box`/`Vec`-holding
    // enum can't implement `Copy`), so producing a second owned value
    // from `a` requires this explicit `.clone()` -- unlike the old flat
    // scalar `Ty`, where an implicit copy would have made `.clone()`
    // merely redundant rather than required. (`.clone()` alone can't
    // prove Copy's absence at compile time -- it also compiles, just
    // redundantly, for a `Copy` type -- so this comment documents the
    // property rather than the assertions below enforcing it.)
    let d = a.clone();
    assert_eq!(a, d);
}

#[test]
fn ty_shrinks_after_boxing_dict_and_tuple_d109() {
    // D-109: before this task, size_of::<Ty>() measured 24 bytes (Vec<Ty>'s
    // ptr+len+cap dominates). This is a real regression guard, not a vibe --
    // it must stay strictly smaller than 24 forever, catching any future
    // change that re-inflates Ty back to its pre-fix size. This test's
    // own numeric assertion documents PR-11's variant set specifically
    // (PR-11 itself added no new Ty variants, only the Dict/Tuple boxing
    // fix) -- every later PR that adds a new dataful variant (D-133's
    // `Ty::Param`, D-154's `Ty::Instance`) is covered instead by the
    // more general `ty_size_stays_within_d109_ceiling` test below, which
    // tracks the ceiling for the *current* variant set rather than
    // pinning it to one historical PR's shape.
    assert_eq!(
        std::mem::size_of::<Ty>(),
        16,
        "size_of::<Ty>() must stay 16 bytes (PR-10 Task 14, D-109) -- if it \
             moves, something accidentally widened Ty's boxing, not the containers \
             themselves",
    );
}

#[test]
fn ty_size_stays_within_d109_ceiling() {
    // D-133 added `Ty::Param(Box<String>)`; D-154 added
    // `Ty::Instance(Box<String>)`. Both are a single (thin, 8-byte)
    // pointer -- unlike `Box<str>`, which measured 24 bytes here because
    // `str` is unsized (see each variant's own doc comment) -- so
    // neither pushes `size_of::<Ty>()` back past the 16-byte ceiling
    // D-109 established.
    assert!(
        std::mem::size_of::<Ty>() <= 16,
        "size_of::<Ty>() must stay within the D-109 16-byte ceiling; adding \
             Ty::Param(Box<String>) (D-133) and Ty::Instance(Box<String>) (D-154) \
             must not regress it, got {}",
        std::mem::size_of::<Ty>(),
    );
}

#[test]
fn ty_name_describes_nested_container_types() {
    assert_eq!(Ty::Int.name(), "int");
    assert_eq!(Ty::List(Box::new(Ty::Int)).name(), "list[int]");
    assert_eq!(
        Ty::Dict(Box::new((Ty::Str, Ty::Float))).name(),
        "dict[str, float]"
    );
    assert_eq!(Ty::Set(Box::new(Ty::Bool)).name(), "set[bool]");
    assert_eq!(
        Ty::Tuple(Box::new(vec![Ty::Int, Ty::Str])).name(),
        "tuple[int, str]"
    );
}

#[test]
fn ty_optional_name_is_inner_pipe_none() {
    // `Ty::Optional`'s own `.name()` (D-197, #763, Part 1 of #747) spells
    // itself the same way PEP 604 source does (`int | None`), not the
    // `typing.Optional[int]` form, matching this compiler's own
    // parse-side entry point.
    assert_eq!(Ty::Optional(Box::new(Ty::Int)).name(), "int | None");
}

#[test]
fn ty_instance_name_is_the_bare_class_name() {
    // Unlike every other dataful variant, `Ty::Instance`'s `.name()`
    // is not wrapped in a `<kind>[...]` shape -- a class instance's
    // type is spelled exactly like the class itself in real Python
    // (`Point`, not `Instance[Point]`).
    assert_eq!(Ty::Instance(Box::new("Point".to_string())).name(), "Point");
}

#[test]
fn unsupported_statement_and_expression_return_spanned_capability_diagnostics() {
    // #435: `pass` is now supported, so use `with` — a valid Python
    // statement that is still unsupported — to exercise the C0001
    // capability error path for statements.
    assert_capability_error(
        "with open(\"x\") as f:\n    pass\n",
        "statement kind not supported yet",
        Span::new(0, 29),
    );
    assert_capability_error(
        "x = lambda: 1\n",
        "expression kind not supported yet",
        Span::new(4, 13),
    );
}

#[test]
fn capability_errors_propagate_through_every_supported_container() {
    // `(1, 2)` was this table's "genuinely unhandled at every level"
    // poison fixture -- a list literal used to fill this role (see
    // `a_tuple_literal_expression_lowers_successfully`'s own comment)
    // until Task 7 (D-105) added list-literal lowering, and a tuple
    // literal took over in turn until this task (PR-11b Task 2, D-116)
    // added tuple-literal lowering too. `lambda: 1` (parenthesized
    // throughout, purely to dodge grammar ambiguity with the
    // surrounding syntax -- not because any position here actually
    // requires it) takes over now, since `lower_expr` still has no
    // `Expr::Lambda` arm.
    let cases = [
        // #435: `pass` is now supported (filtered as a no-op in
        // `lower_body`), so use `with open("x") as f: pass` — a valid
        // Python statement that is still unsupported — to exercise the
        // C0001 capability error path in statement positions.
        (
            "function body",
            "def _f():\n    with open(\"x\") as f:\n        pass\n",
        ),
        ("if test", "if (lambda: 1):\n    print(1)\n"),
        (
            "if else body",
            "if True:\n    print(1)\nelse:\n    with open(\"x\") as f:\n        pass\n",
        ),
        ("while test", "while (lambda: 1):\n    print(1)\n"),
        (
            "while body",
            "while True:\n    with open(\"x\") as f:\n        pass\n",
        ),
        (
            "one-argument range stop",
            "for i in range((lambda: 1)):\n    print(i)\n",
        ),
        (
            "two-argument range start",
            "for i in range((lambda: 1), 1):\n    print(i)\n",
        ),
        (
            "two-argument range stop",
            "for i in range(0, (lambda: 1)):\n    print(i)\n",
        ),
        (
            "three-argument range start",
            "for i in range((lambda: 1), 1, 1):\n    print(i)\n",
        ),
        (
            "three-argument range stop",
            "for i in range(0, (lambda: 1), 1):\n    print(i)\n",
        ),
        (
            "three-argument range step",
            "for i in range(0, 1, (lambda: 1)):\n    print(i)\n",
        ),
        (
            "for body",
            "for i in range(1):\n    with open(\"x\") as f:\n        pass\n",
        ),
        ("return value", "def _f():\n    return (lambda: 1)\n"),
        (
            "elif test",
            "if True:\n    print(1)\nelif (lambda: 1):\n    print(2)\n",
        ),
        (
            "elif body",
            "if True:\n    print(1)\nelif True:\n    with open(\"x\") as f:\n        pass\n",
        ),
        (
            "nested else body",
            "if True:\n    print(1)\nelif True:\n    print(2)\nelse:\n    with open(\"x\") as f:\n        pass\n",
        ),
        ("binary left operand", "x = (lambda: 1) + 1\n"),
        ("binary right operand", "x = 1 + (lambda: 1)\n"),
        ("f-string interpolation", "x = f\"{(lambda: 1)}\"\n"),
        ("comparison left operand", "x = (lambda: 1) == 1\n"),
        ("comparison right operand", "x = 1 == (lambda: 1)\n"),
    ];

    for (container, source) in cases {
        let module = pycc_parser_test_helper::parse(source);
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "C0001", "wrong diagnostic for {container}");
        assert!(
            diagnostic
                .message
                .contains("expression kind not supported yet")
                || diagnostic
                    .message
                    .contains("statement kind not supported yet"),
            "wrong message for {container}: {}",
            diagnostic.message
        );
        assert!(diagnostic.span.is_some(), "missing span for {container}");
    }
}

#[test]
fn lowers_a_function_definition_without_calling_it() {
    // Defining `main` alone has no observable effect -- matches
    // CPython exactly (confirmed empirically: `python3.14 hello.py`
    // on this exact source prints nothing). Only an explicit call
    // (see the next test) makes it run.
    let module = pycc_parser_test_helper::parse("def main() -> None:\n    print(42)\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::Function {
            name: "main".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::IntLiteral(42)],
            })],
        }]
    );
}

#[test]
fn lowers_a_call_to_a_user_defined_function() {
    let module = pycc_parser_test_helper::parse("def main() -> None:\n    print(42)\n\nmain()\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![
            HirItem::Function {
                name: "main".to_string(),
                params: vec![],
                return_ty: Ty::None,
                body: vec![HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::IntLiteral(42)],
                })],
            },
            HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                callee: "main".to_string(),
                args: vec![],
            })),
        ]
    );
}

#[test]
fn lowers_top_level_print_with_no_main() {
    let module = pycc_parser_test_helper::parse("print(42)\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::IntLiteral(42)],
        }))]
    );
}

#[test]
fn lowers_a_name_reference_used_as_a_call_argument() {
    // Exercises HirExpr::Name specifically -- every other test so far
    // only ever passes an IntLiteral or zero args to a call, never a
    // bare name reference used as a *value* (as opposed to an
    // assignment target, which Task 6 handles separately).
    let module = pycc_parser_test_helper::parse("def f() -> None:\n    print(x)\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::Name("x".to_string())],
            })],
        }]
    );
}

#[test]
fn a_bare_boolean_literal_expression_is_now_supported() {
    let module = pycc_parser_test_helper::parse("True\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(
            HirExpr::BoolLiteral(true)
        ))]
    );
}

#[test]
fn lowers_an_assignment_and_a_later_reference_to_it() {
    let module = pycc_parser_test_helper::parse("x = 1\nprint(x)\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            }),
            HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::Name("x".to_string())],
            })),
        ]
    );
}

#[test]
fn lowers_a_binary_addition() {
    let module = pycc_parser_test_helper::parse("x = 1 + 2\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::Assign {
            target: "x".to_string(),
            value: HirExpr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(HirExpr::IntLiteral(1)),
                right: Box::new(HirExpr::IntLiteral(2)),
            },
        })]
    );
}

#[test]
fn lowers_every_arithmetic_operator() {
    let cases = [
        ("x = 1 - 2\n", BinOpKind::Sub),
        ("x = 1 * 2\n", BinOpKind::Mul),
        ("x = 1 / 2\n", BinOpKind::Div),
        ("x = 1 // 2\n", BinOpKind::FloorDiv),
        ("x = 1 % 2\n", BinOpKind::Mod),
        ("x = 1 ** 2\n", BinOpKind::Pow),
    ];
    for (source, expected_op) in cases {
        let module = pycc_parser_test_helper::parse(source);
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::BinOp {
                    op: expected_op,
                    left: Box::new(HirExpr::IntLiteral(1)),
                    right: Box::new(HirExpr::IntLiteral(2)),
                },
            })],
            "wrong lowering for {source:?}"
        );
    }
}

#[test]
fn lowers_a_float_literal() {
    let module = pycc_parser_test_helper::parse("x = 1.5\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::Assign {
            target: "x".to_string(),
            value: HirExpr::FloatLiteral(1.5),
        })]
    );
}

#[test]
fn a_multi_target_assignment_is_unsupported() {
    assert_capability_error_message(
        "x = y = 1\n",
        "only a single assignment target is supported so far",
    );
}

#[test]
fn assigning_to_an_attribute_target_lowers_to_attr_set() {
    // D-154 (Part 1 of #375) supersedes this test's own former
    // "`x.attr = 1` is unsupported" invariant: attribute-assignment
    // targets are now structurally recognized (`HirStmt::AttrSet`) for
    // every class method's own `self.<attr> = ...` writes, and -- since
    // this lowering step has no type information, mirroring
    // `HirStmt::DictSet`'s own bare-name-base precedent -- for any other
    // base expression too. `pycc_types` rejects a base that isn't
    // actually a class instance, or an attribute name the base's class
    // never declares.
    let module = pycc_parser_test_helper::parse("x.attr = 1\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::AttrSet {
            base: HirExpr::Name("x".to_string()),
            attr: "attr".to_string(),
            value: HirExpr::IntLiteral(1),
        })]
    );
}

#[test]
fn assigning_to_an_attribute_target_propagates_an_unsupported_base_expression() {
    // Exercises the `?` inside `Stmt::Assign`'s own `Expr::Attribute`
    // arm's `base` lowering (D-154), mirroring
    // `method_call_propagates_an_unsupported_base_expression` above.
    let module = pycc_parser_test_helper::parse("(1j).attr = 1\n");
    let diagnostic = lower_checked(&module).unwrap_err();

    assert_eq!(diagnostic.code, "C0001");
}

#[test]
fn assigning_to_a_tuple_unpacking_target_is_unsupported() {
    // The remaining assignment-target shape this file still rejects
    // after `Expr::Name`/`Expr::Subscript`/`Expr::Attribute` are all
    // now recognized: multi-target unpacking (`a, b = ...`) has no HIR
    // shape at all yet.
    assert_capability_error_message(
        "a, b = 1, 2\n",
        "only assigning to a bare name is supported so far",
    );
}

#[test]
fn subscript_assignment_to_a_bare_name_base_lowers_to_dict_set() {
    // PR-11 Task 3 (D-123) supersedes D-105's "no subscript assignment
    // target anywhere in this file" invariant this test used to lock in
    // (`list[int]` alone stayed read-only-indexed; `dict[str, int]`
    // ships `d[k] = v`). This lowering step has no type information (the
    // same reason `ForList`'s own bare-name iterable isn't type-checked
    // here either), so `x[0] = 1` lowers to `HirStmt::DictSet`
    // regardless of whether `x` actually turns out to be a `list` or a
    // `dict` -- `pycc_types` now owns rejecting a `list`-typed base with
    // `T0033` (see that crate's own test module), relocating rather than
    // removing the read-only-list invariant.
    let module = pycc_parser_test_helper::parse("x[0] = 1\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::DictSet {
            dict: "x".to_string(),
            key: HirExpr::IntLiteral(0),
            value: HirExpr::IntLiteral(1),
        })]
    );
}

#[test]
fn subscript_assignment_to_a_non_bare_name_base_is_unsupported() {
    // `f()[0] = 1` has no plain variable name to record as `DictSet`'s
    // own `dict` field -- rejected explicitly rather than guessed at.
    assert_capability_error_message(
        "f()[0] = 1\n",
        "only assigning to a bare-name subscript target (`name[key] = value`) is supported so far",
    );
}

#[test]
fn a_dict_set_target_with_an_unsupported_key_propagates_the_key_error() {
    // (1, 2) no longer fails to lower (this task) -- lambda is still
    // unsupported and exercises the identical propagation path.
    assert_capability_error_message("x[lambda: 1] = 1\n", "expression kind not supported yet");
}

#[test]
fn a_dict_set_target_with_an_unsupported_value_propagates_the_value_error() {
    // (1, 2) no longer fails to lower (this task) -- lambda is still
    // unsupported and exercises the identical propagation path.
    assert_capability_error_message("x[0] = lambda: 1\n", "expression kind not supported yet");
}

#[test]
fn matrix_multiplication_is_unsupported() {
    assert_capability_error_message("x = a @ b\n", "binary operator not supported yet");
}

#[test]
fn a_with_statement_is_unsupported() {
    // `with` is valid Python but not implemented — it exercises the
    // same catch-all that
    // `unsupported_statement_and_expression_return_spanned_capability_diagnostics`
    // does for expressions. (#435: `pass` was previously used here but
    // is now supported as a no-op for PEP 487 hook bodies.)
    assert_capability_error_message(
        "with open(\"x\") as f:\n    pass\n",
        "statement kind not supported yet",
    );
}

#[test]
fn a_bare_literal_expression_statement_is_now_supported() {
    // `42` alone at module level is legal (if pointless) Python -- an
    // expression statement whose value is simply discarded. The old
    // HIR shape only ever represented a *call* expression statement,
    // so this used to panic; HirExpr::IntLiteral now represents any
    // expression, not just call arguments.
    let module = pycc_parser_test_helper::parse("42\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(
            HirExpr::IntLiteral(42)
        ))]
    );
}

#[test]
fn non_name_callee_is_unsupported() {
    // `foo.bar()` no longer reaches this message (Task 7, D-105): a
    // `.` callee is now checked for the `.append()` shape first, and
    // rejected with its own dedicated message when it isn't `.append`
    // (see `calling_a_non_append_method_is_unsupported`). This fixture
    // instead calls the *result* of a call -- neither a bare name nor
    // an attribute access -- to keep exercising the fallback rejection.
    assert_capability_error_message("foo()()\n", "only calling a bare name");
}

#[test]
fn calling_a_zero_arg_function_other_than_print_is_supported() {
    let module = pycc_parser_test_helper::parse("foo()\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
            callee: "foo".to_string(),
            args: vec![],
        }))]
    );
}

#[test]
fn calling_a_non_print_function_with_arguments_is_now_supported() {
    // This used to panic in the pre-Task-5 HIR shape (only zero-arg
    // user-function calls were representable at all) -- HirExpr::Call
    // now carries arbitrary args for every callee, print included;
    // real type-checking of a call's arguments against a declared
    // signature is Task 9's job, not this lowering step's.
    let module = pycc_parser_test_helper::parse("foo(42)\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
            callee: "foo".to_string(),
            args: vec![HirExpr::IntLiteral(42)],
        }))]
    );
}

#[test]
fn print_with_more_than_one_argument_is_now_supported_at_the_hir_level() {
    // Same rationale as above -- HirExpr::Call no longer special-cases
    // `print`'s arity at lowering time.
    let module = pycc_parser_test_helper::parse("print(1, 2)\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
        }))]
    );
}

#[test]
fn print_with_a_float_argument_is_now_supported_at_the_hir_level() {
    // MIR/codegen still only understands an integer-literal argument to
    // `print` (see pycc_mir::lower_instr); this is HIR-only.
    let module = pycc_parser_test_helper::parse("print(2.5)\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::FloatLiteral(2.5)],
        }))]
    );
}

#[test]
fn print_with_an_integer_too_large_for_i64_is_unsupported() {
    assert_capability_error_message(
        "print(99999999999999999999999999999999)\n",
        "does not fit in i64",
    );
}

#[test]
fn a_complex_number_literal_is_unsupported() {
    // Complex isn't in v0.1's type-representation table (int/float/bool/str/None
    // per TYPE_SYSTEM.md) -- unlike float/bool, this isn't deferred to a later
    // PR-4 task, it's simply out of scope for pycc entirely.
    assert_capability_error_message("x = 3j\n", "numeric literal kind not supported yet");
}

#[test]
fn lowers_a_boolean_literal() {
    let module = pycc_parser_test_helper::parse("x = True\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::Assign {
            target: "x".to_string(),
            value: HirExpr::BoolLiteral(true),
        })]
    );
}

#[test]
fn lowers_a_single_comparison() {
    let module = pycc_parser_test_helper::parse("x = 1 < 2\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::Assign {
            target: "x".to_string(),
            value: HirExpr::Compare {
                op: CmpOpKind::Lt,
                left: Box::new(HirExpr::IntLiteral(1)),
                right: Box::new(HirExpr::IntLiteral(2)),
            },
        })]
    );
}

#[test]
fn lowers_every_comparison_operator() {
    let cases = [
        ("x = 1 == 2\n", CmpOpKind::Eq),
        ("x = 1 != 2\n", CmpOpKind::NotEq),
        ("x = 1 < 2\n", CmpOpKind::Lt),
        ("x = 1 <= 2\n", CmpOpKind::LtE),
        ("x = 1 > 2\n", CmpOpKind::Gt),
        ("x = 1 >= 2\n", CmpOpKind::GtE),
    ];
    for (source, expected_op) in cases {
        let module = pycc_parser_test_helper::parse(source);
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::Compare {
                    op: expected_op,
                    left: Box::new(HirExpr::IntLiteral(1)),
                    right: Box::new(HirExpr::IntLiteral(2)),
                },
            })],
            "wrong lowering for {source:?}"
        );
    }
}

#[test]
fn a_chained_comparison_is_not_supported_yet() {
    assert_capability_error_message("x = 1 < 2 < 3\n", "chained comparisons");
}

#[test]
fn an_is_comparison_is_not_supported_yet() {
    assert_capability_error_message("x = 1 is 2\n", "comparison operator not supported yet");
}

#[test]
fn an_is_not_comparison_between_two_non_none_operands_is_not_supported_yet() {
    // Sibling of `an_is_comparison_is_not_supported_yet` for `is not`
    // (D-197, #763, Part 1 of #747): `is_none_operand_shape` is `false`
    // here too, since neither side is `Expr::NoneLiteral`, so this still
    // falls through to the pre-existing `C0001` rejection unchanged.
    assert_capability_error_message("x = 1 is not 2\n", "comparison operator not supported yet");
}

#[test]
fn none_literal_lowers_to_a_bare_hir_none_literal() {
    let module = pycc_parser_test_helper::parse("x = None\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::Assign {
            target: "x".to_string(),
            value: HirExpr::NoneLiteral,
        })]
    );
}

#[test]
fn x_is_none_lowers_to_a_cmp_is_comparison() {
    // `None` on the right (D-197, #763, Part 1 of #747): exercises
    // `is_none_operand_shape`'s `cmp.comparators[0]` check.
    let module = pycc_parser_test_helper::parse("x = 1\ny = x is None\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items[1],
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: "y".to_string(),
            value: HirExpr::Compare {
                op: CmpOpKind::Is,
                left: Box::new(HirExpr::Name("x".to_string())),
                right: Box::new(HirExpr::NoneLiteral),
            },
        })
    );
}

#[test]
fn none_is_not_x_lowers_to_a_cmp_is_not_comparison() {
    // `None` on the left this time (D-197, #763, Part 1 of #747):
    // exercises `is_none_operand_shape`'s `cmp.left` check, the other half
    // of the `||` this test's sibling above does not reach.
    let module = pycc_parser_test_helper::parse("x = 1\ny = None is not x\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items[1],
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: "y".to_string(),
            value: HirExpr::Compare {
                op: CmpOpKind::IsNot,
                left: Box::new(HirExpr::NoneLiteral),
                right: Box::new(HirExpr::Name("x".to_string())),
            },
        })
    );
}

#[test]
fn int_or_none_annotation_produces_ty_optional_int() {
    let module = pycc_parser_test_helper::parse("def f(x: int | None) -> None:\n    pass\n");
    let hir = lower_checked(&module).unwrap();
    let HirItem::Function { params, .. } = &hir.items[0] else {
        panic!("expected a function item");
    };
    assert_eq!(params[0].1, Ty::Optional(Box::new(Ty::Int)));
}

#[test]
fn none_or_int_annotation_also_produces_ty_optional_int() {
    // The other operand order (D-197, #763, Part 1 of #747): exercises
    // `annotation_to_ty`'s `(Expr::NoneLiteral(_), other)` match arm rather
    // than its `(other, Expr::NoneLiteral(_))` sibling.
    let module = pycc_parser_test_helper::parse("def f(x: None | int) -> None:\n    pass\n");
    let hir = lower_checked(&module).unwrap();
    let HirItem::Function { params, .. } = &hir.items[0] else {
        panic!("expected a function item");
    };
    assert_eq!(params[0].1, Ty::Optional(Box::new(Ty::Int)));
}

#[test]
fn an_int_or_none_return_annotation_produces_ty_optional_int() {
    let module = pycc_parser_test_helper::parse("def f() -> int | None:\n    return None\n");
    let hir = lower_checked(&module).unwrap();
    let HirItem::Function { return_ty, .. } = &hir.items[0] else {
        panic!("expected a function item");
    };
    assert_eq!(*return_ty, Ty::Optional(Box::new(Ty::Int)));
}

#[test]
fn a_general_union_annotation_with_neither_side_none_produces_t0048() {
    let module = pycc_parser_test_helper::parse("def f(x: int | str) -> None:\n    pass\n");
    let diag = lower_checked(&module).unwrap_err();
    assert_eq!(diag.code, "T0048");
}

#[test]
fn a_three_way_union_chain_also_produces_t0048() {
    // `A | B | None` (D-197, #763, Part 1 of #747): confirms the scope cut
    // is on the syntactic 2-operand shape, not merely on whether `None`
    // appears anywhere in the chain -- `ast::BinOp` associates `|` left,
    // so the outer node's `left` is itself a `BinOp`, never
    // `Expr::NoneLiteral`, landing in the same `_ =>` T0048 arm as
    // `int | str`.
    let module = pycc_parser_test_helper::parse("def f(x: int | str | None) -> None:\n    pass\n");
    let diag = lower_checked(&module).unwrap_err();
    assert_eq!(diag.code, "T0048");
}

#[test]
fn an_optional_of_a_non_int_type_produces_t0049() {
    let module = pycc_parser_test_helper::parse("def f(x: str | None) -> None:\n    pass\n");
    let diag = lower_checked(&module).unwrap_err();
    assert_eq!(diag.code, "T0049");
}

#[test]
fn a_float_or_none_return_annotation_produces_ty_optional_float() {
    // #809 (Part 3 of #747): widens `T0049` to also admit `T == float`.
    let module = pycc_parser_test_helper::parse("def f() -> float | None:\n    return None\n");
    let hir = lower_checked(&module).unwrap();
    let HirItem::Function { return_ty, .. } = &hir.items[0] else {
        panic!("expected a function item");
    };
    assert_eq!(*return_ty, Ty::Optional(Box::new(Ty::Float)));
}

#[test]
fn a_bool_or_none_return_annotation_produces_ty_optional_bool() {
    // #809 (Part 3 of #747): widens `T0049` to also admit `T == bool`.
    let module = pycc_parser_test_helper::parse("def f() -> bool | None:\n    return None\n");
    let hir = lower_checked(&module).unwrap();
    let HirItem::Function { return_ty, .. } = &hir.items[0] else {
        panic!("expected a function item");
    };
    assert_eq!(*return_ty, Ty::Optional(Box::new(Ty::Bool)));
}

#[test]
fn a_tuple_literal_expression_lowers_successfully() {
    // Tuple literals were this file's own "genuinely unhandled at every
    // level" fixture before this task (list/dict/set literals filled
    // that role earlier and became supported in turn). Now that
    // `Expr::Tuple` has a real arm, this asserts the actual shape
    // rather than a lowering failure -- `pycc_types` (not this crate)
    // now owns which element types/index forms are valid (D-116).
    let module = pycc_parser_test_helper::parse("x = (1, 2)\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items[0],
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: "x".to_string(),
            value: HirExpr::TupleLiteral(vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)]),
        })
    );
}

#[test]
fn lowers_an_if_with_no_else() {
    let module = pycc_parser_test_helper::parse("if True:\n    print(1)\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::If {
            test: HirExpr::BoolLiteral(true),
            body: vec![HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::IntLiteral(1)],
            })],
            orelse: vec![],
        })]
    );
}

#[test]
fn lowers_an_if_with_an_else() {
    let module = pycc_parser_test_helper::parse("if True:\n    print(1)\nelse:\n    print(2)\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::If {
            test: HirExpr::BoolLiteral(true),
            body: vec![HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::IntLiteral(1)],
            })],
            orelse: vec![HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::IntLiteral(2)],
            })],
        })]
    );
}

#[test]
fn lowers_an_elif_as_a_nested_if_in_orelse() {
    let module = pycc_parser_test_helper::parse(
        "if False:\n    print(1)\nelif True:\n    print(2)\nelse:\n    print(3)\n",
    );
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::If {
            test: HirExpr::BoolLiteral(false),
            body: vec![HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::IntLiteral(1)],
            })],
            orelse: vec![HirStmt::If {
                test: HirExpr::BoolLiteral(true),
                body: vec![HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::IntLiteral(2)],
                })],
                orelse: vec![HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::IntLiteral(3)],
                })],
            }],
        })]
    );
}

#[test]
fn folds_a_bare_type_checking_guard_to_an_empty_dead_body() {
    // #790: `from typing import TYPE_CHECKING; if TYPE_CHECKING: ...` must
    // fold to a literal-false test with an empty body -- the guarded body's
    // `import` (unsupported when nested inside a block) never reaches
    // `lower_stmt`, so `lower_checked` succeeds instead of failing with
    // `C0001`.
    let module = pycc_parser_test_helper::parse(
        "from typing import TYPE_CHECKING\nif TYPE_CHECKING:\n    import some_module_that_does_not_exist_at_runtime_or_compile_time\n",
    );
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::If {
            test: HirExpr::BoolLiteral(false),
            body: vec![],
            orelse: vec![],
        })]
    );
}

#[test]
fn folds_a_qualified_type_checking_guard_to_an_empty_dead_body() {
    // #790: the qualified `import typing; if typing.TYPE_CHECKING:` spelling
    // gets the identical fold as the bare-name form.
    let module = pycc_parser_test_helper::parse(
        "import typing\nif typing.TYPE_CHECKING:\n    import some_module_that_does_not_exist_at_runtime_or_compile_time\n",
    );
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::If {
            test: HirExpr::BoolLiteral(false),
            body: vec![],
            orelse: vec![],
        })]
    );
}

#[test]
fn an_attribute_named_type_checking_on_another_receiver_is_not_folded() {
    // #790 (D-068 review): `is_type_checking_guard`'s qualified-form arm
    // requires *both* the attribute name `TYPE_CHECKING` *and* a `typing`
    // receiver -- an attribute access that happens to be named
    // `TYPE_CHECKING` on some other object must not be folded, and must
    // lower as an ordinary `if` whose test is a plain attribute read.
    let module =
        pycc_parser_test_helper::parse("if some_other_module.TYPE_CHECKING:\n    print(1)\n");
    let hir = lower_checked(&module).unwrap();
    let HirItem::TopLevelStmt(HirStmt::If { test, body, orelse }) = &hir.items[0] else {
        panic!(
            "expected the `if` statement to lower to `HirStmt::If`, got {:?}",
            hir.items[0]
        );
    };
    assert_eq!(
        *test,
        HirExpr::AttrGet {
            base: Box::new(HirExpr::Name("some_other_module".to_string())),
            attr: "TYPE_CHECKING".to_string(),
        }
    );
    assert_eq!(
        *body,
        vec![HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::IntLiteral(1)],
        })]
    );
    assert_eq!(*orelse, Vec::<HirStmt>::new());
}

#[test]
fn lowers_the_else_branch_of_a_type_checking_guard_normally() {
    // #790: only the `TYPE_CHECKING` branch itself is dead code -- an
    // `else` clause is live at runtime whenever the guard is skipped, so it
    // must still be lowered exactly like any other `if`/`else`.
    let module = pycc_parser_test_helper::parse(
        "from typing import TYPE_CHECKING\nif TYPE_CHECKING:\n    import some_module_that_does_not_exist_at_runtime_or_compile_time\nelse:\n    print(1)\n",
    );
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::If {
            test: HirExpr::BoolLiteral(false),
            body: vec![],
            orelse: vec![HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::IntLiteral(1)],
            })],
        })]
    );
}

#[test]
fn folds_an_elif_type_checking_guard_to_an_empty_dead_body() {
    // #790: `elif TYPE_CHECKING:` gets the same constant-fold as a leading
    // `if TYPE_CHECKING:`, nested inside the enclosing `else` the same way
    // `lowers_an_elif_as_a_nested_if_in_orelse` shows for an ordinary `elif`.
    let module = pycc_parser_test_helper::parse(
        "from typing import TYPE_CHECKING\nif False:\n    print(1)\nelif TYPE_CHECKING:\n    import some_module_that_does_not_exist_at_runtime_or_compile_time\nelse:\n    print(2)\n",
    );
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::If {
            test: HirExpr::BoolLiteral(false),
            body: vec![HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::IntLiteral(1)],
            })],
            orelse: vec![HirStmt::If {
                test: HirExpr::BoolLiteral(false),
                body: vec![],
                orelse: vec![HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::IntLiteral(2)],
                })],
            }],
        })]
    );
}

#[test]
fn a_lowering_error_in_the_else_after_an_if_type_checking_guard_propagates() {
    // #790: the leading `if TYPE_CHECKING:` arm still lowers its `orelse`
    // chain normally (only the guard's own body is dead) -- proves the `?`
    // on that recursive `lower_elif_else_clauses` call actually propagates
    // a genuine error from a live `else`, not just the empty-`orelse`
    // success path every other test above exercises.
    assert_capability_error_message(
        "from typing import TYPE_CHECKING\nif TYPE_CHECKING:\n    pass\nelse:\n    import some_module_that_does_not_exist_at_runtime_or_compile_time\n",
        "statement kind not supported yet",
    );
}

#[test]
fn a_lowering_error_after_an_elif_type_checking_guard_propagates() {
    // #790: same as above, but for the `elif TYPE_CHECKING:` fold's own
    // recursive `orelse` lowering (a separate `?` in
    // `lower_elif_else_clauses`, reached only once an earlier ordinary
    // branch is already live).
    assert_capability_error_message(
        "from typing import TYPE_CHECKING\nif False:\n    pass\nelif TYPE_CHECKING:\n    pass\nelse:\n    import some_module_that_does_not_exist_at_runtime_or_compile_time\n",
        "statement kind not supported yet",
    );
}

#[test]
fn lowers_a_while_loop() {
    let module = pycc_parser_test_helper::parse("while True:\n    print(1)\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::While {
            test: HirExpr::BoolLiteral(true),
            body: vec![HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::IntLiteral(1)],
            })],
        })]
    );
}

#[test]
fn a_while_else_is_not_supported_yet() {
    assert_capability_error_message(
        "while True:\n    print(1)\nelse:\n    print(2)\n",
        "while/else is not supported yet",
    );
}

#[test]
fn lowers_a_for_range_loop_with_one_argument() {
    let module = pycc_parser_test_helper::parse("for i in range(3):\n    print(i)\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::ForRange {
            var: "i".to_string(),
            start: HirExpr::IntLiteral(0),
            stop: HirExpr::IntLiteral(3),
            step: HirExpr::IntLiteral(1),
            body: vec![HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::Name("i".to_string())],
            })],
        })]
    );
}

#[test]
fn lowers_a_for_range_loop_with_start_and_stop() {
    let module = pycc_parser_test_helper::parse("for i in range(1, 3):\n    print(i)\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::ForRange {
            var: "i".to_string(),
            start: HirExpr::IntLiteral(1),
            stop: HirExpr::IntLiteral(3),
            step: HirExpr::IntLiteral(1),
            body: vec![HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::Name("i".to_string())],
            })],
        })]
    );
}

#[test]
fn lowers_a_for_range_loop_with_start_stop_and_step() {
    let module = pycc_parser_test_helper::parse("for i in range(0, 6, 2):\n    print(i)\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::ForRange {
            var: "i".to_string(),
            start: HirExpr::IntLiteral(0),
            stop: HirExpr::IntLiteral(6),
            step: HirExpr::IntLiteral(2),
            body: vec![HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::Name("i".to_string())],
            })],
        })]
    );
}

#[test]
fn iterating_a_non_call_expression_is_not_supported_yet() {
    // A list *literal* iterable is still rejected (Task 7/D-105 only
    // added support for a bare-name iterable, i.e. `for v in some_list:`
    // -- see `lowers_for_over_a_list_name_to_for_list`); the rejection
    // message itself changed to mention that new bare-name-list form.
    assert_capability_error_message(
        "for i in [1, 2, 3]:\n    print(i)\n",
        "only `for x in range(...)` or `for x in <list>` is supported so far",
    );
}

#[test]
fn calling_something_other_than_range_in_a_for_is_not_supported_yet() {
    assert_capability_error_message(
        "for i in items(3):\n    print(i)\n",
        "only iterating over `range(...)` is supported so far",
    );
}

#[test]
fn calling_via_an_attribute_in_a_for_is_not_supported_yet() {
    assert_capability_error_message(
        "for i in a.b(3):\n    print(i)\n",
        "only `for x in range(...)` is supported so far",
    );
}

#[test]
fn range_with_too_many_arguments_is_not_supported() {
    assert_capability_error_message(
        "for i in range(1, 2, 3, 4):\n    print(i)\n",
        "range() with 4 arguments is not supported",
    );
}

#[test]
fn a_tuple_for_target_is_not_supported_yet() {
    assert_capability_error_message(
        "for i, j in range(3):\n    print(i)\n",
        "only a bare name for-target is supported so far",
    );
}

#[test]
fn a_for_else_is_not_supported_yet() {
    assert_capability_error_message(
        "for i in range(3):\n    print(i)\nelse:\n    print(0)\n",
        "for/else is not supported yet",
    );
}

#[test]
fn an_async_for_inside_an_async_function_is_not_supported_yet() {
    // `async for` is only valid Python syntax inside an `async def` body,
    // so this now hits the (newer, more general) async-function rejection
    // in lower_function before lower_stmt's own `for_stmt.is_async` check
    // is ever reached -- the fixture still exercises real, valid Python
    // that must be rejected, just via the outer boundary now.
    assert_capability_error_message(
        "async def f() -> None:\n    async for i in range(3):\n        print(i)\n",
        "async functions are not supported yet",
    );
}

#[test]
fn a_top_level_async_for_is_context_invalid() {
    // Issue #141 / D-148: `async for` outside an async function is
    // syntactically well-formed but CPython rejects it as a
    // `SyntaxError`, so this is now `L0001`, not `C0001` -- there is no
    // reachable "valid, just unimplemented" case here (see Correction 2
    // in the published plan): `lower_function` already rejects any
    // `async def` before this arm could ever be reached from inside a
    // real async function.
    assert_context_invalid_error_message(
        "async for i in range(3):\n    print(i)\n",
        "'async for' outside async function",
    );
}

#[test]
fn an_async_for_inside_a_synchronous_function_is_context_invalid() {
    // The issue's own "other invalid async context" example beyond top
    // level: a synchronous `def` body reaches `lower_function`'s body
    // lowering (which resets `in_loop` but not any async-context state),
    // then this `for_stmt.is_async` arm -- unconditionally `L0001`,
    // exactly like the top-level case, since a synchronous function
    // provides no more of an "async function" context than module scope
    // does.
    assert_context_invalid_error_message(
        "def f() -> None:\n    async for i in range(3):\n        print(i)\n",
        "'async for' outside async function",
    );
}

#[test]
fn a_top_level_break_is_context_invalid() {
    assert_context_invalid_error_message("break\n", "'break' outside loop");
}

#[test]
fn a_top_level_continue_is_context_invalid() {
    assert_context_invalid_error_message("continue\n", "'continue' not properly in loop");
}

#[test]
fn a_break_inside_a_synchronous_function_with_no_loop_is_context_invalid() {
    // Regression guard for `lower_function`'s own `false` call site
    // (entering a function body resets `in_loop`): a `break` directly in
    // a function body, with no enclosing loop, must still be
    // context-invalid, not silently inherit `true` from some outer
    // caller state.
    assert_context_invalid_error_message("def f() -> None:\n    break\n", "'break' outside loop");
}

#[test]
fn a_break_inside_a_for_loop_is_still_unsupported() {
    // Regression guard: a real enclosing loop keeps break/continue on
    // the existing valid-but-unimplemented `C0001` path -- this issue is
    // scoped to classification, not to implementing loop control flow.
    assert_capability_error_message(
        "for i in range(3):\n    break\n",
        "statement kind not supported yet",
    );
}

#[test]
fn a_continue_inside_a_while_loop_is_still_unsupported() {
    assert_capability_error_message(
        "while True:\n    continue\n",
        "statement kind not supported yet",
    );
}

#[test]
fn a_break_inside_an_if_inside_a_for_loop_is_still_unsupported() {
    // Guards the `If` arm's and `lower_elif_else_clauses`' pass-through
    // of the caller's `in_loop` value: an `if` nested inside a loop body
    // must not reset loop context to `false`.
    assert_capability_error_message(
        "for i in range(3):\n    if i:\n        break\n",
        "statement kind not supported yet",
    );
}

// -- PEP 765 (#738, Part 1 of #543): `return`/`break`/`continue` inside a
// `finally` block --
//
// The exact shape below (one `in_finally` flag shared by all three
// statement kinds, reset by the nearest enclosing loop reached from inside
// the `finally` but not by a plain conditional or a nested non-`finally`
// `try` part) was verified empirically against CPython 3.14's actual
// `SyntaxWarning` behavior rather than assumed from the PEP text alone --
// see `stmt.rs`'s own module doc comment for the full account, including
// the naive-but-wrong two-flag design this rejected (a `return` inside a
// loop defined inside the `finally` is, perhaps counter-intuitively, NOT
// flagged by CPython even though it still exits the function).

#[test]
fn a_return_directly_in_a_finally_block_is_context_invalid() {
    assert_context_invalid_error_message(
        "def f() -> int:\n    try:\n        pass\n    finally:\n        return 3\n",
        "'return' in a 'finally' block",
    );
}

#[test]
fn a_break_directly_in_a_finally_block_is_context_invalid() {
    // The classic PEP 765 pattern: a real enclosing loop exists outside
    // the `try`/`finally` (`in_loop` is `true`), but the `finally`
    // violation takes precedence over the pre-existing valid-but-
    // unimplemented `C0001` path.
    assert_context_invalid_error_message(
        "def f() -> None:\n    while True:\n        try:\n            pass\n        finally:\n            break\n",
        "'break' in a 'finally' block",
    );
}

#[test]
fn a_continue_directly_in_a_finally_block_is_context_invalid() {
    assert_context_invalid_error_message(
        "def f() -> None:\n    while True:\n        try:\n            pass\n        finally:\n            continue\n",
        "'continue' in a 'finally' block",
    );
}

#[test]
fn a_return_nested_under_if_inside_a_finally_block_is_still_context_invalid() {
    // Guards the `If` arm's pass-through of `in_finally`: a conditional
    // does not shield a `return` from the enclosing `finally`.
    assert_context_invalid_error_message(
        "def f() -> int:\n    try:\n        pass\n    finally:\n        if True:\n            return 3\n",
        "'return' in a 'finally' block",
    );
}

#[test]
fn a_return_in_the_try_body_of_a_nested_try_inside_a_finally_block_is_context_invalid() {
    // Guards the `Try` arm's pass-through of `in_finally` for its own
    // `body`/`handlers`/`orelse` (only entering a *new* `finalbody` forces
    // it back to `true` unconditionally) -- a nested try's non-`finally`
    // parts still inherit the outer `finally`'s restriction.
    assert_context_invalid_error_message(
        "def f() -> int:\n    try:\n        pass\n    finally:\n        try:\n            return 3\n        except ValueError:\n            pass\n",
        "'return' in a 'finally' block",
    );
}

#[test]
fn a_return_in_the_except_body_of_a_nested_try_inside_a_finally_block_is_context_invalid() {
    assert_context_invalid_error_message(
        "def f() -> None:\n    while True:\n        try:\n            pass\n        finally:\n            try:\n                pass\n            except ValueError:\n                break\n",
        "'break' in a 'finally' block",
    );
}

#[test]
fn a_return_in_the_else_body_of_a_nested_try_inside_a_finally_block_is_context_invalid() {
    // Guards the `Try` arm's pass-through of `in_finally` for its own
    // `orelse` (the `else:` clause), the last of the three non-`finally`
    // `Try` parts -- `body` and `handlers` are already covered by the two
    // sibling tests above.
    assert_context_invalid_error_message(
        "def f() -> int:\n    try:\n        pass\n    finally:\n        try:\n            pass\n        except ValueError:\n            pass\n        else:\n            return 3\n",
        "'return' in a 'finally' block",
    );
}

#[test]
fn a_break_in_a_nested_finally_inside_a_finally_block_is_context_invalid() {
    // A nested `try`/`finally`'s own `finalbody`, itself lexically inside
    // an outer `finally`, is a violation on its own -- entering it forces
    // `in_finally` back to `true` regardless of the outer state, so no
    // loop needs to intervene for this to already be true.
    assert_context_invalid_error_message(
        "def f() -> None:\n    while True:\n        try:\n            pass\n        finally:\n            try:\n                pass\n            finally:\n                break\n",
        "'break' in a 'finally' block",
    );
}

#[test]
fn a_return_in_a_loop_defined_inside_a_finally_block_lowers_successfully() {
    // A loop introduced *inside* the finally shields `return` from the
    // enclosing `finally`, exactly like `break`/`continue` (see the two
    // tests below) -- verified directly against CPython 3.14 rather than
    // assumed: despite a naive PEP-text-only reading predicting this
    // *should* still be rejected (a `return` inside the inner loop still
    // exits the function, same as it would without the loop), CPython's
    // actual compiler does not flag it. `Stmt::Return` has no capability
    // gap of its own (unlike `break`/`continue`, whose control-flow
    // codegen is unimplemented for any real loop), so this lowers all the
    // way through successfully rather than landing on `C0001`.
    let module = pycc_parser_test_helper::parse(
        "def f() -> int:\n    try:\n        pass\n    finally:\n        for i in range(3):\n            return i\n",
    );
    lower_checked(&module).expect("a return shielded by an inner loop must lower successfully");
}

#[test]
fn a_break_targeting_a_loop_defined_inside_a_finally_block_is_still_unsupported() {
    // A `break` that targets a loop defined *inside* the `finally` itself
    // does not escape the finally -- it falls through to the ordinary
    // valid-but-unimplemented `C0001` path (break/continue codegen isn't
    // implemented at all yet, independent of this PEP 765 check), not the
    // new `L0001` finally violation.
    assert_capability_error_message(
        "def f() -> None:\n    try:\n        pass\n    finally:\n        for i in range(3):\n            break\n",
        "statement kind not supported yet",
    );
}

#[test]
fn a_continue_targeting_a_loop_defined_inside_a_finally_block_is_still_unsupported() {
    assert_capability_error_message(
        "def f() -> None:\n    try:\n        pass\n    finally:\n        for i in range(3):\n            continue\n",
        "statement kind not supported yet",
    );
}

#[test]
fn a_return_in_a_match_case_inside_a_finally_block_is_context_invalid() {
    // Guards `lower_match`'s pass-through of `in_finally` to each case
    // body.
    assert_context_invalid_error_message(
        "def f(x: int) -> int:\n    try:\n        pass\n    finally:\n        match x:\n            case 1:\n                return 1\n            case _:\n                pass\n",
        "'return' in a 'finally' block",
    );
}

#[test]
fn a_return_in_a_nested_def_inside_a_finally_block_hits_capability_error_not_context_invalid() {
    // A nested function definition has its own return scope and would NOT
    // be a PEP 765 violation in real Python -- but nested `def`s are
    // wholesale unsupported in this compiler today (`Stmt::FunctionDef`
    // has no arm in `lower_stmt`'s match, so it falls to the generic
    // "statement kind not supported yet" catch-all before this check
    // could ever run). This is a negative control proving the new
    // `in_finally` check doesn't misfire on it: the nested `def` itself is
    // what's rejected, with the pre-existing `C0001`, not the new PEP 765
    // `L0001`.
    assert_capability_error_message(
        "def f() -> int:\n    try:\n        pass\n    finally:\n        def g() -> int:\n            return 1\n        return g()\n",
        "statement kind not supported yet",
    );
}

#[test]
fn a_top_level_yield_is_context_invalid() {
    // Issue #361 / D-149, this crate's expression-lowering sequel to
    // #141/D-148: `yield` outside any function is syntactically
    // well-formed but CPython rejects it as a `SyntaxError`, so this is
    // now `L0001`, not `C0001`.
    assert_context_invalid_error_message("yield 1\n", "'yield' outside function");
}

#[test]
fn a_top_level_yield_from_is_context_invalid() {
    assert_context_invalid_error_message("yield from [1, 2]\n", "'yield from' outside function");
}

#[test]
fn a_yield_nested_inside_a_top_level_if_is_still_context_invalid() {
    // Pins that `in_function` correctly stays `false` through `If`/
    // `lower_elif_else_clauses` recursion at module scope -- mirrors the
    // equivalent existing coverage for `break`/`continue` (D-148).
    assert_context_invalid_error_message("if True:\n    yield 1\n", "'yield' outside function");
}

#[test]
fn a_yield_inside_a_real_function_is_still_unsupported() {
    // Regression guard: a real enclosing function keeps `yield` on the
    // existing valid-but-unimplemented `C0001` path -- generator codegen
    // remains out of scope for this issue.
    assert_capability_error_message(
        "def f() -> None:\n    yield 1\n",
        "expression kind not supported yet",
    );
}

#[test]
fn a_yield_from_inside_a_real_function_is_still_unsupported() {
    assert_capability_error_message(
        "def f() -> None:\n    yield from [1, 2]\n",
        "expression kind not supported yet",
    );
}

#[test]
fn a_yield_inside_a_comprehension_if_filter_stays_unsupported_at_module_scope() {
    // Regression-pinning test (D-149 correction 5): a `yield` reached
    // through a comprehension's `if`-filter is governed by a third,
    // scope-independent CPython rule (`'yield' inside list
    // comprehension`) this issue does not implement. The comprehension
    // helper cluster hardcodes a literal `true` at its own internal
    // `lower_expr` call sites instead of forwarding the real ambient
    // `in_function` value, so this must stay `C0001` at module scope,
    // unchanged by this fix -- guarding against a future edit that
    // "helpfully" threads the real value through and starts emitting the
    // wrong-per-CPython `L0001: 'yield' outside function` message here.
    assert_capability_error_message(
        "y = [x for x in range(3) if (yield x)]\n",
        "expression kind not supported yet",
    );
}

#[test]
fn lowers_a_fully_annotated_public_function_with_params_and_return() {
    let module =
        pycc_parser_test_helper::parse("def add(a: int, b: int) -> int:\n    return a + b\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::Function {
            name: "add".to_string(),
            params: vec![("a".to_string(), Ty::Int), ("b".to_string(), Ty::Int)],
            return_ty: Ty::Int,
            body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(HirExpr::Name("a".to_string())),
                right: Box::new(HirExpr::Name("b".to_string())),
            }))],
        }]
    );
}

#[test]
fn lowers_a_return_with_no_value() {
    let module = pycc_parser_test_helper::parse("def f() -> None:\n    return\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::Return(None)],
        }]
    );
}

#[test]
fn an_unannotated_public_function_missing_param_annotations_produces_t0001() {
    let module = pycc_parser_test_helper::parse("def add(a, b) -> int:\n    return a + b\n");
    let diag = lower_checked(&module).unwrap_err();
    assert_eq!(diag.code, "T0001");
    assert!(diag.message.contains('a'));
}

#[test]
fn an_unannotated_public_function_missing_return_annotation_produces_t0001() {
    let module = pycc_parser_test_helper::parse("def add(a: int, b: int):\n    return a + b\n");
    let diag = lower_checked(&module).unwrap_err();
    assert_eq!(diag.code, "T0001");
    assert!(diag.message.contains("add"));
}

#[test]
fn an_unannotated_private_function_is_allowed() {
    let module = pycc_parser_test_helper::parse("def _add(a, b):\n    return a + b\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::Function {
            name: "_add".to_string(),
            params: vec![("a".to_string(), Ty::Infer), ("b".to_string(), Ty::Infer)],
            return_ty: Ty::Infer,
            body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(HirExpr::Name("a".to_string())),
                right: Box::new(HirExpr::Name("b".to_string())),
            }))],
        }]
    );
}

#[test]
fn every_supported_annotation_type_lowers_correctly() {
    let cases = [
        ("def f(x: int) -> int:\n    return x\n", Ty::Int),
        ("def f(x: float) -> float:\n    return x\n", Ty::Float),
        ("def f(x: bool) -> bool:\n    return x\n", Ty::Bool),
        ("def f(x: str) -> str:\n    return x\n", Ty::Str),
        ("def f(x: None) -> None:\n    return x\n", Ty::None),
    ];
    for (source, expected_ty) in cases {
        let module = pycc_parser_test_helper::parse(source);
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::Function {
                name: "f".to_string(),
                params: vec![("x".to_string(), expected_ty.clone())],
                return_ty: expected_ty,
                body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
            }],
            "wrong lowering for {source:?}"
        );
    }
}

#[test]
fn an_unsupported_annotation_type_returns_a_capability_error() {
    assert_capability_error_message(
        "def f(x: list) -> None:\n    return\n",
        "type annotation `list` is not supported yet",
    );
}

#[test]
fn a_non_bare_name_annotation_returns_a_capability_error() {
    assert_capability_error_message(
        "def f(x: a.b) -> None:\n    return\n",
        "only a bare name type annotation is supported so far",
    );
}

#[test]
fn a_default_parameter_value_returns_a_capability_error() {
    // Regression test (self-review finding, pre-merge): lower_params
    // used to only read `.parameter`, silently ignoring `.default` --
    // producing a wrong signature (as if `b` had no default at all)
    // instead of an explicit capability diagnostic.
    assert_capability_error_message(
        "def f(a: int, b: int = 2) -> int:\n    return a + b\n",
        "default parameter values are not supported yet",
    );
}

#[test]
fn a_positional_only_parameter_lowers_successfully() {
    // PEP 570 (#383): positional-only parameters (`/` marker) are now
    // lowered via the same path as ordinary args, prepended before `args`.
    let module =
        pycc_parser_test_helper::parse("def f(a: int, /, b: int) -> int:\n    return a + b\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::Function {
            name: "f".to_string(),
            params: vec![("a".to_string(), Ty::Int), ("b".to_string(), Ty::Int),],
            return_ty: Ty::Int,
            body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(HirExpr::Name("a".to_string())),
                right: Box::new(HirExpr::Name("b".to_string())),
            }))],
        }]
    );
}

#[test]
fn a_positional_only_parameter_with_a_default_value_is_rejected() {
    // PEP 570 (#383): the `lower_arg_list` error path for posonlyargs
    // (default values are unsupported) must fire, not be silently
    // bypassed by the posonlyargs concatenation.
    assert_capability_error_message(
        "def f(a: int = 0, /) -> int:\n    return a\n",
        "default parameter values are not supported yet",
    );
}

#[test]
fn a_keyword_only_parameter_returns_a_capability_error() {
    assert_capability_error_message(
        "def f(a: int, *, b: int) -> int:\n    return a + b\n",
        "keyword-only parameters",
    );
}

#[test]
fn a_vararg_parameter_returns_a_capability_error() {
    assert_capability_error_message(
        "def f(*args: int) -> None:\n    return\n",
        "*args` is not supported yet",
    );
}

#[test]
fn a_kwarg_parameter_returns_a_capability_error() {
    assert_capability_error_message(
        "def f(**kwargs: int) -> None:\n    return\n",
        "**kwargs` is not supported yet",
    );
}

#[test]
fn an_async_function_is_rejected_without_losing_async_semantics() {
    assert_capability_error_message(
        "async def f() -> None:\n    return\n",
        "async functions are not supported yet",
    );
}

#[test]
fn a_decorated_function_is_rejected_without_losing_the_decorator() {
    assert_capability_error_message(
        "@decorator\ndef f() -> None:\n    return\n",
        "function decorators are not supported yet",
    );
}

#[test]
fn a_generic_function_with_two_type_parameters_is_rejected() {
    // D-133: exactly one type parameter is accepted (see
    // `a_single_type_parameter_is_lowered_to_ty_param` below); two or
    // more still hit the frontend arity gate, since the underlying
    // representation and call-site substitution (D-134) are scoped to
    // the single-type-parameter case only.
    assert_capability_error_message(
        "def f[T, U](x: T) -> U:\n    return x\n",
        "generic functions with more than one type parameter are not supported yet",
    );
}

#[test]
fn a_single_type_parameter_is_lowered_to_ty_param() {
    // D-133: `Ty::Param` is resolved by call-site substitution (D-134),
    // not by unification -- this test only asserts the frontend lowers
    // `T` consistently to `Ty::Param("T")` everywhere it appears in the
    // signature, not that substitution happens yet.
    let module = pycc_parser_test_helper::parse("def f[T](x: T) -> T:\n    return x\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::Function {
            name: "f".to_string(),
            params: vec![("x".to_string(), Ty::Param(Box::new("T".to_string())))],
            return_ty: Ty::Param(Box::new("T".to_string())),
            body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
        }]
    );
}

#[test]
fn a_type_var_tuple_type_parameter_is_rejected() {
    // D-133: `Ty::Param` models one `TypeVar` resolved to one concrete
    // scalar via call-site substitution (D-134) -- `*Ts` stands for a
    // variable-length sequence of types instead, which `Ty::Param`
    // cannot represent, so this must be an explicit capability
    // rejection rather than silently treated like a plain `TypeVar`.
    assert_capability_error_message(
        "def f[*Ts](x: int) -> None:\n    return\n",
        "a `TypeVarTuple` type parameter (`*Ts`) is not supported yet",
    );
}

#[test]
fn a_param_spec_type_parameter_is_rejected() {
    // D-133: same reasoning as the `TypeVarTuple` case above -- `**P`
    // stands for a parameter-list shape, not a single scalar type.
    assert_capability_error_message(
        "def f[**P](x: int) -> None:\n    return\n",
        "a `ParamSpec` type parameter (`**P`) is not supported yet",
    );
}

#[test]
fn a_keyword_call_argument_is_rejected_instead_of_being_erased() {
    assert_capability_error_message(
        "def f() -> None:\n    return\n\nf(extra=undefined)\n",
        "keyword call arguments are not supported yet",
    );
}

#[test]
fn a_keyword_range_argument_is_rejected_instead_of_being_erased() {
    assert_capability_error_message(
        "for i in range(stop=3):\n    i\n",
        "keyword arguments to range() are not supported yet",
    );
}

#[test]
fn lowers_a_plain_string_literal() {
    let module = pycc_parser_test_helper::parse("x = \"hi\"\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::Assign {
            target: "x".to_string(),
            value: HirExpr::StringLiteral("hi".to_string()),
        })]
    );
}

#[test]
fn an_any_annotation_produces_t0002() {
    let module = pycc_parser_test_helper::parse("def f(x: Any) -> None:\n    pass\n");
    let diag = lower_checked(&module).unwrap_err();
    assert_eq!(diag.code, "T0002");
}

#[test]
fn an_any_return_annotation_produces_t0002() {
    let module = pycc_parser_test_helper::parse("def f() -> Any:\n    pass\n");
    let diag = lower_checked(&module).unwrap_err();
    assert_eq!(diag.code, "T0002");
}

#[test]
fn lowers_a_basic_f_string_with_one_interpolation() {
    let module = pycc_parser_test_helper::parse("x = 1\ny = f\"value: {x}\"\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items[1],
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: "y".to_string(),
            value: HirExpr::FString(vec![
                FStringPart::Literal("value: ".to_string()),
                FStringPart::Interpolation(Box::new(HirExpr::Name("x".to_string()))),
            ]),
        })
    );
}

#[test]
fn lowers_an_f_string_with_only_literal_parts() {
    let module = pycc_parser_test_helper::parse("y = f\"no interpolation\"\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::Assign {
            target: "y".to_string(),
            value: HirExpr::FString(vec![FStringPart::Literal("no interpolation".to_string())]),
        })]
    );
}

#[test]
fn an_f_string_with_a_format_spec_is_not_supported_yet() {
    assert_capability_error_message("x = 1.5\ny = f\"{x:.2f}\"\n", "format spec");
}

#[test]
fn an_f_string_with_a_conversion_flag_is_not_supported_yet() {
    assert_capability_error_message("x = 1\ny = f\"{x!r}\"\n", "conversion");
}

#[test]
fn lowers_an_annotated_assignment_with_a_value() {
    let module = pycc_parser_test_helper::parse("x: int = 1\nprint(x)\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![
            HirItem::TopLevelStmt(HirStmt::AnnAssign {
                is_final: false,
                target: "x".to_string(),
                annotation: Ty::Int,
                value: Some(HirExpr::IntLiteral(1)),
            }),
            HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::Name("x".to_string())],
            })),
        ]
    );
}

#[test]
fn lowers_an_annotated_assignment_with_no_value() {
    let module = pycc_parser_test_helper::parse("x: int\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::AnnAssign {
            is_final: false,
            target: "x".to_string(),
            annotation: Ty::Int,
            value: None,
        })]
    );
}

#[test]
fn rejects_an_annotated_assignment_to_a_non_name_target() {
    // Unlike `Stmt::Assign` (which now accepts an `Expr::Attribute`
    // target -- `obj.attr = 1` -- as of D-154 Part 1 of #375, see
    // `stmt.rs`'s own comment on that arm), `Stmt::AnnAssign` still only
    // accepts a bare-name target: `obj.attr: int = 1` has no
    // attribute-annotated-assignment support anywhere in the compiler.
    assert_capability_error_message(
        "obj.attr: int = 1\n",
        "only assigning to a bare name is supported so far",
    );
}

#[test]
fn rejects_a_parenthesized_annotated_assignment_target_instead_of_erasing_it() {
    // Regression test (advisor-review finding, pre-merge): `(x): int = 1`
    // still lowers `ann.target` to `Expr::Name("x")` -- identical to the
    // unparenthesized `x: int = 1` -- but upstream's own parser sets
    // `simple = false` for it (verified against the pinned
    // ruff_python_parser = "0.0.6" registry source), a real CPython
    // semantic difference this compiler doesn't model. An earlier draft
    // of this arm only matched on `Expr::Name` and ignored `ann.simple`
    // entirely, silently treating the two forms as identical instead of
    // producing the explicit capability diagnostic this file uses for
    // every other unmodeled AST field (see `lower_params`'s own
    // documented self-review finding above).
    assert_capability_error_message(
        "(x): int = 1\n",
        "a parenthesized annotated-assignment target is not supported yet",
    );
}

#[test]
fn an_annotated_assignment_with_an_unsupported_annotation_returns_a_capability_error() {
    // Exercises the `annotation_to_ty(...)?` early-return branch inside
    // the new AnnAssign arm specifically (as opposed to the already
    // covered function-parameter/return-annotation call sites).
    assert_capability_error_message("x: list\n", "type annotation `list` is not supported yet");
}

#[test]
fn an_annotated_assignment_with_an_unsupported_value_returns_a_capability_error() {
    // Exercises the `lower_expr(...)?` early-return branch for
    // AnnAssign's value expression specifically.
    // (1, 2) no longer fails to lower (this task) -- lambda is still
    // unsupported and exercises the identical propagation path.
    assert_capability_error_message("x: int = lambda: 1\n", "expression kind not supported yet");
}

// -- Task 7 (D-105): list[int] frontend HIR forms --------------------

#[test]
fn lowers_a_list_literal() {
    // Not a `let PATTERN = ... else { panic!(...) }` destructure -- this
    // file's own coverage-gate lesson (see pycc_ast's documented
    // `re_exported_grammar_types_resolve_and_have_the_expected_shape`
    // finding) is that a hand-written panic arm is never taken on the
    // happy path and shows up as a permanently uncovered region under
    // D-014's 100%-regions gate. A direct `assert_eq!` against the whole
    // expected `HirItem` avoids that without weakening the assertion.
    let module = pycc_parser_test_helper::parse("x = [1, 2, 3]\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items[0],
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: "x".to_string(),
            value: HirExpr::ListLiteral(vec![
                HirExpr::IntLiteral(1),
                HirExpr::IntLiteral(2),
                HirExpr::IntLiteral(3),
            ]),
        })
    );
}

#[test]
fn lowers_a_read_subscript() {
    let module = pycc_parser_test_helper::parse("x = [1, 2, 3]\ny = x[0]\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items[1],
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: "y".to_string(),
            value: HirExpr::Subscript {
                base: Box::new(HirExpr::Name("x".to_string())),
                index: Box::new(HirExpr::IntLiteral(0)),
            },
        })
    );
}

#[test]
fn lowers_append_as_a_dedicated_hir_node_not_a_generic_call() {
    let module = pycc_parser_test_helper::parse("x = [1]\nx.append(2)\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items[1],
        HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::ListAppend {
            list: "x".to_string(),
            value: Box::new(HirExpr::IntLiteral(2)),
        }))
    );
}

#[test]
fn list_append_used_as_a_value_lowers_successfully_today() {
    // Real Python's `list.append()` always returns `None`, so using its
    // result as a value (as opposed to a bare `ExprStmt`) is meaningless
    // -- but rejecting that is a type judgment, which is `pycc_types`'
    // job (see `HirExpr::ListAppend`'s own doc comment), not this
    // lowering step's. This test locks in today's actual behavior --
    // `y = x.append(2)` lowers successfully -- so a future change to
    // this arm doesn't silently start rejecting (or accepting some
    // different shape of) value-position `.append()` without its own
    // deliberate decision.
    let module = pycc_parser_test_helper::parse("x = [1]\ny = x.append(2)\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items[1],
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: "y".to_string(),
            value: HirExpr::ListAppend {
                list: "x".to_string(),
                value: Box::new(HirExpr::IntLiteral(2)),
            },
        })
    );
}

#[test]
fn lowers_for_over_a_list_name_to_for_list() {
    let module = pycc_parser_test_helper::parse("x = [1, 2, 3]\nfor v in x:\n    print(v)\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items[1],
        HirItem::TopLevelStmt(HirStmt::ForList {
            var: "v".to_string(),
            list: "x".to_string(),
            body: vec![HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::Name("v".to_string())],
            })],
        })
    );
}

// -- PR-11 Task 3 (D-123): dict[str, int] frontend HIR forms ---------

#[test]
fn lowers_a_dict_literal() {
    let module = pycc_parser_test_helper::parse("x = {\"a\": 1, \"b\": 2}\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items[0],
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: "x".to_string(),
            value: HirExpr::DictLiteral(vec![
                (
                    HirExpr::StringLiteral("a".to_string()),
                    HirExpr::IntLiteral(1),
                ),
                (
                    HirExpr::StringLiteral("b".to_string()),
                    HirExpr::IntLiteral(2),
                ),
            ]),
        })
    );
}

#[test]
fn lowers_an_empty_dict_literal() {
    // `{}` is an empty *dict* literal in Python grammar (an empty set has
    // no literal spelling -- `set()` is a call) -- `pycc_types` rejects
    // it (its element types can't be inferred), but lowering itself
    // succeeds, mirroring `HirExpr::ListLiteral(vec![])`'s own split of
    // responsibility.
    let module = pycc_parser_test_helper::parse("x = {}\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::Assign {
            target: "x".to_string(),
            value: HirExpr::DictLiteral(vec![]),
        })]
    );
}

#[test]
fn dict_unpacking_inside_a_literal_is_unsupported() {
    assert_capability_error_message(
        "x = {**y}\n",
        "dict-unpacking (`**expr`) inside a dict literal is not supported yet",
    );
}

#[test]
fn a_dict_literal_with_an_unsupported_key_propagates_the_key_error() {
    // (1, 2) no longer fails to lower (this task) -- lambda is still
    // unsupported and exercises the identical propagation path.
    assert_capability_error_message(
        "x = {(lambda: 1): 1}\n",
        "expression kind not supported yet",
    );
}

#[test]
fn a_dict_literal_with_an_unsupported_value_propagates_the_value_error() {
    // (1, 2) no longer fails to lower (this task) -- lambda is still
    // unsupported and exercises the identical propagation path.
    assert_capability_error_message(
        "x = {\"a\": (lambda: 1)}\n",
        "expression kind not supported yet",
    );
}

// -- PR-11 Task 7 (D-123): set[int] frontend HIR forms ---------------

#[test]
fn lowers_a_set_literal() {
    let module = pycc_parser_test_helper::parse("x = {1, 2, 3}\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items[0],
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: "x".to_string(),
            value: HirExpr::SetLiteral(vec![
                HirExpr::IntLiteral(1),
                HirExpr::IntLiteral(2),
                HirExpr::IntLiteral(3),
            ]),
        })
    );
}

#[test]
fn a_set_literal_with_an_unsupported_element_propagates_the_element_error() {
    // (1, 2) no longer fails to lower (this task) -- lambda is still
    // unsupported and exercises the identical propagation path.
    assert_capability_error_message("x = {(lambda: 1)}\n", "expression kind not supported yet");
}

#[test]
fn subscripted_type_annotation_with_unknown_base_is_rejected() {
    // #435 (Part D): subscripted type annotations (`ClassName[type_arg]`)
    // are now supported for known class names (PEP 560
    // `__class_getitem__`). `list[int]` is still rejected because `list`
    // itself is not a recognized type annotation in pycc (only
    // int/float/bool/str and user-defined class names are), not because
    // subscript syntax is universally rejected.
    assert_capability_error_message(
        "x: list[int] = []\n",
        "type annotation `list` is not supported yet",
    );
}

/// PEP 560 (#611): a class body defining `__class_getitem__` with the
/// given decorator, plus an `__init__` so the class is instantiable.
fn class_with_hook(decorator: &str) -> String {
    // A `@classmethod` hook declares `cls` explicitly, exactly as
    // `pycc_types`' own value-position tests spell it; the
    // `@staticmethod` form does not.
    let cls_param = if decorator == "@classmethod" {
        "cls, "
    } else {
        ""
    };
    format!(
        "class C:\n    {decorator}\n    def __class_getitem__({cls_param}key: int) -> int:\n        return key\n\n    def __init__(self) -> None:\n        self.x = 1\n"
    )
}

fn assert_type_error_message(source: &str, expected_message: &str) {
    let module = pycc_parser_test_helper::parse(source);
    let diagnostic = lower_checked(&module).unwrap_err();

    assert_eq!(diagnostic.code, "T0044");
    assert!(diagnostic.message.contains(expected_message));
    assert!(diagnostic.span.is_some());
}

#[test]
fn a_subscripted_annotation_on_a_class_defining_the_hook_is_accepted() {
    // #611: `C[int]` in annotation position is legal exactly when `C` is
    // subscriptable. Both spellings CPython accepts for the hook -- the
    // explicit `@staticmethod` and the `@classmethod` one -- are checked,
    // mirroring `pycc_types`' own value-position dispatch (#610).
    for decorator in ["@staticmethod", "@classmethod"] {
        let src = format!("{}\nv: C[int] = C()\n", class_with_hook(decorator));
        let module = pycc_parser_test_helper::parse(&src);
        assert!(
            lower_checked(&module).is_ok(),
            "`C[int]` must be accepted for a class whose hook is spelled `{decorator}`"
        );
    }
}

#[test]
fn a_subscripted_annotation_on_a_class_inheriting_the_hook_is_accepted() {
    // #611: the gate walks the MRO, so a hook declared on a base class
    // makes the derived class subscriptable too.
    let src = format!(
        "{}\nclass D(C):\n    def value(self) -> int:\n        return self.x\n\nv: D[int] = D()\n",
        class_with_hook("@staticmethod")
    );
    let module = pycc_parser_test_helper::parse(&src);
    assert!(lower_checked(&module).is_ok());
}

#[test]
fn a_subscripted_annotation_on_a_generic_class_is_accepted() {
    // #611: a PEP 695 generic class (`class G[T]:`) declares no
    // `__class_getitem__` of its own -- CPython gives it one implicitly
    // through `Generic`. `G[int]` in an annotation lowers successfully
    // today, and the gate must not regress that.
    let module = pycc_parser_test_helper::parse(
        "class G[T]:\n    def __init__(self, v: T) -> None:\n        self.v = v\n\nv: G[int] = G[int](1)\n",
    );
    assert!(lower_checked(&module).is_ok());
}

#[test]
fn a_subscripted_annotation_on_a_class_without_the_hook_is_rejected() {
    // #611: this is the over-acceptance the issue exists to close --
    // `D[int]` was accepted for any known class name. CPython raises
    // `TypeError: type 'D' is not subscriptable`, so this reuses the
    // `T0044` the value-position path (#610) already reports.
    assert_type_error_message(
        "class D:\n    def __init__(self) -> None:\n        self.x = 1\n\nv: D[int] = D()\n",
        "class `D` does not define `__class_getitem__`",
    );
}

#[test]
fn a_subscripted_annotation_inside_the_class_s_own_body_is_gated_too() {
    // #611: the class being lowered is not yet in the already-defined
    // class table, so `lower_class` adds an entry for it explicitly.
    // Without that, a self-referential `D[int]` would slip past the gate
    // that every other class name goes through.
    assert_type_error_message(
        "class D:\n    def __init__(self) -> None:\n        self.x = 1\n\n    def me(self) -> D[int]:\n        return self\n",
        "class `D` does not define `__class_getitem__`",
    );
}

#[test]
fn a_subscripted_annotation_inside_a_hooked_class_s_own_body_is_accepted() {
    // The accepting half of the self-reference gate above: the class's
    // own `static_methods` table is still empty while its body is being
    // lowered, so the hook is found by the class-body pre-scan.
    let module = pycc_parser_test_helper::parse(
        "class C:\n    @staticmethod\n    def __class_getitem__(key: int) -> int:\n        return key\n\n    def __init__(self) -> None:\n        self.x = 1\n\n    def me(self) -> C[int]:\n        return self\n",
    );
    assert!(lower_checked(&module).is_ok());
}

#[test]
fn an_undecorated_class_getitem_does_not_make_a_class_subscriptable() {
    // pycc's value-position dispatch resolves `__class_getitem__` only
    // through the static-method and class-method tables (#610), so a
    // plain `def __class_getitem__(self)` is an ordinary method and does
    // not make the class subscriptable. The annotation gate agrees,
    // which is what keeps the two positions from disagreeing.
    assert_type_error_message(
        "class D:\n    def __init__(self) -> None:\n        self.x = 1\n\n    def __class_getitem__(self) -> int:\n        return 1\n\nv: D[int] = D()\n",
        "class `D` does not define `__class_getitem__`",
    );
}

/// Issue #693: lowers `src` (expected to be exactly one top-level
/// `AnnAssign`) and returns the `Ty` it resolved the annotation to, panicking
/// with the full module on any other shape -- mirroring this file's other
/// single-purpose lowering-result extractors.
fn annassign_ty(src: &str) -> Ty {
    let module = pycc_parser_test_helper::parse(src);
    let hir = lower_checked(&module).unwrap_or_else(|e| {
        panic!("expected `{src}` to lower successfully, got {e:?}");
    });
    hir.items
        .iter()
        .find_map(|item| match item {
            HirItem::TopLevelStmt(HirStmt::AnnAssign { annotation, .. }) => {
                Some(annotation.clone())
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("expected exactly one top-level `AnnAssign` in `{src}`"))
}

#[test]
fn an_annotation_subscript_on_a_class_defining_the_hook_resolves_to_the_hook_s_return_type() {
    // Issue #693 (PEP 560): `ClassName[type_arg]` in annotation position
    // used to resolve to `Ty::Instance(ClassName)` unconditionally,
    // discarding the type argument. It must instead route through
    // `__class_getitem__`'s declared return type -- `-> int` here -- exactly
    // as `pycc_types::resolve_static_or_class_method_call` already does for
    // the value-position spelling `C[3]` (#610).
    let src = format!("{}\nv: C[3] = 1\n", class_with_hook("@staticmethod"));
    assert_eq!(annassign_ty(&src), Ty::Int);
}

#[test]
fn an_annotation_subscript_on_a_classmethod_hook_also_resolves_to_the_return_type() {
    // Issue #693: the `@classmethod` spelling of the hook (`cls` as the
    // first parameter) must resolve identically to the `@staticmethod`
    // spelling above -- the two decorator forms are equally valid PEP 560
    // hooks and must not diverge in annotation position.
    let src = format!("{}\nv: C[3] = 1\n", class_with_hook("@classmethod"));
    assert_eq!(annassign_ty(&src), Ty::Int);
}

#[test]
fn an_annotation_subscript_on_an_inherited_hook_resolves_through_the_mro() {
    // Issue #693: `D` defines no `__class_getitem__` of its own but
    // inherits `C`'s through the MRO -- the same inheritance
    // `a_subscripted_annotation_on_a_class_inheriting_the_hook_is_accepted`
    // already proves is *subscriptable*; this proves the *resolved type*
    // also correctly follows the MRO to `C`'s hook, not just the
    // subscriptability bit.
    let src = format!(
        "{}\nclass D(C):\n    def value(self) -> int:\n        return self.x\n\nv: D[3] = 1\n",
        class_with_hook("@staticmethod")
    );
    assert_eq!(annassign_ty(&src), Ty::Int);
}

#[test]
fn an_annotation_subscript_prefers_a_base_s_staticmethod_hook_over_a_derived_classmethod_override()
{
    // Issue #693 deep-review, Finding 1: `class_getitem_return_ty` must walk
    // the MRO in the exact same two-pass order as `pycc_types`'
    // `resolve_static_or_class_method_call` (every MRO entry's
    // `static_methods` first, then, only if none declare the hook, every
    // MRO entry's `class_methods`) -- not a single combined pass that lets
    // whichever MRO entry comes first win regardless of decorator kind.
    //
    // `C` declares `__class_getitem__` as a `@staticmethod` returning `int`;
    // `D(C)` overrides it as a `@classmethod` returning `str`. A single
    // combined pass over the MRO (most-derived first: `D`, then `C`) would
    // find `D`'s classmethod entry first and resolve `D[3]` to `str`. The
    // correct two-pass order finds no `static_methods` entry on `D`, then
    // finds `C`'s on the *second* pass over the full MRO -- resolving to
    // `int`, exactly as `pycc_types::resolve_static_or_class_method_call`
    // resolves the value-position `D[3]` for the same hierarchy (see
    // `pycc_types::tests::class_getitem_value_position_prefers_a_base_s_staticmethod_hook_over_a_derived_classmethod_override`).
    let src = "\
class C:
    @staticmethod
    def __class_getitem__(key: int) -> int:
        return key

    def __init__(self) -> None:
        self.x = 1

class D(C):
    @classmethod
    def __class_getitem__(cls, key: int) -> str:
        return \"overridden\"

v: D[3] = 1
";
    assert_eq!(annassign_ty(src), Ty::Int);
}

#[test]
fn a_generic_class_s_annotation_subscript_is_unaffected_by_the_hook_return_type_field() {
    // Issue #693: a PEP 695 generic class (`class G[T]:`) is subscriptable
    // through `Generic`, not through an explicit `__class_getitem__` hook,
    // so `class_getitem_return` must stay `None` for it and `G[int]` must
    // keep resolving to `Ty::Instance(G)` -- the `GenericClassInstantiate`
    // mechanism, not this issue's field, owns actual generic instantiation.
    // Guards against a regression where `type_param.is_some()` alone would
    // be mistaken for "has a resolvable hook return type".
    let src = "class G[T]:\n    def __init__(self, v: T) -> None:\n        self.v = v\n\nv: G[int] = G[int](1)\n";
    assert_eq!(annassign_ty(src), Ty::Instance(Box::new("G".to_string())));
}

#[test]
fn a_self_referential_annotation_inside_the_hook_s_own_class_body_still_falls_back_to_instance() {
    // Issue #693: `lower_class`'s self-referential `ClassAnnotationInfo`
    // entry (pushed before the class's own methods are lowered) cannot yet
    // know `__class_getitem__`'s return type, so a `C[int]` annotation used
    // *inside* `C`'s own body -- the same shape
    // `a_subscripted_annotation_inside_a_hooked_class_s_own_body_is_accepted`
    // already proves is accepted -- keeps resolving to `Ty::Instance(C)`,
    // exactly as it did before this issue. This documents the accepted
    // narrow limitation rather than silently losing coverage of it.
    let module = pycc_parser_test_helper::parse(
        "class C:\n    @staticmethod\n    def __class_getitem__(key: int) -> int:\n        return key\n\n    def __init__(self) -> None:\n        self.x = 1\n\n    def me(self) -> C[int]:\n        return self\n",
    );
    let hir = lower_checked(&module).expect("self-referential annotation must still lower");
    let return_ty = hir
        .items
        .iter()
        .find_map(|item| match item {
            HirItem::Function {
                name, return_ty, ..
            } if name == "C.me" => Some(return_ty.clone()),
            _ => None,
        })
        .expect("expected `C.me` to lower to an `HirItem::Function`");
    assert_eq!(return_ty, Ty::Instance(Box::new("C".to_string())));
}

#[test]
fn an_annotation_subscript_on_a_class_with_an_unannotated_hook_falls_back_to_instance() {
    // Issue #693 review (codex finding): `__class_getitem__` with no
    // explicit return annotation lowers, at this crate's own HIR-lowering
    // time, to a raw `Ty::Infer` placeholder -- this crate never runs its
    // own type-inference pass (see `lower_method`'s doc comment), so the
    // hook's return type is only resolved later, by
    // `pycc_types::check_and_resolve`. `class_getitem_return_ty` must treat
    // that `Ty::Infer` as unresolved rather than propagating the internal
    // placeholder into a resolved annotation type (which previously caused
    // a spurious `T0025` on `x: C[3]`), falling back to
    // `Ty::Instance(ClassName)` exactly as the self-referential and
    // PEP-695-generic cases above already do.
    let src = "class C:\n    @staticmethod\n    def __class_getitem__(key: int):\n        return key\n\n    def __init__(self) -> None:\n        self.x = 1\n\nv: C[3] = C()\n";
    assert_eq!(annassign_ty(src), Ty::Instance(Box::new("C".to_string())));
}

#[test]
fn subscripted_type_annotation_with_non_name_base_is_rejected() {
    // #435 (Part D): a subscripted annotation whose base is not a bare
    // name (e.g. `a.b[int]`) is rejected — only a bare class name is
    // supported as the base of a subscripted type annotation.
    assert_capability_error_message(
        "x: a.b[int] = 1\n",
        "a subscripted type annotation's base must be a bare class name",
    );
}

#[test]
fn annotated_unwraps_to_the_first_type_argument() {
    // PEP 593 (#383): `Annotated[X, "meta"]` unwraps to `X`, discarding
    // metadata. `Annotated` is recognized as a bare name without
    // requiring `from typing import Annotated`, matching the existing
    // `TypeAlias`/`Any` precedent.
    let module = pycc_parser_test_helper::parse(
        "def f(x: Annotated[int, \"meta\"]) -> int:\n    return x\n",
    );
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::Function {
            name: "f".to_string(),
            params: vec![("x".to_string(), Ty::Int)],
            return_ty: Ty::Int,
            body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
        }]
    );
}

#[test]
fn annotated_with_a_single_argument_is_rejected() {
    // PEP 593 (#383): `Annotated[X]` without metadata is rejected — PEP 593
    // requires at least two arguments (the type and at least one metadata
    // element), matching CPython's own `TypeError`.
    assert_capability_error_message(
        "x: Annotated[str] = \"hello\"\n",
        "Annotated requires at least two arguments: the type and at least one metadata element",
    );
}

#[test]
fn annotated_with_multiple_metadata_args_discards_all_metadata() {
    // PEP 593 (#383): `Annotated[X, 1, "x", 2]` unwraps to `X`, discarding
    // all metadata arguments.
    let module = pycc_parser_test_helper::parse(
        "def f(x: Annotated[int, 1, \"x\", 2]) -> int:\n    return x\n",
    );
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::Function {
            name: "f".to_string(),
            params: vec![("x".to_string(), Ty::Int)],
            return_ty: Ty::Int,
            body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
        }]
    );
}

#[test]
fn annotated_with_an_unsupported_inner_type_still_rejects() {
    // PEP 593 (#383): `Annotated[NonExistent, "meta"]` still rejects via
    // the recursive `annotation_to_ty` call — the unwrap does not bypass
    // type resolution.
    assert_capability_error_message(
        "x: Annotated[NonExistent, \"meta\"] = 1\n",
        "type annotation `NonExistent` is not supported yet",
    );
}

#[test]
fn annotated_with_an_empty_tuple_is_rejected() {
    // PEP 593 (#383): `Annotated[()]` (empty tuple) is rejected —
    // `Annotated` requires at least two arguments (the type and at
    // least one metadata element).
    assert_capability_error_message(
        "x: Annotated[()] = 1\n",
        "Annotated requires at least two arguments: the type and at least one metadata element",
    );
}

#[test]
fn final_unwraps_to_the_inner_type() {
    // PEP 591 (#383): `Final[X]` unwraps to `X`. `Final` is recognized
    // as a bare name without requiring `from typing import Final`,
    // matching the existing `TypeAlias`/`Any` precedent.
    let module = pycc_parser_test_helper::parse("x: Final[int] = 1\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::AnnAssign {
            target: "x".to_string(),
            annotation: Ty::Int,
            value: Some(HirExpr::IntLiteral(1)),
            is_final: true,
        })]
    );
}

#[test]
fn final_with_str_unwraps_to_str() {
    let module = pycc_parser_test_helper::parse("x: Final[str] = \"hello\"\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::AnnAssign {
            target: "x".to_string(),
            annotation: Ty::Str,
            value: Some(HirExpr::StringLiteral("hello".to_string())),
            is_final: true,
        })]
    );
}

#[test]
fn final_with_two_type_arguments_is_rejected() {
    // PEP 591 (#383): `Final[X, Y]` is rejected — `Final` takes exactly
    // one type argument.
    assert_capability_error_message(
        "x: Final[int, str] = 1\n",
        "Final takes exactly one type argument",
    );
}

#[test]
fn final_with_a_single_element_tuple_unwraps_to_the_inner_type() {
    // PEP 591 (#383): `Final[(int,)]` (a single-element tuple) unwraps
    // to `int` — the tuple shape with exactly one element is accepted.
    let module = pycc_parser_test_helper::parse("x: Final[(int,)] = 1\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::AnnAssign {
            target: "x".to_string(),
            annotation: Ty::Int,
            value: Some(HirExpr::IntLiteral(1)),
            is_final: true,
        })]
    );
}

#[test]
fn final_value_less_declaration_sets_is_final() {
    // PEP 591 (#383): a value-less `Final` declaration (`x: Final[int]`
    // with no `= ...`) still sets `is_final` — the name is tracked as
    // non-reassignable even before its initial assignment.
    let module = pycc_parser_test_helper::parse("x: Final[int]\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::AnnAssign {
            target: "x".to_string(),
            annotation: Ty::Int,
            value: None,
            is_final: true,
        })]
    );
}

#[test]
fn an_empty_list_literal_lowers_to_an_empty_vec() {
    let module = pycc_parser_test_helper::parse("x = []\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::Assign {
            target: "x".to_string(),
            value: HirExpr::ListLiteral(vec![]),
        })]
    );
}

#[test]
fn appending_to_a_non_bare_name_base_is_unsupported() {
    // `.append()` recognition is restricted to a bare-name list (D-105);
    // `a.b.append(1)` has a non-name `attr.value` (itself an attribute
    // access), so it must be rejected rather than silently accepted.
    assert_capability_error_message(
        "a.b.append(1)\n",
        "`.append()` is only supported on a bare-name list so far",
    );
}

#[test]
fn append_with_zero_arguments_is_unsupported() {
    assert_capability_error_message(
        "x.append()\n",
        "list.append() takes exactly one argument, got 0",
    );
}

#[test]
fn append_with_two_arguments_is_unsupported() {
    assert_capability_error_message(
        "x.append(1, 2)\n",
        "list.append() takes exactly one argument, got 2",
    );
}

#[test]
fn calling_an_unrecognized_method_lowers_to_a_generic_method_call() {
    // Before D-154 (Part 1 of #375), any `.method()` call other than
    // `.append()`/`.pop()`/`.get()`/`.add()` was rejected right here at
    // HIR-lowering time (D-105, widened by D-119) -- this project had no
    // general method-dispatch shape at all yet. D-154 adds one
    // (`HirExpr::MethodCall`, for instance methods), and -- since this
    // lowering step has no type information to distinguish "receiver is
    // a class instance" from "receiver is anything else" -- every
    // `.method()` call not claimed by a hand-recognized container
    // method or a resolved stdlib symbol call now lowers successfully
    // into that generic shape. `foo`/`x` are never assigned in either
    // snippet below, so real rejection now happens downstream, at
    // `pycc_types` (an unbound-name or non-instance-receiver
    // diagnostic), not here. `.remove()` is kept as the same
    // deliberately-chosen example as before (a real `list` method this
    // compiler doesn't special-case, D-119) to show it now takes the
    // identical generic path as an arbitrary, entirely unrecognized
    // name like `.bar()`.
    let module = pycc_parser_test_helper::parse("foo.bar()\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(
            HirExpr::MethodCall {
                base: Box::new(HirExpr::Name("foo".to_string())),
                method: "bar".to_string(),
                args: vec![],
            }
        ))]
    );

    let module = pycc_parser_test_helper::parse("x.remove(1)\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(
            HirExpr::MethodCall {
                base: Box::new(HirExpr::Name("x".to_string())),
                method: "remove".to_string(),
                args: vec![HirExpr::IntLiteral(1)],
            }
        ))]
    );
}

// The five tests below exercise each new arm's own `?`-propagation path
// specifically (an inner element/base/index/argument/body expression
// that itself fails to lower), as opposed to every test above, which
// only ever supplies inner expressions that lower successfully.

#[test]
fn a_list_literal_with_an_unsupported_element_propagates_the_element_error() {
    // (1, 2) no longer fails to lower (this task) -- lambda is still
    // unsupported and exercises the identical propagation path.
    assert_capability_error_message("x = [(lambda: 1)]\n", "expression kind not supported yet");
}

#[test]
fn a_subscript_with_an_unsupported_base_propagates_the_base_error() {
    // (1, 2) no longer fails to lower (this task) -- lambda is still
    // unsupported and exercises the identical propagation path.
    assert_capability_error_message("y = (lambda: 1)[0]\n", "expression kind not supported yet");
}

#[test]
fn a_subscript_with_an_unsupported_index_propagates_the_index_error() {
    // (1, 2) no longer fails to lower (this task) -- lambda is still
    // unsupported and exercises the identical propagation path.
    assert_capability_error_message("y = x[lambda: 1]\n", "expression kind not supported yet");
}

#[test]
fn an_append_with_an_unsupported_argument_propagates_the_argument_error() {
    // (1, 2) no longer fails to lower (this task) -- lambda is still
    // unsupported and exercises the identical propagation path.
    assert_capability_error_message("x.append(lambda: 1)\n", "expression kind not supported yet");
}

// -- PR-12 Task 10 (D-119): remaining container methods depth --------
// `list.pop()`, `dict.get(key, default)`, `set.add(value)` -- each
// mirrors `.append()`'s own hand-recognized-special-form shape and test
// coverage exactly (bare-name-base gate, arity gate, value-position
// lowering, argument-propagation).

#[test]
fn lowers_pop_as_a_dedicated_hir_node_not_a_generic_call() {
    let module = pycc_parser_test_helper::parse("x = [1]\nx.pop()\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items[1],
        HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::ListPop {
            list: "x".to_string(),
        }))
    );
}

#[test]
fn list_pop_used_as_a_value_lowers_successfully() {
    // Unlike `ListAppend`, `.pop()`'s value is the list's element type,
    // not `None` -- `y = x.pop()` is the primary intended use, not a
    // curiosity being merely tolerated.
    let module = pycc_parser_test_helper::parse("x = [1]\ny = x.pop()\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items[1],
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: "y".to_string(),
            value: HirExpr::ListPop {
                list: "x".to_string(),
            },
        })
    );
}

#[test]
fn popping_from_a_non_bare_name_base_is_unsupported() {
    assert_capability_error_message(
        "a.b.pop()\n",
        "`.pop()` is only supported on a bare-name list so far",
    );
}

#[test]
fn pop_with_one_argument_is_unsupported() {
    assert_capability_error_message("x.pop(0)\n", "list.pop() takes no arguments, got 1");
}

#[test]
fn lowers_get_as_a_dedicated_hir_node_not_a_generic_call() {
    let module = pycc_parser_test_helper::parse("d = {\"a\": 1}\nd.get(\"a\", 0)\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items[1],
        HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::DictGetOrDefault {
            dict: "d".to_string(),
            key: Box::new(HirExpr::StringLiteral("a".to_string())),
            default: Box::new(HirExpr::IntLiteral(0)),
        }))
    );
}

#[test]
fn dict_get_used_as_a_value_lowers_successfully() {
    let module = pycc_parser_test_helper::parse("d = {\"a\": 1}\ny = d.get(\"a\", 0)\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items[1],
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: "y".to_string(),
            value: HirExpr::DictGetOrDefault {
                dict: "d".to_string(),
                key: Box::new(HirExpr::StringLiteral("a".to_string())),
                default: Box::new(HirExpr::IntLiteral(0)),
            },
        })
    );
}

#[test]
fn getting_from_a_non_bare_name_base_is_unsupported() {
    assert_capability_error_message(
        "a.b.get(\"a\", 0)\n",
        "`.get()` is only supported on a bare-name dict so far",
    );
}

#[test]
fn get_with_zero_arguments_is_unsupported() {
    assert_capability_error_message(
        "d.get()\n",
        "`.get()` is only supported as `dict.get(key, default)` with exactly two arguments so far, got 0",
    );
}

#[test]
fn get_with_one_argument_is_unsupported() {
    assert_capability_error_message(
        "d.get(\"a\")\n",
        "`.get()` is only supported as `dict.get(key, default)` with exactly two arguments so far, got 1",
    );
}

#[test]
fn get_with_three_arguments_is_unsupported() {
    assert_capability_error_message(
        "d.get(\"a\", 0, 1)\n",
        "`.get()` is only supported as `dict.get(key, default)` with exactly two arguments so far, got 3",
    );
}

#[test]
fn a_get_call_with_an_unsupported_key_propagates_the_key_error() {
    assert_capability_error_message("d.get(lambda: 1, 0)\n", "expression kind not supported yet");
}

#[test]
fn a_get_call_with_an_unsupported_default_propagates_the_default_error() {
    assert_capability_error_message(
        "d.get(\"a\", lambda: 1)\n",
        "expression kind not supported yet",
    );
}

#[test]
fn lowers_add_as_a_dedicated_hir_node_not_a_generic_call() {
    let module = pycc_parser_test_helper::parse("s = {1}\ns.add(2)\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items[1],
        HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::SetAdd {
            set: "s".to_string(),
            value: Box::new(HirExpr::IntLiteral(2)),
        }))
    );
}

#[test]
fn set_add_used_as_a_value_lowers_successfully_today() {
    // Mirrors `ListAppend`'s own "today's actual behavior" test exactly
    // -- `.add()`'s value is always `None`, and D-131 lets an assignment
    // preserve that materialized unit value in ordinary storage.
    let module = pycc_parser_test_helper::parse("s = {1}\ny = s.add(2)\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items[1],
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: "y".to_string(),
            value: HirExpr::SetAdd {
                set: "s".to_string(),
                value: Box::new(HirExpr::IntLiteral(2)),
            },
        })
    );
}

#[test]
fn adding_to_a_non_bare_name_base_is_unsupported() {
    assert_capability_error_message(
        "a.b.add(1)\n",
        "`.add()` is only supported on a bare-name set so far",
    );
}

#[test]
fn add_with_zero_arguments_is_unsupported() {
    assert_capability_error_message("s.add()\n", "set.add() takes exactly one argument, got 0");
}

#[test]
fn add_with_two_arguments_is_unsupported() {
    assert_capability_error_message(
        "s.add(1, 2)\n",
        "set.add() takes exactly one argument, got 2",
    );
}

#[test]
fn an_add_with_an_unsupported_argument_propagates_the_argument_error() {
    assert_capability_error_message("s.add(lambda: 1)\n", "expression kind not supported yet");
}

#[test]
fn a_for_list_body_with_an_unsupported_statement_propagates_the_body_error() {
    // (1, 2) no longer fails to lower (this task) -- lambda is still
    // unsupported and exercises the identical propagation path.
    assert_capability_error_message(
        "x = [1, 2, 3]\nfor v in x:\n    lambda: 1\n",
        "expression kind not supported yet",
    );
}

// -- PR-11b Task 2 (D-116): tuple[...] frontend HIR forms -------------

#[test]
fn a_bare_unparenthesized_tuple_expression_lowers_the_same_as_parenthesized() {
    // Python's tuple literal syntax does not require parentheses
    // (`1, 2` and `(1, 2)` parse to the same `Expr::Tuple` node); this
    // locks in that this crate's own lowering treats both identically,
    // since `lower_expr` never inspects `ExprTuple::parenthesized`.
    let module = pycc_parser_test_helper::parse("x = 1, 2\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items[0],
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: "x".to_string(),
            value: HirExpr::TupleLiteral(vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)]),
        })
    );
}

#[test]
fn a_heterogeneous_tuple_literal_lowers_with_mixed_element_kinds() {
    // Unlike `ListLiteral`/`SetLiteral`, this crate does not reject
    // mixed element kinds at the HIR layer -- D-116 makes heterogeneity
    // tuple's own defining feature, judged (for element *type*, not
    // syntactic kind) entirely by `pycc_types`, not here.
    let module = pycc_parser_test_helper::parse("x = (1, True, 2.5)\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items[0],
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: "x".to_string(),
            value: HirExpr::TupleLiteral(vec![
                HirExpr::IntLiteral(1),
                HirExpr::BoolLiteral(true),
                HirExpr::FloatLiteral(2.5),
            ]),
        })
    );
}

#[test]
fn a_tuple_element_that_fails_to_lower_propagates_its_own_error() {
    assert_capability_error_message("x = (1, lambda: 1)\n", "expression kind not supported yet");
}

// -- PR-12 Task 2 (D-117): comprehension frontend HIR forms ----------

#[test]
fn lowers_a_list_comprehension_over_range_to_list_comp_assign() {
    let module = pycc_parser_test_helper::parse("y = [i for i in range(3)]\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::ListCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: None,
            elt: Box::new(HirExpr::Name("0comp_11_i".to_string())),
        })]
    );
}

#[test]
fn lowers_a_list_comprehension_with_an_if_filter() {
    let module = pycc_parser_test_helper::parse("y = [i for i in range(5) if i]\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::ListCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(5),
                step: HirExpr::IntLiteral(1),
            },
            cond: Some(Box::new(HirExpr::Name("0comp_11_i".to_string()))),
            elt: Box::new(HirExpr::Name("0comp_11_i".to_string())),
        })]
    );
}

#[test]
fn lowers_a_dict_comprehension_with_an_f_string_key_renaming_the_interpolation() {
    // Confirms `FString`'s own `rename_name_in_expr` arm actually
    // rewrites an interpolated loop-variable reference, not just a bare
    // `Name` expression.
    let module = pycc_parser_test_helper::parse("y = {f\"n{i}\": i for i in range(3)}\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::DictCompAssign {
            target: "y".to_string(),
            var: "0comp_20_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: None,
            key: Box::new(HirExpr::FString(vec![
                FStringPart::Literal("n".to_string()),
                FStringPart::Interpolation(Box::new(HirExpr::Name("0comp_20_i".to_string()))),
            ])),
            value: Box::new(HirExpr::Name("0comp_20_i".to_string())),
        })]
    );
}

#[test]
fn lowers_a_set_comprehension_to_set_comp_assign() {
    let module = pycc_parser_test_helper::parse("y = {i for i in range(3)}\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::SetCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: None,
            elt: Box::new(HirExpr::Name("0comp_11_i".to_string())),
        })]
    );
}

#[test]
fn lowers_a_set_comprehension_with_an_if_filter() {
    // `SetCompAssign`'s own `cond` field needs a dedicated if-filter
    // test distinct from the plain set-comprehension test above: the
    // `cond.map(|c| rename_name_in_expr(...))` closure inside
    // `lower_set_comp_assign` is only reached when `cond.is_some()`.
    let module = pycc_parser_test_helper::parse("y = {i for i in range(5) if i}\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::SetCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(5),
                step: HirExpr::IntLiteral(1),
            },
            cond: Some(Box::new(HirExpr::Name("0comp_11_i".to_string()))),
            elt: Box::new(HirExpr::Name("0comp_11_i".to_string())),
        })]
    );
}

#[test]
fn lowers_a_dict_comprehension_with_an_if_filter() {
    // Same reasoning as `lowers_a_set_comprehension_with_an_if_filter`
    // above, for `lower_dict_comp_assign`'s own `cond.map(...)` closure.
    let module = pycc_parser_test_helper::parse("y = {i: i for i in range(5) if i}\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::DictCompAssign {
            target: "y".to_string(),
            var: "0comp_14_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(5),
                step: HirExpr::IntLiteral(1),
            },
            cond: Some(Box::new(HirExpr::Name("0comp_14_i".to_string()))),
            key: Box::new(HirExpr::Name("0comp_14_i".to_string())),
            value: Box::new(HirExpr::Name("0comp_14_i".to_string())),
        })]
    );
}

#[test]
fn a_comprehension_with_two_for_clauses_is_unsupported() {
    assert_capability_error_message(
        "y = [i for i in range(3) for j in range(3)]\n",
        "a comprehension with more than one `for` clause is not supported yet",
    );
}

#[test]
fn a_comprehension_with_two_if_filters_is_unsupported() {
    assert_capability_error_message(
        "y = [i for i in range(5) if i if i]\n",
        "a comprehension with more than one `if` filter is not supported yet",
    );
}

#[test]
fn a_comprehension_with_a_non_bare_name_target_is_unsupported() {
    assert_capability_error_message(
        "y = [a for (a, b) in xs]\n",
        "only a bare name comprehension target is supported so far",
    );
}

#[test]
fn an_async_for_comprehension_is_unsupported() {
    assert_capability_error_message(
        "y = [i async for i in xs]\n",
        "async comprehensions are not supported yet",
    );
}

#[test]
fn a_comprehension_used_as_a_call_argument_is_not_specially_recognized() {
    // Pins the "only `Stmt::Assign`-RHS position" restriction (D-117): a
    // comprehension anywhere else still falls through to `lower_expr`'s
    // existing generic catch-all, not a new comprehension-specific
    // error path.
    assert_capability_error_message(
        "print([i for i in range(3)])\n",
        "expression kind not supported yet",
    );
}

#[test]
fn a_comprehension_iterating_a_bare_name_produces_comp_iter_name() {
    let module = pycc_parser_test_helper::parse("xs = [1, 2, 3]\ny = [i for i in xs]\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items[1],
        HirItem::TopLevelStmt(HirStmt::ListCompAssign {
            target: "y".to_string(),
            var: "0comp_26_i".to_string(),
            iter: CompIter::Name("xs".to_string()),
            cond: None,
            elt: Box::new(HirExpr::Name("0comp_26_i".to_string())),
        })
    );
}

#[test]
fn a_comprehension_range_iterable_referencing_the_loop_variables_own_source_name_is_not_renamed() {
    // Pins `lower_comprehension_header`'s own documented asymmetry
    // (D-117): `iter` is deliberately never passed through
    // `rename_name_in_expr`, unlike `elt`/`cond`/`key`/`value` -- this
    // is correct CPython scoping, since a comprehension's iterable
    // expression evaluates in the *enclosing* scope, before the
    // comprehension's own loop variable is ever bound.
    // `[i for i in range(i)]`'s `range(i)` must read the *outer* `i`
    // (here, the module-level `i = 5`), not the comprehension's own
    // synthesized loop variable. Without this test, a future change
    // that "fixed" this asymmetry by renaming `iter` too would silently
    // break correct scoping with every other existing test (and 100%
    // coverage) still green, since no other test uses an iterable
    // expression that shares the loop variable's own source name.
    let module = pycc_parser_test_helper::parse("i = 5\ny = [i for i in range(i)]\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items[1],
        HirItem::TopLevelStmt(HirStmt::ListCompAssign {
            target: "y".to_string(),
            var: "0comp_17_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                // The un-renamed enclosing-scope `i`, not
                // `HirExpr::Name("0comp_17_i")`.
                stop: HirExpr::Name("i".to_string()),
                step: HirExpr::IntLiteral(1),
            },
            cond: None,
            elt: Box::new(HirExpr::Name("0comp_17_i".to_string())),
        })
    );
}

#[test]
fn a_comprehension_bare_name_iterable_sharing_the_loop_variables_own_source_name_is_not_renamed() {
    // Same reasoning as the `range(i)` test above, for `CompIter::Name`
    // instead of `CompIter::Range`: `[xs for xs in xs]`'s iterable `xs`
    // must resolve to the un-renamed enclosing-scope name, not the
    // comprehension's own synthesized loop variable, even though both
    // are spelled identically in the source.
    let module = pycc_parser_test_helper::parse("xs = [1, 2, 3]\ny = [xs for xs in xs]\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items[1],
        HirItem::TopLevelStmt(HirStmt::ListCompAssign {
            target: "y".to_string(),
            var: "0comp_27_xs".to_string(),
            // The un-renamed enclosing-scope `xs`, not
            // `CompIter::Name("0comp_27_xs".to_string())`.
            iter: CompIter::Name("xs".to_string()),
            cond: None,
            elt: Box::new(HirExpr::Name("0comp_27_xs".to_string())),
        })
    );
}

#[test]
fn a_comprehension_iterating_a_list_literal_is_unsupported() {
    // Neither a bare name nor a call -- exercises
    // `lower_comprehension_iter`'s own "not Name, not Call" branch,
    // which is not shared with `Stmt::For`'s separate (textually
    // similar but distinct) iterable-shape checks.
    assert_capability_error_message(
        "y = [i for i in [1, 2]]\n",
        "only `range(...)` or a bare-name iterable is supported so far in a comprehension",
    );
}

#[test]
fn a_comprehension_iterating_a_non_name_callee_call_is_unsupported() {
    assert_capability_error_message(
        "y = [i for i in f()()]\n",
        "only calling `range(...)` is supported so far in a comprehension",
    );
}

#[test]
fn a_comprehension_iterating_a_non_range_call_is_unsupported() {
    assert_capability_error_message(
        "y = [i for i in foo(3)]\n",
        "only iterating over `range(...)` is supported so far in a comprehension, got `foo`",
    );
}

#[test]
fn a_comprehension_range_call_with_keyword_arguments_is_unsupported() {
    assert_capability_error_message(
        "y = [i for i in range(stop=3)]\n",
        "keyword arguments to range() are not supported yet",
    );
}

// The eight tests below each exercise one `?`-propagation region on its
// own `?` operator's specific call site (mirroring this file's existing
// "the five tests below exercise each new arm's own `?`-propagation
// path specifically" precedent above): `lower_set_comp_assign` and
// `lower_dict_comp_assign` are structurally near-identical to
// `lower_list_comp_assign`, but each function's own `?` is a distinct
// coverage region, so an error test against one function's call site
// does not also cover its sibling's.

#[test]
fn a_set_comprehension_with_an_unsupported_header_propagates_the_header_error() {
    // Exercises both `Stmt::Assign`'s own `Expr::SetComp(comp) =>
    // lower_set_comp_assign(...)?` call site and
    // `lower_set_comp_assign`'s own internal
    // `lower_comprehension_header(&comp.generators)?` call site in one
    // test, since the header error propagates through both in the same
    // nested call.
    assert_capability_error_message(
        "y = {i for i in range(3) for j in range(3)}\n",
        "a comprehension with more than one `for` clause is not supported yet",
    );
}

#[test]
fn a_comprehension_iter_range_call_with_too_many_arguments_is_unsupported() {
    // Exercises `lower_comprehension_iter`'s own
    // `lower_range_call(call)?` call site specifically -- distinct from
    // `Stmt::For`'s own separate call site to the same shared helper
    // (already covered by `range_with_too_many_arguments_is_not_supported`
    // above).
    assert_capability_error_message(
        "y = [i for i in range(1, 2, 3, 4)]\n",
        "range() with 4 arguments is not supported",
    );
}

#[test]
fn a_comprehension_if_filter_with_an_unsupported_expression_propagates_the_filter_error() {
    assert_capability_error_message(
        "y = [i for i in range(3) if (lambda: 1)]\n",
        "expression kind not supported yet",
    );
}

#[test]
fn a_list_comprehension_element_that_fails_to_lower_propagates_the_element_error() {
    assert_capability_error_message(
        "y = [(lambda: 1) for i in range(3)]\n",
        "expression kind not supported yet",
    );
}

#[test]
fn a_set_comprehension_element_that_fails_to_lower_propagates_the_element_error() {
    assert_capability_error_message(
        "y = {(lambda: 1) for i in range(3)}\n",
        "expression kind not supported yet",
    );
}

#[test]
fn a_dict_comprehension_with_an_unsupported_header_propagates_the_header_error() {
    assert_capability_error_message(
        "y = {i: i for i in range(3) for j in range(3)}\n",
        "a comprehension with more than one `for` clause is not supported yet",
    );
}

#[test]
fn a_dict_comprehension_key_that_fails_to_lower_propagates_the_key_error() {
    assert_capability_error_message(
        "y = {(lambda: 1): i for i in range(3)}\n",
        "expression kind not supported yet",
    );
}

#[test]
fn a_dict_comprehension_value_that_fails_to_lower_propagates_the_value_error() {
    assert_capability_error_message(
        "y = {i: (lambda: 1) for i in range(3)}\n",
        "expression kind not supported yet",
    );
}

#[test]
fn dict_comp_key_unpacking_parses_successfully_and_is_rejected_at_lowering() {
    // The task-2 brief this test suite followed assumed
    // `{**x for k in y}`-shaped source could never actually parse (so
    // `comp.key == None` would be unreachable, modeled with an internal
    // panic). Verified false directly against the vendored
    // `ruff_python_parser`: it parses this successfully as
    // `ExprDictComp { key: None, value: Name("x"), .. }`, silently
    // dropping the `**` rather than erroring -- so `pycc_parser::parse`
    // itself succeeds here, and `lower_dict_comp_assign` is the one
    // that must reject it, with an ordinary `C0001` capability
    // diagnostic instead of a panic.
    assert!(pycc_parser::parse("y = {**x for k in z}\n").is_ok());
    assert_capability_error_message(
        "y = {**x for k in z}\n",
        "dict-unpacking (`**expr`) inside a dict comprehension is not supported yet",
    );
}

#[test]
fn lower_comprehension_header_rejects_an_empty_generators_slice() {
    // Real parsed source can never produce a comprehension with zero
    // generators -- this is the only way to reach the `[generator] =
    // generators else { ... }` arm's failure branch and its own
    // `generators.first().map(|g| g.range).unwrap_or_default()`
    // span-fallback expression at all (D-014's region coverage gate
    // would otherwise flag that fallback as an uncoverable dead
    // branch).
    let err = lower_comprehension_header(&[], None).unwrap_err();
    assert_eq!(err.code, "C0001");
    assert!(
        err.message
            .contains("a comprehension with more than one `for` clause is not supported yet")
    );
    assert_eq!(err.span, Some(Span::new(0, 0)));
}

// -- Task 6 (D-118): list[int] slicing frontend HIR forms ------------

#[test]
fn lowers_a_slice_expression_with_both_bounds_present() {
    let module = pycc_parser_test_helper::parse("xs = [1, 2, 3]\ny = xs[1:3]\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items[1],
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: "y".to_string(),
            value: HirExpr::Slice {
                base: Box::new(HirExpr::Name("xs".to_string())),
                start: Some(Box::new(HirExpr::IntLiteral(1))),
                stop: Some(Box::new(HirExpr::IntLiteral(3))),
                step: None,
            },
        })
    );
}

#[test]
fn lowers_a_slice_expression_with_only_the_stop_bound() {
    let module = pycc_parser_test_helper::parse("xs = [1, 2, 3]\ny = xs[:3]\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items[1],
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: "y".to_string(),
            value: HirExpr::Slice {
                base: Box::new(HirExpr::Name("xs".to_string())),
                start: None,
                stop: Some(Box::new(HirExpr::IntLiteral(3))),
                step: None,
            },
        })
    );
}

#[test]
fn lowers_a_slice_expression_with_only_the_start_bound() {
    let module = pycc_parser_test_helper::parse("xs = [1, 2, 3]\ny = xs[2:]\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items[1],
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: "y".to_string(),
            value: HirExpr::Slice {
                base: Box::new(HirExpr::Name("xs".to_string())),
                start: Some(Box::new(HirExpr::IntLiteral(2))),
                stop: None,
                step: None,
            },
        })
    );
}

#[test]
fn lowers_a_slice_expression_with_all_bounds_omitted() {
    let module = pycc_parser_test_helper::parse("xs = [1, 2, 3]\ny = xs[:]\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items[1],
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: "y".to_string(),
            value: HirExpr::Slice {
                base: Box::new(HirExpr::Name("xs".to_string())),
                start: None,
                stop: None,
                step: None,
            },
        })
    );
}

#[test]
fn lowers_a_slice_expression_with_a_step() {
    let module = pycc_parser_test_helper::parse("xs = [1, 2, 3]\ny = xs[::2]\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items[1],
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: "y".to_string(),
            value: HirExpr::Slice {
                base: Box::new(HirExpr::Name("xs".to_string())),
                start: None,
                stop: None,
                step: Some(Box::new(HirExpr::IntLiteral(2))),
            },
        })
    );
}

#[test]
fn an_ordinary_single_expression_subscript_still_lowers_to_subscript_not_slice() {
    // Regression pin for Step 2's new `match sub.slice.as_ref() { ... }`
    // dispatch (D-118): a colon-free subscript must keep taking the `_`
    // arm and producing the pre-existing `HirExpr::Subscript` shape,
    // unaffected by the new `Expr::Slice` arm added alongside it.
    // (Already exercised incidentally by `lowers_a_read_subscript`
    // above; pinned again here, by name, as the dedicated regression
    // test for this specific change.)
    let module = pycc_parser_test_helper::parse("xs = [1, 2, 3]\ny = xs[0]\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items[1],
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: "y".to_string(),
            value: HirExpr::Subscript {
                base: Box::new(HirExpr::Name("xs".to_string())),
                index: Box::new(HirExpr::IntLiteral(0)),
            },
        })
    );
}

#[test]
fn a_slice_expressions_base_and_bounds_are_recursively_lowered() {
    // `f()`/`g()` stand in for "some already-lowerable non-literal
    // shape" -- confirms `base`/`start`/`stop` are each passed through
    // the real `lower_expr` recursively, not merely accepted as raw
    // literals.
    let module = pycc_parser_test_helper::parse("xs = [1, 2, 3]\ny = xs[f():g()]\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items[1],
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: "y".to_string(),
            value: HirExpr::Slice {
                base: Box::new(HirExpr::Name("xs".to_string())),
                start: Some(Box::new(HirExpr::Call {
                    callee: "f".to_string(),
                    args: vec![],
                })),
                stop: Some(Box::new(HirExpr::Call {
                    callee: "g".to_string(),
                    args: vec![],
                })),
                step: None,
            },
        })
    );
}

#[test]
fn a_slice_with_an_unsupported_base_propagates_the_base_error() {
    // Exercises the `Expr::Slice(slice) => ...` arm's own
    // `lower_expr(&sub.value)?` call site specifically -- a distinct
    // region from the `_` arm's identically-worded call site, already
    // covered by `a_subscript_with_an_unsupported_base_propagates_the_base_error`.
    assert_capability_error_message(
        "y = (lambda: 1)[0:1]\n",
        "expression kind not supported yet",
    );
}

#[test]
fn a_slice_with_an_unsupported_start_bound_propagates_the_start_error() {
    assert_capability_error_message(
        "xs = [1, 2, 3]\ny = xs[(lambda: 1):3]\n",
        "expression kind not supported yet",
    );
}

#[test]
fn a_slice_with_an_unsupported_stop_bound_propagates_the_stop_error() {
    assert_capability_error_message(
        "xs = [1, 2, 3]\ny = xs[1:(lambda: 1)]\n",
        "expression kind not supported yet",
    );
}

#[test]
fn a_slice_with_an_unsupported_step_propagates_the_step_error() {
    assert_capability_error_message(
        "xs = [1, 2, 3]\ny = xs[1:3:(lambda: 1)]\n",
        "expression kind not supported yet",
    );
}

#[test]
fn a_slice_assignment_target_is_not_specially_recognized() {
    // Pins `HirExpr::Slice`'s own documented "Load position only"
    // restriction (D-118): unlike a plain-index assignment target
    // (`xs[0] = 1`, which reaches `HirStmt::DictSet` via `Stmt::Assign`'s
    // own `Expr::Subscript` target arm), a slice assignment target
    // (`xs[1:3] = value`) calls `lower_expr` directly on the bare
    // `Expr::Slice` node -- which has no top-level arm -- and falls
    // through to the same generic catch-all as any other unsupported
    // expression kind. This is intentional (slicing ships read-only in
    // this PR), not a regression to fix later without revisiting D-118.
    assert_capability_error_message(
        "xs = [1, 2, 3]\nxs[1:3] = [4, 5]\n",
        "expression kind not supported yet",
    );
}

#[test]
fn rename_name_in_expr_rewrites_every_hir_expr_variant() {
    // Exhaustiveness-pinning test for `rename_name_in_expr`'s own
    // "let the compiler enumerate every site" discipline (D-117,
    // mirroring D-107's `Scalar::List` precedent) -- every arm must be
    // hit by at least one case, and every conditional inside an arm
    // (name matches `from` vs. doesn't) must be hit on both sides.

    // Name: renamed when it matches `from`, left alone otherwise.
    assert_eq!(
        rename_name_in_expr(HirExpr::Name("old".to_string()), "old", "new"),
        HirExpr::Name("new".to_string())
    );
    assert_eq!(
        rename_name_in_expr(HirExpr::Name("other".to_string()), "old", "new"),
        HirExpr::Name("other".to_string())
    );

    // The four grouped literal variants are returned unchanged.
    assert_eq!(
        rename_name_in_expr(HirExpr::IntLiteral(1), "old", "new"),
        HirExpr::IntLiteral(1)
    );
    assert_eq!(
        rename_name_in_expr(HirExpr::FloatLiteral(1.5), "old", "new"),
        HirExpr::FloatLiteral(1.5)
    );
    assert_eq!(
        rename_name_in_expr(HirExpr::BoolLiteral(true), "old", "new"),
        HirExpr::BoolLiteral(true)
    );
    assert_eq!(
        rename_name_in_expr(HirExpr::StringLiteral("s".to_string()), "old", "new"),
        HirExpr::StringLiteral("s".to_string())
    );
    // `NoneLiteral` (D-197, #763, Part 1 of #747) joined this same
    // returned-unchanged group.
    assert_eq!(
        rename_name_in_expr(HirExpr::NoneLiteral, "old", "new"),
        HirExpr::NoneLiteral
    );

    // Call: renames every argument.
    assert_eq!(
        rename_name_in_expr(
            HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::Name("old".to_string())],
            },
            "old",
            "new",
        ),
        HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::Name("new".to_string())],
        }
    );

    // BinOp: renames both sides.
    assert_eq!(
        rename_name_in_expr(
            HirExpr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(HirExpr::Name("old".to_string())),
                right: Box::new(HirExpr::Name("old".to_string())),
            },
            "old",
            "new",
        ),
        HirExpr::BinOp {
            op: BinOpKind::Add,
            left: Box::new(HirExpr::Name("new".to_string())),
            right: Box::new(HirExpr::Name("new".to_string())),
        }
    );

    // Compare: renames both sides.
    assert_eq!(
        rename_name_in_expr(
            HirExpr::Compare {
                op: CmpOpKind::Eq,
                left: Box::new(HirExpr::Name("old".to_string())),
                right: Box::new(HirExpr::Name("old".to_string())),
            },
            "old",
            "new",
        ),
        HirExpr::Compare {
            op: CmpOpKind::Eq,
            left: Box::new(HirExpr::Name("new".to_string())),
            right: Box::new(HirExpr::Name("new".to_string())),
        }
    );

    // FString: covers both a `Literal` part (passed through unchanged)
    // and an `Interpolation` part (recursed into) in the same tree.
    assert_eq!(
        rename_name_in_expr(
            HirExpr::FString(vec![
                FStringPart::Literal("text".to_string()),
                FStringPart::Interpolation(Box::new(HirExpr::Name("old".to_string()))),
            ]),
            "old",
            "new",
        ),
        HirExpr::FString(vec![
            FStringPart::Literal("text".to_string()),
            FStringPart::Interpolation(Box::new(HirExpr::Name("new".to_string()))),
        ])
    );

    // ListLiteral: renames every element.
    assert_eq!(
        rename_name_in_expr(
            HirExpr::ListLiteral(vec![HirExpr::Name("old".to_string())]),
            "old",
            "new",
        ),
        HirExpr::ListLiteral(vec![HirExpr::Name("new".to_string())])
    );

    // Subscript: renames both base and index.
    assert_eq!(
        rename_name_in_expr(
            HirExpr::Subscript {
                base: Box::new(HirExpr::Name("old".to_string())),
                index: Box::new(HirExpr::Name("old".to_string())),
            },
            "old",
            "new",
        ),
        HirExpr::Subscript {
            base: Box::new(HirExpr::Name("new".to_string())),
            index: Box::new(HirExpr::Name("new".to_string())),
        }
    );

    // Slice: renames `base` and every present bound. Only the `Some`
    // side of each `Option::map` is exercised here -- the `None` side
    // has no closure body of its own to cover (it is just the same
    // `start`/`stop`/`step` value flowing through unchanged), and is
    // separately pinned by `lowers_a_slice_expression_with_all_bounds_omitted`
    // above at the `lower_expr` level.
    assert_eq!(
        rename_name_in_expr(
            HirExpr::Slice {
                base: Box::new(HirExpr::Name("old".to_string())),
                start: Some(Box::new(HirExpr::Name("old".to_string()))),
                stop: Some(Box::new(HirExpr::Name("old".to_string()))),
                step: Some(Box::new(HirExpr::Name("old".to_string()))),
            },
            "old",
            "new",
        ),
        HirExpr::Slice {
            base: Box::new(HirExpr::Name("new".to_string())),
            start: Some(Box::new(HirExpr::Name("new".to_string()))),
            stop: Some(Box::new(HirExpr::Name("new".to_string()))),
            step: Some(Box::new(HirExpr::Name("new".to_string()))),
        }
    );

    // ListAppend: covers both the `list` field matching `from` and not
    // matching it, plus renaming `value` in both cases.
    assert_eq!(
        rename_name_in_expr(
            HirExpr::ListAppend {
                list: "old".to_string(),
                value: Box::new(HirExpr::Name("old".to_string())),
            },
            "old",
            "new",
        ),
        HirExpr::ListAppend {
            list: "new".to_string(),
            value: Box::new(HirExpr::Name("new".to_string())),
        }
    );
    assert_eq!(
        rename_name_in_expr(
            HirExpr::ListAppend {
                list: "other".to_string(),
                value: Box::new(HirExpr::Name("old".to_string())),
            },
            "old",
            "new",
        ),
        HirExpr::ListAppend {
            list: "other".to_string(),
            value: Box::new(HirExpr::Name("new".to_string())),
        }
    );

    // DictLiteral: renames both key and value of every pair.
    assert_eq!(
        rename_name_in_expr(
            HirExpr::DictLiteral(vec![(
                HirExpr::Name("old".to_string()),
                HirExpr::Name("old".to_string()),
            )]),
            "old",
            "new",
        ),
        HirExpr::DictLiteral(vec![(
            HirExpr::Name("new".to_string()),
            HirExpr::Name("new".to_string()),
        )])
    );

    // SetLiteral: renames every element.
    assert_eq!(
        rename_name_in_expr(
            HirExpr::SetLiteral(vec![HirExpr::Name("old".to_string())]),
            "old",
            "new",
        ),
        HirExpr::SetLiteral(vec![HirExpr::Name("new".to_string())])
    );

    // TupleLiteral: renames every element.
    assert_eq!(
        rename_name_in_expr(
            HirExpr::TupleLiteral(vec![HirExpr::Name("old".to_string())]),
            "old",
            "new",
        ),
        HirExpr::TupleLiteral(vec![HirExpr::Name("new".to_string())])
    );

    // ListPop: covers both the `list` field matching `from` and not
    // matching it (PR-12 Task 10, D-119; mirrors `ListAppend` above).
    assert_eq!(
        rename_name_in_expr(
            HirExpr::ListPop {
                list: "old".to_string(),
            },
            "old",
            "new",
        ),
        HirExpr::ListPop {
            list: "new".to_string(),
        }
    );
    assert_eq!(
        rename_name_in_expr(
            HirExpr::ListPop {
                list: "other".to_string(),
            },
            "old",
            "new",
        ),
        HirExpr::ListPop {
            list: "other".to_string(),
        }
    );

    // DictGetOrDefault: covers both the `dict` field matching `from` and
    // not matching it, plus renaming `key`/`default` in both cases.
    assert_eq!(
        rename_name_in_expr(
            HirExpr::DictGetOrDefault {
                dict: "old".to_string(),
                key: Box::new(HirExpr::Name("old".to_string())),
                default: Box::new(HirExpr::Name("old".to_string())),
            },
            "old",
            "new",
        ),
        HirExpr::DictGetOrDefault {
            dict: "new".to_string(),
            key: Box::new(HirExpr::Name("new".to_string())),
            default: Box::new(HirExpr::Name("new".to_string())),
        }
    );
    assert_eq!(
        rename_name_in_expr(
            HirExpr::DictGetOrDefault {
                dict: "other".to_string(),
                key: Box::new(HirExpr::Name("old".to_string())),
                default: Box::new(HirExpr::Name("old".to_string())),
            },
            "old",
            "new",
        ),
        HirExpr::DictGetOrDefault {
            dict: "other".to_string(),
            key: Box::new(HirExpr::Name("new".to_string())),
            default: Box::new(HirExpr::Name("new".to_string())),
        }
    );

    // SetAdd: covers both the `set` field matching `from` and not
    // matching it, plus renaming `value` in both cases.
    assert_eq!(
        rename_name_in_expr(
            HirExpr::SetAdd {
                set: "old".to_string(),
                value: Box::new(HirExpr::Name("old".to_string())),
            },
            "old",
            "new",
        ),
        HirExpr::SetAdd {
            set: "new".to_string(),
            value: Box::new(HirExpr::Name("new".to_string())),
        }
    );
    assert_eq!(
        rename_name_in_expr(
            HirExpr::SetAdd {
                set: "other".to_string(),
                value: Box::new(HirExpr::Name("old".to_string())),
            },
            "old",
            "new",
        ),
        HirExpr::SetAdd {
            set: "other".to_string(),
            value: Box::new(HirExpr::Name("new".to_string())),
        }
    );

    // AttrGet (D-154): renames `base`, `attr` is untouched (it names a
    // field, never a local variable this rename could shadow).
    assert_eq!(
        rename_name_in_expr(
            HirExpr::AttrGet {
                base: Box::new(HirExpr::Name("old".to_string())),
                attr: "x".to_string(),
            },
            "old",
            "new",
        ),
        HirExpr::AttrGet {
            base: Box::new(HirExpr::Name("new".to_string())),
            attr: "x".to_string(),
        }
    );

    // MethodCall (D-154): renames `base` and every argument; `method`
    // is untouched for the same reason `attr` is above.
    assert_eq!(
        rename_name_in_expr(
            HirExpr::MethodCall {
                base: Box::new(HirExpr::Name("old".to_string())),
                method: "bump".to_string(),
                args: vec![HirExpr::Name("old".to_string())],
            },
            "old",
            "new",
        ),
        HirExpr::MethodCall {
            base: Box::new(HirExpr::Name("new".to_string())),
            method: "bump".to_string(),
            args: vec![HirExpr::Name("new".to_string())],
        }
    );

    // Super (#433): carries no names to rename — returned unchanged.
    assert_eq!(
        rename_name_in_expr(HirExpr::Super, "old", "new"),
        HirExpr::Super
    );
}

// -- D-135: `type` statement and legacy `TypeAlias` -------------------

#[test]
fn a_type_statement_resolves_the_alias_in_a_later_parameter_annotation() {
    let module = pycc_parser_test_helper::parse(
        "type IntAlias = int\n\
             def f(x: IntAlias) -> int:\n\
             \x20   return x\n",
    );
    let hir = lower_checked(&module).unwrap();

    // Zero HIR footprint: the `type` statement itself contributes no
    // `HirItem` -- the only item present is the function it fed, and its
    // `x` parameter resolved to `Ty::Int` through the alias.
    assert_eq!(
        hir.items,
        vec![HirItem::Function {
            name: "f".to_string(),
            params: vec![("x".to_string(), Ty::Int)],
            return_ty: Ty::Int,
            body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
        }]
    );
    assert_eq!(hir.type_aliases, vec![("IntAlias".to_string(), Ty::Int)]);
}

#[test]
fn a_legacy_type_alias_annotated_assignment_resolves_the_alias_the_same_way() {
    let module = pycc_parser_test_helper::parse(
        "IntAlias: TypeAlias = int\n\
             def f(x: IntAlias) -> int:\n\
             \x20   return x\n",
    );
    let hir = lower_checked(&module).unwrap();

    // Same zero-HIR-footprint contract as the `type` statement form:
    // the legacy annotated assignment contributes no `HirItem` either.
    assert_eq!(
        hir.items,
        vec![HirItem::Function {
            name: "f".to_string(),
            params: vec![("x".to_string(), Ty::Int)],
            return_ty: Ty::Int,
            body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
        }]
    );
    assert_eq!(hir.type_aliases, vec![("IntAlias".to_string(), Ty::Int)]);
}

#[test]
fn a_generic_type_alias_is_rejected_with_t0042() {
    let module = pycc_parser_test_helper::parse("type Alias[T] = list[T]\n");
    let diagnostic = lower_checked(&module).unwrap_err();

    assert_eq!(diagnostic.code, "T0042");
}

#[test]
fn a_generic_type_alias_t0042_span_points_at_the_type_statement_not_byte_zero() {
    // The `type` statement is deliberately not the first line, so a
    // regression back to a hardcoded `Span::new(0, 0)` would be caught:
    // byte 0 falls inside the preceding `def f() -> int:` line, not the
    // `type Alias[T] = int` statement this diagnostic is actually about.
    let source = "def f() -> int:\n    return 1\ntype Alias[T] = int\n";
    let type_stmt_start = source.find("type Alias").unwrap() as u32;
    let type_stmt_end = source.rfind('\n').unwrap() as u32;

    let module = pycc_parser_test_helper::parse(source);
    let diagnostic = lower_checked(&module).unwrap_err();

    assert_eq!(diagnostic.code, "T0042");
    assert_ne!(diagnostic.span, Some(Span::new(0, 0)));
    assert_eq!(
        diagnostic.span,
        Some(Span::new(type_stmt_start, type_stmt_end))
    );
}

#[test]
fn an_unresolvable_type_alias_rhs_falls_through_to_the_existing_c0001_diagnostic() {
    let module = pycc_parser_test_helper::parse("type Bad = NotARealType\n");
    let diagnostic = lower_checked(&module).unwrap_err();

    assert_eq!(diagnostic.code, "C0001");
}

#[test]
fn an_unresolvable_legacy_type_alias_rhs_also_falls_through_to_c0001() {
    let module = pycc_parser_test_helper::parse("Bad: TypeAlias = NotARealType\n");
    let diagnostic = lower_checked(&module).unwrap_err();

    assert_eq!(diagnostic.code, "C0001");
}

#[test]
fn a_bare_type_alias_annotation_with_no_value_is_not_treated_as_an_alias() {
    // `X: TypeAlias` with no assigned value can't define an alias (there
    // is no RHS to resolve) -- it falls through to the ordinary
    // `AnnAssign` lowering path, where `annotation_to_ty` rejects the
    // bare name `TypeAlias` itself with the same `C0001` catch-all as
    // any other unrecognized annotation name, and the alias table stays
    // empty.
    let module = pycc_parser_test_helper::parse("X: TypeAlias\n");
    let diagnostic = lower_checked(&module).unwrap_err();

    assert_eq!(diagnostic.code, "C0001");
}

#[test]
fn a_legacy_type_alias_annotation_on_a_non_name_target_is_not_treated_as_an_alias() {
    // Unlike a `type` statement's own target (always a bare name, see
    // `lower_type_alias_stmt`'s doc comment), a legacy `AnnAssign`
    // target can be an `Attribute`/`Subscript`, e.g. `obj.x: TypeAlias =
    // int`. `lower_legacy_type_alias_ann_assign` recognizes the `X:
    // TypeAlias = ...` shape only for a bare-name target and otherwise
    // falls through to the ordinary `AnnAssign` lowering path, which
    // rejects a non-name annotated-assignment target with the same
    // `C0001` catch-all it already uses for every other non-name
    // target -- the alias table stays empty either way.
    let module = pycc_parser_test_helper::parse("obj.x: TypeAlias = int\n");
    let diagnostic = lower_checked(&module).unwrap_err();

    assert_eq!(diagnostic.code, "C0001");
}

#[test]
fn import_math_binds_the_module_namespace() {
    let module = pycc_parser_test_helper::parse("import math\n");
    let hir = lower_checked(&module).expect("recognized stdlib import must lower");

    assert_eq!(
        hir.imports,
        vec![ImportBinding::Module {
            local_name: "math".to_string(),
            module: pycc_std::StdModule::Math,
        }]
    );
    assert!(hir.items.is_empty(), "a bare `import math` has no HirItem");
}

#[test]
fn import_cgi_is_c0001() {
    let module = pycc_parser_test_helper::parse("import cgi\n");
    let diagnostic = lower_checked(&module).unwrap_err();

    assert_eq!(diagnostic.code, "C0001");
}

#[test]
fn from_math_import_sqrt_and_pi_binds_both_names() {
    let module = pycc_parser_test_helper::parse("from math import sqrt, pi\n");
    let hir = lower_checked(&module).expect("both registered symbols must resolve");

    let sqrt_symbol = pycc_std::resolve_symbol(pycc_std::StdModule::Math, "sqrt")
        .expect("math.sqrt is registered");
    let pi_symbol =
        pycc_std::resolve_symbol(pycc_std::StdModule::Math, "pi").expect("math.pi is registered");
    assert_eq!(
        hir.imports,
        vec![
            ImportBinding::Symbol {
                local_name: "sqrt".to_string(),
                module: pycc_std::StdModule::Math,
                symbol: sqrt_symbol,
            },
            ImportBinding::Symbol {
                local_name: "pi".to_string(),
                module: pycc_std::StdModule::Math,
                symbol: pi_symbol,
            },
        ]
    );
}

#[test]
fn from_math_import_sqrt_and_unregistered_tan_is_c0002_not_a_partial_bind() {
    let module = pycc_parser_test_helper::parse("from math import sqrt, tan\n");
    let diagnostic = lower_checked(&module).unwrap_err();

    // Whole statement fails closed -- `sqrt` is not partially bound
    // even though it is itself registered.
    assert_eq!(diagnostic.code, "C0002");
}

#[test]
fn from_enum_import_enum_binds_enum_marker() {
    let module = pycc_parser_test_helper::parse("from enum import Enum\n");
    let hir = lower_checked(&module).expect("enum.Enum must resolve");

    let enum_symbol = pycc_std::resolve_symbol(pycc_std::StdModule::Enum, "Enum")
        .expect("enum.Enum is registered");
    assert_eq!(
        hir.imports,
        vec![ImportBinding::Symbol {
            local_name: "Enum".to_string(),
            module: pycc_std::StdModule::Enum,
            symbol: enum_symbol,
        }]
    );
}

#[test]
fn import_math_as_m_is_c0001() {
    let module = pycc_parser_test_helper::parse("import math as m\n");
    let diagnostic = lower_checked(&module).unwrap_err();

    assert_eq!(diagnostic.code, "C0001");
}

#[test]
fn from_math_import_sqrt_as_s_is_c0001() {
    let module = pycc_parser_test_helper::parse("from math import sqrt as s\n");
    let diagnostic = lower_checked(&module).unwrap_err();

    assert_eq!(diagnostic.code, "C0001");
}

#[test]
fn from_unregistered_module_import_is_c0001() {
    let module = pycc_parser_test_helper::parse("from os import path\n");
    let diagnostic = lower_checked(&module).unwrap_err();

    assert_eq!(diagnostic.code, "C0001");
}

#[test]
fn from_dot_import_x_is_c0001() {
    let module = pycc_parser_test_helper::parse("from . import x\n");
    let diagnostic = lower_checked(&module).unwrap_err();

    assert_eq!(diagnostic.code, "C0001");
}

#[test]
fn from_math_import_star_is_c0001() {
    let module = pycc_parser_test_helper::parse("from math import *\n");
    let diagnostic = lower_checked(&module).unwrap_err();

    assert_eq!(diagnostic.code, "C0001");
}

#[test]
fn import_two_modules_in_one_statement_is_c0001() {
    let module = pycc_parser_test_helper::parse("import math, os\n");
    let diagnostic = lower_checked(&module).unwrap_err();

    assert_eq!(diagnostic.code, "C0001");
}

#[test]
fn math_sqrt_call_lowers_to_a_qualified_callee() {
    let module = pycc_parser_test_helper::parse("import math\nprint(math.sqrt(2.0))\n");
    let hir = lower_checked(&module).expect("math.sqrt(...) must lower");

    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::Call {
                callee: "math.sqrt".to_string(),
                args: vec![HirExpr::FloatLiteral(2.0)],
            }],
        }))]
    );
}

#[test]
fn math_pi_bare_reference_lowers_to_a_qualified_name() {
    let module = pycc_parser_test_helper::parse("import math\nprint(math.pi)\n");
    let hir = lower_checked(&module).expect("math.pi must lower");

    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::Name("math.pi".to_string())],
        }))]
    );
}

#[test]
fn math_tan_call_is_unsupported_since_it_is_not_registered() {
    let module = pycc_parser_test_helper::parse("import math\nmath.tan(1.0)\n");
    let diagnostic = lower_checked(&module).unwrap_err();

    assert_eq!(diagnostic.code, "C0001");
}

#[test]
fn math_tan_bare_reference_is_unsupported_since_it_is_not_registered() {
    let module = pycc_parser_test_helper::parse("import math\nprint(math.tan)\n");
    let diagnostic = lower_checked(&module).unwrap_err();

    assert_eq!(diagnostic.code, "C0001");
}

#[test]
fn os_path_bare_attribute_access_lowers_to_a_generic_attr_get() {
    // No `import os` here on purpose -- `os` isn't a registered
    // `pycc_std` module at all, so this exercises the "receiver name
    // does not resolve to a stdlib module" branch directly, distinct
    // from `math_tan_bare_reference_is_unsupported_since_it_is_not_registered`
    // above (recognized module, unregistered attribute -- which stays
    // `C0001`, see that test and `math_tan_call_is_unsupported_since_it_is_not_registered`'s
    // own updated comment). Before D-154 (Part 1 of #375), a receiver
    // that didn't resolve to a stdlib module made *any* attribute
    // access `C0001` unconditionally; D-154 adds a generic
    // instance-attribute-read shape (`HirExpr::AttrGet`) that this now
    // falls through to instead, deferring real rejection (`os` is
    // never assigned, so it isn't a class instance either) to
    // `pycc_types`.
    let module = pycc_parser_test_helper::parse("print(os.path)\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::AttrGet {
                base: Box::new(HirExpr::Name("os".to_string())),
                attr: "path".to_string(),
            }],
        }))]
    );
}

#[test]
fn attribute_access_on_a_non_name_receiver_lowers_to_a_generic_attr_get() {
    // Same D-154 widening as the `os.path` test above, exercised on a
    // receiver that isn't even a bare name (a list literal) -- `base`
    // is lowered generically regardless of its own shape.
    let module = pycc_parser_test_helper::parse("print([1, 2].sqrt)\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::AttrGet {
                base: Box::new(HirExpr::ListLiteral(vec![
                    HirExpr::IntLiteral(1),
                    HirExpr::IntLiteral(2)
                ])),
                attr: "sqrt".to_string(),
            }],
        }))]
    );
}

#[test]
fn math_sqrt_call_propagates_an_unsupported_argument_expression() {
    // Exercises the `?` inside the stdlib-call arm's own argument
    // lowering (`call.arguments.args.iter().map(lower_expr).collect()`)
    // taking its error path, as opposed to every other stdlib-call test
    // above, which only exercises the success path.
    let module = pycc_parser_test_helper::parse("import math\nmath.sqrt(1j)\n");
    let diagnostic = lower_checked(&module).unwrap_err();

    assert_eq!(diagnostic.code, "C0001");
}

#[test]
fn method_call_propagates_an_unsupported_base_expression() {
    // Exercises the `?` inside `MethodCall`'s own `base` lowering
    // (D-154), as opposed to `method_call_on_a_non_name_receiver_lowers_to_a_generic_method_call`
    // below, which only exercises the success path for a non-name base.
    let module = pycc_parser_test_helper::parse("(1j).bump()\n");
    let diagnostic = lower_checked(&module).unwrap_err();

    assert_eq!(diagnostic.code, "C0001");
}

#[test]
fn method_call_propagates_an_unsupported_argument_expression() {
    // Exercises the `?` inside `MethodCall`'s own argument lowering
    // (D-154), mirroring `math_sqrt_call_propagates_an_unsupported_argument_expression`
    // above.
    let module = pycc_parser_test_helper::parse("p.bump(1j)\n");
    let diagnostic = lower_checked(&module).unwrap_err();

    assert_eq!(diagnostic.code, "C0001");
}

#[test]
fn attr_get_propagates_an_unsupported_base_expression() {
    // Exercises the `?` inside `AttrGet`'s own `base` lowering (D-154).
    let module = pycc_parser_test_helper::parse("(1j).x\n");
    let diagnostic = lower_checked(&module).unwrap_err();

    assert_eq!(diagnostic.code, "C0001");
}

#[test]
fn method_call_on_a_non_name_receiver_lowers_to_a_generic_method_call() {
    // Exercises the call-position stdlib-intrinsic branch's own
    // `Expr::Name(receiver)` guard failing (as opposed to the bare
    // attribute-access arm's analogous guard above) -- before D-154
    // (Part 1 of #375) this was unconditionally `C0001`; now it falls
    // through to the generic `HirExpr::MethodCall` shape instead, same
    // as `os_path_bare_attribute_access_lowers_to_a_generic_attr_get`
    // above.
    let module = pycc_parser_test_helper::parse("[1, 2].sqrt()\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(
            HirExpr::MethodCall {
                base: Box::new(HirExpr::ListLiteral(vec![
                    HirExpr::IntLiteral(1),
                    HirExpr::IntLiteral(2)
                ])),
                method: "sqrt".to_string(),
                args: vec![],
            }
        ))]
    );
}

#[test]
fn import_inside_a_function_body_is_c0001() {
    // The module-level side-table is populated only by `module::lower_all`'s
    // top-level loop (mirroring `type_aliases`); a nested import still
    // reaches plain `lower_stmt`, which has no arm for `Stmt::Import`.
    let module =
        pycc_parser_test_helper::parse("def f() -> None:\n    import math\n    return None\n");
    let diagnostic = lower_checked(&module).unwrap_err();

    assert_eq!(diagnostic.code, "C0001");
}

// -- PEP 695 generic class instantiation (#387) -----------------------

/// Helper: lowers source that defines a generic class `C[T]` and then
/// uses `C[<type_arg>](<args>)` at module scope, returning the lowered
/// HIR so the test can inspect the `GenericClassInstantiate` expression.
fn lower_generic_class_instantiation(source: &str) -> crate::HirModule {
    let module = pycc_parser_test_helper::parse(source);
    lower_checked(&module).expect("test fixture should lower successfully")
}

#[test]
fn generic_class_instantiation_lowers_with_int_type_arg() {
    let hir = lower_generic_class_instantiation(
        "class C[T]:\n    def __init__(self, x: T) -> None:\n        self.x = x\nC[int](1)\n",
    );
    // The last item should be a top-level ExprStmt wrapping a
    // GenericClassInstantiate with class "C", type_arg Int, and one arg.
    let last = hir.items.last().expect("should have items");
    assert!(matches!(
        last,
        HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::GenericClassInstantiate {
            class,
            type_arg,
            args,
        })) if class == "C" && *type_arg == Ty::Int && args.len() == 1 && args[0] == HirExpr::IntLiteral(1)
    ));
}

#[test]
fn generic_class_instantiation_lowers_with_float_bool_and_str_type_args() {
    for (source, expected_ty) in [
        (
            "class C[T]:\n    def __init__(self, x: T) -> None:\n        self.x = x\nC[float](1)\n",
            Ty::Float,
        ),
        (
            "class C[T]:\n    def __init__(self, x: T) -> None:\n        self.x = x\nC[bool](1)\n",
            Ty::Bool,
        ),
        (
            "class C[T]:\n    def __init__(self, x: T) -> None:\n        self.x = x\nC[str](1)\n",
            Ty::Str,
        ),
    ] {
        let hir = lower_generic_class_instantiation(source);
        let last = hir.items.last().expect("should have items");
        assert!(matches!(
            last,
            HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::GenericClassInstantiate {
                type_arg, ..
            })) if *type_arg == expected_ty
        ));
    }
}

#[test]
fn generic_class_instantiation_rejects_a_non_name_type_arg() {
    // `C[1](args)` — the slice is a number literal, not a bare name.
    assert_capability_error_message(
        "class C[T]:\n    def __init__(self, x: T) -> None:\n        self.x = x\nC[1](1)\n",
        "a generic class type argument must be a bare type name",
    );
}

#[test]
fn generic_class_instantiation_rejects_an_unrecognized_type_arg_name() {
    // `C[unknown](args)` — the name is not one of int/float/bool/str.
    assert_capability_error_message(
        "class C[T]:\n    def __init__(self, x: T) -> None:\n        self.x = x\nC[unknown](1)\n",
        "a generic class type argument `unknown` is not supported yet",
    );
}

#[test]
fn generic_class_instantiation_rejects_a_non_name_subscript_base() {
    // `(1 + 2)[int](args)` — the subscript base is a BinOp, not a bare
    // name, so the "calling a subscript expression" rejection fires.
    assert_capability_error_message(
        "(1 + 2)[int](1)\n",
        "calling a subscript expression is not supported yet",
    );
}

#[test]
fn generic_class_instantiation_propagates_an_arg_lowering_error() {
    // `C[int](lambda: 1)` — the arg `lambda: 1` is an unsupported
    // expression that `lower_expr` rejects. This exercises the `?` on
    // the `.collect::<Result<Vec<_>, _>>()?` at expr.rs line 352,
    // propagating the error from the arg's `lower_expr` call.
    assert_capability_error_message(
        "class C[T]:\n    def __init__(self, x: T) -> None:\n        self.x = x\nC[int](lambda: 1)\n",
        "expression kind not supported yet",
    );
}

#[test]
fn rename_name_in_expr_handles_generic_class_instantiate_in_comprehension() {
    // A list comprehension whose `elt` is a `GenericClassInstantiate`
    // expression exercises `rename_name_in_expr`'s
    // `GenericClassInstantiate` arm: the loop variable `x` inside the
    // instantiation's args is renamed to the comprehension's synthesized
    // variable. The expression must lower successfully and produce a
    // `ListCompAssign` whose `elt` is a `GenericClassInstantiate`.
    let hir = lower_generic_class_instantiation(
        "class C[T]:\n    def __init__(self, x: T) -> None:\n        self.x = x\nxs = [C[int](x) for x in range(3)]\n",
    );
    // Find the ListCompAssign statement.
    let comp = hir.items.iter().find_map(|item| match item {
        HirItem::TopLevelStmt(HirStmt::ListCompAssign { elt, .. }) => Some(elt.clone()),
        _ => None,
    });
    let elt = comp.expect("should find a ListCompAssign");
    // The elt should be a GenericClassInstantiate — proving
    // rename_name_in_expr's GenericClassInstantiate arm was traversed.
    assert!(
        matches!(elt.as_ref(), HirExpr::GenericClassInstantiate { class, type_arg, .. }
                if class == "C" && *type_arg == Ty::Int),
        "expected GenericClassInstantiate elt, got {elt:?}",
    );
}

// #433: super() lowering tests.

#[test]
fn super_init_lowers_to_method_call_with_super_base() {
    // `super().__init__()` inside a method body lowers to
    // `HirExpr::MethodCall { base: Super, method: "__init__", args: [] }`.
    let module = pycc_parser_test_helper::parse(
        "class A:\n    def __init__(self) -> None:\n        return\nclass B(A):\n    def __init__(self) -> None:\n        super().__init__()\n",
    );
    let hir = lower_checked(&module).unwrap();
    // Find B.__init__'s body.
    let init = hir.items.iter().find_map(|item| match item {
        HirItem::Function { name, body, .. } if name == "B.__init__" => Some(body.first().cloned()),
        _ => None,
    });
    let stmt = init
        .flatten()
        .expect("should find B.__init__ with a non-empty body");
    assert_eq!(
        stmt,
        HirStmt::ExprStmt(HirExpr::MethodCall {
            base: Box::new(HirExpr::Super),
            method: "__init__".to_string(),
            args: vec![],
        })
    );
}

#[test]
fn super_method_lowers_to_method_call_with_super_base() {
    // `super().greet()` inside a method body lowers to
    // `HirExpr::MethodCall { base: Super, method: "greet", args: [] }`.
    let module = pycc_parser_test_helper::parse(
        "class A:\n    def __init__(self) -> None:\n        return\n    def greet(self) -> int:\n        return 1\nclass B(A):\n    def __init__(self) -> None:\n        return\n    def greet(self) -> int:\n        return super().greet()\n",
    );
    let hir = lower_checked(&module).unwrap();
    let greet = hir.items.iter().find_map(|item| match item {
        HirItem::Function { name, body, .. } if name == "B.greet" => body.first().cloned(),
        _ => None,
    });
    let stmt = greet.expect("should find B.greet with a non-empty body");
    assert_eq!(
        stmt,
        HirStmt::Return(Some(HirExpr::MethodCall {
            base: Box::new(HirExpr::Super),
            method: "greet".to_string(),
            args: vec![],
        }))
    );
}

#[test]
fn super_attr_lowers_to_attr_get_with_super_base() {
    // `super().x` inside a method body lowers to
    // `HirExpr::AttrGet { base: Super, attr: "x" }`.
    let module = pycc_parser_test_helper::parse(
        "class A:\n    def __init__(self) -> None:\n        self.x = 1\nclass B(A):\n    def __init__(self) -> None:\n        super().__init__()\n    def get_x(self) -> int:\n        return super().x\n",
    );
    let hir = lower_checked(&module).unwrap();
    let get_x = hir.items.iter().find_map(|item| match item {
        HirItem::Function { name, body, .. } if name == "B.get_x" => body.first().cloned(),
        _ => None,
    });
    let stmt = get_x.expect("should find B.get_x with a non-empty body");
    assert_eq!(
        stmt,
        HirStmt::Return(Some(HirExpr::AttrGet {
            base: Box::new(HirExpr::Super),
            attr: "x".to_string(),
        }))
    );
}

#[test]
fn bare_super_outside_method_is_c0001() {
    // A bare `super()` at top level is rejected with C0001.
    let module = pycc_parser_test_helper::parse("x = super()\n");
    let err = lower_checked(&module).unwrap_err();
    assert_eq!(err.code, "C0001");
    assert!(
        err.message.contains("bare `super()`"),
        "should mention bare super(), got: {}",
        err.message
    );
}

#[test]
fn super_with_arguments_is_not_zero_arg_super() {
    // `super(A, self)` (two-arg super) is not a zero-arg super() call,
    // so `is_zero_arg_super_call` returns false — it falls through to
    // the ordinary `Expr::Call` path, which lowers `super(A, self)` as
    // a regular call to the `super` builtin. The type checker then
    // rejects it with C0001 ("call to builtin `super` is valid Python
    // but not implemented yet"). This test verifies the HIR lowering
    // succeeds (the rejection happens later, at type-check time).
    let module = pycc_parser_test_helper::parse(
        "class A:\n    def __init__(self) -> None:\n        super(A, self).__init__()\n",
    );
    let hir = lower_checked(&module).unwrap();
    // The base of the MethodCall should be a Call to "super", not a Super.
    assert_eq!(
        hir.items,
        vec![HirItem::Function {
            name: "A.__init__".to_string(),
            params: vec![("self".to_string(), Ty::Instance(Box::new("A".to_string())))],
            return_ty: Ty::None,
            body: vec![HirStmt::ExprStmt(HirExpr::MethodCall {
                base: Box::new(HirExpr::Call {
                    callee: "super".to_string(),
                    args: vec![
                        HirExpr::Name("A".to_string()),
                        HirExpr::Name("self".to_string()),
                    ],
                }),
                method: "__init__".to_string(),
                args: vec![],
            })],
        }]
    );
}

#[test]
fn super_method_outside_method_is_c0001() {
    // `super().foo()` at top level (outside a method body) is rejected
    // with C0001 at HIR-lowering time.
    let module = pycc_parser_test_helper::parse("x = super().foo()\n");
    let err = lower_checked(&module).unwrap_err();
    assert_eq!(err.code, "C0001");
}

#[test]
fn super_attr_outside_method_is_c0001() {
    // `super().foo` at top level (outside a method body) is rejected
    // with C0001 at HIR-lowering time.
    let module = pycc_parser_test_helper::parse("x = super().foo\n");
    let err = lower_checked(&module).unwrap_err();
    assert_eq!(err.code, "C0001");
}

#[test]
fn super_method_with_unsupported_arg_is_c0001() {
    // `super().foo(x if True else y)` — the ternary argument is an
    // unsupported expression kind, so `lower_expr` on the argument
    // returns Err, which the `?` in the super().method() lowering path
    // propagates as C0001.
    let module = pycc_parser_test_helper::parse(
        "class A:\n    def __init__(self) -> None:\n        super().foo(x if True else y)\n",
    );
    let err = lower_checked(&module).unwrap_err();
    assert_eq!(err.code, "C0001");
}

#[test]
fn super_attr_assignment_is_c0001() {
    // #448: `super().attr = value` is rejected with a dedicated C0001
    // diagnostic that names super() attribute assignment, not the
    // confusing bare-super() message.
    let module = pycc_parser_test_helper::parse(
        "class A:\n    def __init__(self) -> None:\n        super().x = 1\n",
    );
    let err = lower_checked(&module).unwrap_err();
    assert_eq!(err.code, "C0001");
    assert!(
        err.message.contains("super().attr = value"),
        "should mention super().attr = value, got: {}",
        err.message
    );
    assert!(
        !err.message.contains("bare `super()`"),
        "should not use the bare-super() message, got: {}",
        err.message
    );
}

// -----------------------------------------------------------------------
// #435: compile-time isinstance/issubclass helper unit tests
// -----------------------------------------------------------------------

#[test]
fn eval_isinstance_single_covers_all_builtin_types() {
    // `float` target against `float` object — covers the `Ty::Float` arm.
    assert!(eval_isinstance_single(&Ty::Float, "float", &[]));
    assert!(!eval_isinstance_single(&Ty::Float, "int", &[]));
    // `None` object against any target — covers the `_ => false` arm.
    assert!(!eval_isinstance_single(&Ty::None, "int", &[]));
    // `Ty::Int` arm — true for "int", false for others.
    assert!(eval_isinstance_single(&Ty::Int, "int", &[]));
    assert!(!eval_isinstance_single(&Ty::Int, "str", &[]));
    // `Ty::Bool` arm — true for "bool" and "int" (subtype).
    assert!(eval_isinstance_single(&Ty::Bool, "bool", &[]));
    assert!(eval_isinstance_single(&Ty::Bool, "int", &[]));
    assert!(!eval_isinstance_single(&Ty::Bool, "str", &[]));
    // `Ty::Str` arm — true for "str", false for others.
    assert!(eval_isinstance_single(&Ty::Str, "str", &[]));
    assert!(!eval_isinstance_single(&Ty::Str, "int", &[]));
    // `Ty::Instance` arm — checks MRO membership.
    let mro = vec!["D".to_string(), "B".to_string(), "A".to_string()];
    assert!(eval_isinstance_single(
        &Ty::Instance(Box::new("D".to_string())),
        "D",
        &mro
    ));
    assert!(eval_isinstance_single(
        &Ty::Instance(Box::new("D".to_string())),
        "B",
        &mro
    ));
    assert!(eval_isinstance_single(
        &Ty::Instance(Box::new("D".to_string())),
        "A",
        &mro
    ));
    assert!(!eval_isinstance_single(
        &Ty::Instance(Box::new("D".to_string())),
        "C",
        &mro
    ));
}

#[test]
fn eval_issubclass_single_covers_builtin_same_type_and_user_vs_builtin() {
    // Same builtin type — covers the `return cls == target_class` line.
    assert!(eval_issubclass_single("int", "int", &[]));
    assert!(eval_issubclass_single("str", "str", &[]));
    assert!(!eval_issubclass_single("int", "str", &[]));
    // User class vs builtin target — covers the `return false` line.
    assert!(!eval_issubclass_single("D", "int", &["D".to_string()]));
    // `bool` is a subtype of `int` — covers the `bool`/`int` special case.
    assert!(eval_issubclass_single("bool", "int", &[]));
    assert!(eval_issubclass_single("bool", "bool", &[]));
    assert!(!eval_issubclass_single("bool", "str", &[]));
    // User class MRO check — covers the `cls_mro.iter().any` line.
    let mro = vec!["D".to_string(), "B".to_string(), "A".to_string()];
    assert!(eval_issubclass_single("D", "D", &mro));
    assert!(eval_issubclass_single("D", "B", &mro));
    assert!(eval_issubclass_single("D", "A", &mro));
    assert!(!eval_issubclass_single("D", "C", &mro));
}

#[test]
fn extract_class_names_rejects_empty_tuple_and_non_name_elements() {
    // Empty tuple — covers the `elements.is_empty()` error path.
    let result = extract_class_names(&HirExpr::TupleLiteral(vec![]));
    assert!(result.is_err());

    // Tuple with a non-name element — covers the `_ => return Err` path.
    let result = extract_class_names(&HirExpr::TupleLiteral(vec![HirExpr::IntLiteral(42)]));
    assert!(result.is_err());

    // Non-name, non-tuple expression — covers the top-level `_ => Err` path.
    let result = extract_class_names(&HirExpr::IntLiteral(99));
    assert!(result.is_err());

    // Valid single name.
    let result = extract_class_names(&HirExpr::Name("D".to_string()));
    assert_eq!(result.unwrap(), vec!["D".to_string()]);

    // Valid tuple of names.
    let result = extract_class_names(&HirExpr::TupleLiteral(vec![
        HirExpr::Name("B".to_string()),
        HirExpr::Name("C".to_string()),
    ]));
    assert_eq!(result.unwrap(), vec!["B".to_string(), "C".to_string()]);
}

#[test]
fn is_builtin_type_name_recognizes_four_scalar_types() {
    assert!(is_builtin_type_name("int"));
    assert!(is_builtin_type_name("str"));
    assert!(is_builtin_type_name("float"));
    assert!(is_builtin_type_name("bool"));
    assert!(!is_builtin_type_name("D"));
    assert!(!is_builtin_type_name("object"));
}

// -- #381: match statement lowering coverage ---------------------------

fn lower_match_stmt(source: &str) -> HirStmt {
    let module = pycc_parser_test_helper::parse(source);
    let hir = lower_checked(&module).expect("match fixture must lower");
    hir.items
        .into_iter()
        .find_map(|item| match item {
            HirItem::TopLevelStmt(s) if matches!(s, HirStmt::Match { .. }) => Some(s),
            _ => None,
        })
        .expect("expected a top-level Match statement")
}

fn lower_match_in_function(source: &str) -> Vec<HirStmt> {
    let module = pycc_parser_test_helper::parse(source);
    let hir = lower_checked(&module).expect("match fixture must lower");
    hir.items
        .into_iter()
        .find_map(|item| match item {
            HirItem::Function { body, .. } => Some(body),
            HirItem::TopLevelStmt(_) => None,
        })
        .expect("expected a function")
}

fn match_cases(stmt: &HirStmt) -> &[HirMatchCase] {
    match stmt {
        HirStmt::Match { cases, .. } => cases,
        _ => panic!("expected HirStmt::Match"),
    }
}

#[test]
#[should_panic(expected = "expected HirStmt::Match")]
fn match_cases_panics_on_non_match() {
    match_cases(&HirStmt::ExprStmt(HirExpr::IntLiteral(0)));
}

#[test]
fn lowers_match_with_int_literal_pattern() {
    let stmt =
        lower_match_stmt("x = 1\nmatch x:\n    case 1:\n        pass\n    case _:\n        pass\n");
    assert_eq!(
        stmt,
        HirStmt::Match {
            subject: HirExpr::Name("x".to_string()),
            cases: vec![
                HirMatchCase {
                    pattern: HirPattern::Literal(HirExpr::IntLiteral(1)),
                    guard: None,
                    body: vec![],
                },
                HirMatchCase {
                    pattern: HirPattern::Wildcard,
                    guard: None,
                    body: vec![],
                },
            ],
        }
    );
}

#[test]
fn lowers_match_with_float_literal_pattern() {
    let stmt = lower_match_stmt(
        "x = 1.0\nmatch x:\n    case 2.5:\n        pass\n    case _:\n        pass\n",
    );
    let cases = match_cases(&stmt);
    assert_eq!(cases.len(), 2);
    assert_eq!(
        cases[0].pattern,
        HirPattern::Literal(HirExpr::FloatLiteral(2.5))
    );
}

#[test]
fn lowers_match_with_string_literal_pattern() {
    let stmt = lower_match_stmt(
        "x = \"hi\"\nmatch x:\n    case \"hi\":\n        pass\n    case _:\n        pass\n",
    );
    let cases = match_cases(&stmt);
    assert_eq!(cases.len(), 2);
    assert_eq!(
        cases[0].pattern,
        HirPattern::Literal(HirExpr::StringLiteral("hi".to_string()))
    );
}

#[test]
fn lowers_match_with_singleton_true_pattern() {
    let stmt = lower_match_stmt(
        "x = True\nmatch x:\n    case True:\n        pass\n    case _:\n        pass\n",
    );
    let cases = match_cases(&stmt);
    assert_eq!(cases.len(), 2);
    assert_eq!(cases[0].pattern, HirPattern::Singleton(true));
}

#[test]
fn lowers_match_with_singleton_false_pattern() {
    let stmt = lower_match_stmt(
        "x = False\nmatch x:\n    case False:\n        pass\n    case _:\n        pass\n",
    );
    let cases = match_cases(&stmt);
    assert_eq!(cases.len(), 2);
    assert_eq!(cases[0].pattern, HirPattern::Singleton(false));
}

#[test]
fn lowers_match_with_none_singleton_pattern() {
    let stmt = lower_match_stmt(
        "def f() -> None:\n    pass\nmatch f():\n    case None:\n        pass\n    case _:\n        pass\n",
    );
    let cases = match_cases(&stmt);
    assert_eq!(cases.len(), 2);
    assert_eq!(cases[0].pattern, HirPattern::NoneSingleton);
}

#[test]
fn lowers_match_with_capture_pattern() {
    let stmt = lower_match_stmt("x = 42\nmatch x:\n    case y:\n        pass\n");
    let cases = match_cases(&stmt);
    assert_eq!(cases.len(), 1);
    assert_eq!(cases[0].pattern, HirPattern::Capture("y".to_string()));
}

#[test]
fn lowers_match_with_wildcard_pattern() {
    let stmt = lower_match_stmt("x = 1\nmatch x:\n    case _:\n        pass\n");
    let cases = match_cases(&stmt);
    assert_eq!(cases.len(), 1);
    assert_eq!(cases[0].pattern, HirPattern::Wildcard);
}

#[test]
fn lowers_match_with_sequence_pattern() {
    let stmt = lower_match_stmt(
        "x = [1, 2]\nmatch x:\n    case [a, b]:\n        pass\n    case _:\n        pass\n",
    );
    let cases = match_cases(&stmt);
    assert_eq!(cases.len(), 2);
    assert_eq!(
        cases[0].pattern,
        HirPattern::Sequence(vec![
            HirPattern::Capture("a".to_string()),
            HirPattern::Capture("b".to_string()),
        ])
    );
}

#[test]
fn lowers_match_with_sequence_star_pattern() {
    let stmt = lower_match_stmt(
        "x = [1, 2]\nmatch x:\n    case [a, *rest]:\n        pass\n    case _:\n        pass\n",
    );
    let cases = match_cases(&stmt);
    assert_eq!(cases.len(), 2);
    assert_eq!(
        cases[0].pattern,
        HirPattern::SequenceStar(
            vec![HirPattern::Capture("a".to_string())],
            Some("rest".to_string()),
        )
    );
}

#[test]
fn lowers_match_with_sequence_star_wildcard_pattern() {
    let stmt = lower_match_stmt(
        "x = [1, 2]\nmatch x:\n    case [a, *_]:\n        pass\n    case _:\n        pass\n",
    );
    let cases = match_cases(&stmt);
    assert_eq!(cases.len(), 2);
    assert_eq!(
        cases[0].pattern,
        HirPattern::SequenceStar(vec![HirPattern::Capture("a".to_string())], None,)
    );
}

#[test]
fn lowers_match_with_mapping_pattern() {
    let stmt = lower_match_stmt(
        "x = {\"k\": 1}\nmatch x:\n    case {\"k\": v}:\n        pass\n    case _:\n        pass\n",
    );
    let cases = match_cases(&stmt);
    assert_eq!(cases.len(), 2);
    assert_eq!(
        cases[0].pattern,
        HirPattern::Mapping(
            vec![(
                HirExpr::StringLiteral("k".to_string()),
                HirPattern::Capture("v".to_string())
            )],
            None,
        )
    );
}

#[test]
fn lowers_match_with_mapping_rest_pattern() {
    let stmt = lower_match_stmt(
        "x = {\"k\": 1}\nmatch x:\n    case {\"k\": v, **rest}:\n        pass\n    case _:\n        pass\n",
    );
    let cases = match_cases(&stmt);
    assert_eq!(cases.len(), 2);
    assert_eq!(
        cases[0].pattern,
        HirPattern::Mapping(
            vec![(
                HirExpr::StringLiteral("k".to_string()),
                HirPattern::Capture("v".to_string())
            )],
            Some("rest".to_string()),
        )
    );
}

#[test]
fn lowers_match_with_class_pattern() {
    let stmt = lower_match_stmt(
        "class P:\n    def __init__(self):\n        pass\nx = P()\nmatch x:\n    case P():\n        pass\n    case _:\n        pass\n",
    );
    let cases = match_cases(&stmt);
    assert_eq!(cases.len(), 2);
    assert_eq!(
        cases[0].pattern,
        HirPattern::Class {
            class_name: "P".to_string(),
            positional: vec![],
            keyword: vec![],
        }
    );
}

#[test]
fn lowers_match_with_class_positional_pattern() {
    let stmt = lower_match_stmt(
        "class P:\n    def __init__(self, a: int):\n        self.a = a\nx = P(1)\nmatch x:\n    case P(1):\n        pass\n    case _:\n        pass\n",
    );
    let cases = match_cases(&stmt);
    assert_eq!(cases.len(), 2);
    assert_eq!(
        cases[0].pattern,
        HirPattern::Class {
            class_name: "P".to_string(),
            positional: vec![HirPattern::Literal(HirExpr::IntLiteral(1))],
            keyword: vec![],
        }
    );
}

#[test]
fn lowers_match_with_class_keyword_pattern() {
    let stmt = lower_match_stmt(
        "class P:\n    def __init__(self):\n        self.a = 1\nx = P()\nmatch x:\n    case P(a=1):\n        pass\n    case _:\n        pass\n",
    );
    let cases = match_cases(&stmt);
    assert_eq!(cases.len(), 2);
    assert_eq!(
        cases[0].pattern,
        HirPattern::Class {
            class_name: "P".to_string(),
            positional: vec![],
            keyword: vec![("a".to_string(), HirPattern::Literal(HirExpr::IntLiteral(1)))],
        }
    );
}

#[test]
fn lowers_match_with_or_pattern() {
    let stmt = lower_match_stmt(
        "x = 2\nmatch x:\n    case 1 | 2 | 3:\n        pass\n    case _:\n        pass\n",
    );
    let cases = match_cases(&stmt);
    assert_eq!(cases.len(), 2);
    assert_eq!(
        cases[0].pattern,
        HirPattern::Or(vec![
            HirPattern::Literal(HirExpr::IntLiteral(1)),
            HirPattern::Literal(HirExpr::IntLiteral(2)),
            HirPattern::Literal(HirExpr::IntLiteral(3)),
        ])
    );
}

#[test]
fn lowers_match_with_as_pattern() {
    let stmt = lower_match_stmt(
        "x = [1, 2]\nmatch x:\n    case [a, b] as pair:\n        pass\n    case _:\n        pass\n",
    );
    let cases = match_cases(&stmt);
    assert_eq!(cases.len(), 2);
    assert_eq!(
        cases[0].pattern,
        HirPattern::As(
            Box::new(HirPattern::Sequence(vec![
                HirPattern::Capture("a".to_string()),
                HirPattern::Capture("b".to_string()),
            ])),
            "pair".to_string(),
        )
    );
}

#[test]
fn lowers_match_with_guard() {
    let stmt = lower_match_stmt(
        "x = 5\nmatch x:\n    case y if y > 3:\n        pass\n    case _:\n        pass\n",
    );
    let cases = match_cases(&stmt);
    assert_eq!(cases.len(), 2);
    assert!(cases[0].guard.is_some());
    assert!(cases[1].guard.is_none());
}

#[test]
fn lowers_match_inside_function() {
    let body = lower_match_in_function(
        "x = 1\ndef f(x: int) -> int:\n    match x:\n        case 1:\n            return 10\n        case _:\n            return 0\n",
    );
    assert!(body.iter().any(|s| matches!(s, HirStmt::Match { .. })));
}

#[test]
fn lowers_match_with_pass_only_body() {
    let stmt =
        lower_match_stmt("x = 1\nmatch x:\n    case 1:\n        pass\n    case _:\n        pass\n");
    let cases = match_cases(&stmt);
    assert!(cases[0].body.is_empty());
    assert!(cases[1].body.is_empty());
}

#[test]
fn lowers_match_with_non_literal_value_pattern_is_c0001() {
    let module = pycc_parser_test_helper::parse(
        "x = 1\nmatch x:\n    case foo.bar:\n        pass\n    case _:\n        pass\n",
    );
    let err = lower_checked(&module).unwrap_err();
    assert_eq!(err.code, "C0001");
    assert!(
        err.message
            .contains("only a literal value pattern is supported so far")
    );
}

#[test]
fn lowers_match_with_non_name_class_pattern_is_c0001() {
    let module = pycc_parser_test_helper::parse(
        "x = 1\nmatch x:\n    case int.foo():\n        pass\n    case _:\n        pass\n",
    );
    let err = lower_checked(&module).unwrap_err();
    assert_eq!(err.code, "C0001");
    assert!(
        err.message
            .contains("only a bare-name class pattern is supported so far")
    );
}

#[test]
fn lowers_match_with_nested_match() {
    let stmt = lower_match_stmt(
        "x = 1\ny = 2\nmatch x:\n    case 1:\n        match y:\n            case 2:\n                pass\n            case _:\n                pass\n    case _:\n        pass\n",
    );
    let cases = match_cases(&stmt);
    assert_eq!(cases.len(), 2);
    let inner_cases = match_cases(&cases[0].body[0]);
    assert_eq!(inner_cases.len(), 2);
}

// -- #382 coverage tests --

#[test]
fn builtin_exception_parent_returns_none_for_exception_root() {
    assert_eq!(builtin_exception_parent("Exception"), None);
}

#[test]
fn builtin_exception_parent_returns_exception_for_subclasses() {
    assert_eq!(builtin_exception_parent("ValueError"), Some("Exception"));
    assert_eq!(builtin_exception_parent("TypeError"), Some("Exception"));
    assert_eq!(builtin_exception_parent("KeyError"), Some("Exception"));
    assert_eq!(builtin_exception_parent("IndexError"), Some("Exception"));
    assert_eq!(
        builtin_exception_parent("ZeroDivisionError"),
        Some("Exception")
    );
    assert_eq!(builtin_exception_parent("RuntimeError"), Some("Exception"));
}

#[test]
fn builtin_exception_parent_returns_none_for_unknown_class() {
    assert_eq!(builtin_exception_parent("NotAnException"), None);
}

#[test]
fn is_builtin_exception_class_recognizes_all_builtins() {
    assert!(is_builtin_exception_class("Exception"));
    assert!(is_builtin_exception_class("ValueError"));
    assert!(is_builtin_exception_class("TypeError"));
    assert!(is_builtin_exception_class("KeyError"));
    assert!(is_builtin_exception_class("IndexError"));
    assert!(is_builtin_exception_class("ZeroDivisionError"));
    assert!(is_builtin_exception_class("RuntimeError"));
}

#[test]
fn is_builtin_exception_class_rejects_unknown_names() {
    assert!(!is_builtin_exception_class("NotAnException"));
    assert!(!is_builtin_exception_class(""));
}

// -- #769 (Part 2 of #747) `optional_none_test` recognizer coverage --

#[test]
fn optional_none_test_recognizes_name_is_none() {
    let test = HirExpr::Compare {
        op: CmpOpKind::Is,
        left: Box::new(HirExpr::Name("x".to_string())),
        right: Box::new(HirExpr::NoneLiteral),
    };
    assert_eq!(optional_none_test(&test), Some(("x", NoneTestPolarity::Is)));
}

#[test]
fn optional_none_test_recognizes_none_is_name_reversed_operand_order() {
    let test = HirExpr::Compare {
        op: CmpOpKind::Is,
        left: Box::new(HirExpr::NoneLiteral),
        right: Box::new(HirExpr::Name("x".to_string())),
    };
    assert_eq!(optional_none_test(&test), Some(("x", NoneTestPolarity::Is)));
}

#[test]
fn optional_none_test_recognizes_name_is_not_none() {
    let test = HirExpr::Compare {
        op: CmpOpKind::IsNot,
        left: Box::new(HirExpr::Name("x".to_string())),
        right: Box::new(HirExpr::NoneLiteral),
    };
    assert_eq!(
        optional_none_test(&test),
        Some(("x", NoneTestPolarity::IsNot))
    );
}

#[test]
fn optional_none_test_rejects_non_compare_test() {
    let test = HirExpr::Name("flag".to_string());
    assert_eq!(optional_none_test(&test), None);
}

#[test]
fn optional_none_test_rejects_non_is_compare_op() {
    let test = HirExpr::Compare {
        op: CmpOpKind::Eq,
        left: Box::new(HirExpr::Name("x".to_string())),
        right: Box::new(HirExpr::NoneLiteral),
    };
    assert_eq!(optional_none_test(&test), None);
}

#[test]
fn optional_none_test_rejects_neither_side_a_bare_name() {
    let test = HirExpr::Compare {
        op: CmpOpKind::Is,
        left: Box::new(HirExpr::NoneLiteral),
        right: Box::new(HirExpr::NoneLiteral),
    };
    assert_eq!(optional_none_test(&test), None);
}

// Issue #769 (Part 2 of #747): `definitely_terminates` direct unit tests.
// `pycc_types::narrow`'s and `pycc_mir`'s own test suites only ever exercise
// this shared predicate indirectly (through a full `check_source`/`build`
// call), which happens to reach every *reachable* code path in it but never
// isolates each one -- these tests pin `definitely_terminates`'s own
// branches directly, covering every arm of both the outer `match` and the
// inner `&&`-chain's short-circuit structure.

#[test]
fn definitely_terminates_is_false_for_an_empty_body() {
    assert!(!definitely_terminates(&[]));
}

#[test]
fn definitely_terminates_is_true_when_the_last_statement_is_a_bare_return() {
    assert!(definitely_terminates(&[HirStmt::Return(None)]));
}

#[test]
fn definitely_terminates_is_false_when_the_last_statement_is_an_unrelated_stmt() {
    assert!(!definitely_terminates(&[HirStmt::ExprStmt(HirExpr::Name(
        "x".to_string()
    ))]));
}

#[test]
fn definitely_terminates_is_false_for_a_trailing_if_with_no_else() {
    // `!orelse.is_empty()` is the first conjunct of the `&&`-chain and is
    // false here, short-circuiting before either recursive call runs.
    let body = [HirStmt::If {
        test: HirExpr::Name("flag".to_string()),
        body: vec![HirStmt::Return(None)],
        orelse: vec![],
    }];
    assert!(!definitely_terminates(&body));
}

#[test]
fn definitely_terminates_is_false_when_the_if_body_does_not_terminate() {
    // `orelse` is non-empty (first conjunct true), but
    // `definitely_terminates(body)` (the second conjunct) is false.
    let body = [HirStmt::If {
        test: HirExpr::Name("flag".to_string()),
        body: vec![HirStmt::ExprStmt(HirExpr::Name("x".to_string()))],
        orelse: vec![HirStmt::Return(None)],
    }];
    assert!(!definitely_terminates(&body));
}

#[test]
fn definitely_terminates_is_false_when_the_if_orelse_does_not_terminate() {
    // Both `orelse.is_empty()` is false and `definitely_terminates(body)`
    // is true, but the third conjunct, `definitely_terminates(orelse)`, is
    // false.
    let body = [HirStmt::If {
        test: HirExpr::Name("flag".to_string()),
        body: vec![HirStmt::Return(None)],
        orelse: vec![HirStmt::ExprStmt(HirExpr::Name("x".to_string()))],
    }];
    assert!(!definitely_terminates(&body));
}

#[test]
fn definitely_terminates_is_true_when_both_if_branches_terminate() {
    // All three conjuncts true: `orelse` is non-empty, and both the body
    // and the orelse themselves recursively terminate.
    let body = [HirStmt::If {
        test: HirExpr::Name("flag".to_string()),
        body: vec![HirStmt::Return(None)],
        orelse: vec![HirStmt::Return(None)],
    }];
    assert!(definitely_terminates(&body));
}

// Issue #769 follow-up (D-068 re-review of #780, third round): direct unit
// tests on `killed_names`/`collect_killed_names` itself, pinning every
// match arm the way `definitely_terminates`'s own tests above pin every
// arm of that predicate. `pycc_types::narrow`'s and `pycc_mir`'s own test
// suites only ever exercise this shared function indirectly (through a
// full `check_source`/`build` call over a real `.py`-shaped program),
// which happens to reach most but not all of its recursive branches --
// these tests isolate each `HirStmt` variant's own contribution to the
// killed-name set directly.

#[test]
fn killed_names_is_empty_for_an_empty_body() {
    assert!(killed_names(&[]).is_empty());
}

#[test]
fn killed_names_includes_a_plain_assign_target() {
    let body = [HirStmt::Assign {
        target: "x".to_string(),
        value: HirExpr::NoneLiteral,
    }];
    let killed = killed_names(&body);
    assert_eq!(killed, HashSet::from(["x".to_string()]));
}

#[test]
fn killed_names_includes_a_valued_ann_assign_target() {
    let body = [HirStmt::AnnAssign {
        target: "x".to_string(),
        annotation: Ty::Int,
        value: Some(HirExpr::IntLiteral(1)),
        is_final: false,
    }];
    let killed = killed_names(&body);
    assert_eq!(killed, HashSet::from(["x".to_string()]));
}

#[test]
fn killed_names_excludes_a_value_less_ann_assign_target() {
    // A value-less `AnnAssign` (`x: int` with no `= value`) is a bare
    // declaration, not an assignment -- `check_assignment` is never
    // reached for it, so it must not count as a kill.
    let body = [HirStmt::AnnAssign {
        target: "x".to_string(),
        annotation: Ty::Int,
        value: None,
        is_final: false,
    }];
    assert!(killed_names(&body).is_empty());
}

#[test]
fn killed_names_includes_the_for_range_loop_variable_and_recurses_into_its_body() {
    let body = [HirStmt::ForRange {
        var: "i".to_string(),
        start: HirExpr::IntLiteral(0),
        stop: HirExpr::IntLiteral(3),
        step: HirExpr::IntLiteral(1),
        body: vec![HirStmt::Assign {
            target: "y".to_string(),
            value: HirExpr::NoneLiteral,
        }],
    }];
    let killed = killed_names(&body);
    assert_eq!(killed, HashSet::from(["i".to_string(), "y".to_string()]));
}

#[test]
fn killed_names_includes_the_for_list_loop_variable_and_recurses_into_its_body() {
    let body = [HirStmt::ForList {
        var: "elt".to_string(),
        list: "xs".to_string(),
        body: vec![HirStmt::Assign {
            target: "y".to_string(),
            value: HirExpr::NoneLiteral,
        }],
    }];
    let killed = killed_names(&body);
    assert_eq!(killed, HashSet::from(["elt".to_string(), "y".to_string()]));
}

#[test]
fn killed_names_includes_both_target_and_var_for_a_list_comprehension_assign() {
    let body = [HirStmt::ListCompAssign {
        target: "result".to_string(),
        var: "v".to_string(),
        iter: CompIter::Name("xs".to_string()),
        cond: None,
        elt: Box::new(HirExpr::Name("v".to_string())),
    }];
    let killed = killed_names(&body);
    assert_eq!(
        killed,
        HashSet::from(["result".to_string(), "v".to_string()])
    );
}

#[test]
fn killed_names_includes_both_target_and_var_for_a_set_comprehension_assign() {
    let body = [HirStmt::SetCompAssign {
        target: "result".to_string(),
        var: "v".to_string(),
        iter: CompIter::Name("xs".to_string()),
        cond: None,
        elt: Box::new(HirExpr::Name("v".to_string())),
    }];
    let killed = killed_names(&body);
    assert_eq!(
        killed,
        HashSet::from(["result".to_string(), "v".to_string()])
    );
}

#[test]
fn killed_names_includes_both_target_and_var_for_a_dict_comprehension_assign() {
    let body = [HirStmt::DictCompAssign {
        target: "result".to_string(),
        var: "v".to_string(),
        iter: CompIter::Name("xs".to_string()),
        cond: None,
        key: Box::new(HirExpr::Name("v".to_string())),
        value: Box::new(HirExpr::Name("v".to_string())),
    }];
    let killed = killed_names(&body);
    assert_eq!(
        killed,
        HashSet::from(["result".to_string(), "v".to_string()])
    );
}

#[test]
fn killed_names_recurses_into_both_an_ifs_body_and_orelse() {
    let body = [HirStmt::If {
        test: HirExpr::Name("flag".to_string()),
        body: vec![HirStmt::Assign {
            target: "a".to_string(),
            value: HirExpr::NoneLiteral,
        }],
        orelse: vec![HirStmt::Assign {
            target: "b".to_string(),
            value: HirExpr::NoneLiteral,
        }],
    }];
    let killed = killed_names(&body);
    assert_eq!(killed, HashSet::from(["a".to_string(), "b".to_string()]));
}

#[test]
fn killed_names_recurses_into_a_whiles_body() {
    let body = [HirStmt::While {
        test: HirExpr::Name("flag".to_string()),
        body: vec![HirStmt::Assign {
            target: "x".to_string(),
            value: HirExpr::NoneLiteral,
        }],
    }];
    let killed = killed_names(&body);
    assert_eq!(killed, HashSet::from(["x".to_string()]));
}

#[test]
fn killed_names_recurses_into_every_match_case_body() {
    let body = [HirStmt::Match {
        subject: HirExpr::Name("flag".to_string()),
        cases: vec![
            HirMatchCase {
                pattern: HirPattern::Literal(HirExpr::IntLiteral(0)),
                guard: None,
                body: vec![HirStmt::Assign {
                    target: "a".to_string(),
                    value: HirExpr::NoneLiteral,
                }],
            },
            HirMatchCase {
                pattern: HirPattern::Wildcard,
                guard: None,
                body: vec![HirStmt::Assign {
                    target: "b".to_string(),
                    value: HirExpr::NoneLiteral,
                }],
            },
        ],
    }];
    let killed = killed_names(&body);
    assert_eq!(killed, HashSet::from(["a".to_string(), "b".to_string()]));
}

#[test]
fn killed_names_includes_a_match_cases_own_pattern_capture_names() {
    // D-068 re-review of #780 (fourth round, blocker finding 2): a case's
    // pattern can itself bind a bare name (`case x:`) exactly like an
    // `Assign` does -- `check_match` routes every pattern capture through
    // `check_assignment` (see `collect_killed_names`'s `Match` arm's own
    // doc comment). This must be visible even when the case body itself
    // kills nothing.
    let body = [HirStmt::Match {
        subject: HirExpr::Name("y".to_string()),
        cases: vec![HirMatchCase {
            pattern: HirPattern::Capture("x".to_string()),
            guard: None,
            body: vec![],
        }],
    }];
    let killed = killed_names(&body);
    assert_eq!(killed, HashSet::from(["x".to_string()]));
}

#[test]
fn killed_names_includes_capture_names_nested_inside_a_sequence_pattern() {
    let body = [HirStmt::Match {
        subject: HirExpr::Name("y".to_string()),
        cases: vec![HirMatchCase {
            pattern: HirPattern::SequenceStar(
                vec![HirPattern::Capture("a".to_string())],
                Some("rest".to_string()),
            ),
            guard: None,
            body: vec![],
        }],
    }];
    let killed = killed_names(&body);
    assert_eq!(killed, HashSet::from(["a".to_string(), "rest".to_string()]));
}

#[test]
fn killed_names_covers_every_pattern_kind_and_the_no_rest_branches() {
    // Exercises every `HirPattern` variant `collect_pattern_capture_names_as_killed`
    // matches on, including both arms of its `SequenceStar`/`Mapping`
    // `rest: Option<String>` branches (`Some`/`None`), mirroring
    // `pycc_types::collect_pattern_capture_names_covers_all_pattern_kinds`'s
    // own exhaustive coverage of its sibling function.
    let body = [HirStmt::Match {
        subject: HirExpr::Name("y".to_string()),
        cases: vec![
            HirMatchCase {
                pattern: HirPattern::Wildcard,
                guard: None,
                body: vec![],
            },
            HirMatchCase {
                pattern: HirPattern::Literal(HirExpr::IntLiteral(1)),
                guard: None,
                body: vec![],
            },
            HirMatchCase {
                pattern: HirPattern::Singleton(true),
                guard: None,
                body: vec![],
            },
            HirMatchCase {
                pattern: HirPattern::NoneSingleton,
                guard: None,
                body: vec![],
            },
            HirMatchCase {
                pattern: HirPattern::Sequence(vec![HirPattern::Capture("a".to_string())]),
                guard: None,
                body: vec![],
            },
            HirMatchCase {
                pattern: HirPattern::SequenceStar(vec![HirPattern::Capture("b".to_string())], None),
                guard: None,
                body: vec![],
            },
            HirMatchCase {
                pattern: HirPattern::Mapping(
                    vec![(
                        HirExpr::StringLiteral("k".to_string()),
                        HirPattern::Capture("c".to_string()),
                    )],
                    Some("mrest".to_string()),
                ),
                guard: None,
                body: vec![],
            },
            HirMatchCase {
                pattern: HirPattern::Mapping(vec![], None),
                guard: None,
                body: vec![],
            },
            HirMatchCase {
                pattern: HirPattern::Class {
                    class_name: "P".to_string(),
                    positional: vec![HirPattern::Capture("d".to_string())],
                    keyword: vec![("a".to_string(), HirPattern::Capture("e".to_string()))],
                },
                guard: None,
                body: vec![],
            },
            HirMatchCase {
                pattern: HirPattern::Or(vec![HirPattern::Capture("f".to_string())]),
                guard: None,
                body: vec![],
            },
            HirMatchCase {
                pattern: HirPattern::As(
                    Box::new(HirPattern::Capture("g".to_string())),
                    "h".to_string(),
                ),
                guard: None,
                body: vec![],
            },
        ],
    }];
    let killed = killed_names(&body);
    for expected in ["a", "b", "c", "mrest", "d", "e", "f", "g", "h"] {
        assert!(
            killed.contains(expected),
            "expected `killed_names` to include capture name `{expected}`, got {killed:?}"
        );
    }
}

#[test]
fn killed_names_recurses_into_every_part_of_a_try_statement() {
    let body = [HirStmt::Try {
        body: vec![HirStmt::Assign {
            target: "a".to_string(),
            value: HirExpr::NoneLiteral,
        }],
        handlers: vec![HirExceptHandler {
            exc_type: Some(vec!["ValueError".to_string()]),
            name: None,
            body: vec![HirStmt::Assign {
                target: "b".to_string(),
                value: HirExpr::NoneLiteral,
            }],
        }],
        orelse: vec![HirStmt::Assign {
            target: "c".to_string(),
            value: HirExpr::NoneLiteral,
        }],
        finalbody: vec![HirStmt::Assign {
            target: "d".to_string(),
            value: HirExpr::NoneLiteral,
        }],
    }];
    let killed = killed_names(&body);
    assert_eq!(
        killed,
        HashSet::from([
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
        ])
    );
}

#[test]
fn killed_names_includes_an_except_handlers_own_as_binding_name() {
    // D-068 re-review of #780 (fourth round, blocker finding 1's MIR/HIR
    // shared prescan half): `except ValueError as e:` binds `e` before the
    // handler body runs, exactly like an `Assign` -- this must be visible
    // even when the handler body itself kills nothing.
    let body = [HirStmt::Try {
        body: vec![],
        handlers: vec![HirExceptHandler {
            exc_type: Some(vec!["ValueError".to_string()]),
            name: Some("e".to_string()),
            body: vec![],
        }],
        orelse: vec![],
        finalbody: vec![],
    }];
    let killed = killed_names(&body);
    assert_eq!(killed, HashSet::from(["e".to_string()]));
}

#[test]
fn killed_names_recurses_into_every_part_of_a_try_star_statement() {
    // D-068 re-review of #780 (rebase onto #542's except* landing):
    // mirrors `killed_names_recurses_into_every_part_of_a_try_statement`
    // above for `TryStar` -- #542 landed independently of #769/#780's
    // narrowing overlay, so its own `HirStmt::TryStar` arm needs the same
    // coverage as the plain `Try` arm it mirrors.
    let body = [HirStmt::TryStar {
        body: vec![HirStmt::Assign {
            target: "a".to_string(),
            value: HirExpr::NoneLiteral,
        }],
        handlers: vec![HirExceptHandler {
            exc_type: Some(vec!["ValueError".to_string()]),
            name: None,
            body: vec![HirStmt::Assign {
                target: "b".to_string(),
                value: HirExpr::NoneLiteral,
            }],
        }],
        orelse: vec![HirStmt::Assign {
            target: "c".to_string(),
            value: HirExpr::NoneLiteral,
        }],
        finalbody: vec![HirStmt::Assign {
            target: "d".to_string(),
            value: HirExpr::NoneLiteral,
        }],
    }];
    let killed = killed_names(&body);
    assert_eq!(
        killed,
        HashSet::from([
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
        ])
    );
}

#[test]
fn killed_names_includes_a_try_star_handlers_own_as_binding_name() {
    // D-068 re-review of #780 (rebase onto #542's except* landing):
    // mirrors `killed_names_includes_an_except_handlers_own_as_binding_name`
    // above for `except* ValueError as e:` -- binds `e` to the caught
    // `ExceptionGroup` before the handler body runs, the same kill a plain
    // `except ... as e:` performs, and must be visible even when the
    // handler body itself kills nothing.
    let body = [HirStmt::TryStar {
        body: vec![],
        handlers: vec![HirExceptHandler {
            exc_type: Some(vec!["ValueError".to_string()]),
            name: Some("e".to_string()),
            body: vec![],
        }],
        orelse: vec![],
        finalbody: vec![],
    }];
    let killed = killed_names(&body);
    assert_eq!(killed, HashSet::from(["e".to_string()]));
}

#[test]
fn killed_names_ignores_statement_kinds_that_do_not_rebind_a_bare_name() {
    // `ExprStmt`, `DictSet`, `AttrSet`, `Return`, and `Raise` all route
    // through the catch-all no-op arm: none of them ever passes a bare
    // name through `check_assignment` (`DictSet`/`AttrSet` write through
    // a container/attribute slot, not a name binding).
    let body = [
        HirStmt::ExprStmt(HirExpr::Name("x".to_string())),
        HirStmt::DictSet {
            dict: "d".to_string(),
            key: HirExpr::IntLiteral(0),
            value: HirExpr::NoneLiteral,
        },
        HirStmt::AttrSet {
            base: HirExpr::Name("self".to_string()),
            attr: "field".to_string(),
            value: HirExpr::NoneLiteral,
        },
        HirStmt::Return(Some(HirExpr::IntLiteral(0))),
        HirStmt::Raise {
            exc: Some(HirExpr::Name("err".to_string())),
            cause: None,
        },
    ];
    assert!(killed_names(&body).is_empty());
}

// D-068 review of #780/#774's interaction (blocker finding 2): a bare walrus
// (`(x := ...)`) is a reassignment exactly like `HirStmt::Assign`, but it
// never appears as its own `HirStmt` variant -- it only ever shows up nested
// inside a bare `ExprStmt`'s expression or an `If`/`While` statement's
// `test`. Before this fix, `collect_killed_names` never inspected either of
// those two expression positions, so a walrus-only kill was invisible to the
// prescan.

#[test]
fn killed_names_includes_a_walrus_target_inside_a_bare_expr_stmt() {
    let body = [HirStmt::ExprStmt(HirExpr::NamedExpr {
        name: "x".to_string(),
        value: Box::new(HirExpr::NoneLiteral),
    })];
    let killed = killed_names(&body);
    assert_eq!(killed, HashSet::from(["x".to_string()]));
}

#[test]
fn killed_names_includes_a_walrus_target_inside_an_ifs_test() {
    let body = [HirStmt::If {
        test: HirExpr::NamedExpr {
            name: "x".to_string(),
            value: Box::new(HirExpr::NoneLiteral),
        },
        body: vec![],
        orelse: vec![],
    }];
    let killed = killed_names(&body);
    assert_eq!(killed, HashSet::from(["x".to_string()]));
}

#[test]
fn killed_names_includes_a_walrus_target_inside_a_whiles_test() {
    let body = [HirStmt::While {
        test: HirExpr::NamedExpr {
            name: "x".to_string(),
            value: Box::new(HirExpr::NoneLiteral),
        },
        body: vec![],
    }];
    let killed = killed_names(&body);
    assert_eq!(killed, HashSet::from(["x".to_string()]));
}

#[test]
fn killed_names_ignores_an_expr_stmt_with_no_embedded_walrus() {
    // Regresses the pre-fix behavior for the common case: a bare `ExprStmt`
    // that contains no `NamedExpr` anywhere still contributes nothing.
    let body = [HirStmt::ExprStmt(HirExpr::Call {
        callee: "print".to_string(),
        args: vec![HirExpr::Name("x".to_string())],
    })];
    assert!(killed_names(&body).is_empty());
}

/// Exercises every remaining arm of `collect_named_expr_targets_in_expr`
/// (the walrus-in-expression walker `collect_killed_names` now calls for
/// `ExprStmt`/`If`/`While`) in one deeply nested expression, pinning D-014's
/// 100% line/region coverage for a function with no wildcard arm. Each
/// non-walrus arm nests a `NamedExpr` for a distinct name one level inside
/// it, so every arm both recurses correctly and the walk still terminates
/// through the plain-literal/`Name`/`ListPop`/`Super` no-op arms at the
/// leaves.
#[test]
fn killed_names_finds_a_walrus_nested_inside_every_expression_kind() {
    fn walrus(name: &str) -> HirExpr {
        HirExpr::NamedExpr {
            name: name.to_string(),
            value: Box::new(HirExpr::NoneLiteral),
        }
    }

    let test = HirExpr::Call {
        callee: "f".to_string(),
        args: vec![
            walrus("call_arg"),
            HirExpr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(walrus("binop_left")),
                right: Box::new(walrus("binop_right")),
            },
            HirExpr::Compare {
                op: CmpOpKind::Eq,
                left: Box::new(walrus("cmp_left")),
                right: Box::new(walrus("cmp_right")),
            },
            HirExpr::UnaryOp {
                op: UnaryOpKind::USub,
                operand: Box::new(walrus("unary")),
            },
            HirExpr::FString(vec![
                FStringPart::Literal("lit".to_string()),
                FStringPart::Interpolation(Box::new(walrus("fstring"))),
            ]),
            HirExpr::ListLiteral(vec![walrus("list_elt")]),
            HirExpr::SetLiteral(vec![walrus("set_elt")]),
            HirExpr::TupleLiteral(vec![walrus("tuple_elt")]),
            HirExpr::Subscript {
                base: Box::new(walrus("subscript_base")),
                index: Box::new(walrus("subscript_index")),
            },
            HirExpr::Slice {
                base: Box::new(walrus("slice_base")),
                start: Some(Box::new(walrus("slice_start"))),
                stop: Some(Box::new(walrus("slice_stop"))),
                step: Some(Box::new(walrus("slice_step"))),
            },
            HirExpr::ListAppend {
                list: "xs".to_string(),
                value: Box::new(walrus("list_append")),
            },
            HirExpr::SetAdd {
                set: "s".to_string(),
                value: Box::new(walrus("set_add")),
            },
            HirExpr::DictLiteral(vec![(walrus("dict_key"), walrus("dict_value"))]),
            HirExpr::DictGetOrDefault {
                dict: "d".to_string(),
                key: Box::new(walrus("dict_get_key")),
                default: Box::new(walrus("dict_get_default")),
            },
            HirExpr::AttrGet {
                base: Box::new(walrus("attr_get_base")),
                attr: "field".to_string(),
            },
            HirExpr::MethodCall {
                base: Box::new(walrus("method_call_base")),
                method: "m".to_string(),
                args: vec![walrus("method_call_arg")],
            },
            HirExpr::GenericClassInstantiate {
                class: "Box".to_string(),
                type_arg: Ty::Int,
                args: vec![walrus("generic_instantiate_arg")],
            },
            // Leaves that terminate the walk without themselves nesting a
            // walrus, exercising the no-op arm.
            HirExpr::IntLiteral(0),
            HirExpr::FloatLiteral(0.0),
            HirExpr::BoolLiteral(true),
            HirExpr::StringLiteral("s".to_string()),
            HirExpr::NoneLiteral,
            HirExpr::Name("plain_name".to_string()),
            HirExpr::ListPop {
                list: "xs".to_string(),
            },
            HirExpr::Super,
        ],
    };

    let body = [HirStmt::While { test, body: vec![] }];
    let killed = killed_names(&body);
    let expected: HashSet<String> = [
        "call_arg",
        "binop_left",
        "binop_right",
        "cmp_left",
        "cmp_right",
        "unary",
        "fstring",
        "list_elt",
        "set_elt",
        "tuple_elt",
        "subscript_base",
        "subscript_index",
        "slice_base",
        "slice_start",
        "slice_stop",
        "slice_step",
        "list_append",
        "set_add",
        "dict_key",
        "dict_value",
        "dict_get_key",
        "dict_get_default",
        "attr_get_base",
        "method_call_base",
        "method_call_arg",
        "generic_instantiate_arg",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    assert_eq!(killed, expected);
}

// Issue #890: every rewritten `C0001` message names the rejected construct
// in Python terms. One exact-wording assertion per rewritten site (and per
// special-cased branch inside a site), so each is a covered region and a
// wording regression is caught here before the byte-exact fixtures in
// `tests/diagnostics` are.

#[test]
fn an_attribute_annotation_names_its_kind() {
    assert_capability_error_message(
        "import typing\ndef f(x: typing.Any) -> int:\n    return 1\n",
        "only a bare name type annotation is supported so far, got an attribute expression (`obj.attr`)",
    );
}

#[test]
fn a_string_annotation_names_its_kind() {
    assert_capability_error_message(
        "def f(x: \"int\") -> int:\n    return x\n",
        "only a bare name type annotation is supported so far, got a string literal",
    );
}

#[test]
fn a_multi_target_assignment_reports_the_target_count() {
    assert_capability_error_message(
        "a = b = c = 1\n",
        "only a single assignment target is supported so far, got 3 targets",
    );
}

#[test]
fn a_tuple_assignment_target_names_its_kind() {
    assert_capability_error_message(
        "a, b = 1, 2\n",
        "only assigning to a bare name is supported so far, got a tuple",
    );
}

#[test]
fn a_list_assignment_target_names_its_kind() {
    assert_capability_error_message(
        "[a, b] = [1, 2]\n",
        "only assigning to a bare name is supported so far, got a list display (`[...]`)",
    );
}

#[test]
fn an_annotated_attribute_target_names_its_kind() {
    assert_capability_error_message(
        "class C:\n    def __init__(self) -> None:\n        self.x: int = 1\n",
        "only assigning to a bare name is supported so far, got an attribute expression (`obj.attr`)",
    );
}

#[test]
fn an_annotated_subscript_target_names_its_kind() {
    assert_capability_error_message(
        "d = {1: 2}\nd[1]: int = 3\n",
        "only assigning to a bare name is supported so far, got a subscript expression (`obj[key]`)",
    );
}

#[test]
fn a_tuple_for_target_names_its_kind() {
    assert_capability_error_message(
        "for a, b in pairs:\n    pass\n",
        "only a bare name for-target is supported so far, got a tuple",
    );
}

#[test]
fn a_literal_for_iterable_names_its_kind() {
    assert_capability_error_message(
        "for x in [1, 2, 3]:\n    pass\n",
        "only `for x in range(...)` or `for x in <list>` is supported so far, got a list display (`[...]`) as the iterable",
    );
}

#[test]
fn a_for_call_with_a_non_bare_name_callee_names_the_callee_kind() {
    assert_capability_error_message(
        "d = {1: 2}\nfor k in d.keys():\n    pass\n",
        "only `for x in range(...)` is supported so far, got a call whose callee is an attribute expression (`obj.attr`)",
    );
}

#[test]
fn calling_the_result_of_a_call_names_the_callee_kind() {
    assert_capability_error_message(
        "def f() -> int:\n    return g()()\n",
        "only calling a bare name is supported so far, got a call whose callee is a call expression",
    );
}

#[test]
fn a_literal_comprehension_iterable_names_its_kind() {
    assert_capability_error_message(
        "xs = [k for k in [1, 2, 3]]\n",
        "only `range(...)` or a bare-name iterable is supported so far in a comprehension, got a list display (`[...]`) as the iterable",
    );
}

#[test]
fn a_protocol_body_assignment_names_its_statement_kind() {
    assert_capability_error_message(
        "from typing import Protocol\nclass P(Protocol):\n    x = 1\n",
        "a protocol class body must contain only method definitions (`def ...`) and annotated assignments (`x: int`) -- an assignment statement is not supported yet",
    );
}

#[test]
fn a_protocol_body_ellipsis_names_the_expression_inside_the_statement() {
    assert_capability_error_message(
        "from typing import Protocol\nclass P(Protocol): ...\n",
        "a protocol class body must contain only method definitions (`def ...`) and annotated assignments (`x: int`) -- an expression statement (an `...` ellipsis literal) is not supported yet",
    );
}

#[test]
fn a_boolean_operator_receiver_names_its_expression_kind() {
    assert_capability_error_message(
        "def f(a: str, b: str) -> str:\n    return (a or b).upper()\n",
        "expression kind not supported yet: an `and`/`or` boolean expression",
    );
}

#[test]
fn an_unsupported_statement_names_its_kind() {
    assert_capability_error_message(
        "with open(\"f\") as fh:\n    pass\n",
        "statement kind not supported yet: a `with` statement",
    );
}

#[test]
fn a_nested_def_is_qualified_by_its_position() {
    assert_capability_error_message(
        "def outer() -> int:\n    def inner() -> int:\n        return 1\n    return 1\n",
        "statement kind not supported yet: a `def` nested inside a function or block body (only a module-level `def` or a method in a class body is supported)",
    );
}

#[test]
fn a_def_under_a_module_level_if_is_qualified_by_its_position() {
    assert_capability_error_message(
        "flag = True\nif flag:\n    def f() -> int:\n        return 1\n",
        "statement kind not supported yet: a `def` nested inside a function or block body (only a module-level `def` or a method in a class body is supported)",
    );
}

#[test]
fn a_nested_class_is_qualified_by_its_position() {
    assert_capability_error_message(
        "def outer() -> int:\n    class Inner:\n        def __init__(self) -> None:\n            return\n    return 1\n",
        "statement kind not supported yet: a `class` nested inside a function or block body (only a module-level `class` is supported)",
    );
}

#[test]
fn a_function_local_import_is_qualified_by_its_position() {
    assert_capability_error_message(
        "def f() -> int:\n    import os\n    return 1\n",
        "statement kind not supported yet: an `import` inside a function or block body (only a module-level import, or one inside an `if TYPE_CHECKING:` guard, is supported)",
    );
}

#[test]
fn a_function_local_from_import_is_qualified_by_its_position() {
    assert_capability_error_message(
        "def f() -> int:\n    from os import path\n    return 1\n",
        "statement kind not supported yet: an `import` inside a function or block body (only a module-level import, or one inside an `if TYPE_CHECKING:` guard, is supported)",
    );
}

#[test]
fn break_inside_a_loop_names_the_construct() {
    assert_capability_error_message(
        "for i in range(3):\n    break\n",
        "statement kind not supported yet: `break` inside a loop",
    );
}

#[test]
fn continue_inside_a_loop_names_the_construct() {
    assert_capability_error_message(
        "for i in range(3):\n    continue\n",
        "statement kind not supported yet: `continue` inside a loop",
    );
}

#[test]
fn no_capability_message_renders_an_ast_debug_dump() {
    // Issue #890: every `C0001` HIR lowering emits names the construct in
    // Python terms; the `Debug` form of a `ruff_python_ast` node always
    // carries `node_index: NodeIndex(`, so its absence proves no dump
    // leaked through any of the rewritten sites.
    for source in [
        "import typing\ndef f(x: typing.Any) -> int:\n    return 1\n",
        "a = b = 1\n",
        "a, b = 1, 2\n",
        "class C:\n    def __init__(self) -> None:\n        self.x: int = 1\n",
        "for a, b in pairs:\n    pass\n",
        "for x in [1]:\n    pass\n",
        "d = {1: 2}\nfor k in d.keys():\n    pass\n",
        "def f() -> int:\n    return g()()\n",
        "xs = [k for k in [1]]\n",
        "from typing import Protocol\nclass P(Protocol):\n    x = 1\n",
    ] {
        let module = pycc_parser_test_helper::parse(source);
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "C0001", "{source:?}");
        assert!(
            !diagnostic.message.contains("NodeIndex("),
            "{source:?}: {}",
            diagnostic.message
        );
    }
}

// -- #911 (Part 1 of #885): class-level attributes at the lowering seam ----
//
// The end-to-end suite (`tests/issue_911_class_attrs.rs`) drives these same
// shapes through the compiled `pycc` binary. These crate-local tests pin the
// same behavior directly at `lower_checked`, the seam that actually owns it:
// `class::body`'s `AnnAssign` arm, `func::annotation_to_ty`'s two `ClassVar`
// rejections, and `lower_class`'s post-walk collision reconciliation.

#[test]
fn an_annotated_class_body_attribute_lowers_to_a_class_attr() {
    let module = pycc_parser_test_helper::parse(
        "class C:\n    X: int = 1\n    K: str = \"a\"\n    S: float = -1.5\n    D: bool = True\n\n    def __init__(self) -> None:\n        self.n = 0\n",
    );
    let hir = lower_checked(&module).expect("the class body must lower");
    let (_, class_def) = hir
        .class_defs
        .iter()
        .find(|(name, _)| name == "C")
        .expect("class `C` must be lowered");

    assert_eq!(
        class_def.class_attrs,
        vec![
            ("X".to_string(), Ty::Int, ClassAttrValue::Int(1)),
            (
                "K".to_string(),
                Ty::Str,
                ClassAttrValue::Str("a".to_string())
            ),
            ("S".to_string(), Ty::Float, ClassAttrValue::Float(-1.5)),
            ("D".to_string(), Ty::Bool, ClassAttrValue::Bool(true)),
        ]
    );
    // D-154: a class attribute is a compile-time constant, so it must never
    // appear among the instance attribute slots.
    assert_eq!(class_def.attrs, vec![("n".to_string(), Ty::Int)]);
}

#[test]
fn a_class_var_wrapped_class_body_attribute_lowers_to_the_same_class_attr() {
    let module = pycc_parser_test_helper::parse(
        "from typing import ClassVar\n\n\nclass C:\n    X: ClassVar[int] = 7\n\n    def __init__(self) -> None:\n        self.n = 0\n",
    );
    let hir = lower_checked(&module).expect("the class body must lower");
    let (_, class_def) = hir
        .class_defs
        .iter()
        .find(|(name, _)| name == "C")
        .expect("class `C` must be lowered");

    assert_eq!(
        class_def.class_attrs,
        vec![("X".to_string(), Ty::Int, ClassAttrValue::Int(7))]
    );
}

#[test]
fn a_bare_class_var_class_body_annotation_propagates_the_strip_error() {
    // `strip_class_var`'s own error propagating out of the `AnnAssign` arm,
    // before the annotation is ever resolved.
    assert_capability_error_message(
        "class C:\n    X: ClassVar = 1\n\n    def __init__(self) -> None:\n        self.n = 0\n",
        "a bare `ClassVar` is not a valid annotation",
    );
}

#[test]
fn a_class_var_in_a_dataclass_body_is_rejected_at_lowering() {
    assert_capability_error_message(
        "@dataclass\nclass C:\n    x: int\n    LIMIT: ClassVar[int] = 8\n",
        "`ClassVar` in a `@dataclass` body is not supported yet",
    );
}

#[test]
fn a_class_var_annotation_outside_a_class_body_is_rejected_at_lowering() {
    // Both `annotation_to_ty` arms: the bare name and the subscripted form.
    assert_capability_error_message(
        "def f(x: ClassVar) -> int:\n    return 1\n",
        "only valid on a class-body attribute declaration",
    );
    assert_capability_error_message(
        "def f(x: ClassVar[int]) -> int:\n    return 1\n",
        "only valid on a class-body attribute declaration",
    );
}

#[test]
fn a_class_attribute_colliding_with_an_instance_slot_is_rejected_at_lowering() {
    // `lower_class`'s post-walk `reject_class_attr_collisions` call, in both
    // declaration orders -- the check deliberately does not run at the
    // `AnnAssign` site, where `attrs` is still empty.
    assert_capability_error_message(
        "class C:\n    x: int = 1\n\n    def __init__(self) -> None:\n        self.x = 2\n",
        "collides with an instance attribute",
    );
    assert_capability_error_message(
        "class C:\n    def __init__(self) -> None:\n        self.x = 2\n\n    x: int = 1\n",
        "collides with an instance attribute",
    );
}

#[test]
fn a_derived_class_attribute_with_no_mro_collision_lowers_at_the_hir_seam() {
    // The "this base is clean, keep walking" path through
    // `reject_class_attr_collisions`: every rejection test returns on the
    // first base it inspects, so none of them completes a loop iteration.
    let module = pycc_parser_test_helper::parse(
        "class Base:\n    def __init__(self) -> None:\n        self.n = 3\n\n    @property\n    def twice(self) -> int:\n        return self.n * 2\n\n\nclass Derived(Base):\n    LIMIT: int = 7\n\n    def __init__(self) -> None:\n        self.n = 5\n",
    );
    let hir = lower_checked(&module).expect("the derived class must lower");
    let (_, derived) = hir
        .class_defs
        .iter()
        .find(|(name, _)| name == "Derived")
        .expect("class `Derived` must be lowered");

    assert_eq!(
        derived.class_attrs,
        vec![("LIMIT".to_string(), Ty::Int, ClassAttrValue::Int(7))]
    );
}
