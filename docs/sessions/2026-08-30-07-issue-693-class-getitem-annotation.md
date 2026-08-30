# Session handoff: #693 PEP 560 annotation-position `__class_getitem__` routing

## Status: implementation complete, local gates green, PR about to open (carries `Fixes #693`)

This session ran the project's own autopilot pipeline
(`issue-select` → `issue-implement`) synchronously end to end inside the
isolated worktree `.claude/worktrees/agent-aaa93cb1a10760f73`, from
`origin/main` tip `89540097761aeb68d4ae3d425209fe02f1d11b3d`.

## Selection

#693 ("P2: PEP 560 annotation-position `ClassName[type_arg]` never routes
through `__class_getitem__`") was picked as milestone-in-scope (v0.4), P2,
small, with no blocking dependency and no open-pull-request collision on
`crates/pycc_hir`. Re-verified against the roadmap's own recorded gap for
the PEP 560 row (`docs/ROADMAP.md` line 183 at the time) and against the
issue's own completion criteria before implementing. An adversarial
`advisor` consult, run mid-session to check the overall gate list rather
than re-litigate the pick, confirmed the selection needed no change and
flagged four concrete follow-ups (addressed below): two now-stale roadmap
sentences, the `Fixes` keyword decision, a caution about interpreting an
unisolated local coverage run, and an evidence-freshness note for the
fixture extension.

## What changed

PEP 560 (`__class_getitem__`) was already wired for **value position**
(`ClassName[key]` as an expression — #610/#612) and for **gating**
annotation position on the hook's existence (#611/#613). What remained,
and what #693 tracked, was that a *passing* annotation-position subscript
still resolved to `Ty::Instance(ClassName)` unconditionally, discarding
the hook's actual return type. CPython never evaluates an annotation at
runtime, so the only observable effect is on pycc's own static type
checker: what type it assigns and what it accepts/rejects downstream.

- `crates/pycc_hir/src/class.rs`: `ClassAnnotationInfo` gained a
  `class_getitem_return: Option<Ty>` field. `class_annotation_infos` now
  takes the module's `&[HirItem]` (added to `class_annotation_infos` and
  `lower_class`'s signatures) and computes each class's field via a new
  `class_getitem_return_ty` helper, which walks the class's MRO
  (most-derived first, matching `pycc_types::resolve_static_or_class_method_call`'s
  existing walk for value position) looking up each candidate class's
  `__class_getitem__` mangled name in the module's lowered
  `HirItem::Function` list to read its already-resolved `return_ty`. The
  field is `None` when subscriptability comes only from a PEP 695 type
  parameter with no explicit hook (deliberately left to the pre-existing,
  separate `GenericClassInstantiate` mechanism, untouched by this fix) and
  for the self-referential entry `lower_class` pushes for the class
  currently being lowered (that method's own `HirItem::Function` does not
  exist yet at that point — an unchanged, narrow fallback, not a
  regression).
- `crates/pycc_hir/src/func.rs`: `annotation_to_ty`'s `Expr::Subscript`
  catch-all arm, entered only when the base name matches an entry in
  `class_defs` (i.e. never for builtins like `list`/`dict`/`Optional`,
  which are not in that table), now returns `info.class_getitem_return`
  directly when set, before falling through to the previous
  `Ty::Instance` resolution.
- `crates/pycc_hir/src/lib.rs`: two call sites updated to pass `&items`.
- `crates/pycc_hir/src/class/mro.rs`: one existing test's `lower_class`
  call site updated for the new arity.
- `crates/pycc_hir/src/tests.rs`: five new unit tests — own-class
  `@staticmethod` hook, `@classmethod` hook, MRO-inherited hook, a PEP 695
  generic-class regression guard (must stay `Ty::Instance`, unaffected by
  the new field), and the documented self-referential-in-own-body fallback
  case.
- `tests/fixtures/pep_0560_class_getitem.py`: extended with
  annotation-position cases — a function-local annotation resolved via the
  class's own hook, and two module-scope annotations resolved via a
  `@classmethod` hook and via a hook inherited through the MRO
  respectively — each read back with real `int` arithmetic so a wrong
  static type (e.g. still `Ty::Instance`) would fail to compile.
- `docs/TYPE_SYSTEM.md`: the class-model paragraph's `__class_getitem__`
  sentence rewritten to describe hook-return-type resolution (with the
  unchanged fallbacks named).
- `docs/ROADMAP.md` (line 183) and `docs/PYTHON_STANDARDS.md` (rule 9):
  both had present-tense sentences describing the now-fixed old behavior
  as current; each got a dated `**Update (2026-08-30, #693):**` note
  appended in the file's own established style, rather than rewriting the
  historical narrative in place.
- `tests/fixtures/conformance-breadth-manifest.json`: the PEP 560 row's
  `not_proven` entry for this gap keeps `kind: "core"` (the row stays
  `◐`, not `✅`) — only its `reason` text was updated to record that the
  gap is implemented and fixture-exercised, pending the D-102 CI-observed-
  evidence hand-flip. Also had to bump every row's cached `matrix_line` by
  +6: the `docs/PYTHON_STANDARDS.md` edit above shifted the matrix table
  down by six lines, which `scripts/check_conformance_breadth.py` checks
  mechanically. First attempt at this fix used a full `json.dump` rewrite,
  which round-tripped fine functionally but escaped every non-ASCII
  character (`—`, `✅`, accented letters in test fixtures) to `\uXXXX`,
  producing a large, noisy diff unrelated to this change; reverted and
  redid both the `matrix_line` bump and the `reason` text edit with
  targeted `Edit`/regex substitution that touches only the intended bytes.

## Evidence-freshness caveat (read before touching the PEP 560 row again)

The `82d63301` / run `32494747082` citation in both `docs/ROADMAP.md` and
`docs/PYTHON_STANDARDS.md` for the PEP 560 row's `◐` mark describes the
**pre-extension** `pep_0560_class_getitem.py`. This PR's fixture extension
means that citation no longer asserts current-fixture-set currency by
itself for this row; a future CI-observed-green run of the *extended*
fixture is what's needed before any hand-flip, per D-102. Both docs now
say this explicitly.

## Local gates (this session, on the branch tip about to be pushed)

- `cargo build --workspace`: clean.
- `cargo test --workspace --lib`: all 11 crates green (e.g. `pycc_hir`
  735 passed after the 5 new tests).
- `cargo test --workspace` (full suite incl. integration tests and doc
  tests): exit 0, every `test result:` line `ok`, zero `FAILED` anywhere.
- `cargo clippy --workspace --all-targets -- -D warnings`: exit 0. Only
  pre-existing, unrelated `escaped newline` warnings in
  `tests/slice1_codegen_depth.rs` print; they do not fail the build and
  are not introduced by this diff.
- `cargo doc --workspace --no-deps`: clean generation; the one pre-existing
  `bind_class`/`Self::bind_synthetic_class` private-link warning in
  `pycc_types` is unrelated and unchanged.
- D-014 hard gate: `cargo llvm-cov --workspace --fail-under-lines 100
  --fail-under-regions 100` (run directly rather than under CI's
  `run_isolated`/`sudo -u nobody` wrapper, which is a security boundary
  for untrusted PR content in CI, not a correctness precondition for a
  local check of this session's own code): **100.00% lines and 100.00%
  regions across every crate**, including the three touched files
  (`pycc_hir/src/class.rs`, `class/mro.rs`, `func.rs`) each individually
  at 100%. One caveat carried forward per the advisor's review: CI's
  coverage job pre-builds the x86_64 macOS `pycc_rt` runtime first
  specifically so the cross-compilation path can't skip its own success
  path; this local run did not reproduce that ordering, so a CI-only
  divergence on unrelated cross-target/`pycc_rt` coverage remains
  possible in principle, though nothing in this diff touches that surface.
- `python3 scripts/check_conformance_breadth.py`: passes, same aggregate
  counts as before this PR (38 evidence-backed rows, 39 distinct PEPs) —
  this PR changes a `reason` string and cached line numbers, not row
  status or counts.
- `LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8 ruby scripts/check_roadmap_evidence.rb .`:
  passes ("Roadmap evidence policy passed.").

Manual triangulation of the fix's runtime correctness (in addition to the
above, since the pinned CPython 3.14.7 oracle is unavailable in this
sandbox — only 3.14.6 is installed, and the exact
`pep_0560_class_getitem_matches_cpython_3_14_7_byte_for_byte` test is
`#[ignore]`d and asserts the 3.14.7 pin): ran the extended fixture through
pycc debug, pycc release, and real installed CPython 3.14.6, and diffed
all three outputs — byte-for-byte identical.

## Local pinned reviewer (`ievo:deep-reviewer`)

**Not run — reported unavailable, not silently skipped.** This session
attempted to dispatch it via `Skill(skill="deep-review", ...)` and the tool
itself refused: `deep-review` is registered with
`disable-model-invocation` and its own error text says plainly "Ask the
user to run `/deep-review` themselves — it cannot be invoked via the Skill
tool. Do not replicate this skill's workflow by other means." Two
`ToolSearch` queries for an alternative dispatch path (a generic
Agent/Task-launch tool) found none available in this session. Per
AGENTS.md's own fallback for a reviewer this session cannot bind or
invoke ("report the local review as unavailable instead of silently
weakening the gate"), no substitute review was run and none of the
skill's workflow was replicated by other means.

This does not block *opening* this pull request — AGENTS.md's D-068
gate is phrased as a **merge** precondition ("Before completing
significant work or merging a pull request…", "Merge only when … no
unresolved actionable review finding … remains"), and this task's scope is
to open, not merge. The PR body carries this same disclosure and asks
for `/deep-review` to be run against this branch before it is merged.

## Intent: this PR should close #693

`gh issue view 693`'s four completion criteria are all satisfiable inside
this PR: (1) the in-scope-vs-defer decision is recorded as "in scope,
implemented" via this change itself; (2) annotation resolution now routes
through the hook; (3) the fixture is extended; (4) the manifest's
`not_proven` entry is updated. Criterion 4's own text — "If no `core` gap
remains, the row **may** move to `✅` per D-102's evidence rule" — states
the `✅` flip as conditional on D-102 evidence accruing later, not as a
separate blocking requirement of this criterion, so leaving the row at
`◐` pending that evidence does not leave any criterion incomplete. The PR
therefore carries `Fixes #693`. `gh api graphql`'s `closingIssuesReferences`
confirmation (expected `totalCount: 1`, node `{693}`) is recorded in the
coordinating session's own report after the PR opens.

No `docs/decisions/` ADR: this is a scoped completion of an
already-tracked gap in an existing, documented design (the MRO-walk /
declared-return-type resolution strategy mirrors value position's own
existing precedent), not an irreversible or project-wide choice.
