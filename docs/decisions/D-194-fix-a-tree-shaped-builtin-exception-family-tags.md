---
id: D-194
title: "Fix a tree-shaped builtin exception family's tags to its class table's array index"
status: accepted
---

## D-194: Fix a tree-shaped builtin exception family's tags to its class table's array index
- Status: accepted
- Context: [D-188](D-188-synthesize-hirclassdefs-for-the-builtin-exception.md)
  and [D-189](D-189-assign-user-exception-classes-a-compile-time.md) gave the
  original flat seven builtin exception classes (`Exception`, `ValueError`,
  `TypeError`, `KeyError`, `IndexError`, `ZeroDivisionError`, `RuntimeError`)
  real `HirClassDef`s and a fixed, name-resolved runtime type tag each
  (`0..=6`), with `7..=255` reserved for user-declared classes assigned
  dynamically, in module source order, by `lower_checked`. Part 2 of
  [#543](https://github.com/rotnov/pycc/issues/543)
  ([#739](https://github.com/rotnov/pycc/issues/739)) adds the real PEP 3151
  `OSError` hierarchy: 16 more compiler-defined names — `OSError` itself, its
  10 other direct subclasses, and `ConnectionError`'s 4 further subclasses —
  three levels deep from `Exception`, unlike the original seven's flat shape.
  These 16 are not user-declared, so D-189's dynamic per-module assignment is
  the wrong mechanism: every module that seeds `FileNotFoundError` must agree
  on the *same* tag for it, the way the original seven already do, not a
  module-local one. The question this decision answers is how a second,
  compiler-defined exception family gets fixed tags without hand-maintaining a
  second table alongside `BUILTIN_EXCEPTION_CLASSES`, and without requiring
  `pycc_mir::exception::handler_type_tags`'s existing MRO-containment scan
  (which reads `HirClassDef::exception_type_tag` and `mro` directly, with no
  name-based fallback of its own) to grow special-case logic for the new
  names.
- Decision: extend `BUILTIN_EXCEPTION_CLASSES` from 7 to 23 entries — the
  original 7 followed by the 16 new names in a fixed order — and derive each
  entry's `exception_type_tag` from its **array index** inside
  `builtin_exception_class_defs()`: index `< 7` stays `None` (the original
  seven, unchanged, still resolved by name through
  `pycc_mir::exception::resolve_exception_tag`); index `>= 7` gets
  `Some(index as u8)`. This requires no formula change to the two derived
  constants: `FIRST_USER_EXCEPTION_TYPE_TAG = BUILTIN_EXCEPTION_CLASSES.len()
  as u8` evaluates to `23` once the array grows, and `MAX_USER_EXCEPTION_CLASSES
  = 256 - BUILTIN_EXCEPTION_CLASSES.len()` evaluates to `233` — both derived
  from the array's length and order, not a second hand-maintained constant.
  `builtin_exception_parent` becomes a real (non-flat) tree — `OSError`'s 10
  other direct children and `ConnectionError`'s 4 children each name their
  real immediate parent instead of assuming `Exception` — and
  `builtin_exception_class_defs()`'s MRO construction generalizes from a
  fixed 2-entry push to a `while let` walk up `builtin_exception_parent`, so a
  class like `BrokenPipeError` gets its real 4-entry MRO
  (`[BrokenPipeError, ConnectionError, OSError, Exception]`).

  Widening the seeding table from 7 to 23 names also widens the blast radius
  of the existing all-or-nothing shadow gate
  (`pycc_hir::exception::module_shadows_builtin_exception_name`): a module
  that shadows any one of the 23 names withholds class-table seeding for all
  23. For the original seven this was harmless, because `raise`/`except`
  still name-resolve their tag independent of the class table. For the 16 new
  names it is not: `handler_type_tags` has no name-based fallback, so an
  unrelated, unseeded occurrence of e.g. `FileNotFoundError` in such a module
  reached its `.expect()` and aborted the compiler. This decision's widening
  is therefore also what surfaces that crash risk — not a separate design
  choice, but a direct consequence of extending the table this decision
  covers. `pycc_types::exception::is_unshadowed_builtin_exception` was fixed
  to require actual class-table presence for any of the 16 new names (the
  original seven keep their name-only resolution), turning the crash into a
  clean `T0021`/`C0001` diagnostic; see
  [#739](https://github.com/rotnov/pycc/issues/739)'s implementation plan and
  `tests/issue_739_oserror_hierarchy.rs` for the regression coverage.
- Alternatives:
  - **A separate table and a separate first-tag constant for the 16 new
    names.** Rejected: it duplicates the "array length defines the next free
    tag" invariant across two tables instead of one, and turns
    `FIRST_USER_EXCEPTION_TYPE_TAG` into a two-term sum that can drift from
    either array it describes, for no behavioral difference from the chosen
    design.
  - **Give the 16 new names `None` too, resolved by name like the original
    seven, and extend `resolve_exception_tag` with 16 more match arms.**
    Rejected: it does not compose with `handler_type_tags`'s existing
    MRO-containment scan, which is invisible to a `None`-tagged class and
    would itself need special-case name matching to stay correct — exactly
    the coupling this decision avoids. It also fails on a different axis than
    the shadow-gate crash above: in a module where seeding was withheld but
    every name still resolved fine through a hypothetical extended
    `resolve_exception_tag`, `except OSError:` would scan an empty class
    table and resolve to `OSError`'s own hardcoded tag alone, while
    `raise FileNotFoundError(...)` would resolve via the same hypothetical
    name table to its own distinct tag, independent of the table — the raised
    exception would then simply not match the handler's tag set and
    propagate uncaught. That is a silently wrong program, not a diagnostic
    and not a panic; the chosen design's loud, compile-time `T0021` is
    strictly better.
- Consequences: `BUILTIN_EXCEPTION_CLASSES`'s array order is now load-bearing
  in a way it was not before — reordering the array silently renumbers every
  tag from that point on, including every already-shipped user-class tag
  range starting at `FIRST_USER_EXCEPTION_TYPE_TAG`. A pinned test
  (`crates/pycc_hir/src/exception.rs`'s
  `builtin_exception_tags_are_pinned_to_their_array_index`) locks the exact
  index-to-tag mapping so a future reorder is a loud test failure, not a
  silent renumbering. This is the pattern any *future* compiler-defined
  exception family (e.g. a hypothetical `Warning` hierarchy) would also
  follow: extend the one array, derive tags from index, and extend
  `builtin_exception_parent`'s tree — not invent a parallel mechanism per
  family. `MAX_USER_EXCEPTION_CLASSES` shrinks from 249 to 233 as a direct,
  intended consequence of reserving 16 more fixed tags; a module already at
  or above 233 user-declared exception classes (extremely unlikely in
  practice) would newly hit the existing `C0001` cap.
