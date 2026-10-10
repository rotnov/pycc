//! #1490: a call from compiled code that reaches a function-pointer slot
//! whose `def` has not executed -- a function body calling a sibling defined
//! further down, or constructing a class above its class statement --
//! raises a catchable `NameError` instead of panicking.
//!
//! The panic could not unwind past the `extern "C"` boundary, so under
//! `--ext` it aborted the host interpreter (`SIGABRT`) where CPython raises.
//! A module compiled for a CPython host now raises CPython's own `NameError`
//! through the shim's `pycc_ext_name_error` and bridges it, so the host
//! catches the real class and compiled code catches it with
//! `except Exception` (pycc models no `NameError` class: `except NameError`
//! is `T0021`). A native build raises a pycc exception of `Exception`'s tag
//! named `NameError`.
//!
//! The slot is checked where CPython looks the name up: before a
//! function's arguments, before a constructed instance is allocated, and
//! between a method call's receiver and its other arguments.
//!
//! Each `--ext` program is run twice -- built with `--ext`, and imported as
//! the same source under CPython -- and the outputs compared, in the layout
//! `tests/issue_1199_ext_reentrant_init.rs` uses: the compiled module's
//! source in `src/`, the foreign helper that imports it back in `lib/`, the
//! artifact in `ext/`, and a working directory on neither `PYTHONPATH`.
//! Those tests, and the embedded one, are `#[ignore]`d for the reason every
//! `ext` test is: they build against and run an installed CPython with
//! development headers. The native test runs everywhere and pins the
//! CPython 3.14.7 output as a literal; the IR the coverage host sees is
//! pinned by `crates/pycc_codegen/src/unbound_slot_tests.rs`.

use pycc_scratch::ScratchDir;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn pycc() -> Command {
    Command::new(env!("CARGO_BIN_EXE_pycc"))
}

fn host_python() -> Command {
    Command::new(std::env::var_os("PYCC_PYTHON").unwrap_or_else(|| "python3".into()))
}

fn stdout_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n")
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n")
}

fn assert_ok(run: &Output) {
    assert!(
        run.status.success(),
        "stdout: {}\nstderr: {}",
        stdout_of(run),
        stderr_of(run)
    );
}

fn subdir(root: &Path, name: &str) -> PathBuf {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).expect("create the fixture directory");
    dir
}

fn write(dir: &Path, file: &str, body: &str) -> PathBuf {
    let path = dir.join(file);
    std::fs::write(&path, body).expect("write the fixture source");
    path
}

/// The last non-empty line of a run's stderr: the exception line of a
/// traceback, which is all the two implementations share.
fn last_stderr_line(output: &Output) -> String {
    stderr_of(output)
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .to_string()
}

/// `my.py` built with `--ext`, imported back by `cb.py` while its body is at
/// `import cb`.
struct Fixture {
    _root: ScratchDir,
    src: PathBuf,
    lib: PathBuf,
    ext: PathBuf,
    run: PathBuf,
}

impl Fixture {
    fn new(label: &str, module: &str, helper: &str) -> Self {
        let root = ScratchDir::new(label).expect("scratch");
        let src = subdir(&root, "src");
        let lib = subdir(&root, "lib");
        let ext = subdir(&root, "ext");
        let run = subdir(&root, "run");
        let source = write(&src, "my.py", module);
        write(&lib, "cb.py", helper);
        let build = pycc()
            .arg("build")
            .arg(&source)
            .arg("-o")
            .arg(ext.join("my"))
            .arg("--ext")
            .output()
            .expect("pycc should spawn");
        assert_ok(&build);
        Fixture {
            _root: root,
            src,
            lib,
            ext,
            run,
        }
    }

    /// Runs `script` with `first` ahead of `lib` on `PYTHONPATH`.
    ///
    /// Unbuffered, as every `ext` comparison in this tree runs: compiled
    /// `print` writes through pycc's own line-buffered stdout, not
    /// CPython's `sys.stdout`, which block-buffers into a pipe and would
    /// emit the host's lines only at exit -- after every compiled line,
    /// whatever order they ran in. Unbuffered, the captured order is the
    /// execution order for both runs, so the comparison checks it.
    fn run(&self, first: &Path, script: &str) -> Output {
        let path = std::env::join_paths([first, self.lib.as_path()]).expect("a PYTHONPATH");
        host_python()
            .arg("-c")
            .arg(script)
            .env("PYTHONPATH", path)
            .env("PYTHONUNBUFFERED", "1")
            .current_dir(&self.run)
            .output()
            .expect("python3 should spawn")
    }

    /// The driver's output against the extension, after asserting it
    /// matches CPython importing the same source.
    fn assert_matches_cpython(&self, script: &str) -> String {
        let compiled = self.run(&self.ext, script);
        let oracle = self.run(&self.src, script);
        assert_ok(&compiled);
        assert_ok(&oracle);
        assert_eq!(stdout_of(&compiled), stdout_of(&oracle));
        stdout_of(&compiled)
    }
}

/// The issue's own program: `make` constructs `C` above its class
/// statement, and `cb` calls it while `my`'s body is at `import cb`.
const ISSUE_MODULE: &str = r#"def make() -> object:
    return C()


import cb


class C:
    def __init__(self) -> None:
        self.v = 1
"#;

#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn the_host_catches_a_construction_above_its_class_statement() {
    let fixture = Fixture::new(
        "1490_issue",
        ISSUE_MODULE,
        r#"import my

try:
    my.make()
except NameError as e:
    print("host caught", type(e).__name__, e)
"#,
    );
    let out = fixture.assert_matches_cpython("import my\nprint(my.make().v)\n");
    assert_eq!(out, "host caught NameError name 'C' is not defined\n1\n");
}

#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn compiled_code_catches_the_name_error_with_except_exception() {
    let fixture = Fixture::new(
        "1490_compiled_catch",
        r#"def make() -> int:
    return C().v


def guarded() -> int:
    try:
        return make()
    except Exception as e:
        print("compiled caught", e)
        return -1


import cb


class C:
    def __init__(self) -> None:
        self.v = 1
"#,
        "import my\n\nprint(my.guarded())\n",
    );
    let out = fixture.assert_matches_cpython("import my\nprint(my.guarded())\n");
    assert_eq!(out, "compiled caught name 'C' is not defined\n-1\n1\n");
}

#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn an_unbound_function_call_raises_before_evaluating_its_arguments() {
    // CPython looks `late` up before it evaluates `side()`, so `side` never
    // prints.
    let fixture = Fixture::new(
        "1490_function_call",
        r#"def side() -> int:
    print("side")
    return 1


def call() -> int:
    return late(side())


import cb


def late(x: int) -> int:
    return x + 1
"#,
        r#"import my

try:
    my.call()
except NameError as e:
    print("host caught", e)
"#,
    );
    let out = fixture.assert_matches_cpython("import my\nprint(my.call())\n");
    assert_eq!(out, "host caught name 'late' is not defined\nside\n2\n");
}

/// A method call evaluates its receiver before the method lookup, and its
/// arguments after: `_make_d()` prints and then raises constructing `D`
/// above `class D`, so `side()` never runs and the error names `D`, not
/// the method. The unannotated private helper is how a function above a
/// class statement returns its instances (a forward class annotation is
/// `C0001`).
const RECEIVER_MODULE: &str = r#"def side() -> int:
    print("side")
    return 1


def _make_d():
    print("made")
    return D()


def use() -> int:
    return _make_d().m(side())


import cb


class D:
    def m(self, x: int) -> int:
        return x + 1
"#;

#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn a_method_call_evaluates_its_receiver_before_the_lookup() {
    let fixture = Fixture::new(
        "1490_receiver",
        RECEIVER_MODULE,
        r#"import my

try:
    my.use()
except NameError as e:
    print("host caught", e)
"#,
    );
    let out = fixture.assert_matches_cpython("import my\nprint(my.use())\n");
    assert_eq!(
        out,
        "made\nhost caught name 'D' is not defined\nmade\nside\n2\n"
    );
}

/// `C.s(...)` and `C.cm(...)` above `class C`: CPython fails looking `C`
/// up, before any argument, so both report the class rather than the
/// method -- pycc's mangled `C.s.static` / `C.cm.classmethod` never leak.
const CLASS_LEVEL_MODULE: &str = r#"def side() -> int:
    print("side")
    return 1


def use_static() -> int:
    return C.s(side())


def use_class() -> int:
    return C.cm(side())


import cb


class C:
    @staticmethod
    def s(x: int) -> int:
        return x + 1

    @classmethod
    def cm(cls, x: int) -> int:
        return x + 2
"#;

#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn a_class_level_call_above_its_class_reports_the_class() {
    let fixture = Fixture::new(
        "1490_class_level",
        CLASS_LEVEL_MODULE,
        r#"import my

for use in (my.use_static, my.use_class):
    try:
        use()
    except NameError as e:
        print("host caught", e)
"#,
    );
    let out = fixture
        .assert_matches_cpython("import my\nprint(my.use_static())\nprint(my.use_class())\n");
    assert_eq!(
        out,
        "host caught name 'C' is not defined\nhost caught name 'C' is not defined\n\
         side\n2\nside\n3\n"
    );
}

#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn a_module_body_raising_the_name_error_fails_the_import() {
    // The module body itself calls `make` above `class C`: the import fails
    // with the `NameError`, and the host catches it.
    let fixture = Fixture::new(
        "1490_module_body",
        r#"def make() -> object:
    return C()


print("before")
make()
print("unreached")


class C:
    pass
"#,
        "",
    );
    let out = fixture.assert_matches_cpython(
        r#"import sys
try:
    import my
except NameError as e:
    print("import failed", type(e).__name__, e)
print("my" in sys.modules)
"#,
    );
    assert_eq!(
        out,
        "before\nimport failed NameError name 'C' is not defined\nFalse\n"
    );
}

/// Exercises every shape at once in a native build: a construction and a
/// call caught by compiled `except Exception`, argument evaluation skipped,
/// and an uncaught one reported by class name.
const NATIVE_PROGRAM: &str = r#"def side() -> int:
    print("side")
    return 1


def make() -> int:
    return C().v


def run() -> int:
    return late(side())


def safe() -> str:
    try:
        make()
        return "made"
    except Exception as e:
        print("caught", e)
        return "x"


print(safe())
try:
    run()
except Exception as e:
    print("caught", e)
make()


def late(x: int) -> int:
    return x


class C:
    def __init__(self) -> None:
        self.v = 1
"#;

/// CPython 3.14.7's stdout for [`NATIVE_PROGRAM`].
const NATIVE_STDOUT: &str =
    "caught name 'C' is not defined\nx\ncaught name 'late' is not defined\n";

#[test]
fn a_native_build_raises_a_catchable_name_error() {
    let dir = ScratchDir::new("1490_native").expect("scratch");
    let source = write(&dir, "m.py", NATIVE_PROGRAM);
    let build = pycc()
        .arg("build")
        .arg(&source)
        .arg("-o")
        .arg(dir.join("m"))
        .output()
        .expect("pycc should spawn");
    assert_ok(&build);
    let run = Command::new(dir.join("m"))
        .output()
        .expect("the binary runs");
    assert_eq!(stdout_of(&run), NATIVE_STDOUT, "{}", stderr_of(&run));
    // An uncaught one exits non-zero by its class name, not by a panic.
    assert_eq!(run.status.code(), Some(1), "{}", stderr_of(&run));
    assert_eq!(last_stderr_line(&run), "NameError: name 'C' is not defined");
    assert!(!stderr_of(&run).contains("panicked"), "{}", stderr_of(&run));
}

#[test]
#[ignore = "requires CPython 3.14.7 as PYCC_PYTHON; run with --include-ignored"]
fn the_native_expectation_is_cpythons_output() {
    let dir = ScratchDir::new("1490_native_oracle").expect("scratch");
    let source = write(&dir, "m.py", NATIVE_PROGRAM);
    let oracle = host_python()
        .arg(&source)
        .output()
        .expect("CPython runs the oracle program");
    assert_eq!(stdout_of(&oracle), NATIVE_STDOUT);
    assert_eq!(
        last_stderr_line(&oracle),
        "NameError: name 'C' is not defined"
    );
}

/// [`RECEIVER_MODULE`]'s receiver order in a native build: the module body
/// calls `use` above `class D` and again below it.
const NATIVE_RECEIVER_PROGRAM: &str = r#"def side() -> int:
    print("side")
    return 1


def _make_d():
    print("made")
    return D()


def use() -> int:
    return _make_d().m(side())


try:
    use()
except Exception as e:
    print("caught", e)


class D:
    def m(self, x: int) -> int:
        return x + 1


print(use())
"#;

/// CPython 3.14.7's stdout for [`NATIVE_RECEIVER_PROGRAM`].
const NATIVE_RECEIVER_STDOUT: &str = "made\ncaught name 'D' is not defined\nmade\nside\n2\n";

#[test]
fn a_native_method_call_evaluates_its_receiver_before_the_lookup() {
    let dir = ScratchDir::new("1490_native_receiver").expect("scratch");
    let source = write(&dir, "m.py", NATIVE_RECEIVER_PROGRAM);
    let build = pycc()
        .arg("build")
        .arg(&source)
        .arg("-o")
        .arg(dir.join("m"))
        .output()
        .expect("pycc should spawn");
    assert_ok(&build);
    let run = Command::new(dir.join("m"))
        .output()
        .expect("the binary runs");
    assert_ok(&run);
    assert_eq!(stdout_of(&run), NATIVE_RECEIVER_STDOUT);
}

#[test]
#[ignore = "requires CPython 3.14.7 as PYCC_PYTHON; run with --include-ignored"]
fn the_native_receiver_expectation_is_cpythons_output() {
    let dir = ScratchDir::new("1490_native_receiver_oracle").expect("scratch");
    let source = write(&dir, "m.py", NATIVE_RECEIVER_PROGRAM);
    let oracle = host_python()
        .arg(&source)
        .output()
        .expect("CPython runs the oracle program");
    assert_ok(&oracle);
    assert_eq!(stdout_of(&oracle), NATIVE_RECEIVER_STDOUT);
}

/// [`CLASS_LEVEL_MODULE`]'s calls in a native build: a `@staticmethod`
/// and a `@classmethod` called on their class above its statement.
const NATIVE_CLASS_LEVEL_PROGRAM: &str = r#"def side() -> int:
    print("side")
    return 1


def use_static() -> int:
    return C.s(side())


def use_class() -> int:
    return C.cm(side())


try:
    use_static()
except Exception as e:
    print("caught", e)
try:
    use_class()
except Exception as e:
    print("caught", e)


class C:
    @staticmethod
    def s(x: int) -> int:
        return x + 1

    @classmethod
    def cm(cls, x: int) -> int:
        return x + 2


print(use_static())
print(use_class())
"#;

/// CPython 3.14.7's stdout for [`NATIVE_CLASS_LEVEL_PROGRAM`].
const NATIVE_CLASS_LEVEL_STDOUT: &str =
    "caught name 'C' is not defined\ncaught name 'C' is not defined\nside\n2\nside\n3\n";

#[test]
fn a_native_class_level_call_reports_the_class() {
    let dir = ScratchDir::new("1490_native_class_level").expect("scratch");
    let source = write(&dir, "m.py", NATIVE_CLASS_LEVEL_PROGRAM);
    let build = pycc()
        .arg("build")
        .arg(&source)
        .arg("-o")
        .arg(dir.join("m"))
        .output()
        .expect("pycc should spawn");
    assert_ok(&build);
    let run = Command::new(dir.join("m"))
        .output()
        .expect("the binary runs");
    assert_ok(&run);
    assert_eq!(stdout_of(&run), NATIVE_CLASS_LEVEL_STDOUT);
}

#[test]
#[ignore = "requires CPython 3.14.7 as PYCC_PYTHON; run with --include-ignored"]
fn the_native_class_level_expectation_is_cpythons_output() {
    let dir = ScratchDir::new("1490_native_class_level_oracle").expect("scratch");
    let source = write(&dir, "m.py", NATIVE_CLASS_LEVEL_PROGRAM);
    let oracle = host_python()
        .arg(&source)
        .output()
        .expect("CPython runs the oracle program");
    assert_ok(&oracle);
    assert_eq!(stdout_of(&oracle), NATIVE_CLASS_LEVEL_STDOUT);
}

/// An embedded executable compiles for a CPython host too, so its raise
/// goes through the shim, and the interpreter reports it.
#[cfg(not(windows))]
#[test]
#[ignore = "needs CPython 3.14.7 as python3.14 or PYCC_PYTHON; run with --include-ignored"]
fn an_embedded_program_raises_a_catchable_name_error() {
    let dir = ScratchDir::new("1490_embedded").expect("scratch");
    let source = write(&dir, "m.py", &format!("import json\n\n\n{NATIVE_PROGRAM}"));
    let build = pycc()
        .arg("build")
        .arg(&source)
        .arg("-o")
        .arg(dir.join("app"))
        .output()
        .expect("pycc should spawn");
    assert_ok(&build);
    let marker = std::fs::read_to_string(dir.join("app.pycc").join("PYCC-BUNDLE"))
        .expect("an embedded build writes its marker");
    let python = marker
        .lines()
        .find_map(|line| line.strip_prefix("executable "))
        .expect("the marker names its interpreter")
        .to_string();
    let run = Command::new(dir.join("app"))
        .output()
        .expect("the embedded binary runs");
    let oracle = Command::new(python)
        .arg(&source)
        .output()
        .expect("CPython runs the oracle program");
    assert_eq!(stdout_of(&run), stdout_of(&oracle), "{}", stderr_of(&run));
    assert_eq!(stdout_of(&run), NATIVE_STDOUT);
    assert!(!run.status.success(), "{}", stderr_of(&run));
    assert_eq!(last_stderr_line(&run), last_stderr_line(&oracle));
}
