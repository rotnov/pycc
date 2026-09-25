//! Class-attribute initializer shapes, extracted from [`super::attrs`] (D-185:
//! decompose the part a change touches).
//!
//! `super::attrs` owns the declaration-level checks (target shape, reserved
//! names, duplicates, annotation wrappers, collisions). This module owns the
//! right-hand side: inferring an un-annotated attribute's type from its
//! literal, extracting the compile-time constant, and the shared `C0001`s
//! both spellings report for a missing or unsupported initializer.

use super::ClassAttrValue;
use crate::{Ty, unsupported};
use pycc_ast::{Expr, Number, UnaryOp};
use pycc_diag::Diagnostic;

/// #910: The natural type of an un-annotated class attribute's literal
/// right-hand side, or `None` when the shape is not one this pass folds.
///
/// A unary `+`/`-` is unwrapped first, because `X = -1` parses as
/// `UnaryOp(USub, NumberLiteral(1))` and is not a literal at all. Only
/// `USub`/`UAdd` are unwrapped: `~1` and `not True` are constant-foldable in
/// principle but are deliberately out of scope, so they yield `None` here and
/// are reported as an unsupported initializer shape.
///
/// This deliberately mirrors, and never widens, [`class_attr_value`]'s
/// accepted set. A complex literal (`1j`) has no `Ty` in this compiler and
/// yields `None` rather than a type `class_attr_value` would then reject with
/// a second, less specific message.
pub(super) fn infer_class_attr_ty(value: &Expr) -> Option<Ty> {
    let inner = match value {
        Expr::UnaryOp(unary) if matches!(unary.op, UnaryOp::USub | UnaryOp::UAdd) => {
            unary.operand.as_ref()
        }
        other => other,
    };
    match inner {
        Expr::NumberLiteral(number) => match number.value {
            Number::Int(_) => Some(Ty::Int),
            Number::Float(_) => Some(Ty::Float),
            Number::Complex { .. } => None,
        },
        // `BooleanLiteral` is matched before `NumberLiteral` cannot apply --
        // Python's `True`/`False` parse as their own node, not as an int.
        Expr::BooleanLiteral(_) => Some(Ty::Bool),
        Expr::StringLiteral(_) => Some(Ty::Str),
        _ => None,
    }
}

/// The `C0001` for a class-attribute declaration with no initializer.
///
/// Shared by the annotated path's own check and #916's bare-`Final` arm,
/// which must reach the same conclusion before it has a type to infer.
pub(super) fn no_class_attr_value(attr_name: &str, range: std::ops::Range<u32>) -> Diagnostic {
    unsupported(
        format!(
            "class attribute `{attr_name}` has no value -- a class attribute is a \
             compile-time constant and must be initialized with a literal \
             (`{attr_name}: int = 1`)"
        ),
        range,
    )
}

/// The `C0001` for a class-attribute initializer whose shape this pass does
/// not fold, shared by both spellings so they report identically.
pub(super) fn bad_class_attr_shape(attr_name: &str, range: std::ops::Range<u32>) -> Diagnostic {
    unsupported(
        format!(
            "class attribute `{attr_name}` must be initialized with a literal -- only an \
             `int`, `float`, `str`, or `bool` literal (optionally with a unary `+`/`-` on a \
             number) is supported, because a class attribute is a compile-time constant"
        ),
        range,
    )
}

/// #911: Extracts the compile-time constant value of a class attribute from
/// its right-hand side, checking it against the declared annotation.
///
/// Accepted shapes are deliberately wider than the enum-member extractor's
/// (`class/enum_class.rs`): a unary `+`/`-` applied to a numeric literal is
/// accepted too, because the motivating example in #885 is
/// `MIN_WIDTH: int = -1024`, which parses as `UnaryOp(USub,
/// NumberLiteral(1024))` and not as a literal at all.
pub(super) fn class_attr_value(
    value: &Expr,
    attr_ty: &Ty,
    attr_name: &str,
    range: std::ops::Range<u32>,
) -> Result<ClassAttrValue, Diagnostic> {
    let bad_shape = || bad_class_attr_shape(attr_name, range.clone());
    let mismatch = |found: &str| {
        unsupported(
            format!(
                "class attribute `{attr_name}` is annotated `{}` but is initialized with a \
                 `{found}` literal",
                attr_ty.name()
            ),
            range.clone(),
        )
    };
    // Unary `+`/`-` on a numeric literal, unwrapped to a signed number.
    let (negate, literal) = match value {
        Expr::UnaryOp(unary) => {
            let sign = match unary.op {
                pycc_ast::UnaryOp::USub => true,
                pycc_ast::UnaryOp::UAdd => false,
                _ => return Err(bad_shape()),
            };
            if !matches!(unary.operand.as_ref(), Expr::NumberLiteral(_)) {
                return Err(bad_shape());
            }
            (sign, unary.operand.as_ref())
        }
        other => (false, other),
    };
    match literal {
        Expr::NumberLiteral(number) => match &number.value {
            Number::Int(i) => {
                let Some(magnitude) = i.as_i64() else {
                    return Err(unsupported(
                        format!(
                            "class attribute `{attr_name}` has an integer value that does not \
                             fit in i64 -- only i64-range values are supported"
                        ),
                        range,
                    ));
                };
                let signed = if negate { -magnitude } else { magnitude };
                match attr_ty {
                    Ty::Int => Ok(ClassAttrValue::Int(signed)),
                    // An `int` literal under a `float` annotation widens,
                    // matching Python's own numeric tower (`x: float = 1`).
                    Ty::Float => Ok(ClassAttrValue::Float(signed as f64)),
                    _ => Err(mismatch("int")),
                }
            }
            Number::Float(f) => {
                let signed = if negate { -*f } else { *f };
                match attr_ty {
                    Ty::Float => Ok(ClassAttrValue::Float(signed)),
                    _ => Err(mismatch("float")),
                }
            }
            Number::Complex { .. } => Err(bad_shape()),
        },
        // `negate` is `true` only when the operand was a `NumberLiteral`
        // (checked above), so no sign can reach these two arms.
        Expr::BooleanLiteral(b) => match attr_ty {
            Ty::Bool => Ok(ClassAttrValue::Bool(b.value)),
            _ => Err(mismatch("bool")),
        },
        Expr::StringLiteral(s) => match attr_ty {
            Ty::Str => Ok(ClassAttrValue::Str(s.value.to_str().to_string())),
            _ => Err(mismatch("str")),
        },
        _ => Err(bad_shape()),
    }
}

#[cfg(test)]
mod tests {
    use super::{ClassAttrValue, Ty};
    use crate::lower_checked;

    /// Asserts `source` is rejected with a `C0001` whose message contains
    /// `needle`. `crate::class::tests::assert_c0001` pins only the code, and
    /// every case here needs the *specific* collision partner named.
    fn assert_collision(source: &str, needle: &str) {
        let module = crate::pycc_parser_test_helper::parse(source);
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "C0001", "source: {source:?}");
        assert!(
            diagnostic.message.contains(needle),
            "expected {needle:?} in {:?}",
            diagnostic.message
        );
    }

    // -- #910: un-annotated class attributes, pure-rejection paths ---------
    //
    // The accepted surface and its constant folding are pinned end to end
    // through the CLI in `tests/issue_910_unannotated_class_attrs.rs`; these
    // cover the shapes that never produce a program to run.

    #[test]
    fn an_unannotated_complex_literal_is_rejected() {
        assert_collision(
            "class C:\n    X = 1j\n",
            "must be initialized with a literal",
        );
    }

    #[test]
    fn an_unannotated_bitwise_not_is_rejected() {
        assert_collision(
            "class C:\n    X = ~1\n",
            "must be initialized with a literal",
        );
    }

    #[test]
    fn an_unannotated_logical_not_is_rejected() {
        assert_collision(
            "class C:\n    X = not True\n",
            "must be initialized with a literal",
        );
    }

    #[test]
    fn an_unannotated_call_initializer_is_rejected() {
        assert_collision(
            "def f() -> int:\n    return 1\n\n\nclass C:\n    X = f()\n",
            "must be initialized with a literal",
        );
    }

    #[test]
    fn an_unannotated_negated_call_initializer_is_rejected() {
        assert_collision(
            "def f() -> int:\n    return 1\n\n\nclass C:\n    X = -f()\n",
            "must be initialized with a literal",
        );
    }

    #[test]
    fn an_unannotated_bare_name_initializer_is_rejected() {
        assert_collision(
            "Y: int = 1\n\n\nclass C:\n    X = Y\n",
            "must be initialized with a literal",
        );
    }

    #[test]
    fn an_unannotated_arithmetic_initializer_is_rejected() {
        assert_collision(
            "class C:\n    X = 1 + 2\n",
            "must be initialized with a literal",
        );
    }

    /// A unary `-` is unwrapped before inference, so `-"a"` infers `str` and
    /// is caught by the literal extractor's own numeric-operand check rather
    /// than by inference. Both report the same message.
    #[test]
    fn an_unannotated_negated_string_is_rejected() {
        assert_collision(
            "class C:\n    X = -\"a\"\n",
            "must be initialized with a literal",
        );
    }

    #[test]
    fn an_unannotated_int_outside_i64_is_rejected() {
        assert_collision(
            "class C:\n    X = 99999999999999999999999\n",
            "does not fit in i64",
        );
    }

    /// The annotated path resolves its type first and then checks the literal
    /// against it, so a well-shaped literal of the wrong type is rejected by
    /// the shared value lowering rather than by annotation resolution.
    #[test]
    fn an_annotated_class_attribute_with_a_mismatched_literal_is_rejected() {
        assert_collision(
            "class C:\n    X: int = \"a\"\n",
            "is annotated `int` but is initialized with a `str` literal",
        );
    }

    // -- #910: every inferred literal shape, at the lowering seam ---------

    #[test]
    fn every_unannotated_literal_shape_infers_its_natural_type() {
        let module = crate::pycc_parser_test_helper::parse(
            "class C:\n    I = 1\n    F = 1.5\n    B = True\n    S = \"cfg\"\n    NI = -1024\n    PI = +2048\n    NF = -1.5\n    NB = False\n",
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
                ("I".to_string(), Ty::Int, ClassAttrValue::Int(1)),
                ("F".to_string(), Ty::Float, ClassAttrValue::Float(1.5)),
                ("B".to_string(), Ty::Bool, ClassAttrValue::Bool(true)),
                (
                    "S".to_string(),
                    Ty::Str,
                    ClassAttrValue::Str("cfg".to_string())
                ),
                ("NI".to_string(), Ty::Int, ClassAttrValue::Int(-1024)),
                ("PI".to_string(), Ty::Int, ClassAttrValue::Int(2048)),
                ("NF".to_string(), Ty::Float, ClassAttrValue::Float(-1.5)),
                ("NB".to_string(), Ty::Bool, ClassAttrValue::Bool(false)),
            ]
        );
        // D-154 / D-224: an inferred class attribute is a compile-time
        // constant, so it takes no instance slot -- the same invariant the
        // annotated spelling carries.
        assert!(class_def.attrs.is_empty());
    }
}
