---
id: D-246
title: "Seed `__name__` all-or-nothing per program, deviating from CPython"
status: accepted
---

## D-246: Seed `__name__` all-or-nothing per program, deviating from CPython
- Status: accepted
- Context: `__name__` is the first Tier-0 builtin this compiler provides that
  CPython's *interpreter* supplies rather than the program (W0 of
  [#882](https://github.com/rotnov/pycc/issues/882),
  [#1156](https://github.com/rotnov/pycc/issues/1156)). It is what makes the
  `if __name__ == "__main__":` idiom compile, which the reference workload's
  modules use pervasively. Two properties of this compiler make a faithful
  reproduction of CPython's semantics unavailable. First, CPython binds
  `__name__` in the module namespace *before* any module statement runs, so a
  read that textually precedes a later `__name__ = ...` still observes the
  interpreter-provided name; this compiler has no pre-module execution phase in
  which to bind a name that the module's own statements then rebind, and every
  global has exactly one static type
  ([D-006](D-006-static-dispatch-declared-types.md)). Second, Part 1 of
  [#881](https://github.com/rotnov/pycc/issues/881) links every module of a
  program into one flat namespace, so a per-module `__name__` does not exist:
  the seed and any module's own `__name__` binding are the same global. A
  partial seed — provided sometimes, withheld other times, within one program —
  would therefore produce silently wrong values rather than diagnostics.
- Decision: the seed is all-or-nothing **per program**, not per module. It is
  emitted as the entry module's first top-level statement — an ordinary `str`
  assignment, with no new HIR, MIR, type, or codegen node — only when the module
  references the name and **no module of the program** binds the name
  `__name__` at its own top level. A binding in the entry module or in any
  dependency withholds the seed program-wide, so that binding stays the
  program's only `__name__` and the program compiles exactly as it did before
  this feature existed. A value-less annotation (`__name__: str`) emits no store
  and is not such a binding, matching CPython. A binding inside a function body
  is an ordinary local and shadows only within that function, also matching
  CPython. The accepted deviation follows: a read that textually precedes a
  module-level `__name__ = ...` does not see a module name, and is reported as
  `T0021` "name `__name__` is not defined" rather than silently yielding one
  value where CPython yields another. The value itself is `"__main__"` under
  `pycc build`, `pycc run`, and `pycc check`, and the extension module's own
  name under `pycc build --ext`.
  `docs/STDLIB_PLAN.md` and `crates/pycc_hir/src/dunder_name.rs` carry the
  canonical statement of the rule; this entry records why it takes that shape.
- Alternatives:
  - *Reproduce CPython exactly by seeding unconditionally and letting a user
    binding rebind the global.* Rejected: with one static type per global, a
    dependency's `__name__ = 7` makes the program fail to compile with `T0023`
    against a synthetic statement no user wrote, and a dependency's
    `__name__ = "dep"` is silently overwritten by the seed — a wrong value, not
    a diagnostic. Both were measured on the implementation before this decision
    was recorded.
  - *Seed per module.* Rejected: it presupposes the per-module namespaces
    #881 has not delivered. Until it does, per-module seeds are one global under
    several names, which is the previous alternative with more surface.
  - *Record the shadowing seed in `definition_spans` so `program::link` reports
    the collision.* Rejected: it converts the collision into a `C0001` reported
    against a synthetic statement, which is a worse diagnostic than withholding
    the seed and letting the user's own binding stand.
  - *Provide no `__name__` at all until #881 lands.* Rejected: it blocks the
    `if __name__ == "__main__":` idiom, and with it the main-guard shape the
    reference workload uses, for an unbounded period.
- Consequences: the `if __name__ == "__main__":` idiom compiles, and every
  divergence from CPython this design admits is a diagnostic rather than a wrong
  value. A program whose entry module reads `__name__` before assigning it does
  not compile; rewriting the read after the assignment, or removing the
  assignment, resolves it. A dependency's own top-level read is `T0021`, because
  linking places dependencies ahead of the entry module and the seed has not run
  yet, and a dependency's function-body read observes the entry module's value —
  both are the flat-namespace consequences #881 will remove. Withdrawing the
  deviation later is not a breaking change in the usual direction: it turns
  programs that are rejected today into programs that compile, so #881's
  per-module namespaces can supersede this entry without invalidating any
  program that compiles under it.
- Amendment (2026-09-20): the dependency half of the gate is widened from
  "binds the name" to "mentions the name at all", and this entry's own
  statement that "a dependency's function-body read observes the entry
  module's value" is withdrawn with it. That statement held only while nothing
  in the dependency ran before the seed, and linking places every dependency's
  top-level statements ahead of it: a dependency whose module scope neither
  binds nor reads `__name__`, but whose top-level call reaches one of its own
  functions that does, read the global before the seed had stored anything.
  `pycc check` accepted such a program and the built artifact aborted at
  codegen's uninitialized-global trap — a wrong outcome rather than a
  diagnostic, which is the one thing this entry's design is meant to exclude.
  Any mention in any dependency now withholds the seed program-wide, so that
  read is a `T0021` instead. The `T0021` this entry already predicted for a
  dependency's own top-level read is unchanged; what changes is that the same
  diagnostic now also covers the indirect and function-body cases, and that a
  dependency's `if __name__ == "__main__":` is rejected rather than silently
  taking the entry module's name. This narrows the set of programs that
  compile, so #881's per-module namespaces still supersede this entry without
  invalidating any program that compiles under it.
