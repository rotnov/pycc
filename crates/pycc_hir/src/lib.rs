use pycc_ast::{Expr, ExprCall, ModModule, Number, Ranged, Stmt, StmtFunctionDef, TextRange};
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

enum PendingItem {
    Function(String),
    TopLevelStmt(HirStmt),
}

#[derive(Default)]
struct ResolutionState {
    bodies: HashMap<String, Vec<HirStmt>>,
    bindings: HashSet<(String, Vec<String>)>,
    active_calls: HashSet<String>,
    misses: usize,
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
    lower_with_resolution_misses(module).map(|(module, _)| module)
}

fn lower_with_resolution_misses(module: &ModModule) -> Result<(HirModule, usize), Diagnostic> {
    let mut function_names = HashSet::new();
    for stmt in &module.body {
        if let Stmt::FunctionDef(function) = stmt {
            let name = function.name.id.as_str();
            if !function_names.insert(name.to_string()) {
                return Err(unsupported(
                    format!("redefining function `{name}` is not supported so far"),
                    function.name.range(),
                ));
            }
        }
    }

    let mut available_functions = HashSet::new();
    let mut functions = HashMap::<String, &StmtFunctionDef>::new();
    let mut resolution = ResolutionState::default();
    let mut pending_items = Vec::new();
    for stmt in &module.body {
        match stmt {
            Stmt::FunctionDef(function) => {
                validate_function_signature(function)?;
                let name = function.name.id.as_str().to_string();
                functions.insert(name.clone(), function);
                available_functions.insert(name.clone());
                pending_items.push(PendingItem::Function(name));
            }
            other => {
                let stmt = lower_stmt(other, &available_functions, &functions, &mut resolution)?;
                pending_items.push(PendingItem::TopLevelStmt(stmt));
            }
        }
    }

    for item in &pending_items {
        let PendingItem::Function(name) = item else {
            continue;
        };
        if !resolution.bodies.contains_key(name) {
            let function = functions
                .get(name)
                .expect("every pending function must have a definition");
            resolve_function(
                name,
                function.name.range(),
                &available_functions,
                &functions,
                &mut resolution,
            )?;
        }
    }

    let items = pending_items
        .into_iter()
        .map(|item| match item {
            PendingItem::Function(name) => HirItem::Function {
                body: resolution
                    .bodies
                    .remove(&name)
                    .expect("every function body must be resolved"),
                name,
            },
            PendingItem::TopLevelStmt(stmt) => HirItem::TopLevelStmt(stmt),
        })
        .collect();
    Ok((HirModule { items }, resolution.misses))
}

fn lower_stmt(
    stmt: &Stmt,
    available_functions: &HashSet<String>,
    functions: &HashMap<String, &StmtFunctionDef>,
    resolution: &mut ResolutionState,
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

    let name = name.id.as_str();
    if available_functions.contains(name) {
        let [] = arguments.args.as_ref() else {
            return Err(unsupported(
                format!(
                    "calling a user-defined function with arguments is not supported yet -- only \
                 zero-argument calls like `{name}()`",
                ),
                stmt.range(),
            ));
        };
        resolve_function(
            name,
            func.range(),
            available_functions,
            functions,
            resolution,
        )?;
        Ok(HirStmt::CallUserFunction {
            name: name.to_string(),
        })
    } else if name == "print" {
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
                 zero-argument calls like `{name}()`",
                ),
                stmt.range(),
            ));
        };
        if is_python_builtin(name) {
            Err(unsupported_builtin(name, func.range()))
        } else {
            Err(undefined_function(name, func.range()))
        }
    }
}

fn resolve_function(
    name: &str,
    invocation_range: TextRange,
    available_functions: &HashSet<String>,
    functions: &HashMap<String, &StmtFunctionDef>,
    resolution: &mut ResolutionState,
) -> Result<(), Diagnostic> {
    let mut bindings = available_functions.iter().cloned().collect::<Vec<_>>();
    bindings.sort_unstable();
    let resolution_key = (name.to_string(), bindings);
    if resolution.bindings.contains(&resolution_key) {
        return Ok(());
    }
    if !resolution.active_calls.insert(name.to_string()) {
        return Ok(());
    }
    resolution.misses += 1;
    let function = functions
        .get(name)
        .expect("every available function must have a definition");
    let body = function
        .body
        .iter()
        .map(|stmt| lower_stmt(stmt, available_functions, functions, resolution))
        .collect::<Result<Vec<_>, _>>();
    resolution.active_calls.remove(name);
    let body = body?;

    let result = match resolution.bodies.get(name) {
        Some(resolved) if resolved != &body => Err(unsupported(
            format!(
                "calling function `{name}` under different module bindings is not supported so far"
            ),
            invocation_range,
        )),
        Some(_) => Ok(()),
        None => {
            resolution.bodies.insert(name.to_string(), body);
            Ok(())
        }
    };
    if result.is_ok() {
        resolution.bindings.insert(resolution_key);
    }
    result
}

fn validate_function_signature(function: &StmtFunctionDef) -> Result<(), Diagnostic> {
    if function.is_async {
        return Err(unsupported_function_signature(function.range()));
    }
    if !function.decorator_list.is_empty() {
        return Err(unsupported_function_signature(function.range()));
    }
    if function.type_params.is_some() {
        return Err(unsupported_function_signature(function.range()));
    }
    if !function.parameters.is_empty() {
        return Err(unsupported_function_signature(function.range()));
    }
    if !matches!(function.returns.as_deref(), Some(Expr::NoneLiteral(_))) {
        return Err(unsupported_function_signature(function.range()));
    }
    Ok(())
}

fn unsupported_function_signature(range: TextRange) -> Diagnostic {
    unsupported(
        "only undecorated synchronous zero-argument functions returning `None` are supported so far",
        range,
    )
}

fn unsupported_builtin(name: &str, range: TextRange) -> Diagnostic {
    unsupported(
        format!("the Python built-in `{name}` is not supported so far"),
        range,
    )
}

fn is_python_builtin(name: &str) -> bool {
    matches!(
        name,
        "__build_class__"
            | "__import__"
            | "ArithmeticError"
            | "AssertionError"
            | "AttributeError"
            | "BaseException"
            | "BaseExceptionGroup"
            | "BlockingIOError"
            | "BrokenPipeError"
            | "BufferError"
            | "BytesWarning"
            | "ChildProcessError"
            | "ConnectionAbortedError"
            | "ConnectionError"
            | "ConnectionRefusedError"
            | "ConnectionResetError"
            | "DeprecationWarning"
            | "EOFError"
            | "EncodingWarning"
            | "EnvironmentError"
            | "Exception"
            | "ExceptionGroup"
            | "FileExistsError"
            | "FileNotFoundError"
            | "FloatingPointError"
            | "FutureWarning"
            | "GeneratorExit"
            | "ImportError"
            | "ImportWarning"
            | "IndentationError"
            | "IndexError"
            | "InterruptedError"
            | "IOError"
            | "IsADirectoryError"
            | "KeyError"
            | "KeyboardInterrupt"
            | "LookupError"
            | "MemoryError"
            | "ModuleNotFoundError"
            | "NameError"
            | "NotADirectoryError"
            | "NotImplementedError"
            | "OSError"
            | "OverflowError"
            | "PendingDeprecationWarning"
            | "PermissionError"
            | "ProcessLookupError"
            | "PythonFinalizationError"
            | "RecursionError"
            | "ReferenceError"
            | "ResourceWarning"
            | "RuntimeError"
            | "RuntimeWarning"
            | "StopAsyncIteration"
            | "StopIteration"
            | "SyntaxError"
            | "SyntaxWarning"
            | "SystemError"
            | "SystemExit"
            | "TabError"
            | "TimeoutError"
            | "TypeError"
            | "UnboundLocalError"
            | "UnicodeDecodeError"
            | "UnicodeEncodeError"
            | "UnicodeError"
            | "UnicodeTranslateError"
            | "UnicodeWarning"
            | "UserWarning"
            | "ValueError"
            | "Warning"
            | "ZeroDivisionError"
            | "abs"
            | "aiter"
            | "all"
            | "anext"
            | "any"
            | "ascii"
            | "bin"
            | "bool"
            | "breakpoint"
            | "bytearray"
            | "bytes"
            | "callable"
            | "chr"
            | "classmethod"
            | "compile"
            | "complex"
            | "delattr"
            | "dict"
            | "dir"
            | "divmod"
            | "enumerate"
            | "eval"
            | "exec"
            | "filter"
            | "float"
            | "format"
            | "frozenset"
            | "getattr"
            | "globals"
            | "hasattr"
            | "hash"
            | "help"
            | "hex"
            | "id"
            | "input"
            | "int"
            | "isinstance"
            | "issubclass"
            | "iter"
            | "len"
            | "list"
            | "locals"
            | "map"
            | "max"
            | "memoryview"
            | "min"
            | "next"
            | "object"
            | "oct"
            | "open"
            | "ord"
            | "pow"
            | "print"
            | "property"
            | "range"
            | "repr"
            | "reversed"
            | "round"
            | "set"
            | "setattr"
            | "slice"
            | "sorted"
            | "staticmethod"
            | "str"
            | "sum"
            | "super"
            | "tuple"
            | "type"
            | "vars"
            | "zip"
    ) || is_platform_builtin(name)
}

#[cfg(windows)]
fn is_platform_builtin(name: &str) -> bool {
    name == "WindowsError"
}

#[cfg(not(windows))]
fn is_platform_builtin(_name: &str) -> bool {
    false
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
    fn unsupported_function_signatures_are_rejected() {
        for source in [
            "def f(value: int) -> None:\n    print(1)\n",
            "async def f() -> None:\n    print(1)\n",
            "@decorator\ndef f() -> None:\n    print(1)\n",
            "def f[T]() -> None:\n    print(1)\n",
            "def f():\n    print(1)\n",
            "def f() -> int:\n    print(1)\n",
        ] {
            let module = pycc_parser_test_helper::parse(source);
            let error = lower(&module).unwrap_err();
            assert_eq!(error.code, "C0001", "source: {source}");
            assert!(
                error.message.contains(
                    "only undecorated synchronous zero-argument functions returning `None`"
                ),
                "source: {source}"
            );
        }
    }

    #[test]
    fn unsupported_python_builtins_are_capability_errors() {
        for source in [
            "input()\n",
            "int()\n",
            "Exception()\n",
            "ValueError()\n",
            "input()\n\ndef input() -> None:\n    print(1)\n",
        ] {
            let module = pycc_parser_test_helper::parse(source);
            let error = lower(&module).unwrap_err();
            assert_eq!(error.code, "C0001", "source: {source}");
            assert!(error.message.contains("built-in"), "source: {source}");
        }
    }

    #[test]
    fn a_module_function_can_shadow_a_python_builtin_after_its_definition() {
        let source = "def input() -> None:\n    print(1)\n\ninput()\n";
        let module = pycc_parser_test_helper::parse(source);
        let hir = lower(&module).unwrap();
        assert_eq!(hir.items.len(), 2);
    }

    #[test]
    fn a_module_function_named_print_shadows_the_print_builtin() {
        let source = "\
def print() -> None:
    helper()

def helper() -> None:
    print()

helper()
";
        let module = pycc_parser_test_helper::parse(source);
        let hir = lower(&module).unwrap();

        assert_eq!(
            hir.items,
            vec![
                HirItem::Function {
                    name: "print".to_string(),
                    body: vec![HirStmt::CallUserFunction {
                        name: "helper".to_string()
                    }],
                },
                HirItem::Function {
                    name: "helper".to_string(),
                    body: vec![HirStmt::CallUserFunction {
                        name: "print".to_string()
                    }],
                },
                HirItem::TopLevelStmt(HirStmt::CallUserFunction {
                    name: "helper".to_string()
                }),
            ]
        );
    }

    #[test]
    fn a_later_print_definition_does_not_shadow_an_earlier_execution() {
        let source = "\
def first() -> None:
    print(1)

first()

def print() -> None:
    print()
";
        let module = pycc_parser_test_helper::parse(source);
        let hir = lower(&module).unwrap();

        assert_eq!(
            hir.items,
            vec![
                HirItem::Function {
                    name: "first".to_string(),
                    body: vec![HirStmt::CallPrint { arg: 1 }],
                },
                HirItem::TopLevelStmt(HirStmt::CallUserFunction {
                    name: "first".to_string()
                }),
                HirItem::Function {
                    name: "print".to_string(),
                    body: vec![HirStmt::CallUserFunction {
                        name: "print".to_string()
                    }],
                },
            ]
        );
    }

    #[test]
    fn repeated_calls_under_the_same_bindings_reuse_the_resolved_body() {
        let source = "\
def first() -> None:
    print(1)

first()
first()
";
        let module = pycc_parser_test_helper::parse(source);
        let hir = lower(&module).unwrap();

        assert_eq!(hir.items.len(), 3);
    }

    #[test]
    fn unchanged_body_is_reused_when_unrelated_bindings_are_added() {
        let source = "\
def first() -> None:
    print(1)

first()

def later() -> None:
    print(2)

first()
";
        let module = pycc_parser_test_helper::parse(source);
        let hir = lower(&module).unwrap();

        assert_eq!(hir.items.len(), 4);
    }

    #[test]
    fn doubling_call_graph_is_lowered_once_per_function_and_binding_set() {
        let depth = 20;
        let mut source = String::from("def f0() -> None:\n    print(0)\n\n");
        for index in 1..=depth {
            source.push_str(&format!(
                "def f{index}() -> None:\n    f{}()\n    f{}()\n\n",
                index - 1,
                index - 1,
            ));
        }
        source.push_str(&format!("f{depth}()\n"));

        let module = pycc_parser_test_helper::parse(&source);
        let (hir, resolution_misses) = lower_with_resolution_misses(&module).unwrap();

        assert_eq!(hir.items.len(), depth + 2);
        assert_eq!(resolution_misses, depth + 1);
    }

    #[test]
    fn conflicting_cached_function_bodies_are_rejected() {
        let source = "print(0)\n\ndef first() -> None:\n    print(1)\n";
        let module = pycc_parser_test_helper::parse(source);
        let function = module
            .body
            .iter()
            .find_map(|stmt| match stmt {
                Stmt::FunctionDef(function) => Some(function),
                _ => None,
            })
            .unwrap();
        let mut functions = HashMap::new();
        functions.insert("first".to_string(), function);
        let available_functions = HashSet::from(["first".to_string()]);
        let mut resolution = ResolutionState {
            bodies: HashMap::from([(
                "first".to_string(),
                vec![HirStmt::CallUserFunction {
                    name: "print".to_string(),
                }],
            )]),
            ..ResolutionState::default()
        };

        let error = resolve_function(
            "first",
            function.name.range(),
            &available_functions,
            &functions,
            &mut resolution,
        )
        .unwrap_err();

        assert_eq!(error.code, "C0001");
        assert!(error.message.contains("under different module bindings"));
    }

    #[test]
    fn every_python_3_14_builtin_exception_class_is_classified_as_a_builtin() {
        for name in [
            "BaseException",
            "BaseExceptionGroup",
            "GeneratorExit",
            "KeyboardInterrupt",
            "SystemExit",
            "Exception",
            "ArithmeticError",
            "FloatingPointError",
            "OverflowError",
            "ZeroDivisionError",
            "AssertionError",
            "AttributeError",
            "BufferError",
            "EOFError",
            "ExceptionGroup",
            "ImportError",
            "ModuleNotFoundError",
            "LookupError",
            "IndexError",
            "KeyError",
            "MemoryError",
            "NameError",
            "UnboundLocalError",
            "OSError",
            "EnvironmentError",
            "IOError",
            "BlockingIOError",
            "ChildProcessError",
            "ConnectionError",
            "BrokenPipeError",
            "ConnectionAbortedError",
            "ConnectionRefusedError",
            "ConnectionResetError",
            "FileExistsError",
            "FileNotFoundError",
            "InterruptedError",
            "IsADirectoryError",
            "NotADirectoryError",
            "PermissionError",
            "ProcessLookupError",
            "TimeoutError",
            "ReferenceError",
            "RuntimeError",
            "NotImplementedError",
            "PythonFinalizationError",
            "RecursionError",
            "StopAsyncIteration",
            "StopIteration",
            "SyntaxError",
            "IndentationError",
            "TabError",
            "SystemError",
            "TypeError",
            "ValueError",
            "UnicodeError",
            "UnicodeDecodeError",
            "UnicodeEncodeError",
            "UnicodeTranslateError",
            "Warning",
            "BytesWarning",
            "DeprecationWarning",
            "EncodingWarning",
            "FutureWarning",
            "ImportWarning",
            "PendingDeprecationWarning",
            "ResourceWarning",
            "RuntimeWarning",
            "SyntaxWarning",
            "UnicodeWarning",
            "UserWarning",
        ] {
            assert!(is_python_builtin(name), "missing built-in: {name}");
        }
        assert_eq!(is_python_builtin("WindowsError"), cfg!(windows));
    }

    #[test]
    fn redefining_a_function_is_rejected_until_bindings_have_identity() {
        let source = "\
def f() -> None:
    print(1)

f()

def f() -> None:
    print(2)

f()
";
        let module = pycc_parser_test_helper::parse(source);
        let error = lower(&module).unwrap_err();
        let second_definition = source.match_indices("def f").nth(1).unwrap().0;
        let start = u32::try_from(second_definition + "def ".len()).unwrap();

        assert_eq!(error.code, "C0001");
        assert_eq!(error.span, Some(Span::new(start, start + 1)));
        assert!(error.message.contains("redefining function `f`"));
    }

    #[test]
    fn calling_a_non_print_function_with_arguments_is_unsupported() {
        for source in ["foo(42)\n", "def foo() -> None:\n    print(1)\n\nfoo(42)\n"] {
            let module = pycc_parser_test_helper::parse(source);
            let error = lower(&module).unwrap_err();
            assert_eq!(error.code, "C0001", "source: {source}");
            assert!(
                error
                    .message
                    .contains("calling a user-defined function with arguments"),
                "source: {source}"
            );
        }
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
