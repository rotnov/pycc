---
id: D-037
title: "`pycc_testkit`/`tests/conformance/` remain deferred to PR-6"
status: accepted
---

## D-037: `pycc_testkit`/`tests/conformance/` remain deferred to PR-6

- Status: accepted (reaffirms D-018 for PR-4 specifically)
- Context: D-018 already deferred `pycc_testkit` "past PR-1/PR-2... built for real at PR-4/PR-6," leaving genuine ambiguity about which of those two PRs. PYTHON_STANDARDS.md's PEP matrix is still 100% `☐` (unstarted) and DELIVERY_PLAN.md's own task breakdown names PR-6 "Conformance + benchmark gate" -- a dedicated PR for exactly this harness.
- Decision: PR-4 does not create `pycc_testkit` or `tests/conformance/`. It creates only `tests/diagnostics/` (D-036), which DIAGNOSTICS.md and PYTHON_STANDARDS.md's "rejected by design" table already specify as its own, separate concern from PEP conformance fixtures.
- Alternatives: stand up a minimal `pycc_testkit` now too (rejected -- no PEP matrix exists yet for it to check against, the same YAGNI reasoning D-017/D-018 already used; would be structure without function).
- Consequences: PR-6 is where the PEP-by-PEP conformance harness and its `tests/conformance/pyXY/` fixtures actually get built; PR-4's diagnostic fixtures are unaffected by that later work since they live in a separate directory with a separate, already-real purpose.

