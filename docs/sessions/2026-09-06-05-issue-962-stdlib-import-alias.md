# 2026-09-06 — #962: stdlib `import X as Y` module aliasing (Part 1 of #883)

## Overall status

Implemented [#962](https://github.com/rotnov/pycc/issues/962) on
`feat/issue-962` in the `issue-883` worktree, cut from `origin/main` at
`f29d245b` (the #961 merge). Five commits, all local at the time of this
snapshot -- nothing pushed, no pull request opened; the orchestrating
`issue-implement` session reviews, pushes, and opens the pull request. The
pull request must reference #962 with a closing keyword and #883 without one:
#883 is the parent and stays open.

The plan is the `issue-to-plan` comment on #962, fetched in full before the
first edit and followed as written except where "Deviations from the plan"
below says otherwise.

## What the change is

[#883](https://github.com/rotnov/pycc/issues/883) (from the meddylib sweep)
was decomposed by architectural seam into three dependency-ordered issues in
the `v0.4` milestone:

- [#962](https://github.com/rotnov/pycc/issues/962) -- Part 1, a stdlib
  module behind an alias (`import math as m`). **This change.**
- [#963](https://github.com/rotnov/pycc/issues/963) -- Part 2,
  `from X import a as b`. Open, blocked on this part.
- [#964](https://github.com/rotnov/pycc/issues/964) -- Part 3, a project
  module behind an alias. Open, blocked on this part and on the
  module-namespace work.

The design is [D-231](../decisions/D-231-lower-stdlib-import-x-as-y-to-canonical-names-with-an.md).
In one paragraph: `pycc_std` gains `module_name`, the inverse of
`resolve_module`; `pycc_hir` threads the module's import table as a trailing
`imports: &[ImportBinding]` parameter through every lowering entry point,
resolves a receiver against that table (last binding wins, alias beats the
textual module name) before the textual `resolve_module` fallback, and always
emits the canonical `math.sqrt`/`math.pi` string so `pycc_mir` and
`pycc_codegen` are untouched; diagnostics append `` (imported as `m`) `` only
when the spelling differs. `pycc_types` makes the D-136 receiver shadow check
alias-aware through one `bind_std_module_aliases` table on both environments
and a `shadowed_std_receiver` helper that checks the canonical spelling and
every alias bound to the module, consulting `binding_state` and the
source-order `def_rebound`/`defs_rebound` sets.

Commits, oldest first:

1. `feat(std): add module_name, the canonical inverse of resolve_module`
2. `feat(hir): lower import <stdlib module> as <alias> and resolve aliased receivers`
3. `feat(types): make the stdlib receiver shadow check alias-aware`
4. `test: pin stdlib import aliasing end to end`
5. `docs: record stdlib import aliasing (D-231) and refresh the status page`

plus this session file.

Two behaviour changes for the *canonical* spelling ride along, both in the
CPython-faithful direction and recorded in D-231 point 4: a module-level
conditionally bound `math` (`if c: math = 2.0`) and a `def math()` above the
use are now `C0001` where they used to reach libm; a `def math()` below a
module-level use stays accepted.

## Deviations from the plan

- The plan placed the two-module "program-wide alias table" pin in
  `src/modules/tests.rs`. That harness exercises `modules::load`, which
  resolves and links but never type-checks, so the residual is not observable
  there. Both linked-program tests (the false reject and the clean control)
  live in `tests/issue_881_project_imports.rs`, which drives the real `pycc
  check` binary against a scratch directory tree.
- Un-annotated solver-path fixtures use a private helper (`def _f(m):`); a
  public un-annotated function is `T0001` before any receiver check runs.
- `ConstraintEnvironment::empty` is `#[cfg(test)]`: production code never
  builds an environment without bindings, and an unconditional constructor
  is a `dead_code` error under `-D warnings`.
- `shadowed_std_receiver` returns `Option<&str>` with elided lifetimes
  (clippy `needless_lifetimes`) rather than the plan's explicit `'a`.
- The plan's non-code deliverable "leave a comment on #798 noting that the
  `TYPE_CHECKING` guard now folds through a `typing` alias" is deferred to the
  orchestrator: this session has no GitHub write access by its brief.
- The `--include-ignored` test run and the conformance gate could not be
  taken against the pinned oracle on this machine: `python3.14` here is
  3.14.6, and `tests/conformance.rs` refuses anything but 3.14.7, so every
  oracle-diffed test in that file (pre-existing ones included) fails at the
  version assertion. `tests/fixtures/conformance_import_alias_math.py` was
  diffed by hand against the local 3.14.6 in both profiles (identical output:
  `4.0`, `3.141592653589793`, `5.0`); CI's pinned oracle is the authority.

## Known follow-ups

- [#963](https://github.com/rotnov/pycc/issues/963) and
  [#964](https://github.com/rotnov/pycc/issues/964) -- the remaining parts of
  #883, in dependency order. #964 wants the `local_name` plumbing this change
  added and the per-module namespaces of #901.
- [#768](https://github.com/rotnov/pycc/issues/768) -- closes D-231 residual
  (a): a local named `math` while the module only ever writes `m.sqrt` is a
  false reject because the HIR keeps only the canonical spelling.
- [#901](https://github.com/rotnov/pycc/issues/901) -- closes D-231 residual
  (b): the alias table is program-wide across linked modules, pinned by
  `a_stdlib_alias_in_a_dependency_is_visible_to_the_whole_linked_program`.
  When #901 lands, that test must flip to a clean check.
- [#798](https://github.com/rotnov/pycc/issues/798) -- the `TYPE_CHECKING`
  guard fold is still not import-gated for the bare name; it now folds
  through `<alias>.TYPE_CHECKING` when the alias is bound to `typing`, which
  narrows nothing and widens nothing about #798 itself.
- Attribute-form bases, decorators, and annotations (`class C(enum.Enum)`,
  `class C(e.Enum)`) stay `C0001` in both spellings -- unchanged by this
  change, not a regression.

## Where to resume

Read `crates/pycc_hir/src/expr/std_receiver.rs` first (twenty lines: the
alias-then-textual resolution and the diagnostic spelling), then
`crates/pycc_types/src/std_receiver.rs`, whose doc comments carry the reasons
for every choice in the shadow check -- why `binding_state` and not `lookup`,
why `def_rebound` and not `lookup_function`, and why the check must try every
spelling bound to the module. The four call sites are the two arms in
`pycc_types/src/expr.rs` (`is_std_receiver_bound`) and the two in
`pycc_types/src/constraints.rs` (`ConstraintEnvironment::is_std_receiver_bound`).

The alias table is populated in exactly two places, and the comment at each
says why that place: `module::check_with_environment_all`'s entry (the common
sink of both `Environment` constructors) and the globals literal in
`constraints/signatures.rs`, whose per-function literal copies the field
explicitly because it is a field-by-field copy, not a `.clone()`. Anyone
adding a field to `ConstraintEnvironment` should add it to `empty` and to
that per-function literal in the same edit.
