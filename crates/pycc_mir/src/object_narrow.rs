//! Flow narrowing in MIR lowering: the `Optional` shape of #769 and, since
//! #1476 (Part 3 of #1387), an `object` narrowed back to a native type
//! under `isinstance`.
//!
//! Mirrors `pycc_types::narrow` one layer down. Both layers call the same
//! `pycc_hir` recognizers (`optional_none_test`, `isinstance_test`) and the
//! same class gate (`isinstance_narrow_target`); only the scope lookup is
//! per layer. Where the two walkers' overlays could differ, MIR follows the
//! constraint solver's, whose verdict decides the program (D-220): across
//! a `try`, `stmt::try_stmt` mirrors `pycc_types::constraints::try_stmt`
//! position by position.

use super::{MirExpr, narrowed_ty};
use pycc_hir::{HirClassDef, HirExpr, IsInstancePolarity, NoneTestPolarity, Ty};
use std::collections::HashMap;

/// Which branch of an `if` a recognized test narrows. Only `Orelse` also
/// narrows the continuation after an `if` whose body definitely terminates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NarrowSide {
    Body,
    Orelse,
}

/// A name's recorded type, or `None` when no frame binds it.
fn scoped_ty(scopes: &[HashMap<String, Ty>], name: &str) -> Option<Ty> {
    scopes
        .iter()
        .rev()
        .find_map(|scope| scope.get(name).cloned())
}

/// The `(name, narrowed type, side)` an `if` test narrows, or `None`:
/// `name is [not] None` on an `Optional` name, or `[not] isinstance(name,
/// C)` on an `object` name with the builtin `isinstance` and an unshadowed,
/// admissible `C`.
pub(super) fn narrowing_target(
    test: &HirExpr,
    scopes: &[HashMap<String, Ty>],
    classes: &HashMap<String, HirClassDef>,
) -> Option<(String, Ty, NarrowSide)> {
    if let Some((name, polarity)) = pycc_hir::optional_none_test(test) {
        let Some(Ty::Optional(inner)) = scoped_ty(scopes, name) else {
            return None;
        };
        let side = match polarity {
            NoneTestPolarity::IsNot => NarrowSide::Body,
            NoneTestPolarity::Is => NarrowSide::Orelse,
        };
        return Some((name.to_string(), *inner, side));
    }
    let (name, class, polarity) = pycc_hir::isinstance_test(test)?;
    let shadowed = |key: &str| scopes.iter().any(|scope| scope.contains_key(key));
    if shadowed("$fn:isinstance") || shadowed(class) || scoped_ty(scopes, name) != Some(Ty::Object)
    {
        return None;
    }
    let inner = pycc_hir::isinstance_narrow_target(class, |class| classes.get(class))?;
    let side = match polarity {
        IsInstancePolarity::Positive => NarrowSide::Body,
        IsInstancePolarity::Negated => NarrowSide::Orelse,
    };
    Some((name.to_string(), inner, side))
}

/// The read of a narrowed name: `OptionalUnwrap` over an `Optional` slot,
/// `ObjectUnbox` over an `object` slot.
pub(super) fn narrowed_read(name: &str, slot_ty: Ty, inner: Ty) -> MirExpr {
    let is_object = slot_ty == Ty::Object;
    let slot = Box::new(MirExpr::Name {
        name: name.to_string(),
        ty: slot_ty,
    });
    if is_object {
        MirExpr::ObjectUnbox(slot, Box::new(inner))
    } else {
        MirExpr::OptionalUnwrap(slot, Box::new(inner))
    }
}

/// Whether `value` is a bare read of an `object` name an `isinstance` guard
/// currently narrows -- the value `pycc_types`' first-binding rule keeps at
/// `object` (`check_assignment_boxing`), so a local's first binding from it
/// declares an `object` slot here too.
pub(super) fn is_bare_narrowed_object_read(
    value: &HirExpr,
    scopes: &[HashMap<String, Ty>],
) -> bool {
    matches!(value, HirExpr::Name(name)
        if narrowed_ty(scopes, name).is_some() && scoped_ty(scopes, name) == Some(Ty::Object))
}

/// `value` with a narrowed `object` read replaced by the object itself:
/// the operand an identity test, or a first binding the checker keeps at
/// `object`, must see.
pub(super) fn object_operand(value: MirExpr) -> MirExpr {
    match value {
        MirExpr::ObjectUnbox(object, _) => *object,
        other => other,
    }
}

#[cfg(test)]
mod tests;
