# Incident: hand-rolled-skip-branch-defies-expect-convention

**Date:** 2026-08-29
**Topic:** hand-rolled-skip-branch-defies-expect-convention
**Verdict:** pending — singleton, no artefact (no journal match found; checked
all 29 existing incident topics plus this batch's own new entries before
concluding no match)
**Batch:** `.harden/findings/issue-676.jsonl`, finding 3 (coverage-convention),
round 1

## Symptom

`check_incompatible_attribute_redeclarations`
(`crates/pycc_types/src/lib.rs`) used a hand-rolled
`let Some(mro_def) = classes.get(...) else { continue }` guard for an MRO
name absent from `hir.class_defs`, defended by a comment calling it
"defensive." That branch is unreachable by any real compiler input --
`class_def.mro` is C3-linearized from `hir.class_defs` itself, so every name
in it already has a registered class def by construction -- and was
coverable only by a synthetic `"Ghost"`-MRO fixture
(`a_mro_entry_with_no_registered_class_def_is_skipped_defensively`) built
purely to satisfy D-014's 100%-region coverage gate. This deviates from
`pycc_hir`'s own established convention for the identical situation: two
precedents in `crates/pycc_hir/src/class.rs` (`collect_init_attrs`'s
target-assign lookup, and the dataclass-field-merge MRO lookup) both use
`.expect(...)` with a doc comment citing the D-014 coverage rationale
directly, rather than a skip branch plus a synthetic test.

## Root cause

Two mechanisms for handling a "this should never happen" lookup failure
coexist in the codebase with no enforcement that new code pick the
established one: a hand-rolled conditional-skip (which invites a
synthetic-only test to satisfy the coverage gate) and `.expect(...)` (which
needs no such test, since it never executes on a well-formed program). The
convention lives only in two doc comments and this crate's own prior
practice -- nothing greps for the pattern or flags a new occurrence of the
weaker form.

## Termination point

Not analyzed to a specific artefact this pass -- singleton, below the
batch's frequency threshold, and no journal match. A plausible future rung
if this recurs: a `review-check` (not `precommit`, since "structurally
unreachable" requires the same judgment call a human/LLM reviewer already
makes, not a lexical pattern -- a hand-rolled skip branch guarding a
genuinely reachable condition is legitimate and must not be flagged) asking
whether a new `if let ... else { continue/return/None }` guarding an
HIR/MRO-shape lookup is provably unreachable, and if so, recommending
`.expect(...)` with a doc-comment citation of the established precedents
instead. Not built now: a single occurrence does not clear this journal's
frequency-or-journal-match threshold, and a review-check for a
judgment-dependent shape is exactly the kind of artefact this journal's own
audit step (4a) warns against over-building on a first sighting.

## Fix (this incident, not a durable artefact)

Replaced the `continue` guard with
`.expect("every name in a well-formed MRO has a registered class def")`,
matching the cited `pycc_hir` precedents' phrasing and rationale. Removed
the now-unnecessary `a_mro_entry_with_no_registered_class_def_is_skipped_defensively`
test. Re-ran the coverage gate after the change (removing a branch changes
region/line counts): `cargo llvm-cov --workspace --fail-under-lines 100
--fail-under-regions 100` passed at 100.00%/100.00% (47968 regions, 31017
lines, 0 missed of either).

**fixture:** none -- singleton, no artefact built
**artifact:** none -- verdict pending
**verify:** manual -- verified both cited `pycc_hir` precedents directly
before treating the convention claim as real, and re-ran the full coverage
gate after applying the `.expect(...)` substitution
