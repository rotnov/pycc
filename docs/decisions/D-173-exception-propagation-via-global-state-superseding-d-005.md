---
id: D-173
title: "Exception propagation via explicit per-thread state, superseding D-005"
status: accepted
---

## D-173: Exception propagation via explicit per-thread state, superseding D-005

- Status: accepted (supersedes [D-005](./D-005-exceptions-native-unwinding-itanium-seh-zero-cost.md))
- Context: D-005's title committed to "native unwinding (Itanium/SEH), zero-cost happy path — not result-codes" but was left `proposed` because that commitment required cross-platform work that did not yet exist. PR-22 (#382) needs correct exception semantics before that optimization is justified. Its implementation review established these constraints:

  - **Itanium vs SEH is a real implementation split.** Unix-like targets and Windows use different exception-handling ABIs and LLVM IR shapes. Supporting both in the same initial codegen pass is a genuine cross-platform engineering cost, not a portability detail.
  - **Rust FFI boundary interaction.** `pycc_rt` uses plain `extern "C"`, not `extern "C-unwind"`. Native unwinding through runtime entry points would require an ABI change or a separate exception-aware boundary, both outside this thin slice.
  - **Generated IR has no unwind tables today.** pycc-generated LLVM IR has no personality routine and no unwind tables. Adding them is a codegen-level change that affects every function, not just functions containing `try` blocks.
  - **Zero-cost claim is unverified for this compiler.** The "zero-cost happy path" is a property of the Itanium/SEH ABIs in the abstract, not of this compiler's actual output. Measuring it requires a working `try`/`except` fixture on all 5 targets — which is what this PR is building. Asserting it before the measurement exists repeats the same unverified-claim pattern D-153 corrected for v0.3's PEP count.

  The cost of native unwinding is real and front-loaded; the benefit (zero-cost happy path) is an optimization that can be layered on once the exception *semantics* are correct and stable.

- Decision: implement exception propagation via **explicit per-thread runtime state + check-and-branch**, not native unwinding. Specifically:

  1. A per-thread runtime state stores an `i8` active flag and an opaque pointer to the pending heap-allocated exception object. The exported `pycc_rt_exception_active` and `pycc_rt_exception_value` functions read that state.
  2. `raise ExceptionType(args)` lowers to: allocate the exception object, store it in the per-thread state, set the active flag, and transfer to the nearest exception target.
  3. Codegen conservatively checks the flag after recursively evaluated expressions and emitted statements. If it is set, control transfers before a later operand, argument, statement, or other visible effect can run.
  4. `try`/`except`/`finally` lowers to LLVM basic blocks with explicit exception-type checks. The `try` body runs; after each potentially-raising operation, the inserted check branches to the `except` handler chain on exception. `finally` runs in both the normal and exceptional exit paths.
  5. Every generated user function has an exception-exit block. It returns a neutral ABI value while leaving the runtime flag set; the caller checks the flag before consuming the value or evaluating another effect. Enclosing `finally` blocks run before that exit.
  6. If no `try` block catches the exception by the time it reaches `main`'s top level, the runtime prints the exception and exits with code 1.

  This is not zero-cost: conservative checks add happy-path branches. The design avoids platform-specific unwind machinery, but no performance or five-target portability claim is made until the ordinary CI matrix measures the merged implementation.

- Alternatives:
  - **Native unwinding (Itanium/SEH) — D-005's original title.** Rejected for v0.3: the cross-platform cost (two different personality routines, `extern "C-unwind"` migration, unwind tables in every function) is front-loaded and high, while the benefit (zero-cost happy path) is an optimization better layered on after the exception semantics are stable. D-005 is superseded, not rejected outright — a future decision can re-accept native unwinding once the exception model is proven and the optimization is justified by profiling.
  - **setjmp/longjmp.** Rejected: `longjmp` skips intermediate frames, so `finally` blocks in functions between the `raise` site and the `try` block would not execute — incorrect Python semantics. Fixing this requires the same explicit propagation as the chosen approach, making `setjmp`/`longjmp` an unnecessary additional mechanism.
  - **Result codes (return-value-based).** Rejected: changes the ABI of every `pycc_rt_*` function (every return value needs a success/failure tag), which is a larger and more invasive change than a global flag. The global flag approach achieves the same propagation semantics without touching the existing function signatures.

- Consequences:
  - D-005 is superseded. Its `proposed` status is retained as the historical record; D-173 is the active decision.
  - Every generated user function gains an exception-exit block, and expression/statement emission gains conservative check-and-branch sites. The mechanism does not depend on a platform unwind ABI.
  - The happy path pays for those checks. This is a documented, presently unmeasured performance cost, not a zero-cost abstraction. A future ADR can migrate to native unwinding if profiling justifies it.
  - `pycc_rt`'s `extern "C"` ABI is unchanged — the abort-on-unwind safety property is preserved. Runtime functions that can raise set the global flag and return a sentinel value; the caller checks the flag.
  - Exception objects use a dedicated leak-only heap allocation with a fixed builtin type tag, message pointer, explicit-cause pointer, and reserved implicit-context pointer. The type checker recognizes unshadowed builtin exception names directly at `raise`/`except` sites and represents handler bindings as `Ty::Instance`; the runtime uses tags for matching.
  - Pending state is thread-local, so compiler/runtime tests and future generated threads cannot overwrite another thread's exception. Generated programs themselves remain single-threaded in this milestone.
  - Implicit `__context__`, `raise ... from None`, traceback frames, custom exception classes, and handler-binding deletion after `except` remain outside this thin slice.
