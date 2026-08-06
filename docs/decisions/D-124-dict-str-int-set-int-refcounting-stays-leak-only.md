---
id: D-124
title: "dict[str, int]/set[int] refcounting stays leak-only, extending D-107's exact reasoning to two new heap object types"
status: accepted
---

## D-124: dict[str, int]/set[int] refcounting stays leak-only, extending D-107's exact reasoning to two new heap object types

- Status: accepted
- Context: D-107 chose leak-only refcounting for `list[int]`'s own `PyIntListObj` in v0.2, citing pycc's actual target use (a short-lived, single-shot compiled batch program) as justification for treating an unbounded per-list leak as a bounded-by-process-lifetime resource cost rather than a defect. `PyDictObj`/`PyIntSetObj` (this plan's own two new heap object types) are structurally identical in this respect: nothing about being a dict or a set changes the argument.
- Decision: `PyDictObj`/`PyIntSetObj` get `pycc_rt_dict_incref`/`_decref` and `pycc_rt_int_set_incref`/`_decref` FFI functions (implemented and tested, exactly mirroring `pycc_rt_int_list_incref`/`_decref`'s own unconditional-refcounting-with-free-at-zero shape), but **`pycc_codegen` never calls either pair** in this PR's own scope -- every dict/set allocation lives for the process's lifetime, identically to `list[int]`. `PyDictObj`'s own keys (`*mut PyStrObj` pointers) are stored without incref on insert and without decref ever, for the same leak-only reason -- this additionally sidesteps a subtle double-free risk `d[k] = v`'s update-in-place case would otherwise raise (re-storing a key that compares equal to an already-stored one) without a real design needed to resolve it, since no decref ever happens on the old or new key either way.
- Alternatives considered:
  - Wire full refcounting now for the two new containers even though `list[int]` itself stays leak-only -- rejected: this would make `dict`/`set`'s own memory-lifetime behavior inconsistent with `list`'s, for no user-visible benefit in a batch-compiled-binary target, and duplicates the exact tradeoff D-107 already made once.
- Consequences: identical to D-107's own: a future PR that needs real refcounting for any of `list`/`dict`/`set` should very likely do all three in one pass (the incref/decref call-site wiring at duplicate-reference reads, reassignment cleanup, and scope-exit cleanup is the same shape for all three container types), tracked as a single `docs/ROADMAP.md` follow-up by Task 10 of this plan.

