//! NEG-004 (Part E of #1038, #1067): the CPython-driven conformance
//! harness for D-244's host call boundary.
//!
//! D-244 rule 7 says a non-conforming call raises `TypeError` at the
//! wrapper before the compiled body runs, "with no argument mutated and no
//! partial effect left behind". This file is where that sentence is
//! checked against a real interpreter: one `--ext` artifact, then every
//! refusal shape the boundary can produce plus the three properties a
//! single refusal cannot show on its own -- that nothing was left behind,
//! that the export is still correct afterwards, and that a call with two
//! non-conforming arguments names the first one.
//!
//! There is no oracle here, and that is the point. Rule 4's pinned CPython
//! 3.14.7 oracle scopes to *conforming* calls -- what a compiled function
//! computes. A refusal is pycc's own contract at the boundary, so the
//! interpreter in this file is a **loader**, not an oracle: `PYCC_PYTHON`
//! (default `python3`) only has to be a CPython 3.13+ that can import the
//! artifact. Both supported versions must agree, which is why the harness
//! is expected to pass unchanged under `PYCC_PYTHON=python3.14`.
//!
//! `PYCC_PYTHON` is read twice for two different purposes: `src/ext_build.rs`
//! probes it at build time for the development headers, and every `ext`
//! test reads it at run time as the loader. Building against one version
//! and loading on another is `PYCC_PYTHON_INCLUDE`'s job, not this one's.
//!
//! Assertion convention: text pycc authors is asserted **exactly**; text
//! CPython authors is asserted by exception *type* plus a substring both
//! supported versions share. Exactly two shapes below are CPython-authored
//! -- the keyword-argument refusal, which `METH_FASTCALL` dispatch emits
//! before the wrapper is entered at all, and the lone-surrogate
//! `UnicodeEncodeError` from `PyUnicode_AsUTF8AndSize` -- and each carries
//! a comment saying so.
//!
//! `#[ignore]`d for the reason every `ext` test is: it asks an installed
//! CPython 3.13+ to import a built artifact, which is a property of the
//! machine. CI runs it twice, on both supported loaders: on every Tier-1
//! `native-build-test` leg against the 3.13 stable-ABI floor, and in
//! `build-test-coverage`'s own `cargo test --workspace -- --include-ignored`
//! step, which runs after that job installs CPython 3.14.7. It is measured
//! in neither, because only that job's earlier `llvm-cov` step is, and that
//! one runs without `--include-ignored`. The completeness guard that *is*
//! inside the coverage denominator lives in
//! `src/ext_build_tests/refusal_completeness.rs`.
//!
//! One `#[test]`, deliberately: each one costs a full `pycc build` on every
//! Tier-1 leg, so the shapes share a single artifact and a single script.

use pycc_scratch::ScratchDir;
use std::process::Command;

/// The probe module. Every export exists to be called *wrongly*; the
/// conforming calls are the re-entrancy half of the same statement.
///
/// `_log` is seeded with one element because an empty list literal has no
/// inferable element type here (`T0003`), so the counter's baseline is
/// `size() == 1` rather than 0. It is mutated in place rather than
/// rebound: a module-level rebinding needs a `global` statement, which is
/// not compiled today, and mutation is what the no-partial-effect property
/// needs anyway.
const FIXTURE: &str = r#"
_log: list[int] = [0]


def take_int(x: int) -> int:
    return x + 1


def two(a: int, b: int) -> int:
    return a + b


def take_tuple(t: tuple[int, int]) -> int:
    return t[0] + t[1]


def take_str(s: str) -> str:
    return s


def take_float(x: float) -> float:
    return x + 1.0


def take_bool(b: bool) -> bool:
    return b


def take_tuple_fb(t: tuple[float, bool]) -> float:
    return t[0]


def join(a: str, b: str) -> str:
    return a + b


def bump(x: int) -> int:
    _log.append(x)
    return len(_log)


def size() -> int:
    return len(_log)
"#;

/// The host-side script. It runs under the loader, not under an oracle, so
/// every assertion here is about pycc's own boundary contract.
const SCRIPT: &str = r#"
import pycc_1067_mod as m


class SubInt(int):
    pass


class SubStr(str):
    pass


class SubTuple(tuple):
    pass


class SubFloat(float):
    pass


def refuse(fn, args, kwargs, exc, text, exact, good, want):
    """Refuse `fn(*args, **kwargs)`, then prove `fn(*good)` still works."""
    try:
        fn(*args, **kwargs)
    except exc as e:
        got = str(e)
        if exact:
            assert got == text, got
        else:
            assert text in got, got
    else:
        raise AssertionError('the call was not refused')
    # Re-entrancy. A rule-7 refusal returns before the compiled body runs,
    # so the very next conforming call to the same export must return its
    # correct value -- not merely not crash.
    assert fn(*good) == want, fn


# Shapes 1-3: a wrong type at an `int` parameter. pycc-authored text.
refuse(m.take_int, ('x',), {}, TypeError,
       "take_int() argument 1: 'str' object cannot be interpreted as an integer",
       True, (1,), 2)
refuse(m.take_int, (1.5,), {}, TypeError,
       "take_int() argument 1: 'float' object cannot be interpreted as an integer",
       True, (1,), 2)
refuse(m.take_int, (None,), {}, TypeError,
       "take_int() argument 1: 'NoneType' object cannot be interpreted as an integer",
       True, (1,), 2)

# Shapes 4-5 and 17: arity. Checked before any argument is read, because
# reading `args[1]` of a one-argument call is out of bounds, not a
# `TypeError`. pycc-authored text, including the singular/plural split.
refuse(m.two, (1,), {}, TypeError,
       'two() takes exactly 2 arguments (1 given)', True, (1, 2), 3)
refuse(m.two, (1, 2, 3), {}, TypeError,
       'two() takes exactly 2 arguments (3 given)', True, (1, 2), 3)
refuse(m.take_int, (), {}, TypeError,
       'take_int() takes exactly 1 argument (0 given)', True, (1,), 2)

# Shape 6: keyword arguments. CPython-authored -- `METH_FASTCALL` dispatch
# refuses these before the generated wrapper is entered, and qualifies the
# name with the module, so only the shared substring is asserted.
refuse(m.two, (), {'a': 1, 'b': 2}, TypeError,
       'takes no keyword arguments', False, (1, 2), 3)

# Shapes 7-9: `tuple`. Length, container type, and element type are three
# distinct refusals, and the element one is 1-based in its own message.
# pycc-authored text.
refuse(m.take_tuple, ((1, 2, 3),), {}, TypeError,
       'take_tuple() argument 1: expected a tuple of length 2, got 3',
       True, ((1, 2),), 3)
refuse(m.take_tuple, ([1, 2],), {}, TypeError,
       "take_tuple() argument 1: 'list' object cannot be interpreted as a tuple",
       True, ((1, 2),), 3)
refuse(m.take_tuple, ((1, 'x'),), {}, TypeError,
       "take_tuple() argument 1, element 2: 'str' object cannot be interpreted as an integer",
       True, ((1, 2),), 3)

# Shapes 10-11: a wrong type at a `str` parameter. pycc-authored text.
refuse(m.take_str, (1,), {}, TypeError,
       "take_str() argument 1: 'int' object cannot be interpreted as a str",
       True, ('ok',), 'ok')
refuse(m.take_str, (None,), {}, TypeError,
       "take_str() argument 1: 'NoneType' object cannot be interpreted as a str",
       True, ('ok',), 'ok')

# Shape 12: a lone surrogate. CPython-authored -- the message comes out of
# `PyUnicode_AsUTF8AndSize`, and it is a `UnicodeEncodeError`, not a
# `TypeError`, so the shared substring is what is asserted.
refuse(m.take_str, ('\ud800',), {}, UnicodeEncodeError,
       "'utf-8' codec can't encode character", False, ('ok',), 'ok')

# Shapes 13-16: conformance is by protocol, not by exact type. `bool` is an
# `int`, and a subclass of an admitted type is admitted -- but what comes
# back is the base type, never the subclass.
assert m.take_int(True) == 2
assert m.take_int(SubInt(4)) == 5
back = m.take_str(SubStr('ab'))
assert back == 'ab', back
assert type(back) is str, type(back)
assert m.take_tuple(SubTuple((3, 4))) == 7

# No partial effect: a refused call leaves the module exactly as it found
# it. `bump` is the one export with an observable side effect, and `size`
# reads it without touching it.
before = m.size()
try:
    m.bump('x')
except TypeError:
    pass
else:
    raise AssertionError('bump was not refused')
assert m.size() == before, m.size()
assert m.bump(9) == before + 1
assert m.size() == before + 1, m.size()

# Shapes 18-19: a wrong type at a `float` parameter. `PyFloat_Check` with
# no fallback converter, so an `int` is refused rather than widened.
# pycc-authored text.
refuse(m.take_float, (1,), {}, TypeError,
       "take_float() argument 1: 'int' object cannot be interpreted as a float",
       True, (1.5,), 2.5)
refuse(m.take_float, (None,), {}, TypeError,
       "take_float() argument 1: 'NoneType' object cannot be interpreted as a float",
       True, (1.5,), 2.5)

# Shape 20: a wrong type at a `bool` parameter. `PyBool_Check`, never
# `PyObject_IsTrue`: rule 7's boundary is closed, so `1` is not `True`.
# pycc-authored text.
refuse(m.take_bool, (1,), {}, TypeError,
       "take_bool() argument 1: 'int' object cannot be interpreted as a bool",
       True, (True,), True)

# Shapes 21-22: the `_at` element helpers for the other two D-116 element
# types, each naming its own 1-based element index. pycc-authored text.
refuse(m.take_tuple_fb, ((1, True),), {}, TypeError,
       "take_tuple_fb() argument 1, element 1: 'int' object cannot be "
       "interpreted as a float",
       True, ((1.5, True),), 1.5)
refuse(m.take_tuple_fb, ((1.5, 1),), {}, TypeError,
       "take_tuple_fb() argument 1, element 2: 'int' object cannot be "
       "interpreted as a bool",
       True, ((1.5, True),), 1.5)

# Shapes 23-24: `float` conformance is by protocol too, at a scalar and at
# an element position -- `PyFloat_Check` is subtype-aware -- and what comes
# back is the base type. `bool` has no counterpart: it cannot be subclassed.
back = m.take_float(SubFloat(1.5))
assert back == 2.5, back
assert type(back) is float, type(back)
assert m.take_tuple_fb((SubFloat(1.5), True)) == 1.5

# Refusal ordering: with two simultaneously non-conforming arguments, the
# message names argument 1. Arguments are unpacked left to right and the
# first failure returns, so the second one is never reached.
refuse(m.join, (1, 2), {}, TypeError,
       "join() argument 1: 'int' object cannot be interpreted as a str",
       True, ('a', 'b'), 'ab')
"#;

#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn a_built_ext_module_refuses_every_non_conforming_call_shape_at_the_boundary() {
    let dir = ScratchDir::new("1067_ext").expect("scratch");
    let src = dir.join("m.py");
    std::fs::write(&src, FIXTURE).expect("write the fixture");
    let build = Command::new(std::path::PathBuf::from(env!("CARGO_BIN_EXE_pycc")))
        .arg("build")
        .arg(&src)
        .arg("-o")
        .arg(dir.join("pycc_1067_mod"))
        .arg("--ext")
        .output()
        .expect("pycc should spawn");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(std::env::var_os("PYCC_PYTHON").unwrap_or_else(|| "python3".into()))
        .arg("-c")
        .arg(SCRIPT)
        .current_dir(&*dir)
        .output()
        .expect("python3 should spawn");
    assert!(
        run.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
}
