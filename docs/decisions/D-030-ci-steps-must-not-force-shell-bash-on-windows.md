---
id: D-030
title: "CI steps must not force `shell: bash` on Windows without a reason"
status: accepted
---

## D-030: CI steps must not force `shell: bash` on Windows without a reason

- Status: accepted (PR-3 is the PR that depends on it)
- Context: with D-028 and D-029 resolved, every actual `pycc`/`pycc_codegen` test passed on Windows for the first time in this arc -- but `cargo clippy --workspace --all-targets -- -D warnings` still failed, compiling an unrelated transitive proc-macro dependency (`manyhow-macros`), with `error: linking with \`link.exe\` failed` / `/usr/bin/link: extra operand ...` / `Try '/usr/bin/link --help' for more information` -- not an MSVC `link.exe` error message at all. Root cause: this step forced `shell: bash`; on Windows that's Git Bash, whose own startup reorders `PATH` such that its bundled coreutils `link.exe` (the Unix hardlink tool, at `C:\Program Files\Git\usr\bin\link.exe`) shadows MSVC's real linker that `ilammy/msvc-dev-cmd@v1` had already put on `PATH` correctly earlier in the same job -- a known Windows CI gotcha, confirmed directly by the error text itself (coreutils' own `link --help` message, not MSVC's).
- Decision: drop the explicit `shell: bash` override for this step. `cargo build`/`cargo test` already ran correctly on Windows earlier in the same job using pwsh (the OS's own default, no override needed), and this step's two commands (`rustup component add clippy`, `cargo clippy ...`) have no bash-specific syntax -- nothing is lost by letting each OS use its own default shell instead (bash on Linux/macOS, unchanged; pwsh on Windows, fixed).
- Alternatives: keep `shell: bash` and explicitly fix `PATH` ordering (e.g. removing/renaming Git's own `link.exe`, or setting a target-specific linker override to bypass `PATH` search entirely) -- rejected as more moving parts than simply not forcing a shell this step never needed bash syntax for in the first place.
- Consequences: none of the other `native-build-test` steps specify `shell:`, so this brings the clippy step in line with that existing pattern rather than introducing a new one. If a future step's script genuinely needs bash-specific syntax on Windows, this same PATH-shadowing risk applies and needs to be checked for explicitly, not assumed away.

