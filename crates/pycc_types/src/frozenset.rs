//! Part 1 of #1319: the `frozenset(...)` builtin call and the
//! string-conversion refusal shared by `set[T]` and `frozenset[T]`.
//!
//! `frozenset[int]` is `set[int]`'s immutable sibling (`Ty::FrozenSet`). It
//! shares `set[int]`'s run-time representation, so immutability exists only
//! here, in the type checker: `.add()` on a frozenset is refused by
//! `HirExpr::SetAdd`'s own `let Ty::Set(..) else` arm, and a `set[int]` is not
//! assignable to `frozenset[int]` (or back) because `is_assignable` is
//! structural. The only conversion is an explicit `frozenset(x)`.
//!
//! The call is checked on both of this crate's paths: the public-body path
//! (`crate::expr::infer_expr_in`) and the constraint path for unannotated
//! private helpers (`crate::constraints`). Both place the arm *after* every
//! user-binding lookup -- a program's own `def frozenset` or `class
//! frozenset` keeps its meaning -- and both call into this module, so the
//! two paths cannot drift the way `len`'s once did (#1098).
//!
//! In its own module because `expr.rs`, `constraints.rs` and `lib.rs` are all
//! past AGENTS.md's ~1,000-line decomposition threshold.

use pycc_diag::{Diagnostic, Span};
use pycc_hir::Ty;

/// The builtin's spelling.
pub(crate) const FROZENSET: &str = "frozenset";

/// The one compiled frozenset type, `frozenset[int]` (D-122).
pub(crate) fn frozenset_int() -> Ty {
    Ty::FrozenSet(Box::new(Ty::Int))
}

/// Whether `ty` is a value `frozenset(...)` can copy its elements from: a
/// `set[int]`, a `frozenset[int]` or a `list[int]`.
fn is_admitted_source(ty: &Ty) -> bool {
    matches!(ty, Ty::Set(element) | Ty::FrozenSet(element) | Ty::List(element) if **element == Ty::Int)
}

/// `T0021` for a call with more than one argument.
fn wrong_arity(count: usize) -> Diagnostic {
    Diagnostic::error(
        "T0021",
        format!("`frozenset` expects at most 1 argument, got {count}"),
        Span::new(0, 0),
    )
    .with_help("pass no argument, or one `set[int]`, `frozenset[int]` or `list[int]` value")
}

/// `T0021` for an argument whose type is not an admitted source.
fn wrong_source(ty: &Ty) -> Diagnostic {
    Diagnostic::error(
        "T0021",
        format!(
            "`frozenset()` argument must be a `set[int]`, `frozenset[int]` or `list[int]` here, got `{}`",
            ty.name()
        ),
        Span::new(0, 0),
    )
    .with_help("pass a `set[int]`, `frozenset[int]` or `list[int]` value")
}

/// Checks a `frozenset(...)` call whose argument types are all known (the
/// public-body path) and returns `frozenset[int]`.
pub(crate) fn check_call(arg_tys: &[Ty]) -> Result<Ty, Diagnostic> {
    match arg_tys {
        [] => Ok(frozenset_int()),
        [source] if is_admitted_source(source) => Ok(frozenset_int()),
        [source] => Err(wrong_source(source)),
        _ => Err(wrong_arity(arg_tys.len())),
    }
}

/// The constraint-path twin of [`check_call`]. An argument whose term is
/// still unresolved is admitted, because the result never depends on it:
/// the final check pass (`check_call` through `infer_expr_in`) validates it
/// once it is known -- the lenient-until-known pattern `float` already uses.
pub(crate) fn check_call_terms<V>(arg_terms: &[Option<Result<Ty, V>>]) -> Result<Ty, Diagnostic> {
    match arg_terms {
        [] => Ok(frozenset_int()),
        [Some(Ok(source))] if !is_admitted_source(source) => Err(wrong_source(source)),
        [_] => Ok(frozenset_int()),
        _ => Err(wrong_arity(arg_terms.len())),
    }
}

/// `C0001` for handing a `set[T]` or `frozenset[T]` value to a string
/// conversion (`print`, an f-string interpolation). Refused rather than
/// rendered: pycc keeps a set's insertion order (D-123), so it cannot
/// promise CPython's element order, and before this refusal `print({1})`
/// passed `pycc check` and then panicked in `pycc_codegen`'s `to_str`.
pub(crate) fn unrenderable_set(ty: &Ty) -> Diagnostic {
    Diagnostic::error(
        "C0001",
        format!(
            "printing or formatting a `{}` is not supported yet",
            ty.name()
        ),
        Span::new(0, 0),
    )
    .with_help("print an order-insensitive fact instead, such as `len(...)`")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set() -> Ty {
        Ty::Set(Box::new(Ty::Int))
    }

    #[test]
    fn every_admitted_source_and_the_empty_call_yield_frozenset_int() {
        assert_eq!(check_call(&[]).unwrap(), frozenset_int());
        for source in [set(), frozenset_int(), Ty::List(Box::new(Ty::Int))] {
            assert_eq!(check_call(&[source]).unwrap(), frozenset_int());
        }
    }

    #[test]
    fn a_non_int_or_non_container_source_is_t0021() {
        for source in [Ty::Int, Ty::Str, Ty::List(Box::new(Ty::Infer))] {
            let err = check_call(std::slice::from_ref(&source)).unwrap_err();
            assert_eq!(err.code, "T0021");
            assert!(err.message.contains(&format!("got `{}`", source.name())));
        }
    }

    #[test]
    fn two_arguments_are_t0021() {
        let err = check_call(&[set(), set()]).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(err.message, "`frozenset` expects at most 1 argument, got 2");
    }

    #[test]
    fn the_constraint_twin_is_lenient_only_for_an_unresolved_term() {
        let empty: [Option<Result<Ty, usize>>; 0] = [];
        assert_eq!(check_call_terms(&empty).unwrap(), frozenset_int());
        assert_eq!(
            check_call_terms::<usize>(&[Some(Err(3))]).unwrap(),
            frozenset_int()
        );
        assert_eq!(check_call_terms::<usize>(&[None]).unwrap(), frozenset_int());
        assert_eq!(
            check_call_terms::<usize>(&[Some(Ok(set()))]).unwrap(),
            frozenset_int()
        );
        let err = check_call_terms::<usize>(&[Some(Ok(Ty::Int))]).unwrap_err();
        assert_eq!(err.code, "T0021");
        let err = check_call_terms::<usize>(&[None, None]).unwrap_err();
        assert_eq!(err.message, "`frozenset` expects at most 1 argument, got 2");
    }

    #[test]
    fn the_string_conversion_refusal_names_the_type() {
        let err = unrenderable_set(&frozenset_int());
        assert_eq!(err.code, "C0001");
        assert_eq!(
            err.message,
            "printing or formatting a `frozenset[int]` is not supported yet"
        );
    }
}
