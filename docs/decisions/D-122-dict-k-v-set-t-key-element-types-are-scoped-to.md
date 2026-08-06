---
id: D-122
title: "dict[K, V]/set[T] key/element types are scoped to existing equality-comparable scalars; exactly one combination gets real codegen"
status: accepted
---

## D-122: dict[K, V]/set[T] key/element types are scoped to existing equality-comparable scalars; exactly one combination gets real codegen

- Status: accepted
- Context: D-121's linear-scan lookup needs an equality comparison between two keys (or two set elements) at every scan step. `pycc_codegen` already implements `CmpOpKind::Eq` for all four scalar types (`Int`, `Float`, `Bool`, `Str`) but has no equality comparison for any container type (`List`/`Dict`/`Set`/`Tuple`) -- comparing two lists/dicts/sets/tuples for equality is not implemented anywhere in this compiler. Mirroring `list[int]`'s own D-105 scope cut 5 ("ship exactly one concrete element type, gate everything else with a clean pre-codegen diagnostic"), this PR ships exactly one key/value combination per container.
- Decision: `dict`'s shipped combination is `dict[str, int]`; `set`'s is `set[int]`. Any other combination (`dict[int, str]`, `set[str]`, `dict[list[int], int]`, etc.) type-checks structurally (`Ty::Dict`/`Ty::Set` are already fully general per D-089) but is rejected before codegen by a new diagnostic (`T0036` for dict, `T0038` for set -- see Task 3/Task 7 of `docs/superpowers/plans/2026-08-01-v0-2-pr11-dict-set.md`), exactly mirroring `list[int]`'s own `T0034`. This also transitively rejects every non-scalar key/element type (no combination containing `List`/`Dict`/`Set`/`Tuple` is the one shipped combination), which is the correct behavior anyway since no equality codegen exists for those types.
- Alternatives considered:
  - Shipping `dict[int, int]`/`set[int]` for symmetry with `list[int]` -- rejected: `dict[str, int]` is a more representative real-world dict shape (string keys are the overwhelmingly common case) and exercises `pycc_rt_str_cmp`'s existing comparison path, giving the dict-insertion-order fixture (D-123) a realistic, readable shape (e.g. `{"a": 1, "b": 2}`) rather than an arbitrary int-keyed one.
  - General hashability inference (accept any type with a derivable equality, à la Rust's `#[derive(PartialEq)]`) -- rejected as far beyond this PR's "thin slice" scope; no user-defined types with `__eq__`/`__hash__` overloading exist in this compiler at all yet.
- Consequences: widening either container's type coverage later needs exactly the same two changes D-105's own Consequences section already named for `list[int]`: (a) relax/remove the `T0036`/`T0038` gate (small, isolated), and (b) add a new `pycc_rt` runtime object plus `pycc_codegen` dispatch arms for that specific combination (the surrounding `pycc_types`/`pycc_mir` inference logic is already fully generic over `Ty::Dict`/`Ty::Set`'s structure and needs no changes either way).

