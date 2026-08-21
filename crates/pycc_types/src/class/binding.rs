//! Class-table binding, lookup, and instantiation resolution
//! (`bind_classes`, `expect_class`, `resolve_instantiation`).
//!
//! Extracted verbatim from `crates/pycc_types/src/class.rs` per AGENTS.md's
//! file-decomposition rule and D-185's per-file tracking issue (#549): this
//! is one cohesion-driven seam of that 4,614-line file, not a rewrite. Every
//! diagnostic message, every check, and every panic message is unchanged --
//! the only edits are the ones the module boundary forces (visibility
//! keywords and `use` lines).
//!
//! The seam is the three places where a class *name* meets `Environment`'s
//! class *table*: registering every `HirClassDef` the module lowered
//! (`bind_classes`), looking one back up with the crate's panic-on-internal-
//! inconsistency convention (`expect_class`), and turning a confirmed class
//! name plus argument types into a `Ty::Instance` value by resolving
//! `__init__` through the MRO (`resolve_instantiation`).
//!
//! Everything downstream of a *resolved* `HirClassDef` stays in `class.rs`:
//! attribute reads and writes, method and `super()` call resolution,
//! static/class-method dispatch, protocol conformance, and the `T00xx`
//! diagnostic constructors they share. `check_call_args` also stays there --
//! it is shared with method, static-method, and protocol call resolution and
//! belongs to neither seam, so this module imports it back from its parent.

use crate::Environment;
use pycc_diag::{Diagnostic, Span};
use pycc_hir::{HirClassDef, HirModule, Ty};

use super::check_call_args;

/// Populates `env`'s class table from `hir.class_defs` -- called once by
/// every `Environment` constructor this crate has (`check_with_signatures`'s
/// own per-item loop, `concrete_function_environment`'s literal), mirroring
/// how each already registers every function's signature.
pub(crate) fn bind_classes(env: &mut Environment, hir: &HirModule) {
    for (name, class_def) in &hir.class_defs {
        env.bind_class(name.clone(), class_def.clone());
    }
}

/// Looks up `class_name`'s declared shape, panicking if it isn't
/// registered. Every caller -- all of them in `class.rs`, this function's
/// parent module -- only ever calls this with a class name extracted from a
/// real `Ty::Instance` payload (either produced
/// by `resolve_instantiation` below, which only ever builds one from a
/// class `env.lookup_class` just confirmed exists, or from `self`'s own
/// type, assigned directly by `pycc_hir::class::lower_method` from the
/// enclosing class's own name) -- so an unregistered name reaching here
/// would mean `Environment::classes` was built from a different
/// `HirModule` than the one the `Ty::Instance` value itself came from, an
/// internal-consistency bug this crate has no way to recover from
/// meaningfully, matching `pycc_mir`'s own `lookup` panic-on-inconsistency
/// convention (see that function's own doc comment).
pub(super) fn expect_class<'e>(env: &'e Environment, class_name: &str) -> &'e HirClassDef {
    env.lookup_class(class_name).unwrap_or_else(|| {
        panic!(
            "pycc_types: internal error: class `{class_name}` has no registered \
             HirClassDef -- Environment::classes was built from a different HirModule \
             than the one this Ty::Instance came from"
        )
    })
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
    // #380 (PR-20, PEP 3119): an abstract class (`is_abstract`) cannot be
    // instantiated — it must be subclassed with concrete implementations
    // of all abstract methods first.
    if class_def.is_abstract {
        return Err(Diagnostic::error(
            "C0001",
            format!(
                "cannot instantiate abstract class `{class_name}` -- \
                 it has unimplemented abstract methods; subclass it and \
                 override all `@abstractmethod`-decorated methods first"
            ),
            Span::new(0, 0),
        ));
    }
    // #380 (PR-20, PEP 544): a protocol class cannot be instantiated —
    // it is a compile-time-only interface description.
    if class_def.is_protocol {
        return Err(Diagnostic::error(
            "C0001",
            format!(
                "cannot instantiate protocol class `{class_name}` -- \
                 a protocol is a compile-time-only interface description, \
                 not an instantiable class"
            ),
            Span::new(0, 0),
        ));
    }
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

#[cfg(test)]
mod tests {
    use pycc_hir::HirClassDef;

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
                is_dataclass: false,
                dataclass_fields: Vec::new(),
                is_protocol: false,
                runtime_checkable: false,
                protocol_members: Vec::new(),
                abstract_methods: Vec::new(),
                is_abstract: false,
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
                is_dataclass: false,
                dataclass_fields: Vec::new(),
                is_protocol: false,
                runtime_checkable: false,
                protocol_members: Vec::new(),
                abstract_methods: Vec::new(),
                is_abstract: false,
            },
        );
        let _ = super::resolve_instantiation(&env, "Ghost", &[]);
    }
}
