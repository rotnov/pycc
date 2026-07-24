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
| D-014 | Testing: 100% line+region coverage (`cargo llvm-cov`), CI-gated on every PR from v0.1 on. Exemptions are whole-file only (`--ignore-filename-regex`), each entry justified in TESTING.md — no function-level opt-out exists on stable Rust | accepted |
| D-015 | Codegen toolchain pin: LLVM 22.1.1 (Homebrew, single keg — the `llvm@17`..`llvm@22` opt-paths on the dev host are stale symlinks to it, not distinct installs) + `inkwell = "0.9"` with feature `llvm22-1` | accepted |
| D-016 | Vendored parser pin (D-003): `ruff_python_parser = "0.0.6"`, `ruff_python_ast = "0.0.6"` (crates.io, checked at PR-1 time — re-verify before bumping) | accepted |
| D-017 | No separate `pycc_lexer` crate for v0.1: the vendored `ruff_python_parser` bundles lexing internally and nothing in the v0.1 pipeline consumes a standalone token stream. `pycc_lexer` is created when D-003's own-parser work (v0.6) actually needs to expose one | accepted |
| D-018 | `pycc_testkit` deferred past PR-1/PR-2: no PEP conformance matrix exists yet for a real harness to check against; `tests/slice0.rs` covers the two named fixtures ad hoc in the meantime, built for real at PR-4/PR-6 | proposed |
| D-019 | Every agent task starts from refreshed repository/spec/API evidence and never mutates a stale or dirty base implicitly | accepted |
| D-020 | Confirmed iEvo defects may be reported publicly without per-report permission, within strict scope and privacy limits | accepted |
| D-021 | Auto-evolution intent is shared, but executable hooks and generated scripts remain machine-local | accepted |
| D-022 | `main` is protected by PR, CI, resolved conversations, and an audited emergency path; independent approval starts when a second maintainer is available | accepted |

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

- Status: accepted (PR-1/PR-2 wires the CI gate this depends on)
- Context: pycc is a compiler — silent gaps in its own test suite are exactly the kind of bug that surfaces as a miscompile in someone else's code, far from where the gap was introduced. The project needs a binary, CI-enforced floor, not an aspirational target. Verified on the pinned toolchain (rustc 1.97.1) before adopting: `#[coverage(off)]` is still gated behind `#![feature(coverage_attribute)]` (rust-lang/rust#84605) and unavailable on stable — so any exemption mechanism must not depend on it.
- Decision: `cargo llvm-cov` (wraps `-C instrument-coverage`; a separately distributed cargo subcommand that *drives* the `llvm-tools-preview` rustup component at runtime — the two are installed separately, caught by repo audit issue #13 after an earlier draft of this entry conflated them — independent of the Homebrew LLVM 22.1.1 used for `inkwell` codegen, no version coupling between the two) gates every PR at `--fail-under-lines 100 --fail-under-regions 100`. Branch coverage is reported when available but not gated — it requires a nightly toolchain, and this project has no other reason to leave the stable channel (`rust-toolchain.toml` pins stable 1.97.1; introducing a nightly dependency just for one coverage sub-metric would be its own separate, unwarranted decision). The only exemption granularity is whole-file, via `--ignore-filename-regex`, because per-function exemption has no stable mechanism — this is a feature, not a limitation: it keeps platform-conditional or otherwise-unreachable code in its own file rather than letting an exemption hide inside an otherwise-normal one. Test code itself (`tests/`, `*_tests.rs`, `tests.rs`) is excluded from the denominator by cargo-llvm-cov by default, so coverage measures product code exercised by tests, not tests covering themselves.
- Alternatives: `grcov`/`tarpaulin` (older, less precise than LLVM source-based coverage now that `cargo llvm-cov` exists and the project already standardizes on LLVM tooling); a percentage target below 100% (rejected — a threshold like 95% lets the uncovered 5% drift to wherever it's least inconvenient to test, which is usually exactly the error-handling and edge-case code that matters most for a compiler); no gate at all (rejected outright, contradicts D-013's premise that specs and quality bars must be mechanically checkable).
- Consequences: every new file needs tests before merge, including CI/tooling glue where that's checked into a coverable crate. Genuinely untestable code (OS-specific branches exercised only on a different Tier-1 target, for instance) must live in its own file and get a named, justified entry in the exemption list in TESTING.md — an undocumented exemption is a review-blocking finding, same as an undocumented CPython deviation elsewhere in the spec. Wired into CI starting PR-1 (see DELIVERY_PLAN.md), not retrofitted later, so no crate ever accumulates an uncovered backlog.

## D-015: LLVM/inkwell version pin

- Status: accepted (PR-1/PR-2 is the PR that depends on it)
- Context: ARCHITECTURE.md/D-002 specify "LLVM (inkwell)" without a version. `pycc_codegen` needs one concrete pin to build against at all. Checked empirically on the dev host rather than picked from memory: only one real LLVM install exists (`llvm@17` through `llvm@22` under `/opt/homebrew/opt/` all resolve to the same single Cellar keg, `llvm/22.1.1` — not five distinct installs).
- Decision: LLVM `22.1.1`, `inkwell = "0.9"` (latest at pin time) with `default-features = false, features = ["llvm22-1"]`. `inkwell` 0.9.0's feature list was checked directly (crates.io version metadata) to confirm `llvm22-1` exists before committing to it — it does, alongside `llvm19-1`/`llvm20-1`/`llvm21-1`.
- Alternatives: none genuinely competing — this isn't a preference call between installed options, only one LLVM was actually present. The alternative considered was installing an older/different LLVM version to chase a "more battle-tested" inkwell feature; rejected as unnecessary extra setup work for no concrete benefit given `llvm22-1` already exists and matches what's on the machine.
- Consequences: `pycc_codegen/Cargo.toml`'s `inkwell` feature and CI's `LLVM_SYS_221_PREFIX` (or whatever exact env var `llvm-sys` reports needing — confirm per-build, don't assume the number matches the LLVM minor version) must be changed together if this ever bumps; a version bump is a new ADR, not an edit to this one.

## D-016: Vendored parser version pin

- Status: accepted (PR-1/PR-2 is the PR that depends on it)
- Context: D-003 already decided *that* pycc vendors `ruff_python_parser` through v0.5; it didn't pin *which* version. `ruff_python_parser`/`ruff_python_ast` are published to crates.io infrequently (6 total versions as of this pin) rather than continuously, so drift between what a contributor's local `Cargo.lock` resolves and what CI resolves is a real risk without an explicit pin.
- Decision: `ruff_python_parser = "0.0.6"`, `ruff_python_ast = "0.0.6"` — the newest versions on crates.io when PR-1 was built, verified via the crates.io API rather than assumed from training data.
- Alternatives: pinning to a specific git commit of `astral-sh/ruff` directly (rejected for v0.1 — crates.io releases are simpler to audit and update than a floating git dependency, and the project isn't blocked on any unreleased fix yet).
- Consequences: `pycc_ast`/`pycc_parser`'s `Cargo.toml` pins these exactly; bumping either is a new ADR entry (or an amendment noted here), not a silent `cargo update`.

## D-017: No separate `pycc_lexer` crate for v0.1

- Status: accepted (PR-1/PR-2 is the PR that depends on it)
- Context: ARCHITECTURE.md's crate table lists `pycc_lexer`/`pycc_parser`/`pycc_ast` as three crates, describing the end state after D-003 resolves (pycc's own hand-written lexer and parser, v0.6). During the vendored-parser bootstrap phase, `ruff_python_parser` performs lexing internally and does not expose a standalone token stream pycc could wrap even if it wanted to, and nothing downstream in the v0.1 pipeline (`pycc_hir` onward) consumes tokens directly — only the parsed AST.
- Decision: v0.1 creates `pycc_ast` (a thin, stable re-export boundary over `ruff_python_ast`) and `pycc_parser` (wraps `ruff_python_parser::parse_module`), but not `pycc_lexer`.
- Alternatives: an empty pass-through `pycc_lexer` crate now, for table-completeness with ARCHITECTURE.md (rejected as YAGNI — nothing would depend on it, and an empty crate that exists only to match a table is exactly the kind of unnecessary abstraction the project's own engineering conventions warn against).
- Consequences: `pycc_lexer` gets created in v0.6 when D-003's own-parser work gives it a real reason to exist (a token stream something actually consumes); until then, ARCHITECTURE.md's crate table describes a target state, not every crate's exact creation date.

## D-018: `pycc_testkit` deferred past PR-1/PR-2

- Status: proposed (graduates to accepted when `pycc_testkit` is actually created)
- Context: DELIVERY_PLAN.md's original "v0.1 crate scope" and "Testing scope for v0.1" sections listed `pycc_testkit` as built alongside the other 8 crates, with Layer 2 (the conformance harness) starting "as early as slice 0" -- but PR-1/PR-2 never created it (flagged by repo audit as an undocumented deviation, unlike `pycc_lexer`'s reasoned D-017). TESTING.md's actual spec for `pycc_testkit` is a real harness: `.py` fixtures under `tests/conformance/pyXY/` with PEP/category/milestone header comments, diffed against a pinned CPython 3.14 reference output, flipping ✅ marks in PYTHON_STANDARDS.md. None of that exists yet -- PYTHON_STANDARDS.md's matrix is still 100% unstarted (☐), so there is no PEP-level conformance tracking for a harness to drive yet.
- Decision: PR-1/PR-2 use a hand-written `tests/slice0.rs` integration test instead -- it compiles fixtures through the real `pycc` binary and asserts exact stdout, which is a genuine (if narrow) proof that the pipeline works end to end, but it is not TESTING.md's Layer 2 harness. `pycc_testkit` gets built for real once PR-4/PR-6 gives it PEPs to actually track (PR-6 is where DELIVERY_PLAN.md already schedules the first real conformance run: fib + mandelbrot-ascii vs. CPython on all 5 targets).
- Alternatives: scaffold a minimal `pycc_testkit` crate now, even with nothing real for it to do (rejected as premature -- there's no PEP matrix yet for it to check against, so it would be structure without function, the same YAGNI concern D-017 raised about `pycc_lexer`); keep DELIVERY_PLAN.md's "as early as slice 0" wording as-is and treat `tests/slice0.rs` as satisfying it (rejected -- that wording specifically promises TESTING.md's Layer 2 shape, which `tests/slice0.rs` doesn't have, and leaving the mismatch undocumented is exactly the kind of thing this decisions log exists to prevent).
- Consequences: DELIVERY_PLAN.md's "Testing scope for v0.1" section is corrected to say Layer 2 starts at PR-4/PR-6, not slice 0. `tests/slice0.rs` stays as ad hoc coverage for the two named PR-2 fixtures specifically, not a stand-in for the conformance harness.

## D-019: Agent task preflight and documentation refresh

- Status: accepted
- Context: this AI-first repository treats specifications and generated Rust API documentation as implementation inputs. Starting from a stale remote ref, stale rustdoc, or an unidentified dirty tree can make an otherwise-correct agent optimize against the wrong contract or overwrite user work.
- Decision: before a new task mutates the repository, the agent records status and HEAD, fetches/prunes without changing checked-out files, resolves the remote default branch, starts from that exact commit in a clean task branch/worktree, generates `cargo doc --workspace --no-deps`, and reads the specification/rustdoc that owns the affected area. Continuing dirty work is allowed only in place and only after comparing it with refreshed remote state; no implicit pull, merge, rebase, reset, or branch switch is authorized.
- Authority and scope: the preflight authorizes read-only Git/network discovery, creation of an isolated task branch/worktree, and local generated rustdoc. It does not authorize integrating upstream changes into a dirty tree, publishing a branch, or changing external systems.
- Privacy and failure behavior: preflight output stays in the task transcript and must not publish local paths or repository content. A failed fetch or documentation build is recorded and understood; older generated documentation must not be represented as current.
- Rollback: the isolated worktree/branch can be removed after preserving any intended commits. Changing this mandatory sequence requires a superseding decision because every later agent task depends on it.

## D-020: Autonomous public iEvo bug reporting

- Status: accepted
- Context: reproducible defects in the iEvo control plane can affect every agent task until upstream fixes ship. Requiring a new permission round-trip after the user has adopted the project policy delays correction and encourages local-only workarounds that other users cannot discover.
- Decision: after reproducing an iEvo malfunction, contradiction, broken hook, or invalid command and searching open and closed upstream issues, agents may create or update a public `ievo-ai/skills` issue without additional per-report permission. Duplicates, expected behavior, unverified suspicion, and ordinary pycc failures are not reportable.
- Authority and scope: the standing authority covers only the minimum useful iEvo report and evidence. It does not authorize publishing unrelated pycc material, reporting to other projects, or changing upstream code/labels/issue state beyond the report itself.
- Privacy and failure behavior: reports omit credentials, personal data, private/proprietary content, raw conversations, and identifying local paths even when the pycc repository is public. If authentication/network submission fails, preserve the pending sanitized report locally and surface the failure; do not repeatedly spam the endpoint.
- Rollback: a public report cannot be made private retroactively. A mistaken report is corrected and closed with an explicit explanation. Revoking standing authority requires a superseding decision and removal of the matching AGENTS.md section.

## D-021: Shared auto-evolution intent with local hook execution

- Status: accepted
- Context: the repository wants correction-driven evolution to survive across contributors, but iEvo generates hook scripts under gitignored `.ievo/hooks/`. Committing shared hook entries that invoke those absent targets made every clean clone emit hook errors (issue #28; upstream `ievo-ai/skills#446`).
- Decision: `.ievo/evo-auto.flag` records shared intent with `signal: corrections-only` and `auto_write_scope: project-wide-only`. Generated scripts and their hook wiring are machine-local: `.ievo/hooks/` and `.claude/settings.local.json` are ignored, while shared `.claude/settings.json` may reference only tracked targets or tracked fail-silent wrappers. Agents validate the tracked-file view in CI.
- Authority and scope: automatic capture may record confirmed user corrections under the configured project-wide scope. It does not broaden authority for external writes, source-code changes, secrets collection, or arbitrary per-agent behavior changes. Candidate lessons outside the auto-write scope remain reviewable candidates.
- Privacy and failure behavior: local hook inputs and generated logs remain local/ignored. Missing local setup must be a silent no-op in shared state, never a failing committed command. Validation failure blocks merge.
- Rollback: run the iEvo auto-disable workflow locally, remove or supersede the shared flag, remove local hook wiring/scripts, and verify the clean tracked view. A future tracked-wrapper design requires a superseding decision and security review.

## D-022: Protected main and audited emergency bypass

- Status: accepted
- Context: commit `0ac9b1d` reached `main` while PR #3 remained open after a timed-out merge request, proving that commit messages and monitoring conventions do not create a review boundary. An AI-authored compiler needs the repository host—not agent intent—to enforce the PR/CI/review path. The repository currently has one maintainer, and GitHub does not count an author's approval of their own pull request.
- Decision: protect `main` for administrators and automation alike. Require an up-to-date pull request, the current `build-test-coverage` status, and resolved conversations, and disallow force pushes/deletion. Set required approving reviews to zero while there is only one maintainer; enable one independent approval, stale-review dismissal, and last-push approval when a second human maintainer is available. A push-triggered audit verifies every resulting main commit is associated with a merged PR. New required checks are added to protection only after they have run successfully on `main`.
- Authority and scope: normal agent/bot credentials may open/update PRs but cannot bypass protection. Repository administrators retain the platform ability to edit protection only for the documented emergency procedure; that authority is not delegated to routine tasks.
- Privacy and failure behavior: the audit publishes only repository-native commit/PR identifiers. A direct or unassociated commit fails the audit, requires a governance incident, and blocks releases until history/protection are reconciled. CI/provider outages delay merge rather than weakening requirements.
- Rollback: emergency relaxation is time-bounded, linked to an incident, records before/after settings, and restores protection immediately after recovery. Permanently weakening the rule requires a superseding decision and explicit review.
