---
id: D-038
title: "A leading underscore marks a top-level function \"private\" for T0001"
status: accepted
---

## D-038: A leading underscore marks a top-level function "private" for T0001

- Status: accepted (PR-4 is the PR that depends on it)
- Context: TYPE_SYSTEM.md's strictness rule 1 requires annotations on "every public function/method," rule 2 says "locals and private helpers" are inferred -- but the document never defines what makes a *module-level function* public or private (there is no Python keyword for this, unlike a class's `_`-prefixed attribute convention, which IS an established Python-ecosystem norm this extends).
- Decision: a module-level function whose name does not start with `_` is public and requires a fully annotated signature (T0001 on any missing parameter or return annotation); a leading `_` marks it private, eligible for local inference instead.
- Alternatives: treat every top-level function as public, always requiring annotations regardless of naming (rejected -- forecloses the "private helper" case TYPE_SYSTEM.md's rule 2 explicitly names, with no way to opt out); require an explicit `pycc.toml`-level allowlist (rejected -- no `pycc.toml` support exists yet in v0.1, per CLI_SPEC.md's own `[project]`/`[build]` sections being unimplemented; premature complexity).
- Consequences: `def _helper(x): ...` (unannotated) type-checks via inference; `def helper(x): ...` (unannotated) raises `T0001`. Revisit if a real `pycc.toml` visibility mechanism is designed later -- this convention becomes the default, not a permanent rule.

