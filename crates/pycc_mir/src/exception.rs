//! MIR exception operands, handlers, and HIR-to-MIR lowering (#382).

use super::{HirClassDef, MirExpr, MirStmt, lower_expr};
use pycc_hir::{HirExpr, Ty};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum MirExceptionValue {
    Constructed {
        type_tag: u8,
        /// The exception class's source name, carried through so codegen can
        /// hand the runtime a name to print for an uncaught exception. Part 2
        /// of #541 (D-189) moved the tag -> name mapping off the runtime's own
        /// `match`, which could only ever name the seven builtin classes.
        class_name: String,
        message: MirExpr,
    },
    Existing(MirExpr),
}

#[derive(Debug, Clone, PartialEq)]
pub struct MirExceptHandler {
    /// The runtime exception type tags this handler accepts, sorted ascending,
    /// or `None` for a bare `except:`. Part 2 of #541 (D-189) widened this
    /// from a single tag to a set: `except MyError:` must also catch every
    /// user-defined subclass of `MyError`, and each subclass carries its own
    /// distinct tag. The vector is sorted so codegen's emitted IR does not
    /// depend on the class table's hash-map iteration order.
    pub exc_type_tag: Option<Vec<u8>>,
    pub binding_name: Option<String>,
    pub binding_ty: Option<Ty>,
    pub body: Vec<MirStmt>,
}

/// `Exception`'s tag, which `pycc_rt_exception_type_matches` treats as a
/// catch-all rather than as an ordinary equality test.
const EXCEPTION_TAG_CATCH_ALL: u8 = 0;

/// Resolves one of the original flat seven builtin exception names to its
/// fixed tag, independent of the class table. Part 2 of #543 (#739) widened
/// [`pycc_hir::BUILTIN_EXCEPTION_CLASSES`] with a 16-member `OSError` family
/// that is *not* resolved here -- those 16 carry their own fixed tag directly
/// on their `HirClassDef` (see `pycc_hir::exception::builtin_exception_class_defs`)
/// and are picked up by [`exception_type_tag`]'s class-table fallback below
/// instead. This `match` implements exactly the 7-name set
/// `pycc_hir::is_flat_builtin_exception_class` names as its single source of
/// truth; the two must be kept in sync by construction, not by a shared
/// constant, since a `match` on `&str` cannot be built from an array at
/// compile time here.
pub(super) fn resolve_exception_tag(name: &str) -> Option<u8> {
    match name {
        "Exception" => Some(0),
        "ValueError" => Some(1),
        "TypeError" => Some(2),
        "KeyError" => Some(3),
        "IndexError" => Some(4),
        "ZeroDivisionError" => Some(5),
        "RuntimeError" => Some(6),
        _ => None,
    }
}

pub(super) fn lower_raise(
    exc: &Option<HirExpr>,
    cause: &Option<HirExpr>,
    scopes: &mut [HashMap<String, Ty>],
    classes: &HashMap<String, HirClassDef>,
    current_class: Option<&str>,
) -> MirStmt {
    let Some(exc) = exc else {
        return MirStmt::Reraise;
    };
    let exception = lower_exception_value(exc, scopes, classes, current_class);
    if let Some(cause) = cause {
        return MirStmt::RaiseFrom {
            exception,
            cause: lower_exception_value(cause, scopes, classes, current_class),
        };
    }
    MirStmt::Raise { exception }
}

pub(super) fn lower_exception_value(
    expr: &HirExpr,
    scopes: &mut [HashMap<String, Ty>],
    classes: &HashMap<String, HirClassDef>,
    current_class: Option<&str>,
) -> MirExceptionValue {
    if let HirExpr::Call { callee, args } = expr
        && let Some(type_tag) = exception_type_tag(callee, classes)
    {
        let message = args.first().map_or_else(
            || MirExpr::StringLiteral("unknown".to_string()),
            |message| lower_expr(message, scopes, classes, current_class),
        );
        return MirExceptionValue::Constructed {
            type_tag,
            class_name: callee.clone(),
            message,
        };
    }
    MirExceptionValue::Existing(lower_expr(expr, scopes, classes, current_class))
}

/// Resolves an exception class name to its runtime type tag, whether it is one
/// of the original flat seven builtins (resolved by name, above), one of the
/// 16-member `OSError` family that carries a fixed tag on its own
/// `HirClassDef` (Part 2 of #543, #739), or a user-defined class that HIR
/// lowering assigned a tag to (Part 2 of #541, D-189) -- the class-table
/// fallback below resolves both of the latter two identically.
pub(super) fn exception_type_tag(name: &str, classes: &HashMap<String, HirClassDef>) -> Option<u8> {
    resolve_exception_tag(name).or_else(|| classes.get(name).and_then(|def| def.exception_type_tag))
}

/// The full set of runtime type tags an `except <name>:` handler accepts,
/// sorted ascending (Part 2 of #541, D-189).
///
/// A handler naming a class must also catch that class's subclasses, and each
/// user-defined exception class carries its own distinct tag, so the set is
/// the named class's own tag plus every raisable class whose MRO reaches it.
/// `except Exception:` is the one shortcut: tag 0 is the runtime's own
/// catch-all in `pycc_rt_exception_type_matches`, so it already matches every
/// tag and needs no companions.
///
/// The result is sorted rather than left in `classes`' iteration order, which
/// is a `HashMap`'s and therefore not reproducible across runs.
pub(super) fn handler_type_tags(name: &str, classes: &HashMap<String, HirClassDef>) -> Vec<u8> {
    let own = exception_type_tag(name, classes)
        .expect("pycc_types rejects unknown exception handler types before MIR");
    if own == EXCEPTION_TAG_CATCH_ALL {
        return vec![own];
    }
    let mut tags: Vec<u8> = classes
        .values()
        .filter(|def| def.name != name)
        .filter_map(|def| {
            def.exception_type_tag
                .filter(|_| def.mro.iter().any(|ancestor| ancestor == name))
        })
        .collect();
    tags.push(own);
    tags.sort_unstable();
    tags
}
