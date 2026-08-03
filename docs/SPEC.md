# pycc — Project Specification Index

Entry point to the full spec. Development model is AI-first (D-013): the normative documents indexed here are the contract the code is written against — every normative claim must be checkable by a test, benchmark gate, or CI rule. The D-066 retrospective and session logs are explicitly informational records, not specifications; their table rows are included only so agents can discover the project-history and handoff context.

| Doc | Contents | Drives |
|---|---|---|
| [README.md](../README.md) | Vision, compiler-landscape positioning, quick start | everything |
| [PYTHON_STANDARDS.md](./PYTHON_STANDARDS.md) | Language PEPs 3.0→3.14 plus the Python 3.15 preview track, one conformance test each; rejected-by-design list; OSS corpus tiers | `tests/conformance`, `tests/diagnostics` |
| [ARCHITECTURE.md](./ARCHITECTURE.md) | Pipeline, crates, incremental/parallel design, **cross-platform Tier-1 matrix** | workspace layout, CI matrix |
| [TYPE_SYSTEM.md](./TYPE_SYSTEM.md) | Strictness rules, type↔representation table, generics, narrowing | `pycc_types` |
| [MEMORY_OWNERSHIP.md](./MEMORY_OWNERSHIP.md) | Inferred ownership, RC elision, cycles, GIL-free native thread safety | `pycc_own`, `pycc_rt` |
| [RUNTIME.md](./RUNTIME.md) | Object model, exceptions, generators, allocator, planned transparent CPython interop | `pycc_rt` |
| [CLI_SPEC.md](./CLI_SPEC.md) | Commands, flags, `pycc.toml`, planned interop policies, exit codes, diagnostic formats | `pycc` driver |
| [DIAGNOSTICS.md](./DIAGNOSTICS.md) | Error-code registry, quality bar, stability rules | `pycc_diag` |
| [STDLIB_PLAN.md](./STDLIB_PLAN.md) | Builtins + module tiers with target versions, compat policy | `pycc_std` |
| [TESTING.md](./TESTING.md) | 7 test layers, conformance harness, differential fuzzing, corpus bot | CI, `pycc_testkit` |
| [ROADMAP.md](./ROADMAP.md) | Delivery status for the repository tree in the containing commit plus v0.1→v1.0 milestones with binary acceptance criteria | releases |
| [DECISIONS.md](./DECISIONS.md) | ADR log D-001…D-062, D-066…D-068, D-070…D-130 (int representation, LLVM, UTF-8 strings, no-GIL model, coverage, agent surfaces, CI trust, iEvo, repository governance, Windows CI, roadmap evidence, frontend depth, performance-gate activation and audited recovery, paired-runner/source-aware/fixed-replicate stabilization, type-check environment sharing, local-binding classification, agent retrospective/session logs, source-installed pre-commit distribution, pinned local review, PR-5 MIR/codegen/runtime boundaries including bigint and small-string layout, corrected string-refcounting scope, `print()` nested-expression handling, PIC relocation, backend lexical scope, representation preservation, the canonical `None` parameter ABI, portable runtime-failure exit normalization, the symmetric fail-closed local iEvo hook lifecycle, checkpointed live repository monitoring, the separate plain-timed throughput-floor check design, the conformance-oracle CI trust boundary, the Windows newline-translation strip, the CLI_SPEC.md diagnostic-example correction, the `pycc check` throughput-floor CI trust boundary, the plain-integration-test shape for PR-6's conformance fixtures, the equality-vs-numeric-widening strictness split, relaxed iEvo bug-report scrubbing, the v0.2 acceptance-criteria correction, the recursive `Ty` representation for v0.2 generics, the main-seeded Windows vcpkg binary cache, the `ubuntu-24.04-arm` nbody-gate floor, the base-owned search-policy successor manifest, PR-9's conformance-harness architecture, v0.2's `list[T]` thin-slice scope cuts, `PyIntListObj`'s untagged-element representation, `pycc_codegen::Scalar::List`, the negative-index rejection scope cut, boxing `Ty`'s container variants and the subsequent withdrawal of its "confirmed closed" claim once the CI record contradicted it, the module-value-binding call-shadowing rule with source-order `def` rebinding, the `frontend-perf-gate` runner move to `ubuntu-latest`, `--root` ancestor-symlink/mount-point rejection, raising `frontend-perf-gate`'s regression threshold to 7.0% for the `Ty`-migration cost, dict/set's dense insertion-ordered-array representation and scoped key/element types, dict[str, int]'s insert-or-update semantics alongside set[int]'s deferred membership test, dict/set leak-only refcounting, tuple[...]'s SSA-struct-value representation with its scoped element types and literal-index reads, list/dict/set comprehensions' statement-level, synthesized-loop-variable scoping, `list[int]` slicing's clamped-bounds scope, the three new container methods' (`.pop()`/`.get()`/`.add()`) diagnostic-reuse scope, the PEP-709 conformance fixture's loop-variable-non-leakage scope, session-driven temporary CI-check relaxation narrowly superseding D-024/D-054, gathering exact-child CPU-time evidence before changing the nbody gate, the autonomous agent operation model's judgment-fork resolution and escalation exceptions, source-compatible automatic CPython interop with strict build policies, the evidence-backed decision to retain wall-clock nbody gating, and decomposing the session handoff log into per-session files) | irreversible calls |
| [DELIVERY_PLAN.md](./DELIVERY_PLAN.md) | Milestone decomposition, verified environment baseline, v0.1 crate scope + PR breakdown, v0.2 execution strategy + PR-8..PR-14 breakdown (design detail in [`superpowers/specs/`](./superpowers/specs/)), autonomy policy | execution order, PR sequencing |
| [REPOSITORY_GOVERNANCE.md](./REPOSITORY_GOVERNANCE.md) | Protected-branch contract, live external-monitor checkpoints and event scope, required controls, emergency bypass, and audit response | GitHub settings, governance workflows |
| [AGENT_RETROSPECTIVE.md](./AGENT_RETROSPECTIVE.md) | Informational process-mistake journal for autonomous agent work (not code bugs or normative requirements) | future-session process learning only |
| [sessions/](./sessions/README.md) | Informational handoff snapshots, one dated file per checkpoint: overall delivery status, in-flight work, resume points (not normative requirements) | session continuity/handoff only |
| [AGENT_TOOLING.md](./AGENT_TOOLING.md) | Agent plugin pins, reviewed update process, validation, and rollback | `.agents/`, `.claude/`, agent-asset CI |
| [WEBSITE.md](./WEBSITE.md) | Public website, search metadata, canonical URL, and GitHub Pages publication | `site/`, Pages workflow |
| [DISTRIBUTION.md](./DISTRIBUTION.md) | Source-based pre-commit publication, hook contract, installation limits | `.pre-commit-hooks.yaml`, `pycc check`, release tags |
| [SEARCH_VISIBILITY.md](./SEARCH_VISIBILITY.md), [SEARCH_QUERY_REGISTRY.json](./SEARCH_QUERY_REGISTRY.json), and [SEARCH_VISIBILITY_CHECKPOINTS.json](./SEARCH_VISIBILITY_CHECKPOINTS.json) | Chronological provider-qualified measurements, the machine-readable query-intent registry, and append-only history-prefix checkpoints | discoverability monitoring, local validation, and the active trusted search audit |

## Invariants (short version)

1. **Standard Python in, autonomous deployment artifact out.** Native and
   `deny`/`--pure` builds produce a standalone native binary without CPython;
   a permitted CPython-backed import produces a self-contained bundle with its
   pinned runtime and dependency closure (planned v0.7, D-128). pycc never adds
   its own dialect or syntax. The v1.0 language level is exactly CPython 3.14;
   admitting a later standard Python language level requires its versioned
   roadmap gate and a superseding ADR.
2. **Strict types are the only mode.** Untyped public API doesn't compile.
3. **Ownership is inferred, never written.** Semantics-preserving; only performance changes.
4. **No pycc-wide GIL; native safety proven at compile time.** Native pycc
   execution is GIL-free. A planned embedded CPython boundary retains its own
   GIL only while executing CPython-backed operations (D-128).
5. **Cross-platform Tier-1** (Linux/macOS/Windows, x64+arm64) — a feature doesn't exist until it's green everywhere.
6. **Every feature has a test; every rejection has a diagnostic test; every deviation is documented.**
7. **Compiler speed is a feature**: check like ruff, not like mypy.

## Doc lifecycle

Spec change = PR touching the normative doc + the tests that enforce it, reviewed against DECISIONS.md. CI owns the ✅ marks in PYTHON_STANDARDS.md — humans and agents only add rows, never flip statuses by hand. `AGENT_RETROSPECTIVE.md` and `docs/sessions/` are reviewed for factual accuracy, links, privacy, and safe handoff instructions, but their entries do not create implementation requirements. Promote a lesson or snapshot claim into the owning policy, ADR, or specification before treating it as normative.

## Not yet specced (known gaps)

`docs/semantics.md` (deviation ledger — starts at v0.1 with D-007 str notes) · binary installers and `rustup`-style distribution beyond the source-based pre-commit integration · LSP protocol details (post-1.0) · release artifact signing and provenance (for example, sigstore).
