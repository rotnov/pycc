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
use pycc_hir::{HirClassDef, HirExpr, HirModule, Ty, extract_class_names, is_builtin_type_name};

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
        format!(
            "cannot {action} on `{}`: it is not a class instance",
            ty.name()
        ),
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
/// confirms `class_name` is a real, registered class. #432: the `__init__`
/// is resolved via the MRO -- a derived class without its own `__init__`
/// inherits the base class's constructor. The MRO is ordered
/// most-derived-first, so the first `__init__` found is the one called.
pub(crate) fn resolve_instantiation(
    env: &Environment,
    class_name: &str,
    arg_tys: &[Ty],
) -> Result<Ty, Diagnostic> {
    let class_def = env.lookup_class(class_name).unwrap_or_else(|| {
        panic!(
            "pycc_types: internal error: class `{class_name}` was not registered -- \
             infer_expr_in should have checked lookup_class before calling this"
        )
    });
    // #432: walk the MRO to find the first class with an `__init__` method.
    let mangled = class_def
        .mro
        .iter()
        .find_map(|mro_class| {
            let mro_def = env.lookup_class(mro_class)?;
            if mro_def.methods.iter().any(|(mn, _)| mn == "__init__") {
                Some(format!("{mro_class}.__init__"))
            } else {
                None
            }
        })
        .unwrap_or_else(|| {
            panic!(
                "pycc_types: internal error: no `__init__` found in class `{class_name}`'s MRO -- \
             pycc_hir::lower_class should have rejected this before it reached pycc_types"
            )
        });
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
///
/// #432: walks the class's MRO (C3 linearization) in order, checking each
/// class's property table and attribute slots. The first class in the MRO
/// that declares the attribute (or a property of that name) wins, matching
/// CPython's own MRO-based attribute resolution.
pub(crate) fn resolve_attr_get(
    env: &Environment,
    base_ty: &Ty,
    attr: &str,
) -> Result<Ty, Diagnostic> {
    let Ty::Instance(class_name) = base_ty else {
        return Err(t0043_not_an_instance("read an attribute", base_ty));
    };
    let class_def = expect_class(env, class_name);
    // #432/#377: walk the MRO for property lookup first (matching CPython's
    // descriptor protocol precedence — a property descriptor intercepts
    // attribute access before `__dict__`), across ALL classes in the MRO,
    // then fall back to regular attribute slots. This matches the MIR
    // lowering's own properties-first-across-full-MRO logic exactly,
    // avoiding a type-checker/MIR disagreement when a derived class has a
    // regular attr with the same name as a base class property.
    for mro_class in &class_def.mro {
        let mro_def = expect_class(env, mro_class);
        if let Some(prop) = mro_def.properties.iter().find(|p| p.name == attr) {
            let (_, return_ty) = env.lookup_function(&prop.getter).unwrap_or_else(|| {
                panic!(
                    "pycc_types: internal error: property getter `{}` is in class `{mro_class}`'s \
                     own property table but was not registered as an ordinary function",
                    prop.getter
                )
            });
            return Ok(return_ty.clone());
        }
    }
    // No property matched in any class — now check regular attribute slots
    // by walking the MRO in order (most-derived first, so a re-declared
    // attr uses the most-derived type).
    for mro_class in &class_def.mro {
        let mro_def = expect_class(env, mro_class);
        if let Some((_, ty)) = mro_def.attrs.iter().find(|(name, _)| name == attr) {
            return Ok(ty.clone());
        }
    }
    Err(t0044_unknown_member("attribute", class_name, attr))
}

/// Resolves `base.method(args)` against `base_ty`, checking the call's
/// arguments against the method's own resolved signature (excluding
/// `self`, exactly like `resolve_instantiation` excludes it from a
/// constructor call) and returning the method's return type.
///
/// #432: walks the class's MRO (C3 linearization) in order, checking each
/// class's method table. The first class in the MRO that declares the
/// method wins, matching CPython's own MRO-based method resolution. A
/// subclass method shadows a base class method of the same name (the
/// subclass appears first in the MRO).
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
    // #432: walk the MRO in order. The first class that declares the
    // method wins.
    for mro_class in &class_def.mro {
        let mro_def = expect_class(env, mro_class);
        if let Some((_, mangled)) = mro_def.methods.iter().find(|(name, _)| name == method) {
            let (param_tys, return_ty) = env.lookup_function(mangled).unwrap_or_else(|| {
                panic!(
                    "pycc_types: internal error: `{mangled}` is in class `{mro_class}`'s own \
                     method table but was not registered as an ordinary function"
                )
            });
            let method_param_tys = &param_tys[1..]; // exclude `self`
            check_call_args(method, arg_tys, method_param_tys)?;
            return Ok(return_ty.clone());
        }
    }
    Err(t0044_unknown_member("method", class_name, method))
}

/// #436: Resolves a call to a `@staticmethod` or `@classmethod` through
/// the MRO. `class_name` is the class to start the MRO walk from (either
/// the class itself for `ClassName.method(args)`, or the instance's class
/// for `instance.method(args)`). `is_class_call` is `true` when the call
/// is made on the class name (`ClassName.method(args)`), `false` when on
/// an instance. Both static and class methods can be called on either.
///
/// For a static method, the full parameter list is checked (no `self`/`cls`
/// exclusion). For a class method, the first parameter (`cls`) is excluded
/// from the argument check, matching how `resolve_method_call` excludes
/// `self`.
pub(crate) fn resolve_static_or_class_method_call(
    env: &Environment,
    class_name: &str,
    method: &str,
    arg_tys: &[Ty],
) -> Result<Ty, Diagnostic> {
    let class_def = expect_class(env, class_name);
    // #436: walk the MRO in order, checking static_methods first, then
    // class_methods. The first class that declares the method wins.
    for mro_class in &class_def.mro {
        let mro_def = expect_class(env, mro_class);
        if let Some((_, mangled)) = mro_def
            .static_methods
            .iter()
            .find(|(name, _)| name == method)
        {
            let (param_tys, return_ty) = env.lookup_function(mangled).unwrap_or_else(|| {
                panic!(
                    "pycc_types: internal error: `{mangled}` is in class `{mro_class}`'s own \
                     static_methods table but was not registered as an ordinary function"
                )
            });
            check_call_args(method, arg_tys, param_tys)?;
            return Ok(return_ty.clone());
        }
    }
    for mro_class in &class_def.mro {
        let mro_def = expect_class(env, mro_class);
        if let Some((_, mangled)) = mro_def
            .class_methods
            .iter()
            .find(|(name, _)| name == method)
        {
            let (param_tys, return_ty) = env.lookup_function(mangled).unwrap_or_else(|| {
                panic!(
                    "pycc_types: internal error: `{mangled}` is in class `{mro_class}`'s own \
                     class_methods table but was not registered as an ordinary function"
                )
            });
            let method_param_tys = &param_tys[1..]; // exclude `cls`
            check_call_args(method, arg_tys, method_param_tys)?;
            return Ok(return_ty.clone());
        }
    }
    Err(t0044_unknown_member("method", class_name, method))
}

/// #436: Checks whether `class_name` has a static or class method named
/// `method` in its MRO. Used by `infer_expr_in`'s `MethodCall` arm to
/// decide whether to intercept before the regular `resolve_method_call`
/// fallback (which requires a `Ty::Instance` base).
pub(crate) fn has_static_or_class_method(
    env: &Environment,
    class_name: &str,
    method: &str,
) -> bool {
    let Some(class_def) = env.lookup_class(class_name) else {
        return false;
    };
    class_def.mro.iter().any(|mro_class| {
        let Some(mro_def) = env.lookup_class(mro_class) else {
            return false;
        };
        mro_def
            .static_methods
            .iter()
            .any(|(name, _)| name == method)
            || mro_def.class_methods.iter().any(|(name, _)| name == method)
    })
}

/// #433: Resolves `super().attr` — an attribute read through zero-arg
/// `super()`. The resolution starts from the class *after* the current
/// class in the MRO (not the current class itself), matching CPython's own
/// `super().__getattribute__` semantics: `super()` skips the current
/// class's own entries and searches the rest of the MRO. The `self`
/// instance retains its actual (most-derived) type, so the slot index
/// computed downstream by `pycc_mir` is still the full MRO's flat layout
/// index — only the *resolution* (which class's declaration wins) is
/// affected by the `super()` skip.
///
/// Properties are checked before regular attribute slots, matching
/// `resolve_attr_get`'s own properties-first-across-full-MRO precedence.
pub(crate) fn resolve_super_attr_get(env: &Environment, attr: &str) -> Result<Ty, Diagnostic> {
    let current_class = env.current_class().unwrap();
    let class_def = expect_class(env, current_class);
    // Find the current class's position in its own MRO, then search
    // starting from the next position.
    let current_pos = class_def
        .mro
        .iter()
        .position(|c| c == current_class)
        .unwrap();
    let super_mro = &class_def.mro[current_pos + 1..];
    // Properties first (matching `resolve_attr_get`'s precedence).
    for mro_class in super_mro {
        let mro_def = expect_class(env, mro_class);
        if let Some(prop) = mro_def.properties.iter().find(|p| p.name == attr) {
            let (_, return_ty) = env.lookup_function(&prop.getter).unwrap();
            return Ok(return_ty.clone());
        }
    }
    // Regular attribute slots.
    for mro_class in super_mro {
        let mro_def = expect_class(env, mro_class);
        if let Some((_, ty)) = mro_def.attrs.iter().find(|(name, _)| name == attr) {
            // #433 review fix: if a class between the current class and the
            // declaring class (i.e. in the MRO before super_mro) redeclares
            // this attribute with a different type, the shared slot will
            // contain the redeclared value at runtime, not the base type's
            // value. Reject this to avoid a type-safety violation.
            for before_class in &class_def.mro[..current_pos + 1] {
                let before_def = expect_class(env, before_class);
                if let Some((_, before_ty)) = before_def.attrs.iter().find(|(n, _)| n == attr)
                    && before_ty != ty
                {
                    return Err(Diagnostic::error(
                        "T0021",
                        format!(
                            "super().{attr} has type `{}` in class `{mro_class}` but is \
                             redeclared as `{}` in class `{before_class}` — the shared \
                             attribute slot will contain the redeclared type at runtime, \
                             so super().{attr} would read the wrong type",
                            ty.name(),
                            before_ty.name(),
                        ),
                        Span::new(0, 0),
                    ));
                }
            }
            return Ok(ty.clone());
        }
    }
    Err(t0044_unknown_member("attribute", current_class, attr))
}

/// #433: Resolves `super().method(args)` — a method call through zero-arg
/// `super()`. The resolution starts from the class *after* the current
/// class in the MRO, matching CPython's own `super()` semantics. The
/// `self` instance (the most-derived object) is the implicit first
/// argument, but the method's *signature* is checked against the caller's
/// supplied arguments only (excluding `self`), exactly like
/// `resolve_method_call`.
pub(crate) fn resolve_super_method_call(
    env: &Environment,
    method: &str,
    arg_tys: &[Ty],
) -> Result<Ty, Diagnostic> {
    let current_class = env.current_class().unwrap();
    let class_def = expect_class(env, current_class);
    let current_pos = class_def
        .mro
        .iter()
        .position(|c| c == current_class)
        .unwrap();
    let super_mro = &class_def.mro[current_pos + 1..];
    for mro_class in super_mro {
        let mro_def = expect_class(env, mro_class);
        if let Some((_, mangled)) = mro_def.methods.iter().find(|(name, _)| name == method) {
            let (param_tys, return_ty) = env.lookup_function(mangled).unwrap();
            let method_param_tys = &param_tys[1..]; // exclude `self`
            check_call_args(method, arg_tys, method_param_tys)?;
            return Ok(return_ty.clone());
        }
    }
    Err(t0044_unknown_member("method", current_class, method))
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
///
/// #432: property lookup walks the MRO, so a property defined in a base
/// class is found when setting an attribute on a derived class instance.
pub(crate) fn check_attr_set(
    env: &Environment,
    local_names: &[&str],
    base: &HirExpr,
    attr: &str,
    value: &HirExpr,
) -> Result<(), Diagnostic> {
    let base_ty = infer_expr_in(env, local_names, base)?;
    // #432/#377: walk the MRO for property lookup first (matching
    // `resolve_attr_get`'s own properties-first-across-full-MRO logic and
    // the MIR lowering's own logic), across ALL classes in the MRO, then
    // fall back to regular attribute slots. A property setter has its own
    // parameter type (the value the setter accepts), which may differ from
    // the getter's return type -- so the value is checked against the
    // setter's parameter, not `resolve_attr_get`'s getter-return-type.
    if let Ty::Instance(class_name) = &base_ty {
        let class_def = expect_class(env, class_name);
        for mro_class in &class_def.mro {
            let mro_def = expect_class(env, mro_class);
            if let Some(prop) = mro_def.properties.iter().find(|p| p.name == attr) {
                let value_ty = infer_expr_in(env, local_names, value)?;
                let Some(setter_mangled) = &prop.setter else {
                    return Err(Diagnostic::error(
                        "T0044",
                        format!(
                            "property `{attr}` of class `{mro_class}` is read-only (has no setter)"
                        ),
                        Span::new(0, 0),
                    ));
                };
                let (param_tys, _) = env.lookup_function(setter_mangled).unwrap_or_else(|| {
                    panic!(
                        "pycc_types: internal error: property setter `{setter_mangled}` is in class \
                         `{mro_class}`'s own property table but was not registered as an ordinary \
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
    }
    // Regular attribute slot -- `resolve_attr_get` already walks the MRO.
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

// ---------------------------------------------------------------------------
// Issue #435: compile-time `isinstance`/`issubclass` type checking.
//
// pycc uses static dispatch (D-006) — every variable's runtime type is
// exactly its declared static type, so `isinstance`/`issubclass` can always
// be evaluated at compile time. These functions validate the call's
// arguments and return `Ty::Bool` (the result is computed at MIR lowering
// time, not here — the type checker only needs to confirm the call is
// well-formed and returns `bool`).
// ---------------------------------------------------------------------------

/// Validates a class name argument to `isinstance`/`issubclass`: it must be
/// either a registered user-defined class or one of the builtin scalar type
/// names (`int`, `str`, `float`, `bool`). Returns `Ok(())` if valid, or a
/// `T0001` diagnostic if the name is unknown.
fn validate_class_name(env: &Environment, name: &str) -> Result<(), Diagnostic> {
    if env.lookup_class(name).is_some() || is_builtin_type_name(name) {
        Ok(())
    } else {
        Err(Diagnostic::error(
            "T0001",
            format!("`{name}` is not a known class or builtin type"),
            Span::new(0, 0),
        ))
    }
}

/// #435: Type-checks `isinstance(obj, class_arg)` — validates the argument
/// count, infers the object's type, extracts and validates the class
/// argument(s), and returns `Ok(Ty::Bool)`. The compile-time boolean result
/// is computed at MIR lowering time (not here — the type checker only
/// confirms the call is well-formed).
pub(crate) fn check_isinstance(
    env: &Environment,
    local_names: &[&str],
    args: &[HirExpr],
) -> Result<Ty, Diagnostic> {
    if args.len() != 2 {
        return Err(Diagnostic::error(
            "T0021",
            format!(
                "`isinstance` expects exactly 2 arguments, got {}",
                args.len()
            ),
            Span::new(0, 0),
        )
        .with_help("pass exactly 2 arguments: the object and the class"));
    }
    // #435 review fix (P1): `isinstance` is a compile-time predicate in
    // pycc's static-dispatch model — the result is a `BoolLiteral` constant
    // computed from the operand's declared type. A side-effecting operand
    // (a function call or class instantiation) would have its effects
    // silently discarded, changing standard Python semantics. Reject such
    // operands with `C0001` rather than silently dropping the call.
    if let HirExpr::Call { .. } = &args[0] {
        return Err(Diagnostic::error(
            "C0001",
            "`isinstance` is a compile-time predicate in pycc and cannot evaluate a \
             call expression as its first argument (side effects would be lost)",
            Span::new(0, 0),
        )
        .with_help(
            "assign the call result to a variable first, then pass the variable to \
             `isinstance`",
        ));
    }
    // Infer the object's type normally.
    let obj_ty = infer_expr_in(env, local_names, &args[0])?;
    // Extract class names from the second argument (do NOT infer it as a
    // regular expression — class names are not value bindings).
    let class_names = extract_class_names(&args[1]).map_err(|_| {
        Diagnostic::error(
            "T0021",
            "`isinstance`'s second argument must be a class name or a tuple of class names",
            Span::new(0, 0),
        )
        .with_help("pass a class name (e.g. `int`) or a tuple of class names (e.g. `(int, str)`)")
    })?;
    // Validate each class name.
    for name in &class_names {
        validate_class_name(env, name)?;
    }
    // The result is always `Ty::Bool`. The actual compile-time boolean value
    // is computed by `eval_isinstance_single` at MIR lowering time.
    // (We could compute it here too, but the type checker's job is just
    // validation — the MIR computes the constant.)
    let _ = obj_ty; // obj_ty is validated; the MIR uses it to compute the result
    Ok(Ty::Bool)
}

/// #435: Type-checks `issubclass(cls_arg, class_arg)` — validates the
/// argument count, extracts and validates both class arguments, and returns
/// `Ok(Ty::Bool)`. Neither argument is inferred as a regular expression
/// (both are class references, not values).
pub(crate) fn check_issubclass(env: &Environment, args: &[HirExpr]) -> Result<Ty, Diagnostic> {
    if args.len() != 2 {
        return Err(Diagnostic::error(
            "T0021",
            format!(
                "`issubclass` expects exactly 2 arguments, got {}",
                args.len()
            ),
            Span::new(0, 0),
        )
        .with_help("pass exactly 2 arguments: the class and the target class"));
    }
    // The first argument must be a bare class name (not a tuple).
    let cls_name = match &args[0] {
        HirExpr::Name(name) => name.clone(),
        _ => {
            return Err(Diagnostic::error(
                "T0021",
                "`issubclass`'s first argument must be a class name",
                Span::new(0, 0),
            )
            .with_help("pass a bare class name (e.g. `int` or `MyClass`)"));
        }
    };
    // Extract class names from the second argument.
    let target_names = extract_class_names(&args[1]).map_err(|_| {
        Diagnostic::error(
            "T0021",
            "`issubclass`'s second argument must be a class name or a tuple of class names",
            Span::new(0, 0),
        )
        .with_help("pass a class name (e.g. `int`) or a tuple of class names (e.g. `(int, str)`)")
    })?;
    // Validate the first argument's class name.
    validate_class_name(env, &cls_name)?;
    // Validate each target class name.
    for name in &target_names {
        validate_class_name(env, name)?;
    }
    Ok(Ty::Bool)
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
                    bases: Vec::new(),
                    mro: vec!["Point".to_string()],
                    attrs: vec![("x".to_string(), Ty::Int), ("y".to_string(), Ty::Int)],
                    methods: vec![
                        ("__init__".to_string(), "Point.__init__".to_string()),
                        ("bump".to_string(), "Point.bump".to_string()),
                    ],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    enum_members: Vec::new(),
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
        check(&hir).expect(
            "a well-typed class instantiation/method-call/attribute-read program should check",
        );
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
            args: vec![
                HirExpr::IntLiteral(1),
                HirExpr::StringLiteral("y".to_string()),
            ],
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
                    bases: Vec::new(),
                    mro: vec!["Point".to_string()],
                    attrs: vec![("x".to_string(), Ty::Int)],
                    methods: vec![
                        ("__init__".to_string(), "Point.__init__".to_string()),
                        ("bump".to_string(), "Point.bump".to_string()),
                        ("_touch".to_string(), "Point._touch".to_string()),
                    ],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    enum_members: Vec::new(),
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
                    bases: Vec::new(),
                    mro: vec!["Point".to_string()],
                    attrs: vec![("x".to_string(), Ty::Int)],
                    methods: vec![("__init__".to_string(), "Point.__init__".to_string())],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    enum_members: Vec::new(),
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
                bases: Vec::new(),
                mro: vec!["Counter".to_string()],
                attrs: vec![("n".to_string(), Ty::Int)],
                methods: vec![
                    ("__init__".to_string(), "Counter.__init__".to_string()),
                    ("add".to_string(), "Counter.add".to_string()),
                ],
                type_param: None,
                properties: Vec::new(),
                static_methods: Vec::new(),
                class_methods: Vec::new(),
                enum_members: Vec::new(),
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
                bases: Vec::new(),
                mro: vec!["Ghost".to_string()],
                attrs: vec![],
                methods: vec![("__init__".to_string(), "Ghost.__init__".to_string())],
                type_param: None,
                properties: Vec::new(),
                static_methods: Vec::new(),
                class_methods: Vec::new(),
                enum_members: Vec::new(),
            },
        );
        let _ = super::resolve_instantiation(&env, "Ghost", &[]);
    }

    #[test]
    #[should_panic(expected = "class `Ghost` was not registered")]
    fn resolve_instantiation_panics_when_the_class_is_not_registered() {
        // #432: `resolve_instantiation` is only called after
        // `infer_expr_in`'s own `lookup_class` confirms the class exists,
        // so reaching it with an unregistered class name is an internal
        // error. This test bypasses the normal entry point and calls
        // `resolve_instantiation` directly with a bare `Environment`.
        let env = crate::Environment::new();
        let _ = super::resolve_instantiation(&env, "Ghost", &[]);
    }

    #[test]
    #[should_panic(expected = "no `__init__` found in class `Ghost`'s MRO")]
    fn resolve_instantiation_panics_when_no_init_is_in_the_mro() {
        // #432: `lower_class` rejects a class with no `__init__` anywhere
        // in its MRO before it reaches `pycc_types`, so this panic is an
        // internal error. This test bypasses the normal entry point and
        // binds a class whose MRO contains no `__init__` method. The MRO
        // also includes `Phantom` (not registered), exercising the `?`
        // arm of the `find_map` closure -- `Ghost` is found but has no
        // `__init__`, then `Phantom` is not found, so `find_map` returns
        // `None` and the `unwrap_or_else` panic fires.
        let mut env = crate::Environment::new();
        env.bind_class(
            "Ghost".to_string(),
            HirClassDef {
                name: "Ghost".to_string(),
                bases: vec!["Phantom".to_string()],
                mro: vec!["Ghost".to_string(), "Phantom".to_string()],
                attrs: vec![],
                methods: vec![("f".to_string(), "Ghost.f".to_string())],
                type_param: None,
                properties: Vec::new(),
                static_methods: Vec::new(),
                class_methods: Vec::new(),
                enum_members: Vec::new(),
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
                bases: Vec::new(),
                mro: vec!["Ghost".to_string()],
                attrs: vec![],
                methods: vec![("foo".to_string(), "Ghost.foo".to_string())],
                type_param: None,
                properties: Vec::new(),
                static_methods: Vec::new(),
                class_methods: Vec::new(),
                enum_members: Vec::new(),
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
                bases: Vec::new(),
                mro: vec!["Ghost".to_string()],
                attrs: vec![],
                methods: vec![("__init__".to_string(), "Ghost.__init__".to_string())],
                type_param: None,
                properties: vec![PropertyDef {
                    name: "x".to_string(),
                    getter: "Ghost.x".to_string(),
                    setter: None,
                }],
                static_methods: Vec::new(),
                class_methods: Vec::new(),
                enum_members: Vec::new(),
            },
        );
        let _ = super::resolve_attr_get(&env, &Ty::Instance(Box::new("Ghost".to_string())), "x");
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
                bases: Vec::new(),
                mro: vec!["Ghost".to_string()],
                attrs: vec![],
                methods: vec![("__init__".to_string(), "Ghost.__init__".to_string())],
                type_param: None,
                properties: vec![PropertyDef {
                    name: "x".to_string(),
                    getter: "Ghost.x".to_string(),
                    setter: Some("Ghost.x.setter".to_string()),
                }],
                static_methods: Vec::new(),
                class_methods: Vec::new(),
                enum_members: Vec::new(),
            },
        );
        // `base` must infer as a `Ghost` instance so `check_attr_set`
        // reaches the property branch; `value` must infer successfully so
        // the `?` on `infer_expr_in` (line 228) does not short-circuit
        // before the setter lookup panic.
        env.bind_function(
            "Ghost.__init__".to_string(),
            vec![Ty::Instance(Box::new("Ghost".to_string()))],
            Ty::None,
        );
        env.bind("b".to_string(), Ty::Instance(Box::new("Ghost".to_string())));
        let _ = super::check_attr_set(
            &env,
            &[],
            &HirExpr::Name("b".to_string()),
            "x",
            &HirExpr::IntLiteral(42),
        );
    }

    // -- #433: super() type-checking tests ----------------------------------

    /// Builds an `Environment` with `current_class` set to `"B"`, a base
    /// class `"A"` in the MRO, and `extra_setup` to customize the class
    /// definitions and function registrations per test.
    fn super_env(extra_setup: impl FnOnce(&mut crate::Environment)) -> crate::Environment {
        let mut env = crate::Environment::new();
        env.current_class = Some("B".to_string());
        extra_setup(&mut env);
        env
    }

    #[test]
    fn resolve_super_attr_get_returns_property_type() {
        use pycc_hir::PropertyDef;
        let env = super_env(|env| {
            env.bind_class(
                "A".to_string(),
                HirClassDef {
                    name: "A".to_string(),
                    bases: vec![],
                    mro: vec!["A".to_string()],
                    attrs: vec![("_val".to_string(), Ty::Int)],
                    methods: vec![("__init__".to_string(), "A.__init__".to_string())],
                    type_param: None,
                    properties: vec![PropertyDef {
                        name: "val".to_string(),
                        getter: "A.val".to_string(),
                        setter: None,
                    }],
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    enum_members: Vec::new(),
                },
            );
            env.bind_class(
                "B".to_string(),
                HirClassDef {
                    name: "B".to_string(),
                    bases: vec!["A".to_string()],
                    mro: vec!["B".to_string(), "A".to_string()],
                    attrs: vec![],
                    methods: vec![("__init__".to_string(), "B.__init__".to_string())],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    enum_members: Vec::new(),
                },
            );
            env.bind_function(
                "A.val".to_string(),
                vec![Ty::Instance(Box::new("A".to_string()))],
                Ty::Int,
            );
        });
        assert_eq!(super::resolve_super_attr_get(&env, "val"), Ok(Ty::Int));
    }

    #[test]
    fn resolve_super_attr_get_returns_attr_type() {
        let env = super_env(|env| {
            env.bind_class(
                "A".to_string(),
                HirClassDef {
                    name: "A".to_string(),
                    bases: vec![],
                    mro: vec!["A".to_string()],
                    attrs: vec![("x".to_string(), Ty::Int)],
                    methods: vec![("__init__".to_string(), "A.__init__".to_string())],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    enum_members: Vec::new(),
                },
            );
            env.bind_class(
                "B".to_string(),
                HirClassDef {
                    name: "B".to_string(),
                    bases: vec!["A".to_string()],
                    mro: vec!["B".to_string(), "A".to_string()],
                    attrs: vec![],
                    methods: vec![("__init__".to_string(), "B.__init__".to_string())],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    enum_members: Vec::new(),
                },
            );
        });
        assert_eq!(super::resolve_super_attr_get(&env, "x"), Ok(Ty::Int));
    }

    #[test]
    fn resolve_super_attr_get_rejects_redeclared_type_mismatch() {
        let env = super_env(|env| {
            env.bind_class(
                "A".to_string(),
                HirClassDef {
                    name: "A".to_string(),
                    bases: vec![],
                    mro: vec!["A".to_string()],
                    attrs: vec![("x".to_string(), Ty::Int)],
                    methods: vec![("__init__".to_string(), "A.__init__".to_string())],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    enum_members: Vec::new(),
                },
            );
            env.bind_class(
                "B".to_string(),
                HirClassDef {
                    name: "B".to_string(),
                    bases: vec!["A".to_string()],
                    mro: vec!["B".to_string(), "A".to_string()],
                    attrs: vec![("x".to_string(), Ty::Str)],
                    methods: vec![("__init__".to_string(), "B.__init__".to_string())],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    enum_members: Vec::new(),
                },
            );
        });
        let result = super::resolve_super_attr_get(&env, "x");
        assert!(result.is_err());
        let diag = result.unwrap_err();
        assert_eq!(diag.code, "T0021");
        assert!(diag.message.contains("redeclared"));
    }

    #[test]
    fn resolve_super_attr_get_allows_same_type_redeclaration() {
        let env = super_env(|env| {
            env.bind_class(
                "A".to_string(),
                HirClassDef {
                    name: "A".to_string(),
                    bases: vec![],
                    mro: vec!["A".to_string()],
                    attrs: vec![("x".to_string(), Ty::Int)],
                    methods: vec![("__init__".to_string(), "A.__init__".to_string())],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    enum_members: Vec::new(),
                },
            );
            env.bind_class(
                "B".to_string(),
                HirClassDef {
                    name: "B".to_string(),
                    bases: vec!["A".to_string()],
                    mro: vec!["B".to_string(), "A".to_string()],
                    attrs: vec![("x".to_string(), Ty::Int)],
                    methods: vec![("__init__".to_string(), "B.__init__".to_string())],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    enum_members: Vec::new(),
                },
            );
        });
        assert_eq!(super::resolve_super_attr_get(&env, "x"), Ok(Ty::Int));
    }

    #[test]
    fn resolve_super_method_call_returns_return_type() {
        let env = super_env(|env| {
            env.bind_class(
                "A".to_string(),
                HirClassDef {
                    name: "A".to_string(),
                    bases: vec![],
                    mro: vec!["A".to_string()],
                    attrs: vec![],
                    methods: vec![("greet".to_string(), "A.greet".to_string())],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    enum_members: Vec::new(),
                },
            );
            env.bind_class(
                "B".to_string(),
                HirClassDef {
                    name: "B".to_string(),
                    bases: vec!["A".to_string()],
                    mro: vec!["B".to_string(), "A".to_string()],
                    attrs: vec![],
                    methods: vec![("__init__".to_string(), "B.__init__".to_string())],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    enum_members: Vec::new(),
                },
            );
            env.bind_function(
                "A.greet".to_string(),
                vec![Ty::Instance(Box::new("A".to_string()))],
                Ty::Int,
            );
        });
        assert_eq!(
            super::resolve_super_method_call(&env, "greet", &[]),
            Ok(Ty::Int)
        );
    }

    #[test]
    fn resolve_super_method_call_returns_t0044_for_unknown_method() {
        let env = super_env(|env| {
            env.bind_class(
                "A".to_string(),
                HirClassDef {
                    name: "A".to_string(),
                    bases: vec![],
                    mro: vec!["A".to_string()],
                    attrs: vec![],
                    methods: vec![("__init__".to_string(), "A.__init__".to_string())],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    enum_members: Vec::new(),
                },
            );
            env.bind_class(
                "B".to_string(),
                HirClassDef {
                    name: "B".to_string(),
                    bases: vec!["A".to_string()],
                    mro: vec!["B".to_string(), "A".to_string()],
                    attrs: vec![],
                    methods: vec![("__init__".to_string(), "B.__init__".to_string())],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    enum_members: Vec::new(),
                },
            );
        });
        let err = super::resolve_super_method_call(&env, "nonexistent", &[]).unwrap_err();
        assert_eq!(err.code, "T0044");
    }

    #[test]
    fn resolve_super_attr_get_returns_t0044_for_unknown_attr() {
        let env = super_env(|env| {
            env.bind_class(
                "A".to_string(),
                HirClassDef {
                    name: "A".to_string(),
                    bases: vec![],
                    mro: vec!["A".to_string()],
                    attrs: vec![("x".to_string(), Ty::Int)],
                    methods: vec![("__init__".to_string(), "A.__init__".to_string())],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    enum_members: Vec::new(),
                },
            );
            env.bind_class(
                "B".to_string(),
                HirClassDef {
                    name: "B".to_string(),
                    bases: vec!["A".to_string()],
                    mro: vec!["B".to_string(), "A".to_string()],
                    attrs: vec![],
                    methods: vec![("__init__".to_string(), "B.__init__".to_string())],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    enum_members: Vec::new(),
                },
            );
        });
        let err = super::resolve_super_attr_get(&env, "nonexistent").unwrap_err();
        assert_eq!(err.code, "T0044");
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
                    bases: Vec::new(),
                    mro: vec!["Box".to_string()],
                    attrs: vec![("_val".to_string(), Ty::Int)],
                    methods: vec![("__init__".to_string(), "Box.__init__".to_string())],
                    properties: vec![PropertyDef {
                        name: "val".to_string(),
                        getter: "Box.val".to_string(),
                        setter: Some("Box.val.setter".to_string()),
                    }],
                    type_param: None,
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    enum_members: Vec::new(),
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
                    bases: Vec::new(),
                    mro: vec!["Box".to_string()],
                    attrs: vec![("_val".to_string(), Ty::Int)],
                    methods: vec![("__init__".to_string(), "Box.__init__".to_string())],
                    properties: vec![PropertyDef {
                        name: "val".to_string(),
                        getter: "Box.val".to_string(),
                        setter: None,
                    }],
                    type_param: None,
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    enum_members: Vec::new(),
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
        cd.methods
            .push(("read_val".to_string(), "Box.read_val".to_string()));
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
            params: vec![("self".to_string(), self_ty), ("v".to_string(), Ty::Int)],
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
        hir.items
            .push(top_level(HirStmt::ExprStmt(HirExpr::MethodCall {
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

    // -- #432: MRO-aware type checking --------------------------------------

    /// Builds an `Animal` base class with a `name` attribute and a `speak`
    /// method, plus a `Dog` derived class with its own `speak` override,
    /// plus `extra_items` for each test's own exercise.
    fn inheritance_module(extra_items: Vec<HirItem>) -> HirModule {
        let animal_ty = Ty::Instance(Box::new("Animal".to_string()));
        let dog_ty = Ty::Instance(Box::new("Dog".to_string()));
        let animal_init = HirItem::Function {
            name: "Animal.__init__".to_string(),
            params: vec![
                ("self".to_string(), animal_ty.clone()),
                ("name".to_string(), Ty::Str),
            ],
            return_ty: Ty::None,
            body: vec![
                HirStmt::AttrSet {
                    base: HirExpr::Name("self".to_string()),
                    attr: "name".to_string(),
                    value: HirExpr::Name("name".to_string()),
                },
                HirStmt::Return(None),
            ],
        };
        let animal_speak = HirItem::Function {
            name: "Animal.speak".to_string(),
            params: vec![("self".to_string(), animal_ty.clone())],
            return_ty: Ty::Str,
            body: vec![HirStmt::Return(Some(HirExpr::StringLiteral(
                "...".to_string(),
            )))],
        };
        let dog_init = HirItem::Function {
            name: "Dog.__init__".to_string(),
            params: vec![
                ("self".to_string(), dog_ty.clone()),
                ("name".to_string(), Ty::Str),
            ],
            return_ty: Ty::None,
            body: vec![
                HirStmt::AttrSet {
                    base: HirExpr::Name("self".to_string()),
                    attr: "name".to_string(),
                    value: HirExpr::Name("name".to_string()),
                },
                HirStmt::Return(None),
            ],
        };
        let dog_speak = HirItem::Function {
            name: "Dog.speak".to_string(),
            params: vec![("self".to_string(), dog_ty.clone())],
            return_ty: Ty::Str,
            body: vec![HirStmt::Return(Some(HirExpr::StringLiteral(
                "Woof".to_string(),
            )))],
        };
        let mut items = vec![animal_init, animal_speak, dog_init, dog_speak];
        items.extend(extra_items);
        HirModule {
            items,
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: vec![
                (
                    "Animal".to_string(),
                    HirClassDef {
                        name: "Animal".to_string(),
                        bases: Vec::new(),
                        mro: vec!["Animal".to_string()],
                        attrs: vec![("name".to_string(), Ty::Str)],
                        methods: vec![
                            ("__init__".to_string(), "Animal.__init__".to_string()),
                            ("speak".to_string(), "Animal.speak".to_string()),
                        ],
                        type_param: None,
                        properties: Vec::new(),
                        static_methods: Vec::new(),
                        class_methods: Vec::new(),
                        enum_members: Vec::new(),
                    },
                ),
                (
                    "Dog".to_string(),
                    HirClassDef {
                        name: "Dog".to_string(),
                        bases: vec!["Animal".to_string()],
                        mro: vec!["Dog".to_string(), "Animal".to_string()],
                        attrs: vec![("name".to_string(), Ty::Str)],
                        methods: vec![
                            ("__init__".to_string(), "Dog.__init__".to_string()),
                            ("speak".to_string(), "Dog.speak".to_string()),
                        ],
                        type_param: None,
                        properties: Vec::new(),
                        static_methods: Vec::new(),
                        class_methods: Vec::new(),
                        enum_members: Vec::new(),
                    },
                ),
            ],
        }
    }

    #[test]
    fn derived_class_inherits_base_attribute_through_mro() {
        // `d.name` on a `Dog` instance resolves `name` through the MRO --
        // `Dog` declares it, so it's found on the first MRO entry.
        let hir = inheritance_module(vec![
            top_level(HirStmt::Assign {
                target: "d".to_string(),
                value: HirExpr::Call {
                    callee: "Dog".to_string(),
                    args: vec![HirExpr::StringLiteral("Rex".to_string())],
                },
            }),
            top_level(HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::AttrGet {
                    base: Box::new(HirExpr::Name("d".to_string())),
                    attr: "name".to_string(),
                }],
            })),
        ]);
        check(&hir).expect("inherited attribute read should type-check");
    }

    #[test]
    fn derived_class_method_call_resolves_through_mro() {
        // `d.speak()` on a `Dog` instance resolves to `Dog.speak` (the
        // override), not `Animal.speak`.
        let hir = inheritance_module(vec![
            top_level(HirStmt::Assign {
                target: "d".to_string(),
                value: HirExpr::Call {
                    callee: "Dog".to_string(),
                    args: vec![HirExpr::StringLiteral("Rex".to_string())],
                },
            }),
            top_level(HirStmt::ExprStmt(HirExpr::MethodCall {
                base: Box::new(HirExpr::Name("d".to_string())),
                method: "speak".to_string(),
                args: vec![],
            })),
        ]);
        check(&hir).expect("method call on derived class should type-check");
    }

    #[test]
    fn derived_class_instantiation_with_inherited_init_type_checks() {
        // A `Dog` instance is created with `(name: str)` -- the `__init__`
        // is `Dog.__init__`, which takes `(self, name: str)`.
        let hir = inheritance_module(vec![top_level(HirStmt::Assign {
            target: "d".to_string(),
            value: HirExpr::Call {
                callee: "Dog".to_string(),
                args: vec![HirExpr::StringLiteral("Rex".to_string())],
            },
        })]);
        check(&hir).expect("derived class instantiation should type-check");
    }

    #[test]
    fn derived_class_instantiation_with_wrong_arg_type_is_rejected() {
        let hir = inheritance_module(vec![top_level(HirStmt::Assign {
            target: "d".to_string(),
            value: HirExpr::Call {
                callee: "Dog".to_string(),
                args: vec![HirExpr::IntLiteral(42)],
            },
        })]);
        assert_eq!(check(&hir).unwrap_err().code, "T0021");
    }

    /// Builds a `Base` class with `__init__` and a `Derived` class with no
    /// `__init__` of its own (inheriting `Base.__init__`).
    fn inherited_init_module(extra_items: Vec<HirItem>) -> HirModule {
        let base_ty = Ty::Instance(Box::new("Base".to_string()));
        let base_init = HirItem::Function {
            name: "Base.__init__".to_string(),
            params: vec![
                ("self".to_string(), base_ty.clone()),
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
        let mut items = vec![base_init];
        items.extend(extra_items);
        HirModule {
            items,
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: vec![
                (
                    "Base".to_string(),
                    HirClassDef {
                        name: "Base".to_string(),
                        bases: Vec::new(),
                        mro: vec!["Base".to_string()],
                        attrs: vec![("x".to_string(), Ty::Int)],
                        methods: vec![("__init__".to_string(), "Base.__init__".to_string())],
                        type_param: None,
                        properties: Vec::new(),
                        static_methods: Vec::new(),
                        class_methods: Vec::new(),
                        enum_members: Vec::new(),
                    },
                ),
                (
                    "Derived".to_string(),
                    HirClassDef {
                        name: "Derived".to_string(),
                        bases: vec!["Base".to_string()],
                        mro: vec!["Derived".to_string(), "Base".to_string()],
                        attrs: Vec::new(),
                        methods: Vec::new(),
                        type_param: None,
                        properties: Vec::new(),
                        static_methods: Vec::new(),
                        class_methods: Vec::new(),
                        enum_members: Vec::new(),
                    },
                ),
            ],
        }
    }

    #[test]
    fn derived_class_without_init_inherits_base_init_for_instantiation() {
        // `Derived(42)` should resolve to `Base.__init__` via the MRO and
        // type-check against its `(self, x: int)` parameter list.
        let hir = inherited_init_module(vec![
            top_level(HirStmt::Assign {
                target: "d".to_string(),
                value: HirExpr::Call {
                    callee: "Derived".to_string(),
                    args: vec![HirExpr::IntLiteral(42)],
                },
            }),
            top_level(HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::AttrGet {
                    base: Box::new(HirExpr::Name("d".to_string())),
                    attr: "x".to_string(),
                }],
            })),
        ]);
        check(&hir).expect("inherited __init__ instantiation should type-check");
    }

    #[test]
    fn derived_class_without_init_instantiation_wrong_arg_type_is_rejected() {
        let hir = inherited_init_module(vec![top_level(HirStmt::Assign {
            target: "d".to_string(),
            value: HirExpr::Call {
                callee: "Derived".to_string(),
                args: vec![HirExpr::StringLiteral("wrong".to_string())],
            },
        })]);
        assert_eq!(check(&hir).unwrap_err().code, "T0021");
    }

    #[test]
    fn inherited_attribute_read_resolves_through_check_and_resolve() {
        // Full pipeline: `check_and_resolve` → MIR build, exercising the
        // MIR lowering's MRO-aware attribute resolution.
        let hir = inherited_init_module(vec![
            top_level(HirStmt::Assign {
                target: "d".to_string(),
                value: HirExpr::Call {
                    callee: "Derived".to_string(),
                    args: vec![HirExpr::IntLiteral(42)],
                },
            }),
            top_level(HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::AttrGet {
                    base: Box::new(HirExpr::Name("d".to_string())),
                    attr: "x".to_string(),
                }],
            })),
        ]);
        let resolved = check_and_resolve(&hir).expect("check_and_resolve should succeed");
        let _mir = pycc_mir::build(&resolved);
    }

    #[test]
    fn inherited_method_call_resolves_through_check_and_resolve() {
        // Full pipeline for a method call that resolves to a base class
        // method via the MRO.
        let base_ty = Ty::Instance(Box::new("Base".to_string()));
        let base_init = HirItem::Function {
            name: "Base.__init__".to_string(),
            params: vec![("self".to_string(), base_ty.clone())],
            return_ty: Ty::None,
            body: vec![HirStmt::Return(None)],
        };
        let base_greet = HirItem::Function {
            name: "Base.greet".to_string(),
            params: vec![("self".to_string(), base_ty.clone())],
            return_ty: Ty::Str,
            body: vec![HirStmt::Return(Some(HirExpr::StringLiteral(
                "hi".to_string(),
            )))],
        };
        let hir = HirModule {
            items: vec![
                base_init,
                base_greet,
                top_level(HirStmt::Assign {
                    target: "d".to_string(),
                    value: HirExpr::Call {
                        callee: "Derived".to_string(),
                        args: vec![],
                    },
                }),
                top_level(HirStmt::ExprStmt(HirExpr::MethodCall {
                    base: Box::new(HirExpr::Name("d".to_string())),
                    method: "greet".to_string(),
                    args: vec![],
                })),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: vec![
                (
                    "Base".to_string(),
                    HirClassDef {
                        name: "Base".to_string(),
                        bases: Vec::new(),
                        mro: vec!["Base".to_string()],
                        attrs: Vec::new(),
                        methods: vec![
                            ("__init__".to_string(), "Base.__init__".to_string()),
                            ("greet".to_string(), "Base.greet".to_string()),
                        ],
                        type_param: None,
                        properties: Vec::new(),
                        static_methods: Vec::new(),
                        class_methods: Vec::new(),
                        enum_members: Vec::new(),
                    },
                ),
                (
                    "Derived".to_string(),
                    HirClassDef {
                        name: "Derived".to_string(),
                        bases: vec!["Base".to_string()],
                        mro: vec!["Derived".to_string(), "Base".to_string()],
                        attrs: Vec::new(),
                        methods: Vec::new(),
                        type_param: None,
                        properties: Vec::new(),
                        static_methods: Vec::new(),
                        class_methods: Vec::new(),
                        enum_members: Vec::new(),
                    },
                ),
            ],
        };
        let resolved = check_and_resolve(&hir).expect("check_and_resolve should succeed");
        let _mir = pycc_mir::build(&resolved);
    }

    // -- #436: @staticmethod / @classmethod type checking -------------------

    /// Builds a module with a class `C` that has `__init__`, a static method
    /// `create(x: int) -> int`, and a class method `greet(cls, x: int) ->
    /// int`. `extra_items`/`extra_stmts` let each test append its own
    /// call-site exercise.
    fn static_class_module(extra_items: Vec<HirItem>) -> HirModule {
        let self_ty = Ty::Instance(Box::new("C".to_string()));
        let init = HirItem::Function {
            name: "C.__init__".to_string(),
            params: vec![("self".to_string(), self_ty.clone())],
            return_ty: Ty::None,
            body: vec![HirStmt::Return(None)],
        };
        let static_fn = HirItem::Function {
            name: "C.create.static".to_string(),
            params: vec![("x".to_string(), Ty::Int)],
            return_ty: Ty::Int,
            body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
        };
        let class_fn = HirItem::Function {
            name: "C.greet.classmethod".to_string(),
            params: vec![
                ("cls".to_string(), self_ty.clone()),
                ("x".to_string(), Ty::Int),
            ],
            return_ty: Ty::Int,
            body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
        };
        let mut items = vec![init, static_fn, class_fn];
        items.extend(extra_items);
        HirModule {
            items,
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: vec![(
                "C".to_string(),
                HirClassDef {
                    name: "C".to_string(),
                    bases: Vec::new(),
                    mro: vec!["C".to_string()],
                    attrs: Vec::new(),
                    methods: vec![("__init__".to_string(), "C.__init__".to_string())],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: vec![("create".to_string(), "C.create.static".to_string())],
                    class_methods: vec![("greet".to_string(), "C.greet.classmethod".to_string())],
                    enum_members: Vec::new(),
                },
            )],
        }
    }

    #[test]
    fn static_method_call_through_class_name_type_checks() {
        let hir = static_class_module(vec![top_level(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::MethodCall {
                base: Box::new(HirExpr::Name("C".to_string())),
                method: "create".to_string(),
                args: vec![HirExpr::IntLiteral(42)],
            }],
        }))]);
        check(&hir).expect("a static method call through a class name should type-check");
    }

    #[test]
    fn class_method_call_through_class_name_type_checks() {
        let hir = static_class_module(vec![top_level(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::MethodCall {
                base: Box::new(HirExpr::Name("C".to_string())),
                method: "greet".to_string(),
                args: vec![HirExpr::IntLiteral(42)],
            }],
        }))]);
        check(&hir).expect("a class method call through a class name should type-check");
    }

    #[test]
    fn static_method_call_through_instance_type_checks() {
        let hir = static_class_module(vec![
            top_level(HirStmt::Assign {
                target: "c".to_string(),
                value: HirExpr::Call {
                    callee: "C".to_string(),
                    args: vec![],
                },
            }),
            top_level(HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::MethodCall {
                    base: Box::new(HirExpr::Name("c".to_string())),
                    method: "create".to_string(),
                    args: vec![HirExpr::IntLiteral(42)],
                }],
            })),
        ]);
        check(&hir).expect("a static method call through an instance should type-check");
    }

    #[test]
    fn class_method_call_through_instance_type_checks() {
        let hir = static_class_module(vec![
            top_level(HirStmt::Assign {
                target: "c".to_string(),
                value: HirExpr::Call {
                    callee: "C".to_string(),
                    args: vec![],
                },
            }),
            top_level(HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::MethodCall {
                    base: Box::new(HirExpr::Name("c".to_string())),
                    method: "greet".to_string(),
                    args: vec![HirExpr::IntLiteral(42)],
                }],
            })),
        ]);
        check(&hir).expect("a class method call through an instance should type-check");
    }

    #[test]
    fn static_method_wrong_argument_type_is_rejected() {
        let hir = static_class_module(vec![top_level(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::MethodCall {
                base: Box::new(HirExpr::Name("C".to_string())),
                method: "create".to_string(),
                args: vec![HirExpr::StringLiteral("hi".to_string())],
            }],
        }))]);
        let diagnostic = check(&hir).unwrap_err();
        assert_eq!(diagnostic.code, "T0021");
    }

    #[test]
    fn class_method_wrong_argument_type_is_rejected() {
        let hir = static_class_module(vec![top_level(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::MethodCall {
                base: Box::new(HirExpr::Name("C".to_string())),
                method: "greet".to_string(),
                args: vec![HirExpr::StringLiteral("hi".to_string())],
            }],
        }))]);
        let diagnostic = check(&hir).unwrap_err();
        assert_eq!(diagnostic.code, "T0021");
    }

    #[test]
    fn static_method_wrong_argument_count_is_rejected() {
        let hir = static_class_module(vec![top_level(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::MethodCall {
                base: Box::new(HirExpr::Name("C".to_string())),
                method: "create".to_string(),
                args: vec![],
            }],
        }))]);
        let diagnostic = check(&hir).unwrap_err();
        assert_eq!(diagnostic.code, "T0021");
    }

    #[test]
    fn class_method_excludes_cls_from_argument_count() {
        // `greet` takes `(cls, x: int)` — the caller passes only `x`. The
        // type checker excludes `cls` from the argument count check, so
        // passing one argument is correct.
        let hir = static_class_module(vec![top_level(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::MethodCall {
                base: Box::new(HirExpr::Name("C".to_string())),
                method: "greet".to_string(),
                args: vec![HirExpr::IntLiteral(42)],
            }],
        }))]);
        check(&hir).expect("cls is excluded from the argument count check");
    }

    #[test]
    fn unknown_static_or_class_method_on_class_name_falls_through() {
        // A method call on a class name that is neither a static method nor
        // a class method falls through to the regular resolution, which
        // tries to infer the base expression (`HirExpr::Name("C")`). A bare
        // class name is not a binding, so `infer_expr_in` rejects it with
        // T0021 ("name `C` is not defined").
        let hir = static_class_module(vec![top_level(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::MethodCall {
                base: Box::new(HirExpr::Name("C".to_string())),
                method: "nonexistent".to_string(),
                args: vec![],
            }],
        }))]);
        let diagnostic = check(&hir).unwrap_err();
        assert_eq!(diagnostic.code, "T0021");
    }

    #[test]
    fn attribute_access_on_a_class_name_is_rejected() {
        // `C.x` — accessing an attribute on a class name (not an instance)
        // is not supported. A static or class method must be called, not
        // accessed as a bare attribute value.
        let hir = static_class_module(vec![top_level(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::AttrGet {
                base: Box::new(HirExpr::Name("C".to_string())),
                attr: "create".to_string(),
            }],
        }))]);
        let diagnostic = check(&hir).unwrap_err();
        assert_eq!(diagnostic.code, "T0044");
    }

    #[test]
    fn resolve_static_or_class_method_call_with_unknown_method_returns_t0044() {
        // Directly exercises the `Err(t0044_unknown_member(...))` fallback
        // in `resolve_static_or_class_method_call` — unreachable from
        // `infer_expr_in` (which calls `has_static_or_class_method` first),
        // but the function is `pub(crate)` and can be called directly.
        let mut env = crate::Environment::new();
        env.bind_class(
            "C".to_string(),
            HirClassDef {
                name: "C".to_string(),
                bases: Vec::new(),
                mro: vec!["C".to_string()],
                attrs: Vec::new(),
                methods: vec![("__init__".to_string(), "C.__init__".to_string())],
                type_param: None,
                properties: Vec::new(),
                static_methods: Vec::new(),
                class_methods: Vec::new(),
                enum_members: Vec::new(),
            },
        );
        let diagnostic =
            super::resolve_static_or_class_method_call(&env, "C", "nonexistent", &[]).unwrap_err();
        assert_eq!(diagnostic.code, "T0044");
    }

    #[test]
    fn has_static_or_class_method_returns_false_for_unknown_class() {
        // Exercises the `env.lookup_class(class_name)` → `None` →
        // `return false` path in `has_static_or_class_method`.
        let env = crate::Environment::new();
        assert!(!super::has_static_or_class_method(
            &env,
            "Nonexistent",
            "create"
        ));
    }

    #[test]
    fn has_static_or_class_method_returns_false_for_unknown_method() {
        // Exercises the `mro_def.static_methods.iter().any(..)` → false
        // and `mro_def.class_methods.iter().any(..)` → false path.
        let mut env = crate::Environment::new();
        env.bind_class(
            "C".to_string(),
            HirClassDef {
                name: "C".to_string(),
                bases: Vec::new(),
                mro: vec!["C".to_string()],
                attrs: Vec::new(),
                methods: vec![("__init__".to_string(), "C.__init__".to_string())],
                type_param: None,
                properties: Vec::new(),
                static_methods: vec![("create".to_string(), "C.create.static".to_string())],
                class_methods: vec![("greet".to_string(), "C.greet.classmethod".to_string())],
                enum_members: Vec::new(),
            },
        );
        assert!(!super::has_static_or_class_method(&env, "C", "nonexistent"));
    }

    #[test]
    fn has_static_or_class_method_returns_false_for_ghost_mro_class() {
        // Exercises the `env.lookup_class(mro_class)` → `None` →
        // `return false` path inside the MRO walk of
        // `has_static_or_class_method`.
        let mut env = crate::Environment::new();
        env.bind_class(
            "Derived".to_string(),
            HirClassDef {
                name: "Derived".to_string(),
                bases: vec!["Ghost".to_string()],
                mro: vec!["Derived".to_string(), "Ghost".to_string()],
                attrs: Vec::new(),
                methods: vec![("__init__".to_string(), "Derived.__init__".to_string())],
                type_param: None,
                properties: Vec::new(),
                static_methods: Vec::new(),
                class_methods: Vec::new(),
                enum_members: Vec::new(),
            },
        );
        // `Ghost` is in the MRO but not registered — the defensive
        // `return false` prevents a panic and the overall result is false.
        assert!(!super::has_static_or_class_method(
            &env, "Derived", "create"
        ));
    }

    #[test]
    #[should_panic(expected = "internal error: `C.create.static` is in class `C`'s own \
                   static_methods table but was not registered as an ordinary function")]
    fn resolve_static_method_call_panics_if_function_not_registered() {
        // Exercises the defensive `unwrap_or_else(|| panic!(..))` in the
        // static-method lookup path of `resolve_static_or_class_method_call`.
        // This is unreachable from normal HIR (the HIR lowering always
        // emits the function alongside the table entry), so a hand-built
        // environment with a table entry but no registered function is needed.
        let mut env = crate::Environment::new();
        env.bind_class(
            "C".to_string(),
            HirClassDef {
                name: "C".to_string(),
                bases: Vec::new(),
                mro: vec!["C".to_string()],
                attrs: Vec::new(),
                methods: vec![("__init__".to_string(), "C.__init__".to_string())],
                type_param: None,
                properties: Vec::new(),
                static_methods: vec![("create".to_string(), "C.create.static".to_string())],
                class_methods: Vec::new(),
                enum_members: Vec::new(),
            },
        );
        let _ = super::resolve_static_or_class_method_call(&env, "C", "create", &[Ty::Int]);
    }

    #[test]
    #[should_panic(
        expected = "internal error: `C.greet.classmethod` is in class `C`'s own \
                   class_methods table but was not registered as an ordinary function"
    )]
    fn resolve_class_method_call_panics_if_function_not_registered() {
        // Exercises the defensive `unwrap_or_else(|| panic!(..))` in the
        // class-method lookup path of `resolve_static_or_class_method_call`.
        let mut env = crate::Environment::new();
        env.bind_class(
            "C".to_string(),
            HirClassDef {
                name: "C".to_string(),
                bases: Vec::new(),
                mro: vec!["C".to_string()],
                attrs: Vec::new(),
                methods: vec![("__init__".to_string(), "C.__init__".to_string())],
                type_param: None,
                properties: Vec::new(),
                static_methods: Vec::new(),
                class_methods: vec![("greet".to_string(), "C.greet.classmethod".to_string())],
                enum_members: Vec::new(),
            },
        );
        let _ = super::resolve_static_or_class_method_call(&env, "C", "greet", &[Ty::Int]);
    }

    #[test]
    fn class_name_static_method_call_propagates_arg_inference_error() {
        // Exercises the `?` on argument inference at lib.rs:3289 inside the
        // class-name static/class method call branch of `infer_expr_in`.
        // `C.create(undefined_name)` — the class `C` is registered and has a
        // static method `create`, so the class-name branch is entered, but
        // the argument `undefined_name` fails inference (T0021 "name not
        // defined"), which propagates through the `collect::<Result<..>>?`.
        let hir = static_class_module(vec![top_level(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::MethodCall {
                base: Box::new(HirExpr::Name("C".to_string())),
                method: "create".to_string(),
                args: vec![HirExpr::Name("undefined_name".to_string())],
            }],
        }))]);
        let diagnostic = check(&hir).unwrap_err();
        assert_eq!(diagnostic.code, "T0021");
    }

    // -----------------------------------------------------------------------
    // #435: isinstance/issubclass type checker unit tests
    // -----------------------------------------------------------------------

    #[test]
    fn isinstance_with_float_target_type_checks_as_bool() {
        // `isinstance(1.5, float)` — covers the `Ty::Float` arm of
        // `eval_isinstance_single` at the type-checker level (the type
        // checker only validates and returns `Ty::Bool`; the MIR computes
        // the constant). This also exercises `check_isinstance` with a
        // builtin type target.
        let hir = static_class_module(vec![top_level(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::Call {
                callee: "isinstance".to_string(),
                args: vec![
                    HirExpr::FloatLiteral(1.5),
                    HirExpr::Name("float".to_string()),
                ],
            }],
        }))]);
        let result = check(&hir);
        assert!(result.is_ok(), "isinstance(1.5, float) should type-check");
    }

    #[test]
    fn isinstance_with_wrong_arg_count_is_t0021() {
        // `isinstance(1)` — only 1 argument. Covers the `args.len() != 2`
        // error branch in `check_isinstance`.
        let hir = static_class_module(vec![top_level(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::Call {
                callee: "isinstance".to_string(),
                args: vec![HirExpr::IntLiteral(1)],
            }],
        }))]);
        let diagnostic = check(&hir).unwrap_err();
        assert_eq!(diagnostic.code, "T0021");
    }

    #[test]
    fn isinstance_with_non_class_second_arg_is_t0021() {
        // `isinstance(1, 5)` — the second argument is not a class name or
        // tuple of class names. Covers the `extract_class_names` error
        // branch in `check_isinstance`.
        let hir = static_class_module(vec![top_level(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::Call {
                callee: "isinstance".to_string(),
                args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(5)],
            }],
        }))]);
        let diagnostic = check(&hir).unwrap_err();
        assert_eq!(diagnostic.code, "T0021");
    }

    #[test]
    fn issubclass_with_wrong_arg_count_is_t0021() {
        // `issubclass(int)` — only 1 argument. Covers the `args.len() != 2`
        // error branch in `check_issubclass`.
        let hir = static_class_module(vec![top_level(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::Call {
                callee: "issubclass".to_string(),
                args: vec![HirExpr::Name("int".to_string())],
            }],
        }))]);
        let diagnostic = check(&hir).unwrap_err();
        assert_eq!(diagnostic.code, "T0021");
    }

    #[test]
    fn isinstance_with_undefined_name_in_first_arg_is_t0021() {
        // `isinstance(undefined_name, int)` — the first argument references
        // an undefined name, so `infer_expr_in` fails with T0021 before
        // the class argument is even examined. This covers the `?` error
        // branch on the `infer_expr_in` call in `check_isinstance`.
        let hir = static_class_module(vec![top_level(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::Call {
                callee: "isinstance".to_string(),
                args: vec![
                    HirExpr::Name("undefined_name".to_string()),
                    HirExpr::Name("int".to_string()),
                ],
            }],
        }))]);
        let diagnostic = check(&hir).unwrap_err();
        assert_eq!(diagnostic.code, "T0021");
    }

    #[test]
    fn issubclass_with_unknown_class_in_first_arg_is_t0001() {
        // `issubclass(UnknownClass, int)` — the first argument is a class
        // name that is neither a user-defined class nor a builtin type.
        // This covers the `validate_class_name` error path for the first
        // argument in `check_issubclass`.
        let hir = static_class_module(vec![top_level(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::Call {
                callee: "issubclass".to_string(),
                args: vec![
                    HirExpr::Name("UnknownClass".to_string()),
                    HirExpr::Name("int".to_string()),
                ],
            }],
        }))]);
        let diagnostic = check(&hir).unwrap_err();
        assert_eq!(diagnostic.code, "T0001");
    }

    #[test]
    fn issubclass_with_non_class_second_arg_is_t0021() {
        // `issubclass(C, 5)` — the second argument is not a class name or
        // tuple of class names. This exercises the `extract_class_names`
        // error path in `check_issubclass`.
        let hir = static_class_module(vec![top_level(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::Call {
                callee: "issubclass".to_string(),
                args: vec![HirExpr::Name("C".to_string()), HirExpr::IntLiteral(5)],
            }],
        }))]);
        let diagnostic = check(&hir).unwrap_err();
        assert_eq!(diagnostic.code, "T0021");
    }

    #[test]
    fn issubclass_with_int_str_same_type_returns_bool() {
        // `issubclass(int, int)` and `issubclass(str, str)` — covers the
        // `return cls == target_class` line in `eval_issubclass_single`
        // at the type-checker level.
        let hir = static_class_module(vec![top_level(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::Call {
                callee: "issubclass".to_string(),
                args: vec![
                    HirExpr::Name("int".to_string()),
                    HirExpr::Name("int".to_string()),
                ],
            }],
        }))]);
        let result = check(&hir);
        assert!(result.is_ok(), "issubclass(int, int) should type-check");
    }

    #[test]
    fn isinstance_with_unknown_class_in_second_arg_is_t0001() {
        // `isinstance(1, UnknownClass)` — the second argument is a valid
        // name expression but not a registered class. Covers the
        // `validate_class_name` error path in `check_isinstance`.
        let hir = static_class_module(vec![top_level(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::Call {
                callee: "isinstance".to_string(),
                args: vec![
                    HirExpr::IntLiteral(1),
                    HirExpr::Name("UnknownClass".to_string()),
                ],
            }],
        }))]);
        let diagnostic = check(&hir).unwrap_err();
        assert_eq!(diagnostic.code, "T0001");
    }

    #[test]
    fn issubclass_with_unknown_class_in_second_arg_is_t0001() {
        // `issubclass(int, UnknownClass)` — the second argument is a valid
        // name expression but not a registered class. Covers the
        // `validate_class_name` error path for target classes in
        // `check_issubclass`.
        let hir = static_class_module(vec![top_level(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::Call {
                callee: "issubclass".to_string(),
                args: vec![
                    HirExpr::Name("int".to_string()),
                    HirExpr::Name("UnknownClass".to_string()),
                ],
            }],
        }))]);
        let diagnostic = check(&hir).unwrap_err();
        assert_eq!(diagnostic.code, "T0001");
    }

    #[test]
    fn issubclass_with_non_name_first_arg_is_t0021() {
        // `issubclass(5, int)` — the first argument is not a bare class
        // name. Covers the `_ =>` error branch in `check_issubclass`'s
        // first-argument match.
        let hir = static_class_module(vec![top_level(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::Call {
                callee: "issubclass".to_string(),
                args: vec![HirExpr::IntLiteral(5), HirExpr::Name("int".to_string())],
            }],
        }))]);
        let diagnostic = check(&hir).unwrap_err();
        assert_eq!(diagnostic.code, "T0021");
    }

    #[test]
    fn isinstance_with_call_first_arg_is_c0001() {
        // #435 review fix (P1): `isinstance(D(), D)` — a call expression as
        // the first argument is rejected with C0001 because pycc's
        // compile-time `isinstance` would silently discard the call's side
        // effects. Covers the `if let HirExpr::Call { .. }` branch in
        // `check_isinstance`.
        let hir = static_class_module(vec![top_level(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::Call {
                callee: "isinstance".to_string(),
                args: vec![
                    HirExpr::Call {
                        callee: "D".to_string(),
                        args: vec![],
                    },
                    HirExpr::Name("D".to_string()),
                ],
            }],
        }))]);
        let diagnostic = check(&hir).unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(diagnostic.message.contains("isinstance"));
    }
}
