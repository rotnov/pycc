# PR-8: --release/LTO Profile + pycc.toml + nbody Benchmark Harness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deliver `docs/DELIVERY_PLAN.md`'s v0.2 PR-8 row: a real `--release` profile that applies LLVM optimization to generated code, `pycc.toml` parsing wired into `pycc init` and `pycc build`'s default-profile resolution, and the nbody benchmark harness that measures pycc's `--release` binary against pinned CPython 3.14.6 (ratio ≥ 20 gate, per `docs/superpowers/specs/2026-07-28-v0-2-collections-generics-design.md`'s §1 measurement contract).

**Architecture:** No new crate. `--release` threads a new CLI flag through `src/cli.rs`/`src/main.rs` into `pycc_codegen::compile_to_object`, which calls inkwell's `Module::run_passes("default<O3>", &target_machine, PassBuilderOptions::create())` when requested (verified against the actual installed `inkwell` 0.9 API via `cargo doc -p inkwell --no-deps` — see Task 1's decision for why this, not a different pass-manager API). `pycc.toml` parsing is a new `src/project_config.rs` module using the `toml`+`serde` crates (new dependencies — verified absent from `Cargo.lock` today), deserializing only the `[project]`/`[build]` fields v0.2 actually reads; `[interop]`/`[test]` sections `docs/CLI_SPEC.md` already documents for later milestones are accepted in a user's file (serde ignores unknown sections by default) but not acted on yet. The nbody benchmark is a plain `tests/nbody_bench.rs` integration test (matching `tests/conformance.rs`'s `std::process::Command`-based pattern per D-085's precedent), `#[ignore]`d like the conformance tests, run explicitly in CI — **not** `benches/check_bench.rs`'s Criterion machinery, which measures a completely different thing (`pycc check`'s own speed across commits, with an "exact benchmark revision" integrity check this PR must not trip).

**Tech Stack:** Rust 1.97+ (edition 2024, unchanged), `inkwell` 0.9 (already a dependency, new APIs used), `toml` + `serde` (new dependencies), CPython 3.14.6 as an external oracle process (unchanged pattern from `tests/conformance.rs`).

## Global Constraints

- 100% line and region coverage is a hard merge invariant (D-014) — `cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100` must pass after every single task.
- `cargo clippy --workspace --all-targets -- -D warnings` must stay clean after every task.
- `cargo doc --workspace --no-deps` must stay clean after any public API change — and must be re-run before Task 3 specifically, to verify the exact `inkwell` API this plan cites still matches what's installed.
- Do not touch `benches/check_bench.rs`, its `[[bench]]` entry in the root `Cargo.toml`, or any file the existing `frontend-perf-measure`/`frontend-perf-gate` "Verify exact benchmark revisions" step diffs (`benches`, `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `rust-toolchain`, `.cargo`, every `crates/**/Cargo.toml`/`build.rs`) — adding the `toml`/`serde` dependency to the root `Cargo.toml`/`Cargo.lock` **will** trip that integrity check once, which is expected and correct (a real dependency change); do not work around it, let the check fail once and confirm the failure message matches "dependency changed" rather than something else, per that gate's own documented contract in `docs/DELIVERY_PLAN.md`'s Performance gate section.
- Record any genuinely-undecided implementation-fork decision as a new `docs/DECISIONS.md` entry. Re-check the current highest `D-0NN` ID in the actual file before picking a number — at the time this plan was drafted, D-087 was the last *merged* decision, with D-088 and D-089 open as accepted-but-unmerged PRs (#184, #185); by the time Task 1 runs, verify via `git log`/`gh pr view` whether those have merged (and whether a concurrent `main` PR claimed a number in between, exactly like PR-5's and PR-6's own plans had to handle) before committing to "D-090" as this plan assumes.
- Every out-of-scope construct still gets an explicit panic or documented gap, never a silently wrong result — this project's standing convention.
- Follow the existing TDD-per-task discipline: write failing test, verify it fails, implement, verify it passes, full workspace test+clippy+coverage, commit, push, then a docs-only commit flipping that task's plan checkboxes.
- `pycc.toml`'s full project-mode directory resolution (`docs/CLI_SPEC.md`: "PATH = file or project directory... once project mode exists") remains explicitly deferred — this PR gives `pycc.toml` real parsing, validation, `pycc init` scaffolding, and a narrow consumption point (Task 2's Step 8), not full directory-based project resolution.
- Known, accepted v0.1 gaps documented in `docs/ROADMAP.md`'s "Language surface" row are not bugs — the nbody fixture (Task 5) must avoid exercising any of them (no bigint/float mixing, no negative exponents, stay within CPython's non-scientific float-formatting range).

---

## Task 1: Record the three implementation-fork decisions this PR makes

**Files:**
- Modify: `docs/DECISIONS.md` (new ADR)

**Interfaces:**
- Produces: the accepted decision ID (assumed `D-090` below — re-verify per the Global Constraints note above) every later task's commit messages and doc updates cite.

- [ ] **Step 1: Re-verify the current highest decision ID**

Run: `git fetch origin --prune && git log origin/main --oneline -5` and `grep -n "^| D-0" docs/DECISIONS.md | tail -5` against a freshly-checked-out `origin/main`. Confirm whether #184 (D-088) and #185 (D-089) have merged. If a concurrent PR claimed a number this plan didn't anticipate, renumber every reference below (and in later tasks) the same way `docs/superpowers/plans/2026-07-26-pr6-conformance-benchmark-gate.md`'s own Global Constraints note records that PR having to do it three times.

- [ ] **Step 2: Write the ADR**

Append to `docs/DECISIONS.md` (adjust the number per Step 1):

```markdown
## D-090: PR-8 implementation choices — TOML dependency, nbody harness shape, release-profile mechanism

- Status: accepted
- Context: PR-8 (`docs/DELIVERY_PLAN.md`'s v0.2 row 8) needs three things the design spec (`docs/superpowers/specs/2026-07-28-v0-2-collections-generics-design.md`) deliberately left open: what parses `pycc.toml`'s already-specified TOML schema (`docs/CLI_SPEC.md`), what shape the nbody benchmark harness takes, and what LLVM mechanism `--release` actually invokes.
- Decision:
  1. **TOML parsing**: the `toml` crate (serde-based, the de facto standard) is added as a new dependency of the root `pycc` package. Verified absent from `Cargo.lock` today (`grep -c '^name = "toml"' Cargo.lock` returns 0). A new `PyccToml` struct (in `src/project_config.rs`) derives `serde::Deserialize` for exactly the fields v0.2 reads (`[project]` `name`/`entry`/`python`, `[build]` `opt`/`targets`/`static`) — `[interop]`/`[test]`, which `docs/CLI_SPEC.md` documents for v0.7/later, are not modeled as structs at all; serde's default behavior silently ignores unmodeled TOML sections, so a user's file following the full documented schema parses without error even though those sections do nothing yet.
  2. **nbody harness shape**: a plain `tests/nbody_bench.rs` integration test, matching `tests/conformance.rs`'s `std::process::Command`-based pattern (D-085's precedent for exactly this "spawn pycc and a CPython oracle, compare something" shape) rather than a new crate or `benches/check_bench.rs`'s Criterion machinery. `#[ignore]`d (like the conformance tests) since it builds a `--release` binary and runs both programs multiple times — genuinely slow, run explicitly via `--include-ignored` in CI, not on every local `cargo test`.
  3. **`--release`'s optimization mechanism**: verified via `cargo doc -p inkwell --no-deps` that the installed `inkwell` 0.9 exposes `Module::run_passes(passes: &str, machine: &TargetMachine, options: PassBuilderOptions) -> Result<(), LLVMString>`. `--release` creates the target machine with `OptimizationLevel::Aggressive` (currently hardcoded to `OptimizationLevel::None` in `crates/pycc_codegen/src/lib.rs`'s `compile_to_object`) and calls `module.run_passes("default<O3>", &target_machine, PassBuilderOptions::create())` before writing the object file; `--debug` (today's only behavior) skips `run_passes` entirely, unchanged. **True cross-translation-unit LTO has no effect yet**: pycc emits exactly one LLVM module per compilation today (single-file only through v0.4's multi-file work) — `docs/CLI_SPEC.md`'s "LTO" is honored for v0.2 as "maximum whole-module optimization" (there is only one module to optimize), not literal cross-file link-time optimization, which becomes meaningful once multi-file compilation exists. This is stated explicitly rather than silently claiming an LTO benefit that can't exist yet.
- Alternatives: model the full `pycc.toml` schema including `[interop]`/`[test]` now (rejected — those sections govern v0.7/later features with no consuming code yet; modeling unused struct fields is speculative scope, contrary to D-057's "simplest correct thing" precedent). Use `benches/check_bench.rs`'s Criterion harness for nbody too (rejected — that harness's whole design, including its "exact benchmark revision" integrity check, exists for a *different* threat model: comparing `pycc check`'s own speed across two commits on potentially different runners. nbody is a same-machine, same-run comparison between two different programs; reusing that machinery would mean fighting an integrity check built for a problem this measurement doesn't have). Pursue genuine multi-module LTO now (rejected — there is no second module yet for it to apply to; the work would be unverifiable until v0.4).
- Consequences: `Cargo.lock` gains `toml`+`serde` (and their transitive deps) as a real, reviewable dependency change — the existing `frontend-perf-*` gate's benchmark-revision integrity check will correctly flag this once; that is expected, not a bug to route around. `docs/CLI_SPEC.md`'s "LTO" line may eventually need a footnote once v0.4 makes real cross-file LTO possible, at which point this decision's "no effect yet" framing is superseded, not silently forgotten.
```

- [ ] **Step 3: Verify roadmap-evidence and agent-policy checks still pass**

Run: `ruby scripts/check_roadmap_evidence.rb` (force `LC_ALL=en_US.UTF-8 LANG=en_US.UTF-8` if it crashes on non-ASCII — a known, separately-tracked locale bug) and `python3 scripts/validate_agent_policies.py`. Expected: both pass (prose-only change, no hook/config touched).

- [ ] **Step 4: Commit**

```bash
git add docs/DECISIONS.md
git commit -m "Record D-090: PR-8's TOML dependency, nbody harness shape, and release-profile mechanism"
```

---

## Task 2: `pycc.toml` parsing + `pycc init` scaffolding

**Files:**
- Create: `src/project_config.rs` (with an inline `#[cfg(test)] mod tests` block — see Step 1's note)
- Modify: `Cargo.toml` (root package `[dependencies]`: add `toml`, `serde` with the `derive` feature)
- Modify: `src/main.rs` (wire `mod project_config;`, implement the `Command::Init` arm)
- Modify: `src/cli.rs` (no signature change needed — `Init { name: Option<String> }` already exists)

**Interfaces:**
- Produces: `pub struct PyccToml { pub project: ProjectSection, pub build: BuildSection }` with `ProjectSection { name: String, entry: String, python: String }` and `BuildSection { opt: Option<String>, targets: Option<Vec<String>>, static_: Option<bool> }` (note: `static` is a Rust keyword — use `#[serde(rename = "static")] pub static_: Option<bool>`), a `pub fn parse(contents: &str) -> Result<PyccToml, String>` that also validates `project.python == "3.14"` (per D-012 — anything else is a validation error, not a silent accept), and `pub fn scaffold(name: Option<&str>, dir: &Path) -> std::io::Result<()>` that writes a starter `pycc.toml` + `src/main.py`.
- Consumes: nothing from earlier tasks.

- [ ] **Step 1: Write the failing parse test**

**Verified**: the root `pycc` package (`Cargo.toml`) is `[[bin]]`-only — there is no `[lib]` target. `tests/conformance.rs` and every other file under `tests/` reach the compiled binary via `std::process::Command` + `env!("CARGO_BIN_EXE_pycc")`, never by linking the crate directly, because there is nothing to link against. `src/cli.rs` and `src/source.rs` both establish this repo's actual convention for testing a `src/`-level module: an inline `#[cfg(test)] mod tests` block in the same file, using `super::*`/`super::{...}` — not a separate `tests/*.rs` integration test. Follow that convention here; do not add a `[lib]` target just to make a separate integration test file possible.

```rust
// bottom of src/project_config.rs, alongside the implementation from Step 4
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_minimal_valid_pycc_toml() {
        let toml = r#"
[project]
name = "myapp"
entry = "src/main.py"
python = "3.14"

[build]
opt = "release"
targets = ["x86_64-unknown-linux-gnu"]
static = true
"#;
        let config = parse(toml).expect("valid pycc.toml should parse");
        assert_eq!(config.project.name, "myapp");
        assert_eq!(config.project.entry, "src/main.py");
        assert_eq!(config.project.python, "3.14");
        assert_eq!(config.build.opt.as_deref(), Some("release"));
        assert_eq!(
            config.build.targets.as_deref(),
            Some(&["x86_64-unknown-linux-gnu".to_string()][..])
        );
        assert_eq!(config.build.static_, Some(true));
    }

    #[test]
    fn rejects_an_unsupported_python_version() {
        let toml = r#"
[project]
name = "myapp"
entry = "src/main.py"
python = "3.15"
"#;
        let err = parse(toml).expect_err("python != 3.14 must be rejected in v1");
        assert!(err.contains("3.14"), "error should mention the only supported version: {err}");
    }

    #[test]
    fn accepts_a_file_with_not_yet_implemented_sections() {
        // [interop] and [test] are documented in docs/CLI_SPEC.md for later
        // milestones -- a file using the full schema must still parse today.
        let toml = r#"
[project]
name = "myapp"
entry = "src/main.py"
python = "3.14"

[interop]
allow = ["numpy"]

[test]
paths = ["tests/"]
"#;
        parse(toml).expect("documented-but-not-yet-consumed sections must not fail parsing");
    }

    #[test]
    fn rejects_malformed_toml_syntax() {
        let err = parse("this is not [valid toml").expect_err("malformed TOML must be rejected");
        assert!(!err.is_empty());
    }

    #[test]
    fn scaffold_writes_a_valid_pycc_toml_and_main_py() {
        let dir = std::env::temp_dir().join(format!("pycc_init_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        scaffold(Some("scaffoldtest"), &dir).expect("scaffold should succeed");

        let toml_contents = std::fs::read_to_string(dir.join("pycc.toml")).unwrap();
        let config = parse(&toml_contents).expect("scaffolded pycc.toml must itself parse");
        assert_eq!(config.project.name, "scaffoldtest");

        let main_py = std::fs::read_to_string(dir.join("src").join("main.py")).unwrap();
        assert!(main_py.contains("def main"));

        std::fs::remove_dir_all(&dir).ok();
    }
}
```

This single `mod tests` block covers both `parse` and `scaffold` — they're added together in Step 4 below since `scaffold`'s own test round-trips through `parse`, so splitting them into two separate red-green cycles would just mean re-declaring the same test module twice. Do not create a second `#[cfg(test)] mod tests` block later in this file — extend this one.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p pycc project_config`
Expected: FAIL — `project_config` module doesn't exist yet (compile error: `parse`/`PyccToml` unresolved).

- [ ] **Step 3: Add the `toml`/`serde` dependencies**

In the root `Cargo.toml`'s `[dependencies]`:
```toml
serde = { version = "1", features = ["derive"] }
toml = "0.8"
```
Run `cargo build` once to update `Cargo.lock` with the new dependency tree.

- [ ] **Step 4: Implement `src/project_config.rs`**

```rust
use serde::Deserialize;

#[derive(Debug, Deserialize, PartialEq)]
pub struct PyccToml {
    pub project: ProjectSection,
    #[serde(default)]
    pub build: BuildSection,
}

#[derive(Debug, Deserialize, PartialEq)]
pub struct ProjectSection {
    pub name: String,
    pub entry: String,
    pub python: String,
}

#[derive(Debug, Deserialize, PartialEq, Default)]
pub struct BuildSection {
    pub opt: Option<String>,
    pub targets: Option<Vec<String>>,
    #[serde(rename = "static")]
    pub static_: Option<bool>,
}

/// v1 accepts exactly Python 3.14 (D-012) -- a `pycc.toml` naming any other
/// version is a validation error, not a silent accept of an unsupported
/// language level.
pub fn parse(contents: &str) -> Result<PyccToml, String> {
    let config: PyccToml = toml::from_str(contents).map_err(|e| e.to_string())?;
    if config.project.python != "3.14" {
        return Err(format!(
            "pycc.toml: unsupported python version `{}` -- v1 accepts exactly \"3.14\" (D-012)",
            config.project.python
        ));
    }
    Ok(config)
}

/// `pycc init [NAME]`: scaffolds a starter `pycc.toml` + `src/main.py` in
/// `dir`. `name` defaults to `dir`'s own file-name component when omitted,
/// matching how `cargo init`/`npm init` derive a project name from the
/// target directory.
pub fn scaffold(name: Option<&str>, dir: &std::path::Path) -> std::io::Result<()> {
    let project_name = name
        .map(str::to_string)
        .or_else(|| dir.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "myapp".to_string());

    let toml_contents = format!(
        "[project]\nname = \"{project_name}\"\nentry = \"src/main.py\"\npython = \"3.14\"\n"
    );
    std::fs::write(dir.join("pycc.toml"), toml_contents)?;

    std::fs::create_dir_all(dir.join("src"))?;
    let main_py = "def main() -> None:\n    print(\"hello from pycc\")\n";
    std::fs::write(dir.join("src").join("main.py"), main_py)?;
    Ok(())
}

// The #[cfg(test)] mod tests block from Step 1 (parse tests + the
// scaffold_writes_a_valid_pycc_toml_and_main_py test) goes at the bottom
// of this same file, unchanged from Step 1 -- do not declare it twice.
```

- [ ] **Step 5: Wire `mod project_config;` and the `Init` command arm in `src/main.rs`**

Add `mod project_config;` alongside the existing `mod cli;`/`mod source;` at the top of `src/main.rs`. Replace the `Command::Init { .. }` arm (currently folded into the shared "not yet implemented" arm alongside `Test`/`Explain`/`Clean`) with its own:
```rust
Command::Init { name } => {
    let cwd = std::env::current_dir().expect("current directory must be readable");
    match project_config::scaffold(name.as_deref(), &cwd) {
        Ok(()) => {
            println!("Created pycc.toml and src/main.py");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: pycc init failed: {e}");
            ExitCode::from(2)
        }
    }
}
Command::Test | Command::Explain { .. } | Command::Clean => {
    eprintln!("pycc: this subcommand is not yet implemented");
    ExitCode::from(2)
}
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p pycc project_config`.
Expected: all PASS (the four parse-related tests from Step 1, plus `scaffold_writes_a_valid_pycc_toml_and_main_py`).

- [ ] **Step 7: Verify coverage and clippy**

Run: `cargo clippy --workspace --all-targets -- -D warnings` (expect clean) and `cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100` (expect pass — Step 1's `rejects_malformed_toml_syntax` test already covers the `toml::from_str` error branch, so `.map_err(...)` is not a dead line; double check no other branch in `parse`/`scaffold` is missed).

- [ ] **Step 8: Give `pycc.toml` a narrow, real consumption point in `pycc build`**

Write a failing test proving that `pycc build` without an explicit `--release` flag, given a source file whose containing directory also has a `pycc.toml` with `[build] opt = "release"`, still builds in the release profile (Task 3 must land first for `--release`'s actual effect to be observable — if this task is executed before Task 3, write the test now and mark it pending/ignored with a comment citing Task 3, then un-ignore it once Task 3's flag exists). The consumption logic in `try_build` (`src/main.rs`): after resolving `path`, check for a `pycc.toml` in the same directory; if present and parses successfully, and no explicit `--release` was passed, use its `build.opt` as the default. An explicit CLI flag always overrides the file. This is NOT full project-mode directory resolution (`docs/CLI_SPEC.md`'s deferred "PATH = ... project directory" case) — it only reads a neighboring file's default, and only for the optimization profile.

- [ ] **Step 9: Commit**

```bash
git add src/project_config.rs src/main.rs Cargo.toml Cargo.lock
git commit -m "Add pycc.toml parsing, pycc init scaffolding, and a narrow default-profile consumption point"
```

---

## Task 3: `--release` CLI flag + LLVM optimization wiring

**Files:**
- Modify: `src/cli.rs` (`Command::Build` gains a `#[arg(long)] release: bool` field)
- Modify: `src/main.rs` (thread `release` through `try_build` into `compile_to_object`)
- Modify: `crates/pycc_codegen/src/lib.rs` (`compile_to_object`'s signature and the hardcoded `OptimizationLevel::None`)
- Modify: `crates/pycc_codegen/Cargo.toml` (none expected — `inkwell`'s `passes` module is already part of the existing `inkwell = { version = "0.9", ... }` dependency; confirm via `cargo doc -p inkwell --no-deps` before assuming no manifest change is needed)

**Interfaces:**
- Consumes: nothing from Task 2 directly (independent of `pycc.toml`'s existence — an explicit `--release` flag must work even with no `pycc.toml` present).
- Produces: `pycc_codegen::compile_to_object(mir: &MirModule, output_path: &Path, target_triple: Option<&str>, release: bool) -> Result<(), String>` — Task 2 Step 8 and Task 5's benchmark harness both call this with `release: bool` from here on.

- [ ] **Step 1: Re-verify the inkwell API this task depends on**

Run: `cargo doc -p inkwell --no-deps` then inspect `target/doc/inkwell/module/struct.Module.html` for `run_passes` and `target/doc/inkwell/passes/struct.PassBuilderOptions.html` for `create`. Confirm the signature `pub fn run_passes(&self, passes: &str, machine: &TargetMachine, options: PassBuilderOptions) -> Result<(), LLVMString>` still matches (D-090 recorded this against the version installed when this plan was drafted — re-verify since a `Cargo.lock` update in Task 2 could have changed the resolved `inkwell` version, though the manifest pins `"0.9"` so a breaking change is unlikely within that range).

- [ ] **Step 2: Write the failing test proving `--release` measurably changes the emitted object**

```rust
// in crates/pycc_codegen/src/lib.rs's existing #[cfg(test)] mod tests block
#[test]
fn release_mode_actually_runs_llvm_optimization_passes() {
    // A compute-heavy but v0.1-grammar-only function: repeated float
    // multiplication in a loop, the kind O3 constant-folds/vectorizes
    // very differently from an unoptimized build. Compare the emitted
    // object file's size as a coarse, environment-independent proxy for
    // "optimization passes actually ran" -- avoids a flaky timing
    // assertion inside a unit test.
    let source = r#"
def main() -> None:
    x: float = 1.0
    i: int = 0
    while i < 1000:
        x = x * 1.0000001
        i = i + 1
    print(x)
"#;
    let debug_obj = compile_fixture_to_object(source, /* release */ false);
    let release_obj = compile_fixture_to_object(source, /* release */ true);
    assert_ne!(
        debug_obj, release_obj,
        "release and debug object files must differ -- optimization passes did not run"
    );
}
```
(`compile_fixture_to_object` is a test helper this file's existing tests likely already have some variant of for "compile this source string to an object and read it back" -- reuse whatever helper the file's existing codegen tests use for building a `MirModule` from a source string and calling `compile_to_object`, rather than duplicating that plumbing; check the existing test module for the established pattern before writing a new one.)

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p pycc_codegen release_mode_actually_runs_llvm_optimization_passes`
Expected: FAIL — `compile_to_object` doesn't yet accept a `release` parameter (compile error), or if a stub is added first with no behavior change, the two object files are identical.

- [ ] **Step 4: Add the `release` parameter and wire the optimization passes**

In `crates/pycc_codegen/src/lib.rs`, change the signature (doc comment updated to match):
```rust
pub fn compile_to_object(
    mir: &MirModule,
    output_path: &Path,
    target_triple: Option<&str>,
    release: bool,
) -> Result<(), String> {
```
Change the hardcoded optimization level:
```rust
let target_machine = target
    .create_target_machine(
        &triple,
        "generic",
        "",
        if release { OptimizationLevel::Aggressive } else { OptimizationLevel::None },
        RelocMode::PIC,
        CodeModel::Default,
    )
    .expect(/* unchanged message */);
```
Immediately before the existing `target_machine.write_to_file(&module, FileType::Object, output_path)` call, add:
```rust
if release {
    module
        .run_passes("default<O3>", &target_machine, PassBuilderOptions::create())
        .map_err(llvm_string_to_owned)?;
}
```
Add the necessary `use inkwell::passes::PassBuilderOptions;` import at the top of the file.

- [ ] **Step 5: Update every existing call site of `compile_to_object`**

Run: `grep -rn "compile_to_object(" --include="*.rs" .` to find every caller (expected: `src/main.rs`'s `try_build`, and any test helper inside `crates/pycc_codegen/src/lib.rs`'s own test module). Update each to pass `false` for `release` except the ones this task's own new test and Task 5's harness explicitly want `true`.

- [ ] **Step 6: Thread `--release` through the CLI**

In `src/cli.rs`, add to `Command::Build`:
```rust
Build {
    path: String,
    #[arg(short = 'o')]
    out: String,
    #[arg(long)]
    target: Option<String>,
    /// Enable LLVM optimization (O3-equivalent whole-module pipeline).
    /// True cross-file LTO has no effect yet -- pycc compiles exactly one
    /// module per invocation until v0.4's multi-file support lands (D-090).
    #[arg(long)]
    release: bool,
},
```
In `src/main.rs`'s `main()` match arm and `try_build`'s signature, thread `release: bool` through to the `compile_to_object` call (combining with Task 2 Step 8's `pycc.toml`-derived default: explicit `--release` always wins; absent the flag, fall back to a neighboring `pycc.toml`'s `build.opt == "release"`; absent both, `false`).

- [ ] **Step 7: Run tests to verify they pass**

Run: `cargo test -p pycc_codegen release_mode_actually_runs_llvm_optimization_passes` (PASS) and the full `cargo test --workspace` (no regressions from the new required parameter at every call site).

- [ ] **Step 8: Verify coverage and clippy**

Run: `cargo clippy --workspace --all-targets -- -D warnings` and `cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`. The new `if release { ... }` branch needs both a `true` and a `false` path exercised somewhere in the existing/new test suite for region coverage.

- [ ] **Step 9: Commit**

```bash
git add src/cli.rs src/main.rs crates/pycc_codegen/src/lib.rs
git commit -m "Wire --release to LLVM's O3 optimization pipeline via inkwell's run_passes"
```

---

## Task 4: nbody fixture (hand-adapted, v0.1-grammar-only)

**Files:**
- Create: `tests/fixtures/nbody.py`

**Interfaces:**
- Consumes: nothing (a plain Python source fixture).
- Produces: `tests/fixtures/nbody.py`, read by Task 5's harness and also runnable directly by the pinned CPython oracle for the differential comparison.

- [ ] **Step 1: Confirm v0.1's actual shipped grammar before writing the fixture**

Read `docs/ROADMAP.md`'s "Language surface" row and skim `tests/slice1_codegen_depth.rs`'s existing fixtures (the `mandelbrot_ascii_...` test in particular) to confirm: functions with `float` parameters and return values, `while`, `if`/`elif`/`else`, arithmetic (`+ - * /`), comparisons, and `print` all work; **no** `list`/`dict`/`tuple`/`set` and no destructuring assignment (`a, b = ...`) exist yet (containers are PR-10/11's job). This fixture must use only scalar `float` variables.

- [ ] **Step 2: Write the fixture**

`tests/fixtures/nbody.py` — a hand-adapted rewrite of `pyperformance`'s `bm_nbody` (D-090/design-spec §1: same 5 bodies, same physical constants, same `advance`/`report_energy` structure), fully unrolled into named scalar variables since there is no container type to hold a list of bodies yet:

```python
def main() -> None:
    pi: float = 3.14159265358979323
    solar_mass: float = 4.0 * pi * pi
    days_per_year: float = 365.24

    # Position (x, y, z), velocity (vx, vy, vz), mass -- one variable each,
    # per body, since v0.1/early-v0.2 has no list/tuple to group them yet
    # (PR-10/11's job). Values are pyperformance's bm_nbody constants
    # verbatim (jupiter/saturn/uranus/neptune; sun's own velocity is fixed
    # up after the fact to offset momentum, exactly like the original).
    sun_x: float = 0.0
    sun_y: float = 0.0
    sun_z: float = 0.0
    sun_vx: float = 0.0
    sun_vy: float = 0.0
    sun_vz: float = 0.0
    sun_mass: float = solar_mass

    jupiter_x: float = 4.84143144246472090
    jupiter_y: float = -1.16032004402742839
    jupiter_z: float = -1.03622044471123109e-01
    jupiter_vx: float = 1.66007664274403694e-03 * days_per_year
    jupiter_vy: float = 7.69901118419740425e-03 * days_per_year
    jupiter_vz: float = -6.90460016972063023e-05 * days_per_year
    jupiter_mass: float = 9.54791938424326609e-04 * solar_mass

    saturn_x: float = 8.34336671824457987
    saturn_y: float = 4.12479856412430479
    saturn_z: float = -4.03523417114321381e-01
    saturn_vx: float = -2.76742510726862411e-03 * days_per_year
    saturn_vy: float = 4.99852801234917238e-03 * days_per_year
    saturn_vz: float = 2.30417297573763929e-05 * days_per_year
    saturn_mass: float = 2.85885980666130812e-04 * solar_mass

    uranus_x: float = 1.28943695621391310e01
    uranus_y: float = -1.51111514016986312e01
    uranus_z: float = -2.23307578892655734e-01
    uranus_vx: float = 2.96460137564761618e-03 * days_per_year
    uranus_vy: float = 2.37847173959480950e-03 * days_per_year
    uranus_vz: float = -2.96589568540237556e-05 * days_per_year
    uranus_mass: float = 4.36624404335156298e-05 * solar_mass

    neptune_x: float = 1.53796971148509165e01
    neptune_y: float = -2.59193146099879641e01
    neptune_z: float = 1.79258772950371181e-01
    neptune_vx: float = 2.68067772490389322e-03 * days_per_year
    neptune_vy: float = 1.62824170038242295e-03 * days_per_year
    neptune_vz: float = -9.51592254519715870e-05 * days_per_year
    neptune_mass: float = 5.15138902046611451e-05 * solar_mass

    # offset_momentum: sun's velocity absorbs the system's total momentum,
    # exactly like pyperformance's own offset_momentum(SYSTEM[0], *SYSTEM[1:]).
    sun_vx = 0.0 - (
        jupiter_vx * jupiter_mass
        + saturn_vx * saturn_mass
        + uranus_vx * uranus_mass
        + neptune_vx * neptune_mass
    ) / solar_mass
    sun_vy = 0.0 - (
        jupiter_vy * jupiter_mass
        + saturn_vy * saturn_mass
        + uranus_vy * uranus_mass
        + neptune_vy * neptune_mass
    ) / solar_mass
    sun_vz = 0.0 - (
        jupiter_vz * jupiter_mass
        + saturn_vz * saturn_mass
        + uranus_vz * uranus_mass
        + neptune_vz * neptune_mass
    ) / solar_mass

    dt: float = 0.01
    iterations: int = 20000
    step: int = 0
    while step < iterations:
        # Pairwise gravitational update -- 10 pairs for 5 bodies, unrolled
        # (no list/enumerate/itertools.combinations available yet).
        # Pair: sun/jupiter
        dx: float = sun_x - jupiter_x
        dy: float = sun_y - jupiter_y
        dz: float = sun_z - jupiter_z
        d2: float = dx * dx + dy * dy + dz * dz
        mag: float = dt * (d2 ** (-1.5))
        sun_vx = sun_vx - dx * jupiter_mass * mag
        sun_vy = sun_vy - dy * jupiter_mass * mag
        sun_vz = sun_vz - dz * jupiter_mass * mag
        jupiter_vx = jupiter_vx + dx * sun_mass * mag
        jupiter_vy = jupiter_vy + dy * sun_mass * mag
        jupiter_vz = jupiter_vz + dz * sun_mass * mag

        # Pair: sun/saturn
        dx = sun_x - saturn_x
        dy = sun_y - saturn_y
        dz = sun_z - saturn_z
        d2 = dx * dx + dy * dy + dz * dz
        mag = dt * (d2 ** (-1.5))
        sun_vx = sun_vx - dx * saturn_mass * mag
        sun_vy = sun_vy - dy * saturn_mass * mag
        sun_vz = sun_vz - dz * saturn_mass * mag
        saturn_vx = saturn_vx + dx * sun_mass * mag
        saturn_vy = saturn_vy + dy * sun_mass * mag
        saturn_vz = saturn_vz + dz * sun_mass * mag

        # Pair: sun/uranus
        dx = sun_x - uranus_x
        dy = sun_y - uranus_y
        dz = sun_z - uranus_z
        d2 = dx * dx + dy * dy + dz * dz
        mag = dt * (d2 ** (-1.5))
        sun_vx = sun_vx - dx * uranus_mass * mag
        sun_vy = sun_vy - dy * uranus_mass * mag
        sun_vz = sun_vz - dz * uranus_mass * mag
        uranus_vx = uranus_vx + dx * sun_mass * mag
        uranus_vy = uranus_vy + dy * sun_mass * mag
        uranus_vz = uranus_vz + dz * sun_mass * mag

        # Pair: sun/neptune
        dx = sun_x - neptune_x
        dy = sun_y - neptune_y
        dz = sun_z - neptune_z
        d2 = dx * dx + dy * dy + dz * dz
        mag = dt * (d2 ** (-1.5))
        sun_vx = sun_vx - dx * neptune_mass * mag
        sun_vy = sun_vy - dy * neptune_mass * mag
        sun_vz = sun_vz - dz * neptune_mass * mag
        neptune_vx = neptune_vx + dx * sun_mass * mag
        neptune_vy = neptune_vy + dy * sun_mass * mag
        neptune_vz = neptune_vz + dz * sun_mass * mag

        # Pair: jupiter/saturn
        dx = jupiter_x - saturn_x
        dy = jupiter_y - saturn_y
        dz = jupiter_z - saturn_z
        d2 = dx * dx + dy * dy + dz * dz
        mag = dt * (d2 ** (-1.5))
        jupiter_vx = jupiter_vx - dx * saturn_mass * mag
        jupiter_vy = jupiter_vy - dy * saturn_mass * mag
        jupiter_vz = jupiter_vz - dz * saturn_mass * mag
        saturn_vx = saturn_vx + dx * jupiter_mass * mag
        saturn_vy = saturn_vy + dy * jupiter_mass * mag
        saturn_vz = saturn_vz + dz * jupiter_mass * mag

        # Pair: jupiter/uranus
        dx = jupiter_x - uranus_x
        dy = jupiter_y - uranus_y
        dz = jupiter_z - uranus_z
        d2 = dx * dx + dy * dy + dz * dz
        mag = dt * (d2 ** (-1.5))
        jupiter_vx = jupiter_vx - dx * uranus_mass * mag
        jupiter_vy = jupiter_vy - dy * uranus_mass * mag
        jupiter_vz = jupiter_vz - dz * uranus_mass * mag
        uranus_vx = uranus_vx + dx * jupiter_mass * mag
        uranus_vy = uranus_vy + dy * jupiter_mass * mag
        uranus_vz = uranus_vz + dz * jupiter_mass * mag

        # Pair: jupiter/neptune
        dx = jupiter_x - neptune_x
        dy = jupiter_y - neptune_y
        dz = jupiter_z - neptune_z
        d2 = dx * dx + dy * dy + dz * dz
        mag = dt * (d2 ** (-1.5))
        jupiter_vx = jupiter_vx - dx * neptune_mass * mag
        jupiter_vy = jupiter_vy - dy * neptune_mass * mag
        jupiter_vz = jupiter_vz - dz * neptune_mass * mag
        neptune_vx = neptune_vx + dx * jupiter_mass * mag
        neptune_vy = neptune_vy + dy * jupiter_mass * mag
        neptune_vz = neptune_vz + dz * jupiter_mass * mag

        # Pair: saturn/uranus
        dx = saturn_x - uranus_x
        dy = saturn_y - uranus_y
        dz = saturn_z - uranus_z
        d2 = dx * dx + dy * dy + dz * dz
        mag = dt * (d2 ** (-1.5))
        saturn_vx = saturn_vx - dx * uranus_mass * mag
        saturn_vy = saturn_vy - dy * uranus_mass * mag
        saturn_vz = saturn_vz - dz * uranus_mass * mag
        uranus_vx = uranus_vx + dx * saturn_mass * mag
        uranus_vy = uranus_vy + dy * saturn_mass * mag
        uranus_vz = uranus_vz + dz * saturn_mass * mag

        # Pair: saturn/neptune
        dx = saturn_x - neptune_x
        dy = saturn_y - neptune_y
        dz = saturn_z - neptune_z
        d2 = dx * dx + dy * dy + dz * dz
        mag = dt * (d2 ** (-1.5))
        saturn_vx = saturn_vx - dx * neptune_mass * mag
        saturn_vy = saturn_vy - dy * neptune_mass * mag
        saturn_vz = saturn_vz - dz * neptune_mass * mag
        neptune_vx = neptune_vx + dx * saturn_mass * mag
        neptune_vy = neptune_vy + dy * saturn_mass * mag
        neptune_vz = neptune_vz + dz * saturn_mass * mag

        # Pair: uranus/neptune
        dx = uranus_x - neptune_x
        dy = uranus_y - neptune_y
        dz = uranus_z - neptune_z
        d2 = dx * dx + dy * dy + dz * dz
        mag = dt * (d2 ** (-1.5))
        uranus_vx = uranus_vx - dx * neptune_mass * mag
        uranus_vy = uranus_vy - dy * neptune_mass * mag
        uranus_vz = uranus_vz - dz * neptune_mass * mag
        neptune_vx = neptune_vx + dx * uranus_mass * mag
        neptune_vy = neptune_vy + dy * uranus_mass * mag
        neptune_vz = neptune_vz + dz * uranus_mass * mag

        # Position update for all 5 bodies.
        sun_x = sun_x + dt * sun_vx
        sun_y = sun_y + dt * sun_vy
        sun_z = sun_z + dt * sun_vz
        jupiter_x = jupiter_x + dt * jupiter_vx
        jupiter_y = jupiter_y + dt * jupiter_vy
        jupiter_z = jupiter_z + dt * jupiter_vz
        saturn_x = saturn_x + dt * saturn_vx
        saturn_y = saturn_y + dt * saturn_vy
        saturn_z = saturn_z + dt * saturn_vz
        uranus_x = uranus_x + dt * uranus_vx
        uranus_y = uranus_y + dt * uranus_vy
        uranus_z = uranus_z + dt * uranus_vz
        neptune_x = neptune_x + dt * neptune_vx
        neptune_y = neptune_y + dt * neptune_vy
        neptune_z = neptune_z + dt * neptune_vz

        step = step + 1

    print(sun_x, sun_y, sun_z)
```

- [ ] **Step 3: Verify the fixture runs correctly under CPython first**

Run: `python3.14 tests/fixtures/nbody.py`
Expected: prints three floating-point numbers (the sun's final position) with no error. If `**` with a negative float exponent hits any CPython edge case, adjust `d2 ** (-1.5)` to `1.0 / (d2 ** 1.5)`-style equivalent and re-verify — this fixture must not exercise any of `docs/ROADMAP.md`'s documented v0.1 gaps (it doesn't use bigint, only `float`, so this is not expected to be an issue, but confirm empirically rather than assuming).

- [ ] **Step 4: Verify the fixture compiles and runs under pycc**

Run: `cargo run -- build tests/fixtures/nbody.py -o /tmp/nbody_test && /tmp/nbody_test`
Expected: same three floating-point numbers CPython printed in Step 3 (allowing for pycc's own documented float-formatting boundary from `docs/ROADMAP.md`'s Language surface row — if the output differs only in formatting and not value, note this and move on; if values genuinely differ, stop and investigate before proceeding, since Task 5's whole harness depends on this fixture producing identical output).

- [ ] **Step 5: Commit**

```bash
git add tests/fixtures/nbody.py
git commit -m "Add hand-adapted nbody fixture using only pycc's shipped v0.1 grammar"
```

---

## Task 5: nbody measurement harness (`tests/nbody_bench.rs`)

**Files:**
- Create: `tests/nbody_bench.rs`

**Interfaces:**
- Consumes: `tests/fixtures/nbody.py` (Task 4), the `--release` flag (Task 3), the pinned `python3.14` oracle (already established by `tests/conformance.rs`).
- Produces: an `#[ignore]`d test that fails if the measured ratio is `< 20.0`, matching the design spec's §1 gate.

- [ ] **Step 1: Read `tests/conformance.rs` in full first**

This harness reuses its subprocess-invocation pattern (building the `pycc` binary, spawning `python3.14`, matching the exact oracle-version-check convention already established there) — do not reinvent argument-passing or oracle-verification logic that file already has correct.

- [ ] **Step 2: Write the failing test**

```rust
// tests/nbody_bench.rs
use std::process::Command;
use std::time::Instant;

const RUNS: usize = 5;

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(|a, b| a.partial_cmp(b).expect("timings are never NaN"));
    values[values.len() / 2]
}

fn time_command(mut command: Command) -> f64 {
    let start = Instant::now();
    let status = command.status().expect("command must spawn");
    let elapsed = start.elapsed().as_secs_f64();
    assert!(status.success(), "command failed: {command:?}");
    elapsed
}

#[test]
#[ignore] // slow: builds a --release binary and runs both programs 5 times each
fn nbody_release_binary_is_at_least_20x_faster_than_cpython() {
    // Build the pycc binary once, --release, outside the timed loop.
    let bin_path = std::env::temp_dir().join(format!("pycc_nbody_{}", std::process::id()));
    let build_status = Command::new(env!("CARGO_BIN_EXE_pycc"))
        .args(["build", "tests/fixtures/nbody.py", "-o"])
        .arg(&bin_path)
        .arg("--release")
        .status()
        .expect("pycc build must spawn");
    assert!(build_status.success(), "pycc --release build of nbody.py failed");

    let pycc_times: Vec<f64> = (0..RUNS)
        .map(|_| time_command(Command::new(&bin_path)))
        .collect();
    let cpython_times: Vec<f64> = (0..RUNS)
        .map(|_| time_command(Command::new("python3.14").arg("tests/fixtures/nbody.py")))
        .collect();

    let pycc_median = median(pycc_times);
    let cpython_median = median(cpython_times);
    let ratio = cpython_median / pycc_median;

    assert!(
        ratio >= 20.0,
        "nbody speedup ratio {ratio:.2}x is below the required 20x gate \
         (cpython median {cpython_median:.4}s, pycc --release median {pycc_median:.4}s)"
    );
}
```

- [ ] **Step 3: Run test to verify it currently fails or is skipped correctly**

Run: `cargo test --test nbody_bench -- --ignored`
Expected: runs the real build+measure+compare; if `--release` (Task 3) and the fixture (Task 4) are already merged by this point, this may PASS immediately -- if so, that is the desired outcome, not a sign the test is wrong. If it fails, read the printed ratio: a ratio well below 20 with `--release` active points to Task 3's optimization wiring not actually taking effect (re-verify Task 3 Step 2's object-file-diff test still passes); a ratio far above 20 is not a failure.

- [ ] **Step 4: Wire into CI**

Find the existing `build-test-coverage` (or equivalent) job in `.github/workflows/ci.yml` that already runs `cargo test --workspace ... --include-ignored` for the conformance tests (per D-078's established pattern). Confirm `tests/nbody_bench.rs`'s new test runs there too (it will, automatically, once `--include-ignored` is already passed workspace-wide — verify rather than assume by checking the actual current invocation). If the existing job doesn't already pass `--include-ignored`, or if `nbody_bench` needs to run on a subset of the matrix rather than all 5 targets (re-read the design spec — it does not restrict to one target, and `docs/ROADMAP.md`'s own opening line requires every milestone's acceptance criteria green on all Tier-1 platforms, so this runs on all 5 unless a concrete platform-specific reason surfaces during implementation, recorded via its own ADR if so), make the minimal CI change needed and record why via a new `docs/DECISIONS.md` entry if it's a genuinely new fork (e.g., if Windows subprocess-timing precision turns out to need special handling).

- [ ] **Step 5: Verify coverage and clippy**

Run: `cargo clippy --workspace --all-targets -- -D warnings` and the full coverage gate. `#[ignore]`d tests still need to compile cleanly under clippy even though they don't run by default.

- [ ] **Step 6: Commit**

```bash
git add tests/nbody_bench.rs .github/workflows/ci.yml
git commit -m "Add nbody same-machine paired-comparison benchmark, gated at 20x speedup"
```

---

## Task 6: Final docs sweep

**Files:**
- Modify: `docs/ROADMAP.md` (v0.2 section — note `--release`/`pycc.toml`/nbody as delivered, once genuinely green in CI; do not flip any binary acceptance checkbox that depends on PR-9 through PR-14's own work)
- Modify: `docs/DELIVERY_PLAN.md` (PR-8 row — mark delivered)
- Modify: `docs/CLI_SPEC.md` (only if Tasks 1-5 found `--release`'s or `pycc.toml`'s documented behavior to be inexact — e.g. if `[interop]`/`[test]` needed different wording once actually attempted; do not edit speculatively)

- [ ] **Step 1: Re-read every doc this PR touched for staleness**

`docs/CLI_SPEC.md`'s `--release` and `pycc.toml` sections, `docs/ROADMAP.md`'s v0.2 line, `docs/DELIVERY_PLAN.md`'s new v0.2 PR-8 row — confirm each still accurately describes what Tasks 1-5 actually built, not what was originally planned, the same self-check every prior PR in this repo's history performed before merge.

- [ ] **Step 2: Run the pinned local reviewer (D-068)**

Dispatch the pinned `ievo:deep-reviewer` agent against this PR's full diff (merge-base to HEAD) before opening/merging the PR. Fix every actionable finding and re-review if the diff changes materially, per `docs/AGENT_TOOLING.md`'s established process.

- [ ] **Step 3: Full workspace verification**

Run: `cargo test --workspace --include-ignored`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`, `ruby scripts/check_roadmap_evidence.rb`, `python3 scripts/validate_agent_policies.py`, `cargo doc --workspace --no-deps`. All must pass before merge.

- [ ] **Step 4: Open the PR and merge once CI is green on all 5 Tier-1 targets**

Follow this repo's established PR-opening convention (see any of #180-#186's bodies this same session for the exact style: Summary, evidence, Test plan checklist, `🤖 Generated with Claude Code` footer).
