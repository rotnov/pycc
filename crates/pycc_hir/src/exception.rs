//! HIR exception-class metadata and handler shape (PEP 3110, #382).

use super::{HirClassDef, HirItem, HirStmt, Ty};
use pycc_ast::visitor::{self, Visitor};
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
/// Lowering seeds them only into a module that actually references one of
/// the seven names and shadows none of them -- see this module's
/// (crate-private) `module_references_builtin_exception_name` and
/// `module_shadows_builtin_exception_name`.
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

/// The seven synthetic definitions, built once per process.
///
/// [`is_builtin_exception_class_def`] runs on *every* `bind_class` call, and
/// `bind_class` runs once per class per `Environment` construction -- of
/// which the type checker performs one per checked function. Rebuilding the
/// seven definitions (seven `HirClassDef`s, each with owned `String`s and
/// `Vec<String>`s) on each of those calls made class binding cost
/// `O(items x classes x 7)` allocations; caching collapses that to a single
/// construction. Measured on `benches/check_bench.rs`'s fixture, the rebuild
/// alone accounted for ~10.4 us of the ~13.2 us the seeding added to
/// `pycc_types::check`.
static SYNTHETIC_CLASS_DEFS: std::sync::LazyLock<Vec<(String, HirClassDef)>> =
    std::sync::LazyLock::new(builtin_exception_class_defs);

/// Whether `def` is exactly the synthetic definition
/// [`builtin_exception_class_defs`] produces for `name` (Part 1 of #541).
///
/// This is how the type checker tells a synthesized builtin exception class
/// apart from a user class that happens to share one of the seven names.
/// Structural equality is sufficient and exact: HIR lowering seeds the
/// synthetic definitions all-or-nothing, and only for a module that
/// references at least one of the seven names and binds none of them
/// itself, so a user-authored `class ValueError:` is never accompanied by a
/// synthetic one.
///
/// The name test comes first so an ordinary user class -- the overwhelmingly
/// common case -- costs one `is_builtin_exception_class` scan rather than up
/// to seven deep `HirClassDef` comparisons.
pub fn is_builtin_exception_class_def(name: &str, def: &HirClassDef) -> bool {
    is_builtin_exception_class(name)
        && SYNTHETIC_CLASS_DEFS
            .iter()
            .any(|(synthetic_name, synthetic_def)| synthetic_name == name && synthetic_def == def)
}

/// Whether `module` spells any of the seven names in
/// [`BUILTIN_EXCEPTION_CLASSES`] anywhere at all (Part 1 of #541).
///
/// HIR lowering seeds the synthetic class definitions only into a module
/// that can actually observe them. Seeding unconditionally put seven
/// `HirClassDef`s into *every* module's class table, and the frontend's
/// per-item work is proportional to the size of that table: the seven
/// entries cost `benches/check_bench.rs`'s class-free fixture roughly 3x its
/// whole `parse` + `lower_checked` + `check` time, which the
/// `frontend-perf-gate` rejected. A module that never names a builtin
/// exception cannot tell a seeded class table from an unseeded one, so it
/// pays nothing.
///
/// Absence must never be read as "the user shadowed this name". It is not:
/// `pycc_types::exception::is_user_defined_class` is
/// `classes.contains_key(name) && !is_synthetic_class(name)`, so an absent
/// name is *not* user-defined, which is exactly the pre-#541 reading.
///
/// Four consumers instead need a seeded class to be *present*: base
/// resolution (`class MyError(ValueError):`), `class::expect_class` behind a
/// `Ty::Instance` (`except ValueError as e: e.args`), `annotation_to_ty`
/// projection (`e: ValueError`), and `isinstance`/`issubclass`. Each of them
/// requires one of the seven to be spelled in the module, so *this* gate
/// never withholds a definition from a module that could have reached one:
/// a module it refuses to seed cannot name a builtin exception at all.
///
/// That is a property of this gate alone, not of the pair. The
/// all-or-nothing shadow gate below can still withhold all seven from a
/// module that spells one of them, whenever the module's top level binds a
/// *different* one -- and `class Exception: ...` plus
/// `except ValueError as e: print(e.args)` still reaches
/// `class::expect_class`'s internal-error panic for exactly that reason.
/// That gap predates this gate (it reproduces identically at Part 1 of
/// #541's own commit, before the reference gate existed) and is unchanged
/// by it; closing it belongs to Part 2, which has to decide what a partially
/// shadowed builtin hierarchy means.
///
/// The walk is [`pycc_ast::visitor::Visitor`], ruff's own generic AST
/// traversal, rather than a hand-rolled `Stmt`/`Expr` match. That is
/// deliberate: a hand-rolled match needs a `_ =>` arm or exhaustive
/// enumeration of every upstream node, and a spelling missed there fails
/// *silently* -- as a spurious `C0001` or an internal-compiler-error abort,
/// not a diagnostic. Overriding only `visit_expr` on the generic walker
/// makes every name-bearing position -- class bases, decorators, `raise`
/// operands, `except` types, annotations, call arguments, attribute values,
/// comprehensions, `match` patterns, f-string interpolations -- reachable by
/// construction, and keeps new upstream AST nodes covered automatically.
/// String forward references (`x: "ValueError"`) are not scanned because
/// `func::annotation_to_ty` does not resolve them either.
pub(crate) fn module_references_builtin_exception_name(module: &ModModule) -> bool {
    struct ReferenceScan {
        found: bool,
    }
    impl<'a> Visitor<'a> for ReferenceScan {
        fn visit_expr(&mut self, expr: &'a Expr) {
            // Once one spelling is seen the answer cannot change, so stop
            // descending rather than walking the rest of the module.
            if self.found {
                return;
            }
            if let Expr::Name(name) = expr
                && is_builtin_exception_class(name.id.as_str())
            {
                self.found = true;
                return;
            }
            visitor::walk_expr(self, expr);
        }
    }
    let mut scan = ReferenceScan { found: false };
    scan.visit_body(&module.body);
    scan.found
}

/// Whether `module`'s own top level binds any of the seven names in
/// [`BUILTIN_EXCEPTION_CLASSES`] (Part 1 of #541).
///
/// This is the second of lowering's two seeding gates: a module is seeded
/// only when [`module_references_builtin_exception_name`] holds *and* this
/// does not. The two ask deliberately different questions and are kept
/// separate rather than fused into one pass -- shadowing is a property of a
/// module's *top level* only (a `class ValueError:` nested inside a function
/// shadows nothing at module scope), while a reference counts at any depth.
///
/// The seeding is all-or-nothing: when this returns `true`, *no* synthetic
/// class is seeded, and the seven names keep exactly the pre-#541 behavior
/// of being recognized by name alone. Two reasons for all-or-nothing rather
/// than per-name:
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
