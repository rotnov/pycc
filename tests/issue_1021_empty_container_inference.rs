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

/// #1021 bot review round: the rewrite walk reaches every `try` suite. Before
/// this, `nested_bodies` had no `Try` arm at all, so an annotated
/// `xs: list[int] = []` inside a `try` body was never visited and reported
/// `T0003` -- contradicting the contract that the annotation source is purely
/// syntactic and always works. All four suites (`try`, `except`, `else`,
/// `finally`) are exercised in one program; the `else` suite additionally
/// uses an *unannotated* literal so the widened producer scan is pinned too.
#[test]
fn empty_literals_inside_every_try_suite_resolve() {
    let output = check_build_and_run(
        "try_suites",
        "\
def f() -> int:
    total = 0
    try:
        xs: list[int] = []
        xs.append(1)
        total = total + len(xs)
    except ValueError:
        ys: list[int] = []
        ys.append(2)
        total = total + len(ys)
    else:
        zs = []
        zs.append(3)
        total = total + len(zs)
    finally:
        ws: list[int] = []
        ws.append(4)
        total = total + len(ws)
    return total

print(f())
",
    );
    assert!(output.status.success());
    assert_eq!(output.stdout, b"3\n");
}

/// The `match` half of the same defect: a `case` body is a nested statement
/// sequence like any other, and both container shapes resolve inside one.
#[test]
fn empty_literals_inside_match_case_bodies_resolve() {
    let output = check_build_and_run(
        "match_cases",
        "\
def f(n: int) -> int:
    match n:
        case 1:
            xs: list[int] = []
            xs.append(1)
            return len(xs)
        case _:
            d: dict[str, int] = {}
            d[\"k\"] = 2
            return len(d)

print(f(1))
print(f(9))
",
    );
    assert!(output.status.success());
    assert_eq!(output.stdout, b"1\n1\n");
}

/// `except*` is a separate `HirStmt` variant with the same field shape, so it
/// shares one arm with `Try` -- and a fix that extended only `Try` would
/// reproduce the defect here. Pinned separately from the `Try` case because
/// the variant, not the arm, is what a future edit can miss.
#[test]
fn empty_literals_inside_an_except_star_suite_resolve() {
    let output = check_build_and_run(
        "try_star_suites",
        "\
def f() -> int:
    total = 0
    try:
        xs: list[int] = []
        xs.append(1)
        total = total + len(xs)
    except* ValueError:
        ys: list[int] = []
        total = total + len(ys)
    return total

print(f())
",
    );
    assert!(output.status.success());
    assert_eq!(output.stdout, b"1\n");
}

/// Review round 4: `nested_bodies` and `rewrite_body` must enumerate a
/// `try`'s suites in the same order, and that order must be source order,
/// because `find_producer` recurses through the former while documenting that
/// it returns the *first syntactic* producer. Both a handler and the `else`
/// suite produce here, with different element types: source order picks the
/// handler's `str` and the program is `T0034`. An inventory that returned
/// `orelse` before the handlers would pick `int` and compile this instead, so
/// the diagnostic is what pins the order.
#[test]
fn a_try_s_suites_are_scanned_for_producers_in_source_order() {
    let rendered = check_error(
        "try_suite_order",
        "\
def f(n: int) -> int:
    xs = []
    try:
        n = n + 1
    except ValueError:
        xs.append(\"a\")
    else:
        xs.append(1)
    return len(xs)

print(f(1))
",
    );
    assert!(
        rendered.contains("error[T0034]: list[str] is not compiled yet"),
        "{rendered}"
    );
}

/// Review round 4: D-245's Consequences section argues that a producer-derived
/// element type can be a `Ty::Param` inside a generic function, and that such
/// a resolution can never reach codegen because `check_container_ty` rejects
/// `list[T]` in the check phase, which runs before `monomorphize`. That is the
/// argument; this is the fixture for it.
#[test]
fn a_producer_derived_type_parameter_element_is_rejected_in_the_check_phase() {
    let rendered = check_error(
        "generic_producer_param",
        "\
def f[T](x: T) -> int:
    xs = []
    xs.append(x)
    return len(xs)

print(f(1))
",
    );
    assert!(
        rendered.contains("error[T0034]: list[T] is not compiled yet"),
        "{rendered}"
    );
}

/// Review round 5 (Codex bot, P2): the empty-container pre-pass rewrites
/// `xs = []` into `EmptyList(Int)`, which the D-146 private-helper constraint
/// solver must carry as the same destructured element-type carrier it already
/// produces for the equivalent `xs = [1]`. Before the fix, the `EmptyList`
/// arm returned no term, `ListPop` found no `Ty::List` to destructure, and
/// this unannotated helper reported `T0021` -- an asymmetry introduced by
/// #1021's own feature, since only the non-empty spelling compiled.
#[test]
fn an_empty_list_in_an_unannotated_private_helper_infers_its_return_type() {
    let output = check_build_and_run(
        "private_helper_empty_list",
        "\
def _pick():
    xs = []
    xs.append(7)
    return xs.pop()

print(_pick())
",
    );
    assert!(output.status.success());
    assert_eq!(output.stdout, b"7\n");
}

/// The carrier above is gated on `is_private_solver_scalar`, exactly as
/// `homogeneous_private_solver_scalar_list_element` gates the `ListLiteral`
/// arm. `EmptyList` hands its element `Ty` over directly with no element terms
/// to check, so a nested `list[list[int]]` element -- which reaches the solver
/// before the D-228 element gate fires -- must keep the historical `Ok(None)`
/// rather than becoming a carrier this scalar-only solver cannot represent.
/// The D-228 gate then reports `T0034` for the annotation itself.
#[test]
fn a_nested_empty_list_element_is_not_carried_by_the_private_helper_solver() {
    let rendered = check_error(
        "private_helper_nested_empty_list",
        "\
def _pick():
    xs: list[list[int]] = []
    return xs.pop()

print(_pick())
",
    );
    assert!(
        rendered.contains("error[T0034]: list[list[int]] is not compiled yet"),
        "{rendered}"
    );
}

/// Review round 5: a producer that reads a module-level global. The pre-pass
/// builds its own module environment, and before the fix that environment
/// carried no module-level bindings at all -- so `xs.append(VALUE)` found
/// `VALUE` unbound and the container stayed unresolved, while the equivalent
/// `xs = [VALUE]` compiled. The pass now seeds the module scope exactly as
/// `check_with_environment_all` does before it checks any function body.
#[test]
fn a_module_level_global_resolves_an_empty_list_producer() {
    let output = check_build_and_run(
        "global_producer_empty_list",
        "\
VALUE = 1


def _pick() -> int:
    xs = []
    xs.append(VALUE)
    return xs.pop()

print(_pick())
",
    );
    assert!(output.status.success());
    assert_eq!(output.stdout, b"1\n");
}

/// The dict arm of the same seeding, through a global used as the key. `{}`
/// carries two element types, and the key's own source is the module scope
/// rather than the enclosing function.
#[test]
fn a_module_level_global_resolves_an_empty_dict_producer() {
    let output = check_build_and_run(
        "global_producer_empty_dict",
        "\
KEY = \"k\"


def _pick() -> int:
    d = {}
    d[KEY] = 1
    return d[KEY]

print(_pick())
",
    );
    assert!(output.status.success());
    assert_eq!(output.stdout, b"1\n");
}

/// A global initialized from an annotated helper. This is what forced the
/// pre-pass off `concrete_function_environment`, which refuses the whole
/// module the moment any signature carries `Ty::Infer` -- true here, because
/// `_pick` is exactly the unannotated private helper the feature serves. The
/// pass now registers the annotated signatures alone, in source order so a
/// `def` is callable only from its own position onward.
#[test]
fn a_global_initialized_from_an_annotated_helper_resolves_an_empty_list() {
    let output = check_build_and_run(
        "global_call_producer_empty_list",
        "\
def _base() -> int:
    return 3


VALUE = _base()


def _pick():
    xs = []
    xs.append(VALUE)
    return xs.pop()

print(_pick())
",
    );
    assert!(output.status.success());
    assert_eq!(output.stdout, b"3\n");
}

/// The rejection half of the same property. A `str` global resolves the
/// container to `list[str]`, which D-228's shared element gate rejects with
/// `T0034` -- byte for byte the diagnostic the equivalent `xs = [VALUE]`
/// already produces. Seeding the module scope must make the two spellings
/// agree on rejection, not only on success.
#[test]
fn a_str_global_producer_is_rejected_exactly_as_the_non_empty_spelling_is() {
    let rendered = check_error(
        "global_producer_str_element",
        "\
VALUE = \"s\"


def _pick() -> str:
    xs = []
    xs.append(VALUE)
    return xs.pop()

print(_pick())
",
    );
    assert!(
        rendered.contains("error[T0034]: list[str] is not compiled yet"),
        "{rendered}"
    );
    let non_empty = check_error(
        "global_producer_str_element_non_empty",
        "\
VALUE = \"s\"


def _pick() -> str:
    xs = [VALUE]
    return xs.pop()

print(_pick())
",
    );
    assert!(
        non_empty.contains("error[T0034]: list[str] is not compiled yet"),
        "{non_empty}"
    );
}

/// Review round 5, second finding: the pre-pass binder must keep D-040's
/// sticky representation. `v = 5` then `v = True` leaves `v` an `int` for the
/// checker -- a compatible reassignment returns without rebinding -- so the
/// producer resolves `list[int]`. A binder that overwrote the recorded type
/// resolved `list[bool]` instead and D-228 reported a `T0034` naming a type
/// the source never mentions, for a program whose non-empty spelling
/// compiles. Both spellings are asserted here, because the property is
/// agreement, not a particular element type.
#[test]
fn a_compatible_reassignment_keeps_the_first_inferred_element_type() {
    let output = check_build_and_run(
        "sticky_reassignment_empty_list",
        "\
def f() -> int:
    v = 5
    v = True
    xs = []
    xs.append(v)
    return xs.pop()

print(f())
",
    );
    assert!(output.status.success());
    assert_eq!(output.stdout, b"True\n");
    let non_empty = check_build_and_run(
        "sticky_reassignment_non_empty_list",
        "\
def f() -> int:
    v = 5
    v = True
    xs = [v]
    return xs.pop()

print(f())
",
    );
    assert!(non_empty.status.success());
    assert_eq!(non_empty.stdout, output.stdout);
}

/// Review round 5, third finding: a `def` that shadows an earlier value of
/// the same name is callable from its own position onward (D-110), and the
/// pre-pass's module-scope walk must record that in `def_rebound` as well as
/// `defined_functions`. Seeding only the latter left the pre-pass's own
/// callee gate treating `helper` as value-shadowed, so the producer's call
/// did not infer and the container fell through to a spurious `T0003` -- for
/// a program the checker itself accepts, as the non-empty spelling shows.
#[test]
fn a_def_shadowing_an_earlier_value_binding_resolves_a_producer_call() {
    let output = check_build_and_run(
        "def_rebound_producer_empty_list",
        "\
helper = 1


def helper() -> int:
    return 2


def f() -> int:
    xs = []
    xs.append(helper())
    return xs.pop()

print(f())
",
    );
    assert!(output.status.success());
    assert_eq!(output.stdout, b"2\n");
}
