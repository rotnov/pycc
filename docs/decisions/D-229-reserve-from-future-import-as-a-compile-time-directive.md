---
id: D-229
title: "Reserve `from __future__ import ...` as a compile-time directive at both import sites"
status: accepted
---

## D-229: Reserve `from __future__ import ...` as a compile-time directive at both import sites

- Status: accepted (issue [#919](https://github.com/rotnov/pycc/issues/919)).
- Context:
  `from __future__ import annotations` is the first line of a large share of
  real typed-Python files, and pycc rejected it with `C0001: import of module
  `__future__` is not supported yet` -- the `pycc_std::resolve_module`
  fallback in `crates/pycc_hir/src/import.rs`'s `ImportFrom` arm. The
  directive asks for exactly what pycc already does (annotations are
  evaluated statically, at compile time, per `docs/TYPE_SYSTEM.md`), so the
  rejection was a false capability gap. Reproducing the issue at `4eca5e24`
  showed the fix has three sites, not one: (a) the driver's project resolver,
  reached through `project_import_request`, resolved `__future__` against
  the project directory, so a sibling `__future__.py` was loaded and its
  names bound (a `main.py` doing `from __future__ import annotations;
  print(annotations)` next to a `__future__.py` with `annotations = 3`
  compiled and printed `3`); (b) the HIR `ImportFrom` arm the issue names;
  (c) `module::poisonable_names`'s `ImportFrom` arm, which must mirror the
  lowering's success condition exactly under [D-222](./D-222-project-modules-link-at-the-hir-level-into-one.md)
  or the `IMPORT_SHAPES` biconditional test fails. CPython 3.14 also has a
  precise grammar around the directive: only a module docstring and other
  future imports may precede it, an unknown feature name (including `*`) is
  `SyntaxError: future feature <name> is not defined`, `braces` is
  `SyntaxError: not a chance`, and `barry_as_FLUFL` is a valid feature that
  changes the grammar (`<>` replaces `!=`).
  [#882](https://github.com/rotnov/pycc/issues/882), the `pycc_std`
  registry-widening issue, had listed `__future__.annotations` among its
  "typing-only registration" candidates.
- Decision:
  `__future__` is a **compiler directive, never a module**, reserved at both
  module-level import sites through one shared predicate
  (`import::is_future_import`: `level == 0` and module `__future__`):
  - `project_import_request` returns no request for it, so the driver never
    probes the project for `__future__.py`; a bare `import __future__` is
    deliberately unchanged (still a request, still `C0001`).
  - `lower_import_stmt` routes it to `lower_future_import` before the
    registry fallback, with this precedence ladder (CPython 3.14's, verified
    against `compile()`): (1) a future import after the prologue is `L0001`
    ``from __future__ imports must occur at the beginning of the file``
    regardless of names; (2) names left to right -- `braces` is `L0001` ``not
    a chance``, `*` and any other unknown name is `L0001` ``future feature
    <name> is not defined``; (3) any `as` alias is the generic `C0001`
    aliasing gap; (4) `barry_as_FLUFL` is `C0001` ``the `barry_as_FLUFL`
    future feature (`<>` in place of `!=`) is not supported yet``. The nine
    remaining names (`annotations` and the eight mandatory features:
    `nested_scopes`, `generators`, `division`, `absolute_import`,
    `with_statement`, `print_function`, `unicode_literals`,
    `generator_stop`) lower to nothing: no binding, no `HirItem`.
  - The prologue is computed once per module by `future_prologue_len` (an
    optional docstring at index 0 -- a bare `Expr::StringLiteral` statement
    only, so an f-string or bytes literal is not one, matching CPython --
    then a contiguous run of future imports) and threaded to
    `lower_import_stmt` as a two-variant `FuturePosition` enum.
  - `poisonable_names` gets an `is_future_import` branch first: every name a
    no-op feature and no alias yields nothing; otherwise the names it would
    have bound, alias-aware, so `barry_as_FLUFL` (CPython-accepted,
    pycc-rejected) poisons.
  - `L0001` for the CPython-rejected shapes follows D-148/D-149/D-193's
    "CPython `SyntaxError` caught at HIR lowering" precedent; `C0001` stays
    reserved for valid Python the frontend does not lower yet.
  - No `pycc_std` registry entry is added; this supersedes #882's
    `__future__.annotations` candidate for that one item.
  - `tests/fixtures/pep_0563_lazy_annotations.py` is authored and registered
    as an `#[ignore]`d dual-profile conformance test; the matrix row stays
    `☐` under rule 5 until [#937](https://github.com/rotnov/pycc/issues/937)
    flips it.
- Alternatives:
  - A `pycc_std` registry entry for `__future__` -- rejected with the
    issue's own reasoning: the directive has no module surface, and a
    registry entry would still leave site (a) resolving a sibling
    `__future__.py` first.
  - Handling `__future__` in the driver only -- rejected: single-file
    `check` never runs the driver's resolver (`lower_all` passes an empty
    `ResolvedImports`).
  - `C0001` for an unknown feature name -- rejected: contradicts
    `docs/DIAGNOSTICS.md`'s `C0001`/`L0001` split and D-148.
  - Accepting `barry_as_FLUFL` as a no-op -- rejected: it would silently
    compile a program CPython rejects (`1 != 2` is a `SyntaxError` under it).
  - A pre-pass stripping future imports from the AST before lowering --
    rejected: loses the span the `L0001`s report at and bypasses the poison
    mirror.
- Consequences:
  - Easier: every `from __future__ import annotations` file, and every ported
    file carrying the mandatory features, now compiles; the diagnostics for a
    typo or a misplaced directive match CPython's wording.
  - Recorded divergences, all deliberate:
    - CPython binds each feature name to its `__future__._Feature` object
      (or, with a sibling `__future__.py`, to that file's value, found first
      on `sys.path` at run time); pycc binds nothing, so a later read of
      `annotations` is `T0021`, and the `__future__.py` project prints
      nothing where CPython prints `3`. No real program reads the feature
      object, and a project module named `__future__` is not a supported
      layout.
    - The poison mirror is position-blind (it sees one statement, not its
      index), so a *late* future import fails with the position `L0001` and
      poisons nothing -- a divergence from D-222's "a failing import poisons
      what it would have bound" rule that is harmless because an accepted
      future import binds nothing either, so no later diagnostic can be a
      cascade of it. Likewise `from __future__ import *` poisons the literal
      name `*`, which nothing can later name.
    - Two non-module-level sites are left as they are: a future import inside
      a function or block body keeps today's generic block-import `C0001`
      (that path never reaches `lower_import_stmt`; the block-import gap is a
      separate, broader issue), and one inside an `if TYPE_CHECKING:` body is
      silently accepted because the whole body is constant-folded away before
      lowering (`crates/pycc_hir/src/stmt.rs`). Neither is given a test that
      would lock the divergence in.
  - Harder: `lower_import_stmt` carries a `FuturePosition` parameter its one
    caller must compute; any future statement-level directive (a hypothetical
    `from __pycc__ import ...`) would follow this same reserved-at-both-sites
    shape rather than the registry.
