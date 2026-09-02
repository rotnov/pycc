//! Module-level lowering: the single left-to-right walk over a module's
//! top-level statements that dispatches each one to the alias, import,
//! class, function, or statement lowering in the sibling modules and
//! assembles the resulting `HirModule`.
//!
//! Extracted from `lib.rs` per AGENTS.md's file-decomposition rule (issue
//! #867): the crate root keeps the HIR data types and the diagnostic
//! constructors; the walk itself lives here. `lib.rs` re-exports
//! `lower_all` and `lower_checked` so the `pycc_hir::lower_checked` path is
//! unchanged.
//!
//! Part 2 of #864 (#867, D-219): the walk collects one diagnostic per
//! failing top-level item instead of stopping at the first. A failing item
//! is skipped as a unit -- a `def` aborts only that function, a failing
//! method aborts its whole class -- and lowering continues with the next
//! item. Two `C0001` shapes are *cascades* of an earlier skipped item rather
//! than independent gaps: a bare-name annotation that names a class or
//! type alias which failed to lower, and a base-class reference to one.
//! Those are suppressed silently through the "poisoned bindings" set kept by
//! `lower_all` (see `poisonable_name` and `cascade_name`); everything else
//! is reported. HIR failures still stop the pipeline before the type
//! checker (`src/frontend.rs`), so no partial module is ever type-checked.

use crate::{
    FIRST_USER_EXCEPTION_TYPE_TAG, HirClassDef, HirItem, HirModule, ImportBinding,
    MAX_USER_EXCEPTION_CLASSES, Ty, builtin_exception_class_defs, builtin_exception_init_item,
    class, exception, import_local_name, is_builtin_exception_class, lower_function,
    lower_import_stmt, lower_legacy_type_alias_ann_assign, lower_type_alias_stmt, stmt,
    unsupported,
};
use pycc_ast::{Expr, ModModule, Stmt};
use pycc_diag::{Diagnostic, Span};

/// Lowers a parsed module into the HIR subset implemented by this pycc
/// version. Syntactically valid Python outside that subset returns `C0001`
/// with the unsupported node's source span instead of panicking.
///
/// First-diagnostic view of [`lower_all`] for the crate's many test, bench,
/// and downstream callers that consume a single `Diagnostic` (D-217's
/// `parse`/`parse_all` precedent): the `Err` is exactly `lower_all`'s first
/// collected diagnostic, which is byte-identical to what this function
/// reported before per-item collection landed (D-219). The `.expect` follows
/// the crate's documented coverage convention (`import.rs`): `lower_all`'s
/// `Err` is never empty by construction, and the panic path lives in
/// libcore, adding no in-crate region.
pub fn lower_checked(module: &ModModule) -> Result<HirModule, Diagnostic> {
    lower_all(module).map_err(|diagnostics| {
        diagnostics
            .into_iter()
            .next()
            .expect("lower_all's Err is never empty by construction")
    })
}

/// The tables `lower_all` builds up as it walks a module's top-level
/// statements, in source order. Each is a `Vec` rather than a map so the
/// lookups every later item performs see earlier items in a stable order.
struct ModuleState<'a> {
    aliases: Vec<(String, Ty)>,
    imports: Vec<ImportBinding>,
    class_defs: Vec<(String, HirClassDef)>,
    // #585: parallel to `class_defs`, but keeps each user-authored class's
    // original `StmtClassDef` (borrowed from the module, so it outlives this
    // whole pass) instead of its already-lowered `HirClassDef`. A synthetic
    // builtin-exception class seeded into `class_defs` has no such AST node
    // and is simply never present here -- `class::lower_class` treats that
    // absence as "nothing further to validate", matching how this crate
    // already handles bases it cannot introspect elsewhere.
    class_asts: Vec<(String, &'a pycc_ast::StmtClassDef)>,
    items: Vec<HirItem>,
}

/// Lowers every top-level item of a parsed module, collecting one
/// diagnostic per failing item (in source order) and skipping that item;
/// the `Err` is never empty (D-219, Part 2 of #864).
///
/// Type aliases (D-135) are resolved in a single left-to-right pass: a
/// `type X = <expr>` or legacy `X: TypeAlias = <expr>` statement is
/// evaluated and recorded into `aliases` as soon as it is reached, so it is
/// visible to every later statement's annotations (including a later
/// function's parameter/return annotations and later top-level
/// `AnnAssign`s) but not to any earlier one -- matching this compiler's
/// existing single-pass, source-order lowering model instead of
/// introducing hoisting.
///
/// Cascade suppression ("poisoned bindings", D-219): every item is lowered
/// first and its failure classified afterwards. When an item fails, the
/// class or type-alias name it would have bound (`poisonable_name`) is
/// recorded as poisoned; when a later item fails with one of the two
/// cascade-shaped `C0001`s (`cascade_name`) naming a poisoned name, that
/// item is skipped *silently* -- no diagnostic of any kind -- and its own
/// poisonable name is recorded too, so `class B(A)` after a skipped `A`
/// silences a following `class C(B)`. A later class or alias that binds a
/// poisoned name and lowers successfully un-poisons it. Nothing before the
/// first failing item is ever skipped, and that item's diagnostic is pushed
/// unconditionally (the set is still empty), so the first collected
/// diagnostic is byte-identical to the pre-#867 single diagnostic (D-217
/// rule 2). The post-loop phases (rotating the seeded exception classes to
/// the back, assigning exception type tags, seeding `Exception.__init__`)
/// run only when nothing was collected.
pub fn lower_all(module: &ModModule) -> Result<HirModule, Vec<Diagnostic>> {
    let mut state = ModuleState {
        aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
        class_asts: Vec::new(),
        items: Vec::with_capacity(module.body.len()),
    };
    // Part 1 of #541 (extending D-173): give the builtin exception
    // hierarchy a real presence in the class table, seeded *before* any
    // user statement is lowered so a user class can inherit from one
    // (`class MyError(ValueError):`) exactly as it inherits from a user
    // base. Two gates, both of which must pass:
    //
    // * The module must actually *reference* one of the seven names. Every
    //   entry in `class_defs` costs the per-item work below (the projected
    //   class slice, the name-collision checks) and the per-function class
    //   binding in `pycc_types`, and a module that never names a builtin
    //   exception cannot observe the difference -- see
    //   `exception::module_references_builtin_exception_name`.
    // * The module's own top level must not *bind* any of the seven names.
    //   That gate is all-or-nothing, so every existing name-collision check
    //   below applies to the synthetic definitions with no exemption -- see
    //   `exception::module_shadows_builtin_exception_name`. Both gates are
    //   whole-module AST scans decided here, before the loop, so a
    //   shadowing class that later fails to lower still counts as a shadow.
    let seeded_builtin_exception_classes =
        exception::module_references_builtin_exception_name(module)
            && !exception::module_shadows_builtin_exception_name(module);
    if seeded_builtin_exception_classes {
        state.class_defs.extend(builtin_exception_class_defs());
    }
    // Seeded at the *front* so every lookup below (base resolution,
    // annotation projection, the name-collision checks) sees them, then
    // rotated to the back once lowering finishes so `class_defs` still
    // opens with the module's own classes in source order.
    let synthetic_class_count = state.class_defs.len();
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    // Small and searched linearly; insertion order keeps tests deterministic.
    let mut poisoned: Vec<String> = Vec::new();
    for stmt in &module.body {
        match lower_top_level_item(stmt, &mut state) {
            Ok(()) => {
                // P5: a class or alias that binds a poisoned name and lowers
                // un-poisons it. `retain`, never `position` + `remove`, so a
                // duplicate could never survive even if one were inserted.
                if let Some(name) = poisonable_name(stmt) {
                    poisoned.retain(|poisoned_name| poisoned_name != name);
                }
            }
            Err(diagnostic) => {
                // P2: a cascade-shaped error naming a poisoned binding is a
                // consequence of the earlier skip, not a new gap -- skip
                // this item silently (P4). Anything else is reported.
                let is_cascade = cascade_name(&diagnostic)
                    .is_some_and(|name| poisoned.iter().any(|poisoned_name| poisoned_name == name));
                if !is_cascade {
                    diagnostics.push(diagnostic);
                }
                // P1: whatever the reason, the item bound nothing, so the
                // name it would have bound is now poisoned (transitively for
                // a cascade skip). Insert only if absent.
                if let Some(name) = poisonable_name(stmt)
                    && !poisoned.iter().any(|poisoned_name| poisoned_name == name)
                {
                    poisoned.push(name.to_string());
                }
            }
        }
    }
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }
    let ModuleState {
        aliases,
        imports,
        mut class_defs,
        class_asts: _,
        mut items,
    } = state;
    class_defs.rotate_left(synthetic_class_count);
    let user_class_count = class_defs.len() - synthetic_class_count;
    let mut any_user_exception_class = false;
    if synthetic_class_count > 0 {
        // Part 2 of #541 (D-189): assign each raisable user class its runtime
        // exception type tag here, in source order, so every downstream
        // consumer (`pycc_types`, `pycc_mir`, `pycc_codegen`) reads the same
        // number for the same class without re-deriving it. Source order is
        // the only ordering available that is stable across runs -- a hash
        // map's iteration order is not (risk R3 of this issue's plan).
        //
        // A class is raisable when its MRO reaches one of the seeded builtin
        // exception classes. `synthetic_class_count > 0` is exactly
        // `seeded_builtin_exception_classes`, so this branch never mistakes a
        // user class named `Exception` for the builtin one.
        let mut next_tag: u16 = u16::from(FIRST_USER_EXCEPTION_TYPE_TAG);
        for (_, def) in &mut class_defs[..user_class_count] {
            if !def
                .mro
                .iter()
                .any(|ancestor| is_builtin_exception_class(ancestor))
            {
                continue;
            }
            any_user_exception_class = true;
            if next_tag > u16::from(u8::MAX) {
                // The tag is a `u8` in `PyExceptionObj` and in every runtime
                // entry point that carries one, so the hierarchy cannot grow
                // past 256 types. No span is available here: `class_defs`
                // records no source range, and the diagnostic is about the
                // module's class count rather than any one declaration.
                // Reached only when every item lowered, so this stays a
                // one-element `Err` (P6).
                return Err(vec![Diagnostic::error(
                    "C0001",
                    format!(
                        "module declares more than {} exception classes; pycc \
                         supports at most {} user-defined exception classes \
                         per module",
                        MAX_USER_EXCEPTION_CLASSES, MAX_USER_EXCEPTION_CLASSES
                    ),
                    Span::new(0, 0),
                )]);
            }
            def.exception_type_tag = Some(next_tag as u8);
            next_tag += 1;
        }
    }
    // The synthetic `Exception.__init__` body is emitted only when a user
    // class actually inherits it -- that is, when some user class's computed
    // MRO reaches one of the seeded builtin exception classes, which is
    // exactly the condition that assigned at least one tag above. The
    // class-table entries above are metadata every module needs for name and
    // base resolution; this is *code*, and emitting an uncallable constructor
    // into every compiled module would put a dead function in every object
    // file. The synthetic classes themselves can never call it: instantiating
    // one is rejected by the type checker
    // (`pycc_types::class::resolve_instantiation`).
    if any_user_exception_class {
        items.push(builtin_exception_init_item());
    }
    Ok(HirModule {
        items,
        type_aliases: aliases,
        imports,
        class_defs,
        seeded_builtin_exception_classes,
    })
}

/// Lowers one top-level statement into `state`, in exactly the order the
/// pre-#867 loop body did: type alias, legacy type alias, import, class,
/// then function or plain statement, each with its own reverse-direction
/// name-collision checks. On `Err` nothing was recorded into `state`, which
/// is what lets `lower_all` skip the item as a unit.
fn lower_top_level_item<'a>(stmt: &'a Stmt, state: &mut ModuleState<'a>) -> Result<(), Diagnostic> {
    // #380 (PR-20): build the projected class slice `annotation_to_ty`
    // uses to resolve cross-class annotations; #611 (PEP 560) added the
    // per-class subscriptability flag it carries. Recomputed per item so a
    // later item sees every earlier class.
    let class_name_defs = class::class_annotation_infos(&state.class_defs, &state.items);
    if let Some((name, ty)) = lower_type_alias_stmt(stmt, &state.aliases, &class_name_defs)? {
        // D-068 review finding on #385, second round: the class-vs-alias
        // check below (at the `Stmt::ClassDef` arm) only ever catches a
        // class defined *after* a same-named alias -- without this
        // check, `class Foo: ...` followed by `type Foo = int` would
        // silently establish a second, alias-shaped `Foo` binding with
        // no diagnostic, the exact failure mode this finding exists to
        // close, just in the untreated direction.
        if state
            .class_defs
            .iter()
            .any(|(class_name, _)| *class_name == name)
        {
            return Err(unsupported(
                format!(
                    "type alias `{name}` collides with a class of the same name \
                     already defined in this module"
                ),
                pycc_ast::stmt_range(stmt),
            ));
        }
        state.aliases.push((name, ty));
        return Ok(());
    }
    if let Some((name, ty)) =
        lower_legacy_type_alias_ann_assign(stmt, &state.aliases, &class_name_defs)?
    {
        // Same reverse-direction check as the `type X = ...` arm above,
        // for the legacy `X: TypeAlias = <expr>` spelling.
        if state
            .class_defs
            .iter()
            .any(|(class_name, _)| *class_name == name)
        {
            return Err(unsupported(
                format!(
                    "type alias `{name}` collides with a class of the same name \
                     already defined in this module"
                ),
                pycc_ast::stmt_range(stmt),
            ));
        }
        state.aliases.push((name, ty));
        return Ok(());
    }
    if let Some(mut bound) = lower_import_stmt(stmt)? {
        // Same reverse-direction check as the two type-alias arms above,
        // for `import ...`/`from ... import ...` (a single statement can
        // bind more than one local name, e.g. `from math import sqrt,
        // pi`, so every bound name is checked, not just the first).
        if let Some(colliding) = bound.iter().map(import_local_name).find(|local_name| {
            state
                .class_defs
                .iter()
                .any(|(class_name, _)| class_name == local_name)
        }) {
            return Err(unsupported(
                format!(
                    "import `{colliding}` collides with a class of the same name \
                     already defined in this module"
                ),
                pycc_ast::stmt_range(stmt),
            ));
        }
        state.imports.append(&mut bound);
        return Ok(());
    }
    if let Stmt::ClassDef(def) = stmt {
        let (class_def, mut method_items) = class::lower_class(
            def,
            &state.aliases,
            &state.class_defs,
            &state.items,
            &state.class_asts,
        )?;
        // D-154 Part 1's own post-merge review finding: two module-level
        // classes sharing a name would each lower their own `__init__`
        // (and any other same-named method) to the identical mangled
        // `<Name>.<method>` function name, silently colliding in
        // `HirModule::items`/`class_defs`'s `HashMap`-collected class
        // table downstream (`pycc_types::Environment::classes`,
        // `pycc_mir`'s own `classes` map) rather than producing a clean
        // diagnostic -- reject it here, at the same point `lower_class`'s
        // own duplicate-method check (`crates/pycc_hir/src/class.rs`)
        // fires for the identical shape one level down.
        if state
            .class_defs
            .iter()
            .any(|(name, _)| name == &class_def.name)
        {
            return Err(unsupported(
                format!(
                    "class `{}` is defined more than once in this module",
                    class_def.name
                ),
                def.range,
            ));
        }
        // D-068 review finding on #385: a class name colliding with an
        // already-defined top-level function, type alias, or import
        // name produced no diagnostic and silently, permanently
        // shadowed the earlier binding -- `pycc_types::Environment`
        // checks `env.lookup_class(callee)` before the ordinary
        // function lookup at every call site (`crates/pycc_types/src/
        // lib.rs`), on the (until now unenforced) assumption that a
        // class name can never collide with a real function name in
        // this compiler's flat, single-namespace model. Enforce that
        // assumption here, at the same point the class-vs-class check
        // above already fires, rather than leaving it merely asserted
        // in a comment one crate over. Only a top-level function name
        // is checked against `items` (a method's own mangled
        // `<ClassName>.<method>` name can never collide with a bare
        // class name -- a real Python `NAME` token can never contain a
        // `.`, `pycc_hir::class`'s own doc comment).
        if state
            .items
            .iter()
            .any(|item| matches!(item, HirItem::Function { name, .. } if *name == class_def.name))
        {
            return Err(unsupported(
                format!(
                    "class `{}` collides with a function of the same name already \
                     defined in this module",
                    class_def.name
                ),
                def.range,
            ));
        }
        if state
            .aliases
            .iter()
            .any(|(name, _)| name == &class_def.name)
        {
            return Err(unsupported(
                format!(
                    "class `{}` collides with a type alias of the same name already \
                     defined in this module",
                    class_def.name
                ),
                def.range,
            ));
        }
        if state
            .imports
            .iter()
            .any(|binding| import_local_name(binding) == class_def.name)
        {
            return Err(unsupported(
                format!(
                    "class `{}` collides with an import of the same name already \
                     defined in this module",
                    class_def.name
                ),
                def.range,
            ));
        }
        state.class_asts.push((class_def.name.clone(), def));
        state.class_defs.push((class_def.name.clone(), class_def));
        state.items.append(&mut method_items);
        return Ok(());
    }
    if let Stmt::FunctionDef(def) = stmt
        && state
            .class_defs
            .iter()
            .any(|(name, _)| name == def.name.as_str())
    {
        // The reverse direction of the check above: a top-level
        // function defined *after* a same-named class must be rejected
        // too, not only a class defined after a same-named function.
        return Err(unsupported(
            format!(
                "function `{}` collides with a class of the same name already \
                 defined in this module",
                def.name
            ),
            def.range,
        ));
    }
    let item = match stmt {
        Stmt::FunctionDef(def) => lower_function(def, &state.aliases, &class_name_defs)?,
        other => HirItem::TopLevelStmt(stmt::lower_stmt(
            other,
            &state.aliases,
            false,
            false,
            false,
            None,
            None,
            &class_name_defs,
        )?),
    };
    state.items.push(item);
    Ok(())
}

/// The class or type-alias name a top-level statement would bind -- the
/// only binding kinds that can be the root of an HIR cascade (D-219, P1).
///
/// `class C` -> `C`; `type X = ...` -> `X`; legacy `X: TypeAlias = ...` ->
/// `X`. Every other statement kind -- `import`, `from ... import`, `def`,
/// assignment, expression statement -- yields `None` on purpose: the two
/// cascade lookups (`annotation_to_ty`'s bare-name arm and
/// `validate_bases`) consult only the class table and the alias table and
/// can never resolve an import-, function-, or variable-bound name, so an
/// annotation naming one fails today whether or not that binding lowered.
/// That diagnostic is a genuine, independent gap and must stay reported.
/// This is deliberately narrower than
/// `exception::expr_binds_builtin_exception_name`'s destructuring scan,
/// which answers a different question (does the module shadow a name at
/// all).
///
/// The legacy predicate is exactly `lower_legacy_type_alias_ann_assign`'s
/// shape minus the value: a `Stmt::AnnAssign` whose annotation is the bare
/// name `TypeAlias` and whose target is a `Name`. A valueless `X: TypeAlias`
/// falls through to ordinary `AnnAssign` lowering and fails on `TypeAlias`
/// itself; poisoning `X` there is harmless (a name that is also validly
/// bound never produces a cascade-shaped error) and keeps this predicate
/// free of a fourth early-out.
pub(crate) fn poisonable_name(stmt: &Stmt) -> Option<&str> {
    match stmt {
        Stmt::ClassDef(def) => Some(def.name.as_str()),
        // Same `.expect` as `lower_type_alias_stmt`: ruff unconditionally
        // parses a `type` statement's name as `Expr::Name`.
        Stmt::TypeAlias(alias) => Some(
            alias
                .name
                .as_name_expr()
                .expect("ruff always parses a `type` statement's name as Expr::Name")
                .id
                .as_str(),
        ),
        Stmt::AnnAssign(ann) => {
            let Expr::Name(annotation) = ann.annotation.as_ref() else {
                return None;
            };
            if annotation.id.as_str() != "TypeAlias" {
                return None;
            }
            let Expr::Name(target) = ann.target.as_ref() else {
                return None;
            };
            Some(target.id.as_str())
        }
        _ => None,
    }
}

const UNKNOWN_ANNOTATION_PREFIX: &str = "type annotation `";
const UNKNOWN_ANNOTATION_SUFFIX: &str = "` is not supported yet";
const UNKNOWN_BASE_PREFIX: &str = "class `";
const UNKNOWN_BASE_INFIX: &str = "` inherits from unknown class `";
const UNKNOWN_BASE_SUFFIX: &str = "` -- base classes must be defined earlier in the same module";

/// The `C0001` message for a bare annotation name that is neither a known
/// class nor a type alias (`func::annotation_to_ty`'s bare-name arm). The
/// only producer of this message; `cascade_name` parses it back, and a unit
/// test round-trips the two so the wording cannot drift apart.
pub(crate) fn unknown_annotation_name_message(name: &str) -> String {
    format!("{UNKNOWN_ANNOTATION_PREFIX}{name}{UNKNOWN_ANNOTATION_SUFFIX}")
}

/// The `C0001` message for a base class that is not defined earlier in the
/// module (`class::mro::validate_bases`). The only producer of this message;
/// `cascade_name` parses it back under the same round-trip test.
pub(crate) fn unknown_base_message(class_name: &str, base_name: &str) -> String {
    format!("{UNKNOWN_BASE_PREFIX}{class_name}{UNKNOWN_BASE_INFIX}{base_name}{UNKNOWN_BASE_SUFFIX}")
}

/// Classifies a failed item's diagnostic (D-219, P2): `Some(name)` when it
/// is one of the two cascade-shaped `C0001`s -- the bare-name annotation
/// message naming `name`, or the unknown-base message whose base is `name`
/// -- and `None` for every other diagnostic. Only `lower_all` decides
/// whether `name` is actually poisoned; a `Some` for an un-poisoned name is
/// an ordinary, reported gap.
pub(crate) fn cascade_name(diagnostic: &Diagnostic) -> Option<&str> {
    if diagnostic.code != "C0001" {
        return None;
    }
    unknown_annotation_name(&diagnostic.message).or_else(|| unknown_base_name(&diagnostic.message))
}

fn unknown_annotation_name(message: &str) -> Option<&str> {
    message
        .strip_prefix(UNKNOWN_ANNOTATION_PREFIX)?
        .strip_suffix(UNKNOWN_ANNOTATION_SUFFIX)
}

fn unknown_base_name(message: &str) -> Option<&str> {
    let (_, base) = message
        .strip_prefix(UNKNOWN_BASE_PREFIX)?
        .split_once(UNKNOWN_BASE_INFIX)?;
    base.strip_suffix(UNKNOWN_BASE_SUFFIX)
}

#[cfg(test)]
mod tests;
