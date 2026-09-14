//! Part 1 of #1026 (PR 1c of #1080): what a CPython `import` binds, and what
//! the compiler refuses to do with it.
//!
//! Three separable claims live here, and they are kept together because they
//! are the same user-visible story told at three levels:
//!
//! * `pycc check` *accepts* a plain foreign import -- it is a frontend-only
//!   pass (`docs/CLI_SPEC.md`) with no `--ext` flag to judge against, so a
//!   bound-but-unused module object is not an error there;
//! * a native `pycc build` refuses it with `I0403`, because a native
//!   executable embeds no interpreter to import into;
//! * every operation on the bound name is `I0404`, at each of the three
//!   choke points `crates/pycc_types/src/foreign.rs` documents.
//!
//! The hosted tests at the bottom are `#[ignore]`d and contribute no line
//! coverage (CI's coverage job runs `llvm-cov` without `--include-ignored`);
//! they are run by the Tier-1 `native-build-test` leg's
//! `cargo test --workspace -- --include-ignored`. Everything this change
//! needs *covered* is covered by the non-ignored tests above them and by the
//! unit tests in `src/foreign_import.rs`,
//! `crates/pycc_codegen/src/foreign_import.rs` and
//! `crates/pycc_types/src/foreign/tests.rs`.

use pycc_scratch::ScratchDir;
use std::path::Path;
use std::process::{Command, Output};

fn pycc() -> Command {
    Command::new(env!("CARGO_BIN_EXE_pycc"))
}

fn stdout_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Writes `body` to `dir/m.py` and returns the path.
fn source(dir: &Path, body: &str) -> std::path::PathBuf {
    let src = dir.join("m.py");
    std::fs::write(&src, body).expect("write the fixture source");
    src
}

fn check(dir: &Path, body: &str) -> Output {
    pycc()
        .arg("check")
        .arg(source(dir, body))
        .output()
        .expect("pycc should spawn")
}

/// `pycc check` is frontend-only and mode-agnostic: it has no `--ext` flag,
/// so it cannot know whether the program will be built as an extension
/// module, and refusing a foreign import there would make it unusable for
/// every `--ext` project. The import binds, nothing reads the binding, and
/// there is nothing to report.
#[test]
fn a_bound_but_unused_foreign_import_is_accepted_by_check() {
    let dir = ScratchDir::new("foreign_check_ok").expect("scratch");
    let output = check(&dir, "import numpy\n");
    assert_eq!(output.status.code(), Some(0), "{}", stderr_of(&output));
    assert_eq!(stdout_of(&output), "");
}

/// Work item 11: the driver gate. A native executable embeds no CPython
/// interpreter, so the import has no meaning there and the build refuses
/// before codegen -- which is what lets
/// `crates/pycc_codegen/src/foreign_import.rs` ignore the item under
/// `!options.ext` rather than assert.
///
/// This is `I0403`'s end-to-end assertion. It deliberately has no fixture
/// under `tests/diagnostics/`: that harness invokes `pycc check`, which
/// (see above) accepts the program, so the plan's "a fixture under
/// `tests/diagnostics/`" is not constructible for this code.
#[test]
fn a_native_build_refuses_a_foreign_import_with_i0403() {
    let dir = ScratchDir::new("foreign_native_refused").expect("scratch");
    let output = pycc()
        .arg("build")
        .arg(source(&dir, "import numpy\n"))
        .arg("-o")
        .arg(dir.join("m"))
        .output()
        .expect("pycc should spawn");
    assert_eq!(output.status.code(), Some(1), "{}", stderr_of(&output));
    // `build` renders diagnostics to stderr, where `check` renders to
    // stdout (`src/frontend.rs`'s `render_all`).
    let rendered = stderr_of(&output);
    assert!(rendered.contains("error[I0403]"), "{rendered}");
    assert!(rendered.contains("`import numpy`"), "{rendered}");
    assert!(rendered.contains("--ext"), "{rendered}");
}

/// Every shape that reads the binding, one row per choke point.
///
/// `tests/diagnostics/i0404_foreign_module_operation.py` pins the exact
/// rendering of one of them; this states the *set*. A shape that silently
/// started compiling instead of being refused is what Part 1's containment
/// invariant forbids -- a `Ty::Object` value must not escape into any
/// operation, because no operation on one is implemented yet.
#[test]
fn every_operation_on_a_foreign_module_is_refused_with_i0404() {
    let dir = ScratchDir::new("foreign_i0404").expect("scratch");
    let bodies = [
        // An expression-position read: assignment, argument, attribute,
        // call, f-string interpolation.
        "import numpy\n\nx = numpy\n",
        "import numpy\n\nprint(numpy)\n",
        "import numpy\n\nnumpy.append(1)\n",
        "import numpy\n\nnumpy(1)\n",
        "import numpy\n\nprint(f\"{numpy}\")\n",
        // The iterable of a `for` and of a comprehension.
        "import numpy\n\nfor x in numpy:\n    pass\n",
        "import numpy\n\nxs = [e for e in numpy]\n",
    ];
    for body in bodies {
        let output = check(&dir, body);
        assert_eq!(output.status.code(), Some(1), "{body}");
        assert!(stdout_of(&output).contains("error[I0404]"), "{body}");
    }
}

/// Rebinding is not an operation on the object, so it is not `I0404`: the
/// name already has a representation, and D-040's sticky representation
/// rule reports `T0023` against it. Pinned rendering lives in
/// `tests/diagnostics/t0023_foreign_module_rebinding.py`; what matters here
/// is that the two codes do not overlap.
#[test]
fn rebinding_a_foreign_module_is_t0023_rather_than_i0404() {
    let dir = ScratchDir::new("foreign_rebind").expect("scratch");
    let output = check(&dir, "import numpy\n\nnumpy = 3\n");
    let rendered = stdout_of(&output);
    assert!(rendered.contains("error[T0023]"), "{rendered}");
    assert!(!rendered.contains("I0404"), "{rendered}");
}

/// Builds `body` as an extension module named `module` inside `dir`.
fn build_ext(dir: &Path, module: &str, body: &str) {
    let build = pycc()
        .arg("build")
        .arg(source(dir, body))
        .arg("-o")
        .arg(dir.join(module))
        .arg("--ext")
        .output()
        .expect("pycc should spawn");
    assert!(build.status.success(), "{}", stderr_of(&build));
}

fn python(dir: &Path, script: &str) -> Output {
    Command::new(std::env::var_os("PYCC_PYTHON").unwrap_or_else(|| "python3".into()))
        .arg("-c")
        .arg(script)
        .current_dir(dir)
        .output()
        .expect("python3 should spawn")
}

/// The obligation recorded on #1080 (issuecomment-5658045798), deferred by
/// PR 1a and discharged here: a foreign import of a module that does not
/// exist must surface to the host as CPython's own `ModuleNotFoundError`,
/// raised out of the `Py_mod_exec` slot, and the extension module must not
/// end up in `sys.modules`.
///
/// That is the whole reason `pycc_ext_obj_import` returns CPython's `NULL`
/// convention untouched instead of translating the failure: the exception
/// the host sees is the one `PyImport_ImportModule` set.
#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn a_missing_foreign_module_raises_module_not_found_error_in_the_host() {
    let dir = ScratchDir::new("foreign_missing_hosted").expect("scratch");
    build_ext(
        &dir,
        "pycc_missing_import_mod",
        "import pycc_no_such_module_1080\n\ndef answer() -> int:\n    return 42\n",
    );
    let run = python(
        &dir,
        "import sys\n\
         try:\n\
         \x20   import pycc_missing_import_mod\n\
         except ModuleNotFoundError as e:\n\
         \x20   assert 'pycc_no_such_module_1080' in str(e), str(e)\n\
         else:\n\
         \x20   raise AssertionError('the import should have failed')\n\
         assert 'pycc_missing_import_mod' not in sys.modules\n",
    );
    assert!(
        run.status.success(),
        "stdout: {}\nstderr: {}",
        stdout_of(&run),
        stderr_of(&run)
    );
}

/// The success path of the same entry point: a module that really exists
/// imports, the `Py_mod_exec` slot returns 0, and the artifact's own
/// exports work afterwards. Without this, every hosted assertion here
/// would be satisfied by a `pycc_ext_obj_import` that always failed.
#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn an_existing_foreign_module_imports_and_the_artifact_stays_usable() {
    let dir = ScratchDir::new("foreign_present_hosted").expect("scratch");
    build_ext(
        &dir,
        "pycc_present_import_mod",
        "import json

def answer() -> int:
    return 42
",
    );
    let run = python(
        &dir,
        "import pycc_present_import_mod as m
assert m.answer() == 42, m.answer()
",
    );
    assert!(
        run.status.success(),
        "stdout: {}\nstderr: {}",
        stdout_of(&run),
        stderr_of(&run)
    );
}

/// The hosted half of the #1080 ordering obligation
/// (issuecomment-5658356849), and the only form of it that is *observable*
/// rather than structural: a module-body statement with a side effect,
/// written above a failing import, must have run before the import fails,
/// exactly as CPython runs a module body statement by statement (D-244
/// rule 3).
///
/// A failing import is what makes the ordering observable at all -- with a
/// successful one, both orders produce the same output. The structural
/// halves are non-ignored and carry the coverage:
/// `crates/pycc_mir/src/tests/import.rs` for the item's position in the IR
/// and `crates/pycc_codegen/src/foreign_import.rs` for the emitted call's
/// position in the entry block.
#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn a_statement_above_a_failing_foreign_import_has_already_run_in_the_host() {
    let dir = ScratchDir::new("foreign_order_hosted").expect("scratch");
    build_ext(
        &dir,
        "pycc_import_order_mod",
        "print(\"before the import\")\n\
         import pycc_no_such_module_1080\n\
         print(\"after the import\")\n\n\
         def answer() -> int:\n    return 42\n",
    );
    let run = python(
        &dir,
        "try:\n\
         \x20   import pycc_import_order_mod\n\
         except ModuleNotFoundError:\n\
         \x20   pass\n\
         else:\n\
         \x20   raise AssertionError('the import should have failed')\n",
    );
    assert!(
        run.status.success(),
        "stdout: {}\nstderr: {}",
        stdout_of(&run),
        stderr_of(&run)
    );
    let printed = stdout_of(&run);
    assert!(printed.contains("before the import"), "{printed}");
    assert!(!printed.contains("after the import"), "{printed}");
}
