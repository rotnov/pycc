use pycc_ast::{CmpOp, ElifElseClause, Expr, ModModule, Number, Operator, Stmt};
use pycc_diag::{Diagnostic, Span};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ty {
    Int,
    Float,
    Bool,
    Str,
    None,
}

impl Ty {
    pub fn name(self) -> &'static str {
        match self {
            Ty::Int => "int",
            Ty::Float => "float",
            Ty::Bool => "bool",
            Ty::Str => "str",
            Ty::None => "None",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOpKind {
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv,
    Mod,
    Pow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOpKind {
    Eq,
    NotEq,
    Lt,
    LtE,
    Gt,
    GtE,
}

#[derive(Debug, Clone, PartialEq)]
pub enum HirExpr {
    IntLiteral(i64),
    FloatLiteral(f64),
    BoolLiteral(bool),
    StringLiteral(String),
    Name(String),
    Call { callee: String, args: Vec<HirExpr> },
    BinOp { op: BinOpKind, left: Box<HirExpr>, right: Box<HirExpr> },
    Compare { op: CmpOpKind, left: Box<HirExpr>, right: Box<HirExpr> },
}

#[derive(Debug, PartialEq)]
pub enum HirStmt {
    ExprStmt(HirExpr),
    Assign { target: String, value: HirExpr },
    If { test: HirExpr, body: Vec<HirStmt>, orelse: Vec<HirStmt> },
    While { test: HirExpr, body: Vec<HirStmt> },
    ForRange { var: String, start: HirExpr, stop: HirExpr, step: HirExpr, body: Vec<HirStmt> },
    Return(Option<HirExpr>),
}

#[derive(Debug, PartialEq)]
pub enum HirItem {
    Function { name: String, params: Vec<(String, Ty)>, return_ty: Ty, body: Vec<HirStmt> },
    TopLevelStmt(HirStmt),
}

#[derive(Debug)]
pub struct HirModule {
    pub items: Vec<HirItem>,
}

pub fn lower_checked(module: &ModModule) -> Result<HirModule, Diagnostic> {
    let items = module
        .body
        .iter()
        .map(|stmt| match stmt {
            Stmt::FunctionDef(def) => lower_function(def),
            other => Ok(HirItem::TopLevelStmt(lower_stmt(other))),
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(HirModule { items })
}

fn lower_function(def: &pycc_ast::StmtFunctionDef) -> Result<HirItem, Diagnostic> {
    let is_public = !def.name.as_str().starts_with('_'); // D-037
    let params = lower_params(&def.parameters, is_public, def.name.as_str())?;
    let return_ty = lower_return_annotation(def.returns.as_deref(), is_public, def.name.as_str())?;
    let body = def.body.iter().map(lower_stmt).collect();
    Ok(HirItem::Function { name: def.name.to_string(), params, return_ty, body })
}

fn lower_params(
    parameters: &pycc_ast::Parameters,
    is_public: bool,
    fn_name: &str,
) -> Result<Vec<(String, Ty)>, Diagnostic> {
    parameters
        .args
        .iter()
        .map(|param| {
            let name = param.parameter.name.as_str();
            match &param.parameter.annotation {
                Some(ann) => Ok((name.to_string(), annotation_to_ty(ann)?)),
                None if is_public => Err(Diagnostic::error(
                    "T0001",
                    format!("parameter `{name}` of public function `{fn_name}` needs a type annotation"),
                    Span::new(0, 0),
                )),
                None => Ok((name.to_string(), Ty::None)), // private helper: unannotated is allowed; real inference is a later task's job
            }
        })
        .collect()
}

fn lower_return_annotation(returns: Option<&Expr>, is_public: bool, fn_name: &str) -> Result<Ty, Diagnostic> {
    match returns {
        Some(ann) => annotation_to_ty(ann),
        None if is_public => Err(Diagnostic::error(
            "T0001",
            format!("public function `{fn_name}` needs a return type annotation"),
            Span::new(0, 0),
        )),
        None => Ok(Ty::None),
    }
}

fn annotation_to_ty(annotation: &Expr) -> Result<Ty, Diagnostic> {
    match annotation {
        Expr::NoneLiteral(_) => Ok(Ty::None),
        Expr::Name(name) => match name.id.as_str() {
            "int" => Ok(Ty::Int),
            "float" => Ok(Ty::Float),
            "bool" => Ok(Ty::Bool),
            "str" => Ok(Ty::Str),
            "Any" => Err(Diagnostic::error(
                "T0002",
                "`Any` is not permitted in pycc code outside a declared interop boundary".to_string(),
                Span::new(0, 0),
            )),
            other => panic!("pycc_hir: type annotation `{other}` is not supported yet"),
        },
        other => panic!("pycc_hir: only a bare name type annotation is supported so far: {other:?}"),
    }
}

fn lower_stmt(stmt: &Stmt) -> HirStmt {
    match stmt {
        Stmt::Expr(expr_stmt) => HirStmt::ExprStmt(lower_expr(&expr_stmt.value)),
        Stmt::Assign(assign) => {
            let [target] = assign.targets.as_slice() else {
                panic!(
                    "pycc_hir: only a single assignment target is supported so far: {:?}",
                    assign.targets
                );
            };
            let Expr::Name(name) = target else {
                panic!("pycc_hir: only assigning to a bare name is supported so far: {target:?}");
            };
            HirStmt::Assign { target: name.id.as_str().to_string(), value: lower_expr(&assign.value) }
        }
        Stmt::If(if_stmt) => HirStmt::If {
            test: lower_expr(&if_stmt.test),
            body: lower_body(&if_stmt.body),
            orelse: lower_elif_else_clauses(&if_stmt.elif_else_clauses),
        },
        Stmt::While(while_stmt) => {
            if !while_stmt.orelse.is_empty() {
                panic!("pycc_hir: while/else is not supported yet");
            }
            HirStmt::While { test: lower_expr(&while_stmt.test), body: lower_body(&while_stmt.body) }
        }
        Stmt::For(for_stmt) => {
            if for_stmt.is_async {
                panic!("pycc_hir: async for is not supported yet");
            }
            if !for_stmt.orelse.is_empty() {
                panic!("pycc_hir: for/else is not supported yet");
            }
            let Expr::Name(var) = for_stmt.target.as_ref() else {
                panic!("pycc_hir: only a bare name for-target is supported so far: {:?}", for_stmt.target);
            };
            let Expr::Call(call) = for_stmt.iter.as_ref() else {
                panic!("pycc_hir: only `for x in range(...)` is supported so far: {:?}", for_stmt.iter);
            };
            let Expr::Name(callee) = call.func.as_ref() else {
                panic!("pycc_hir: only `for x in range(...)` is supported so far: {:?}", call.func);
            };
            if callee.id.as_str() != "range" {
                panic!("pycc_hir: only iterating over `range(...)` is supported so far, got `{}`", callee.id);
            }
            let (start, stop, step) = match &*call.arguments.args {
                [stop] => (HirExpr::IntLiteral(0), lower_expr(stop), HirExpr::IntLiteral(1)),
                [start, stop] => (lower_expr(start), lower_expr(stop), HirExpr::IntLiteral(1)),
                [start, stop, step] => (lower_expr(start), lower_expr(stop), lower_expr(step)),
                other => panic!("pycc_hir: range() with {} arguments is not supported", other.len()),
            };
            HirStmt::ForRange { var: var.id.to_string(), start, stop, step, body: lower_body(&for_stmt.body) }
        }
        Stmt::Return(ret) => HirStmt::Return(ret.value.as_deref().map(lower_expr)),
        other => panic!("pycc_hir: statement kind not supported yet: {other:?}"),
    }
}

fn lower_body(body: &[Stmt]) -> Vec<HirStmt> {
    body.iter().map(lower_stmt).collect()
}

fn lower_elif_else_clauses(clauses: &[ElifElseClause]) -> Vec<HirStmt> {
    let Some((first, rest)) = clauses.split_first() else {
        return vec![];
    };
    match &first.test {
        Some(test) => vec![HirStmt::If {
            test: lower_expr(test),
            body: lower_body(&first.body),
            orelse: lower_elif_else_clauses(rest),
        }],
        None => {
            assert!(rest.is_empty(), "pycc_hir: an else clause must be the last elif_else_clause");
            lower_body(&first.body)
        }
    }
}

fn lower_expr(expr: &Expr) -> HirExpr {
    match expr {
        Expr::NumberLiteral(lit) => match &lit.value {
            Number::Int(i) => HirExpr::IntLiteral(
                i.as_i64().unwrap_or_else(|| panic!("pycc_hir: integer literal does not fit in i64: {i:?}")),
            ),
            Number::Float(f) => HirExpr::FloatLiteral(*f),
            other => panic!("pycc_hir: numeric literal kind not supported yet: {other:?}"),
        },
        Expr::Name(name) => HirExpr::Name(name.id.as_str().to_string()),
        Expr::Call(call) => {
            let Expr::Name(callee) = call.func.as_ref() else {
                panic!("pycc_hir: only calling a bare name is supported so far: {:?}", call.func);
            };
            let args = call.arguments.args.iter().map(lower_expr).collect();
            HirExpr::Call { callee: callee.id.as_str().to_string(), args }
        }
        Expr::BinOp(bin_op) => {
            let op = match bin_op.op {
                Operator::Add => BinOpKind::Add,
                Operator::Sub => BinOpKind::Sub,
                Operator::Mult => BinOpKind::Mul,
                Operator::Div => BinOpKind::Div,
                Operator::FloorDiv => BinOpKind::FloorDiv,
                Operator::Mod => BinOpKind::Mod,
                Operator::Pow => BinOpKind::Pow,
                other => panic!("pycc_hir: binary operator not supported yet: {other:?}"),
            };
            HirExpr::BinOp {
                op,
                left: Box::new(lower_expr(&bin_op.left)),
                right: Box::new(lower_expr(&bin_op.right)),
            }
        }
        Expr::BooleanLiteral(lit) => HirExpr::BoolLiteral(lit.value),
        Expr::StringLiteral(lit) => HirExpr::StringLiteral(lit.value.to_str().to_string()),
        Expr::Compare(cmp) => {
            if cmp.ops.len() != 1 {
                panic!("pycc_hir: chained comparisons are not supported yet: {:?}", cmp.ops);
            }
            let op = match cmp.ops[0] {
                CmpOp::Eq => CmpOpKind::Eq,
                CmpOp::NotEq => CmpOpKind::NotEq,
                CmpOp::Lt => CmpOpKind::Lt,
                CmpOp::LtE => CmpOpKind::LtE,
                CmpOp::Gt => CmpOpKind::Gt,
                CmpOp::GtE => CmpOpKind::GtE,
                other => panic!("pycc_hir: comparison operator not supported yet: {other:?}"),
            };
            HirExpr::Compare {
                op,
                left: Box::new(lower_expr(&cmp.left)),
                right: Box::new(lower_expr(&cmp.comparators[0])),
            }
        }
        other => panic!("pycc_hir: expression kind not supported yet: {other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ty_name_returns_the_python_spelling_of_every_variant() {
        assert_eq!(Ty::Int.name(), "int");
        assert_eq!(Ty::Float.name(), "float");
        assert_eq!(Ty::Bool.name(), "bool");
        assert_eq!(Ty::Str.name(), "str");
        assert_eq!(Ty::None.name(), "None");
    }

    #[test]
    fn lowers_a_function_definition_without_calling_it() {
        // Defining `main` alone has no observable effect -- matches
        // CPython exactly (confirmed empirically: `python3.14 hello.py`
        // on this exact source prints nothing). Only an explicit call
        // (see the next test) makes it run.
        let module = pycc_parser_test_helper::parse("def main() -> None:\n    print(42)\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::Function {
                name: "main".to_string(),
                params: vec![],
                return_ty: Ty::None,
                body: vec![HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::IntLiteral(42)],
                })],
            }]
        );
    }

    #[test]
    fn lowers_a_call_to_a_user_defined_function() {
        let module = pycc_parser_test_helper::parse("def main() -> None:\n    print(42)\n\nmain()\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![
                HirItem::Function {
                    name: "main".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![HirStmt::ExprStmt(HirExpr::Call {
                        callee: "print".to_string(),
                        args: vec![HirExpr::IntLiteral(42)],
                    })],
                },
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "main".to_string(),
                    args: vec![],
                })),
            ]
        );
    }

    #[test]
    fn lowers_top_level_print_with_no_main() {
        let module = pycc_parser_test_helper::parse("print(42)\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::IntLiteral(42)],
            }))]
        );
    }

    #[test]
    fn lowers_a_name_reference_used_as_a_call_argument() {
        // Exercises HirExpr::Name specifically -- every other test so far
        // only ever passes an IntLiteral or zero args to a call, never a
        // bare name reference used as a *value* (as opposed to an
        // assignment target, which Task 6 handles separately).
        let module = pycc_parser_test_helper::parse("def f() -> None:\n    print(x)\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: Ty::None,
                body: vec![HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::Name("x".to_string())],
                })],
            }]
        );
    }

    #[test]
    fn a_bare_boolean_literal_expression_is_now_supported() {
        let module = pycc_parser_test_helper::parse("True\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(hir.items, vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::BoolLiteral(true)))]);
    }

    #[test]
    fn lowers_an_assignment_and_a_later_reference_to_it() {
        let module = pycc_parser_test_helper::parse("x = 1\nprint(x)\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(1),
                }),
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::Name("x".to_string())],
                })),
            ]
        );
    }

    #[test]
    fn lowers_a_binary_addition() {
        let module = pycc_parser_test_helper::parse("x = 1 + 2\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(HirExpr::IntLiteral(1)),
                    right: Box::new(HirExpr::IntLiteral(2)),
                },
            })]
        );
    }

    #[test]
    fn lowers_every_arithmetic_operator() {
        let cases = [
            ("x = 1 - 2\n", BinOpKind::Sub),
            ("x = 1 * 2\n", BinOpKind::Mul),
            ("x = 1 / 2\n", BinOpKind::Div),
            ("x = 1 // 2\n", BinOpKind::FloorDiv),
            ("x = 1 % 2\n", BinOpKind::Mod),
            ("x = 1 ** 2\n", BinOpKind::Pow),
        ];
        for (source, expected_op) in cases {
            let module = pycc_parser_test_helper::parse(source);
            let hir = lower_checked(&module).unwrap();
            assert_eq!(
                hir.items,
                vec![HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::BinOp {
                        op: expected_op,
                        left: Box::new(HirExpr::IntLiteral(1)),
                        right: Box::new(HirExpr::IntLiteral(2)),
                    },
                })],
                "wrong lowering for {source:?}"
            );
        }
    }

    #[test]
    fn lowers_a_float_literal() {
        let module = pycc_parser_test_helper::parse("x = 1.5\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::FloatLiteral(1.5),
            })]
        );
    }

    #[test]
    #[should_panic(expected = "only a single assignment target is supported so far")]
    fn a_multi_target_assignment_is_unsupported() {
        let module = pycc_parser_test_helper::parse("x = y = 1\n");
        lower_checked(&module).unwrap();
    }

    #[test]
    #[should_panic(expected = "only assigning to a bare name is supported so far")]
    fn assigning_to_a_non_name_target_is_unsupported() {
        let module = pycc_parser_test_helper::parse("x.attr = 1\n");
        lower_checked(&module).unwrap();
    }

    #[test]
    #[should_panic(expected = "binary operator not supported yet")]
    fn matrix_multiplication_is_unsupported() {
        let module = pycc_parser_test_helper::parse("x = a @ b\n");
        lower_checked(&module).unwrap();
    }

    #[test]
    #[should_panic(expected = "statement kind not supported yet")]
    fn a_pass_statement_is_unsupported() {
        // `if` itself is supported (Task 8); `pass` inside it is not -- no
        // v0.1 grammar construct needs it (empty bodies aren't reachable
        // through anything pycc lowers) and it exercises the same catch-all
        // as `a_list_literal_expression_is_unsupported` does for expressions.
        let module = pycc_parser_test_helper::parse("if True:\n    pass\n");
        lower_checked(&module).unwrap();
    }

    #[test]
    fn a_bare_literal_expression_statement_is_now_supported() {
        // `42` alone at module level is legal (if pointless) Python -- an
        // expression statement whose value is simply discarded. The old
        // HIR shape only ever represented a *call* expression statement,
        // so this used to panic; HirExpr::IntLiteral now represents any
        // expression, not just call arguments.
        let module = pycc_parser_test_helper::parse("42\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(hir.items, vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::IntLiteral(42)))]);
    }

    #[test]
    #[should_panic(expected = "only calling a bare name")]
    fn non_name_callee_is_unsupported() {
        let module = pycc_parser_test_helper::parse("foo.bar()\n");
        lower_checked(&module).unwrap();
    }

    #[test]
    fn calling_a_zero_arg_function_other_than_print_is_supported() {
        let module = pycc_parser_test_helper::parse("foo()\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                callee: "foo".to_string(),
                args: vec![],
            }))]
        );
    }

    #[test]
    fn calling_a_non_print_function_with_arguments_is_now_supported() {
        // This used to panic in the pre-Task-5 HIR shape (only zero-arg
        // user-function calls were representable at all) -- HirExpr::Call
        // now carries arbitrary args for every callee, print included;
        // real type-checking of a call's arguments against a declared
        // signature is Task 9's job, not this lowering step's.
        let module = pycc_parser_test_helper::parse("foo(42)\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                callee: "foo".to_string(),
                args: vec![HirExpr::IntLiteral(42)],
            }))]
        );
    }

    #[test]
    fn print_with_more_than_one_argument_is_now_supported_at_the_hir_level() {
        // Same rationale as above -- HirExpr::Call no longer special-cases
        // `print`'s arity at lowering time.
        let module = pycc_parser_test_helper::parse("print(1, 2)\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
            }))]
        );
    }

    #[test]
    fn print_with_a_float_argument_is_now_supported_at_the_hir_level() {
        // MIR/codegen still only understands an integer-literal argument to
        // `print` (see pycc_mir::lower_instr); this is HIR-only.
        let module = pycc_parser_test_helper::parse("print(2.5)\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::FloatLiteral(2.5)],
            }))]
        );
    }

    #[test]
    #[should_panic(expected = "does not fit in i64")]
    fn print_with_an_integer_too_large_for_i64_is_unsupported() {
        let module = pycc_parser_test_helper::parse("print(99999999999999999999999999999999)\n");
        lower_checked(&module).unwrap();
    }

    #[test]
    #[should_panic(expected = "numeric literal kind not supported yet")]
    fn a_complex_number_literal_is_unsupported() {
        // Complex isn't in v0.1's type-representation table (int/float/bool/str/None
        // per TYPE_SYSTEM.md) -- unlike float/bool, this isn't deferred to a later
        // PR-4 task, it's simply out of scope for pycc entirely.
        let module = pycc_parser_test_helper::parse("x = 3j\n");
        lower_checked(&module).unwrap();
    }

    #[test]
    fn lowers_a_boolean_literal() {
        let module = pycc_parser_test_helper::parse("x = True\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::BoolLiteral(true),
            })]
        );
    }

    #[test]
    fn lowers_a_single_comparison() {
        let module = pycc_parser_test_helper::parse("x = 1 < 2\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::Compare {
                    op: CmpOpKind::Lt,
                    left: Box::new(HirExpr::IntLiteral(1)),
                    right: Box::new(HirExpr::IntLiteral(2)),
                },
            })]
        );
    }

    #[test]
    fn lowers_every_comparison_operator() {
        let cases = [
            ("x = 1 == 2\n", CmpOpKind::Eq),
            ("x = 1 != 2\n", CmpOpKind::NotEq),
            ("x = 1 < 2\n", CmpOpKind::Lt),
            ("x = 1 <= 2\n", CmpOpKind::LtE),
            ("x = 1 > 2\n", CmpOpKind::Gt),
            ("x = 1 >= 2\n", CmpOpKind::GtE),
        ];
        for (source, expected_op) in cases {
            let module = pycc_parser_test_helper::parse(source);
            let hir = lower_checked(&module).unwrap();
            assert_eq!(
                hir.items,
                vec![HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::Compare {
                        op: expected_op,
                        left: Box::new(HirExpr::IntLiteral(1)),
                        right: Box::new(HirExpr::IntLiteral(2)),
                    },
                })],
                "wrong lowering for {source:?}"
            );
        }
    }

    #[test]
    #[should_panic(expected = "chained comparisons")]
    fn a_chained_comparison_is_not_supported_yet() {
        let module = pycc_parser_test_helper::parse("x = 1 < 2 < 3\n");
        lower_checked(&module).unwrap();
    }

    #[test]
    #[should_panic(expected = "comparison operator not supported yet")]
    fn an_is_comparison_is_not_supported_yet() {
        let module = pycc_parser_test_helper::parse("x = 1 is 2\n");
        lower_checked(&module).unwrap();
    }

    #[test]
    #[should_panic(expected = "expression kind not supported yet")]
    fn a_list_literal_expression_is_unsupported() {
        // No v0.1 grammar node reaches this catch-all today (every kind
        // handled so far -- numbers, names, calls, binops, bools,
        // comparisons -- has its own dedicated arm/panic); a list literal is
        // genuinely unhandled at every level and exercises the final arm.
        let module = pycc_parser_test_helper::parse("x = [1]\n");
        lower_checked(&module).unwrap();
    }

    #[test]
    fn lowers_an_if_with_no_else() {
        let module = pycc_parser_test_helper::parse("if True:\n    print(1)\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::If {
                test: HirExpr::BoolLiteral(true),
                body: vec![HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::IntLiteral(1)],
                })],
                orelse: vec![],
            })]
        );
    }

    #[test]
    fn lowers_an_if_with_an_else() {
        let module = pycc_parser_test_helper::parse("if True:\n    print(1)\nelse:\n    print(2)\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::If {
                test: HirExpr::BoolLiteral(true),
                body: vec![HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::IntLiteral(1)],
                })],
                orelse: vec![HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::IntLiteral(2)],
                })],
            })]
        );
    }

    #[test]
    fn lowers_an_elif_as_a_nested_if_in_orelse() {
        let module =
            pycc_parser_test_helper::parse("if False:\n    print(1)\nelif True:\n    print(2)\nelse:\n    print(3)\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::If {
                test: HirExpr::BoolLiteral(false),
                body: vec![HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::IntLiteral(1)],
                })],
                orelse: vec![HirStmt::If {
                    test: HirExpr::BoolLiteral(true),
                    body: vec![HirStmt::ExprStmt(HirExpr::Call {
                        callee: "print".to_string(),
                        args: vec![HirExpr::IntLiteral(2)],
                    })],
                    orelse: vec![HirStmt::ExprStmt(HirExpr::Call {
                        callee: "print".to_string(),
                        args: vec![HirExpr::IntLiteral(3)],
                    })],
                }],
            })]
        );
    }

    #[test]
    fn lowers_a_while_loop() {
        let module = pycc_parser_test_helper::parse("while True:\n    print(1)\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::While {
                test: HirExpr::BoolLiteral(true),
                body: vec![HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::IntLiteral(1)],
                })],
            })]
        );
    }

    #[test]
    #[should_panic(expected = "while/else is not supported yet")]
    fn a_while_else_is_not_supported_yet() {
        let module = pycc_parser_test_helper::parse("while True:\n    print(1)\nelse:\n    print(2)\n");
        lower_checked(&module).unwrap();
    }

    #[test]
    fn lowers_a_for_range_loop_with_one_argument() {
        let module = pycc_parser_test_helper::parse("for i in range(3):\n    print(i)\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::ForRange {
                var: "i".to_string(),
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
                body: vec![HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::Name("i".to_string())],
                })],
            })]
        );
    }

    #[test]
    fn lowers_a_for_range_loop_with_start_and_stop() {
        let module = pycc_parser_test_helper::parse("for i in range(1, 3):\n    print(i)\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::ForRange {
                var: "i".to_string(),
                start: HirExpr::IntLiteral(1),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
                body: vec![HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::Name("i".to_string())],
                })],
            })]
        );
    }

    #[test]
    fn lowers_a_for_range_loop_with_start_stop_and_step() {
        let module = pycc_parser_test_helper::parse("for i in range(0, 6, 2):\n    print(i)\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::ForRange {
                var: "i".to_string(),
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(6),
                step: HirExpr::IntLiteral(2),
                body: vec![HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::Name("i".to_string())],
                })],
            })]
        );
    }

    #[test]
    #[should_panic(expected = "only `for x in range(...)` is supported so far")]
    fn iterating_a_non_call_expression_is_not_supported_yet() {
        let module = pycc_parser_test_helper::parse("for i in [1, 2, 3]:\n    print(i)\n");
        lower_checked(&module).unwrap();
    }

    #[test]
    #[should_panic(expected = "only iterating over `range(...)` is supported so far")]
    fn calling_something_other_than_range_in_a_for_is_not_supported_yet() {
        let module = pycc_parser_test_helper::parse("for i in items(3):\n    print(i)\n");
        lower_checked(&module).unwrap();
    }

    #[test]
    #[should_panic(expected = "only `for x in range(...)` is supported so far")]
    fn calling_via_an_attribute_in_a_for_is_not_supported_yet() {
        let module = pycc_parser_test_helper::parse("for i in a.b(3):\n    print(i)\n");
        lower_checked(&module).unwrap();
    }

    #[test]
    #[should_panic(expected = "range() with 4 arguments is not supported")]
    fn range_with_too_many_arguments_is_not_supported() {
        let module = pycc_parser_test_helper::parse("for i in range(1, 2, 3, 4):\n    print(i)\n");
        lower_checked(&module).unwrap();
    }

    #[test]
    #[should_panic(expected = "only a bare name for-target is supported so far")]
    fn a_tuple_for_target_is_not_supported_yet() {
        let module = pycc_parser_test_helper::parse("for i, j in range(3):\n    print(i)\n");
        lower_checked(&module).unwrap();
    }

    #[test]
    #[should_panic(expected = "for/else is not supported yet")]
    fn a_for_else_is_not_supported_yet() {
        let module = pycc_parser_test_helper::parse("for i in range(3):\n    print(i)\nelse:\n    print(0)\n");
        lower_checked(&module).unwrap();
    }

    #[test]
    #[should_panic(expected = "async for is not supported yet")]
    fn an_async_for_is_not_supported_yet() {
        let module = pycc_parser_test_helper::parse("async def f() -> None:\n    async for i in range(3):\n        print(i)\n");
        lower_checked(&module).unwrap();
    }

    #[test]
    fn lowers_a_fully_annotated_public_function_with_params_and_return() {
        let module = pycc_parser_test_helper::parse("def add(a: int, b: int) -> int:\n    return a + b\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::Function {
                name: "add".to_string(),
                params: vec![("a".to_string(), Ty::Int), ("b".to_string(), Ty::Int)],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(HirExpr::Name("a".to_string())),
                    right: Box::new(HirExpr::Name("b".to_string())),
                }))],
            }]
        );
    }

    #[test]
    fn lowers_a_return_with_no_value() {
        let module = pycc_parser_test_helper::parse("def f() -> None:\n    return\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: Ty::None,
                body: vec![HirStmt::Return(None)],
            }]
        );
    }

    #[test]
    fn an_unannotated_public_function_missing_param_annotations_produces_t0001() {
        let module = pycc_parser_test_helper::parse("def add(a, b) -> int:\n    return a + b\n");
        let diag = lower_checked(&module).unwrap_err();
        assert_eq!(diag.code, "T0001");
        assert!(diag.message.contains('a'));
    }

    #[test]
    fn an_unannotated_public_function_missing_return_annotation_produces_t0001() {
        let module = pycc_parser_test_helper::parse("def add(a: int, b: int):\n    return a + b\n");
        let diag = lower_checked(&module).unwrap_err();
        assert_eq!(diag.code, "T0001");
        assert!(diag.message.contains("add"));
    }

    #[test]
    fn an_unannotated_private_function_is_allowed() {
        let module = pycc_parser_test_helper::parse("def _add(a, b):\n    return a + b\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::Function {
                name: "_add".to_string(),
                params: vec![("a".to_string(), Ty::None), ("b".to_string(), Ty::None)],
                return_ty: Ty::None,
                body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(HirExpr::Name("a".to_string())),
                    right: Box::new(HirExpr::Name("b".to_string())),
                }))],
            }]
        );
    }

    #[test]
    fn every_supported_annotation_type_lowers_correctly() {
        let cases = [
            ("def f(x: int) -> int:\n    return x\n", Ty::Int),
            ("def f(x: float) -> float:\n    return x\n", Ty::Float),
            ("def f(x: bool) -> bool:\n    return x\n", Ty::Bool),
            ("def f(x: str) -> str:\n    return x\n", Ty::Str),
            ("def f(x: None) -> None:\n    return x\n", Ty::None),
        ];
        for (source, expected_ty) in cases {
            let module = pycc_parser_test_helper::parse(source);
            let hir = lower_checked(&module).unwrap();
            assert_eq!(
                hir.items,
                vec![HirItem::Function {
                    name: "f".to_string(),
                    params: vec![("x".to_string(), expected_ty)],
                    return_ty: expected_ty,
                    body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
                }],
                "wrong lowering for {source:?}"
            );
        }
    }

    #[test]
    #[should_panic(expected = "type annotation `list` is not supported yet")]
    fn an_unsupported_annotation_type_panics() {
        let module = pycc_parser_test_helper::parse("def f(x: list) -> None:\n    return\n");
        lower_checked(&module).unwrap();
    }

    #[test]
    #[should_panic(expected = "only a bare name type annotation is supported so far")]
    fn a_non_bare_name_annotation_panics() {
        let module = pycc_parser_test_helper::parse("def f(x: a.b) -> None:\n    return\n");
        lower_checked(&module).unwrap();
    }

    #[test]
    fn lowers_a_plain_string_literal() {
        let module = pycc_parser_test_helper::parse("x = \"hi\"\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::StringLiteral("hi".to_string()),
            })]
        );
    }

    #[test]
    fn an_any_annotation_produces_t0002() {
        let module = pycc_parser_test_helper::parse("def f(x: Any) -> None:\n    pass\n");
        let diag = lower_checked(&module).unwrap_err();
        assert_eq!(diag.code, "T0002");
    }

    #[test]
    fn an_any_return_annotation_produces_t0002() {
        let module = pycc_parser_test_helper::parse("def f() -> Any:\n    pass\n");
        let diag = lower_checked(&module).unwrap_err();
        assert_eq!(diag.code, "T0002");
    }
}

#[cfg(test)]
mod pycc_parser_test_helper {
    pub fn parse(source: &str) -> pycc_ast::ModModule {
        pycc_parser::parse(source).expect("test fixture must parse")
    }
}
