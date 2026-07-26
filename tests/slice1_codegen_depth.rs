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
    assert_eq!(
        output.stdout,
        b"0\n1\n1\n2\n3\n5\n8\n13\n21\n34\n55\n"
    );
}

#[test]
fn iterative_fibonacci_overflows_into_a_bigint_and_prints_only_decimal_digits() {
    // `fib(100)` genuinely exceeds `i64::MAX` (19 decimal digits) -- this
    // asserts the *shape* of the result (more digits than `i64::MAX` can
    // hold, an optional leading `-` aside, no digits lost/garbled)
    // rather than a hand-computed 21-digit reference value, which this
    // plan has no way to verify independently without executing Python.
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
    let text = String::from_utf8(output.stdout).expect("output should be valid UTF-8");
    let digits = text.trim_end_matches('\n');
    assert!(
        digits.chars().all(|c| c.is_ascii_digit()),
        "expected only decimal digits, got {digits:?}"
    );
    assert!(
        digits.len() > 19, // i64::MAX ("9223372036854775807") is 19 digits
        "expected a value exceeding i64::MAX's own digit count, got {digits:?}"
    );
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
    // lowered anywhere in `pycc_hir`/`pycc_types`/`pycc_mir`/`pycc_codegen`
    // -- confirmed empirically, `pycc build` panics with "pycc_hir:
    // expression kind not supported yet: UnaryOp(...)" -- and this gap
    // is not tracked in DECISIONS.md, TYPE_SYSTEM.md, or any PR-1..5
    // plan. Rewritten as `0.0 - 2.0` / `0.0 - 1.0`, which is exactly
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
