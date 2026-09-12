// Issue #1021 (D-245): an empty `[]` / `{}` gets its element type from a
// pre-check HIR-to-HIR pass, end to end through the real `pycc build` /
// `pycc check` CLI -- parser -> hir -> types -> mir -> codegen -> link -> run.
//
// Every positive case below is built and *run*, because the element type has
// to survive into MIR: `MirExpr::ty()` derives a container's type from its
// first element and panics on an empty literal, so a resolution that stopped
// at `pycc_types` would type-check and then abort the compiler. The negative
// and unresolved cases live in `tests/diagnostics/t0003_*` and
// `tests/diagnostics/t003{4,6}_inferred_*`.
//
// Every expected stdout below was verified against CPython 3.14 on the same
// source.

use pycc_scratch::ScratchDir;
use std::io::Write;
use std::process::Command;

fn pycc_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_pycc"))
}

fn write_source(label: &str, source: &str) -> (ScratchDir, std::path::PathBuf) {
    let dir =
        ScratchDir::new(&format!("issue_1021_{label}")).expect("failed to create scratch dir");
    let path = dir.join(format!("{label}.py"));
    let mut file = std::fs::File::create(&path).unwrap();
    file.write_all(source.as_bytes()).unwrap();
    (dir, path)
}

/// Builds and runs `source`, and additionally asserts that `pycc check`
/// accepts it. The two entry points (`check_all_keyed` and
/// `check_and_resolve_all_keyed`) each run the resolution pass themselves, so
/// every positive case pins that they agree: `pycc check` must never accept
/// what `pycc build` rejects, nor the reverse.
fn check_build_and_run(label: &str, source: &str) -> std::process::Output {
    let (dir, path) = write_source(label, source);
    let checked = Command::new(pycc_bin())
        .args(["check", path.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(
        checked.status.code(),
        Some(0),
        "`pycc check` rejected {label}: {}",
        String::from_utf8_lossy(&checked.stdout)
    );
    let out = dir.join(label);
    let status = Command::new(pycc_bin())
        .args(["build", path.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(status.success(), "`pycc build` failed for {label}");
    Command::new(&out).output().unwrap()
}

/// Runs `pycc check` on `source` and returns its rendered diagnostic output,
/// asserting the exit status really is the compile-error one. Also asserts
/// that `pycc build` agrees it is an error, which is the check-phase /
/// resolve-phase agreement property in its negative direction.
fn check_error(label: &str, source: &str) -> String {
    let (dir, path) = write_source(label, source);
    let output = Command::new(pycc_bin())
        .args(["check", path.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(1),
        "{label} should be a compile error"
    );
    let out = dir.join(label);
    let built = Command::new(pycc_bin())
        .args(["build", path.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(
        built.status.code(),
        Some(1),
        "`pycc build` disagreed with `pycc check` on {label}"
    );
    String::from_utf8(output.stdout).expect("diagnostics are UTF-8")
}

/// The motivating case of #927/#1021: the annotation on the assignment is the
/// element type. This path needs no environment at all -- it is purely
/// syntactic -- which is why it works even in a module whose signatures
/// cannot be resolved concretely.
#[test]
fn an_annotated_empty_list_takes_its_element_type_from_the_annotation() {
    let output = check_build_and_run(
        "annotated_list",
        "\
def f() -> int:
    xs: list[int] = []
    xs.append(1)
    xs.append(2)
    return len(xs)

print(f())
",
    );
    assert!(output.status.success());
    assert_eq!(output.stdout, b"2\n");
}

/// The dict half of the same path, with a `d[k] = v` producer that is a
/// statement (`HirStmt::DictSet`) rather than an expression.
#[test]
fn an_annotated_empty_dict_takes_its_types_from_the_annotation() {
    let output = check_build_and_run(
        "annotated_dict",
        "\
def f() -> int:
    d: dict[str, int] = {}
    d[\"a\"] = 1
    d[\"b\"] = 2
    return len(d)

print(f())
",
    );
    assert!(output.status.success());
    assert_eq!(output.stdout, b"2\n");
}

/// No annotation anywhere: the element type comes from a forward scan of the
/// function body for the first *producer* use. `ListAppend` is an expression
/// nested inside a statement, so the scan has to walk expressions.
#[test]
fn an_unannotated_empty_list_resolves_from_a_later_append() {
    let output = check_build_and_run(
        "forward_append",
        "\
def f() -> int:
    xs = []
    xs.append(3)
    xs.append(4)
    total = 0
    for x in xs:
        total = total + x
    return total

print(f())
",
    );
    assert!(output.status.success());
    assert_eq!(output.stdout, b"7\n");
}

/// The dict forward-scan case: `d[k] = v` is the producer.
#[test]
fn an_unannotated_empty_dict_resolves_from_a_later_subscript_assignment() {
    let output = check_build_and_run(
        "forward_dict_set",
        "\
def f() -> int:
    d = {}
    d[\"k\"] = 5
    return d[\"k\"]

print(f())
",
    );
    assert!(output.status.success());
    assert_eq!(output.stdout, b"5\n");
}

/// Resolution from another binding of the same name: the environment already
/// holds `list[int]` for `xs` when the empty literal re-assigns it, so that
/// binding supplies the element type. D-040's sticky-representation rule
/// (`T0023`) still applies -- the re-assignment keeps the same
/// representation, not merely the same `Ty`.
#[test]
fn an_empty_list_reassignment_resolves_from_the_existing_binding() {
    let output = check_build_and_run(
        "backward_binding",
        "\
def f() -> int:
    xs = [1, 2, 3]
    xs = []
    xs.append(9)
    return len(xs)

print(f())
",
    );
    assert!(output.status.success());
    assert_eq!(output.stdout, b"1\n");
}

/// `for x in xs` over a list that is still empty at runtime. Before #1021 the
/// codegen comment at this loop called the empty case unreachable from real
/// source *because* `pycc_types` rejected an empty list literal; that premise
/// no longer holds, so the zero-iteration path is pinned here.
#[test]
fn iterating_an_inferred_empty_list_runs_zero_iterations() {
    let output = check_build_and_run(
        "empty_for",
        "\
def f() -> int:
    xs: list[int] = []
    total = 0
    for x in xs:
        total = total + x
    return total

print(f())
",
    );
    assert!(output.status.success());
    assert_eq!(output.stdout, b"0\n");
}

/// Branch collision resolves first-wins within the scope: both `[]` nodes take
/// the *first* producer's type, and the second branch's `append` then fails
/// with the ordinary element-type mismatch rather than resolving to a second,
/// conflicting type. A name-keyed resolution cannot represent two types for
/// one binding, and neither can the binding itself.
#[test]
fn a_branch_collision_resolves_first_wins_and_the_loser_reports_a_mismatch() {
    let rendered = check_error(
        "branch_collision",
        "\
def f(n: int) -> int:
    if n > 0:
        xs = []
        xs.append(1)
    else:
        xs = []
        xs.append(\"a\")
    return len(xs)

print(f(1))
",
    );
    assert!(
        rendered.contains("error[T0021]: cannot append `str` to a list of `int`"),
        "{rendered}"
    );
}

/// An empty literal in a module with no empty containers at all must not pay
/// for the pass: the pre-scan returns early and the module is never cloned.
/// Observable only as behavior preservation, pinned here so a regression that
/// rewrote unrelated modules would show up as a changed result.
#[test]
fn a_module_with_no_empty_container_is_unaffected() {
    let output = check_build_and_run(
        "no_empty",
        "\
def f() -> int:
    xs = [1, 2]
    xs.append(3)
    return len(xs)

print(f())
",
    );
    assert!(output.status.success());
    assert_eq!(output.stdout, b"3\n");
}

/// The rewrite recurses into every block statement the pass models, not only
/// `if`: an empty literal inside a `while` body resolves from a producer in
/// that same body, and the surrounding `for ... in range(...)` body is walked
/// on the same pass. `for x in <list>` is covered by
/// `iterating_an_inferred_empty_list_runs_zero_iterations` above.
#[test]
fn empty_literals_inside_while_and_for_range_bodies_resolve() {
    let output = check_build_and_run(
        "loop_bodies",
        "\
def f() -> int:
    total = 0
    n = 0
    while n < 2:
        xs = []
        xs.append(3)
        total = total + len(xs)
        n = n + 1
    for _i in range(2):
        d = {}
        d[\"k\"] = 1
        total = total + len(d)
    return total

print(f())
",
    );
    assert!(output.status.success());
    assert_eq!(output.stdout, b"4\n");
}

/// Source 2 is *order-insensitive*, not a backward scan: the whole-function
/// environment is completed before any rewriting, so a binding that appears
/// *after* the empty literal resolves it just as one before it would. This is
/// safe rather than merely convenient -- `check_container_ty` admits only
/// `list[int]`/`dict[str, int]`, and the check phase re-validates in true
/// program order -- and is pinned here because the module doc and D-245 now
/// describe exactly this behaviour.
#[test]
fn an_empty_list_resolves_from_a_later_binding_of_the_same_name() {
    let output = check_build_and_run(
        "later_binding",
        "\
def f() -> int:
    xs = []
    xs = [1]
    return xs[0]

print(f())
",
    );
    assert!(output.status.success());
    assert_eq!(output.stdout, b"1\n");
}

/// #1021 review round 1: an annotated empty container seeds the pre-pass
/// environment from its *annotation*. `ys`'s element type comes from `x`,
/// whose own type comes from `xs`'s binding -- and before the fix
/// `xs: list[int] = []` left `xs` unbound (its raw literal cannot infer
/// before the pass rewrites it), so `x` was unbound too and `ys = []` fell
/// through to a spurious `T0003`.
#[test]
fn an_annotated_empty_list_seeds_the_environment_for_a_dependent_resolution() {
    let output = check_build_and_run(
        "annotation_seeds_env",
        "\
def f() -> int:
    xs: list[int] = []
    xs.append(7)
    ys = []
    for x in xs:
        ys.append(x)
    return len(ys)

print(f())
",
    );
    assert!(output.status.success());
    assert_eq!(output.stdout, b"1\n");
}
