---
id: D-199
title: "Track opaque bindings in the private-helper constraint solver (#771)"
status: accepted
---

## D-199: Track opaque bindings in the private-helper constraint solver (#771)

- Status: accepted
- Context: Issue #771 filed a misleading diagnostic: `d = cast(Derived, base); return d.b` inside a
  module the private-helper constraint solver (`infer_function_signatures_with_solver`) reaches
  reported `T0021` ("`d` is not bound before this use") instead of the real `C0001` down-cast
  rejection the equivalent inline form (`return cast(Derived, base).b`) already reported correctly.
  Root cause, traced to source: `check()` (`crates/pycc_types/src/lib.rs`) first tries a fast
  concrete-only path, which correctly computes the `C0001` rejection, but discards that result on
  `Err` and unconditionally falls back to `infer_function_signatures_with_solver` — a second,
  independent walk of the same module. That solver's `collect_block_constraints` `Assign` arm only
  inserts a binding into `env.bindings` when `collect_expr_constraints` returns `Some(term)`; the
  `Cast` arm (added for #767) deliberately returns `Ok(None)` for a non-scalar target, since the
  solver has no resolved `Ty` for the cast's argument to validate the cast against. `d` is therefore
  left in neither `env.bindings` nor `env.maybe_bindings` (the existing "assigned in only one
  branch" tracking, D-147). When `return d.b` is walked, `HirExpr::Name("d")` finds no binding but
  *is* a syntactic local, so it returns `Err(unbound_local("d"))` — `T0021` — which propagates
  straight out of `check()`, and the correct `C0001` computed by the first, discarded pass never
  surfaces.

  This is not cast-specific or novel: it is a pre-existing, already-documented architectural gap in
  the same solver, present since before D-146. `crates/pycc_hir/src/lib.rs`'s `ListPop`/
  `DictGetOrDefault` doc comments and `constraints.rs`'s `Subscript`/`AttrGet`/`MethodCall`/
  `TupleLiteral`/`DictLiteral`/`SetLiteral` arms all state that assigning one of these expressions
  to a name inside a solver-checked function registers no binding, and a later read then fails with
  the same misleading "not bound before this use". D-146 already fixed this exact symptom once, for
  `ListLiteral`/`Subscript`/`ListPop` specifically, via a "destructured carrier term" (a `Ty::List`
  term the solver can partially interpret). `isinstance`/`issubclass` do not exhibit this bug —
  their solver arm unconditionally returns a term. `cast`'s class-target case is simply the newest
  construct to expose the same pre-D-146-unfixed class, on the *result* side rather than the
  argument side D-146 fixed. Confirmed cosmetic, not a soundness gap: `T0021` and `C0001` are both
  `error`-tier and the program is rejected under either code.
- Decision: Add a second, purpose-distinct binding set to `ConstraintEnvironment`
  (`crates/pycc_types/src/constraints.rs`), `opaque_bindings: HashSet<String>`, fixing the general
  class rather than `cast` specifically (matching D-146's own precedent and directly serving #771's
  own suggestion to check whether other constructs share the symptom — they do). In
  `collect_block_constraints`'s `Assign` arm only, when `collect_expr_constraints` returns `Ok(None)`
  for an *unconditional* assignment target, `target` is inserted into `env.opaque_bindings` instead
  of being left untracked; a fresh assignment first clears any stale opaque marker for the same name
  (mirroring the existing `maybe_bindings.remove` at the same site), then either binds a real term or
  reinstates the opaque marker. The `AnnAssign`-with-`value` arm is deliberately *not* wired to insert
  into `opaque_bindings` — unlike `Assign`, it already unconditionally binds `target` into
  `env.bindings` from the annotation itself, regardless of the initializer term, so the insertion
  would be dead code at best and, given the lookup order below, a precision regression at worst; it
  only gets the same stale-marker cleanup for hygiene. In `HirExpr::Name`'s lookup, a name in
  `env.maybe_bindings` still short-circuits to `Ok(None)` first (unchanged, D-147 semantics); then a
  present `env.bindings` term always wins; only when *neither* holds a term does `env.opaque_bindings`
  membership return `Ok(None)` instead of falling through to `unbound_local` — so a real term always
  takes priority over a stale opaque marker for the same name. `opaque_bindings` is kept semantically
  separate from `maybe_bindings` (not reused or folded in) and mirrors `maybe_bindings`'s own
  lifecycle at every rebind, scope-clear, and environment-construction/clone site, *and* at the
  solver's branch/loop join sites (`crates/pycc_types/src/solver.rs`'s `join_if_branches_solver` and
  `join_loop_body_solver`): both helpers merge each branch's `opaque_bindings` alongside its
  `bindings`, so a name opaquely assigned in only one branch of an `if`/`else`, in a loop body, in a
  `match` arm, or in a `try`/`except` handler is folded into the post-join `maybe_bindings` exactly
  like a real-term binding introduced there, and a name opaquely assigned in *both* branches of an
  `if`/`else` is folded into the post-join `opaque_bindings` as definitely bound. This closes a gap an
  earlier draft of this fix left open: without the join-site changes, an opaquely-assigned name was
  invisible to `env.bindings`, so the join loops (which only walked `body_env.bindings`/
  `orelse_env.bindings`) silently dropped it from every post-join set, including a name assigned
  unconditionally in *both* branches — reproducing the exact misdiagnosis this decision exists to fix
  for a case that is not "branch-conditional" in the definite-assignment sense at all.

  A second reviewer pass over the join-site mirroring above found a further, more serious gap: every
  `pre_existing` snapshot the six `collect_block_constraints` call sites compute (`If`, `While`,
  `ForRange`, `ForList`, per-`case` `Match`, and the shared `Try` snapshot) was `env.bindings.keys()`
  alone, blind to `env.opaque_bindings`. A name already opaquely bound *before* the construct (e.g.
  `y = d.get("a", 0)` two lines above an `if`) was therefore invisible to `pre_existing`, so a branch
  that reassigned it to a real, solver-representable term looked "newly introduced" by that branch;
  when only one branch performed the reassignment, the untouched branch's still-opaque entry for the
  same name *also* looked "newly introduced" (relative to the opaque-blind snapshot), so both branches
  appeared to introduce the name and it was wrongly classified as *definitely* bound with the
  one-branch-only term — silently discarding what the other branch's actual (opaque) value was. This
  is fixed at the source (`crates/pycc_types/src/constraints.rs`'s six `pre_existing` computations now
  chain in `env.opaque_bindings.iter()`), but that alone was not sufficient: `join_if_branches_solver`'s
  two `env.bindings` merge loops (one per branch) are unconditional `entry().or_insert()` calls that
  do not consult `pre_existing` at all. A pre-existing *real* binding is naturally protected because
  `env.bindings` already holds an entry for it, so `or_insert` is a no-op; a pre-existing *opaque-only*
  binding has no such protection, since it does not occupy a `bindings` slot, so the unconditional
  merge would still write the one branch's term into `env.bindings` even after the `pre_existing` fix
  — the name would correctly stay out of `maybe_bindings`, but `HirExpr::Name`'s lookup checks
  `maybe_bindings` first and `bindings` second, so an unmasked term there is read as if it applied on
  every path. `join_loop_body_solver` did not need the equivalent fix: its own real-binding merge loop
  was already conditioned on `!pre_existing.contains(name)`, so the `pre_existing` fix alone closes the
  same gap there.

  A third reviewer pass over the second pass's fix (`pre_existing.contains(name) &&
  !env.bindings.contains_key(name)` as the skip condition in both of `join_if_branches_solver`'s merge
  loops) found it too broad: it also fires when *both* branches reassign the same pre-existing-opaque
  name to a real term (`y = d.get(...)` then `if cond: y = 1 else: y = 2`), because neither loop has
  inserted into `env.bindings` yet when the other runs — dropping the name from `env.bindings`,
  `env.opaque_bindings` (neither branch's cleared opaque marker survives a real-term reassignment), and
  `maybe_bindings` (both `pre_existing`-filtered) all at once, reproducing the exact `unbound_local`
  misdiagnosis for a name that is not branch-conditional at all: every path assigns it. The guard now
  consults the *other* branch's own `bindings` map instead of `env.bindings`'s own mutation state —
  `pre_existing.contains(name) && !orelse_env.bindings.contains_key(name)` in the body loop, and the
  symmetric check against `body_env.bindings` in the orelse loop — so it only skips when exactly one
  branch supplies a real term for a pre-existing-opaque name; when both branches supply one, neither
  loop's guard fires and the merge proceeds via the ordinary first-wins `entry().or_insert()`, identical
  to how a name genuinely new to both branches is handled.
- Alternatives: Reuse `maybe_bindings` directly instead of adding a new field (rejected —
  `maybe_bindings` has a specific, D-147-tied meaning inspected by the if/loop branch-merge logic
  at two other sites; polluting the same set with "definitely assigned, but the solver has no term
  for it" entries risks changing that merge behavior in ways not fully traced here). Short-circuit
  `check()`/`checked_function_signatures()` to return the concrete path's `Err` directly instead of
  falling back to the solver (rejected — both functions carry an explicit comment preserving the
  historical solver-first diagnostic selection for modules with multiple errors, so short-circuiting
  changes that selection more broadly than this issue calls for; it also does not fix the general
  class, since a module that legitimately reaches the solver path for an unrelated unannotated
  helper would still hit the same misfire for `.pop()`, `AttrGet`, `MethodCall`, etc.). Give `cast`'s
  solver arm a real "carrier" term, mirroring D-146's `Ty::List` carrier (rejected — D-146's carrier
  works because a list's element type is still derivable without full class resolution; a
  `cast(Derived, base)` target's validity depends on `check_cast`'s representation/layout/dispatch
  analysis, which needs a resolved `Ty` the solver does not have by design; the same is true for
  `AttrGet`/`MethodCall`, so a carrier only works for the D-146 subset, not the general class).
- Consequences: `crates/pycc_types/src/constraints.rs` gains the `opaque_bindings` field (mirrored
  at its two production construction sites and every `maybe_bindings` rebind/scope-clear site), and
  its six `pre_existing` snapshots now chain in `env.opaque_bindings.iter()` as described above.
  `crates/pycc_types/src/solver.rs` gains the join-site mirroring described above, plus the
  other-branch-aware guard in `join_if_branches_solver`'s two real-binding merge loops. Three further
  regression tests pin the composed pre-existing-opaque-then-branch-reassignment scenarios directly:
  `solver_if_reassigns_pre_existing_opaque_binding_in_one_branch_only` and its orelse-branch mirror
  (`..._in_orelse_branch_only`) assert that a name opaquely bound before an `if`, reassigned to a real
  term in exactly one branch, does not become newly *maybe* bound (it was already definite) and remains
  resolvable only through `opaque_bindings` afterward, not through the one branch's unmasked term;
  `..._in_both_branches_to_real_terms` asserts the complementary case — reassigned in *both* branches —
  ends up definitely bound with a real term rather than dropped from every tracked set.
  `crates/pycc_types/src/tests.rs`'s 78 direct
  `ConstraintEnvironment { ... }` construction sites each gain `opaque_bindings: HashSet::new()`.
  `crates/pycc_hir/src/lib.rs`'s `ListPop`/`DictGetOrDefault` doc comments and the shared `AttrGet`/
  `MethodCall` arm comment in `constraints.rs` are updated to strike only the "later read fails with
  a misleading diagnostic" clause — the "this construct contributes no solver term" clause is
  unchanged and remains true; that underlying gap is not closed, only its misleading consequence is.
  `Subscript`/`TupleLiteral`/`DictLiteral`/`SetLiteral`'s own arm comments in `constraints.rs` do not
  restate that clause independently (they only cross-reference the reasoning above them) and were not
  separately edited. The pre-existing pinned regression test
  `dict_get_or_default_assigned_inside_an_unannotated_private_helper_...` (renamed to drop its now-
  inaccurate "hits the pre-existing solver binding gap today" suffix) is updated in place: it still
  reports `T0021`, but now with the genuine "cannot infer return type of private helper `_h`; add an
  annotation" message instead of the misleading "not bound before this use" one. A new test pins the
  #771 repro itself (`d = cast(Derived, base); return d.b` now reports `C0001`), and a new positive-
  path test confirms an unconditional assignment from an opaque expression whose value is never read
  still compiles cleanly. Every construct in the affected-site inventory (`Cast`, `Subscript`,
  `ListPop`, `AttrGet`, `MethodCall`, `DictGetOrDefault`, `SetAdd`, `TupleLiteral`, `DictLiteral`,
  `SetLiteral`, non-scalar `ListLiteral`) is fixed by this single shared `HirExpr::Name` change, since
  the bug's mechanism is identical in every case. Branch-conditional variants (e.g.
  `if cond: d = cast(...)` read later, whether `d` is assigned in one branch or both, in a loop body,
  in a `match` arm, or in a `try`/`except` handler) are covered too, via the `solver.rs` join-site
  mirroring described above — they are not deferred. What remains out of scope is only what D-147's
  own `maybe_bindings` mechanism itself does not attempt: an opaque binding that is *definitely*
  assigned in one branch but the other branch of the same `if` returns, raises, or otherwise diverges
  (D-147-style narrowing of "maybe" to "definite" from control-flow-terminal branches) is tracked no
  more precisely here than the equivalent real-term case already is — this decision does not change
  that pre-existing precision boundary, it only extends `opaque_bindings` to track everywhere
  `maybe_bindings` already does. This extends D-146's own precedent (a construct the solver can't fully model still
  shouldn't cause a spurious `unbound_local`) via a general mechanism instead of a per-construct
  carrier term; it references, and does not supersede, D-146. It also references the existing
  `maybe_bindings`/`BindingState::Maybe` definite-assignment tracking the surrounding source comments
  attribute to D-147 (issue #118, `T0041`), whose semantics `opaque_bindings` is deliberately kept
  distinct from.
