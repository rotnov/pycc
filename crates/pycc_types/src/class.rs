//! Class-body type-checking (D-154, Part 1 of #375): resolving instance
//! instantiation, attribute access, and method calls against
//! `Environment`'s class table.
//!
//! Unlike a hand-rolled per-class checking pass, every method (including
//! `__init__`) is already an ordinary `HirItem::Function` under its mangled
//! `<ClassName>.<method_name>` name (see `pycc_hir::class`'s own doc
//! comment) -- so `check_function_in`/`check_generic_function_in` and the
//! constraint solver already check a method body exactly like any other
//! function, with `self` bound to `Ty::Instance(class_name)` by the same
//! ordinary parameter-binding logic every other parameter goes through. The
//! functions in this module are only the *additional* pieces that ordinary
//! function-checking has no shape for: resolving `ClassName(...)`
//! (instantiation), `base.attr` (an instance attribute read or write), and
//! `base.method(...)` (an instance method call) against the class's
//! declared shape.

use crate::{Environment, infer_expr_in, is_assignable};
use pycc_diag::{Diagnostic, Span};
use pycc_hir::{HirClassDef, HirExpr, HirModule, Ty};

/// Populates `env`'s class table from `hir.class_defs` -- called once by
/// every `Environment` constructor this crate has (`check_with_signatures`'s
/// own per-item loop, `concrete_function_environment`'s literal), mirroring
/// how each already registers every function's signature.
pub(crate) fn bind_classes(env: &mut Environment, hir: &HirModule) {
    for (name, class_def) in &hir.class_defs {
        env.bind_class(name.clone(), class_def.clone());
    }
}

fn t0043_not_an_instance(action: &str, ty: &Ty) -> Diagnostic {
    Diagnostic::error(
        "T0043",
        format!("cannot {action} on `{}`: it is not a class instance", ty.name()),
        Span::new(0, 0),
    )
}

fn t0044_unknown_member(kind: &str, class_name: &str, member: &str) -> Diagnostic {
    Diagnostic::error(
        "T0044",
        format!("class `{class_name}` has no {kind} named `{member}`"),
        Span::new(0, 0),
    )
}

/// Looks up `class_name`'s declared shape, panicking if it isn't
/// registered. Every caller in this module only ever calls this with a
/// class name extracted from a real `Ty::Instance` payload (either produced
/// by `resolve_instantiation` below, which only ever builds one from a
/// class `env.lookup_class` just confirmed exists, or from `self`'s own
/// type, assigned directly by `pycc_hir::class::lower_method` from the
/// enclosing class's own name) -- so an unregistered name reaching here
/// would mean `Environment::classes` was built from a different
/// `HirModule` than the one the `Ty::Instance` value itself came from, an
/// internal-consistency bug this crate has no way to recover from
/// meaningfully, matching `pycc_mir`'s own `lookup` panic-on-inconsistency
/// convention (see that function's own doc comment).
fn expect_class<'e>(env: &'e Environment, class_name: &str) -> &'e HirClassDef {
    env.lookup_class(class_name).unwrap_or_else(|| {
        panic!(
            "pycc_types: internal error: class `{class_name}` has no registered \
             HirClassDef -- Environment::classes was built from a different HirModule \
             than the one this Ty::Instance came from"
        )
    })
}

/// Validates a call's arguments against a resolved `(param_tys, return_ty)`
/// signature, reusing this crate's own existing "call to undefined
/// function"-adjacent diagnostic shape (`T0021`, `infer_expr_in`'s own
/// `HirExpr::Call` arm) rather than inventing a class-specific arity/type
/// mismatch code -- an instantiation call and a method call are both, at
/// their core, "call this mangled function with these arguments," the same
/// shape an ordinary function call already validates.
fn check_call_args(callee: &str, arg_tys: &[Ty], param_tys: &[Ty]) -> Result<(), Diagnostic> {
    if arg_tys.len() != param_tys.len() {
        return Err(Diagnostic::error(
            "T0021",
            format!(
                "`{callee}` expects {} argument(s), got {}",
                param_tys.len(),
                arg_tys.len()
            ),
            Span::new(0, 0),
        )
        .with_help(format!("pass exactly {} argument(s)", param_tys.len())));
    }
    for (i, (arg_ty, param_ty)) in arg_tys.iter().zip(param_tys.iter()).enumerate() {
        if !is_assignable(arg_ty.clone(), param_ty.clone()) {
            return Err(Diagnostic::error(
                "T0021",
                format!(
                    "argument {} of `{callee}` expects `{}`, got `{}`",
                    i + 1,
                    param_ty.name(),
                    arg_ty.name()
                ),
                Span::new(0, 0),
            )
            .with_help(format!("pass a `{}` value", param_ty.name())));
        }
    }
    Ok(())
}

/// Resolves `ClassName(args)` (instantiation) -- called by
/// `infer_expr_in`'s `HirExpr::Call` arm only after `env.lookup_class`
/// confirms `class_name` is a real, registered class, so the mangled
/// `<ClassName>.__init__` function this looks up is always present (every
/// `HirClassDef` requires an `__init__`, per `pycc_hir::class::lower_class`).
pub(crate) fn resolve_instantiation(
    env: &Environment,
    class_name: &str,
    arg_tys: &[Ty],
) -> Result<Ty, Diagnostic> {
    let mangled = format!("{class_name}.__init__");
    let (param_tys, _return_ty) = env.lookup_function(&mangled).unwrap_or_else(|| {
        panic!(
            "pycc_types: internal error: `{mangled}` was not registered as an ordinary \
             function -- every HirClassDef requires an __init__, mangled and lowered \
             into HirModule::items exactly like this crate's other functions"
        )
    });
    // `param_tys[0]` is always `self`'s own `Ty::Instance(class_name)` --
    // never part of the argument list a caller actually supplies.
    let ctor_param_tys = &param_tys[1..];
    check_call_args(class_name, arg_tys, ctor_param_tys)?;
    Ok(Ty::Instance(Box::new(class_name.to_string())))
}

/// Resolves `base.attr` (an instance attribute read) against `base_ty`.
/// Shared by `infer_expr_in`'s `HirExpr::AttrGet` arm and `check_stmt`'s
/// `HirStmt::AttrSet` arm (which also needs `base`'s attribute type, to
/// check the assigned value against it).
///
/// #377: a `@property` getter is checked *before* the regular attribute
/// slot table -- `obj.x` where `x` is a property resolves to the getter
/// method's return type, not a slot type. This mirrors CPython's own
/// observable behavior, where a property descriptor intercepts attribute
/// access before the instance's `__dict__`/slot table is consulted.
pub(crate) fn resolve_attr_get(env: &Environment, base_ty: &Ty, attr: &str) -> Result<Ty, Diagnostic> {
    let Ty::Instance(class_name) = base_ty else {
        return Err(t0043_not_an_instance("read an attribute", base_ty));
    };
    let class_def = expect_class(env, class_name);
    // #377: check properties before regular attribute slots, matching
    // CPython's descriptor protocol precedence (a property descriptor
    // intercepts attribute access before `__dict__`).
    if let Some(prop) = class_def.properties.iter().find(|p| p.name == attr) {
        let (_, return_ty) = env.lookup_function(&prop.getter).unwrap_or_else(|| {
            panic!(
                "pycc_types: internal error: property getter `{}` is in class `{class_name}`'s \
                 own property table but was not registered as an ordinary function",
                prop.getter
            )
        });
        return Ok(return_ty.clone());
    }
    class_def
        .attrs
        .iter()
        .find(|(name, _)| name == attr)
        .map(|(_, ty)| ty.clone())
        .ok_or_else(|| t0044_unknown_member("attribute", class_name, attr))
}

/// Resolves `base.method(args)` against `base_ty`, checking the call's
/// arguments against the method's own resolved signature (excluding
/// `self`, exactly like `resolve_instantiation` excludes it from a
/// constructor call) and returning the method's return type.
pub(crate) fn resolve_method_call(
    env: &Environment,
    base_ty: &Ty,
    method: &str,
    arg_tys: &[Ty],
) -> Result<Ty, Diagnostic> {
    let Ty::Instance(class_name) = base_ty else {
        return Err(t0043_not_an_instance("call a method", base_ty));
    };
    let class_def = expect_class(env, class_name);
    let Some((_, mangled)) = class_def.methods.iter().find(|(name, _)| name == method) else {
        return Err(t0044_unknown_member("method", class_name, method));
    };
    let (param_tys, return_ty) = env.lookup_function(mangled).unwrap_or_else(|| {
        panic!(
            "pycc_types: internal error: `{mangled}` is in class `{class_name}`'s own \
             method table but was not registered as an ordinary function"
        )
    });
    let method_param_tys = &param_tys[1..]; // exclude `self`
    check_call_args(method, arg_tys, method_param_tys)?;
    Ok(return_ty.clone())
}

/// Checks `base.attr = value` (`HirStmt::AttrSet`), shared between module
/// scope (`check_stmt`, `local_names = &[]`) and function-body scope
/// (`check_stmt_in_function`) -- mirroring how `check_dict_set` is already
/// split the same way for `HirStmt::DictSet`. Reuses [`resolve_attr_get`]
/// for the attribute-type lookup, so a base that isn't a class instance or
/// an attribute name the class never declares produces the identical
/// `T0043`/`T0044` diagnostic an attribute *read* would.
///
/// #377: if `attr` is a `@property`, the check is redirected to the
/// property's setter: a read-only property (no setter) is rejected with
/// `T0044`, and a property with a setter checks the assigned value against
/// the setter's own parameter type (not the getter's return type -- the
/// two may differ, though they usually match). This mirrors CPython's own
/// observable behavior, where `obj.x = value` invokes the property's
/// `__set__` descriptor method, not a bare slot write.
pub(crate) fn check_attr_set(
    env: &Environment,
    local_names: &[&str],
    base: &HirExpr,
    attr: &str,
    value: &HirExpr,
) -> Result<(), Diagnostic> {
    let base_ty = infer_expr_in(env, local_names, base)?;
    // #377: check properties before regular attribute slots. A property
    // setter has its own parameter type (the value the setter accepts),
    // which may differ from the getter's return type -- so the value is
    // checked against the setter's parameter, not `resolve_attr_get`'s
    // getter-return-type result.
    if let Ty::Instance(class_name) = &base_ty {
        let class_def = expect_class(env, class_name);
        if let Some(prop) = class_def.properties.iter().find(|p| p.name == attr) {
            let value_ty = infer_expr_in(env, local_names, value)?;
            let Some(setter_mangled) = &prop.setter else {
                return Err(Diagnostic::error(
                    "T0044",
                    format!(
                        "property `{attr}` of class `{class_name}` is read-only (has no setter)"
                    ),
                    Span::new(0, 0),
                ));
            };
            let (param_tys, _) = env.lookup_function(setter_mangled).unwrap_or_else(|| {
                panic!(
                    "pycc_types: internal error: property setter `{setter_mangled}` is in class \
                     `{class_name}`'s own property table but was not registered as an ordinary \
                     function"
                )
            });
            let setter_param_ty = &param_tys[1]; // exclude `self`
            if !is_assignable(value_ty.clone(), setter_param_ty.clone()) {
                return Err(Diagnostic::error(
                    "T0021",
                    format!(
                        "cannot assign `{}` to property `{attr}` (setter expects `{}`)",
                        value_ty.name(),
                        setter_param_ty.name()
                    ),
                    Span::new(0, 0),
                )
                .with_help(format!(
                    "change the value to `{}` (the setter's expected type), or the \
                     setter's parameter annotation to `{}` (the actual type)",
                    setter_param_ty.name(),
                    value_ty.name()
                )));
            }
            return Ok(());
        }
    }
    // Regular attribute slot (existing behavior).
    let attr_ty = resolve_attr_get(env, &base_ty, attr)?;
    let value_ty = infer_expr_in(env, local_names, value)?;
    if !is_assignable(value_ty.clone(), attr_ty.clone()) {
        return Err(Diagnostic::error(
            "T0021",
            format!(
                "cannot assign `{}` to attribute `{attr}` of type `{}`",
                value_ty.name(),
                attr_ty.name()
            ),
            Span::new(0, 0),
        )
        .with_help(format!(
            "change the value to `{}` (the expected/declared type), or the \
             declaration/annotation to `{}` (the actual type)",
            attr_ty.name(),
            value_ty.name()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::{check, check_and_resolve};
    use pycc_hir::{BinOpKind, HirItem, HirModule, HirStmt, Ty};
    use pycc_hir::{HirClassDef, HirExpr};

    /// Builds a minimal `Point` class module: `__init__(self, x: int, y:
    /// int)` sets both attributes from its own parameters; `bump(self) ->
    /// None` reads and mutates `self.x`. `extra_items`/`extra_stmts` let
    /// each test append its own instantiation/attribute/method-call
    /// exercise without duplicating this fixture.
    fn point_module(extra_items: Vec<HirItem>) -> HirModule {
        let self_ty = Ty::Instance(Box::new("Point".to_string()));
        let init = HirItem::Function {
            name: "Point.__init__".to_string(),
            params: vec![
                ("self".to_string(), self_ty.clone()),
                ("x".to_string(), Ty::Int),
                ("y".to_string(), Ty::Int),
            ],
            return_ty: Ty::None,
            body: vec![
                HirStmt::AttrSet {
                    base: HirExpr::Name("self".to_string()),
                    attr: "x".to_string(),
                    value: HirExpr::Name("x".to_string()),
                },
                HirStmt::AttrSet {
                    base: HirExpr::Name("self".to_string()),
                    attr: "y".to_string(),
                    value: HirExpr::Name("y".to_string()),
                },
                HirStmt::Return(None),
            ],
        };
        let bump = HirItem::Function {
            name: "Point.bump".to_string(),
            params: vec![("self".to_string(), self_ty.clone())],
            return_ty: Ty::None,
            body: vec![
                HirStmt::AttrSet {
                    base: HirExpr::Name("self".to_string()),
                    attr: "x".to_string(),
                    value: HirExpr::BinOp {
                        op: BinOpKind::Add,
                        left: Box::new(HirExpr::AttrGet {
                            base: Box::new(HirExpr::Name("self".to_string())),
                            attr: "x".to_string(),
                        }),
                        right: Box::new(HirExpr::IntLiteral(1)),
                    },
                },
                HirStmt::Return(None),
            ],
        };
        let mut items = vec![init, bump];
        items.extend(extra_items);
        HirModule {
            items,
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: vec![(
                "Point".to_string(),
                HirClassDef {
                    name: "Point".to_string(),
                    attrs: vec![("x".to_string(), Ty::Int), ("y".to_string(), Ty::Int)],
                    methods: vec![
                        ("__init__".to_string(), "Point.__init__".to_string()),
                        ("bump".to_string(), "Point.bump".to_string()),
                    ],
                    type_param: None,
                    properties: Vec::new(),
                },
            )],
        }
    }

    fn top_level(stmt: HirStmt) -> HirItem {
        HirItem::TopLevelStmt(stmt)
    }

    #[test]
    fn instantiation_attribute_read_and_method_call_all_type_check() {
        let hir = point_module(vec![
            top_level(HirStmt::Assign {
                target: "p".to_string(),
                value: HirExpr::Call {
                    callee: "Point".to_string(),
                    args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
                },
            }),
            top_level(HirStmt::ExprStmt(HirExpr::MethodCall {
                base: Box::new(HirExpr::Name("p".to_string())),
                method: "bump".to_string(),
                args: vec![],
            })),
            top_level(HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::AttrGet {
                    base: Box::new(HirExpr::Name("p".to_string())),
                    attr: "x".to_string(),
                }],
            })),
        ]);
        check(&hir).expect("a well-typed class instantiation/method-call/attribute-read program should check");
    }

    #[test]
    fn instantiating_with_the_wrong_argument_count_is_rejected() {
        let hir = point_module(vec![top_level(HirStmt::ExprStmt(HirExpr::Call {
            callee: "Point".to_string(),
            args: vec![HirExpr::IntLiteral(1)],
        }))]);
        let diagnostic = check(&hir).unwrap_err();
        assert_eq!(diagnostic.code, "T0021");
    }

    #[test]
    fn instantiating_with_a_wrong_argument_type_is_rejected() {
        let hir = point_module(vec![top_level(HirStmt::ExprStmt(HirExpr::Call {
            callee: "Point".to_string(),
            args: vec![HirExpr::IntLiteral(1), HirExpr::StringLiteral("y".to_string())],
        }))]);
        let diagnostic = check(&hir).unwrap_err();
        assert_eq!(diagnostic.code, "T0021");
    }

    #[test]
    fn reading_an_attribute_on_a_non_instance_value_is_rejected() {
        let hir = point_module(vec![top_level(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::AttrGet {
                base: Box::new(HirExpr::IntLiteral(1)),
                attr: "x".to_string(),
            }],
        }))]);
        let diagnostic = check(&hir).unwrap_err();
        assert_eq!(diagnostic.code, "T0043");
    }

    #[test]
    fn reading_an_undeclared_attribute_is_rejected() {
        let hir = point_module(vec![
            top_level(HirStmt::Assign {
                target: "p".to_string(),
                value: HirExpr::Call {
                    callee: "Point".to_string(),
                    args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
                },
            }),
            top_level(HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::AttrGet {
                    base: Box::new(HirExpr::Name("p".to_string())),
                    attr: "z".to_string(),
                }],
            })),
        ]);
        let diagnostic = check(&hir).unwrap_err();
        assert_eq!(diagnostic.code, "T0044");
    }

    #[test]
    fn calling_a_method_on_a_non_instance_value_is_rejected() {
        let hir = point_module(vec![top_level(HirStmt::ExprStmt(HirExpr::MethodCall {
            base: Box::new(HirExpr::IntLiteral(1)),
            method: "bump".to_string(),
            args: vec![],
        }))]);
        let diagnostic = check(&hir).unwrap_err();
        assert_eq!(diagnostic.code, "T0043");
    }

    #[test]
    fn calling_an_undeclared_method_is_rejected() {
        let hir = point_module(vec![
            top_level(HirStmt::Assign {
                target: "p".to_string(),
                value: HirExpr::Call {
                    callee: "Point".to_string(),
                    args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
                },
            }),
            top_level(HirStmt::ExprStmt(HirExpr::MethodCall {
                base: Box::new(HirExpr::Name("p".to_string())),
                method: "fly".to_string(),
                args: vec![],
            })),
        ]);
        let diagnostic = check(&hir).unwrap_err();
        assert_eq!(diagnostic.code, "T0044");
    }

    #[test]
    fn calling_a_method_with_a_wrong_argument_count_is_rejected() {
        let hir = point_module(vec![
            top_level(HirStmt::Assign {
                target: "p".to_string(),
                value: HirExpr::Call {
                    callee: "Point".to_string(),
                    args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
                },
            }),
            top_level(HirStmt::ExprStmt(HirExpr::MethodCall {
                base: Box::new(HirExpr::Name("p".to_string())),
                method: "bump".to_string(),
                args: vec![HirExpr::IntLiteral(1)],
            })),
        ]);
        let diagnostic = check(&hir).unwrap_err();
        assert_eq!(diagnostic.code, "T0021");
    }

    #[test]
    fn assigning_a_wrong_typed_value_to_an_attribute_is_rejected() {
        let hir = point_module(vec![
            top_level(HirStmt::Assign {
                target: "p".to_string(),
                value: HirExpr::Call {
                    callee: "Point".to_string(),
                    args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
                },
            }),
            top_level(HirStmt::AttrSet {
                base: HirExpr::Name("p".to_string()),
                attr: "x".to_string(),
                value: HirExpr::StringLiteral("nope".to_string()),
            }),
        ]);
        let diagnostic = check(&hir).unwrap_err();
        assert_eq!(diagnostic.code, "T0021");
    }

    #[test]
    fn setting_an_attribute_on_a_non_instance_value_is_rejected() {
        let hir = point_module(vec![top_level(HirStmt::AttrSet {
            base: HirExpr::IntLiteral(1),
            attr: "x".to_string(),
            value: HirExpr::IntLiteral(1),
        })]);
        let diagnostic = check(&hir).unwrap_err();
        assert_eq!(diagnostic.code, "T0043");
    }

    #[test]
    fn attribute_set_propagates_an_ill_typed_base_s_error() {
        // Exercises `check_attr_set`'s own `?` on `base`'s own inference --
        // as opposed to `setting_an_attribute_on_a_non_instance_value_is_rejected`
        // above, which only exercises `base` resolving successfully to a
        // non-instance type.
        let hir = point_module(vec![top_level(HirStmt::AttrSet {
            base: HirExpr::Name("undefined".to_string()),
            attr: "x".to_string(),
            value: HirExpr::IntLiteral(1),
        })]);
        let diagnostic = check(&hir).unwrap_err();
        assert_eq!(diagnostic.code, "T0021");
    }

    #[test]
    fn attribute_set_propagates_an_ill_typed_value_s_error() {
        // Exercises `check_attr_set`'s own `?` on `value`'s own inference.
        let hir = point_module(vec![
            top_level(HirStmt::Assign {
                target: "p".to_string(),
                value: HirExpr::Call {
                    callee: "Point".to_string(),
                    args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
                },
            }),
            top_level(HirStmt::AttrSet {
                base: HirExpr::Name("p".to_string()),
                attr: "x".to_string(),
                value: HirExpr::Name("undefined".to_string()),
            }),
        ]);
        let diagnostic = check(&hir).unwrap_err();
        assert_eq!(diagnostic.code, "T0021");
    }

    #[test]
    fn a_class_instance_is_rejected_as_a_numeric_operand() {
        // Task 3's own explicit rule: reject arithmetic on `Ty::Instance`
        // (this crate's existing D-116/D-124-style precedent), exercised
        // through the ordinary `numeric_result_type` catch-all (no
        // class-specific code needed there -- see that function's own
        // `as_numeric` closure).
        let hir = point_module(vec![
            top_level(HirStmt::Assign {
                target: "p".to_string(),
                value: HirExpr::Call {
                    callee: "Point".to_string(),
                    args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
                },
            }),
            top_level(HirStmt::ExprStmt(HirExpr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(HirExpr::Name("p".to_string())),
                right: Box::new(HirExpr::IntLiteral(1)),
            })),
        ]);
        let diagnostic = check(&hir).unwrap_err();
        assert_eq!(diagnostic.code, "T0021");
    }

    #[test]
    fn attribute_read_propagates_an_ill_typed_base_s_error() {
        // Exercises `infer_expr_in`'s own `HirExpr::AttrGet` arm's `?` on
        // `base`'s own inference -- as opposed to
        // `reading_an_attribute_on_a_non_instance_value_is_rejected` above,
        // which only exercises `base` itself resolving successfully to a
        // non-instance type.
        let hir = point_module(vec![top_level(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::AttrGet {
                base: Box::new(HirExpr::Name("undefined".to_string())),
                attr: "x".to_string(),
            }],
        }))]);
        let diagnostic = check(&hir).unwrap_err();
        assert_eq!(diagnostic.code, "T0021");
    }

    #[test]
    fn method_call_propagates_an_ill_typed_base_s_error() {
        // Exercises `infer_expr_in`'s own `HirExpr::MethodCall` arm's `?`
        // on `base`'s own inference.
        let hir = point_module(vec![top_level(HirStmt::ExprStmt(HirExpr::MethodCall {
            base: Box::new(HirExpr::Name("undefined".to_string())),
            method: "bump".to_string(),
            args: vec![],
        }))]);
        let diagnostic = check(&hir).unwrap_err();
        assert_eq!(diagnostic.code, "T0021");
    }

    #[test]
    fn method_call_propagates_an_ill_typed_argument_s_error() {
        // Exercises `infer_expr_in`'s own `HirExpr::MethodCall` arm's `?`
        // on its own argument-collection loop -- distinct from the `base`
        // propagation test above.
        let hir = point_module(vec![
            top_level(HirStmt::Assign {
                target: "p".to_string(),
                value: HirExpr::Call {
                    callee: "Point".to_string(),
                    args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
                },
            }),
            top_level(HirStmt::ExprStmt(HirExpr::MethodCall {
                base: Box::new(HirExpr::Name("p".to_string())),
                method: "bump".to_string(),
                args: vec![HirExpr::Name("undefined".to_string())],
            })),
        ]);
        let diagnostic = check(&hir).unwrap_err();
        assert_eq!(diagnostic.code, "T0021");
    }

    #[test]
    fn an_unannotated_private_method_forces_the_solver_to_walk_attribute_and_method_access() {
        // Exercises `collect_expr_constraints`'s and
        // `collect_block_constraints`'s own new `AttrGet`/`MethodCall`/
        // `AttrSet` arms (D-154): the constraint solver only runs at all
        // when at least one function in the module is not fully annotated
        // (`concrete_function_signatures` returns `None`, routing `check`
        // through `infer_function_signatures_with_solver` instead of the
        // concrete fast path) -- every other test in this module uses only
        // fully annotated methods, so none of them exercises this path.
        // `_touch` is private (D-038: an unannotated *private* name is
        // permitted) and has no return annotation, forcing exactly that.
        let self_ty = Ty::Instance(Box::new("Point".to_string()));
        let init = HirItem::Function {
            name: "Point.__init__".to_string(),
            params: vec![
                ("self".to_string(), self_ty.clone()),
                ("x".to_string(), Ty::Int),
            ],
            return_ty: Ty::None,
            body: vec![
                HirStmt::AttrSet {
                    base: HirExpr::Name("self".to_string()),
                    attr: "x".to_string(),
                    value: HirExpr::Name("x".to_string()),
                },
                HirStmt::Return(None),
            ],
        };
        let bump = HirItem::Function {
            name: "Point.bump".to_string(),
            params: vec![("self".to_string(), self_ty.clone())],
            return_ty: Ty::None,
            body: vec![
                HirStmt::AttrSet {
                    base: HirExpr::Name("self".to_string()),
                    attr: "x".to_string(),
                    value: HirExpr::IntLiteral(0),
                },
                HirStmt::Return(None),
            ],
        };
        let touch = HirItem::Function {
            name: "Point._touch".to_string(),
            params: vec![("self".to_string(), self_ty.clone())],
            // Explicitly annotated (unlike a truly unannotated private
            // method) so the solver never needs to *infer* this function's
            // own return type from `self.x` -- `AttrGet`/`MethodCall`
            // deliberately give the solver no unification term at all
            // (mirroring `ListPop`/`Subscript`'s own pre-existing
            // consequence, see `collect_expr_constraints`'s own doc
            // comment), so a truly unannotated `_touch` returning `self.x`
            // cannot be solved. This method's own job is only to put an
            // `AttrGet`/`AttrSet`/`MethodCall` inside a body the solver
            // still *walks* (see `_identity` below for what actually
            // forces solver mode for the whole module).
            return_ty: Ty::None,
            body: vec![
                HirStmt::ExprStmt(HirExpr::MethodCall {
                    base: Box::new(HirExpr::Name("self".to_string())),
                    method: "bump".to_string(),
                    args: vec![],
                }),
                HirStmt::AttrSet {
                    base: HirExpr::Name("self".to_string()),
                    attr: "x".to_string(),
                    value: HirExpr::IntLiteral(0),
                },
                HirStmt::ExprStmt(HirExpr::AttrGet {
                    base: Box::new(HirExpr::Name("self".to_string())),
                    attr: "x".to_string(),
                }),
                HirStmt::Return(None),
            ],
        };
        // Forces the whole module through the solver path (see this
        // test's own doc comment): a private, unannotated helper
        // completely unrelated to `Point`, mirroring this crate's own
        // existing `private_identity_signature_is_inferred_from_its_call_site_and_return`
        // precedent -- its own return type is inferred as `int` from its
        // one call site's argument and its own `return x`.
        let identity = HirItem::Function {
            name: "_identity".to_string(),
            params: vec![("x".to_string(), Ty::Infer)],
            return_ty: Ty::Infer,
            body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
        };
        let hir = HirModule {
            items: vec![
                init,
                bump,
                touch,
                identity,
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "_identity".to_string(),
                    args: vec![HirExpr::IntLiteral(1)],
                })),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: vec![(
                "Point".to_string(),
                HirClassDef {
                    name: "Point".to_string(),
                    attrs: vec![("x".to_string(), Ty::Int)],
                    methods: vec![
                        ("__init__".to_string(), "Point.__init__".to_string()),
                        ("bump".to_string(), "Point.bump".to_string()),
                        ("_touch".to_string(), "Point._touch".to_string()),
                    ],
                    type_param: None,
                    properties: Vec::new(),
                },
            )],
        };
        check(&hir).expect(
            "a class method reading/writing an instance attribute and calling another \
             method should check when an unrelated unannotated helper forces the solver \
             path for the whole module",
        );
    }

    #[test]
    fn solver_path_attr_get_propagates_an_ill_typed_base_s_error() {
        // Exercises `collect_expr_constraints`'s own `HirExpr::AttrGet`
        // arm's `?` on `base`'s own constraint collection -- as opposed to
        // `an_unannotated_private_method_forces_the_solver_to_walk_attribute_and_method_access`
        // above, which only exercises `base` collecting successfully. `base`
        // must be a name that is genuinely a *local* referenced before its
        // own first assignment (`collect_expr_constraints`'s `HirExpr::Name`
        // arm only errors for `is_local`-registered names -- an entirely
        // undeclared name silently resolves to `Ok(None)`, never reaching
        // this `?` at all).
        let self_ty = Ty::Instance(Box::new("Point".to_string()));
        let init = HirItem::Function {
            name: "Point.__init__".to_string(),
            params: vec![("self".to_string(), self_ty.clone())],
            return_ty: Ty::None,
            body: vec![
                HirStmt::AttrSet {
                    base: HirExpr::Name("self".to_string()),
                    attr: "x".to_string(),
                    value: HirExpr::IntLiteral(0),
                },
                HirStmt::Return(None),
            ],
        };
        let bad = HirItem::Function {
            name: "_bad".to_string(),
            params: vec![("y".to_string(), Ty::Infer)],
            return_ty: Ty::Infer,
            body: vec![
                HirStmt::ExprStmt(HirExpr::AttrGet {
                    base: Box::new(HirExpr::Name("z".to_string())),
                    attr: "x".to_string(),
                }),
                HirStmt::Assign {
                    target: "z".to_string(),
                    value: HirExpr::IntLiteral(0),
                },
                HirStmt::Return(Some(HirExpr::Name("y".to_string()))),
            ],
        };
        let hir = HirModule {
            items: vec![init, bad],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: vec![(
                "Point".to_string(),
                HirClassDef {
                    name: "Point".to_string(),
                    attrs: vec![("x".to_string(), Ty::Int)],
                    methods: vec![("__init__".to_string(), "Point.__init__".to_string())],
                    type_param: None,
                    properties: Vec::new(),
                },
            )],
        };
        assert!(check(&hir).is_err());
    }

    #[test]
    fn solver_path_method_call_propagates_an_ill_typed_base_and_argument_error() {
        // Exercises `collect_expr_constraints`'s own `HirExpr::MethodCall`
        // arm's `?` on both `base`'s own constraint collection and its
        // per-argument loop's. As in
        // `solver_path_attr_get_propagates_an_ill_typed_base_s_error` above,
        // the erroring name must be a genuine local referenced before its
        // own first assignment -- an entirely undeclared name resolves to
        // `Ok(None)` and never reaches either `?`.
        let bad_base = HirItem::Function {
            name: "_bad_base".to_string(),
            params: vec![("y".to_string(), Ty::Infer)],
            return_ty: Ty::Infer,
            body: vec![
                HirStmt::ExprStmt(HirExpr::MethodCall {
                    base: Box::new(HirExpr::Name("z".to_string())),
                    method: "bump".to_string(),
                    args: vec![],
                }),
                HirStmt::Assign {
                    target: "z".to_string(),
                    value: HirExpr::IntLiteral(0),
                },
                HirStmt::Return(Some(HirExpr::Name("y".to_string()))),
            ],
        };
        let hir1 = HirModule {
            items: vec![bad_base],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        assert!(check(&hir1).is_err());

        let bad_arg = HirItem::Function {
            name: "_bad_arg".to_string(),
            params: vec![("p".to_string(), Ty::Infer), ("y".to_string(), Ty::Infer)],
            return_ty: Ty::Infer,
            body: vec![
                HirStmt::ExprStmt(HirExpr::MethodCall {
                    base: Box::new(HirExpr::Name("p".to_string())),
                    method: "bump".to_string(),
                    args: vec![HirExpr::Name("z".to_string())],
                }),
                HirStmt::Assign {
                    target: "z".to_string(),
                    value: HirExpr::IntLiteral(0),
                },
                HirStmt::Return(Some(HirExpr::Name("y".to_string()))),
            ],
        };
        let hir2 = HirModule {
            items: vec![bad_arg],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        assert!(check(&hir2).is_err());
    }

    #[test]
    fn solver_path_attr_set_propagates_an_ill_typed_base_and_value_error() {
        // Exercises `collect_block_constraints`'s own `HirStmt::AttrSet`
        // arm's `?` on both `base`'s and `value`'s own constraint
        // collection. As in the `AttrGet`/`MethodCall` solver-path tests
        // above, the erroring name must be a genuine local referenced
        // before its own first assignment -- an entirely undeclared name
        // resolves to `Ok(None)` and never reaches either `?`.
        let bad_base = HirItem::Function {
            name: "_bad_base".to_string(),
            params: vec![("y".to_string(), Ty::Infer)],
            return_ty: Ty::Infer,
            body: vec![
                HirStmt::AttrSet {
                    base: HirExpr::Name("z".to_string()),
                    attr: "x".to_string(),
                    value: HirExpr::IntLiteral(0),
                },
                HirStmt::Assign {
                    target: "z".to_string(),
                    value: HirExpr::IntLiteral(0),
                },
                HirStmt::Return(Some(HirExpr::Name("y".to_string()))),
            ],
        };
        let hir1 = HirModule {
            items: vec![bad_base],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        assert!(check(&hir1).is_err());

        let bad_value = HirItem::Function {
            name: "_bad_value".to_string(),
            params: vec![("p".to_string(), Ty::Infer), ("y".to_string(), Ty::Infer)],
            return_ty: Ty::Infer,
            body: vec![
                HirStmt::AttrSet {
                    base: HirExpr::Name("p".to_string()),
                    attr: "x".to_string(),
                    value: HirExpr::Name("z".to_string()),
                },
                HirStmt::Assign {
                    target: "z".to_string(),
                    value: HirExpr::IntLiteral(0),
                },
                HirStmt::Return(Some(HirExpr::Name("y".to_string()))),
            ],
        };
        let hir2 = HirModule {
            items: vec![bad_value],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        assert!(check(&hir2).is_err());
    }

    #[test]
    fn a_generic_function_body_containing_attribute_and_method_access_type_checks() {
        // Exercises `reject_generic_calls_in_stmt/expr`'s and
        // `rewrite_generic_calls_in_stmt/expr`'s own new `AttrSet`/
        // `AttrGet`/`MethodCall` arms (D-154): none of this module's other
        // tests combine a class with a PEP 695 generic function, so a
        // generic function whose own body reads/writes an instance
        // attribute and calls a method is needed to walk those recursive-
        // descent helpers into these new node shapes at all.
        // `check_and_resolve` exercises both: `checked_function_signatures`
        // routes `helper` through `check_generic_function_in` (which calls
        // `reject_generic_calls_in_stmt` to reject self-recursion).
        // `monomorphize`'s own Pass 2, however, explicitly *skips* a generic
        // function's own body (`if generics.contains_key(name) { continue; }`
        // -- only a call *site* that instantiates a generic gets its
        // `substitute_body`-produced specialization, which is appended
        // as-is without ever being re-walked by `rewrite_generic_calls_in_stmt`
        // itself), so `helper`'s own `AttrSet`/`MethodCall`/`AttrGet` nodes
        // never reach `rewrite_generic_calls_in_stmt/expr` at all. `use_counter`
        // below is the ordinary (non-generic) twin of `helper`'s body,
        // existing purely so Pass 2 actually walks this exact node shape
        // -- `helper` itself still exists to keep `monomorphize`'s early
        // "no generics" return from short-circuiting the whole pass, and to
        // keep exercising `reject_generic_calls_in_stmt/expr`'s own walk of
        // a generic function's body.
        // A standalone `Counter` class, not `point_module`'s shared
        // `Point` fixture: `add` takes a real `n: int` argument, needed to
        // exercise `reject_generic_calls_in_expr`'s/
        // `rewrite_generic_calls_in_expr`'s own `MethodCall` arm's
        // per-argument loop body at all -- `point_module`'s own `bump`
        // takes no arguments.
        let counter_ty = Ty::Instance(Box::new("Counter".to_string()));
        let counter_init = HirItem::Function {
            name: "Counter.__init__".to_string(),
            params: vec![("self".to_string(), counter_ty.clone())],
            return_ty: Ty::None,
            body: vec![
                HirStmt::AttrSet {
                    base: HirExpr::Name("self".to_string()),
                    attr: "n".to_string(),
                    value: HirExpr::IntLiteral(0),
                },
                HirStmt::Return(None),
            ],
        };
        let counter_add = HirItem::Function {
            name: "Counter.add".to_string(),
            params: vec![
                ("self".to_string(), counter_ty.clone()),
                ("n".to_string(), Ty::Int),
            ],
            return_ty: Ty::None,
            body: vec![
                HirStmt::AttrSet {
                    base: HirExpr::Name("self".to_string()),
                    attr: "n".to_string(),
                    value: HirExpr::Name("n".to_string()),
                },
                HirStmt::Return(None),
            ],
        };
        let helper = HirItem::Function {
            name: "helper".to_string(),
            params: vec![("x".to_string(), Ty::Param(Box::new("T".to_string())))],
            return_ty: Ty::Param(Box::new("T".to_string())),
            body: vec![
                HirStmt::Assign {
                    target: "c".to_string(),
                    value: HirExpr::Call {
                        callee: "Counter".to_string(),
                        args: vec![],
                    },
                },
                HirStmt::AttrSet {
                    base: HirExpr::Name("c".to_string()),
                    attr: "n".to_string(),
                    value: HirExpr::IntLiteral(1),
                },
                HirStmt::ExprStmt(HirExpr::MethodCall {
                    base: Box::new(HirExpr::Name("c".to_string())),
                    method: "add".to_string(),
                    args: vec![HirExpr::IntLiteral(5)],
                }),
                HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::AttrGet {
                        base: Box::new(HirExpr::Name("c".to_string())),
                        attr: "n".to_string(),
                    }],
                }),
                HirStmt::Return(Some(HirExpr::Name("x".to_string()))),
            ],
        };
        let use_counter = HirItem::Function {
            name: "use_counter".to_string(),
            params: vec![("c".to_string(), counter_ty.clone())],
            return_ty: Ty::None,
            body: vec![
                HirStmt::AttrSet {
                    base: HirExpr::Name("c".to_string()),
                    attr: "n".to_string(),
                    value: HirExpr::IntLiteral(2),
                },
                HirStmt::ExprStmt(HirExpr::MethodCall {
                    base: Box::new(HirExpr::Name("c".to_string())),
                    method: "add".to_string(),
                    args: vec![HirExpr::IntLiteral(5)],
                }),
                HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::AttrGet {
                        base: Box::new(HirExpr::Name("c".to_string())),
                        attr: "n".to_string(),
                    }],
                }),
                HirStmt::Return(None),
            ],
        };
        let mut hir = point_module(vec![
            counter_init,
            counter_add,
            helper,
            use_counter,
            top_level(HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::Call {
                    callee: "helper".to_string(),
                    args: vec![HirExpr::IntLiteral(5)],
                }],
            })),
        ]);
        hir.class_defs.push((
            "Counter".to_string(),
            HirClassDef {
                name: "Counter".to_string(),
                attrs: vec![("n".to_string(), Ty::Int)],
                methods: vec![
                    ("__init__".to_string(), "Counter.__init__".to_string()),
                    ("add".to_string(), "Counter.add".to_string()),
                ],
                type_param: None,
                properties: Vec::new(),
            },
        ));
        check_and_resolve(&hir).expect(
            "a well-typed generic function body using class instance attribute/method \
             access should check",
        );
    }

    #[test]
    fn reject_generic_calls_in_expr_propagates_a_method_call_s_base_and_argument_errors() {
        // Exercises `reject_generic_calls_in_expr`'s own `HirExpr::MethodCall`
        // arm's `?` on both `base`'s own rejection walk and its
        // per-argument loop's -- as opposed to
        // `a_generic_function_body_containing_attribute_and_method_access_type_checks`
        // above, which only exercises both succeeding. A self-recursive
        // call nested inside a `MethodCall`'s `base` (first case) or one of
        // its `args` (second case) is rejected by `reject_generic_calls_in_expr`'s
        // own `HirExpr::Call` arm and must propagate back up through
        // `MethodCall`'s own two recursive positions. Neither case needs a
        // real class -- `reject_generic_calls_in_expr` never resolves
        // `MethodCall`'s own `base`/`method` against any `Environment`,
        // only walks the expression tree structurally.
        let bad_base = HirItem::Function {
            name: "bad_base".to_string(),
            params: vec![("x".to_string(), Ty::Param(Box::new("T".to_string())))],
            return_ty: Ty::Param(Box::new("T".to_string())),
            body: vec![HirStmt::Return(Some(HirExpr::MethodCall {
                base: Box::new(HirExpr::Call {
                    callee: "bad_base".to_string(),
                    args: vec![HirExpr::Name("x".to_string())],
                }),
                method: "whatever".to_string(),
                args: vec![],
            }))],
        };
        let hir1 = HirModule {
            items: vec![bad_base],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        assert_eq!(check(&hir1).unwrap_err().code, "T0042");

        let bad_arg = HirItem::Function {
            name: "bad_arg".to_string(),
            params: vec![("x".to_string(), Ty::Param(Box::new("T".to_string())))],
            return_ty: Ty::Param(Box::new("T".to_string())),
            body: vec![HirStmt::Return(Some(HirExpr::MethodCall {
                base: Box::new(HirExpr::Name("x".to_string())),
                method: "whatever".to_string(),
                args: vec![HirExpr::Call {
                    callee: "bad_arg".to_string(),
                    args: vec![HirExpr::Name("x".to_string())],
                }],
            }))],
        };
        let hir2 = HirModule {
            items: vec![bad_arg],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        assert_eq!(check(&hir2).unwrap_err().code, "T0042");
    }

    // -- internal-consistency panics ----------------------------------------
    //
    // Every test below bypasses the normal `check`/`check_and_resolve`
    // entry points, building an inconsistent `Environment` by hand (a
    // `Ty::Instance` payload naming a class the `Environment` was never
    // told about, or a class whose `__init__`/method was never registered
    // as an ordinary function) -- exactly the "a class's declared shape and
    // its `Environment` disagree" scenario each of these functions' own doc
    // comments name as unreachable from any real `check`-validated program,
    // mirroring `pycc_mir`'s own established convention for the identical
    // kind of internal-consistency panic (see e.g. that crate's
    // `referencing_an_unbound_name_panics_with_an_internal_error` test).

    #[test]
    #[should_panic(expected = "class `Ghost` has no registered HirClassDef")]
    fn resolve_attr_get_panics_when_the_class_is_not_registered() {
        let env = crate::Environment::new();
        let _ = super::resolve_attr_get(&env, &Ty::Instance(Box::new("Ghost".to_string())), "x");
    }

    #[test]
    #[should_panic(expected = "was not registered as an ordinary function")]
    fn resolve_instantiation_panics_when_init_is_not_registered() {
        let mut env = crate::Environment::new();
        env.bind_class(
            "Ghost".to_string(),
            HirClassDef {
                name: "Ghost".to_string(),
                attrs: vec![],
                methods: vec![("__init__".to_string(), "Ghost.__init__".to_string())],
                type_param: None,
                properties: Vec::new(),
            },
        );
        let _ = super::resolve_instantiation(&env, "Ghost", &[]);
    }

    #[test]
    #[should_panic(expected = "was not registered as an ordinary function")]
    fn resolve_method_call_panics_when_the_method_is_not_registered() {
        let mut env = crate::Environment::new();
        env.bind_class(
            "Ghost".to_string(),
            HirClassDef {
                name: "Ghost".to_string(),
                attrs: vec![],
                methods: vec![("foo".to_string(), "Ghost.foo".to_string())],
                type_param: None,
                properties: Vec::new(),
            },
        );
        let _ = super::resolve_method_call(
            &env,
            &Ty::Instance(Box::new("Ghost".to_string())),
            "foo",
            &[],
        );
    }

    #[test]
    #[should_panic(expected = "was not registered as an ordinary function")]
    fn resolve_attr_get_panics_when_a_property_getter_is_not_registered() {
        // #377: a property's getter is in the class's own property table
        // but was never registered in `Environment::functions` -- the
        // "declared shape and Environment disagree" scenario
        // `resolve_attr_get`'s own doc comment names as unreachable from
        // any real `check`-validated program, mirroring
        // `resolve_instantiation_panics_when_init_is_not_registered` above.
        use pycc_hir::PropertyDef;
        let mut env = crate::Environment::new();
        env.bind_class(
            "Ghost".to_string(),
            HirClassDef {
                name: "Ghost".to_string(),
                attrs: vec![],
                methods: vec![("__init__".to_string(), "Ghost.__init__".to_string())],
                type_param: None,
                properties: vec![PropertyDef {
                    name: "x".to_string(),
                    getter: "Ghost.x".to_string(),
                    setter: None,
                }],
            },
        );
        let _ = super::resolve_attr_get(
            &env,
            &Ty::Instance(Box::new("Ghost".to_string())),
            "x",
        );
    }

    #[test]
    #[should_panic(expected = "was not registered as an ordinary function")]
    fn check_attr_set_panics_when_a_property_setter_is_not_registered() {
        // #377: a property's setter is in the class's own property table
        // but was never registered in `Environment::functions` -- the
        // "declared shape and Environment disagree" scenario
        // `check_attr_set`'s own doc comment names as unreachable from
        // any real `check`-validated program, mirroring
        // `resolve_method_call_panics_when_the_method_is_not_registered`
        // above.
        use pycc_hir::PropertyDef;
        let mut env = crate::Environment::new();
        env.bind_class(
            "Ghost".to_string(),
            HirClassDef {
                name: "Ghost".to_string(),
                attrs: vec![],
                methods: vec![("__init__".to_string(), "Ghost.__init__".to_string())],
                type_param: None,
                properties: vec![PropertyDef {
                    name: "x".to_string(),
                    getter: "Ghost.x".to_string(),
                    setter: Some("Ghost.x.setter".to_string()),
                }],
            },
        );
        // `base` must infer as a `Ghost` instance so `check_attr_set`
        // reaches the property branch; `value` must infer successfully so
        // the `?` on `infer_expr_in` (line 228) does not short-circuit
        // before the setter lookup panic.
        env.bind_function("Ghost.__init__".to_string(), vec![Ty::Instance(Box::new("Ghost".to_string()))], Ty::None);
        env.bind("b".to_string(), Ty::Instance(Box::new("Ghost".to_string())));
        let _ = super::check_attr_set(
            &env,
            &[],
            &HirExpr::Name("b".to_string()),
            "x",
            &HirExpr::IntLiteral(42),
        );
    }

    // -- @property type checking (#377) -------------------------------------

    /// Builds a `Box` class with a read-write `@property` `val` backed by
    /// the `_val` slot, plus `extra_items`/`extra_stmts` for each test's
    /// own exercise. The getter returns `self._val` (int); the setter
    /// accepts an `int` and stores it.
    fn property_module(extra_items: Vec<HirItem>) -> HirModule {
        use pycc_hir::PropertyDef;
        let self_ty = Ty::Instance(Box::new("Box".to_string()));
        let init = HirItem::Function {
            name: "Box.__init__".to_string(),
            params: vec![("self".to_string(), self_ty.clone())],
            return_ty: Ty::None,
            body: vec![
                HirStmt::AttrSet {
                    base: HirExpr::Name("self".to_string()),
                    attr: "_val".to_string(),
                    value: HirExpr::IntLiteral(0),
                },
                HirStmt::Return(None),
            ],
        };
        let getter = HirItem::Function {
            name: "Box.val".to_string(),
            params: vec![("self".to_string(), self_ty.clone())],
            return_ty: Ty::Int,
            body: vec![HirStmt::Return(Some(HirExpr::AttrGet {
                base: Box::new(HirExpr::Name("self".to_string())),
                attr: "_val".to_string(),
            }))],
        };
        let setter = HirItem::Function {
            name: "Box.val.setter".to_string(),
            params: vec![
                ("self".to_string(), self_ty.clone()),
                ("v".to_string(), Ty::Int),
            ],
            return_ty: Ty::None,
            body: vec![
                HirStmt::AttrSet {
                    base: HirExpr::Name("self".to_string()),
                    attr: "_val".to_string(),
                    value: HirExpr::Name("v".to_string()),
                },
                HirStmt::Return(None),
            ],
        };
        let mut items = vec![init, getter, setter];
        items.extend(extra_items);
        HirModule {
            items,
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: vec![(
                "Box".to_string(),
                HirClassDef {
                    name: "Box".to_string(),
                    attrs: vec![("_val".to_string(), Ty::Int)],
                    methods: vec![("__init__".to_string(), "Box.__init__".to_string())],
                    properties: vec![PropertyDef {
                        name: "val".to_string(),
                        getter: "Box.val".to_string(),
                        setter: Some("Box.val.setter".to_string()),
                    }],
                    type_param: None,
                },
            )],
        }
    }

    /// Like `property_module` but the property has no setter (read-only).
    fn read_only_property_module(extra_items: Vec<HirItem>) -> HirModule {
        use pycc_hir::PropertyDef;
        let self_ty = Ty::Instance(Box::new("Box".to_string()));
        let init = HirItem::Function {
            name: "Box.__init__".to_string(),
            params: vec![("self".to_string(), self_ty.clone())],
            return_ty: Ty::None,
            body: vec![
                HirStmt::AttrSet {
                    base: HirExpr::Name("self".to_string()),
                    attr: "_val".to_string(),
                    value: HirExpr::IntLiteral(0),
                },
                HirStmt::Return(None),
            ],
        };
        let getter = HirItem::Function {
            name: "Box.val".to_string(),
            params: vec![("self".to_string(), self_ty)],
            return_ty: Ty::Int,
            body: vec![HirStmt::Return(Some(HirExpr::AttrGet {
                base: Box::new(HirExpr::Name("self".to_string())),
                attr: "_val".to_string(),
            }))],
        };
        let mut items = vec![init, getter];
        items.extend(extra_items);
        HirModule {
            items,
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: vec![(
                "Box".to_string(),
                HirClassDef {
                    name: "Box".to_string(),
                    attrs: vec![("_val".to_string(), Ty::Int)],
                    methods: vec![("__init__".to_string(), "Box.__init__".to_string())],
                    properties: vec![PropertyDef {
                        name: "val".to_string(),
                        getter: "Box.val".to_string(),
                        setter: None,
                    }],
                    type_param: None,
                },
            )],
        }
    }

    #[test]
    fn a_property_getter_read_type_checks() {
        let hir = property_module(vec![
            top_level(HirStmt::Assign {
                target: "b".to_string(),
                value: HirExpr::Call {
                    callee: "Box".to_string(),
                    args: vec![],
                },
            }),
            top_level(HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::AttrGet {
                    base: Box::new(HirExpr::Name("b".to_string())),
                    attr: "val".to_string(),
                }],
            })),
        ]);
        check(&hir).expect("a property getter read should type-check");
    }

    #[test]
    fn a_property_setter_assignment_type_checks() {
        let hir = property_module(vec![
            top_level(HirStmt::Assign {
                target: "b".to_string(),
                value: HirExpr::Call {
                    callee: "Box".to_string(),
                    args: vec![],
                },
            }),
            top_level(HirStmt::AttrSet {
                base: HirExpr::Name("b".to_string()),
                attr: "val".to_string(),
                value: HirExpr::IntLiteral(42),
            }),
        ]);
        check(&hir).expect("a property setter assignment should type-check");
    }

    #[test]
    fn a_read_only_property_assignment_is_rejected() {
        let hir = read_only_property_module(vec![
            top_level(HirStmt::Assign {
                target: "b".to_string(),
                value: HirExpr::Call {
                    callee: "Box".to_string(),
                    args: vec![],
                },
            }),
            top_level(HirStmt::AttrSet {
                base: HirExpr::Name("b".to_string()),
                attr: "val".to_string(),
                value: HirExpr::IntLiteral(42),
            }),
        ]);
        let diagnostic = check(&hir).unwrap_err();
        assert_eq!(diagnostic.code, "T0044");
        assert!(
            diagnostic.message.contains("read-only"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn a_property_setter_type_mismatch_is_rejected() {
        let hir = property_module(vec![
            top_level(HirStmt::Assign {
                target: "b".to_string(),
                value: HirExpr::Call {
                    callee: "Box".to_string(),
                    args: vec![],
                },
            }),
            top_level(HirStmt::AttrSet {
                base: HirExpr::Name("b".to_string()),
                attr: "val".to_string(),
                value: HirExpr::StringLiteral("nope".to_string()),
            }),
        ]);
        let diagnostic = check(&hir).unwrap_err();
        assert_eq!(diagnostic.code, "T0021");
    }

    #[test]
    fn a_property_setter_assignment_with_an_ill_typed_value_propagates_the_value_error() {
        // Exercises `check_attr_set`'s `?` on `infer_expr_in` for the
        // *value* expression (line 228) -- distinct from
        // `a_property_setter_type_mismatch_is_rejected` above, where the
        // value infers successfully (`Ty::Str`) and the rejection happens
        // later at the `is_assignable` check (line 246). Here the value is
        // an undefined name, so `infer_expr_in` itself returns `Err`
        // before the setter's parameter type is ever consulted.
        let hir = property_module(vec![
            top_level(HirStmt::Assign {
                target: "b".to_string(),
                value: HirExpr::Call {
                    callee: "Box".to_string(),
                    args: vec![],
                },
            }),
            top_level(HirStmt::AttrSet {
                base: HirExpr::Name("b".to_string()),
                attr: "val".to_string(),
                value: HirExpr::Name("undefined_name".to_string()),
            }),
        ]);
        let diagnostic = check(&hir).unwrap_err();
        assert_eq!(diagnostic.code, "T0021");
        assert!(
            diagnostic.message.contains("undefined_name"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn a_property_getter_read_inside_a_method_body_type_checks() {
        // A method that reads the property via `self.val` -- exercises
        // `resolve_attr_get`'s property check from within a function body
        // (pass 3), not just top-level (pass 2).
        let self_ty = Ty::Instance(Box::new("Box".to_string()));
        let reader = HirItem::Function {
            name: "Box.read_val".to_string(),
            params: vec![("self".to_string(), self_ty)],
            return_ty: Ty::Int,
            body: vec![HirStmt::Return(Some(HirExpr::AttrGet {
                base: Box::new(HirExpr::Name("self".to_string())),
                attr: "val".to_string(),
            }))],
        };
        let mut hir = property_module(vec![]);
        // Add the reader method to items and to the class's method table.
        // `.expect(...)`, not `if let Some(...)` -- the latter's implicit
        // else (the no-match arm) is its own hand-written region, never
        // executed because `property_module` always defines `Box` -- this
        // crate's own established coverage-gate convention (see
        // `lower_ok`'s own doc comment in `pycc_hir::class::tests`) is
        // `.expect()`, whose panic path lives in libcore, outside this
        // crate's instrumented regions.
        hir.items.push(reader);
        let (_, cd) = hir
            .class_defs
            .iter_mut()
            .find(|(n, _)| n == "Box")
            .expect("property_module always defines Box");
        cd.methods.push(("read_val".to_string(), "Box.read_val".to_string()));
        hir.items.push(top_level(HirStmt::Assign {
            target: "b".to_string(),
            value: HirExpr::Call {
                callee: "Box".to_string(),
                args: vec![],
            },
        }));
        hir.items.push(top_level(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::MethodCall {
                base: Box::new(HirExpr::Name("b".to_string())),
                method: "read_val".to_string(),
                args: vec![],
            }],
        })));
        check(&hir).expect("a property read inside a method body should type-check");
    }

    #[test]
    fn a_property_setter_assignment_inside_a_method_body_type_checks() {
        // A method that writes the property via `self.val = v` -- exercises
        // `check_attr_set`'s property check from within a function body.
        let self_ty = Ty::Instance(Box::new("Box".to_string()));
        let writer = HirItem::Function {
            name: "Box.write_val".to_string(),
            params: vec![
                ("self".to_string(), self_ty),
                ("v".to_string(), Ty::Int),
            ],
            return_ty: Ty::None,
            body: vec![
                HirStmt::AttrSet {
                    base: HirExpr::Name("self".to_string()),
                    attr: "val".to_string(),
                    value: HirExpr::Name("v".to_string()),
                },
                HirStmt::Return(None),
            ],
        };
        let mut hir = property_module(vec![]);
        hir.items.push(writer);
        // `.expect(...)`, not `if let Some(...)` -- see the sibling test
        // `a_property_getter_read_inside_a_method_body_type_checks` above
        // for the coverage-gate rationale.
        let (_, cd) = hir
            .class_defs
            .iter_mut()
            .find(|(n, _)| n == "Box")
            .expect("property_module always defines Box");
        cd.methods
            .push(("write_val".to_string(), "Box.write_val".to_string()));
        hir.items.push(top_level(HirStmt::Assign {
            target: "b".to_string(),
            value: HirExpr::Call {
                callee: "Box".to_string(),
                args: vec![],
            },
        }));
        hir.items.push(top_level(HirStmt::ExprStmt(HirExpr::MethodCall {
            base: Box::new(HirExpr::Name("b".to_string())),
            method: "write_val".to_string(),
            args: vec![HirExpr::IntLiteral(99)],
        })));
        check(&hir).expect("a property write inside a method body should type-check");
    }

    #[test]
    fn a_property_getter_read_resolves_through_check_and_resolve() {
        // Exercises the full `check_and_resolve` → MIR pipeline for a
        // property getter read, ensuring the MIR lowering's property
        // rewrite (AttrGet → Call) produces valid MIR.
        let hir = property_module(vec![
            top_level(HirStmt::Assign {
                target: "b".to_string(),
                value: HirExpr::Call {
                    callee: "Box".to_string(),
                    args: vec![],
                },
            }),
            top_level(HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::AttrGet {
                    base: Box::new(HirExpr::Name("b".to_string())),
                    attr: "val".to_string(),
                }],
            })),
        ]);
        let resolved = check_and_resolve(&hir).expect("check_and_resolve should succeed");
        // Build MIR from the resolved HIR -- this exercises the MIR
        // lowering's property rewrite (AttrGet → MirExpr::Call).
        let _mir = pycc_mir::build(&resolved);
    }

    #[test]
    fn a_property_setter_assignment_resolves_through_check_and_resolve() {
        // Exercises the full `check_and_resolve` → MIR pipeline for a
        // property setter assignment, ensuring the MIR lowering's property
        // rewrite (AttrSet → ExprStmt(Call)) produces valid MIR.
        let hir = property_module(vec![
            top_level(HirStmt::Assign {
                target: "b".to_string(),
                value: HirExpr::Call {
                    callee: "Box".to_string(),
                    args: vec![],
                },
            }),
            top_level(HirStmt::AttrSet {
                base: HirExpr::Name("b".to_string()),
                attr: "val".to_string(),
                value: HirExpr::IntLiteral(42),
            }),
        ]);
        let resolved = check_and_resolve(&hir).expect("check_and_resolve should succeed");
        let _mir = pycc_mir::build(&resolved);
    }
}
