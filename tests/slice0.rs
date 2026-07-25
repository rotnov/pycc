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

fn rendered_diagnostic_path(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "/")
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
        .args([
            "build",
            missing_path.to_str().unwrap(),
            "-o",
            out.to_str().unwrap(),
        ])
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
    let src = write_fixture(
        &dir,
        "hello.py",
        "def main() -> None:\n    print(42)\n\nmain()\n",
    );
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
    let src = write_fixture(
        &dir,
        "hello_uncalled.py",
        "def main() -> None:\n    print(42)\n",
    );
    let out = dir.join("hello_uncalled");

    let status = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(status.success());

    let output = Command::new(&out).output().unwrap();
    assert_eq!(
        output.stdout, b"",
        "must match CPython's actual output for this source"
    );
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

    let output = Command::new(pycc_bin())
        .args(["run", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"42\n");
}

#[test]
fn run_subcommand_propagates_a_build_failure() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_run_fail_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "bad_run.py", "def main(:\n");

    let output = Command::new(pycc_bin())
        .args(["run", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("L0001"));
}

#[test]
fn version_flag_prints_something() {
    let output = Command::new(pycc_bin())
        .args(["version", "--verbose"])
        .output()
        .unwrap();
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
fn build_rejects_an_undefined_function_during_frontend_checking() {
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
    assert!(
        description.contains("x86_64"),
        "expected an x86_64 binary, got: {description}"
    );

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
        .args([
            "build",
            src.to_str().unwrap(),
            "-o",
            out.to_str().unwrap(),
            "--target",
            triple,
        ])
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
fn check_rejects_an_undefined_function_before_codegen() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_check_undefined_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "undefined.py", "does_not_exist()\n");

    let output = Command::new(pycc_bin())
        .arg("check")
        .arg(&src)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("does_not_exist"));
}

#[test]
fn check_rejects_a_call_before_its_definition_to_match_python_module_order() {
    let dir = std::env::temp_dir().join(format!(
        "pycc_e2e_check_definition_order_{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "definition_order.py",
        "helper()\n\ndef helper() -> None:\n    print(1)\n",
    );

    let output = Command::new(pycc_bin())
        .arg("check")
        .arg(&src)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(1));
    assert!(stderr.contains("error[T0004]: call to undefined function `helper`"));
    assert!(stderr.contains("definition_order.py:1:1"));
}

#[test]
fn check_rejects_function_redefinitions_before_codegen() {
    let dir = std::env::temp_dir().join(format!(
        "pycc_e2e_check_redefinition_{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "redefinition.py",
        "\
def helper() -> None:
    print(1)

helper()

def helper() -> None:
    print(2)

helper()
",
    );

    let output = Command::new(pycc_bin())
        .arg("check")
        .arg(&src)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(1));
    assert!(stderr.contains("error[C0001]: redefining function `helper`"));
    assert!(stderr.contains("redefinition.py:6:5"));
}

#[test]
fn check_rejects_unsupported_function_parameters_before_codegen() {
    let dir =
        std::env::temp_dir().join(format!("pycc_e2e_check_parameters_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "parameters.py",
        "def helper(value: int) -> None:\n    print(1)\n\nhelper()\n",
    );

    let output = Command::new(pycc_bin())
        .arg("check")
        .arg(&src)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(1));
    assert!(stderr.contains("error[C0001]"));
    assert!(stderr.contains("zero-argument functions"));
}

#[test]
fn check_classifies_an_unsupported_builtin_as_a_capability_error() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_check_builtin_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "builtin.py", "input()\n");

    let output = Command::new(pycc_bin())
        .arg("check")
        .arg(&src)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(1));
    assert!(stderr.contains("error[C0001]"));
    assert!(stderr.contains("built-in `input`"));
}

#[test]
fn check_classifies_a_builtin_exception_as_a_capability_error() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_check_exception_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "exception.py", "ValueError()\n");

    let output = Command::new(pycc_bin())
        .arg("check")
        .arg(&src)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(1));
    assert!(stderr.contains("error[C0001]"));
    assert!(stderr.contains("built-in `ValueError`"));
}

#[test]
fn check_resolves_print_to_an_earlier_module_function() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_check_print_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "shadowed_print.py",
        "\
def print() -> None:
    helper()

def helper() -> None:
    print()

helper()
",
    );

    let output = Command::new(pycc_bin())
        .arg("check")
        .arg(&src)
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn run_uses_builtin_print_before_a_later_module_binding() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_late_print_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "late_print.py",
        "\
def first() -> None:
    print(1)

first()

def print() -> None:
    print()
",
    );

    let output = Command::new(pycc_bin())
        .arg("run")
        .arg(&src)
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(output.stdout, b"1\n");
    assert!(output.stderr.is_empty());
}

#[test]
fn check_normalizes_the_displayed_diagnostic_path() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_check_path_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    write_fixture(&dir, "bad.py", "x = 1\n");

    let output = Command::new(pycc_bin())
        .current_dir(&dir)
        .args(["check", "./bad.py"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(1));
    assert!(stderr.contains(" --> bad.py:1:1"));
    assert!(!stderr.contains(" --> ./bad.py"));
}

#[cfg(unix)]
#[test]
fn check_preserves_a_literal_backslash_in_a_unix_diagnostic_path() {
    let dir = std::env::temp_dir().join(format!(
        "pycc_e2e_check_backslash_path_{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    write_fixture(&dir, r"bad\name.py", "x = 1\n");

    let output = Command::new(pycc_bin())
        .current_dir(&dir)
        .args(["check", r"bad\name.py"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(1));
    assert!(stderr.contains(r" --> bad\name.py:1:1"));
    assert!(!stderr.contains(" --> bad/name.py"));
}

#[cfg(unix)]
#[test]
fn check_escapes_terminal_controls_in_diagnostic_paths() {
    let dir = std::env::temp_dir().join(format!(
        "pycc_e2e_check_control_path_{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "bad\n\u{1b}\u{202e}.py", "x = 1\n");

    let output = Command::new(pycc_bin())
        .arg("check")
        .arg(&src)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr.contains("bad\\n\\u{1b}\\u{202e}.py:1:1"),
        "stderr: {stderr}"
    );
    assert!(!stderr.contains('\u{1b}'));
    assert!(!stderr.contains('\u{202e}'));
    assert!(!stderr.contains(&format!(" --> {}", src.display())));
}

#[test]
fn a_bad_output_path_is_a_link_error_exit_code_1() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_badout_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "hello_badout.py", "print(42)\n");
    let bad_out = dir.join("does_not_exist_dir").join("hello");

    let status = Command::new(pycc_bin())
        .args([
            "build",
            src.to_str().unwrap(),
            "-o",
            bad_out.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(1));
}

#[test]
fn a_bad_temporary_directory_is_a_codegen_error_exit_code_1() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_badtmp_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "hello_badtmp.py", "print(42)\n");
    let out = dir.join("hello");
    let missing_tmp = dir.join("does_not_exist");

    let output = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .env("TMPDIR", &missing_tmp)
        .env("TMP", &missing_tmp)
        .env("TEMP", &missing_tmp)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("codegen failed"));
}

#[test]
fn check_accepts_every_staged_file_in_one_invocation() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_check_many_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let first = write_fixture(&dir, "first.py", "print(1)\n");
    let second = write_fixture(&dir, "second.py", "def helper() -> None:\n    print(2)\n");

    let output = Command::new(pycc_bin())
        .arg("check")
        .arg(&first)
        .arg(&second)
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn check_accepts_a_pep_263_latin_1_source_file() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_check_latin1_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("latin1.py");
    std::fs::write(
        &src,
        b"# -*- coding: latin-1 -*- # Andr\xe9\n# caf\xe9\nprint(42)\n",
    )
    .unwrap();

    let output = Command::new(pycc_bin())
        .arg("check")
        .arg(&src)
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
}

#[test]
fn check_accepts_python_normalized_encoding_separators() {
    let dir = std::env::temp_dir().join(format!(
        "pycc_e2e_check_encoding_name_{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let utf8 = dir.join("utf8.py");
    std::fs::write(&utf8, b"# coding: utf--8\nprint(1)\n").unwrap();
    let latin1 = dir.join("latin1.py");
    std::fs::write(&latin1, b"# coding: latin__1\n# caf\xe9\nprint(2)\n").unwrap();
    let dotted_ascii = dir.join("dotted_ascii.py");
    std::fs::write(&dotted_ascii, b"# coding: us.ascii\nprint(3)\n").unwrap();
    let dotted_latin1 = dir.join("dotted_latin1.py");
    std::fs::write(
        &dotted_latin1,
        b"# coding: iso.8859.1\n# caf\xe9\nprint(4)\n",
    )
    .unwrap();

    let output = Command::new(pycc_bin())
        .arg("check")
        .arg(&utf8)
        .arg(&latin1)
        .arg(&dotted_ascii)
        .arg(&dotted_latin1)
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn check_rejects_collapsed_utf8_separators_when_a_bom_is_present() {
    let dir = std::env::temp_dir().join(format!(
        "pycc_e2e_check_bom_encoding_name_{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("bom_conflict.py");
    std::fs::write(&src, b"\xef\xbb\xbf# coding: utf--8\nprint(1)\n").unwrap();

    let output = Command::new(pycc_bin())
        .arg("check")
        .arg(&src)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(2));
    assert!(stderr.contains("UTF-8 BOM conflicts with the declared source encoding"));
}

#[test]
fn check_normalizes_python_universal_newlines_before_rendering_diagnostics() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_check_newlines_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    for (name, source) in [
        ("cr.py", b"print(1)\rx = 1\r".as_slice()),
        ("crlf.py", b"print(1)\r\nx = 1\r\n".as_slice()),
    ] {
        let src = dir.join(name);
        std::fs::write(&src, source).unwrap();
        let output = Command::new(pycc_bin())
            .arg("check")
            .arg(&src)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert_eq!(output.status.code(), Some(1));
        assert!(stderr.contains(&format!("{}:2:1", rendered_diagnostic_path(&src))));
        assert!(stderr.contains("2 | x = 1\n"));
        assert!(!stderr.contains("x = 1\r"));
    }
}

#[test]
fn check_reports_a_malformed_encoded_source_as_an_input_error() {
    let dir = std::env::temp_dir().join(format!(
        "pycc_e2e_check_bad_encoding_{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("invalid_utf8.py");
    std::fs::write(&src, b"print(42)\n# \xff\n").unwrap();

    let output = Command::new(pycc_bin())
        .arg("check")
        .arg(&src)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(2));
    assert!(stderr.contains("could not read"));
    assert!(stderr.contains("source is not valid utf-8"));
}

#[test]
fn check_rejects_a_codec_without_python_compatible_mappings() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_check_gbk_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("invalid_gbk.py");
    std::fs::write(&src, b"# coding: gbk\n# \x80\nprint(42)\n").unwrap();

    let output = Command::new(pycc_bin())
        .arg("check")
        .arg(&src)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(2));
    assert!(stderr.contains("unknown source encoding `gbk`"));
}

#[test]
fn check_accepts_a_staged_filename_that_starts_with_a_hyphen() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_check_hyphen_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    write_fixture(&dir, "--staged.py", "print(1)\n");

    let output = Command::new(pycc_bin())
        .args(["check", "--", "--staged.py"])
        .current_dir(&dir)
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
}

#[test]
fn check_help_flags_show_help_instead_of_becoming_filenames() {
    for flag in ["--help", "-h"] {
        let output = Command::new(pycc_bin())
            .args(["check", flag])
            .output()
            .unwrap();

        assert!(output.status.success(), "flag: {flag}");
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("Usage:"),
            "flag: {flag}"
        );
        assert!(output.stderr.is_empty(), "flag: {flag}");
    }
}

#[cfg(target_os = "linux")]
#[test]
fn check_accepts_a_non_utf8_staged_path_losslessly() {
    use std::os::unix::ffi::OsStringExt;

    let dir = std::env::temp_dir().join(format!("pycc_e2e_check_non_utf8_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let filename = std::ffi::OsString::from_vec(b"invalid_\xff.py".to_vec());
    let src = dir.join(filename);
    std::fs::write(&src, b"x = 1\n").unwrap();

    let output = Command::new(pycc_bin())
        .arg("check")
        .arg(&src)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(1));
    assert!(stderr.contains("error[C0001]"));
    assert!(stderr.contains("invalid_\u{fffd}.py"));
}

#[test]
fn check_reports_every_failure_and_io_errors_take_exit_code_precedence() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_check_errors_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let invalid = write_fixture(&dir, "invalid.py", "print(1)\n$\n");
    let missing = dir.join("missing.py");

    let output = Command::new(pycc_bin())
        .arg("check")
        .arg(&invalid)
        .arg(&missing)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(2));
    assert!(stderr.contains(&rendered_diagnostic_path(&invalid)));
    assert!(stderr.contains("L0001"));
    assert!(stderr.contains(&format!("{}:2:1", rendered_diagnostic_path(&invalid))));
    assert!(stderr.contains("2 | $"));
    assert!(stderr.contains("  | ^"));
    assert!(stderr.contains(&rendered_diagnostic_path(&missing)));
    assert!(stderr.contains("could not read"));
}

#[test]
fn check_rejects_a_currently_unsupported_construct_without_panicking() {
    let dir =
        std::env::temp_dir().join(format!("pycc_e2e_check_unsupported_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "assignment.py", "def main() -> None:\n    x = 1\n");

    let output = Command::new(pycc_bin())
        .arg("check")
        .arg(&src)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(1));
    assert!(stderr.contains("C0001"));
    assert!(stderr.contains(&format!("{}:2:5", rendered_diagnostic_path(&src))));
    assert!(stderr.contains("2 |     x = 1"));
    assert!(stderr.contains("  |     ^^^^^ unsupported by this pycc version"));
    assert!(!stderr.contains("panicked"));
}

#[test]
fn check_aligns_a_diagnostic_caret_after_tab_indentation() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_check_tab_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "tab.py", "def main() -> None:\n\tx = 1\n");

    let output = Command::new(pycc_bin())
        .arg("check")
        .arg(&src)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(1));
    assert!(stderr.contains(&format!("{}:2:2", rendered_diagnostic_path(&src))));
    assert!(stderr.contains("2 | \tx = 1"));
    assert!(stderr.contains("  | \t^^^^^"));
}

#[test]
fn check_uses_unicode_display_width_for_diagnostic_carets() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_check_unicode_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    for (filename, source, expected) in [
        (
            "wide.py",
            "\u{53d8}\u{91cf}$\n",
            "1 | \u{53d8}\u{91cf}$\n  |     ^ invalid syntax",
        ),
        (
            "combining.py",
            "e\u{301}$\n",
            "1 | e\u{301}$\n  |  ^ invalid syntax",
        ),
    ] {
        let src = write_fixture(&dir, filename, source);
        let output = Command::new(pycc_bin())
            .arg("check")
            .arg(&src)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert_eq!(output.status.code(), Some(1));
        assert!(stderr.contains(expected), "stderr: {stderr}");
    }
}

#[test]
fn check_sizes_the_diagnostic_gutter_for_three_digit_line_numbers() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_check_gutter_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut source = "print(1)\n".repeat(99);
    source.push_str("x = 1\n");
    let src = write_fixture(&dir, "line_100.py", &source);

    let output = Command::new(pycc_bin())
        .arg("check")
        .arg(&src)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(1));
    assert!(stderr.contains(&format!("{}:100:1", rendered_diagnostic_path(&src))));
    assert!(stderr.contains("100 | x = 1"));
    assert!(stderr.contains("    | ^^^^^"));
}

#[test]
fn check_without_files_is_a_clean_invocation_error() {
    let output = Command::new(pycc_bin()).arg("check").output().unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("at least one Python file"));
}

#[test]
fn build_rejects_a_currently_unsupported_construct_without_panicking() {
    let dir =
        std::env::temp_dir().join(format!("pycc_e2e_build_unsupported_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "assignment.py", "x = 1");
    let out = dir.join("assignment");

    let output = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(1));
    assert!(stderr.contains("C0001"));
    assert!(!stderr.contains("panicked"));
}

#[test]
fn repository_publishes_the_pycc_check_pre_commit_hook() {
    let manifest_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".pre-commit-hooks.yaml");
    let manifest = std::fs::read_to_string(&manifest_path).unwrap();

    assert_eq!(
        manifest,
        "\
- id: pycc-check
  name: pycc check
  description: Check typed Python files with the pycc frontend
  entry: pycc check --
  language: rust
  types: [python]
  require_serial: true
"
    );

    let fixture = manifest_path
        .parent()
        .unwrap()
        .join("tests/fixtures/pre_commit_valid.py");
    let output = Command::new(pycc_bin())
        .arg("check")
        .arg(fixture)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}
