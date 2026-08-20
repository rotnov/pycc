---
id: D-182
title: "Retain a borrowed int word at tuple-literal ingress"
status: accepted
---

## D-182: Retain a borrowed int word at tuple-literal ingress

- Status: accepted (issue
  [#633](https://github.com/rotnov/pycc/issues/633) direction B).
  **Narrows**
  [D-181](./D-181-release-a-heap-bigint-s-birth-reference-at-every.md)
  rather than superseding it. D-181's release-site list, its
  `int_value_is_a_duplicate_reference` predicate and every one of its
  classifications, its exception-path residual, and the
  `crates/pycc_codegen/src/bigint_rc.rs` carve-out are all unchanged and
  remain accepted. This decision narrows exactly two of D-181's recorded
  items: its **known, unfixed memory-safety defect** (#633's remaining
  direction, now closed) and its **residual leak item 1** (a fresh `int`
  word stored into a `TupleLiteral` element, whose justification changes
  and whose scope widens — see *Consequences*).

- Context:

  D-181 gave `MirExpr::Subscript`'s tuple branch a retain, closing one
  direction of #633: reading a tuple field into an `int` local and then
  overwriting *that* local no longer frees a word the tuple's supplier
  still holds. The mirror direction stayed open. A `MirExpr::TupleLiteral`
  stored the element word it was handed without taking any reference for
  the field, so the field was a pure alias of whatever supplied it.
  Overwriting the *supplying name* released the object the field still
  pointed at:

  ```python
  b: int = 4611686018427387904
  t = (b, 1)
  b = 0                          # releases b's word -> rc 0, freed
  c: int = 4611686018427387905   # reuses the freed BigIntObj
  print(t[0])                    # printed ...905, expected ...904
  ```

  The loop shape of the same defect did not merely print a wrong value —
  it hung under `pycc run`, walking a freed object as a bigint. This is
  genuine undefined behavior, not a wrong-answer bug, and D-181 recorded it
  as such while deliberately leaving it open.

  D-181's tuple-`Subscript` retain arm also carried an explicitly
  **unenforced precondition**: it was sound only while the supplying name
  was still bound to the word. That precondition is exactly what this
  defect violates, so the two are one problem.

- Decision:

  1. `MirExpr::TupleLiteral` passes each element scalar through the
     existing `retain_if_int_duplicate` at ingress, between the element's
     `emit_expr` and the `build_insert_value` that stores it. A tuple field
     now holds a reference of its own.

  2. The retain goes through `retain_if_int_duplicate`'s existing
     classification, so **only a borrowed element is retained** — a `Name`,
     an `AttrGet`, or a tuple `Subscript`. An *owning* element (a `BinOp`
     result, an out-of-range `IntLiteral`, a call result) already arrives
     holding the single reference the field will keep, and is not retained
     again.

  3. `int_value_is_a_duplicate_reference`'s tuple-`Subscript` arm stays
     `borrowed`. `MirExpr::Subscript`'s tuple branch hands the stored word
     back out without transferring the field's reference, so a reader owns
     nothing.

  4. No release is added anywhere. A `Ty::Tuple` slot that is overwritten
     or dies does not release its fields.

  5. D-181's unenforced precondition on the tuple-`Subscript` retain arm is
     retired. The arm's classification is unchanged; its justification is
     now "the field holds its own reference, and a read does not transfer
     it" instead of "the field is a pure alias, sound only while the
     supplier is still bound".

- Alternatives:

  - **Retain every `int` element unconditionally, not only borrowed ones.**
    Rejected. An owning element would reach rc 2 with exactly one owner
    (the field). Nothing could ever balance that: a future `Ty::Tuple`
    slot-death release under
    [D-124](./D-124-dict-str-int-set-int-refcounting-stays-leak-only.md)'s container refcounting would
    retire one reference and leave the object permanently at rc 1. The
    borrowed-only shape keeps every field at exactly one reference per
    owner, so a future slot-death release balances exactly.

  - **Emit a real release when a `Ty::Tuple` slot is overwritten or dies,
    balancing the ingress retain now rather than accepting a leak.**
    Rejected on the whole-tuple-copy case: `t2 = t` copies the aggregate
    field-by-field without going through `MirExpr::TupleLiteral`, so the
    copy takes no reference. A per-field release at slot death would then
    fire twice against one ingress reference — a double-free, strictly
    worse than the leak. Balancing the retain safely requires copies to
    take references too, which is D-124's container-refcounting work, not
    this change's.

  - **Release the element at ingress instead of retaining it,** treating
    the tuple as never owning anything. Rejected: it frees a value
    `Subscript` can still read back out, which is D-181's own recorded
    reason for not making `TupleLiteral` a release site.

  - **Leave #633 direction B open and document it harder.** Rejected. It is
    a use-after-free that hangs on a plain loop, not a leak; the whole
    point of D-180/D-181's refcounting is that a heap bigint's lifetime is
    sound. A leak is a bounded-memory problem; this was a soundness
    problem.

- Consequences:

  - #633 is closed in both directions. Three behavioral fixtures in
    `tests/issue_146_bigint_release.rs` pin it: the issue's own repro, the
    whole-tuple-copy variant, and the loop shape. All three were verified
    failing at the base commit — the first two printing the successor
    allocation's value, the third hanging until killed.

  - **A new accepted leak class.** D-181's residual leak item 1 covered a
    *fresh* word stored into a tuple element; it now also covers a
    *borrowed* one, because the ingress retain has no matching release. The
    practical shape is a supplier rebound inside a loop:

    ```python
    b: int = 4611686018427387904
    for i in range(n):
        b = b + 1
        t = (b, 1)     # one retained object leaked per trip
    ```

    This leak is trip-count-linear, measured rather than derived. Peak
    resident set size for that exact shape, built with `pycc build` and
    measured with `/usr/bin/time -l` on the authoring host:

    | trips     | before this decision | after this decision |
    | --------- | -------------------- | ------------------- |
    | 500 000   | 1 949 696 B          | 26 116 096 B        |
    | 1 000 000 | 1 933 312 B          | 50 200 576 B        |

    The "before" column is flat in the trip count -- each iteration's
    object was freed, which is precisely the use-after-free this decision
    closes, since the tuple still pointed at it. The "after" column scales
    at 1.92x for a 2x trip count, which is the linearity claimed above.

    It is accepted knowingly as the price
    of closing a use-after-free, on the standing principle that a bounded
    wrong-memory-usage is preferable to unbounded undefined behavior. It is
    deliberately **not** gated by a peak-RSS ratio assertion in
    `tests/issue_146_bigint_release.rs`'s `peak_rss` module: pinning a
    memory ceiling on a shape this decision knowingly regresses would
    enshrine the wrong number.

  - Closing the leak is a successor work item — "release a `Ty::Tuple`
    slot's owned `int` fields at slot death, and take a reference on
    whole-tuple copy" — tracked as
    [#636](https://github.com/rotnov/pycc/issues/636) in the same
    milestone, and blocked on
    [D-124](./D-124-dict-str-int-set-int-refcounting-stays-leak-only.md)'s container refcounting,
    which is what makes a copy take its own reference and therefore what
    makes a slot-death release safe. Both halves of that item are
    required together: a slot-death release without a copy retain would
    fire twice against one ingress reference, which is a double-free.

  - The `rc: Cell<u32>` overflow exposure recorded against D-180
    (`crates/pycc_rt/src/int_encoding.rs`, no saturation) is widened
    slightly: a tuple element is one more site that increments a count. It
    stays out of scope here — its failure mode is a wraparound to a wrong
    count, not a use-after-free introduced by this change, and it is
    D-180's residual to close.

  - Two in-crate IR-observer assertions moved with the new call and are
    part of this change:
    `a_tuple_element_operand_is_borrowed_while_a_literal_operand_is_owned`
    goes from `(0, 1)` to `(2, 1)` and
    `binding_a_tuple_element_to_an_int_local_retains_the_shared_word` from
    `(2, 1)` to `(4, 1)`, in both cases because the `TupleLiteral` of two
    borrowed `Name` elements now emits two ingress retains.
    `a_tuple_literal_retains_only_its_borrowed_int_elements` is new and is
    what discriminates decision 2's borrowed-only shape from the rejected
    unconditional one.
