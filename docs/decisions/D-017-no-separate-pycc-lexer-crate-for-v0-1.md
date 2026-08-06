---
id: D-017
title: "No separate `pycc_lexer` crate for v0.1"
status: accepted
---

## D-017: No separate `pycc_lexer` crate for v0.1

- Status: accepted (PR-1/PR-2 is the PR that depends on it)
- Context: ARCHITECTURE.md's crate table lists `pycc_lexer`/`pycc_parser`/`pycc_ast` as three crates, describing the end state after D-003 resolves (pycc's own hand-written lexer and parser, v0.6). During the vendored-parser bootstrap phase, `ruff_python_parser` performs lexing internally and does not expose a standalone token stream pycc could wrap even if it wanted to, and nothing downstream in the v0.1 pipeline (`pycc_hir` onward) consumes tokens directly — only the parsed AST.
- Decision: v0.1 creates `pycc_ast` (a thin, stable re-export boundary over `ruff_python_ast`) and `pycc_parser` (wraps `ruff_python_parser::parse_module`), but not `pycc_lexer`.
- Alternatives: an empty pass-through `pycc_lexer` crate now, for table-completeness with ARCHITECTURE.md (rejected as YAGNI — nothing would depend on it, and an empty crate that exists only to match a table is exactly the kind of unnecessary abstraction the project's own engineering conventions warn against).
- Consequences: `pycc_lexer` gets created in v0.6 when D-003's own-parser work gives it a real reason to exist (a token stream something actually consumes); until then, ARCHITECTURE.md's crate table describes a target state, not every crate's exact creation date.

