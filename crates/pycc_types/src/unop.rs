//! Unary-operator result typing.
//!
//! Issue #603 (Part 2 of #573) lowers `-x` and `+x` over non-literal
//! operands; issue #604 (Part 3) adds `not x` and `~x`. This submodule
//! holds [`unary_result_type`], a direct sibling of [`binop`](crate::binop)
//! placed here for the same reason that one was extracted from
//! [`lib.rs`](crate): the repository's source-file decomposition rule
//! (AGENTS.md "Keep source files decomposable").
//!
//! `USub`/`UAdd` share one rule, the *unary projection* of
//! [`numeric_result_type`](crate::binop::numeric_result_type)'s numeric
//! mapper: `bool` and `int` both yield `int` (`-True` is `-1` in Python, so
//! the operand crosses into `int`), `float` yields `float`, and everything
//! else is `T0021`. `str` has no unary form at all -- unlike `str * int`,
//! there is no `-"ab"` -- so it needs no guard clause and falls through to
//! the error arm with every other non-numeric type.
//!
//! `Not` and `Invert` each need their own rule instead of sharing that one:
//!
//! * `not x` is defined by truthiness, not by numeric promotion. Its result
//!   is always `bool`, for every operand type this compiler can actually
//!   compute a truth value for at codegen time --
//!   [`truthy`](../../pycc_codegen/fn.truthy.html) in `pycc_codegen`
//!   handles `bool`, `int`, `float`, `str`, and `Optional`, but panics on
//!   `list`/`dict`/`set` (no `pycc_rt_*_truthy` entry point exists for
//!   them yet). Accepting those container types here would let a `not`
//!   expression reach that panic, so they are rejected with `T0021` instead
//!   -- the same "every type pycc models a truth value for" reading the
//!   issue's own completion criteria use, not a narrowing of them.
//! * `~x` is `int -> int` only (`bool` included, since `pycc_types` treats
//!   it as a numeric subtype of `int`); every other operand is `T0021`.

use pycc_diag::{Diagnostic, Span};
use pycc_hir::{Ty, UnaryOpKind};

/// Types a unary expression from its operator and its operand type.
pub(crate) fn unary_result_type(op: UnaryOpKind, operand: Ty) -> Result<Ty, Diagnostic> {
    match op {
        UnaryOpKind::USub | UnaryOpKind::UAdd => match operand {
            Ty::Bool | Ty::Int => Ok(Ty::Int),
            Ty::Float => Ok(Ty::Float),
            _ => Err(unary_type_error(op, operand)),
        },
        UnaryOpKind::Not => match operand {
            Ty::Bool | Ty::Int | Ty::Float | Ty::Str | Ty::Optional(_) => Ok(Ty::Bool),
            _ => Err(unary_type_error(op, operand)),
        },
        UnaryOpKind::Invert => match operand {
            Ty::Bool | Ty::Int => Ok(Ty::Int),
            _ => Err(unary_type_error(op, operand)),
        },
    }
}

fn unary_type_error(op: UnaryOpKind, operand: Ty) -> Diagnostic {
    Diagnostic::error(
        "T0021",
        format!(
            "unary operator {op:?} is not defined for `{}`",
            operand.name()
        ),
        Span::new(0, 0),
    )
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
