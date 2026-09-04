//! PEP 435 (#379, PR-19) enum-class lowering: `lower_enum_class`, split out
//! of `class.rs` under AGENTS.md's "Keep source files decomposable" rule
//! (#892). The enum member-value model itself (`EnumMemberValue`) stays in
//! `class.rs` next to `HirClassDef`, which owns the field it populates, and
//! the `is_enum` detection and dispatch stay in `lower_class` -- only the
//! enum-body validation and member-value derivation live here.
//!
//! `lower_enum_class` keeps its extracted-standalone-function shape (rather
//! than being inlined back into `lower_class`) for the reason recorded on
//! the function itself: cargo-llvm-cov#276's instantiation-merge gap.

use crate::{HirClassDef, HirItem, Ty, unsupported};
use pycc_ast::{Expr, Number, Stmt};
use pycc_diag::Diagnostic;

/// #379 (PR-19): Lower a PEP 435-style enum class. An enum class body is
/// assignments only (`RED = 1`), not method definitions. This function
/// validates the enum body and constructs the `HirClassDef` with
/// `enum_members` populated. Extracted from `lower_class` to isolate the
/// enum-specific code paths (see cargo-llvm-cov#276 for the coverage
/// instantiation issue that motivated the extraction).
pub(super) fn lower_enum_class(
    def: &pycc_ast::StmtClassDef,
    class_name: String,
    bases: Vec<String>,
    mro: Vec<String>,
    type_param: Option<String>,
) -> Result<(HirClassDef, Vec<HirItem>), Diagnostic> {
    // An enum class's attrs are the two reserved member attributes
    // (`value`, `name`), so `member.value`/`member.name` resolve via
    // the existing `resolve_attr_get`/MIR slot resolution unchanged.
    let attrs = vec![
        ("value".to_string(), Ty::Int),
        ("name".to_string(), Ty::Str),
    ];
    let mut enum_members: Vec<(String, i64)> = Vec::new();
    for stmt in &def.body {
        // #744: a docstring (a bare string-literal expression statement) is
        // a no-op, matching `validate_init_subclass_body`'s existing
        // precedent for the same construct.
        if let Stmt::Expr(expr_stmt) = stmt
            && matches!(*expr_stmt.value, Expr::StringLiteral(_))
        {
            continue;
        }
        let Stmt::Assign(assign) = stmt else {
            return Err(unsupported(
                "an enum class body must contain only member assignments (`RED = 1`) -- \
                 no method definitions or other statements are supported yet",
                pycc_ast::stmt_range(stmt),
            ));
        };
        // The target must be a single bare name (not a tuple or
        // subscript).
        if assign.targets.len() != 1 {
            return Err(unsupported(
                "an enum member assignment must have a single target (`RED = 1`), not \
                 multiple targets",
                assign.range,
            ));
        }
        let Expr::Name(target_name) = &assign.targets[0] else {
            return Err(unsupported(
                "an enum member name must be a bare name (`RED = 1`), not an attribute \
                 access, subscript, or other expression",
                pycc_ast::expr_range(&assign.targets[0]),
            ));
        };
        let member_name = target_name.id.to_string();
        // Reject duplicate member names (matching CPython's
        // `TypeError: Attempted to reuse key`).
        if enum_members.iter().any(|(name, _)| name == &member_name) {
            return Err(unsupported(
                format!(
                    "enum member `{member_name}` is already defined in class \
                     `{class_name}` -- duplicate member names are not allowed"
                ),
                assign.range,
            ));
        }
        // The value must be an int literal (the only supported member
        // value type in v0.3, matching TYPE_SYSTEM.md's "integer
        // discriminant" representation). A bool literal is rejected
        // (it is a separate type in pycc, not an int subtype for
        // enum-value purposes). A non-literal value (e.g. `RED = f()`)
        // is also rejected -- enum values must be compile-time
        // literals in pycc's static model. The actual integer value is
        // extracted and carried in `enum_members` so codegen can
        // initialize each member's `value` slot with the correct
        // literal, not a position-derived guess.
        let member_value: i64 = match &*assign.value {
            Expr::NumberLiteral(number) => match &number.value {
                Number::Int(i) => {
                    let Some(value) = i.as_i64() else {
                        return Err(unsupported(
                            format!(
                                "enum member `{member_name}` has an integer value that does \
                                 not fit in i64 -- only i64-range values are supported"
                            ),
                            assign.range,
                        ));
                    };
                    value
                }
                _ => {
                    return Err(unsupported(
                        format!(
                            "enum member `{member_name}` has a non-integer value -- only \
                             `int` member values are supported in v0.3"
                        ),
                        assign.range,
                    ));
                }
            },
            _ => {
                return Err(unsupported(
                    format!(
                        "enum member `{member_name}` must be assigned an integer literal \
                         (`{member_name} = 1`), not an expression or non-integer value"
                    ),
                    assign.range,
                ));
            }
        };
        enum_members.push((member_name, member_value));
    }
    // An enum class has no methods, no __init__, and no items. Its
    // members are compile-time singletons allocated by codegen, not
    // runtime-instantiated objects.
    Ok((
        HirClassDef {
            exception_type_tag: None,
            name: class_name,
            bases,
            mro,
            attrs,
            methods: Vec::new(),
            properties: Vec::new(),
            static_methods: Vec::new(),
            class_methods: Vec::new(),
            type_param,
            enum_members,
            is_dataclass: false,
            dataclass_fields: Vec::new(),
            is_protocol: false,
            runtime_checkable: false,
            protocol_members: Vec::new(),
            abstract_methods: Vec::new(),
            is_abstract: false,
        },
        Vec::new(),
    ))
}

#[cfg(test)]
mod tests {
    use crate::class::tests::{assert_c0001, lower_ok};

    // -- #379 (PR-19): PEP 435 enum class lowering ------------------------

    #[test]
    fn generic_enum_class_is_rejected() {
        // `class C[T](Enum):` — a generic class whose single base is `Enum`.
        // The type parameter `T` triggers the generic-enum rejection at
        // line 448-455, distinct from the multiple-bases rejection.
        assert_c0001("class C[T](Enum):\n    RED = 1\n");
    }

    #[test]
    fn enum_member_with_multiple_targets_is_rejected() {
        // `RED = GREEN = 1` — a chain assignment with multiple targets,
        // which has `assign.targets.len() == 2`, triggering the rejection
        // at line 551-556. (Tuple unpacking `RED, GREEN = 1, 2` has a
        // single tuple target and hits a different path.)
        assert_c0001("class C(Enum):\n    RED = GREEN = 1\n");
    }

    #[test]
    fn enum_member_with_non_name_target_is_rejected() {
        assert_c0001("class C(Enum):\n    C.RED = 1\n");
    }

    #[test]
    fn enum_member_value_overflowing_i64_is_rejected() {
        assert_c0001("class C(Enum):\n    RED = 99999999999999999999999999\n");
    }

    #[test]
    fn enum_member_with_non_literal_value_is_rejected() {
        assert_c0001("x = 1\nclass C(Enum):\n    RED = x\n");
    }

    // -- #379: enum error paths covered via unit tests (not integration
    //    tests) to avoid cargo-llvm-cov issue #276 (instantiation merging) --

    #[test]
    fn enum_body_with_method_is_rejected_via_unit_test() {
        // Exercises the "enum class body must contain only member
        // assignments" error path (lines 828-832).
        assert_c0001(
            "class Color(Enum):\n    RED = 1\n    def f(self) -> int:\n        return 1\n",
        );
    }

    #[test]
    fn duplicate_enum_member_is_rejected_via_unit_test() {
        // Exercises the "duplicate enum member" error path (lines 854-860).
        assert_c0001("class Color(Enum):\n    RED = 1\n    RED = 2\n");
    }

    #[test]
    fn enum_member_float_value_is_rejected_via_unit_test() {
        // Exercises the "non-integer value" error path (lines 884-893).
        assert_c0001("class Color(Enum):\n    RED = 1.5\n");
    }

    #[test]
    fn enum_member_bool_value_is_rejected_via_unit_test() {
        // Exercises the "non-integer value" error path (lines 884-893)
        // with a `bool` literal, a distinct match arm from `float`.
        assert_c0001("class Color(Enum):\n    RED = True\n");
    }

    #[test]
    fn enum_class_with_docstring_is_accepted() {
        // #744: a class docstring (a bare string-literal expression
        // statement) is a no-op in an enum body, not a member assignment.
        let hir = lower_ok("class Color(Enum):\n    \"A color.\"\n    RED = 1\n    GREEN = 2\n");
        let (_, class_def) = &hir.class_defs[0];
        assert_eq!(class_def.enum_members.len(), 2);
        assert_eq!(class_def.enum_members[0].0, "RED");
    }

    #[test]
    fn an_enum_class_with_a_non_leading_docstring_is_accepted() {
        // #744's guard has no position check: place the docstring after a
        // member assignment to exercise the non-leading case directly.
        let hir = lower_ok("class Color(Enum):\n    RED = 1\n    \"A color.\"\n    GREEN = 2\n");
        let (_, class_def) = &hir.class_defs[0];
        assert_eq!(class_def.enum_members.len(), 2);
        assert_eq!(class_def.enum_members[0].0, "RED");
        assert_eq!(class_def.enum_members[1].0, "GREEN");
    }

    #[test]
    fn a_non_string_expression_statement_in_an_enum_body_is_still_rejected() {
        // #744's docstring exemption covers only a bare string-literal
        // expression statement: a bare non-string expression statement in
        // an enum body remains C0001, exercising the guard's false branch
        // distinctly from a non-`Stmt::Expr` statement (which already
        // short-circuits before the guard).
        assert_c0001("class Color(Enum):\n    42\n    RED = 1\n");
    }

    #[test]
    fn valid_enum_class_lowers_via_unit_test() {
        // Covers `lower_enum_class`'s `Ok` return path (lines 911-932)
        // inside this crate's own unit-test binary, working around
        // cargo-llvm-cov issue #276 (instantiation-merge gap between
        // the library and integration-test binaries).
        let hir = lower_ok("class Color(Enum):\n    RED = 1\n    GREEN = 2\n    BLUE = 3\n");
        assert_eq!(hir.class_defs.len(), 1);
        let (_, class_def) = &hir.class_defs[0];
        assert!(!class_def.is_protocol);
        assert_eq!(class_def.enum_members.len(), 3);
        assert_eq!(class_def.enum_members[0].0, "RED");
        assert_eq!(class_def.enum_members[0].1, 1);
    }

    #[test]
    fn single_member_enum_class_lowers_via_unit_test() {
        // Additional `lower_enum_class` `Ok` return coverage with a
        // minimal single-member enum.
        let hir = lower_ok("class Color(Enum):\n    RED = 0\n");
        let (_, class_def) = &hir.class_defs[0];
        assert_eq!(class_def.enum_members.len(), 1);
        assert_eq!(class_def.enum_members[0].0, "RED");
        assert_eq!(class_def.enum_members[0].1, 0);
    }
}
