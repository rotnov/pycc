//! Part 2 of #1026 (PR 2b of #1081): calling a method on a CPython object.
//!
//! `tests/issue_1080_foreign_object.rs` owns what a foreign `import` binds
//! and the refusal table around it, including the one row this PR flips
//! (`numpy.sqrt(2.0)`, refused as `T0043` by PR 2a and accepted here). This
//! file owns the capability itself: what the checker still refuses about a
//! call, that PR 2a's positional bound is inherited unchanged, and -- in the
//! `#[ignore]`d hosted tests at the bottom -- that a real CPython
//! interpreter observes the call's effect and its exceptions.
//!
//! The hosted tests contribute no line coverage (CI's coverage job runs
//! `llvm-cov` without `--include-ignored`); they are run by the Tier-1
//! `native-build-test` leg's `cargo test --workspace -- --include-ignored`.
//! Every new Rust line is covered by the non-ignored tests here and by the
//! unit tests in `crates/pycc_codegen/src/foreign_call.rs`.

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

/// Every argument type the shim has a `pycc_ext_obj_pack_*` helper for is
/// admitted, and zero arguments are too.
///
/// The zero-argument row is the acceptance shape (`gc.disable()`); the
/// other four are the marshalling surface. Asserted through `pycc check`
/// rather than a build so the row set stays readable and fast -- the
/// emitted code for each packer is asserted at the IR level in
/// `crates/pycc_codegen/src/foreign_call.rs`.
#[test]
fn every_marshallable_argument_type_is_admitted() {
    let dir = ScratchDir::new("foreign_call_arg_types").expect("scratch");
    for body in [
        "import gc\n\ngc.disable()\n",
        "import gc\n\ngc.set_threshold(700)\n",
        "import json\n\njson.dumps(2.0)\n",
        "import gc\n\ngc.set_debug(True)\n",
        "import json\n\njson.dumps(\"x\")\n",
        // Several arguments at once, and a name rather than a literal.
        "import gc\n\nx: int = 1\ngc.set_threshold(700, x)\n",
    ] {
        let output = check(&dir, body);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{body}: {}",
            stdout_of(&output)
        );
    }
}

/// An argument whose type has no boundary representation is refused with
/// `I0404`, naming the type.
///
/// The `Ty::Object` row is the one that matters most: `gc.foo(math.pi)`
/// would otherwise reach codegen with a `Scalar::Object` argument the
/// packer table has no entry for, and the refusal is what makes
/// `foreign_call::packer_for`'s panic a front-end-defect assertion rather
/// than a reachable failure.
#[test]
fn an_unmarshallable_argument_is_refused_with_i0404() {
    let dir = ScratchDir::new("foreign_call_bad_arg").expect("scratch");
    for (body, ty) in [
        (
            "import json\nimport gc\n\ngc.set_debug(json.dumps)\n",
            "object",
        ),
        ("import gc\n\ngc.set_debug([1])\n", "list[int]"),
        ("import gc\n\ngc.set_debug(None)\n", "None"),
    ] {
        let output = check(&dir, body);
        assert_eq!(
            output.status.code(),
            Some(1),
            "{body}: {}",
            stderr_of(&output)
        );
        let rendered = stdout_of(&output);
        assert!(rendered.contains("error[I0404]"), "{body}: {rendered}");
        assert!(
            rendered.contains(&format!(
                "passing a `{ty}` argument to a CPython object's method"
            )),
            "{body}: {rendered}"
        );
    }
}

/// PR 2a's positional bound is inherited unchanged: a call inside a
/// function body is `I0404`, and a call above the `import` is `T0021`.
///
/// This is the load-bearing pair. Both bounds are what make PR 2b's
/// failure protocol free: every admitted call is emitted inside
/// `pycc_ext_module_exec`, the one function with the `ret i64 -1` edge a
/// failed call takes, so no CPython-to-pycc exception bridge is needed.
/// Lifting either bound would cost both an ordering analysis and that
/// bridge.
#[test]
fn a_method_call_inherits_the_positional_bound() {
    let dir = ScratchDir::new("foreign_call_position").expect("scratch");

    let output = check(
        &dir,
        "import gc\n\n\ndef off() -> None:\n    gc.disable()\n",
    );
    assert_eq!(output.status.code(), Some(1), "{}", stderr_of(&output));
    let rendered = stdout_of(&output);
    assert!(rendered.contains("error[I0404]"), "{rendered}");

    let output = check(&dir, "gc.disable()\n\nimport gc\n");
    assert_eq!(output.status.code(), Some(1), "{}", stderr_of(&output));
    let rendered = stdout_of(&output);
    assert!(rendered.contains("error[T0021]"), "{rendered}");
    assert!(rendered.contains("`gc` is not defined"), "{rendered}");
}

/// A native `pycc build` still refuses the import itself with `I0403`
/// before any of this is reachable: a native executable embeds no
/// interpreter to call into.
#[test]
fn a_native_build_of_a_method_call_is_still_refused_with_i0403() {
    let dir = ScratchDir::new("foreign_call_native").expect("scratch");
    let output = pycc()
        .arg("build")
        .arg(source(&dir, "import gc\n\ngc.disable()\n"))
        .arg("-o")
        .arg(dir.join("m"))
        .output()
        .expect("pycc should spawn");
    assert_eq!(output.status.code(), Some(1), "{}", stdout_of(&output));
    // `build` renders diagnostics to stderr where `check` renders to stdout
    // (`src/frontend.rs`'s `render_all`).
    let rendered = stderr_of(&output);
    assert!(rendered.contains("error[I0403]"), "{rendered}");
}

/// Builds `body` as `<module>.so` in `dir` with `pycc build --ext`.
fn build_ext(dir: &Path, module: &str, body: &str) {
    let src = dir.join(format!("{module}.py"));
    std::fs::write(&src, body).expect("write the fixture source");
    let build = pycc()
        .arg("build")
        .arg(&src)
        .arg("-o")
        .arg(dir.join(format!("{module}.so")))
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

/// **The acceptance test for #1081.** `import gc` / `gc.disable()`, built
/// as an extension module, and the host observes `gc.isenabled()` go
/// `False` merely by importing it.
///
/// This is the capability PR 2a deliberately did not ship: every program 2a
/// admitted did nothing observable with the CPython object it loaded. Here
/// the compiled module body reaches into the hosting interpreter and
/// changes its state.
#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn a_zero_argument_method_call_has_its_effect_in_the_host() {
    let dir = ScratchDir::new("foreign_call_gc_hosted").expect("scratch");
    build_ext(&dir, "pycc_gc_disable_mod", "import gc\n\ngc.disable()\n");
    let run = python(
        &dir,
        "import gc\n\
         assert gc.isenabled(), 'the host starts with gc enabled'\n\
         import pycc_gc_disable_mod\n\
         assert not gc.isenabled(), 'importing the artifact must have disabled gc'\n",
    );
    assert!(
        run.status.success(),
        "stdout: {}\nstderr: {}",
        stdout_of(&run),
        stderr_of(&run)
    );
}

/// The argument half of the same claim: an `int` argument really reaches
/// CPython with its value intact.
///
/// `gc.set_threshold` is chosen because its effect is readable back through
/// `gc.get_threshold()`, so this asserts the marshalled *value* rather than
/// only that a call happened.
#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn an_int_argument_reaches_the_host_with_its_value() {
    let dir = ScratchDir::new("foreign_call_threshold_hosted").expect("scratch");
    build_ext(
        &dir,
        "pycc_gc_threshold_mod",
        "import gc\n\ngc.set_threshold(12345)\n",
    );
    let run = python(
        &dir,
        "import gc\n\
         import pycc_gc_threshold_mod\n\
         assert gc.get_threshold()[0] == 12345, gc.get_threshold()\n",
    );
    assert!(
        run.status.success(),
        "stdout: {}\nstderr: {}",
        stdout_of(&run),
        stderr_of(&run)
    );
}

/// A missing method surfaces as CPython's own `AttributeError`, and the
/// module body stops there.
///
/// The same edge PR 2a's `a_failed_attribute_lookup_raises_attribute_error_in_the_host`
/// asserts for a load, now for a call: `pycc_ext_obj_call` returns `NULL`
/// with CPython's exception set and `foreign_call::emit`'s `NULL` check
/// returns `-1` from the `Py_mod_exec` slot. The `print` below the call is
/// what proves the body stopped rather than merely reported.
#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn a_missing_method_raises_attribute_error_in_the_host() {
    let dir = ScratchDir::new("foreign_call_missing_hosted").expect("scratch");
    build_ext(
        &dir,
        "pycc_missing_method_mod",
        "import gc\n\ngc.pycc_no_such_method_1081()\nprint(\"ran past the call\")\n",
    );
    let run = python(
        &dir,
        "try:\n\
         \x20   import pycc_missing_method_mod\n\
         except AttributeError as e:\n\
         \x20   assert 'pycc_no_such_method_1081' in str(e), str(e)\n\
         else:\n\
         \x20   raise AssertionError('the method call should have failed')\n",
    );
    assert!(
        run.status.success(),
        "stdout: {}\nstderr: {}",
        stdout_of(&run),
        stderr_of(&run)
    );
    assert!(
        !stdout_of(&run).contains("ran past the call"),
        "the module body must stop at the failed call: {}",
        stdout_of(&run)
    );
}

/// A method that *raises* surfaces its own exception, not an
/// `AttributeError` and not a `SystemError`.
///
/// `json.loads("not json")` also exercises the `str` packer end to end: the
/// `ValueError` only happens if the argument really arrived as that text.
/// (`math` is not a foreign module -- `pycc_std::resolve_module` answers
/// `StdModule::Math` for it, so `import math` never produces a
/// `Ty::Object`; every fixture here uses a module pycc has no native
/// implementation of.)
#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn a_raising_method_surfaces_its_own_exception_in_the_host() {
    let dir = ScratchDir::new("foreign_call_raises_hosted").expect("scratch");
    build_ext(
        &dir,
        "pycc_raising_method_mod",
        "import json\n\njson.loads(\"not json\")\nprint(\"ran past the call\")\n",
    );
    let run = python(
        &dir,
        "try:\n\
         \x20   import pycc_raising_method_mod\n\
         except ValueError as e:\n\
         \x20   assert 'Expecting value' in str(e), str(e)\n\
         else:\n\
         \x20   raise AssertionError('the method call should have raised')\n",
    );
    assert!(
        run.status.success(),
        "stdout: {}\nstderr: {}",
        stdout_of(&run),
        stderr_of(&run)
    );
    assert!(
        !stdout_of(&run).contains("ran past the call"),
        "the module body must stop at the raising call: {}",
        stdout_of(&run)
    );
}

/// A `str` argument crosses as an independent CPython `str` with the right
/// contents, and the artifact stays usable afterwards.
///
/// The packer *borrows* its `PyStrObj` rather than consuming it (unlike the
/// `str` *result* packer, which consumes -- `docs/RUNTIME.md` owns the
/// distinction). Passing the same name to two calls is what would expose a
/// premature release: under a consuming packer the second call would read a
/// freed string.
#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn a_str_argument_may_be_passed_twice_without_a_premature_release() {
    let dir = ScratchDir::new("foreign_call_str_hosted").expect("scratch");
    build_ext(
        &dir,
        "pycc_str_arg_mod",
        "import warnings\n\ns: str = \"pycc-1081\"\n\
         warnings.filterwarnings(\"ignore\", s)\n\
         warnings.filterwarnings(\"ignore\", s)\n\n\
         def answer() -> int:\n    return 42\n",
    );
    let run = python(
        &dir,
        "import warnings\n\
         import pycc_str_arg_mod as m\n\
         assert m.answer() == 42, m.answer()\n\
         assert any('pycc-1081' in str(f[1]) for f in warnings.filters), warnings.filters\n",
    );
    assert!(
        run.status.success(),
        "stdout: {}\nstderr: {}",
        stdout_of(&run),
        stderr_of(&run)
    );
}

/// An `int` argument outside the D-141 inline range reaches the host as an
/// `OverflowError` naming the range, and the module body stops there.
///
/// This is the one refusal the *type* checker cannot make: `n` is an `int`
/// like any other, and only its run-time word tells `pycc_rt_ext_int_classify`
/// it is a heap bigint. `pycc_ext_obj_pack_int` therefore raises rather than
/// truncating or aborting, on the same edge a failed call takes -- the shim
/// sees a `NULL` argument, skips the attribute lookup and the vectorcall
/// entirely, and hands the module-exec slot its `-1`. `#1040` is the issue
/// that would widen the boundary to a real bigint; until it lands this arm is
/// the documented boundary behavior, not a defect.
///
/// The C shim is outside `cargo llvm-cov`'s denominator, so this test is the
/// only thing that exercises the arm at all.
#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn a_bigint_int_argument_raises_overflow_error_in_the_host() {
    let dir = ScratchDir::new("foreign_call_bigint_hosted").expect("scratch");
    build_ext(
        &dir,
        "pycc_bigint_arg_mod",
        // 2**62 -- the first value the inline range excludes.
        "import gc\n\nn: int = 4611686018427387904\ngc.set_threshold(n)\nprint(\"ran past the call\")\n",
    );
    let run = python(
        &dir,
        "try:\n\
         \x20   import pycc_bigint_arg_mod\n\
         except OverflowError as e:\n\
         \x20   assert '2**62' in str(e), str(e)\n\
         \x20   assert '#1040' in str(e), str(e)\n\
         else:\n\
         \x20   raise AssertionError('the bigint argument should have been refused')\n",
    );
    assert!(
        run.status.success(),
        "stdout: {}\nstderr: {}",
        stdout_of(&run),
        stderr_of(&run)
    );
    assert!(
        !stdout_of(&run).contains("ran past the call"),
        "the module body must stop at the refused call: {}",
        stdout_of(&run)
    );
}
