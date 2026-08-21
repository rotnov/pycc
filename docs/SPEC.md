# pycc — Project Specification Index

Entry point to the full spec. Development model is AI-first (D-013): the normative documents indexed here are the contract the code is written against — every normative claim must be checkable by a test, benchmark gate, or CI rule. The D-066 retrospective and session logs are explicitly informational records, not specifications; their table rows are included only so agents can discover the project-history and handoff context.

| Doc | Contents | Drives |
|---|---|---|
| [README.md](../README.md) | Vision, compiler-landscape positioning, quick start | everything |
| [PYTHON_STANDARDS.md](./PYTHON_STANDARDS.md) | Language PEPs 3.0→3.14 plus the Python 3.15 preview and Python 3.16 watchlist, one conformance test each; rejected-by-design list; OSS corpus tiers | `tests/conformance`, `tests/diagnostics` |
| [conformance-breadth-manifest.json](../tests/fixtures/conformance-breadth-manifest.json) | Per-row breadth declaration for every evidence-backed (`◐`/`✅`) PYTHON_STANDARDS.md row: what its fixtures prove, and what the PEP contains that they do not, each gap classified `core` or `out-of-scope` (D-176, D-177) | `scripts/check_conformance_breadth.py` |
| [ARCHITECTURE.md](./ARCHITECTURE.md) | Pipeline, crates, incremental/parallel design, **cross-platform Tier-1 matrix** | workspace layout, CI matrix |
| [TYPE_SYSTEM.md](./TYPE_SYSTEM.md) | Strictness rules, type↔representation table, generics, narrowing | `pycc_types` |
| [MEMORY_OWNERSHIP.md](./MEMORY_OWNERSHIP.md) | Inferred ownership, RC elision, cycles, GIL-free native thread safety | `pycc_own`, `pycc_rt` |
| [RUNTIME.md](./RUNTIME.md) | Object model, exceptions, generators, allocator, planned transparent CPython interop | `pycc_rt` |
| [CLI_SPEC.md](./CLI_SPEC.md) | Commands, flags, `pycc.toml`, planned interop policies, exit codes, diagnostic formats | `pycc` driver |
| [DIAGNOSTICS.md](./DIAGNOSTICS.md) | Error-code registry, quality bar, stability rules | `pycc_diag` |
| [STDLIB_PLAN.md](./STDLIB_PLAN.md) | Builtins + module tiers with target versions, compat policy | `pycc_std` |
| [TESTING.md](./TESTING.md) | 7 test layers, conformance harness, differential fuzzing, corpus bot | CI, `pycc_testkit` |
| [CI feedback routing design](./superpowers/specs/2026-08-15-ci-feedback-routing-design.md) | Fail-closed change classification, required-gate topology, cancellation, and the protected three-merge activation sequence | `scripts/classify_ci_changes.py`, CI workflow, D-103 policy successors |
| [ROADMAP.md](./ROADMAP.md) | Delivery status for the repository tree in the containing commit plus v0.1→v1.0 milestones with binary acceptance criteria | releases |
| [decisions/](./decisions/README.md) | ADR log, one file per decision — see the generated index for the full list | irreversible calls |
| [DELIVERY_PLAN.md](./DELIVERY_PLAN.md) | Milestone decomposition, verified environment baseline, v0.1 crate scope + PR breakdown, v0.2 execution strategy + PR-8..PR-14 breakdown, v0.3 execution strategy + PR-15..PR-23 breakdown (design detail in [`superpowers/specs/`](./superpowers/specs/)), autonomy policy | execution order, PR sequencing |
| [REPOSITORY_GOVERNANCE.md](./REPOSITORY_GOVERNANCE.md) | Protected-branch contract, live external-monitor checkpoints and event scope, required controls, emergency bypass, and audit response | GitHub settings, governance workflows |
| [AGENT_RETROSPECTIVE.md](./AGENT_RETROSPECTIVE.md) | Informational process-mistake journal for autonomous agent work (not code bugs or normative requirements) | future-session process learning only |
| [sessions/](./sessions/README.md) | Informational handoff snapshots, one dated file per checkpoint: overall delivery status, in-flight work, resume points (not normative requirements) | session continuity/handoff only |
| [AGENT_TOOLING.md](./AGENT_TOOLING.md) | Agent plugin pins, reviewed update process, validation, and rollback | `.agents/`, `.claude/`, agent-asset CI |
| [WEBSITE.md](./WEBSITE.md) | Public website, search metadata, canonical URL, and GitHub Pages publication | `site/`, Pages workflow |
| `site/evidence-heroes.json` | Versioned evidence-hero inventory, immutable artifact/run provenance, explicit unavailable states, and cross-surface projection contract (D-186) | `scripts/check-site.sh`, `scripts/test-check-site.sh`, HTML/Markdown/LLM/structured/social projections |
| [DISTRIBUTION.md](./DISTRIBUTION.md) | Source-based pre-commit publication, hook contract, installation limits, collision-safe package identity | `.pre-commit-hooks.yaml`, `pycc check`, release tags, `scripts/check_package_identity.rb` |
| [SEARCH_VISIBILITY.md](./SEARCH_VISIBILITY.md), [SEARCH_QUERY_REGISTRY.json](./SEARCH_QUERY_REGISTRY.json), and [SEARCH_VISIBILITY_CHECKPOINTS.json](./SEARCH_VISIBILITY_CHECKPOINTS.json) | Chronological provider-qualified measurements, the machine-readable query-intent registry, and append-only history-prefix checkpoints | discoverability monitoring, local validation, and the active trusted search audit |
| [GITHUB_TRAFFIC_OBSERVATIONS.json](./GITHUB_TRAFFIC_OBSERVATIONS.json) | Sanitized immutable GitHub Traffic API observation artifact with daily views/clones rows, referrers, popular paths, and repository state | daily traffic evidence preservation, local validation (`scripts/check_github_traffic_observations.py`), and prose binding |
| [ENGINE_VISIBILITY_OBSERVATIONS.json](./ENGINE_VISIBILITY_OBSERVATIONS.json) | Sanitized immutable engine-qualified visibility observation artifact for web-search and LLM answer-engine surfaces, separate from indexability | engine-qualified visibility evidence preservation, local validation (`scripts/check_engine_visibility_observations.py`), and prose binding |
| [PAGES_VISIT_OBSERVATIONS.json](./PAGES_VISIT_OBSERVATIONS.json) | Sanitized immutable GitHub Pages visit observation artifact with measurement contract, data-minimization boundary, and separation rules, separate from Search Console, GitHub traffic, and engine visibility | Pages visit measurement capability preservation, local validation (`scripts/check_pages_visit_observations.py`), and prose binding |
| `CITATION.cff` | Machine-readable software citation metadata (CFF 1.2.0) | GitHub "Cite this repository" panel, citation identity |
| `README.md` testing-strategy section | Current-vs-planned testing claims (issue #214) | `scripts/check_readme_claims.rb`, `scripts/test_check_readme_claims.rb` |

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

Spec change = PR touching the normative doc + the tests that enforce it, reviewed against docs/decisions/README.md. CI owns the ✅ marks in PYTHON_STANDARDS.md — humans and agents only add rows, never flip statuses by hand. `AGENT_RETROSPECTIVE.md` and `docs/sessions/` are reviewed for factual accuracy, links, privacy, and safe handoff instructions, but their entries do not create implementation requirements. Promote a lesson or snapshot claim into the owning policy, ADR, or specification before treating it as normative.

## Not yet specced (known gaps)

`docs/semantics.md` (deviation ledger — starts at v0.1 with D-007 str notes) · binary installers and `rustup`-style distribution beyond the source-based pre-commit integration · LSP protocol details (post-1.0) · release artifact signing and provenance (for example, sigstore).
