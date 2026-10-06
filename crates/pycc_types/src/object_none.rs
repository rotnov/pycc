//! `None` into an `object` return slot (#1387, D-258's return-position
//! amendment).
//!
//! D-258 makes `object` (and, in an `--ext` module, `Any`) the opaque
//! CPython top type, and `None` is a CPython object -- but `None` stays a
//! native `Ty::None` value inside a compiled body, and `crate::is_assignable`
//! deliberately keeps it out of every `object` slot (an annotated
//! assignment, a call argument, an attribute), because no conversion exists
//! at those seams yet.
//!
//! The return position is the one seam this narrows, and only for the two
//! spellings whose value is CPython's `None` by construction: a bare
//! `return` and `return None` (the literal). A `None`-typed *expression*
//! such as `return g()` with `g -> None` is not admitted: it would need a
//! conversion of an arbitrary native value, which is exactly what the
//! deferred seams are waiting on. Falling off the end of an `object`
//! function also stays `T0022` -- an implicit `None` is not written down
//! anywhere and `crate::return_coverage` keeps its existing contract.
//!
//! Both walkers -- `crate::check_stmt_in_function` and the solver's
//! `HirStmt::Return` arm -- call [`admits_none_return`], so the two can
//! never disagree about the admitted set. Codegen's counterpart is
//! `pycc_codegen`'s `object_return` module.

use pycc_hir::HirExpr;
use pycc_hir::Ty;

/// Whether `return <value>` (`value == None` for a bare `return`) in a
/// function declared to return `declared` returns CPython's `None` into an
/// `object` slot -- and so is admitted without the assignability check that
/// would otherwise report `T0022`.
pub(crate) fn admits_none_return(declared: &Ty, value: Option<&HirExpr>) -> bool {
    *declared == Ty::Object && matches!(value, None | Some(HirExpr::NoneLiteral))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admits_only_bare_and_literal_none_into_object() {
        assert!(admits_none_return(&Ty::Object, None));
        assert!(admits_none_return(&Ty::Object, Some(&HirExpr::NoneLiteral)));
        assert!(!admits_none_return(
            &Ty::Object,
            Some(&HirExpr::IntLiteral(0))
        ));
        assert!(!admits_none_return(&Ty::Int, None));
        assert!(!admits_none_return(&Ty::Int, Some(&HirExpr::NoneLiteral)));
    }
}
