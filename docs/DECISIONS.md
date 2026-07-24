# pycc Design Decisions (ADR log)

Format: one entry per irreversible-ish call. Statuses: `proposed` → `accepted` → (`superseded by D-xxx`). Changing an accepted decision requires a new entry, not an edit.

| ID | Decision | Status |
|---|---|---|
| D-001 | `int` = hybrid: `i64` fast path, overflow promotes to heap bigint. CPython semantics preserved; `--int native` opt-in deviation for max speed | proposed |
| D-002 | Backend: LLVM (inkwell) as the only v1 backend. Cranelift for debug builds = post-1.0 experiment | proposed |
| D-003 | Parser: vendor `ruff_python_parser` for v0.1–0.5 velocity; replace with own parser in v0.6 (grammar-coverage gate proves parity) | proposed |
| D-004 | Memory: RC + inferred ownership (moves, borrows, elision) + Bacon-Rajan cycle collector for the residue. No tracing GC, ever | proposed |
| D-005 | Exceptions: native unwinding (Itanium/SEH), zero-cost happy path — not result-codes | proposed |
| D-006 | Generics: monomorphization; vtable dispatch only for explicit dynamic-Protocol use and `--opt-size` cold code | proposed |
| D-007 | `str` = UTF-8 (not PEP 393 UTF-32 arrays). Codepoint indexing via lazy offset index. Rationale: memory, SIMD, FFI; deviation invisible except perf profile | proposed |
| D-008 | No GIL in binaries; thread safety = compile-time Shareable/move checks (Rust Send/Sync analog), not runtime locks | proposed |
| D-009 | Stdlib written in typed Python, compiled by pycc itself; Rust intrinsics only at the syscall/math floor | proposed |
| D-010 | Diagnostics: codes stable forever, JSON format versioned, `explain` registry mandatory per code | proposed |
| D-011 | Cross-platform is Tier-1 from v0.1: Windows/MSVC is CI-gated day one, bundled lld, no system toolchain required | proposed |
| D-012 | Language level: exactly CPython 3.14 in v1 (`python = "3.14"` in pycc.toml). No per-version grammar switches until v1.x | proposed |
| D-013 | Development model: AI-first — the compiler is written by AI agents; these specs are the executable contract. Every spec claim must be mechanically checkable (test, benchmark gate, or CI rule), because "the spec" is what agents optimize against | proposed |

## Template

```
## D-0XX: Title
- Status: proposed
- Context: what forces the choice
- Decision: what we do
- Alternatives: what we rejected and why
- Consequences: what gets easier / harder / irreversible
```

Entries D-001…D-013 get their long-form sections as they graduate to `accepted` (first PR that depends on the decision must include it).
