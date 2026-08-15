---
id: D-172
title: "Exception propagation via global exception state + explicit checks, superseding D-005"
status: accepted
---

## D-172: Exception propagation via global exception state + explicit checks, superseding D-005

- Status: accepted (supersedes [D-005](./D-005-exceptions-native-unwinding-itanium-seh-zero-cost.md))
- Context: D-005's title committed to "native unwinding (Itanium/SEH), zero-cost happy path — not result-codes" but was left `proposed` because verifying that commitment requires cross-platform spikes that are naturally PR-22's own first task, not a documentation-only issue's. The v0.3 design doc's §2 deliberately deferred D-005 rather than flipping it prematurely. Now that PR-22 (#382) is the active task, the verification work has been done:

  - **Itanium vs SEH split is real.** LLVM `invoke`/`landingpad` with `__gxx_personality_v0` covers `x86_64`/`aarch64`-linux and `x86_64`/`aarch64`-darwin (4 of 5 Tier-1 targets). `x86_64-pc-windows-msvc` needs Windows SEH (`__C_specific_handler`, different landing-pad schema). Supporting both in the same codegen pass is a genuine cross-platform engineering cost, not a portability detail.
  - **Rust FFI boundary interaction.** `pycc_rt` uses plain `extern "C"` (not `extern "C-unwind"`) for every runtime function — a deliberate design choice confirmed empirically: a panic unwinding past an `extern "C"` boundary is caught and turned into a process abort (stable since Rust 1.71). Native LLVM unwinding through `pycc_rt` functions would require either changing every `pycc_rt_*` function to `extern "C-unwind"` (relaxing the abort-on-unwind safety property) or introducing a separate set of exception-aware runtime entry points. Both are invasive changes with subtle safety implications.
  - **Generated IR has no unwind tables today.** pycc-generated LLVM IR has no personality routine and no unwind tables. Adding them is a codegen-level change that affects every function, not just functions containing `try` blocks.
  - **Zero-cost claim is unverified for this compiler.** The "zero-cost happy path" is a property of the Itanium/SEH ABIs in the abstract, not of this compiler's actual output. Measuring it requires a working `try`/`except` fixture on all 5 targets — which is what this PR is building. Asserting it before the measurement exists repeats the same unverified-claim pattern D-153 corrected for v0.3's PEP count.

  The cost of native unwinding is real and front-loaded; the benefit (zero-cost happy path) is an optimization that can be layered on once the exception *semantics* are correct and stable.

- Decision: implement exception propagation via a **global exception state + explicit check-and-branch** mechanism, not native unwinding. Specifically:

  1. A global `pycc_rt_exception_active` flag (`i1`) and `pycc_rt_exception_value` slot (opaque pointer to a heap-allocated exception object) live in `pycc_rt`.
  2. `raise ExceptionType(args)` lowers to: allocate the exception object, store it in the global slot, set the active flag, and jump to the current function's exception-exit block.
  3. The compiler tracks which operations can raise (division by zero, subscript out of range, missing key, explicit `raise`, calls to functions that can raise) and inserts a check-and-branch after each: if the flag is set, jump to the current function's exception-exit block.
  4. `try`/`except`/`finally` lowers to LLVM basic blocks with explicit exception-type checks. The `try` body runs; after each potentially-raising operation, the inserted check branches to the `except` handler chain on exception. `finally` runs in both the normal and exceptional exit paths.
  5. Functions that can raise propagate the exception by checking the flag before each `return` and jumping to the exception-exit block if set. The exception-exit block runs any `finally` blocks and then either re-raises (propagates to caller) or returns normally.
  6. If no `try` block catches the exception by the time it reaches `main`'s top level, the runtime prints the exception and exits with code 1.

  This is not zero-cost: every potentially-raising operation has a branch on the happy path. But the branch is highly predictable (almost never taken on the happy path), modern CPUs predict it correctly, and the cost is negligible compared to the actual operation. The mechanism is simple, correct for Python semantics (including `finally` blocks that must run during propagation), and works identically on all 5 Tier-1 targets with no platform-specific unwind machinery.

- Alternatives:
  - **Native unwinding (Itanium/SEH) — D-005's original title.** Rejected for v0.3: the cross-platform cost (two different personality routines, `extern "C-unwind"` migration, unwind tables in every function) is front-loaded and high, while the benefit (zero-cost happy path) is an optimization better layered on after the exception semantics are stable. D-005 is superseded, not rejected outright — a future decision can re-accept native unwinding once the exception model is proven and the optimization is justified by profiling.
  - **setjmp/longjmp.** Rejected: `longjmp` skips intermediate frames, so `finally` blocks in functions between the `raise` site and the `try` block would not execute — incorrect Python semantics. Fixing this requires the same explicit propagation as the chosen approach, making `setjmp`/`longjmp` an unnecessary additional mechanism.
  - **Result codes (return-value-based).** Rejected: changes the ABI of every `pycc_rt_*` function (every return value needs a success/failure tag), which is a larger and more invasive change than a global flag. The global flag approach achieves the same propagation semantics without touching the existing function signatures.

- Consequences:
  - D-005 is superseded. Its `proposed` status is retained as the historical record; D-172 is the active decision.
  - Every function's codegen gains an exception-exit block and a small number of check-and-branch sites. The codegen diff is larger than a native-unwinding approach would be (which could use `invoke`/`landingpad` at fewer sites) but simpler and platform-independent.
  - The happy path has a predictable branch after each potentially-raising operation. This is a documented performance cost, not a zero-cost abstraction. A future ADR can migrate to native unwinding if profiling justifies it.
  - `pycc_rt`'s `extern "C"` ABI is unchanged — the abort-on-unwind safety property is preserved. Runtime functions that can raise set the global flag and return a sentinel value; the caller checks the flag.
  - Exception objects are heap-allocated with a type tag, matching the existing class-instance representation from PR-15. Built-in exception types (`ValueError`, `TypeError`, `KeyError`, `IndexError`, `ZeroDivisionError`, `Exception`) are pre-declared in `pycc_rt` with fixed type tags.
