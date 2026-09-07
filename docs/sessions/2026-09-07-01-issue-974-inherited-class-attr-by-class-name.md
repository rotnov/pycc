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

## Codex review round on PR #992 (round 2)

The optional `chatgpt-codex-connector` review raised one P1 on the `#436` class-name `AttrGet` arm: it fired whenever `env.lookup_class(class_name)` succeeded, without first asking whether an active value binding claims that name. Confirmed by measurement rather than by reading, against CPython 3.13.9 and against the pre-#974 base commit `3ba4a027`:

| shape | CPython | branch before the fix | base `3ba4a027` |
|---|---|---|---|
| inherited class attribute | `3` | `2` | `T0044` |
| own-declared class attribute | `3` | `2` | `2` |
| enum member | `3` | `T0022` | `T0022` |

Only the first row is a regression this issue introduced; the other two predate it.

**Scope fork, resolved per [D-127](../decisions/D-127-autonomous-agent-operation-model.md) by consulting this session's advisor.** The fork was whether to fix only the regression row and leave the two pre-existing rows to a follow-up. The advisor's decisive argument is structural, not a preference: the shadowing question is answered *before* the walk knows whether the hit will be own, inherited or an enum member, so a guard that fixes only the inherited row does not exist at this seam — it would have to run the MRO walk and then retract, which is strictly worse code than the correct predicate. Fixing all three is therefore the cheaper option as well as the more honest one, and the widened diff is a consequence of the guard the regression needs anyway rather than scope creep. Recorded here as the decision made.

The guard is the `binding_state(name).is_none() && !is_local(local_names, name)` pair the class-name `Subscript` (`__class_getitem__`) path in the same file already used, placed before the whole block. `pycc_mir` needed the mirror on **two** sites, not one: the class-attribute fold, and the enum-member interception that runs before it. That second site was not hypothetical — with only the fold guarded, the enum repro type-checked and then failed LLVM module verification (`ret ptr` from an `i64` function), because MIR still lowered `Color.RED` to the member singleton. Guarding `pycc_types` alone would also have routed a shadowed name into the fold's internal-error `panic!`.

Three tests pin the shapes (`an_active_binding_shadows_an_{inherited,own,enum_member}_class_attribute_read`, all asserting stdout `3`, never merely exit 0). The whole workspace suite was re-run after the guard to check nothing depended on the class-name arm winning a shadow race; nothing did.

## Tracker observations (recorded here, not filed)

Two things the `issue-select` pass measured that are worth a future session's attention. Neither is filed as an issue, because [D-192](../decisions/D-192-bound-the-tracker-with-milestone-at-filing-a.md)'s non-milestone ceiling forbids it.

- **The non-milestone open-issue count is 67 against D-192's ceiling of 20.** The ceiling is therefore permanently in force, and no new non-milestone issue may be opened. Triage cannot drain this structurally: the count only falls when non-milestone issues close, and the 4:1 merge quota caps how fast that can happen. Whoever revisits D-192 should decide whether the ceiling needs a one-time reconciliation pass rather than treating 67 as a transient overshoot.
- **#663, #695 and #696 are per-oversized-file decomposition trackers carrying no milestone, but D-192's exemption enumerates only #544–#552.** They are the same kind of artefact as the enumerated set and were presumably meant to be covered. As written they are ordinary non-milestone issues consuming the ceiling. Fixing this means amending D-192's enumeration, which is an ADR edit rather than a drive-by write on the issues themselves.

## The fourth site: a class-name method call (round 3)

While cleaning up the throwaway worktree after the Codex round, the same defect class
was probed at a fourth site — `crates/pycc_types/src/expr.rs`'s class-name `MethodCall`
arm, guarded only by `lookup_class(...).is_some() && has_static_or_class_method(...)`,
with no binding/local check. It misfires:

| program | CPython 3.13.9 | pycc before | pycc after |
| --- | --- | --- | --- |
| `def f(B: D) -> int: return B.m()`, `B.m` a `@staticmethod` | `3` | `2` | `3` |
| the same with `B.m` a `@classmethod` | `3` | `2` | `3` |

**The decision, per D-127, taken with the advisor rather than deferred.** The fork was
whether to extend this pull request's guard to the fourth site or to record it and leave
it out, given that D-192's non-milestone open-issue ceiling stands at 67 against a
ceiling of 20 and forbids filing a new non-milestone issue. The advisor's argument is
what settled it, and it is not about scope: the round-2 documentation this pull request
already carries states the general rule — a class name is a class name only when no
active value binding claims it — as a spec sentence in `docs/TYPE_SYSTEM.md` and as a
roadmap claim. Shipping that sentence with a known counterexample inside the same diff is
a doc-drift defect, so the pull request either makes the documented invariant true or
narrows the sentence to name the exception. Making it true costs one guard clause per
crate plus two tests; narrowing it produces a worse artifact. Recorded here as the
decision made.

Both crates needed the mirror again, for the same reason the enum site did in round 2:
`pycc_mir`'s class-name method-call interception at `crates/pycc_mir/src/expr.rs` runs
before the base is lowered, so guarding `pycc_types` alone would leave MIR calling the
class's static method against a checker that had inferred the instance method's type.
The guard is placed *before* the method-table walk in `pycc_types`, matching the
`Subscript` path's ordering, so a shadowed name short-circuits rather than walking and
retracting.

The classmethod carrier was probed separately rather than assumed: `has_static_or_class_method`
covers both tables and they lower differently, so both shapes have their own test —
`an_active_binding_shadows_a_class_name_static_method_call` and
`an_active_binding_shadows_a_class_name_class_method_call`. Both doc statements that
enumerate the guard's reach were widened from three shapes to four. Commit `a66576ea`;
the harden pile carries a matching third round-2 row with that `fix_commit`.

## The fifth site: the monomorphizer's own walk (round 4)

The Codex reviewer then raised a P2 on a different mechanism. `rewrite_generic_calls_in_expr`
(`crates/pycc_types/src/monomorphize.rs`) walks every function body looking for generic calls to
rewrite, and its `AttrGet` and `MethodCall` arms recursed into the base unconditionally — so a
bare class name reached `infer_expr_in` as a value and failed with `T0021`. The pass runs only
when the module declares a PEP 695 generic function, which is why nothing before this round saw
it. Measured against the tree, with a declared-but-never-called `def ident[T](x: T) -> T`:

| program | CPython | pycc (before) |
| --- | --- | --- |
| `class A: X: int = 2` / `class B(A): pass` / `print(B.X)` | `2` | ``error[T0021]: name `B` is not defined`` |
| same with `print(A.X)` (own attribute) | `2` | ``error[T0021]: name `A` is not defined`` |
| `class A:` with `@staticmethod def m()` / `print(A.m())` | `2` | ``error[T0021]: name `A` is not defined`` |

The own-attribute probe is the one that settles provenance: it fails identically with no
inherited read anywhere in the program, so the defect is **pre-existing** and was not introduced
by #974's MRO walk. Codex's own framing — that the newly accepted inherited read still fails —
is therefore narrower than the truth, and the reply on the thread says so with the measurement.

**The D-127 fork was whether a pre-existing defect raised on this pull request belongs in it.**
Put to the advisor, as the fourth site was. The call: fix it here. "Pre-existing" is a reason to
defer only when deferring is available, and it is not — D-192's non-milestone open-issue ceiling
stands at 67 against 20, so no new issue may be filed, and the thread blocks merge through
required conversation resolution either way. Against that, the fix is one predicate already
present thirty lines above the defect, and the round-2 `docs/TYPE_SYSTEM.md` sentence enumerates
the sites the guard covers — leaving a fourth `pycc_types` site that dispatches on the same "is
this a class name" question out of it repeats the exact doc-drift defect the previous two rounds
were spent closing. Recorded here as the decision made.

The predicate was extracted into a shared `is_class_name_base` helper rather than copied a third
time, the same reasoning that produced `declares_name_outside_class_attrs` in round 2. The
`Slice` arm was probed rather than assumed: `A[1:2]` on a class name is rejected with `T0021` by
the checker itself, with or without a generic in the module, so no class-name base reaches that
arm in a program that compiles and it provably needs no branch.

Coverage needed one thing the CLI tests could not give. `rewrite_generic_calls_in_expr` has two
instantiation records — the compiler binary the integration tests drive, and the crate's own
unit-test build — and `llvm-cov` takes the per-function minimum across records, so the skip
branch showed as two missed lines even with every merged view at 100%. Two crate-internal tests
in `crates/pycc_types/src/tests.rs`, modelled on the existing
`class_getitem_dispatch_survives_the_generic_rewrite_pass`, close it. This is the same
per-instantiation trap the round-2 note above records, hit a second time in one task; the
diagnosis path (`--show-instantiations`) was already known and cost minutes rather than a round.

Commit `5b238d8c`; the harden pile carries a round-4 row with that `fix_commit`.

## The guard's own blind spot: module-scope rebinding (round 5)

A second Codex review of PR #992 raised a P1 against the round-2/3 guard itself: it consults
`binding_state`, and that answer means two different things depending on where it is asked.
`check_and_resolve_all_keyed`'s pass 2 walks the module's top-level statements sequentially, so
at module scope the answer is the binding active at that point in the source. Pass 3 then checks
every function body against the environment as it stands after **all** top-level code has run
(D-041 late binding), so inside a body the same answer is ordering-blind. `pycc_mir` lowers
function bodies the same way, after collecting every top-level binding.

The report was correct, and probing it produced a row the report did not have. Four programs,
measured against CPython 3.13.9:

| program | CPython | `origin/main` `7bc37e03` | branch before the fix | branch with the fix |
| --- | --- | --- | --- | --- |
| `A = D()` **after** `print(f())`, `f` returns `A.X` (own attribute) | `2` | exit 0, prints `2` | exit 101, no output | exit 1, `C0001` |
| `B = D()` after, `f` returns `B.X` (inherited, `class B(A)`) | `2` | exit 1, `T0044` | exit 101, no output | exit 1, `C0001` |
| `A = D()` **before** `print(f())` (own attribute) | `1` | exit 0, prints `2` | exit 0, prints `1` | exit 1, `C0001` |
| `B = D()` before (inherited) | `1` | exit 1, `T0044` | exit 0, prints `1` | exit 1, `C0001` |

The asymmetry is the finding. `main` answered `2` for both orderings and was wrong on the
rebind-before rows; the round-2 guard answered `1` for both and is wrong on the rebind-after
rows. One environment snapshot cannot serve two answers, so **no** resolution of the bare class
name is correct here, and the read is rejected with `C0001` instead. Resolving it properly needs
ordering-aware name resolution this compiler does not have, which is a new analysis and not this
pull request's scope. Recorded here as the decision made, per D-127; the fork was put to this
session's advisor and settled there.

The first attempt at that rejection was wrong, and the tree said so. It rejected the *rebinding
statement* in `module.rs` pass 2 — one module-walk check instead of edits at every dispatch site —
and the existing test `a_value_binding_shadowing_a_class_name_indexes_as_a_value` failed. That
test does not pin a bug: pass 2 resolves in source order, so **module scope is already sound**,
and a rejection there removes a capability the suite proves works. The defect lives only in
function bodies, and a fix aimed anywhere else is over-broad by construction.

What landed is narrow. `Environment::in_function_body`, set only by `child_for_function`,
distinguishes the two positions; the three `pycc_types` class-name dispatch sites (`Subscript`,
`AttrGet`, `MethodCall`) route the whole decision through one shared `expr::class_name_dispatch`
predicate that returns "the class", "a shadowing value", or the rejection — the same
extract-then-change-one-place move round 4 used for `is_class_name_base`. Module scope keeps
resolving in source order. A parameter or function-local shadow is untouched, because it is bound
at the call rather than by top-level code; that is pinned by a dedicated test *and* by the two
pre-existing subscript tests above, which stayed green without being edited and are the actual
evidence the rejection did not widen. `pycc_mir` needed no matching branch: the checker rejects
before MIR runs, confirmed by running all four programs through `pycc run`.

Commit `663cbfd7`; the harden pile carries a round-5 row with that `fix_commit`.

## Where to resume

- PR [#992](https://github.com/rotnov/pycc/pull/992) is open and carries `Fixes #974` (confirmed with the `closingIssuesReferences` query: `totalCount: 1`). It has been through the D-068 round, the Codex round, and the round-3 fourth-site fix above; the full local gate set was re-run from scratch after round 5 and is green at the branch head, coverage 100.00% lines and regions with zero missed.
- The D-014 gate was red for one round on a single region in `crates/pycc_mir/src/expr.rs` that every merged coverage view reported as covered. `llvm-cov`'s file summary takes `min(NotCovered)` across a function's instantiation records rather than merging them, so a region needs to be covered within one *single* instantiation. `cargo llvm-cov report -p pycc_mir --show-instantiations --html` is the view that names such a miss; `docs/AGENT_RETROSPECTIVE.md`'s 2026-09-07 entry records the full diagnosis. The fix was the extra `pycc_mir` unit test `an_inherited_class_attribute_read_through_the_derived_class_name_folds`, which walks past a derived class declaring nothing and folds the base's constant.
- `cargo test --workspace -- --include-ignored` fails 57 conformance tests **locally only**, all with the identical message ``conformance oracle must be exactly Python 3.14.7, found "Python 3.14.6"``. That is this machine's interpreter version, not the change: no other panic appears in the log. CI pins 3.14.7 and is the authority.
- **The ROADMAP byte budget was raised in this pull request, as that same note recommended.** `docs/ROADMAP.md` had 45 bytes of headroom against a 168960-byte per-document ceiling and the Codex round needed a sentence there, so `budget_bytes` for that entry in `site/llms-txt-context-manifest.json` is now 172032. The aggregate ceiling is untouched and keeps roughly 15 KB spare, so this trades a per-document limit that had become the binding constraint on three consecutive merges for headroom that already existed. Trimming unrelated prose a fourth time was the alternative and was rejected.
