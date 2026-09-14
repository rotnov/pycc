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

/// The multi-file shape of the same gate: the `import` lives in a
/// dependency, not in the file named on the command line, and the `I0403`
/// must name the dependency (PR 1c of #1080 review finding 2).
///
/// Both orders are exercised because the import's recorded *item* index is
/// the item count at the moment it lowered, so a trailing import in the
/// dependency records the same linked index as a leading import in the
/// entry -- `src/frontend.rs`'s `owner_of_import` keys on the import
/// table's own position instead, which has no such ambiguity. The third row
/// is the control: an import the entry itself wrote still belongs to the
/// entry.
#[test]
fn a_foreign_import_in_a_dependency_names_the_dependency_not_the_entry() {
    let rows = [
        // (dep.py, main.py, the file the diagnostic must name)
        (
            "import numpy\ndef f() -> int:\n    return 1\n",
            "from dep import f\nx = f()\n",
            "dep.py",
        ),
        // The dependency's import is *trailing*: its item index equals the
        // dependency's own end bound, the boundary an item-index join would
        // hand to the entry file instead.
        (
            "def f() -> int:\n    return 1\nimport numpy\n",
            "from dep import f\nx = f()\n",
            "dep.py",
        ),
        // The entry's own import, at the same boundary index from the other
        // side.
        (
            "def f() -> int:\n    return 1\n",
            "from dep import f\nimport numpy\nx = f()\n",
            "main.py",
        ),
    ];
    for (dep, entry, owner) in rows {
        let dir = ScratchDir::new("foreign_native_multifile").expect("scratch");
        std::fs::write(dir.join("dep.py"), dep).expect("write the dependency");
        let main = dir.join("main.py");
        std::fs::write(&main, entry).expect("write the entry");
        let output = pycc()
            .arg("build")
            .arg(&main)
            .arg("-o")
            .arg(dir.join("m"))
            .output()
            .expect("pycc should spawn");
        assert_eq!(output.status.code(), Some(1), "{}", stderr_of(&output));
        let rendered = stderr_of(&output);
        assert!(rendered.contains("error[I0403]"), "{rendered}");
        let located = format!("{}:1:1", dir.join(owner).display());
        assert!(rendered.contains(&located), "{owner}: {rendered}");
        let other = if owner == "dep.py" {
            "main.py"
        } else {
            "dep.py"
        };
        assert!(
            !rendered.contains(&format!("{}:", dir.join(other).display())),
            "{owner}: {rendered}"
        );
    }
}

/// A type error in the program is still reported instead of the `I0403`:
/// the native gate is computed before the type check (its indices have to
/// match the pre-monomorphization item list) but reported after it, so the
/// ordering the single-file gate had is unchanged.
#[test]
fn a_type_error_is_reported_before_the_native_foreign_refusal() {
    let dir = ScratchDir::new("foreign_native_type_error").expect("scratch");
    let output = pycc()
        .arg("build")
        .arg(source(&dir, "import numpy\n\nx: int = \"s\"\n"))
        .arg("-o")
        .arg(dir.join("m"))
        .output()
        .expect("pycc should spawn");
    assert_eq!(output.status.code(), Some(1), "{}", stderr_of(&output));
    let rendered = stderr_of(&output);
    assert!(!rendered.contains("I0403"), "{rendered}");
}

/// Finding 1 of the same review: a private helper that returns the bound
/// module object must be refused with the documented `I0404`, not with a
/// `T0021` telling the user to add a return annotation. No annotation can
/// satisfy that advice -- the foreign object type is deliberately
/// unspellable -- so the solver's `Name` arm hands back the concrete
/// `Ty::Object` term and lets the check phase report the real refusal.
#[test]
fn an_unannotated_helper_returning_a_foreign_module_is_i0404_not_t0021() {
    let dir = ScratchDir::new("foreign_helper_return").expect("scratch");
    let output = check(
        &dir,
        "import numpy\n\ndef _helper():\n    return numpy\n\nx = _helper()\n",
    );
    assert_eq!(output.status.code(), Some(1), "{}", stderr_of(&output));
    let rendered = stdout_of(&output);
    assert!(rendered.contains("error[I0404]"), "{rendered}");
    assert!(!rendered.contains("T0021"), "{rendered}");
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

/// PR 1c of #1080 review finding 2, end to end. `def json()` above
/// `import json` is a program CPython runs and then fails on
/// (`TypeError: 'module' object is not callable`), because the import
/// rebinds the name to the module object. pycc used to accept it and emit
/// a call to the shadowed function; the import now supersedes the earlier
/// `def` at its own position, so the call is refused.
///
/// `import json` is deliberately a *real* stdlib module pycc does not
/// implement, which is what makes it a foreign binding rather than a
/// `pycc_std` one.
#[test]
fn a_foreign_import_below_a_same_named_def_refuses_the_later_call() {
    let dir = ScratchDir::new("foreign_shadows_def").expect("scratch");
    let output = check(
        &dir,
        "def json() -> int:\n    return 1\n\n\nimport json\n\nx = json()\n",
    );
    assert_eq!(output.status.code(), Some(1), "{}", stderr_of(&output));
    assert!(stdout_of(&output).contains("error[I0404]"), "{output:?}");
}

/// The other order stays accepted: the `def` runs after the import and
/// rebinds the name to a function, exactly as CPython does, so the call is
/// an ordinary call. Pinned so the refusal above cannot quietly widen into
/// an over-rejection.
#[test]
fn a_def_below_a_foreign_import_keeps_the_call_accepted() {
    let dir = ScratchDir::new("foreign_shadowed_by_def").expect("scratch");
    let output = check(
        &dir,
        "import json\n\ndef json() -> int:\n    return 1\n\n\nx = json()\n",
    );
    assert_eq!(output.status.code(), Some(0), "{}", stdout_of(&output));
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

/// PR 1c of #1080 review finding 1, end to end. `monomorphize` drops every
/// original generic function, which used to leave the import's recorded
/// position pointing past the end of the item list: this exact module
/// aborted the build with `insertion index (is 2) should be <= len (is 0)`
/// out of `pycc_mir::splice_foreign_imports`. The structural assertions --
/// that the recomputed position is both in range and still ahead of the
/// items that followed the import in the source -- are non-ignored, in
/// `crates/pycc_types/src/foreign/tests.rs`; this is the hosted
/// confirmation that the artifact such a module produces really loads.
#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn a_module_whose_generics_are_all_dropped_still_builds_and_loads() {
    let dir = ScratchDir::new("foreign_dropped_generics_hosted").expect("scratch");
    build_ext(
        &dir,
        "pycc_dropped_generics_mod",
        "def _a[T](x: T) -> T:\n    return x\n\n\n\
         def _b[T](x: T) -> T:\n    return x\n\n\
         import json\n\n\
         def answer() -> int:\n    return 42\n",
    );
    let run = python(
        &dir,
        "import pycc_dropped_generics_mod as m\nassert m.answer() == 42, m.answer()\n",
    );
    assert!(
        run.status.success(),
        "stdout: {}\nstderr: {}",
        stdout_of(&run),
        stderr_of(&run)
    );
}
