//! End-to-end proof for `__slots__` in a class body ([#1368]).
//!
//! `docs/TYPE_SYSTEM.md`'s class row and "Current state" paragraph are the
//! contract. The byte-exact oracle fixture is `tests/fixtures/instance_slots.py`
//! (registered in `tests/conformance/classes.rs`, pinned-oracle only); this
//! file runs the same fixture against whatever `python3` is available (every
//! shape in it behaves the same on CPython 3.9 through 3.14) and against its
//! recorded output, and owns the CLI refusals.
//!
//! [#1368]: https://github.com/rotnov/pycc/issues/1368

use pycc_scratch::ScratchDir;
use std::path::Path;
use std::process::{Command, Output};

fn pycc() -> Command {
    Command::new(env!("CARGO_BIN_EXE_pycc"))
}

fn python() -> Command {
    Command::new(std::env::var_os("PYCC_PYTHON").unwrap_or_else(|| "python3".into()))
}

fn rendered(output: &Output) -> String {
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    text.replace("\r\n", "\n")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n")
}

fn fixture() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/instance_slots.py")
}

const FIXTURE_OUTPUT: &str =
    "2 3 3\n7\n3\nlisted\n3\n4\n5\n10\n11 12\n13\n14 15\n17 18\n20 21\ncaught\n";

/// The fixture builds, runs, and prints its recorded output, which is also
/// what the available CPython prints for it.
#[test]
fn the_fixture_matches_cpython() {
    let dir = ScratchDir::new("e2e_1368_fixture").expect("scratch");
    let build = pycc()
        .arg("build")
        .arg(fixture())
        .arg("-o")
        .arg(dir.join("app"))
        .output()
        .expect("pycc should spawn");
    assert!(build.status.success(), "{}", rendered(&build));
    let run = Command::new(dir.join("app"))
        .output()
        .expect("the program should spawn");
    assert!(run.status.success(), "{}", rendered(&run));
    assert_eq!(stdout(&run), FIXTURE_OUTPUT);

    let oracle = python()
        .arg(fixture())
        .output()
        .expect("python3 should spawn");
    assert!(oracle.status.success(), "{}", rendered(&oracle));
    assert_eq!(stdout(&oracle), FIXTURE_OUTPUT);
}

/// Runs `pycc check a.py` from a scratch directory holding `source` as
/// `a.py`, asserting failure, and returns the rendered diagnostics.
fn fails(category: &str, source: &str) -> String {
    let dir = ScratchDir::new(category).expect("scratch");
    std::fs::write(dir.join("a.py"), source).expect("write the subject");
    let output = pycc()
        .arg("check")
        .arg("a.py")
        .current_dir(dir.join("."))
        .output()
        .expect("pycc should spawn");
    assert!(!output.status.success(), "{source:?} was accepted");
    rendered(&output)
}

/// Runs `source` under the available CPython, asserting it fails with
/// `error` in its standard error -- the class-creation errors are the same
/// on every supported version.
fn cpython_rejects(category: &str, source: &str, error: &str) {
    let dir = ScratchDir::new(category).expect("scratch");
    std::fs::write(dir.join("a.py"), source).expect("write the subject");
    let output = python()
        .arg("a.py")
        .current_dir(dir.join("."))
        .output()
        .expect("python3 should spawn");
    assert!(!output.status.success(), "CPython accepted {source:?}");
    assert!(rendered(&output).contains(error), "{}", rendered(&output));
}

#[test]
fn an_undeclared_store_is_t0044() {
    let source = "class C:\n    __slots__ = ('a',)\n\n    def __init__(self) -> None:\n        \
                  self.a = 1\n        self.b = 2\n\n\nC()\n";
    let text = fails("e2e_1368_undeclared", source);
    assert!(text.contains("error[T0044]"), "{text}");
    assert!(
        text.contains("class `C` has no slot for attribute `b`"),
        "{text}"
    );
    assert!(text.contains("a.py:6:9"), "{text}");
    cpython_rejects(
        "e2e_1368_undeclared_cpython",
        source,
        "AttributeError: 'C' object has no attribute 'b'",
    );
}

#[test]
fn each_class_creation_error_quotes_cpython() {
    for (category, source, cpython_error) in [
        (
            "e2e_1368_class_var",
            "class C:\n    __slots__ = ('a',)\n    a = 1\n",
            "ValueError: 'a' in __slots__ conflicts with class variable",
        ),
        (
            "e2e_1368_method",
            "class C:\n    __slots__ = ('f',)\n\n    def f(self) -> int:\n        return 1\n",
            "ValueError: 'f' in __slots__ conflicts with class variable",
        ),
        (
            "e2e_1368_init",
            "class C:\n    __slots__ = ('__init__',)\n\n    def __init__(self) -> None:\n        \
             pass\n",
            "ValueError: '__init__' in __slots__ conflicts with class variable",
        ),
        (
            "e2e_1368_private",
            "class C:\n    __slots__ = ('__x',)\n    __x = 1\n",
            "ValueError: '_C__x' in __slots__ conflicts with class variable",
        ),
        (
            "e2e_1368_doc",
            "class C:\n    \"\"\"Doc.\"\"\"\n    __slots__ = ('__doc__',)\n",
            "ValueError: '__doc__' in __slots__ conflicts with class variable",
        ),
        (
            "e2e_1368_int",
            "class C:\n    __slots__ = 1\n",
            "TypeError: 'int' object is not iterable",
        ),
        (
            "e2e_1368_int_item",
            "class C:\n    __slots__ = (1,)\n",
            "TypeError: __slots__ items must be strings, not 'int'",
        ),
        (
            "e2e_1368_identifier",
            "class C:\n    __slots__ = ('a b',)\n",
            "TypeError: __slots__ must be identifiers",
        ),
        (
            "e2e_1368_layout",
            "class A:\n    __slots__ = ('x',)\n\n\nclass B:\n    __slots__ = ('y',)\n\n\nclass \
             D(A, B):\n    pass\n",
            "TypeError: multiple bases have instance lay-out conflict",
        ),
        (
            "e2e_1368_exception_layout",
            "class A:\n    __slots__ = ('x',)\n\n\nclass D(Exception, A):\n    pass\n",
            "TypeError: multiple bases have instance lay-out conflict",
        ),
    ] {
        let text = fails(category, source);
        assert!(text.contains("error[C0001]"), "{category}: {text}");
        assert!(text.contains(cpython_error), "{category}: {text}");
        cpython_rejects(&format!("{category}_cpython"), source, cpython_error);
    }
}

#[test]
fn each_unmodelled_spelling_is_c0001() {
    for (category, source, message) in [
        (
            "e2e_1368_dict_entry",
            "class C:\n    __slots__ = ('a', '__dict__')\n",
            "a `__dict__` entry in `__slots__` is not supported yet",
        ),
        (
            "e2e_1368_non_literal",
            "names = ('a',)\n\n\nclass C:\n    __slots__ = names\n",
            "a `__slots__` value that is not a string literal or a tuple or list of string \
             literals is not supported yet",
        ),
        (
            "e2e_1368_second",
            "class C:\n    __slots__ = ('a',)\n    __slots__ = ('b',)\n",
            "a second `__slots__` binding in one class body is not supported yet",
        ),
        (
            "e2e_1368_value_less",
            "class C:\n    __slots__: tuple\n",
            "this `__slots__` spelling is not supported yet",
        ),
    ] {
        let text = fails(category, source);
        assert!(text.contains("error[C0001]"), "{category}: {text}");
        assert!(text.contains(message), "{category}: {text}");
    }
}

/// A declared slot that is never assigned is accepted, and a read of it is
/// refused at compile time; CPython raises `AttributeError` only when the
/// read runs.
#[test]
fn a_read_of_a_never_assigned_slot_is_t0044() {
    let source = "class C:\n    __slots__ = ('a', 'b')\n\n    def __init__(self) -> None:\n        \
                  self.a = 1\n\n    def get(self) -> int:\n        return self.b\n\n\n\
                  print(C().get())\n";
    let text = fails("e2e_1368_unassigned_read", source);
    assert!(text.contains("error[T0044]"), "{text}");
    assert!(
        text.contains("class `C` has no attribute named `b`"),
        "{text}"
    );
    // CPython's text for an unset slot read differs by version: 3.14 says
    // `'C' object has no attribute 'b'`, while 3.9 (the macOS coverage job's
    // reference interpreter) says only `AttributeError: b`. Both end the
    // traceback with an `AttributeError` naming `b`, which is what is pinned.
    let dir = ScratchDir::new("e2e_1368_unassigned_read_cpython").expect("scratch");
    std::fs::write(dir.join("a.py"), source).expect("write the subject");
    let output = python()
        .arg("a.py")
        .current_dir(dir.join("."))
        .output()
        .expect("python3 should spawn");
    assert!(!output.status.success(), "CPython accepted {source:?}");
    let error = rendered(&output);
    assert!(
        error.contains("AttributeError: 'C' object has no attribute 'b'")
            || error.contains("AttributeError: b\n"),
        "{error}"
    );
}

/// A dunder-named slot is refused: CPython gives many `__x__` names a
/// special meaning, and a `__hash__` slot makes the class unhashable.
#[test]
fn a_dunder_slot_is_c0001() {
    let source = "class C:\n    __slots__ = ('a', '__hash__')\n\n    def __init__(self) -> None:\n        \
                  self.a = 1\n\n\nc = C()\nprint(hash(c) == hash(c))\n";
    let text = fails("e2e_1368_dunder", source);
    assert!(text.contains("error[C0001]"), "{text}");
    assert!(
        text.contains("a `__slots__` entry named `__hash__` is not supported yet"),
        "{text}"
    );
    cpython_rejects(
        "e2e_1368_dunder_cpython",
        source,
        "TypeError: unhashable type: 'C'",
    );
}

/// An own slot shadowing an inherited class variable: CPython accepts the
/// class, but the unset slot's member descriptor hides `B.a`, so the read
/// raises `AttributeError` (3.9 words it `AttributeError: a`, 3.13+ with the
/// object's type) where pycc would have found `B.a`.
#[test]
fn a_slot_shadowing_an_inherited_class_variable_is_c0001() {
    let source = "class B:\n    a = 1\n\n\nclass C(B):\n    __slots__ = ('a',)\n\n\nprint(C().a)\n";
    let text = fails("e2e_1368_inherited", source);
    assert!(text.contains("error[C0001]"), "{text}");
    assert!(
        text.contains("the `__slots__` entry `a` of class `C` is not supported yet"),
        "{text}"
    );
    cpython_rejects("e2e_1368_inherited_cpython", source, "AttributeError");
}
