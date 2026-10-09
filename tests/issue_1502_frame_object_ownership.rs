//! Part 2 of #1499 (#1502): a function-frame `object` slot -- a parameter
//! or a local -- owns its reference, so a rebind releases the old value, the
//! scope exit releases every slot, an argument is an owned reference the
//! callee's parameter takes over, and a return value is a new reference the
//! caller owns (`crates/pycc_codegen/src/object_frame.rs`).
//!
//! **The oracle is CPython itself.** The module below is also run as plain
//! Python source, and the probe expects the compiled extension to report
//! what CPython reports, line for line: for each shape, called fifty times,
//! the change in the number of live stub objects (each counts itself in
//! `__init__` and `__del__`) and in `sys.getrefcount` of the mortal stub
//! object `T` (PEP 683, see `tests/issue_1084_refcount_probe.rs`). A leak
//! shows as a positive delta, an over-release as a negative one -- or as a
//! crash, which the debug-allocator run turns into a hard failure.
//!
//! An embedded executable compiles its entry module for the CPython host
//! too, so the last test builds a frame-ownership program as one and
//! compares it with CPython 3.14.7's run of the same source.
//!
//! The hosted tests are `#[ignore]`d and contribute no line coverage; the
//! Tier-1 `native-build-test` leg runs them with `cargo test --workspace --
//! --include-ignored`. The changed codegen lines are covered by the unit
//! tests in `crates/pycc_codegen/src/object_frame_tests.rs`.

use pycc_scratch::ScratchDir;
use std::path::Path;
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

/// The foreign stub: every `Thing` counts itself in `LIVE`.
const STUB: &str = "\
LIVE = 0


class Thing:
    def __init__(self):
        global LIVE
        LIVE += 1

    def __del__(self):
        global LIVE
        if LIVE is not None:
            LIVE -= 1

    def fail(self):
        raise ValueError()


def fresh():
    return Thing()


def boom():
    raise ValueError()


T = Thing()
";

/// The module under test: one function per frame shape.
const MODULE: &str = "\
import pycc_t1502_stub as s


class Box:
    def __init__(self, a: object) -> None:
        self.a = a


class Num:
    def __init__(self, n: int) -> None:
        self.n = n

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, int):
            return NotImplemented
        return True


def rebind() -> None:
    x = s.fresh()
    x = s.fresh()
    x = s.fresh()


def take(o: object) -> None:
    pass


def pair(a: object, b: object) -> None:
    pass


def keep_param(o: object) -> None:
    y = o
    y = s.fresh()
    o = y


def ident(o: object) -> object:
    return o


def get_global() -> object:
    return G


def get_none() -> object:
    return None


def get_fresh() -> object:
    return s.fresh()


def discard() -> None:
    get_fresh()


def keep_call() -> None:
    x = get_fresh()


def pass_borrowed(o: object) -> None:
    take(o)


def pass_produced() -> None:
    take(s.fresh())


def pass_call() -> None:
    take(get_fresh())


def pass_boxed() -> None:
    take(None)
    take(3)


def pass_raising(o: object) -> None:
    pair(o, s.boom())


def pass_raising_produced() -> None:
    pair(s.fresh(), s.boom())


def raising() -> None:
    x = s.fresh()
    s.T.fail()


def mixed(a: object, n: int) -> int:
    return n


G: object = s.fresh()
";

/// The probe: each `run` calls one shape fifty times and reports the change
/// in live stub objects and in `T`'s reference count.
const PROBE: &str = "\
import gc, importlib, sys
sys.path.insert(0, '.')
sys.path.insert(0, 'pure')
import pycc_t1502_stub as s
m = importlib.import_module(sys.argv[1])
N = 50
def rc():
    return sys.getrefcount(s.T)
def report(*fields):
    print(*fields, file=sys.stderr, flush=True)
def run(label, fn, *args, catch=None):
    live, t = s.LIVE, rc()
    for _ in range(N):
        try:
            fn(*args)
        except Exception as e:
            if catch is None or not isinstance(e, catch):
                raise
    gc.collect()
    report(label, s.LIVE - live, rc() - t)
run('rebind', m.rebind)
run('keep_param', m.keep_param, s.T)
run('ident', m.ident, s.T)
run('get_global', m.get_global)
run('get_none', m.get_none)
run('get_fresh', m.get_fresh)
run('discard', m.discard)
run('keep_call', m.keep_call)
run('pass_borrowed', m.pass_borrowed, s.T)
run('pass_produced', m.pass_produced)
run('pass_call', m.pass_call)
run('pass_boxed', m.pass_boxed)
run('pass_raising', m.pass_raising, s.T, catch=ValueError)
run('pass_raising_produced', m.pass_raising_produced, catch=ValueError)
run('raising', m.raising, catch=ValueError)
run('unpack_bail', m.mixed, s.T, 'x', catch=TypeError)
run('mixed', m.mixed, s.T, 1)
run('not_implemented', lambda: m.Num(1) == s.T)
t = rc()
b = m.Box(s.T)
report('box', rc() - t)
run('getter', lambda: b.a)
kept = [m.ident(s.T) for _ in range(N)]
report('held', rc() - t - 1)
g = sys.getrefcount(m.get_global())
report('global', sys.getrefcount(m.get_global()) - g)
";

/// What CPython reports: every shape is balanced; `Box(T)` keeps one
/// reference in its field, and fifty returned references kept in a list
/// hold fifty.
const EXPECTED: &str = "\
rebind 0 0
keep_param 0 0
ident 0 0
get_global 0 0
get_none 0 0
get_fresh 0 0
discard 0 0
keep_call 0 0
pass_borrowed 0 0
pass_produced 0 0
pass_call 0 0
pass_boxed 0 0
pass_raising 0 0
pass_raising_produced 0 0
raising 0 0
unpack_bail 0 0
mixed 0 0
not_implemented 0 0
box 1
getter 0 0
held 50
global 0
";

/// Writes the stub into the scratch root, `MODULE` as `src/<module>.py`
/// built into the scratch root, and the same source as plain Python
/// `pure/<module>_py.py`, the CPython oracle.
fn build(tag: &str, module: &str) -> ScratchDir {
    let dir = ScratchDir::new(tag).expect("scratch");
    std::fs::write(dir.join("pycc_t1502_stub.py"), STUB).expect("write the stub");
    std::fs::create_dir_all(dir.join("src")).expect("create the entry directory");
    std::fs::create_dir_all(dir.join("pure")).expect("create the oracle directory");
    let entry = dir.join("src").join(format!("{module}.py"));
    std::fs::write(&entry, MODULE).expect("write the module");
    std::fs::write(dir.join("pure").join(format!("{module}_py.py")), MODULE)
        .expect("write the oracle");
    let build = pycc()
        .arg("build")
        .arg(&entry)
        .arg("-o")
        .arg(dir.join(module))
        .arg("--ext")
        .output()
        .expect("pycc should spawn");
    assert!(build.status.success(), "{}", stderr_of(&build));
    dir
}

/// Runs `PROBE` with `module` as `sys.argv[1]` under the host interpreter,
/// with `flags` before `-c` and `env` added, and returns what it reported on
/// stderr.
fn python(dir: &Path, flags: &[&str], env: &[(&str, &str)], module: &str) -> String {
    let run = Command::new(std::env::var_os("PYCC_PYTHON").unwrap_or_else(|| "python3".into()))
        .args(flags)
        .arg("-c")
        .arg(PROBE)
        .arg(module)
        .envs(env.iter().copied())
        .current_dir(dir)
        .output()
        .expect("python3 should spawn");
    assert!(
        run.status.success(),
        "stdout: {}\nstderr: {}",
        stdout_of(&run),
        stderr_of(&run)
    );
    assert_eq!(stdout_of(&run), "");
    stderr_of(&run)
}

/// Every frame shape -- a local rebind, a rebound parameter, a returned
/// parameter, global, `None` and produced value, a discarded or bound call
/// result, a borrowed, produced, boxed or call-result argument, a borrowed
/// or produced argument abandoned by a raising later argument, a raising
/// body, the wrapper's unpack bail path, `return NotImplemented`, and the
/// field getter --
/// leaves the live objects and `T`'s reference count exactly as CPython
/// does.
#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn frame_object_slots_balance_their_references_as_cpython_does() {
    let dir = build("t1502_frame", "pycc_t1502_m");
    let compiled = python(&dir, &[], &[], "pycc_t1502_m");
    let oracle = python(&dir, &[], &[], "pycc_t1502_m_py");
    assert_eq!(compiled, oracle, "against CPython");
    assert_eq!(compiled, EXPECTED);
}

/// The same probe under CPython's development mode and debug allocator,
/// which detect a release of freed memory and a refcount underflow.
#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn the_frame_probe_is_clean_under_the_debug_allocator() {
    let dir = build("t1502_dev", "pycc_t1502_dev");
    let got = python(
        &dir,
        &["-X", "dev"],
        &[("PYTHONMALLOC", "debug")],
        "pycc_t1502_dev",
    );
    assert_eq!(got, EXPECTED);
}

/// An embedded executable (Part 1 of #1028) compiles its entry module for
/// the CPython host as an `--ext` artifact does, so its frames own their
/// `object` slots too. Each `functools.partial` holds a reference to `T`: a
/// leaked local -- a rebound one, or one a raising body abandons -- would
/// raise `T`'s reference count by one per call.
#[cfg(not(windows))]
const EMBEDDED: &str = "\
import sys
from fractions import Fraction
from functools import partial

T = Fraction(1, 3)


def rebind() -> None:
    x = partial(sys.getrefcount, T)
    x = partial(sys.getrefcount, T)
    y = x
    y = T


def raising() -> None:
    x = partial(sys.getrefcount, T)
    Fraction(1, 0)


before = sys.getrefcount(T)
for i in range(50):
    rebind()
    try:
        raising()
    except ZeroDivisionError:
        pass
print(sys.getrefcount(T) == before)
";

/// The embedded executable's frames balance `T`'s reference count as
/// CPython's run of the same source does. (Its launcher's isolated
/// configuration ignores `PYTHONMALLOC`, so there is no debug-allocator
/// run here; the `--ext` probe above has one.)
#[cfg(not(windows))]
#[test]
#[ignore = "needs CPython 3.14.7 as python3.14 or PYCC_PYTHON; run with --include-ignored"]
fn an_embedded_executable_frame_owns_its_object_slots_as_cpython_does() {
    let dir = ScratchDir::new("t1502_embedded").expect("scratch");
    let src = dir.join("m.py");
    std::fs::write(&src, EMBEDDED).expect("write the program");
    let build = pycc()
        .arg("build")
        .arg(&src)
        .arg("-o")
        .arg(dir.join("app"))
        .output()
        .expect("pycc should spawn");
    assert!(build.status.success(), "{}", stderr_of(&build));
    assert!(dir.join("app.pycc").is_dir(), "an embedded build");
    let oracle =
        Command::new(std::env::var_os("PYCC_PYTHON").unwrap_or_else(|| "python3.14".into()))
            .arg(&src)
            .output()
            .expect("CPython runs the program");
    assert_eq!(stdout_of(&oracle), "True\n", "{}", stderr_of(&oracle));
    let run = Command::new(dir.join("app"))
        .output()
        .expect("the embedded binary runs");
    assert_eq!(run.status.code(), Some(0), "{}", stderr_of(&run));
    assert_eq!(stdout_of(&run), stdout_of(&oracle));
}
