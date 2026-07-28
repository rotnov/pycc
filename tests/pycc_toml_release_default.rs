use std::process::Command;

fn pycc_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_pycc"))
}

/// A compute-heavy but v0.1-grammar-only function -- the same shape this
/// PR's Task 3 (`--release` CLI flag + LLVM optimization wiring) uses to
/// prove O3 passes actually ran: repeated float multiplication in a loop
/// is exactly the kind of code O3 constant-folds/vectorizes very
/// differently from an unoptimized build, making the emitted binary's
/// size a coarse, environment-independent proxy for "the release profile
/// was actually used" -- avoids a flaky timing assertion in a CLI-level
/// test.
const OPT_SENSITIVE_SOURCE: &str = r#"
def main() -> None:
    x = 1.0
    i = 0
    while i < 1000:
        x = x * 1.0000001
        i = i + 1
    print(x)
"#;

/// Task 2 Step 8 of this PR's plan
/// (`docs/superpowers/plans/2026-07-28-v0-2-pr8-release-profile-pycctoml-nbody.md`)
/// asks for a narrow, real consumption point: `pycc build`, given no
/// explicit `--release` flag, should still build in the release profile
/// when a `pycc.toml` next to the source file names `[build] opt =
/// "release"`. That flag does not exist in `src/cli.rs` yet -- it is
/// Task 3's own deliverable, landing together with `pycc_codegen`'s LLVM
/// `run_passes("default<O3>", ...)` wiring -- so there is nothing for
/// `try_build` to combine a file-derived default *with* yet, and no
/// optimization mechanism whose effect this test could observe even if
/// `try_build` read the file today. Per the plan's own contingency for
/// exactly this ordering, this test is written now (so it compiles
/// against today's `pycc.toml` parsing from Task 2) and left `#[ignore]`d
/// until Task 3 lands both the flag and the actual profile-selection
/// logic in `try_build` (that task's Step 6 explicitly folds in this
/// file's neighboring-`pycc.toml` default). Un-ignore only once that
/// wiring exists and this assertion can actually pass.
#[test]
#[ignore = "pending Task 3's --release flag + LLVM O3 wiring and its own \
            try_build consumption of a neighboring pycc.toml's build.opt \
            (this PR's plan, Task 3 Step 6) -- un-ignore once that lands"]
fn build_without_an_explicit_release_flag_still_honors_a_neighboring_pycc_toml_release_default()
{
    let with_toml_dir =
        std::env::temp_dir().join(format!("pycc_toml_release_default_{}", std::process::id()));
    std::fs::create_dir_all(&with_toml_dir).unwrap();
    std::fs::write(with_toml_dir.join("main.py"), OPT_SENSITIVE_SOURCE).unwrap();
    std::fs::write(
        with_toml_dir.join("pycc.toml"),
        "[project]\nname = \"t\"\nentry = \"main.py\"\npython = \"3.14\"\n\n\
         [build]\nopt = \"release\"\n",
    )
    .unwrap();
    let with_toml_out = with_toml_dir.join("out_with_toml");

    let without_toml_dir = std::env::temp_dir().join(format!(
        "pycc_toml_release_default_plain_{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&without_toml_dir).unwrap();
    std::fs::write(without_toml_dir.join("main.py"), OPT_SENSITIVE_SOURCE).unwrap();
    let without_toml_out = without_toml_dir.join("out_without_toml");

    // Neither invocation passes `--release` explicitly -- the flag does
    // not exist in the CLI yet. Once Task 3 adds it, `pycc build` run
    // inside `with_toml_dir` must still build in the release profile
    // purely because of the neighboring `pycc.toml`.
    let status_with_toml = Command::new(pycc_bin())
        .args(["build", "main.py", "-o", with_toml_out.to_str().unwrap()])
        .current_dir(&with_toml_dir)
        .status()
        .unwrap();
    assert!(status_with_toml.success());

    let status_without_toml = Command::new(pycc_bin())
        .args([
            "build",
            "main.py",
            "-o",
            without_toml_out.to_str().unwrap(),
        ])
        .current_dir(&without_toml_dir)
        .status()
        .unwrap();
    assert!(status_without_toml.success());

    let with_toml_len = std::fs::metadata(&with_toml_out).unwrap().len();
    let without_toml_len = std::fs::metadata(&without_toml_out).unwrap().len();
    assert_ne!(
        with_toml_len, without_toml_len,
        "a neighboring pycc.toml's `build.opt = \"release\"` must change the build \
         output even with no explicit --release flag -- differing binary size is a \
         coarse, environment-independent proxy for \"the release profile was \
         actually used\""
    );

    std::fs::remove_dir_all(&with_toml_dir).ok();
    std::fs::remove_dir_all(&without_toml_dir).ok();
}
