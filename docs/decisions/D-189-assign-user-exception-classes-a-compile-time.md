---
id: D-189
title: "Assign user exception classes a compile-time type tag and carry the class name on the exception object"
status: accepted
---

## D-189: Assign user exception classes a compile-time type tag and carry the class name on the exception object
- Status: accepted
- Context: [D-173](D-173-exception-propagation-via-per-thread-runtime-state.md)
  propagates a raised exception through per-thread global state holding a
  single `PyExceptionObj`, whose identity is one `u8` type tag drawn from a
  fixed table of seven builtin exception classes. There is no exception
  *instance*: `raise ValueError("x")` never allocates a `PyInstanceObj`, and
  handler selection is an equality test on that tag, with tag 0 (`Exception`)
  treated as a catch-all. [D-188](D-188-synthesize-hirclassdefs-for-the-builtin-exception.md)
  (Part 1 of [#541](https://github.com/rotnov/pycc/issues/541)) then gave the
  seven builtins real `HirClassDef`s, so `class MyError(ValueError):` already
  linearizes an MRO and type-checks. What it did not do is let such a class be
  raised or caught — Part 2 ([#702](https://github.com/rotnov/pycc/issues/702)).
  Three things stood in the way. A user class has no tag. A handler naming a
  base class must catch every subclass, but each subclass would carry a
  *different* tag, so a single-tag equality test cannot express it. And the
  runtime derived an uncaught exception's printed class name from the tag with
  a `match` over exactly those seven constants, which can name no user class at
  all.
- Decision: assign the tag at compile time, in HIR lowering, and move the name
  onto the object.
  1. `HirClassDef` gains `exception_type_tag: Option<u8>`. `lower_checked`
     assigns `7..=255` in module source order to every user-declared class
     whose MRO reaches a seeded builtin exception class; `0..=6` stay reserved
     for the builtins, which are resolved by name and therefore carry `None`
     here. A module declaring more than 249 such classes is rejected with
     `C0001`. Source order is the only ordering stable across runs — the class
     table is a `HashMap` downstream.
  2. `MirExceptHandler::exc_type_tag` widens from `Option<u8>` to
     `Option<Vec<u8>>`: the set of tags a handler accepts, sorted ascending —
     the named class's own tag plus every raisable class whose MRO reaches it.
     Codegen emits one `pycc_rt_exception_type_matches` call per tag, joined by
     `or`. A handler naming `Exception` is the one shortcut: tag 0 is already
     the runtime's catch-all, so it stays a single tag.
  3. `PyExceptionObj` gains `name: *const u8` / `name_len: usize`, filled by
     codegen from a private constant and by the runtime's own `raise_builtin`
     from a `&'static str`. The tag-to-name `match` is deleted.
  4. A user class is raisable only if it has a tag *and* the first `__init__`
     along its MRO is the synthetic `Exception.__init__`. A class with its own
     constructor is rejected with `C0001` naming Part 3, because the message
     string is the only payload the object carries and its fields would be
     silently dropped.
  5. Acceptance in the type checker is keyed **structurally**, on the
     `HirExpr::Call` shape, never on the inferred type. `raise <bound value>`
     stays `T0021`.
- Alternatives:
  - *Runtime subclass table.* Emit a tag-to-parent-tag array and let
    `pycc_rt_exception_type_matches` walk it. Rejected: it moves a decision the
    compiler already knows into runtime data that must be kept in sync across
    the FFI boundary, and it buys nothing — the MRO is fully known at compile
    time, so the OR-chain is a constant-size expansion of information already
    in hand.
  - *Widen the `Ty::Instance` predicate in `check_raise_operand`.* The obvious
    two-line change: accept `Ty::Instance(name)` when `name` is a raisable user
    class. Rejected on memory-safety grounds, and this is the single most
    important line of this entry. `e = MyError("x"); raise e` infers the
    *identical* `Ty::Instance("MyError")` that `raise MyError("boom")` does. A
    type-keyed predicate cannot separate them, so it would also admit the bound
    form — which MIR lowers to `MirExceptionValue::Existing`, and which codegen
    hands to `pycc_rt_exception_raise` as a `*mut PyExceptionObj` while the
    value is really a `*mut PyInstanceObj`. Two unrelated layouts reinterpreted
    as one another: memory corruption, not a diagnostic, and one that a
    passing-looking test suite would not surface.
  - *Materialize a real exception instance now.* The eventual answer, and what
    Part 3 ([#703](https://github.com/rotnov/pycc/issues/703)) does. Rejected
    for Part 2 because it changes the representation D-173 built the whole
    propagation path on, and doing it in the same change as tag assignment
    would make neither bisectable.
  - *Derive the name from the tag with a compiler-emitted table.* Rejected as a
    second synchronization surface for the same fact; the name is a per-object
    constant and costs one pointer plus one length.
- Consequences:
  - Raising and catching user-defined exception classes works end to end, and
    an uncaught one prints its own class name. The PEP 3110 conformance row
    gains proven breadth without moving, since two core gaps remain.
  - The exception hierarchy is capped at 256 types per module (249
    user-declared). Raising the cap means widening the tag past `u8` in
    `PyExceptionObj` and in every runtime entry point that carries one.
  - `except MyError as e:` is `C0001`, not merely unimplemented: binding would
    give the name a `Ty::Instance`, and every consumer of that type reads an
    instance as a `PyInstanceObj`. It stays rejected until Part 3 makes the two
    representations one.
  - `exception_type_tag == None` must never be read as "synthetic". D-188 makes
    `HirModule::seeded_builtin_exception_classes` the sole provenance signal,
    and the builtins — the most raisable classes in the language — carry `None`
    here.
  - `pycc_rt_exception_alloc`'s signature changed, so a stale prebuilt
    `libpycc_rt.a` fails to link rather than silently mismatching.
  - `instantiate_generic_class_methods` hardcodes `exception_type_tag: None` on
    the monomorphized `HirClassDef`, which would be a tag-stripping hazard if a
    generic class could inherit an exception class. It cannot: #432 rejects any
    generic class with base classes during HIR lowering
    (`crates/pycc_hir/src/class.rs:1092`), long before monomorphization runs.
    Verified at this revision — `class MyError[T](Exception)` reports `C0001:
    generic class \`MyError\` with base classes is not supported yet` — and
    locked by
    `a_generic_class_cannot_inherit_from_an_exception_class` in
    `tests/issue_702_user_exceptions.rs`, so relaxing #432 cannot silently mint
    a tagless exception class.
  - A user class rooted at a builtin other than `Exception` (for example
    `class ParseError(ValueError)`) is fully supported: it receives a user tag,
    and a `ValueError` handler widens its tag set to include it. This follows
    from keying raisability on the MRO reaching *any* builtin exception rather
    than on `Exception` specifically.
