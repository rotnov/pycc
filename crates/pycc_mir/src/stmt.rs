//! HIR-to-MIR statement lowering (#546): `lower_stmt`, the per-`HirStmt`
//! dispatch that the crate root's `build`/`lower_item` walk drives.

use super::matching::lower_match;
use super::{
    HirClassDef, MirExceptHandler, MirExpr, MirStmt, bind, bind_variable, class_def_of,
    handler_type_tags, lookup, lower_expr, lower_raise, mro_attrs, mro_class_def,
    resolve_comp_source,
};
use pycc_hir::{HirStmt, Ty};
use std::collections::HashMap;

pub(super) fn lower_stmt(
    stmt: &HirStmt,
    scopes: &mut Vec<HashMap<String, Ty>>,
    classes: &HashMap<String, HirClassDef>,
    current_class: Option<&str>,
) -> MirStmt {
    match stmt {
        HirStmt::ExprStmt(expr) => {
            MirStmt::ExprStmt(lower_expr(expr, scopes, classes, current_class))
        }
        HirStmt::Assign { target, value } => {
            let value = lower_expr(value, scopes, classes, current_class);
            // The first assignment fixes a binding's representation.
            // In particular, assigning `bool` to an existing `int` is
            // accepted by the type checker but must not silently change the
            // later MIR name type from tagged i64 to i8.
            bind_variable(scopes, target.clone(), value.ty());
            MirStmt::Assign {
                target: target.clone(),
                value,
            }
        }
        HirStmt::AnnAssign {
            target,
            annotation,
            value: Some(value),
            is_final: _,
        } => {
            let value = lower_expr(value, scopes, classes, current_class);
            // `pycc_types::is_assignable` accepts an annotated initializer
            // in exactly two shapes: an exact type match, or a `bool`
            // initializer under an `int` annotation (`bool` is an `int`
            // subtype -- the only widening `is_assignable` allows). Unlike
            // plain `Assign` (whose bound type and lowered value type are
            // always the same, since both come from `value`), `pycc_types`
            // itself binds its checker `env` to the *annotation's* type for
            // `AnnAssign` (`check_assignment(env, target, *annotation)`,
            // not the initializer's inferred type) specifically so a later
            // annotated re-declaration is checked consistently -- see its
            // own comment citing this exact invariant. D-074's "first
            // assignment fixes a binding's representation" rule then
            // requires this lowering to agree, or a later plain
            // reassignment (`x: int = True; x = 5`) would silently widen
            // into a slot still permanently sized for `bool` (confirmed
            // empirically before this fix: the program above printed `11`,
            // the raw tagged-int bit pattern truncated through an `i8`
            // slot, instead of `5`). Keep the static `int` slot without
            // manufacturing arithmetic: `True + 0` is the integer `1`, but
            // an annotated boundary must retain the runtime identity
            // `True`.
            let value = if value.ty() == *annotation {
                value
            } else if value.ty() == Ty::Bool && *annotation == Ty::Int {
                // `bool` initializer under an `int` annotation — widen
                // to `int` via `IntBoundary` (D-141), preserving the
                // runtime `bool` identity while reporting `Ty::Int`.
                MirExpr::IntBoundary(Box::new(value))
            } else if let Ty::Optional(inner) = annotation {
                // `T | None` (PEP 604, D-197, #763, Part 1 of #747): a bare
                // `None` initializer, or a bare `inner`-typed (or
                // `inner`-assignable, e.g. `bool` under `Optional[int]`)
                // initializer, under an `Optional[inner]` annotation widens
                // via `OptionalWrap` -- mirroring `IntBoundary`'s identical
                // "fix `.ty()` so `collect_stmt_bindings` derives the right
                // slot representation" reason immediately above; see
                // `OptionalWrap`'s own doc comment for why a wrapper node
                // is needed here but not for a later plain reassignment.
                MirExpr::OptionalWrap(Box::new(value), inner.clone())
            } else {
                value
            };
            // #380 (PR-20): when the annotation is a protocol type, bind
            // with the value's concrete type instead — the protocol type
            // is a compile-time-only interface, and the MIR needs the
            // concrete type for method/attribute resolution (static
            // dispatch). `pycc_types` already validated conformance and
            // binds the concrete type in its own environment; the MIR
            // must agree.
            let bind_ty = if matches!(annotation, Ty::Protocol(_)) {
                value.ty()
            } else {
                annotation.clone()
            };
            bind_variable(scopes, target.clone(), bind_ty);
            MirStmt::Assign {
                target: target.clone(),
                value,
            }
        }
        HirStmt::AnnAssign { value: None, .. } => MirStmt::NoOp,
        HirStmt::If { test, body, orelse } => MirStmt::If {
            test: lower_expr(test, scopes, classes, current_class),
            body: body
                .iter()
                .map(|s| lower_stmt(s, scopes, classes, current_class))
                .collect(),
            orelse: orelse
                .iter()
                .map(|s| lower_stmt(s, scopes, classes, current_class))
                .collect(),
        },
        HirStmt::While { test, body } => MirStmt::While {
            test: lower_expr(test, scopes, classes, current_class),
            body: body
                .iter()
                .map(|s| lower_stmt(s, scopes, classes, current_class))
                .collect(),
        },
        HirStmt::ForRange {
            var,
            start,
            stop,
            step,
            body,
        } => {
            let start = lower_expr(start, scopes, classes, current_class);
            let stop = lower_expr(stop, scopes, classes, current_class);
            let step = lower_expr(step, scopes, classes, current_class);
            bind_variable(scopes, var.clone(), Ty::Int);
            let body = body
                .iter()
                .map(|s| lower_stmt(s, scopes, classes, current_class))
                .collect();
            MirStmt::ForRange {
                var: var.clone(),
                start,
                stop,
                step,
                body,
            }
        }
        HirStmt::ForList { var, list, body } => {
            // The loop variable's type is `list`'s element (or, for a
            // dict-typed binding, key) type, derived via the same `lookup`
            // mechanism every other name reference in this crate uses --
            // mirroring `pycc_types::check_stmt`'s own `ForList` arm
            // (`check_assignment(env, var, *elem_ty)` / `check_assignment(env,
            // var, kv.0)` / `check_assignment(env, var, *elem_ty)` for the
            // set case), not hardcoded to `Ty::Int`. Empirically only a
            // `Ty::List(Box::new(Ty::Int))`, `Ty::Dict(Box::new((Ty::Str,
            // Ty::Int)))`, or `Ty::Set(Box::new(Ty::Int))` binding ever
            // reaches this arm today (`pycc_types`' T0034/T0036/T0037/T0038
            // gates reject every other element/key-value combination before
            // HIR ever constructs one -- see those gates' own comments and
            // this crate's own genericity tests), but deriving here keeps
            // this lowering correct on its own terms rather than baking in
            // an assumption this crate has no way to verify independently.
            // `HirStmt::ForList` is reused unconditionally by `pycc_hir`'s
            // own lowering for any bare-name iterable, dict, list, or set
            // alike (it has no type information to pick a different node)
            // -- this is the point where the real type is resolved and
            // where a dict- or set-typed binding is routed into
            // `MirStmt::ForDict`/`MirStmt::ForSet` instead of
            // `MirStmt::ForList`, mirroring `lower_expr`'s own
            // `HirExpr::Subscript` arm doing the same list/dict routing for
            // reads (subscripting a set is rejected earlier, by
            // `pycc_types`' own T0033, so there is no set counterpart there).
            match lookup(scopes, list) {
                Ty::List(elem_ty) => {
                    bind_variable(scopes, var.clone(), *elem_ty);
                    let body = body
                        .iter()
                        .map(|s| lower_stmt(s, scopes, classes, current_class))
                        .collect();
                    MirStmt::ForList {
                        var: var.clone(),
                        list: list.clone(),
                        body,
                    }
                }
                Ty::Dict(kv) => {
                    bind_variable(scopes, var.clone(), kv.0);
                    let body = body
                        .iter()
                        .map(|s| lower_stmt(s, scopes, classes, current_class))
                        .collect();
                    MirStmt::ForDict {
                        var: var.clone(),
                        dict: list.clone(),
                        body,
                    }
                }
                // `for x in s:` (PR-11 Task 8, D-123): iterates a set's own
                // elements, binding the loop variable as the set's element
                // type -- mirrors the `Ty::Dict` arm immediately above,
                // which mirrors `pycc_types::check_stmt`'s own identical
                // `Ty::Set(elem_ty) => *elem_ty` arm (added in that crate's
                // Task 7 fix round).
                Ty::Set(elem_ty) => {
                    bind_variable(scopes, var.clone(), *elem_ty);
                    let body = body
                        .iter()
                        .map(|s| lower_stmt(s, scopes, classes, current_class))
                        .collect();
                    MirStmt::ForSet {
                        var: var.clone(),
                        set: list.clone(),
                        body,
                    }
                }
                other => panic!(
                    "pycc_mir: internal error: `{list}` is neither a list, dict, nor set (found `{}`) -- pycc_types::check should have rejected this HIR before it reached pycc_mir",
                    other.name()
                ),
            }
        }
        HirStmt::ListCompAssign {
            target,
            var,
            iter,
            cond,
            elt,
        } => {
            let (source, var_ty) = resolve_comp_source(iter, var, scopes, classes, current_class);
            let cond = cond
                .as_deref()
                .map(|c| lower_expr(c, scopes, classes, current_class));
            let elt = lower_expr(elt, scopes, classes, current_class);
            bind_variable(scopes, target.clone(), Ty::List(Box::new(elt.ty())));
            MirStmt::ListCompAssign {
                target: target.clone(),
                var: var.clone(),
                var_ty,
                source,
                cond: cond.map(Box::new),
                elt: Box::new(elt),
            }
        }
        HirStmt::SetCompAssign {
            target,
            var,
            iter,
            cond,
            elt,
        } => {
            let (source, var_ty) = resolve_comp_source(iter, var, scopes, classes, current_class);
            let cond = cond
                .as_deref()
                .map(|c| lower_expr(c, scopes, classes, current_class));
            let elt = lower_expr(elt, scopes, classes, current_class);
            bind_variable(scopes, target.clone(), Ty::Set(Box::new(elt.ty())));
            MirStmt::SetCompAssign {
                target: target.clone(),
                var: var.clone(),
                var_ty,
                source,
                cond: cond.map(Box::new),
                elt: Box::new(elt),
            }
        }
        HirStmt::DictCompAssign {
            target,
            var,
            iter,
            cond,
            key,
            value,
        } => {
            let (source, var_ty) = resolve_comp_source(iter, var, scopes, classes, current_class);
            let cond = cond
                .as_deref()
                .map(|c| lower_expr(c, scopes, classes, current_class));
            let key = lower_expr(key, scopes, classes, current_class);
            let value = lower_expr(value, scopes, classes, current_class);
            bind_variable(
                scopes,
                target.clone(),
                Ty::Dict(Box::new((key.ty(), value.ty()))),
            );
            MirStmt::DictCompAssign {
                target: target.clone(),
                var: var.clone(),
                var_ty,
                source,
                cond: cond.map(Box::new),
                key: Box::new(key),
                value: Box::new(value),
            }
        }
        HirStmt::Return(value) => MirStmt::Return(
            value
                .as_ref()
                .map(|v| lower_expr(v, scopes, classes, current_class)),
        ),
        HirStmt::DictSet { dict, key, value } => MirStmt::DictSet {
            dict: dict.clone(),
            key: lower_expr(key, scopes, classes, current_class),
            value: lower_expr(value, scopes, classes, current_class),
        },
        // D-154 (Part 1 of #375): `base.attr = value`, resolved to a
        // compile-time slot index exactly like `MirExpr::AttrGet` above.
        // #377: if `attr` is a `@property` with a setter, the assignment is
        // rewritten to an ordinary `MirStmt::ExprStmt(MirExpr::Call)` to
        // the setter's mangled name (with `base` as `self` and `value` as
        // the setter's parameter), reusing the existing method-call/codegen
        // infrastructure with no new MIR/codegen variant. A read-only
        // property (no setter) never reaches here -- `pycc_types::check`
        // rejects it with `T0044` before MIR lowering runs.
        HirStmt::AttrSet { base, attr, value } => {
            let base = lower_expr(base, scopes, classes, current_class);
            let value = lower_expr(value, scopes, classes, current_class);
            let class_def = class_def_of(&base, classes);
            // #432: walk the MRO for property lookup first (matching
            // `AttrGet`'s own MRO walk), then for regular attribute slots
            // using the flat MRO layout.
            for mro_class in &class_def.mro {
                let mro_def = mro_class_def(mro_class, classes);
                if let Some(prop) = mro_def.properties.iter().find(|p| p.name == *attr) {
                    let setter = prop.setter.as_ref().unwrap_or_else(|| {
                        panic!(
                            "pycc_mir: internal error: property `{attr}` on class `{mro_class}` \
                             has no setter -- pycc_types::check should have rejected this \
                             assignment before it reached pycc_mir"
                        )
                    });
                    let ty = lookup(scopes, &format!("$fn:{setter}"));
                    return MirStmt::ExprStmt(MirExpr::Call {
                        callee: setter.clone(),
                        args: vec![base, value],
                        ty,
                    });
                }
            }
            let flat_attrs = mro_attrs(class_def, classes);
            let (slot, (_, slot_ty)) = flat_attrs
                .iter()
                .enumerate()
                .find(|(_, (name, _))| name == attr)
                .unwrap_or_else(|| {
                    panic!(
                        "pycc_mir: internal error: attribute `{attr}` not declared on class `{}` \
                         or any base in its MRO -- pycc_types::check should have rejected this \
                         HIR before it reached pycc_mir",
                        class_def.name
                    )
                });
            // #627: `obj.attr = <bool>` where `attr` is declared `int`.
            // `pycc_types::is_assignable` accepts this -- `int` admits
            // `bool` as a subtype at a checked boundary
            // (`docs/TYPE_SYSTEM.md`) -- and an attribute store is such a
            // boundary, exactly like the `AnnAssign` arm above. The slot
            // holds D-141-encoded `int` words, so an unencoded `bool`
            // word lands there as a raw `1`/`0`: `1` reads back as the
            // smallint `0`, and `0` is not a valid encoded word at all,
            // aborting the next read with `pycc_rt: invalid encoded int
            // word 0x0`. Widen through `MirExpr::IntBoundary`, the
            // mechanism D-141 mandates ("MIR represents an annotated
            // initializer boundary with `MirExpr::IntBoundary`, not
            // synthetic arithmetic"), which preserves the runtime `True`/
            // `False` identity CPython prints. The declared type comes
            // from the same `flat_attrs` tuple the slot index did, never
            // from a fresh lookup on the class's own `attrs`, because an
            // MRO re-declaration can make the two differ (#432).
            //
            // Reporting `Ty::Int` for the stored value additionally makes
            // codegen's `MirStmt::AttrSet` release gate fire, so a bigint
            // already in the slot is released instead of leaked -- D-180
            // Consequences item 6, corrected by D-187.
            let value = if *slot_ty == Ty::Int && value.ty() == Ty::Bool {
                MirExpr::IntBoundary(Box::new(value))
            } else {
                value
            };
            MirStmt::AttrSet { base, slot, value }
        }
        HirStmt::Match { subject, cases } => {
            lower_match(subject, cases, scopes, classes, current_class)
        }
        HirStmt::Try {
            body,
            handlers,
            orelse,
            finalbody,
        } => {
            let body = body
                .iter()
                .map(|s| lower_stmt(s, scopes, classes, current_class))
                .collect();
            let handlers = handlers
                .iter()
                .map(|h| {
                    // PEP 758 (#740): a handler may name more than one
                    // exception type. Union each named type's own tag set,
                    // then dedup -- overlapping families (e.g. `OSError`
                    // and `ConnectionError` both include tags 10, 19-22)
                    // would otherwise double-count.
                    let exc_type_tag = h.exc_type.as_ref().map(|names| {
                        let mut tags: Vec<u8> = names
                            .iter()
                            .flat_map(|name| handler_type_tags(name, classes))
                            .collect();
                        tags.sort_unstable();
                        tags.dedup();
                        tags
                    });
                    let binding_type = h
                        .exc_type
                        .as_ref()
                        .map(|names| pycc_hir::except_handler_binding_type_name(names));
                    if let (Some(binding_type), Some(name)) = (&binding_type, &h.name) {
                        // The type checker binds `except T as name` only in
                        // the handler's cloned environment. MIR maintains
                        // its own type scopes, so record the same binding
                        // before lowering expressions in the handler body.
                        // A bare handler cannot have an `as` name in Python.
                        bind(
                            scopes,
                            name.clone(),
                            Ty::Instance(Box::new(binding_type.clone())),
                        );
                    }
                    let handler_body = h
                        .body
                        .iter()
                        .map(|s| lower_stmt(s, scopes, classes, current_class))
                        .collect();
                    MirExceptHandler {
                        exc_type_tag,
                        binding_name: h.name.clone(),
                        binding_ty: h
                            .name
                            .as_ref()
                            .zip(binding_type.as_ref())
                            .map(|(_, ty)| Ty::Instance(Box::new(ty.clone()))),
                        body: handler_body,
                    }
                })
                .collect();
            let orelse = orelse
                .iter()
                .map(|s| lower_stmt(s, scopes, classes, current_class))
                .collect();
            let finalbody = finalbody
                .iter()
                .map(|s| lower_stmt(s, scopes, classes, current_class))
                .collect();
            MirStmt::Try {
                body,
                handlers,
                orelse,
                finalbody,
            }
        }
        HirStmt::Raise { exc, cause } => lower_raise(exc, cause, scopes, classes, current_class),
    }
}
