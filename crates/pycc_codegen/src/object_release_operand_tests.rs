//! Part 3 of #1092: the release of unbound produced operands of `print`,
//! f-strings, `raise`, conditional expressions and boolean operators.
//!
//! Each test pairs a produced operand (`copy.a`, a new reference the
//! consumer must release) with its borrowed control (`copy`, a module
//! global the consumer must never release), so a test that passes because
//! nothing is ever released, or because everything is, fails one side.

use super::*;
use pycc_mir::{BoolOpKind, MirFStringPart};

/// A call to the shim's `pycc_ext_obj_retain` (Part 1 of #1499).
const RETAIN: &str = "call void @pycc_ext_obj_retain(";

fn retains(text: &str) -> usize {
    text.matches(RETAIN).count()
}

/// The header line (`label: ; preds = ...`) of every block that retains,
/// so a test can tell which arm's branch the retain sits on.
fn retaining_headers(ir: &str) -> Vec<&str> {
    ir.split("\n\n")
        .filter(|block| block.contains(RETAIN))
        .map(|block| block.lines().next().unwrap_or_default())
        .collect()
}

fn print(args: Vec<MirExpr>) -> MirStmt {
    MirStmt::ExprStmt(MirExpr::Call {
        callee: "print".to_string(),
        args,
        ty: Ty::None,
    })
}

fn fstring(value: MirExpr) -> MirExpr {
    MirExpr::FString(vec![
        MirFStringPart::Literal("v=".to_string()),
        MirFStringPart::Interpolation(boxed(value)),
    ])
}

fn if_exp(test: MirExpr, body: MirExpr, orelse: MirExpr, ty: Ty) -> MirExpr {
    MirExpr::IfExp {
        test: boxed(test),
        body: boxed(body),
        orelse: boxed(orelse),
        ty,
    }
}

fn bool_op(op: BoolOpKind, left: MirExpr, right: MirExpr, ty: Ty, truth_only: bool) -> MirExpr {
    MirExpr::BoolOp {
        op,
        left: boxed(left),
        right: boxed(right),
        ty,
        truth_only,
    }
}

/// `print(copy.a, 1, copy.b)`: each produced argument is held until its
/// own write, so the second argument's failure releases the first, and each
/// is released once its text is written. `print(copy, 1, copy)` releases
/// nothing.
#[test]
fn print_releases_each_produced_argument_after_writing_it() {
    let ir = f_ir(
        "release_print",
        vec![print(vec![attr("a"), MirExpr::IntLiteral(1), attr("b")])],
    );
    let fails = blocks(&ir, "foreign_attr_fail");
    assert_eq!(fails.len(), 2, "{ir}");
    assert_eq!(releases(fails[0]), 0, "{ir}");
    assert_eq!(releases(fails[1]), 1, "the first argument: {ir}");
    // The first argument's conversion fails holding both; the second's
    // holds only itself, the first being written and released already.
    let to_str = blocks(&ir, "foreign_to_str_fail");
    assert_eq!(to_str.len(), 2, "{ir}");
    assert_eq!(releases(to_str[0]), 2, "{ir}");
    assert_eq!(releases(to_str[1]), 1, "{ir}");
    // Plus the second argument's `NameError` exit and effect check (one
    // each) and each argument's release after its write.
    assert_eq!(releases(&ir), 8, "{ir}");
    let borrowed = f_ir(
        "release_print_borrowed",
        vec![print(vec![
            copy_name(),
            MirExpr::IntLiteral(1),
            copy_name(),
        ])],
    );
    assert_eq!(releases(&borrowed), 0, "{borrowed}");
}

/// `f"v={copy.a}"` releases the interpolated object once it is formatted;
/// `f"v={copy}"` does not.
#[test]
fn an_fstring_releases_a_produced_interpolation() {
    let ir = discard_ir("release_fstring", fstring(attr("a")));
    assert_eq!(releases(&ir), 2, "{ir}");
    let borrowed = discard_ir("release_fstring_borrowed", fstring(copy_name()));
    assert_eq!(releases(&borrowed), 0, "{borrowed}");
}

/// `raise copy.a` releases its produced operand after the raise has taken
/// its own reference; `raise copy` does not.
#[test]
fn a_raise_releases_its_produced_operand() {
    let raise = |value| MirStmt::ObjRaise { value };
    let ir = f_ir("release_raise", vec![raise(attr("a"))]);
    assert_eq!(releases(&ir), 1, "{ir}");
    let raised = ir
        .find("@pycc_ext_obj_raise(")
        .unwrap_or_else(|| panic!("{ir}"));
    let release = ir.rfind(RELEASE).unwrap_or_else(|| panic!("{ir}"));
    assert!(raised < release, "released after the raise: {ir}");
    let borrowed = f_ir("release_raise_borrowed", vec![raise(copy_name())]);
    assert_eq!(releases(&borrowed), 0, "{borrowed}");
}

/// `1 if copy.a else 2` releases its produced test after the truth test;
/// `1 if copy else 2` does not.
#[test]
fn a_conditional_expression_releases_its_produced_test() {
    let int = MirExpr::IntLiteral;
    let ir = discard_ir(
        "release_if_exp_test",
        if_exp(attr("a"), int(1), int(2), Ty::Int),
    );
    assert_eq!(releases(&ir), 2, "{ir}");
    let borrowed = discard_ir(
        "release_if_exp_test_borrowed",
        if_exp(copy_name(), int(1), int(2), Ty::Int),
    );
    assert_eq!(releases(&borrowed), 0, "{borrowed}");
}

/// An `object`-typed conditional expression with an owned arm is produced
/// (Part 3 of #1499): its borrowed arm is retained on its own branch, a
/// produced one is not, and the discarded result is released once either
/// way. With both arms borrowed it stays a borrow: no retain, no release.
#[test]
fn a_conditional_object_result_retains_a_borrowed_arm() {
    let ir = discard_ir(
        "release_if_exp_result",
        if_exp(copy_name(), attr("a"), attr("b"), Ty::Object),
    );
    assert_eq!((retains(&ir), releases(&ir)), (0, 1), "{ir}");
    let mixed = discard_ir(
        "release_if_exp_mixed",
        if_exp(copy_name(), attr("a"), copy_name(), Ty::Object),
    );
    assert_eq!((retains(&mixed), releases(&mixed)), (1, 1), "{mixed}");
    // The borrowed arm's `copy` read is a guarded global load, so its
    // retain sits in the block the `orelse` branch falls into.
    let headers = retaining_headers(&mixed);
    assert_eq!(headers.len(), 1, "{mixed}");
    assert!(
        headers[0].ends_with("preds = %ifexp_orelse"),
        "the borrowed arm retains on its own branch\n{mixed}"
    );
    let borrowed = discard_ir(
        "release_if_exp_borrowed",
        if_exp(copy_name(), copy_name(), copy_name(), Ty::Object),
    );
    assert_eq!(
        (retains(&borrowed), releases(&borrowed)),
        (0, 0),
        "{borrowed}"
    );
}

/// `if copy.a and copy.b:` tests each operand's truth and releases each
/// produced one after its test; borrowed operands are never released.
#[test]
fn a_truth_only_boolean_operator_releases_each_produced_operand() {
    let stmt = |left, right| MirStmt::If {
        test: bool_op(BoolOpKind::And, left, right, Ty::Bool, true),
        body: vec![MirStmt::Return(None)],
        orelse: Vec::new(),
    };
    let ir = f_ir("release_boolop_truth", vec![stmt(attr("a"), attr("b"))]);
    assert_eq!(releases(&ir), 4, "{ir}");
    let borrowed = f_ir(
        "release_boolop_truth_borrowed",
        vec![stmt(copy_name(), copy_name())],
    );
    assert_eq!(releases(&borrowed), 0, "{borrowed}");
}

/// `copy.a or copy.b` (an object result): the left operand is held across
/// its truth test, released when the right is evaluated instead, and
/// otherwise becomes the result, which the discarding statement releases.
/// Since Part 3 of #1499 a borrowed arm beside an owned one is retained
/// where it is selected, so the result is released whichever arm produced
/// it; with every arm borrowed the node is a borrow with no traffic.
#[test]
fn a_value_boolean_operator_releases_an_unselected_left_operand() {
    let ir = discard_ir(
        "release_boolop_value",
        bool_op(BoolOpKind::Or, attr("a"), attr("b"), Ty::Object, false),
    );
    assert_eq!((retains(&ir), releases(&ir)), (0, 3), "{ir}");
    let mixed = discard_ir(
        "release_boolop_value_mixed",
        bool_op(BoolOpKind::Or, attr("a"), copy_name(), Ty::Object, false),
    );
    assert_eq!((retains(&mixed), releases(&mixed)), (1, 3), "{mixed}");
    let headers = retaining_headers(&mixed);
    assert_eq!(headers.len(), 1, "{mixed}");
    assert!(
        headers[0].ends_with("preds = %boolop_eval_right"),
        "{mixed}"
    );
    let left_borrowed = discard_ir(
        "release_boolop_value_left_borrowed",
        bool_op(BoolOpKind::And, copy_name(), attr("b"), Ty::Object, false),
    );
    assert_eq!(
        (retains(&left_borrowed), releases(&left_borrowed)),
        (1, 1),
        "{left_borrowed}"
    );
    let take_left = blocks(&left_borrowed, "boolop_take_left");
    assert_eq!(take_left.len(), 1, "{left_borrowed}");
    assert_eq!(retains(take_left[0]), 1, "{left_borrowed}");
    let borrowed = discard_ir(
        "release_boolop_value_borrowed",
        bool_op(BoolOpKind::Or, copy_name(), copy_name(), Ty::Object, false),
    );
    assert_eq!(
        (retains(&borrowed), releases(&borrowed)),
        (0, 0),
        "{borrowed}"
    );
    // A boxed native arm is a new reference already: only the borrowed
    // `copy` beside it is retained.
    let boxed_native = discard_ir(
        "release_boolop_value_boxed",
        bool_op(
            BoolOpKind::Or,
            copy_name(),
            MirExpr::IntLiteral(1),
            Ty::Object,
            false,
        ),
    );
    assert_eq!(
        (retains(&boxed_native), releases(&boxed_native)),
        (1, 1),
        "{boxed_native}"
    );
}

/// The classifier's Part 3 arms: a comprehension over an object, and an
/// object-typed value `BoolOp`/`IfExp` with at least one owned arm -- a
/// produced object or a converted scalar -- whose borrowed arm (Part 3 of
/// #1499) is then retained. An all-borrowed node and a truth-only `BoolOp`
/// are never produced.
#[test]
fn the_part_3_producers_are_classified() {
    let comprehension = |source| {
        MirExpr::Comprehension(Box::new(pycc_mir::MirComprehension {
            var: "x".to_string(),
            var_ty: Ty::Object,
            source,
            cond: None,
            elt: pycc_mir::MirCompElt::List(MirExpr::IntLiteral(1)),
        }))
    };
    assert!(is_produced(&comprehension(pycc_mir::CompSource::Object(
        copy_name()
    ))));
    let value = |left, right| bool_op(BoolOpKind::Or, left, right, Ty::Object, false);
    assert!(is_produced(&value(attr("a"), attr("b"))));
    assert!(is_produced(&value(attr("a"), MirExpr::IntLiteral(1))));
    assert!(is_produced(&value(attr("a"), copy_name())));
    assert!(!is_produced(&value(copy_name(), copy_name())));
    let boxed = |inner| MirExpr::ObjectBox(Box::new(inner));
    // A boxed object arm is never inserted inside an operator today; it is
    // not produced, so it is not an owned arm (see `selects_an_owned_arm`).
    assert!(!is_produced(&value(
        copy_name(),
        boxed(MirExpr::IntLiteral(7))
    )));
    assert!(!is_produced(&value(
        copy_name(),
        boxed(MirExpr::NoneLiteral)
    )));
    // An arm of any other type is never treated as owned.
    assert!(!is_produced(&value(copy_name(), MirExpr::NoneLiteral)));
    assert!(!is_produced(&bool_op(
        BoolOpKind::Or,
        attr("a"),
        attr("b"),
        Ty::Bool,
        true
    )));
    let choose = |body, orelse| if_exp(copy_name(), body, orelse, Ty::Object);
    assert!(is_produced(&choose(attr("a"), attr("b"))));
    assert!(is_produced(&choose(copy_name(), attr("b"))));
    assert!(!is_produced(&choose(copy_name(), copy_name())));
    assert!(!is_produced(&choose(
        boxed(MirExpr::IntLiteral(7)),
        copy_name()
    )));
    assert!(!is_produced(&if_exp(
        copy_name(),
        MirExpr::IntLiteral(1),
        MirExpr::IntLiteral(2),
        Ty::Int
    )));
}
