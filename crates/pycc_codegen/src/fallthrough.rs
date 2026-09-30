//! MIR-level fallthrough proof for LLVM terminator placement.
//!
//! Extracted from `exception.rs` per AGENTS.md's file-decomposition rule.

use pycc_mir::MirStmt;

/// Mirrors the type checker's fallthrough proof at MIR level. Structured
/// exception code generation sometimes leaves an LLVM continuation block
/// whose no-exception edge is statically impossible (for example,
/// `try: raise ... finally: cleanup`). Such a block still needs an LLVM
/// terminator even though it cannot be reached at runtime.
pub(crate) fn block_always_terminates(body: &[MirStmt]) -> bool {
    for stmt in body {
        let terminates = match stmt {
            MirStmt::Return(_)
            // Part 2 of #1175 (#1179): a buffer sub-range egress is a
            // `return`, so it terminates its block exactly as `Return`
            // does. Classified explicitly rather than left to inherit any
            // grouping, because a wrong answer here is a missing LLVM
            // terminator that still compiles.
            | MirStmt::ReturnBufferSlice { .. }
            | MirStmt::Raise { .. }
            | MirStmt::RaiseFrom { .. }
            | MirStmt::Reraise
            | MirStmt::Unreachable => true,
            MirStmt::If { body, orelse, .. } => {
                !orelse.is_empty() & block_always_terminates(body) & block_always_terminates(orelse)
            }
            MirStmt::Seq(stmts) => block_always_terminates(stmts),
            MirStmt::Try {
                body,
                handlers,
                orelse,
                finalbody,
            }
            | MirStmt::TryStar {
                body,
                handlers,
                orelse,
                finalbody,
            } => {
                // These are pure structural predicates. Non-short-circuit boolean
                // operators keep every component explicit to coverage tooling and
                // make the fallthrough proof auditable as a complete truth table.
                // `except*` shares `Try`'s exact fallthrough shape: normal path
                // (body or `else`) plus every handler must terminate, unless
                // `finally` already does.
                let normal_path_terminates = block_always_terminates(body)
                    | ((!orelse.is_empty()) & block_always_terminates(orelse));
                let mut handled_paths_terminate = true;
                for handler in handlers {
                    handled_paths_terminate &= block_always_terminates(&handler.body);
                }
                block_always_terminates(finalbody)
                    | (normal_path_terminates & handled_paths_terminate)
            }
            MirStmt::ExprStmt(_)
            | MirStmt::Assign { .. }
            | MirStmt::NoOp
            | MirStmt::While { .. }
            | MirStmt::ForRange { .. }
            | MirStmt::ForList { .. }
            | MirStmt::ForObject { .. }
            | MirStmt::DictSet { .. }
            | MirStmt::BufferSet { .. }
            | MirStmt::ForDict { .. }
            | MirStmt::ForSet { .. }
            | MirStmt::ListCompAssign { .. }
            | MirStmt::DictCompAssign { .. }
            | MirStmt::SetCompAssign { .. }
            // #1291: the success path falls through. A failed nested foreign
            // import either raises a pycc exception to the innermost target
            // (#1293) or returns from the entry point on its own edge.
            | MirStmt::ForeignImport { .. }
            | MirStmt::AttrSet { .. } => false,
        };
        if terminates {
            return true;
        }
    }
    false
}
