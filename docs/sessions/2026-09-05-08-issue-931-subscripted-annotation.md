# 2026-09-05 — #931: reject a subscript on a non-class annotation base with `T0044`

## Previous checkpoint's outcome

The preceding snapshot (`2026-09-05-07`) delivered
[#941](https://github.com/rotnov/pycc/issues/941) as PR
[#945](https://github.com/rotnov/pycc/pull/945), which merged as `f37ca75f`
while this task was in flight. This task was planned and first implemented
against `e6781b2c` (the `origin/main` its worktree `feat/issue-931` was cut
from) and rebased onto `f37ca75f` before the gates below were re-run; the
two changes touch the same three documentation files (`docs/DIAGNOSTICS.md`,
`docs/ROADMAP.md`, `docs/TYPE_SYSTEM.md`) in textually disjoint places, so
the rebase was clean.

## Overall status

Implemented [#931](https://github.com/rotnov/pycc/issues/931) end to end on
`feat/issue-931` from the issue's five-round plan
([comment](https://github.com/rotnov/pycc/issues/931#issuecomment-5553704081)):
one code seam in `pycc_hir`, unit and CLI tests, and the documentation that
described the old behavior. One pull request, body carrying `Fixes #931`;
the orchestrating `issue-implement` session opens it, watches CI, runs the
D-068 reviewer, and merges. No new ADR (the change closes a gap inside the
#611 rule and supersedes nothing; the plan's section 6 records why).

## What the change is

`crates/pycc_hir/src/func.rs::annotation_to_ty`'s `Expr::Subscript` fallthrough
arm used to recurse on the bare base name and silently discard the type
argument for every base that was not a known class, so `T[int]`, `int[str]`,
`Self[int]` and `type A = int` / `A[str]` all lowered to the base's own type.
It now:

1. resolves the class the base denotes — directly, or through a `type A = C`
   alias, which PEP 695 makes transparent (`A[int]` behaves exactly as
   `C[int]`). The alias table is consulted only for a name the bare-name arm
   would itself resolve through it (not a type parameter, `Self` inside a
   class, the enclosing class's own name, a builtin scalar, or `Any`), and
   the direct `class_defs` lookup is gated on the type parameter only, so
   `def f[G](x: G[int])` inside a program that also defines `class G[U]` is
   the type parameter, as the bare-name arm already said;
2. runs the unchanged #611/#693 known-class ladder, with one wording change:
   the trailing "so `X[...]` is not a valid type annotation" clause now
   spells the *written* base (`A`, `list`) while the first clause keeps the
   class noun, so the text agrees with the caret (byte-identical for every
   pre-existing fixture, where base and class coincide);
3. fails open for an alias whose target is a class absent from `class_defs`
   (`from lib import A` where `lib` has `type A = G`; `import.rs` copies
   only Class bindings — a pre-existing #881-area gap, out of scope);
4. keeps the D-228 builtin-container branch as it was; and
5. resolves the bare base one last time so an undefined name keeps the exact
   `C0001` that `module::cascade_name` parses back (D-219) and `Any` keeps
   `T0002`, then rejects everything else with `T0044`:
   `<noun> is not subscriptable, so `X[...]` is not a valid type annotation`,
   where the noun (`subscripted_base_description`) is `type parameter `T``,
   ``Self``, `builtin type `int``, or `type alias `A``, chosen in the
   bare-name arm's own precedence. `help` is `None` (D-152: no determinate
   safe replacement).

Intentional consequences the plan records and the tests pin:
`def f[T](x: list[T[int]])` now reports `T0044` where the container element
gate used to report `T0042`; `def f[list](x: list[int])` and
`type list = int` / `x: list[int]` are rejected where they used to lower to
`Ty::Param("list")` / `Ty::Int`; `class C:` + `def f[C](x: C[int])` names the
type parameter instead of the class; `type list = C` / `x: list[int]` is the
class-flavored `T0044` where it used to lower silently.

- `crates/pycc_hir/src/tests/subscript_annotations.rs` (new, per the
  AGENTS.md decomposition rule and #663): the contiguous PEP 560 block and
  the two builtin-container shadowing tests moved out of `tests.rs`
  (6,939 → 6,613 lines), the two breaking tests rewritten, and 12 new
  tests covering every position, every noun, every gate and precedence
  rule, alias-to-class transparency, the unchanged outcomes, and a direct
  unit test of `subscripted_base_description`'s defensive `class_name` arm.
- `crates/pycc_hir/src/module/tests.rs`: `x: Foo[int]` after a failed
  `class Foo:` is still a silent D-219 cascade.
- `crates/pycc_hir/src/import/tests.rs`: the step-3 fail-open path through
  the multi-module harness, with the bare `x: G` `C0001` proving `G` is
  absent from the importer.
- `tests/diagnostics/`: six new `T0044` fixtures (type parameter, builtin
  scalar, `Self`, non-class alias, class attribute, return position) plus the
  JSON shape for the type-parameter one (`"help":[]`), all captured from the
  real CLI and registered in `tests/diagnostics_test.rs`.
- `crates/pycc_diag/src/explain.rs` `T0044` explanation, the
  `docs/DIAGNOSTICS.md` `T0044` row, `docs/TYPE_SYSTEM.md` (the
  `__class_getitem__` parenthetical and the generics paragraph), and the
  existing `#435` paragraph in `docs/ROADMAP.md` (extended, not a new
  feature paragraph, so the status page's pin set is untouched).

## Gates run locally on the rebased head

`cargo doc --workspace --no-deps`, `cargo fmt --all -- --check`,
`cargo clippy --workspace --all-targets -- -D warnings`,
`cargo build --workspace`, `cargo test --workspace`,
`cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`
(TOTAL 100.00% lines, 100.00% regions), the `scripts/` Python unit tests,
`check_roadmap_evidence.rb`, `generate_decisions_index.py --check`,
`check_status_page_freshness.rb origin/main HEAD .`,
`check_scratch_dir_usage.py`, `check_conformance_breadth.py`, and
`check_ci_permissions.rb` — all exit 0.

## In flight / follow-ups

- The pull request for this branch is opened, reviewed (D-068), and merged
  by the orchestrating session; CI and review threads are its to watch.
- Out of scope, recorded in the plan: `from typing import Self` (`C0002`),
  generic aliases and bounded type parameters, copying an imported alias's
  target class into the importer's `class_defs` (the reason the fail-open
  step exists), and type-argument arity checks for generic and hook classes.

## Where to resume

`crates/pycc_hir/src/func.rs` (`annotation_to_ty`'s `Expr::Subscript` `_ =>`
arm and `subscripted_base_description`),
`crates/pycc_hir/src/tests/subscript_annotations.rs`, and the plan comment on
#931.
