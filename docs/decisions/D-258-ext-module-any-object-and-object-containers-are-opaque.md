---
id: D-258
title: "In an `--ext` module, `Any`, `object` and object-element container annotations are the opaque CPython object"
status: accepted
---

## D-258: In an `--ext` module, `Any`, `object` and object-element container annotations are the opaque CPython object
- Status: accepted (planned contract, settles [#1285](https://github.com/rotnov/pycc/issues/1285) as "admitted"; no current compiler behaviour is claimed: `T0002` and the `C0003`/`C0001` refusals described below stand until the implementing pull requests land)
- Context: [D-244](./D-244-add-a-hosted-cpython-extension-module-artifact-mode.md) adds the hosted
  `ext` artifact mode (`pycc build --ext`). Its rule 3 binds a foreign import to an opaque `object`:
  `Ty::Object` in `crates/pycc_types`, carried in generated code as an owned `PyObject *`. Since
  [#1278](https://github.com/rotnov/pycc/issues/1278) the from-import form binds the same way. Since Part 1
  of [#1367](https://github.com/rotnov/pycc/issues/1367), an annotation naming a class that a foreign import
  binds resolves to it as well. Two spellings still cannot reach that type:
  - `typing.Any` is refused everywhere with `T0002` "`Any` is not permitted in pycc code outside a declared
    interop boundary" (`crates/pycc_hir/src/func.rs`, `docs/TYPE_SYSTEM.md` strictness rule 3).
  - The builtin name `object` is deliberately unspellable in an annotation (`docs/TYPE_SYSTEM.md`, `object`
    row).

  The #1207 kill-criterion subject is `lark` 1.3.1's `ParserState.feed_token(self, token: Token, is_end:
  bool = False) -> Any`, in `lark/parsers/lalr_parser_state.py`, annotated under
  [D-252](./D-252-admit-annotation-only-additions-to-a-kill-criterion-subject.md) and scoped to the subject
  module by [D-257](./D-257-kill-criterion-compile-scope-is-the-subject-module.md). It needs an answer for
  each of these:
  - its `-> Any` return, line 67. That return is `value_stack[-1]`, an arbitrary result of a user callback,
    so no honest concrete type exists. D-252 admits only annotation *additions*, so replacing `Any` is not
    an admissible fix;
  - `__eq__(self, other)`, line 51, whose only faithful annotation is `object`
    ([#1387](https://github.com/rotnov/pycc/issues/1387));
  - attribute annotations over CPython containers: `state_stack: List[StateT]`, `value_stack: list` and
    `states: Dict[StateT, Dict[str, tuple]]`. Each value is a list or dict that a `lark` caller built and
    passes in. Where the module creates one itself, it starts as `[]` or `[start_state]`.

  [#1285](https://github.com/rotnov/pycc/issues/1285) asked whether `Any` is admitted at the `ext` boundary,
  and how the value crosses it. The orchestrating session resolved the question under
  [D-127](./D-127-autonomous-agent-operation-model.md), after consulting an independent reviewer. This entry
  records the result.
- Decision:
  1. **Scope.** The rules below apply only to modules compiled into an `ext` artifact, that is, by `pycc
     build --ext` (D-244's hosted mode). They cover every module compiled into that artifact: the entry
     module, and any project module linked into it under
     [D-222](./D-222-project-modules-link-at-the-hir-level-into-one.md). The artifact mode is a property of
     the build, not of a module, and a second discriminator would only create a seam where the same source
     line means two things in one artifact. A `native` program (`pycc build` without `--ext`, `pycc run`,
     `pycc check` of such a program) is unchanged: `Any` remains `T0002`, with the same code and message,
     and `object` stays unspellable there unless #1387 decides otherwise for that mode.
  2. **`Any` is the opaque object.** In an `ext` module, `typing.Any` lowers to the type a foreign import
     binds (D-244 rule 3, `Ty::Object`). This holds in a parameter, return, attribute (class-body or
     `self.<attr>`) or local annotation, and as a type argument (rule 4). A value of that type is a strong
     `PyObject *` reference. Its ownership follows `docs/RUNTIME.md`'s rule for `object`, including the
     release protocol [#1092](https://github.com/rotnov/pycc/issues/1092) tracks. At the export boundary it
     crosses **unchanged**, with no conversion in either direction:
     - an `Any` parameter receives the host's object, and an `Any` return hands the host a new reference
       to the object the body produced. So `-> Any` on `ParserState.feed_token` returns the very CPython
       object `value_stack[-1]` holds;
     - every value conforms to `Any`, so D-244 rule 7's non-conforming `TypeError` can never fire on such a
       parameter, and pass-through is the only behaviour available. This decides nothing about a parameter
       annotated with a foreign *class* (`token: Token`). Whether the thunk passes that through or checks
       `isinstance` remains [#1386](https://github.com/rotnov/pycc/issues/1386)'s to decide;
     - the wrapper slot that carries an object, which `collect_exports` refuses today with `C0003`, is one
       mechanism shared with #1386. Whichever pull request lands first builds it, and the other reuses it.

     This settles #1285 as **admitted**.
  3. **`object` means the same type.** In an `ext` module the builtin name `object`, used as an
     annotation, lowers to the same opaque type as `Any`. The two spellings are interchangeable there. This
     answers the spelling question #1387 asks for the `ext` mode only. #1387's implementation is unchanged:
     boxing a native `int`, `float`, `str`, `bool`, pycc instance or container into an `object` slot, and
     `isinstance` narrowing back out of one.
  4. **Object containers are the CPython container.** In an `ext` module, the following annotations also
     lower to the opaque object:
     - a container annotation whose element, key or value type is the object type of rules 2 and 3
       (`list[Any]`, `dict[str, object]`, `tuple[int, Any]`);
     - an unparametrised `list`, `dict`, `tuple` or `set` annotation;
     - `List[...]`, `Dict[...]`, `Tuple[...]` or `Set[...]` (or the lower-case forms) over a foreign class or
       a foreign type variable (`List[StateT]`, `Dict[StateT, Dict[str, tuple]]`).

     One object-typed argument anywhere in the annotation makes the whole value opaque: `tuple[int, Any]` is
     one CPython tuple, not a native struct with one object field. The value is the CPython container
     itself, for example the stacks and the parse table a `lark` caller passes in, so aliasing is preserved:
     a mutation through it is visible to every other holder, as in CPython. A list, dict, tuple or set
     literal assigned to such a slot (`[]`, `[x]`, `{}`) builds a CPython container (`PyList_New`,
     `PyDict_New` and so on), not a pycc-native one. A native container whose element types are all
     native, including a pycc class (`list[int]`, `dict[str, int]`, `List[MyPyccClass]`), is unaffected
     and stays under [D-105](./D-105-v0-2-s-list-t-thin-slice-scope-cuts-and-runtime.md)
     and [D-122](./D-122-dict-k-v-set-t-key-element-types-are-scoped-to.md). Those decisions govern native
     containers, and this rule neither lifts nor contradicts their element-type scopes. A pycc-native
     container value passed where an object container is expected has no conversion, and it is refused at
     compile time. Copying it into a CPython container would break the aliasing this rule exists to keep.
  5. **Operations lower to the C-API.** Every operation on a value of the opaque type lowers to the
     corresponding CPython C-API call, and CPython's own exceptions propagate. The operations are:
     - identity, `is` / `is not` (`Py_Is`);
     - rich comparison, `==`, `!=`, `<` and the rest (`PyObject_RichCompare`);
     - subscript load, store and delete, slices included (`PyObject_GetItem` / `SetItem` / `DelItem` with
       a slice object);
     - a call with positional and keyword arguments (`PyObject_Call`), including a call on a subscript
       result (`callbacks[k](v)`);
     - `isinstance` (`PyObject_IsInstance`) and `type(x)` (`Py_TYPE`);
     - truth testing, iteration and method calls;
     - `raise` of an object that is an exception instance.

     A pycc-native operand of such an operation (an `int` index, a `str` key, a native argument) is boxed
     with the existing boundary conversions. In the other direction an object reaches a native-typed slot
     only through an explicit conversion (`int(o)`, `float(o)`, `str(o)`, `bool(o)`, D-244's Part 4
     amendments) or through `isinstance` narrowing (#1387). Assigning it to a native slot directly stays
     refused (`I0401`/`I0404`), as D-244 rule 3 states. The list is implemented incrementally:
     [#1371](https://github.com/rotnov/pycc/issues/1371)'s series, and the issues the #1207 subject-module
     frontier table in `docs/TESTING.md` links (#1095, #1255, #1333, #891 and others). An operation not yet
     implemented keeps its current diagnostic until its part lands.
  6. **Implementation seam.** `pycc_hir` has no artifact-mode awareness, as the comment on the
     `memoryview` arm next to `T0002` in `crates/pycc_hir/src/func.rs` records. The implementing pull
     request therefore follows the `memoryview` precedent: the annotation lowers unconditionally, and the
     mode-dependent refusal lives where the mode is known (`src/memoryview_mode.rs`, `src/ext_build.rs`).
     For `Any` that means a `native` build must still report `T0002` with its current code, message and
     span. The refusal moves to the mode-aware layer, and its meaning does not change. Any other shape
     that keeps `native` output byte-identical is admissible.
- Alternatives:
  - *Refuse `Any`, and record #1285 as a standing blocker at the 2026-10-22 deadline.* Rejected.
    The subject returns an arbitrary callback result, so no honest concrete type exists, and D-252 admits
    only annotation additions, so the annotation cannot be replaced. The opaque object already exists for
    foreign imports. Refusing to spell it in an `ext` module would record a by-choice refusal as a verdict
    on the compiler, which is the attribution error D-247 and D-257 rule 2 were written to prevent.
  - *Admit `Any` on returns only.* Rejected. The same opaque representation already exists for foreign
    imports, so a return-only rule saves no machinery. The subject's containers and `__eq__(self, other:
    object)` need the same answer for parameters and attributes. A position-limited rule would only move
    the refusal one line down.
  - *Lift D-105/D-122 to object element types inside native containers* (a native `list[object]` holding
    boxed references). Rejected for this sprint. The subject's containers come from CPython callers, so a
    native container would require a copy at every ingress. That copy is slower than operating on the
    CPython container, and it is unfaithful: a callback that mutates the list it was handed would mutate
    the copy, not the caller's list. A native object-element container may still be decided later for
    containers pycc itself creates and never shares. That would be a separate decision.
  - *Treat `Any` as PEP 484's gradual type* (assignable to and from every type without a check).
    Rejected. It would let an object flow silently into a native `int` slot, which D-244 rule 3's `I0401`
    forbids, and pycc's typed lowering has no representation to receive it. `Any` here is a top type
    with an explicit way out (rule 5), not a hole in the checker.
- Consequences:
  - **Deviations.** Code typed with the object type gets no static checking beyond the type itself.
    Misuse is whatever CPython raises at run time (`AttributeError`, `TypeError`, ...), exactly as the
    interpreter would raise it. Unlike mypy's gradual `Any`, an object value does not pass silently into a
    native-typed slot; it needs rule 5's explicit conversion. That is stricter than CPython, which does not
    enforce annotations at all, and it is recorded as a deliberate deviation in the same spirit as D-244
    rule 7.
  - **Performance and the kill criterion.** Object-typed code runs at C-API speed: each operation is a
    call into CPython, with no typed lowering to gain from. `feed_token`'s loop body works almost entirely
    on objects pycc does not own, as D-257 rule 6 already records, so under this decision it runs almost
    entirely on that path. This bears directly on D-244 rule 6's ≥ 5× kill criterion, which is judged on
    2026-10-22 and is not changed here. D-244's 2026-09-16 amendments also bar evaluating the criterion on
    a hot function containing foreign loads until #1092's release protocol lands, and an object-typed
    `feed_token` is exactly such a function. A compile under this decision makes a timed run *possible*;
    it does not predict that the bet is won.
  - **D-257's scoping stands.** Under rule 1, the two `T0002`s inside `lark/utils.py` become admissible
    in an in-tree `--ext` build once this decision is implemented. D-257's subject-module reading still
    stands, because the two `T0001`s in that closure module keep the whole-closure reading unreachable on
    their own (D-257 rule 2).
  - **Documentation and diagnostics.** The implementing pull request updates these together with the code:
    - `docs/TYPE_SYSTEM.md` strictness rule 3 and its `object` row;
    - `docs/DIAGNOSTICS.md`'s `T0002` row and `pycc explain T0002` (`crates/pycc_diag/src/explain.rs`),
      which must then describe `T0002` as a `native`-mode refusal;
    - `docs/PYTHON_STANDARDS.md`'s `Any` row;
    - the #1207 frontier table in `docs/TESTING.md`.

    Until then `docs/TYPE_SYSTEM.md` and `docs/TESTING.md` mark the rule as decided and not yet
    implemented.
  - **What becomes harder.** The `ext` mode now has two container representations behind similar
    spellings: `list[int]` is native and `list[Any]` is a CPython list. A diagnostic that mixes them
    (for example a native `list[int]` passed to a `list` parameter) must name both, so the reader can see
    which one was meant.
