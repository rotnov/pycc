//! End-to-end proof for unannotated empty `list` instance-attribute
//! initialisers ([#1265], Part 4 of [#1218]).
//!
//! `docs/TYPE_SYSTEM.md`'s class "Current state" paragraph and D-245's
//! 2026-09-24 amendment for #1265 are the contract. The byte-exact oracle
//! fixture is `tests/fixtures/instance_unannotated_list_slots.py`
//! (registered in `tests/conformance/classes.rs`, pinned-oracle only); this
//! file runs the same fixture against its recorded CPython 3.14.7 output on
//! every CI job, adds programs checked against whatever `python3` is
//! available (every one is Python 3.6 syntax), and owns the refusals.
//!
//! [#1265]: https://github.com/rotnov/pycc/issues/1265
//! [#1218]: https://github.com/rotnov/pycc/issues/1218

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

/// Builds `source` into `dir/app`, runs it, and returns its standard output.
fn build_and_run(dir: &Path, source: &Path) -> String {
    let build = pycc()
        .arg("build")
        .arg(source)
        .arg("-o")
        .arg(dir.join("app"))
        .output()
        .expect("pycc should spawn");
    assert!(build.status.success(), "{}", rendered(&build));
    let run = Command::new(dir.join("app"))
        .output()
        .expect("the program should spawn");
    assert!(run.status.success(), "{}", rendered(&run));
    stdout(&run)
}

/// Builds and runs `entry` in `dir`, runs the same file under `python3`, and
/// asserts the two agree; returns pycc's output.
fn matches_cpython(dir: &Path, entry: &str) -> String {
    let pycc_out = build_and_run(dir, &dir.join(entry));
    let oracle = python()
        .arg(entry)
        .current_dir(dir)
        .output()
        .expect("python3 should spawn");
    assert!(oracle.status.success(), "{}", rendered(&oracle));
    assert_eq!(pycc_out, stdout(&oracle));
    pycc_out
}

/// Runs `pycc <subcommand>` on `a.py` holding `source`, asserting failure,
/// and returns the rendered diagnostics.
fn fails(category: &str, subcommand: &str, source: &str) -> String {
    let dir = ScratchDir::new(category).expect("scratch");
    std::fs::write(dir.join("a.py"), source).expect("write the subject");
    let mut command = pycc();
    command.arg(subcommand).arg(dir.join("a.py"));
    if subcommand == "build" {
        command.arg("-o").arg(dir.join("app"));
    }
    let output = command.output().expect("pycc should spawn");
    assert!(!output.status.success(), "{source:?} was accepted");
    rendered(&output)
}

const NO_PRODUCER: &str = "class Buffer:\n    def __init__(self) -> None:\n        \
                           self.xs = []\n\n    def size(self) -> int:\n        \
                           return len(self.xs)\n\n\nprint(Buffer().size())\n";

/// The oracle fixture, against CPython 3.14.7's recorded output: two
/// instances with independent containers, producers in another method and
/// in `__init__`, `.pop()`, a reset, a subclass typed by its inherited
/// slots, and a `this` receiver.
#[test]
fn the_fixture_matches_its_recorded_cpython_output() {
    let dir = ScratchDir::new("e2e_1265_fixture").expect("scratch");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/instance_unannotated_list_slots.py");
    assert_eq!(
        build_and_run(&dir, &fixture),
        "2 0 1 100\n4 4 1\n0\n11 0\n25 2\n"
    );
}

/// Module-scope instances and an alias sum, compared with CPython.
#[test]
fn module_scope_instances_match_cpython() {
    let dir = ScratchDir::new("e2e_1265_module").expect("scratch");
    std::fs::write(
        dir.join("a.py"),
        "class Bag:\n    def __init__(self) -> None:\n        self.xs = []\n\n    \
         def add(self, v: int) -> None:\n        self.xs.append(v)\n\n\n\
         b = Bag()\nc = Bag()\nb.add(4)\nys = c.xs\nys.append(5)\nys.append(6)\n\
         t = 0\nfor y in ys:\n    t = t + y\nprint(len(b.xs), len(c.xs), t)\n",
    )
    .expect("write the subject");
    assert_eq!(matches_cpython(&dir, "a.py"), "1 2 11\n");
}

/// A module that also routes through the private-helper solver (an
/// unannotated private helper) still sees the resolved slot.
#[test]
fn the_solver_path_sees_the_resolved_slot() {
    let dir = ScratchDir::new("e2e_1265_solver").expect("scratch");
    std::fs::write(
        dir.join("a.py"),
        "class Bag:\n    def __init__(self) -> None:\n        self.xs = []\n\n    \
         def add(self, v: int) -> None:\n        self.xs.append(v)\n\n\n\
         def _twice(n):\n    return n * 2\n\n\n\
         def main() -> None:\n    b = Bag()\n    b.add(_twice(3))\n    \
         print(b.xs[0], len(b.xs))\n\n\nmain()\n",
    )
    .expect("write the subject");
    assert_eq!(matches_cpython(&dir, "a.py"), "6 1\n");
}

/// A subclass in another module is typed by its base's resolved slot.
#[test]
fn a_subclass_in_another_module_is_typed_by_the_base_slot() {
    let dir = ScratchDir::new("e2e_1265_two_modules").expect("scratch");
    std::fs::write(
        dir.join("store.py"),
        "class Log:\n    def __init__(self) -> None:\n        self.items = []\n\n    \
         def add(self, v: int) -> None:\n        self.items.append(v)\n",
    )
    .expect("write the base module");
    std::fs::write(
        dir.join("main.py"),
        "from store import Log\n\n\nclass Big(Log):\n    def __init__(self) -> None:\n        \
         self.items = []\n\n\ndef main() -> None:\n    b = Big()\n    b.add(7)\n    \
         print(len(b.items), b.items[0])\n\n\nmain()\n",
    )
    .expect("write the entry module");
    assert_eq!(matches_cpython(&dir, "main.py"), "1 7\n");
}

/// An unresolved slot in an imported module is reported against that
/// module's file.
#[test]
fn an_unresolved_slot_in_another_module_points_at_that_module() {
    let dir = ScratchDir::new("e2e_1265_two_modules_refused").expect("scratch");
    std::fs::write(
        dir.join("store.py"),
        "class Bare:\n    def __init__(self) -> None:\n        self.items = []\n",
    )
    .expect("write the base module");
    std::fs::write(
        dir.join("main.py"),
        "from store import Bare\n\nprint(Bare())\n",
    )
    .expect("write the entry module");
    let output = pycc()
        .arg("check")
        .arg(dir.join("main.py"))
        .output()
        .expect("pycc should spawn");
    assert!(!output.status.success());
    let text = rendered(&output);
    assert!(text.contains("error[T0003]"), "{text}");
    assert!(text.contains("store.py:1:1"), "{text}");
    assert!(text.contains("`self.items` in class `Bare`"), "{text}");
}

/// `pycc check` and `pycc build` refuse a slot no source types identically.
#[test]
fn check_and_build_report_the_same_t0003() {
    let check = fails("e2e_1265_check", "check", NO_PRODUCER);
    let build = fails("e2e_1265_build", "build", NO_PRODUCER);
    let message = "error[T0003]: an empty list literal has no inferable element type for \
                   `self.xs` in class `Buffer`";
    assert!(check.contains(message), "{check}");
    assert!(build.contains(message), "{build}");
}

/// A base's unresolved slot redeclared by an annotated subclass is the
/// `T0003`, not D-210's `T0052` naming a placeholder type.
#[test]
fn an_unresolved_base_slot_is_t0003_rather_than_t0052() {
    let text = fails(
        "e2e_1265_t0052",
        "check",
        "class A:\n    def __init__(self) -> None:\n        self.xs = []\n\n\n\
         class B(A):\n    def __init__(self) -> None:\n        self.xs: list[int] = []\n\n\n\
         print(len(B().xs))\n",
    );
    assert!(
        text.contains("error[T0003]") && text.contains("`self.xs` in class `A`"),
        "{text}"
    );
    assert!(!text.contains("T0052"), "{text}");
}

/// A static method whose first parameter is named `self` is not one of the
/// instance's own methods, so its `append` types nothing.
#[test]
fn a_static_method_named_self_parameter_is_not_a_producer() {
    let text = fails(
        "e2e_1265_static",
        "check",
        "class Box:\n    def __init__(self) -> None:\n        self.xs = []\n\n    \
         @staticmethod\n    def fill(self: Box, v: int) -> None:\n        \
         self.xs.append(v)\n\n\nprint(len(Box().xs))\n",
    );
    assert!(
        text.contains("error[T0003]") && text.contains("`self.xs` in class `Box`"),
        "{text}"
    );
}

/// The unannotated `{}` stays `C0001`, naming the annotated spelling.
#[test]
fn an_unannotated_empty_dict_stays_refused() {
    let text = fails(
        "e2e_1265_dict",
        "check",
        "class Index:\n    def __init__(self) -> None:\n        self.d = {}\n\n\nprint(Index())\n",
    );
    assert!(text.contains("error[C0001]"), "{text}");
    assert!(text.contains("`self.d: dict[str, int] = {}`"), "{text}");
    assert!(text.contains("#891"), "{text}");
}
