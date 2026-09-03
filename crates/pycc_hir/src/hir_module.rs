//! The per-module HIR container (`HirModule`), its compile-time-only
//! import side table (`ImportBinding`), and the `killed_names` walk the type
//! checker's narrowing pass runs over a module's statements.
//!
//! Extracted from `lib.rs` per AGENTS.md's file-decomposition rule (issue
//! #898): these three definitions are the module-level surface the driver
//! and `pycc_types` consume, as opposed to the statement/expression node
//! types that stay in `lib.rs`.

use crate::{FStringPart, HirClassDef, HirExpr, HirItem, HirPattern, HirStmt, Ty};
use std::collections::HashSet;

/// Issue #769 follow-up (D-068 re-review of #780, third round): the set of
/// bare names `body` reassigns anywhere within it, recursively -- every
/// name that some execution of `body` could pass to
/// `pycc_types::check_assignment` (checker) or its MIR lowering
/// counterpart, which is exactly the set of names whose `Optional`
/// narrowing overlay entry that assignment kills.
///
/// This exists to support a *kill-prescan*: before checking/lowering a
/// body that can be entered such that a read inside it observes a kill
/// from an execution earlier than that read's own source position -- a
/// loop body re-entered on a later iteration, or an `except` handler
/// reached partway through the `try` body it guards -- the caller drops
/// every currently-narrowed name this function reports from its working
/// narrowing state *before* checking/lowering starts, rather than only
/// from the kill's own source position onward. See
/// `crates/pycc_types/src/narrow.rs`'s module doc comment for the full
/// soundness rationale (loop re-entry and except-from-mid-try both
/// unsound under a single left-to-right source-order pass) and
/// `docs/decisions/` for the decision record narrowing D-199's
/// "never survives a reassignment" consequence to this conservative rule.
///
/// Mirrors exactly the set of statement kinds that route a bare-name
/// target through `check_assignment` in `pycc_types::lib`: `Assign`,
/// a *valued* `AnnAssign` (a value-less `Final` declaration never calls
/// `check_assignment` -- see that function's own doc comment), a
/// `ForRange`/`ForList` loop variable, and both names a list/set/dict
/// comprehension assignment binds (its own loop variable and its result
/// `target`). `DictSet`/`AttrSet` do not rebind a bare name (they mutate
/// a container/attribute the name still refers to) and are correctly
/// excluded. Recurses into every nested body this HIR can hold --
/// `If`/`While`/`ForRange`/`ForList`/`Match`/`Try` -- so a kill nested
/// arbitrarily deep (e.g. `while ...: if flag: x = None`) is still
/// found. This match is intentionally exhaustive over every `HirStmt`
/// variant (no wildcard arm): adding a new statement kind that can kill a
/// binding forces this function to be updated rather than silently
/// under-reporting kills.
pub fn killed_names(body: &[HirStmt]) -> HashSet<String> {
    let mut killed = HashSet::new();
    collect_killed_names(body, &mut killed);
    killed
}

fn collect_killed_names(body: &[HirStmt], killed: &mut HashSet<String>) {
    for stmt in body {
        match stmt {
            HirStmt::Assign { target, .. } => {
                killed.insert(target.clone());
            }
            HirStmt::AnnAssign { target, value, .. } => {
                if value.is_some() {
                    killed.insert(target.clone());
                }
            }
            HirStmt::ForRange { var, body, .. } => {
                killed.insert(var.clone());
                collect_killed_names(body, killed);
            }
            HirStmt::ForList { var, body, .. } => {
                killed.insert(var.clone());
                collect_killed_names(body, killed);
            }
            HirStmt::ListCompAssign { target, var, .. }
            | HirStmt::SetCompAssign { target, var, .. }
            | HirStmt::DictCompAssign { target, var, .. } => {
                killed.insert(target.clone());
                killed.insert(var.clone());
            }
            HirStmt::If { test, body, orelse } => {
                collect_named_expr_targets_in_expr(test, killed);
                collect_killed_names(body, killed);
                collect_killed_names(orelse, killed);
            }
            HirStmt::While { test, body } => {
                collect_named_expr_targets_in_expr(test, killed);
                collect_killed_names(body, killed);
            }
            HirStmt::Match { cases, .. } => {
                for case in cases {
                    // D-068 re-review of #780 (fourth round): a case's
                    // pattern can itself bind bare names (`case x:`, an
                    // `As`/`Sequence`/`Mapping`/`Class` capture) exactly
                    // like `check_pattern` routes through
                    // `check_assignment` in `pycc_types::lib`'s
                    // `check_match` -- these are kills too, not just
                    // statements inside `case.body`. Omitting them left
                    // `apply_kill_prescan` under-reporting a kill routed
                    // through a capturing `match` pattern inside a
                    // re-enterable body (see this function's own doc
                    // comment above).
                    collect_pattern_capture_names_as_killed(&case.pattern, killed);
                    collect_killed_names(&case.body, killed);
                }
            }
            HirStmt::Try {
                body,
                handlers,
                orelse,
                finalbody,
            } => {
                collect_killed_names(body, killed);
                for handler in handlers {
                    // D-068 re-review of #780 (fourth round): `except ...
                    // as name:` binds `name` to the caught exception
                    // instance before the handler body runs -- the same
                    // kill of a bare name that a plain `Assign` performs
                    // (see the paired `check_assignment`/`bind` fix in
                    // `crates/pycc_types/src/exception.rs` and
                    // `crates/pycc_mir/src/stmt.rs`'s `Try` handler
                    // arm). Without this, `apply_kill_prescan` treated a
                    // handler's own `as` binding as invisible, letting a
                    // later re-entry into a body containing this handler
                    // (e.g. this `Try` nested inside a `while` loop)
                    // still read the pre-handler narrowed type.
                    if let Some(name) = &handler.name {
                        killed.insert(name.clone());
                    }
                    collect_killed_names(&handler.body, killed);
                }
                collect_killed_names(orelse, killed);
                collect_killed_names(finalbody, killed);
            }
            HirStmt::TryStar {
                body,
                handlers,
                orelse,
                finalbody,
            } => {
                // D-068 re-review of #780 (rebase onto #542's except* landing):
                // mirrors the plain `Try` arm above -- each `except* T as
                // name:` clause binds `name` to the caught `ExceptionGroup`
                // before the handler body runs, exactly the same bare-name
                // kill a plain `except ... as name:` performs (see the
                // paired MIR-side `kill_narrowing` fix in
                // `crates/pycc_mir/src/stmt.rs`'s `TryStar` handler arm).
                collect_killed_names(body, killed);
                for handler in handlers {
                    if let Some(name) = &handler.name {
                        killed.insert(name.clone());
                    }
                    collect_killed_names(&handler.body, killed);
                }
                collect_killed_names(orelse, killed);
                collect_killed_names(finalbody, killed);
            }
            HirStmt::ExprStmt(expr) => {
                collect_named_expr_targets_in_expr(expr, killed);
            }
            HirStmt::DictSet { .. }
            | HirStmt::AttrSet { .. }
            | HirStmt::Return(_)
            | HirStmt::Raise { .. } => {}
        }
    }
}

/// D-068 re-review of #780 (fourth round): walks `pattern` for every bare
/// capture name it binds (recursively through `Sequence`/`SequenceStar`/
/// `Mapping`/`Class`/`Or`/`As`) and inserts each into `killed`. Mirrors
/// `pycc_types::collect_pattern_capture_names`'s own identical walk
/// (duplicated here rather than shared because `pycc_hir` has no dependency
/// on `pycc_types`, and `check_match`'s `check_pattern` routes every one of
/// these names through `check_assignment` -- see `collect_killed_names`'s
/// `Match` arm above and its own doc comment's inclusion criterion).
fn collect_pattern_capture_names_as_killed(pattern: &HirPattern, killed: &mut HashSet<String>) {
    match pattern {
        HirPattern::Wildcard
        | HirPattern::Literal(_)
        | HirPattern::Singleton(_)
        | HirPattern::NoneSingleton => {}
        HirPattern::Capture(name) => {
            killed.insert(name.clone());
        }
        HirPattern::Sequence(subs) | HirPattern::Or(subs) => {
            for sub in subs {
                collect_pattern_capture_names_as_killed(sub, killed);
            }
        }
        HirPattern::SequenceStar(subs, rest) => {
            for sub in subs {
                collect_pattern_capture_names_as_killed(sub, killed);
            }
            if let Some(rest) = rest {
                killed.insert(rest.clone());
            }
        }
        HirPattern::Mapping(pairs, rest) => {
            for (_, sub) in pairs {
                collect_pattern_capture_names_as_killed(sub, killed);
            }
            if let Some(rest) = rest {
                killed.insert(rest.clone());
            }
        }
        HirPattern::Class {
            positional,
            keyword,
            ..
        } => {
            for sub in positional {
                collect_pattern_capture_names_as_killed(sub, killed);
            }
            for (_, sub) in keyword {
                collect_pattern_capture_names_as_killed(sub, killed);
            }
        }
        HirPattern::As(inner, name) => {
            collect_pattern_capture_names_as_killed(inner, killed);
            killed.insert(name.clone());
        }
    }
}

/// D-068 review of #780/#774's interaction (blocker finding 2): walks `expr`
/// for every `HirExpr::NamedExpr { name, .. }` node at any depth and inserts
/// each `name` into `killed`. A bare walrus (`(x := None)`) is a reassignment
/// exactly like `HirStmt::Assign`, but it never appears as its own
/// `HirStmt` variant -- `lower_stmt`'s placement restriction (PEP 572) only
/// allows a `NamedExpr` inside a bare `HirStmt::ExprStmt`'s expression or an
/// `If`/`While` statement's `test`, so [`collect_killed_names`] must inspect
/// those two expression positions directly instead of relying on a
/// statement-level match arm the way every other kill kind above does.
///
/// Before this fix, `collect_killed_names` put a bare `ExprStmt` in a no-op
/// arm and never inspected `If`/`While`'s own `test`, so a re-enterable loop
/// body whose only kill of a narrowed name was a walrus (see
/// `docs/decisions/D-206-kill-prescan-for-re-enterable-narrowed-bodies.md`)
/// was invisible to the prescan: a read textually before the walrus within
/// the loop body was wrongly treated as still narrowed on every iteration
/// after the first, even though the *previous* iteration's walrus already
/// killed it. Mirrors `pycc_types::collect_named_expr_names_in_expr`'s own
/// identical walk (duplicated here rather than shared because `pycc_hir` has
/// no dependency on `pycc_types`, and this function only needs to collect
/// names, not also bind their inferred types the way that one does).
fn collect_named_expr_targets_in_expr(expr: &HirExpr, killed: &mut HashSet<String>) {
    match expr {
        HirExpr::NamedExpr { name, value } => {
            collect_named_expr_targets_in_expr(value, killed);
            killed.insert(name.clone());
        }
        HirExpr::IntLiteral(_)
        | HirExpr::FloatLiteral(_)
        | HirExpr::BoolLiteral(_)
        | HirExpr::StringLiteral(_)
        | HirExpr::NoneLiteral
        | HirExpr::Name(_)
        | HirExpr::ListPop { .. }
        | HirExpr::Super => {}
        HirExpr::Call { args, .. } => {
            for arg in args {
                collect_named_expr_targets_in_expr(arg, killed);
            }
        }
        HirExpr::BinOp { left, right, .. } | HirExpr::Compare { left, right, .. } => {
            collect_named_expr_targets_in_expr(left, killed);
            collect_named_expr_targets_in_expr(right, killed);
        }
        HirExpr::UnaryOp { operand, .. } => collect_named_expr_targets_in_expr(operand, killed),
        HirExpr::FString(parts) => {
            for part in parts {
                if let FStringPart::Interpolation(inner) = part {
                    collect_named_expr_targets_in_expr(inner, killed);
                }
            }
        }
        HirExpr::ListLiteral(es) | HirExpr::SetLiteral(es) | HirExpr::TupleLiteral(es) => {
            for e in es {
                collect_named_expr_targets_in_expr(e, killed);
            }
        }
        HirExpr::Subscript { base, index } => {
            collect_named_expr_targets_in_expr(base, killed);
            collect_named_expr_targets_in_expr(index, killed);
        }
        HirExpr::Slice {
            base,
            start,
            stop,
            step,
        } => {
            collect_named_expr_targets_in_expr(base, killed);
            for bound in [start, stop, step].into_iter().flatten() {
                collect_named_expr_targets_in_expr(bound, killed);
            }
        }
        HirExpr::ListAppend { value, .. } | HirExpr::SetAdd { value, .. } => {
            collect_named_expr_targets_in_expr(value, killed);
        }
        HirExpr::DictLiteral(pairs) => {
            for (k, v) in pairs {
                collect_named_expr_targets_in_expr(k, killed);
                collect_named_expr_targets_in_expr(v, killed);
            }
        }
        HirExpr::DictGetOrDefault { key, default, .. } => {
            collect_named_expr_targets_in_expr(key, killed);
            collect_named_expr_targets_in_expr(default, killed);
        }
        HirExpr::AttrGet { base, .. } => collect_named_expr_targets_in_expr(base, killed),
        HirExpr::MethodCall { base, args, .. } => {
            collect_named_expr_targets_in_expr(base, killed);
            for arg in args {
                collect_named_expr_targets_in_expr(arg, killed);
            }
        }
        HirExpr::GenericClassInstantiate { args, .. } => {
            for arg in args {
                collect_named_expr_targets_in_expr(arg, killed);
            }
        }
    }
}

/// A compile-time-only import binding recorded by a module-level
/// `import`/`from ... import ...` statement resolved against `pycc_std`'s
/// registry (D-136/D-137). Mirrors `type_aliases`' side-table shape: an
/// import has zero runtime footprint of its own (no `HirStmt`/`HirItem` is
/// produced for it), it only makes a later name/attribute lookup resolve to
/// a stdlib registry entry.
#[derive(Debug, Clone, PartialEq)]
pub enum ImportBinding {
    /// `import math` -- binds `math` (or, for a dotted-but-single-segment
    /// name, whatever `local_name` ends up being; D-137 rejects every
    /// import shape other than a single bare recognized module name, so in
    /// practice `local_name` always equals the resolved module's source
    /// spelling) as a module namespace marker. `math` itself never carries
    /// a `Ty` -- only `math.<attr>` attribute access on this bound name
    /// resolves further, via `pycc_std::resolve_symbol`.
    Module {
        local_name: String,
        module: pycc_std::StdModule,
    },
    /// `from math import sqrt` -- binds `sqrt` directly to the resolved
    /// registry symbol, as if it were a fixed, non-inferred `Ty`/signature
    /// (the alias table from PR-13 is the closest existing precedent).
    Symbol {
        local_name: String,
        module: pycc_std::StdModule,
        symbol: pycc_std::StdSymbol,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct HirModule {
    pub items: Vec<HirItem>,
    /// Compile-time-only name-to-`Ty` bindings from a `type X = <expr>`
    /// statement or a legacy `X: TypeAlias = <expr>` annotated assignment
    /// (D-135). Populated in source order by `module::lower_all` as it walks the
    /// module body. Neither form lowers to any `HirItem`/`HirStmt` of its
    /// own -- the alias has zero HIR/MIR/codegen/runtime footprint, and this
    /// field exists purely so a later annotation naming the alias resolves
    /// to the same `Ty` (see `annotation_to_ty`'s alias-table lookup).
    pub type_aliases: Vec<(String, Ty)>,
    /// Compile-time-only stdlib import bindings (D-136/D-137), populated in
    /// source order by `module::lower_all` exactly like `type_aliases`. Only a
    /// module-level `import`/`from ... import ...` statement is recognized
    /// here -- one nested inside a function body or any other block still
    /// reaches plain `lower_stmt`, which has no arm for `Stmt::Import`/
    /// `Stmt::ImportFrom` and falls through to the generic `C0001`
    /// catch-all, exactly like every other statement kind this compiler
    /// does not support inside a nested block.
    pub imports: Vec<ImportBinding>,
    /// Class name -> declared shape (attribute slots in first-`__init__`-
    /// assignment order, method table) (D-154, Part 1 of #375). Populated by
    /// `class::lower_class` as `module::lower_all` walks the module body, in
    /// source order, mirroring `type_aliases`/`imports`'s own shape: a class
    /// definition has no `HirItem`/`HirStmt` footprint of its own (unlike a
    /// top-level function) -- only its individual methods do, each lowered
    /// into `items` as an ordinary mangled `HirItem::Function` (see
    /// `class::lower_class`'s own doc comment for the mangling scheme and
    /// the reasoning for not adding a dedicated `HirItem::ClassDef` variant).
    pub class_defs: Vec<(String, HirClassDef)>,
    /// Provenance for the builtin exception hierarchy (Part 1 of #541,
    /// D-188): `true` exactly when *this* lowering pass seeded the seven
    /// `BUILTIN_EXCEPTION_CLASSES` entries into `class_defs`, and `false`
    /// for every module whose classes are all user-authored.
    ///
    /// Seeding is all-or-nothing and its shadow gate guarantees no user
    /// top-level binding of any of the seven names survives alongside it,
    /// so this single flag plus `is_builtin_exception_class` identifies the
    /// synthetic entries exactly -- see `pycc_types`'s `bind_classes`.
    /// Provenance is recorded here rather than re-derived downstream
    /// because *no* property of a `HirClassDef`'s own shape is a sound
    /// proxy for who produced it: a user can author a class that is
    /// byte-for-byte identical to a synthetic one.
    pub seeded_builtin_exception_classes: bool,
}
