//! #1156 (W0 of #882): `__name__` reads as a module-level `str` constant.
//!
//! THE RULE, stated once here and owned by
//! `crates/pycc_hir/src/dunder_name.rs` and `docs/STDLIB_PLAN.md`'s
//! "Tier 0 — builtins" section:
//!
//! > `__name__` is a compiler-provided module-level `str` binding, seeded as
//! > the module's first top-level statement. It is provided only when the
//! > module references the name and the module's own top level binds no name
//! > `__name__`. A user binding of `__name__` at module top level wins
//! > outright: nothing is seeded and every `__name__` in that module resolves
//! > through the ordinary name path, exactly as before this change. A binding
//! > inside a function body is an ordinary local and shadows the module
//! > binding only within that function, matching CPython.
//! >
//! > Deviation from CPython, deliberate and documented: in CPython a read that
//! > textually precedes a module-level `__name__ = ...` still sees the
//! > interpreter-provided module name. Here the seed is withheld for the whole
//! > module, so such a read resolves to the user's binding — in practice a
//! > `T0021` "name `__name__` is not defined" when the read precedes the
//! > assignment. This is fail-closed (a diagnostic, never a silently wrong
//! > value) and mirrors the all-or-nothing shape D-188 already established for
//! > the builtin exception hierarchy.
//!
//! The value is `"__main__"` for `pycc check` and for a native
//! `pycc build`/`pycc run`, and the extension module's own name for a
//! `pycc build --ext`.
//!
//! Known gap, deliberate: only the *entry* module is given a name. Part 1 of
//! #881 links every module of a program into one flat namespace, so a
//! per-module `__name__` global would collide and a dependency's function
//! would read the entry module's value anyway. The three observable
//! consequences are pinned by the multi-module tests at the end of this file.
//!
//! Only the last test is `#[ignore]`d: it builds an artifact and asks an
//! installed CPython to import it, which is a property of the machine rather
//! than of the change under test (the convention
//! `tests/issue_1143_ext_methods.rs` states).

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

/// Builds `source` as a native executable and returns its stdout. Panics with
/// the compiler's own diagnostics when the build fails, so a regression shows
/// the diagnostic rather than an opaque missing-file error.
fn build_and_run(category: &str, source: &str) -> String {
    let dir = ScratchDir::new(category).expect("scratch");
    let src = dir.join("m.py");
    std::fs::write(&src, source).expect("write the fixture source");
    let out = dir.join("m");
    let build = pycc()
        .arg("build")
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .output()
        .expect("pycc should spawn");
    assert!(build.status.success(), "{}", stderr_of(&build));
    let run = Command::new(&out)
        .output()
        .expect("the artifact should spawn");
    assert!(run.status.success(), "{}", stderr_of(&run));
    stdout_of(&run)
}

/// `pycc check` on `source`, returning success plus both streams: `check`
/// renders diagnostics on stdout where `build` renders them on stderr, so
/// both are read rather than guessing which one a subcommand uses.
fn check(category: &str, source: &str) -> (bool, String) {
    let dir = ScratchDir::new(category).expect("scratch");
    let src = dir.join("m.py");
    std::fs::write(&src, source).expect("write the fixture source");
    let out = pycc()
        .arg("check")
        .arg(&src)
        .output()
        .expect("pycc should spawn");
    (
        out.status.success(),
        format!("{}{}", stdout_of(&out), stderr_of(&out)),
    )
}

// -- the provided binding -------------------------------------------

#[test]
fn a_module_level_read_prints_dunder_main() {
    assert_eq!(
        build_and_run("dn_module", "print(__name__)\n"),
        "__main__\n"
    );
}

#[test]
fn a_read_inside_a_function_body_sees_the_same_value() {
    assert_eq!(
        build_and_run(
            "dn_function",
            "\
def report() -> None:
    print(__name__)

report()
print(__name__)
",
        ),
        "__main__\n__main__\n"
    );
}

#[test]
fn the_dunder_main_guard_takes_its_true_branch() {
    assert_eq!(
        build_and_run(
            "dn_guard",
            "\
if __name__ == \"__main__\":
    print(\"entry\")
else:
    print(\"imported\")
",
        ),
        "entry\n"
    );
}

#[test]
fn the_binding_is_a_str_and_supports_str_operations() {
    assert_eq!(
        build_and_run(
            "dn_str",
            "\
greeting: str = \"hello \" + __name__
print(greeting)
print(__name__ != \"other\")
"
        ),
        "hello __main__\nTrue\n"
    );
}

#[test]
fn pycc_check_accepts_a_read_with_no_diagnostic() {
    let (ok, rendered) = check("dn_check", "print(__name__)\n");
    assert!(ok, "{rendered}");
    assert_eq!(rendered, "");
}

// -- a user binding at module top level wins outright ---------------

#[test]
fn a_module_level_assignment_wins_over_the_seed() {
    assert_eq!(
        build_and_run("dn_user_assign", "__name__ = \"custom\"\nprint(__name__)\n",),
        "custom\n"
    );
}

#[test]
fn a_module_level_annotated_assignment_wins_over_the_seed() {
    assert_eq!(
        build_and_run(
            "dn_user_ann_assign",
            "__name__: str = \"custom\"\nprint(__name__)\n",
        ),
        "custom\n"
    );
}

#[test]
fn a_user_binding_of_a_non_str_type_is_accepted_because_nothing_is_seeded() {
    // The seed is withheld for the *whole* module, so the user's binding
    // introduces the name with its own type rather than rebinding a `str`.
    assert_eq!(
        build_and_run("dn_user_int", "__name__ = 7\nprint(__name__ + 1)\n"),
        "8\n"
    );
}

#[test]
fn a_read_that_precedes_a_module_level_assignment_is_t0021() {
    // The documented deviation from CPython: CPython's interpreter-provided
    // module name would still be visible here.
    let (ok, rendered) = check("dn_precedes", "print(__name__)\n__name__ = \"custom\"\n");
    assert!(!ok, "{rendered}");
    assert!(
        rendered.contains("error[T0021]") && rendered.contains("name `__name__` is not defined"),
        "{rendered}"
    );
}

// -- a value-less annotation only declares a type ------------------

#[test]
fn a_value_less_annotation_does_not_withhold_the_seed() {
    // `__name__: str` emits no store in CPython, so the module name survives;
    // only an annotated *assignment* shadows the compiler-provided binding.
    assert_eq!(
        build_and_run("dn_ann_only", "__name__: str\nprint(__name__)\n"),
        "__main__\n"
    );
}

#[test]
fn a_value_less_annotation_of_another_type_still_yields_the_str_seed() {
    // The annotation declares nothing, so the seed's own `str` type is what
    // every later use sees -- a mismatched one is a type error at the use
    // site, never a silently mistyped global.
    assert_eq!(
        build_and_run("dn_ann_int", "__name__: int\nprint(__name__)\n"),
        "__main__\n"
    );
    let (ok, rendered) = check("dn_ann_int_use", "__name__: int\nprint(__name__ + 1)\n");
    assert!(!ok, "{rendered}");
    assert!(
        rendered.contains("operator Add is not defined for `str` and `int`"),
        "{rendered}"
    );
}

// -- expression-level bindings the top-level scan deliberately skips --
//
// `binds_dunder_name_at_top_level` scans statement targets, not every
// expression that can bind a name. These pin the two forms it does not scan,
// so a future change to either is a visible test failure rather than a silent
// race with the seed.

#[test]
fn a_match_capture_over_a_str_subject_rebinds_the_seeded_global() {
    // The seed is the module's first statement, so the capture overwrites an
    // existing `str` global rather than introducing the name.
    assert_eq!(
        build_and_run(
            "dn_match_str",
            "x: str = \"a\"\nmatch x:\n    case __name__:\n        pass\nprint(__name__)\n",
        ),
        "a\n"
    );
}

#[test]
fn a_match_capture_over_a_non_str_subject_is_t0023() {
    let (ok, rendered) = check(
        "dn_match_int",
        "x: int = 7\nmatch x:\n    case __name__:\n        pass\nprint(__name__)\n",
    );
    assert!(!ok, "{rendered}");
    assert!(
        rendered.contains("error[T0023]") && rendered.contains("previously inferred as `str`"),
        "{rendered}"
    );
}

#[test]
fn a_str_valued_walrus_binding_of_dunder_name_is_t0050() {
    // #774: `str` is not a supported walrus value, so this *particular* walrus
    // shape cannot bind a module name at all today.
    let (ok, rendered) = check("dn_walrus", "print((__name__ := \"custom\"))\n");
    assert!(!ok, "{rendered}");
    assert!(rendered.contains("error[T0050]"), "{rendered}");
}

#[test]
fn a_scalar_walrus_binding_wins_over_the_seed_as_a_bare_expression_statement() {
    // #774 permits `int`/`float`/`bool`/`None` as walrus values, so this is a
    // fully supported top-level binding of the name and THE RULE's "a top-level
    // user binding wins outright" must hold for it: nothing is seeded, and the
    // `int` rebind is not rejected with `T0023`.
    assert_eq!(
        build_and_run("dn_walrus_int", "print((__name__ := 7))\n"),
        "7\n"
    );
}

#[test]
fn a_scalar_walrus_binding_in_a_top_level_if_condition_wins_over_the_seed() {
    // The walrus scan walks *into* a top-level compound statement's own header,
    // which is where PEP 572's own motivating shape puts it.
    assert_eq!(
        build_and_run(
            "dn_walrus_if",
            "if (__name__ := 7) > 3:\n    pass\nprint(__name__)\n",
        ),
        "7\n"
    );
}

#[test]
fn a_walrus_binding_inside_a_function_body_does_not_withhold_the_seed() {
    // A walrus binds in the enclosing *function or module* scope (PEP 572), so
    // one inside a function body is a local and must leave the module seed
    // intact -- exactly as a plain `__name__ = "x"` in a function body does.
    assert_eq!(
        build_and_run(
            "dn_walrus_local",
            concat!(
                "def f() -> int:\n",
                "    if (__name__ := 7) > 3:\n",
                "        return 1\n",
                "    return 0\n",
                "\n",
                "print(f())\n",
                "print(__name__)\n",
            ),
        ),
        "1\n__main__\n"
    );
}

#[test]
fn a_dependencys_walrus_binding_withholds_the_entry_seed() {
    // The cross-module half of the gate, reached through the walrus door: a
    // dependency's own top-level `(__name__ := 7)` is the program's single
    // `__name__` global in #881's flat namespace, so the entry module is not
    // seeded and both modules read the dependency's value.
    assert_eq!(
        build_and_run_program(
            "dn_dep_walrus",
            "from dep import helper\n\nprint(__name__)\nprint(helper())\n",
            "print((__name__ := 7))\n\ndef helper() -> int:\n    return 1\n",
        ),
        "7\n7\n1\n"
    );
}

#[test]
fn a_binding_nested_in_a_top_level_compound_statement_rebinds_the_seed() {
    // The documented limit of the flat scan: the module is seeded *and* the
    // user's statement still executes and still wins at runtime.
    assert_eq!(
        build_and_run(
            "dn_nested_if",
            "flag: bool = True\nif flag:\n    __name__ = \"custom\"\nprint(__name__)\n",
        ),
        "custom\n"
    );
}

// -- a function-local binding shadows only inside that function -----

#[test]
fn a_function_local_binding_shadows_only_within_that_function() {
    assert_eq!(
        build_and_run(
            "dn_local",
            "\
def local() -> None:
    __name__ = \"local\"
    print(__name__)

local()
print(__name__)
",
        ),
        "local\n__main__\n"
    );
}

// -- the multi-module gap -------------------------------------------

/// Writes a two-file program into a fresh scratch directory and runs
/// `pycc check` on its entry module, returning success plus both streams.
fn check_program(category: &str, entry: &str, dependency: &str) -> (bool, String) {
    let dir = ScratchDir::new(category).expect("scratch");
    std::fs::write(dir.join("dep.py"), dependency).expect("write the dependency");
    let src = dir.join("m.py");
    std::fs::write(&src, entry).expect("write the entry module");
    let out = pycc()
        .arg("check")
        .arg(&src)
        .output()
        .expect("pycc should spawn");
    (
        out.status.success(),
        format!("{}{}", stdout_of(&out), stderr_of(&out)),
    )
}

/// Builds a two-file program (`dep.py` plus the `m.py` entry) and returns the
/// entry artifact's stdout, panicking with the compiler's own diagnostics when
/// the build fails.
fn build_and_run_program(category: &str, entry: &str, dependency: &str) -> String {
    let dir = ScratchDir::new(category).expect("scratch");
    std::fs::write(dir.join("dep.py"), dependency).expect("write the dependency");
    let src = dir.join("m.py");
    std::fs::write(&src, entry).expect("write the entry module");
    let out = dir.join("m");
    let build = pycc()
        .arg("build")
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .output()
        .expect("pycc should spawn");
    assert!(build.status.success(), "{}", stderr_of(&build));
    let run = Command::new(&out)
        .output()
        .expect("the artifact should spawn");
    assert!(run.status.success(), "{}", stderr_of(&run));
    stdout_of(&run)
}

// -- a dependency's own top-level binding withholds the entry seed --
//
// The program is one flat namespace (Part 1 of #881), so the seed and a
// dependency's binding are the same global. Seeding anyway would silently
// overwrite a `str`-valued dependency binding and would fail the whole
// program with `T0023` for any other type; both compiled before #1156, so
// both must keep compiling to the same values.

#[test]
fn a_dependencys_str_binding_withholds_the_entry_seed() {
    assert_eq!(
        build_and_run_program(
            "dn_dep_str",
            "from dep import helper\nprint(__name__)\nprint(helper())\n",
            "__name__ = \"dep\"\n\ndef helper() -> str:\n    return __name__\n",
        ),
        "dep\ndep\n"
    );
}

#[test]
fn a_dependencys_non_str_binding_withholds_the_entry_seed() {
    assert_eq!(
        build_and_run_program(
            "dn_dep_int",
            "from dep import helper\nprint(__name__)\nprint(helper())\n",
            "__name__ = 7\n\ndef helper() -> int:\n    return __name__\n",
        ),
        "7\n7\n"
    );
}

#[test]
fn a_dependency_function_reads_the_entry_modules_value() {
    // The flat-namespace consequence: the entry module's seed is the
    // program's only `__name__` global, so a dependency's own function reads
    // it. Correct per-module values wait on #881's per-module namespaces.
    let dir = ScratchDir::new("dn_multi_run").expect("scratch");
    std::fs::write(
        dir.join("dep.py"),
        "\
def report() -> None:
    print(__name__)
",
    )
    .expect("write the dependency");
    let src = dir.join("m.py");
    std::fs::write(
        &src,
        "\
from dep import report

report()
print(__name__)
",
    )
    .expect("write the entry module");
    let out = dir.join("m");
    let build = pycc()
        .arg("build")
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .output()
        .expect("pycc should spawn");
    assert!(build.status.success(), "{}", stderr_of(&build));
    let run = Command::new(&out)
        .output()
        .expect("the artifact should spawn");
    assert!(run.status.success(), "{}", stderr_of(&run));
    assert_eq!(stdout_of(&run), "__main__\n__main__\n");
}

#[test]
fn a_dependency_read_is_t0021_when_the_entry_module_never_references_the_name() {
    // No reference in the entry module means Gate 1 withholds the seed, so
    // the program has no `__name__` global at all.
    let (ok, rendered) = check_program(
        "dn_multi_unseeded",
        "from dep import report\n\nreport()\n",
        "\
def report() -> None:
    print(__name__)
",
    );
    assert!(!ok, "{rendered}");
    assert!(
        rendered.contains("error[T0021]") && rendered.contains("dep.py"),
        "{rendered}"
    );
}

#[test]
fn a_dependencys_own_top_level_read_is_t0021() {
    // Linking concatenates dependencies before the entry module, so the
    // entry's seed has not run yet when a dependency's module-level code
    // does — the type checker reports the read against the dependency.
    let (ok, rendered) = check_program(
        "dn_multi_top_level",
        "from dep import report\n\nreport()\nprint(__name__)\n",
        "\
print(__name__)


def report() -> None:
    pass
",
    );
    assert!(!ok, "{rendered}");
    assert!(
        rendered.contains("error[T0021]") && rendered.contains("dep.py"),
        "{rendered}"
    );
}

// -- `--ext` uses the extension module's own name -------------------

#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn an_extension_module_reads_its_own_module_name() {
    let dir = ScratchDir::new("dn_ext").expect("scratch");
    let src = dir.join("m.py");
    std::fs::write(
        &src,
        "\
def modname() -> str:
    return __name__
",
    )
    .expect("write the fixture source");
    let build = pycc()
        .arg("build")
        .arg(&src)
        .arg("-o")
        .arg(dir.join("dunder_probe"))
        .arg("--ext")
        .output()
        .expect("pycc should spawn");
    assert!(build.status.success(), "{}", stderr_of(&build));
    let cwd: &Path = &dir;
    let run = Command::new(std::env::var_os("PYCC_PYTHON").unwrap_or_else(|| "python3".into()))
        .arg("-c")
        .arg(
            "\
import dunder_probe
assert dunder_probe.modname() == \"dunder_probe\", dunder_probe.modname()
assert dunder_probe.modname() == dunder_probe.__name__, dunder_probe.modname()
",
        )
        .current_dir(cwd)
        .output()
        .expect("python3 should spawn");
    assert!(
        run.status.success(),
        "stdout: {}\nstderr: {}",
        stdout_of(&run),
        stderr_of(&run)
    );
}
