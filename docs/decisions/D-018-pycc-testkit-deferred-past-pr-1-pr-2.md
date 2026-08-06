---
id: D-018
title: "`pycc_testkit` deferred past PR-1/PR-2"
status: proposed
---

## D-018: `pycc_testkit` deferred past PR-1/PR-2

- Status: proposed (graduates to accepted when `pycc_testkit` is actually created)
- Context: DELIVERY_PLAN.md's original "v0.1 crate scope" and "Testing scope for v0.1" sections listed `pycc_testkit` as built alongside the other 8 crates, with Layer 2 (the conformance harness) starting "as early as slice 0" -- but PR-1/PR-2 never created it (flagged by repo audit as an undocumented deviation, unlike `pycc_lexer`'s reasoned D-017). TESTING.md's actual spec for `pycc_testkit` is a real harness: `.py` fixtures under `tests/conformance/pyXY/` with PEP/category/milestone header comments, diffed against a pinned CPython 3.14 reference output, flipping ✅ marks in PYTHON_STANDARDS.md. None of that exists yet -- PYTHON_STANDARDS.md's matrix is still 100% unstarted (☐), so there is no PEP-level conformance tracking for a harness to drive yet.
- Decision: PR-1/PR-2 use a hand-written `tests/slice0.rs` integration test instead -- it compiles fixtures through the real `pycc` binary and asserts exact stdout, which is a genuine (if narrow) proof that the pipeline works end to end, but it is not TESTING.md's Layer 2 harness. `pycc_testkit` gets built for real once PR-4/PR-6 gives it PEPs to actually track (PR-6 is where DELIVERY_PLAN.md already schedules the first real conformance run: fib + mandelbrot-ascii vs. CPython on all 5 targets).
- Alternatives: scaffold a minimal `pycc_testkit` crate now, even with nothing real for it to do (rejected as premature -- there's no PEP matrix yet for it to check against, so it would be structure without function, the same YAGNI concern D-017 raised about `pycc_lexer`); keep DELIVERY_PLAN.md's "as early as slice 0" wording as-is and treat `tests/slice0.rs` as satisfying it (rejected -- that wording specifically promises TESTING.md's Layer 2 shape, which `tests/slice0.rs` doesn't have, and leaving the mismatch undocumented is exactly the kind of thing this decisions log exists to prevent).
- Consequences: DELIVERY_PLAN.md's "Testing scope for v0.1" section is corrected to say Layer 2 starts at PR-4/PR-6, not slice 0. `tests/slice0.rs` stays as ad hoc coverage for the two named PR-2 fixtures specifically, not a stand-in for the conformance harness.

