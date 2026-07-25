use std::io::Write;
use std::process::Command;

fn pycc_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_pycc"))
}

fn write_fixture(dir: &std::path::Path, name: &str, source: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(source.as_bytes()).unwrap();
    path
}

#[test]
fn a_missing_input_file_is_a_clean_error_not_a_panic() {
    // Regression test (found in PR review): an earlier version used
    // .expect() on read_to_string, so a typo'd path panicked (exit 101,
    // raw backtrace) instead of a clean CLI error.
    let dir = std::env::temp_dir().join(format!("pycc_e2e_missing_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let missing_path = dir.join("does_not_exist.py");
    let out = dir.join("out");

    let output = Command::new(pycc_bin())
        .args(["build", missing_path.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("could not read"));
}

#[test]
fn build_and_run_explicit_call_to_main() {
    // Python never auto-invokes a function merely because it's named
    // `main` -- the source has to call it, same as any other function.
    let dir = std::env::temp_dir().join(format!("pycc_e2e_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "hello.py", "def main() -> None:\n    print(42)\n\nmain()\n");
    let out = dir.join("hello");

    let status = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(status.success());

    let output = Command::new(&out).output().unwrap();
    assert_eq!(output.stdout, b"42\n");
}

#[test]
fn defining_main_without_calling_it_produces_no_output() {
    // Regression test for a confirmed bug (found in PR review, fixed in
    // the same PR): an earlier version of pycc_codegen treated a function
    // literally named `main` as an auto-invoked entry point, which isn't
    // real Python semantics -- CPython never runs a function just because
    // of its name. Verified manually against `python3.14` on this exact
    // source (not shelled out to from this test, since a hardcoded
    // interpreter path would be machine-specific and break CI; the real
    // conformance harness that runs this kind of check portably is
    // pycc_testkit, deferred per DECISIONS.md): zero bytes of stdout.
    let dir = std::env::temp_dir().join(format!("pycc_e2e_uncalled_main_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "hello_uncalled.py", "def main() -> None:\n    print(42)\n");
    let out = dir.join("hello_uncalled");

    let status = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(status.success());

    let output = Command::new(&out).output().unwrap();
    assert_eq!(output.stdout, b"", "must match CPython's actual output for this source");
}

#[test]
fn build_and_run_top_level_print_with_no_main() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_toplevel_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "hello_toplevel.py", "print(42)\n");
    let out = dir.join("hello_toplevel");

    let status = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(status.success());

    let output = Command::new(&out).output().unwrap();
    assert_eq!(output.stdout, b"42\n");
}

#[test]
fn run_subcommand_builds_and_executes_in_one_step() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_run_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "hello_run.py", "print(42)\n");

    let output = Command::new(pycc_bin()).args(["run", src.to_str().unwrap()]).output().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"42\n");
}

#[test]
fn run_subcommand_propagates_a_build_failure() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_run_fail_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "bad_run.py", "def main(:\n");

    let output = Command::new(pycc_bin()).args(["run", src.to_str().unwrap()]).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("L0001"));
}

#[test]
fn version_flag_prints_something() {
    let output = Command::new(pycc_bin()).args(["version", "--verbose"]).output().unwrap();
    assert!(output.status.success());
    assert!(!output.stdout.is_empty());
}

#[test]
fn unimplemented_subcommands_exit_with_code_2() {
    let output = Command::new(pycc_bin()).args(["clean"]).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn a_syntax_error_is_a_compile_error_exit_code_1() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_synerr_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "bad.py", "def main(:\n");
    let out = dir.join("bad");

    let output = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("L0001"));
}

#[test]
fn defining_a_function_under_any_name_without_calling_it_succeeds() {
    // There's no "must be named main" restriction: any function name is
    // legal to define; only calling one runs it (matches CPython, which
    // has no such restriction either).
    let dir = std::env::temp_dir().join(format!("pycc_e2e_anyfn_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "anyfn.py", "def helper() -> None:\n    print(1)\n");
    let out = dir.join("anyfn");

    let status = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(status.success());
    let output = Command::new(&out).output().unwrap();
    assert_eq!(output.stdout, b"");
}

#[test]
fn calling_an_undefined_function_is_a_codegen_error() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_undefined_fn_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "undefined_fn.py", "does_not_exist()\n");
    let out = dir.join("undefined_fn");

    let output = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("does_not_exist"));
}

/// `--target x86_64-apple-darwin`'s pycc_rt build (`rustup target add
/// x86_64-apple-darwin && cargo build --target x86_64-apple-darwin -p
/// pycc_rt`) is set up on the PR-3 CI job and on this repo's dev host, but
/// isn't guaranteed for every environment `cargo test` might run in --
/// skip cleanly rather than fail on an environment gap the rest of this
/// test suite doesn't require.
fn x86_64_apple_darwin_pycc_rt_is_available() -> bool {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/x86_64-apple-darwin/debug/libpycc_rt.a")
        .exists()
}

#[test]
fn build_and_run_cross_compiled_to_a_different_tier_1_target() {
    if !x86_64_apple_darwin_pycc_rt_is_available() {
        eprintln!("skipping: x86_64-apple-darwin pycc_rt build not available in this environment");
        return;
    }
    let dir = std::env::temp_dir().join(format!("pycc_e2e_cross_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "hello_cross.py", "print(42)\n");
    let out = dir.join("hello_cross");

    let status = Command::new(pycc_bin())
        .args([
            "build",
            src.to_str().unwrap(),
            "-o",
            out.to_str().unwrap(),
            "--target",
            "x86_64-apple-darwin",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let file_output = Command::new("file").arg(&out).output().unwrap();
    let description = String::from_utf8_lossy(&file_output.stdout);
    assert!(description.contains("x86_64"), "expected an x86_64 binary, got: {description}");

    // Runs via Rosetta 2 on this arm64 dev host / CI runner.
    let output = Command::new(&out).output().unwrap();
    assert_eq!(output.stdout, b"42\n");
}

/// Not real cross-compilation (the triple matches this runner's own arch)
/// -- this exercises the same --target code path (linker_command,
/// effective_link_target) using a triple that's available on every Linux
/// CI runner, unlike x86_64-apple-darwin above which only exists on the
/// macOS legs. Catches regressions like D-031 (GCC's `cc` rejecting
/// clang-only `-target` syntax on Linux) that only manifest once
/// --target reaches the actual link step -- which
/// targeting_a_valid_triple_with_no_local_pycc_rt_build_is_a_clean_error
/// above never reaches (it returns from find_pycc_rt_lib_dir before the
/// linker is ever invoked).
#[test]
fn build_and_run_with_target_set_to_the_host_s_own_triple_on_linux() {
    if std::env::consts::OS != "linux" {
        eprintln!("skipping: this test only applies on Linux (see D-031)");
        return;
    }
    let triple = match std::env::consts::ARCH {
        "x86_64" => "x86_64-unknown-linux-gnu",
        "aarch64" => "aarch64-unknown-linux-gnu",
        other => {
            eprintln!("skipping: no known Tier-1 triple for Linux/{other}");
            return;
        }
    };
    let rt_lib = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(triple)
        .join("debug/libpycc_rt.a");
    if !rt_lib.exists() {
        eprintln!("skipping: {triple} pycc_rt build not available in this environment");
        return;
    }

    let dir = std::env::temp_dir().join(format!("pycc_e2e_owntriple_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "hello_owntriple.py", "print(42)\n");
    let out = dir.join("hello_owntriple");

    let status = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap(), "--target", triple])
        .status()
        .unwrap();
    assert!(status.success());

    let output = Command::new(&out).output().unwrap();
    assert_eq!(output.stdout, b"42\n");
}

#[test]
fn an_unknown_target_triple_is_a_clean_build_error() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_badtriple_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "hello_badtriple.py", "print(42)\n");
    let out = dir.join("hello_badtriple");

    let output = Command::new(pycc_bin())
        .args([
            "build",
            src.to_str().unwrap(),
            "-o",
            out.to_str().unwrap(),
            "--target",
            "not-a-real-target-triple",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn targeting_a_valid_triple_with_no_local_pycc_rt_build_is_a_clean_error() {
    // riscv64-unknown-linux-gnu is a real LLVM target (codegen succeeds)
    // but isn't part of this project's Tier-1 or Tier-2 matrix (see
    // ARCHITECTURE.md), so no CI job anywhere builds pycc_rt for it -- the
    // clean, actionable error from find_pycc_rt_lib_dir, not a raw linker
    // failure about a missing -lpycc_rt. Deliberately NOT one of the two
    // Linux Tier-1 triples (x86_64/aarch64-unknown-linux-gnu): D-031's own
    // CI step builds pycc_rt for whichever of those matches the runner's
    // own arch, so using either here would flip this test's assumption
    // false on that one runner (this exact regression was caught in PR
    // review: this test failed on ubuntu-24.04-arm once D-031 gave that
    // runner its own aarch64-unknown-linux-gnu build).
    let dir = std::env::temp_dir().join(format!("pycc_e2e_no_rt_build_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "hello_no_rt.py", "print(42)\n");
    let out = dir.join("hello_no_rt");

    let output = Command::new(pycc_bin())
        .args([
            "build",
            src.to_str().unwrap(),
            "-o",
            out.to_str().unwrap(),
            "--target",
            "riscv64-unknown-linux-gnu",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("no pycc_rt build found"));
}

#[test]
fn a_bad_output_path_is_a_link_error_exit_code_1() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_badout_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "hello_badout.py", "print(42)\n");
    let bad_out = dir.join("does_not_exist_dir").join("hello");

    let status = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", bad_out.to_str().unwrap()])
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(1));
}
