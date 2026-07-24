# Repository Instructions

## Project navigation

- Start with `docs/SPEC.md`. It is the index for the project's specifications and points to the document that owns each area.
- Treat the documents under `docs/` as part of the implementation contract, not as an after-the-fact description.
- Before changing behavior, architecture, public APIs, the CLI, diagnostics, build or release processes, tests, or supported language semantics, read the relevant specification.

## Before starting a new task

1. Update the worktree from the task's target branch and confirm which commit is the current base.
2. Run `cargo doc --workspace --no-deps` before making implementation changes. This refreshes the local Rust API documentation in `target/doc/` from the exact source revision being used for the task.
3. Read `docs/SPEC.md`, the relevant linked specifications, and the generated documentation for the crates affected by the task.
4. If documentation generation fails, record and understand the failure before changing code. Do not present documentation generated from an older revision as current.

## Keep documentation current

- Documentation work is part of every implementation task. Update all affected documentation in the same change and commit as the code; a change is incomplete while its docs describe the old behavior.
- Keep descriptions honest about what exists now versus what is planned. Update examples, commands, status markers, acceptance criteria, and cross-references when their underlying behavior changes.
- When adding, removing, renaming, or changing the purpose of a document, update `docs/SPEC.md` so it remains the reliable project map.
- Record irreversible or project-wide design choices in `docs/DECISIONS.md`. Do not silently rewrite an accepted decision; add a new decision that supersedes it.
- Every normative documentation claim should be enforceable where practical by a test, benchmark, or CI check, following the lifecycle rules in `docs/SPEC.md`.
- If a code change genuinely has no documentation impact, explicitly verify that conclusion rather than skipping the docs review by default.

## Generated documentation

- Markdown specifications and architectural explanations are human- and agent-authored source documents. Do not auto-generate or overwrite them from code.
- Rust API documentation is generated from source comments. After changing a public Rust API or its docs, run `cargo doc --workspace --no-deps` to verify it; do not commit `target/doc/`.
- If the repository later adds a checked-in documentation generator, keep its output deterministic, document one canonical regeneration command, and add a CI `--check`-style freshness gate.
- When a checked-in generated document exists, edit its source or generator, regenerate it, and commit the source and generated output together. Never patch generated output by hand.

## Completion check

Before finishing a change:

1. Re-read the relevant entries linked from `docs/SPEC.md`.
2. Update the affected docs in the same patch as the implementation.
3. Check links, examples, commands, status statements, and references to renamed files.
4. Run the relevant tests and documentation generation or freshness checks.
