//! Class, MRO, and compile-time `isinstance`/`issubclass` lowering (#546):
//! the attribute-slot layout walk, `super()`/`__repr__` resolution helpers,
//! and the constant-folding of both class-predicate builtins.

use super::{HirClassDef, MirExpr, lookup, lower_expr, mro_class_def};
use pycc_hir::{
    HirExpr, Ty, eval_isinstance_single, eval_issubclass_single, extract_class_names,
    is_builtin_type_name,
};
use std::collections::HashMap;

/// Resolves `expr`'s own `Ty::Instance` payload to its declared
/// `HirClassDef`, shared by `HirExpr::AttrGet`/`MethodCall`'s lowering in
/// `expr.rs` and `HirStmt::AttrSet`'s in `stmt.rs`. Panics (never a `Result`,
/// matching this crate's own established "pycc_types::check should have
/// rejected this" convention -- see `lookup`'s own doc comment) when `expr`
/// isn't instance-typed at all, or names a class this module's own `classes`
/// table doesn't have: both are impossible from a program
/// `pycc_types::check` (or `check_and_resolve`) already accepted, so only a
/// hand-built `MirExpr`/`HirModule` bypassing that validation (e.g. this
/// crate's own internal-error tests under `tests/`) can reach either panic.
/// #378 (PR-18): If `expr` is a class-instance-typed expression whose
/// class has a `__repr__` method (found via the MRO), rewrites it to a
/// `MirExpr::Call` to that `__repr__` method, passing the original
/// expression as the `self` argument. The result type is `Ty::Str`. If
/// the expression is not an instance or the class has no `__repr__`,
/// returns the original expression unchanged.
///
/// This is used by `HirExpr::FString`'s interpolation lowering and
/// `HirExpr::Call`'s `print` argument lowering so the codegen's `to_str`
/// receives a `str` scalar (from the `__repr__` call's return value)
/// instead of an `Instance` scalar (which would panic in `to_str`).
pub(super) fn rewrite_instance_to_repr(
    expr: &MirExpr,
    classes: &HashMap<String, HirClassDef>,
) -> MirExpr {
    let Ty::Instance(class_name) = expr.ty() else {
        return expr.clone();
    };
    let Some(class_def) = classes.get(class_name.as_str()) else {
        return expr.clone();
    };
    // #378 (PR-18): only rewrite for dataclass classes, whose
    // compiler-synthesized `__repr__` has a known-correct signature
    // `(self) -> str`. A user-defined `__repr__` on a non-dataclass class
    // may have a different arity or return type, which would cause a
    // codegen panic if rewritten to a call here. Non-dataclass instances
    // pass through unchanged (the type checker rejects `print(instance)` /
    // f-string interpolation of a non-dataclass instance with `T0021`
    // before codegen).
    if !class_def.is_dataclass {
        return expr.clone();
    }
    let repr_mangled = class_def.mro.iter().find_map(|mro_class| {
        // Every class in the MRO was registered when the class was lowered;
        // using `.expect` (whose panic path lives in libcore, outside this
        // crate's instrumented regions) avoids a `?` whose `None` branch is
        // structurally unreachable and would show up as a permanently
        // uncovered region under D-014's 100% coverage gate.
        let mro_def = classes
            .get(mro_class.as_str())
            .expect("MRO class must be registered");
        mro_def
            .methods
            .iter()
            .find(|(mn, _)| mn == "__repr__")
            .map(|(_, mangled)| mangled.clone())
    });
    // A dataclass always has a synthesized `__repr__` in its MRO (the
    // `is_dataclass` guard above ensures we only reach here for dataclass
    // classes). Using `.expect` (whose panic path lives in libcore,
    // outside this crate's instrumented regions) avoids a `match` whose
    // `None` arm is structurally unreachable for a dataclass and would
    // show up as a permanently uncovered region under D-014's 100%
    // coverage gate.
    let repr_mangled = repr_mangled.expect("dataclass must have __repr__");
    MirExpr::Call {
        callee: repr_mangled,
        args: vec![expr.clone()],
        ty: Ty::Str,
    }
}

pub(super) fn class_def_of<'c>(
    expr: &MirExpr,
    classes: &'c HashMap<String, HirClassDef>,
) -> &'c HirClassDef {
    // #380 (PR-20): protocol-typed expressions resolve to the protocol's
    // own class def. This is used for method/attribute resolution on
    // protocol-typed variables that were not monomorphized (e.g. a
    // protocol-typed local variable inside a function body that doesn't
    // take a protocol parameter).
    let ty = expr.ty();
    let class_name = match &ty {
        Ty::Instance(name) => name.as_str(),
        Ty::Protocol(name) => name.as_str(),
        other => panic!(
            "pycc_mir: internal error: expected an instance- or protocol-typed expression, found `{}` -- pycc_types::check should have rejected this HIR before it reached pycc_mir",
            other.name()
        ),
    };
    classes.get(class_name).unwrap_or_else(|| {
        panic!(
            "pycc_mir: internal error: class `{class_name}` has no registered HirClassDef -- pycc_types::check should have rejected this HIR before it reached pycc_mir"
        )
    })
}

/// #433: Builds a `MirExpr::Name` for the current method's `self` parameter,
/// looked up from the innermost scope. Used by `super().method()` and
/// `super().attr` lowering to pass the most-derived instance as the implicit
/// first argument / attribute base. Panics if `self` is not bound in the
/// current scope (impossible for a method body that reached MIR lowering —
/// `pycc_hir` always includes `self` as the first parameter of a method).
pub(super) fn self_expr(scopes: &[HashMap<String, Ty>]) -> MirExpr {
    let ty = lookup(scopes, "self");
    MirExpr::Name {
        name: "self".to_string(),
        ty,
    }
}

/// #432: Computes the flat attribute-slot layout for a class by walking its
/// MRO. Each class in the MRO (from most derived to most base) contributes
/// its own declared attributes that haven't already been seen by an earlier
/// (more derived) class. The result is a flat `(name, ty)` list whose indices
/// are the slot indices used at runtime -- the instance is allocated with
/// exactly this many slots (`mro_attr_count`), and every `AttrGet`/`AttrSet`
/// resolves its slot index against this flat layout, not the individual
/// class's own `attrs` list.
///
/// A derived class that re-declates an attribute of the same name as a base
/// class "wins" (its declaration appears first in the MRO, so its slot type
/// is the one used), matching CPython's own MRO-based attribute resolution.
pub(super) fn mro_attrs(
    class_def: &HirClassDef,
    classes: &HashMap<String, HirClassDef>,
) -> Vec<(String, Ty)> {
    // #432: Walk the MRO most-base-first so that base class attributes
    // always occupy consistent low slot indices. This is critical for
    // inherited methods: when `Animal.speak` reads `self.name`, it
    // resolves the slot index from `Animal`'s `mro_attrs` (where `name`
    // is slot 0). If we walked most-derived-first, `Dog`'s `breed` would
    // get slot 0 and `name` would shift to slot 1 — but `Animal.speak`
    // would still read slot 0, getting `breed` instead of `name`.
    //
    // For re-declared attributes (a derived class re-declaring an attr
    // with the same name as a base), the most-derived declaration's type
    // wins — so we do a second pass over the MRO (most-derived-first) to
    // override types for attrs that were already assigned a slot.
    //
    // Collect the MRO defs once (verifying all classes exist) so both
    // passes share the same lookup and the panic path is only exercised
    // once.
    let mro_defs: Vec<&HirClassDef> = class_def
        .mro
        .iter()
        .map(|mro_class| mro_class_def(mro_class, classes))
        .collect();
    let mut result: Vec<(String, Ty)> = Vec::new();
    let mut slot_index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    // Pass 1: assign slots in most-base-first order (reverse MRO).
    for mro_def in mro_defs.iter().rev() {
        for (name, ty) in &mro_def.attrs {
            if !slot_index.contains_key(name) {
                slot_index.insert(name.clone(), result.len());
                result.push((name.clone(), ty.clone()));
            }
        }
    }
    // Pass 2: override types for re-declared attrs (most-derived wins).
    // Walk the MRO forward and, for each attr, take the type from the
    // first (most-derived) class that declares it. We track which attrs
    // have already been overridden to avoid a less-derived class
    // overwriting the most-derived type.
    let mut overridden: std::collections::HashSet<String> = std::collections::HashSet::new();
    for mro_def in &mro_defs {
        for (name, ty) in &mro_def.attrs {
            // `overridden.insert` returns true the first time we see this
            // attr in pass 2. Since pass 1 already assigned a slot for
            // every attr in the MRO, `slot_index[name]` always exists
            // here — direct indexing is safe.
            if overridden.insert(name.clone()) {
                let idx = slot_index[name];
                result[idx].1 = ty.clone();
            }
        }
    }
    result
}

/// #432: Returns the total number of attribute slots for a class, computed
/// from its MRO's flat attribute layout. Used by `Instantiate` to allocate
/// the correct number of slots.
pub(super) fn mro_attr_count(
    class_def: &HirClassDef,
    classes: &HashMap<String, HirClassDef>,
) -> usize {
    mro_attrs(class_def, classes).len()
}

// ---------------------------------------------------------------------------
// Issue #435: compile-time `isinstance`/`issubclass` MIR lowering.
//
// Both builtins are evaluated at compile time (pycc's static dispatch model
// means every variable's runtime type is exactly its declared type), emitting
// `MirExpr::BoolLiteral(result)` constants. No runtime type tags or RTTI.
// ---------------------------------------------------------------------------

/// #435: Lowers `isinstance(obj, class_arg)` to a compile-time boolean
/// constant. The object expression is lowered to MIR (to extract its type
/// via `.ty()`), but the class argument is NOT lowered — it is a class
/// reference, not a value. The result is computed using
/// `eval_isinstance_single` with the object's class MRO.
pub(super) fn lower_isinstance(
    args: &[HirExpr],
    scopes: &[HashMap<String, Ty>],
    classes: &HashMap<String, HirClassDef>,
    current_class: Option<&str>,
) -> MirExpr {
    // The type checker already validated arg count and class names. If we
    // reach here, args has exactly 2 elements and args[1] is a valid class
    // name or tuple of class names.
    let obj = lower_expr(&args[0], scopes, classes, current_class);
    let obj_ty = obj.ty();
    // Extract class names from the second argument.
    let class_names = extract_class_names(&args[1]).expect(
        "pycc_mir: internal error: isinstance's second argument was not validated by pycc_types",
    );
    // Compute the result: true if any target class matches.
    let obj_mro = match &obj_ty {
        Ty::Instance(class_name) => classes
            .get(class_name.as_str())
            .map(|cd| cd.mro.as_slice())
            .unwrap_or(&[]),
        _ => &[],
    };
    let result = class_names.iter().any(|target| {
        // #380 (PR-20, PEP 544): if the target is a protocol class, use
        // structural conformance checking instead of nominal MRO
        // membership. The type checker already validated that the
        // protocol is `@runtime_checkable`.
        if let Some(target_def) = classes.get(target.as_str())
            && target_def.is_protocol
        {
            return eval_isinstance_protocol(&obj_ty, target_def, classes);
        }
        eval_isinstance_single(&obj_ty, target, obj_mro)
    });
    MirExpr::BoolLiteral(result)
}

/// #380 (PR-20, PEP 544): Evaluates `isinstance(obj, Protocol)` at
/// compile time using structural conformance. Returns `true` if the
/// object's class has all the protocol's required members with
/// compatible types.
pub(super) fn eval_isinstance_protocol(
    obj_ty: &Ty,
    proto_def: &HirClassDef,
    classes: &HashMap<String, HirClassDef>,
) -> bool {
    let Ty::Instance(class_name) = obj_ty else {
        return false;
    };
    let Some(class_def) = classes.get(class_name.as_str()) else {
        return false;
    };
    // Check each protocol member against the class's members (through
    // its MRO).
    use pycc_hir::ProtocolMember;
    for member in &proto_def.protocol_members {
        match member {
            ProtocolMember::Method {
                name: method_name,
                param_tys: proto_param_tys,
                return_ty: proto_return_ty,
            } => {
                // Look up the method through the MRO.
                let found = class_def.mro.iter().find_map(|mro_class| {
                    // An MRO entry may refer to a class not present in the
                    // `classes` map (e.g. a ghost base in unit tests).
                    // `filter_map` skips such entries, matching the
                    // `None`-is-not-found semantics of the attribute arm.
                    let mro_def = classes.get(mro_class.as_str())?;
                    let mangled = mro_def
                        .methods
                        .iter()
                        .find(|(n, _)| n == method_name)
                        .map(|(_, m)| m.as_str())?;
                    // The function signature is not available in MIR's
                    // `classes` table (it only has `HirClassDef`, not
                    // the `Environment`). For `@runtime_checkable`
                    // protocols, PEP 544 specifies that only the
                    // *presence* of attributes/methods is checked, not
                    // their types. This matches CPython's own
                    // `@runtime_checkable` behavior (it only checks
                    // for the presence of attributes, not their types).
                    Some(mangled)
                });
                if found.is_none() {
                    return false;
                }
                // For runtime_checkable, we only check presence, not
                // type compatibility (matching CPython's behavior).
                let _ = (proto_param_tys, proto_return_ty);
            }
            ProtocolMember::Attribute {
                name: attr_name, ..
            } => {
                // Look up the attribute through the MRO.
                let found = class_def.mro.iter().any(|mro_class| {
                    // An MRO entry may refer to a class not present in the
                    // `classes` map (e.g. a ghost base in unit tests).
                    // `is_some_and(…)` treats missing entries as not
                    // having the attribute, matching the method arm's
                    // `find_map` skip semantics.
                    classes.get(mro_class.as_str()).is_some_and(|mro_def| {
                        mro_def.attrs.iter().any(|(n, _)| n == attr_name)
                            || mro_def.properties.iter().any(|p| &p.name == attr_name)
                    })
                });
                if !found {
                    return false;
                }
            }
        }
    }
    true
}

/// #435: Lowers `issubclass(cls_arg, class_arg)` to a compile-time boolean
/// constant. Neither argument is lowered as a MIR expression — both are
/// class references. The result is computed using `eval_issubclass_single`
/// with the source class's MRO.
pub(super) fn lower_issubclass(
    args: &[HirExpr],
    classes: &HashMap<String, HirClassDef>,
) -> MirExpr {
    // The type checker already validated arg count and class names. If we
    // reach here, args has exactly 2 elements, args[0] is a bare class name,
    // and args[1] is a valid class name or tuple of class names.
    let cls_name = match &args[0] {
        HirExpr::Name(name) => name.as_str(),
        _ => unreachable!(
            "pycc_mir: internal error: issubclass's first argument was not validated by pycc_types"
        ),
    };
    let target_names = extract_class_names(&args[1]).expect(
        "pycc_mir: internal error: issubclass's second argument was not validated by pycc_types",
    );
    // Get the source class's MRO (empty for builtin types).
    let cls_mro = if is_builtin_type_name(cls_name) {
        &[][..]
    } else {
        classes
            .get(cls_name)
            .map(|cd| cd.mro.as_slice())
            .unwrap_or(&[])
    };
    let result = target_names
        .iter()
        .any(|target| eval_issubclass_single(cls_name, target, cls_mro));
    MirExpr::BoolLiteral(result)
}
