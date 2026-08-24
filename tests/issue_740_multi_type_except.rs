//! End-to-end coverage for PEP 758 multi-type `except` handlers -- Part 3
//! of #543, issue #740.
//!
//! `except A, B:` (bare comma, no parentheses) and `except (A, B):`
//! (parenthesized) both name more than one exception type in a single
//! handler. Everything here goes through the public `pycc` CLI, mirroring
//! `tests/issue_739_oserror_hierarchy.rs`'s `build_and_run`/`check_error`
//! harness.

use std::io::Write;
use std::process::Command;

fn pycc_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_pycc"))
}

fn scratch(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("pycc_740_{tag}_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_fixture(dir: &std::path::Path, name: &str, source: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    let mut file = std::fs::File::create(&path).unwrap();
    file.write_all(source.as_bytes()).unwrap();
    path
}

/// `(exit_success, stdout, stderr)` of the built program.
fn build_and_run(tag: &str, source: &str) -> (bool, String, String) {
    let dir = scratch(tag);
    let src = write_fixture(&dir, &format!("{tag}.py"), source);
    let out = dir.join(tag);
    let build = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&out).output().unwrap();
    (
        run.status.success(),
        String::from_utf8_lossy(&run.stdout).to_string(),
        String::from_utf8_lossy(&run.stderr).to_string(),
    )
}

/// The combined diagnostic text of a rejected `pycc check`.
fn check_error(tag: &str, source: &str) -> String {
    let dir = scratch(tag);
    let src = write_fixture(&dir, &format!("{tag}.py"), source);
    let output = Command::new(pycc_bin())
        .args(["check", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!output.status.success(), "expected `{tag}` to be rejected");
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

// -- Acceptance criterion 1: bare-comma multi-type handler ---------------

#[test]
fn bare_comma_handler_catches_either_named_type() {
    for (raised, tag) in [("ValueError", "a"), ("TypeError", "b")] {
        let full_tag = format!("bare_comma_{tag}");
        let (ok, stdout, stderr) = build_and_run(
            &full_tag,
            &format!(
                "def main() -> None:\n\
                 \x20   try:\n        raise {raised}(\"boom\")\n\
                 \x20   except ValueError, TypeError:\n        print(\"caught\")\n\n\n\
                 main()\n"
            ),
        );
        assert!(ok, "`{raised}` program failed: {stderr}");
        assert_eq!(stdout, "caught\n", "`{raised}` did not report caught");
    }
}

// -- Acceptance criterion 2: parenthesized form behaves identically -----

#[test]
fn parenthesized_handler_catches_either_named_type() {
    for (raised, tag) in [("ValueError", "a"), ("TypeError", "b")] {
        let full_tag = format!("parens_{tag}");
        let (ok, stdout, stderr) = build_and_run(
            &full_tag,
            &format!(
                "def main() -> None:\n\
                 \x20   try:\n        raise {raised}(\"boom\")\n\
                 \x20   except (ValueError, TypeError):\n        print(\"caught\")\n\n\n\
                 main()\n"
            ),
        );
        assert!(ok, "`{raised}` program failed: {stderr}");
        assert_eq!(stdout, "caught\n", "`{raised}` did not report caught");
    }
}

// -- Acceptance criterion 3: `as` binding + re-raise --------------------

#[test]
fn parenthesized_handler_as_binding_reraises_successfully() {
    let (ok, stdout, stderr) = build_and_run(
        "as_binding_reraise",
        "def main() -> None:\n\
         \x20   try:\n\
         \x20       try:\n            raise TypeError(\"boom\")\n\
         \x20       except (ValueError, TypeError) as e:\n            print(\"caught\")\n            raise e\n\
         \x20   except TypeError:\n        print(\"outer\")\n\n\n\
         main()\n",
    );
    assert!(ok, "program failed: {stderr}");
    assert_eq!(stdout, "caught\nouter\n");
}

// -- Acceptance criterion 4: a non-matching instance falls through -------

#[test]
fn non_matching_instance_falls_through_to_next_handler() {
    let (ok, stdout, stderr) = build_and_run(
        "falls_through_next_handler",
        "def main() -> None:\n\
         \x20   try:\n        raise KeyError(\"boom\")\n\
         \x20   except (ValueError, TypeError):\n        print(\"wrong\")\n\
         \x20   except KeyError:\n        print(\"right\")\n\n\n\
         main()\n",
    );
    assert!(ok, "program failed: {stderr}");
    assert_eq!(stdout, "right\n");
}

#[test]
fn non_matching_instance_with_no_other_handler_propagates_uncaught() {
    let (ok, _stdout, stderr) = build_and_run(
        "propagates_uncaught",
        "def main() -> None:\n\
         \x20   try:\n        raise KeyError(\"boom\")\n\
         \x20   except (ValueError, TypeError):\n        print(\"wrong\")\n\n\n\
         main()\n",
    );
    assert!(!ok, "program should have exited with an uncaught exception");
    assert!(
        stderr.contains("KeyError"),
        "expected the uncaught KeyError to be reported, got: {stderr}"
    );
}

// -- Acceptance criterion 5: subclasses of named types are caught -------

#[test]
fn subclass_of_a_named_type_is_still_caught() {
    let (ok, stdout, stderr) = build_and_run(
        "subclass_caught",
        "def main() -> None:\n\
         \x20   try:\n        raise FileNotFoundError(\"missing\")\n\
         \x20   except (OSError, ValueError):\n        print(\"caught\")\n\n\n\
         main()\n",
    );
    assert!(ok, "program failed: {stderr}");
    assert_eq!(stdout, "caught\n");
}

#[test]
fn subclass_reachable_via_either_named_ancestor_is_deduped_and_caught() {
    // `BrokenPipeError` derives from `ConnectionError`, which derives from
    // `OSError` -- reachable via both named ancestors, exercising the
    // union+dedup path in MIR's handler tag computation.
    let (ok, stdout, stderr) = build_and_run(
        "dedup_double_ancestor",
        "def main() -> None:\n\
         \x20   try:\n        raise BrokenPipeError(\"pipe\")\n\
         \x20   except (OSError, ConnectionError):\n        print(\"caught\")\n\n\n\
         main()\n",
    );
    assert!(ok, "program failed: {stderr}");
    assert_eq!(stdout, "caught\n");
}

// -- Acceptance criterion 6: single bare-name handler still works -------

#[test]
fn single_bare_name_handler_still_works() {
    let (ok, stdout, stderr) = build_and_run(
        "single_bare_name",
        "def main() -> None:\n\
         \x20   try:\n        raise ValueError(\"boom\")\n\
         \x20   except ValueError:\n        print(\"caught\")\n\n\n\
         main()\n",
    );
    assert!(ok, "program failed: {stderr}");
    assert_eq!(stdout, "caught\n");
}

// -- Acceptance criterion 7: three or more types, each independently ----
// -- catchable -------------------------------------------------------

#[test]
fn three_type_handler_catches_each_independently() {
    for (raised, tag) in [
        ("ValueError", "a"),
        ("TypeError", "b"),
        ("KeyError", "c"),
    ] {
        let full_tag = format!("three_type_{tag}");
        let (ok, stdout, stderr) = build_and_run(
            &full_tag,
            &format!(
                "def main() -> None:\n\
                 \x20   try:\n        raise {raised}(\"boom\")\n\
                 \x20   except (ValueError, TypeError, KeyError):\n        print(\"caught\")\n\n\n\
                 main()\n"
            ),
        );
        assert!(ok, "`{raised}` program failed: {stderr}");
        assert_eq!(stdout, "caught\n", "`{raised}` did not report caught");
    }
}

// -- Acceptance criterion 12: a user-defined class alongside a builtin --
// -- with `as` is rejected -----------------------------------------

#[test]
fn user_defined_class_alongside_builtin_with_as_binding_is_rejected() {
    let combined = check_error(
        "user_class_with_as",
        "class MyUserError(ValueError):\n    pass\n\n\n\
         def main() -> None:\n\
         \x20   try:\n        raise MyUserError(\"boom\")\n\
         \x20   except (ValueError, MyUserError) as e:\n        print(\"caught\")\n\n\n\
         main()\n",
    );
    assert!(
        combined.contains("C0001"),
        "expected C0001 for a user-defined class with `as` in a multi-type handler, got: {combined}"
    );
    assert!(
        combined.contains("MyUserError"),
        "expected the offending class to be named, got: {combined}"
    );
}
