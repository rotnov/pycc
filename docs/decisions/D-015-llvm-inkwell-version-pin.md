---
id: D-015
title: "LLVM/inkwell version pin"
status: accepted
---

## D-015: LLVM/inkwell version pin

- Status: accepted (PR-1/PR-2 is the PR that depends on it)
- Context: ARCHITECTURE.md/D-002 specify "LLVM (inkwell)" without a version. `pycc_codegen` needs one concrete pin to build against at all. Checked empirically on the dev host rather than picked from memory: only one real LLVM install exists (`llvm@17` through `llvm@22` under `/opt/homebrew/opt/` all resolve to the same single Cellar keg, `llvm/22.1.1` — not five distinct installs).
- Decision: LLVM `22.1.1`, `inkwell = "0.9"` (latest at pin time) with `default-features = false, features = ["llvm22-1"]`. `inkwell` 0.9.0's feature list was checked directly (crates.io version metadata) to confirm `llvm22-1` exists before committing to it — it does, alongside `llvm19-1`/`llvm20-1`/`llvm21-1`.
- Alternatives: none genuinely competing — this isn't a preference call between installed options, only one LLVM was actually present. The alternative considered was installing an older/different LLVM version to chase a "more battle-tested" inkwell feature; rejected as unnecessary extra setup work for no concrete benefit given `llvm22-1` already exists and matches what's on the machine.
- Consequences: `pycc_codegen/Cargo.toml`'s `inkwell` feature and CI's `LLVM_SYS_221_PREFIX` (or whatever exact env var `llvm-sys` reports needing — confirm per-build, don't assume the number matches the LLVM minor version) must be changed together if this ever bumps; a version bump is a new ADR, not an edit to this one.

