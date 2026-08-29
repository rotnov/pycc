# Incident: doc-comment-overclaims-unqualified-scope

**Date:** 2026-08-29
**Topic:** doc-comment-overclaims-unqualified-scope
**Verdict:** pending — singleton, no artefact
**Batch:** `.harden/findings/issue-676.jsonl`, finding 2 (doc-accuracy), round 1

## Symptom

`check_incompatible_attribute_redeclarations`'s doc comment
(`crates/pycc_types/src/lib.rs`) claimed its MRO walk compares each
declaring class's own "(non-inherited) `attrs` entries" as an unqualified,
universal premise. It is not universal: for a `@dataclass` hierarchy,
`pycc_hir::class`'s own field merge (`merged_fields`/`field_name_present`,
`crates/pycc_hir/src/class.rs`) already deduplicates same-named fields by
first-encountered-wins across the MRO before this check ever runs, so a
dataclass field-name/type conflict is resolved upstream and never reaches
this check as a divergent pair. The doc comment's blanket wording did not
account for this carve-out.

A related-but-not-identical shape already exists in this journal under
`conditional-rejection-missing-its-negative-complement-test`'s own body
(recurrence 1, "a doc comment overclaiming 'the one production site' for an
invariant" — that container topic's *name* is about a missing negative-path
test, not about doc-comment scope, so this instance is filed as its own
topic rather than folded into that one, per the batch process's guidance to
promote a soft/secondary match to its own name rather than let it keep
riding inside an unrelated container).

## Root cause

A doc comment stating a general rule ("the walk compares X") does not, by
itself, record the rule's own exceptions when an earlier pass in the
pipeline silently resolves a subset of cases before the documented function
ever sees them. The doc comment is accurate for the cases that do reach the
function; it is not accurate as a claim about every case the reader might
reasonably assume it covers (here, every class kind, including
`@dataclass`). Nothing mechanical catches this: the code is correct, the
tests are correct, only the English claim is over-broad.

## Termination point

Not analyzed to a specific artefact this pass — singleton, below the
batch's frequency threshold (this journal's threshold rule: class size >= 2,
or any journal match, triggers the full ladder; a genuinely novel singleton
with no match is recorded as an open counter instead). What detects this
failure is exactly what caught it here: a reviewer reading the function body
this doc comment describes against the doc comment's own words, for a
class-kind boundary the code carves out but the prose does not name.

## Fix (this incident, not a durable artefact)

Added a paragraph to the doc comment stating precisely what happens for
`@dataclass` classes (the merge happens upstream via `merged_fields`, so the
MRO walk here cannot observe a divergent pair for a dataclass field name; an
ordinary class's own MRO conflict is unaffected).

**fixture:** none — singleton, no artefact built
**artifact:** none — verdict pending
**verify:** manual — verified the `@dataclass` field-merge claim directly
against `crates/pycc_hir/src/class.rs`'s `merged_fields`/`field_name_present`
construction before writing the corrected doc comment
