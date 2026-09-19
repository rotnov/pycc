//! Exported instance methods and the generated `Py_tp_init` against a real
//! CPython (Part 2 of #1131, issue #1145).
//!
//! Every test here is `#[ignore]`d, and for exactly the reason
//! `tests/issue_1143_ext_methods.rs` gives: each builds an artifact and asks
//! an installed CPython 3.13+ to import it, which is a property of the
//! machine rather than of the change under test. CI runs them on every
//! Tier-1 `native-build-test` leg through that job's
//! `cargo test --workspace -- --include-ignored`, and the coverage job --
//! which runs `llvm-cov` *without* `--include-ignored` -- deliberately takes
//! no line coverage from them. `src/ext_build_tests/generated_c.rs` covers
//! the emitted text and `src/ext_build_tests/exports.rs` the export set.
//!
//! What only this file can cover is the part that is true of the running
//! interpreter rather than of the emitted bytes: that `mod.Class(...)` really
//! allocates and initializes a native instance, that a bound
//! `mod.Class(...).method()` really reaches the compiled body with the right
//! receiver, that the hand-written keyword boundary in `tp_init` really
//! raises where `METH_FASTCALL` would have raised for free, and that the
//! object model around the published type (subclassing, refcounts,
//! `__new__`) behaves as D-244's amendment says it does.

use pycc_scratch::ScratchDir;
use std::path::Path;
use std::process::{Command, Output};

fn pycc() -> Command {
    Command::new(env!("CARGO_BIN_EXE_pycc"))
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn build_ext(dir: &Path, module: &str, body: &str) {
    let src = dir.join("m.py");
    std::fs::write(&src, body).expect("write the fixture source");
    let build = pycc()
        .arg("build")
        .arg(&src)
        .arg("-o")
        .arg(dir.join(module))
        .arg("--ext")
        .output()
        .expect("pycc should spawn");
    assert!(build.status.success(), "{}", stderr_of(&build));
}

fn run_python(dir: &Path, script: &str) {
    let run = Command::new(std::env::var_os("PYCC_PYTHON").unwrap_or_else(|| "python3".into()))
        .arg("-c")
        .arg(script)
        .current_dir(dir)
        .output()
        .expect("python3 should spawn");
    assert!(
        run.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&run.stdout),
        stderr_of(&run)
    );
}

/// One class carrying all three method kinds, so the instance-method rows
/// and Part 1's rows are observed on the *same* type object rather than on
/// two fixtures that could drift apart.
const GRID: &str = "\
class Grid:
    def __init__(self, w: int, h: int) -> None:
        self.w = w
        self.h = h

    def area(self) -> int:
        return self.w * self.h

    @staticmethod
    def scale(n: int) -> int:
        return n * 3

    @classmethod
    def make(cls, n: int) -> int:
        return n + 1
";

#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn an_instance_is_constructed_and_its_method_reaches_the_compiled_body() {
    let dir = ScratchDir::new("ext_1145_happy").expect("scratch");
    build_ext(&dir, "grid", GRID);
    run_python(
        &dir,
        "\
import grid
g = grid.Grid(2, 3)
assert type(g) is grid.Grid, type(g)
assert g.area() == 6, g.area()
assert grid.Grid(5, 7).area() == 35, grid.Grid(5, 7).area()
",
    );
}

#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn part_ones_surface_is_unchanged_and_no_flat_attribute_appears() {
    // Adding a third method kind to the type object must not move or
    // duplicate the two Part 1 already published, and an instance method is
    // published under exactly one spelling -- `mod.Grid.area` -- never as a
    // flat `mod."Grid.area"` module attribute.
    let dir = ScratchDir::new("ext_1145_surface").expect("scratch");
    build_ext(&dir, "grid", GRID);
    run_python(
        &dir,
        "\
import grid
assert grid.Grid.scale(3) == 9, grid.Grid.scale(3)
assert grid.Grid.make(3) == 4, grid.Grid.make(3)
assert not hasattr(grid, 'Grid.area'), dir(grid)
assert not hasattr(grid, 'area'), dir(grid)
# `__init__` reaches the host only as the type's `tp_init` slot, never as a
# method-table row: its second mangled segment starts with `_`, so both
# export predicates refuse it. A `hasattr` probe cannot see the difference --
# every object has `__init__` -- but the descriptor kind can.
assert type(grid.Grid.__dict__['__init__']) is not type(grid.Grid.__dict__['area']), (
    type(grid.Grid.__dict__['__init__'])
)
assert not hasattr(grid, 'Grid.__init__'), dir(grid)
",
    );
}

#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn the_constructor_refuses_a_keyword_even_with_the_right_positional_count() {
    // The discriminating case for D-244 rule 7 at `tp_init`. Every other
    // export gets the keyword `TypeError` from `METH_FASTCALL` for free;
    // `tp_init` receives a `kwds` dictionary and has to refuse it itself. A
    // keyword with a *wrong* positional count would be caught by the arity
    // check and prove nothing, so this passes the right count and a keyword.
    let dir = ScratchDir::new("ext_1145_kw").expect("scratch");
    build_ext(&dir, "grid", GRID);
    run_python(
        &dir,
        "\
import grid
for call in (
    lambda: grid.Grid(2, 3, extra=1),
    lambda: grid.Grid(w=2, h=3),
    lambda: grid.Grid(2, h=3),
):
    try:
        call()
    except TypeError:
        pass
    else:
        raise AssertionError('a keyword argument should be refused')
# The instance method's own boundary, which `METH_FASTCALL` closes.
try:
    grid.Grid(2, 3).area(n=1)
except TypeError:
    pass
else:
    raise AssertionError('a keyword argument should be refused')
",
    );
}

#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn the_constructor_refuses_both_arities_and_names_the_source_level_spelling() {
    // Both directions: too few and too many. The message renders
    // `Grid.__init__`, which is what CPython itself renders for a Python
    // class, and never the mangled C symbol.
    let dir = ScratchDir::new("ext_1145_arity").expect("scratch");
    build_ext(&dir, "grid", GRID);
    run_python(
        &dir,
        "\
import grid
messages = []
for call in (lambda: grid.Grid(1), lambda: grid.Grid(1, 2, 3)):
    try:
        call()
    except TypeError as exc:
        messages.append(str(exc))
    else:
        raise AssertionError('a wrong arity should be refused')
for message in messages:
    assert 'Grid.__init__() takes exactly 2 arguments' in message, message
    assert '0m' not in message, message
assert '(1 given)' in messages[0], messages[0]
assert '(3 given)' in messages[1], messages[1]
",
    );
}

#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn a_method_on_an_instance_that_never_ran_tp_init_raises_instead_of_crashing() {
    // `PyType_GenericNew` zeroes the carrier and never runs `tp_init`, so
    // the wrapper's `self_inst` is really NULL here. Without the guard this
    // is a null dereference inside the compiled body, not a `TypeError`.
    let dir = ScratchDir::new("ext_1145_new").expect("scratch");
    build_ext(&dir, "grid", GRID);
    run_python(
        &dir,
        "\
import grid
raw = grid.Grid.__new__(grid.Grid)
try:
    raw.area()
except TypeError as exc:
    assert 'uninitialized instance' in str(exc), str(exc)
else:
    raise AssertionError('an uninitialized instance should be refused')
",
    );
}

#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn an_abstract_base_is_excluded_while_its_concrete_subclass_is_not() {
    // The abstract exclusion is carried entirely by `is_abstract` inside the
    // constructibility predicate, so an `@abstractmethod`'s stub body -- which
    // returns nothing while its `return_ty` says otherwise -- is never
    // reachable from the host. `Shape` has no exportable member at all, so it
    // gets no type object; `Sq`, which overrides the method, is a full
    // constructible export. The `C0003` wording for the general case is
    // "excluded because the class is not constructible".
    let dir = ScratchDir::new("ext_1145_abstract").expect("scratch");
    build_ext(
        &dir,
        "shapes",
        "\
from abc import ABC, abstractmethod


class Shape(ABC):
    @abstractmethod
    def area(self) -> int: ...


class Sq(Shape):
    def __init__(self, side: int) -> None:
        self.side = side

    def area(self) -> int:
        return self.side * self.side
",
    );
    run_python(
        &dir,
        "\
import shapes
assert not hasattr(shapes, 'Shape'), dir(shapes)
assert not hasattr(shapes, 'Shape.area'), dir(shapes)
assert not hasattr(shapes.Sq, 'Shape'), dir(shapes.Sq)
assert shapes.Sq(2).area() == 4, shapes.Sq(2).area()
",
    );
}

#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn a_property_getter_is_not_published_while_a_plain_method_of_the_same_class_is() {
    // A `@property` getter carries the same bare `<Class>.<name>` mangling an
    // instance method does, so the lexical predicate cannot tell them apart.
    // The `properties` table on the class definition is what excludes it, and
    // this is the discriminator that the exclusion is still in force now that
    // the bare spelling is admitted.
    let dir = ScratchDir::new("ext_1145_property").expect("scratch");
    build_ext(
        &dir,
        "cells",
        "\
class Cell:
    def __init__(self, v: int) -> None:
        self.v = v

    @property
    def doubled(self) -> int:
        return self.v * 2

    def plain(self) -> int:
        return self.v
",
    );
    run_python(
        &dir,
        "\
import cells
assert cells.Cell(3).plain() == 3, cells.Cell(3).plain()
assert not hasattr(cells.Cell, 'doubled'), dir(cells.Cell)
assert not hasattr(cells, 'Cell.doubled'), dir(cells)
",
    );
}

#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn an_instance_holds_one_reference_and_a_construct_call_drop_loop_leaks_none() {
    // The carrier object itself is an ordinary CPython object with ordinary
    // refcounting: `sys.getrefcount` sees the argument reference and nothing
    // else, and dropping it runs `tp_dealloc` without holding the type. The
    // *inner* `pycc_rt_instance_new` allocation is never freed -- that is
    // D-244's recorded residual, not something this test can observe -- but a
    // held type reference would grow the type's own count here.
    let dir = ScratchDir::new("ext_1145_refcount").expect("scratch");
    build_ext(&dir, "grid", GRID);
    run_python(
        &dir,
        "\
import sys
import grid
g = grid.Grid(2, 3)
assert sys.getrefcount(g) == 2, sys.getrefcount(g)
del g
before = sys.getrefcount(grid.Grid)
for _ in range(1000):
    h = grid.Grid(2, 3)
    assert h.area() == 6
    del h
assert sys.getrefcount(grid.Grid) == before, (before, sys.getrefcount(grid.Grid))
",
    );
}

#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn the_published_type_is_still_immutable_and_still_refuses_subclassing() {
    // Becoming constructible does not make the type a base type: without
    // `Py_TPFLAGS_BASETYPE` a host subclass could override a method the
    // compiled bodies resolved statically, and without
    // `Py_TPFLAGS_IMMUTABLETYPE` it could add one.
    let dir = ScratchDir::new("ext_1145_flags").expect("scratch");
    build_ext(&dir, "grid", GRID);
    run_python(
        &dir,
        "\
import grid
try:
    type('Sub', (grid.Grid,), {})
except TypeError:
    pass
else:
    raise AssertionError('subclassing should be refused')
try:
    grid.Grid.other = 1
except TypeError:
    pass
else:
    raise AssertionError('the type should be immutable')
",
    );
}

#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn a_published_but_unconstructible_class_keeps_refusing_instantiation() {
    // A `tuple` parameter cannot cross `tp_init`'s boundary, so `Pairish` is
    // published -- its `@staticmethod` is an export -- but gets no
    // `Py_tp_init` and keeps `Py_TPFLAGS_DISALLOW_INSTANTIATION`. It has no
    // instance method, so nothing about it is a `C0003` gap either.
    //
    // The constructor body stores a literal rather than the parameter
    // because storing a `tuple` in an instance attribute is itself a
    // `C0001` today -- an unrelated limit, and one that would make the
    // fixture fail to compile long before it could exercise the boundary
    // this test is about.
    let dir = ScratchDir::new("ext_1145_unconstructible").expect("scratch");
    build_ext(
        &dir,
        "pairish",
        "\
class Pairish:
    def __init__(self, t: tuple[int, int]) -> None:
        self.n = 0

    @staticmethod
    def scale(n: int) -> int:
        return n * 2
",
    );
    run_python(
        &dir,
        "\
import pairish
assert pairish.Pairish.scale(2) == 4, pairish.Pairish.scale(2)
try:
    pairish.Pairish((1, 2))
except TypeError:
    pass
else:
    raise AssertionError('an unconstructible class should refuse instantiation')
",
    );
}

#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn a_host_constructed_instance_has_the_same_slot_layout_as_a_native_one() {
    // The generated `tp_init` allocates `pycc_rt_instance_new(<slot_count>)`
    // from `pycc_hir::flat_attr_layout`, which is the same function
    // `MirExpr::Instantiate` goes through. A disagreement would not raise --
    // it would read or write past the native allocation -- so the only way to
    // see it is to compare the *same* attribute read on an instance the host
    // built against one the compiled code built, including the last slot.
    let dir = ScratchDir::new("ext_1145_slots").expect("scratch");
    build_ext(
        &dir,
        "boxes",
        "\
class Box:
    def __init__(self, a: int, b: int, c: int) -> None:
        self.a = a
        self.b = b
        self.c = c

    def last(self) -> int:
        return self.c

    def first(self) -> int:
        return self.a

    @staticmethod
    def native_last(a: int, b: int, c: int) -> int:
        return Box(a, b, c).c
",
    );
    run_python(
        &dir,
        "\
import boxes
host = boxes.Box(10, 20, 30)
assert host.last() == 30, host.last()
assert host.first() == 10, host.first()
assert boxes.Box.native_last(10, 20, 30) == host.last(), boxes.Box.native_last(10, 20, 30)
",
    );
}

#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn a_user_exception_class_stays_an_exception_class_and_exports_no_method() {
    // A user exception class is registered by #1066's path, is instantiable
    // as an exception, and is outside the constructibility predicate -- so
    // #1145 must not hand it a second, conflicting type object or a
    // `Py_tp_init`, and must not export its instance method.
    let dir = ScratchDir::new("ext_1145_exc").expect("scratch");
    build_ext(
        &dir,
        "failing",
        "\
class Failure(Exception):
    def __init__(self, n: int) -> None:
        self.n = n

    def code(self) -> int:
        return self.n
",
    );
    run_python(
        &dir,
        "\
import failing
assert issubclass(failing.Failure, Exception), failing.Failure.__mro__
assert not hasattr(failing.Failure, 'code'), dir(failing.Failure)
assert not hasattr(failing, 'Failure.code'), dir(failing)
try:
    raise failing.Failure('boom')
except failing.Failure as exc:
    assert isinstance(exc, Exception)
",
    );
}
