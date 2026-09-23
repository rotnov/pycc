//! End-to-end proof for comprehensions in any expression position ([#1254],
//! Part 1 of [#1214]; D-250).
//!
//! `docs/TYPE_SYSTEM.md`'s comprehension rule is the contract. The
//! byte-exact oracle fixture is `tests/fixtures/comprehension_expr.py`
//! (registered in `tests/conformance/classes.rs`); this file owns the
//! uncaught-exception exits, which CPython reports with a traceback pycc
//! does not print, and a comprehension re-run from a loop in a function.
//!
//! [#1254]: https://github.com/rotnov/pycc/issues/1254
//! [#1214]: https://github.com/rotnov/pycc/issues/1214

use pycc_scratch::ScratchDir;
use std::process::{Command, Output};

fn pycc() -> Command {
    Command::new(env!("CARGO_BIN_EXE_pycc"))
}

fn rendered(output: &Output) -> String {
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    text.replace("\r\n", "\n")
}

/// Builds `source` as a standalone executable and runs it.
fn build_and_run(category: &str, source: &str) -> Output {
    let dir = ScratchDir::new(category).expect("scratch");
    let path = dir.join("subject.py");
    std::fs::write(&path, source).expect("write the subject");
    let build = pycc()
        .arg("build")
        .arg(&path)
        .arg("-o")
        .arg(dir.join("subject"))
        .output()
        .expect("pycc should spawn");
    assert!(build.status.success(), "{source}: {}", rendered(&build));
    Command::new(dir.join("subject"))
        .output()
        .expect("the built program should spawn")
}

/// Asserts the program printed exactly `stdout`, then stopped with exit
/// status 1 and `exception` on stderr, as CPython does.
fn assert_uncaught(run: &Output, stdout: &str, exception: &str) {
    assert_eq!(run.status.code(), Some(1), "{}", rendered(run));
    assert_eq!(
        String::from_utf8_lossy(&run.stdout).replace("\r\n", "\n"),
        stdout
    );
    assert!(
        String::from_utf8_lossy(&run.stderr).contains(exception),
        "{}",
        rendered(run)
    );
}

/// A `range` step of `0` in a comprehension argument raises `ValueError`
/// before the enclosing call runs, and nothing after it prints.
#[test]
fn a_zero_range_step_in_a_comprehension_argument_exits_with_value_error() {
    let run = build_and_run(
        "e2e_1254_step_zero",
        "print(\"before\")\nprint(len([x for x in range(0, 3, 0)]))\nprint(\"after\")\n",
    );
    assert_uncaught(
        &run,
        "before\n",
        "ValueError: range() arg 3 must not be zero",
    );
}

/// A raising element stops the loop and the program at that element.
#[test]
fn a_raising_element_in_a_comprehension_argument_exits_with_zero_division_error() {
    let run = build_and_run(
        "e2e_1254_elt_raises",
        "print(\"before\")\nprint(len([1 // (x - 2) for x in range(3)]))\nprint(\"after\")\n",
    );
    assert_uncaught(&run, "before\n", "ZeroDivisionError");
}

/// The same comprehension, run many times from one call and from many
/// calls, rebuilds its container each time and never grows the stack: the
/// loop-variable slot is hoisted to the entry block.
#[test]
fn a_comprehension_rerun_in_a_loop_rebuilds_its_container_each_time() {
    let run = build_and_run(
        "e2e_1254_rerun",
        "def count(n: int) -> int:\n    t = 0\n    for r in range(n):\n        \
         t = t + len([i for i in range(r) if i % 2 == 0])\n    return t\n\n\
         print(count(2000))\nprint(count(3))\n",
    );
    assert!(run.status.success(), "{}", rendered(&run));
    assert_eq!(
        String::from_utf8_lossy(&run.stdout).replace("\r\n", "\n"),
        "1000000\n2\n"
    );
}
