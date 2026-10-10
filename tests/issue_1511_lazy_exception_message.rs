//! #1511: the `--ext` exception bridge never formats an exception it only
//! propagates.
//!
//! CPython never calls `__str__` on an exception that merely passes through a
//! function. The bridge used to call `str(exc)` the moment a CPython
//! exception entered compiled code, which a counting `__str__` could observe
//! (and which made lark's `UnexpectedToken` cache a message too early). Now a
//! bridged exception's message is produced only when compiled code renders
//! it (`pycc_rt::exception::message`).
//!
//! Each case builds `m.py` as an extension and runs a host script against
//! it, then runs the same script against the same source imported by
//! CPython itself, and asserts both runs print the same stdout. Helper
//! modules sit on a host-only `PYTHONPATH`.
//!
//! Every test here is `#[ignore]`d and contributes no line coverage; the
//! Tier-1 `native-build-test` leg runs them with
//! `cargo test --workspace -- --include-ignored`. The Rust lines this
//! change adds are covered by `pycc_rt::exception::message`'s unit tests.

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

fn oracle() -> Command {
    Command::new(std::env::var_os("PYCC_PYTHON").unwrap_or_else(|| "python3".into()))
}

/// Writes each `(name, text)` helper module into `dir/hostlib`, the
/// host-only `PYTHONPATH` entry.
fn write_helpers(dir: &Path, helpers: &[(&str, &str)]) {
    let lib = dir.join("hostlib");
    std::fs::create_dir_all(&lib).expect("create hostlib");
    for (name, text) in helpers {
        std::fs::write(lib.join(format!("{name}.py")), text).expect("write a helper module");
    }
}

/// Builds `body` as the extension module `module` inside `dir`.
fn build_ext(dir: &Path, module: &str, body: &str) {
    let src = dir.join("m.py");
    std::fs::write(&src, body).expect("write the fixture source");
    let build = pycc()
        .arg("build")
        .arg(&src)
        .arg("-o")
        .arg(dir.join(module))
        .arg("--ext")
        .env("PYTHONPATH", dir.join("hostlib"))
        .output()
        .expect("pycc should spawn");
    assert!(
        build.status.success(),
        "{}{}",
        stdout_of(&build),
        stderr_of(&build)
    );
}

/// Runs `script` in the host with `dir` first on `sys.path`.
fn python(dir: &Path, script: &str) -> Output {
    oracle()
        .arg("-c")
        .arg(script)
        .current_dir(dir)
        .env("PYTHONPATH", dir.join("hostlib"))
        .env("PYTHONUNBUFFERED", "1")
        .output()
        .expect("python3 should spawn")
}

fn assert_ok(run: &Output, what: &str) {
    assert!(
        run.status.success(),
        "{what} -- stdout: {}\nstderr: {}",
        stdout_of(run),
        stderr_of(run)
    );
}

/// Runs `script` against `body` built by pycc as `module`, and against
/// `body` imported as `module` by CPython, and asserts both print
/// `expected`.
fn assert_matches_cpython(
    tag: &str,
    module: &str,
    body: &str,
    helpers: &[(&str, &str)],
    script: &str,
    expected: &str,
) {
    let hosted = ScratchDir::new(tag).expect("scratch");
    write_helpers(&hosted, helpers);
    build_ext(&hosted, module, body);
    let run = python(&hosted, script);
    assert_ok(&run, "pycc");
    assert_eq!(stdout_of(&run), expected, "pycc on {body}");

    let reference = ScratchDir::new(&format!("{tag}_cpython")).expect("scratch");
    write_helpers(&reference, helpers);
    std::fs::write(reference.join(format!("{module}.py")), body).expect("write the oracle source");
    let cpython = python(&reference, script);
    assert_ok(&cpython, "CPython");
    assert_eq!(stdout_of(&cpython), expected, "CPython on {body}");
}

/// A host helper module defining an exception whose `__str__` counts its
/// calls, plus a function that raises one.
const COUNTING: &str = "\
class Counting(ValueError):
    calls = 0

    def __str__(self) -> str:
        Counting.calls += 1
        return 'counted'


def fail() -> None:
    raise Counting('original')
";

/// The issue's own reproduction: `raise e` of a host exception, escaping
/// unchanged. No `__str__` call, and the host sees the original object with
/// its args, explicit cause, implicit context and traceback.
#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn propagating_a_raised_exception_never_calls_str() {
    assert_matches_cpython(
        "lazy1511_reraise",
        "pycc_lazy1511_reraise",
        "from typing import Any\n\n\ndef reraise(e: Any) -> None:\n    raise e\n",
        &[("counting", COUNTING)],
        "import pycc_lazy1511_reraise as m\n\
         from counting import Counting\n\
         cause = KeyError('cause')\n\
         context = IndexError('context')\n\
         c = Counting('a', 2)\n\
         c.__cause__ = cause\n\
         c.__context__ = context\n\
         try:\n    m.reraise(c)\n\
         except Counting as got:\n\
         \x20   print(got is c, got.args, got.__cause__ is cause, got.__context__ is context)\n\
         \x20   print(got.__traceback__ is not None, type(got).__name__)\n\
         print(Counting.calls)\n",
        "True ('a', 2) True True\nTrue Counting\n0\n",
    );
}

/// A foreign call that fails inside a compiled function, through a `try`
/// whose handler does not match, and through a `finally`: still no
/// `__str__` call.
#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn a_failed_foreign_call_propagates_without_calling_str() {
    assert_matches_cpython(
        "lazy1511_call",
        "pycc_lazy1511_call",
        "import counting\n\n\n\
         def run() -> int:\n\
         \x20   try:\n\
         \x20       counting.fail()\n\
         \x20   except KeyError:\n\
         \x20       return 1\n\
         \x20   finally:\n\
         \x20       print('finally')\n\
         \x20   return 0\n\n\n\
         def outer() -> int:\n    return run()\n",
        &[("counting", COUNTING)],
        "import pycc_lazy1511_call as m\n\
         from counting import Counting\n\
         try:\n    m.outer()\n\
         except Counting as got:\n    print(got.args)\n\
         print(Counting.calls)\n",
        "finally\n('original',)\n0\n",
    );
}

/// An exception caught in compiled code and swallowed, and one an `except*`
/// handler matches: neither renders it, so neither calls `__str__`.
#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn catching_without_rendering_never_calls_str() {
    assert_matches_cpython(
        "lazy1511_swallow",
        "pycc_lazy1511_swallow",
        "import counting\n\n\n\
         def swallow() -> str:\n\
         \x20   try:\n\
         \x20       counting.fail()\n\
         \x20   except ValueError:\n\
         \x20       return 'swallowed'\n\
         \x20   return 'unreached'\n\n\n\
         def star() -> str:\n\
         \x20   out = 'none'\n\
         \x20   try:\n\
         \x20       counting.fail()\n\
         \x20   except* ValueError:\n\
         \x20       out = 'star'\n\
         \x20   return out\n",
        &[("counting", COUNTING)],
        "import pycc_lazy1511_swallow as m\n\
         from counting import Counting\n\
         print(m.swallow(), m.star(), Counting.calls)\n",
        "swallowed star 0\n",
    );
}

/// A caught exception that compiled code does render still shows CPython's
/// own `str(exc)` -- through `print`, and through an f-string -- calling
/// `__str__` once per rendering, as CPython does.
#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn a_caught_exception_renders_its_own_message() {
    assert_matches_cpython(
        "lazy1511_render",
        "pycc_lazy1511_render",
        "from typing import Any\n\nimport counting\n\n\n\
         def show() -> None:\n\
         \x20   try:\n\
         \x20       counting.fail()\n\
         \x20   except ValueError as err:\n\
         \x20       print(err)\n\n\n\
         def describe(e: Any) -> str:\n\
         \x20   try:\n\
         \x20       raise e\n\
         \x20   except Exception as err:\n\
         \x20       return f'caught: {err}'\n\
         \x20   return 'unreached'\n",
        &[("counting", COUNTING)],
        "import pycc_lazy1511_render as m\n\
         from counting import Counting\n\
         m.show()\n\
         print(Counting.calls)\n\
         print(m.describe(KeyError('k')))\n\
         print(m.describe(Counting()), Counting.calls)\n",
        "counted\n1\ncaught: 'k'\ncaught: counted 2\n",
    );
}

/// A host helper whose exception's `__str__` calls back into a compiled
/// export (`hook`, set by the host script), recording what each call
/// returned or raised.
const REENTRANT: &str = "\
hook = None
pings = []
errors = []


class Reentrant(ValueError):
    def __str__(self) -> str:
        try:
            pings.append(hook())
        except BaseException as exc:
            errors.append(type(exc).__name__)
        return 'msg'


def fail() -> None:
    raise Reentrant('x')
";

/// The compiled side of the re-entrant cases: an export the `__str__` calls,
/// and a function whose `except*` leaves the bridged exception unmatched.
const REENTRANT_BODY: &str = "import reentrant\n\n\n\
     def ping() -> int:\n    return 1\n\n\n\
     def star() -> int:\n\
     \x20   try:\n\
     \x20       reentrant.fail()\n\
     \x20   except* KeyError:\n\
     \x20       pass\n\
     \x20   return 0\n";

/// While the message is produced, the exception being rendered is no longer
/// pending. Here the unmatched `except*` rest group escapes, and the export
/// wrapper renders it, which runs the original's `__str__`. That `__str__`
/// calls the compiled `ping`, which must return normally. If the escaping
/// group were still pending, `ping`'s own wrapper would re-raise it and
/// re-render it recursively.
///
/// The host sees the recorded `except*` deviation (a plain `Exception`
/// carrying the member's message, see `issue_1293_import_bridge`), so the
/// pycc run is pinned on its own. CPython is run on the same script for
/// the part both share: rendering the original calls `ping` once, it
/// returns 1, and nothing raises.
#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn a_str_that_calls_a_compiled_export_while_rendering_sees_nothing_pending() {
    let script = "import pycc_lazy1511_reenter as m\n\
                  import reentrant\n\
                  reentrant.hook = m.ping\n\
                  try:\n    m.star()\n\
                  except BaseException as e:\n    caught = e\n\
                  leaf = caught.exceptions[0] if isinstance(caught, BaseExceptionGroup) else caught\n\
                  text = str(leaf)\n\
                  print(text, reentrant.pings, reentrant.errors)\n";

    let hosted = ScratchDir::new("lazy1511_reenter").expect("scratch");
    write_helpers(&hosted, &[("reentrant", REENTRANT)]);
    build_ext(&hosted, "pycc_lazy1511_reenter", REENTRANT_BODY);
    let run = python(&hosted, script);
    assert_ok(&run, "pycc");
    assert_eq!(stdout_of(&run), "msg [1] []\n", "pycc");

    let reference = ScratchDir::new("lazy1511_reenter_cpython").expect("scratch");
    write_helpers(&reference, &[("reentrant", REENTRANT)]);
    std::fs::write(reference.join("pycc_lazy1511_reenter.py"), REENTRANT_BODY)
        .expect("write the oracle source");
    let cpython = python(&reference, script);
    assert_ok(&cpython, "CPython");
    assert_eq!(stdout_of(&cpython), "msg [1] []\n", "CPython");
}
