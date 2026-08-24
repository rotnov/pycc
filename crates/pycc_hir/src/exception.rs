//! HIR exception-class metadata and handler shape (PEP 3110, #382).

use super::{HirClassDef, HirItem, HirStmt, Ty};
use pycc_ast::visitor::{self, Visitor};
use pycc_ast::{Expr, ModModule, Stmt};

pub const BUILTIN_EXCEPTION_CLASSES: [&str; 23] = [
    "Exception",
    "ValueError",
    "TypeError",
    "KeyError",
    "IndexError",
    "ZeroDivisionError",
    "RuntimeError",
    // Part 2 of #543 (#739): PEP 3151 `OSError` hierarchy, tags 7..=22.
    "OSError",
    "BlockingIOError",
    "ChildProcessError",
    "ConnectionError",
    "FileExistsError",
    "FileNotFoundError",
    "InterruptedError",
    "IsADirectoryError",
    "NotADirectoryError",
    "PermissionError",
    "ProcessLookupError",
    "TimeoutError",
    "BrokenPipeError",
    "ConnectionAbortedError",
    "ConnectionRefusedError",
    "ConnectionResetError",
];

/// Part 2 of #541 (D-189): the number of names in [`BUILTIN_EXCEPTION_CLASSES`]
/// that predate Part 2 of #543 (#739) and are resolved by name, independent of
/// the class table, through `pycc_mir::exception::resolve_exception_tag`'s
/// hardcoded `match`. Everything from this index onward -- the PEP 3151
/// `OSError` family -- carries a fixed `Some(index as u8)` tag on its
/// [`HirClassDef`] instead (see [`builtin_exception_class_defs`]) and has no
/// entry in that `match`.
///
/// This is deliberately a separate, smaller constant from
/// [`FIRST_USER_EXCEPTION_TYPE_TAG`] (7 vs. 23) and the two must never be
/// conflated: this one names "the first index in `BUILTIN_EXCEPTION_CLASSES`
/// that is *not* one of the original flat seven", while
/// `FIRST_USER_EXCEPTION_TYPE_TAG` names "the first tag available to a user's
/// own classes" (one past the *whole* builtin array, including the `OSError`
/// family).
const FIRST_OSERROR_FAMILY_TAG: usize = 7;

/// Part 2 of #543 (#739): whether `name` is one of the original seven
/// builtin exception classes -- the ones resolved by name through
/// `pycc_mir::exception::resolve_exception_tag`'s hardcoded `match`,
/// independent of whether a `HirClassDef` for it is present in the class
/// table. Used by `pycc_types::exception::is_unshadowed_builtin_exception` to
/// decide which builtin names may be treated as "unshadowed" even when the
/// class table withheld seeding (see `module_shadows_builtin_exception_name`)
/// -- the flat seven may be, by the same table-independent-resolution
/// property that keeps `resolve_exception_tag` correct for them; the sixteen
/// `OSError`-family names, which carry no such fallback, may not.
pub fn is_flat_builtin_exception_class(name: &str) -> bool {
    BUILTIN_EXCEPTION_CLASSES[..FIRST_OSERROR_FAMILY_TAG].contains(&name)
}

/// Part 2 of #541 (D-189): the first runtime exception type tag available to
/// a user-defined exception class. Tags `0..=22` are permanently reserved for
/// [`BUILTIN_EXCEPTION_CLASSES`], in that array's order: `0..=6` for the
/// original flat seven, resolved by name; `7..=22` for the PEP 3151
/// `OSError` family added by Part 2 of #543 (#739), resolved by a fixed tag
/// stored on each class's own [`HirClassDef`].
pub const FIRST_USER_EXCEPTION_TYPE_TAG: u8 = BUILTIN_EXCEPTION_CLASSES.len() as u8;

/// Part 2 of #541 (D-189): how many user-defined exception classes one module
/// may declare. The runtime carries the type tag as a `u8`, so the whole
/// hierarchy is capped at 256 types; the 23 builtins (Part 2 of #543, #739,
/// added the 16-member `OSError` family to the original flat seven) take the
/// low tags and the remaining `23..=255` are available to the module's own
/// classes. Exceeding this is rejected with `C0001` during HIR lowering.
pub const MAX_USER_EXCEPTION_CLASSES: usize = 256 - BUILTIN_EXCEPTION_CLASSES.len();

pub fn is_builtin_exception_class(name: &str) -> bool {
    BUILTIN_EXCEPTION_CLASSES.contains(&name)
}

/// Returns the builtin exception class's parent, or `None` for the root and
/// unknown names.
///
/// Part 1 of #541 supported only a flat, two-level hierarchy (every
/// non-`Exception` name's parent was `Exception`). Part 2 of #543 (#739)
/// added the real PEP 3151 `OSError` tree: `OSError` and the other six
/// original names are still direct children of `Exception`; ten more names
/// are direct children of `OSError`; and four more (`BrokenPipeError`,
/// `ConnectionAbortedError`, `ConnectionRefusedError`, `ConnectionResetError`)
/// are children of `ConnectionError`, itself a direct child of `OSError`.
pub fn builtin_exception_parent(name: &str) -> Option<&'static str> {
    match name {
        "Exception" => None,
        "BrokenPipeError" | "ConnectionAbortedError" | "ConnectionRefusedError"
        | "ConnectionResetError" => Some("ConnectionError"),
        "OSError" | "ValueError" | "TypeError" | "KeyError" | "IndexError"
        | "ZeroDivisionError" | "RuntimeError" => Some("Exception"),
        "BlockingIOError" | "ChildProcessError" | "ConnectionError" | "FileExistsError"
        | "FileNotFoundError" | "InterruptedError" | "IsADirectoryError"
        | "NotADirectoryError" | "PermissionError" | "ProcessLookupError" | "TimeoutError" => {
            Some("OSError")
        }
        _ => None,
    }
}

/// The mangled `HirItem::Function` name of the synthetic `Exception`
/// constructor (Part 1 of #541). Uses the same `<ClassName>.<method>`
/// mangling every user-defined method already uses, so no downstream
/// consumer needs to special-case it.
pub const EXCEPTION_INIT_MANGLED_NAME: &str = "Exception.__init__";

/// Builds the 23 synthetic [`HirClassDef`]s that give the builtin
/// exception hierarchy a first-class presence in the class table
/// (Part 1 of #541, extending D-173; widened from 7 to 23 by Part 2 of
/// #543/#739's PEP 3151 `OSError` family).
///
/// Before this existed, `Exception`/`ValueError`/... were recognized only
/// by name, through [`is_builtin_exception_class`], with no `HirClassDef`
/// behind them. That left `class MyError(ValueError):` resolving a base
/// that the class table did not contain, and left every consumer of
/// `Environment::classes` unable to tell "not a class" from "a class the
/// frontend never materialized".
///
/// Lowering seeds them only into a module that actually references one of
/// the 23 names and shadows none of them -- see this module's
/// (crate-private) `module_references_builtin_exception_name` and
/// `module_shadows_builtin_exception_name`.
///
/// The definitions are derived from [`BUILTIN_EXCEPTION_CLASSES`] and
/// [`builtin_exception_parent`], so the hierarchy has exactly one source of
/// truth. `Exception` is the root (no bases); the other six original names
/// derive from it directly, as does `OSError`; the `OSError` family
/// (Part 2 of #543/#739) reaches up to three levels deep (e.g.
/// `BrokenPipeError` -> `ConnectionError` -> `OSError` -> `Exception`), so
/// each entry's MRO is built by walking [`builtin_exception_parent`] up to
/// the root rather than a fixed-length push -- the same linearization
/// `compute_c3_mro` would produce, written directly here because the shape
/// is fixed and single-inheritance.
///
/// Only `Exception` carries a method: `__init__(self, message: str)`. Every
/// other class inherits it through its MRO, exactly as a user subclass
/// would. The synthetic classes deliberately carry no attribute slots
/// (`attrs` is empty): D-173 propagates a raised exception through global
/// runtime state rather than through an allocated instance with fields, so
/// there is no storage for a `message` slot to name. See
/// `docs/RUNTIME.md`.
pub fn builtin_exception_class_defs() -> Vec<(String, HirClassDef)> {
    BUILTIN_EXCEPTION_CLASSES
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let parent = builtin_exception_parent(name);
            let bases: Vec<String> = parent.map(|p| vec![p.to_string()]).unwrap_or_default();
            // Part 2 of #543 (#739): the `OSError` family is up to three
            // levels deep (e.g. `BrokenPipeError` -> `ConnectionError` ->
            // `OSError` -> `Exception`), so the MRO is a walk up the parent
            // chain rather than the single push the flat, two-level
            // hierarchy needed.
            let mut mro = vec![(*name).to_string()];
            let mut walk = parent;
            while let Some(p) = walk {
                mro.push(p.to_string());
                walk = builtin_exception_parent(p);
            }
            let methods = if parent.is_none() {
                vec![(
                    "__init__".to_string(),
                    EXCEPTION_INIT_MANGLED_NAME.to_string(),
                )]
            } else {
                Vec::new()
            };
            let def = HirClassDef {
                // Part 2 of #543 (#739): the original flat seven keep `None`
                // (resolved by name, table-independent, through
                // `resolve_exception_tag`); the 16-member `OSError` family
                // gets a fixed `Some(index)` tag stored directly on the
                // `HirClassDef`, since it has no such name-based fallback.
                exception_type_tag: (index >= FIRST_OSERROR_FAMILY_TAG).then_some(index as u8),
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

/// Whether `module` spells any of the [`BUILTIN_EXCEPTION_CLASSES`] names
/// anywhere at all (Part 1 of #541; the array grew from 7 to 23 entries in
/// Part 2 of #543/#739, this gate's own logic is name-count-agnostic and
/// unchanged).
///
/// HIR lowering seeds the synthetic class definitions only into a module
/// that can actually observe them. Seeding unconditionally put the whole
/// `HirClassDef` array into *every* module's class table, and the frontend's
/// per-item work is proportional to the size of that table: the original seven
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
/// requires one of the 23 to be spelled in the module, so *this* gate
/// never withholds a definition from a module that could have reached one:
/// a module it refuses to seed cannot name a builtin exception at all.
///
/// That is a property of this gate alone, not of the pair. The
/// all-or-nothing shadow gate below can still withhold all 23 from a
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

/// Whether `module`'s own top level binds any of the 23 names in
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
/// class is seeded, and the 23 names keep exactly the pre-#541 behavior
/// of being recognized by name alone. Two reasons for all-or-nothing rather
/// than per-name:
///
/// * Seeding a subset would make the hierarchy incoherent -- a synthetic
///   `ValueError` whose `bases`/`mro` name an `Exception` that a user's own
///   `class Exception:` has replaced resolves its inherited `__init__`
///   against the wrong class.
/// * It makes `HirModule::seeded_builtin_exception_classes` -- the single
///   flag `lower_checked` records when it seeds -- an exact provenance
///   record. Because a module containing a user binding of any of the 23
///   names carries no synthetic definitions at all, "the flag is set and the
///   name is one of the 23" identifies precisely the compiler-produced
///   entries, with no user class among them. Provenance is never re-derived
///   from a definition's shape: a user can author a class structurally
///   identical to a synthetic one (see D-188).
///
/// The scan is deliberately conservative: it reports `true` for any
/// top-level `class`/`def`/`type`-alias/annotated-assignment/assignment
/// target spelling one of the 23 names, whether or not that particular
/// spelling would go on to collide with a seeded definition. Over-reporting
/// only costs the module its synthetic classes; under-reporting would let a
/// user definition collide with a compiler-synthesized one and surface as a
/// spurious `C0001`.
///
/// `import`/`from ... import ...` are not scanned: D-136/D-137 resolve every
/// import against `pycc_std`'s registry, which contains none of the 23
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

/// Whether `expr`, used as an assignment target, binds one of the 23
/// builtin exception names. Recurses through the unpacking-target shapes
/// (`a, b = ...`, `[a, b] = ...`, `*rest`) so a name buried in one is still
/// seen. Any other target shape (an attribute, a subscript) rebinds
/// something other than a bare module-level name, so it cannot shadow one
/// of the 23.
fn expr_binds_builtin_exception_name(expr: &Expr) -> bool {
    match expr {
        Expr::Name(name) => is_builtin_exception_class(name.id.as_str()),
        Expr::Tuple(tuple) => tuple.elts.iter().any(expr_binds_builtin_exception_name),
        Expr::List(list) => list.elts.iter().any(expr_binds_builtin_exception_name),
        Expr::Starred(starred) => expr_binds_builtin_exception_name(&starred.value),
        _ => false,
    }
}

/// A single `except` handler. `exc_type` is `None` for bare `except:`.
/// `Some(names)` names one or more exception types (PEP 758 `except A, B:`
/// and `except (A, B):` both lower to the same shape, `names` always in
/// source order); `names` is never empty -- an empty list (`except ():`) is
/// rejected with `C0001` at the one site that constructs a fresh handler
/// from source (`lower_except_handler`), so every downstream consumer may
/// assume non-emptiness without re-checking. HIR-to-HIR rewrite passes
/// (e.g. `pycc_types::unroll_enum_loops_in_stmts`) only clone this field
/// through unchanged and introduce no second construction site; a future
/// pass that builds a `HirExceptHandler` with new logic rather than a pure
/// clone would need to uphold the same non-empty invariant itself.
/// `name` is the optional `as` binding.
#[derive(Debug, Clone, PartialEq)]
pub struct HirExceptHandler {
    pub exc_type: Option<Vec<String>>,
    pub name: Option<String>,
    pub body: Vec<HirStmt>,
}

/// The representative exception type name for a multi-type `except`
/// handler's `as` binding, generic-call rewriting, and constraint
/// collection. All four binding sites (MIR lowering, type checking,
/// monomorphization, constraint collection) must agree on the same name,
/// so they all call this one helper rather than inlining `names[0]`
/// independently. Binds to the first-listed type in source order (see the
/// D-195 decision entry for the two rejected alternatives).
///
/// # Panics
/// Panics if `names` is empty. `HirExceptHandler::exc_type` is documented
/// as never containing an empty `Vec`, so callers passing that field's
/// contents never trigger this.
pub fn except_handler_binding_type_name(names: &[String]) -> &String {
    names.first().expect("except handler type list must not be empty")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_exception_table_and_tree_shape_are_consistent() {
        // Every name in the table is recognized, and every name's declared
        // parent (if any) is itself in the table -- the tree has no dangling
        // edges. Part 2 of #543 (#739) replaced the flat, every-parent-is-
        // `Exception` shape with a real tree, so this no longer asserts a
        // single fixed parent for every non-root name; see the two
        // OSError-family tests below for the parts of the shape that matter.
        for name in BUILTIN_EXCEPTION_CLASSES {
            assert!(is_builtin_exception_class(name));
            if let Some(parent) = builtin_exception_parent(name) {
                assert!(
                    is_builtin_exception_class(parent),
                    "`{name}`'s parent `{parent}` must itself be in the table"
                );
            } else {
                assert_eq!(name, "Exception", "only `Exception` has no parent");
            }
        }
        assert!(!is_builtin_exception_class("NotAnException"));
        assert_eq!(builtin_exception_parent("NotAnException"), None);
    }

    #[test]
    fn the_original_flat_seven_still_derive_directly_from_exception() {
        for name in ["ValueError", "TypeError", "KeyError", "IndexError", "ZeroDivisionError", "RuntimeError"] {
            assert_eq!(builtin_exception_parent(name), Some("Exception"));
        }
    }

    #[test]
    fn the_oserror_family_has_the_real_pep_3151_tree_shape() {
        // `OSError` itself and its ten non-`ConnectionError` direct children
        // are direct children of `Exception`/`OSError` respectively;
        // `ConnectionError`'s own four children are one level deeper still.
        assert_eq!(builtin_exception_parent("OSError"), Some("Exception"));
        for name in [
            "BlockingIOError",
            "ChildProcessError",
            "ConnectionError",
            "FileExistsError",
            "FileNotFoundError",
            "InterruptedError",
            "IsADirectoryError",
            "NotADirectoryError",
            "PermissionError",
            "ProcessLookupError",
            "TimeoutError",
        ] {
            assert_eq!(builtin_exception_parent(name), Some("OSError"));
        }
        for name in [
            "BrokenPipeError",
            "ConnectionAbortedError",
            "ConnectionRefusedError",
            "ConnectionResetError",
        ] {
            assert_eq!(builtin_exception_parent(name), Some("ConnectionError"));
        }
    }

    /// Work item 4 of the Part 2 of #543 (#739) implementation plan: pins the
    /// exact tag-to-name mapping so a future edit cannot silently reorder
    /// [`BUILTIN_EXCEPTION_CLASSES`] and renumber every tag with no test
    /// catching it.
    #[test]
    fn builtin_exception_tags_are_pinned_to_their_array_index() {
        let defs = builtin_exception_class_defs();
        // `.expect` rather than `.unwrap_or_else(|| panic!(...))`: every name
        // this closure is called with is drawn from `BUILTIN_EXCEPTION_CLASSES`
        // itself, so `.find` always succeeds and a user-code panic closure
        // would be an uncovered region under the 100% coverage gate. `.expect`
        // keeps the same failure message without adding an unreachable-in-practice
        // branch to this file's own instrumented code.
        let tag_of = |name: &str| {
            defs.iter()
                .find(|(n, _)| n == name)
                .expect("name must be synthesized")
                .1
                .exception_type_tag
        };
        // The original flat seven carry no tag of their own -- they are
        // resolved by name, independent of the class table.
        for name in [
            "Exception",
            "ValueError",
            "TypeError",
            "KeyError",
            "IndexError",
            "ZeroDivisionError",
            "RuntimeError",
        ] {
            assert_eq!(tag_of(name), None, "`{name}` must carry no assigned tag");
        }
        // The OSError family's tags are pinned to the array order declared
        // in `BUILTIN_EXCEPTION_CLASSES`.
        for (name, tag) in [
            ("OSError", 7),
            ("BlockingIOError", 8),
            ("ChildProcessError", 9),
            ("ConnectionError", 10),
            ("FileExistsError", 11),
            ("FileNotFoundError", 12),
            ("InterruptedError", 13),
            ("IsADirectoryError", 14),
            ("NotADirectoryError", 15),
            ("PermissionError", 16),
            ("ProcessLookupError", 17),
            ("TimeoutError", 18),
            ("BrokenPipeError", 19),
            ("ConnectionAbortedError", 20),
            ("ConnectionRefusedError", 21),
            ("ConnectionResetError", 22),
        ] {
            assert_eq!(tag_of(name), Some(tag), "`{name}` must carry tag {tag}");
        }
    }

    #[test]
    fn is_flat_builtin_exception_class_recognizes_exactly_the_original_seven() {
        for name in [
            "Exception",
            "ValueError",
            "TypeError",
            "KeyError",
            "IndexError",
            "ZeroDivisionError",
            "RuntimeError",
        ] {
            assert!(is_flat_builtin_exception_class(name), "`{name}` must be flat");
        }
        for name in ["OSError", "FileNotFoundError", "ConnectionResetError", "NotAnException"] {
            assert!(
                !is_flat_builtin_exception_class(name),
                "`{name}` must not be flat"
            );
        }
    }

    #[test]
    fn except_handler_binding_type_name_picks_the_first_listed_name() {
        assert_eq!(
            except_handler_binding_type_name(&[
                "ValueError".to_string(),
                "TypeError".to_string(),
            ]),
            "ValueError"
        );
        assert_eq!(
            except_handler_binding_type_name(&["ValueError".to_string()]),
            "ValueError"
        );
    }

    #[test]
    #[should_panic(expected = "except handler type list must not be empty")]
    fn except_handler_binding_type_name_panics_on_empty_slice() {
        // `HirExceptHandler::exc_type` is documented as never `Some(vec![])`
        // (enforced at `lower_except_handler`, the one production site), so
        // this panic is unreachable through normal lowering -- but the
        // invariant is pinned here, at the point it is actually consumed,
        // independent of whichever HIR lowering path currently happens to
        // prevent it from firing (deep-reviewer finding on #740).
        except_handler_binding_type_name(&[]);
    }
}

#[cfg(test)]
mod synthetic_class_tests;

#[cfg(test)]
mod tag_tests;
