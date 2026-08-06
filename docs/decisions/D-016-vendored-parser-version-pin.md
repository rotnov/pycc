---
id: D-016
title: "Vendored parser version pin"
status: accepted
---

## D-016: Vendored parser version pin

- Status: accepted (PR-1/PR-2 is the PR that depends on it)
- Context: D-003 already decided *that* pycc vendors `ruff_python_parser` through v0.5; it didn't pin *which* version. `ruff_python_parser`/`ruff_python_ast` are published to crates.io infrequently (6 total versions as of this pin) rather than continuously, so drift between what a contributor's local `Cargo.lock` resolves and what CI resolves is a real risk without an explicit pin.
- Decision: `ruff_python_parser = "0.0.6"`, `ruff_python_ast = "0.0.6"` — the newest versions on crates.io when PR-1 was built, verified via the crates.io API rather than assumed from training data.
- Alternatives: pinning to a specific git commit of `astral-sh/ruff` directly (rejected for v0.1 — crates.io releases are simpler to audit and update than a floating git dependency, and the project isn't blocked on any unreleased fix yet).
- Consequences: `pycc_ast`/`pycc_parser`'s `Cargo.toml` pins these exactly; bumping either is a new ADR entry (or an amendment noted here), not a silent `cargo update`.

