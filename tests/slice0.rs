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
fn run_subcommand_normalizes_a_generated_runtime_failure_to_101() {
    let dir =
        std::env::temp_dir().join(format!("pycc_e2e_run_runtime_fail_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "runtime_fail.py", "print(1.0 / 0.0)\n");

    let output = Command::new(pycc_bin())
        .args(["run", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(101));
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

#[cfg(unix)]
#[test]
fn run_subcommand_maps_a_signal_terminated_compiled_program_to_exit_code_101() {
    // A `pycc_rt` panic crosses a plain (non-unwinding) `extern "C"`
    // boundary and aborts the child process via `SIGABRT` (see pycc_rt's
    // own panic-across-FFI convention), so on Unix `Command::status()`'s
    // `.code()` is `None` (the child was killed by a signal, not a normal
    // exit) -- `run`'s previous `.unwrap_or(1)` silently mapped that to
    // exit code 1 instead of the `101` CLI_SPEC.md promises for "compiled
    // program panicked/uncaught exception".
    let dir = std::env::temp_dir().join(format!("pycc_e2e_run_signal_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "div_zero_run.py", "print(1.0 / 0.0)\n");

    let output = Command::new(pycc_bin())
        .args(["run", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(101));
}

/// #133's backend regression: before the frontend gained the
/// module-binding-shadows-call rule, this exact source passed `pycc check`
/// AND compiled, and the produced binary silently called the shadowed
/// function (printing "function") where CPython 3.14 raises
/// `TypeError: 'int' object is not callable`. `pycc build` runs
/// `pycc_types::check_and_resolve` before any codegen, so the frontend
/// rejection must now stop the whole pipeline -- this pins that the
/// observable miscompile (issue #133's first follow-up comment) can never
/// come back through the public build path.
#[test]
fn build_rejects_a_module_value_binding_that_shadows_a_function_call() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_shadowed_call_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "shadowed_call.py",
        "def helper() -> None:\n    print(\"function\")\n\nhelper = 1\nhelper()\n",
    );
    let out = dir.join("shadowed_call");

    let output = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    // `report_build_failure` renders compile diagnostics to stderr
    // (`eprint!`), unlike `pycc check`'s stdout rendering.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("error[T0021]: name `helper` is bound to a non-callable value"),
        "build must fail with the shadowing diagnostic, got: {stderr}"
    );
    assert!(
        !out.exists(),
        "no binary may be produced for a rejected shadowed call"
    );
}

/// The summary line both `pycc version` forms print.
///
/// `pycc {version}` is built from the same manifest macro as the
/// implementation (`CARGO_PKG_VERSION`, identical in both binaries since this
/// integration test compiles inside the `pycc` package), so the snapshot
/// tracks a version bump instead of rotting.
///
/// `rustc {version}` is deliberately **not** derived from
/// `env!("PYCC_BUILD_RUSTC_VERSION")`, the same source `src/main.rs` uses:
/// reusing it would only prove the binary's output matches its own build
/// script, not that the build script captured the real compiler (#247 -- the
/// original bug reused `CARGO_PKG_RUST_VERSION` on both sides and stayed
/// green while reporting the wrong field). Instead this shells out to
/// `rustc --version` itself, at test run time, and parses it the same way
/// `build.rs` does, giving an independent live check against the actual
/// installed compiler.
///
/// "LLVM 22.1.1" is D-015's pinned contract literal on both sides -- see the
/// version arm's own comment in `src/main.rs` for why the *installed* LLVM's
/// truthfulness is #75's scope, not this test's.
fn expected_version_summary_line() -> String {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let output = Command::new(&rustc)
        .arg("--version")
        .output()
        .unwrap_or_else(|err| panic!("failed to run `{rustc} --version`: {err}"));
    assert!(output.status.success(), "`{rustc} --version` failed");
    let stdout = String::from_utf8(output.stdout)
        .unwrap_or_else(|err| panic!("`{rustc} --version` printed non-UTF-8 output: {err}"));
    let installed_rustc_version = stdout
        .split_whitespace()
        .nth(1)
        .unwrap_or_else(|| panic!("unexpected `{rustc} --version` output: {stdout:?}"));
    format!(
        "pycc {} (rustc {}, LLVM 22.1.1)\n",
        env!("CARGO_PKG_VERSION"),
        installed_rustc_version,
    )
}

#[test]
fn version_verbose_reports_compiler_llvm_and_tier1_targets() {
    // #38: the predecessor test accepted any non-empty stdout, so
    // `println!("garbage")` passed it while CLI_SPEC.md's command table
    // promised "compiler, LLVM, target list". An exact full-stdout snapshot
    // is the strongest mutation-resistant form: a dropped field, a missing
    // or reordered target line, or arbitrary output all fail. The five
    // triples and their order mirror ARCHITECTURE.md's Tier-1 table via
    // `src/main.rs::TIER1_TARGETS`.
    let output = Command::new(pycc_bin())
        .args(["version", "--verbose"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let expected = format!(
        "{}tier-1 targets:\n  x86_64-unknown-linux-gnu\n  aarch64-unknown-linux-gnu\n  \
         x86_64-apple-darwin\n  aarch64-apple-darwin\n  x86_64-pc-windows-msvc\n",
        expected_version_summary_line()
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), expected);
}

#[test]
fn version_without_verbose_prints_only_the_summary_line() {
    // The bare form is undocumented surface whose behavior predates #38;
    // this pins that the target block is `--verbose`-gated and the bare
    // command's output stays exactly the single summary line it always
    // printed.
    let output = Command::new(pycc_bin()).args(["version"]).output().unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        expected_version_summary_line()
    );
}

#[test]
fn unimplemented_subcommands_exit_with_code_2() {
    let output = Command::new(pycc_bin()).args(["clean"]).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn init_scaffolds_pycc_toml_and_main_py_in_the_current_directory() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_init_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let output = Command::new(pycc_bin())
        .args(["init", "e2e_init_project"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stdout).contains("Created pycc.toml and src/main.py"));

    // A substring check on the raw name (rather than an exact
    // `name = "..."` slice) avoids coupling this end-to-end test to the
    // `toml` crate's own quote-style/spacing choice for serialized output
    // -- `src/project_config.rs`'s own unit tests instead round-trip
    // through `parse`, which this integration-test binary can't reach
    // directly (no `[lib]` target to link against).
    let toml_contents = std::fs::read_to_string(dir.join("pycc.toml")).unwrap();
    assert!(toml_contents.contains("e2e_init_project"));
    assert!(
        std::fs::read_to_string(dir.join("src").join("main.py"))
            .unwrap()
            .contains("def main")
    );
}

#[test]
fn init_refuses_when_pycc_toml_exists_as_a_directory() {
    // Pre-#237 this same setup forced `scaffold`'s *write* to fail; the
    // pre-check phase now refuses it up front (any entry type counts as
    // an existing `pycc.toml`). Same exit code and stderr prefix, honest
    // new name for the new mechanism.
    let dir = std::env::temp_dir().join(format!("pycc_e2e_init_conflict_{}", std::process::id()));
    std::fs::create_dir_all(dir.join("pycc.toml")).unwrap();

    let output = Command::new(pycc_bin())
        .args(["init"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error: pycc init failed"));
    assert!(stderr.contains("`pycc.toml` already exists"));
}

/// #237 regression 1: both scaffold files already exist -- refuse with
/// exit 2 and leave both byte-for-byte unchanged.
#[test]
fn init_refuses_to_overwrite_an_existing_project() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_init_existing_{}", std::process::id()));
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("pycc.toml"), "user toml").unwrap();
    std::fs::write(dir.join("src").join("main.py"), "user code").unwrap();

    let output = Command::new(pycc_bin())
        .args(["init", "clobber"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error: pycc init failed"));
    assert!(stderr.contains("`pycc.toml` already exists"));
    assert_eq!(
        std::fs::read_to_string(dir.join("pycc.toml")).unwrap(),
        "user toml"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("src").join("main.py")).unwrap(),
        "user code"
    );
}

/// #237 regression 2 (variant A): only `pycc.toml` exists.
#[test]
fn init_refuses_when_only_pycc_toml_exists() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_init_toml_only_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("pycc.toml"), "user toml").unwrap();

    let output = Command::new(pycc_bin())
        .args(["init"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("`pycc.toml` already exists"));
    assert_eq!(
        std::fs::read_to_string(dir.join("pycc.toml")).unwrap(),
        "user toml"
    );
    assert!(!dir.join("src").exists());
}

/// #237 regression 2 (variant B): only `src/main.py` exists -- the refusal
/// must also leave `pycc.toml` uncreated.
#[test]
fn init_refuses_when_only_main_py_exists() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_init_py_only_{}", std::process::id()));
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src").join("main.py"), "user code").unwrap();

    let output = Command::new(pycc_bin())
        .args(["init"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("`src/main.py` already exists"));
    assert_eq!(
        std::fs::read_to_string(dir.join("src").join("main.py")).unwrap(),
        "user code"
    );
    assert!(!dir.join("pycc.toml").exists());
}

/// #237 regression 3: `src` exists as a plain file.
#[test]
fn init_refuses_when_src_is_a_plain_file() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_init_src_file_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("src"), "not a directory").unwrap();

    let output = Command::new(pycc_bin())
        .args(["init"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("`src` exists and is not a directory")
    );
    assert!(!dir.join("pycc.toml").exists());
}

/// #237 regression 4, extended by #256: a late write failure leaves
/// `pycc.toml` uncreated, AND rolls back whatever this same invocation
/// already created (`main.py`, then the `src/` it made -- now empty).
/// Unix-only injection (a dangling `pycc.toml` symlink passes the
/// follow-symlink pre-check, both `src` steps succeed, and the final
/// `create_new` write fails with `EEXIST` on the symlink entry itself,
/// never following it) -- the ordering guarantee is additionally pinned by
/// `src/project_config.rs`'s unit-level injection trio, and the coverage
/// leg runs on macOS, so this counts toward the 100% gate.
#[cfg(unix)]
#[test]
fn init_rolls_back_main_py_when_a_late_write_fails() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_init_late_fail_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::os::unix::fs::symlink(
        dir.join("missing_dir").join("pycc.toml"),
        dir.join("pycc.toml"),
    )
    .unwrap();

    let output = Command::new(pycc_bin())
        .args(["init"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error: pycc init failed"));
    // Positive proof main.py genuinely existed and was cleanly removed
    // (not that it never existed) -- see the unit-level twin in
    // src/project_config.rs for why an absent "could not remove" substring
    // proves this, not just non-emptiness of the error.
    assert!(!stderr.contains("could not remove"));
    assert!(!stderr.contains("remove it manually"));
    // The invocation-owned main.py and the src/ it created are rolled back...
    assert!(!dir.join("src").exists());
    // ...and the real pycc.toml path still resolves to nothing.
    assert!(!dir.join("pycc.toml").try_exists().unwrap());
}

/// #256 item 2 at the public-CLI level: after the underlying cause is
/// removed, an immediate retry succeeds -- the whole point of rolling back
/// invocation-owned residue is that a fixed retry is never blocked by it.
#[cfg(unix)]
#[test]
fn init_succeeds_on_retry_after_a_late_write_failure_is_fixed() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_init_retry_ok_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::os::unix::fs::symlink(
        dir.join("missing_dir").join("pycc.toml"),
        dir.join("pycc.toml"),
    )
    .unwrap();

    let first = Command::new(pycc_bin())
        .args(["init"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert_eq!(first.status.code(), Some(2));

    std::fs::remove_file(dir.join("pycc.toml")).unwrap();

    let second = Command::new(pycc_bin())
        .args(["init"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert_eq!(second.status.code(), Some(0));
    assert!(dir.join("pycc.toml").is_file());
    assert!(dir.join("src").join("main.py").is_file());
}

/// #251: an unavailable current directory (deleted, unmounted, or otherwise
/// inaccessible after launch) is an invocation/environment error reported
/// with the exit-2 class and a stable stderr diagnostic, not a Rust panic
/// (exit 101 with a raw backtrace, the pre-fix behavior). `#[cfg(unix)]`:
/// only Unix allows a process to `rmdir` its own cwd -- the inode stays
/// alive until the process exits, but `getcwd(2)` then fails with `ENOENT`,
/// which is exactly the `std::env::current_dir()` `Err` this test exercises.
/// The coverage gate runs on macOS (a Unix target), so this test covers the
/// new `Err` arm for D-014's 100% line/region requirement. A shell wrapper
/// is used because `Command::current_dir()` requires the directory to exist
/// at child-start time -- the child must inherit a cwd that is already
/// deleted, which only `cd && rmdir && exec` can achieve.
#[cfg(unix)]
#[test]
fn init_reports_an_unavailable_cwd_without_panicking() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_init_no_cwd_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let pycc = pycc_bin();

    // `sh -c '...' sh "$dir" "$pycc"` — positional args ($1, $2) avoid
    // shell-escaping issues with temp paths that may contain spaces.
    let output = Command::new("sh")
        .arg("-c")
        .arg("cd \"$1\" && rmdir \"$1\" && exec \"$2\" init")
        .arg("sh")
        .arg(&dir)
        .arg(&pycc)
        .output()
        .unwrap();

    assert_eq!(
        output.status.code(),
        Some(2),
        "expected exit 2, got {:?}; stderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr),
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("error: pycc init failed"),
        "expected the init-failed prefix, got: {stderr}",
    );
    assert!(
        stderr.contains("cannot read current directory"),
        "expected the cwd-resolution diagnostic, got: {stderr}",
    );
    assert!(
        !stderr.contains("panicked"),
        "the failure must not be a Rust panic: {stderr}",
    );
    assert!(
        !stderr.contains("RUST_BACKTRACE"),
        "the failure must not include a backtrace hint: {stderr}",
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("Created"),
        "no success message should appear on failure: {stdout}",
    );
    // No scaffold files created in the parent of the deleted cwd.
    assert!(!dir.join("pycc.toml").exists());
    assert!(!dir.join("src").exists());
}

/// #250: a missing host linker driver is an environment failure reported
/// with the exit-2 invocation/environment class, not a Rust panic (exit
/// 101 with a raw backtrace, the pre-fix behavior). `#[cfg(unix)]`: the
/// Unix drivers resolve `cc` via `PATH`, so an empty `PATH` deterministically
/// makes only the linker spawn fail (pycc itself is invoked by absolute
/// path and the frontend + LLVM codegen run in-process); Windows resolves
/// its bundled `clang.exe` from a compile-time absolute prefix (D-028),
/// which no environment crafting can make absent. This pair (with the
/// `run` twin below) is load-bearing for D-014: the new spawn-error
/// region executes only in these two tests, and either alone would
/// satisfy the gate.
#[cfg(unix)]
#[test]
fn build_reports_a_missing_linker_driver_without_panicking() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_no_linker_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "no_linker.py", "print(42)\n");
    let out = dir.join("no_linker");

    let output = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .env("PATH", "/nonexistent")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("error: could not run the linker driver"),
        "expected the actionable diagnostic, got: {stderr}"
    );
    assert!(
        !stderr.contains("panicked"),
        "the failure must not be a Rust panic: {stderr}"
    );
    assert!(!out.exists());
}

/// The `pycc run` twin: `run` funnels through the same `try_build`, so the
/// same environment failure gets the same diagnostic and exit class.
#[cfg(unix)]
#[test]
fn run_reports_a_missing_linker_driver_without_panicking() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_run_no_linker_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "no_linker_run.py", "print(42)\n");

    let output = Command::new(pycc_bin())
        .args(["run", src.to_str().unwrap()])
        .env("PATH", "/nonexistent")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error: could not run the linker driver"));
    assert!(!stderr.contains("panicked"));
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
    cross_pycc_rt_is_available("x86_64-apple-darwin")
}

/// Asks the same resolver `pycc build` itself uses whether a cross-target
/// `pycc_rt` debug build is present, instead of joining a `target/`-shaped
/// path here. That keeps these guards correct when `CARGO_TARGET_DIR`
/// redirects the artifact directory -- as this project's own coverage job
/// does -- where a manifest-relative literal would always report "not
/// available" and silently skip the tests it guards, which D-014 counts as
/// a coverage gap rather than a pass.
fn cross_pycc_rt_is_available(triple: &str) -> bool {
    let target_root = pycc_codegen::artifact_layout::resolve_cargo_target_root(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")),
        pycc_codegen::artifact_layout::cargo_target_dir_from_env,
    );
    pycc_codegen::artifact_layout::find_pycc_rt_lib_dir_in(
        &target_root,
        Some(triple),
        false,
        std::path::Path::exists,
    )
    .is_ok()
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
    if !cross_pycc_rt_is_available(triple) {
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
fn check_subcommand_honors_an_annotated_private_local_without_widening_its_source() {
    let dir = std::env::temp_dir().join(format!(
        "pycc_e2e_check_annotated_private_local_{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "annotated_private_local.py",
        "def _identity(x):\n    y: int = x\n    return y\n\nprint(_identity(True))\n",
    );

    let output = Command::new(pycc_bin())
        .args(["check", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"");
}

#[test]
fn build_accepts_an_annotated_private_local_with_a_bool_initializer() {
    let dir = std::env::temp_dir().join(format!(
        "pycc_e2e_build_annotated_private_local_{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "annotated_private_local.py",
        "def _identity(x):\n    y: int = x\n    return y\n\nprint(_identity(True))\n",
    );
    let out = dir.join("annotated_private_local");

    let status = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(status.success());
    // Do not execute this exact fixture here: #240 separately tracks the
    // backend's existing loss of bool object identity in an int-typed slot.
}

#[test]
fn annotated_private_local_does_not_widen_an_independently_returned_bool() {
    let dir = std::env::temp_dir().join(format!(
        "pycc_e2e_run_annotated_private_echo_{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "annotated_private_echo.py",
        "def _echo(x):\n    y: int = x\n    return x\n\nprint(_echo(True))\n",
    );
    let out = dir.join("annotated_private_echo");

    let status = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(status.success());
    let output = Command::new(out).output().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"True\n");
}

#[test]
fn check_subcommand_still_rejects_annotated_private_local_narrowing() {
    let dir = std::env::temp_dir().join(format!(
        "pycc_e2e_check_annotated_private_narrowing_{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "annotated_private_narrowing.py",
        "def _narrow(x):\n    y: bool = x\n    return y\n\nprint(_narrow(1))\n",
    );

    let output = Command::new(pycc_bin())
        .args(["check", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stdout).contains("T0025"));
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

// `pycc explain` (#366, D-150): one domain-representative code per
// `docs/DIAGNOSTICS.md` code range, chosen because each is the first code
// in its domain in the registry table and is a real, non-`(planned)`,
// non-internal-assertion-only code except where the domain has none
// (`O0201` is documented internal-only, which is fine for `explain` since
// it only documents the code, not whether it fires on legal Python).
const EXPLAIN_DOMAIN_REPRESENTATIVE_CODES: [&str; 7] = [
    "C0001", "L0001", "T0001", "O0201", "E0100", "I0401", "W1001",
];

#[test]
fn explain_subcommand_prints_a_human_explanation_for_a_known_code() {
    for code in EXPLAIN_DOMAIN_REPRESENTATIVE_CODES {
        let output = Command::new(pycc_bin())
            .args(["explain", code])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0), "code {code}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains(code),
            "stdout for {code} should contain its own code: {stdout}"
        );
    }
}

#[test]
fn explain_subcommand_human_output_contains_the_explanation_text() {
    let output = Command::new(pycc_bin())
        .args(["explain", "T0001"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("T0001 (error): public function missing annotation"));
    assert!(stdout.contains("Example:"));
    assert!(stdout.contains("def add(a: int, b: int) -> int:"));
}

#[test]
fn explain_subcommand_supports_json_format_for_every_domain_representative_code() {
    for code in EXPLAIN_DOMAIN_REPRESENTATIVE_CODES {
        let output = Command::new(pycc_bin())
            .args(["explain", code, "--format", "json"])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0), "code {code}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
            .unwrap_or_else(|e| panic!("code {code} produced invalid JSON: {e}\n{stdout}"));
        assert_eq!(parsed["format_version"], 1, "code {code}");
        assert_eq!(parsed["kind"], "diagnostic_explanation", "code {code}");
        assert_eq!(parsed["code"], code, "code {code}");
        assert!(parsed["severity"].is_string(), "code {code}");
    }
}

#[test]
fn explain_subcommand_rejects_an_unknown_format() {
    let output = Command::new(pycc_bin())
        .args(["explain", "T0001", "--format", "xml"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid value 'xml'"));
    assert!(stderr.contains("[possible values: human, json]"));
}

#[test]
fn explain_subcommand_rejects_an_unrecognized_code() {
    let output = Command::new(pycc_bin())
        .args(["explain", "X9999"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unknown diagnostic code"));
}

#[test]
fn explain_subcommand_rejects_an_unrecognized_code_with_plain_stderr_even_in_json_format() {
    // Deliberate: an unrecognized code is an out-of-band invocation
    // failure, not a diagnostic occurrence, so it is never subject to
    // `--format` -- the stderr message stays plain text, not a JSON error
    // object, regardless of `--format json`.
    let output = Command::new(pycc_bin())
        .args(["explain", "X9999", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unknown diagnostic code"));
    assert!(serde_json::from_str::<serde_json::Value>(stderr.trim()).is_err());
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
        seeded_builtin_exception_classes: false,
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
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    assert_eq!(pycc_types::check(&hir).unwrap_err().code, "T0023");
}

#[test]
fn direct_type_api_rejects_an_unconstrained_private_parameter() {
    let hir = pycc_hir::HirModule {
        seeded_builtin_exception_classes: false,
        items: vec![pycc_hir::HirItem::Function {
            name: "_unused".to_string(),
            params: vec![("value".to_string(), pycc_hir::Ty::Infer)],
            return_ty: pycc_hir::Ty::None,
            body: vec![],
        }],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    assert_eq!(
        pycc_types::check_and_resolve(&hir).unwrap_err().code,
        "T0021"
    );
}

#[test]
fn direct_type_api_rejects_an_unconstrained_private_return() {
    let hir = pycc_hir::HirModule {
        seeded_builtin_exception_classes: false,
        items: vec![pycc_hir::HirItem::Function {
            name: "_unknown".to_string(),
            params: vec![],
            return_ty: pycc_hir::Ty::Infer,
            body: vec![pycc_hir::HirStmt::Return(Some(pycc_hir::HirExpr::Name(
                "missing".to_string(),
            )))],
        }],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let err = pycc_types::check_and_resolve(&hir).unwrap_err();
    assert_eq!(err.code, "T0021");
    assert!(err.message.contains("return type"));
}

#[test]
fn direct_type_api_propagates_an_annotated_binary_result() {
    let hir = pycc_hir::HirModule {
        seeded_builtin_exception_classes: false,
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
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    assert!(pycc_types::check_and_resolve(&hir).is_ok());
}

#[test]
fn direct_type_api_rejects_incompatible_resolved_binary_operands() {
    let hir = pycc_hir::HirModule {
        seeded_builtin_exception_classes: false,
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
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
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
    // pycc_types::check's own inference pass, exercising check_frontend's
    // third (and otherwise untested) diagnostic-producing stage.
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
fn check_reports_capability_errors_and_continues_the_batch() {
    let dir = std::env::temp_dir().join(format!(
        "pycc_e2e_check_capability_batch_{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    // #435: `pass` is now supported (filtered as a no-op in `lower_body`),
    // so use `with` — a valid Python statement that is still unsupported —
    // to exercise the C0001 capability error path.
    let unsupported = write_fixture(&dir, "unsupported.py", "with open(\"x\") as f:\n    pass\n");
    let syntax_error = write_fixture(&dir, "syntax_error.py", "$\n");

    let output = Command::new(pycc_bin())
        .arg("check")
        .arg(&unsupported)
        .arg(&syntax_error)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(1));
    assert!(stdout.contains("error[C0001]"));
    assert!(stdout.contains(&format!("{}:1:1", rendered_diagnostic_path(&unsupported))));
    assert!(stdout.contains("1 | with open(\"x\") as f:"));
    assert!(stdout.contains("error[L0001]"));
    assert!(stdout.contains(&rendered_diagnostic_path(&syntax_error)));
    assert!(!stderr.contains("panicked"));
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
    assert!(output.stdout.is_empty());
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
        ("cr.py", b"print(1)\r$\r".as_slice()),
        ("crlf.py", b"print(1)\r\n$\r\n".as_slice()),
    ] {
        let src = dir.join(name);
        std::fs::write(&src, source).unwrap();
        let output = Command::new(pycc_bin())
            .arg("check")
            .arg(&src)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);

        assert_eq!(output.status.code(), Some(1));
        assert!(stdout.contains(&format!("{}:2:1", rendered_diagnostic_path(&src))));
        assert!(stdout.contains("2 | $\n"));
        assert!(!stdout.contains("$\r"));
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
    std::fs::write(&src, b"$\n").unwrap();

    let output = Command::new(pycc_bin())
        .arg("check")
        .arg(&src)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert_eq!(output.status.code(), Some(1));
    assert!(stdout.contains("error[L0001]"));
    assert!(stdout.contains("invalid_\u{fffd}.py"));
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
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(2));
    assert!(stdout.contains(&rendered_diagnostic_path(&invalid)));
    assert!(stdout.contains("L0001"));
    assert!(stdout.contains(&format!("{}:2:1", rendered_diagnostic_path(&invalid))));
    assert!(stdout.contains("2 | $"));
    assert!(stdout.contains("  | ^"));
    assert!(stderr.contains(&rendered_diagnostic_path(&missing)));
    assert!(stderr.contains("could not read"));
}

#[test]
fn check_aligns_a_diagnostic_caret_after_tab_indentation() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_check_tab_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "tab.py", "def main() -> None:\n\t$\n");

    let output = Command::new(pycc_bin())
        .arg("check")
        .arg(&src)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert_eq!(output.status.code(), Some(1));
    assert!(stdout.contains(&format!("{}:2:2", rendered_diagnostic_path(&src))));
    assert!(stdout.contains("2 | \t$"));
    assert!(stdout.contains("  | \t^"));
}

#[test]
fn check_uses_unicode_display_width_for_diagnostic_carets() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_check_unicode_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    for (filename, source, expected) in [
        (
            "wide.py",
            "\u{53d8}\u{91cf}$\n",
            "1 | \u{53d8}\u{91cf}$\n  |     ^",
        ),
        ("combining.py", "e\u{301}$\n", "1 | e\u{301}$\n  |  ^"),
    ] {
        let src = write_fixture(&dir, filename, source);
        let output = Command::new(pycc_bin())
            .arg("check")
            .arg(&src)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);

        assert_eq!(output.status.code(), Some(1));
        assert!(stdout.contains(expected), "stdout: {stdout}");
    }
}

#[test]
fn check_sizes_the_diagnostic_gutter_for_three_digit_line_numbers() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_check_gutter_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut source = "print(1)\n".repeat(99);
    source.push_str("$\n");
    let src = write_fixture(&dir, "line_100.py", &source);

    let output = Command::new(pycc_bin())
        .arg("check")
        .arg(&src)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert_eq!(output.status.code(), Some(1));
    assert!(stdout.contains(&format!("{}:100:1", rendered_diagnostic_path(&src))));
    assert!(stdout.contains("100 | $"));
    assert!(stdout.contains("    | ^"));
}

#[test]
fn check_without_files_is_a_clean_invocation_error() {
    let output = Command::new(pycc_bin()).arg("check").output().unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("at least one Python file"));
}

#[test]
fn repository_publishes_the_pycc_check_pre_commit_hook() {
    let manifest_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".pre-commit-hooks.yaml");
    let manifest = std::fs::read_to_string(&manifest_path)
        .unwrap()
        .replace("\r\n", "\n");

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

#[cfg(target_os = "windows")]
#[test]
fn native_windows_agent_lifecycle_and_policy_parsers_pass() {
    let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let python = std::env::var_os("PYTHON").unwrap_or_else(|| "python".into());
    for pattern in [
        "test_manage_ievo_hooks.py",
        "test_validate_agent_policies.py",
    ] {
        let output = Command::new(&python)
            .args([
                "-B", "-m", "unittest", "discover", "-s", "scripts", "-p", pattern,
            ])
            .current_dir(repository)
            .output()
            .expect("the Tier-1 Windows runner must provide Python");
        assert!(
            output.status.success(),
            "{pattern} failed under native Windows Python:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
}
