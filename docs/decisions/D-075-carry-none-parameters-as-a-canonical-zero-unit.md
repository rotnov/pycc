---
id: D-075
title: "Carry `None` parameters as a canonical zero unit"
status: accepted
---

## D-075: Carry `None` parameters as a canonical zero unit

- Status: accepted (PR #132 Codex review found that the frontend accepted a `None`-typed parameter while codegen panicked merely declaring the function)
- Context: LLVM `void` is a return type, not a legal parameter or storage type. pycc's frontend nevertheless accepts the standard Python annotation `None` for a parameter, and a `None`-returning call or another `None` parameter can supply that argument. Rejecting the program only in codegen violates the checked-program contract and made even an unused `def f(value: None) -> None: return` crash during function declaration.
- Decision: keep `None` function returns as LLVM `void`, and use the canonical LLVM `i8 0` as the unit carrier whenever a `None` value crosses the v0.1 user-function parameter ABI or is held in that parameter's entry slot. MIR's static `Ty::None` is the semantic tag; the identical physical width of `bool` does not make the values interchangeable. Evaluating `return value` in a `None` function preserves expression side effects and emits a void return. Printing or interpolating a `None`-typed parameter materializes Python's `None` text. D-072's explicit rejection of using `print()` itself as a nested expression remains unchanged.
- Alternatives: reject `None` annotations on parameters during HIR lowering (rejected because the standard annotation and unit type are already part of the v0.1 frontend contract); use an empty LLVM struct (rejected because an internal aggregate adds target-ABI complexity without carrying information); implement a general tagged `None` object now (rejected because the singleton carries no payload and D-072 still deliberately excludes `print()`'s result from general expression flow).
- Consequences: every source program accepted solely because it declares or passes a `None` parameter now reaches codegen without a declaration-time panic, and the carrier has one deterministic representation across Tier-1 targets. The representation is an internal ABI detail, not a new user-visible value or a relaxation of D-072.

