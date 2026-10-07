//! #1476 (Part 3 of #1387): an `isinstance(o, C)` guard narrows an
//! `object` name back to `int`, `float`, `bool`, `str` or a class compiled
//! in the module, for the reads the guard dominates. A narrowed read unboxes
//! the `PyObject *` once (`pycc_ext_obj_unbox_*`); a read that packs the
//! value straight back into an object (a return to `object`, an identity
//! test, a list element) hands over the object itself.
//!
//! [`DRIVER`] runs against the `--ext` artifact of [`MODULE`] and against
//! the same source imported as plain Python, so CPython is the oracle for
//! every line. The deliberate deviations are pinned separately: an `int`
//! past 64 bits raises `OverflowError` at a narrowed read, a compiled-class
//! carrier whose `__init__` never ran raises `TypeError` where CPython
//! raises `AttributeError`, and a `str` subclass handed to a native `str`
//! parameter comes back a plain `str`.
//!
//! The hosted tests are `#[ignore]`d for the reason every `ext` test is;
//! the refusals need no interpreter and run everywhere.

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

fn write(dir: &Path, file: &str, body: &str) -> PathBuf {
    let path = dir.join(file);
    std::fs::write(&path, body).expect("write the fixture source");
    path
}

fn assert_ok(run: &Output) {
    assert!(
        run.status.success(),
        "stdout: {}\nstderr: {}",
        stdout_of(run),
        stderr_of(run)
    );
}

fn run(script: &str, path_entry: &Path, cwd: &Path) -> Output {
    host_python()
        .arg("-c")
        .arg(script)
        .env("PYTHONPATH", path_entry)
        .current_dir(cwd)
        .output()
        .expect("python3 should spawn")
}

/// Every narrowing shape, each reached from a public function the driver
/// calls.
const MODULE: &str = r#"class Token:
    def __init__(self, kind: str, value: int) -> None:
        self.kind = kind
        self.value = value

    def doubled(self) -> int:
        return self.value * 2


def as_int(o: object) -> int:
    if isinstance(o, int):
        return o + 1
    return -1


def as_float(o: object) -> float:
    if isinstance(o, float):
        return o * 2.0
    return -1.0


def as_bool(o: object) -> str:
    if isinstance(o, bool):
        if o:
            return "yes"
        return "no"
    return "not bool"


def as_str(o: object) -> str:
    if not isinstance(o, str):
        return "?"
    return o + "!"


def as_token(o: object) -> int:
    if isinstance(o, Token):
        return o.value + o.doubled()
    return 0


def kind_of(o: object) -> str:
    if isinstance(o, Token):
        return o.kind
    return "none"


def int_then_bool(o: object) -> int:
    if isinstance(o, int):
        if isinstance(o, bool):
            return -2
        return o + 10
    return 0


def same(o: object) -> object:
    if isinstance(o, str):
        return o
    return None


def is_same(o: object, p: object) -> bool:
    if isinstance(o, str):
        return o is p
    return False


def wrap(o: object) -> object:
    if isinstance(o, str):
        r: object = [o]
        return r
    return None


def shifted(o: object, n: int) -> int:
    xs = [i for i in range(n)]
    if isinstance(o, int):
        ys = [x + o for x in xs]
        total = 0
        for y in ys:
            total = total + y
        return total
    return -1


def keep(o: object) -> int:
    if isinstance(o, Token):
        t: Token = o
        o = None
        return t.value
    return -1


def _echo(s: str) -> str:
    return s


def relay(o: object) -> object:
    if isinstance(o, str):
        return _echo(o)
    return None
"#;

/// A `str` subclass instance keeps its identity through every use that
/// packs it back; a host-side class named `Token` is not the compiled one;
/// host `int` and `float` subclasses narrow like their bases.
const DRIVER: &str = r#"import narrowing as m


class S(str):
    pass


class Token:
    value = 99


class I(int):
    pass


class F(float):
    pass


t = m.Token("name", 4)
s = S("a")
print(m.as_int(41), m.as_int(True), m.as_int("x"), m.as_int(2.0))
print(m.as_float(1.25), m.as_float(3))
print(m.as_bool(True), m.as_bool(False), m.as_bool(1))
print(m.as_str("hi"), m.as_str(s), type(m.as_str(s)).__name__, m.as_str(3))
print(m.as_token(t), m.as_token(Token()), m.kind_of(t), m.kind_of("t"))
print(m.int_then_bool(True), m.int_then_bool(5), m.int_then_bool("5"))
print(m.same(s) is s, type(m.same(s)).__name__, m.is_same(s, s), m.is_same(s, S("a")))
w = m.wrap(s)
print(type(w).__name__, w[0] is s, type(w[0]).__name__)
print(m.shifted(2, 4), m.shifted("2", 4))
print(m.keep(t), m.keep(3))
print(m.as_int(I(5)), m.as_float(F(1.5)), m.int_then_bool(I(1)))
"#;

/// CPython 3.14.7's own output for [`DRIVER`] over [`MODULE`].
const EXPECTED: &str = "42 2 -1 -1
2.5 -1.0
yes no not bool
hi! a! str ?
12 0 name none
-2 15 0
True S True False
list True S
14 -1
4 -1
6 3.0 11
";

fn build_module(dir: &Path) -> (PathBuf, PathBuf) {
    let src_dir = dir.join("src");
    let out_dir = dir.join("out");
    std::fs::create_dir_all(&src_dir).expect("create the source directory");
    std::fs::create_dir_all(&out_dir).expect("create the output directory");
    let source = write(&src_dir, "narrowing.py", MODULE);
    let build = pycc()
        .arg("build")
        .arg(&source)
        .arg("-o")
        .arg(out_dir.join("narrowing"))
        .arg("--ext")
        .output()
        .expect("pycc should spawn");
    assert_ok(&build);
    (src_dir, out_dir)
}

#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn every_narrowing_shape_matches_cpython_in_an_ext_module() {
    let dir = ScratchDir::new("ext_1476_narrowing").expect("scratch");
    let (src_dir, out_dir) = build_module(&dir);
    let oracle = run(DRIVER, &src_dir, &dir);
    assert_ok(&oracle);
    assert_eq!(stdout_of(&oracle), EXPECTED, "CPython's own answer");
    let compiled = run(DRIVER, &out_dir, &dir);
    assert_ok(&compiled);
    assert_eq!(stdout_of(&compiled), EXPECTED);
}

/// The deliberate deviations, each against CPython's own answer: an `int`
/// past 64 bits does not fit the native `int` a narrowed read produces
/// (#1040), a carrier whose `__init__` never ran has no native instance to
/// hand over, and a `str` subclass handed to a native `str` parameter is
/// copied as a plain `str`.
#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn the_narrowed_read_deviations_are_the_documented_ones() {
    let dir = ScratchDir::new("ext_1476_deviations").expect("scratch");
    let (src_dir, out_dir) = build_module(&dir);
    let script = "import narrowing as m\n\
                  for f in (lambda: m.as_int(2**70), \
                  lambda: m.as_token(m.Token.__new__(m.Token))):\n    \
                  try:\n        print(f())\n    \
                  except Exception as e:\n        print(type(e).__name__)\n\
                  class S(str):\n    pass\n\
                  print(type(m.relay(S('a'))).__name__)\n";
    let oracle = run(script, &src_dir, &dir);
    assert_ok(&oracle);
    assert_eq!(
        stdout_of(&oracle),
        "1180591620717411303425\nAttributeError\nS\n"
    );
    let compiled = run(script, &out_dir, &dir);
    assert_ok(&compiled);
    assert_eq!(stdout_of(&compiled), "OverflowError\nTypeError\nstr\n");
}

fn refused(name: &str, body: &str) -> String {
    let dir = ScratchDir::new(name).expect("scratch");
    let output = pycc()
        .arg("build")
        .arg(write(&dir, "m.py", body))
        .arg("-o")
        .arg(dir.join("m"))
        .arg("--ext")
        .env("PYCC_PYTHON", "/nonexistent/pycc-no-python")
        .output()
        .expect("pycc should spawn");
    let rendered = stderr_of(&output);
    assert_eq!(output.status.code(), Some(1), "{rendered}");
    rendered
}

/// Past the guard the name is an `object` again.
#[test]
fn a_read_outside_the_guard_is_still_refused() {
    let rendered = refused(
        "ext_1476_outside",
        "def f(o: object) -> int:\n    if isinstance(o, int):\n        pass\n    return o + 1\n",
    );
    assert!(
        rendered.contains("error[T0021]: operator Add is not defined for `object` and `int`"),
        "{rendered}"
    );
}

/// A tuple of classes names no single native type to narrow to.
#[test]
fn a_tuple_of_classes_does_not_narrow() {
    let rendered = refused(
        "ext_1476_tuple",
        "def f(o: object) -> int:\n    if isinstance(o, (int, str)):\n        return o + 1\n    \
         return 0\n",
    );
    assert!(
        rendered.contains("error[T0021]: operator Add is not defined for `object` and `int`"),
        "{rendered}"
    );
}
