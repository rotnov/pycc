//! Type-checking tests for raising and catching user-defined exception
//! classes (Part 2 of #541, D-189).
//!
//! In its own submodule rather than appended to the crate's `tests.rs`, per
//! AGENTS.md's decomposability rule.

use super::*;
use crate::check;

fn check_source(source: &str) -> Result<(), Diagnostic> {
    let module = pycc_parser::parse(source).expect("test fixture must parse");
    let hir = pycc_hir::lower_checked(&module).expect("test fixture must lower");
    check(&hir)
}

fn expect_error(source: &str) -> Diagnostic {
    check_source(source).expect_err("source must be rejected")
}

const HIERARCHY: &str = "class AppError(Exception):\n    pass\n\n\n\
                         class DatabaseError(AppError):\n    pass\n\n\n";

#[test]
fn raising_a_user_exception_class_is_accepted() {
    check_source(&format!(
        "{HIERARCHY}def main() -> None:\n    raise DatabaseError(\"boom\")\n"
    ))
    .expect("raising a user exception class must be accepted");
}

#[test]
fn catching_a_user_exception_class_is_accepted() {
    check_source(&format!(
        "{HIERARCHY}def main() -> None:\n    try:\n        raise DatabaseError(\"boom\")\n\
         \x20   except AppError:\n        print(\"caught\")\n"
    ))
    .expect("catching a user exception class must be accepted");
}

#[test]
fn a_user_exception_class_is_accepted_as_a_raise_cause() {
    check_source(&format!(
        "{HIERARCHY}def main() -> None:\n\
         \x20   raise DatabaseError(\"boom\") from AppError(\"root\")\n"
    ))
    .expect("a user exception class must be accepted as a cause");
}

#[test]
fn raising_a_bound_user_exception_value_stays_rejected() {
    // The memory-safety invariant of Part 2: `e` infers the identical
    // `Ty::Instance("AppError")` that `AppError("x")` does, but MIR lowers a
    // bound value to `MirExceptionValue::Existing`, which codegen hands to
    // `pycc_rt_exception_raise` as a `*mut PyExceptionObj` while the value is
    // really a `*mut PyInstanceObj`. Accepting it would be memory corruption,
    // not a feature. Part 3 of #541 (#703) makes the two representations one.
    let diagnostic = expect_error(&format!(
        "{HIERARCHY}def main() -> None:\n    e = AppError(\"x\")\n    raise e\n"
    ));
    assert_eq!(diagnostic.code, "T0021");
    assert!(
        diagnostic
            .message
            .contains("can only raise exception instances"),
        "unexpected message: {}",
        diagnostic.message
    );
}

#[test]
fn raising_a_class_outside_the_exception_hierarchy_is_rejected() {
    let diagnostic = expect_error(
        "class Point:\n    def __init__(self, x: int) -> None:\n        self.x = x\n\n\n\
         def main() -> None:\n    raise ValueError(\"a\")\n    raise Point(1)\n",
    );
    assert_eq!(diagnostic.code, "T0021");
}

#[test]
fn catching_a_class_outside_the_exception_hierarchy_is_rejected() {
    let diagnostic = expect_error(&format!(
        "class Point:\n    def __init__(self, x: int) -> None:\n        self.x = x\n\n\n\
         {HIERARCHY}def main() -> None:\n    try:\n        raise AppError(\"x\")\n\
         \x20   except Point:\n        print(\"caught\")\n"
    ));
    assert_eq!(diagnostic.code, "T0021");
    assert!(
        diagnostic
            .message
            .contains("is not a recognized exception class"),
        "unexpected message: {}",
        diagnostic.message
    );
}

#[test]
fn binding_a_caught_user_exception_with_as_is_a_capability_gap() {
    let diagnostic = expect_error(&format!(
        "{HIERARCHY}def main() -> None:\n    try:\n        raise AppError(\"x\")\n\
         \x20   except AppError as exc:\n        print(\"caught\")\n"
    ));
    assert_eq!(diagnostic.code, "C0001");
    assert!(
        diagnostic
            .message
            .contains("with `as` is not supported yet"),
        "unexpected message: {}",
        diagnostic.message
    );
}

#[test]
fn binding_a_caught_builtin_exception_with_as_still_works() {
    // The `as` rejection above must be scoped to user classes; the builtin
    // form has been accepted since #382 and stays accepted.
    check_source(
        "def main() -> None:\n    try:\n        raise ValueError(\"x\")\n\
         \x20   except ValueError as exc:\n        print(\"caught\")\n",
    )
    .expect("binding a caught builtin exception must still be accepted");
}

const OWN_INIT: &str = "class AppError(Exception):\n\
                        \x20   def __init__(self, code: int) -> None:\n\
                        \x20       self.code = code\n\n\n";

#[test]
fn raising_an_exception_class_with_its_own_constructor_is_a_capability_gap() {
    let diagnostic = expect_error(&format!(
        "{OWN_INIT}def main() -> None:\n    raise AppError(3)\n"
    ));
    assert_eq!(diagnostic.code, "C0001");
    assert!(
        diagnostic
            .message
            .contains("declares or inherits an `__init__` other than"),
        "unexpected message: {}",
        diagnostic.message
    );
}

#[test]
fn catching_an_exception_class_with_its_own_constructor_is_a_capability_gap() {
    let diagnostic = expect_error(&format!(
        "{OWN_INIT}def main() -> None:\n    try:\n        raise ValueError(\"x\")\n\
         \x20   except AppError:\n        print(\"caught\")\n"
    ));
    assert_eq!(diagnostic.code, "C0001");
    assert!(
        diagnostic
            .message
            .contains("declares or inherits an `__init__` other than"),
        "unexpected message: {}",
        diagnostic.message
    );
}

#[test]
fn a_subclass_inheriting_an_own_constructor_is_also_a_capability_gap() {
    // The gate walks the MRO, so the defect is reported for a subclass that
    // never wrote a constructor of its own but inherits a non-synthetic one.
    let diagnostic = expect_error(&format!(
        "{OWN_INIT}class DatabaseError(AppError):\n    pass\n\n\n\
         def main() -> None:\n    raise DatabaseError(4)\n"
    ));
    assert_eq!(diagnostic.code, "C0001");
    assert!(
        diagnostic
            .message
            .contains("`DatabaseError` declares or inherits an `__init__` other than"),
        "unexpected message: {}",
        diagnostic.message
    );
}

#[test]
fn a_bad_constructor_argument_reports_its_own_argument_diagnostic() {
    // Inference runs before the raisability gate so the user sees the real
    // problem rather than a generic "can only raise exception instances".
    let diagnostic = expect_error(&format!(
        "{HIERARCHY}def main() -> None:\n    raise AppError(3)\n"
    ));
    assert_eq!(diagnostic.code, "T0021");
    assert!(
        diagnostic.message.contains("expects `str`"),
        "unexpected message: {}",
        diagnostic.message
    );
}

#[test]
fn a_local_binding_shadowing_an_exception_class_is_not_raisable() {
    let diagnostic = expect_error(&format!(
        "{HIERARCHY}def main() -> None:\n    AppError = 3\n    raise AppError(\"x\")\n"
    ));
    assert_eq!(diagnostic.code, "T0021");
}
