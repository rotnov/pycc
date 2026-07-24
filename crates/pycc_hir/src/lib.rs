use pycc_ast::{Expr, ExprCall, ModModule, Number, Ranged, Stmt};
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

#[derive(Debug, PartialEq)]
pub struct HirModule {
    pub items: Vec<HirItem>,
}

pub fn lower(module: &ModModule) -> Result<HirModule, Diagnostic> {
    let mut items = Vec::new();
    for stmt in &module.body {
        match stmt {
            Stmt::FunctionDef(f) => {
                if f.is_async {
                    return Err(unsupported(f, "async functions are not implemented yet"));
                }
                if !f.decorator_list.is_empty() {
                    return Err(unsupported(
                        f,
                        "function decorators are not implemented yet",
                    ));
                }
                if f.type_params.is_some() {
                    return Err(unsupported(
                        f,
                        "generic function type parameters are not implemented yet",
                    ));
                }
                if !f.parameters.is_empty() {
                    return Err(unsupported(
                        f.parameters.as_ref(),
                        "function parameters are not implemented yet",
                    ));
                }
                if let Some(annotation) = f.returns.as_deref()
                    && !matches!(annotation, Expr::NoneLiteral(_))
                {
                    return Err(unsupported(
                        annotation,
                        "only a `None` return annotation is implemented yet",
                    ));
                }
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
            stmt,
            "only call expression statements are implemented yet",
        ));
    };
    let Expr::Call(ExprCall {
        func, arguments, ..
    }) = expr_stmt.value.as_ref()
    else {
        return Err(unsupported(
            expr_stmt.value.as_ref(),
            "only call expression statements are implemented yet",
        ));
    };
    let Expr::Name(name) = func.as_ref() else {
        return Err(unsupported(
            func.as_ref(),
            "only calls through a bare function name are implemented yet",
        ));
    };
    if !arguments.keywords.is_empty() {
        return Err(unsupported(
            arguments,
            "keyword arguments are not implemented yet",
        ));
    }
    if name.id.as_str() == "print" {
        let [Expr::NumberLiteral(lit)] = arguments.args.as_ref() else {
            return Err(unsupported(
                arguments,
                "print() must take exactly one integer literal argument",
            ));
        };
        let Number::Int(i) = &lit.value else {
            return Err(unsupported(
                lit,
                "only integer literal arguments to print() are implemented yet",
            ));
        };
        let Some(arg) = i.as_i64() else {
            return Err(unsupported(
                lit,
                "integer literal is outside the implemented signed 64-bit range",
            ));
        };
        Ok(HirStmt::CallPrint { arg })
    } else {
        let [] = arguments.args.as_ref() else {
            return Err(unsupported(
                arguments,
                format!(
                    "arguments to user-defined function `{}` are not implemented yet",
                    name.id.as_str()
                ),
            ));
        };
        Ok(HirStmt::CallUserFunction {
            name: name.id.as_str().to_string(),
        })
    }
}

fn unsupported(node: &impl Ranged, message: impl Into<String>) -> Diagnostic {
    let range = node.range();
    Diagnostic::error(
        "L0003",
        message,
        Span::new(range.start().to_u32(), range.end().to_u32()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowers_a_function_definition_without_calling_it() {
        let module = parse("def main() -> None:\n    print(42)\n");
        let hir = lower(&module).expect("supported source should lower");
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
        let module = parse("def main() -> None:\n    print(42)\n\nmain()\n");
        let hir = lower(&module).expect("supported source should lower");
        assert_eq!(
            hir.items,
            vec![
                HirItem::Function {
                    name: "main".to_string(),
                    body: vec![HirStmt::CallPrint { arg: 42 }],
                },
                HirItem::TopLevelStmt(HirStmt::CallUserFunction {
                    name: "main".to_string(),
                }),
            ]
        );
    }

    #[test]
    fn lowers_top_level_print_with_no_main() {
        let module = parse("print(42)\n");
        let hir = lower(&module).expect("supported source should lower");
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::CallPrint { arg: 42 })]
        );
    }

    #[test]
    fn lowers_a_function_without_a_return_annotation() {
        let module = parse("def main():\n    print(42)\n");
        let hir = lower(&module).expect("supported source should lower");
        assert_eq!(
            hir.items,
            vec![HirItem::Function {
                name: "main".to_string(),
                body: vec![HirStmt::CallPrint { arg: 42 }],
            }]
        );
    }

    #[test]
    fn non_expr_statement_is_a_diagnostic() {
        assert_unsupported("x = 1\n", "call expression statements");
    }

    #[test]
    fn unsupported_statement_inside_a_function_is_a_diagnostic() {
        assert_unsupported(
            "def f() -> None:\n    x = 1\n",
            "call expression statements",
        );
    }

    #[test]
    fn non_call_expression_statement_is_a_diagnostic() {
        assert_unsupported("42\n", "call expression statements");
    }

    #[test]
    fn non_name_callee_is_a_diagnostic() {
        assert_unsupported("foo.bar()\n", "bare function name");
    }

    #[test]
    fn calling_a_zero_arg_function_other_than_print_is_supported() {
        let module = parse("foo()\n");
        let hir = lower(&module).expect("supported source should lower");
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::CallUserFunction {
                name: "foo".to_string(),
            })]
        );
    }

    #[test]
    fn calling_a_non_print_function_with_arguments_is_a_diagnostic() {
        assert_unsupported("foo(42)\n", "arguments to user-defined function `foo`");
    }

    #[test]
    fn keyword_arguments_are_a_diagnostic() {
        assert_unsupported("print(42, end=\"\")\n", "keyword arguments");
    }

    #[test]
    fn print_with_wrong_argument_count_is_a_diagnostic() {
        assert_unsupported("print(1, 2)\n", "exactly one integer literal");
    }

    #[test]
    fn print_with_a_float_argument_is_a_diagnostic() {
        assert_unsupported("print(3.14)\n", "only integer literal");
    }

    #[test]
    fn print_with_an_integer_too_large_for_i64_is_a_diagnostic() {
        assert_unsupported(
            "print(99999999999999999999999999999999)\n",
            "signed 64-bit range",
        );
    }

    #[test]
    fn function_parameters_are_a_diagnostic_until_preserved_in_hir() {
        assert_unsupported("def f(x) -> None:\n    print(42)\n", "function parameters");
    }

    #[test]
    fn async_functions_are_a_diagnostic() {
        assert_unsupported("async def f() -> None:\n    print(42)\n", "async functions");
    }

    #[test]
    fn decorated_functions_are_a_diagnostic() {
        assert_unsupported(
            "@decorator\ndef f() -> None:\n    print(42)\n",
            "function decorators",
        );
    }

    #[test]
    fn generic_functions_are_a_diagnostic() {
        assert_unsupported(
            "def f[T]() -> None:\n    print(42)\n",
            "generic function type parameters",
        );
    }

    #[test]
    fn non_none_return_annotations_are_a_diagnostic() {
        assert_unsupported(
            "def f() -> int:\n    print(42)\n",
            "only a `None` return annotation",
        );
    }

    fn parse(source: &str) -> ModModule {
        pycc_parser::parse(source).expect("test fixture must parse")
    }

    fn assert_unsupported(source: &str, expected_message: &str) {
        let module = parse(source);
        let diagnostic = lower(&module).expect_err("unsupported source should be rejected");
        assert_eq!(diagnostic.code, "L0003");
        assert!(
            diagnostic.message.contains(expected_message),
            "unexpected diagnostic: {diagnostic:?}"
        );
        let span = diagnostic
            .span
            .expect("lowering diagnostics need a primary span");
        assert!(span.end > span.start, "diagnostic span must not be empty");
    }
}
