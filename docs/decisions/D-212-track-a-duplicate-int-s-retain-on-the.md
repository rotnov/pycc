---
id: D-212
title: "Track a duplicate int's exception-edge retain, and correct D-208's site count"
status: accepted
---

## D-212: Track a duplicate int's exception-edge retain, and correct D-208's site count

- Status: accepted
- Context:
  [D-208](./D-208-release-a-pending-bigint-temporary-on-the-d-173.md) closed
  [#638](https://github.com/rotnov/pycc/issues/638) by pushing an owned
  `Ty::Int` temporary's word onto `rt.exceptions.pending_int_releases`
  immediately after it materializes, so a later sibling's raising evaluation
  releases it on the exception-unwinding edge instead of orphaning it. Its
  own review found a residual gap and opened
  [#834](https://github.com/rotnov/pycc/issues/834): at the two call sites
  where `retain_if_int_duplicate` retains a *borrowed* (duplicate) `int`
  before staging it for transfer into an aggregate field or a callee's
  parameter slot, that extra retained reference was never pushed onto the
  pending stack -- `int_temporary_word` (the classifier
  `push_pending_int_release_if_(scalar_)temporary` uses) excludes a
  duplicate reference by construction, since it exists to protect an
  *owning* temporary's word, not a retain layered on top of a borrowed one.
  A later sibling's raise therefore abandoned the retained reference with
  nothing left to release it -- a pure leak (never a use-after-free, since
  the original owner's own reference is untouched).

  D-208's own Context and Consequences prose also overstates which of its
  six protected sites actually call `retain_if_int_duplicate`: it describes
  the pending stack as protecting "the duplicated reference
  `retain_if_int_duplicate` produces for a `Compare` or a `Call`/
  `Instantiate` argument", which reads as though most or all six sites pair
  with a `retain_if_int_duplicate` call. Direct audit of the current tree
  (`crates/pycc_codegen/src/lib.rs`) shows this is not so: `BinOp`
  (push/pop at `lib.rs:2152`/`2154`), `Compare` (push/pop at
  `lib.rs:2341`/`2343`), and the `range()` preheater
  (`emit_range_operands_with_exception_safety`, `lib.rs:1359-1397`) push an
  *owning* temporary's own word directly and never call
  `retain_if_int_duplicate` at all -- none of them can receive a
  borrowed/duplicate operand through that code path. Only two of D-208's
  six protected sites do both (retain a duplicate, then push): the
  `MirExpr::TupleLiteral` element loop and
  `build_call_to_with_leading_args`'s argument loop. Per AGENTS.md, an
  accepted decision entry is never edited in place; this entry is the
  correction of record for that overstatement.

- Decision:
  Close #834 with a narrow, two-call-site fix in
  `crates/pycc_codegen/src/bigint_rc.rs` and
  `crates/pycc_codegen/src/lib.rs`, rather than widening
  `retain_if_int_duplicate`'s five other (assign-shaped, unprotected) call
  sites or reshaping the existing owning-temporary push mechanism:

  - `retain_if_int_duplicate`'s classification `match` is extracted into a
    new `retain_if_int_duplicate_reporting(...) -> (Scalar<'ctx>, bool)`,
    the single source of truth for "did this call actually retain". The
    public `retain_if_int_duplicate` becomes a thin `.0`-returning wrapper
    over it, so its five unprotected call sites (`MirExpr::OptionalWrap`,
    `MirExpr::NamedExpr`, `MirStmt::Assign`, `MirStmt::Return`,
    `MirStmt::AttrSet`) are a zero-diff.
  - A new `retain_if_int_duplicate_and_track_for_exception_edge(...)` calls
    the reporting variant and, when it reports `true`, pushes the retained
    word onto `rt.exceptions.pending_int_releases` via a new shared
    `push_word_onto_pending_int_releases(rt, word)` primitive, extracted
    from `push_pending_int_release_if_temporary`'s own inner push rather
    than duplicated. This new wrapper is used only at the
    `MirExpr::TupleLiteral` element loop and
    `build_call_to_with_leading_args`'s argument loop -- the only two call
    sites that both retain a duplicate and already bracket that retain with
    a `mark`/`truncate` pair on the pending stack (D-208's own
    owning-temporary protection at those same two sites). That existing
    bracket is what makes the new push safe: ownership of the retained word
    transfers to the tuple field / parameter slot on the normal path
    exactly as an owning temporary's word already does at that same site,
    so the site's existing truncate-without-release on success handles the
    new entry identically, with no new double-release risk.
  - The other five `retain_if_int_duplicate` call sites are unchanged: they
    have no `mark`/`truncate` bracket and no exception edge to protect
    (each releases or hands off its slot synchronously, or is itself an
    assign-shaped read with no staged transfer at all).
  - `bigint_rc.rs`'s doc comment on `retain_if_int_duplicate` is rewritten
    to enumerate all 7 real call sites (the pre-existing comment named only
    five, omitting `MirExpr::NamedExpr` and `MirExpr::OptionalWrap`) and
    their push/no-push status.

- Alternatives:
  - *Push at every call site of `retain_if_int_duplicate`, including the
    five unprotected ones.* Rejected: those five sites have no
    `mark`/`truncate` bracket and no exception-edge check nearby; pushing
    there would either dangle an entry across an unrelated
    `pop_pending_int_release`'s `debug_assert_eq!` or require inventing a
    bracket that serves no existing purpose, contradicting the issue's own
    explicit "out of scope" list for those sites.
  - *A boolean out-parameter instead of a tuple return from the reporting
    variant.* Rejected as a pure style choice with no functional
    difference; the tuple-return form matches this module's own existing
    `Option`/`bool`-returning classifier idiom and needs no `&mut bool`
    plumbing through two call sites.
  - *Reclassify at the `pending_int_releases` consumer side* -- teach
    `int_temporary_word` to re-derive "was this retained" from
    `source_expr` alone, matching `int_value_is_a_duplicate_reference`'s
    own classification. Rejected: `retain_if_int_duplicate`'s doc comment
    warns that predicate and `int_value_is_a_duplicate_reference` "must
    fail in opposite directions" and are kept deliberately separate;
    re-deriving retain-happened from source-expression shape a second time
    reintroduces exactly the two-classifiers-drift risk that warning is
    about, and does not change the push call sites either way.

- Consequences:
  - #834 is closed: a borrowed/duplicate `int`'s extra retain at the
    `TupleLiteral` element and call-argument sites is now released on the
    D-173 exception-unwinding edge exactly like an owning temporary's word
    already was, closing the last leak flavor D-208's own review left open.
  - **Double-release safety for the call-argument site is a derived, not
    merely assumed, property**: `build_call_to_with_leading_args`'s
    argument-marshalling loop truncates `pending_int_releases` back to
    `mark` only after every argument has evaluated successfully, and only
    then emits the call. If any argument raises, the guard branches away
    before the call (and therefore before the callee's own parameter-slot
    release machinery can ever observe that parameter) is reached, so no
    code path lets both the new exception-edge release and a callee-side
    parameter release fire on the same reference. This was verified with a
    concrete reproduction
    (`a_call_argument_borrowed_value_survives_the_calls_own_raising_argument`
    in `tests/issue_638_bigint_exception_release.rs`) rather than left as
    an unverified argument: `f(x, 1 // z)` where `x` is a borrowed `int`
    argument and the second argument raises, printing `x` and `x + 1`
    afterward to prove the value survives uncorrupted.
  - `pop_pending_int_release`'s `debug_assert_eq!` cannot observe an
    unexpected top-of-stack because of the new pushes: both fix sites push
    only after the element's/argument's own `emit_expr` (and now
    `retain_if_int_duplicate_reporting`) call has fully returned, so any
    nested `BinOp`/`Compare` push/pop pair inside that sub-expression's own
    evaluation has already balanced before the new push executes.
  - D-208's Context and Consequences sections' "all six sites pair with
    `retain_if_int_duplicate`" implication is superseded by this entry:
    only the `TupleLiteral` element loop and the call-argument transfer
    site do so; `BinOp`, `Compare`, and the `range()` preheater push an
    owning temporary's own word directly and never call
    `retain_if_int_duplicate`. D-208's file is left unedited per AGENTS.md's
    decision-log rule; this entry is the correction of record.
  - `tests/issue_638_bigint_exception_release.rs` gains two value-
    correctness tests (one per fix site) and two peak-RSS marginal-ratio
    tests (`< 1.15`, matching the file's own established threshold and
    same-allocation-rate-control convention), extending the existing suite
    rather than duplicating its helpers in a new file.
  - `docs/RUNTIME.md` and `docs/ROADMAP.md` are updated in the same pull
    request to state #834's closure and name the two actual sites, rather
    than continuing to describe the gap as open or as spanning all six
    D-208 sites.
