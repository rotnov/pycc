//! Class-table binding, lookup, and instantiation resolution
//! (`bind_classes`, `expect_class`, `resolve_instantiation`).
//!
//! Extracted verbatim from `crates/pycc_types/src/class.rs` per AGENTS.md's
//! file-decomposition rule and D-185's per-file tracking issue (#549): this
//! is one cohesion-driven seam of that 4,614-line file, not a rewrite. That
//! extraction changed no diagnostic message, no check, and no panic message
//! -- its only edits were the ones the module boundary forces (visibility
//! keywords and `use` lines). Later changes are free to edit this file like
//! any other; #912 reworded `resolve_instantiation`'s internal-error panic,
//! so read the claim above as "unchanged relative to the #549 extraction",
//! not as a standing guarantee.
//!
//! The seam is the three places where a class *name* meets `Environment`'s
//! class *table*: registering every `HirClassDef` the module lowered
//! (`bind_classes`), looking one back up with the crate's panic-on-internal-
//! inconsistency convention (`expect_class`), and turning a confirmed class
//! name plus argument types into a `Ty::Instance` value by resolving
//! `__init__` through the MRO (`resolve_instantiation`).
//!
//! Everything downstream of a *resolved* `HirClassDef` stays in `class.rs`
//! or its own further seam: attribute reads and writes, `super()` call
//! resolution, static/class-method dispatch, protocol conformance, and the
//! `T00xx` diagnostic constructors they share stay in `class.rs` directly;
//! method-call resolution has its own sibling seam in
//! `class/method_call.rs` (#815, Part 1 of #737). `check_call_args` stays
//! in `class.rs` -- it is shared across method, static-method, and
//! protocol call resolution and belongs to neither seam, so this module
//! and `method_call.rs` both import it back from their shared parent.

use crate::Environment;
use pycc_diag::{Diagnostic, Span};
use pycc_hir::{HirClassDef, HirModule, Ty};

use super::check_call_args;

/// Populates `env`'s class table from `hir.class_defs` -- called once by
/// every `Environment` constructor this crate has (`check_with_signatures_all`'s
/// own per-item loop, `concrete_function_environment`'s literal), mirroring
/// how each already registers every function's signature.
///
/// Part 1 of #541 (D-188): a class is marked synthetic if and only if
/// *this compiler's own* HIR lowering produced it. The provenance record is
/// `hir.seeded_builtin_exception_classes`, set by `lower_checked` at the
/// point it seeds; combined with `is_builtin_exception_class` it is exact,
/// because seeding is all-on/all-off and its shadow gate guarantees a
/// seeded module has no user top-level binding of any of the 25 names.
/// Nothing here inspects a `HirClassDef`'s shape: a user-authored class can
/// be structurally identical to a synthetic one, so shape is not evidence
/// of origin.
pub(crate) fn bind_classes(env: &mut Environment, hir: &HirModule) {
    for (name, class_def) in &hir.class_defs {
        if hir.seeded_builtin_exception_classes && pycc_hir::is_builtin_exception_class(name) {
            env.bind_synthetic_class(name.clone(), class_def.clone());
        } else {
            env.bind_class(name.clone(), class_def.clone());
        }
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
    // #921 (PEP 435): an enum class cannot be called. Its members are
    // compile-time singletons reached by name (`Color.RED`); CPython's
    // `Color(1)` is a by-value member lookup and `Color()` is a
    // `TypeError`, and neither shape is implemented -- `lower_enum_class`
    // deliberately gives an enum no `__init__`, so this guard is also what
    // keeps the MRO walk below from reaching its internal-error panic for
    // an enum. Keyed on `is_enum` (provenance, D-188), not on a non-empty
    // `enum_members`: a docstring-only enum (#744) has an empty member
    // table and is an enum all the same. The "not supported yet" clause
    // attaches only to the by-value lookup, which a later slice can
    // implement; `Color()` is a CPython error too and no slice will accept
    // it (`docs/DIAGNOSTICS.md`'s `C0001` is a versioned capability code).
    if class_def.is_enum {
        return Err(Diagnostic::error(
            "C0001",
            format!(
                "cannot call enum class `{class_name}` -- enum members are accessed \
                 by name (`{class_name}.MEMBER`); looking a member up by value \
                 (`{class_name}(1)`) is not supported yet, and a zero-argument call \
                 (`{class_name}()`) is a `TypeError` in CPython as well"
            ),
            Span::new(0, 0),
        ));
    }
    // Part 1 of #541 (extending D-173): a *synthetic* builtin exception
    // class cannot be instantiated as a value. D-173 propagates a raised
    // exception through global runtime state rather than through an
    // allocated instance, so `e = ValueError("x")` has nothing to bind and
    // no storage to allocate; `raise ValueError("x")` is the only supported
    // construction, and it is checked by `exception::check_raise_operand`
    // without ever reaching here. Keyed on `is_synthetic_class`, not on the
    // name alone: a user's own `class ValueError:` is an ordinary class and
    // stays instantiable.
    if env.is_synthetic_class(class_name) {
        return Err(Diagnostic::error(
            "C0001",
            format!(
                "cannot instantiate builtin exception class `{class_name}` -- \
                 a builtin exception can only be constructed by raising it \
                 (`raise {class_name}(\"message\")`), not bound as a value"
            ),
            Span::new(0, 0),
        ));
    }
    // #432: walk the MRO to find the first class with an `__init__` method.
    // #714: also record whether that ancestor is itself a *synthetic*
    // (seeded) class -- see the check just below, which must not key on the
    // mangled name string alone: a user-authored class can be named
    // `Exception` and declare its own `__init__`, mangling to the identical
    // `"Exception.__init__"` string as the synthetic placeholder without
    // being it (`is_synthetic_class` is provenance-based, D-188; shape and
    // name never decide it).
    let (mangled, init_owner_is_synthetic) = class_def
        .mro
        .iter()
        .find_map(|mro_class| {
            let mro_def = env.lookup_class(mro_class)?;
            if mro_def.methods.iter().any(|(mn, _)| mn == "__init__") {
                Some((
                    format!("{mro_class}.__init__"),
                    env.is_synthetic_class(mro_class),
                ))
            } else {
                None
            }
        })
        .unwrap_or_else(|| {
            panic!(
                "pycc_types: internal error: no `__init__` found in class `{class_name}`'s MRO -- \
             pycc_hir guarantees an `__init__` for every non-enum class it lowers (D-225: by \
             inheritance or by synthesis), and the `is_enum` guard above rejects an enum class \
             with C0001 before this walk (#921)"
            )
        });
    // The resolved `__init__` coming from a *synthetic* ancestor means
    // `class_name` is a user-declared class (the `is_synthetic_class` check
    // above already excluded a directly seeded builtin) whose MRO reaches a
    // builtin exception without overriding its constructor -- exactly the
    // shape `exception::reject_own_constructor` permits for a *raise*
    // operand. `pycc_hir::lower_checked` always appends the synthetic
    // placeholder's HIR item after every real module-level statement
    // (regardless of source position) so the exception's own raise-path
    // type checking has a real `env.lookup_function` entry to resolve
    // against; codegen does emit a real, callable body for it like any
    // other `MirItem::Function`, but binds its function-pointer slot at
    // that same always-last module position, so any earlier call through
    // the slot -- which is effectively every real program, since the item
    // is always last -- observes a null pointer and aborts via the
    // runtime's name-error path. A generic instantiation call here
    // (`e = MyError("boom")`, `MyError()` as a default argument, etc.)
    // would therefore compile cleanly and abort at runtime on a
    // `NameError` naming a symbol the user never wrote -- a call-ordering
    // artifact, not a missing body. `exception::check_raise_operand`
    // validates the one shape that *is* supported (a fresh
    // `raise MyError("boom")`) itself, directly against this same
    // single-`str`-argument signature, without ever reaching this function
    // -- see that function's own comment on its `user_exception_class`
    // branch.
    if init_owner_is_synthetic {
        return Err(Diagnostic::error(
            "C0001",
            format!(
                "cannot instantiate exception class `{class_name}` as a value -- \
                 pycc does not materialize an exception instance for a class that \
                 inherits `Exception`'s constructor without overriding it; \
                 `raise {class_name}(\"message\")` is supported, binding the result \
                 to a name is not (Part 3 of #541)"
            ),
            Span::new(0, 0),
        ));
    }
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
                class_attrs: Vec::new(),
                exception_type_tag: None,
                name: "Ghost".to_string(),
                bases: Vec::new(),
                mro: vec!["Ghost".to_string()],
                attrs: vec![],
                methods: vec![("__init__".to_string(), "Ghost.__init__".to_string())],
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
        // #432 / #912 / #921: `pycc_hir` guarantees an `__init__` for
        // every non-enum class it lowers (D-225: by inheritance or by
        // synthesis), and an enum class -- the one shape lowered without a
        // constructor -- is rejected by the `is_enum` guard before the MRO
        // walk, so this panic is an internal error. This test bypasses the
        // normal entry point and binds a non-enum class (`is_enum: false`)
        // whose MRO contains no `__init__` method. The MRO
        // also includes `Phantom` (not registered), exercising the `?`
        // arm of the `find_map` closure -- `Ghost` is found but has no
        // `__init__`, then `Phantom` is not found, so `find_map` returns
        // `None` and the `unwrap_or_else` panic fires.
        let mut env = crate::Environment::new();
        env.bind_class(
            "Ghost".to_string(),
            HirClassDef {
                class_attrs: Vec::new(),
                exception_type_tag: None,
                name: "Ghost".to_string(),
                bases: vec!["Phantom".to_string()],
                mro: vec!["Ghost".to_string(), "Phantom".to_string()],
                attrs: vec![],
                methods: vec![("f".to_string(), "Ghost.f".to_string())],
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
        let _ = super::resolve_instantiation(&env, "Ghost", &[]);
    }

    // -- #921 (PEP 435): calling an enum class is `C0001`, never a panic --

    /// Parses and lowers source code, then type-checks it with
    /// `check_and_resolve`, returning the first diagnostic. Every program
    /// below panicked in `resolve_instantiation`'s MRO walk before #921;
    /// these stay unit tests because `resolve_instantiation`'s error paths
    /// are covered in-crate (cargo-llvm-cov#276, see `class.rs`).
    fn check_source_err(source: &str) -> pycc_diag::Diagnostic {
        let module = pycc_parser::parse(source).expect("test fixture must parse");
        let hir = pycc_hir::lower_checked(&module).expect("test fixture must lower");
        crate::check_and_resolve(&hir).expect_err("calling an enum class must be rejected")
    }

    fn assert_enum_call_rejected(err: &pycc_diag::Diagnostic, class_name: &str) {
        assert_eq!(err.code, "C0001", "unexpected diagnostic: {err:?}");
        assert!(
            err.message
                .contains(&format!("cannot call enum class `{class_name}`")),
            "unexpected message: {}",
            err.message
        );
    }

    #[test]
    fn calling_an_enum_class_with_no_arguments_is_c0001() {
        // The issue's own program: a zero-argument call inside a function.
        let err = check_source_err(
            "from enum import Enum\n\nclass Color(Enum):\n    RED = 1\n    GREEN = 2\n\ndef main() -> None:\n    c = Color()\n    print(c.value)\n\nmain()\n",
        );
        assert_enum_call_rejected(&err, "Color");
        assert!(
            err.message
                .contains("(`Color()`) is a `TypeError` in CPython as well"),
            "the zero-argument form must not be described as merely unsupported: {}",
            err.message
        );
    }

    #[test]
    fn calling_an_enum_class_with_a_value_is_c0001() {
        // CPython's by-value member lookup (`Color(1)` -> `Color.RED`) is
        // not implemented; the guard fires regardless of arity, so the MRO
        // walk is never reached for an enum.
        let err = check_source_err(
            "from enum import Enum\n\nclass Color(Enum):\n    RED = 1\n    GREEN = 2\n\nc = Color(1)\nprint(c.value)\n",
        );
        assert_enum_call_rejected(&err, "Color");
        assert!(
            err.message.contains("(`Color(1)`) is not supported yet"),
            "the by-value form carries the versioned-capability clause: {}",
            err.message
        );
    }

    #[test]
    fn calling_a_member_less_enum_class_is_c0001() {
        // A docstring-only enum (#744) has an empty `enum_members` table,
        // so `is_enum` -- not the table -- must be what the guard keys on.
        let err = check_source_err(
            "from enum import Enum\n\nclass E(Enum):\n    \"doc\"\n\ne = E()\nprint(1)\n",
        );
        assert_enum_call_rejected(&err, "E");
    }

    #[test]
    fn calling_a_str_enum_class_is_c0001() {
        // `lower_enum_class` serves both marker bases, so a `StrEnum`
        // subclass carries `is_enum` and is rejected the same way.
        let err = check_source_err(
            "from enum import StrEnum\n\nclass S(StrEnum):\n    A = \"a\"\n\ns = S(\"a\")\nprint(s.value)\n",
        );
        assert_enum_call_rejected(&err, "S");
    }

    #[test]
    fn raising_an_enum_class_call_is_c0001() {
        // `raise Color()` never enters `check_raise_operand`'s
        // exception-class branch (an enum carries no `exception_type_tag`),
        // so the operand is inferred as an ordinary call and lands on the
        // same guard rather than the MRO-walk panic.
        let err = check_source_err(
            "from enum import Enum\n\nclass Color(Enum):\n    RED = 1\n\nraise Color()\n",
        );
        assert_enum_call_rejected(&err, "Color");
    }
}
