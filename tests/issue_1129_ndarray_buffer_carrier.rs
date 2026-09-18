//! #1129: the bare name `ndarray` as a second spelling of the `ext`
//! boundary's buffer carrier, and the widened admission predicate that
//! makes a bare array reach it.
//!
//! Two halves, and they are one change: the annotation arm alone would
//! compile a signature that fails at run time for every real array, and the
//! run-time widening alone would be unreachable from any source program.
//!
//! What lives here rather than in `tests/issue_1114_numpy_oracle.rs` is
//! exactly what that file cannot carry. It is numpy's oracle, so every
//! assertion in it skips on a host without numpy; the annotation arm and
//! the buffer-protocol arms must be stated on a host that has never heard
//! of numpy, because the widened predicate is the *buffer protocol* and
//! not numpy — an `array.array('d')` from the standard library is as much
//! a conforming operand as an `ndarray` is, and CI installs numpy into one
//! job only (`native-build-test`'s hosted ext floor interpreter).
//!
//! The scope boundary, so a later reader does not over-read this file:
//! #1129 owns *carrier registration* — which pycc type the boundary admits
//! and what the wrapper does with the object. How a user may *spell* a
//! type is owned elsewhere: attribute-qualified `numpy.ndarray` by #889,
//! subscripted `NDArray[...]` by #1130, import aliasing by #883/#963/#964.
//! None of those is advanced here, and no numpy source file in the wild is
//! made compilable by this change on its own.
//!
//! The six tests below that need no interpreter at all — every refusal
//! and acceptance they assert is resolved on the program before `plan_ext`
//! probes the host toolchain — are not `#[ignore]`d, and they are what runs
//! inside the coverage job. Only the seventh, which builds an artifact and
//! loads it into a live interpreter, is hosted.

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

/// Writes `source` as the entry module of a fresh scratch directory.
fn fixture(category: &str, source: &str) -> ScratchDir {
    let dir = ScratchDir::new(category).expect("scratch");
    std::fs::write(dir.join("nd_probe.py"), source).expect("write the subject");
    dir
}

/// Builds the entry module as a CPython extension module directly into
/// `dir`, so a CPython run with `dir` as its working directory imports it.
///
/// The output path carries no extension suffix, for the reason
/// `tests/issue_1114_numpy_oracle.rs`'s own helper records: `pycc build
/// --ext` appends the one its target triple calls for and derives the
/// exported `PyInit_<mod>` name from the path's own spelling.
fn build_ext(dir: &Path) -> Output {
    pycc()
        .arg("build")
        .arg(dir.join("nd_probe.py"))
        .arg("-o")
        .arg(dir.join("nd_probe"))
        .arg("--ext")
        .output()
        .expect("pycc should spawn")
}

/// The subject: the same loop the buffer carrier has always admitted, at a
/// parameter spelled `ndarray` instead of `memoryview`.
///
/// `len(b)` and `b[i]` are both in the body deliberately. Neither is a new
/// capability (#1116 and Part 2 of #1027 own them) and neither branches on
/// the spelling — the constraint solver's two interceptions dispatch on
/// `Ty::MemoryView`, which is what `ndarray` lowers to — but "it should
/// work for free" is a claim, so the subject exercises it rather than
/// assuming it.
const SUBJECT: &str = "\
def total(b: ndarray) -> float:
    s: float = 0.0
    i: int = 0
    for i in range(len(b)):
        s = s + b[i]
    return s
";

/// A program that binds `ndarray` itself keeps its own meaning for the
/// name, because the spelling is resolved *after* `class_defs` and the
/// alias table rather than beside `memoryview` in the keyword list.
///
/// This is the rule the placement exists for, not a corner case. Every
/// other name that list reserves is a Python builtin or a `typing` name;
/// `ndarray` is an ordinary identifier, and in Python a module-level
/// definition shadows an imported name rather than losing to it. Reserving
/// it first was measured to refuse both programs below, each of which
/// compiles clean before the spelling exists and must keep doing so after.
#[test]
fn a_program_that_binds_ndarray_itself_keeps_its_own_meaning() {
    // A class of that name: the parameter is an instance of it, so reading
    // the parameter is an ordinary read and not the buffer capability gap.
    let cls = fixture(
        "1129_shadow_class",
        "class ndarray:\n    pass\n\ndef g(a: ndarray) -> int:\n    b = a\n    return 1\n",
    );
    let out = pycc()
        .arg("check")
        .arg(cls.join("nd_probe.py"))
        .output()
        .expect("pycc should spawn");
    assert!(
        out.status.success(),
        "a user-defined `class ndarray` must win over the buffer spelling: {}{}",
        stdout_of(&out),
        stderr_of(&out)
    );

    // An alias of that name, which resolves one link later in the same
    // chain: `a` is an `int`, so arithmetic on it type-checks.
    let alias = fixture(
        "1129_shadow_alias",
        "type ndarray = int\n\ndef g(a: ndarray) -> int:\n    return a + 1\n",
    );
    let out = pycc()
        .arg("check")
        .arg(alias.join("nd_probe.py"))
        .output()
        .expect("pycc should spawn");
    assert!(
        out.status.success(),
        "a user-defined `type ndarray` alias must win over the buffer spelling: {}{}",
        stdout_of(&out),
        stderr_of(&out)
    );
}

/// The subscripted form stays refused, and says what the name is.
///
/// `ndarray[...]` belongs to #1130 and is deliberately not admitted here.
/// What this change does own is the noun the refusal uses: before the bare
/// name resolved at all, the subscript path's own recursion failed and the
/// user saw the generic unknown-name `C0001`; now that it resolves, the
/// path reaches `T0044`, whose catch-all called every unrecognized base a
/// "type alias" -- a word for something the program never wrote. Both
/// spellings of the carrier get their own noun instead, `memoryview`
/// included, which had the same wrong word before this issue.
#[test]
fn a_subscripted_ndarray_is_refused_as_a_buffer_type_rather_than_an_alias() {
    for spelling in ["ndarray", "memoryview"] {
        let dir = fixture(
            "1129_subscript",
            &format!("def f(a: {spelling}[float]) -> int:\n    return 1\n"),
        );
        let out = pycc()
            .arg("check")
            .arg(dir.join("nd_probe.py"))
            .output()
            .expect("pycc should spawn");
        assert!(!out.status.success(), "{}", stdout_of(&out));
        // `check` renders on stdout where `build` renders on stderr, so
        // both are read rather than guessing which one this subcommand
        // uses.
        let err = format!("{}{}", stdout_of(&out), stderr_of(&out));
        assert!(err.contains("error[T0044]"), "{err}");
        assert!(
            err.contains(&format!("buffer type `{spelling}` is not subscriptable")),
            "{err}"
        );
    }
}

/// A program that binds a spelling itself gets the noun its own binding
/// earns -- which is not the same answer for the two spellings.
///
/// `memoryview` is a reserved keyword the `Expr::Name` arm decides before it
/// reads `class_defs` or the alias table, so `type memoryview = int` does
/// **not** win: the bare name still lowers to the buffer carrier, and the
/// refusal must not claim an alias resolved. `ndarray` is an ordinary
/// identifier resolved only after both, so there the alias really does win
/// and `type alias` is the truthful word. This is measured through the CLI
/// rather than through the helper, because the helper cannot observe which
/// of the two the pipeline actually resolved.
#[test]
fn a_shadowed_spelling_is_named_by_whichever_binding_actually_wins() {
    for (spelling, expected) in [
        ("memoryview", "buffer type `memoryview`"),
        ("ndarray", "type alias `ndarray`"),
    ] {
        let dir = fixture(
            "1129_subscript_shadow",
            &format!(
                "type {spelling} = int

def f(a: {spelling}[float]) -> int:
    return 1
"
            ),
        );
        let out = pycc()
            .arg("check")
            .arg(dir.join("nd_probe.py"))
            .output()
            .expect("pycc should spawn");
        assert!(!out.status.success(), "{}", stdout_of(&out));
        let err = format!("{}{}", stdout_of(&out), stderr_of(&out));
        assert!(err.contains("error[T0044]"), "{err}");
        assert!(
            err.contains(&format!("{expected} is not subscriptable")),
            "{err}"
        );
    }
}

/// The same precedence holds when the shadowing alias targets a class.
///
/// The subscript path resolves an alias to a class one step earlier than
/// `subscripted_base_description` runs, through its own predicate, so a
/// scalar-targeted alias cannot exercise this ladder at all. With
/// `type memoryview = C` the reserved keyword still wins and the refusal
/// must name the buffer type rather than `C`; with `type ndarray = C` the
/// alias wins and naming `C` is the truthful answer.
#[test]
fn an_alias_to_a_class_wins_for_ndarray_and_loses_to_memoryview() {
    for (spelling, expected) in [
        (
            "memoryview",
            "buffer type `memoryview` is not subscriptable",
        ),
        ("ndarray", "class `C` does not define `__class_getitem__`"),
    ] {
        let dir = fixture(
            "1129_subscript_class_alias",
            &format!(
                "class C:\n    pass\n\ntype {spelling} = C\n\ndef f(a: {spelling}[int]) -> int:\n    return 1\n"
            ),
        );
        let out = pycc()
            .arg("check")
            .arg(dir.join("nd_probe.py"))
            .output()
            .expect("pycc should spawn");
        assert!(!out.status.success(), "{}", stdout_of(&out));
        let err = format!("{}{}", stdout_of(&out), stderr_of(&out));
        assert!(err.contains("error[T0044]"), "{err}");
        assert!(err.contains(expected), "{err}");
    }
}

/// The annotation compiles, with no interpreter, no numpy, and no import.
///
/// The one assertion in this file that depends on nothing about the host
/// at all. `import numpy` is itself refused today (`I0403`), so a spelling
/// that required one could not be written; the bare name is recognized
/// without it, exactly as `Any`, `Annotated`, `TypeAlias` and `Self` are.
#[test]
fn an_ndarray_parameter_type_checks_without_any_import() {
    let dir = fixture("1129_check", SUBJECT);
    let check = pycc()
        .arg("check")
        .arg(dir.join("nd_probe.py"))
        .output()
        .expect("pycc should spawn");
    assert!(
        check.status.success(),
        "{}{}",
        stdout_of(&check),
        stderr_of(&check)
    );
}

/// Every position that is *not* an `--ext` parameter is refused for the new
/// spelling exactly as it is for the old one.
///
/// This is the half of the change that is deliberately *not* a widening:
/// `ndarray` lowers to `Ty::MemoryView`, so it inherits every existing
/// refusal rather than opening a second, laxer path to them.
///
/// Two message styles, and which one a site uses is a deliberate split
/// rather than an inconsistency. A message that names the *type* is
/// spelling-neutral ("a buffer"), because a user who wrote `ndarray` must
/// not be told about a `memoryview` they never mentioned; a message that
/// quotes a whole *signature position* back (`I0405`, and `C0003`'s
/// `-> memoryview`) renders the canonical spelling, because a `Ty` is the
/// compiler's canonical name for a type -- the same thing already happens
/// for `type Arr = memoryview`. #1129's D-244 amendment records the split
/// rather than leaving it to be discovered. All four arms are asserted
/// here so a later reword cannot quietly move a site from one style to the
/// other.
#[test]
fn every_non_parameter_ndarray_position_is_refused() {
    // A bare declaration: `C0001`, in both modes, because nothing produces
    // a buffer value to bind to the name. The message is the reworded,
    // spelling-neutral one — a user who wrote `ndarray` must not be told
    // about a `memoryview` they never mentioned.
    let dir = fixture(
        "1129_decl",
        "def f() -> int:\n    x: ndarray\n    return 1\n",
    );
    let native = pycc()
        .arg("build")
        .arg(dir.join("nd_probe.py"))
        .arg("-o")
        .arg(dir.join("nd_probe"))
        .output()
        .expect("pycc should spawn");
    assert!(!native.status.success(), "{}", stdout_of(&native));
    let err = stderr_of(&native);
    assert!(err.contains("error[C0001]"), "{err}");
    assert!(err.contains("declaring `x` as a buffer"), "{err}");

    // A public `-> ndarray` return under `--ext`: `C0003`. The wrapper has
    // released the buffer by the time it would have to hand one back, and
    // no development headers are needed to say so — `plan_ext` resolves the
    // capability gap on the program before it probes the host toolchain.
    let dir = fixture("1129_return", "def make() -> ndarray:\n    return make()\n");
    let ext = build_ext(&dir);
    assert!(!ext.status.success(), "{}", stdout_of(&ext));
    let err = stderr_of(&ext);
    assert!(err.contains("error[C0003]"), "{err}");
    assert!(err.contains("its return type `-> memoryview`"), "{err}");
    // The remediation enumerates what the boundary carries, so it has to
    // name the second spelling too: a user who reached this message by
    // writing `ndarray` and is shown a list without it reads the list as
    // "not that type at all".
    assert!(err.contains("(or its second spelling `ndarray`)"), "{err}");

    // A *private* `-> ndarray` return under `--ext`: `C0001`, from the
    // separate walk that closes what `collect_exports` never visits. The
    // public arm above cannot reach this site at all -- `collect_exports`
    // answers a public offender with `C0003` first -- which is exactly why
    // this one is stated: the site was left spelling-pinned and citing
    // only #1027 while its two documented siblings in `crates/pycc_types`
    // were reworded, and no existing assertion would have caught it.
    let dir = fixture(
        "1129_private_return",
        "def _f() -> ndarray:\n    return _f()\n",
    );
    let ext = build_ext(&dir);
    assert!(!ext.status.success(), "{}", stdout_of(&ext));
    let err = stderr_of(&ext);
    assert!(err.contains("error[C0001]"), "{err}");
    assert!(err.contains("`_f`'s return type is a buffer"), "{err}");
    assert!(err.contains("#1129"), "{err}");
    // And the spelling the user did not write appears nowhere in the
    // diagnostic. Scoped to the `error[C0001]` line rather than asserted
    // over the whole stream: stderr also carries scratch paths and, on some
    // hosts, toolchain warnings, none of which this arm owns.
    let line = err
        .lines()
        .find(|line| line.contains("error[C0001]"))
        .expect("the C0001 line");
    assert!(!line.contains("memoryview"), "{line}");

    // An `ndarray` *signature* in a build without `--ext`: `I0405`, the
    // artifact-mode refusal, naming the parameter in the canonical
    // spelling.
    let dir = fixture("1129_native", SUBJECT);
    let native = pycc()
        .arg("build")
        .arg(dir.join("nd_probe.py"))
        .arg("-o")
        .arg(dir.join("nd_probe"))
        .output()
        .expect("pycc should spawn");
    assert!(!native.status.success(), "{}", stdout_of(&native));
    let err = stderr_of(&native);
    assert!(err.contains("error[I0405]"), "{err}");
    assert!(err.contains("parameter `b: memoryview`"), "{err}");
}

/// The widened admission predicate, stated on the standard library alone.
///
/// The boundary's first refusal arm moved from `PyMemoryView_Check` to
/// `PyObject_CheckBuffer`, so what it admits is every conforming *buffer
/// exporter* and not numpy: an `array.array('d')` and a `memoryview` are
/// both accepted at a parameter spelled `ndarray`, a `bytes` passes arm 1
/// and is refused by the format arm, and only an object that exports
/// nothing at all is refused by arm 1 itself. That set is the deliberate
/// consequence of the predicate, not a test convenience — `docs/RUNTIME.md`
/// and D-244's #1129 amendment both state it in those terms.
///
/// Hosted, for the reason every `--ext` build-and-load test is: `--ext`
/// requires a CPython 3.13+ with development headers, and CI's coverage
/// interpreter is a different one.
#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn an_ndarray_parameter_admits_every_conforming_buffer_exporter() {
    let dir = fixture("1129_exporters", SUBJECT);
    let build = build_ext(&dir);
    assert!(build.status.success(), "{}", stderr_of(&build));

    let run = Command::new(std::env::var_os("PYCC_PYTHON").unwrap_or_else(|| "python3".into()))
        .arg("-c")
        .arg(
            "import array, nd_probe\n\
             assert not nd_probe.__file__.endswith('.py'), nd_probe.__file__\n\
             data = array.array('d', [1.5, -2.25, 3.0])\n\
             # A conforming exporter that is not a `memoryview`: accepted\n\
             # bare, which is the whole statement of the widening.\n\
             assert nd_probe.total(data) == 2.25, nd_probe.total(data)\n\
             # And a `memoryview` over the same store, which the boundary\n\
             # accepted before this change and must still accept.\n\
             assert nd_probe.total(memoryview(data)) == 2.25\n\
             def refused(arg):\n\
             \x20   try:\n\
             \x20       nd_probe.total(arg)\n\
             \x20   except BaseException as error:\n\
             \x20       return type(error).__name__, str(error)\n\
             \x20   raise AssertionError('the thunk accepted %r' % (type(arg),))\n\
             # A buffer exporter of the wrong element type reaches the\n\
             # format arm now, not arm 1.\n\
             kind, text = refused(b'abcdefgh')\n\
             assert kind == 'TypeError', (kind, text)\n\
             assert \"format 'B'\" in text, text\n\
             # Exports no buffer at all: the one arm the widening did not\n\
             # move, in its reworded pycc-authored text.\n\
             kind, text = refused([1.5, 2.5])\n\
             assert kind == 'TypeError', (kind, text)\n\
             assert text == (\"total() argument 1: 'list' object \"\n\
             \x20                'does not export a buffer'), text\n\
             print('ok')\n",
        )
        .current_dir(&*dir)
        .output()
        .expect("python3 should spawn");
    assert!(
        run.status.success(),
        "stdout: {}\nstderr: {}",
        stdout_of(&run),
        stderr_of(&run)
    );
    assert_eq!(stdout_of(&run), "ok\n");
}
