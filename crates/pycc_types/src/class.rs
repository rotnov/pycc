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

mod binding;
mod method_call;

use crate::{Environment, infer_expr_in, is_assignable};
use pycc_diag::{Diagnostic, Span};
use pycc_hir::{HirExpr, ProtocolMember, Ty, extract_class_names, is_builtin_type_name};

// A private import, not a re-export: it keeps this module's own unqualified
// `expect_class` call sites compiling now that the function itself lives in
// `class/binding.rs`.
use binding::expect_class;
// A re-export, so every existing `class::bind_classes` /
// `class::resolve_instantiation` call site (and the doc comments in `lib.rs`
// naming those paths) keeps working across the `class/binding.rs` extraction.
pub(crate) use binding::{bind_classes, resolve_instantiation};
// A re-export, so every existing `class::resolve_method_call` call site
// (`expr.rs`'s `HirExpr::MethodCall` arm) keeps working across the
// `class/method_call.rs` extraction (#815, Part 1 of #737).
pub(crate) use method_call::resolve_method_call;

/// #380 (PR-20): Checks whether a concrete class structurally conforms to
/// a protocol. A class conforms if, for every protocol member (method or
/// attribute), the class has a member of the same name with a compatible
/// type. Method compatibility requires matching parameter count and
/// assignable parameter/return types. Attribute compatibility requires
/// an assignable attribute type. Members are looked up through the
/// class's MRO (inherited methods and attributes count toward
/// conformance). Returns `Ok(())` if the class conforms, or a `T0046`
/// diagnostic identifying the first missing or incompatible member.
pub(crate) fn check_protocol_conformance(
    env: &Environment,
    class_name: &str,
    protocol_name: &str,
) -> Result<(), Diagnostic> {
    let class_def = env
        .lookup_class(class_name)
        .expect("pycc_types: internal error: class was not registered -- check_protocol_conformance should only be called with a known class");
    let proto_def = env
        .lookup_class(protocol_name)
        .expect("pycc_types: internal error: protocol was not registered -- check_protocol_conformance should only be called with a known protocol");
    // The protocol's members are already merged (including inherited
    // members from base protocols) at HIR-lowering time.
    for member in &proto_def.protocol_members {
        match member {
            ProtocolMember::Method {
                name: method_name,
                param_tys: proto_param_tys,
                return_ty: proto_return_ty,
            } => {
                // Look up the method through the MRO.
                let found = lookup_method_through_mro(env, &class_def.mro, method_name);
                let Some((concrete_param_tys, concrete_return_ty)) = found else {
                    return Err(Diagnostic::error(
                        "T0046",
                        format!(
                            "class `{class_name}` does not conform to protocol \
                             `{protocol_name}`: missing method `{method_name}`"
                        ),
                        Span::new(0, 0),
                    ));
                };
                // Check parameter count (excluding `self`).
                // `concrete_param_tys` includes `self`; `proto_param_tys`
                // excludes it (stripped at HIR-lowering time).
                let concrete_non_self: Vec<Ty> = concrete_param_tys
                    .iter()
                    .skip_while(|(_, n)| n == "self")
                    .map(|(t, _)| t.clone())
                    .collect();
                if concrete_non_self.len() != proto_param_tys.len() {
                    return Err(Diagnostic::error(
                        "T0046",
                        format!(
                            "class `{class_name}` does not conform to protocol \
                             `{protocol_name}`: method `{method_name}` has {} parameter(s), \
                             expected {}",
                            concrete_non_self.len(),
                            proto_param_tys.len()
                        ),
                        Span::new(0, 0),
                    ));
                }
                // Check parameter types (contravariant — a caller through
                // the protocol may pass any value of the protocol's declared
                // parameter type, so the concrete method must accept at least
                // that type: the protocol param type must be assignable to
                // the concrete param type).
                for (i, (cp, pp)) in concrete_non_self
                    .iter()
                    .zip(proto_param_tys.iter())
                    .enumerate()
                {
                    if !is_assignable(pp.clone(), cp.clone()) {
                        return Err(Diagnostic::error(
                            "T0046",
                            format!(
                                "class `{class_name}` does not conform to protocol \
                                 `{protocol_name}`: method `{method_name}` parameter {} has \
                                 type `{}`, expected `{}`",
                                i + 1,
                                cp.name(),
                                pp.name()
                            ),
                            Span::new(0, 0),
                        ));
                    }
                }
                // Check return type (covariant — concrete return must be
                // assignable to protocol return).
                if !is_assignable(concrete_return_ty.clone(), proto_return_ty.clone()) {
                    return Err(Diagnostic::error(
                        "T0046",
                        format!(
                            "class `{class_name}` does not conform to protocol \
                             `{protocol_name}`: method `{method_name}` return type is `{}`, \
                             expected `{}`",
                            concrete_return_ty.name(),
                            proto_return_ty.name()
                        ),
                        Span::new(0, 0),
                    ));
                }
            }
            ProtocolMember::Attribute {
                name: attr_name,
                ty: proto_attr_ty,
            } => {
                // Look up the attribute through the MRO.
                let found = lookup_attr_through_mro(env, &class_def.mro, attr_name);
                let Some(concrete_attr_ty) = found else {
                    return Err(Diagnostic::error(
                        "T0046",
                        format!(
                            "class `{class_name}` does not conform to protocol \
                             `{protocol_name}`: missing attribute `{attr_name}`"
                        ),
                        Span::new(0, 0),
                    ));
                };
                if !is_assignable(concrete_attr_ty.clone(), proto_attr_ty.clone()) {
                    return Err(Diagnostic::error(
                        "T0046",
                        format!(
                            "class `{class_name}` does not conform to protocol \
                             `{protocol_name}`: attribute `{attr_name}` has type `{}`, \
                             expected `{}`",
                            concrete_attr_ty.name(),
                            proto_attr_ty.name()
                        ),
                        Span::new(0, 0),
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Looks up a method through the MRO, returning its parameter types
/// (including `self`) and return type. Walks the MRO most-derived-first
/// and returns the first match.
fn lookup_method_through_mro(
    env: &Environment,
    mro: &[String],
    method_name: &str,
) -> Option<(Vec<(Ty, String)>, Ty)> {
    for mro_class in mro {
        // Every class in the MRO was defined earlier in the module and
        // is present in the `Environment`'s class table — the C3 MRO is
        // built from already-lowered class definitions. Using `.expect`
        // (whose panic path lives in libcore, outside this crate's
        // instrumented regions) avoids a permanently-uncovered `?`-None
        // region under D-014's 100 %-coverage gate.
        let mro_def = env
            .lookup_class(mro_class)
            .expect("MRO classes are always in the environment");
        if let Some(mangled) = mro_def
            .methods
            .iter()
            .find(|(name, _)| name == method_name)
            .map(|(_, mangled)| mangled.as_str())
            && let Some((param_tys, return_ty)) = env.lookup_function(mangled)
        {
            // `param_tys` is `Vec<Ty>` (just types, no names). We
            // pair each with an empty name since conformance
            // checking only needs the types. The `self` parameter
            // is identified by position (first), not by name.
            let named: Vec<(Ty, String)> = param_tys
                .iter()
                .enumerate()
                .map(|(i, ty)| {
                    (
                        ty.clone(),
                        if i == 0 {
                            "self".to_string()
                        } else {
                            String::new()
                        },
                    )
                })
                .collect();
            return Some((named, return_ty.clone()));
        }
    }
    None
}

/// Looks up an attribute type through the MRO. Walks the MRO
/// most-derived-first and returns the first match.
fn lookup_attr_through_mro(env: &Environment, mro: &[String], attr_name: &str) -> Option<Ty> {
    for mro_class in mro {
        // Same invariant as `lookup_method_through_mro` above: every
        // class in the MRO is present in the `Environment`'s class table.
        let mro_def = env
            .lookup_class(mro_class)
            .expect("MRO classes are always in the environment");
        if let Some((_, ty)) = mro_def.attrs.iter().find(|(name, _)| name == attr_name) {
            return Some(ty.clone());
        }
        // A @property satisfies an attribute requirement just as a direct
        // attribute does — `eval_isinstance_protocol` already checks both,
        // and the conformance check must be consistent with it (#380 W2).
        if let Some(prop) = mro_def.properties.iter().find(|p| p.name == attr_name) {
            let (_, return_ty) = env.lookup_function(&prop.getter).unwrap_or_else(|| {
                panic!(
                    "pycc_types: internal error: property getter `{}` is in class `{mro_class}`'s \
                     own property table but was not registered as an ordinary function",
                    prop.getter
                )
            });
            return Some(return_ty.clone());
        }
    }
    None
}

/// #380 (PR-20): Checks assignability with protocol conformance support.
/// Delegates to `is_assignable` for non-protocol cases. When `to` is
/// `Ty::Protocol(name)` and `from` is `Ty::Instance(class_name)`, checks
/// structural conformance via `check_protocol_conformance`. Returns
/// `true` if assignable, `false` otherwise. For detailed error messages,
/// call `check_protocol_conformance` directly.
pub(crate) fn is_assignable_env(env: &Environment, from: &Ty, to: &Ty) -> bool {
    // Protocol conformance: Instance -> Protocol
    if let (Ty::Instance(class_name), Ty::Protocol(protocol_name)) = (from, to) {
        return check_protocol_conformance(env, class_name, protocol_name).is_ok();
    }
    // Protocol to Protocol: same protocol or inherited
    if let (Ty::Protocol(from_name), Ty::Protocol(to_name)) = (from, to) {
        if from_name == to_name {
            return true;
        }
        // Check if from_name's MRO includes to_name
        if let Some(from_def) = env.lookup_class(from_name)
            && from_def.mro.iter().any(|m| m.as_str() == to_name.as_str())
        {
            return true;
        }
        return false;
    }
    // For all other cases, use the plain `is_assignable`.
    is_assignable(from.clone(), to.clone())
}

/// #380 (PR-20): Returns a `T0046` diagnostic for a protocol conformance
/// failure, or a `T0021` for a general type mismatch. Called when
/// `is_assignable_env` returns `false` to produce a detailed error.
pub(crate) fn assignable_error(env: &Environment, from: &Ty, to: &Ty) -> Diagnostic {
    if let (Ty::Instance(class_name), Ty::Protocol(protocol_name)) = (from, to) {
        // `assignable_error` is only called when `is_assignable_env`
        // returned `false`, which means `check_protocol_conformance`
        // already returned `Err`.  Using `.expect()` (whose panic path
        // lives in libcore, outside this crate's instrumented regions)
        // avoids a permanently-uncovered `unwrap_or_else` closure under
        // D-014's 100 %-region coverage gate.
        return check_protocol_conformance(env, class_name, protocol_name)
            .expect_err("is_assignable_env already returned false, so check_protocol_conformance must return Err");
    }
    if let (Ty::Protocol(from_name), Ty::Protocol(to_name)) = (from, to)
        && from_name != to_name
    {
        return Diagnostic::error(
            "T0046",
            format!("protocol `{from_name}` does not conform to protocol `{to_name}`"),
            Span::new(0, 0),
        );
    }
    Diagnostic::error(
        "T0021",
        format!(
            "type mismatch: `{}` is not assignable to `{}`",
            from.name(),
            to.name()
        ),
        Span::new(0, 0),
    )
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

/// #587: `super().<attr>` naming an *instance* attribute — one established
/// by `self.<attr> = ...` inside `__init__`, and therefore living in the
/// instance's own slot rather than on any class in the MRO. CPython's
/// `super` object proxies class-level attributes and descriptors only, so
/// this raises `AttributeError` there; pycc used to resolve it against
/// `self`'s slot and return a value instead.
fn t0047_super_instance_attr(attr: &str, declaring_class: &str) -> Diagnostic {
    Diagnostic::error(
        "T0047",
        format!(
            "`super().{attr}` is not readable: `{attr}` is an instance attribute of class \
             `{declaring_class}`, and `super()` proxies class-level attributes and \
             descriptors along the MRO, not the instance's own attributes -- CPython raises \
             `AttributeError: 'super' object has no attribute '{attr}'` here"
        ),
        Span::new(0, 0),
    )
    .with_help(format!("read it through `self` instead: `self.{attr}`"))
}

/// Validates a call's arguments against a resolved `(param_tys, return_ty)`
/// signature, reusing this crate's own existing "call to undefined
/// function"-adjacent diagnostic shape (`T0021`, `infer_expr_in`'s own
/// `HirExpr::Call` arm) rather than inventing a class-specific arity/type
/// mismatch code -- an instantiation call and a method call are both, at
/// their core, "call this mangled function with these arguments," the same
/// shape an ordinary function call already validates.
pub(crate) fn check_call_args(
    callee: &str,
    arg_tys: &[Ty],
    param_tys: &[Ty],
) -> Result<(), Diagnostic> {
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
    // #380 (PR-20, PEP 544): protocol-typed variable attribute access.
    // When `base_ty` is `Ty::Protocol(P)`, look up the attribute in
    // `P`'s `protocol_members`.
    if let Ty::Protocol(protocol_name) = base_ty {
        let proto_def = expect_class(env, protocol_name);
        for member in &proto_def.protocol_members {
            if let ProtocolMember::Attribute { name, ty } = member
                && name == attr
            {
                return Ok(ty.clone());
            }
        }
        return Err(t0044_unknown_member("attribute", protocol_name, attr));
    }
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
    // #911 (Part 1 of #885): a class-level attribute (`MIN_WIDTH: int =
    // -1024`) read through an instance. Checked *after* the instance slots
    // so a real slot always wins -- the two can never actually collide,
    // because `pycc_hir` rejects a class attribute that shares a name with
    // an instance slot or a `@property` in either declaration order, but the
    // ordering keeps that invariant from being load-bearing here.
    if let Some(ty) = lookup_class_attr_through_mro(env, class_name, attr) {
        return Ok(ty);
    }
    Err(t0044_unknown_member("attribute", class_name, attr))
}

/// #911 (Part 1 of #885): looks a class-level attribute up through
/// `class_name`'s MRO, most-derived first, returning its declared type.
///
/// A class attribute occupies no instance slot -- `pycc_mir` folds every
/// read of it to the constant recorded in `HirClassDef::class_attrs` -- so
/// this walk is deliberately separate from the `attrs` walk above rather
/// than merged into it.
pub(crate) fn lookup_class_attr_through_mro(
    env: &Environment,
    class_name: &str,
    attr: &str,
) -> Option<Ty> {
    let class_def = expect_class(env, class_name);
    for mro_class in &class_def.mro {
        let mro_def = expect_class(env, mro_class);
        if let Some((_, ty, _)) = mro_def.class_attrs.iter().find(|(name, _, _)| name == attr) {
            return Some(ty.clone());
        }
    }
    None
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
/// class's own entries and searches the rest of the MRO.
///
/// Only *class-level* members resolve here. A `super` object proxies the
/// class-level attributes and descriptors it finds along the MRO; it does
/// not proxy the instance `__dict__`, so an attribute established by
/// `self.<attr> = ...` inside `__init__` is not reachable through it and
/// CPython raises `AttributeError: 'super' object has no attribute
/// '<attr>'`. pycc used to resolve that form against `self`'s own slot and
/// return a value, which disagreed with the pinned oracle on the value of
/// an expression rather than merely on what compiles (#587); it is now
/// rejected with `T0047`. Of the class-level members pycc models, only
/// properties are reachable this way today, so this function resolves a
/// property or rejects.
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
    // #587: an instance attribute — established by `self.<attr> = ...`
    // inside `__init__` and stored in the instance's own slot — is not
    // proxied by a `super` object in CPython, which raises
    // `AttributeError: 'super' object has no attribute '<attr>'`. Reject
    // it rather than resolving it against `self`'s slot: the slot read is
    // exactly what `self.<attr>` already spells, so the rejection costs no
    // expressive power and the diagnostic can name the fix.
    for mro_class in super_mro {
        let mro_def = expect_class(env, mro_class);
        if mro_def.attrs.iter().any(|(name, _)| name == attr) {
            return Err(t0047_super_instance_attr(attr, mro_class));
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
    // #911 (Part 1 of #885): every write path to a class-level attribute is
    // rejected. A class attribute is a compile-time constant folded at each
    // read (`pycc_mir` never allocates a slot for it), so `obj.X = 5` has
    // nowhere to write -- and CPython's own semantics here (the write
    // creates an *instance* attribute that shadows the class one, leaving
    // the class attribute itself untouched) are not modelled at all. This
    // check runs before the property walk and before `resolve_attr_get`, and
    // covers every write that reaches the type checker: `obj.X = 5` and
    // `self.X = 5` in a method other than `__init__`.
    //
    // A `self.X = 5` inside `__init__` splits by where `X` was declared.
    // When the *same* class declares it, `collect_init_attrs` turns the
    // write into an instance slot and HIR's own
    // `reject_class_attr_collisions` (see `pycc_hir::class::body`) rejects
    // that shape first with C0001. When an *ancestor* declares it, HIR does
    // not look in that direction at all, and this check is the only thing
    // that rejects it -- which is why `lookup_class_attr_through_mro` must
    // stay a full-MRO walk and must not be narrowed to the class's own
    // `class_attrs`. `pycc_mir`'s instance-read fold (`fold_class_attr` in
    // the MRO loop of `pycc_mir::expr`) walks the same MRO and would
    // otherwise fold the read of a genuinely written slot to the ancestor's
    // constant.
    if let Ty::Instance(class_name) = &base_ty
        && let Some(class_attr_ty) = lookup_class_attr_through_mro(env, class_name, attr)
    {
        return Err(Diagnostic::error(
            "T0044",
            format!(
                "cannot assign to `{attr}`: it is a class-level attribute of class \
                 `{class_name}` (declared `{}`), which is a compile-time constant with no \
                 storage to write to",
                class_attr_ty.name()
            ),
            Span::new(0, 0),
        ));
    }
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
/// either a registered user-defined class (including protocols) or one of
/// the builtin scalar type names (`int`, `str`, `float`, `bool`). Returns
/// `Ok(())` if valid, or a `T0001` diagnostic if the name is unknown.
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

/// #767: Maps a validated `cast` target name to the `Ty` the call
/// expression has. The four builtin scalar names map to their scalar `Ty`;
/// every other name is a user-defined class name and maps to
/// `Ty::Instance`. Shared by `check_cast` (validation pass) and the
/// constraint solver's own `cast` mirror, so the two passes cannot drift.
/// The caller is responsible for having validated that a non-builtin name
/// really names a known class — this function only performs the mapping.
pub(crate) fn cast_target_ty(name: &str) -> Ty {
    match name {
        "int" => Ty::Int,
        "float" => Ty::Float,
        "bool" => Ty::Bool,
        "str" => Ty::Str,
        other => Ty::Instance(Box::new(other.to_string())),
    }
}

/// #767: Validates the environment-independent half of a `cast(T, value)`
/// call shape — exactly two arguments, the first of them a bare name — and
/// returns that target name. Shared by `check_cast` (validation pass) and
/// the constraint solver's own `cast` mirror, which has no class table but
/// can and should report these two malformed-shape diagnostics itself:
/// returning "no type term" for them instead would surface the solver's
/// generic "cannot infer return type of private helper" in place of the
/// accurate message whenever the call is in a return-type-inferred helper.
pub(crate) fn cast_target_name(args: &[HirExpr]) -> Result<&str, Diagnostic> {
    if args.len() != 2 {
        return Err(Diagnostic::error(
            "T0021",
            format!("`cast` expects exactly 2 arguments, got {}", args.len()),
            Span::new(0, 0),
        )
        .with_help("pass exactly 2 arguments: the target type and the value"));
    }
    let HirExpr::Name(target) = &args[0] else {
        return Err(Diagnostic::error(
            "C0001",
            "`cast`'s first argument must be a bare type name in this pycc version -- \
             subscripted generics and other type expressions are not supported yet",
            Span::new(0, 0),
        )
        .with_help(
            "pass `int`, `float`, `bool`, `str`, or the name of a class defined in this module",
        ));
    };
    Ok(target)
}

/// #767 (D-198): Whether erasing a `cast(T, value)` to `value` alone leaves
/// the emitted code's representation *and* attribute layout agreeing with
/// the static type the rest of the program is then checked against.
///
/// `Ok(())` for a cast to the value's own type. For two distinct classes,
/// `Ok(())` only when `to` is one of `from`'s ancestors in its MRO — an
/// up-cast — and no class from `from` up to (but excluding) `to` overrides a
/// method `to` also defines or inherits (see the method-dispatch paragraph
/// below). Rejected for every other pair, including a genuine down-cast
/// (`to` is a strict descendant of `from`) and every scalar-representation
/// change such as `bool` -> `int`.
///
/// The class case looks like a pure representation question at first —
/// every class instance is one heap-object pointer regardless of class
/// (`ty_to_basic_type` in `pycc_codegen`), so a pointer reinterpretation
/// alone would never crash. But representation is not the whole story here:
/// `pycc_mir` erases `cast(...)` to `value` alone with no wrapper node (see
/// `check_cast`'s doc comment), so nothing downstream ever learns the
/// checker-verified target type `to` — MIR keeps tracking the expression's
/// *runtime* class, `from`. A down-cast the checker accepted as `to` would
/// then have the type checker validate `.attr`/`.method()` access against
/// `to`'s (larger) attribute set while MIR/codegen still resolve the access
/// against `from`'s (smaller) instance-slot layout: a `pycc_mir` panic for
/// an unannotated binding or inline access (`class_def_of` can't find the
/// attribute in `from`'s MRO), or, if an `AnnAssign` re-anchors the MIR
/// type to `to`, an out-of-bounds `pycc_rt` instance-slot abort at runtime,
/// since the object was never actually allocated with `to`'s extra slots.
/// An up-cast or identity cast has the opposite shape and stays sound on
/// *attribute layout*: `to`'s attribute set is a subset of (or equal to)
/// `from`'s, so whatever slot layout MIR keeps tracking already has every
/// attribute the checker will let code reach through the cast result.
///
/// Attribute layout is not the only thing erasure puts at risk, though.
/// `pycc_mir` also resolves *method calls* statically from the same
/// MIR-tracked runtime type `from` (no vtable -- see the comment at
/// `crates/pycc_mir/src/expr.rs:608`), walking `from`'s own MRO to find the
/// implementation. An `AnnAssign` re-anchors that MIR-tracked type to the
/// declared annotation for any non-protocol annotation
/// (`crates/pycc_mir/src/stmt.rs`'s `bind_ty` computation), so
/// `b: Base = cast(Base, d)` makes every later `b.m()` dispatch through
/// `Base`'s MRO even though the allocated object is `d`'s real class. If some
/// class between `from` and `to` (inclusive of `from`, exclusive of `to`)
/// overrides a method that `to` itself already defines or inherits, that
/// static resolution silently returns `to`'s (or an intermediate ancestor's)
/// implementation instead of the override CPython's dynamic dispatch would
/// have called -- a wrong answer with no diagnostic and no crash, which is
/// strictly worse than the attribute-layout failures above. So an up-cast is
/// sound only when no class strictly more derived than `to` (down to and
/// including `from`) overrides a method reachable from `to`'s own MRO.
/// `__init__` is excluded from this check on both sides: it runs once at
/// construction, before any `cast` of the resulting object could apply, so
/// every subclass defining its own `__init__` -- the ordinary case, not an
/// exception -- would otherwise make nearly every up-cast of a real class
/// hierarchy unsound by this rule for a call that can never actually be
/// re-dispatched through the cast result.
///
/// There is no `Ty::Protocol` case. A protocol name is not reachable as a
/// `cast` target (`cast_target_ty` maps every non-builtin bare name to
/// `Ty::Instance`), and a protocol-typed parameter is monomorphized to its
/// concrete class before its function body is checked.
///
/// Deliberately not `is_assignable_env`: that is a *subtyping* test, and it
/// admits `bool` -> `int` (a representation change) while restricting
/// `Instance` -> `Instance` to protocol conformance rather than MRO
/// ancestry.
///
/// Returns `Ok(())` when `to` is sound, or a [`CastMismatch`] describing
/// which of the three distinct failure shapes applies -- `check_cast` uses
/// the variant to report a message specific to the actual cause instead of
/// one fused string that would be true (and therefore uninformative) for
/// any of the three.
fn cast_compatibility(env: &Environment, from: &Ty, to: &Ty) -> Result<(), CastMismatch> {
    if from == to {
        return Ok(());
    }
    let (Ty::Instance(from_name), Ty::Instance(to_name)) = (from, to) else {
        return Err(CastMismatch::Representation);
    };
    let Some(from_def) = env.lookup_class(from_name) else {
        return Err(CastMismatch::Representation);
    };
    let Some(to_pos) = from_def
        .mro
        .iter()
        .position(|m| m.as_str() == to_name.as_str())
    else {
        return Err(CastMismatch::Layout);
    };
    let Some(to_def) = env.lookup_class(to_name) else {
        return Err(CastMismatch::Layout);
    };
    let target_methods: std::collections::HashSet<&str> = to_def
        .mro
        .iter()
        .filter_map(|name| env.lookup_class(name))
        .flat_map(|def| def.methods.iter().map(|(name, _)| name.as_str()))
        .filter(|name| *name != "__init__")
        .collect();
    for def in from_def.mro[..to_pos]
        .iter()
        .filter_map(|name| env.lookup_class(name))
    {
        if let Some((name, _)) = def
            .methods
            .iter()
            .filter(|(name, _)| name != "__init__")
            .find(|(name, _)| target_methods.contains(name.as_str()))
        {
            return Err(CastMismatch::OverriddenMethod(name.clone()));
        }
    }
    Ok(())
}

/// The three distinct ways `cast_compatibility` rejects a target, each
/// reported with its own message by `check_cast`: a scalar or unrelated-type
/// representation change, a genuine down-cast or unrelated-class layout
/// narrowing, and an up-cast that crosses a method-override boundary (see
/// `cast_compatibility`'s doc comment for why the third is unsound too).
#[derive(Debug, PartialEq)]
enum CastMismatch {
    Representation,
    Layout,
    OverriddenMethod(String),
}

/// #767: Type-checks `cast(T, value)` — `typing.cast` is a runtime no-op in
/// CPython whose only effect is to declare `value`'s static type as `T`, so
/// the call's type is `T` and `pycc_mir` lowers the whole expression to
/// `value` alone (no call is emitted, and codegen never sees `cast`).
///
/// `T` must be a bare name: one of the four builtin scalar type names or a
/// class defined in this module. Subscripted generics (`cast(list[int], x)`)
/// and every other type expression are outside the current subset and are
/// rejected with `C0001`, the versioned capability code, rather than a
/// by-design rejection — the same classification `check_isinstance` uses for
/// in-scope-but-unimplemented call shapes.
///
/// `value` is inferred normally so its own errors are still reported.
///
/// Unlike CPython's `typing.cast`, a target whose *runtime representation*
/// differs from the value's own is rejected (D-198). `pycc_mir` elides the
/// whole `cast(...)` call to `value` alone (see the module doc above), so
/// nothing at MIR/codegen time ever applies a representation conversion. If
/// the two representations differed — `cast(str, 5)`, `cast(int, flag)` —
/// the checker would validate the rest of the program against `T` while the
/// emitted code still carried the value's real representation: a codegen
/// panic in a debug build (the `pycc_codegen` local-type-drift guard is a
/// `debug_assert_eq!`) or silently misinterpreted bits in a release one.
/// Per `docs/TYPE_SYSTEM.md`'s representation table each of `int` (`i64`),
/// `float` (`f64`), `bool` (`i8`), `str` (heap pointer) has its own
/// distinct representation, so `bool` -> `int` is a representation change
/// like any other despite `bool` being an `int` subtype in the static type
/// system.
///
/// What remains permitted is a cast to the value's own type, and an
/// up-cast between two class instances (`to` is `from` or one of `from`'s
/// MRO ancestors) that crosses no method-override boundary. A genuine
/// down-cast (`to` is a strict descendant of `from`) is rejected: unlike a
/// representation change, an unsound down-cast cannot be caught by any
/// runtime guard, because MIR erasure (see `cast_compatibility`'s doc
/// comment) never threads `to`'s verified attribute layout through to the
/// object `value` actually names, so `cast`'s single most common legitimate
/// use — narrowing after an `isinstance` check — is deferred to a version
/// that either keeps a runtime class tag or otherwise verifies layout
/// compatibility instead of relying on pure erasure. An up-cast that
/// overrides a method along the way is rejected for the same erasure
/// reason, on the *dispatch* axis rather than the layout axis (see
/// `cast_compatibility`'s doc comment). `cast` does not otherwise verify
/// the nominal relationship between the two — CPython's and mypy's `cast`
/// are deliberately unchecked assertions, and pycc preserves that for the
/// subset it does erase; the checks above are codegen-soundness limits, not
/// a re-introduction of static checking on the assertion itself.
///
/// This check runs only on the validation-pass route (`infer_expr_in`,
/// reached for an annotated function's body and for module-level/`AnnAssign`
/// statements). `constraints.rs`'s solver mirror — reached only for a
/// return-type-inferred private helper — has no resolved `Ty` for the value
/// at that point and does not perform it, mirroring `check_isinstance`'s own
/// documented asymmetry between the two passes.
///
/// Two guards `check_isinstance` carries are deliberately absent here.
/// There is no side-effect guard on the value operand: `isinstance` needs
/// one because it discards its operand and evaluates to a compile-time
/// constant, whereas `cast` *is* its value operand after lowering, so a
/// call expression there is evaluated exactly once, as CPython does. And
/// there is no `@runtime_checkable` gate on a protocol target: `isinstance`
/// needs one because it performs a runtime class test, whereas `cast`
/// performs none — `cast(P, x)` only renames the static type and emits no
/// code, which is exactly what `typing.cast` means for a protocol in
/// CPython too.
pub(crate) fn check_cast(
    env: &Environment,
    local_names: &[&str],
    args: &[HirExpr],
) -> Result<Ty, Diagnostic> {
    let target = cast_target_name(args)?;
    validate_class_name(env, target)?;
    let target_ty = cast_target_ty(target);
    let value_ty = infer_expr_in(env, local_names, &args[1])?;
    if let Err(mismatch) = cast_compatibility(env, &value_ty, &target_ty) {
        let (message, help) = match mismatch {
            CastMismatch::Representation => (
                format!(
                    "`cast({target}, ...)` would change the value's runtime representation \
                     (`{}` to `{}`), which this pycc version does not support",
                    value_ty.name(),
                    target_ty.name()
                ),
                "pycc erases `cast` at compile time and emits no conversion, so the target \
                 type must already share the value's runtime representation: cast to the \
                 value's own type, or to one of its base classes",
            ),
            CastMismatch::Layout => (
                format!(
                    "`cast({target}, ...)` would narrow the value's attribute layout (`{}` \
                     to `{}`), which this pycc version does not support",
                    value_ty.name(),
                    target_ty.name()
                ),
                "pycc erases `cast` at compile time and never re-derives the target's \
                 attribute layout, so the target type must already be a subset of the \
                 value's own: cast to the value's own type, or to one of its base classes",
            ),
            CastMismatch::OverriddenMethod(method) => (
                format!(
                    "`cast({target}, ...)` would let a later call to `{method}` on the \
                     result statically resolve to `{}`'s implementation instead of `{}`'s \
                     override, which this pycc version does not support",
                    target_ty.name(),
                    value_ty.name()
                ),
                "pycc resolves method calls statically from the cast result's declared type, \
                 so casting across a class that overrides a base method is unsound: cast to \
                 the value's own type, or to a base class not separated from the value's class \
                 by any subclass that overrides one of that base class's methods",
            ),
        };
        return Err(Diagnostic::error("C0001", message, Span::new(0, 0)).with_help(help));
    }
    Ok(target_ty)
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
        // #380 (PR-20, PEP 544): `isinstance` against a protocol class is
        // only valid if the protocol is `@runtime_checkable`. A
        // non-runtime-checkable protocol used with `isinstance` is
        // rejected with `C0001`.
        if let Some(class_def) = env.lookup_class(name)
            && class_def.is_protocol
            && !class_def.runtime_checkable
        {
            return Err(Diagnostic::error(
                "C0001",
                format!(
                    "`isinstance` against protocol `{name}` is not valid -- the protocol is \
                     not `@runtime_checkable`; add `@runtime_checkable` to the protocol \
                     class declaration to use it with `isinstance`"
                ),
                Span::new(0, 0),
            ));
        }
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
    // #380 (PR-20, PEP 544): `issubclass` against a protocol class is
    // rejected — protocols use structural typing, not nominal
    // inheritance, so `issubclass` does not apply.
    if let Some(class_def) = env.lookup_class(&cls_name)
        && class_def.is_protocol
    {
        return Err(Diagnostic::error(
            "C0001",
            format!(
                "`issubclass` with protocol `{cls_name}` as the first argument is not valid \
                 -- protocols use structural typing, not nominal inheritance; use \
                 `isinstance` with a `@runtime_checkable` protocol instead"
            ),
            Span::new(0, 0),
        ));
    }
    // Validate each target class name.
    for name in &target_names {
        validate_class_name(env, name)?;
        // #380 (PR-20): `issubclass` against a protocol as the target is
        // also rejected.
        if let Some(class_def) = env.lookup_class(name)
            && class_def.is_protocol
        {
            return Err(Diagnostic::error(
                "C0001",
                format!(
                    "`issubclass` against protocol `{name}` is not valid -- protocols use \
                     structural typing, not nominal inheritance; use `isinstance` with a \
                     `@runtime_checkable` protocol instead"
                ),
                Span::new(0, 0),
            ));
        }
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
            seeded_builtin_exception_classes: false,
            items,
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: vec![(
                "Point".to_string(),
                HirClassDef {
                    class_attrs: Vec::new(),
                    exception_type_tag: None,
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
                    is_enum: false,
                    enum_members: Vec::new(),
                    is_dataclass: false,
                    dataclass_fields: Vec::new(),
                    is_protocol: false,
                    runtime_checkable: false,
                    protocol_members: Vec::new(),
                    abstract_methods: Vec::new(),
                    is_abstract: false,
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
        // (`concrete_function_environment` returns `None`, routing `check`
        // through `infer_function_signatures_with_solver_all` instead of the
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
            seeded_builtin_exception_classes: false,
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
                    class_attrs: Vec::new(),
                    exception_type_tag: None,
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
                    is_enum: false,
                    enum_members: Vec::new(),
                    is_dataclass: false,
                    dataclass_fields: Vec::new(),
                    is_protocol: false,
                    runtime_checkable: false,
                    protocol_members: Vec::new(),
                    abstract_methods: Vec::new(),
                    is_abstract: false,
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
            seeded_builtin_exception_classes: false,
            items: vec![init, bad],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: vec![(
                "Point".to_string(),
                HirClassDef {
                    class_attrs: Vec::new(),
                    exception_type_tag: None,
                    name: "Point".to_string(),
                    bases: Vec::new(),
                    mro: vec!["Point".to_string()],
                    attrs: vec![("x".to_string(), Ty::Int)],
                    methods: vec![("__init__".to_string(), "Point.__init__".to_string())],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    is_enum: false,
                    enum_members: Vec::new(),
                    is_dataclass: false,
                    dataclass_fields: Vec::new(),
                    is_protocol: false,
                    runtime_checkable: false,
                    protocol_members: Vec::new(),
                    abstract_methods: Vec::new(),
                    is_abstract: false,
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
            seeded_builtin_exception_classes: false,
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
            seeded_builtin_exception_classes: false,
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
            seeded_builtin_exception_classes: false,
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
            seeded_builtin_exception_classes: false,
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
        // `check_and_resolve` exercises both: `checked_function_signatures_all`
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
                class_attrs: Vec::new(),
                exception_type_tag: None,
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
                is_enum: false,
                enum_members: Vec::new(),
                is_dataclass: false,
                dataclass_fields: Vec::new(),
                is_protocol: false,
                runtime_checkable: false,
                protocol_members: Vec::new(),
                abstract_methods: Vec::new(),
                is_abstract: false,
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
            seeded_builtin_exception_classes: false,
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
            seeded_builtin_exception_classes: false,
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
                class_attrs: Vec::new(),
                exception_type_tag: None,
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
                is_enum: false,
                enum_members: Vec::new(),
                is_dataclass: false,
                dataclass_fields: Vec::new(),
                is_protocol: false,
                runtime_checkable: false,
                protocol_members: Vec::new(),
                abstract_methods: Vec::new(),
                is_abstract: false,
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
        // `class/method_call.rs`'s
        // `resolve_method_call_panics_when_the_method_is_not_registered`
        // (moved there by #815, Part 1 of #737).
        use pycc_hir::PropertyDef;
        let mut env = crate::Environment::new();
        env.bind_class(
            "Ghost".to_string(),
            HirClassDef {
                class_attrs: Vec::new(),
                exception_type_tag: None,
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
                is_enum: false,
                enum_members: Vec::new(),
                is_dataclass: false,
                dataclass_fields: Vec::new(),
                is_protocol: false,
                runtime_checkable: false,
                protocol_members: Vec::new(),
                abstract_methods: Vec::new(),
                is_abstract: false,
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

    #[test]
    fn cast_compatibility_treats_an_unregistered_from_class_as_a_representation_mismatch() {
        // #767 (D-198, third pass): every `Ty::Instance` `cast_compatibility`
        // sees from a real `check`-validated program names a class the
        // `Environment` was told about -- either an ordinary user class or
        // one of the 23 seeded builtin exception classes (see
        // `crate::exception::is_user_defined_class`'s doc comment). This
        // directly exercises the function's own defensive fallback for the
        // "declared shape and Environment disagree" state that scenario
        // rules out from any real caller, matching the internal-consistency
        // convention above.
        let env = crate::Environment::new();
        let mismatch = super::cast_compatibility(
            &env,
            &Ty::Instance(Box::new("Ghost".to_string())),
            &Ty::Instance(Box::new("AlsoGhost".to_string())),
        )
        .unwrap_err();
        assert_eq!(mismatch, super::CastMismatch::Representation);
    }

    #[test]
    fn cast_compatibility_treats_an_mro_entry_missing_its_own_class_def_as_a_layout_mismatch() {
        // Same "declared shape and Environment disagree" scenario as above,
        // but for the ancestor lookup instead of the value's own class:
        // `from`'s own MRO names a class that isn't registered. Normal class
        // registration (`validate_bases` in `pycc_hir::class::mro`) only
        // ever admits an already-defined class into a base's MRO, so this
        // state cannot arise from any real `check`-validated program either.
        let mut env = crate::Environment::new();
        env.bind_class(
            "Derived".to_string(),
            HirClassDef {
                class_attrs: Vec::new(),
                exception_type_tag: None,
                name: "Derived".to_string(),
                bases: vec!["GhostBase".to_string()],
                mro: vec!["Derived".to_string(), "GhostBase".to_string()],
                attrs: vec![],
                methods: vec![],
                type_param: None,
                properties: Vec::new(),
                static_methods: Vec::new(),
                class_methods: Vec::new(),
                is_enum: false,
                enum_members: Vec::new(),
                is_dataclass: false,
                dataclass_fields: Vec::new(),
                is_protocol: false,
                runtime_checkable: false,
                protocol_members: Vec::new(),
                abstract_methods: Vec::new(),
                is_abstract: false,
            },
        );
        let mismatch = super::cast_compatibility(
            &env,
            &Ty::Instance(Box::new("Derived".to_string())),
            &Ty::Instance(Box::new("GhostBase".to_string())),
        )
        .unwrap_err();
        assert_eq!(mismatch, super::CastMismatch::Layout);
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
                    class_attrs: Vec::new(),
                    exception_type_tag: None,
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
                    is_enum: false,
                    enum_members: Vec::new(),
                    is_dataclass: false,
                    dataclass_fields: Vec::new(),
                    is_protocol: false,
                    runtime_checkable: false,
                    protocol_members: Vec::new(),
                    abstract_methods: Vec::new(),
                    is_abstract: false,
                },
            );
            env.bind_class(
                "B".to_string(),
                HirClassDef {
                    class_attrs: Vec::new(),
                    exception_type_tag: None,
                    name: "B".to_string(),
                    bases: vec!["A".to_string()],
                    mro: vec!["B".to_string(), "A".to_string()],
                    attrs: vec![],
                    methods: vec![("__init__".to_string(), "B.__init__".to_string())],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    is_enum: false,
                    enum_members: Vec::new(),
                    is_dataclass: false,
                    dataclass_fields: Vec::new(),
                    is_protocol: false,
                    runtime_checkable: false,
                    protocol_members: Vec::new(),
                    abstract_methods: Vec::new(),
                    is_abstract: false,
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
    fn resolve_super_attr_get_rejects_instance_attr() {
        // #587: `super().x` where `x` is an instance attribute declared by
        // a base class's `__init__`. CPython raises `AttributeError` here
        // because a `super` object does not proxy the instance's own
        // attributes, so pycc rejects it with `T0047` rather than
        // resolving it against `self`'s slot. This one test replaces the
        // three the old resolve-against-`self` behaviour needed (the
        // success path plus the `T0021` shared-slot redeclaration guard,
        // whose condition is unreachable once the form itself is gone).
        let env = super_env(|env| {
            env.bind_class(
                "A".to_string(),
                HirClassDef {
                    class_attrs: Vec::new(),
                    exception_type_tag: None,
                    name: "A".to_string(),
                    bases: vec![],
                    mro: vec!["A".to_string()],
                    attrs: vec![("x".to_string(), Ty::Int)],
                    methods: vec![("__init__".to_string(), "A.__init__".to_string())],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    is_enum: false,
                    enum_members: Vec::new(),
                    is_dataclass: false,
                    dataclass_fields: Vec::new(),
                    is_protocol: false,
                    runtime_checkable: false,
                    protocol_members: Vec::new(),
                    abstract_methods: Vec::new(),
                    is_abstract: false,
                },
            );
            env.bind_class(
                "B".to_string(),
                HirClassDef {
                    class_attrs: Vec::new(),
                    exception_type_tag: None,
                    name: "B".to_string(),
                    bases: vec!["A".to_string()],
                    mro: vec!["B".to_string(), "A".to_string()],
                    attrs: vec![],
                    methods: vec![("__init__".to_string(), "B.__init__".to_string())],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    is_enum: false,
                    enum_members: Vec::new(),
                    is_dataclass: false,
                    dataclass_fields: Vec::new(),
                    is_protocol: false,
                    runtime_checkable: false,
                    protocol_members: Vec::new(),
                    abstract_methods: Vec::new(),
                    is_abstract: false,
                },
            );
        });
        let err = super::resolve_super_attr_get(&env, "x").unwrap_err();
        assert_eq!(err.code, "T0047");
        assert!(
            err.message.contains("instance attribute of class `A`"),
            "T0047 should name the declaring class, got: {}",
            err.message
        );
        assert_eq!(
            err.help.as_deref(),
            Some("read it through `self` instead: `self.x`"),
            "T0047 should point at the equivalent `self` read"
        );
    }

    #[test]
    fn resolve_super_method_call_returns_return_type() {
        let env = super_env(|env| {
            env.bind_class(
                "A".to_string(),
                HirClassDef {
                    class_attrs: Vec::new(),
                    exception_type_tag: None,
                    name: "A".to_string(),
                    bases: vec![],
                    mro: vec!["A".to_string()],
                    attrs: vec![],
                    methods: vec![("greet".to_string(), "A.greet".to_string())],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    is_enum: false,
                    enum_members: Vec::new(),
                    is_dataclass: false,
                    dataclass_fields: Vec::new(),
                    is_protocol: false,
                    runtime_checkable: false,
                    protocol_members: Vec::new(),
                    abstract_methods: Vec::new(),
                    is_abstract: false,
                },
            );
            env.bind_class(
                "B".to_string(),
                HirClassDef {
                    class_attrs: Vec::new(),
                    exception_type_tag: None,
                    name: "B".to_string(),
                    bases: vec!["A".to_string()],
                    mro: vec!["B".to_string(), "A".to_string()],
                    attrs: vec![],
                    methods: vec![("__init__".to_string(), "B.__init__".to_string())],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    is_enum: false,
                    enum_members: Vec::new(),
                    is_dataclass: false,
                    dataclass_fields: Vec::new(),
                    is_protocol: false,
                    runtime_checkable: false,
                    protocol_members: Vec::new(),
                    abstract_methods: Vec::new(),
                    is_abstract: false,
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
                    class_attrs: Vec::new(),
                    exception_type_tag: None,
                    name: "A".to_string(),
                    bases: vec![],
                    mro: vec!["A".to_string()],
                    attrs: vec![],
                    methods: vec![("__init__".to_string(), "A.__init__".to_string())],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    is_enum: false,
                    enum_members: Vec::new(),
                    is_dataclass: false,
                    dataclass_fields: Vec::new(),
                    is_protocol: false,
                    runtime_checkable: false,
                    protocol_members: Vec::new(),
                    abstract_methods: Vec::new(),
                    is_abstract: false,
                },
            );
            env.bind_class(
                "B".to_string(),
                HirClassDef {
                    class_attrs: Vec::new(),
                    exception_type_tag: None,
                    name: "B".to_string(),
                    bases: vec!["A".to_string()],
                    mro: vec!["B".to_string(), "A".to_string()],
                    attrs: vec![],
                    methods: vec![("__init__".to_string(), "B.__init__".to_string())],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    is_enum: false,
                    enum_members: Vec::new(),
                    is_dataclass: false,
                    dataclass_fields: Vec::new(),
                    is_protocol: false,
                    runtime_checkable: false,
                    protocol_members: Vec::new(),
                    abstract_methods: Vec::new(),
                    is_abstract: false,
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
                    class_attrs: Vec::new(),
                    exception_type_tag: None,
                    name: "A".to_string(),
                    bases: vec![],
                    mro: vec!["A".to_string()],
                    attrs: vec![("x".to_string(), Ty::Int)],
                    methods: vec![("__init__".to_string(), "A.__init__".to_string())],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    is_enum: false,
                    enum_members: Vec::new(),
                    is_dataclass: false,
                    dataclass_fields: Vec::new(),
                    is_protocol: false,
                    runtime_checkable: false,
                    protocol_members: Vec::new(),
                    abstract_methods: Vec::new(),
                    is_abstract: false,
                },
            );
            env.bind_class(
                "B".to_string(),
                HirClassDef {
                    class_attrs: Vec::new(),
                    exception_type_tag: None,
                    name: "B".to_string(),
                    bases: vec!["A".to_string()],
                    mro: vec!["B".to_string(), "A".to_string()],
                    attrs: vec![],
                    methods: vec![("__init__".to_string(), "B.__init__".to_string())],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    is_enum: false,
                    enum_members: Vec::new(),
                    is_dataclass: false,
                    dataclass_fields: Vec::new(),
                    is_protocol: false,
                    runtime_checkable: false,
                    protocol_members: Vec::new(),
                    abstract_methods: Vec::new(),
                    is_abstract: false,
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
            seeded_builtin_exception_classes: false,
            items,
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: vec![(
                "Box".to_string(),
                HirClassDef {
                    class_attrs: Vec::new(),
                    exception_type_tag: None,
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
                    is_enum: false,
                    enum_members: Vec::new(),
                    is_dataclass: false,
                    dataclass_fields: Vec::new(),
                    is_protocol: false,
                    runtime_checkable: false,
                    protocol_members: Vec::new(),
                    abstract_methods: Vec::new(),
                    is_abstract: false,
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
            seeded_builtin_exception_classes: false,
            items,
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: vec![(
                "Box".to_string(),
                HirClassDef {
                    class_attrs: Vec::new(),
                    exception_type_tag: None,
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
                    is_enum: false,
                    enum_members: Vec::new(),
                    is_dataclass: false,
                    dataclass_fields: Vec::new(),
                    is_protocol: false,
                    runtime_checkable: false,
                    protocol_members: Vec::new(),
                    abstract_methods: Vec::new(),
                    is_abstract: false,
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
            seeded_builtin_exception_classes: false,
            items,
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: vec![
                (
                    "Animal".to_string(),
                    HirClassDef {
                        class_attrs: Vec::new(),
                        exception_type_tag: None,
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
                        is_enum: false,
                        enum_members: Vec::new(),
                        is_dataclass: false,
                        dataclass_fields: Vec::new(),
                        is_protocol: false,
                        runtime_checkable: false,
                        protocol_members: Vec::new(),
                        abstract_methods: Vec::new(),
                        is_abstract: false,
                    },
                ),
                (
                    "Dog".to_string(),
                    HirClassDef {
                        class_attrs: Vec::new(),
                        exception_type_tag: None,
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
                        is_enum: false,
                        enum_members: Vec::new(),
                        is_dataclass: false,
                        dataclass_fields: Vec::new(),
                        is_protocol: false,
                        runtime_checkable: false,
                        protocol_members: Vec::new(),
                        abstract_methods: Vec::new(),
                        is_abstract: false,
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
            seeded_builtin_exception_classes: false,
            items,
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: vec![
                (
                    "Base".to_string(),
                    HirClassDef {
                        class_attrs: Vec::new(),
                        exception_type_tag: None,
                        name: "Base".to_string(),
                        bases: Vec::new(),
                        mro: vec!["Base".to_string()],
                        attrs: vec![("x".to_string(), Ty::Int)],
                        methods: vec![("__init__".to_string(), "Base.__init__".to_string())],
                        type_param: None,
                        properties: Vec::new(),
                        static_methods: Vec::new(),
                        class_methods: Vec::new(),
                        is_enum: false,
                        enum_members: Vec::new(),
                        is_dataclass: false,
                        dataclass_fields: Vec::new(),
                        is_protocol: false,
                        runtime_checkable: false,
                        protocol_members: Vec::new(),
                        abstract_methods: Vec::new(),
                        is_abstract: false,
                    },
                ),
                (
                    "Derived".to_string(),
                    HirClassDef {
                        class_attrs: Vec::new(),
                        exception_type_tag: None,
                        name: "Derived".to_string(),
                        bases: vec!["Base".to_string()],
                        mro: vec!["Derived".to_string(), "Base".to_string()],
                        attrs: Vec::new(),
                        methods: Vec::new(),
                        type_param: None,
                        properties: Vec::new(),
                        static_methods: Vec::new(),
                        class_methods: Vec::new(),
                        is_enum: false,
                        enum_members: Vec::new(),
                        is_dataclass: false,
                        dataclass_fields: Vec::new(),
                        is_protocol: false,
                        runtime_checkable: false,
                        protocol_members: Vec::new(),
                        abstract_methods: Vec::new(),
                        is_abstract: false,
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
            seeded_builtin_exception_classes: false,
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
                        class_attrs: Vec::new(),
                        exception_type_tag: None,
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
                        is_enum: false,
                        enum_members: Vec::new(),
                        is_dataclass: false,
                        dataclass_fields: Vec::new(),
                        is_protocol: false,
                        runtime_checkable: false,
                        protocol_members: Vec::new(),
                        abstract_methods: Vec::new(),
                        is_abstract: false,
                    },
                ),
                (
                    "Derived".to_string(),
                    HirClassDef {
                        class_attrs: Vec::new(),
                        exception_type_tag: None,
                        name: "Derived".to_string(),
                        bases: vec!["Base".to_string()],
                        mro: vec!["Derived".to_string(), "Base".to_string()],
                        attrs: Vec::new(),
                        methods: Vec::new(),
                        type_param: None,
                        properties: Vec::new(),
                        static_methods: Vec::new(),
                        class_methods: Vec::new(),
                        is_enum: false,
                        enum_members: Vec::new(),
                        is_dataclass: false,
                        dataclass_fields: Vec::new(),
                        is_protocol: false,
                        runtime_checkable: false,
                        protocol_members: Vec::new(),
                        abstract_methods: Vec::new(),
                        is_abstract: false,
                    },
                ),
            ],
        };
        let resolved = check_and_resolve(&hir).expect("check_and_resolve should succeed");
        let _mir = pycc_mir::build(&resolved);
    }

    // PEP 572 (#774): `pycc_mir::expr::pre_bind_named_expr_targets` is a
    // function private to `pycc_mir` (`pub(super)`), reachable from outside
    // that crate only indirectly through `pycc_mir::build`. Its `BinOp`/
    // `Compare`/`UnaryOp`/`FString` arms (a walrus nested inside one of
    // those, at any depth within an `ExprStmt`) are already exercised
    // through `pycc_mir`'s own hand-built-HIR test suite (see that crate's
    // `walrus_in_an_if_test_binds_the_name_for_the_body` and neighbors) and
    // through `pycc_types::tests::check_source`'s
    // `a_walrus_nested_inside_every_remaining_container_shape_binds_every_name_at_module_scope`,
    // but every one of those calls a *different* crate's own compiled copy
    // of `pycc_mir`: this crate's own `[dev-dependencies]` link on
    // `pycc_mir` is measured as its own separate coverage instantiation,
    // independent of `pycc_mir`'s own `cfg(test)` binary. Only this
    // module's handful of `pycc_mir::build(&resolved)` calls (see the
    // property/inheritance tests above) exercise *this* crate's copy of the
    // HIR→MIR pipeline at all, and none of them contain a walrus, so this
    // test closes that gap directly.
    #[test]
    fn a_walrus_nested_inside_binop_compare_unaryop_and_fstring_binds_every_name_via_mir_build() {
        let hir = HirModule {
            seeded_builtin_exception_classes: false,
            items: vec![
                top_level(HirStmt::ExprStmt(HirExpr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(HirExpr::NamedExpr {
                        name: "bo_l".to_string(),
                        value: Box::new(HirExpr::IntLiteral(1)),
                    }),
                    right: Box::new(HirExpr::NamedExpr {
                        name: "bo_r".to_string(),
                        value: Box::new(HirExpr::IntLiteral(2)),
                    }),
                })),
                top_level(HirStmt::ExprStmt(HirExpr::Compare {
                    op: pycc_hir::CmpOpKind::Lt,
                    left: Box::new(HirExpr::NamedExpr {
                        name: "cmp_l".to_string(),
                        value: Box::new(HirExpr::IntLiteral(1)),
                    }),
                    right: Box::new(HirExpr::NamedExpr {
                        name: "cmp_r".to_string(),
                        value: Box::new(HirExpr::IntLiteral(2)),
                    }),
                })),
                top_level(HirStmt::ExprStmt(HirExpr::UnaryOp {
                    op: pycc_hir::UnaryOpKind::USub,
                    operand: Box::new(HirExpr::NamedExpr {
                        name: "uop".to_string(),
                        value: Box::new(HirExpr::IntLiteral(3)),
                    }),
                })),
                top_level(HirStmt::ExprStmt(HirExpr::FString(vec![
                    pycc_hir::FStringPart::Literal("x=".to_string()),
                    pycc_hir::FStringPart::Interpolation(Box::new(HirExpr::NamedExpr {
                        name: "fsp".to_string(),
                        value: Box::new(HirExpr::IntLiteral(4)),
                    })),
                ]))),
                top_level(HirStmt::Assign {
                    target: "readback".to_string(),
                    value: HirExpr::Name("fsp".to_string()),
                }),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: vec![],
        };
        let resolved = check_and_resolve(&hir).expect("check_and_resolve should succeed");
        let mir = pycc_mir::build(&resolved);
        // Proof every name was actually bound (not just that lowering
        // didn't panic): the trailing `Assign` above reads `fsp` back by
        // name, which would panic looking it up in `scopes` if
        // `pre_bind_named_expr_targets` had skipped the `FString`
        // interpolation arm.
        assert_eq!(
            mir.items[4],
            pycc_mir::MirItem::TopLevelStmt(pycc_mir::MirStmt::Assign {
                target: "readback".to_string(),
                value: pycc_mir::MirExpr::Name {
                    name: "fsp".to_string(),
                    ty: Ty::Int,
                },
            })
        );
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
            seeded_builtin_exception_classes: false,
            items,
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: vec![(
                "C".to_string(),
                HirClassDef {
                    class_attrs: Vec::new(),
                    exception_type_tag: None,
                    name: "C".to_string(),
                    bases: Vec::new(),
                    mro: vec!["C".to_string()],
                    attrs: Vec::new(),
                    methods: vec![("__init__".to_string(), "C.__init__".to_string())],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: vec![("create".to_string(), "C.create.static".to_string())],
                    class_methods: vec![("greet".to_string(), "C.greet.classmethod".to_string())],
                    is_enum: false,
                    enum_members: Vec::new(),
                    is_dataclass: false,
                    dataclass_fields: Vec::new(),
                    is_protocol: false,
                    runtime_checkable: false,
                    protocol_members: Vec::new(),
                    abstract_methods: Vec::new(),
                    is_abstract: false,
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
                class_attrs: Vec::new(),
                exception_type_tag: None,
                name: "C".to_string(),
                bases: Vec::new(),
                mro: vec!["C".to_string()],
                attrs: Vec::new(),
                methods: vec![("__init__".to_string(), "C.__init__".to_string())],
                type_param: None,
                properties: Vec::new(),
                static_methods: Vec::new(),
                class_methods: Vec::new(),
                is_enum: false,
                enum_members: Vec::new(),
                is_dataclass: false,
                dataclass_fields: Vec::new(),
                is_protocol: false,
                runtime_checkable: false,
                protocol_members: Vec::new(),
                abstract_methods: Vec::new(),
                is_abstract: false,
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
                class_attrs: Vec::new(),
                exception_type_tag: None,
                name: "C".to_string(),
                bases: Vec::new(),
                mro: vec!["C".to_string()],
                attrs: Vec::new(),
                methods: vec![("__init__".to_string(), "C.__init__".to_string())],
                type_param: None,
                properties: Vec::new(),
                static_methods: vec![("create".to_string(), "C.create.static".to_string())],
                class_methods: vec![("greet".to_string(), "C.greet.classmethod".to_string())],
                is_enum: false,
                enum_members: Vec::new(),
                is_dataclass: false,
                dataclass_fields: Vec::new(),
                is_protocol: false,
                runtime_checkable: false,
                protocol_members: Vec::new(),
                abstract_methods: Vec::new(),
                is_abstract: false,
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
                class_attrs: Vec::new(),
                exception_type_tag: None,
                name: "Derived".to_string(),
                bases: vec!["Ghost".to_string()],
                mro: vec!["Derived".to_string(), "Ghost".to_string()],
                attrs: Vec::new(),
                methods: vec![("__init__".to_string(), "Derived.__init__".to_string())],
                type_param: None,
                properties: Vec::new(),
                static_methods: Vec::new(),
                class_methods: Vec::new(),
                is_enum: false,
                enum_members: Vec::new(),
                is_dataclass: false,
                dataclass_fields: Vec::new(),
                is_protocol: false,
                runtime_checkable: false,
                protocol_members: Vec::new(),
                abstract_methods: Vec::new(),
                is_abstract: false,
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
                class_attrs: Vec::new(),
                exception_type_tag: None,
                name: "C".to_string(),
                bases: Vec::new(),
                mro: vec!["C".to_string()],
                attrs: Vec::new(),
                methods: vec![("__init__".to_string(), "C.__init__".to_string())],
                type_param: None,
                properties: Vec::new(),
                static_methods: vec![("create".to_string(), "C.create.static".to_string())],
                class_methods: Vec::new(),
                is_enum: false,
                enum_members: Vec::new(),
                is_dataclass: false,
                dataclass_fields: Vec::new(),
                is_protocol: false,
                runtime_checkable: false,
                protocol_members: Vec::new(),
                abstract_methods: Vec::new(),
                is_abstract: false,
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
                class_attrs: Vec::new(),
                exception_type_tag: None,
                name: "C".to_string(),
                bases: Vec::new(),
                mro: vec!["C".to_string()],
                attrs: Vec::new(),
                methods: vec![("__init__".to_string(), "C.__init__".to_string())],
                type_param: None,
                properties: Vec::new(),
                static_methods: Vec::new(),
                class_methods: vec![("greet".to_string(), "C.greet.classmethod".to_string())],
                is_enum: false,
                enum_members: Vec::new(),
                is_dataclass: false,
                dataclass_fields: Vec::new(),
                is_protocol: false,
                runtime_checkable: false,
                protocol_members: Vec::new(),
                abstract_methods: Vec::new(),
                is_abstract: false,
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

    // -- #378 (PR-18): dataclass __eq__ comparison type checking ---------

    #[test]
    fn a_same_class_instance_comparison_without_eq_is_t0021() {
        // `Point` has `__init__` and `bump` but no `__eq__` method.
        // Comparing two `Point` instances with `==` should fall through
        // the `has_eq` check and produce T0021 (cannot compare instances).
        // This covers the `}` (merge point) of the `if has_eq` block in
        // `infer_expr_in`'s `Compare` arm.
        let hir = point_module(vec![
            top_level(HirStmt::Assign {
                target: "p".to_string(),
                value: HirExpr::Call {
                    callee: "Point".to_string(),
                    args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
                },
            }),
            top_level(HirStmt::Assign {
                target: "q".to_string(),
                value: HirExpr::Call {
                    callee: "Point".to_string(),
                    args: vec![HirExpr::IntLiteral(3), HirExpr::IntLiteral(4)],
                },
            }),
            top_level(HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::Compare {
                    op: pycc_hir::CmpOpKind::Eq,
                    left: Box::new(HirExpr::Name("p".to_string())),
                    right: Box::new(HirExpr::Name("q".to_string())),
                }],
            })),
        ]);
        let diagnostic = check(&hir).unwrap_err();
        assert_eq!(diagnostic.code, "T0021");
    }

    #[test]
    fn a_same_class_dataclass_instance_comparison_with_eq_type_checks() {
        // A dataclass class with a compiler-synthesized `__eq__` method
        // accepts `==` between same-class instances. This covers the
        // `return Ok(Ty::Bool)` path in the dataclass comparison check.
        // Non-dataclass classes with a user-defined `__eq__` are rejected
        // (the MIR rewrite assumes the synthesized signature).
        let self_ty = Ty::Instance(Box::new("EqPoint".to_string()));
        let init = HirItem::Function {
            name: "EqPoint.__init__".to_string(),
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
        let eq = HirItem::Function {
            name: "EqPoint.__eq__".to_string(),
            params: vec![
                ("self".to_string(), self_ty.clone()),
                ("other".to_string(), self_ty.clone()),
            ],
            return_ty: Ty::Bool,
            body: vec![HirStmt::Return(Some(HirExpr::BoolLiteral(true)))],
        };
        let hir = HirModule {
            seeded_builtin_exception_classes: false,
            items: vec![
                init,
                eq,
                top_level(HirStmt::Assign {
                    target: "p".to_string(),
                    value: HirExpr::Call {
                        callee: "EqPoint".to_string(),
                        args: vec![HirExpr::IntLiteral(1)],
                    },
                }),
                top_level(HirStmt::Assign {
                    target: "q".to_string(),
                    value: HirExpr::Call {
                        callee: "EqPoint".to_string(),
                        args: vec![HirExpr::IntLiteral(2)],
                    },
                }),
                top_level(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::Compare {
                        op: pycc_hir::CmpOpKind::Eq,
                        left: Box::new(HirExpr::Name("p".to_string())),
                        right: Box::new(HirExpr::Name("q".to_string())),
                    }],
                })),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: vec![(
                "EqPoint".to_string(),
                pycc_hir::HirClassDef {
                    class_attrs: Vec::new(),
                    exception_type_tag: None,
                    name: "EqPoint".to_string(),
                    bases: Vec::new(),
                    mro: vec!["EqPoint".to_string()],
                    attrs: vec![("x".to_string(), Ty::Int)],
                    methods: vec![
                        ("__init__".to_string(), "EqPoint.__init__".to_string()),
                        ("__eq__".to_string(), "EqPoint.__eq__".to_string()),
                    ],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    is_enum: false,
                    enum_members: Vec::new(),
                    is_dataclass: true,
                    dataclass_fields: vec![("x".to_string(), Ty::Int)],
                    is_protocol: false,
                    runtime_checkable: false,
                    protocol_members: Vec::new(),
                    abstract_methods: Vec::new(),
                    is_abstract: false,
                },
            )],
        };
        check(&hir).expect("a same-class dataclass comparison with __eq__ should type-check");
    }

    #[test]
    fn a_non_dataclass_instance_comparison_with_user_eq_is_t0021() {
        // A non-dataclass class with a user-defined `__eq__` method does
        // NOT accept `==` between same-class instances -- the MIR rewrite
        // for `__eq__` is restricted to dataclass classes (whose
        // synthesized `__eq__` has a known-correct signature), so the type
        // checker must reject this to prevent a codegen panic.
        let self_ty = Ty::Instance(Box::new("PlainEq".to_string()));
        let init = HirItem::Function {
            name: "PlainEq.__init__".to_string(),
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
        let eq = HirItem::Function {
            name: "PlainEq.__eq__".to_string(),
            params: vec![
                ("self".to_string(), self_ty.clone()),
                ("other".to_string(), self_ty.clone()),
            ],
            return_ty: Ty::Bool,
            body: vec![HirStmt::Return(Some(HirExpr::BoolLiteral(true)))],
        };
        let hir = HirModule {
            seeded_builtin_exception_classes: false,
            items: vec![
                init,
                eq,
                top_level(HirStmt::Assign {
                    target: "p".to_string(),
                    value: HirExpr::Call {
                        callee: "PlainEq".to_string(),
                        args: vec![HirExpr::IntLiteral(1)],
                    },
                }),
                top_level(HirStmt::Assign {
                    target: "q".to_string(),
                    value: HirExpr::Call {
                        callee: "PlainEq".to_string(),
                        args: vec![HirExpr::IntLiteral(2)],
                    },
                }),
                top_level(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::Compare {
                        op: pycc_hir::CmpOpKind::Eq,
                        left: Box::new(HirExpr::Name("p".to_string())),
                        right: Box::new(HirExpr::Name("q".to_string())),
                    }],
                })),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: vec![(
                "PlainEq".to_string(),
                pycc_hir::HirClassDef {
                    class_attrs: Vec::new(),
                    exception_type_tag: None,
                    name: "PlainEq".to_string(),
                    bases: Vec::new(),
                    mro: vec!["PlainEq".to_string()],
                    attrs: vec![("x".to_string(), Ty::Int)],
                    methods: vec![
                        ("__init__".to_string(), "PlainEq.__init__".to_string()),
                        ("__eq__".to_string(), "PlainEq.__eq__".to_string()),
                    ],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    is_enum: false,
                    enum_members: Vec::new(),
                    is_dataclass: false,
                    dataclass_fields: Vec::new(),
                    is_protocol: false,
                    runtime_checkable: false,
                    protocol_members: Vec::new(),
                    abstract_methods: Vec::new(),
                    is_abstract: false,
                },
            )],
        };
        let diagnostic = check(&hir).unwrap_err();
        assert_eq!(diagnostic.code, "T0021");
    }

    // -- PEP 544 (#380): Protocol conformance checking --------------------

    /// Parses and lowers source code, then type-checks it with
    /// `check_and_resolve`. Returns the resolved HIR on success or the
    /// diagnostic on failure.
    fn check_source(source: &str) -> Result<pycc_hir::HirModule, pycc_diag::Diagnostic> {
        let module = pycc_parser::parse(source).expect("test fixture must parse");
        let hir = pycc_hir::lower_checked(&module).expect("test fixture must lower");
        check_and_resolve(&hir)
    }

    #[test]
    fn protocol_conformance_with_wrong_parameter_count_is_t0046() {
        let err = check_source(
            "from typing import Protocol\nclass P(Protocol):\n    def foo(self, x: int) -> int: ...\nclass C:\n    def __init__(self) -> None:\n        self.x = 0\n    def foo(self) -> int:\n        return 1\nc: P = C()\n",
        )
        .unwrap_err();
        assert_eq!(err.code, "T0046");
        assert!(
            err.message.contains("parameter"),
            "unexpected message: {}",
            err.message
        );
    }

    #[test]
    fn protocol_conformance_with_wrong_parameter_type_is_t0046() {
        let err = check_source(
            "from typing import Protocol\nclass P(Protocol):\n    def foo(self, x: int) -> int: ...\nclass C:\n    def __init__(self) -> None:\n        self.x = 0\n    def foo(self, x: str) -> int:\n        return 1\nc: P = C()\n",
        )
        .unwrap_err();
        assert_eq!(err.code, "T0046");
        assert!(
            err.message.contains("parameter 1 has type"),
            "unexpected message: {}",
            err.message
        );
    }

    #[test]
    fn protocol_conformance_with_wrong_return_type_is_t0046() {
        let err = check_source(
            "from typing import Protocol\nclass P(Protocol):\n    def foo(self) -> int: ...\nclass C:\n    def __init__(self) -> None:\n        self.x = 0\n    def foo(self) -> str:\n        return \"hi\"\nc: P = C()\n",
        )
        .unwrap_err();
        assert_eq!(err.code, "T0046");
        assert!(
            err.message.contains("return type is"),
            "unexpected message: {}",
            err.message
        );
    }

    #[test]
    fn protocol_conformance_with_missing_attribute_is_t0046() {
        let err = check_source(
            "from typing import Protocol\nclass P(Protocol):\n    x: int\nclass C:\n    def __init__(self) -> None:\n        self.y = 0\nc: P = C()\n",
        )
        .unwrap_err();
        assert_eq!(err.code, "T0046");
        assert!(
            err.message.contains("missing attribute"),
            "unexpected message: {}",
            err.message
        );
    }

    #[test]
    fn protocol_conformance_with_wrong_attribute_type_is_t0046() {
        let err = check_source(
            "from typing import Protocol\nclass P(Protocol):\n    x: int\nclass C:\n    def __init__(self) -> None:\n        self.x = \"hi\"\nc: P = C()\n",
        )
        .unwrap_err();
        assert_eq!(err.code, "T0046");
        assert!(
            err.message.contains("attribute `x` has type"),
            "unexpected message: {}",
            err.message
        );
    }

    #[test]
    fn protocol_conformance_with_contravariant_param_narrowing_is_t0046() {
        let err = check_source(
            "from typing import Protocol\nclass P(Protocol):\n    def foo(self, x: int) -> int: ...\nclass C:\n    def __init__(self) -> None:\n        self.x = 0\n    def foo(self, x: bool) -> int:\n        return 1\nc: P = C()\n",
        )
        .unwrap_err();
        assert_eq!(err.code, "T0046");
        assert!(
            err.message.contains("parameter 1 has type"),
            "unexpected message: {}",
            err.message
        );
    }

    #[test]
    fn protocol_conformance_with_matching_method_succeeds() {
        let result = check_source(
            "from typing import Protocol\nclass P(Protocol):\n    def foo(self, x: int) -> int: ...\nclass C:\n    def __init__(self) -> None:\n        self.x = 0\n    def foo(self, x: int) -> int:\n        return x\nc: P = C()\n",
        );
        assert!(result.is_ok(), "conforming class should type-check");
    }

    #[test]
    fn protocol_conformance_with_matching_attribute_succeeds() {
        let result = check_source(
            "from typing import Protocol\nclass P(Protocol):\n    x: int\nclass C:\n    def __init__(self) -> None:\n        self.x = 0\nc: P = C()\n",
        );
        assert!(result.is_ok(), "conforming class should type-check");
    }

    #[test]
    fn protocol_to_protocol_assignability_with_different_protocols_is_t0046() {
        let err = check_source(
            "from typing import Protocol\nclass P(Protocol):\n    def foo(self) -> int: ...\nclass Q(Protocol):\n    def bar(self) -> int: ...\nclass C:\n    def __init__(self) -> None:\n        self.x = 0\n    def foo(self) -> int:\n        return 1\nc: P = C()\nq: Q = c\n",
        )
        .unwrap_err();
        assert_eq!(err.code, "T0046");
    }

    #[test]
    fn protocol_cannot_be_instantiated() {
        let err = check_source(
            "from typing import Protocol\nclass P(Protocol):\n    def foo(self) -> int: ...\np = P()\n",
        )
        .unwrap_err();
        assert_eq!(err.code, "C0001");
        assert!(
            err.message.contains("cannot instantiate protocol"),
            "unexpected message: {}",
            err.message
        );
    }

    #[test]
    fn abstract_class_cannot_be_instantiated() {
        // #380 (PR-20): exercises `resolve_instantiation`'s
        // `is_abstract` error path. Covered here as a unit test to
        // avoid cargo-llvm-cov issue #276.
        let err = check_source(
            "from abc import ABC, abstractmethod\nclass A(ABC):\n    def __init__(self) -> None:\n        self.x = 0\n    @abstractmethod\n    def foo(self) -> int: ...\na = A()\n",
        )
        .unwrap_err();
        assert_eq!(err.code, "C0001");
        assert!(
            err.message.contains("cannot instantiate abstract class"),
            "unexpected message: {}",
            err.message
        );
    }

    #[test]
    fn protocol_typed_variable_unknown_attribute_is_t0044() {
        let err = check_source(
            "from typing import Protocol\nclass P(Protocol):\n    def foo(self) -> int: ...\nclass C:\n    def __init__(self) -> None:\n        self.x = 0\n    def foo(self) -> int:\n        return 1\nc: P = C()\nprint(c.bar)\n",
        )
        .unwrap_err();
        assert_eq!(err.code, "T0044");
    }

    #[test]
    fn protocol_typed_variable_unknown_method_is_t0044() {
        let err = check_source(
            "from typing import Protocol\nclass P(Protocol):\n    def foo(self) -> int: ...\nclass C:\n    def __init__(self) -> None:\n        self.x = 0\n    def foo(self) -> int:\n        return 1\nc: P = C()\nc.bar()\n",
        )
        .unwrap_err();
        assert_eq!(err.code, "T0044");
    }

    #[test]
    fn protocol_typed_variable_method_call_wrong_arity_is_t0023() {
        // #380 (PR-20): exercises `resolve_method_call`'s `check_call_args`
        // error path for a protocol-typed variable — calling a protocol
        // method with the wrong number of arguments. Covered here as a
        // unit test to avoid cargo-llvm-cov issue #276.
        //
        // NOTE: A top-level `c: P = C()` binds `c` as the concrete
        // `Ty::Instance("C")` (D-040 sticky representation), so the call
        // resolves through the instance-method path, not the protocol
        // path.  To exercise the *protocol* branch of
        // `class::method_call::resolve_method_call` (moved there by
        // #815, Part 1 of #737), the receiver must be a function
        // parameter annotated `x: P`, which is bound as
        // `Ty::Protocol("P")` in the function body.
        let err = check_source(
            "from typing import Protocol\nclass P(Protocol):\n    def foo(self) -> int: ...\nclass C:\n    def __init__(self) -> None:\n        self.x = 0\n    def foo(self) -> int:\n        return 1\ndef f(x: P) -> int:\n    return x.foo(99)\n",
        )
        .unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn protocol_typed_param_method_call_wrong_arg_type_is_t0021() {
        // #380 (PR-20): exercises the `check_call_args` *type-mismatch*
        // error path (not just arity) inside
        // `class::method_call::resolve_method_call`'s protocol branch
        // (moved there by #815, Part 1 of #737).  The receiver is a
        // function parameter typed `x: P`, so it is bound as
        // `Ty::Protocol("P")` and the protocol branch is entered.  The
        // argument count matches
        // (1 == 1) but the type (`str` vs `int`) does not, so
        // `check_call_args` returns a type-mismatch T0021.
        let err = check_source(
            "from typing import Protocol\nclass P(Protocol):\n    def foo(self, x: int) -> int: ...\nclass C:\n    def __init__(self) -> None:\n        self.x = 0\n    def foo(self, x: int) -> int:\n        return x\ndef f(x: P) -> int:\n    return x.foo(\"hello\")\n",
        )
        .unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(
            err.message.contains("argument 1"),
            "unexpected message: {}",
            err.message
        );
    }

    #[test]
    fn isinstance_against_non_runtime_checkable_protocol_is_c0001() {
        let err = check_source(
            "from typing import Protocol\nclass P(Protocol):\n    def foo(self) -> int: ...\nclass C:\n    def __init__(self) -> None:\n        self.x = 0\n    def foo(self) -> int:\n        return 1\nc = C()\nprint(isinstance(c, P))\n",
        )
        .unwrap_err();
        assert_eq!(err.code, "C0001");
        assert!(
            err.message.contains("not `@runtime_checkable`"),
            "unexpected message: {}",
            err.message
        );
    }

    #[test]
    fn issubclass_with_protocol_first_arg_is_c0001() {
        let err = check_source(
            "from typing import Protocol\nclass P(Protocol):\n    def foo(self) -> int: ...\nprint(issubclass(P, int))\n",
        )
        .unwrap_err();
        assert_eq!(err.code, "C0001");
        assert!(
            err.message.contains("`issubclass` with protocol"),
            "unexpected message: {}",
            err.message
        );
    }

    #[test]
    fn issubclass_against_protocol_target_is_c0001() {
        let err = check_source(
            "from typing import Protocol\nclass P(Protocol):\n    def foo(self) -> int: ...\nclass C:\n    def __init__(self) -> None:\n        self.x = 0\nprint(issubclass(C, P))\n",
        )
        .unwrap_err();
        assert_eq!(err.code, "C0001");
        assert!(
            err.message.contains("`issubclass` against protocol"),
            "unexpected message: {}",
            err.message
        );
    }

    // -- #380 W2: @property satisfies a protocol attribute requirement -----

    /// Verifies that a class with a `@property` getter (but no direct
    /// attribute slot) satisfies a protocol's attribute requirement.
    /// This exercises the `properties` check added to
    /// `lookup_attr_through_mro` (lines 233-242): the protocol requires
    /// attribute `x: int`, the class has no `x` in its `attrs` table, but
    /// it does have a `@property` getter for `x` whose return type is
    /// `int`. `check_protocol_conformance` should return `Ok(())`.
    #[test]
    fn protocol_conformance_with_property_satisfying_attribute_succeeds() {
        use pycc_hir::PropertyDef;

        // --- Protocol "P": requires attribute `x: int` ---
        let proto_def = HirClassDef {
            class_attrs: Vec::new(),
            exception_type_tag: None,
            name: "P".to_string(),
            bases: Vec::new(),
            mro: vec!["P".to_string()],
            attrs: Vec::new(),
            methods: Vec::new(),
            type_param: None,
            properties: Vec::new(),
            static_methods: Vec::new(),
            class_methods: Vec::new(),
            is_enum: false,
            enum_members: Vec::new(),
            is_dataclass: false,
            dataclass_fields: Vec::new(),
            is_protocol: true,
            runtime_checkable: false,
            protocol_members: vec![pycc_hir::ProtocolMember::Attribute {
                name: "x".to_string(),
                ty: Ty::Int,
            }],
            abstract_methods: Vec::new(),
            is_abstract: false,
        };

        // --- Concrete class "C": `@property` getter for `x`, no `x` attr ---
        let c_self_ty = Ty::Instance(Box::new("C".to_string()));
        let c_init = HirItem::Function {
            name: "C.__init__".to_string(),
            params: vec![("self".to_string(), c_self_ty.clone())],
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
        let c_getter = HirItem::Function {
            name: "C.x".to_string(),
            params: vec![("self".to_string(), c_self_ty.clone())],
            return_ty: Ty::Int,
            body: vec![HirStmt::Return(Some(HirExpr::AttrGet {
                base: Box::new(HirExpr::Name("self".to_string())),
                attr: "_val".to_string(),
            }))],
        };
        let c_def = HirClassDef {
            class_attrs: Vec::new(),
            exception_type_tag: None,
            name: "C".to_string(),
            bases: Vec::new(),
            mro: vec!["C".to_string()],
            // Deliberately NO `x` in `attrs` — the property must satisfy
            // the protocol requirement, not a direct attribute slot.
            attrs: vec![("_val".to_string(), Ty::Int)],
            methods: vec![("__init__".to_string(), "C.__init__".to_string())],
            type_param: None,
            properties: vec![PropertyDef {
                name: "x".to_string(),
                getter: "C.x".to_string(),
                setter: None,
            }],
            static_methods: Vec::new(),
            class_methods: Vec::new(),
            is_enum: false,
            enum_members: Vec::new(),
            is_dataclass: false,
            dataclass_fields: Vec::new(),
            is_protocol: false,
            runtime_checkable: false,
            protocol_members: Vec::new(),
            abstract_methods: Vec::new(),
            is_abstract: false,
        };

        // --- Trigger `check_protocol_conformance` via `c: P = C()` ---
        let hir = HirModule {
            seeded_builtin_exception_classes: false,
            items: vec![
                c_init,
                c_getter,
                top_level(HirStmt::AnnAssign {
                    target: "c".to_string(),
                    annotation: Ty::Protocol(Box::new("P".to_string())),
                    value: Some(HirExpr::Call {
                        callee: "C".to_string(),
                        args: vec![],
                    }),
                    is_final: false,
                }),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: vec![("P".to_string(), proto_def), ("C".to_string(), c_def)],
        };

        check(&hir).expect(
            "a class with a @property getter should satisfy a protocol \
             attribute requirement",
        );
    }

    /// #380 W2: covers the defensive panic in `lookup_attr_through_mro`
    /// (lines 234-239) — a property's getter is in the class's own
    /// property table but was never registered in
    /// `Environment::functions`. This "declared shape and Environment
    /// disagree" scenario is unreachable from any real `check`-validated
    /// program, mirroring `resolve_attr_get`'s own analogous panic test.
    #[test]
    #[should_panic(expected = "was not registered as an ordinary function")]
    fn lookup_attr_through_mro_panics_when_a_property_getter_is_not_registered() {
        use pycc_hir::PropertyDef;
        let mut env = crate::Environment::new();
        env.bind_class(
            "Ghost".to_string(),
            HirClassDef {
                class_attrs: Vec::new(),
                exception_type_tag: None,
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
                is_enum: false,
                enum_members: Vec::new(),
                is_dataclass: false,
                dataclass_fields: Vec::new(),
                is_protocol: false,
                runtime_checkable: false,
                protocol_members: Vec::new(),
                abstract_methods: Vec::new(),
                is_abstract: false,
            },
        );
        let _ = super::lookup_attr_through_mro(&env, &["Ghost".to_string()], "x");
    }
}
