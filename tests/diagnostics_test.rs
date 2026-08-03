use std::path::Path;
use std::process::Command;

fn pycc_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_pycc"))
}

/// `pycc check` embeds whatever `path` string it was invoked with verbatim
/// into its diagnostic output (`render_human`'s ` --> {file_path}:...`
/// line, `render_json`'s `"file"` field). If this harness passed an
/// absolute path (e.g. one built from `CARGO_MANIFEST_DIR`), the checked-in
/// `.expected.txt` fixtures would bake in a machine-specific checkout path,
/// failing on every other machine and violating DIAGNOSTICS.md's
/// byte-identical-across-platforms bar. Instead, `pycc` is invoked with its
/// `current_dir` set to the repo root and a repo-relative, forward-slash
/// path literal (stable on every OS -- this exact string is what gets
/// embedded, not a `PathBuf`'s platform-dependent `Display`).
fn assert_diagnostic_matches_fixture(fixture_stem: &str) {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let expected_path = repo_root
        .join("tests/diagnostics")
        .join(format!("{fixture_stem}.expected.txt"));
    // pycc's diagnostic renderer always emits `\n` line endings, matching
    // DIAGNOSTICS.md's byte-identical-across-platforms bar. Git on Windows
    // checks these fixtures out with `\r\n` under the default `core.autocrlf`
    // text conversion, so the raw file bytes must be normalized before
    // comparison rather than compared as checked out.
    let expected = std::fs::read_to_string(&expected_path)
        .unwrap_or_else(|e| panic!("could not read {}: {e}", expected_path.display()))
        .replace("\r\n", "\n");

    let relative_py_path = format!("tests/diagnostics/{fixture_stem}.py");
    let output = Command::new(pycc_bin())
        .args(["check", &relative_py_path])
        .current_dir(repo_root)
        .output()
        .unwrap();
    let actual = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        actual, expected,
        "diagnostic output for {fixture_stem} did not match its .expected.txt fixture"
    );
    assert_eq!(
        output.status.code(),
        Some(1),
        "{fixture_stem} should be a compile error"
    );
}

fn assert_json_diagnostic_matches_fixture(fixture_stem: &str) {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let expected_path = repo_root
        .join("tests/diagnostics")
        .join(format!("{fixture_stem}.expected.json"));
    let expected = std::fs::read_to_string(&expected_path)
        .unwrap_or_else(|e| panic!("could not read {}: {e}", expected_path.display()))
        .replace("\r\n", "\n");

    let relative_py_path = format!("tests/diagnostics/{fixture_stem}.py");
    let output = Command::new(pycc_bin())
        .args(["check", &relative_py_path, "--error-format", "json"])
        .current_dir(repo_root)
        .output()
        .unwrap();
    let actual = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        actual, expected,
        "JSON diagnostic output for {fixture_stem} did not match its .expected.json fixture"
    );
    assert_eq!(
        output.status.code(),
        Some(1),
        "{fixture_stem} should be a compile error"
    );
}

#[test]
fn c0001_unsupported_valid_python() {
    assert_diagnostic_matches_fixture("c0001_unsupported_valid_python");
}

#[test]
fn d0001_missing_public_annotation() {
    assert_diagnostic_matches_fixture("d0001_missing_public_annotation");
}

#[test]
fn d0002_any_forbidden() {
    assert_diagnostic_matches_fixture("d0002_any_forbidden");
}

#[test]
fn d0021_range_argument_type() {
    assert_diagnostic_matches_fixture("d0021_range_argument_type");
}

#[test]
fn d0021_unbound_local() {
    assert_diagnostic_matches_fixture("d0021_unbound_local");
    assert_json_diagnostic_matches_fixture("d0021_unbound_local");
}

#[test]
fn d0022_missing_return() {
    assert_diagnostic_matches_fixture("d0022_missing_return");
}

#[test]
fn d0023_incompatible_assignment() {
    assert_diagnostic_matches_fixture("d0023_incompatible_assignment");
}

#[test]
fn d0024_return_outside_function() {
    assert_diagnostic_matches_fixture("d0024_return_outside_function");
}

#[test]
fn d0025_annotated_assignment_mismatch() {
    assert_diagnostic_matches_fixture("d0025_annotated_assignment_mismatch");
}

#[test]
fn d0026_annotation_only_unbound() {
    assert_diagnostic_matches_fixture("d0026_annotation_only_unbound");
}

// #133: a module-level value binding shadows builtin and user-function call
// lookup at every later call site. The four fixtures cover the issue's
// completion criteria through the public `pycc check` path: a user-function
// shadow, a builtin shadow, a call inside an annotated function body (checked
// against the final module environment, D-041), and the private-helper
// inference path.
#[test]
fn d0027_module_value_shadows_function() {
    assert_diagnostic_matches_fixture("d0027_module_value_shadows_function");
}

#[test]
fn d0028_module_value_shadows_builtin() {
    assert_diagnostic_matches_fixture("d0028_module_value_shadows_builtin");
}

#[test]
fn d0029_function_body_calls_shadowed_name() {
    assert_diagnostic_matches_fixture("d0029_function_body_calls_shadowed_name");
}

#[test]
fn d0030_private_helper_calls_shadowed_name() {
    assert_diagnostic_matches_fixture("d0030_private_helper_calls_shadowed_name");
}

// D-110: the shadowed-builtin shape inside an inference-signature helper,
// pinned through the public CLI. The mirror gate fires before the solver's
// `print` special case; pass 3 would independently reject this shape too, so
// this fixture pins behavior rather than discriminating the mirror (the
// discriminating case lives in pycc_types' own unit tests).
#[test]
fn d0031_private_helper_calls_shadowed_builtin() {
    assert_diagnostic_matches_fixture("d0031_private_helper_calls_shadowed_builtin");
}

#[test]
fn d0032_heterogeneous_list_literal() {
    assert_diagnostic_matches_fixture("d0032_heterogeneous_list_literal");
}

#[test]
fn d0033_subscript_on_non_list() {
    assert_diagnostic_matches_fixture("d0033_subscript_on_non_list");
}

#[test]
fn d0034_list_element_type_not_int() {
    assert_diagnostic_matches_fixture("d0034_list_element_type_not_int");
}

#[test]
fn d0035_heterogeneous_dict_literal() {
    assert_diagnostic_matches_fixture("d0035_heterogeneous_dict_literal");
}

#[test]
fn d0036_dict_key_value_type_not_str_int() {
    assert_diagnostic_matches_fixture("d0036_dict_key_value_type_not_str_int");
}

#[test]
fn d0037_heterogeneous_set_literal() {
    assert_diagnostic_matches_fixture("d0037_heterogeneous_set_literal");
}

#[test]
fn d0038_set_element_type_not_int() {
    assert_diagnostic_matches_fixture("d0038_set_element_type_not_int");
}

#[test]
fn d0039_tuple_element_type_not_int_bool_float() {
    assert_diagnostic_matches_fixture("d0039_tuple_element_type_not_int_bool_float");
}

#[test]
fn d0040_tuple_index_not_a_literal() {
    assert_diagnostic_matches_fixture("d0040_tuple_index_not_a_literal");
}

#[test]
fn d0021_float_wrong_arity() {
    assert_diagnostic_matches_fixture("d0021_float_wrong_arity");
}

#[test]
fn d0021_float_non_numeric_argument() {
    assert_diagnostic_matches_fixture("d0021_float_non_numeric_argument");
}

#[test]
fn cli_spec_example() {
    assert_diagnostic_matches_fixture("cli_spec_example");
}

// PR-12 Task 12 (D-119): comprehensions, slicing, and the new
// `.pop()`/`.get()`/`.add()` container methods mint zero new diagnostic
// codes -- every rejection below reuses T0021/T0033/T0034/T0036/T0038 or
// the existing C0001 capability catch-all. These fixtures pin the new
// *rejection paths* through the public `pycc check` CLI, one per genuinely
// new way to reach an already-registered code.

#[test]
fn d0033_slice_on_non_list() {
    assert_diagnostic_matches_fixture("d0033_slice_on_non_list");
}

#[test]
fn d0021_slice_bound_not_int() {
    assert_diagnostic_matches_fixture("d0021_slice_bound_not_int");
}

#[test]
fn d0033_pop_on_non_list() {
    assert_diagnostic_matches_fixture("d0033_pop_on_non_list");
}

// `.pop()` with an argument is an arity mismatch caught during HIR
// lowering, before `pycc_types` ever runs -- it reports through the
// generic C0001 capability path, not T0033, exactly like `.append()`'s
// own pre-existing arity check.
#[test]
fn c0001_list_pop_with_argument() {
    assert_diagnostic_matches_fixture("c0001_list_pop_with_argument");
}

#[test]
fn d0033_get_on_non_dict() {
    assert_diagnostic_matches_fixture("d0033_get_on_non_dict");
}

#[test]
fn d0021_dict_get_key_type_mismatch() {
    assert_diagnostic_matches_fixture("d0021_dict_get_key_type_mismatch");
}

#[test]
fn d0021_dict_get_default_type_mismatch() {
    assert_diagnostic_matches_fixture("d0021_dict_get_default_type_mismatch");
}

// Same HIR-lowering-time arity shape as `.pop()` above -- C0001, not T0033.
#[test]
fn c0001_dict_get_wrong_argument_count() {
    assert_diagnostic_matches_fixture("c0001_dict_get_wrong_argument_count");
}

#[test]
fn d0033_add_on_non_set() {
    assert_diagnostic_matches_fixture("d0033_add_on_non_set");
}

#[test]
fn d0021_set_add_value_type_mismatch() {
    assert_diagnostic_matches_fixture("d0021_set_add_value_type_mismatch");
}

#[test]
fn c0001_comprehension_two_for_clauses() {
    assert_diagnostic_matches_fixture("c0001_comprehension_two_for_clauses");
}

#[test]
fn c0001_comprehension_two_if_filters() {
    assert_diagnostic_matches_fixture("c0001_comprehension_two_if_filters");
}

// D-117: a comprehension outside the `Stmt::Assign`-RHS position it is
// specially recognized in (here, a `print(...)` call argument) falls
// through to `lower_expr`'s existing generic "expression kind not
// supported yet" C0001 catch-all, not a new comprehension-specific error.
#[test]
fn c0001_comprehension_as_call_argument() {
    assert_diagnostic_matches_fixture("c0001_comprehension_as_call_argument");
}

#[test]
fn d0034_comprehension_list_str() {
    assert_diagnostic_matches_fixture("d0034_comprehension_list_str");
}

#[test]
fn d0036_comprehension_dict_str_str() {
    assert_diagnostic_matches_fixture("d0036_comprehension_dict_str_str");
}

#[test]
fn d0038_comprehension_set_str() {
    assert_diagnostic_matches_fixture("d0038_comprehension_set_str");
}

// Real Python's dict-comprehension grammar has no `**`-unpacking form the
// way a plain dict literal does, but the vendored parser accepts
// `{**x for k in y}` anyway (silently dropping the `**`) rather than
// rejecting it at parse time -- so this is real, parseable Python that
// reaches a dedicated C0001 capability diagnostic, not an internal panic.
#[test]
fn c0001_dict_comprehension_unpacking() {
    assert_diagnostic_matches_fixture("c0001_dict_comprehension_unpacking");
}
