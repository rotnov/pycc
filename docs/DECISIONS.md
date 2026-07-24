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
| D-014 | Testing: 100% line+region coverage (`cargo llvm-cov`), CI-gated on every PR from v0.1 on. Exemptions are whole-file only (`--ignore-filename-regex`), each entry justified in TESTING.md — no function-level opt-out exists on stable Rust | proposed |

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

## D-014: 100% test coverage requirement

- Status: proposed (graduates to accepted with PR-1, the first PR that wires the CI gate — same convention as D-001…D-013)
- Context: pycc is a compiler — silent gaps in its own test suite are exactly the kind of bug that surfaces as a miscompile in someone else's code, far from where the gap was introduced. The project needs a binary, CI-enforced floor, not an aspirational target. Verified on the pinned toolchain (rustc 1.97.1) before adopting: `#[coverage(off)]` is still gated behind `#![feature(coverage_attribute)]` (rust-lang/rust#84605) and unavailable on stable — so any exemption mechanism must not depend on it.
- Decision: `cargo llvm-cov` (wraps `-C instrument-coverage`, ships via the `llvm-tools` rustup component — independent of the Homebrew LLVM 22.1.1 used for `inkwell` codegen, no version coupling between the two) gates every PR at `--fail-under-lines 100 --fail-under-regions 100`. Branch coverage is reported when available but not gated — it requires a nightly toolchain, and this project has no other reason to leave the stable channel (`rust-toolchain.toml` pins stable 1.97.1; introducing a nightly dependency just for one coverage sub-metric would be its own separate, unwarranted decision). The only exemption granularity is whole-file, via `--ignore-filename-regex`, because per-function exemption has no stable mechanism — this is a feature, not a limitation: it keeps platform-conditional or otherwise-unreachable code in its own file rather than letting an exemption hide inside an otherwise-normal one. Test code itself (`tests/`, `*_tests.rs`, `tests.rs`) is excluded from the denominator by cargo-llvm-cov by default, so coverage measures product code exercised by tests, not tests covering themselves.
- Alternatives: `grcov`/`tarpaulin` (older, less precise than LLVM source-based coverage now that `cargo llvm-cov` exists and the project already standardizes on LLVM tooling); a percentage target below 100% (rejected — a threshold like 95% lets the uncovered 5% drift to wherever it's least inconvenient to test, which is usually exactly the error-handling and edge-case code that matters most for a compiler); no gate at all (rejected outright, contradicts D-013's premise that specs and quality bars must be mechanically checkable).
- Consequences: every new file needs tests before merge, including CI/tooling glue where that's checked into a coverable crate. Genuinely untestable code (OS-specific branches exercised only on a different Tier-1 target, for instance) must live in its own file and get a named, justified entry in the exemption list in TESTING.md — an undocumented exemption is a review-blocking finding, same as an undocumented CPython deviation elsewhere in the spec. Wired into CI starting PR-1 (see DELIVERY_PLAN.md), not retrofitted later, so no crate ever accumulates an uncovered backlog.
