//! Shared element-type gates for pycc's container types (D-228, issue #918).
//!
//! Before #918 the only way a `Ty::List`/`Ty::Dict`/`Ty::Set`/`Ty::Tuple`
//! could come into existence was by inferring a container *literal*'s type in
//! `pycc_types`, so the four codegen-capability gates (`T0034`, `T0036`,
//! `T0038`, `T0039`) lived there, next to the inference that produced the
//! type. Lowering a written `list[int]`-style annotation (D-228) creates the
//! same types in `pycc_hir`, which sits *below* `pycc_types` in the crate
//! graph and so cannot call into it. The gates therefore move down here and
//! `pycc_types` calls in, keeping a single definition of "which container
//! types does this version actually compile" for both producers.
//!
//! Seven further copies of the same *capability* rule deliberately stay where
//! they are rather than routing through here, because each states the rule in
//! terms of its own position and routing it would change user-visible text:
//! the slice-position `T0034` in `pycc_types::expr` ("cannot slice
//! `list[...]`") and the six comprehension gates in `pycc_types` (module-scope
//! and function-scope `ListCompAssign`/`SetCompAssign`/`DictCompAssign`, "got
//! a comprehension producing `list[...]`"). Giving this helper a
//! message-variant parameter to absorb them would make it harder to read than
//! the seven three-line checks it replaced, so the shared definition covers
//! the two *type-producing* sites -- container literals and container
//! annotations -- which are the two that must agree.
//!
//! Both entry points take an explicit [`Span`]. The `pycc_types` literal
//! callers pass what they passed before (`Span::new(0, 0)`, rendering as a
//! `1:1` caret) so their diagnostic output is byte-identical to the previous
//! release; annotation lowering passes the annotation's own source range, so
//! the newly reachable annotation diagnostics carry a real caret.

use crate::Ty;
use pycc_diag::{Diagnostic, Span};

/// Rejects one `tuple` element type this version's tuple codegen cannot
/// represent (`T0039`, D-116): a tuple is a fixed SSA struct of scalars, so
/// only `int`, `bool` and `float` elements are compilable.
///
/// This is deliberately *element*-shaped rather than a whole-`Ty` postcheck.
/// `pycc_types`' tuple-literal inference calls it from inside its own
/// element loop, immediately after inferring each element, so the elements
/// are gated in source order: an earlier element's type gate is reported
/// ahead of a later element's own inference failure (an undefined name,
/// say). Folding it into a whole-`Ty` check would silently reorder those
/// diagnostics -- `(1, "a", undefined_name)` would report the undefined
/// name instead of `T0039`.
pub fn check_tuple_element_ty(element: &Ty, span: Span) -> Result<(), Diagnostic> {
    if matches!(element, Ty::Int | Ty::Bool | Ty::Float) {
        return Ok(());
    }
    Err(Diagnostic::error(
        "T0039",
        format!(
            "tuple element type `{}` is not compiled yet (D-116) -- only int/bool/float elements are",
            element.name()
        ),
        span,
    ))
}

/// Rejects a container `Ty` whose element types this version's codegen
/// cannot represent: `list[int]` (`T0034`, D-105), `dict[str, int]`
/// (`T0036`, D-122) and `set[int]` (`T0038`, D-122) are the only admitted
/// shapes, and a `tuple`'s elements are each checked with
/// [`check_tuple_element_ty`].
///
/// A non-container `Ty` is accepted unchanged, so an annotation-lowering
/// caller can route every lowered type through one call without first
/// matching on the shape itself.
pub fn check_container_ty(ty: &Ty, span: Span) -> Result<(), Diagnostic> {
    match ty {
        Ty::List(element) => {
            if **element != Ty::Int {
                return Err(Diagnostic::error(
                    "T0034",
                    format!(
                        "list[{}] is not compiled yet (D-105) -- only list[int] is",
                        element.name()
                    ),
                    span,
                ));
            }
            Ok(())
        }
        Ty::Dict(pair) => {
            if **pair != (Ty::Str, Ty::Int) {
                return Err(Diagnostic::error(
                    "T0036",
                    format!(
                        "{} is not compiled yet (D-122) -- only dict[str, int] is",
                        ty.name()
                    ),
                    span,
                ));
            }
            Ok(())
        }
        Ty::Set(element) => {
            if **element != Ty::Int {
                return Err(Diagnostic::error(
                    "T0038",
                    format!(
                        "{} is not compiled yet (D-122) -- only set[int] is",
                        ty.name()
                    ),
                    span,
                ));
            }
            Ok(())
        }
        Ty::Tuple(elements) => {
            for element in elements.iter() {
                check_tuple_element_ty(element, span)?;
            }
            Ok(())
        }
        Ty::Int
        | Ty::Float
        | Ty::Bool
        | Ty::Str
        | Ty::None
        | Ty::Infer
        | Ty::Param(_)
        | Ty::Instance(_)
        | Ty::Protocol(_)
        | Ty::Optional(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests;
