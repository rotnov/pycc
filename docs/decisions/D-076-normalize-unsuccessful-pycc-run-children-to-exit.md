---
id: D-076
title: "Normalize unsuccessful `pycc run` children to exit 101"
status: accepted
---

## D-076: Normalize unsuccessful `pycc run` children to exit 101

- Status: accepted (PR #132 Codex review found that a generated runtime panic terminated by signal and surfaced as exit 1 on Unix)
- Context: CLI_SPEC.md promises exit 101 for a compiled-program panic or uncaught runtime failure. `pycc run` previously converted the child's `ExitStatus::code()` directly and substituted 1 when no code existed. Unix reports signal termination with no code, while Windows can report an abort status outside the portable one-byte range, so the implementation leaked platform process details and contradicted the stable CLI contract.
- Decision: generated programs in the current CLI have no user-controlled non-zero exit status. `pycc run` therefore returns 0 only when the child succeeds and maps every other child termination to 101, regardless of whether the platform represents it as a normal non-zero code, signal, exception, or abort status. Build and invocation failures retain their existing compiler-owned exit codes because they occur before child execution.
- Alternatives: preserve ordinary child codes and special-case Unix signals (rejected because no current language surface can request those codes and the result would remain platform-dependent); map only missing codes to 101 (rejected because Windows abort statuses would still be truncated); make every runtime failure unwind back through the compiler process now (deferred until Python exception objects and runtime propagation exist).
- Consequences: `pycc run` now fulfills one cross-platform exit contract for runtime panics and traps; callers need not decode platform-specific child status. A future explicit `sys.exit(n)` feature must supersede this decision with a documented rule for user-requested statuses.

