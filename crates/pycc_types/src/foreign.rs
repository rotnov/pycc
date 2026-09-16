//! Foreign (CPython-object) import bindings in the type environment
//! (Part 1 of #1026, PR 1c of #1080).
//!
//! `import numpy` binds `numpy` to [`Ty::Object`], an opaque `PyObject *`
//! whose shape pycc knows nothing about. Every refusal of an operation on
//! such a value is `I0404`.
//!
//! **Part 2 of #1026 (#1081) moved the refusal from the producer to the
//! consumers, and this paragraph replaces the containment invariant Part 1
//! rested on.** Part 1 refused the *read* of a foreign binding, which
//! refused every derived operation at once without a refusal per
//! operation. That works only while no operation is supported. Part 2
//! supports `numpy.pi`, and `infer_expr_in` is context-free -- it cannot
//! see whether the read it is answering feeds an attribute load or a
//! `print` -- so a conditional relaxation of the read is not expressible.
//! The read is therefore admitted *in a module body* and each **consuming**
//! site refuses on its own.
//!
//! Two consequences follow, and both are load-bearing:
//!
//! 1. A `Ty::Object` value no longer has a name. The diagnostic builder
//!    below names the **operation** instead ([`object_operation_unsupported`]),
//!    because the consumer knows what it was about to do while the value
//!    it holds is an anonymous temporary.
//! 2. There are now several producer shapes, not one: `o.attr`
//!    (`HirExpr::AttrGet` over a `Ty::Object` base), a call to an
//!    unannotated private helper whose inferred return is `Ty::Object`
//!    (`constraints.rs`'s `AttrGet` term), `o.method(...)` (PR 2b of
//!    #1081), `o[k]` (PR 3b of #1082), and the loop variable of
//!    `for x in <object>:` (`HirStmt::ForObject`, PR 3c of #1082, which
//!    binds `x` to `Ty::Object` directly rather than through
//!    `check_assignment`). Every refusal must
//!    therefore key on the **type**, never on the producing expression
//!    shape, and the list is expected to keep growing.
//!
//! **PR 2a of #1081 bounded the admitted read by position.** A foreign
//! object may be read only where the compiler can tell the `import` has
//! already run and can report a failed lookup: a module body. Two shapes
//! Part 2 briefly admitted are refused again, both of which had turned a
//! compile error into a run-time trap --
//!
//! * a read inside a *function body*, refused by
//!   [`reject_object_read`] from `expr::infer_expr_in`'s `HirExpr::Name`
//!   arm, because D-041 checks a body against the module environment as it
//!   stands after all top-level code and so cannot see whether the call
//!   site precedes the `import` (and because `pycc_codegen`'s module-exec
//!   failure edge does not exist inside one);
//! * a module-body read placed *above* the `import`, which is now an
//!   ordinary unbound-name `T0021` -- see [`bind_foreign_objects`].
//!
//! `docs/TYPE_SYSTEM.md` carries the user-facing statement of both.
//!
//! **PR 2b of #1081 added the second supported operation: a method call.**
//! `expr::infer_expr_in`'s `HirExpr::MethodCall` arm now answers
//! [`Ty::Object`] for a `Ty::Object` base, where it previously fell through
//! to `class::resolve_method_call`'s "not a class instance" `T0043`. The
//! branch admits only *positional* arguments (`HirExpr::MethodCall` carries
//! no keyword arguments at all) whose types are `int`, `float`, `bool` or
//! `str` -- the four scalars the shim has a `pycc_ext_obj_pack_*` helper
//! for -- and refuses every other argument type with
//! [`object_operation_unsupported`], including a second `Ty::Object`. The
//! call inherits PR 2a's positional bound unchanged: the base is read
//! through the same `HirExpr::Name` arm, so a call inside a function body
//! is still `I0404` and a call above the `import` is still `T0021`.
//!
//! The arm is not reached for four method names. `pycc_hir`'s
//! `CONTAINER_METHOD_NAMES` (`append`, `pop`, `get`, `add`) claims those
//! spellings while lowering, so `gc.get(1, 2)` never becomes a
//! `HirExpr::MethodCall` at all and is refused here through a different
//! consumer. `docs/TYPE_SYSTEM.md`'s `object` row owns that statement,
//! and #1095 tracks routing them to foreign dispatch.
//!
//! **PR 3a of #1082 (Part 3 of #1026) added `len` and truth testing, and
//! deleted a whole class of refusal.** `len(o)` type-checks to `Ty::Int`
//! (`expr.rs`'s and `constraints.rs`'s `len` guards both admit
//! [`Ty::Object`] now), and an `if`/`while` test or a comprehension guard
//! places no constraint on its operand at all -- `pycc_codegen`'s `truthy`
//! grew a `Scalar::Object` arm that calls the shim's `pycc_ext_obj_truthy`,
//! so the ten `reject_object_condition` sites that existed only to keep an
//! object away from a codegen panic are gone. Nothing else about the
//! positional bound changes: both operations read their operand through the
//! same `HirExpr::Name` arm, so both are still `I0404` inside a function
//! body and still `T0021` above the `import`.
//!
//! `not o` is *not* part of this: `unop.rs`'s `Not` arm answers `T0021` for
//! a non-`bool` operand, which it did before PR 3a and still does.
//!
//! **PR 3b of #1082 added another producer shape: a subscript load.**
//! `o[k]` type-checks to [`Ty::Object`] (`expr.rs`'s `HirExpr::Subscript`
//! arm has a `Ty::Object` base arm ahead of the `T0033` catch-all, and
//! `constraints.rs`'s own `Subscript` arm lifts the term exactly as its
//! `AttrGet` arm does), so a key is now a fourth place a foreign value can
//! be consumed. The admitted keys are the same four scalars a method call's
//! arguments are -- `int`, `float`, `bool`, `str`, the ones with a
//! `pycc_ext_obj_pack_*` helper -- and every other key type, including a
//! second [`Ty::Object`], is refused with
//! [`object_operation_unsupported`] naming the key's type.
//!
//! Only the **load** is admitted. `o[k] = v` stays refused: it is a
//! different HIR shape, which `pycc_hir` rejects with `C0001` ("only
//! assigning to a bare-name subscript target") before this crate sees it,
//! and `check_assignment`'s own `reject_object_operand` guard keeps
//! `x = o[k]` an `I0404` besides. A *slice* (`o[a:b]`) is a third shape
//! again and keeps `expr.rs`'s `HirExpr::Slice` `T0033`. The positional
//! bound is inherited unchanged.
//!
//! [`reject_object_read`] serves the three sites that key on a *named*
//! binding rather than on a consumed value:
//!
//! 1. `lookup_bound_name` (D-105's `ForList`/`ListAppend` HIR shape carries
//!    its list as a plain `String`, so it never becomes a `HirExpr::Name`),
//! 2. `expr::infer_expr_in`'s `HirExpr::Call` arm, whose value-binding gate
//!    would otherwise report the generic `non_callable_binding` `T0021`.
//!    Calling a CPython object stays refused in Part 2 deliberately: the
//!    environment does not record whether a `Ty::Object` came from a
//!    foreign global or from an attribute load, so admitting `f(2.0)`
//!    would also admit `numpy(1)`, which CPython itself answers with
//!    `TypeError: 'module' object is not callable`,
//! 3. the in-function read above, which is the one site that keys on
//!    *position* as well as on the type.

use crate::Environment;
use pycc_diag::{Diagnostic, Span};
use pycc_hir::{ImportBinding, Ty};

/// The diagnostic every unsupported operation on a CPython object gets.
///
/// `operation` is a noun phrase naming what the *consumer* was about to
/// do, written so the message reads as a sentence: "printing a CPython
/// object is not supported yet". Part 1 interpolated the binding's local
/// name here instead; Part 2's refusals sit at consuming sites that hold
/// an anonymous temporary and have no name to report (see the module doc).
pub(crate) fn object_operation_unsupported(operation: &str) -> Diagnostic {
    Diagnostic::error(
        "I0404",
        format!(
            "{operation} is not supported yet -- pycc models a CPython object as an opaque \
             value, and #1026 implements attribute access, positional \
             scalar-argument method calls, `len`, truth testing, a \
             scalar-key subscript load and `for` iteration on it and \
             nothing else"
        ),
        Span::new(0, 0),
    )
}

/// `Err(I0404)` when `ty` is the opaque object type, `Ok(())` otherwise.
///
/// The guard for the three sites that key on a *named* binding rather than
/// on a consumed value -- `lookup_bound_name`, the `HirExpr::Call`
/// value-binding gate, and the in-function-body read (module doc). A
/// consuming site calls [`object_operation_unsupported`] directly instead,
/// because it knows the operation and its operand has no name.
pub(crate) fn reject_object_read(name: &str, ty: &Ty) -> Result<(), Diagnostic> {
    if matches!(ty, Ty::Object) {
        return Err(object_operation_unsupported(&format!(
            "using `{name}`, which is bound to a CPython object, in this position"
        )));
    }
    Ok(())
}

/// `Err(I0404)` when `ty` is the opaque object type, naming `operation`.
///
/// The consumer-side counterpart of [`reject_object_read`]: one helper so
/// the three remaining consuming sites -- `check_assignment`
/// (`lib.rs`), `check_match` (`lib.rs`) and `check_isinstance`
/// (`class.rs`) -- cannot drift into three spellings of the same rule.
///
/// Part 3 of #1026 (PR 3a of #1082) removed the largest group of callers:
/// the ten `if`/`while`/comprehension-guard condition sites, which refused
/// `Ty::Object` only because `pycc_codegen`'s `truthy` had no object arm.
/// It has one now, so a condition no longer consults this helper at all.
pub(crate) fn reject_object_operand(ty: &Ty, operation: &str) -> Result<(), Diagnostic> {
    if matches!(ty, Ty::Object) {
        return Err(object_operation_unsupported(operation));
    }
    Ok(())
}

/// The local names a module's import table binds to a CPython object, in
/// source order.
pub(crate) fn foreign_object_names(imports: &[ImportBinding]) -> Vec<&str> {
    imports
        .iter()
        .filter_map(|binding| match binding {
            ImportBinding::Foreign { local_name, .. } => Some(local_name.as_str()),
            ImportBinding::Module { .. }
            | ImportBinding::Symbol { .. }
            | ImportBinding::Project { .. } => None,
        })
        .collect()
}

/// Seeds every foreign import as a definitely-bound `Ty::Object` global,
/// with no regard for the import's own position.
///
/// **The check phase no longer uses this.** Part 1 of #1026 seeded here
/// before the source-order top-level pass (D-041 pass 2), which made a read
/// placed *above* the `import` `I0404` rather than the `T0021` CPython's
/// `NameError` would justify -- sound only while Part 1 refused the read
/// unconditionally. Part 2 admitted the read, and the seed then admitted
/// the whole program: it lowered, built, and trapped at run time
/// (`llvm.trap`, rc 133) on the global-initialization failure edge. PR 2a
/// of #1081 removed the seed from `crate::module`, so such a read is now an
/// ordinary unbound-name `T0021` -- which is also the closer answer.
/// [`bind_foreign_objects_at`] does the positional binding the checker
/// keeps, and `docs/TYPE_SYSTEM.md` records the outcome.
///
/// The one remaining caller is [`crate::empty_container`]'s pre-pass, which
/// wants the position-blind form on purpose: it reports no diagnostic, so a
/// wider module scope than the checker's is the safe direction for it (that
/// call site carries the full argument).
///
/// D-040's sticky-representation rule is unaffected: a later `numpy = 3` in
/// the same module is `T0023`, because the name's recorded representation
/// is `object` and `int` is not assignable to it.
pub(crate) fn bind_foreign_objects(env: &mut Environment, imports: &[ImportBinding]) {
    for name in foreign_object_names(imports) {
        env.bind(name.to_string(), Ty::Object);
    }
}

/// Applies every foreign import recorded at `position` in the item list,
/// at the point the source-order pass reaches that position.
///
/// `ImportBinding::Foreign::item_index` is the item count at the moment the
/// `import` lowered, so the statement runs immediately *before* item
/// `position`; a trailing import records the item count itself, which the
/// caller applies once the loop is done. `Environment::bind` clears the
/// name's `def_rebound` mark, which is precisely what makes a later call
/// reach the `I0404` refusal instead of the stale function pointer -- and,
/// symmetrically, a `def` *below* the import re-marks the name and keeps
/// working, matching CPython's own last-binding-wins order.
pub(crate) fn bind_foreign_objects_at(
    env: &mut Environment,
    imports: &[ImportBinding],
    position: usize,
) {
    for binding in imports {
        if let ImportBinding::Foreign {
            local_name,
            item_index,
            ..
        } = binding
            && *item_index == position
        {
            env.bind(local_name.clone(), Ty::Object);
        }
    }
}

#[cfg(test)]
mod tests;
