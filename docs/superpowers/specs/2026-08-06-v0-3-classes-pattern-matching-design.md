# v0.3 "classes & pattern matching" — design

> Brainstormed in autopilot mode per the standing `/goal` directive, with the
> `advisor` tool as dialogue partner in place of interactive user Q&A (the
> user is unavailable mid-autopilot and explicitly delegated design-fork
> decisions, per [D-127](../../decisions/D-127-autonomous-agent-operation-model.md)).
> Each approval gate the `brainstorming` skill normally asks the user for is
> instead recorded here with its rationale, the way this repository's own
> ADRs already work — this doc **is** the audit trail. Written by #374's own
> implementation (a documentation/planning deliverable, no code), against
> [#374's published, adversarially-reviewed decomposition plan](https://github.com/rotnov/pycc/issues/374#issuecomment-5204010514).

**Goal:** Ship v0.3 ("classes & pattern matching") per `docs/ROADMAP.md`:
classes with single/multiple inheritance and C3 MRO, `@property`,
`classmethod`/`staticmethod`, dataclasses, enums, protocols (structural
typing), `match` with exhaustiveness checking, and `try`/`except`/`finally`
exception handling — plus the diagnostics registry growth (`T0030`/`T0031`)
that comes with `match` and `@override` landing. Acceptance criteria as
corrected by [D-153](../../decisions/D-153-correct-v0-3-s-conformance-target-before-any-v0.md)
(this file lives under `docs/superpowers/specs/`, two directories below
`docs/` — not one, mirroring v0.2's own design doc).

**Architecture:** Same "thin slice first, then breadth" strategy v0.1/v0.2
used (`docs/DELIVERY_PLAN.md`'s "v0.1 execution strategy" and "v0.2 execution
strategy"), unchanged in its basic shape: `pycc_hir` needs a real
class-definition representation and a class-instance `Ty` variant before any
of PR-16 through PR-22 have anything to build on, so PR-15 (class model
foundation) is the mandatory first slice, mirroring PR-10's role for v0.2's
own container-generics track. Two things are new relative to v0.1/v0.2's
single-track sequencing, both recorded in `docs/DELIVERY_PLAN.md`'s new "v0.3
execution strategy" section rather than repeated here: (1) v0.3 has real
*parallel* tracks once PR-15 lands — `match`'s non-class-pattern work
(PR-21) and the breadth PEP sweep (PR-23) do not depend on PR-16 through
PR-20 at all — and (2) two decisions this milestone's own scope depends on
(D-005, native exception unwinding; D-006, vtable dispatch scope) were
unexamined `proposed` stubs going in, and this doc's own §2/§3 record why
both stay `proposed` rather than being flipped prematurely.

**Tech stack:** No new external dependencies. Class-instance representation
follows the existing heap-object-pointer codegen pattern already used for
`list`/`dict`/`set` in `crates/pycc_codegen/src/lib.rs` (not the by-value
`tuple` struct pattern — TYPE_SYSTEM.md's own "Class model" section already
specifies "struct; fields fixed at compile time"). `match` reuses the
existing control-flow-join machinery #118 already built for `check_stmt`
(see Context below). Exception unwinding's actual mechanism is explicitly
**not decided by this doc** — see §2.

## Global Constraints

- D-014: 100% line/region coverage is a hard merge gate for every crate,
  every PR, no exceptions without a documented `docs/TESTING.md` exemption.
- D-021: every PR/task starts from a freshly fetched `origin/main`, in its
  own branch/worktree; never merge/rebase over uncommitted work.
- D-068: the pinned `ievo:deep-reviewer` reviews every significant diff
  before merge; actionable findings get fixed and re-reviewed.
- Every behavior change ships with its documentation update in the same
  commit (AGENTS.md's "Keep documentation current").
- `docs/ROADMAP.md`'s acceptance criteria are binary, not aspirational — a
  gate that can't be measured isn't a gate (D-088 fixed this once for v0.2,
  D-153 fixes the analogous problem for v0.3; don't reintroduce it a third
  time).
- Spec-is-law autonomy policy (`docs/DELIVERY_PLAN.md`): where this doc is
  silent, the implementer picks the most conservative *actually available*
  option and records it as a new ADR — does not stop to ask.
- AGENTS.md's decompose-large-issues rule: any of PR-15..23 whose own
  completion criteria turn out to span multiple independent code seams once
  its implementer starts real design work must be split into
  dependency-ordered sub-issues before implementation, exactly as this
  file's own drafting split #374 into PR-15..23 in the first place.

---

## Context already verified (don't re-derive)

- **`docs/TYPE_SYSTEM.md`'s "Class model (compiled subset)" section**
  (lines 130-132) is the authoritative, already-written spec for what v0.3
  implements: single + multiple inheritance with C3 MRO resolved at compile
  time, `@property`, `classmethod`/`staticmethod`, `__init_subclass__`/
  `__set_name__` executed at compile time when statically evaluable,
  dataclasses (557) and `dataclass_transform` (681), `@override` (698)
  enforced, dunder protocol methods → static dispatch. Its "Rejected" list
  separately names `E0102`/`E0105`-guarded dynamic patterns v0.3 does
  **not** support.
- **`crates/pycc_hir/src/lib.rs`'s `HirItem` has exactly two variants,
  `Function` and `TopLevelStmt`** (lines 421-430) — no class variant exists
  yet, and `lower_stmt` has no `Stmt::ClassDef` arm; a class statement
  currently falls through to the generic `C0001` "statement kind not
  supported yet" path, the same clean-rejection pattern D-088 verified for
  `import` before v0.2's PR-14 built real import support.
- **The diagnostics registry mapping is narrower than an earlier draft of
  #374's plan assumed.** Verified directly in `crates/pycc_diag/src/explain.rs`:
  only `T0030` ("non-exhaustive `match`") and `T0031` ("`@override` without
  a matching base method") are genuine v0.3-shipped-feature diagnostics —
  both are pre-reserved with plain "not currently emitted because pycc has
  no [match/class] support yet" text and no further qualifier.
  `E0101`/`E0102`/`E0103`/`E0105` are each explicitly reserved as *"part of
  the planned v0.7 dynamic-Python rejection surface"* and are **not** v0.3
  deliverables; PR-15/PR-16 do not implement them.
- **PR #358's execution-order mechanism does not generalize to class
  bodies.** PR #358 ("Fix #22") establishes a global, name-keyed
  LLVM-function-pointer slot scheme for module-level `def`
  execution-order/redefinition binding. That mechanism is specifically a
  *global, flat* namespace scheme — a per-class namespace (where two
  different classes may share a method name without colliding) needs its
  own analogous-but-distinct scheme. PR-15 owns designing that scheme,
  reusing the *pattern* PR #358 establishes (source-order registration,
  redefinition-is-rebind), not its literal mechanism.
- **The `match`-exhaustiveness ↔ #359 dependency in #374's original body is
  not supported by #359's own text.** #359 ("Part 2 of #118") never
  mentions "match," "pattern," or "exhaustive" anywhere; its actual scope is
  extending definite-assignment join-tracking into the *separate* private-
  helper return-type constraint solver. #118 (merged) already covers
  `check_stmt`'s control-flow joins, and `match` arms are joins like
  `if`/`for` — so non-class-pattern `match` gets correct definite-assignment
  tracking for free once match lowering exists, without needing #359. #359
  only matters for the narrower case of a `match` arm's binding *inside a
  private helper function* whose return type the solver infers — recorded
  as recommended-early, not blocking, for PR-21.
- **`crates/pycc_ast` already parses `match` syntactically** via
  `ruff_python_ast::Stmt::Match`, but `pycc_ast`'s own facade (`lib.rs`)
  does not re-export `StmtMatch`/pattern types, and `lower_stmt` has no arm
  for `Stmt::Match` — non-class-pattern `match` support is greenfield
  regardless of class-model status (see §1's PR-21 row).
- **D-005 and D-006 are both title-only, unexamined `proposed` stubs.**
  Verified by reading both files directly: neither has rationale,
  alternatives, or consequences content beyond a two-line frontmatter-plus-
  title stub. `docs/TYPE_SYSTEM.md`'s own "Class model" section already
  states ordinary inheritance/C3 dispatch is *"resolved at compile time"*
  and dunder protocol methods use *"static dispatch"* — i.e. the standing
  design intent for ordinary classes (PR-16) is static dispatch, and D-006's
  own title scopes the vtable question specifically to explicit
  dynamic-`Protocol` use (PR-20), not ordinary inheritance.
- **No `--opt-size` flag exists in this compiler today** (verified: no
  match on `opt-size`/`opt_size` anywhere under `crates/`, and
  `docs/CLI_SPEC.md` does not define it) — relevant to §3's D-006 discussion,
  since D-006's own title has a second clause ("...and `--opt-size` cold
  code") that names a flag that does not exist yet.

## Design decisions this doc adds

### 1. The v0.3 PEP→fixture→owning-PR table (resolves the conformance gap)

D-153 did the itemized feasibility pass and revised v0.3's accept bullet
from an unverified "≥45 PEPs" down to **≥37 `PYTHON_STANDARDS.md` rows,
encompassing 39 distinct PEP numbers** — see that decision for the full
per-PEP reasoning (which candidate PEPs are genuinely reachable without a
new subsystem, which were deliberately not attempted to avoid repeating a
mistake this project's own history already logged twice, and which need a
prerequisite subsystem v0.3 does not build). This table assigns every
reachable row an owning PR and fixture path, the same shape v0.2's design
doc §2 built for its own conformance target:

| PEP | Feature | Fixture | Owning PR | Why reachable there |
|---|---|---|---|---|
| 3135 | Zero-argument `super()` | `py30/pep_3135_super.py` | PR-16 | Needs C3 MRO + method-override resolution |
| 3119 | ABCs, `isinstance` hooks | `py30/pep_3119_abc.py` | PR-16 | `isinstance`/`issubclass` against the real class hierarchy |
| 3129 | Class decorators | `py30/pep_3129_class_deco.py` | PR-18 | Dataclasses' own `@dataclass` decorator is the first PR-15..23 feature that needs class-decorator syntax to work at all; general function-decorator syntax stays unsupported (`lower_function` still rejects `decorator_list`) until a PR actually needs it |
| 409 | `raise ... from None` | `py33/pep_0409_from_none.py` | PR-22 | Part of PR-22's exception-chaining scope |
| 3151 | `OSError` hierarchy | `py33/pep_3151_oserror.py` | PR-22 | Part of PR-22's custom-exception-class scope |
| 435 | `enum` | `py34/pep_0435_enum.py` | PR-19 | v0.3's own enum row |
| 487 | `__init_subclass__`, `__set_name__` | `py36/pep_0487_init_subclass.py` | PR-16 | Explicitly named in TYPE_SYSTEM.md's Class model section as PR-16 scope |
| 557 | dataclasses | `py37/pep_0557_dataclasses.py` | PR-18 | v0.3's own dataclasses row |
| 560 | `__class_getitem__` | `py37/pep_0560_class_getitem.py` | PR-16 | A dunder-dispatch mechanism, same family as `__init_subclass__`/`__set_name__`; reuses PR-16's method-resolution machinery |
| 544 | `Protocol` — structural typing | `py38/pep_0544_protocol.py` | PR-20 | v0.3's own protocols row |
| 570 | Positional-only params `/` | `py38/pep_0570_pos_only.py` | PR-23 | D-153: reachable now — `parameters.posonlyargs` already parsed, keyword call args are already globally unsupported, so positional-only is already the *only* calling convention this compiler has |
| 591 | `Final` | `py38/pep_0591_final.py` | PR-23 | D-153: bounded `annotation_to_ty` unwrap plus a new reassignment diagnostic |
| 593 | `Annotated` | `py39/pep_0593_annotated.py` | PR-23 | D-153: bounded `annotation_to_ty` unwrap — PEP 593's own spec says an unrecognizing checker must treat `Annotated[X, ...]` as `X`, so unwrapping is correct behavior, not a shortcut |
| 634–636 | Structural pattern matching (`match`) | `py310/pep_0634_match.py` | PR-21 | v0.3's own headline `match` row; class patterns need PR-15, non-class patterns do not |
| 654 | `except*` / `ExceptionGroup` | `py311/pep_0654_except_star.py` | PR-22 | Part of PR-22's exception scope |
| 673 | `Self` | `py311/pep_0673_self.py` | PR-15 | A method return-type annotation meaningful once any class model exists, even without inheritance |
| 681 | `dataclass_transform` | `py311/pep_0681_dc_transform.py` | PR-18 | Explicitly named alongside 557 in TYPE_SYSTEM.md's Class model section |
| 695 (generic classes) | `class C[T]` | `py312/pep_0695_generic_classes.py` | PR-15 | Reuses PR-13's existing `Ty::Param` call-site-substitution mechanism (D-133/D-134), scoped the same way PR-13 scoped generic functions: exactly one type parameter, scalar-only instantiation — confirm this scoping choice empirically during PR-15, don't assume it transfers cleanly from functions to classes |
| 698 | `@override` | `py312/pep_0698_override.py` | PR-16 | `T0031` diagnostic, pre-reserved, becomes real once PR-16 lands |
| 649/749 | Deferred annotations (self-referential case) | `py314/pep_0649_deferred_ann.py` | PR-15 | v0.2's design doc's own §2 update flagged the only realistic reachable case as a self-referential forward reference inside a class body (`class Node: def next(self) -> Node: ...`) — needs a class model, which PR-15 provides |
| 758 | `except A, B:` without parens | `py314/pep_0758_except_noparens.py` | PR-22 | Part of PR-22's exception scope |
| 765 | No `return`/`break`/`continue` in `finally` | `py314/pep_0765_finally.py` | PR-22 | Part of PR-22's `finally`-semantics scope |

**Not attempted in v0.3** (D-153's own itemization — repeated here only as a
pointer, full reasoning lives in that decision): PEP 3102 (keyword-only
params — needs keyword call arguments, unimplemented), 3104 (`nonlocal` —
needs nested function definitions, unimplemented), 3132 (extended unpacking
— needs destructuring-assignment targets, unimplemented), 448 (unpacking
generalizations — needs starred call/literal handling, unimplemented), 604
(union syntax — needs a new `Ty::Union` variant and real union
type-checking), 589 (`TypedDict` — needs a new structural per-key-typed dict
representation), 586 (`Literal` — deliberately not attempted to avoid a
fourth repeat of the "accept syntax, drop semantics" mistake D-088 and the
v0.2 design doc already logged three times), 572 (walrus `:=` — needs a new
side-effecting `MirExpr` form, which does not exist).

Row total: 15 (already checked) + 19 (v0.3-implied, PR-15/16/18/19/20/21/22
above) + 3 (PR-23) = **37 rows**, encompassing 14 + 22 + 3 = **39 distinct
PEP numbers** — matches D-153's revised target exactly, with zero margin
(the same "zero margin, every remaining row must land" posture D-088's own
v0.2 target ended up with).

### 2. D-005 (native exception unwinding): deferred, not resolved

**Decision: leave D-005 `status: proposed`.** This is a deliberate,
reasoned deferral per #374's own plan (which explicitly allows either
resolving now or deferring with recorded reasoning), confirmed via the
`advisor` tool per D-127's judgment-call process.

D-005's title commits to "native unwinding (Itanium/SEH), zero-cost happy
path — not result-codes." Verifying that commitment for real, before
flipping `status: accepted`, requires answering questions this doc cannot
answer from static inspection alone:

- Itanium C++ ABI unwinding covers `x86_64-unknown-linux-gnu`,
  `aarch64-unknown-linux-gnu`, `x86_64-apple-darwin`, and
  `aarch64-apple-darwin` (4 of pycc's 5 Tier-1 targets); `x86_64-pc-windows-msvc`
  needs Windows SEH instead — a genuinely different unwind-table format and
  personality-routine ABI, not a portability detail that can be papered
  over.
- pycc's generated LLVM IR unwinds through code that also links against
  `pycc_rt`, a Rust crate. Whether LLVM `invoke`/`landingpad` unwinding
  interacts safely with Rust's own panic/unwind machinery inside the same
  process (or whether `pycc_rt` needs `-C panic=abort` plus a fully separate
  unwind path) is an empirical question, not a design-review one.
- The "zero-cost happy path" claim — no runtime overhead when no exception
  is thrown, cost paid only on the unwind path — needs to be measured against
  a real generated-code sample on each of the 5 Tier-1 targets before it can
  be asserted as a real property of this compiler's output, not just of the
  Itanium/SEH ABIs in the abstract.

None of these are answerable by reading source and docs the way §1's PEP
feasibility pass was; they need actual cross-platform spikes (a minimal
`try`/`except` fixture compiled and unwound on all 5 targets) that are more
naturally PR-22's own first task than a documentation-only issue's. Flipping
`status: accepted` now, without that verification, would misrepresent
confidence the same way an unverified "45 PEPs" bullet misrepresented
reachability (D-153's own root cause) — this doc deliberately does not
repeat that pattern for an ADR instead of a roadmap bullet.

**Consequence:** PR-22's sub-issue body (§4's issue list) states D-005
resolution as its first implementation task, flagged as v0.3's single
highest technical-risk item — matching #374's plan's own "Risks" section and
its recommendation to start D-005's cross-platform research early, in
parallel with PR-15..21, rather than deferred to after 7 other PRs land.

### 3. D-006 (vtable dispatch scope): deferred, not resolved

**Decision: leave D-006 `status: proposed`.** Also confirmed via `advisor`
per D-127.

D-006's title has two clauses: "vtable dispatch only for explicit
dynamic-Protocol use **and `--opt-size` cold code**." The first clause is
already well-grounded in this project's existing, already-written design
intent — `docs/TYPE_SYSTEM.md` line 101 independently states *"`Protocol` —
structural — static dispatch via monomorphization; vtable only for explicit
`dyn`-like use,"* so accepting just that clause would mostly be formalizing
a decision this project has effectively already made elsewhere. The second
clause names a flag, `--opt-size`, that **does not exist in this compiler
today** (verified: no match anywhere under `crates/`, not in
`docs/CLI_SPEC.md`) — flipping `accepted` on a clause describing a
non-existent flag's behavior would accept a decision about something that
isn't real yet.

A clean partial-accept (accept the Protocol clause now, leave the
`--opt-size` clause open) was considered and rejected: #374's own plan
(Correction 4, Risk 4) explicitly warns that PR-16's own open
runtime-polymorphism question — does v0.3 support a base-typed reference
holding a subtype instance, e.g. a `list[Animal]` mixing `Dog`/`Cat`
instances — feeds directly into whatever D-006 ultimately decides, and that
resolving the two independently risks v0.3 shipping two incompatible
dispatch designs. D-006 is sequenced as "primarily PR-20's concern" in the
plan specifically so PR-16's answer is available first. Flipping any part of
D-006 to `accepted` before PR-16 lands would be exactly the fragmentation
risk the plan already flagged, not a hedge against it.

**Consequence:** PR-20's sub-issue body states "resolve or supersede D-006
before implementation" as the plan's Correction 4 already specifies. PR-16's
sub-issue body poses the runtime-polymorphism question as an open design
question for that PR to answer (not as something D-006 already settles),
consistent with Correction 4's own framing.

### 4. Explicitly flagged, not resolved by this doc

- **Class instance layout specifics** (field ordering, alignment, `__slots__`-
  implicit representation details beyond TYPE_SYSTEM.md's existing
  high-level "struct; fields fixed at compile time" statement) — PR-15's own
  new ADR, mirroring D-089's role for the recursive `Ty` in v0.2.
- **Class-body execution-order mechanism specifics** (the per-class
  namespace scheme reusing PR #358's pattern, not its mechanism, per Context
  above) — PR-15's own design work.
- **`match` exhaustiveness-checking algorithm choice** (decision-tree
  compilation vs. a simpler per-arm coverage check sufficient for the
  literal/capture/wildcard/sequence/mapping/or-pattern/class-pattern set
  v0.3 targets) — PR-21's own design work; T0030's existing reserved text
  does not commit to an algorithm.
- **PR-16's runtime-polymorphism open question** (base-typed reference
  holding a subtype instance) — see §3; PR-16's own design work, feeding
  into PR-20's D-006 resolution.

## PR breakdown

(Full rationale — including the hard-dependency vs. recommended-sequence
distinction, mirroring v0.2's own "Two corrections to v0.1's ordering
pattern" subsection — lives in `docs/DELIVERY_PLAN.md`'s new "v0.3 execution
strategy" section, committed alongside this doc.)

1. **PR-15**: Class model foundation (thin vertical slice) — new `Ty`
   class-instance variant, new HIR class-definition representation, a single
   class with instance attributes/methods/attribute access, class-body
   execution order, `Self` (PEP 673) and the self-referential deferred-
   annotation case (649/749), and a scoped, scalar-only single-type-parameter
   slice of generic classes (695).
2. **PR-16**: Inheritance, C3 linearization, `super()` (3135), `isinstance`/
   `issubclass` (3119), `__init_subclass__`/`__set_name__` (487),
   `classmethod`/`staticmethod`, `__class_getitem__` (560), `@override`
   (698, `T0031`). Poses the runtime-polymorphism open question (§3/§4).
3. **PR-17**: `@property`.
4. **PR-18**: Dataclasses (557, 681), which brings class decorators (3129)
   along as their first real vehicle.
5. **PR-19**: Enums (435).
6. **PR-20**: Protocols and structural typing (544). Resolves or supersedes
   D-006 (§3).
7. **PR-21**: `match` statement with exhaustiveness checking (634–636,
   `T0030`). Non-class patterns are independent of PR-15; class patterns
   need it.
8. **PR-22**: Exceptions — `try`/`except`/`finally`, chains (409), `except*`
   (654), custom exception classes, `OSError` hierarchy (3151), `finally`
   semantics (765), `except A, B:` (758). Resolves or supersedes D-005 as
   its first task (§2) — the milestone's single highest-risk item.
9. **PR-23**: Breadth PEP sweep (570, 591, 593) closing D-153's revised
   conformance target. Independent of PR-15..22; can run at any point in
   parallel.

## Testing

Every PR above follows this project's existing TDD convention (failing test
first, D-014's 100% coverage, `docs/AGENT_TOOLING.md`'s pinned reviewer
before merge) — no new testing philosophy introduced. Each PR adds its own
fixture(s) to the existing `tests/conformance.rs` harness (D-102's PR-9
precedent) as its feature lands, so the ≥37-row count is verified
incrementally, the same discipline v0.2 used for its own ≥15 target.

## Error handling

No new error-handling philosophy: unsupported valid Python still gets a
spanned `C0001`, malformed input still gets `L0001`, and the type/binding
diagnostics families (`T00xx`) grow exactly as many new codes as new
rejection cases (e.g., "`match` arm is unreachable," "class does not define
`__init_subclass__`'s required signature," PEP 591's new reassignment-of-
`Final` code) — each with its own snapshot test, matching
`docs/DIAGNOSTICS.md`'s existing quality bar. `T0030`/`T0031` are the only
two diagnostics counted against v0.3's own "diagnostics registry fully
implemented for shipped features" accept bullet (Context above); every other
new `T0xxx`/`C0001` occurrence is ordinary incremental diagnostics work, not
a milestone-gate deliverable in its own right.
