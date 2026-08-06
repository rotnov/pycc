---
id: D-067
title: "Publish the pre-commit hook from the main pycc repository"
status: accepted
---

## D-067: Publish the pre-commit hook from the main pycc repository

- Status: accepted
- Context: consumers need an immutable revision containing both the hook manifest and the exact `pycc` implementation it runs. pycc is pre-alpha, and the manifest, CLI batching contract, diagnostics, source-encoding support, installation requirements, and tests are still evolving together. A separate hook repository would require a second release and synchronization boundary before there is an independent adapter worth versioning.
- Decision: the main `rotnov/pycc` repository is the source repository for the `language: rust` `pycc-check` hook. `.pre-commit-hooks.yaml` invokes `pycc check --`; consumers pin a release tag or immutable commit from this repository. The hook remains read-only and serial, and its distribution and verification contract is owned by `docs/DISTRIBUTION.md`.
- Alternatives: create a dedicated pre-commit repository immediately (rejected because it would duplicate or remotely pin the manifest while adding cross-repository release drift); publish a wrapper that downloads a binary (rejected because binary distribution, signing, and provenance are not specified yet); use `language: system` (rejected because it would silently depend on an unpinned user installation).
- Consequences: hook releases and compiler revisions are atomic, and integration tests can exercise the exact implementation named by the manifest. The initial install builds the current root package and therefore requires pinned Rust 1.97.1 and LLVM 22.1.1 until the package boundary is deliberately changed. Splitting the hook into a separate repository or a lightweight package requires a superseding ADR and equivalent Codex/Claude documentation and validation updates.

