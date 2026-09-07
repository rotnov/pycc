# 2026-09-07-01 — #974: a class-name-qualified read of an inherited class attribute

## Status

Implementation complete on the branch and committed; **no pull request opened**. This session ran as `issue-implement`'s step 4 under [D-142](../decisions/D-142-issue-implement-s-step-4-implementation-runs-in-a.md); the orchestrating session opens the PR after reviewing the report.

- Worktree: `/Users/denis/projects/pycc-worktrees/issue-974`, branch `feat/issue-974`.
- Base: `origin/main` = `3ba4a027` (re-fetched during the session; unchanged from the plan's own baseline).
- Issue: [#974](https://github.com/rotnov/pycc/issues/974), milestone v0.4.
- Plan followed: [the issue's plan comment](https://github.com/rotnov/pycc/issues/974#issuecomment-5569735233), which survived four adversarial review rounds and carries six numbered corrections to the issue text.

## What changed

A class-name-qualified read of an **inherited** class attribute (`Derived.LIMIT` where a base declares `LIMIT`) was rejected with `T0044` while `derived_instance.LIMIT` succeeded. CPython accepts both. The fix walks the MRO on the class-name path too, under a rule the plan's corrections made specific:

- **New shared predicate** `pycc_hir::declares_name_outside_class_attrs` (`crates/pycc_hir/src/class/shadow.rs`, a new submodule rather than more lines in the 4.5k-line `class.rs`). It answers whether a class declares a name in `methods`, `static_methods`, `class_methods`, `properties`, `enum_members`, or as a `ProtocolMember::Method`. `attrs` (instance slots) and `ProtocolMember::Attribute` are excluded on purpose — a class object has no instance `__dict__`, and a bare `x: int` in a protocol body is an interface requirement rather than a binding. `abstract_methods` is redundant (pushed into `methods` at lowering) and `dataclass_fields` is a subset of `attrs`. The `Method` half was carved back out of that exclusion by the deep-review round below.
- **`pycc_types::class::lookup_class_attr_by_class_name`** — a *new* helper beside `lookup_class_attr_through_mro`, which is left untouched: its four other callers (`resolve_attr_get`, `resolve_super_attr_get`, `check_attr_set`, `check_protocol_conformance`, plus the non-`Name`-base gate) do not want the shadow rule. `crates/pycc_types/src/expr.rs`'s class-name `AttrGet` arm routes through it.
- **`crates/pycc_mir/src/expr.rs`** — the fused three-condition `let`-chain was split so the same MRO walk over the same shared predicate runs on the fold side, with a guarded internal-error panic. The guard matters: the panic fires only when `classes.get(class_name)` *succeeded* and the walk then missed; a `classes.get` failure still falls through to `lower_expr` (the ordinary `d.LIMIT` local/parameter case).

Sharing the predicate rather than duplicating it is the #960 precedent applied deliberately: when the checker's lookup and the MIR fold drift, the result is a silently wrong value, and a `pycc_codegen` abort when the two declared types differ.

## The two traps the plan identified, and how they were avoided

1. **Do not route the class-name read through the instance path's MRO walk.** The two paths diverge on purpose, and CPython agrees. Pinned as one program in `tests/issue_974_inherited_class_attr_by_class_name.rs`: for `class A` establishing an instance slot `x = 1` in `__init__`, `class B` declaring `x: int = 2`, and `class C(A, B)`, `c.x` is `1` (#960's slot precedence) and `C.x` is `2`.
2. **Do not use `lookup_class_attr_through_mro` alone either.** It iterates `class_attrs` only, so for `class A: x: int = 2` / `class B(A)` re-declaring `x` as a `@staticmethod`, it would answer `A`'s `2` and turn today's spurious `T0044` into a wrong-value mis-compile of a program that compiles correctly today. All four shadow shapes (`@staticmethod`, plain method, `@classmethod`, `@property`) are pinned as staying `T0044`, and the `@staticmethod` case is paired with a build-and-run test asserting `B.x()` still prints `1`.

## Decisions made in this session

- **`enum_members` is included in the shared predicate**, not omitted. The plan left this open as a judgment call, framed against a *two-predicate* design where the MIR copy's `enum_members` closure would have been uncoverable under D-014. Sharing the predicate (which the plan separately mandated) collapses the tradeoff: the closure exists exactly once and workspace coverage reaches it from either crate. It is exercised by `Color.NOPE` on `class Color(Enum): RED = 1`, which falls past `enum_member_attr_type` into the new walk with a non-empty `enum_members` table. The namespace is behaviourally inert — an enum's MRO is self-only and nothing may inherit from an enum — and the predicate's doc comment says so.
- **No ADR.** Neither #960 nor #915, the two closest precedents for a cross-crate read-path precedence rule, carries one. The honest counter-argument is that the class-name path's *deliberate* divergence from #960's instance precedence is exactly the kind of choice an ADR records, and that `docs/TYPE_SYSTEM.md`'s read bullet is now the only place it is written down. If a reviewer disagrees, the entry is cheap to add at PR-open time at the next free number.
- **No `site/status/index.html` change.** The `docs/ROADMAP.md` edit appends a sentence *inside* the existing #910/#911 paragraph rather than adding or removing a feature-landing paragraph, so the freshness gate that would require the status page (and its four-pin rotation) does not fire.
- Per [D-127](../decisions/D-127-autonomous-agent-operation-model.md), both judgment calls above were resolved by consulting this session's advisor rather than the repository owner.

## Documentation

`docs/TYPE_SYSTEM.md` carries four edits, all in the same commit as the code:

- The paragraph at the end of the `ClassVar`-in-a-dataclass section that documented this exact defect and cited #974 is rewritten to record the fix.
- The "Class-level attributes" read bullets gain a new leading bullet stating the class-name rule precisely — MRO walk, first-declaring-class-wins, the shadow stop, the shared predicate, and the explicit statement that it does **not** apply #960's instance precedence, with the measured `c.x` = 1 / `C.x` = 2 pair.
- The read-only bullet's claim that every write path is `T0044` was **wrong** and is corrected: `obj.X = ...` and `self.X = ...` are `T0044`, but `C.X = ...` (own or inherited alike) is `T0021` ``name `C` is not defined`` — the store side has no class-name arm, so the class name is inferred as a value binding and fails there first. Tracked with the rest of that family by [#965](https://github.com/rotnov/pycc/issues/965).
- The `Final[X]` section repeated the same wrong claim as its justification for accepting `Final` on a class attribute without modelling a new guarantee; it moves with the correction.

`crates/pycc_types/src/class/binding.rs`'s `expect_class` doc comment claimed every caller passes a name extracted from a `Ty::Instance` payload. The new helper is entered from a name `env.lookup_class` itself just resolved, so the comment now states both provenances.

## Follow-up filed

[#989](https://github.com/rotnov/pycc/issues/989) (v0.4) carries four measured findings deliberately left out of this change, each with its own minimal reproduction and measured actual-versus-expected:

1. A class pattern matching an inherited class attribute (`case B(LIMIT=v)`) passes `pycc check` and panics in `pycc build` at `crates/pycc_mir/src/matching.rs:375` — an ICE where CPython prints `8`.
2. `crates/pycc_types/src/lib.rs` (~:1620-1625) types a class-pattern keyword sub-pattern from the matched class's own `attrs`, so an inherited *instance* attribute falls back to `Ty::Infer` and the binding is spuriously rejected.
3. `class Box[T]: LIMIT: int = 8` then `Box.LIMIT` passes `pycc check` and fails `pycc build` with `T0021` — a check/build divergence, which is why #974's test matrix deliberately carries no PEP 695 case.
4. The instance-path counterpart of this change's shadow rule is a **mis-compile**: `class A: x: int = 2` / `class B(A): def x(self)` then `b.x` builds and prints `2`, where CPython gives a bound method. Checker and MIR agree on the wrong answer, so #960's cross-crate guard does not catch it.

## Deep-review round (D-068)

The pinned local reviewer (`ievo:deep-reviewer`) ran against the full committed range from the merge base through the branch head and returned one blocker and two documentation notes. All three were fixed on the branch; the pile is `.harden/findings/issue-974.jsonl`.

- **Blocker, confirmed by measurement.** The predicate excluded `protocol_members` wholesale. That is right for `ProtocolMember::Attribute` but wrong for `ProtocolMember::Method`: a `Protocol` class executes its body like any other class, so `def x(self) -> int: ...` really binds `x` in `P.__dict__`. Measured on this machine: for `class P(Protocol): def x(self) -> int: ...` / `class A: x: int = 2` / `class C(P, A)`, CPython prints the function object while pycc printed `2`. Both crates agreed on the wrong answer, so the shared predicate could not catch it — the guard protects against drift between the two walks, not against a shared premise being wrong. The exclusion is now narrowed to the `Attribute` variant and `C.x` stays `T0044`, consistent with pycc not modelling a bare, uncalled class-name-qualified method read. Pinned by `a_protocol_method_member_shadows_an_inherited_class_attribute`; the pre-existing positive test was renamed `a_protocol_attribute_member_does_not_shadow_an_inherited_class_attribute` so the pair reads as the two halves it is.
- **Note, `docs/ROADMAP.md`.** Its #974 sentence listed four of the shadow namespaces and omitted `enum_members`; it now lists all of them plus the protocol `def`.
- **Note, `docs/TYPE_SYSTEM.md`.** The read bullet described the MRO walk without disclosing that a PEP 695 generic class does not follow it (`Box.LIMIT` passes `pycc check` and fails `pycc build` with `T0021`). The divergence predates #974 and is item 3 of #989; the bullet now says so inline.

## Tracker observations (recorded here, not filed)

Two things the `issue-select` pass measured that are worth a future session's attention. Neither is filed as an issue, because [D-192](../decisions/D-192-bound-the-tracker-with-milestone-at-filing-a.md)'s non-milestone ceiling forbids it.

- **The non-milestone open-issue count is 67 against D-192's ceiling of 20.** The ceiling is therefore permanently in force, and no new non-milestone issue may be opened. Triage cannot drain this structurally: the count only falls when non-milestone issues close, and the 4:1 merge quota caps how fast that can happen. Whoever revisits D-192 should decide whether the ceiling needs a one-time reconciliation pass rather than treating 67 as a transient overshoot.
- **#663, #695 and #696 are per-oversized-file decomposition trackers carrying no milestone, but D-192's exemption enumerates only #544–#552.** They are the same kind of artefact as the enumerated set and were presumably meant to be covered. As written they are ordinary non-milestone issues consuming the ceiling. Fixing this means amending D-192's enumeration, which is an ADR edit rather than a drive-by write on the issues themselves.

## Where to resume

- The branch is committed, gate-green locally, and has been through the D-068 review round above; the next step is the PR.
- The D-014 gate was red for one round on a single region in `crates/pycc_mir/src/expr.rs` that every merged coverage view reported as covered. `llvm-cov`'s file summary takes `min(NotCovered)` across a function's instantiation records rather than merging them, so a region needs to be covered within one *single* instantiation. `cargo llvm-cov report -p pycc_mir --show-instantiations --html` is the view that names such a miss; `docs/AGENT_RETROSPECTIVE.md`'s 2026-09-07 entry records the full diagnosis. The fix was the extra `pycc_mir` unit test `an_inherited_class_attribute_read_through_the_derived_class_name_folds`, which walks past a derived class declaring nothing and folds the base's constant.
- `cargo test --workspace -- --include-ignored` fails 57 conformance tests **locally only**, all with the identical message ``conformance oracle must be exactly Python 3.14.7, found "Python 3.14.6"``. That is this machine's interpreter version, not the change: no other panic appears in the log. CI pins 3.14.7 and is the authority.
- `wc -c docs/ROADMAP.md` is 168915 against `scripts/check-site.sh`'s 168960-byte ceiling — 45 bytes of headroom left. This has now been the binding constraint on three consecutive merges, and the review round's own ROADMAP correction had to be cut to a bare namespace list to fit. The next roadmap sentence will not fit at all; raising `budget_bytes` for that entry in `site/llms-txt-context-manifest.json` is the intended route (the aggregate ceiling has room), not trimming unrelated prose.
