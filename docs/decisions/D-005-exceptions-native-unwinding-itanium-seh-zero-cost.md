---
id: D-005
title: "Exceptions: native unwinding (Itanium/SEH), zero-cost happy path — not result-codes"
status: superseded
---

# D-005

Index-only: no long-form entry recorded yet.

Exceptions: native unwinding (Itanium/SEH), zero-cost happy path — not result-codes

**Superseded by [D-172](./D-172-exception-propagation-via-global-state-superseding-d-005.md)** — v0.3 ships exception propagation via global exception state + explicit check-and-branch instead of native unwinding. D-005's native-unwinding approach remains a viable future optimization once the exception model is stable and profiling justifies the cross-platform unwinding infrastructure cost.
