//! The single syntactic definition of *"does this function body store into
//! the buffer parameter named `name`?"* (Part 1 of #1142).
//!
//! It lives in `pycc_hir` rather than in the driver because both consumers
//! need it and neither can derive it from the other: `src/ext_build.rs`'s
//! `collect_exports` and `ctor_descriptor` ask it to decide whether the
//! generated wrapper requests `PyBUF_WRITABLE` for that parameter, and the
//! predicate has to be the *same* question `pycc_types` answers when it
//! admits the store -- a store the checker admits but this walk misses is a
//! write through storage acquired read-only, which is a memory-safety
//! defect rather than a diagnostic one.
//!
//! It is necessarily syntactic. `collect_exports` runs on the typed HIR
//! *after* checking, and `Environment` carries no ext-mode or export-set
//! flag for the checker to consult, so there is no shared typed answer to
//! reuse -- only the one HIR shape both sides see. That shape is exact
//! rather than approximate: `pycc_hir` lowers every `<bare name>[k] = v` to
//! [`HirStmt::DictSet`], whose `dict` field is the target's plain name, so
//! "the body stores into `name`" is a name comparison and nothing more.
//!
//! **Why no aliasing analysis is owed.** Every read of a buffer-bound name
//! other than `name[i]`, `len(name)` and -- since Part 1 of #1142 --
//! `name[i] = v` is still `pycc_types`' `reject_memoryview_read` `C0001`
//! (D-244's 2026-09-17 Part-1-of-#1027 amendment, statement (e)), so a
//! buffer cannot be aliased into a local, passed on, stored or returned: it
//! never leaves the slot its own name binds. A later widening of that read
//! surface would invalidate this walk, and would have to revisit it.
//!
//! **Why a module-level walk answers for a class's methods too.** An
//! exported instance, static or class method reaches the driver as a
//! module-level `HirItem::Function` under its mangled `<Class>.<method>`
//! name, so `collect_exports` hands this predicate that method's own body
//! exactly as it hands it a plain `def`'s. A `Protocol` member with a
//! buffer parameter is satisfiable under `--ext` for the same reason: the
//! implementing class's method, not the protocol's stub, is what carries
//! the body this walk reads.

use crate::HirStmt;

/// Whether `body` contains an element store whose target is the name
/// `name`, at any nesting depth.
///
/// The `match` below is deliberately exhaustive with no `_` arm: a missed
/// recursion is the memory-safety failure mode this predicate exists to
/// prevent, so a future block-carrying `HirStmt` variant must be a compile
/// error here rather than a silent hole.
pub fn body_stores_into(body: &[HirStmt], name: &str) -> bool {
    body.iter().any(|stmt| stmt_stores_into(stmt, name))
}

fn stmt_stores_into(stmt: &HirStmt, name: &str) -> bool {
    match stmt {
        HirStmt::DictSet { dict, .. } => dict == name,
        HirStmt::If { body, orelse, .. } => {
            body_stores_into(body, name) || body_stores_into(orelse, name)
        }
        HirStmt::While { body, .. }
        | HirStmt::ForRange { body, .. }
        | HirStmt::ForList { body, .. }
        | HirStmt::ForObject { body, .. } => body_stores_into(body, name),
        HirStmt::Match { cases, .. } => cases.iter().any(|case| body_stores_into(&case.body, name)),
        HirStmt::Try {
            body,
            handlers,
            orelse,
            finalbody,
        }
        | HirStmt::TryStar {
            body,
            handlers,
            orelse,
            finalbody,
        } => {
            body_stores_into(body, name)
                || handlers
                    .iter()
                    .any(|handler| body_stores_into(&handler.body, name))
                || body_stores_into(orelse, name)
                || body_stores_into(finalbody, name)
        }
        // Every remaining statement carries no nested block, so it can
        // contain no store: a `DictSet` is a statement and never a
        // sub-expression, and no expression position can hold one.
        HirStmt::ExprStmt(_)
        | HirStmt::Assign { .. }
        | HirStmt::AnnAssign { .. }
        | HirStmt::ListCompAssign { .. }
        | HirStmt::DictCompAssign { .. }
        | HirStmt::SetCompAssign { .. }
        | HirStmt::Return(_)
        | HirStmt::AttrSet { .. }
        | HirStmt::Raise { .. } => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HirExceptHandler, HirExpr, HirMatchCase, HirPattern};

    /// `b[0] = 1.0`, the only statement shape that can answer `true`.
    fn store(name: &str) -> HirStmt {
        HirStmt::DictSet {
            dict: name.to_string(),
            key: HirExpr::IntLiteral(0),
            value: HirExpr::FloatLiteral(1.0),
        }
    }

    /// A statement carrying no nested block, so the leaf arm answers it.
    fn leaf() -> HirStmt {
        HirStmt::Return(None)
    }

    #[test]
    fn a_top_level_store_is_found_and_another_name_is_not() {
        assert!(body_stores_into(&[store("b")], "b"));
        assert!(!body_stores_into(&[store("other")], "b"));
        assert!(!body_stores_into(&[leaf()], "b"));
        assert!(!body_stores_into(&[], "b"));
    }

    /// Every block-carrying variant, each with the store in one nested
    /// position: a variant this walk failed to recurse into would let the
    /// wrapper acquire read-only storage the compiled body then writes.
    #[test]
    fn a_store_is_found_inside_every_nested_block() {
        let nested: Vec<Vec<HirStmt>> = vec![
            vec![HirStmt::If {
                test: HirExpr::BoolLiteral(true),
                body: vec![store("b")],
                orelse: vec![],
            }],
            vec![HirStmt::If {
                test: HirExpr::BoolLiteral(true),
                body: vec![],
                orelse: vec![store("b")],
            }],
            vec![HirStmt::While {
                test: HirExpr::BoolLiteral(true),
                body: vec![store("b")],
            }],
            vec![HirStmt::ForRange {
                var: "i".to_string(),
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(1),
                step: HirExpr::IntLiteral(1),
                body: vec![store("b")],
            }],
            vec![HirStmt::ForList {
                var: "i".to_string(),
                list: "xs".to_string(),
                body: vec![store("b")],
            }],
            vec![HirStmt::ForObject {
                var: "i".to_string(),
                iter: Box::new(HirExpr::IntLiteral(0)),
                body: vec![store("b")],
            }],
            vec![HirStmt::Match {
                subject: HirExpr::IntLiteral(0),
                cases: vec![HirMatchCase {
                    pattern: HirPattern::Wildcard,
                    guard: None,
                    body: vec![store("b")],
                }],
            }],
            vec![try_stmt(vec![store("b")], vec![], vec![], vec![])],
            vec![try_stmt(
                vec![],
                vec![HirExceptHandler {
                    exc_type: None,
                    name: None,
                    body: vec![store("b")],
                }],
                vec![],
                vec![],
            )],
            vec![try_stmt(vec![], vec![], vec![store("b")], vec![])],
            vec![try_stmt(vec![], vec![], vec![], vec![store("b")])],
            vec![HirStmt::TryStar {
                body: vec![store("b")],
                handlers: vec![],
                orelse: vec![],
                finalbody: vec![],
            }],
            // Two levels deep, so the recursion is not merely one-deep.
            vec![HirStmt::While {
                test: HirExpr::BoolLiteral(true),
                body: vec![HirStmt::If {
                    test: HirExpr::BoolLiteral(true),
                    body: vec![store("b")],
                    orelse: vec![],
                }],
            }],
        ];
        for (index, body) in nested.iter().enumerate() {
            assert!(body_stores_into(body, "b"), "shape {index}: {body:?}");
            assert!(!body_stores_into(body, "c"), "shape {index}: {body:?}");
        }
    }

    fn try_stmt(
        body: Vec<HirStmt>,
        handlers: Vec<HirExceptHandler>,
        orelse: Vec<HirStmt>,
        finalbody: Vec<HirStmt>,
    ) -> HirStmt {
        HirStmt::Try {
            body,
            handlers,
            orelse,
            finalbody,
        }
    }
}
