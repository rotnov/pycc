use pycc_ast::{Expr, ModModule, Number, Stmt};

#[derive(Debug, Clone, PartialEq)]
pub enum HirExpr {
    IntLiteral(i64),
    Name(String),
    Call { callee: String, args: Vec<HirExpr> },
}

#[derive(Debug, PartialEq)]
pub enum HirStmt {
    ExprStmt(HirExpr),
}

#[derive(Debug, PartialEq)]
pub enum HirItem {
    Function { name: String, body: Vec<HirStmt> },
    TopLevelStmt(HirStmt),
}

pub struct HirModule {
    pub items: Vec<HirItem>,
}

pub fn lower(module: &ModModule) -> HirModule {
    let mut items = Vec::new();
    for stmt in &module.body {
        match stmt {
            Stmt::FunctionDef(f) => {
                let body = f.body.iter().map(lower_stmt).collect();
                items.push(HirItem::Function { name: f.name.id.as_str().to_string(), body });
            }
            other => items.push(HirItem::TopLevelStmt(lower_stmt(other))),
        }
    }
    HirModule { items }
}

fn lower_stmt(stmt: &Stmt) -> HirStmt {
    match stmt {
        Stmt::Expr(expr_stmt) => HirStmt::ExprStmt(lower_expr(&expr_stmt.value)),
        other => panic!("pycc_hir: statement kind not supported yet: {other:?}"),
    }
}

fn lower_expr(expr: &Expr) -> HirExpr {
    match expr {
        Expr::NumberLiteral(lit) => match &lit.value {
            Number::Int(i) => HirExpr::IntLiteral(
                i.as_i64().unwrap_or_else(|| panic!("pycc_hir: integer literal does not fit in i64: {i:?}")),
            ),
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
        other => panic!("pycc_hir: expression kind not supported yet: {other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowers_a_function_definition_without_calling_it() {
        // Defining `main` alone has no observable effect -- matches
        // CPython exactly (confirmed empirically: `python3.14 hello.py`
        // on this exact source prints nothing). Only an explicit call
        // (see the next test) makes it run.
        let module = pycc_parser_test_helper::parse("def main() -> None:\n    print(42)\n");
        let hir = lower(&module);
        assert_eq!(
            hir.items,
            vec![HirItem::Function {
                name: "main".to_string(),
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
        let hir = lower(&module);
        assert_eq!(
            hir.items,
            vec![
                HirItem::Function {
                    name: "main".to_string(),
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
        let hir = lower(&module);
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
        let module = pycc_parser_test_helper::parse("def f():\n    print(x)\n");
        let hir = lower(&module);
        assert_eq!(
            hir.items,
            vec![HirItem::Function {
                name: "f".to_string(),
                body: vec![HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::Name("x".to_string())],
                })],
            }]
        );
    }

    #[test]
    #[should_panic(expected = "expression kind not supported yet")]
    fn a_bare_boolean_literal_expression_is_unsupported_until_task_7() {
        let module = pycc_parser_test_helper::parse("True\n");
        lower(&module);
    }

    #[test]
    #[should_panic(expected = "statement kind not supported yet")]
    fn non_expr_statement_is_unsupported() {
        let module = pycc_parser_test_helper::parse("x = 1\n");
        lower(&module);
    }

    #[test]
    fn a_bare_literal_expression_statement_is_now_supported() {
        // `42` alone at module level is legal (if pointless) Python -- an
        // expression statement whose value is simply discarded. The old
        // HIR shape only ever represented a *call* expression statement,
        // so this used to panic; HirExpr::IntLiteral now represents any
        // expression, not just call arguments.
        let module = pycc_parser_test_helper::parse("42\n");
        let hir = lower(&module);
        assert_eq!(hir.items, vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::IntLiteral(42)))]);
    }

    #[test]
    #[should_panic(expected = "only calling a bare name")]
    fn non_name_callee_is_unsupported() {
        let module = pycc_parser_test_helper::parse("foo.bar()\n");
        lower(&module);
    }

    #[test]
    fn calling_a_zero_arg_function_other_than_print_is_supported() {
        let module = pycc_parser_test_helper::parse("foo()\n");
        let hir = lower(&module);
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
        let hir = lower(&module);
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
        let hir = lower(&module);
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
            }))]
        );
    }

    #[test]
    #[should_panic(expected = "numeric literal kind not supported yet")]
    fn print_with_a_float_argument_is_unsupported_until_task_6() {
        let module = pycc_parser_test_helper::parse("print(3.14)\n");
        lower(&module);
    }

    #[test]
    #[should_panic(expected = "does not fit in i64")]
    fn print_with_an_integer_too_large_for_i64_is_unsupported() {
        let module = pycc_parser_test_helper::parse("print(99999999999999999999999999999999)\n");
        lower(&module);
    }
}

#[cfg(test)]
mod pycc_parser_test_helper {
    pub fn parse(source: &str) -> pycc_ast::ModModule {
        pycc_parser::parse(source).expect("test fixture must parse")
    }
}
