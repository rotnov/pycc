//! Unary-operator result typing.
//!
//! Issue #603 (Part 2 of #573) lowers `-x` and `+x` over non-literal
//! operands. This submodule holds [`unary_result_type`], a direct sibling
//! of [`binop`](crate::binop) placed here for the same reason that one was
//! extracted from [`lib.rs`](crate): the repository's source-file
//! decomposition rule (AGENTS.md "Keep source files decomposable").
//!
//! The rules are deliberately the *unary projection* of
//! [`numeric_result_type`](crate::binop::numeric_result_type)'s numeric
//! mapper, not an independent policy: `bool` and `int` both yield `int`
//! (`-True` is `-1` in Python, so the operand crosses into `int`), `float`
//! yields `float`, and everything else is `T0021`. `str` has no unary
//! form at all -- unlike `str * int`, there is no `-"ab"` -- so it needs
//! no guard clause and falls through to the error arm with every other
//! non-numeric type.

use pycc_diag::{Diagnostic, Span};
use pycc_hir::{Ty, UnaryOpKind};

/// Types a unary expression from its operator and its operand type.
///
/// `USub` and `UAdd` share this typing exactly: `+x` is not the identity
/// on `bool`, so it promotes to `int` just as `-x` does, and neither
/// operator narrows a `float`.
pub(crate) fn unary_result_type(op: UnaryOpKind, operand: Ty) -> Result<Ty, Diagnostic> {
    match operand {
        Ty::Bool | Ty::Int => Ok(Ty::Int),
        Ty::Float => Ok(Ty::Float),
        _ => Err(Diagnostic::error(
            "T0021",
            format!(
                "unary operator {op:?} is not defined for `{}`",
                operand.name()
            ),
            Span::new(0, 0),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negating_an_int_stays_int() {
        assert_eq!(unary_result_type(UnaryOpKind::USub, Ty::Int), Ok(Ty::Int));
    }

    #[test]
    fn unary_plus_on_an_int_stays_int() {
        assert_eq!(unary_result_type(UnaryOpKind::UAdd, Ty::Int), Ok(Ty::Int));
    }

    #[test]
    fn negating_a_bool_promotes_to_int() {
        // `-True == -1` in Python: the operand crosses into `int` here.
        assert_eq!(unary_result_type(UnaryOpKind::USub, Ty::Bool), Ok(Ty::Int));
    }

    #[test]
    fn unary_plus_on_a_bool_promotes_to_int() {
        assert_eq!(unary_result_type(UnaryOpKind::UAdd, Ty::Bool), Ok(Ty::Int));
    }

    #[test]
    fn negating_a_float_stays_float() {
        assert_eq!(
            unary_result_type(UnaryOpKind::USub, Ty::Float),
            Ok(Ty::Float)
        );
    }

    #[test]
    fn unary_plus_on_a_float_stays_float() {
        assert_eq!(
            unary_result_type(UnaryOpKind::UAdd, Ty::Float),
            Ok(Ty::Float)
        );
    }

    #[test]
    fn negating_a_str_is_rejected() {
        // There is no `-"ab"` in Python, so `str` gets no repetition-style
        // guard clause the way `numeric_result_type` gives `str * int`.
        let err = unary_result_type(UnaryOpKind::USub, Ty::Str).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("USub") && err.message.contains("str"));
    }

    #[test]
    fn unary_plus_on_none_is_rejected() {
        let err = unary_result_type(UnaryOpKind::UAdd, Ty::None).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("UAdd") && err.message.contains("None"));
    }
}
