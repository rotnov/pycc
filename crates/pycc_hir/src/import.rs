//! Module-level `import` statements and type-alias declarations: the
//! statement kinds `module::lower_all` resolves before it walks a module's
//! remaining items (D-135 for aliases, D-136/D-137 for stdlib imports).
//!
//! Extracted from `lib.rs` per AGENTS.md's file-decomposition rule (issue
//! #547, Part 2). This is a low-fan-in cohesion unit: `lower_import_stmt`,
//! `lower_type_alias_stmt`, and `lower_legacy_type_alias_ann_assign` are
//! each called exactly once, and `import_local_name` twice, all from
//! `module::lower_top_level_item` -- which is why `lib.rs` re-exports them `pub(crate)`
//! rather than making them public. The dependency runs the other way for
//! annotations: the two alias lowerings call `annotation_to_ty`, which
//! lives in the sibling `func` module.

use crate::class::ClassAnnotationInfo;
use crate::{ImportBinding, Ty, annotation_to_ty, unresolved_symbol, unsupported};
use pycc_ast::{Expr, Stmt};
use pycc_diag::{Diagnostic, Span};

/// Recognizes a module-level `Stmt::Import`/`Stmt::ImportFrom` and resolves
/// it against `pycc_std`'s registry (D-136/D-137). Returns `Ok(None)` for
/// any other statement kind, leaving it to the caller's own dispatch --
/// mirroring `lower_type_alias_stmt`'s shape exactly.
///
/// D-137 is fail-closed: every recognized-but-out-of-scope shape (multiple
/// names in one `import` statement, an `as` alias, a relative import, an
/// unresolvable module) is `C0001`, the same generic "statement kind not
/// supported yet" diagnostic the crate already uses for every other
/// unimplemented statement kind -- matching the plan's explicit instruction
/// to reuse `C0001` rather than add a new code for "we recognize this is an
/// import but don't support this particular shape." A recognized module
/// with one unresolvable symbol inside an otherwise-valid `from math import
/// ...` list is instead `C0002` (D-136's own decision text), distinguishing
/// "we don't support this import shape at all" from "we support `math`,
/// just not `math.<this-symbol>`" -- and it fails the whole statement, not
/// a partial bind of the names that did resolve.
pub(crate) fn lower_import_stmt(stmt: &Stmt) -> Result<Option<Vec<ImportBinding>>, Diagnostic> {
    match stmt {
        Stmt::Import(import) => {
            let [alias] = import.names.as_slice() else {
                return Err(unsupported(
                    "only a single module per `import` statement is supported so far",
                    import.range,
                ));
            };
            if alias.asname.is_some() {
                return Err(unsupported(
                    "`import ... as ...` aliasing is not supported yet",
                    import.range,
                ));
            }
            let module_name = alias.name.as_str();
            let Some(module) = pycc_std::resolve_module(module_name) else {
                return Err(unsupported(
                    format!("import of module `{module_name}` is not supported yet"),
                    import.range,
                ));
            };
            Ok(Some(vec![ImportBinding::Module {
                local_name: module_name.to_string(),
                module,
            }]))
        }
        Stmt::ImportFrom(import) => {
            if import.level != 0 {
                return Err(unsupported(
                    "a relative import (`from . import ...`) is not supported yet",
                    import.range,
                ));
            }
            // A `level == 0` `Stmt::ImportFrom` always carries a module name
            // -- the only way to reach `module: None` is a relative import
            // (`from . import x`, `from .. import x`, ...), which always
            // has `level >= 1` and is already rejected above. Verified
            // directly against the vendored parser: `from import x` (no
            // dots, no module name) is a parse error (`L0001`, "Expected a
            // module name"), so `lower_checked` never sees this shape at
            // all, matching this file's existing precedent of verifying an
            // "impossible" shape against the real parser rather than
            // assuming it.
            let module_name = import
                .module
                .as_ref()
                .expect("a non-relative `from ... import ...` always names a module")
                .as_str();
            let Some(module) = pycc_std::resolve_module(module_name) else {
                return Err(unsupported(
                    format!("import of module `{module_name}` is not supported yet"),
                    import.range,
                ));
            };
            if import.names.is_empty()
                || import.names.iter().any(|alias| alias.name.as_str() == "*")
            {
                return Err(unsupported(
                    "`from ... import *` (wildcard import) is not supported yet",
                    import.range,
                ));
            }
            let mut bound = Vec::with_capacity(import.names.len());
            for alias in &import.names {
                if alias.asname.is_some() {
                    return Err(unsupported(
                        "`from ... import x as y` aliasing is not supported yet",
                        import.range,
                    ));
                }
                let symbol_name = alias.name.as_str();
                let Some(symbol) = pycc_std::resolve_symbol(module, symbol_name) else {
                    return Err(unresolved_symbol(
                        format!(
                            "module `{module_name}` has no importable symbol named `{symbol_name}`"
                        ),
                        import.range,
                    ));
                };
                bound.push(ImportBinding::Symbol {
                    local_name: symbol_name.to_string(),
                    module,
                    symbol,
                });
            }
            Ok(Some(bound))
        }
        _ => Ok(None),
    }
}

/// Recognizes a PEP 695 `type X = <expr>` statement and evaluates its RHS as
/// a type expression, reusing `annotation_to_ty` (D-135) -- the same
/// resolver used for parameter/return/variable annotations, since a type
/// alias's RHS is syntactically just another type expression. Returns
/// `Ok(None)` for any other statement kind, leaving it to the caller's own
/// dispatch.
///
/// A generic alias (`type X[T] = ...`) is rejected with `T0042`, not the
/// generic `unsupported`/`C0001` catch-all: D-134/D-135 explicitly scope a
/// generic alias out of this PR, but -- unlike, say, `async def`, which is
/// simply unrecognized syntax -- this shape *is* recognized and type-checked
/// far enough to name precisely why it is rejected, the same reasoning
/// `check_generic_function`'s own `T0042` diagnostics already use for a
/// generic function's out-of-scope shapes.
pub(crate) fn lower_type_alias_stmt(
    stmt: &Stmt,
    aliases: &[(String, Ty)],
    class_defs: &[ClassAnnotationInfo],
) -> Result<Option<(String, Ty)>, Diagnostic> {
    let Stmt::TypeAlias(type_alias) = stmt else {
        return Ok(None);
    };
    // `type_alias.type_params` being `Some(_)` at all is enough to reject:
    // `ruff_python_parser`'s own `parse_type_params` reports a parse error
    // (`EmptyTypeParams`, surfaced by this crate's own `pycc_parser::parse`
    // as `L0001` before this function ever runs) for an empty `[]`, so a
    // `Some(type_params)` reaching this point always has at least one entry
    // -- there is no valid parsed input where an extra `.type_params.is_empty()`
    // check here would ever be reached with a `false` result to skip on
    // (confirmed against the pinned `ruff_python_parser = "0.0.6"` registry
    // source, the same way this function's own name-target extraction below
    // documents its own unreachable shape).
    if type_alias.type_params.is_some() {
        let range = std::ops::Range::<u32>::from(type_alias.range);
        return Err(Diagnostic::error(
            "T0042",
            "a generic type alias (`type X[T] = ...`) is not supported yet".to_string(),
            Span::new(range.start, range.end),
        ));
    }
    // Unlike the legacy `AnnAssign` form's target (which can be an
    // `Attribute`/`Subscript`, see `lower_legacy_type_alias_ann_assign`
    // below), `ruff_python_parser`'s own `parse_type_alias_statement`
    // unconditionally builds this field as `Expr::Name(self.parse_name(...))`
    // -- there is no valid source text that parses a `type` statement with a
    // non-name target, so there is no `unsupported`/unreachable fallback
    // branch to write or cover here (confirmed against the pinned
    // `ruff_python_parser = "0.0.6"` registry source). `.expect(...)`, not a
    // hand-rolled panic arm, per this crate's own documented coverage
    // convention (`pycc_ast::re_exported_grammar_types_resolve_and_have_the_expected_shape`'s
    // comment): the panic path lives in libcore, invisible to instrumented
    // regions, the same way `.unwrap()`'s does.
    let name = type_alias
        .name
        .as_name_expr()
        .expect("ruff always parses a `type` statement's name as Expr::Name");
    let ty = annotation_to_ty(&type_alias.value, None, None, aliases, class_defs)?;
    Ok(Some((name.id.to_string(), ty)))
}

/// Recognizes the legacy `X: TypeAlias = <expr>` annotated-assignment form
/// of a type alias (PEP 613). Real Python requires `from typing import
/// TypeAlias` before this annotation is meaningful, but requiring that
/// import here is not merely inconsistent with existing precedent -- it is
/// currently infeasible: `pycc_hir` has no `Stmt::Import`/`Stmt::ImportFrom`
/// handling anywhere in this crate, so `from typing import TypeAlias` would
/// itself be unconditionally rejected with the generic `C0001` ("statement
/// kind not supported yet") diagnostic if pycc tried to require it first.
/// There is no accepted-bare-typing-name precedent to lean on either --
/// `Any` is the only other typing-shaped bare name `annotation_to_ty`
/// currently recognizes, and it is rejected with `T0002`, not accepted. So
/// this function accepts the bare annotation name `TypeAlias`
/// unconditionally, not by analogy to an existing precedent, but because
/// real import verification cannot be expressed with this crate's current
/// statement coverage (plan-deviation note, since the design doc leaves
/// this specific question open; import support is PR-14's).
///
/// Returns `Ok(None)` for any statement that is not this exact shape --
/// including an ordinary `X: TypeAlias` with no value, which is invalid as a
/// type alias and instead falls through to the ordinary `AnnAssign` lowering
/// path, where `annotation_to_ty` rejects the bare name `TypeAlias` with the
/// same `C0001` catch-all as any other unrecognized annotation name.
pub(crate) fn lower_legacy_type_alias_ann_assign(
    stmt: &Stmt,
    aliases: &[(String, Ty)],
    class_defs: &[ClassAnnotationInfo],
) -> Result<Option<(String, Ty)>, Diagnostic> {
    let Stmt::AnnAssign(ann) = stmt else {
        return Ok(None);
    };
    let Expr::Name(annotation_name) = ann.annotation.as_ref() else {
        return Ok(None);
    };
    if annotation_name.id.as_str() != "TypeAlias" {
        return Ok(None);
    }
    let Some(value) = ann.value.as_deref() else {
        return Ok(None);
    };
    let Expr::Name(target) = ann.target.as_ref() else {
        return Ok(None);
    };
    let ty = annotation_to_ty(value, None, None, aliases, class_defs)?;
    Ok(Some((target.id.to_string(), ty)))
}

/// The bound local name of an import, regardless of which `ImportBinding`
/// variant it is -- used by `module::lower_top_level_item`'s class-name-collision check
/// (D-068 review finding on #385) so it does not need to duplicate the
/// match on both variants at its own call site.
pub(crate) fn import_local_name(binding: &ImportBinding) -> &str {
    match binding {
        ImportBinding::Module { local_name, .. } | ImportBinding::Symbol { local_name, .. } => {
            local_name
        }
    }
}
