use pycc_diag::{Diagnostic, Span};
use pycc_hir::{BinOpKind, FStringPart, HirExpr, HirItem, HirModule, HirStmt};
#[cfg(test)]
use pycc_hir::CmpOpKind;
pub use pycc_hir::Ty;
use std::collections::HashMap;

#[derive(Debug, Default, Clone)]
pub struct Environment {
    bindings: HashMap<String, Ty>,
    functions: HashMap<String, (Vec<Ty>, Ty)>,
}

impl Environment {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn lookup(&self, name: &str) -> Option<Ty> {
        self.bindings.get(name).copied()
    }

    pub fn bind(&mut self, name: String, ty: Ty) {
        self.bindings.insert(name, ty);
    }

    pub fn bind_function(&mut self, name: String, param_tys: Vec<Ty>, return_ty: Ty) {
        self.functions.insert(name, (param_tys, return_ty));
    }

    pub fn lookup_function(&self, name: &str) -> Option<&(Vec<Ty>, Ty)> {
        self.functions.get(name)
    }
}

pub fn infer_expr(env: &Environment, expr: &HirExpr) -> Result<Ty, Diagnostic> {
    match expr {
        HirExpr::IntLiteral(_) => Ok(Ty::Int),
        HirExpr::FloatLiteral(_) => Ok(Ty::Float),
        HirExpr::BoolLiteral(_) => Ok(Ty::Bool),
        HirExpr::StringLiteral(_) => Ok(Ty::Str),
        HirExpr::FString(parts) => {
            for part in parts {
                if let FStringPart::Interpolation(expr) = part {
                    infer_expr(env, expr)?; // any interpolatable type is allowed; Python str()-coerces at runtime
                }
            }
            Ok(Ty::Str)
        }
        HirExpr::Name(name) => env.lookup(name).ok_or_else(|| {
            Diagnostic::error(
                "T0021",
                format!("name `{name}` is not defined"),
                Span::new(0, 0), // real span threading through HIR is out of scope for this task -- see Task 15's follow-up note
            )
        }),
        HirExpr::BinOp { op, left, right } => {
            let left_ty = infer_expr(env, left)?;
            let right_ty = infer_expr(env, right)?;
            numeric_result_type(*op, left_ty, right_ty)
        }
        HirExpr::Compare { op: _, left, right } => {
            let left_ty = infer_expr(env, left)?;
            let right_ty = infer_expr(env, right)?;
            if numeric_or_bool_compatible(left_ty, right_ty) {
                Ok(Ty::Bool)
            } else {
                Err(Diagnostic::error(
                    "T0021",
                    format!("cannot compare `{}` and `{}`", left_ty.name(), right_ty.name()),
                    Span::new(0, 0),
                ))
            }
        }
        HirExpr::Call { callee, args } => {
            let arg_tys = args.iter().map(|a| infer_expr(env, a)).collect::<Result<Vec<_>, _>>()?;
            if callee == "print" {
                return Ok(Ty::None); // print's own signature isn't user-declarable in v0.1
            }
            let Some((param_tys, return_ty)) = env.lookup_function(callee) else {
                return Err(Diagnostic::error(
                    "T0021",
                    format!("call to undefined function `{callee}`"),
                    Span::new(0, 0),
                ));
            };
            if arg_tys.len() != param_tys.len() {
                return Err(Diagnostic::error(
                    "T0021",
                    format!("`{callee}` expects {} argument(s), got {}", param_tys.len(), arg_tys.len()),
                    Span::new(0, 0),
                ));
            }
            for (i, (arg_ty, param_ty)) in arg_tys.iter().zip(param_tys.iter()).enumerate() {
                if !is_assignable(*arg_ty, *param_ty) {
                    return Err(Diagnostic::error(
                        "T0021",
                        format!(
                            "argument {} of `{callee}` expects `{}`, got `{}`",
                            i + 1,
                            param_ty.name(),
                            arg_ty.name()
                        ),
                        Span::new(0, 0),
                    ));
                }
            }
            Ok(*return_ty)
        }
    }
}

fn is_assignable(from: Ty, to: Ty) -> bool {
    from == to || (from == Ty::Bool && to == Ty::Int) // bool is a subtype of int, TYPE_SYSTEM.md's representation table
}

fn numeric_result_type(op: BinOpKind, left: Ty, right: Ty) -> Result<Ty, Diagnostic> {
    if left == Ty::Str && right == Ty::Str {
        return if op == BinOpKind::Add {
            Ok(Ty::Str)
        } else {
            Err(Diagnostic::error(
                "T0021",
                format!("operator {op:?} is not defined for `str` and `str`"),
                Span::new(0, 0),
            ))
        };
    }
    let as_numeric = |t: Ty| match t {
        Ty::Bool | Ty::Int => Some(Ty::Int),
        Ty::Float => Some(Ty::Float),
        _ => None,
    };
    match (as_numeric(left), as_numeric(right)) {
        (Some(Ty::Int), Some(Ty::Int)) => Ok(Ty::Int),
        (Some(_), Some(_)) => Ok(Ty::Float),
        _ => Err(Diagnostic::error(
            "T0021",
            format!("operator {op:?} is not defined for `{}` and `{}`", left.name(), right.name()),
            Span::new(0, 0),
        )),
    }
}

fn numeric_or_bool_compatible(a: Ty, b: Ty) -> bool {
    let is_numeric_like = |t: Ty| matches!(t, Ty::Int | Ty::Float | Ty::Bool);
    (is_numeric_like(a) && is_numeric_like(b)) || (a == Ty::Str && b == Ty::Str)
}

pub fn check_stmt(env: &mut Environment, stmt: &HirStmt) -> Result<(), Diagnostic> {
    match stmt {
        HirStmt::Assign { target, value } => {
            let ty = infer_expr(env, value)?;
            env.bind(target.clone(), ty);
            Ok(())
        }
        HirStmt::ExprStmt(expr) => infer_expr(env, expr).map(|_| ()),
        HirStmt::If { test, body, orelse } => {
            infer_expr(env, test)?; // any type is accepted as truthy for v0.1 -- Python's own truthiness has no static type restriction
            for stmt in body {
                check_stmt(env, stmt)?;
            }
            for stmt in orelse {
                check_stmt(env, stmt)?;
            }
            Ok(())
        }
        HirStmt::While { test, body } => {
            infer_expr(env, test)?;
            for stmt in body {
                check_stmt(env, stmt)?;
            }
            Ok(())
        }
        HirStmt::ForRange { var, start, stop, step, body } => {
            infer_expr(env, start)?;
            infer_expr(env, stop)?;
            infer_expr(env, step)?;
            env.bind(var.clone(), Ty::Int);
            for stmt in body {
                check_stmt(env, stmt)?;
            }
            Ok(())
        }
        HirStmt::Return(_) => {
            panic!("pycc_types: a return statement outside of a function is not supported")
        }
    }
}

pub fn check_function(function: &HirItem) -> Result<(), Diagnostic> {
    check_function_in(&Environment::new(), function)
}

/// Checks one function's body, resolving sibling calls and module-level
/// global reads against a clone of `module_env` (see D-039/D-040) instead
/// of an isolated, self-only scope. Cloning (not sharing) `module_env`
/// means a function's own parameter bindings and local assignments never
/// leak back into the module scope or into any other function's check.
fn check_function_in(module_env: &Environment, function: &HirItem) -> Result<(), Diagnostic> {
    let HirItem::Function { name, params, return_ty, body } = function else {
        panic!("check_function called with a non-Function HirItem");
    };
    let mut env = module_env.clone();
    env.bind_function(name.clone(), params.iter().map(|(_, ty)| *ty).collect(), *return_ty);
    for (param_name, param_ty) in params {
        env.bind(param_name.clone(), *param_ty);
    }
    for stmt in body {
        check_stmt_in_function(&mut env, stmt, *return_ty)?;
    }
    Ok(())
}

fn check_stmt_in_function(env: &mut Environment, stmt: &HirStmt, return_ty: Ty) -> Result<(), Diagnostic> {
    match stmt {
        HirStmt::Return(None) => {
            if return_ty != Ty::None {
                return Err(Diagnostic::error(
                    "T0023",
                    format!("expected a return value of type `{}`, got none", return_ty.name()),
                    Span::new(0, 0),
                ));
            }
            Ok(())
        }
        HirStmt::Return(Some(expr)) => {
            let actual = infer_expr(env, expr)?;
            if !is_assignable(actual, return_ty) {
                return Err(Diagnostic::error(
                    "T0023",
                    format!("expected return type `{}`, got `{}`", return_ty.name(), actual.name()),
                    Span::new(0, 0),
                ));
            }
            Ok(())
        }
        HirStmt::If { test, body, orelse } => {
            infer_expr(env, test)?;
            for s in body {
                check_stmt_in_function(env, s, return_ty)?;
            }
            for s in orelse {
                check_stmt_in_function(env, s, return_ty)?;
            }
            Ok(())
        }
        HirStmt::While { test, body } => {
            infer_expr(env, test)?;
            for s in body {
                check_stmt_in_function(env, s, return_ty)?;
            }
            Ok(())
        }
        HirStmt::ForRange { var, start, stop, step, body } => {
            infer_expr(env, start)?;
            infer_expr(env, stop)?;
            infer_expr(env, step)?;
            env.bind(var.clone(), Ty::Int);
            for s in body {
                check_stmt_in_function(env, s, return_ty)?;
            }
            Ok(())
        }
        other => check_stmt(env, other),
    }
}

pub fn check(hir: &HirModule) -> Result<(), Diagnostic> {
    let mut env = Environment::new();
    // Pass 1: register every function's signature before checking any
    // statement body, matching Python's own "a module runs top to bottom,
    // but any def already executed is callable" semantics -- top-level
    // code and other function bodies (D-039) both need to see every
    // function regardless of its position in the file.
    for item in &hir.items {
        if let HirItem::Function { name, params, return_ty, .. } = item {
            env.bind_function(name.clone(), params.iter().map(|(_, ty)| *ty).collect(), *return_ty);
        }
    }
    // Pass 2: check every top-level statement in source order, growing
    // `env`'s bindings as module-level assignments are encountered --
    // ordinary top-level code is still checked top-to-bottom (a top-level
    // forward reference to a not-yet-assigned name is a genuine error).
    for item in &hir.items {
        if let HirItem::TopLevelStmt(stmt) = item {
            check_stmt(&mut env, stmt)?;
        }
    }
    // Pass 3: check every function body against a clone of `env` as it
    // stands once the whole module's top-level code has been processed
    // (D-040) -- a function can read any module-level global regardless of
    // whether its own `def` appears before or after that global's
    // assignment in the file, since real Python only evaluates a function
    // body when it's *called*, typically after the module has finished
    // running top to bottom.
    for item in &hir.items {
        if let HirItem::Function { .. } = item {
            check_function_in(&env, item)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v0_1_slice_always_type_checks() {
        let hir = HirModule { items: vec![] };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn infers_an_int_literal_as_int() {
        let env = Environment::new();
        assert_eq!(infer_expr(&env, &HirExpr::IntLiteral(1)), Ok(Ty::Int));
    }

    #[test]
    fn infers_a_float_literal_as_float() {
        let env = Environment::new();
        assert_eq!(infer_expr(&env, &HirExpr::FloatLiteral(1.5)), Ok(Ty::Float));
    }

    #[test]
    fn infers_a_bool_literal_as_bool() {
        let env = Environment::new();
        assert_eq!(infer_expr(&env, &HirExpr::BoolLiteral(true)), Ok(Ty::Bool));
    }

    #[test]
    fn infers_a_string_literal_as_str() {
        let env = Environment::new();
        assert_eq!(infer_expr(&env, &HirExpr::StringLiteral("hi".to_string())), Ok(Ty::Str));
    }

    #[test]
    fn adding_an_int_and_a_str_is_a_clean_type_error() {
        let env = Environment::new();
        let expr = HirExpr::BinOp {
            op: BinOpKind::Add,
            left: Box::new(HirExpr::IntLiteral(1)),
            right: Box::new(HirExpr::StringLiteral("x".to_string())),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn adding_two_strings_infers_str() {
        let env = Environment::new();
        let expr = HirExpr::BinOp {
            op: BinOpKind::Add,
            left: Box::new(HirExpr::StringLiteral("a".to_string())),
            right: Box::new(HirExpr::StringLiteral("b".to_string())),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Str));
    }

    #[test]
    fn subtracting_two_strings_is_a_clean_type_error() {
        // Python allows `"a" + "b"` but no other arithmetic operator between
        // two strings -- `"a" - "b"` is a `TypeError` at runtime in CPython.
        let env = Environment::new();
        let expr = HirExpr::BinOp {
            op: BinOpKind::Sub,
            left: Box::new(HirExpr::StringLiteral("a".to_string())),
            right: Box::new(HirExpr::StringLiteral("b".to_string())),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn comparing_two_strings_infers_bool() {
        // `"a" == "b"`, `"a" < "b"`, etc. are ordinary, valid Python
        // (lexicographic ordering) -- not covered by numeric_or_bool_compatible
        // before `Ty::Str` became constructible via literals.
        let env = Environment::new();
        for op in [
            CmpOpKind::Eq,
            CmpOpKind::NotEq,
            CmpOpKind::Lt,
            CmpOpKind::LtE,
            CmpOpKind::Gt,
            CmpOpKind::GtE,
        ] {
            let expr = HirExpr::Compare {
                op,
                left: Box::new(HirExpr::StringLiteral("a".to_string())),
                right: Box::new(HirExpr::StringLiteral("b".to_string())),
            };
            assert_eq!(infer_expr(&env, &expr), Ok(Ty::Bool), "comparison {op:?} should type-check");
        }
    }

    #[test]
    fn an_f_string_always_infers_str_regardless_of_interpolated_types() {
        let env = Environment::new();
        let expr = HirExpr::FString(vec![
            FStringPart::Literal("n=".to_string()),
            FStringPart::Interpolation(Box::new(HirExpr::IntLiteral(1))),
        ]);
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Str));
    }

    #[test]
    fn an_f_string_still_type_checks_its_interpolated_expressions() {
        let env = Environment::new();
        let expr =
            HirExpr::FString(vec![FStringPart::Interpolation(Box::new(HirExpr::Name("undefined".to_string())))]);
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn comparing_a_string_and_an_int_is_a_clean_type_error() {
        let env = Environment::new();
        let expr = HirExpr::Compare {
            op: CmpOpKind::Eq,
            left: Box::new(HirExpr::StringLiteral("a".to_string())),
            right: Box::new(HirExpr::IntLiteral(1)),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn comparing_two_ints_infers_bool() {
        let env = Environment::new();
        let expr = HirExpr::Compare {
            op: CmpOpKind::Lt,
            left: Box::new(HirExpr::IntLiteral(1)),
            right: Box::new(HirExpr::IntLiteral(2)),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Bool));
    }

    #[test]
    fn comparing_a_bool_and_an_int_succeeds_since_bool_is_a_subtype_of_int() {
        let env = Environment::new();
        let expr = HirExpr::Compare {
            op: CmpOpKind::Eq,
            left: Box::new(HirExpr::IntLiteral(1)),
            right: Box::new(HirExpr::BoolLiteral(true)),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Bool));
    }

    #[test]
    fn comparing_an_undefined_left_operand_propagates_the_error() {
        let env = Environment::new();
        let expr = HirExpr::Compare {
            op: CmpOpKind::Eq,
            left: Box::new(HirExpr::Name("undefined".to_string())),
            right: Box::new(HirExpr::IntLiteral(1)),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn comparing_an_undefined_right_operand_propagates_the_error() {
        let env = Environment::new();
        let expr = HirExpr::Compare {
            op: CmpOpKind::Eq,
            left: Box::new(HirExpr::IntLiteral(1)),
            right: Box::new(HirExpr::Name("undefined".to_string())),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn comparing_incompatible_types_is_a_clean_type_error() {
        let mut env = Environment::new();
        // A call to a properly declared, zero-arg, `None`-returning function
        // legitimately infers `Ty::None`, which isn't numeric-like --
        // comparing an int against it is a genuine, both-sides-defined
        // incompatibility.
        env.bind_function("f".to_string(), vec![], Ty::None);
        let expr = HirExpr::Compare {
            op: CmpOpKind::Eq,
            left: Box::new(HirExpr::IntLiteral(1)),
            right: Box::new(HirExpr::Call { callee: "f".to_string(), args: vec![] }),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("int") && err.message.contains("None"));
    }

    #[test]
    fn a_binop_treats_bool_as_int() {
        let env = Environment::new();
        let expr = HirExpr::BinOp {
            op: BinOpKind::Add,
            left: Box::new(HirExpr::BoolLiteral(true)),
            right: Box::new(HirExpr::IntLiteral(1)),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Int));
    }

    #[test]
    fn a_binop_treats_bool_and_float_as_float() {
        let env = Environment::new();
        let expr = HirExpr::BinOp {
            op: BinOpKind::Add,
            left: Box::new(HirExpr::BoolLiteral(true)),
            right: Box::new(HirExpr::FloatLiteral(1.5)),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Float));
    }

    #[test]
    #[should_panic(expected = "a return statement outside of a function is not supported")]
    fn a_top_level_return_is_unsupported() {
        let mut env = Environment::new();
        let _ = check_stmt(&mut env, &HirStmt::Return(None));
    }

    #[test]
    fn an_assignment_binds_the_inferred_type_in_the_environment() {
        let mut env = Environment::new();
        check_stmt(&mut env, &HirStmt::Assign { target: "x".to_string(), value: HirExpr::IntLiteral(1) })
            .unwrap();
        assert_eq!(env.lookup("x"), Some(Ty::Int));
    }

    #[test]
    fn an_assignment_whose_value_is_undefined_propagates_the_error() {
        let mut env = Environment::new();
        let err = check_stmt(
            &mut env,
            &HirStmt::Assign { target: "x".to_string(), value: HirExpr::Name("undefined".to_string()) },
        )
        .unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(env.lookup("x"), None);
    }

    #[test]
    fn an_if_s_test_must_be_bool_like_and_both_branches_are_checked() {
        let mut env = Environment::new();
        let stmt = HirStmt::If {
            test: HirExpr::BoolLiteral(true),
            body: vec![HirStmt::Assign { target: "x".to_string(), value: HirExpr::IntLiteral(1) }],
            orelse: vec![HirStmt::Assign { target: "y".to_string(), value: HirExpr::IntLiteral(2) }],
        };
        check_stmt(&mut env, &stmt).unwrap();
        // Both branches ran in the same (single, unscoped-per-branch)
        // environment for v0.1's simplified model -- neither branch's
        // bindings are undone; real flow-sensitive narrowing is out of scope.
        assert_eq!(env.lookup("x"), Some(Ty::Int));
        assert_eq!(env.lookup("y"), Some(Ty::Int));
    }

    #[test]
    fn an_if_whose_test_is_undefined_propagates_the_error() {
        let mut env = Environment::new();
        let stmt = HirStmt::If {
            test: HirExpr::Name("undefined".to_string()),
            body: vec![],
            orelse: vec![],
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0021");
    }

    #[test]
    fn an_if_whose_body_statement_is_ill_typed_propagates_the_error() {
        let mut env = Environment::new();
        let stmt = HirStmt::If {
            test: HirExpr::BoolLiteral(true),
            body: vec![HirStmt::ExprStmt(HirExpr::Name("undefined".to_string()))],
            orelse: vec![],
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0021");
    }

    #[test]
    fn an_if_whose_orelse_statement_is_ill_typed_propagates_the_error() {
        let mut env = Environment::new();
        let stmt = HirStmt::If {
            test: HirExpr::BoolLiteral(true),
            body: vec![],
            orelse: vec![HirStmt::ExprStmt(HirExpr::Name("undefined".to_string()))],
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_while_loop_s_test_and_body_are_checked() {
        let mut env = Environment::new();
        let stmt = HirStmt::While {
            test: HirExpr::BoolLiteral(true),
            body: vec![HirStmt::Assign { target: "x".to_string(), value: HirExpr::IntLiteral(1) }],
        };
        check_stmt(&mut env, &stmt).unwrap();
        assert_eq!(env.lookup("x"), Some(Ty::Int));
    }

    #[test]
    fn a_while_loop_whose_test_is_undefined_propagates_the_error() {
        let mut env = Environment::new();
        let stmt = HirStmt::While { test: HirExpr::Name("undefined".to_string()), body: vec![] };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_while_loop_whose_body_statement_is_ill_typed_propagates_the_error() {
        let mut env = Environment::new();
        let stmt = HirStmt::While {
            test: HirExpr::BoolLiteral(true),
            body: vec![HirStmt::ExprStmt(HirExpr::Name("undefined".to_string()))],
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_for_range_loop_binds_its_variable_as_int_and_checks_its_body() {
        let mut env = Environment::new();
        let stmt = HirStmt::ForRange {
            var: "i".to_string(),
            start: HirExpr::IntLiteral(0),
            stop: HirExpr::IntLiteral(3),
            step: HirExpr::IntLiteral(1),
            body: vec![HirStmt::Assign { target: "x".to_string(), value: HirExpr::Name("i".to_string()) }],
        };
        check_stmt(&mut env, &stmt).unwrap();
        assert_eq!(env.lookup("i"), Some(Ty::Int));
        assert_eq!(env.lookup("x"), Some(Ty::Int));
    }

    #[test]
    fn a_for_range_loop_whose_start_is_undefined_propagates_the_error() {
        let mut env = Environment::new();
        let stmt = HirStmt::ForRange {
            var: "i".to_string(),
            start: HirExpr::Name("undefined".to_string()),
            stop: HirExpr::IntLiteral(3),
            step: HirExpr::IntLiteral(1),
            body: vec![],
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_for_range_loop_whose_stop_is_undefined_propagates_the_error() {
        let mut env = Environment::new();
        let stmt = HirStmt::ForRange {
            var: "i".to_string(),
            start: HirExpr::IntLiteral(0),
            stop: HirExpr::Name("undefined".to_string()),
            step: HirExpr::IntLiteral(1),
            body: vec![],
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_for_range_loop_whose_step_is_undefined_propagates_the_error() {
        let mut env = Environment::new();
        let stmt = HirStmt::ForRange {
            var: "i".to_string(),
            start: HirExpr::IntLiteral(0),
            stop: HirExpr::IntLiteral(3),
            step: HirExpr::Name("undefined".to_string()),
            body: vec![],
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_for_range_loop_whose_body_statement_is_ill_typed_propagates_the_error() {
        let mut env = Environment::new();
        let stmt = HirStmt::ForRange {
            var: "i".to_string(),
            start: HirExpr::IntLiteral(0),
            stop: HirExpr::IntLiteral(3),
            step: HirExpr::IntLiteral(1),
            body: vec![HirStmt::ExprStmt(HirExpr::Name("undefined".to_string()))],
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0021");
    }

    #[test]
    fn referencing_an_assigned_name_infers_its_bound_type() {
        let mut env = Environment::new();
        check_stmt(&mut env, &HirStmt::Assign { target: "x".to_string(), value: HirExpr::IntLiteral(1) })
            .unwrap();
        assert_eq!(infer_expr(&env, &HirExpr::Name("x".to_string())), Ok(Ty::Int));
    }

    #[test]
    fn adding_two_ints_infers_int() {
        let env = Environment::new();
        let expr = HirExpr::BinOp {
            op: BinOpKind::Add,
            left: Box::new(HirExpr::IntLiteral(1)),
            right: Box::new(HirExpr::IntLiteral(2)),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Int));
    }

    #[test]
    fn a_binop_with_an_undefined_left_operand_propagates_the_error() {
        let env = Environment::new();
        let expr = HirExpr::BinOp {
            op: BinOpKind::Add,
            left: Box::new(HirExpr::Name("undefined".to_string())),
            right: Box::new(HirExpr::IntLiteral(1)),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn a_binop_with_an_undefined_right_operand_propagates_the_error() {
        let env = Environment::new();
        let expr = HirExpr::BinOp {
            op: BinOpKind::Add,
            left: Box::new(HirExpr::IntLiteral(1)),
            right: Box::new(HirExpr::Name("undefined".to_string())),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn numeric_result_type_covers_every_int_float_combination() {
        assert_eq!(numeric_result_type(BinOpKind::Add, Ty::Float, Ty::Float), Ok(Ty::Float));
        assert_eq!(numeric_result_type(BinOpKind::Add, Ty::Float, Ty::Int), Ok(Ty::Float));
    }

    #[test]
    fn referencing_an_undefined_name_is_a_clean_error_not_a_panic() {
        let env = Environment::new();
        let err = infer_expr(&env, &HirExpr::Name("undefined".to_string())).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("undefined"));
    }

    #[test]
    fn numeric_result_type_rejects_a_hypothetical_incompatible_pair() {
        let err = numeric_result_type(BinOpKind::Add, Ty::Int, Ty::None).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn adding_an_int_and_a_float_promotes_to_float() {
        let env = Environment::new();
        let expr = HirExpr::BinOp {
            op: BinOpKind::Add,
            left: Box::new(HirExpr::IntLiteral(1)),
            right: Box::new(HirExpr::FloatLiteral(2.5)),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Float));
    }

    #[test]
    fn numeric_result_type_accepts_float_and_bool_since_bool_is_numeric_like() {
        // Task 7 makes `bool` numeric-like everywhere (`True + 1.5 == 2.5` is
        // legal Python), so this pair is no longer an error -- see
        // `a_binop_treats_bool_and_float_as_float` for the `infer_expr`-level
        // version of this same rule.
        assert_eq!(numeric_result_type(BinOpKind::Add, Ty::Float, Ty::Bool), Ok(Ty::Float));
    }

    #[test]
    fn numeric_result_type_rejects_a_float_and_a_hypothetical_none() {
        // Exercises `.name()` for `Float` in the error arm now that
        // `Float`+`Bool` no longer takes that path.
        let err = numeric_result_type(BinOpKind::Add, Ty::Float, Ty::None).unwrap_err();
        assert!(err.message.contains("float") && err.message.contains("None"));
    }

    #[test]
    fn numeric_result_type_rejects_a_hypothetical_str_operand() {
        let err = numeric_result_type(BinOpKind::Add, Ty::Bool, Ty::Str).unwrap_err();
        assert!(err.message.contains("str"));
    }

    #[test]
    fn a_top_level_binary_addition_type_checks() {
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(HirExpr::IntLiteral(1)),
                right: Box::new(HirExpr::IntLiteral(2)),
            }))],
        };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn a_top_level_reference_to_an_undefined_name_is_a_clean_error() {
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Name("undefined".to_string())))],
        };
        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn a_top_level_call_to_a_previously_defined_function_type_checks() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "main".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![HirStmt::Return(None)],
                },
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "main".to_string(),
                    args: vec![],
                })),
            ],
        };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn a_function_can_call_a_sibling_function_defined_before_it() {
        // Regression test for D-039: `check_function`'s own env used to be
        // seeded empty, so `main` couldn't see `helper` even though both are
        // ordinary module-level functions -- a valid, non-recursive call
        // between two sibling functions was wrongly rejected with T0021.
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "helper".to_string(),
                    params: vec![("x".to_string(), Ty::Int)],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                        op: BinOpKind::Add,
                        left: Box::new(HirExpr::Name("x".to_string())),
                        right: Box::new(HirExpr::IntLiteral(1)),
                    }))],
                },
                HirItem::Function {
                    name: "main".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![HirStmt::ExprStmt(HirExpr::Call {
                        callee: "print".to_string(),
                        args: vec![HirExpr::Call {
                            callee: "helper".to_string(),
                            args: vec![HirExpr::IntLiteral(5)],
                        }],
                    })],
                },
            ],
        };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn a_function_can_call_a_sibling_function_defined_after_it() {
        // Same gap as above, but exercising the pre-registration pass (D-038)
        // from the *other* direction: `main` is checked first (it's first in
        // the module) yet still must see `helper`, which is defined later.
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "main".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![HirStmt::ExprStmt(HirExpr::Call {
                        callee: "print".to_string(),
                        args: vec![HirExpr::Call {
                            callee: "helper".to_string(),
                            args: vec![HirExpr::IntLiteral(5)],
                        }],
                    })],
                },
                HirItem::Function {
                    name: "helper".to_string(),
                    params: vec![("x".to_string(), Ty::Int)],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                        op: BinOpKind::Add,
                        left: Box::new(HirExpr::Name("x".to_string())),
                        right: Box::new(HirExpr::IntLiteral(1)),
                    }))],
                },
            ],
        };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn a_function_can_read_a_module_level_global_defined_before_it() {
        // Regression test for D-040: reading a module global from a function
        // body needs no `global` declaration in real Python (that's only
        // required to *rebind* one) -- child_for_function used to reset
        // bindings to empty, so `f`'s body couldn't see `x` even though it's
        // an ordinary module-level constant, not some caller's local.
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign { target: "x".to_string(), value: HirExpr::IntLiteral(5) }),
                HirItem::Function {
                    name: "f".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
                },
            ],
        };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn a_function_can_read_a_module_level_global_defined_after_it() {
        // Same gap, other direction: a function is only ever *called* after
        // the module has (typically) finished running top to bottom, so a
        // global defined later in the file is still visible inside an
        // earlier function's body.
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "f".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
                },
                HirItem::TopLevelStmt(HirStmt::Assign { target: "x".to_string(), value: HirExpr::IntLiteral(5) }),
            ],
        };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn a_function_parameter_shadows_a_module_level_global_of_the_same_name() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::StringLiteral("global".to_string()),
                }),
                HirItem::Function {
                    name: "f".to_string(),
                    params: vec![("x".to_string(), Ty::Int)],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
                },
            ],
        };
        // If the global (Ty::Str) leaked through instead of the parameter
        // (Ty::Int), this would fail with a T0023 return-type mismatch.
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn check_function_the_public_api_still_has_no_sibling_visibility() {
        // `check_function` is a standalone entry point with no module
        // context, so it must keep working exactly as before: it only ever
        // sees its own signature (needed for recursion), never a sibling's.
        let function = HirItem::Function {
            name: "main".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::ExprStmt(HirExpr::Call { callee: "helper".to_string(), args: vec![] })],
        };
        let err = check_function(&function).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn a_function_body_is_now_checked() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: Ty::None,
                body: vec![HirStmt::ExprStmt(HirExpr::Name("undefined".to_string()))],
            }],
        };
        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn a_bare_call_infers_none() {
        let env = Environment::new();
        let expr = HirExpr::Call { callee: "print".to_string(), args: vec![] };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::None));
    }

    #[test]
    fn calling_an_undefined_function_is_a_clean_error() {
        let env = Environment::new();
        let expr = HirExpr::Call { callee: "undefined".to_string(), args: vec![] };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("undefined"));
    }

    #[test]
    fn calling_a_defined_function_infers_its_declared_return_type() {
        let mut env = Environment::new();
        env.bind_function("add".to_string(), vec![Ty::Int, Ty::Int], Ty::Int);
        let expr = HirExpr::Call {
            callee: "add".to_string(),
            args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Int));
    }

    #[test]
    fn calling_a_function_with_a_bool_argument_for_an_int_parameter_succeeds() {
        let mut env = Environment::new();
        env.bind_function("f".to_string(), vec![Ty::Int], Ty::None);
        let expr = HirExpr::Call { callee: "f".to_string(), args: vec![HirExpr::BoolLiteral(true)] };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::None));
    }

    #[test]
    fn calling_a_function_with_the_wrong_number_of_arguments_is_a_clean_error() {
        let mut env = Environment::new();
        env.bind_function("add".to_string(), vec![Ty::Int, Ty::Int], Ty::Int);
        let expr = HirExpr::Call { callee: "add".to_string(), args: vec![HirExpr::IntLiteral(1)] };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("expects 2 argument"));
    }

    #[test]
    fn calling_a_function_with_a_wrong_typed_argument_is_a_clean_error() {
        let mut env = Environment::new();
        env.bind_function("add".to_string(), vec![Ty::Int, Ty::Int], Ty::Int);
        let expr = HirExpr::Call {
            callee: "add".to_string(),
            args: vec![HirExpr::IntLiteral(1), HirExpr::FloatLiteral(2.5)],
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("argument 2") && err.message.contains("int") && err.message.contains("float"));
    }

    #[test]
    fn calling_a_function_with_an_undefined_argument_propagates_the_error() {
        let mut env = Environment::new();
        env.bind_function("f".to_string(), vec![Ty::Int], Ty::None);
        let expr =
            HirExpr::Call { callee: "f".to_string(), args: vec![HirExpr::Name("undefined".to_string())] };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn a_function_s_body_is_checked_against_its_declared_param_types() {
        let function = HirItem::Function {
            name: "add".to_string(),
            params: vec![("a".to_string(), Ty::Int), ("b".to_string(), Ty::Int)],
            return_ty: Ty::Int,
            body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(HirExpr::Name("a".to_string())),
                right: Box::new(HirExpr::Name("b".to_string())),
            }))],
        };
        check_function(&function).unwrap();
    }

    #[test]
    fn a_return_with_no_value_when_none_is_expected_succeeds() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::Return(None)],
        };
        check_function(&function).unwrap();
    }

    #[test]
    fn a_return_with_no_value_when_a_value_is_expected_is_a_clean_error() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::Int,
            body: vec![HirStmt::Return(None)],
        };
        let err = check_function(&function).unwrap_err();
        assert_eq!(err.code, "T0023");
    }

    #[test]
    fn a_return_type_mismatch_is_a_clean_error() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::Str,
            body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
        };
        let err = check_function(&function).unwrap_err();
        assert_eq!(err.code, "T0023");
    }

    #[test]
    fn a_return_whose_value_is_undefined_propagates_the_error() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::Int,
            body: vec![HirStmt::Return(Some(HirExpr::Name("undefined".to_string())))],
        };
        let err = check_function(&function).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn recursion_is_supported_since_the_function_s_own_signature_is_in_scope() {
        let function = HirItem::Function {
            name: "count".to_string(),
            params: vec![("n".to_string(), Ty::Int)],
            return_ty: Ty::Int,
            body: vec![HirStmt::Return(Some(HirExpr::Call {
                callee: "count".to_string(),
                args: vec![HirExpr::Name("n".to_string())],
            }))],
        };
        check_function(&function).unwrap();
    }

    #[test]
    fn a_function_s_if_while_and_for_bodies_are_checked_against_its_return_type() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::Int,
            body: vec![HirStmt::If {
                test: HirExpr::BoolLiteral(true),
                body: vec![HirStmt::While {
                    test: HirExpr::BoolLiteral(true),
                    body: vec![HirStmt::ForRange {
                        var: "i".to_string(),
                        start: HirExpr::IntLiteral(0),
                        stop: HirExpr::IntLiteral(1),
                        step: HirExpr::IntLiteral(1),
                        body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                    }],
                }],
                orelse: vec![HirStmt::Return(Some(HirExpr::IntLiteral(0)))],
            }],
        };
        check_function(&function).unwrap();
    }

    #[test]
    fn a_bad_return_nested_in_if_while_and_for_is_still_caught() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::Str,
            body: vec![HirStmt::If {
                test: HirExpr::BoolLiteral(true),
                body: vec![HirStmt::While {
                    test: HirExpr::BoolLiteral(true),
                    body: vec![HirStmt::ForRange {
                        var: "i".to_string(),
                        start: HirExpr::IntLiteral(0),
                        stop: HirExpr::IntLiteral(1),
                        step: HirExpr::IntLiteral(1),
                        body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                    }],
                }],
                orelse: vec![],
            }],
        };
        let err = check_function(&function).unwrap_err();
        assert_eq!(err.code, "T0023");
    }

    #[test]
    #[should_panic(expected = "check_function called with a non-Function HirItem")]
    fn check_function_panics_on_a_non_function_item() {
        let _ = check_function(&HirItem::TopLevelStmt(HirStmt::Return(None)));
    }

    #[test]
    fn an_if_s_test_undefined_in_a_function_body_propagates_the_error() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::If {
                test: HirExpr::Name("undefined".to_string()),
                body: vec![],
                orelse: vec![],
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0021");
    }

    #[test]
    fn an_if_s_orelse_ill_typed_in_a_function_body_propagates_the_error() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::If {
                test: HirExpr::BoolLiteral(true),
                body: vec![],
                orelse: vec![HirStmt::ExprStmt(HirExpr::Name("undefined".to_string()))],
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_while_s_test_undefined_in_a_function_body_propagates_the_error() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::While { test: HirExpr::Name("undefined".to_string()), body: vec![] }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_for_range_s_start_undefined_in_a_function_body_propagates_the_error() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::ForRange {
                var: "i".to_string(),
                start: HirExpr::Name("undefined".to_string()),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
                body: vec![],
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_for_range_s_stop_undefined_in_a_function_body_propagates_the_error() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::ForRange {
                var: "i".to_string(),
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::Name("undefined".to_string()),
                step: HirExpr::IntLiteral(1),
                body: vec![],
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_for_range_s_step_undefined_in_a_function_body_propagates_the_error() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::ForRange {
                var: "i".to_string(),
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::Name("undefined".to_string()),
                body: vec![],
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0021");
    }
}
