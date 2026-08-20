//! Heap-bigint reference counting: the guarded retain/release emitter, the
//! two compile-time ownership classifications it is driven by, and the two
//! helpers that apply them ([D-180], [D-181]).
//!
//! Narrow carve out of `lib.rs` under AGENTS.md's decomposability rule: this
//! is exactly the unit #625 touches, and nothing else is relocated.
//!
//! [D-180]: ../../../docs/decisions/D-180-refcount-heap-bigints-and-release-them-at-named.md
//! [D-181]: ../../../docs/decisions/D-181-release-a-heap-bigint-s-birth-reference-at-every.md

use super::*;

/// Which of `pycc_rt`'s two bigint refcount entry points
/// `emit_bigint_refcount_call` should emit.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum BigIntRefcount {
    Retain,
    Release,
}

/// Emits a guarded `pycc_rt_bigint_retain`/`_release` call on the D-141
/// encoded int word `word`.
///
/// The guard is emitted *inline*, as LLVM IR, rather than left to the
/// runtime's own no-op arms: the predicate is `(word & 0b11) == 0 && word
/// != 0`, exactly `classify_encoded_int`'s `BigInt` case, so an ordinary
/// smallint loop performs one `and`, two `icmp`s and a not-taken branch per
/// site instead of a call into `pycc_rt`. D-084/D-140's nbody throughput
/// floor is measured on code that is entirely smallint; routing it through
/// an unconditional call per assignment would regress that gate, which is
/// the reason this guard exists at all and must not be simplified away.
///
/// Word `0` is excluded by the guard and *also* handled by the runtime:
/// `storage_slot_at_entry` zero-initializes `int` slots, and `0` is
/// `classify_encoded_int`'s fail-closed pattern rather than a valid encoded
/// int, so a release of a never-stored slot must reach neither the
/// classifier nor a dereference.
///
/// Leaves the builder positioned in a fresh continuation block. Callers that
/// record a basic block for a phi incoming edge must therefore re-read
/// `get_insert_block()` *after* calling this, never before.
pub(super) fn emit_bigint_refcount_call<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    rt: &RtFns<'ctx>,
    word: IntValue<'ctx>,
    op: BigIntRefcount,
) {
    // A compile-time-constant word is provably never a heap pointer: every
    // constant an `int` expression can produce is a tagged smallint, a bool
    // identity marker, or the empty-slot word, and a `BigIntObj` address is
    // only ever materialized by a runtime call. Emitting the guard anyway
    // would let inkwell fold all four of its halves into a constant `i1`,
    // leaving a conditional branch whose condition is no longer an SSA
    // definition -- dead IR that the D-180 guard-shape observer tests then
    // cannot describe. Skip it instead of emitting a guard that is known
    // false before it is built.
    if word.is_const() {
        return;
    }
    let i64_type = context.i64_type();
    let low_tag = builder
        .build_and(word, i64_type.const_int(0b11, false), "bigint_low_tag")
        .expect("build_and should not fail for two i64 values");
    let aligned = builder
        .build_int_compare(
            inkwell::IntPredicate::EQ,
            low_tag,
            i64_type.const_zero(),
            "bigint_aligned",
        )
        .expect("build_int_compare should not fail for two i64 values");
    let non_zero = builder
        .build_int_compare(
            inkwell::IntPredicate::NE,
            word,
            i64_type.const_zero(),
            "bigint_non_zero",
        )
        .expect("build_int_compare should not fail for two i64 values");
    let is_heap = builder
        .build_and(aligned, non_zero, "bigint_is_heap")
        .expect("build_and should not fail for two i1 values");
    let function = builder
        .get_insert_block()
        .expect("builder is always positioned inside some block while emitting")
        .get_parent()
        .expect("the block the builder is positioned in always belongs to a function");
    let call_block = context.append_basic_block(function, "bigint_rc_call");
    let continue_block = context.append_basic_block(function, "bigint_rc_cont");
    builder
        .build_conditional_branch(is_heap, call_block, continue_block)
        .expect("build_conditional_branch should not fail for a well-formed i1");
    builder.position_at_end(call_block);
    let callee = match op {
        BigIntRefcount::Retain => rt.bigint_retain,
        BigIntRefcount::Release => rt.bigint_release,
    };
    builder
        .build_call(callee, &[word.into()], "bigint_rc")
        .expect("build_call should not fail for a well-formed refcount call");
    builder
        .build_unconditional_branch(continue_block)
        .expect("build_unconditional_branch should not fail for a fresh block");
    builder.position_at_end(continue_block);
}

/// Loads `slot`'s current encoded word and releases it. `emit_assign` calls
/// this for every `Ty::Int` slot before storing, which is what makes the
/// slot's single owned reference an invariant: the slot holds `0` (released
/// as a no-op) until its first store, and exactly one owned word after it.
pub(super) fn release_int_slot_before_store<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    rt: &RtFns<'ctx>,
    slot: &StorageSlot<'ctx>,
) {
    let old = builder
        .build_load(context.i64_type(), slot.ptr, "old_int")
        .expect("build_load should not fail for this function's own alloca")
        .into_int_value();
    emit_bigint_refcount_call(context, builder, rt, old, BigIntRefcount::Release);
}

/// `int`'s counterpart to [`incref_if_str_duplicate`], deliberately kept as
/// a separate function applied at a strictly smaller set of call sites.
///
/// `incref_if_str_duplicate` runs at every site that *evaluates* a `str`,
/// including the two that merely consume the value without taking ownership
/// (`print`'s argument and `to_str`'s operand). That is harmless for `str`
/// because those two paths decref again themselves, but an `int` retain
/// there would be orphaned -- a leak with no matching release. So the
/// int-side retain is applied only where the value's new home actually
/// takes ownership: an assignment target, a `return` value, an instance
/// attribute, and a call argument.
///
/// The predicate itself extends `str_value_is_a_duplicate_reference`'s:
/// a bare `Name` or `AttrGet` read yields a *second* reference to a word
/// something else already owns, whereas every other int-producing
/// expression (`IntLiteral`, arithmetic, a call result) freshly constructs
/// its value already owning exactly one reference.
///
/// A `Ty::Int` element read out of a *tuple* is the third borrowed shape,
/// and is matched here in addition to those two. `MirExpr::TupleLiteral`
/// stores the element word it was handed without retaining it and
/// `MirExpr::Subscript`'s tuple branch hands that same word straight back
/// out, so a tuple field is a pure alias of whatever supplied it.
///
/// This arm carries an **unenforced precondition**: it is sound only while
/// the tuple field holds no owned reference of its own, i.e. while the name
/// that supplied the element is still bound to that word. D-181 records the
/// converse direction -- overwriting the supplying name before the tuple is
/// read -- as a known, unfixed use-after-free tracked by
/// [#633](https://github.com/rotnov/pycc/issues/633). Whoever gives tuple
/// fields a real owner must revisit both this arm and
/// `int_value_is_a_duplicate_reference`'s matching one.
///
/// Deliberately **not** shared with `int_value_is_a_duplicate_reference`
/// even though the two predicates agree on today's three borrowed shapes.
/// The two sides fail in opposite directions and must be free to diverge:
/// a missing retain here leaks, while a missing "borrowed" classification
/// on the release side frees a live word. Sharing one predicate would make
/// every future ownership refinement a simultaneous edit to both an
/// over-approximating and an under-approximating consumer.
///
/// The tuple arm is balanced at each of this helper's four call sites:
/// `MirStmt::Assign` is matched by the slot's own release-before-store,
/// `MirStmt::Return` is matched by the caller's D-181 release of the
/// `Call { ty: Int }` result, and the call-argument and `MirStmt::AttrSet`
/// sites are unmatched -- exactly D-180 residual 3's existing shape, not a
/// new leak class.
pub(super) fn retain_if_int_duplicate<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    rt: &RtFns<'ctx>,
    source_expr: &MirExpr,
    scalar: Scalar<'ctx>,
) -> Scalar<'ctx> {
    if let Scalar::Int(word) = scalar
        && match source_expr {
            MirExpr::Name {
                ty: pycc_mir::Ty::Int,
                ..
            }
            | MirExpr::AttrGet {
                ty: pycc_mir::Ty::Int,
                ..
            } => true,
            // No `Ty::Int` test alongside the tuple test: the enclosing
            // `if let Scalar::Int` has already established that this
            // element is an `int` word, so a `Ty::Float`/`Ty::Bool` tuple
            // field never reaches here at all.
            MirExpr::Subscript { base, .. } => matches!(base.ty(), pycc_mir::Ty::Tuple(_)),
            _ => false,
        }
    {
        emit_bigint_refcount_call(context, builder, rt, word, BigIntRefcount::Retain);
    }
    scalar
}

/// Whether evaluating `expr` yields a word this expression **borrows** from
/// some other owner, rather than a freshly owned one -- the release-side
/// mirror of `retain_if_int_duplicate`'s own inline classification (D-181).
///
/// Deliberately a separate predicate from that one, and deliberately an
/// exhaustive `match` rather than a `matches!`: the two sides fail in
/// opposite directions (a missing retain leaks; a wrongly-"owning" release
/// frees a live word), and exhaustiveness makes a future `MirExpr` variant
/// a compile error here instead of a silent double-free.
///
/// Three borrowed shapes today:
///
/// - `Name { ty: Int }` and `AttrGet { ty: Int }` read a word a storage
///   slot still owns.
/// - A `Subscript` on a *tuple* base (already known `Ty::Int`, see below). `MirExpr::TupleLiteral`
///   inserts its element word without retaining it and the tuple branch of
///   `MirExpr::Subscript` returns that same word unchanged, so the field is
///   a pure alias. The *list* `Subscript` branch is not borrowed: D-141's
///   container ingress rejects bigints outright, so a list element is
///   always a smallint the guard filters out anyway.
///
/// Everything else is **owning**, including every other container egress
/// (`DictGet`, `ListPop`, `DictGetOrDefault`): those all read D-141
/// container payloads, which by that same ingress rule are never heap
/// bigints, so classifying them owning emits a release whose inline guard
/// is statically false in practice and can never free a live word.
///
/// `Call { ty: Int }` is owning **conditionally**, and the premise is worth
/// stating: a user function's `return` runs `retain_if_int_duplicate` on
/// its value (D-180 decision 6) and that retain has no matching release at
/// the callee's boundary (D-180 residual 3), so a returned word does arrive
/// with a reference the caller may retire. Should that boundary ever gain
/// its own release, this classification must change with it. `len` needs no
/// such argument: it lowers to `Call { callee: "len", ty: Int }` whose
/// result is `raw_i64_to_tagged_int`'s odd-tagged smallint, so its release
/// is an unconditional runtime no-op.
fn int_value_is_a_duplicate_reference(expr: &MirExpr) -> bool {
    match expr {
        MirExpr::Name {
            ty: pycc_mir::Ty::Int,
            ..
        }
        | MirExpr::AttrGet {
            ty: pycc_mir::Ty::Int,
            ..
        } => true,
        // No `Ty::Int` test alongside the tuple test, for the same reason
        // `retain_if_int_duplicate`'s own tuple arm omits one: the only
        // caller, `int_temporary_word`, has already established that this
        // expression is `Ty::Int`, so a `Ty::Float`/`Ty::Bool` tuple field
        // never reaches this arm and testing for it here would be a
        // permanently-false branch.
        MirExpr::Subscript { base, .. } => matches!(base.ty(), pycc_mir::Ty::Tuple(_)),
        // One combined arm for every remaining variant, including the
        // non-`Ty::Int` `Name`/`AttrGet` reads the two arms above did not
        // claim. Grouped rather than enumerated one-per-line so the whole
        // "owning" answer is a single coverage region instead of twenty.
        MirExpr::Name { .. }
        | MirExpr::AttrGet { .. }
        | MirExpr::IntLiteral(_)
        | MirExpr::FloatLiteral(_)
        | MirExpr::BoolLiteral(_)
        | MirExpr::IntBoundary(_)
        | MirExpr::StringLiteral(_)
        | MirExpr::Call { .. }
        | MirExpr::BinOp { .. }
        | MirExpr::Compare { .. }
        | MirExpr::FString(_)
        | MirExpr::ListLiteral(_)
        | MirExpr::ListAppend { .. }
        | MirExpr::DictLiteral(_)
        | MirExpr::DictGet { .. }
        | MirExpr::SetLiteral(_)
        | MirExpr::TupleLiteral(_)
        | MirExpr::Slice { .. }
        | MirExpr::ListPop { .. }
        | MirExpr::DictGetOrDefault { .. }
        | MirExpr::SetAdd { .. }
        | MirExpr::Instantiate(_)
        | MirExpr::NullInstance { .. } => false,
    }
}

/// Releases the birth reference of a freshly built `int` word at a site
/// that consumes the word and then discards it (D-181, Part 2 of #146).
///
/// The mirror of `retain_if_int_duplicate`: that helper adds a reference
/// where a value acquires a *second* owner, this one retires the reference
/// a value was born with once its single consumer is done with it. Callers
/// pass the *source expression* -- ownership is a compile-time property of
/// how the word was produced, not something recoverable from the `i64`.
///
/// `MirExpr` is a tree and this crate's emitters evaluate each node exactly
/// once, so every produced word has exactly one consumer and the release
/// dominates nothing else that reads it. Exactly-once does *not* hold: a
/// D-173 exception edge can branch out of an expression after an operand
/// has been evaluated but before the release below is reached, so a pending
/// exception skips it. That is an enumerated D-181 residual leak, not an
/// unsoundness -- skipping a release can only leak.
///
/// The `Ty::Int` test is not redundant with the classification: callers
/// pass the word *after* `to_numeric_encoded_int`, which promotes a
/// `Ty::Bool` operand into an encoded word. Such a word is one of D-141's
/// two bool-identity markers, never a heap pointer, so there is nothing to
/// release and no reason to emit a guard for it.
pub(super) fn release_if_int_temporary<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    rt: &RtFns<'ctx>,
    source_expr: &MirExpr,
    word: IntValue<'ctx>,
) {
    if let Some(word) = int_temporary_word(source_expr, word) {
        emit_bigint_refcount_call(context, builder, rt, word, BigIntRefcount::Release);
    }
}

/// The classification behind `release_if_int_temporary`, exposed as a value
/// for the one shape that cannot release at the point of decision: a
/// comprehension's `stop`/`step` range operands, which `pycc_rt_range_
/// continue` re-reads on every iteration and which therefore have to be
/// released in the loop's `after_bb` instead.
///
/// `Some(word)` means "this word carries a birth reference someone must
/// retire"; `None` means it is borrowed, or not a `Ty::Int` value at all.
pub(super) fn int_temporary_word<'ctx>(
    source_expr: &MirExpr,
    word: IntValue<'ctx>,
) -> Option<IntValue<'ctx>> {
    (source_expr.ty() == pycc_mir::Ty::Int && !int_value_is_a_duplicate_reference(source_expr))
        .then_some(word)
}

/// `release_if_int_temporary` for a site that still holds a `Scalar` rather
/// than an already-encoded word (a discarded statement result, a loop or
/// `if` condition, a `print`/f-string operand).
///
/// A non-`Scalar::Int` value owns no bigint reference by construction, so
/// those kinds fall through untouched.
pub(super) fn release_scalar_if_int_temporary<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    rt: &RtFns<'ctx>,
    source_expr: &MirExpr,
    scalar: &Scalar<'ctx>,
) {
    if let Scalar::Int(word) = scalar {
        release_if_int_temporary(context, builder, rt, source_expr, *word);
    }
}
