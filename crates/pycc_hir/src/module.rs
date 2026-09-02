//! Module-level lowering: the single left-to-right walk over a module's
//! top-level statements that dispatches each one to the alias, import,
//! class, function, or statement lowering in the sibling modules and
//! assembles the resulting `HirModule`.
//!
//! Extracted from `lib.rs` per AGENTS.md's file-decomposition rule (issue
//! #867): the crate root keeps the HIR data types and the diagnostic
//! constructors; the walk itself lives here. `lib.rs` re-exports
//! `lower_checked` so the `pycc_hir::lower_checked` path is unchanged.

use crate::{
    FIRST_USER_EXCEPTION_TYPE_TAG, HirClassDef, HirItem, HirModule, ImportBinding,
    MAX_USER_EXCEPTION_CLASSES, Ty, builtin_exception_class_defs, builtin_exception_init_item,
    class, exception, import_local_name, is_builtin_exception_class, lower_function,
    lower_import_stmt, lower_legacy_type_alias_ann_assign, lower_type_alias_stmt, stmt,
    unsupported,
};
use pycc_ast::{ModModule, Stmt};
use pycc_diag::{Diagnostic, Span};

/// Lowers a parsed module into the HIR subset implemented by this pycc
/// version. Syntactically valid Python outside that subset returns `C0001`
/// with the unsupported node's source span instead of panicking.
///
/// Type aliases (D-135) are resolved in a single left-to-right pass: a
/// `type X = <expr>` or legacy `X: TypeAlias = <expr>` statement is
/// evaluated and recorded into `aliases` as soon as it is reached, so it is
/// visible to every later statement's annotations (including a later
/// function's parameter/return annotations and later top-level
/// `AnnAssign`s) but not to any earlier one -- matching this compiler's
/// existing single-pass, source-order lowering model instead of
/// introducing hoisting.
pub fn lower_checked(module: &ModModule) -> Result<HirModule, Diagnostic> {
    let mut aliases: Vec<(String, Ty)> = Vec::new();
    let mut imports: Vec<ImportBinding> = Vec::new();
    let mut class_defs: Vec<(String, HirClassDef)> = Vec::new();
    // #585: parallel to `class_defs`, but keeps each user-authored class's
    // original `StmtClassDef` (borrowed from `module`, so it outlives this
    // whole pass) instead of its already-lowered `HirClassDef`. A synthetic
    // builtin-exception class seeded into `class_defs` above has no such
    // AST node and is simply never present here -- `class::lower_class`
    // treats that absence as "nothing further to validate", matching how
    // this crate already handles bases it cannot introspect elsewhere.
    let mut class_asts: Vec<(String, &pycc_ast::StmtClassDef)> = Vec::new();
    let mut items = Vec::with_capacity(module.body.len());
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
    //   `exception::module_shadows_builtin_exception_name`.
    let seeded_builtin_exception_classes =
        exception::module_references_builtin_exception_name(module)
            && !exception::module_shadows_builtin_exception_name(module);
    if seeded_builtin_exception_classes {
        class_defs.extend(builtin_exception_class_defs());
    }
    // Seeded at the *front* so every lookup below (base resolution,
    // annotation projection, the name-collision checks) sees them, then
    // rotated to the back once lowering finishes so `class_defs` still
    // opens with the module's own classes in source order.
    let synthetic_class_count = class_defs.len();
    for stmt in &module.body {
        // #380 (PR-20): build the projected class slice `annotation_to_ty`
        // uses to resolve cross-class annotations; #611 (PEP 560) added the
        // per-class subscriptability flag it carries.
        let class_name_defs = class::class_annotation_infos(&class_defs, &items);
        if let Some((name, ty)) = lower_type_alias_stmt(stmt, &aliases, &class_name_defs)? {
            // D-068 review finding on #385, second round: the class-vs-alias
            // check below (at the `Stmt::ClassDef` arm) only ever catches a
            // class defined *after* a same-named alias -- without this
            // check, `class Foo: ...` followed by `type Foo = int` would
            // silently establish a second, alias-shaped `Foo` binding with
            // no diagnostic, the exact failure mode this finding exists to
            // close, just in the untreated direction.
            if class_defs.iter().any(|(class_name, _)| *class_name == name) {
                return Err(unsupported(
                    format!(
                        "type alias `{name}` collides with a class of the same name \
                         already defined in this module"
                    ),
                    pycc_ast::stmt_range(stmt),
                ));
            }
            aliases.push((name, ty));
            continue;
        }
        if let Some((name, ty)) =
            lower_legacy_type_alias_ann_assign(stmt, &aliases, &class_name_defs)?
        {
            // Same reverse-direction check as the `type X = ...` arm above,
            // for the legacy `X: TypeAlias = <expr>` spelling.
            if class_defs.iter().any(|(class_name, _)| *class_name == name) {
                return Err(unsupported(
                    format!(
                        "type alias `{name}` collides with a class of the same name \
                         already defined in this module"
                    ),
                    pycc_ast::stmt_range(stmt),
                ));
            }
            aliases.push((name, ty));
            continue;
        }
        if let Some(mut bound) = lower_import_stmt(stmt)? {
            // Same reverse-direction check as the two type-alias arms above,
            // for `import ...`/`from ... import ...` (a single statement can
            // bind more than one local name, e.g. `from math import sqrt,
            // pi`, so every bound name is checked, not just the first).
            if let Some(colliding) = bound.iter().map(import_local_name).find(|local_name| {
                class_defs
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
            imports.append(&mut bound);
            continue;
        }
        if let Stmt::ClassDef(def) = stmt {
            let (class_def, mut method_items) =
                class::lower_class(def, &aliases, &class_defs, &items, &class_asts)?;
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
            if class_defs.iter().any(|(name, _)| name == &class_def.name) {
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
            if items.iter().any(
                |item| matches!(item, HirItem::Function { name, .. } if *name == class_def.name),
            ) {
                return Err(unsupported(
                    format!(
                        "class `{}` collides with a function of the same name already \
                         defined in this module",
                        class_def.name
                    ),
                    def.range,
                ));
            }
            if aliases.iter().any(|(name, _)| name == &class_def.name) {
                return Err(unsupported(
                    format!(
                        "class `{}` collides with a type alias of the same name already \
                         defined in this module",
                        class_def.name
                    ),
                    def.range,
                ));
            }
            if imports
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
            class_asts.push((class_def.name.clone(), def));
            class_defs.push((class_def.name.clone(), class_def));
            items.append(&mut method_items);
            continue;
        }
        if let Stmt::FunctionDef(def) = stmt
            && class_defs.iter().any(|(name, _)| name == def.name.as_str())
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
            Stmt::FunctionDef(def) => lower_function(def, &aliases, &class_name_defs)?,
            other => HirItem::TopLevelStmt(stmt::lower_stmt(
                other,
                &aliases,
                false,
                false,
                false,
                None,
                None,
                &class_name_defs,
            )?),
        };
        items.push(item);
    }
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
                return Err(Diagnostic::error(
                    "C0001",
                    format!(
                        "module declares more than {} exception classes; pycc \
                         supports at most {} user-defined exception classes \
                         per module",
                        MAX_USER_EXCEPTION_CLASSES, MAX_USER_EXCEPTION_CLASSES
                    ),
                    Span::new(0, 0),
                ));
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
