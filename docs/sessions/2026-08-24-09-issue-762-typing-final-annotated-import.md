# 2026-08-24-09 -- Issue #762: register `typing.Final`/`typing.Annotated`

## Status: merged

PR [#766](https://github.com/rotnov/pycc/pull/766) -- "Register typing.Final
and typing.Annotated so their imports resolve" -- closes issue
[#762](https://github.com/rotnov/pycc/issues/762). Merged to `main` via a
merge commit; branch `fix-typing-final-annotated-import-762` deleted.

## What shipped

`crates/pycc_hir/src/func.rs`'s `annotation_to_ty` already fully handled
`Final[X]` and `Annotated[X, ...]` (PEP 591/593) as bare-name annotation
subscripts, unwrapping to `X` with zero codegen work needed. But
`crates/pycc_std/src/lib.rs`'s stdlib symbol registry did not recognize
`typing.Final`/`typing.Annotated`, so `from typing import Final` /
`from typing import Annotated` failed with `C0002` even though the
underlying feature worked once the import was skipped. This mirrors PR #579's
identical fix for `typing.override`/`dataclass_transform`/
`dataclasses.dataclass`.

Fix: added `typing.Final` and `typing.Annotated` to the registry under a new
`StdSymbolKind::AnnotationMarker` variant -- none of the existing marker
kinds (`ProtocolMarker`/`AbcMarker`/`DecoratorMarker`) fit an
annotation-subscript-only symbol; `DecoratorMarker`'s own doc comment ("only
valid as a class or method decorator") would have been actively false for
`Final`/`Annotated`.

### Review-driven follow-up (same PR)

The `ievo:deep-reviewer` pinned local review found no blockers and two
actionable documentation gaps (narrow an overgeneralized `STDLIB_PLAN.md`
rationale line; add the missing `docs/ROADMAP.md` v0.3 paragraph for #762),
both applied before the first push.

After CI went green once, an external `chatgpt-codex-connector` PR review
left one unresolved thread (P2): routing `AnnotationMarker` through the
generic `marker_is_not_a_value` diagnostic produced a misleading message --
"use it only as a base class marker or decorator" -- when neither `Final`
nor `Annotated` is ever valid in either of those positions; their only valid
use is as an annotation subscript (`Final[int]`, `Annotated[int, ...]`).
Fixed by adding a dedicated `annotation_marker_is_not_a_value` diagnostic in
`crates/pycc_types/src/lib.rs` and routing all four `AnnotationMarker`
match arms (value-reference and call-site marker guards, in both
`expr.rs`'s validation path and `constraints.rs`'s solver path) to it
instead of the generic message. Added two new unit tests exercising the
call-site (`typing.Final()`/`typing.Annotated()`) branches and updated the
two existing qualified-value tests to assert the new message text, for four
total targeted `pycc_types` unit tests. Replied on the review thread
explaining the fix and resolved it via GraphQL before merging (D-024's
required-conversation-resolution branch-protection rule).

## Test evidence

- `crates/pycc_types/src/tests.rs`: 4 new/updated unit tests directly
  exercising the `AnnotationMarker` match arms in both `expr.rs` and
  `constraints.rs`, covering both the bare-value and direct-call qualified
  forms (`typing.Final`, `typing.Final()`, `typing.Annotated`,
  `typing.Annotated()`).
- `tests/issue_762_typing_final_annotated.rs`: 3 end-to-end integration
  tests (`pycc check`/`pycc build`/run success for `Final`/`Annotated`
  usage; a negative `C0002` regression test for a still-unregistered symbol,
  `TypeVar`).
- `cargo test --workspace`: 79 passed, 1 failed -- the failure is the
  pre-existing, diff-unrelated
  `build_and_run_cross_compiled_to_a_different_tier_1_target` cross-compile
  linker failure in `tests/slice0.rs` (a local macOS toolchain limitation,
  not something this change touches). CI's isolated, sanitized 100%
  line/region coverage gate (`build-test-coverage` / `ci-gate`) ran clean on
  every push.
- `cargo clippy -p pycc_types --all-targets`: clean.
- All 18 required and non-required PR checks passed on the final head
  (`5aaceaa4`, a merge of `origin/main` to clear a mid-review
  `mergeStateStatus: BEHIND` after an unrelated PR #765 merged to `main`):
  `audit`, `build`, `build-test-coverage`, `ci-gate`, `classify-changes`,
  `cross-compile-build`, `cross-compile-verify`, `frontend-perf-gate`,
  `frontend-perf-measure`, `governance`, `native-build-test` (all four
  targets), `pages-accessibility`, `pages-performance`,
  `status-page-freshness`.

## Documentation updated in the same PR

- `docs/ROADMAP.md` -- new v0.3 feature-landing paragraph for #762.
- `docs/STDLIB_PLAN.md` -- narrowed the shared #579/#762 rationale line into
  two distinct justifications (conformance-fixture necessity for #579 vs.
  a plain `C0002` removal for #762, no conformance-fixture dependency).
- `site/status/index.html` / `site/sitemap.xml` -- D-156 status-page
  freshness enforcement required touching these once `docs/ROADMAP.md`
  tripped the feature-paragraph signal; bumped `dateModified`/`<lastmod>`
  and added a narrative sentence about the new import support.
- `scripts/check-site.sh` -- fixed a second, independently hardcoded
  `PAGE_SPECS["status"]["date_modified"]` expected-date constant that the
  status-page edit above left stale.
- `tests/fixtures/pages-performance-manifest.json` -- updated the pinned
  SHA-256 identity digest for `site/status/index.html`'s served bytes
  (issue #229's closed canonical performance manifest), required after the
  status-page content edit changed those bytes.

## Known follow-ups / non-actions

- No conformance matrix row moves: `tests/fixtures/pep_0591_final.py` and
  `pep_0593_annotated.py` deliberately omit the import (PEP 649/749
  deferred-annotation-evaluation semantics mean CPython itself never
  evaluates the bare name either), so this change and those fixtures test
  different concerns.
- The deep-reviewer's third (non-actionable) note -- a pre-existing header-
  comment inaccuracy in those same two conformance fixtures, unrelated to
  this change -- was left as-is per the reviewer's own judgment; no issue
  filed since it doesn't meet the AGENTS.md process-observation filing bar
  (not a gate/skill/checker defect, just a stale fixture comment worth
  fixing incidentally next time that file is touched for other reasons).

## Where to resume

Nothing outstanding from this task. For the next session: `docs/sessions/`
listing (sorted by filename) is the resume mechanism; this is currently the
newest entry for 2026-08-24.
