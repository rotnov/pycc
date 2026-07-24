use pycc_ast::{Expr, ExprCall, ModModule, Number, Stmt};

#[derive(Debug, PartialEq)]
pub enum HirStmt {
    CallPrint { arg: i64 },
    /// A zero-argument call to a user-defined function, e.g. a top-level
    /// `main()` invoking `def main() -> None: ...`. Python has no concept
    /// of a function auto-running just because of its name -- a def alone
    /// produces a `HirItem::Function` with no observable effect; only an
    /// explicit call like this one ever executes it. Arguments aren't
    /// supported yet (v0.1 slice-0 scope; see PR-4/PR-5 for real calls).
    CallUserFunction { name: String },
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
    let Stmt::Expr(expr_stmt) = stmt else {
        panic!("pycc_hir v0.1: only a bare call expression statement is supported so far");
    };
    let Expr::Call(ExprCall { func, arguments, .. }) = expr_stmt.value.as_ref() else {
        panic!("pycc_hir v0.1: only a call expression statement is supported so far");
    };
    let Expr::Name(name) = func.as_ref() else {
        panic!("pycc_hir v0.1: only calling a bare name is supported so far");
    };
    if name.id.as_str() == "print" {
        let [Expr::NumberLiteral(lit)] = arguments.args.as_ref() else {
            panic!("pycc_hir v0.1: print() must take exactly one integer literal argument so far");
        };
        let Number::Int(i) = &lit.value else {
            panic!("pycc_hir v0.1: only integer literal arguments are supported so far");
        };
        HirStmt::CallPrint { arg: i.as_i64().expect("literal too large for v0.1's i64-only HIR") }
    } else {
        let [] = arguments.args.as_ref() else {
            panic!(
                "pycc_hir v0.1: calling a user-defined function with arguments is not \
                 supported yet -- only zero-argument calls like `{}()`",
                name.id.as_str()
            );
        };
        HirStmt::CallUserFunction { name: name.id.as_str().to_string() }
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
                body: vec![HirStmt::CallPrint { arg: 42 }],
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
                    body: vec![HirStmt::CallPrint { arg: 42 }],
                },
                HirItem::TopLevelStmt(HirStmt::CallUserFunction { name: "main".to_string() }),
            ]
        );
    }

    #[test]
    fn lowers_top_level_print_with_no_main() {
        let module = pycc_parser_test_helper::parse("print(42)\n");
        let hir = lower(&module);
        assert_eq!(hir.items, vec![HirItem::TopLevelStmt(HirStmt::CallPrint { arg: 42 })]);
    }

    #[test]
    #[should_panic(expected = "only a bare call expression statement")]
    fn non_expr_statement_is_unsupported() {
        let module = pycc_parser_test_helper::parse("x = 1\n");
        lower(&module);
    }

    #[test]
    #[should_panic(expected = "only a call expression statement")]
    fn non_call_expression_statement_is_unsupported() {
        let module = pycc_parser_test_helper::parse("42\n");
        lower(&module);
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
            vec![HirItem::TopLevelStmt(HirStmt::CallUserFunction { name: "foo".to_string() })]
        );
    }

    #[test]
    #[should_panic(expected = "calling a user-defined function with arguments is not supported")]
    fn calling_a_non_print_function_with_arguments_is_unsupported() {
        let module = pycc_parser_test_helper::parse("foo(42)\n");
        lower(&module);
    }

    #[test]
    #[should_panic(expected = "exactly one integer literal argument")]
    fn print_with_wrong_argument_count_is_unsupported() {
        let module = pycc_parser_test_helper::parse("print(1, 2)\n");
        lower(&module);
    }

    #[test]
    #[should_panic(expected = "only integer literal arguments")]
    fn print_with_a_float_argument_is_unsupported() {
        let module = pycc_parser_test_helper::parse("print(3.14)\n");
        lower(&module);
    }

    #[test]
    #[should_panic(expected = "too large for v0.1's i64-only HIR")]
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
