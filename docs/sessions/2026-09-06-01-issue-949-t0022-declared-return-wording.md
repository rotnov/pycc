# 2026-09-06 — #949: T0022 on a declared return names the annotation instead of a "private helper"

## Previous checkpoint's outcome

Iteration 15 delivered [#953](https://github.com/rotnov/pycc/issues/953) (a
conforming concrete argument is accepted for a protocol-typed method
parameter): PR [#955](https://github.com/rotnov/pycc/pull/955) merged by
squash as `923f9886c2b4084256cd09e104fa384cd741ca58` and #953 is CLOSED. CI
on that pull request was green on the first try. One follow-up was filed
from that iteration's own measurements:
[#954](https://github.com/rotnov/pycc/issues/954) (protocol-typed parameters
on static methods, class methods, `super()`-forwarded methods, constructors
and exception constructors).

Post-merge `main` runs for `923f9886` were all `completed`/`success`, each
re-queried immediately before this snapshot was committed: CI
[34001147750](https://github.com/rotnov/pycc/actions/runs/34001147750), Main
history audit
[34001147705](https://github.com/rotnov/pycc/actions/runs/34001147705),
Status page freshness
[34001147699](https://github.com/rotnov/pycc/actions/runs/34001147699), and
Pages [34001147728](https://github.com/rotnov/pycc/actions/runs/34001147728).
The earlier CI run [33996635017](https://github.com/rotnov/pycc/actions/runs/33996635017)
on `0084258b` had also concluded `success`.

Selection note: the advisor round endorsed #949 as the smallest unmarked
`v0.4` issue, with a framing correction attached — the discriminator is
annotated-vs-inferred, not public-vs-private, and protocols are irrelevant to
it. Runners-up were [#908](https://github.com/rotnov/pycc/issues/908) (enum
equality, which needs MIR/codegen identity lowering and is a much larger
seam), [#944](https://github.com/rotnov/pycc/issues/944) and
[#954](https://github.com/rotnov/pycc/issues/954).

## Overall status

Implemented #949 on `autopilot/iter-2026-09-06-16`, cut from `923f9886`. One
pull request carrying `Fixes #949`; the orchestrating session watches CI and
merges.

The issue and the open-PR list were re-checked before the first edit, at the
first commit, and before the push: state `OPEN` throughout, no open pull
request referencing 949, and the only comment on the issue is this session's
own plan comment. The plan is the `issue-to-plan` comment on #949
([issuecomment-5555909075](https://github.com/rotnov/pycc/issues/949#issuecomment-5555909075),
published against `923f9886` after two adversarial review rounds); this
snapshot records where the implementation followed it and where it deviated.

## What the change is

`def f(p: P) -> int: return p` reported
``T0022: private helper return type: conflicting inferred types `int` and `P` ``
even though `f` is public and its return type is *written*, not inferred.
Both properties the issue title leans on turned out to be red herrings, and
both were refuted empirically before any code was touched:

- **Protocols are irrelevant.** The same wording appears with a plain class
  (`class C: ...` / `def f(p: C) -> int: return p` →
  ``conflicting inferred types `int` and `C` ``).
- **Public-vs-private is not the discriminator.**
  `infer_function_signatures_with_solver_all`
  (`crates/pycc_types/src/constraints/signatures.rs`) walks *every*
  module-level function, not only D-038 private helpers, so the solver's
  `HirStmt::Return` arm ran for annotated public functions too and handed
  `unify_terms` the hard-coded context string `"private helper return type"`
  regardless.

The real discriminator is the return *term*: `Ok(ty)` is a written
annotation, `Err(var)` an inference variable standing in for an unannotated
helper's return type. Only the latter is genuinely an inference conflict. So
the fix is message *selection*, not re-routing the check to another pass.

`crates/pycc_types/src/constraints.rs` is the only production file changed.
`unify_terms` keeps its exact six-argument signature as a thin wrapper over a
new `unify_terms_with_declared`, which takes `declared: Option<&Ty>` — that
kept all nine other production call sites and the ~15 in-crate test call
sites byte-identical. `inference_conflict` gained the same parameter and
branches on it:

- `declared: Some(d)` → ``return type mismatch: expected `int`, found `P` ``
  with ``help: return a `int` value``.
- `declared: None` → the original
  ``{context}: conflicting inferred types `X` and `Y` `` verbatim, no help.

The `HirStmt::Return` arm sets `declared` from `return_term.as_ref().ok()`
and, in the same branch, passes `"declared return type"` instead of
`"private helper return type"` as the `context` — that string is also
interpolated into the `T0042` "cannot be inferred through a PEP 695 generic
function's own type parameter" message one arm over, which carried the same
misnomer. No test pinned that sentence, so this cost no assertion churn.

**Two things the plan review changed, both worth recording.** First, the
originally drafted wording was ``declared `int`, returned `P` ``; the
reviewer showed that `declared`/`returned` as a verb pair appears nowhere in
`pycc_types`, while ``<noun> mismatch: expected `X`, found `Y` `` is an
existing sibling (`dict key type mismatch` in
`crates/pycc_types/src/expr.rs`). The final wording follows the sibling. It
still differs textually from the annotation checker's own
``expected return type `X`, got `Y` `` for the same program, which
`crates/pycc_types/src/module/tests.rs`' merge test asserts by `assert_ne!`
— the two phases must word it differently for that test to prove the
solver's text is the one that survives de-duplication.

Second, the draft left `help` unpopulated on the grounds that
`inference_conflict` never produced one. That reasoning described the state
the change itself removes: D-152's rule leaves `help: None` for "ambiguous
multi-value conflicts with no canonical correct side", and once the declared
type is known the conflict *has* a canonical side. `docs/DIAGNOSTICS.md`'s
quality bar makes that a standing contract on the return-type family, not a
snapshot, so the declared branch now populates `help`, reusing the checker's
own phrasing verbatim. `render_human` has no `help:` codepath at all
(D-083/D-152), so no `.expected.txt` fixture moved because of this — but
`tests/diagnostics/t0022_types_per_function.expected.json`'s `"help":[]` did.

**Operand order is not what the code comment claimed.** The D-133/D-134
block in `unify_terms` says `Return` "always puts the declared return term
first". That is true of the *call*, but not of what `inference_conflict`
receives on the `(Err(var), Ok(ty)) | (Ok(ty), Err(var))` arm, which passes
the already-inferred actual first and the known type second. Measured
against the built binary: `def _h(a): return a` plus
`def f() -> int: return _h("s")` rendered
``conflicting inferred types `str` and `int` `` — misdescribed *and*
backwards. The actual type is therefore selected by comparing against
`declared`, never by position, and the new doc comments state that ordering
invariant explicitly so a future argument swap cannot silently invert the
message. No `debug_assert!` guards it: that would be an unreachable region
under D-014's gate.

Tests: two in-crate unit tests in `crates/pycc_types/src/module/tests.rs`
(the unannotated helper keeps the inferred wording; a declared return names
the annotation even when the conflict is raised reversed — the second is the
one that proves the fix) and one in `crates/pycc_types/src/tests.rs`
exercising `unify_terms_with_declared` directly on both reachable arms plus
the undeclared path. In-crate coverage is required rather than optional:
integration tests under `tests/` do not count toward `pycc_types`' own
regions (D-014, `docs/TESTING.md`). Byte-exact fixture
`tests/diagnostics/t0022_declared_return_protocol_param.{py,expected.txt}`
carries the issue's own program, registered in `tests/diagnostics_test.rs`;
its `.expected.txt` was generated from the actual `pycc check` stdout, not
hand-written. `.expected.json` is optional for this harness (12 of 143
fixtures have one) and was not added. Updated assertions: the `T0022_RETURN`
constant in `module/tests.rs` (one edit covering nine use sites, all the same
annotated shape), two message assertions in
`tests/issue_925_container_returns.rs`, and
`tests/diagnostics/t0022_types_per_function.expected.{txt,json}`.

Docs: `docs/DIAGNOSTICS.md`'s `T0022` row — which already read "return type
mismatch" and so already matched — gained a parenthetical naming both
variants and stating that the discriminator is the annotation, not
visibility; `docs/TYPE_SYSTEM.md`'s v0.1-local-inference bullet said only
"conflicting inferred returns are `T0022`", which is now half the story, and
gained the declared case. No new ADR: this changes the text of an existing
diagnostic, not a project-wide or irreversible design choice, and D-038/D-045
are untouched. `docs/ROADMAP.md` received a prose-only
edit in place, surfaced by the committed-range review: its Diagnostics row
asserted that "ambiguous-conflict diagnostics still emit `help: []`", which
this change makes half-false, so the row now records the declared-return
carve-out. No new `**[#949](...) —` feature-landing paragraph, no new
evidence bullet and no checklist change, so the status-page four-pin rotation
is not triggered (`check_status_page_freshness.rb origin/main` reports no
signal).
`docs/sessions/2026-09-05-09-issue-934-protocol-return-dispatch.md` quotes
the old wording and was deliberately left alone: D-066/D-130 session files
are immutable historical snapshots, not text to sweep.

`crates/pycc_types/src/constraints.rs` was 2210 lines before this change and is
2306 after, over AGENTS.md's
~1,000-line decomposability threshold, so that rule is engaged and the
judgment is recorded here deliberately rather than skipped: no extraction was
done, because the file's own header already records a D-185 extraction
rationale, `constraints/signatures.rs`'s header records the split already
performed under this same rule (#868, still tracked by
[#544](https://github.com/rotnov/pycc/issues/544)), and this change's
footprint inside the file is a wrapper, one parameter and one branch — the
rule's own "not by rewriting unrelated code" side.

## Gates

All run from the worktree, all exit 0: `cargo fmt --all -- --check`;
`cargo clippy --workspace --all-targets -- -D warnings`;
`cargo test --workspace` (4740 passed, 0 failed, 58 ignored, across 87 test
binaries); the CI coverage sequence
(`cargo build --target x86_64-apple-darwin -p pycc_rt`,
`cargo build --workspace`, `cargo build --release -p pycc_rt`,
`cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions
100`: TOTAL regions 53629/0 missed = 100.00%, functions 2465/0 missed =
100.00%, lines 35259/0 missed = 100.00%, with
`crates/pycc_types/src/constraints.rs` itself at 2056 regions / 33 functions
/ 1251 lines, all 100.00%);
`python3 -m unittest discover -s scripts -p 'test_*.py'`;
`check_roadmap_evidence.rb`; `check_status_page_freshness.rb origin/main`
(no signal, re-run after the ROADMAP prose edit); `check-site.sh`; `check_conformance_breadth.py`;
`check_readme_milestone_projection.rb`;
`generate_decisions_index.py --check`; `check_ci_permissions.rb`; and
`cargo doc --workspace --no-deps`. Nothing touched appears in
`tests/fixtures/policy-successor-manifest.json`.

Coverage was run *before* dispatching the committed-range review rather than
after, deliberately: `docs/TESTING.md` records that the per-file summary
takes the maximum covered-region count over each function's instantiations,
so a region covered only by the integration suite and not by the crate's own
`--cfg test` build reads as a deficit that every merged view hides. The gate
passed on the first attempt, so the JSON group-by-definition-location
diagnostic that reproduces the gate's number was not needed.

## Deviations from the plan

- The published plan's own message wording and `help` decision are what the
  plan's second review round produced, and the implementation followed the
  published plan exactly on both. Nothing was re-decided after publication.
- `cargo test --workspace` was run without `--include-ignored`: the local
  oracle is CPython 3.14.6 and the ignored conformance tests require the
  pinned 3.14.7. CI runs them.
- The plan listed the `.expected.json` `help`-array update as a separate
  work item because it is easy to miss; it was applied, and the fixture is
  green.
- The plan said `docs/ROADMAP.md` would be edited "prose only, if at all" and
  expected "at all" to be unnecessary. It was necessary: the committed-range
  review found that the Diagnostics row's blanket claim about
  ambiguous-conflict diagnostics emitting `help: []` is exactly what the
  declared branch's new `help` breaks. The edit is prose-in-place, and the
  freshness checker was re-run after it.

## Known follow-ups

- [#877](https://github.com/rotnov/pycc/issues/877) — every `pycc_types`
  diagnostic still renders at `:1:1` (D-043's placeholder span). The new
  fixture inherits it, and says so in a source comment so a reader does not
  mistake it for a regression introduced here. Explicitly out of scope.
- [#908](https://github.com/rotnov/pycc/issues/908) — enum equality; needs
  MIR/codegen identity lowering.
- [#944](https://github.com/rotnov/pycc/issues/944),
  [#954](https://github.com/rotnov/pycc/issues/954),
  [#952](https://github.com/rotnov/pycc/issues/952),
  [#932](https://github.com/rotnov/pycc/issues/932) — untouched here, for
  `issue-select` to weigh.
- D-152's own recorded gap still stands and this change adds one instance of
  it: `help` text is free text beside the `format!` that builds the message,
  with no automated check tying them together. Likewise, no CI gate compares
  `docs/DIAGNOSTICS.md`'s message column against emitted strings — D-150's
  `explain.rs` suite only checks that every registry *code* has an
  explanation entry.

## Paused autopilot

- Directive scope: open-ended (`/goal fix all opened issues`).
- Active milestone: `v0.4` (Accept unmet).
- Last iteration outcome: #953 closed by PR #955 (`923f9886`).
- This iteration: #949 implemented; PR open, awaiting CI and merge by the
  orchestrating session.
- Next step: re-enter `issue-select` for `v0.4` after this PR lands.
- Denylist: empty.

## Where to resume

The whole production change is `inference_conflict`,
`unify_terms`/`unify_terms_with_declared`, and the `HirStmt::Return` arm of
`collect_block_constraints`, all in
`crates/pycc_types/src/constraints.rs`. The doc comments on the first two are
the authoritative statement of the ordering invariant the actual-type
selection depends on; read them before touching either.

Anyone extending this pattern to another diagnostic should note what makes it
safe here: `declared` is `Some` at exactly one call site, so the `(Err, Err)`
arm of `unify_terms_with_declared` is unreachable while it is `Some`, and an
annotated return never lowers to an inference variable
(`signatures::term_for_type` returns `Ok(ty)` for every `ty != Ty::Infer`). A
second caller passing `declared: Some(...)` would need that argument
re-derived, not assumed.

The two remaining `"private helper ..."` context strings in the solver were
left alone deliberately, and each for its own checked reason.
`"private helper implicit return"` (`constraints/signatures.rs`) is gated on
`signature.2.is_err()`, so it is only reachable for genuinely unannotated
helpers — verified: an annotated function with no `return` is reported by the
annotation checker instead, as
``function `f` can exit without returning `int` ``. The argument-side
`"argument N of private helper `{callee}`"` context in `constraints.rs` sits
behind a `matches!((&arg, parameter), (Err(_), _) | (_, Err(_)))` guard, so
it always involves at least one inference variable and the "conflicting
inferred types" half of its wording is accurate; whether the *callee* is
always a private helper there was not audited, and is a separate question
from #949.
