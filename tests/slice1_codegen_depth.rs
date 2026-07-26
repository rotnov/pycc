use pycc_mir::{BinOpKind, MirExpr, MirItem, MirModule, MirStmt, Ty};
use std::io::Write;
use std::process::Command;

fn pycc_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_pycc"))
}

fn write_fixture(dir: &std::path::Path, name: &str, source: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(source.as_bytes()).unwrap();
    path
}

fn build_and_run(label: &str, source: &str) -> std::process::Output {
    let dir = std::env::temp_dir().join(format!("pycc_slice1_{label}_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, &format!("{label}.py"), source);
    let out = dir.join(label);
    let status = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(status.success(), "`pycc build` failed for {label}");
    Command::new(&out).output().unwrap()
}

fn compile_mir(label: &str, mir: &MirModule) -> Result<(), String> {
    let dir = std::env::temp_dir().join(format!("pycc_slice1_mir_{label}_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    pycc_codegen::compile_to_object(mir, &dir.join(format!("{label}.o")), None)
}

#[test]
fn public_codegen_api_covers_float_runtime_ops_and_parameter_reassignment() {
    let mir = MirModule {
        items: vec![
            MirItem::Function {
                name: "increment".to_string(),
                params: vec![("value".to_string(), Ty::Int)],
                return_ty: Ty::Int,
                body: vec![
                    MirStmt::Assign {
                        target: "value".to_string(),
                        value: MirExpr::BinOp {
                            op: BinOpKind::Add,
                            left: Box::new(MirExpr::Name {
                                name: "value".to_string(),
                                ty: Ty::Int,
                            }),
                            right: Box::new(MirExpr::IntLiteral(1)),
                            ty: Ty::Int,
                        },
                    },
                    MirStmt::Return(Some(MirExpr::Name {
                        name: "value".to_string(),
                        ty: Ty::Int,
                    })),
                ],
            },
            MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                callee: "print".to_string(),
                args: vec![MirExpr::BinOp {
                    op: BinOpKind::FloorDiv,
                    left: Box::new(MirExpr::FloatLiteral(5.5)),
                    right: Box::new(MirExpr::FloatLiteral(2.0)),
                    ty: Ty::Float,
                }],
                ty: Ty::None,
            })),
        ],
    };

    compile_mir("public_success_paths", &mir).expect("public codegen paths should compile");
}

#[test]
#[should_panic(expected = "has no local slot")]
fn public_codegen_api_rejects_a_name_without_storage() {
    let mir = MirModule {
        items: vec![MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
            callee: "print".to_string(),
            args: vec![MirExpr::Name {
                name: "missing".to_string(),
                ty: Ty::Int,
            }],
            ty: Ty::None,
        }))],
    };
    let _ = compile_mir("missing_storage", &mir);
}

#[test]
#[should_panic(expected = "call to undefined function")]
fn public_codegen_api_rejects_an_undefined_nested_call() {
    let mir = MirModule {
        items: vec![MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
            callee: "print".to_string(),
            args: vec![MirExpr::Call {
                callee: "missing".to_string(),
                args: vec![],
                ty: Ty::Int,
            }],
            ty: Ty::None,
        }))],
    };
    let _ = compile_mir("undefined_nested_call", &mir);
}

#[test]
#[should_panic(expected = "an f-string with zero parts should not be reachable")]
fn public_codegen_api_rejects_an_empty_structural_fstring() {
    let mir = MirModule {
        items: vec![MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
            callee: "print".to_string(),
            args: vec![MirExpr::FString(vec![])],
            ty: Ty::None,
        }))],
    };
    let _ = compile_mir("empty_structural_fstring", &mir);
}

#[test]
fn public_codegen_api_returns_an_error_for_an_undefined_void_call() {
    let mir = MirModule {
        items: vec![MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
            callee: "missing".to_string(),
            args: vec![],
            ty: Ty::None,
        }))],
    };
    let error = compile_mir("undefined_void_call", &mir).expect_err("the call should fail");
    assert!(error.contains("missing"));
}

#[test]
fn public_codegen_api_propagates_an_error_from_a_function_body() {
    let mir = MirModule {
        items: vec![MirItem::Function {
            name: "broken".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![MirStmt::ExprStmt(MirExpr::Call {
                callee: "missing".to_string(),
                args: vec![],
                ty: Ty::None,
            })],
        }],
    };
    let error = compile_mir("undefined_void_call_in_function", &mir)
        .expect_err("the function body should fail");
    assert!(error.contains("missing"));
}

#[test]
#[should_panic(expected = "declared to return a non-`None` value but fell through")]
fn public_codegen_api_rejects_a_non_none_function_that_falls_through() {
    let mir = MirModule {
        items: vec![MirItem::Function {
            name: "broken".to_string(),
            params: vec![],
            return_ty: Ty::Int,
            body: vec![],
        }],
    };
    let _ = compile_mir("non_none_fallthrough", &mir);
}

#[test]
#[should_panic(expected = "a top-level statement terminated `main`'s entry block")]
fn public_codegen_api_rejects_a_top_level_return() {
    let mir = MirModule {
        items: vec![MirItem::TopLevelStmt(MirStmt::Return(Some(
            MirExpr::IntLiteral(0),
        )))],
    };
    let _ = compile_mir("top_level_return", &mir);
}

#[test]
fn recursive_fibonacci_matches_the_well_known_sequence() {
    let source = "\
def fib(n: int) -> int:
    if n < 2:
        return n
    return fib(n - 1) + fib(n - 2)

i = 0
while i < 11:
    print(fib(i))
    i = i + 1
";
    let output = build_and_run("fib_recursive", source);
    assert!(output.status.success());
    assert_eq!(output.stdout, b"0\n1\n1\n2\n3\n5\n8\n13\n21\n34\n55\n");
}

#[test]
fn iterative_fibonacci_overflows_into_a_bigint_and_prints_only_decimal_digits() {
    // `fib(100)` genuinely exceeds `i64::MAX` (19 decimal digits).
    // 354224848179261915075 is independently verified (`python3`'s own
    // iterative fibonacci, not this compiler's bigint path), so this
    // asserts the exact mathematical value rather than only its shape.
    let source = "\
def fib_iter(n: int) -> int:
    a = 0
    b = 1
    i = 0
    while i < n:
        temp = a + b
        a = b
        b = temp
        i = i + 1
    return a

print(fib_iter(100))
";
    let output = build_and_run("fib_iterative_bigint", source);
    assert!(output.status.success());
    assert_eq!(output.stdout, b"354224848179261915075\n");
}

#[test]
fn a_power_expression_links_and_runs_through_the_real_cli_needing_libm() {
    // Regression coverage for `src/main.rs`'s Linux-only `add_linux_system_libs`
    // fix: `pycc_rt_float_pow` calls into `libm`'s `pow`, which only resolves
    // at link time through the real `pycc build` CLI path (unlike
    // `pycc_codegen`'s own in-process `compile_to_object` tests, which never
    // invoke the host linker at all).
    let source = "\
print(2 ** 10)
print(2.0 ** 0.5)
";
    let output = build_and_run("power_expression_libm", source);
    assert!(output.status.success());
    assert_eq!(output.stdout, b"1024\n1.4142135623730951\n");
}

#[test]
fn float_and_str_ge_and_ne_execute_with_the_correct_boolean_value() {
    // `pycc_codegen`'s own `compiles_the_remaining_float_comparison_operators`/
    // `compiles_the_remaining_string_comparison_operators` unit tests only
    // prove the generated IR compiles -- they never link, run, or check a
    // value, so a predicate swap (e.g. `FloatPredicate::OGE` -> `OGT`, or the
    // analogous string-branch bug) would still pass every existing test.
    // This goes through the real `pycc` CLI and asserts actual values
    // (verified against `python3` on this exact source).
    let source = "\
print(2.5 >= 2.5)
print(2.5 >= 2.6)
print(2.5 != 2.5)
print(2.5 != 2.6)
print(\"b\" >= \"b\")
print(\"b\" >= \"c\")
print(\"b\" != \"b\")
print(\"b\" != \"c\")
";
    let output = build_and_run("float_str_ge_ne_values", source);
    assert!(output.status.success());
    assert_eq!(
        output.stdout,
        b"True\nFalse\nFalse\nTrue\nTrue\nFalse\nFalse\nTrue\n"
    );
}

#[test]
fn multiplication_promotes_and_float_floor_division_matches_cpython() {
    let source = "\
print(3000000000 * 3000000000)
print(1.0 // 0.1)
";
    let output = build_and_run("numeric_runtime_edges", source);
    assert!(output.status.success());
    assert_eq!(output.stdout, b"9000000000000000000\n9.0\n");
}

#[test]
fn true_division_by_zero_fails_explicitly() {
    let output = build_and_run("float_division_by_zero", "print(1 / 0)\n");
    assert!(
        !output.status.success(),
        "true division by zero must fail instead of producing infinity"
    );
}

#[test]
fn a_floor_division_quotient_outside_the_tagged_range_promotes() {
    let source = "\
minimum = 0 - 4611686018427387903 - 1
print(minimum // (0 - 1))
";
    let output = build_and_run("floor_div_promotes", source);
    assert!(output.status.success());
    assert_eq!(output.stdout, b"4611686018427387904\n");
}

#[test]
fn a_float_power_requiring_an_exception_fails_explicitly() {
    let source = "print(0.0 ** (0.0 - 1.0))\n";
    let output = build_and_run("float_pow_zero_negative", source);
    assert!(
        !output.status.success(),
        "zero to a negative float power must not silently produce infinity"
    );
}

#[test]
fn non_finite_float_power_domains_preserve_real_python_results() {
    let source = "\
print((0.0 - 1.0) ** 1e309)
print((0.0 - 1e309) ** 0.5)
print(0.0 ** (0.0 - 1e309))
";
    let output = build_and_run("float_pow_non_finite", source);
    assert!(output.status.success());
    assert_eq!(output.stdout, b"1.0\ninf\ninf\n");
}

#[test]
fn fizzbuzz_exercises_int_arithmetic_modulo_elif_chains_and_mixed_print_types() {
    let source = "\
def fizzbuzz(n: int) -> None:
    i = 1
    while i <= n:
        if i % 15 == 0:
            print(\"FizzBuzz\")
        elif i % 3 == 0:
            print(\"Fizz\")
        elif i % 5 == 0:
            print(\"Buzz\")
        else:
            print(i)
        i = i + 1

fizzbuzz(15)
";
    let output = build_and_run("fizzbuzz", source);
    assert!(output.status.success());
    assert_eq!(
        output.stdout,
        b"1\n2\nFizz\n4\nBuzz\nFizz\n7\n8\nFizz\nBuzz\n11\nFizz\n13\n14\nFizzBuzz\n"
    );
}

#[test]
fn a_for_loop_over_range_binds_its_variable_as_int_and_iterates_the_expected_values() {
    // Post-review follow-up (fix round, same shape as the f-string gap
    // caught in the original post-review pass): `docs/ROADMAP.md`'s
    // "Compiler pipeline" and "Language surface" rows cite `tests/slice0.rs`
    // and this file together as proving `if`/`while`/`for`+`range` end to
    // end in `build`/`run`, but neither file previously contained a single
    // `for ... in range(...)` fixture (`tests/slice0.rs`'s two `for` hits
    // are Rust harness loops, not compiled Python source). `for`/`range` is
    // genuinely implemented end to end (`pycc_hir` -> `pycc_types` ->
    // `pycc_mir` -> `pycc_codegen`, each with its own unit tests), so this
    // closes the citation gap rather than adding a new feature. Exercises
    // both the one-argument (`range(stop)`, implicit `start=0`/`step=1`)
    // and two-argument (`range(start, stop)`) forms.
    let source = "\
total = 0
for i in range(5):
    total = total + i
print(total)
for j in range(1, 4):
    print(j)
";
    let output = build_and_run("for_range_basic", source);
    assert!(output.status.success());
    assert_eq!(output.stdout, b"10\n1\n2\n3\n");
}

#[test]
fn a_basic_f_string_interpolates_a_str_and_an_int_local_between_literal_parts() {
    // Not in the plan brief's own Step 2-5 list, but this file's own task
    // description ("recursive functions, control flow, arithmetic, strings,
    // f-strings") and this task's `docs/ROADMAP.md` update both name
    // f-strings as part of what `tests/slice1_codegen_depth.rs` proves --
    // without a real f-string fixture that citation would be an overclaim.
    // Exercises `HirExpr::FString`/`MirExpr::FString` end to end (parser →
    // HIR → types → MIR → LLVM), plus `print` accepting a `str`-typed
    // argument built from f-string interpolation (Task 8/Task 10's join
    // point).
    let source = "\
n = 7
label = \"answer\"
print(f\"{label} = {n}\")
";
    let output = build_and_run("fstring_basic", source);
    assert!(output.status.success());
    assert_eq!(output.stdout, b"answer = 7\n");
}

#[test]
fn a_multi_argument_print_space_separates_mixed_str_int_float_and_bool_values() {
    // Second fix-round finding (same overclaim class as the `for`/`range`
    // gap fixed above): `docs/ROADMAP.md`'s "Compiler pipeline" row cites
    // this file (jointly with `tests/slice0.rs`) as proving "type-aware
    // multi-argument `print`", and the "Language surface" row claims
    // `print` for every v0.1 scalar type (`int`/`float`/`bool`/`str`) --
    // but every `print(...)` call in both files was single-argument, and
    // neither file ever printed a `float` or a `bool`. That left the
    // multi-argument space-separator path (`pycc_rt_print_space`, emitted
    // between `print` arguments) and the float/bool `to_str` paths
    // (`pycc_rt_float_to_str`/`pycc_rt_bool_to_str`) with zero e2e coverage
    // even though the row asserts all of them work. All are genuinely
    // implemented (`emit_print_arg`'s own doc comment: "any number of
    // int/float/bool/str arguments"), so this closes a missing e2e
    // citation, not a missing feature -- one mixed-type fixture covering
    // every v0.1 scalar type at once rather than a separate test per type.
    // Exact expected output verified directly against `pycc build`/run
    // before being added here: Rust's `f64` `Display` (which
    // `pycc_rt_float_to_str` uses) renders `2.5` as `"2.5"`, matching
    // CPython's own `str(2.5)`, and `pycc_rt_bool_to_str` renders `True`
    // capitalized, matching CPython's own `str(True)`.
    let source = "\
print(\"x\", 1, 2.5, True)
";
    let output = build_and_run("print_multi_arg_mixed_types", source);
    assert!(output.status.success());
    assert_eq!(output.stdout, b"x 1 2.5 True\n");
}

#[test]
fn backend_representation_boundaries_match_the_checked_v0_1_contract() {
    let source = "\
def read_later_global() -> int:
    return later_global

def read_later_label() -> str:
    return later_label

def branch_local(flag: bool) -> int:
    if flag:
        value = 1
    else:
        value = 2
    return value

def accepts_int(value: int) -> int:
    return value

def both_branches_return(flag: bool) -> int:
    if flag:
        return True
    else:
        return False

def returns_none() -> None:
    return

def local_shadows_global() -> int:
    shadowed = 4
    return shadowed

def reassigns_parameter(value: int) -> int:
    value = value + 1
    return value

later_global = 5
later_label = \"global\"
shadowed = 9
print(read_later_global())
print(read_later_label())
print(branch_local(False))
print(accepts_int(True))
print(both_branches_return(False))
print(local_shadows_global())
print(shadowed)
print(reassigns_parameter(10))
for i in range(False, 3, True):
    print(i)
counter = 10
counter = True
print(counter)
print(f\"{returns_none()}\")
";
    let output = build_and_run("backend_representation_boundaries", source);
    assert!(output.status.success());
    // Three of these lines are a documented v0.1 deviation from CPython, not
    // the correct value: `accepts_int(True)` (4th line), `both_branches_
    // return(False)` (5th line), and `counter` after being reassigned `True`
    // (12th line) print "1"/"0"/"1" here, where real CPython (verified via
    // `python3` on this exact source) prints "True"/"False"/"True" -- once a
    // `bool` crosses an `int`-typed boundary its runtime representation
    // becomes an ordinary tagged int with no bit left to recover that it was
    // ever a `bool` (see docs/ROADMAP.md's "Language surface" known-gaps
    // list, D-061/D-074). This assertion pins pycc's actual current output,
    // not CPython's -- it is not itself evidence the divergent lines are
    // correct.
    assert_eq!(
        output.stdout,
        b"5\nglobal\n2\n1\n0\n4\n9\n11\n0\n1\n2\n1\nNone\n"
    );
}

#[test]
fn calling_a_function_before_its_integer_global_is_initialized_fails() {
    let source = "\
def read_later_global() -> int:
    return later_global

later_global = read_later_global()
";
    let output = build_and_run("uninitialized_integer_global", source);
    assert!(
        !output.status.success(),
        "an LLVM initializer must never become a fabricated Python int"
    );
}

#[test]
fn calling_a_function_before_its_string_global_is_initialized_fails_safely() {
    let source = "\
def read_later_label() -> str:
    return later_label

captured = read_later_label()
later_label = \"global\"
";
    let output = build_and_run("uninitialized_string_global", source);
    assert!(
        !output.status.success(),
        "an uninitialized string global must trap before a null runtime dereference"
    );
}

#[test]
fn a_maybe_bound_integer_local_fails_before_loading_undefined_storage() {
    let source = "\
def read_value(flag: bool) -> int:
    if flag:
        value = 1
    return value

captured = read_value(False)
";
    let output = build_and_run("maybe_bound_integer_local", source);
    assert!(
        !output.status.success(),
        "a skipped local assignment must not expose LLVM undef"
    );
}

#[test]
fn a_maybe_bound_string_local_fails_before_returning_null() {
    let source = "\
def read_label(flag: bool) -> str:
    if flag:
        label = \"ready\"
    return label

captured = read_label(False)
";
    let output = build_and_run("maybe_bound_string_local", source);
    assert!(
        !output.status.success(),
        "a skipped string assignment must trap before null reaches a caller"
    );
}

#[test]
fn an_empty_range_leaves_a_new_target_unbound() {
    let source = "\
for item in range(0):
    print(item)
print(item)
";
    let output = build_and_run("empty_range_target", source);
    assert!(
        !output.status.success(),
        "an empty range must not fabricate a visible target value"
    );
}

#[test]
fn range_targets_keep_the_last_element_and_ignore_body_reassignment_for_iteration() {
    let source = "\
empty = 7
for empty in range(0):
    print(empty)
print(empty)
for i in range(3):
    print(i)
print(i)
for j in range(3, 0, 0 - 1):
    print(j)
print(j)
for k in range(3):
    print(k)
    k = 99
print(k)
";
    let output = build_and_run("range_target_lifetime", source);
    assert!(output.status.success());
    assert_eq!(output.stdout, b"7\n0\n1\n2\n2\n3\n2\n1\n1\n0\n1\n2\n99\n");
}

#[test]
fn mandelbrot_ascii_produces_a_grid_of_the_expected_dimensions_and_palette() {
    // A first-cut, deliberately small (20x40) rendering exercising
    // nested `while` loops, `float` arithmetic (including true
    // division), a cascading `if`/`elif`/`else` shade lookup, `str`
    // concatenation building a line character by character, and a
    // recursion-free numeric function. Not a byte-exact CPython
    // differential -- that is `pycc_testkit`'s job (PR-6, per
    // DELIVERY_PLAN.md); this only proves the full v0.1 feature
    // combination compiles and runs to a plausible result.
    //
    // Deviation from the plan brief: the brief's fixture wrote the
    // plane offsets as unary-minus literals (`-2.0`, `-1.0`). Unary
    // operators (`Expr::UnaryOp` / `USub`/`UAdd`/`Not`/`Invert`) are not
    // lowered by the implemented HIR subset: the checked lowering path now
    // reports a spanned `C0001` capability diagnostic before MIR/codegen.
    // Rewritten as `0.0 - 2.0` / `0.0 - 1.0`, which is exactly
    // semantically equivalent (Python's left-to-right `+`/`-` makes
    // `0.0 - 2.0 + x` == `-2.0 + x`) and already exercised by this same
    // fixture's `x2 - y2` subtraction. See this task's report for the
    // corresponding `docs/ROADMAP.md` known-gaps addition.
    let source = "\
def mandel_escape(cx: float, cy: float, max_iter: int) -> int:
    x = 0.0
    y = 0.0
    i = 0
    while i < max_iter:
        x2 = x * x
        y2 = y * y
        if x2 + y2 > 4.0:
            return i
        y = 2.0 * x * y + cy
        x = x2 - y2 + cx
        i = i + 1
    return max_iter

def shade_char(level: int) -> str:
    if level <= 0:
        return \" \"
    if level == 1:
        return \".\"
    if level == 2:
        return \":\"
    if level == 3:
        return \"-\"
    if level == 4:
        return \"=\"
    if level == 5:
        return \"+\"
    if level == 6:
        return \"*\"
    if level == 7:
        return \"#\"
    if level == 8:
        return \"%\"
    return \"@\"

height = 20
width = 40
max_iter = 20
row = 0
while row < height:
    line = \"\"
    col = 0
    while col < width:
        cx = 0.0 - 2.0 + (col / width) * 3.0
        cy = 0.0 - 1.0 + (row / height) * 2.0
        iters = mandel_escape(cx, cy, max_iter)
        level = (iters * 9) // max_iter
        line = line + shade_char(level)
        col = col + 1
    print(line)
    row = row + 1
";
    let output = build_and_run("mandelbrot_ascii", source);
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).expect("output should be valid UTF-8");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 20, "expected exactly `height` printed lines");
    let palette: &[char] = &[' ', '.', ':', '-', '=', '+', '*', '#', '%', '@'];
    for (row_index, line) in lines.iter().enumerate() {
        assert_eq!(
            line.chars().count(),
            40,
            "row {row_index} should be exactly `width` characters wide"
        );
        assert!(
            line.chars().all(|c| palette.contains(&c)),
            "row {row_index} contained a character outside the shading palette: {line:?}"
        );
    }
}
