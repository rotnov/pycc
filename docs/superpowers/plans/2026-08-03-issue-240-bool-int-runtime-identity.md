# Issue #240: Preserve `bool` identity at `int` boundaries

## Status and priority

Issue [#240](https://github.com/rotnov/pycc/issues/240) is the selected P1.
It is a silent wrong-code defect: accepted programs complete successfully but
render an original `True`/`False` as `1`/`0` after an `int`-typed storage or ABI
boundary. That ranks ahead of explicit unsupported-operation failures and
diagnostic-only gaps. The only open pull request at task start was #313; it
changes documentation/session-log structure but does not touch MIR, codegen,
the runtime, or tests for this defect. Its `ROADMAP.md`/session-log edits remain
a documentation-conflict risk to recheck before publishing.

The exact task base is `aaa2e54aaa70d77734cbe0fa8004fc5243daca0d`.
Reproduction on that commit confirmed wrong output for annotated initialization,
reassignment into an existing `int` binding, `int` parameters and returns,
nested forwarding, f-string observation, `list[int].append(True)` followed by
`pop()`, and a `dict[str, int]` update followed by a read. A plain `bool` still
prints correctly, and `True + 0` correctly produces the ordinary integer `1`.

## Semantic contract

Python's `bool` is an `int` subtype for compatibility and numeric operations,
but an annotation, assignment, argument, or return boundary does not construct
a new integer object. The original runtime identity must survive until a real
numeric operation consumes the value. Therefore:

- storage, parameter, return, forwarding, tuple fields, and supported
  `list[int]`/`dict[str, int]`/`set[int]` element or value boundaries preserve
  whether an accepted `int`-compatible value originated as `bool`;
- print and f-string conversion observe `True`/`False` after those boundaries;
- arithmetic, ordering/equality, truthiness, `float(...)`, indices, slice
  bounds, and every `range` start/stop/step position consume an
  identity-carrying boolean numerically as `0`/`1` where CPython does;
- a numeric result is an ordinary `int`, so `True + 0` remains `1`;
- `range(True, 3)` yields ordinary integer targets `1`, `2`, matching CPython;
- the existing honest rejection of bigint values entering the current
  `list[int]`/`dict[str, int]`/`set[int]` storage subset remains in force.

## Representation decision

Keep D-061's one-word LLVM/C ABI. Extend the encoded `i64` domain rather than
introducing a cross-platform aggregate ABI:

- odd values remain D-061 tagged small integers: `(n << 1) | 1`;
- aligned heap-bigint pointers retain low bits `00`;
- `2` (`...0010`) represents an original `False` in an `int`-compatible slot;
- `6` (`...0110`) represents an original `True` in an `int`-compatible slot.

`BigIntObj`'s alignment must be asserted to be at least four bytes, which makes
the `...10` marker class disjoint from every pointer produced by
`Box::into_raw`; the build must also assert that pointers fit the existing
64-bit carrier. Classification is fail-closed and centralized: odd is a
smallint, exact `2`/`6` is a boolean marker, nonzero `...00` is a bigint
pointer, and every other word (including zero or an unrecognized `...10`
pattern) panics deterministically before any pointer cast. Allocation-level
tests prove real bigint pointers cannot collide with markers. Standalone
`Ty::Bool` remains the existing unboxed LLVM `i8`; the markers are used only
when that value crosses an accepted `Ty::Int` boundary.

Runtime numeric helpers decode the two markers as numeric `0`/`1`. They return
ordinary tagged integers after arithmetic. Runtime string conversion and the
legacy integer print helper recognize the markers first and render
`False`/`True`. Every path that may dereference a bigint pointer must classify
the marker class before the pointer cast.

Container element/value slots migrate from D-106's raw integer payload to the
same encoded one-word representation. Lengths, loop counters, indices, and
slice bounds remain raw implementation integers. Codegen still calls the
checked untag helper before storing a container value, but uses that call only
as the existing bigint guard and stores the original encoded value. Reads and
`pop()` return the encoded value unchanged. Set dedup compares decoded numeric
values while retaining the first inserted encoding, matching Python's
`1 == True` behavior without replacing the first object's identity.

This requires a new accepted ADR that narrowly supersedes D-061's two-way
smallint/pointer classification, D-074's representation-changing widening,
and D-106's raw container-element representation. The ABI width, bigint
pointer model, standalone-bool representation, and existing bigint container
scope cut remain unchanged.

## MIR and codegen changes

1. Add an explicit MIR `IntBoundary` expression for annotated-assignment
   lowering. It has static `Ty::Int` but evaluates its child without performing
   arithmetic, then applies the identity-preserving bool-to-int encoding. This
   replaces annotated assignment's synthetic `value + 0`, while retaining an
   `int` target type for binding collection and slot allocation. Reassignment,
   call-argument, and return boundaries remain explicit codegen coercion sites
   and receive independent tests; this node does not claim to model every
   boundary universally.
2. Change the shared bool-to-int codegen helper to construct the identity
   markers. Every existing numeric consumer already funnels through runtime
   integer helpers, which will decode markers numerically.
3. Keep assignment, call-argument, and return coercion centralized in
   `coerce_scalar_to_type`; the same operation must cover module/function
   storage and ABI edges.
4. Normalize every `range` operand to an ordinary tagged integer before it is
   used as an induction phi value: identity-aware encode, checked numeric
   untag, then ordinary smallint retag. The shared range-operand helper must
   make this true for direct loops and the list/set/dict comprehension
   emitters, so the first visible target is never a boolean marker.
5. Store encoded values unchanged in list/dict/set payloads and return them
   unchanged on reads/iteration/pop/get. Retain checked numeric untagging only
   for actual raw positions such as indices and slice bounds, plus a
   validation-only call before container storage to preserve the bigint gate.
6. Apply an explicit conversion matrix. Encoded reads stop retagging at list
   subscript/pop, dict subscript/get/default, list/set iteration, and list/set
   sources in all three comprehension emitters. Encoded writes validate but
   pass the original word at list/dict/set literals, append/add/dict-set,
   dict-get defaults, and every comprehension insertion branch. Raw-to-tagged
   conversion remains only for user-visible lengths; checked untagging remains
   for indices, slice bounds, range normalization, and validation-only storage
   guards.
7. Audit every encoded-int conversion, checked-untag, raw-retag, direct
   load/store, and range call site. Rename helpers/comments where their old
   raw/tagged terminology would become false.

## Test-first validation

Add focused failing tests before each implementation slice:

1. MIR tests: annotated `bool` under `int` lowers to `IntBoundary`, not an
   arithmetic `BinOp`, and still allocates/binds `Ty::Int` storage.
2. Runtime tests: marker disjointness/alignment/pointer width, invalid-marker
   rejection, bool string rendering, truthiness, comparison, numeric
   conversion, add/sub/mul/div/mod/pow, range consumption, checked untagging,
   and no regressions in bigint paths. Exercise both markers in both operand
   positions where ordering matters, marker-plus-bigint and bigint-plus-marker
   in supported add/subtract, and prove every arithmetic result is an ordinary
   odd smallint rather than an identity marker.
3. Container runtime tests: encoded list round trips and slicing, dict
   update/get/default identity, and set numeric dedup preserving first
   encoding for int-first and dynamically-bool-first `0`/`False` and
   `1`/`True` cases, including iteration and comprehension-produced sets.
4. Codegen tests: annotated assignment, reassignment, parameter, return,
   nested forwarding, print/f-string, tuple forwarding where reachable,
   list append/pop, dict update/read/default, set dedup/iteration, and
   numeric-operation controls.
5. Public CLI differential tests against pinned CPython 3.14.6 in debug and
   release for the issue's four original shapes plus nested/f-string,
   containers, and the complete accepted range matrix: `False`/`True` in
   start, stop, and step positions; zero-step failure; direct target
   observation; and list/set/dict range comprehensions.
6. Run crate tests while iterating, then the 100% line/region coverage gate,
   full workspace tests including ignored conformance tests, Python test
   discovery, clippy, docs, policy/roadmap validators, alpha evals, throughput,
   and the pinned independent deep-review loop until clean.

## Documentation and publication

Update `docs/DECISIONS.md`, `docs/SPEC.md`, `docs/TYPE_SYSTEM.md`,
`docs/RUNTIME.md`, `docs/ARCHITECTURE.md`, and `docs/ROADMAP.md` in the same
change. Update the live session handoff path that exists after refreshing
`main`; do not assume #313's unmerged session decomposition. Recheck every open
PR, the exact default-branch commit, review threads, and required checks before
publishing and before merge. Deliver through a draft pull request and protected
`main`; remove #240's `in progress` label only after exact post-merge local and
push-workflow verification.

## Non-goals

- No aggregate `{tag, payload}` ABI and no generic object header.
- No new `type()`/`isinstance()` surface.
- No bigint container support or bigint comparison expansion.
- No change to standalone `Ty::Bool`'s `i8` representation.
- No attempt to preserve `bool` identity after a real numeric operation.
- No unrelated definite-assignment, inference, ownership, or exception work.
