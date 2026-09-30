//! Return coverage: whether a function body can reach its implicit end.
//!
//! [`block_always_returns`] decides the `T0022` "function `f` can exit
//! without returning `T`" check in `check_function_in`, and the
//! fall-through joins after a `try` statement
//! (`exception::try_join`, `constraints::try_stmt`) reuse it to drop a
//! path that never reaches the statement after it.
//!
//! Extracted from `lib.rs` per AGENTS.md's file-decomposition rule.

use pycc_hir::HirStmt;

pub(crate) fn block_always_returns(body: &[HirStmt]) -> bool {
    for stmt in body {
        let returns = match stmt {
            HirStmt::Return(_) => true,
            HirStmt::If { body, orelse, .. } => {
                !orelse.is_empty() & block_always_returns(body) & block_always_returns(orelse)
            }
            HirStmt::ExprStmt(_)
            | HirStmt::Assign { .. }
            | HirStmt::AnnAssign { .. }
            | HirStmt::While { .. }
            | HirStmt::ForRange { .. }
            | HirStmt::ForList { .. }
            | HirStmt::ForObject { .. }
            | HirStmt::DictSet { .. }
            | HirStmt::AttrSet { .. }
            // #1244: a `del` neither returns nor raises.
            | HirStmt::Delete { .. }
            // #1291: a nested foreign import never returns. Its failure is
            // either a pycc raise (an `ImportError`, bridged by #1293) or a
            // direct exit from `Py_mod_exec` (any other exception, #1096).
            | HirStmt::ForeignImport { .. }
            // PR-12 Task 3 (D-117): a comprehension statement never contains a
            // `return` (its `elt`/`cond`/`key`/`value` are expressions, not
            // statements), so it can never make a block always return, exactly
            // like `Assign`/`ForList`/`DictSet` above.
            | HirStmt::ListCompAssign { .. }
            | HirStmt::SetCompAssign { .. }
            | HirStmt::DictCompAssign { .. } => false,
            // A raise transfers control to an exception handler/caller and
            // cannot fall through to the function's implicit return point.
            HirStmt::Raise { .. } => true,
            HirStmt::Match { cases, .. } => {
                let mut all_cases_return = !cases.is_empty();
                for case in cases {
                    all_cases_return &= block_always_returns(&case.body);
                }
                all_cases_return
            }
            // `except*` shares `Try`'s termination shape exactly: a terminal
            // `finally` replaces every earlier outcome, and otherwise the
            // normal path (body or `else`) and every matched subgroup's
            // handler must all terminate.
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
                // A terminal `finally` replaces every earlier outcome. Otherwise
                // the normal path must terminate either in the try body itself or
                // in its `else`, and every matching handler must terminate. With
                // no handlers, an exception simply propagates to the caller and
                // is already a terminal path.
                let normal_path_terminates = block_always_returns(body)
                    | ((!orelse.is_empty()) & block_always_returns(orelse));
                let mut handled_paths_terminate = true;
                for handler in handlers {
                    handled_paths_terminate &= block_always_returns(&handler.body);
                }
                block_always_returns(finalbody)
                    | (normal_path_terminates & handled_paths_terminate)
            }
        };
        if returns {
            return true;
        }
    }
    false
}
