//! #1508: a fully native build -- neither `--ext` nor an embedded
//! executable -- of a legacy `TypeVar`-generic program links no CPython
//! shim, so it must reference no `pycc_ext_*` host helper.
//!
//! A legacy `T = TypeVar("T")` annotation lowers to the opaque `object`
//! (`docs/TYPE_SYSTEM.md`, "Generics"). Two halves keep a native build of
//! it honest:
//!
//! - the type check of a native build refuses every seam that would box a
//!   native value into an `object` slot (`I0406`): the packers live in the
//!   shim, so such a program cannot run natively (`pycc_types::check_without_host`);
//! - a program that boxes nothing still compiles `object`-typed bodies, and
//!   `pycc_codegen`'s `native_host_stubs` gives every `pycc_ext_*`
//!   declaration they reference an internal trapping definition, so the
//!   module links.
//!
//! Before the fix, both shapes failed at the link with undefined
//! `pycc_ext_*` symbols. The hosted paths -- an `--ext` import and an
//! embedded executable -- still box and run as CPython does; those tests are
//! `#[ignore]`d (they need CPython 3.14.7) and run in the Tier-1
//! `native-build-test` leg with `--include-ignored`.

#![cfg(not(windows))]

use pycc_scratch::ScratchDir;
use std::path::PathBuf;
use std::process::{Command, Output};

fn pycc() -> Command {
    Command::new(env!("CARGO_BIN_EXE_pycc"))
}

fn stdout_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n")
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n")
}

fn python() -> std::ffi::OsString {
    std::env::var_os("PYCC_PYTHON").unwrap_or_else(|| "python3.14".into())
}

/// Writes `source` as `m.py` in a fresh scratch directory and runs
/// `pycc build` on it with `extra` arguments, writing `app`.
fn build(tag: &str, source: &str, extra: &[&str]) -> (ScratchDir, PathBuf, Output) {
    let dir = ScratchDir::new(tag).expect("scratch");
    let src = dir.join("m.py");
    std::fs::write(&src, source).expect("write the program");
    let output = pycc()
        .arg("build")
        .arg(&src)
        .arg("-o")
        .arg(dir.join("app"))
        .args(extra)
        .output()
        .expect("pycc should spawn");
    (dir, src, output)
}

/// Legacy `TypeVar` functions and a generic class whose bodies bind, print,
/// compare, format, select and pass `object` values -- every one of them a
/// shim helper reference -- but which nothing calls with a native value.
const BOXES_NOTHING: &str = "\
from typing import Generic, TypeVar

T = TypeVar(\"T\")


def ident(key: T) -> T:
    x = key
    print(x)
    if x == key:
        print(f\"{x}\")
    return x


def pick(a: T, b: T, c: bool) -> T:
    return a if c else b


class Box(Generic[T]):
    def __init__(self, v: T) -> None:
        self.v = v

    def get(self) -> T:
        return ident(self.v)


def total(n: int) -> int:
    return n * 2


print(total(21))
";

/// The native build links -- every host helper the `object` bodies
/// reference is defined in the module -- and runs.
#[test]
fn a_native_typevar_program_that_boxes_nothing_links_and_runs() {
    let (dir, _src, build) = build("t1508_native", BOXES_NOTHING, &[]);
    assert!(build.status.success(), "{}", stderr_of(&build));
    assert!(!dir.join("app.pycc").exists(), "a native build");
    let run = Command::new(dir.join("app"))
        .output()
        .expect("the native binary runs");
    assert_eq!(run.status.code(), Some(0), "{}", stderr_of(&run));
    assert_eq!(stdout_of(&run), "42\n");
}

const TYPE_VAR: &str = "from typing import Generic, TypeVar\n\nT = TypeVar(\"T\")\n\n";

/// Each program boxes a native value (or `None`) into a `T` slot -- an
/// argument, a constructor argument, a returned value, `return None` and a
/// bare `return` -- or builds a CPython `list` in one from a list display
/// (annotated, empty, in a conditional, rebound, or stored into an
/// attribute). A native build refuses each with `I0406` before codegen,
/// never with a link error or a trap at run time.
#[test]
fn a_native_build_refuses_boxing_into_a_typevar_slot_with_i0406() {
    for (shape, body) in [
        (
            "argument",
            "def ident(key: T) -> T:\n    x = key\n    return x\n\nprint(ident(3))\nprint(ident(\"s\"))\n",
        ),
        (
            "constructor",
            "class Box(Generic[T]):\n    def __init__(self, v: T) -> None:\n        self.v = v\n\nb = Box(1)\n",
        ),
        (
            "returned value",
            "def three() -> T:\n    return 3\n\nprint(1)\n",
        ),
        (
            "return None",
            "def nothing() -> T:\n    return None\n\nprint(nothing())\n",
        ),
        (
            "bare return",
            "def nothing() -> T:\n    return\n\nnothing()\n",
        ),
        // A list display bound to a `T` slot is built as a CPython `list`
        // (`HirExpr::ObjectList`); before the fix these built, linked and
        // then hit the trap stub at run time.
        (
            "annotated list display",
            "def f() -> None:\n    x: T = [1]\n\nf()\nprint(1)\n",
        ),
        (
            "empty list display",
            "def f() -> None:\n    x: T = []\n\nf()\nprint(1)\n",
        ),
        (
            "list display in a conditional",
            "def f(c: bool) -> None:\n    x: T = [1] if c else []\n\nf(True)\nprint(1)\n",
        ),
        (
            "rebinding to a list display",
            "def f(k: T) -> None:\n    s = k\n    s = []\n\nprint(1)\n",
        ),
        (
            "attribute store of a list display",
            "class B(Generic[T]):\n    v: T\n\n    def __init__(self, v: T) -> None:\n        self.v = v\n\n    def reset(self) -> None:\n        self.v = [1]\n\nprint(1)\n",
        ),
        (
            "attribute store of an `or` with a list display",
            "class B(Generic[T]):\n    v: T\n\n    def __init__(self, v: T) -> None:\n        self.v = v\n\n    def fill(self) -> None:\n        self.v = self.v or []\n\nprint(1)\n",
        ),
    ] {
        let source = format!("{TYPE_VAR}{body}");
        let (dir, src, build) = build("t1508_refused", &source, &[]);
        let stderr = stderr_of(&build);
        assert_eq!(build.status.code(), Some(1), "{shape}: {stderr}");
        assert!(stderr.contains("error[I0406]"), "{shape}: {stderr}");
        assert!(stderr.contains("PEP 695"), "{shape}: {stderr}");
        assert!(!stderr.contains("undefined reference"), "{shape}: {stderr}");
        assert!(!dir.join("app").exists(), "{shape}: no binary");
        // `pycc check` selects no artifact, so it admits the program: the
        // same source is a valid `--ext` module.
        let check = pycc()
            .arg("check")
            .arg(&src)
            .output()
            .expect("pycc should spawn");
        assert!(check.status.success(), "{shape}: {}", stderr_of(&check));
    }
}

/// The remedy the diagnostic names: a PEP 695 type parameter is
/// monomorphized per call site and builds and runs natively.
#[test]
fn the_pep_695_remedy_builds_natively() {
    let (dir, _src, build) = build(
        "t1508_pep695",
        "def ident[T](key: T) -> T:\n    x = key\n    return x\n\nprint(ident(3))\nprint(ident(\"s\"))\n",
        &[],
    );
    assert!(build.status.success(), "{}", stderr_of(&build));
    let run = Command::new(dir.join("app"))
        .output()
        .expect("the native binary runs");
    assert_eq!(run.status.code(), Some(0), "{}", stderr_of(&run));
    assert_eq!(stdout_of(&run), "3\ns\n");
}

/// The module an `--ext` build boxes into its `T` slots: the hosted path
/// is unchanged.
const EXT_MODULE: &str = "\
from typing import TypeVar

T = TypeVar(\"T\")


def ident(key: T) -> T:
    x = key
    return x


def three() -> T:
    return 3


def nothing() -> T:
    return None


def listed() -> T:
    x: T = [1, \"a\"]
    return x
";

/// An `--ext` module boxes native values into its `T` slots, builds a
/// CPython `list` from a display bound to one, and CPython
/// reads them back, as importing the same source does.
#[test]
#[ignore = "needs CPython 3.14.7 as python3.14 or PYCC_PYTHON; run with --include-ignored"]
fn an_ext_module_still_boxes_into_a_typevar_slot() {
    let dir = ScratchDir::new("t1508_ext").expect("scratch");
    std::fs::create_dir_all(dir.join("src")).expect("create the entry directory");
    let entry = dir.join("src").join("pycc_t1508_m.py");
    std::fs::write(&entry, EXT_MODULE).expect("write the module");
    std::fs::write(dir.join("pycc_t1508_py.py"), EXT_MODULE).expect("write the oracle");
    let build = pycc()
        .arg("build")
        .arg(&entry)
        .arg("-o")
        .arg(dir.join("pycc_t1508_m"))
        .arg("--ext")
        .output()
        .expect("pycc should spawn");
    assert!(build.status.success(), "{}", stderr_of(&build));
    let probe = |module: &str| {
        let run = Command::new(python())
            .arg("-c")
            .arg(format!(
                "import sys; sys.path.insert(0, '.'); import {module} as m\n\
                 print(m.ident(3), m.ident('s'), m.ident([1]), m.three(), m.nothing(), m.listed())"
            ))
            .current_dir(&*dir)
            .output()
            .expect("python should spawn");
        assert!(run.status.success(), "{}", stderr_of(&run));
        stdout_of(&run)
    };
    let compiled = probe("pycc_t1508_m");
    assert_eq!(compiled, probe("pycc_t1508_py"), "against CPython");
    assert_eq!(compiled, "3 s [1] 3 None [1, 'a']\n");
}

/// An embedded executable links the shim, so a foreign import makes the
/// same boxing legal: the program prints what CPython prints.
#[test]
#[ignore = "needs CPython 3.14.7 as python3.14 or PYCC_PYTHON; run with --include-ignored"]
fn an_embedded_executable_still_boxes_into_a_typevar_slot() {
    let source = "\
from fractions import Fraction
from typing import TypeVar

T = TypeVar(\"T\")


def ident(key: T) -> T:
    x = key
    return x


print(ident(3))
print(ident(\"s\"))
print(ident(Fraction(1, 3)))
";
    let (dir, src, build) = build("t1508_embedded", source, &[]);
    assert!(build.status.success(), "{}", stderr_of(&build));
    assert!(dir.join("app.pycc").is_dir(), "an embedded build");
    let oracle = Command::new(python())
        .arg(&src)
        .output()
        .expect("CPython runs the program");
    assert_eq!(stdout_of(&oracle), "3\ns\n1/3\n", "{}", stderr_of(&oracle));
    let run = Command::new(dir.join("app"))
        .output()
        .expect("the embedded binary runs");
    assert_eq!(run.status.code(), Some(0), "{}", stderr_of(&run));
    assert_eq!(stdout_of(&run), stdout_of(&oracle));
}
