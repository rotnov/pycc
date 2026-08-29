---
id: D-208
title: "Release a pending bigint temporary on the D-173 exception-unwinding edge"
status: accepted
---

## D-208: Release a pending bigint temporary on the D-173 exception-unwinding edge

- Status: accepted
- Context:
  [D-181](./D-181-release-a-heap-bigint-s-birth-reference-at-every.md)
  enumerated, as its second residual leak, exactly the defect
  [#638](https://github.com/rotnov/pycc/issues/638) tracks: an owned bigint
  temporary -- the result of a `BinOp`, or the duplicated reference
  `retain_if_int_duplicate` produces for a `Compare` or a `Call`/`Instantiate`
  argument -- can be evaluated and then never released if a *sibling*
  sub-expression raises before the temporary's own scheduled release site
  runs. D-173's `guard_statement_effects` already installs the exception
  branch for any statement whose effects can raise
  (`expression_can_set_exception`: `Call`/`DictGet`/`Instantiate`
  unconditionally, `BinOp` for `Div`/`FloorDiv`/`Mod`, `Subscript` only for a
  `List` base); the released D-181 codegen simply had no way to reach the
  operand's already-materialized word once control took that branch instead
  of falling through to the release call sequence. D-181 deliberately left
  this open, calling it "unwinding work, not refcounting work" -- the two
  concerns need a shared mechanism, not a shared decision, and this is that
  mechanism.

  D-181 also fixed the guard's shape to a two-block branch
  (`effect_exc`/`effect_exc_cont`) specifically to keep the D-084/D-140
  nbody hot loop's inner comparison and arithmetic free of any per-iteration
  refcounting overhead when nothing is pending. Any fix here has to preserve
  that guarantee: it cannot add a runtime check, an unconditional block, or
  even an extra never-taken branch to a call site where no temporary is ever
  pending.

- Decision:
  Thread a per-codegen pending-release stack through the existing exception
  guard, populated and drained entirely at codegen time so a call site with
  nothing pending emits exactly the same two-block shape D-181 left behind:
  - `ExceptionCodegenState` (`crates/pycc_codegen/src/exception.rs`) gains
    `pending_int_releases: RefCell<Vec<IntValue<'ctx>>>`. Every site that
    computes an owned bigint temporary destined for its own later release --
    a `BinOp` result, the retained duplicate `retain_if_int_duplicate`
    returns for a `Compare` operand or a `Call`/`Instantiate` argument --
    pushes the value with `push_pending_int_release_if_temporary` /
    `push_pending_int_release_if_scalar_temporary`
    (`crates/pycc_codegen/src/bigint_rc.rs`) immediately after computing it,
    and pops it with `pop_pending_int_release` at its own ordinary release
    site once the value has actually been consumed (stored, returned, or
    handed off) -- the pending stack tracks a window of "materialized but
    not yet released," not a permanent bookkeeping structure.
  - `guard_statement_effects` snapshots the stack's current contents before
    branching. When it is non-empty at codegen time, the guard grows a third
    LLVM basic block, `effect_exc_unwind`, interposed between the exception
    check and the pre-existing `effect_exc` target: it releases every
    snapshotted value (`pycc_rt_bigint_release`, respecting the existing
    tagged-word guard) and then branches on to the installed exception
    target exactly as `effect_exc` always did. When the stack is empty --
    the common case, including every iteration of the nbody hot loop -- no
    third block is emitted at all; the guard is byte-for-byte the same
    two-block shape D-181 produced. This is a Rust-side `is_empty()` check
    against a compile-time-tracked Rust `Vec`, not a runtime branch, so it
    costs nothing when nothing is pending.
  - Because the stack is drained by each site's own ordinary
    `pop_pending_int_release` on the non-exception path, a value already
    consumed and popped before a later sibling raises is not
    double-released: `effect_exc_unwind` only ever sees what is still on the
    stack at the moment a given guard fires, which by construction is
    exactly the operands evaluated after the last release and before this
    exception check.

- Alternatives:
  - *Wrap every `BinOp`/`Compare`/`Call` argument evaluation in its own
    `invoke`-style unwind landing pad, mirroring LLVM's native exception
    model.* Rejected: pycc's exception model is D-173's own explicit
    poll-and-branch design, not LLVM `invoke`/`landingpad`; adopting the
    native model here would be a much larger, unrelated architectural
    change affecting every exception-settable call site in the compiler,
    not a fix scoped to bigint temporary leaks.
  - *Emit the unwind-release block unconditionally at every guarded site,
    querying "is anything pending" at runtime instead of at codegen time.*
    Rejected: this reintroduces exactly the per-iteration overhead D-181's
    two-block guard redesign eliminated for D-084/D-140's nbody floor, for
    call sites (the overwhelming majority) where nothing is ever pending.
    The codegen-time `is_empty()` check costs nothing and produces
    identical IR to before whenever the stack is empty.
  - *Fix this by tying every temporary's release to Rust-style scope-exit
    (a `Drop`-like codegen construct) instead of an explicit pending-stack.*
    Rejected as a larger, more invasive redesign of the whole D-180/D-181
    refcounting emission strategy; the pending-stack approach fixes the
    specific residual gap D-181 named without touching the non-exception
    release sites at all.

- Consequences:
  - D-181's residual item 2 (the exception-path skip) is closed for every
    site `expression_can_set_exception` recognizes: `BinOp` operands,
    `Compare` operands, and `Call`/`Instantiate` argument transfer sites.
    It is also closed for one further site outside that classifier's own
    scope: `emit_range_operands_with_exception_safety`'s `range()`-shaped
    start/stop/step preheader, shared by `MirStmt::ForRange` and its three
    comprehension-tail copies. That site is reached through statement-level
    lowering, not through `expression_can_set_exception` (which classifies
    `MirExpr` nodes only and has no `MirStmt::ForRange` case at all), but it
    uses the identical `push_pending_int_release_if_temporary`/
    `guard_statement_effects` mechanism to protect each already-evaluated
    bound across the next one's evaluation.
    A second-round independent review found a sixth site the original
    inventory missed: `MirExpr::TupleLiteral`'s own element-evaluation loop
    (`crates/pycc_codegen/src/lib.rs`). A fresh, owning `Ty::Int` element's
    word is meant to transfer into the aggregate's own field via
    `build_insert_value` -- the identical "ownership transfer" shape
    `build_call_to_with_leading_args`'s argument-marshalling loop already
    used, not a "materialize then release" shape -- so if a *later* sibling
    element's own evaluation raises before the loop completes, an earlier
    element's birth reference was orphaned the same way an earlier call
    argument's would be. The fix mirrors `build_call_to_with_leading_args`
    exactly: a `mark` recorded before the loop, each owning (non-duplicate)
    element's word pushed via `push_pending_int_release_if_scalar_temporary`
    right after `retain_if_int_duplicate` runs, and the stack truncated back
    to `mark` -- never released -- once every element has legitimately
    transferred ownership into the aggregate on the normal path. No change
    to `expression_can_set_exception` was needed: each element is evaluated
    through the ordinary recursive `emit_expr` call, so a later element that
    is itself a `Call`/`Div`/`FloorDiv`/`Mod`/etc. already trips its own
    `guard_statement_effects` call before the next element is reached,
    exactly like a `BinOp`'s right operand or a call's later argument.
    This is now the sixth (and, per this review round, final) site closed
    by this decision.

    D-181's residual item 1 (the `TupleLiteral` element, tracked separately
    as [#636](https://github.com/rotnov/pycc/issues/636)) is a genuinely
    different, still-open defect from the one above: #636 is about a
    *borrowed* element's ingress-retain having no matching release at the
    tuple's own slot-death, blocked on D-124's container release
    infrastructure -- it does not involve an owning temporary or the
    exception-unwinding edge at all, and this decision's sixth-site fix does
    not touch it. `docs/RUNTIME.md` and `docs/ROADMAP.md` are updated in the
    same pull request to keep that distinction clear rather than conflating
    the two.
  - **A new residual gap this decision's own review found and left open,
    tracked as [#834](https://github.com/rotnov/pycc/issues/834):** at every
    site above that pairs `retain_if_int_duplicate` with a
    `push_pending_int_release_if_(scalar_)temporary` call, `int_temporary_word`
    classifies only by the *source expression*, so a duplicate/borrowed
    source's freshly created retain is never pushed onto
    `pending_int_releases` even at a call site that just performed that
    retain. If a later sibling operand/element raises before the retained
    reference transfers to its real owner (a call's parameter slot, a
    tuple's field), that reference is abandoned and leaked on every caught
    exception. This is not D-180 residual item 3 (a retain that *does*
    eventually transfer to a real owner, leaked only until that slot is
    later overwritten) -- it is a retain abandoned *before* transfer ever
    completes, which nothing else will ever release. Closing it needs
    `retain_if_int_duplicate` to report whether it actually retained and an
    audit of its other call sites (`MirStmt::Assign`/`Return`/`AttrSet`,
    where a pending-release push would be wrong), which is why it is
    tracked separately rather than folded into this decision's own six-site
    scope.
  - The nbody hot loop's own comparison and arithmetic sites push nothing
    (D-084/D-140's floor scenario never raises across a bigint temporary's
    lifetime) and consequently still emit the original two-block guard
    shape verified structurally by this pull request's own IR-observer unit
    tests (`crates/pycc_codegen/src/bigint_rc.rs`'s
    `a_lone_raising_binop_with_nothing_pending_emits_no_unwind_block`), not
    just asserted by argument.
  - The most direct external evidence this fix works -- a peak-RSS oracle
    comparing single-vs-double iteration counts, following
    `tests/issue_146_bigint_release.rs`'s existing convention -- turned out
    to be blind to it: the repro shapes this issue targets raise on *every*
    iteration, and `crates/pycc_rt/src/exception.rs`'s pre-existing,
    already-documented "leak-only" exception-object lifetime model (
    `pycc_rt_exception_clear` resets only the thread-local `EXCEPTION_STATE`
    cell and never frees the heap-allocated `PyExceptionObj` `pycc_rt_exception_alloc`
    produced) leaks one exception object per iteration regardless of this
    fix, and that leak's own growth dominates a raw single-vs-double ratio
    for either the fixed or the unfixed binary alike (empirically, both read
    approximately 1.9-1.95x at 250k/500k iterations). `tests/issue_638_bigint_exception_release.rs`
    instead measures *marginal* RSS growth (`peak_rss(2N) - peak_rss(N)`)
    for the leak-shape repro against a same-exception-rate control repro
    (identical control flow, but reading an already-live `Name` instead of
    materializing a fresh temporary) and asserts the two marginals track
    within 15% of each other -- calibrating out the shared per-iteration
    exception-object leak and isolating the bigint-specific effect this
    decision fixes. This was verified empirically in both directions before
    being trusted: with the fix applied, `leak_marginal / control_marginal
    ≈ 0.9995` for the original three repro flavors (`BinOp`, `Compare`, `Call`
    argument); with the fix reverted (via a direct `git stash` A/B rebuild
    of the same repro shapes), the ratio widens to `≈ 1.333`. This round adds
    a fourth flavor, `TupleLiteral`, exercising the sixth site described
    above; it passes the same `< 1.15` marginal-ratio assertion
    (`a_tuple_literal_element_orphaned_on_the_exception_edge_does_not_grow_with_the_iteration_count`
    in `tests/issue_638_bigint_exception_release.rs`) and was verified the
    same way -- confirmed to fail without the fix applied. The exception-
    object leak itself remains out of this decision's scope -- it is a
    pre-existing, already-documented defect in `pycc_rt::exception`, not a
    bigint refcounting defect -- and is left for its own future issue.
  - Structural coverage for the new `effect_exc_unwind` block comes from two
    new `crates/pycc_codegen/src/bigint_rc.rs` unit tests built on the
    crate-private `compile_to_object_with_observer` hook (there is no public
    `--emit-llvm` CLI flag): one confirms the block is emitted and contains
    the expected `pycc_rt_bigint_release` call when an owned operand is
    genuinely pending across a sibling's raise; the other confirms no such
    block is emitted at all for a lone raising expression with nothing
    pending, directly proving the hot-loop zero-overhead claim above without
    depending on the `#[ignore]`d nbody throughput benchmark
    (`nbody_release_binary_meets_required_speedup_over_cpython`, which this
    environment cannot run locally for lack of an exact pinned CPython
    build -- a pre-existing gap, not a new one this change introduces).
