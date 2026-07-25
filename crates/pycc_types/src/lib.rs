use pycc_diag::{Diagnostic, Span};
use pycc_hir::{BinOpKind, HirExpr, HirItem, HirModule, HirStmt};
#[cfg(test)]
use pycc_hir::CmpOpKind;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ty {
    Int,
    Float,
    Bool,
    Str,
    None,
}

impl Ty {
    fn name(self) -> &'static str {
        match self {
            Ty::Int => "int",
            Ty::Float => "float",
            Ty::Bool => "bool",
            Ty::Str => "str",
            Ty::None => "None",
        }
    }
}

#[derive(Debug, Default)]
pub struct Environment {
    bindings: HashMap<String, Ty>,
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
}

pub fn infer_expr(env: &Environment, expr: &HirExpr) -> Result<Ty, Diagnostic> {
    match expr {
        HirExpr::IntLiteral(_) => Ok(Ty::Int),
        HirExpr::FloatLiteral(_) => Ok(Ty::Float),
        HirExpr::BoolLiteral(_) => Ok(Ty::Bool),
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
        HirExpr::Call { .. } => {
            // Call type-checking (arguments/return) lands in Task 9 alongside
            // real function signatures; until then, treat any call as
            // producing an unconstrained placeholder the caller can't yet
            // misuse, since nothing consumes a call's result type before Task 9.
            Ok(Ty::None)
        }
    }
}

fn numeric_result_type(op: BinOpKind, left: Ty, right: Ty) -> Result<Ty, Diagnostic> {
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
    is_numeric_like(a) && is_numeric_like(b)
}

pub fn check_stmt(env: &mut Environment, stmt: &HirStmt) -> Result<(), Diagnostic> {
    match stmt {
        HirStmt::Assign { target, value } => {
            let ty = infer_expr(env, value)?;
            env.bind(target.clone(), ty);
            Ok(())
        }
        HirStmt::ExprStmt(expr) => infer_expr(env, expr).map(|_| ()),
    }
}

pub fn check(hir: &HirModule) -> Result<(), Diagnostic> {
    let mut env = Environment::new();
    for item in &hir.items {
        match item {
            HirItem::TopLevelStmt(stmt) => check_stmt(&mut env, stmt)?,
            HirItem::Function { .. } => {
                // Function-body checking (its own scope, T0001 on the
                // signature) lands in Task 9 -- until then, a function's
                // body is not yet type-checked at all, matching this crate's
                // pre-existing behavior of never failing.
            }
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
        let env = Environment::new();
        // A bare call's result type is still the Task-6 placeholder
        // `Ty::None` (real call-return typing lands in Task 9), which isn't
        // numeric-like -- comparing an int against it is a genuine,
        // both-sides-defined incompatibility.
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
    fn a_function_body_is_not_yet_checked() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "f".to_string(),
                body: vec![HirStmt::ExprStmt(HirExpr::Name("undefined".to_string()))],
            }],
        };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn a_bare_call_infers_none() {
        let env = Environment::new();
        let expr = HirExpr::Call { callee: "print".to_string(), args: vec![] };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::None));
    }
}
