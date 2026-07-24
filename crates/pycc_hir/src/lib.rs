use pycc_ast::{Expr, ExprCall, ModModule, Number, Ranged, Stmt, TextRange};
use pycc_diag::{Diagnostic, Span};
use std::collections::{HashMap, HashSet};

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

struct FunctionCall {
    name: String,
    range: TextRange,
}

/// Lowers a parsed module into the subset of HIR implemented by this pycc
/// version.
///
/// Syntactically valid Python outside that subset is returned as `C0001`
/// instead of panicking, while calls to undefined functions are returned as
/// `T0004`, so CLI callers can report ordinary frontend errors. Module-level
/// calls observe Python's source-order name binding; function bodies may refer
/// to later module functions only when top-level execution reaches the call
/// after those definitions have run.
pub fn lower(module: &ModModule) -> Result<HirModule, Diagnostic> {
    let module_functions = module
        .body
        .iter()
        .filter_map(|stmt| match stmt {
            Stmt::FunctionDef(function) => Some(function.name.id.as_str().to_string()),
            _ => None,
        })
        .collect::<HashSet<_>>();
    let mut available_functions = HashSet::new();
    let mut function_calls = HashMap::<String, Vec<FunctionCall>>::new();
    let mut items = Vec::new();
    for stmt in &module.body {
        match stmt {
            Stmt::FunctionDef(f) => {
                let mut calls = Vec::new();
                let body = f
                    .body
                    .iter()
                    .map(|stmt| lower_stmt(stmt, &module_functions, &mut calls))
                    .collect::<Result<Vec<_>, _>>()?;
                let name = f.name.id.as_str().to_string();
                function_calls.insert(name.clone(), calls);
                available_functions.insert(name.clone());
                items.push(HirItem::Function { name, body });
            }
            other => {
                let mut calls = Vec::new();
                let stmt = lower_stmt(other, &module_functions, &mut calls)?;
                for call in &calls {
                    validate_available_call(
                        call,
                        &available_functions,
                        &function_calls,
                        &mut HashSet::new(),
                    )?;
                }
                items.push(HirItem::TopLevelStmt(stmt));
            }
        }
    }
    Ok(HirModule { items })
}

fn lower_stmt(
    stmt: &Stmt,
    module_functions: &HashSet<String>,
    calls: &mut Vec<FunctionCall>,
) -> Result<HirStmt, Diagnostic> {
    let Stmt::Expr(expr_stmt) = stmt else {
        return Err(unsupported(
            "only a bare call expression statement is supported so far",
            stmt.range(),
        ));
    };
    let Expr::Call(ExprCall {
        func, arguments, ..
    }) = expr_stmt.value.as_ref()
    else {
        return Err(unsupported(
            "only a call expression statement is supported so far",
            stmt.range(),
        ));
    };
    let Expr::Name(name) = func.as_ref() else {
        return Err(unsupported(
            "only calling a bare name is supported so far",
            stmt.range(),
        ));
    };
    if !arguments.keywords.is_empty() {
        return Err(unsupported(
            "keyword arguments are not supported so far",
            stmt.range(),
        ));
    }

    if name.id.as_str() == "print" {
        let [Expr::NumberLiteral(lit)] = arguments.args.as_ref() else {
            return Err(unsupported(
                "print() must take exactly one integer literal argument so far",
                stmt.range(),
            ));
        };
        let Number::Int(i) = &lit.value else {
            return Err(unsupported(
                "only integer literal arguments are supported so far",
                stmt.range(),
            ));
        };
        let Some(arg) = i.as_i64() else {
            return Err(unsupported(
                "integer literal is too large for v0.1's i64-only HIR",
                stmt.range(),
            ));
        };
        Ok(HirStmt::CallPrint { arg })
    } else {
        let [] = arguments.args.as_ref() else {
            return Err(unsupported(
                format!(
                    "calling a user-defined function with arguments is not supported yet -- only \
                 zero-argument calls like `{}()`",
                    name.id.as_str(),
                ),
                stmt.range(),
            ));
        };
        if !module_functions.contains(name.id.as_str()) {
            return Err(undefined_function(name.id.as_str(), name.range()));
        }
        calls.push(FunctionCall {
            name: name.id.as_str().to_string(),
            range: name.range(),
        });
        Ok(HirStmt::CallUserFunction {
            name: name.id.as_str().to_string(),
        })
    }
}

fn validate_available_call(
    call: &FunctionCall,
    available_functions: &HashSet<String>,
    function_calls: &HashMap<String, Vec<FunctionCall>>,
    active_calls: &mut HashSet<String>,
) -> Result<(), Diagnostic> {
    if !available_functions.contains(call.name.as_str()) {
        return Err(undefined_function(&call.name, call.range));
    }
    if !active_calls.insert(call.name.clone()) {
        return Ok(());
    }
    for nested_call in &function_calls[call.name.as_str()] {
        validate_available_call(
            nested_call,
            available_functions,
            function_calls,
            active_calls,
        )?;
    }
    active_calls.remove(call.name.as_str());
    Ok(())
}

fn undefined_function(name: &str, range: TextRange) -> Diagnostic {
    Diagnostic::error(
        "T0004",
        format!("call to undefined function `{name}`"),
        Span::new(range.start().into(), range.end().into()),
        "not defined in this module",
    )
}

fn unsupported(message: impl Into<String>, range: TextRange) -> Diagnostic {
    Diagnostic::error(
        "C0001",
        message,
        Span::new(range.start().into(), range.end().into()),
        "unsupported by this pycc version",
    )
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
        let source = "x = 1\n";
        let module = pycc_parser_test_helper::parse(source);
        let error = lower(&module).unwrap_err();
        assert_eq!(error.code, "C0001");
        assert_eq!(
            error.span,
            Some(Span::new(
                0,
                u32::try_from(source.trim_end().len()).unwrap()
            ))
        );
        assert!(
            error
                .message
                .contains("only a bare call expression statement")
        );
    }

    #[test]
    fn unsupported_statement_inside_a_function_is_reported() {
        let source = "def main() -> None:\n    x = 1\n";
        let module = pycc_parser_test_helper::parse(source);
        let error = lower(&module).unwrap_err();
        assert_eq!(error.code, "C0001");
        let start = u32::try_from(source.find("x = 1").unwrap()).unwrap();
        assert_eq!(error.span, Some(Span::new(start, start + 5)));
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
        let module = pycc_parser_test_helper::parse("def foo() -> None:\n    print(1)\n\nfoo()\n");
        let hir = lower(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![
                HirItem::Function {
                    name: "foo".to_string(),
                    body: vec![HirStmt::CallPrint { arg: 1 }],
                },
                HirItem::TopLevelStmt(HirStmt::CallUserFunction {
                    name: "foo".to_string()
                }),
            ]
        );
    }

    #[test]
    fn calling_an_undefined_function_at_top_level_is_rejected() {
        let source = "does_not_exist()\n";
        let module = pycc_parser_test_helper::parse(source);
        let error = lower(&module).unwrap_err();
        assert_eq!(error.code, "T0004");
        assert_eq!(
            error.span,
            Some(Span::new(0, u32::try_from("does_not_exist".len()).unwrap()))
        );
        assert_eq!(error.label.as_deref(), Some("not defined in this module"));
        assert!(error.message.contains("does_not_exist"));
    }

    #[test]
    fn calling_an_undefined_function_inside_a_function_is_rejected() {
        let source = "def main() -> None:\n    does_not_exist()\n";
        let module = pycc_parser_test_helper::parse(source);
        let error = lower(&module).unwrap_err();
        let start = u32::try_from(source.find("does_not_exist").unwrap()).unwrap();
        assert_eq!(error.code, "T0004");
        assert_eq!(
            error.span,
            Some(Span::new(
                start,
                start + u32::try_from("does_not_exist".len()).unwrap()
            ))
        );
    }

    #[test]
    fn calling_a_function_before_its_definition_is_rejected() {
        let source = "helper()\n\ndef helper() -> None:\n    print(1)\n";
        let module = pycc_parser_test_helper::parse(source);
        let error = lower(&module).unwrap_err();
        assert_eq!(error.code, "T0004");
        assert_eq!(error.span, Some(Span::new(0, 6)));
        assert_eq!(error.label.as_deref(), Some("not defined in this module"));
    }

    #[test]
    fn calling_a_function_that_reaches_a_later_definition_is_rejected() {
        let source = "\
def first() -> None:
    second()

first()

def second() -> None:
    print(2)
";
        let module = pycc_parser_test_helper::parse(source);
        let error = lower(&module).unwrap_err();
        let start = u32::try_from(source.find("second()").unwrap()).unwrap();
        assert_eq!(error.code, "T0004");
        assert_eq!(error.span, Some(Span::new(start, start + 6)));
    }

    #[test]
    fn mutually_recursive_functions_are_resolved_after_both_definitions() {
        let source = "\
def first() -> None:
    second()

def second() -> None:
    first()

first()
";
        let module = pycc_parser_test_helper::parse(source);
        let hir = lower(&module).unwrap();
        assert_eq!(hir.items.len(), 3);
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
