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
fn build_and_run_a_function_reading_a_module_level_global_it_does_not_assign() {
    // `x = 5` ; `def f() -> int:\n    return x` ; `print(f())` -- through
    // the real `check`/`build`/codegen pipeline (not hand-crafted MIR):
    // proves `pycc_types` (D-055), `pycc_mir` (local-shadowing
    // classification), and `pycc_codegen` (module globals as real LLVM
    // globals, reachable from any function) all agree end to end that a
    // function may read a module-level global it does not itself assign.
    let dir = std::env::temp_dir().join(format!("pycc_e2e_module_global_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "reads_global.py",
        "x = 5\n\ndef f() -> int:\n    return x\n\nprint(f())\n",
    );
    let out = dir.join("reads_global");

    let status = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(status.success());

    let output = Command::new(&out).output().unwrap();
    assert_eq!(output.stdout, b"5\n");
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
fn a_type_error_is_a_compile_error_exit_code_1() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_typeerr_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "typeerr.py", "x = undefined\n");
    let out = dir.join("typeerr");

    let output = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("T0021"));
}

#[test]
fn an_unannotated_public_function_is_a_compile_error_exit_code_1() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_t0001_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "t0001.py", "def add(a, b):\n    return a + b\n");
    let out = dir.join("t0001");

    let output = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("T0001"));
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
fn calling_an_undefined_function_is_a_compile_error() {
    // Caught by pycc_types (T0021) since Task 9 added real function-call
    // signature checking; previously this only failed later, at codegen.
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
fn check_subcommand_reports_no_issues_on_valid_code() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_check_ok_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "ok.py",
        "def main() -> None:\n    print(42)\n\n1 < 2\nmain()\n",
    );

    let output = Command::new(pycc_bin())
        .args(["check", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"");
}

#[test]
fn check_subcommand_infers_a_private_helper_signature() {
    let dir = std::env::temp_dir().join(format!(
        "pycc_e2e_check_private_inference_{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "private.py",
        "def _identity(value):\n    return value\n\n_identity(1)\n",
    );

    let output = Command::new(pycc_bin())
        .args(["check", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"");
}

#[test]
fn check_subcommand_rejects_conflicting_private_helper_constraints() {
    let dir = std::env::temp_dir().join(format!(
        "pycc_e2e_check_private_conflict_{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "private_conflict.py",
        "def _callee(target):\n    return target\n\ndef _caller(source):\n    return _callee(source)\n\n_callee(1)\n_caller(\"wrong\")\n",
    );

    let output = Command::new(pycc_bin())
        .args(["check", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stdout).contains("T0021"));
}

#[test]
fn check_subcommand_propagates_an_annotated_binary_result() {
    let dir = std::env::temp_dir().join(format!(
        "pycc_e2e_check_private_binop_{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "private_binop.py",
        "def _inc_left(value) -> int:\n    return value + 1\n\ndef _inc_right(value) -> int:\n    return 1 + value\n",
    );

    let output = Command::new(pycc_bin())
        .args(["check", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"");
}

#[test]
fn check_subcommand_rejects_true_division_with_an_int_result_annotation() {
    let dir =
        std::env::temp_dir().join(format!("pycc_e2e_check_private_div_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "private_div.py",
        "def _ratio(value) -> int:\n    return value / 2\n\n_ratio(4)\n",
    );

    let output = Command::new(pycc_bin())
        .args(["check", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stdout).contains("T0021"));
}

#[test]
fn check_subcommand_rejects_known_string_operands_for_an_int_result() {
    let dir = std::env::temp_dir().join(format!(
        "pycc_e2e_check_private_string_binop_{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();

    for (name, source) in [
        (
            "string_left.py",
            "def _bad(value) -> int:\n    return \"wrong\" + value\n",
        ),
        (
            "string_right.py",
            "def _bad(value) -> int:\n    return value + \"wrong\"\n",
        ),
    ] {
        let src = write_fixture(&dir, name, source);
        let output = Command::new(pycc_bin())
            .args(["check", src.to_str().unwrap()])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{name}");
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("T0021"),
            "{name}"
        );
    }
}

#[test]
fn check_subcommand_reports_t0001_on_an_unannotated_public_function() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_check_t0001_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "bad.py", "def add(a, b):\n    return a + b\n");

    let output = Command::new(pycc_bin())
        .args(["check", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stdout).contains("T0001"));
}

#[test]
fn check_subcommand_supports_json_error_format() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_check_json_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "bad.py", "def add(a, b):\n    return a + b\n");

    let output = Command::new(pycc_bin())
        .args(["check", src.to_str().unwrap(), "--error-format", "json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(parsed["code"], "T0001");
}

#[test]
fn check_subcommand_rejects_an_unknown_error_format() {
    let output = Command::new(pycc_bin())
        .args(["check", "unused.py", "--error-format", "xml"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid value 'xml'"));
    assert!(stderr.contains("[possible values: human, json]"));
}

#[test]
fn direct_type_api_rejects_a_for_target_representation_change() {
    let function = pycc_hir::HirItem::Function {
        name: "loop_over".to_string(),
        params: vec![("value".to_string(), pycc_hir::Ty::Str)],
        return_ty: pycc_hir::Ty::None,
        body: vec![pycc_hir::HirStmt::ForRange {
            var: "value".to_string(),
            start: pycc_hir::HirExpr::IntLiteral(0),
            stop: pycc_hir::HirExpr::IntLiteral(3),
            step: pycc_hir::HirExpr::IntLiteral(1),
            body: vec![],
        }],
    };
    assert_eq!(
        pycc_types::check_function(&function).unwrap_err().code,
        "T0023"
    );
}

#[test]
fn module_type_api_rejects_a_for_target_representation_change() {
    let hir = pycc_hir::HirModule {
        items: vec![pycc_hir::HirItem::Function {
            name: "loop_over".to_string(),
            params: vec![("value".to_string(), pycc_hir::Ty::Str)],
            return_ty: pycc_hir::Ty::None,
            body: vec![pycc_hir::HirStmt::ForRange {
                var: "value".to_string(),
                start: pycc_hir::HirExpr::IntLiteral(0),
                stop: pycc_hir::HirExpr::IntLiteral(3),
                step: pycc_hir::HirExpr::IntLiteral(1),
                body: vec![],
            }],
        }],
    };
    assert_eq!(pycc_types::check(&hir).unwrap_err().code, "T0023");
}

#[test]
fn direct_type_api_rejects_an_unconstrained_private_parameter() {
    let hir = pycc_hir::HirModule {
        items: vec![pycc_hir::HirItem::Function {
            name: "_unused".to_string(),
            params: vec![("value".to_string(), pycc_hir::Ty::Infer)],
            return_ty: pycc_hir::Ty::None,
            body: vec![],
        }],
    };
    assert_eq!(
        pycc_types::check_and_resolve(&hir).unwrap_err().code,
        "T0021"
    );
}

#[test]
fn direct_type_api_rejects_an_unconstrained_private_return() {
    let hir = pycc_hir::HirModule {
        items: vec![pycc_hir::HirItem::Function {
            name: "_unknown".to_string(),
            params: vec![],
            return_ty: pycc_hir::Ty::Infer,
            body: vec![pycc_hir::HirStmt::Return(Some(pycc_hir::HirExpr::Name(
                "missing".to_string(),
            )))],
        }],
    };
    let err = pycc_types::check_and_resolve(&hir).unwrap_err();
    assert_eq!(err.code, "T0021");
    assert!(err.message.contains("return type"));
}

#[test]
fn direct_type_api_propagates_an_annotated_binary_result() {
    let hir = pycc_hir::HirModule {
        items: vec![pycc_hir::HirItem::Function {
            name: "_inc".to_string(),
            params: vec![("value".to_string(), pycc_hir::Ty::Infer)],
            return_ty: pycc_hir::Ty::Int,
            body: vec![pycc_hir::HirStmt::Return(Some(pycc_hir::HirExpr::BinOp {
                op: pycc_hir::BinOpKind::Add,
                left: Box::new(pycc_hir::HirExpr::Name("value".to_string())),
                right: Box::new(pycc_hir::HirExpr::IntLiteral(1)),
            }))],
        }],
    };
    assert!(pycc_types::check_and_resolve(&hir).is_ok());
}

#[test]
fn direct_type_api_rejects_incompatible_resolved_binary_operands() {
    let hir = pycc_hir::HirModule {
        items: vec![
            pycc_hir::HirItem::Function {
                name: "_bad_add".to_string(),
                params: vec![("value".to_string(), pycc_hir::Ty::Infer)],
                return_ty: pycc_hir::Ty::Infer,
                body: vec![pycc_hir::HirStmt::Return(Some(pycc_hir::HirExpr::BinOp {
                    op: pycc_hir::BinOpKind::Add,
                    left: Box::new(pycc_hir::HirExpr::Name("value".to_string())),
                    right: Box::new(pycc_hir::HirExpr::StringLiteral("wrong".to_string())),
                }))],
            },
            pycc_hir::HirItem::TopLevelStmt(pycc_hir::HirStmt::ExprStmt(pycc_hir::HirExpr::Call {
                callee: "_bad_add".to_string(),
                args: vec![pycc_hir::HirExpr::IntLiteral(1)],
            })),
        ],
    };
    assert_eq!(
        pycc_types::check_and_resolve(&hir).unwrap_err().code,
        "T0021"
    );
}

#[test]
fn check_subcommand_reports_a_clean_error_on_a_missing_file() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_check_missing_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let missing_path = dir.join("does_not_exist.py");

    let output = Command::new(pycc_bin())
        .args(["check", missing_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("could not read"));
}

#[test]
fn check_subcommand_reports_a_syntax_error() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_check_syntax_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "bad.py", "def main(:\n");

    let output = Command::new(pycc_bin())
        .args(["check", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stdout).contains("L0001"));
}

#[test]
fn check_subcommand_reports_a_type_error() {
    // Distinct from the T0001/T0002 cases above -- those are raised during
    // pycc_hir::lower_checked itself. `x = undefined` parses and lowers
    // cleanly; the undefined-name error only surfaces from
    // pycc_types::check's own inference pass, exercising try_check's third
    // (and otherwise untested) diagnostic-producing stage.
    let dir = std::env::temp_dir().join(format!("pycc_e2e_check_typeerr_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "bad.py", "x = undefined\n");

    let output = Command::new(pycc_bin())
        .args(["check", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stdout).contains("T0021"));
}

#[test]
fn a_top_level_return_is_a_clean_error_not_a_panic() {
    // Regression test (self-review finding, pre-merge): a bare `return` at
    // module scope used to panic pycc_types (exit code 101, raw backtrace)
    // instead of producing a T0024 diagnostic through the documented exit-1
    // contract. `ruff_python_parser` parses this fine -- CPython itself only
    // rejects it in a later compile pass, not the grammar -- so this is
    // reachable from ordinary (if unusual) CLI input.
    let dir =
        std::env::temp_dir().join(format!("pycc_e2e_top_level_return_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "bad.py", "return\n");

    let output = Command::new(pycc_bin())
        .args(["check", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stdout).contains("T0024"));
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
