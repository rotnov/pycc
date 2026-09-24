//! The `C0001` diagnostic `lower_stmt` reports for a statement kind none
//! of its arms lowers (issue #890). Extracted from `stmt.rs` (#1291) with
//! no logic change.

use crate::unsupported;
use pycc_ast::Stmt;
use pycc_diag::Diagnostic;

/// The diagnostic for `other`, a statement `lower_stmt` has no arm for.
pub(super) fn unsupported_statement(other: &Stmt) -> Diagnostic {
    // Issue #890: name the rejected kind. Four kinds that *are*
    // supported at module level reach `lower_stmt`'s catch-all arm only
    // because they are nested, so naming them by kind alone would falsely say
    // "a function definition is not supported"; each gets a
    // position qualifier instead. Every top-level `Import`/
    // `ImportFrom` goes through `import::lower_import_stmt` before
    // `module::lower_all` ever calls `lower_stmt`, and an `if
    // TYPE_CHECKING:` body is constant-folded by `lower_stmt`, so an import
    // here is always inside a function or block body (a class-body
    // import is rejected by `class.rs` first and never gets here).
    let kind = match other {
        Stmt::FunctionDef(_) => {
            "a `def` nested inside a function or block body \
             (only a module-level `def` or a method in a class body is supported)"
        }
        Stmt::ClassDef(_) => {
            "a `class` nested inside a function or block body \
             (only a module-level `class` is supported)"
        }
        Stmt::Import(_) | Stmt::ImportFrom(_) => {
            "an `import` inside a function or block body \
             (only a module-level import, or one inside an `if TYPE_CHECKING:` guard, is supported)"
        }
        _ => pycc_ast::stmt_kind_name(other),
    };
    unsupported(
        format!("statement kind not supported yet: {kind}"),
        pycc_ast::stmt_range(other),
    )
}
