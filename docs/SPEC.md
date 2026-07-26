# pycc — Project Specification Index

Entry point to the full spec. Development model is AI-first (D-013): the normative documents indexed here are the contract the code is written against — every normative claim must be checkable by a test, benchmark gate, or CI rule. The D-056 retrospective and session logs are explicitly informational records, not specifications; their table rows are included only so agents can discover the project-history and handoff context.

| Doc | Contents | Drives |
|---|---|---|
| [README.md](../README.md) | Vision, compiler-landscape positioning, quick start | everything |
| [PYTHON_STANDARDS.md](./PYTHON_STANDARDS.md) | Language PEPs 3.0→3.14 plus the Python 3.15 preview track, one conformance test each; rejected-by-design list; OSS corpus tiers | `tests/conformance`, `tests/diagnostics` |
| [ARCHITECTURE.md](./ARCHITECTURE.md) | Pipeline, crates, incremental/parallel design, **cross-platform Tier-1 matrix** | workspace layout, CI matrix |
| [TYPE_SYSTEM.md](./TYPE_SYSTEM.md) | Strictness rules, type↔representation table, generics, narrowing | `pycc_types` |
| [MEMORY_OWNERSHIP.md](./MEMORY_OWNERSHIP.md) | Inferred ownership, RC elision, cycles, GIL-free thread safety | `pycc_own`, `pycc_rt` |
| [RUNTIME.md](./RUNTIME.md) | Object model, exceptions, generators, allocator, CPython interop hatch | `pycc_rt` |
| [CLI_SPEC.md](./CLI_SPEC.md) | Commands, flags, `pycc.toml`, exit codes, diagnostic formats | `pycc` driver |
| [DIAGNOSTICS.md](./DIAGNOSTICS.md) | Error-code registry, quality bar, stability rules | `pycc_diag` |
| [STDLIB_PLAN.md](./STDLIB_PLAN.md) | Builtins + module tiers with target versions, compat policy | `pycc_std` |
| [TESTING.md](./TESTING.md) | 7 test layers, conformance harness, differential fuzzing, corpus bot | CI, `pycc_testkit` |
| [ROADMAP.md](./ROADMAP.md) | Delivery status for the repository tree in the containing commit plus v0.1→v1.0 milestones with binary acceptance criteria | releases |
| [DECISIONS.md](./DECISIONS.md) | ADR log D-001…D-056 (int repr, LLVM, UTF-8 str, no-GIL model, coverage, agent surfaces, CI trust, iEvo, repository governance, Windows CI, roadmap evidence, frontend depth, performance-gate activation and audited recovery, paired-runner stabilization, type-check environment sharing, local-binding classification, agent retrospective/session logs…) | irreversible calls |
| [DELIVERY_PLAN.md](./DELIVERY_PLAN.md) | Milestone decomposition, verified environment baseline, v0.1 crate scope + PR breakdown, autonomy policy | execution order, PR sequencing |
| [REPOSITORY_GOVERNANCE.md](./REPOSITORY_GOVERNANCE.md) | Protected-branch contract, required controls, emergency bypass, and audit response | GitHub settings, governance workflows |
| [AGENT_RETROSPECTIVE.md](./AGENT_RETROSPECTIVE.md) | Informational process-mistake journal for autonomous agent work (not code bugs or normative requirements) | future-session process learning only |
| [SESSION_LOG.md](./SESSION_LOG.md) | Informational handoff snapshot: overall delivery status, in-flight work, resume points (not normative requirements) | session continuity/handoff only |
| [AGENT_TOOLING.md](./AGENT_TOOLING.md) | Agent plugin pins, reviewed update process, validation, and rollback | `.agents/`, `.claude/`, agent-asset CI |
| [WEBSITE.md](./WEBSITE.md) | Public website, search metadata, canonical URL, and GitHub Pages publication | `site/`, Pages workflow |
| [SEARCH_VISIBILITY.md](./SEARCH_VISIBILITY.md) | Chronological search-query measurements and ranking methodology | discoverability monitoring |

## Invariants (short version)

1. **Standard Python in, native binary out.** pycc never adds its own dialect or
   syntax. The v1.0 language level is exactly CPython 3.14; admitting a later
   standard Python language level requires its versioned roadmap gate and a
   superseding ADR.
2. **Strict types are the only mode.** Untyped public API doesn't compile.
3. **Ownership is inferred, never written.** Semantics-preserving; only performance changes.
4. **No GIL; safety proven at compile time.**
5. **Cross-platform Tier-1** (Linux/macOS/Windows, x64+arm64) — a feature doesn't exist until it's green everywhere.
6. **Every feature has a test; every rejection has a diagnostic test; every deviation is documented.**
7. **Compiler speed is a feature**: check like ruff, not like mypy.

## Doc lifecycle

Spec change = PR touching the normative doc + the tests that enforce it, reviewed against DECISIONS.md. CI owns the ✅ marks in PYTHON_STANDARDS.md — humans and agents only add rows, never flip statuses by hand. `AGENT_RETROSPECTIVE.md` and `SESSION_LOG.md` are reviewed for factual accuracy, links, privacy, and safe handoff instructions, but their entries do not create implementation requirements. Promote a lesson or snapshot claim into the owning policy, ADR, or specification before treating it as normative.

## Not yet specced (known gaps)

`docs/semantics.md` (deviation ledger — starts at v0.1 with D-007 str notes) · packaging/distribution of pycc itself (installers, `rustup`-style) · LSP protocol details (post-1.0) · release artifact signing and provenance (for example, sigstore).
