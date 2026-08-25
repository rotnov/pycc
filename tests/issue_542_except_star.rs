//! `except*`/`ExceptionGroup`/`BaseExceptionGroup` (Part 3 of #382, #542,
//! PEP 654, D-202) end-to-end coverage, following the same per-issue test
//! file convention as its sibling parts of the exception-chain epic
//! (`tests/issue_739_oserror_hierarchy.rs` for Part 2 of #543,
//! `tests/issue_740_multi_type_except.rs` for Part 3 of #543).

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

fn build_and_run(dir: &std::path::Path, src_name: &str, source: &str) -> (bool, Vec<u8>, String) {
    let src = write_fixture(dir, src_name, source);
    let out = dir.join(src_name.replace(".py", ""));
    let output = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .output()
        .unwrap();
    if !output.status.success() {
        return (
            false,
            Vec::new(),
            String::from_utf8_lossy(&output.stderr).to_string(),
        );
    }
    let run = Command::new(&out).output().unwrap();
    (
        run.status.success(),
        run.stdout.clone(),
        String::from_utf8_lossy(&run.stderr).to_string(),
    )
}

fn check_only(dir: &std::path::Path, src_name: &str, source: &str) -> (bool, String) {
    let src = write_fixture(dir, src_name, source);
    let output = Command::new(pycc_bin())
        .args(["check", src.to_str().unwrap()])
        .output()
        .unwrap();
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    (output.status.success(), combined)
}

#[test]
fn except_star_single_clause_catches_a_plain_exception() {
    let dir = std::env::temp_dir().join(format!("pycc_542_single_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, out, err) = build_and_run(
        &dir,
        "single.py",
        "try:\n    raise ValueError(\"bad\")\nexcept* ValueError:\n    print(\"caught\")\n",
    );
    assert!(ok, "build/run failed: {err}");
    assert_eq!(out, b"caught\n");
}

#[test]
fn except_star_dispatches_a_plain_exception_to_the_matching_clause() {
    let dir = std::env::temp_dir().join(format!("pycc_542_dispatch_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, out, err) = build_and_run(
        &dir,
        "dispatch.py",
        "try:\n    raise ValueError(\"bad\")\nexcept* TypeError:\n    print(\"type\")\nexcept* ValueError:\n    print(\"value\")\n",
    );
    assert!(ok, "build/run failed: {err}");
    assert_eq!(out, b"value\n");
}

#[test]
fn except_star_group_dispatches_each_member_to_its_own_clause() {
    let dir = std::env::temp_dir().join(format!("pycc_542_group_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // D-202: an `ExceptionGroup` member must be an *existing* exception
    // value, not a fresh `SomeError("msg")` construction (see
    // `check_exception_group_member_operand`'s own doc comment) -- so `e1`/
    // `e2` are bound by nested `except ... as` handlers before the group
    // that carries them both is constructed and raised.
    let (ok, out, err) = build_and_run(
        &dir,
        "group.py",
        "try:\n    raise ValueError(\"v\")\nexcept ValueError as e1:\n    try:\n        raise TypeError(\"t\")\n    except TypeError as e2:\n        try:\n            raise ExceptionGroup(\"multi\", [e1, e2])\n        except* ValueError:\n            print(\"caught value\")\n        except* TypeError:\n            print(\"caught type\")\n",
    );
    assert!(ok, "build/run failed: {err}");
    assert_eq!(out, b"caught value\ncaught type\n");
}

/// Part 3 of #382 (#542, PEP 654, D-202): a partial match. `except*
/// ValueError:` claims only `e1` from the raised two-member group; `e2`
/// (a `TypeError`) is left in the still-unmatched remainder threaded through
/// `emit_try_star`'s `pycc_rt_exception_group_partition` chain, and since no
/// further clause exists to claim it, the remainder is re-raised at
/// `reraise_remainder_bb` after the matching clause's body has already run.
/// This is the only test in this file that takes the non-null branch of
/// `current_group_slot` at that block -- every other `except*` test either
/// matches every member (so the remainder is empty and that branch's load is
/// never reached) or raises no group at all.
#[test]
fn except_star_partial_match_reraises_the_unmatched_remainder() {
    let dir = std::env::temp_dir().join(format!("pycc_542_partial_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, out, err) = build_and_run(
        &dir,
        "partial.py",
        "try:\n    raise ValueError(\"v\")\nexcept ValueError as e1:\n    try:\n        raise TypeError(\"t\")\n    except TypeError as e2:\n        try:\n            raise ExceptionGroup(\"multi\", [e1, e2])\n        except* ValueError:\n            print(\"caught value\")\n",
    );
    assert!(!ok, "expected non-zero exit: unmatched TypeError remainder should re-raise");
    assert_eq!(
        out, b"caught value\n",
        "the matched clause should still run before the remainder re-raises"
    );
    assert!(
        err.contains("ExceptionGroup"),
        "stderr should mention the re-raised remainder's ExceptionGroup: {err}"
    );
}

/// Part 3 of #382 (#542, PEP 654, D-202): `BaseExceptionGroup` construction
/// and dispatch, exercised nowhere else in this file -- every other
/// `except*`/group test uses `ExceptionGroup`. D-202 records that
/// `BaseExceptionGroup`'s hierarchy parent is treated as `Exception` rather
/// than a separate `BaseException`-only branch; this confirms it still
/// raises, dispatches, and is caught end to end under that simplification.
#[test]
fn base_exception_group_construction_and_dispatch() {
    let dir = std::env::temp_dir().join(format!("pycc_542_baseeg_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, out, err) = build_and_run(
        &dir,
        "baseeg.py",
        "try:\n    raise ValueError(\"v\")\nexcept ValueError as e1:\n    try:\n        raise BaseExceptionGroup(\"multi\", [e1])\n    except* ValueError:\n        print(\"caught\")\n",
    );
    assert!(ok, "build/run failed: {err}");
    assert_eq!(out, b"caught\n");
}

#[test]
fn except_star_as_binding_is_accessible() {
    let dir = std::env::temp_dir().join(format!("pycc_542_asbind_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, out, err) = build_and_run(
        &dir,
        "asbind.py",
        "try:\n    raise ValueError(\"bad\")\nexcept* ValueError as eg:\n    saved = eg\n    print(\"got it\")\n",
    );
    assert!(ok, "build/run failed: {err}");
    assert_eq!(out, b"got it\n");
}

/// `pycc_types::function_local_names` (used only for a function body, not a
/// top-level module body) has its own `collect_local_names` recursion with a
/// `TryStar` arm shared with `Try` -- `except_star_as_binding_is_accessible`
/// above exercises the same source shape, but at module level, which never
/// reaches `function_local_names` at all. Wrap the same shape in a function
/// to exercise that separate path.
#[test]
fn except_star_as_binding_inside_a_function_is_a_local() {
    let dir = std::env::temp_dir().join(format!("pycc_542_asbind_fn_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, out, err) = build_and_run(
        &dir,
        "asbind_fn.py",
        "def f() -> None:\n    try:\n        raise ValueError(\"bad\")\n    except* ValueError as eg:\n        saved = eg\n        print(\"got it in function\")\n\nf()\n",
    );
    assert!(ok, "build/run failed: {err}");
    assert_eq!(out, b"got it in function\n");
}

/// `reject_generic_calls_in_stmt` (D-133/D-134's PEP 695 self/mutual
/// generic-recursion check) walks every statement in a *generic* function's
/// own body looking for a disallowed nested generic call, and shares its
/// `Try`/`TryStar` arm with `pycc_types::lib`'s other structural walks --
/// but it only ever runs for a generic function specifically, so no
/// `except*`-inside-an-ordinary-function fixture reaches it. A generic
/// function whose `try`/`except*` bodies contain no generic call at all
/// still walks all four blocks looking for one.
#[test]
fn except_star_inside_a_generic_function_body_type_checks() {
    let dir = std::env::temp_dir().join(format!("pycc_542_generic_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, combined) = check_only(
        &dir,
        "generic.py",
        "def identity[T](x: T) -> T:\n    try:\n        return x\n    except* ValueError:\n        return x\n    else:\n        return x\n    finally:\n        pass\n",
    );
    assert!(ok, "check failed: {combined}");
}

#[test]
fn except_star_unmatched_member_propagates_uncaught() {
    let dir = std::env::temp_dir().join(format!("pycc_542_unmatched_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, _out, err) = build_and_run(
        &dir,
        "unmatched.py",
        "try:\n    raise ValueError(\"v\")\nexcept ValueError as e1:\n    try:\n        raise TypeError(\"t\")\n    except TypeError as e2:\n        raise ExceptionGroup(\"multi\", [e1, e2])\n",
    );
    assert!(!ok, "expected non-zero exit for an unmatched group remainder");
    assert!(
        err.contains("ExceptionGroup"),
        "stderr should mention ExceptionGroup: {err}"
    );
}

#[test]
fn except_star_finally_always_runs() {
    let dir = std::env::temp_dir().join(format!("pycc_542_finally_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, out, err) = build_and_run(
        &dir,
        "finally.py",
        "try:\n    raise ValueError(\"bad\")\nexcept* ValueError:\n    print(\"caught\")\nfinally:\n    print(\"cleanup\")\n",
    );
    assert!(ok, "build/run failed: {err}");
    assert_eq!(out, b"caught\ncleanup\n");
}

/// Unlike `except_star_finally_always_runs` above (a top-level, `None`-typed
/// `try`/`except*`/`finally`), a value-returning function whose `try`/
/// `except*` construct also carries a `finally` needs `emit_try_star` to
/// allocate a `ret_slot` to stash the return value across the `finally`
/// block's own codegen -- without a fixture like this one, that allocation
/// path is only ever exercised for plain `Try`, never `TryStar`.
#[test]
fn except_star_with_finally_in_a_value_returning_function_returns_correctly() {
    let dir = std::env::temp_dir().join(format!("pycc_542_finally_ret_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, out, err) = build_and_run(
        &dir,
        "finally_ret.py",
        "def f() -> int:\n    try:\n        raise ValueError(\"bad\")\n    except* ValueError:\n        return 2\n    finally:\n        print(\"cleanup\")\n\nprint(f())\n",
    );
    assert!(ok, "build/run failed: {err}");
    assert_eq!(out, b"cleanup\n2\n");
}

#[test]
fn except_star_else_runs_when_no_exception() {
    let dir = std::env::temp_dir().join(format!("pycc_542_else_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, out, err) = build_and_run(
        &dir,
        "else.py",
        "try:\n    print(\"body\")\nexcept* ValueError:\n    print(\"handler\")\nelse:\n    print(\"else\")\n",
    );
    assert!(ok, "build/run failed: {err}");
    assert_eq!(out, b"body\nelse\n");
}

#[test]
fn except_star_bare_clause_is_rejected_at_parse_time() {
    let dir = std::env::temp_dir().join(format!("pycc_542_bare_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, _combined) = check_only(&dir, "bare.py", "try:\n    pass\nexcept*:\n    pass\n");
    assert!(!ok, "a typeless except* should be rejected as a syntax error");
}

#[test]
fn exception_group_construction_rejects_a_non_literal_member_list() {
    let dir = std::env::temp_dir().join(format!("pycc_542_nonlit_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, combined) = check_only(
        &dir,
        "nonlit.py",
        "members = [1, 2]\ntry:\n    raise ExceptionGroup(\"multi\", members)\nexcept* ValueError:\n    pass\n",
    );
    assert!(!ok, "a non-literal ExceptionGroup member list should be rejected");
    assert!(
        combined.contains("T0021"),
        "should mention T0021: {combined}"
    );
}

/// Part 3 of #382 (#542, PEP 654, D-202): `check_exception_group_member_operand`'s
/// fresh-constructor-call rejection branch -- a group member must be an
/// *existing* exception value, never a `SomeError(...)` construction inline
/// in the member list, even though such a call is a perfectly valid
/// top-level `raise` operand.
#[test]
fn exception_group_construction_rejects_a_fresh_constructor_call_member() {
    let dir = std::env::temp_dir().join(format!("pycc_542_freshctor_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, combined) = check_only(
        &dir,
        "freshctor.py",
        "try:\n    raise ExceptionGroup(\"multi\", [ValueError(\"v\")])\nexcept* ValueError:\n    pass\n",
    );
    assert!(
        !ok,
        "a fresh constructor-call ExceptionGroup member should be rejected"
    );
    assert!(
        combined.contains("T0021"),
        "should mention T0021: {combined}"
    );
    assert!(
        combined.contains("fresh"),
        "should explain the member must not be a fresh construction: {combined}"
    );
}

/// Part 3 of #382 (#542, PEP 654, D-202): `check_exception_group_operand`'s
/// argument-count branch -- `ExceptionGroup` requires exactly a message and a
/// member list, so a single-argument call is rejected structurally before
/// either argument's own type is even inspected.
#[test]
fn exception_group_construction_rejects_the_wrong_argument_count() {
    let dir = std::env::temp_dir().join(format!("pycc_542_argc_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, combined) = check_only(
        &dir,
        "argc.py",
        "try:\n    raise ExceptionGroup(\"multi\")\nexcept* ValueError:\n    pass\n",
    );
    assert!(!ok, "a one-argument ExceptionGroup call should be rejected");
    assert!(
        combined.contains("T0021"),
        "should mention T0021: {combined}"
    );
}

/// Part 3 of #382 (#542, PEP 654, D-202): `check_exception_group_operand`'s
/// message-type branch -- the first argument must be a `str`, exactly like a
/// plain `raise SomeError(...)`'s own message argument.
#[test]
fn exception_group_construction_rejects_a_non_str_message() {
    let dir = std::env::temp_dir().join(format!("pycc_542_msgty_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, combined) = check_only(
        &dir,
        "msgty.py",
        "try:\n    raise ExceptionGroup(1, [1, 2])\nexcept* ValueError:\n    pass\n",
    );
    assert!(!ok, "a non-str ExceptionGroup message should be rejected");
    assert!(
        combined.contains("T0021"),
        "should mention T0021: {combined}"
    );
}

/// Part 3 of #382 (#542, PEP 654, D-202): `check_exception_group_operand`'s
/// empty-member-list branch -- PEP 654 requires at least one member exception,
/// so a literal empty list is rejected at type-check time rather than
/// reaching codegen with nothing to partition.
#[test]
fn exception_group_construction_rejects_an_empty_member_list() {
    let dir = std::env::temp_dir().join(format!("pycc_542_empty_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, combined) = check_only(
        &dir,
        "empty.py",
        "try:\n    raise ExceptionGroup(\"multi\", [])\nexcept* ValueError:\n    pass\n",
    );
    assert!(!ok, "an empty ExceptionGroup member list should be rejected");
    assert!(
        combined.contains("T0021"),
        "should mention T0021: {combined}"
    );
}

/// Part 3 of #382 (#542, PEP 654, D-202): `check_exception_group_member_operand`'s
/// final fallback branch -- a group member that isn't an exception instance
/// at all (e.g. a plain `int`) is rejected too.
#[test]
fn exception_group_construction_rejects_a_non_exception_member() {
    let dir = std::env::temp_dir().join(format!("pycc_542_nonexc_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, combined) = check_only(
        &dir,
        "nonexc.py",
        "try:\n    raise ExceptionGroup(\"multi\", [1])\nexcept* ValueError:\n    pass\n",
    );
    assert!(
        !ok,
        "a non-exception ExceptionGroup member should be rejected"
    );
    assert!(
        combined.contains("T0021"),
        "should mention T0021: {combined}"
    );
}

/// `check_try_star_stmt`'s per-clause loop rejects an `except*` type that is
/// neither a builtin exception nor a user-defined exception class -- the
/// same T0021 check plain `except` already exercises for a made-up name, but
/// not yet exercised for `except*`.
#[test]
fn except_star_rejects_an_unrecognized_exception_type() {
    let dir = std::env::temp_dir().join(format!("pycc_542_unrecognized_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, combined) = check_only(
        &dir,
        "unrecognized.py",
        "try:\n    raise ValueError(\"bad\")\nexcept* NotARealException:\n    pass\n",
    );
    assert!(!ok, "an unrecognized except* type should be rejected");
    assert!(
        combined.contains("T0021"),
        "should mention T0021: {combined}"
    );
}

/// Same rationale as `exception_group_construction_rejects_a_fresh_constructor_call_member`
/// above, but for a *user-defined* exception subclass's constructor call
/// rather than a builtin's: `check_exception_group_member_operand` rejects
/// both, but through two different disjuncts of the same `||` -- a builtin
/// callee short-circuits the check before the user-defined-class lookup
/// ever runs, so a builtin-only fixture never exercises the second disjunct.
#[test]
fn exception_group_construction_rejects_a_fresh_user_defined_constructor_call_member() {
    let dir = std::env::temp_dir().join(format!("pycc_542_freshuserctor_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, combined) = check_only(
        &dir,
        "freshuserctor.py",
        "class MyError(ValueError):\n    pass\n\ntry:\n    raise ExceptionGroup(\"multi\", [MyError(\"v\")])\nexcept* MyError:\n    pass\n",
    );
    assert!(
        !ok,
        "a fresh user-defined-class constructor-call ExceptionGroup member should be rejected"
    );
    assert!(
        combined.contains("T0021"),
        "should mention T0021: {combined}"
    );
}

/// Mirrors `a_value_returning_function_may_terminate_only_by_raising` and its
/// siblings in `tests/issue_382_exceptions.rs`, but for `TryStar` instead of
/// `Try`: a non-`None`-returning function whose entire body is a `try`/
/// `except*` where every path (the raising try body, and the re-raising
/// handler) terminates. `pycc_codegen::exception::block_always_terminates`
/// has a dedicated `MirStmt::TryStar` arm (shared textually with `Try`'s) --
/// without a fixture like this one, that arm's pattern binding is never
/// actually matched at runtime, since every other `except*` fixture's outer
/// function returns `None` and falls through normally instead of relying on
/// this fallthrough-proof machinery.
#[test]
fn a_value_returning_function_whose_entire_body_is_a_terminating_try_star_never_falls_through() {
    let dir = std::env::temp_dir().join(format!("pycc_542_terminates_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, out, err) = build_and_run(
        &dir,
        "terminates.py",
        "def fail() -> int:\n    try:\n        raise ValueError(\"from try star\")\n    except* ValueError:\n        raise ValueError(\"rethrown from handler\")\n\ntry:\n    x = fail()\nexcept* ValueError:\n    print(\"caught value callee via star\")\n",
    );
    assert!(ok, "build/run failed: {err}");
    assert_eq!(out, b"caught value callee via star\n");
}
