//! The one purity predicate two rules share: whether evaluating an
//! expression early, late or twice can be observed.
//!
//! Keyword binding (#1204, `docs/TYPE_SYSTEM.md` "Keyword argument
//! evaluation order") uses it to decide whether moving a value is safe, and
//! augmented assignment (#1209, "Augmented assignment") uses it to decide
//! whether a subscript index may be read twice by the desugared assignment.
//! Widening it widens both rules at once, so it must stay limited to
//! expressions that can neither raise differently nor have an effect.

use pycc_ast::Expr;

/// Whether evaluating `value` early, late or twice cannot be observed: a
/// literal in the subset a parameter default admits, or a bare name.
pub(crate) fn is_unobservable(value: &Expr) -> bool {
    matches!(value, Expr::Name(_)) || crate::func::params::literal_default(value).is_some()
}
