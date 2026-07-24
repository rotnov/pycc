use pycc_ast::{Expr, ExprCall, ModModule, Number, Stmt};
use pycc_diag::{Diagnostic, Span};

#[derive(Debug, PartialEq)]
pub enum HirStmt {
    CallPrint {
        arg: i64,
    },
    /// A zero-argument call to a user-defined function, e.g. a top-level
    /// `main()` invoking `def main() -> None: ...`. Python has no concept
    /// of a function auto-running just because of its name -- a def alone
    /// produces a `HirItem::Function` with no observable effect; only an
    /// explicit call like this one ever executes it. Arguments aren't
    /// supported yet (v0.1 slice-0 scope; see PR-4/PR-5 for real calls).
    CallUserFunction {
        name: String,
    },
}

#[derive(Debug, PartialEq)]
pub enum HirItem {
    Function { name: String, body: Vec<HirStmt> },
    TopLevelStmt(HirStmt),
}

#[derive(Debug)]
pub struct HirModule {
    pub items: Vec<HirItem>,
}

/// Lowers a parsed module into the subset of HIR implemented by this pycc
/// version.
///
/// Syntactically valid Python outside that subset is returned as `C0001`
/// instead of panicking, so CLI callers can report an ordinary compile error.
pub fn lower(module: &ModModule) -> Result<HirModule, Diagnostic> {
    let mut items = Vec::new();
    for stmt in &module.body {
        match stmt {
            Stmt::FunctionDef(f) => {
                let body = f
                    .body
                    .iter()
                    .map(lower_stmt)
                    .collect::<Result<Vec<_>, _>>()?;
                items.push(HirItem::Function {
                    name: f.name.id.as_str().to_string(),
                    body,
                });
            }
            other => items.push(HirItem::TopLevelStmt(lower_stmt(other)?)),
        }
    }
    Ok(HirModule { items })
}

fn lower_stmt(stmt: &Stmt) -> Result<HirStmt, Diagnostic> {
    let Stmt::Expr(expr_stmt) = stmt else {
        return Err(unsupported(
            "only a bare call expression statement is supported so far",
        ));
    };
    let Expr::Call(ExprCall {
        func, arguments, ..
    }) = expr_stmt.value.as_ref()
    else {
        return Err(unsupported(
            "only a call expression statement is supported so far",
        ));
    };
    let Expr::Name(name) = func.as_ref() else {
        return Err(unsupported("only calling a bare name is supported so far"));
    };
    if !arguments.keywords.is_empty() {
        return Err(unsupported("keyword arguments are not supported so far"));
    }

    if name.id.as_str() == "print" {
        let [Expr::NumberLiteral(lit)] = arguments.args.as_ref() else {
            return Err(unsupported(
                "print() must take exactly one integer literal argument so far",
            ));
        };
        let Number::Int(i) = &lit.value else {
            return Err(unsupported(
                "only integer literal arguments are supported so far",
            ));
        };
        let Some(arg) = i.as_i64() else {
            return Err(unsupported(
                "integer literal is too large for v0.1's i64-only HIR",
            ));
        };
        Ok(HirStmt::CallPrint { arg })
    } else {
        let [] = arguments.args.as_ref() else {
            return Err(unsupported(format!(
                "calling a user-defined function with arguments is not supported yet -- only \
                 zero-argument calls like `{}()`",
                name.id.as_str(),
            )));
        };
        Ok(HirStmt::CallUserFunction {
            name: name.id.as_str().to_string(),
        })
    }
}

fn unsupported(message: impl Into<String>) -> Diagnostic {
    Diagnostic::error("C0001", message, Span::new(0, 0))
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
        let hir = lower(&module).unwrap();
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
        let module =
            pycc_parser_test_helper::parse("def main() -> None:\n    print(42)\n\nmain()\n");
        let hir = lower(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![
                HirItem::Function {
                    name: "main".to_string(),
                    body: vec![HirStmt::CallPrint { arg: 42 }],
                },
                HirItem::TopLevelStmt(HirStmt::CallUserFunction {
                    name: "main".to_string()
                }),
            ]
        );
    }

    #[test]
    fn lowers_top_level_print_with_no_main() {
        let module = pycc_parser_test_helper::parse("print(42)\n");
        let hir = lower(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::CallPrint { arg: 42 })]
        );
    }

    #[test]
    fn non_expr_statement_is_unsupported() {
        let module = pycc_parser_test_helper::parse("x = 1\n");
        let error = lower(&module).unwrap_err();
        assert_eq!(error.code, "C0001");
        assert!(
            error
                .message
                .contains("only a bare call expression statement")
        );
    }

    #[test]
    fn unsupported_statement_inside_a_function_is_reported() {
        let module = pycc_parser_test_helper::parse("def main() -> None:\n    x = 1\n");
        let error = lower(&module).unwrap_err();
        assert_eq!(error.code, "C0001");
        assert!(
            error
                .message
                .contains("only a bare call expression statement")
        );
    }

    #[test]
    fn non_call_expression_statement_is_unsupported() {
        let module = pycc_parser_test_helper::parse("42\n");
        let error = lower(&module).unwrap_err();
        assert_eq!(error.code, "C0001");
        assert!(error.message.contains("only a call expression statement"));
    }

    #[test]
    fn non_name_callee_is_unsupported() {
        let module = pycc_parser_test_helper::parse("foo.bar()\n");
        let error = lower(&module).unwrap_err();
        assert_eq!(error.code, "C0001");
        assert!(error.message.contains("only calling a bare name"));
    }

    #[test]
    fn calling_a_zero_arg_function_other_than_print_is_supported() {
        let module = pycc_parser_test_helper::parse("foo()\n");
        let hir = lower(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::CallUserFunction {
                name: "foo".to_string()
            })]
        );
    }

    #[test]
    fn calling_a_non_print_function_with_arguments_is_unsupported() {
        let module = pycc_parser_test_helper::parse("foo(42)\n");
        let error = lower(&module).unwrap_err();
        assert_eq!(error.code, "C0001");
        assert!(
            error
                .message
                .contains("calling a user-defined function with arguments")
        );
    }

    #[test]
    fn print_with_wrong_argument_count_is_unsupported() {
        let module = pycc_parser_test_helper::parse("print(1, 2)\n");
        let error = lower(&module).unwrap_err();
        assert_eq!(error.code, "C0001");
        assert!(
            error
                .message
                .contains("exactly one integer literal argument")
        );
    }

    #[test]
    fn print_with_a_float_argument_is_unsupported() {
        let module = pycc_parser_test_helper::parse("print(3.14)\n");
        let error = lower(&module).unwrap_err();
        assert_eq!(error.code, "C0001");
        assert!(error.message.contains("only integer literal arguments"));
    }

    #[test]
    fn print_with_an_integer_too_large_for_i64_is_unsupported() {
        let module = pycc_parser_test_helper::parse("print(99999999999999999999999999999999)\n");
        let error = lower(&module).unwrap_err();
        assert_eq!(error.code, "C0001");
        assert!(error.message.contains("too large for v0.1's i64-only HIR"));
    }

    #[test]
    fn keyword_arguments_are_unsupported() {
        let module = pycc_parser_test_helper::parse("print(value=1)\n");
        let error = lower(&module).unwrap_err();
        assert_eq!(error.code, "C0001");
        assert!(error.message.contains("keyword arguments"));
    }
}

#[cfg(test)]
mod pycc_parser_test_helper {
    pub fn parse(source: &str) -> pycc_ast::ModModule {
        pycc_parser::parse(source).expect("test fixture must parse")
    }
}
