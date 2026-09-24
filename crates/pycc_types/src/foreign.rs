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
//! The arm is not reached for four method names. `pycc_hir`'s container
//! fast paths claim `append`, `pop`, `get` and `add` while lowering, so
//! `gc.get(1, 2)` never becomes a plain `HirExpr::MethodCall` and is refused
//! here through a different consumer. In a module that can see a user class
//! defining one of those names, the call is a
//! `HirExpr::ReceiverDispatchedCall` instead (issue #1188), but
//! `pycc_hir::receiver_takes_method_path` sends a `Ty::Object` receiver down
//! the container path all the same, so the refusal is unchanged.
//! `docs/TYPE_SYSTEM.md`'s `object` row owns that statement, and #1095
//! tracks routing them to foreign dispatch.
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
//! **PR 4a of #1083 (Part 4 of #1026) added two conversions *out* of the
//! opaque type: `float(o)` and `bool(o)`.** Both are handled in `expr.rs`'s
//! `HirExpr::Call` arm (and mirrored in `constraints.rs`) rather than here:
//! `float`'s existing argument gate gains [`Ty::Object`], and `bool` gets a
//! new arm admitted for [`Ty::Object`] **only**, so `bool(1)` keeps its
//! `C0001` verbatim. Both arms carry `float`'s user-defined-function guard,
//! because a `def float(...)`/`def bool(...)` is a valid working program
//! today and must keep winning.
//!
//! These are the first operations that produce a *pycc-native* value from an
//! object, so they are also the first that leave nothing behind: the shim
//! releases its own CPython temporary on every exit and no new reference
//! escapes, which is why Part 4 does not grow #1092. They run CPython's own
//! conversion protocol (`PyNumber_Float`, `PyObject_IsTrue`), which is *not*
//! a D-244 rule-7 violation: rule 7 closes the boundary at the thunk export
//! seam, where a value crosses implicitly and the annotation is the whole
//! contract, whereas `float(o)` in user source is an explicit conversion
//! request that names its destination type. `docs/TYPE_SYSTEM.md`'s `object`
//! row carries the user-facing statement.
//!
//! The residual incoherence is stated rather than hidden: `bool(o)` compiles
//! while `bool(1)` is still `C0001`, because Part 4 relaxes exactly the
//! object case and leaves the general builtin-conversion story to
//! #1017/#1018. The positional bound is inherited unchanged, and
//! `print(float(o))` now type-checks where `print(o)` stays `I0404` --
//! the refusal is on the object, not on a `float` derived from one.
//!
//! **PR 4b of #1083 adds the other two conversions out of the opaque type:
//! `int(o)` and `str(o)`.** Both are new `expr.rs` arms (mirrored in
//! `constraints.rs`) on the `bool` arm's exact shape -- [`Ty::Object`]
//! **only**, both the user-defined-function and the user-defined-class guard
//! on the whole arm, since neither arm admits anything else. `int(o)` runs
//! `PyNumber_Long` and D-141-encodes the result, refusing a value outside the
//! inline-integer range `[-2**62, 2**62-1]` with `OverflowError` (#1040, no
//! bigint path); `str(o)` runs `PyObject_Str` and copies the UTF-8 out with
//! `pycc_rt_str_from_literal`, producing exactly what a `str` literal
//! produces. Both release their CPython temporary on every exit, so Part 4
//! still adds nothing to #1092.
//!
//! The same asymmetry is inherited and stated: `str(o)` compiles while
//! `str(1)` keeps its `C0001`, and `print(str(o))` type-checks where
//! `print(o)` stays `I0404` -- `string_conversion.rs`'s [`Ty::Object`] arm
//! is untouched, because the refusal is on the object and not on a `str`
//! derived from one.
//!
//! **PR 4c of #1083 admits the opaque type at one *annotated assignment*.**
//! `x: tuple[float, float, float] = <object>` in a module body -- and more
//! generally any fixed-arity annotation whose every element is `float` --
//! now compiles. This is the first Part 4 operation that is not a call:
//! `check_stmt`'s module-level `HirStmt::AnnAssign` arm tests
//! [`is_object_float_tuple_annotation`] *ahead of* `is_assignable_env`, so
//! the relaxation is a branch in that one arm rather than a widening of
//! `is_assignable`, which would have admitted the pair everywhere a value
//! flows into a declared type.
//!
//! **Strict container, converting elements.** The shim checks
//! `PyTuple_Check` with an exact-arity test and then runs `PyNumber_Float`
//! on each item; the runtime object's items are never type-checked by pycc,
//! and a bad item fails at run time with whatever CPython raises. The two
//! halves answer two different questions -- D-115/D-116 leave no shape for
//! a differently-sized sequence, while `float` is a type the author wrote,
//! which makes converting to it the same explicit-conversion case PR 4a
//! records.
//!
//! Everything else stays refused, and the refusals are two different
//! diagnostics: a *mixed* annotation such as `tuple[float, int]` is the
//! ordinary `T0025` this arm already produced, while PEP 585's variadic
//! `tuple[float, ...]` never reaches this crate -- `pycc_hir` refuses the
//! `...` type argument with `T0053` while lowering the annotation. The
//! in-function arm gains no branch at all: the read of the foreign name is
//! already `I0404` there, which PR 4c's own test pins.
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
use pycc_hir::{ForeignImportSite, ImportBinding, Ty};

/// Whether a declared annotation is a fixed-arity tuple whose every
/// element is `float` -- the one annotation a [`Ty::Object`] initializer
/// may be assigned to (Part 4 of #1026, PR 4c of #1083).
///
/// This is the **canonical statement of that admission rule**. Two mirrors
/// restate it and must not drift: `pycc_mir::stmt`'s
/// `float_tuple_annotation_arity`, which decides whether the lowering emits
/// `MirExpr::ObjUnpackFloatTuple`, and the shim helper's own exact-arity
/// `PyTuple_Check`, which enforces the container half at run time. Neither
/// can share this function -- `pycc_mir` does not depend on this crate, and
/// the shim is C.
///
/// **Fixed arity only.** PEP 585's variadic `tuple[float, ...]` is not
/// admitted and does not reach here at all: `pycc_hir` refuses the `...`
/// type argument with `T0053` while lowering the annotation, which is a
/// different diagnostic from the `T0025` a *mixed* annotation such as
/// `tuple[float, int]` still gets from the caller below. Both stay refused.
///
/// The emptiness test guards `tuple[()]`, which `pycc_hir` also refuses
/// with `T0053` before this crate runs; it is kept because the rule is
/// "arity at least one", and because codegen's out-slot is an
/// `[arity x double]` array that a zero arity would make degenerate.
pub(crate) fn is_object_float_tuple_annotation(ty: &Ty) -> bool {
    matches!(ty, Ty::Tuple(elems) if !elems.is_empty() && elems.iter().all(|elem| matches!(elem, Ty::Float)))
}

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
             scalar-key subscript load, `for` iteration, the `float`, \
             `bool`, `int` and `str` conversions and an annotated \
             module-level assignment to a fixed-arity all-`float` `tuple` \
             on it and nothing else"
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
/// A `ForeignImportSite::Item` index is the item count at the moment the
/// `import` lowered, so the statement runs immediately *before* item
/// `position`; a trailing import records the item count itself, which the
/// caller applies once the loop is done. `Environment::bind` clears the
/// name's `def_rebound` mark, which is precisely what makes a later call
/// reach the `I0404` refusal instead of the stale function pointer -- and,
/// symmetrically, a `def` *below* the import re-marks the name and keeps
/// working, matching CPython's own last-binding-wins order.
///
/// A [`ForeignImportSite::Block`] import is never bound here: its own
/// `HirStmt::ForeignImport` statement binds it where it runs (#1291).
pub(crate) fn bind_foreign_objects_at(
    env: &mut Environment,
    imports: &[ImportBinding],
    position: usize,
) {
    for binding in imports {
        if let ImportBinding::Foreign {
            local_name,
            site: ForeignImportSite::Item(index),
            ..
        } = binding
            && *index == position
        {
            env.bind(local_name.clone(), Ty::Object);
        }
    }
}

#[cfg(test)]
mod tests;
