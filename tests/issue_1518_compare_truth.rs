//! #1518 (Part 4 of #1514), item B: a rich comparison with a CPython
//! object operand whose only use is a branch -- an `if` or `while` test, an
//! `assert`, a `not` operand, a truth-only `and`/`or` operand or a
//! comprehension filter -- asks the shim `pycc_ext_obj_richcompare_truth`
//! for its truth instead of building the result object and testing it
//! (`docs/RUNTIME.md`'s "A comparison that only feeds a branch"). The shim
//! compares two exact `int`s that fit a C `long` directly and otherwise
//! calls `PyObject_RichCompare` and tests the result, so the test compares
//! every shape with CPython: all six operators, `int`s at and beyond the C
//! `long` range, `bool` and `int` subclasses that override comparisons,
//! `int` against `float`, NaN, an `__eq__` that returns a non-`bool` truthy
//! or falsy value, one that raises, one whose result's `__bool__` raises, an
//! unsupported pair, and a native `int` operand. It also covers the test of
//! a conditional expression, and pins that an owned (packed) operand is
//! released before the result's `__bool__` runs, by having that `__bool__`
//! report `sys.getrefcount` of the operand.
//!
//! The test is hosted, so `#[ignore]`d and contributing no line coverage;
//! the Tier-1 `native-build-test` leg runs it with `cargo test --workspace --
//! --include-ignored`. The changed codegen lines are covered by
//! `crates/pycc_codegen/src/tests/object_compare.rs`.

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

fn artifact_name() -> &'static str {
    if cfg!(windows) { "m.pyd" } else { "m.abi3.so" }
}

fn run_in(dir: &Path, script: &str) -> Output {
    host_python()
        .args(["-B", "-c", script])
        .current_dir(dir)
        .env("PYTHONUNBUFFERED", "1")
        .output()
        .expect("python3 should spawn")
}

const MODULE: &str = r#"from typing import Any


def ops(a: Any, b: Any) -> str:
    out = ""
    if a < b:
        out += "L"
    else:
        out += "l"
    if a <= b:
        out += "E"
    else:
        out += "e"
    if a == b:
        out += "Q"
    else:
        out += "q"
    if a != b:
        out += "N"
    else:
        out += "n"
    if a > b:
        out += "G"
    else:
        out += "g"
    if a >= b:
        out += "H"
    else:
        out += "h"
    return out


def with_int(a: Any, n: int) -> str:
    out = ""
    if a < n + 1:
        out += "L"
    if n == a:
        out += "Q"
    if not (a != n):
        out += "q"
    return out


def countdown(a: Any) -> int:
    k = 0
    while a > k:
        k += 1
    return k


def checked(a: Any, b: Any) -> int:
    assert a == b
    return 1


def negated(a: Any, b: Any) -> bool:
    return not (a == b)


def both(a: Any, b: Any) -> int:
    if a == b and b <= a:
        return 1
    if a < b or b < a:
        return 2
    return 3


def keep(xs: Any, b: Any) -> int:
    return len([x for x in xs[:] if x == b])


def pick(a: Any, b: Any) -> str:
    return "y" if a == b else "n"


def held(a: Any, n: int) -> str:
    out = "y" if a == n + 1 else "n"
    if a == n + 2:
        out += "Y"
    return out
"#;

const SCRIPT: &str = r#"import m
class Weird:
    def __init__(self, r): self.r = r
    def __eq__(self, o): return self.r
    __ne__ = __lt__ = __le__ = __gt__ = __ge__ = __eq__
    __hash__ = None
class Boom:
    def __eq__(self, o): raise RuntimeError('eq boom')
    __lt__ = __eq__
    __hash__ = None
class BadBool:
    def __bool__(self): raise TypeError('bad bool')
class MyInt(int):
    def __lt__(self, o): return True
    def __eq__(self, o): return False
    __hash__ = int.__hash__
nan = float('nan')
cases = [(1, 2), (2, 2), (3, 2), (-1, -2), (0, 0), (nan, nan), (nan, 1.0), (1, 1.0), (True, 1), (False, 0), (True, True),
         (2**70, 2**70 + 1), (-2**70, 2**70), (2**63 - 1, 2**63 - 1), (-2**63, -2**63), (2**63, 2**63 - 1),
         (MyInt(5), 3), (3, MyInt(5)), (MyInt(5), MyInt(5)), (Weird([]), 1), (Weird([0]), 1), (Weird(0), 1),
         (1, Weird('x')), ('a', 'b'), ((1, 2), (1, 3)), (None, None)]
for a, b in cases:
    try:
        print(m.ops(a, b), m.negated(a, b), m.both(a, b))
    except Exception as e:
        print(type(e).__name__, e)
for a, b in [(Boom(), 1), (1, Boom()), (Weird(BadBool()), 1), (1, Weird(BadBool())), ('a', 1), (None, 0)]:
    for f in (m.ops, m.negated, m.both, m.checked):
        try:
            print(f(a, b))
        except Exception as e:
            print(type(e).__name__, e)
for a in [5, 6, 7, 2.5, nan, 2**70, -2**70, True, MyInt(9), Weird([1]), Weird(())]:
    print(m.with_int(a, 5), end=' ')
print()
print([m.countdown(x) for x in (0, 3, 2.5, True, MyInt(4))])
try:
    m.countdown(Weird(BadBool()))
except TypeError as e:
    print('countdown', e)
print(m.checked(2, 2), m.checked(nan if False else 1.0, 1))
for a, b in [(1, 2), (nan, nan), (Weird([]), 0)]:
    try:
        m.checked(a, b)
    except AssertionError as e:
        print('assert', type(e).__name__)
print(m.keep([1, 2, 1, 1.0, True, nan, MyInt(1), 2**70], 1), m.keep([nan, nan], nan), m.keep([Weird([0]), Weird(0)], 1))
try:
    m.keep([1, Boom()], 1)
except RuntimeError as e:
    print('keep', e)
print(''.join(m.pick(a, b) for a, b in cases))
for a, b in [(Boom(), 1), (Weird(BadBool()), 1)]:
    try:
        m.pick(a, b)
    except Exception as e:
        print('pick', type(e).__name__, e)
import sys
class Peek:
    def __init__(self, o): self.o = o
    def __bool__(self):
        print('refs', sys.getrefcount(self.o), end=' ')
        return True
class Probe:
    def __eq__(self, o): return Peek(o)
    __hash__ = None
print(m.held(Probe(), 10**6))
"#;

#[test]
#[ignore = "hosted: builds an --ext artifact and runs it under CPython"]
fn a_branch_only_comparison_matches_cpython_for_every_operand_shape() {
    let compiled_dir = ScratchDir::new("issue_1518_truth").expect("scratch");
    let source_dir = ScratchDir::new("issue_1518_truth_src").expect("scratch");
    let source = write(&source_dir, "m.py", MODULE);
    let build = pycc()
        .arg("build")
        .arg(&source)
        .arg("-o")
        .arg(compiled_dir.join("m"))
        .arg("--ext")
        .output()
        .expect("pycc should spawn");
    assert!(
        build.status.success(),
        "{}{}",
        stdout_of(&build),
        stderr_of(&build)
    );
    assert!(compiled_dir.join(artifact_name()).is_file());

    let compiled = run_in(&compiled_dir, SCRIPT);
    let oracle = run_in(&source_dir, SCRIPT);
    for (what, run) in [("pycc", &compiled), ("cpython", &oracle)] {
        assert!(
            run.status.success(),
            "{what}: {}{}",
            stdout_of(run),
            stderr_of(run)
        );
    }
    assert_eq!(stdout_of(&compiled), stdout_of(&oracle));
    let out = stdout_of(&compiled);
    // NaN against itself, an `__eq__` returning `[]` and one returning
    // `[0]`, and a result whose `__bool__` raises.
    for line in [
        "leqNgh True 3\n",
        "leqngh True 3\n",
        "LEQNGH False 1\n",
        "TypeError bad bool\n",
    ] {
        assert!(out.contains(line), "{line:?}: {out}");
    }
    assert!(out.contains("4 0 1\nkeep eq boom\n"), "{out}");
    assert!(out.contains("pick TypeError bad bool\n"), "{out}");
    // The packed `int` temporary is released before the result's
    // `__bool__` runs, as CPython's `COMPARE_OP` releases its operands
    // before `TO_BOOL`: only `Peek.o` and the call argument remain.
    assert!(out.ends_with("refs 2 refs 2 yY\n"), "{out}");
}
