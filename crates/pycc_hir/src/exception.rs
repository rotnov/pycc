//! HIR exception-class metadata and handler shape (PEP 3110, #382).

use super::{HirClassDef, HirItem, HirStmt, Ty};
use pycc_ast::{Expr, ModModule, Stmt};

pub const BUILTIN_EXCEPTION_CLASSES: [&str; 7] = [
    "Exception",
    "ValueError",
    "TypeError",
    "KeyError",
    "IndexError",
    "ZeroDivisionError",
    "RuntimeError",
];

pub fn is_builtin_exception_class(name: &str) -> bool {
    BUILTIN_EXCEPTION_CLASSES.contains(&name)
}

/// Returns the builtin exception class's parent, or `None` for the root and
/// unknown names. The currently supported hierarchy is intentionally flat.
pub fn builtin_exception_parent(name: &str) -> Option<&'static str> {
    match name {
        "Exception" => None,
        name if is_builtin_exception_class(name) => Some("Exception"),
        _ => None,
    }
}

/// The mangled `HirItem::Function` name of the synthetic `Exception`
/// constructor (Part 1 of #541). Uses the same `<ClassName>.<method>`
/// mangling every user-defined method already uses, so no downstream
/// consumer needs to special-case it.
pub const EXCEPTION_INIT_MANGLED_NAME: &str = "Exception.__init__";

/// Builds the seven synthetic [`HirClassDef`]s that give the builtin
/// exception hierarchy a first-class presence in the class table
/// (Part 1 of #541, extending D-173).
///
/// Before this existed, `Exception`/`ValueError`/... were recognized only
/// by name, through [`is_builtin_exception_class`], with no `HirClassDef`
/// behind them. That left `class MyError(ValueError):` resolving a base
/// that the class table did not contain, and left every consumer of
/// `Environment::classes` unable to tell "not a class" from "a class the
/// frontend never materialized".
///
/// The definitions are derived from [`BUILTIN_EXCEPTION_CLASSES`] and
/// [`builtin_exception_parent`], so the hierarchy has exactly one source of
/// truth. `Exception` is the root (no bases); the other six derive from it
/// directly, giving each a two-entry MRO `[self, "Exception"]` -- the same
/// linearization `compute_c3_mro` would produce, written directly here
/// because the shape is fixed and single-inheritance.
///
/// Only `Exception` carries a method: `__init__(self, message: str)`. The
/// other six inherit it through their MRO, exactly as a user subclass
/// would. The synthetic classes deliberately carry no attribute slots
/// (`attrs` is empty): D-173 propagates a raised exception through global
/// runtime state rather than through an allocated instance with fields, so
/// there is no storage for a `message` slot to name. See
/// `docs/RUNTIME.md`.
pub fn builtin_exception_class_defs() -> Vec<(String, HirClassDef)> {
    BUILTIN_EXCEPTION_CLASSES
        .iter()
        .map(|name| {
            let parent = builtin_exception_parent(name);
            let bases: Vec<String> = parent.map(|p| vec![p.to_string()]).unwrap_or_default();
            let mut mro = vec![(*name).to_string()];
            mro.extend(bases.iter().cloned());
            let methods = if parent.is_none() {
                vec![(
                    "__init__".to_string(),
                    EXCEPTION_INIT_MANGLED_NAME.to_string(),
                )]
            } else {
                Vec::new()
            };
            let def = HirClassDef {
                name: (*name).to_string(),
                bases,
                mro,
                attrs: Vec::new(),
                methods,
                properties: Vec::new(),
                static_methods: Vec::new(),
                class_methods: Vec::new(),
                type_param: None,
                enum_members: Vec::new(),
                is_dataclass: false,
                dataclass_fields: Vec::new(),
                is_protocol: false,
                runtime_checkable: false,
                protocol_members: Vec::new(),
                abstract_methods: Vec::new(),
                is_abstract: false,
            };
            ((*name).to_string(), def)
        })
        .collect()
}

/// Builds the synthetic `Exception.__init__(self, message: str)` item that
/// [`builtin_exception_class_defs`]'s `Exception` entry names in its method
/// table (Part 1 of #541).
///
/// It is emitted as an ordinary mangled `HirItem::Function`, so the type
/// checker registers its signature through the same per-item signature pass
/// every user method already goes through, and instantiation of a user
/// subclass resolves it through the MRO with no new lookup path.
///
/// Its body is a bare `return`: D-173 stores the raised exception in global
/// runtime state at the `raise` site, so the constructor has no field to
/// initialize. The signature is deliberately narrower than CPython's
/// `Exception(*args)` -- this compiler has no variadic-argument support,
/// and the existing `raise ValueError("...")` surface already accepts
/// exactly one `str` message. `docs/RUNTIME.md` records that divergence.
pub fn builtin_exception_init_item() -> HirItem {
    HirItem::Function {
        name: EXCEPTION_INIT_MANGLED_NAME.to_string(),
        params: vec![
            (
                "self".to_string(),
                Ty::Instance(Box::new("Exception".to_string())),
            ),
            ("message".to_string(), Ty::Str),
        ],
        return_ty: Ty::None,
        body: vec![HirStmt::Return(None)],
    }
}

/// Whether `def` is exactly the synthetic definition
/// [`builtin_exception_class_defs`] produces for `name` (Part 1 of #541).
///
/// This is how the type checker tells a synthesized builtin exception class
/// apart from a user class that happens to share one of the seven names.
/// Structural equality is sufficient and exact: HIR lowering seeds the
/// synthetic definitions all-or-nothing, and only when the module binds
/// none of the seven names itself, so a user-authored `class ValueError:`
/// is never accompanied by a synthetic one.
pub fn is_builtin_exception_class_def(name: &str, def: &HirClassDef) -> bool {
    builtin_exception_class_defs()
        .iter()
        .any(|(synthetic_name, synthetic_def)| synthetic_name == name && synthetic_def == def)
}

/// Whether `module`'s own top level binds any of the seven names in
/// [`BUILTIN_EXCEPTION_CLASSES`] (Part 1 of #541).
///
/// HIR lowering seeds the synthetic definitions all-or-nothing: when this
/// returns `true`, *no* synthetic class is seeded, and the seven names keep
/// exactly the pre-#541 behavior of being recognized by name alone. Two
/// reasons for all-or-nothing rather than per-name:
///
/// * Seeding a subset would make the hierarchy incoherent -- a synthetic
///   `ValueError` whose `bases`/`mro` name an `Exception` that a user's own
///   `class Exception:` has replaced resolves its inherited `__init__`
///   against the wrong class.
/// * It keeps [`is_builtin_exception_class_def`]'s structural comparison
///   exact. A user class can never be mistaken for a synthetic one, because
///   a module containing a user binding of any of the seven names carries no
///   synthetic definitions at all.
///
/// The scan is deliberately conservative: it reports `true` for any
/// top-level `class`/`def`/`type`-alias/annotated-assignment/assignment
/// target spelling one of the seven names, whether or not that particular
/// spelling would go on to collide with a seeded definition. Over-reporting
/// only costs the module its synthetic classes; under-reporting would let a
/// user definition collide with a compiler-synthesized one and surface as a
/// spurious `C0001`.
///
/// `import`/`from ... import ...` are not scanned: D-136/D-137 resolve every
/// import against `pycc_std`'s registry, which contains none of the seven
/// names, so an `import ValueError` is rejected as an unresolvable module
/// before any class-table collision could be reached.
pub(crate) fn module_shadows_builtin_exception_name(module: &ModModule) -> bool {
    module.body.iter().any(|stmt| match stmt {
        Stmt::ClassDef(class_def) => is_builtin_exception_class(class_def.name.as_str()),
        Stmt::FunctionDef(function_def) => is_builtin_exception_class(function_def.name.as_str()),
        Stmt::TypeAlias(type_alias) => expr_binds_builtin_exception_name(&type_alias.name),
        Stmt::AnnAssign(ann_assign) => expr_binds_builtin_exception_name(&ann_assign.target),
        Stmt::Assign(assign) => assign.targets.iter().any(expr_binds_builtin_exception_name),
        _ => false,
    })
}

/// Whether `expr`, used as an assignment target, binds one of the seven
/// builtin exception names. Recurses through the unpacking-target shapes
/// (`a, b = ...`, `[a, b] = ...`, `*rest`) so a name buried in one is still
/// seen. Any other target shape (an attribute, a subscript) rebinds
/// something other than a bare module-level name, so it cannot shadow one
/// of the seven.
fn expr_binds_builtin_exception_name(expr: &Expr) -> bool {
    match expr {
        Expr::Name(name) => is_builtin_exception_class(name.id.as_str()),
        Expr::Tuple(tuple) => tuple.elts.iter().any(expr_binds_builtin_exception_name),
        Expr::List(list) => list.elts.iter().any(expr_binds_builtin_exception_name),
        Expr::Starred(starred) => expr_binds_builtin_exception_name(&starred.value),
        _ => false,
    }
}

/// A single `except` handler. `exc_type` is `None` for bare `except:` and
/// `name` is the optional `as` binding.
#[derive(Debug, Clone, PartialEq)]
pub struct HirExceptHandler {
    pub exc_type: Option<String>,
    pub name: Option<String>,
    pub body: Vec<HirStmt>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_exception_table_and_flat_parents_are_consistent() {
        for name in BUILTIN_EXCEPTION_CLASSES {
            assert!(is_builtin_exception_class(name));
            let expected = (name != "Exception").then_some("Exception");
            assert_eq!(builtin_exception_parent(name), expected);
        }
        assert!(!is_builtin_exception_class("NotAnException"));
        assert_eq!(builtin_exception_parent("NotAnException"), None);
    }
}

#[cfg(test)]
mod synthetic_class_tests;
