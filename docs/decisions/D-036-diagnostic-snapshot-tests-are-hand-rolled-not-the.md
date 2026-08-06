---
id: D-036
title: "Diagnostic snapshot tests are hand-rolled, not the `insta` crate"
status: accepted
---

## D-036: Diagnostic snapshot tests are hand-rolled, not the `insta` crate

- Status: accepted (PR-4 is the PR that depends on it)
- Context: TESTING.md's Layer 3 row says diagnostic fixtures use "insta-style snapshots." DELIVERY_PLAN.md's PR-4 row says "first diagnostic codes with snapshot tests." Neither document requires the literal `insta` crate; "insta-style" describes comparing rendered output against a checked-in expected file and failing on any diff, which `std::fs::read_to_string` + `assert_eq!` already does without a new dependency.
- Decision: each `tests/diagnostics/dNNNN_slug.py` fixture pairs with a `tests/diagnostics/dNNNN_slug.expected.txt` file holding the exact expected human-format diagnostic output (CLI_SPEC.md's format). The test harness (`tests/diagnostics_test.rs`) runs `pycc check` on the fixture and asserts the captured stdout equals the expected file's contents exactly.
- Alternatives: add `insta` as a dependency now (rejected -- `insta`'s own workflow (`.snap.new` files, `cargo insta review`) is a genuine ergonomic win at scale, but is a new external dependency this PR doesn't need to introduce yet; revisit if/when the fixture count grows large enough that manual maintenance becomes the bottleneck, as its own new ADR entry).
- Consequences: adding a new diagnostic fixture means hand-writing its exact expected output once, verified by running `pycc check` on it and copying the real output in (never hand-typed from imagination) -- slightly more manual than `insta`'s auto-generate-and-review flow, acceptable at this fixture count.

