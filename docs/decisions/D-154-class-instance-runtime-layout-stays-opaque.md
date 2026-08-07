---
id: D-154
title: "Class instance runtime layout stays opaque with FFI-only accessors (#385, Part 1 of #375)"
status: accepted
---

## D-154: Class instance runtime layout stays opaque with FFI-only accessors (#385, Part 1 of #375)

- Status: accepted
- Context: #385 (Part 1 of #375) adds pycc's first user-defined class model — a single class, no
  inheritance, with instance attributes set in `__init__` and plain instance methods. The issue
  and `docs/TYPE_SYSTEM.md` already settle instances as heap-pointer values, matching the
  existing `list`/`dict`/`set` pattern rather than by-value structs — that choice is not this
  entry's job to restate. What was still open, mirroring D-089's role for the recursive `Ty`
  representation, is one concrete fork: does the runtime instance object stay opaque with
  FFI-only accessors (matching `PyIntListObj`/`PyDictObj`/`PyIntSetObj` in `crates/pycc_rt`, all
  deliberately not `#[repr(C)]`), or does codegen read/write attribute slots directly via LLVM
  `GEP` against a fixed, `#[repr(C)]` layout?
- Decision (opaque accessors, not direct `GEP`): the runtime instance object
  (`PyInstanceObj`, `crates/pycc_rt/src/instance.rs`) is an ordinary Rust struct with no
  `#[repr(C)]` attribute, holding its attribute slots behind a `Cell<Vec<i64>>`. Codegen
  (`crates/pycc_codegen/src/lib.rs`) never reads or writes instance memory directly; it only
  calls three `extern "C"` FFI entry points: `pycc_rt_instance_new(slot_count: i64) -> *mut
  PyInstanceObj`, `pycc_rt_instance_get_slot(instance, slot) -> i64`, and
  `pycc_rt_instance_set_slot(instance, slot, value)`. This is a one-line continuation of the
  pattern every other heap-object `Ty` variant in this codebase already uses (`PyIntListObj`,
  `PyDictObj`, `PyIntSetObj` are all opaque-with-accessors, not `#[repr(C)]`), needs no
  `#[repr(C)]` layout-stability commitment this early in the class model's life — before
  inheritance, `@property`, or `__slots__` semantics are decided in a later PR — and keeps field
  ordering/alignment questions internal to `pycc_rt`'s own struct definition rather than
  promoting them to a cross-crate ABI contract that codegen would need to track.
- Decision (attribute slot storage shape): each declared attribute occupies one `i64` word in a
  flat `Vec<i64>`, in first-`__init__`-assignment source order — matching
  `docs/TYPE_SYSTEM.md`'s "`__slots__` semantics implicit" framing for the compiled subset. A
  scalar value is encoded into its slot word: `int`/`bool` stored directly (`bool` truncated to
  `i8` on read, zero-extended back to the word on write, mirroring the existing tagged-int
  convention elsewhere in `pycc_codegen`), `float` bit-cast to/from `i64`, and `str`/instance
  pointers reinterpreted as `i64` via `inttoptr`/`ptrtoint`. Attribute-name-to-slot-index
  resolution happens entirely at compile time, from the `HirModule`-level `class_defs` side
  table (`crates/pycc_hir/src/class.rs`'s `HirClassDef.attrs`) — there is no runtime
  string-keyed lookup anywhere in this path.
- Decision (method dispatch): a method call resolves to a compile-time-known function pointer —
  static dispatch per D-006's framing for ordinary classes, with the method name mangled as
  `<ClassName>.<method_name>` (a `.` separator, which can never appear in a real Python
  identifier, following the `0gen_`-prefix precedent already used for generic-instantiation
  mangling). No vtable exists in this PR; inheritance and any future dynamic-dispatch question
  is out of scope here and left to whichever later PR adds inheritance.
- Consequences: `crates/pycc_rt/src/instance.rs` is a new, self-contained module exposing only
  the three FFI functions above plus the internal `PyInstanceObj` struct and its private
  helpers; nothing about its internal field layout is observable from `pycc_codegen` or any
  other crate. Adding a future per-instance capability (a real refcount field, a class-identity
  tag for `isinstance`/dynamic dispatch, `__slots__` enforcement) is a purely internal change to
  this one file and its three accessors — no cross-crate signature changes, no codegen changes
  beyond calling a possibly-new accessor. The cost of this choice is one extra FFI call per
  attribute read/write/instantiation (versus a direct in-IR `GEP`), which this project accepts
  as consistent with every other heap-object container's existing performance profile — no
  profiling data currently suggests this is a bottleneck. Direct-`GEP` layout access is
  explicitly not adopted and is a strict subset of what an opaque accessor could grow into
  later, so choosing GEP now would have been the harder-to-reverse choice.
- Alternatives: direct LLVM `GEP` access against a fixed `#[repr(C)]` struct layout (rejected —
  forces a memory-layout commitment across the `pycc_codegen`/`pycc_rt` boundary before
  inheritance, `@property`, or `__slots__` semantics exist to justify freezing it, and breaks
  the established opaque-accessor convention every other heap-object `Ty` variant follows); a
  per-attribute-typed struct field instead of a uniform `i64` slot word (rejected — would need a
  distinct FFI accessor per attribute type, multiplying the runtime surface for no benefit this
  early, when every other scalar-carrying path in `pycc_codegen`/`pycc_rt` already uses a
  uniform tagged-word encoding); a real reference-counted `rc` field on `PyInstanceObj`
  (rejected for this PR — mirrors D-107/D-124's existing leak-only container policy, which this
  entry extends to instances rather than special-casing them; a genuine incref/decref scheme
  remains a distinct, not-yet-scheduled follow-up for every heap-object `Ty` variant, not just
  instances). This entry scopes strictly to instance layout — the class-body
  execution-order/redefinition-binding scheme is #386's own design work, and generic
  classes/`Self` are #385's own sibling Part 3; neither is decided here.
