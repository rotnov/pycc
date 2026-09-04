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
//! #898 (D-222) splits the walk from the program-level phases that used to
//! follow it: `lower_module` is the per-module walk (against the driver's
//! `ResolvedImports` answers for the module's project imports), and
//! `program::link` + `program::finalize` combine any number of lowered
//! modules into one program and run the post-loop phases (exception type
//! tags, `Exception.__init__`). `lower_all` is the single-file composition
//! of the two and is byte-identical to what it produced before the split.
//!
//! Part 2 of #864 (#867, D-219): the walk collects one diagnostic per
//! failing top-level item instead of stopping at the first. A failing item
//! is skipped as a unit -- a `def` aborts only that function, a failing
//! method aborts its whole class -- and lowering continues with the next
//! item. Two `C0001` shapes are *cascades* of an earlier skipped item rather
//! than independent gaps: a bare-name annotation that names a class or
//! type alias which failed to lower, and a base-class reference to one.
//! Those are suppressed silently through the "poisoned bindings" set kept by
//! `lower_module` (see `poisonable_names` and `cascade_name`); everything else
//! is reported. HIR failures still stop the pipeline before the type
//! checker (`src/frontend.rs`), so no partial module is ever type-checked.

use crate::import::ResolvedImports;
use crate::{
    HirClassDef, HirItem, HirModule, ImportBinding, Ty, builtin_exception_class_defs, class,
    exception, import_local_name, killed_names, lower_function, lower_import_stmt,
    lower_legacy_type_alias_ann_assign, lower_type_alias_stmt, program, stmt, unsupported,
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

/// The tables `lower_module` builds up as it walks a module's top-level
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
    // #898: positions in `class_defs`/`aliases` that a project import
    // copied in from another module so this module's annotations and
    // bases can resolve them. Stripped again before the `HirModule` is
    // built: the linked program defines each class and alias exactly once,
    // in the module that authored it.
    imported_class_indices: Vec<usize>,
    imported_alias_indices: Vec<usize>,
    // #898: every top-level definition this module makes, with its span,
    // so `program::link` can report a cross-module name collision at the
    // later definition. Names may repeat (a variable rebound twice).
    definition_spans: Vec<(String, Span)>,
}

/// One module's lowering, before `program::link`/`program::finalize`
/// (#898). `shadowed_builtin_exception_name` is the first builtin exception
/// name this module's top level binds, if any -- the input to `link`'s
/// cross-module seeding check, since a module that shadows one of the 25
/// names is never seeded itself but cannot be linked with a module that
/// was. `definition_spans` feeds `link`'s collision diagnostics.
#[derive(Debug, Clone, PartialEq)]
pub struct LoweredModule {
    pub hir: HirModule,
    pub shadowed_builtin_exception_name: Option<String>,
    pub definition_spans: Vec<(String, Span)>,
}

/// Lowers every top-level item of a parsed module, collecting one
/// diagnostic per failing item (in source order) and skipping that item;
/// the `Err` is never empty (D-219, Part 2 of #864). The single-file
/// entry: exactly `lower_module` with no project imports answered,
/// followed by `program::finalize` -- the same phases in the same order as
/// before #898, so the result is byte-identical.
pub fn lower_all(module: &ModModule) -> Result<HirModule, Vec<Diagnostic>> {
    let lowered = lower_module(module, &ResolvedImports::default())?;
    program::finalize(lowered.hir)
}

/// The per-module walk (#898): lowers every top-level item against the
/// driver's answers for the module's project imports, collecting one
/// diagnostic per failing item (in source order) and skipping that item;
/// the `Err` is never empty (D-219). The result still needs
/// `program::link` (even for a single module) and `program::finalize`
/// before it is a complete program: the exception type tags and the
/// synthetic `Exception.__init__` are program-wide and assigned there.
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
/// class, type-alias, or project-import names it would have bound
/// (`poisonable_names`) are recorded as poisoned; when a later item fails
/// with one of the two cascade-shaped `C0001`s (`cascade_name`) naming a
/// poisoned name, that item is skipped *silently* -- no diagnostic of any
/// kind -- and its own poisonable names are recorded too, so `class B(A)`
/// after a skipped `A` silences a following `class C(B)`. A later item
/// that binds a poisoned name and lowers successfully un-poisons it.
/// Nothing before the first failing item is ever skipped, and that item's
/// diagnostic is pushed unconditionally (the set is still empty), so the
/// first collected diagnostic is byte-identical to the pre-#867 single
/// diagnostic (D-217 rule 2).
pub fn lower_module(
    module: &ModModule,
    resolved: &ResolvedImports<'_>,
) -> Result<LoweredModule, Vec<Diagnostic>> {
    let mut state = ModuleState {
        aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
        class_asts: Vec::new(),
        items: Vec::with_capacity(module.body.len()),
        imported_class_indices: Vec::new(),
        imported_alias_indices: Vec::new(),
        definition_spans: Vec::new(),
    };
    // Part 1 of #541 (extending D-173): give the builtin exception
    // hierarchy a real presence in the class table, seeded *before* any
    // user statement is lowered so a user class can inherit from one
    // (`class MyError(ValueError):`) exactly as it inherits from a user
    // base. Two gates, both of which must pass:
    //
    // * The module must actually *reference* one of the 25 names. Every
    //   entry in `class_defs` costs the per-item work below (the projected
    //   class slice, the name-collision checks) and the per-function class
    //   binding in `pycc_types`, and a module that never names a builtin
    //   exception cannot observe the difference -- see
    //   `exception::module_references_builtin_exception_name`.
    // * The module's own top level must not *bind* any of the 25 names.
    //   That gate is all-or-nothing, so every existing name-collision check
    //   below applies to the synthetic definitions with no exemption -- see
    //   `exception::shadowed_builtin_exception_name`. Both gates are
    //   whole-module AST scans decided here, before the loop, so a
    //   shadowing class that later fails to lower still counts as a shadow.
    let shadowed_builtin_exception_name = exception::shadowed_builtin_exception_name(module);
    let seeded_builtin_exception_classes =
        exception::module_references_builtin_exception_name(module)
            && shadowed_builtin_exception_name.is_none();
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
        match lower_top_level_item(stmt, &mut state, resolved) {
            Ok(()) => {
                // P5: an item that binds a poisoned name and lowers
                // un-poisons it. `retain`, never `position` + `remove`, so a
                // duplicate could never survive even if one were inserted.
                for name in poisonable_names(stmt) {
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
                // names it would have bound are now poisoned (transitively
                // for a cascade skip). Insert only if absent.
                for name in poisonable_names(stmt) {
                    if !poisoned.iter().any(|poisoned_name| poisoned_name == name) {
                        poisoned.push(name.to_string());
                    }
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
        class_defs,
        class_asts: _,
        items,
        imported_class_indices,
        imported_alias_indices,
        definition_spans,
    } = state;
    // The imported copies were pushed after the synthetic set, so
    // stripping them leaves the synthetic entries still at the front.
    let mut class_defs = strip_imported(class_defs, &imported_class_indices);
    let aliases = strip_imported(aliases, &imported_alias_indices);
    class_defs.rotate_left(synthetic_class_count);
    Ok(LoweredModule {
        hir: HirModule {
            items,
            type_aliases: aliases,
            imports,
            class_defs,
            seeded_builtin_exception_classes,
        },
        shadowed_builtin_exception_name,
        definition_spans,
    })
}

/// Drops the entries at `imported_indices` (a project import's copied
/// classes or aliases) from `entries`, keeping every other entry in order.
fn strip_imported<T>(entries: Vec<T>, imported_indices: &[usize]) -> Vec<T> {
    entries
        .into_iter()
        .enumerate()
        .filter(|(index, _)| !imported_indices.contains(index))
        .map(|(_, entry)| entry)
        .collect()
}

/// Lowers one top-level statement into `state`, in exactly the order the
/// pre-#867 loop body did: type alias, legacy type alias, import, class,
/// then function or plain statement, each with its own reverse-direction
/// name-collision checks. On `Err` nothing was recorded into `state`, which
/// is what lets `lower_module` skip the item as a unit.
fn lower_top_level_item<'a>(
    stmt: &'a Stmt,
    state: &mut ModuleState<'a>,
    resolved: &ResolvedImports<'_>,
) -> Result<(), Diagnostic> {
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
        state
            .definition_spans
            .push((name.clone(), statement_span(stmt)));
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
        state
            .definition_spans
            .push((name.clone(), statement_span(stmt)));
        state.aliases.push((name, ty));
        return Ok(());
    }
    if let Some(mut lowered) = lower_import_stmt(stmt, resolved)? {
        // Same reverse-direction check as the two type-alias arms above,
        // for `import ...`/`from ... import ...` (a single statement can
        // bind more than one local name, e.g. `from math import sqrt,
        // pi`, so every bound name is checked, not just the first). A
        // class this module imported earlier is exempt: `from a import
        // Point` twice binds the same definition twice, not a collision.
        if let Some(colliding) = lowered
            .bindings
            .iter()
            .map(import_local_name)
            .find(|local_name| {
                state
                    .class_defs
                    .iter()
                    .enumerate()
                    .any(|(index, (class_name, _))| {
                        class_name == local_name && !state.imported_class_indices.contains(&index)
                    })
            })
        {
            return Err(unsupported(
                format!(
                    "import `{colliding}` collides with a class of the same name \
                     already defined in this module"
                ),
                pycc_ast::stmt_range(stmt),
            ));
        }
        // #898: bring an imported class (with its MRO) and an imported
        // alias into this module's tables so later annotations and bases
        // resolve them; a copy already present (an ancestor shared with an
        // earlier import, or a synthetic exception class this module seeded
        // itself) is not duplicated.
        //
        // The two `continue` guards below have no observable effect on
        // Part 1's output, and no test discriminates them: every copy they
        // would skip is also recorded in `imported_class_indices` /
        // `imported_alias_indices` and removed again by `strip_imported`
        // before anything downstream sees the module, and the name lookups
        // in between take the first match over byte-identical entries.
        // They are kept because they hold the one-name-one-entry invariant
        // that the collision checks just above and the `HashMap`-collected
        // class tables downstream (`pycc_types::Environment::classes`,
        // `pycc_mir`'s own `classes` map) are written against. Part 2
        // (#899, per-module namespaces) is where stripping stops being
        // universal and the guards become load-bearing.
        for entry in lowered.classes {
            if state.class_defs.iter().any(|(name, _)| *name == entry.0) {
                continue;
            }
            state.imported_class_indices.push(state.class_defs.len());
            state.class_defs.push(entry);
        }
        for entry in lowered.aliases {
            if state.aliases.iter().any(|(name, _)| *name == entry.0) {
                continue;
            }
            state.imported_alias_indices.push(state.aliases.len());
            state.aliases.push(entry);
        }
        state.imports.append(&mut lowered.bindings);
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
        // #898: a class copied in by a project import is not a definition
        // of this module, so a same-named `class` here is reported by the
        // import-collision check below, not as a duplicate definition.
        if state
            .class_defs
            .iter()
            .enumerate()
            .any(|(index, (name, _))| {
                name == &class_def.name && !state.imported_class_indices.contains(&index)
            })
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
        state
            .definition_spans
            .push((class_def.name.clone(), statement_span(stmt)));
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
            // #795 (PEP 654): module top level is `Outside` by definition.
            stmt::ExceptStarCtx::Outside,
            None,
            None,
            &class_name_defs,
        )?),
    };
    let span = statement_span(stmt);
    match &item {
        HirItem::Function { name, .. } => state.definition_spans.push((name.clone(), span)),
        HirItem::TopLevelStmt(lowered) => {
            for name in killed_names(std::slice::from_ref(lowered)) {
                state.definition_spans.push((name, span));
            }
        }
    }
    state.items.push(item);
    Ok(())
}

fn statement_span(stmt: &Stmt) -> Span {
    let range = pycc_ast::stmt_range(stmt);
    Span::new(range.start, range.end)
}

/// The class, type-alias, or import names a top-level statement would bind
/// -- the binding kinds that can be the root of an HIR cascade (D-219, P1;
/// #898 added the import kind, amending D-219 rule 3 -- see D-222).
///
/// `class C` -> `[C]`; `type X = ...` -> `[X]`; legacy `X: TypeAlias = ...`
/// -> `[X]`.
///
/// An import yields names exactly when it *fails*, and then the names are
/// the ones it would have bound locally: `from geometry import Point, Line`
/// -> `[Point, Line]`, `import pkg.dep as d` -> `[d]`, and a rejected
/// `from math import *` -> `math`'s whole export list. Both import arms
/// therefore mirror `import::lower_import_stmt`'s own success conditions
/// exactly rather than approximating them, one arm per statement kind, so
/// a shape that lowers poisons nothing and every shape that does not
/// poisons. An import that lowers binds a name the two cascade lookups
/// (`annotation_to_ty`'s bare-name arm and `validate_bases`) cannot resolve
/// anyway -- they consult only the class table and the alias table -- so a
/// later annotation naming it fails today either way, and that diagnostic
/// is a genuine, independent gap that must stay reported.
///
/// Every remaining statement kind -- `def`, assignment, expression
/// statement -- yields nothing on purpose, for that same reason: those
/// lookups can never resolve a function- or variable-bound name.
/// This is deliberately narrower than
/// `exception::expr_bound_builtin_exception_name`'s destructuring scan,
/// which answers a different question (does the module shadow a name at
/// all).
///
/// The legacy predicate is exactly `lower_legacy_type_alias_ann_assign`'s
/// shape, value included: a `Stmt::AnnAssign` whose annotation is the bare
/// name `TypeAlias`, whose target is a `Name`, and which carries a value. A
/// valueless `X: TypeAlias` binds no alias -- it falls through to ordinary
/// `AnnAssign` lowering and fails on `TypeAlias` itself -- so it yields
/// nothing here, and a later `X` diagnostic stays reported.
pub(crate) fn poisonable_names(stmt: &Stmt) -> Vec<&str> {
    match stmt {
        Stmt::ClassDef(def) => vec![def.name.as_str()],
        // Same `.expect` as `lower_type_alias_stmt`: ruff unconditionally
        // parses a `type` statement's name as `Expr::Name`.
        Stmt::TypeAlias(alias) => vec![
            alias
                .name
                .as_name_expr()
                .expect("ruff always parses a `type` statement's name as Expr::Name")
                .id
                .as_str(),
        ],
        Stmt::AnnAssign(ann) => {
            let Expr::Name(annotation) = ann.annotation.as_ref() else {
                return Vec::new();
            };
            if annotation.id.as_str() != "TypeAlias" {
                return Vec::new();
            }
            let Expr::Name(target) = ann.target.as_ref() else {
                return Vec::new();
            };
            // A valueless `X: TypeAlias` binds nothing:
            // `import::lower_legacy_type_alias_ann_assign` records no alias
            // for it and lets it fall through as an ordinary annotated
            // assignment, so poisoning `X` would suppress a genuine later
            // `X` diagnostic (Codex review on #875).
            if ann.value.is_none() {
                return Vec::new();
            }
            vec![target.id.as_str()]
        }
        Stmt::Import(import) => {
            // `import::lower_import_stmt` accepts exactly one shape: a single
            // alias, no `asname`, and a module name `pycc_std` resolves. The
            // condition is exact rather than an approximation of that arm --
            // its earlier `ResolvedImport::Found` branch cannot fire for a
            // stdlib-resolving name, because `project_import_request` returns
            // `None` for one, so no answer is ever recorded for its span.
            if let [alias] = import.names.as_slice()
                && alias.asname.is_none()
                && pycc_std::resolve_module(alias.name.as_str()).is_some()
            {
                return Vec::new();
            }
            // Every other shape fails, so poison what it would have bound:
            // the alias when present, and otherwise the first dotted segment,
            // since `import pkg.dep` binds `pkg`. `import a, b` fails as a
            // whole statement, so both of its names are poisoned.
            import
                .names
                .iter()
                .map(|alias| {
                    alias.asname.as_ref().map_or_else(
                        || {
                            let name = alias.name.as_str();
                            name.split_once('.').map_or(name, |(head, _)| head)
                        },
                        |asname| asname.as_str(),
                    )
                })
                .collect()
        }
        Stmt::ImportFrom(import) => {
            // A `level == 0` statement naming a module `pycc_std` resolves
            // takes `lower_import_stmt`'s stdlib arm; everything else (a
            // relative import, an unresolvable module) takes the project arm
            // or is rejected outright, and poisons below.
            if let Some(module) = (import.level == 0)
                .then_some(import.module.as_ref())
                .flatten()
                .and_then(|module| pycc_std::resolve_module(module.as_str()))
            {
                // Mirror that arm's success condition exactly, as the
                // `Stmt::Import` arm above mirrors its own: the statement
                // lowers when no name is the wildcard, none carries an
                // `asname`, and every name is a symbol of `module`. Anything
                // else fails, and a failed stdlib import poisons what it
                // would have bound just like a failed project one -- a
                // wildcard binds the module's whole export list, every other
                // shape binds its own aliases.
                if import.names.iter().any(|alias| alias.name.as_str() == "*") {
                    return pycc_std::module_symbol_names(module).collect();
                }
                if import.names.iter().all(|alias| {
                    alias.asname.is_none()
                        && pycc_std::resolve_symbol(module, alias.name.as_str()).is_some()
                }) {
                    return Vec::new();
                }
            }
            // The poisoned name is the one this statement would have *bound*
            // locally, not the one it reads from the other module: under
            // `from .dep import helper as h` a later `h` is the cascade to
            // suppress, and a later `helper` is a genuine unknown name.
            import
                .names
                .iter()
                .map(|alias| {
                    alias
                        .asname
                        .as_ref()
                        .map_or(alias.name.as_str(), |asname| asname.as_str())
                })
                .collect()
        }
        _ => Vec::new(),
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

/// The `C0001` message for a *bare* builtin container annotation -- `list`,
/// `set`, `dict` or `tuple` written with no type arguments (D-227, issue
/// #918). Split out from [`unknown_annotation_name_message`] because a bare
/// container is no longer an unknown name: the parameterized form now lowers,
/// so the actionable advice is "write `list[int]`", not "this name means
/// nothing here".
///
/// Deliberately *not* cascade-shaped: it starts with `a bare \``, not
/// `type annotation \``, so [`cascade_name`] returns `None` for it. That is
/// the wanted outcome under D-219 -- a bare `list` annotation does not poison
/// a name the way an unknown class name does, because nothing else in the
/// module can be waiting on `list` to be defined.
///
/// `frozenset` and `type` deliberately keep the generic unknown-name message:
/// neither has a `Ty` variant, so steering a user toward `frozenset[int]`
/// would point at a form this version rejects just as hard.
pub(crate) fn bare_container_annotation_message(name: &str, example: &str) -> String {
    format!(
        "a bare `{name}` type annotation is not supported yet -- write the parameterized form, e.g. `{example}`"
    )
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
/// -- and `None` for every other diagnostic. Only `lower_module` decides
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
