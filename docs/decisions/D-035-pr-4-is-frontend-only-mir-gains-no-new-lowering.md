---
id: D-035
title: "PR-4 is frontend-only; MIR gains no new lowering logic"
status: accepted
---

## D-035: PR-4 is frontend-only; MIR gains no new lowering logic

- Status: accepted (PR-4 is the PR that depends on it)
- Context: DELIVERY_PLAN.md's PR table splits "PR-4: Frontend depth" from "PR-5: Codegen depth: full v0.1 feature set (int/float/str/bool, arithmetic, control flow, recursion, f-strings)" -- the *same* feature list PR-4's own row implies growing (via "full v0.1 grammar"). Read literally, PR-4 grows the grammar the *frontend* accepts and type-checks; PR-5 grows what the *backend* can actually compile and run. `pycc check` (CLI_SPEC.md: "frontend only: parse + types + ownership; ruff-fast, no codegen") is the concrete CLI surface that only needs the frontend, confirming this split is intentional, not an oversight.
- Decision: `pycc_hir` grows a real small-IR shape for the full v0.1 grammar. `pycc_mir::build()` is updated only mechanically, so it still compiles against the wider `HirStmt`/`HirExpr` enums (Rust's exhaustive matching forces this) -- every new construct gets an explicit `panic!("pycc_mir: <construct> codegen lands in PR-5")` arm, not real lowering. `pycc build`/`pycc run` therefore keep working on exactly the subset they already support (module-level `print(<i64 literal>)`, zero-arg function definitions/calls) and fail loudly, with a clear message naming the reason, on anything new PR-4's frontend now accepts but PR-5 hasn't implemented lowering for yet.
- Alternatives: extend `pycc_mir`/`pycc_codegen` in lockstep with every new HIR construct so `pycc build` supports the full grammar immediately (rejected -- this is literally PR-5's scope restated, and doing it here would make PR-5 an empty PR while making PR-4 unboundedly large). Build a second, parallel expression tree used only by `pycc check` so `pycc_mir` never needs touching at all (rejected -- creates two divergent ASTs that must be reconciled again in PR-5 anyway, redundant work for no real benefit given the mechanical MIR update is small).
- Consequences: `pycc build`/`pycc run` on any of PR-4's new grammar (arithmetic, control flow, functions with arguments, f-strings, etc.) panics with a clear, PR-5-referencing message rather than silently miscompiling or producing wrong output -- a real, intentional, and temporary gap, not a silent one. Every `tests/slice0.rs` CLI-level test added in PR-4 for new grammar must go through `pycc check`, not `pycc build`/`pycc run`.

