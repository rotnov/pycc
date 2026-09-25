//! #1331 (Part 1 of #1327): the `hash(...)` builtin call.
//!
//! `hash(x)` is admitted for an `int`, a `bool`, and a tuple whose elements
//! are all `int`/`bool`, and always yields `int`. A `list`, `dict` or `set`
//! argument is `T0021` "unhashable type", the `TypeError` CPython raises for
//! it reported statically (the `len(5)` precedent). Every other argument --
//! a user-class instance (#1332), `str`, `float`, a float-element tuple,
//! `None`, `frozenset[int]` -- is hashable in CPython but not yet here, so
//! it is an honest `C0001`.
//!
//! The call is checked on both of this crate's paths: the public-body path
//! (`crate::expr::infer_expr_in`) and the constraint path for unannotated
//! private helpers (`crate::constraints`). Both place the arm *after* every
//! user-binding lookup, beside `frozenset`'s -- a program's own `def hash`
//! or `class hash` keeps its meaning -- and both also yield to a stdlib
//! module alias spelled `hash` (`import math as hash`), which no user table
//! records. Both call into this module, so the two paths cannot drift the
//! way `len`'s once did (#1098).
//!
//! In its own module because `expr.rs`, `constraints.rs` and `lib.rs` are all
//! past AGENTS.md's ~1,000-line decomposition threshold.

use pycc_diag::{Diagnostic, Span};
use pycc_hir::Ty;

/// The builtin's spelling.
pub(crate) const HASH: &str = "hash";

/// `T0021` for a call with other than exactly one argument.
fn wrong_arity(count: usize) -> Diagnostic {
    Diagnostic::error(
        "T0021",
        format!("`hash` expects exactly 1 argument, got {count}"),
        Span::new(0, 0),
    )
    .with_help("pass exactly 1 argument")
}

/// Whether `ty` is a type `hash()` compiles today.
fn is_admitted(ty: &Ty) -> bool {
    match ty {
        Ty::Int | Ty::Bool => true,
        Ty::Tuple(elements) => elements.iter().all(|e| matches!(e, Ty::Int | Ty::Bool)),
        _ => false,
    }
}

/// The refusal for an argument whose type is not admitted.
fn refusal(ty: &Ty) -> Diagnostic {
    if matches!(ty, Ty::List(_) | Ty::Dict(_) | Ty::Set(_)) {
        return Diagnostic::error(
            "T0021",
            format!("unhashable type: `{}`", ty.name()),
            Span::new(0, 0),
        )
        .with_help("CPython raises `TypeError` here; hash an `int`, `bool` or tuple instead");
    }
    Diagnostic::error(
        "C0001",
        format!(
            "`hash()` of `{}` is valid Python but not implemented yet",
            ty.name()
        ),
        Span::new(0, 0),
    )
    .with_help("`hash()` currently accepts an `int`, a `bool`, or a tuple of them")
}

/// Checks a `hash(...)` call whose argument types are all known (the
/// public-body path) and returns `int`.
pub(crate) fn check_call(arg_tys: &[Ty]) -> Result<Ty, Diagnostic> {
    match arg_tys {
        [arg] if is_admitted(arg) => Ok(Ty::Int),
        [arg] => Err(refusal(arg)),
        _ => Err(wrong_arity(arg_tys.len())),
    }
}

/// The constraint-path twin of [`check_call`]. An argument whose term is
/// unresolved, or still mentions an inference placeholder, is admitted,
/// because the result never depends on it: the final check pass
/// (`check_call` through `infer_expr_in`) validates it once it is known --
/// the lenient-until-known pattern `float` and `frozenset` already use.
pub(crate) fn check_call_terms<V>(arg_terms: &[Option<Result<Ty, V>>]) -> Result<Ty, Diagnostic> {
    match arg_terms {
        [Some(Ok(arg))] if !crate::empty_container::contains_infer(arg) => {
            check_call(std::slice::from_ref(arg))
        }
        [_] => Ok(Ty::Int),
        _ => Err(wrong_arity(arg_terms.len())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tuple(elements: Vec<Ty>) -> Ty {
        Ty::Tuple(Box::new(elements))
    }

    #[test]
    fn int_bool_and_int_bool_tuples_yield_int() {
        for arg in [
            Ty::Int,
            Ty::Bool,
            tuple(vec![Ty::Int]),
            tuple(vec![Ty::Bool, Ty::Int, Ty::Bool]),
        ] {
            assert_eq!(check_call(&[arg]).unwrap(), Ty::Int);
        }
    }

    #[test]
    fn a_container_is_t0021_unhashable() {
        for arg in [
            Ty::List(Box::new(Ty::Int)),
            Ty::Dict(Box::new((Ty::Int, Ty::Int))),
            Ty::Set(Box::new(Ty::Int)),
        ] {
            let err = check_call(std::slice::from_ref(&arg)).unwrap_err();
            assert_eq!(err.code, "T0021");
            assert_eq!(err.message, format!("unhashable type: `{}`", arg.name()));
        }
    }

    #[test]
    fn a_hashable_but_unimplemented_argument_is_c0001() {
        for arg in [
            Ty::Str,
            Ty::Float,
            Ty::None,
            tuple(vec![Ty::Float, Ty::Int]),
            Ty::FrozenSet(Box::new(Ty::Int)),
            Ty::Instance(Box::new("R".to_string())),
        ] {
            let err = check_call(std::slice::from_ref(&arg)).unwrap_err();
            assert_eq!(err.code, "C0001");
            assert_eq!(
                err.message,
                format!(
                    "`hash()` of `{}` is valid Python but not implemented yet",
                    arg.name()
                )
            );
        }
    }

    #[test]
    fn any_other_arity_is_t0021() {
        let err = check_call(&[]).unwrap_err();
        assert_eq!(err.message, "`hash` expects exactly 1 argument, got 0");
        let err = check_call(&[Ty::Int, Ty::Int]).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(err.message, "`hash` expects exactly 1 argument, got 2");
    }

    #[test]
    fn the_constraint_twin_is_lenient_only_until_the_type_is_known() {
        assert_eq!(check_call_terms::<usize>(&[None]).unwrap(), Ty::Int);
        assert_eq!(check_call_terms::<usize>(&[Some(Err(3))]).unwrap(), Ty::Int);
        assert_eq!(
            check_call_terms::<usize>(&[Some(Ok(tuple(vec![Ty::Int, Ty::Infer])))]).unwrap(),
            Ty::Int
        );
        assert_eq!(
            check_call_terms::<usize>(&[Some(Ok(Ty::Bool))]).unwrap(),
            Ty::Int
        );
        let err = check_call_terms::<usize>(&[Some(Ok(Ty::Str))]).unwrap_err();
        assert_eq!(err.code, "C0001");
        let empty: [Option<Result<Ty, usize>>; 0] = [];
        let err = check_call_terms(&empty).unwrap_err();
        assert_eq!(err.message, "`hash` expects exactly 1 argument, got 0");
    }
}
