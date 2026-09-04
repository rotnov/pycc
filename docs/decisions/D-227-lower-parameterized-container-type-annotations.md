---
id: D-227
title: "Lower parameterized container type annotations in every position but return"
status: accepted
---

## D-227: Lower parameterized container type annotations in every position but return

- Status: accepted (issue [#918](https://github.com/rotnov/pycc/issues/918), Part 1).
- Context:
  [D-105](./D-105-v0-2-s-list-t-thin-slice-scope-cuts-and-runtime.md) cut 1
  deliberately shipped `list[T]` with **no annotation syntax at all**: a
  `list[int]` value could only come into existence through bare-name local
  inference (`x = [1, 2, 3]`), never as an annotated parameter or return
  type, and `x: list[int] = []` was rejected by `annotation_to_ty`'s "only a
  bare name type annotation is supported so far" error. D-121/D-122 (dict and
  set) and [D-116](./D-116-tuple-v0-2-scope-int-bool-float-elements-only.md)
  cut 3 each inherited that same deferral by explicit reference, so all four
  container families reached v0.4 with real codegen but no way to *write*
  their types.

  The practical consequence was worse than a missing convenience. Because a
  container type could never be written, it could never cross a function
  boundary from real source: `pycc_types`' signature solver is scalar-only, so
  a container argument had no way to be declared, and users hit a
  capability diagnostic naming only the head name (`type annotation
  \`list\` is not supported yet`) that gave no hint the parameterized form was
  the missing piece rather than lists themselves.

  A naive fix — lowering `Expr::Subscript` on the four builtin names straight
  to the corresponding `Ty` — was prototyped and **panics**: `list[str]` and
  `list[list[int]]` reach unhandled codegen cases, and wrong-arity forms
  (`dict[int]`, `list[int, str]`, `tuple[()]`) are either accepted silently or
  crash while destructuring the type-argument list. The element-type gates
  that make container *literals* safe (`T0034`/`T0036`/`T0038`/`T0039`) lived
  in `pycc_types`, one crate *above* the `pycc_hir` lowering that would now
  produce the very same types, so they could not simply be called.

- Decision:
  1. **Lower `list[T]`, `set[T]`, `dict[K, V]` and `tuple[A, B, ...]`** in
     `pycc_hir::func::annotation_to_ty`, which makes them available in every
     position that routes through it: function parameters, local and
     module-level `AnnAssign`, PEP 695 `type X = ...` and legacy
     `X: TypeAlias = ...` aliases. A protocol member's annotation also
     routes through it, but decision 10's gate then rejects a container
     result, so a container type is *not* usable there.
  2. **Validate arity explicitly**, with a new diagnostic `T0053`: `list` and
     `set` take exactly one type argument, `dict` exactly two, `tuple` at
     least one. `T0053` also rejects two legal-Python spellings this version's
     fixed-arity `Ty::Tuple` cannot represent — the empty `tuple[()]` (which
     arrives as a zero-element `Expr::Tuple`) and the homogeneous-variadic
     `tuple[int, ...]`. Arity is checked *before* element types, so a
     wrong-arity annotation never reports a misleading element-type error.
  3. **Share the element-type gates** rather than duplicating them. The four
     capability checks move down into a new `pycc_hir::container` module and
     `pycc_types` calls *in*. The crate dependency runs `pycc_types` →
     `pycc_hir` and never the reverse, so this is the only direction that
     works. The helper is `pub` and re-exported from `pycc_hir`'s root.
  4. **Two entry points, not one.** `check_container_ty` is a whole-`Ty` check
     for list/dict/set; `check_tuple_element_ty` is per-element. `T0039`
     fires from *inside* `pycc_types`' tuple-literal inference loop, not as a
     postcheck, so that the elements are gated in source order and an
     earlier element's type gate is reported ahead of a later element's own
     inference failure (an undefined name). Folding
     the two together would silently reorder diagnostics — `(1, "a",
     undefined_name)` would report the undefined name instead of `T0039`.
  5. **Both gates take a `Span`.** The four existing literal call sites keep
     passing `Span::new(0, 0)` so their rendered output is byte-identical to
     the previous release; annotation lowering passes the annotation's real
     source range, so the newly reachable annotation diagnostics carry a real
     caret (the precedent is `T0049` in the same function).
  6. **Reject a `Ty::Param` element in `pycc_hir`**, with `T0042` — the same
     code and wording `pycc_types`' own signature scan uses, but with a real
     span instead of that scan's `Span::new(0, 0)`. This is not merely a
     nicer caret: `substitute_ty` is not recursive, so a `Ty::Param` nested
     inside a container would never be substituted at a call site. The
     downstream scan remains as defense in depth.
  7. **Split the bare-container `C0001` message.** A bare `list`/`set`/`dict`/
     `tuple` now gets its own message naming the parameterized form to write
     (`a bare \`list\` type annotation is not supported yet -- write the
     parameterized form, e.g. \`list[int]\``). It is deliberately *not*
     cascade-shaped ([D-219](./D-219-classify-a-failed-item-s-cascade-by-parsing-its.md)):
     `cascade_name` returns `None` for it, which is correct, since nothing in
     a module can be waiting for `list` to be defined. `frozenset` and `type`
     keep the generic unknown-name message — neither has a `Ty` variant, so
     steering a user toward `frozenset[int]` would point at a form this
     version rejects just as hard.
  8. **A user-defined class or type alias named `list` still wins.** The
     container branch is checked *after* the known-class lookup and is gated
     on the alias table, so `class list:` and `type list = int` keep the
     behaviour they had before this decision.
  9. **Container types are rejected in return position**, with `C0001`
     naming the positions that do work. This is deliberate, not an oversight:
     a container-typed *call result* already reaches an unhandled codegen case
     today via [D-146](./D-146-infer-a-private-helper-s-return-type-from-its.md)'s
     private-helper return-type solver (tracked as issue
     [#926](https://github.com/rotnov/pycc/issues/926)), so accepting the
     annotation would widen a known panic's reachability rather than add a
     working feature. Return position is issue
     [#925](https://github.com/rotnov/pycc/issues/925).
  10. **Container types are rejected as protocol attributes**, with `C0001`.
      The protocol-member `AnnAssign` branch ran no type gate at all before
      this decision — not by design, but because no annotation syntax could
      produce a container `Ty` there, unlike the two class-body attribute
      sites, which both run `is_scalar_slot_type`. Structural conformance
      against a container-typed member has no exercised path anywhere in the
      compiler, so it is rejected rather than shipped untested. Every
      non-container member type (`Ty::Instance`, `Ty::Optional`, `Ty::None`,
      `Ty::Protocol`) is unaffected, which is why this is a container check
      and not a reuse of `is_scalar_slot_type`.

  This decision **supersedes, as to annotation syntax only**:
  - **D-105 cut 1** ("No annotation syntax for `list[T]` in v0.2") in full;
  - **D-116 cut 3's** deferral of "a `tuple[...]` annotation syntax", and
    **D-116 cut 4's** parenthetical claim that "there is no annotation syntax
    for any of the four container types".

  It does **not** touch D-105's or D-116's other cuts, and does not touch
  D-115/D-122 at all: D-116 cuts 1 (int/bool/float elements only) and 2
  (literal, non-negative, in-range tuple indices) remain in force unchanged,
  and the one-shipped-shape element restrictions for list/dict/set are exactly
  what this decision's shared gates now enforce in *both* the literal and the
  annotation path.

- Alternatives:
  - *Put the shared gate in `pycc_types` and have `pycc_hir` call it.*
    Impossible: `pycc_types` depends on `pycc_hir`, so this inverts the crate
    graph.
  - *Duplicate the four checks in `pycc_hir`.* Rejected: two copies of "which
    container shapes does this version compile" is exactly the drift the
    element gates exist to prevent, and the whole point of #918 is that a
    literal and an annotation must agree.
  - *One unified whole-`Ty` gate.* Rejected for the diagnostic-ordering
    reason in decision 4 above; measured, not assumed.
  - *A dedicated `"list" | "set" | "dict" | "tuple"` match arm ahead of the
    known-class lookup.* Simpler, and what #918's plan proposed, but it
    shadows a user's own `class list`. Silently retyping `x: list[int]` as a
    builtin list when the user defined their own `list` is a miscompile, not
    merely a worse diagnostic, so the ordering in decision 8 was chosen
    instead; it costs nothing on the lowering path.
  - *Accept container return types too.* Rejected as decision 9 explains: it
    widens issue #926's panic rather than shipping a feature. Splitting it out
    as #925 keeps Part 1 diagnostic-complete.
  - *Accept container-typed protocol attributes.* Rejected as decision 10
    explains.
  - *Also lower `frozenset[T]` and `type[T]`.* Rejected: neither has a `Ty`
    variant, and adding one has to clear
    [D-109](./D-109-keep-size-of-ty-at-16-bytes.md)'s 16-byte
    `size_of::<Ty>()` ceiling first — a separate decision.

- Consequences:
  - *Easier:* a container value can now cross a function boundary from real
    Python source. Measured end to end for all four families (`list[int]`,
    `dict[str, int]`, `set[int]`, and a heterogeneous scalar-element
    `tuple[int, bool, float]`), producing byte-identical stdout to CPython
    3.14 — see `tests/issue_918_container_annotations.rs`.
  - *Easier:* the element-type rule now has exactly one definition, so
    widening codegen to, say, `list[str]` is a single-site change that
    correctly relaxes both the literal and the annotation path at once.
  - *Harder:* `annotation_to_ty` grows a second recursion site, so an
    annotation's lowering cost is now proportional to its nesting depth rather
    than constant. Measured, not assumed: the five-replicate paired
    `check_bench` comparison this gate runs (predecessor `12650781` against
    this change) moved the aggregate median from 8463.34 ns to 8511.59 ns, a
    +0.57% delta against the gate's 7.00% threshold, and the absolute D-084
    throughput floor measured 34.36 ms/1000 LOC against its 75 ms budget.
  - *Unchanged, deliberately:* the pre-existing codegen panics for container
    truthiness (`if xs:`) and string conversion (`print(xs)`) are neither
    fixed nor widened. They are reachable today from a bare container literal
    with no annotation at all — verified at identical panic sites
    (`pycc_codegen/src/lib.rs:4024`/`4036`/`4048`/`4074` and
    `:1528`/`1540`/`1580`) with and without an annotation — so this decision
    adds no new route to them.
  - *Irreversible-ish:* `T0053` is now a published diagnostic code, and the
    bare-container `C0001` wording is now a fixture-pinned public contract.
  - *Known inconsistency, recorded rather than fixed:* `type_arg_name_to_ty`
    (`pycc_hir/src/expr.rs`) and `cast_target_ty`/`cast_target_name`
    (`pycc_types/src/class.rs`) still accept scalars only, so a container type
    cannot be written as a PEP 695 class type argument or a `cast()` target
    even though it can now be written as an annotation.
