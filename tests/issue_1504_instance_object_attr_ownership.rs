//! Part 4 of #1499 (#1504): a compiled instance's `object` attribute owns
//! its reference. A compiled store retains a borrowed value and releases
//! the word it replaces after storing the new one (`Py_XSETREF` order), a
//! compiled read returns a new reference as CPython's `LOAD_ATTR` does, and
//! a host store or `del` through the slot's descriptor releases the
//! replaced or deleted word (`crates/pycc_codegen/src/object_attr.rs`,
//! `pycc_ext_instance_store_slot`/`pycc_ext_instance_delete_slot` in
//! `src/ext/pycc_ext_module.c`).
//!
//! **The oracle is CPython itself.** The module below is also run as plain
//! Python source, and the probe expects the compiled extension to report
//! what CPython reports, line for line: for each shape, called fifty times,
//! the change in the number of live stub objects (each counts itself in
//! `__init__` and `__del__`) and in `sys.getrefcount` of the mortal stub
//! object `T` (PEP 683, see `tests/issue_1084_refcount_probe.rs`). A leak
//! shows as a positive delta, an over-release as a negative one -- or as a
//! crash, which the debug-allocator run turns into a hard failure. The
//! `hazard` and `callback` shapes rebind the attribute -- from compiled code
//! and from a host callback -- while the method call on its old value is
//! still running, with that old value's only other owner the attribute
//! itself: a borrowed read would use it after it is freed.
//!
//! A compiled instance is never freed (D-107, D-154), so a shape that drops
//! the last reference to an instance keeps that instance's attributes alive
//! where CPython frees them. `the_instance_lifetime_residual_keeps_only_the_last_value`
//! pins that residual: one live value per dropped instance, the last one
//! stored -- every value it replaced is released.
//!
//! An embedded executable (Part 1 of #1028) compiles its entry module for
//! the CPython host as an `--ext` artifact does and links the same shim, so
//! an attribute annotated with a class a foreign import binds (Part 1 of
//! #1367) -- the one `object`-typed attribute an embedded build admits, a
//! bare `object` annotation being `C0001` there -- owns its reference too:
//! `an_embedded_executable_instance_attribute_owns_its_reference` pins it.
//! A native build has no `object` value at all: its type check refuses
//! every boxing seam with `I0406` (#1508).
//!
//! The hosted tests are `#[ignore]`d and contribute no line coverage; the
//! Tier-1 `native-build-test` leg runs them with `cargo test --workspace --
//! --include-ignored`. The changed codegen lines are covered by the unit
//! tests in `crates/pycc_codegen/src/object_attr_tests.rs`.

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

/// The foreign stub: every `Thing` counts itself in `LIVE`. `poke` rebinds
/// a box's attribute from the host and then uses `self`.
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

    def bit(self, x):
        return None

    def fail(self):
        raise ValueError()

    def poke(self, box):
        box.a = Thing()
        return self.bit(0)

    def __len__(self):
        return 2

    def __iter__(self):
        return iter((1, 2))

    def __getitem__(self, i):
        return i


def fresh():
    return Thing()


def boom():
    raise ValueError()


T = Thing()
";

/// The module under test: one function or method per attribute shape.
const MODULE: &str = "\
import pycc_t1504_stub as s


class Box:
    a: object
    b: object

    def __init__(self, a: object) -> None:
        self.a = a
        self.b = None

    def reset(self) -> object:
        self.a = s.fresh()
        return None

    def hazard(self) -> object:
        return self.a.bit(self.reset())

    def callback(self) -> object:
        return self.a.poke(self)

    def copy_self(self) -> None:
        self.b = self.a

    def same(self) -> None:
        self.a = self.a

    def store_raising(self) -> None:
        self.a = s.boom()

    def read_raising(self) -> None:
        self.a.fail()

    def in_finally(self) -> None:
        try:
            self.a = s.fresh()
            s.boom()
        finally:
            self.b = s.fresh()

    @property
    def p(self) -> object:
        return self.a

    @p.setter
    def p(self, v: object) -> None:
        self.a = v


def take(o: object) -> None:
    pass


def rebind(b: Box) -> None:
    b.a = s.fresh()
    b.a = s.fresh()


def store_param(b: Box, o: object) -> None:
    b.a = o


def store_global(b: Box) -> None:
    b.a = G


def store_none(b: Box) -> None:
    b.a = None


def read_bind(b: Box) -> None:
    x = b.a
    x = b.a


def read_return(b: Box) -> object:
    return b.a


def read_arg(b: Box) -> None:
    take(b.a)


def read_method(b: Box) -> None:
    b.a.bit(1)


def read_tests(b: Box) -> bool:
    t = b.a is None
    u = b.a == b.a
    if b.a:
        return t
    return not t


def read_format(b: Box) -> str:
    return f\"{b.a}\"


def read_discard(b: Box) -> None:
    b.a


def read_len(b: Box) -> int:
    return len(b.a)


def read_comp(b: Box) -> list[object]:
    return [x for x in b.a]

def read_item(b: Box) -> None:
    x = b.a[0]

def both_if(b: Box, c: bool) -> None:
    x = b.a if c else b.b


def mixed_if(b: Box, c: bool, o: object) -> None:
    x = b.a if c else o


def mixed_or(b: Box, o: object) -> None:
    x = b.a or o


def construct() -> None:
    b = Box(s.fresh())
    b.a = s.fresh()
    b.a = s.fresh()


def del_obj(o: object) -> None:
    o.a = s.fresh()
    del o.a


G: object = s.fresh()
";

/// The probe: each `run` calls one shape fifty times and reports the change
/// in live stub objects and in `T`'s reference count. With `residual` as
/// `sys.argv[2]` it runs only the instance-lifetime shapes instead.
const PROBE: &str = "\
import copy, gc, importlib, sys
sys.path.insert(0, '.')
sys.path.insert(0, 'pure')
import pycc_t1504_stub as s
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
b = m.Box(s.fresh())
if sys.argv[2:] == ['residual']:
    run('construct', m.construct)
    run('init', lambda: m.Box(s.T))
    def copied():
        c = copy.copy(b)
        c.a = s.fresh()
        c.a = s.fresh()
    run('copy_store', copied)
    sys.exit(0)
run('rebind', m.rebind, b)
run('store_param', m.store_param, b, s.T)
run('store_fresh', lambda: m.store_param(b, s.fresh()))
run('store_global', m.store_global, b)
run('store_none', m.store_none, b)
run('reset', b.reset)
b.a = s.T
b.b = s.T
run('read_bind', m.read_bind, b)
run('read_return', m.read_return, b)
run('read_arg', m.read_arg, b)
run('read_method', m.read_method, b)
run('read_tests', m.read_tests, b)
run('read_format', m.read_format, b)
run('read_discard', m.read_discard, b)
run('read_len', m.read_len, b)
run('read_comp', m.read_comp, b)
run('read_item', m.read_item, b)
run('both_if', m.both_if, b, True)
run('mixed_if', m.mixed_if, b, True, s.T)
run('mixed_if_else', m.mixed_if, b, False, s.T)
run('mixed_or', m.mixed_or, b, s.T)
run('copy_self', b.copy_self)
run('same', b.same)
b.a = s.fresh()
run('hazard', b.hazard)
run('callback', b.callback)
b.a = s.T
run('store_raising', b.store_raising, catch=ValueError)
run('read_raising', b.read_raising, catch=ValueError)
run('in_finally', b.in_finally, catch=ValueError)
run('prop_get', lambda: b.p)
run('prop_set', lambda: setattr(b, 'p', s.fresh()))
run('host_set', lambda: setattr(b, 'a', s.fresh()))
run('host_same', lambda: setattr(b, 'a', b.a))
run('host_set_T', lambda: setattr(b, 'a', s.T))
run('del_obj', m.del_obj, b)
run('host_del_missing', lambda: delattr(b, 'a'), catch=AttributeError)
run('read_missing', m.read_discard, b, catch=AttributeError)
run('read_missing_bind', m.read_bind, b, catch=AttributeError)
b.a = s.T
t = rc()
held = [m.read_return(b) for _ in range(N)]
report('held', rc() - t)
del held
b.a = s.fresh()
b.b = s.fresh()
live = s.LIVE
b.a = None
del b.b
report('released', live - s.LIVE)
";

/// What CPython reports. Every shape is balanced; a non-zero line is the
/// box keeping the last value stored where the shape before it left a
/// different one (`store_param` keeps `T` in place of a fresh object,
/// `store_fresh` the reverse, `del_obj` drops `T`). Fifty returned
/// references kept in a list hold fifty, and dropping the box's last two
/// values frees them.
const EXPECTED: &str = "\
rebind 0 0
store_param -1 1
store_fresh 1 -1
store_global -1 0
store_none 0 0
reset 1 0
read_bind 0 0
read_return 0 0
read_arg 0 0
read_method 0 0
read_tests 0 0
read_format 0 0
read_discard 0 0
read_len 0 0
read_comp 0 0
read_item 0 0
both_if 0 0
mixed_if 0 0
mixed_if_else 0 0
mixed_or 0 0
copy_self 0 0
same 0 0
hazard 0 0
callback 0 0
store_raising 0 0
read_raising 0 0
in_finally 2 -2
prop_get 0 0
prop_set 0 0
host_set 0 0
host_same 0 0
host_set_T -1 1
del_obj 0 -1
host_del_missing 0 0
read_missing 0 0
read_missing_bind 0 0
held 50
released 2
";

/// The instance-lifetime residual (D-107, D-154): each dropped instance
/// keeps exactly its last value -- one live object per `construct` and
/// `copy_store` instance, and one reference to `T` per `init` instance.
/// Every value a store replaced was released; before #1504 `construct`
/// read 150 and `copy_store` 100.
const RESIDUAL: &str = "\
construct 50 0
init 0 50
copy_store 50 0
";

/// Writes the stub into the scratch root, `MODULE` as `src/<module>.py`
/// built into the scratch root, and the same source as plain Python
/// `pure/<module>_py.py`, the CPython oracle.
fn build(tag: &str, module: &str) -> ScratchDir {
    let dir = ScratchDir::new(tag).expect("scratch");
    std::fs::write(dir.join("pycc_t1504_stub.py"), STUB).expect("write the stub");
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

/// Runs `PROBE` with `args` as `sys.argv[1:]` under the host interpreter,
/// with `flags` before `-c` and `env` added, and returns what it reported
/// on stderr.
fn python(dir: &Path, flags: &[&str], env: &[(&str, &str)], args: &[&str]) -> String {
    let run = Command::new(std::env::var_os("PYCC_PYTHON").unwrap_or_else(|| "python3".into()))
        .args(flags)
        .arg("-c")
        .arg(PROBE)
        .args(args)
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

/// Every attribute shape -- compiled stores of a produced value, a borrowed
/// parameter, a global and `None`, `self.b = self.a` and `self.a = self.a`,
/// every read consumer, a rebind while a call on the old value runs (from
/// compiled code and from a host callback), raising stores and reads, a
/// store before a raising `finally`, a property, host `setattr` and `del`
/// (including through an `object` value and of an unassigned attribute) --
/// leaves the live objects and `T`'s reference count exactly as CPython
/// does.
#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn instance_object_attributes_balance_their_references_as_cpython_does() {
    let dir = build("t1504_attr", "pycc_t1504_m");
    let compiled = python(&dir, &[], &[], &["pycc_t1504_m"]);
    let oracle = python(&dir, &[], &[], &["pycc_t1504_m_py"]);
    assert_eq!(compiled, oracle, "against CPython");
    assert_eq!(compiled, EXPECTED);
}

/// The same probe under CPython's development mode and debug allocator,
/// which detect a use of freed memory and a refcount underflow.
#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn the_attribute_probe_is_clean_under_the_debug_allocator() {
    let dir = build("t1504_dev", "pycc_t1504_dev");
    let got = python(
        &dir,
        &["-X", "dev"],
        &[("PYTHONMALLOC", "debug")],
        &["pycc_t1504_dev"],
    );
    assert_eq!(got, EXPECTED);
}

/// A dropped compiled instance keeps only the last value each attribute
/// stored (D-107, D-154); CPython frees the instance and reads `0 0` on
/// every line.
#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn the_instance_lifetime_residual_keeps_only_the_last_value() {
    let dir = build("t1504_residual", "pycc_t1504_res");
    let compiled = python(&dir, &[], &[], &["pycc_t1504_res", "residual"]);
    assert_eq!(compiled, RESIDUAL);
    let oracle = python(&dir, &[], &[], &["pycc_t1504_res_py", "residual"]);
    assert_eq!(oracle, "construct 0 0\ninit 0 0\ncopy_store 0 0\n");
}

/// An embedded program whose compiled instance holds a `Fraction`
/// attribute: rebinding it from a method, reading it into a local and
/// comparing two reads, fifty times, leaves `T`'s reference count at what
/// CPython's run of the same source reports -- one reference while the
/// attribute holds `T`, none once it is rebound away.
#[cfg(not(windows))]
const EMBEDDED: &str = "\
import sys
from fractions import Fraction

T = Fraction(1, 3)


class Box:
    f: Fraction

    def __init__(self, f: Fraction) -> None:
        self.f = f

    def rebind(self, g: Fraction) -> None:
        self.f = g
        self.f = Fraction(2, 3)
        self.f = g

    def read(self) -> None:
        x = self.f
        y = x == self.f


b = Box(Fraction(1, 2))
before = int(sys.getrefcount(T))
for i in range(50):
    b.rebind(T)
    b.read()
print(int(sys.getrefcount(T)) - before)
b.rebind(Fraction(5, 7))
print(int(sys.getrefcount(T)) - before)
";

/// The embedded executable's instance attribute balances `T`'s reference
/// count as CPython's run of the same source does. (Its launcher's isolated
/// configuration ignores `PYTHONMALLOC`, so there is no debug-allocator run
/// here; the `--ext` probe above has one.)
#[cfg(not(windows))]
#[test]
#[ignore = "needs CPython 3.14.7 as python3.14 or PYCC_PYTHON; run with --include-ignored"]
fn an_embedded_executable_instance_attribute_owns_its_reference() {
    let dir = ScratchDir::new("t1504_embedded").expect("scratch");
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
    assert_eq!(stdout_of(&oracle), "1\n0\n", "{}", stderr_of(&oracle));
    let run = Command::new(dir.join("app"))
        .output()
        .expect("the embedded binary runs");
    assert_eq!(run.status.code(), Some(0), "{}", stderr_of(&run));
    assert_eq!(stdout_of(&run), stdout_of(&oracle));
}

/// A native build has `object`-typed attributes too -- a generic class's
/// `T`-typed field -- but links no CPython shim, so the store and the read
/// emit no reference traffic there and the program links and runs. (Part 1
/// of #1499's unconditional store-side retain broke this link.)
#[cfg(not(windows))]
#[test]
fn a_native_generic_attribute_links_without_the_shim() {
    let dir = ScratchDir::new("t1504_native").expect("scratch");
    let src = dir.join("n.py");
    std::fs::write(&src, NATIVE_GENERIC).expect("write the program");
    let build = pycc()
        .arg("build")
        .arg(&src)
        .arg("-o")
        .arg(dir.join("app"))
        .output()
        .expect("pycc should spawn");
    assert!(build.status.success(), "{}", stderr_of(&build));
    assert!(!dir.join("app.pycc").exists(), "a native build");
    let run = Command::new(dir.join("app"))
        .output()
        .expect("the native binary runs");
    assert_eq!(run.status.code(), Some(0), "{}", stderr_of(&run));
    assert_eq!(stdout_of(&run), "ok\n");
}

/// A generic class whose `T` fields are stored from a parameter, rebound,
/// bound to a local, discarded, returned, selected by a conditional
/// expression and passed to a generic function and a constructor. Nothing
/// calls them with a native value, whose boxing needs the shim's packers
/// and is `I0406` in a native build (#1508): the build links every emitted
/// body, which is what is pinned.
#[cfg(not(windows))]
const NATIVE_GENERIC: &str = "\
from typing import Generic, TypeVar

T = TypeVar(\"T\")


class Box(Generic[T]):
    def __init__(self, v: T) -> None:
        self.v = v
        self.w = v

    def get(self) -> T:
        y = self.v
        self.v
        return self.v

    def put(self, v: T) -> None:
        self.v = v

    def pick(self, c: bool) -> T:
        return self.v if c else self.w

    def copy(self) -> Box[T]:
        return Box(ident(self.v))


def ident(x: T) -> T:
    return x


def wrap(v: T) -> Box[T]:
    b = Box(v)
    b.put(b.get())
    b.w = b.pick(True)
    return b.copy()


print(\"ok\")
";
