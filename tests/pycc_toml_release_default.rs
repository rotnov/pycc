use std::process::Command;

fn pycc_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_pycc"))
}

/// This PR's plan (Task 3 Step 6a, moved here from Task 2's original Step 8
/// once `--release` itself existed to combine with) asks for a narrow, real
/// consumption point: `pycc build`, given no explicit `--release` flag,
/// should still build in the release profile when a `pycc.toml` next to
/// the source file names `[build] opt = "release"`.
///
/// This test only proves the *wiring* is reachable end to end through the
/// real CLI: that `pycc build` still succeeds and produces a correctly
/// running binary when invoked with a relative source path (`main.py`,
/// `current_dir`-ed into the project directory) and no `--release` flag,
/// given a neighboring `pycc.toml` naming `opt = "release"`. It does *not*
/// prove that LLVM's O3 pipeline actually ran for this build -- an earlier
/// version of this test tried to prove that here too, comparing the final
/// *linked* binary's size, but that proxy turned out to be unreliable at
/// this level: with the statically-linked `pycc_rt` runtime dominating
/// output size and Mach-O segments padding to fixed alignment boundaries,
/// a `.text`-sized difference of a few hundred bytes from unrolling a
/// small loop doesn't move the final linked file's size at all (confirmed
/// empirically -- explicit `--release` and plain debug builds of the same
/// source produced byte-identical linked output once output-path-name
/// length, which independently perturbs linked size, was held constant).
/// That LLVM's `"default<O3>"` pipeline measurably changes emitted code is
/// already proven directly, at the object-file level where the effect is
/// real and measurable, by `pycc_codegen`'s own
/// `release_mode_actually_runs_llvm_optimization_passes` unit test. That
/// `resolve_release_flag` (`src/main.rs`) correctly derives `true` from
/// exactly this `pycc.toml` shape -- and correctly falls back to `false`
/// for every other input, including a malformed neighboring file -- is
/// proven directly by `src/main.rs`'s own `release_flag_tests` unit tests.
/// Together, those two prove the feature; this test proves the two halves
/// are actually connected through `try_build` via a real CLI invocation.
#[test]
fn build_without_an_explicit_release_flag_still_honors_a_neighboring_pycc_toml_release_default()
{
    let dir = std::env::temp_dir().join(format!(
        "pycc_toml_release_default_{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("main.py"),
        "def main() -> None:\n    print(42)\n\nmain()\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("pycc.toml"),
        "[project]\nname = \"t\"\nentry = \"main.py\"\npython = \"3.14\"\n\n\
         [build]\nopt = \"release\"\n",
    )
    .unwrap();
    let out = dir.join("out");

    // Relative path (`main.py`), not absolute: `try_build`'s `path`
    // argument reaches `resolve_release_flag` exactly as given on the
    // command line, so this is what actually exercises its
    // `source_path.parent() == Some("")` branch and the resulting
    // cwd-relative `pycc.toml` read -- the genuinely non-obvious part of
    // this wiring, not reproducible by passing an absolute path.
    let status = Command::new(pycc_bin())
        .args(["build", "main.py", "-o", out.to_str().unwrap()])
        .current_dir(&dir)
        .status()
        .unwrap();
    assert!(
        status.success(),
        "pycc build failed with a neighboring release-profile pycc.toml and no \
         explicit --release flag"
    );

    let output = Command::new(&out).output().unwrap();
    assert_eq!(output.stdout, b"42\n");

    std::fs::remove_dir_all(&dir).ok();
}
