---
id: D-222
title: "Project modules link at the HIR level into one whole-program HirModule"
status: accepted
---

## D-222: Project modules link at the HIR level into one whole-program `HirModule`
- Status: accepted
- Context: pycc compiled exactly one file. Every `from <project module> import ...`
  was rejected with `C0001`, so no real multi-file Python project could be checked
  or built (#881, Part 1 delivered as #898). The pipeline downstream of `pycc_hir`
  (`pycc_types`, `pycc_mir`, `pycc_codegen`) is built entirely around a single
  `HirModule` with one flat item list, one class table, and one exception-tag
  space, and `pycc_hir` itself must stay filesystem-free so its unit tests and the
  `check_bench` throughput gate keep working on in-memory sources.
- Decision:
  - **Link at the HIR level, into one program-wide flat namespace.** The driver
    loads the entry file's whole import closure, lowers each file separately with
    the new `pycc_hir::lower_module`, and `pycc_hir::link` concatenates the results
    (dependency order, entry last) into the single `HirModule` the rest of the
    pipeline already consumes. `pycc_hir::finalize` then runs the program-wide
    phases that used to close `lower_all` (builtin-exception tag assignment,
    `Exception.__init__`); `lower_all` is now exactly
    `finalize(lower_module(m, &ResolvedImports::default())?.hir)`, so single-file
    compilation is byte-identical.
  - **The driver owns the filesystem.** `pycc_hir::project_import_requests` reports
    every import `pycc_std`'s registry does not answer; the driver
    (`src/modules.rs`) answers them through `pycc_hir::ResolvedImports`, keyed by
    the import statement's span. A request the driver leaves unanswered lowers
    exactly as a single-file compilation would.
  - **Source root.** A `pycc.toml` in the entry's directory or an ancestor sets the
    root to that directory joined with the directory part of its `[project].entry`;
    otherwise the loader walks up out of the entry's own package chain (while a
    directory holds `__init__.py`) and uses the first non-package directory.
    Discovery is lazy, so a file with no project import never pays for it.
    A stdlib module name always wins over a same-named project file.
  - **Diagnostic split.** A shape CPython itself rejects is `T0021` (a relative
    import with no parent package, one that climbs above the top-level package, a
    relative target that resolves nowhere, an imported name the origin module does
    not define). A shape pycc has not implemented yet is `C0001` (a bare
    `import m`, `from pkg import submodule`, a namespace package as the terminal
    segment, a top-level name defined by two linked modules). An import cycle is
    `E0108`, now actually emitted.
  - **A rejected import poisons the name it would have bound.** D-219 rule 3
    defined the poisonable name of a statement as the class or type alias it
    binds and said imports bind nothing poisonable. That held while every
    import either lowered or was the only diagnostic in the file; once a file
    can fail on a project import and keep lowering, a rejected `import`/`from`
    statement leaves its bound name undefined and every later reference to it
    is a cascade of that one failure. So a failing import now poisons what it
    would have bound: the `asname` when present, otherwise the imported name
    for `from ... import name`, and the *first dotted segment* for a plain
    `import pkg.dep` (which binds `pkg`, not `pkg.dep`); `import a, b` fails
    as a whole statement and poisons both. A `Stmt::Import` lowers -- and so
    poisons nothing -- exactly when it has one alias, no `asname`, and a
    module name `pycc_std` resolves. The name poisoned is the one bound
    locally, never the source-side name: after `from .dep import helper as h`
    a later `h` is the cascade and a later `helper` is a genuine unknown name.
  - **Seeding reconciliation.** Each module decides its own builtin-exception
    seeding, so `link` strips every module's synthetic entries and appends one set
    if any module seeded. A program that both seeds and shadows a builtin exception
    name in another module is rejected (`C0001`) rather than silently losing a base
    class.
  - **Keyed type diagnostics.** `pycc_types::DiagnosticKey`
    (`Function`/`TopLevel`/`Module`) plus `check_all_keyed` /
    `check_and_resolve_all_keyed` let the driver attribute each type diagnostic to
    the file that owns the item it came from; a `Module`-keyed pre-check or
    post-body-phase failure is attributed to the entry file.
- Alternatives:
  - *Per-module namespaces with qualified item names now.* Rejected for Part 1: it
    changes `pycc_mir`'s `current_class` name splitting, `pycc_codegen`'s module
    bindings, and every name-resolution site at once. It is Part 3 of #881, and the
    flat-namespace collision diagnostic names it so a user hitting the restriction
    knows it is temporary.
  - *Resolving imports inside `pycc_types` from an import table.* Rejected: the
    type checker would need the filesystem, and every downstream pass would need a
    second name-resolution rule. Binding imported names to the defining module's
    own definitions keeps one rule.
  - *Filesystem access inside `pycc_hir`.* Rejected: it would make the crate's unit
    tests and the `check_bench` gate depend on a real tree, and it would put path
    policy (roots, `pycc.toml`, canonicalisation) inside the language frontend.
  - *Linking at the MIR or object level (separate compilation).* Rejected for now:
    it needs a cross-module type-signature format and an incremental cache, neither
    of which exists; whole-program linking is what the current pipeline supports.
- Consequences:
  - `pycc check`/`build`/`run` accept multi-file projects; a dependency's
    diagnostics render against the dependency's own path, and an unreadable
    dependency is exit 2 like any unreadable input.
  - Two modules may not define the same top-level name until Part 3 lands.
  - Every dependency's top-level statements run before the importing module's, in
    dependency order — an approximation of CPython's lazy, first-import-wins
    initialisation that is exact for acyclic programs whose module bodies have no
    ordering dependency on the importer.
  - Two documented gaps follow from an imported base class carrying no AST into the
    importer: an inherited `__init_subclass__` defined in another module is not
    re-validated in the importer (D-213/D-214 read `base_class_asts`), and
    `ImportedClass[...]` falls back to `Ty::Instance` because `class_getitem_return`
    is resolved from the importer's own items. Closing them needs `HirClassDef` to
    carry the validated hook and subscript shape; tracked as a v0.4 follow-up.
  - Every file of the program is re-loaded per `pycc check` path argument, so a
    shared dependency's diagnostics print once under each path. A cross-invocation
    incremental cache is a later v0.4 item.
