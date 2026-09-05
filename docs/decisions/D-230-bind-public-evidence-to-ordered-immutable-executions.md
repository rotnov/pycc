---
id: D-230
title: "Bind public evidence to ordered immutable executions"
status: accepted
---

## D-230: Bind public evidence to ordered immutable executions

- Status: accepted
- Context: D-186's single build/run/output shape cannot represent either an
  independent pycc/CPython comparison or paired human/JSON diagnostics. Issue
  #565 supplies real execution tests in an earlier commit, allowing publication
  to name immutable evidence without claiming its own containing tree was
  already tested. The reviewed plan is the issue's comment 5552101063.
- Decision: supersede D-186's schema version and accepted-record shape with
  `2.0.0`. Preserve its eight ordered identities, kinds, closed state vocabulary,
  null-only unavailable records, and unchanged landing proof through a bounded
  legacy adapter. Language and Diagnostics use one ordered execution contract:
  `command` has repository-root `cwd` and `executions`; each execution has an
  `id`, ordered `argv`, `profile`, ordered `flags`, integer `exit_status`, and
  `stdout`/`stderr`. Each stream is exactly an artifact reference or
  `{"empty": true}`; empty output is not unavailable output. `snapshot` has
  ordered `artifacts`, each with `id`, repository-relative `path`, canonical-LF
  `sha256`, exact `text`, and `format`. Fixture scope names its current owner,
  proven categories and classified gaps; tests name registered production
  functions. Every nested record has a closed key set and reviewed value types.

  1. `scripts/check-site.sh` remains the public offline entrypoint, preserving
     its path overrides. `check_site_evidence.py` owns inventory/legacy and
     cross-surface state validation; `site_execution_evidence.py` owns the one
     shared execution validator and ordered transcript projection. No recorded
     shell text or compiler is executed and no provider is contacted.
  2. Source commit `0d94ad8f30b27131a5da381a034d55165558e56a` is preserved.
     Its complete tree `26bf9fe465c50d8065b1e0260e6100dc3e68f193` equals the
     tree tested at PR merge `321e66ff71f1eb4dedcd34d606f98994ad198758`, based
     on `4eca5e24e09d6972b5717f35652e5201dde2a02f`, in successful CI run
     `33969157527`. The five reviewed runner/target/job tuples are fixed in
     the shared validator. They prove exact CPython 3.14.7 execution for this
     fixture, not whole-language acceptance. The validator resolves the
     preserved source tree and compares local artifact bytes with its blobs;
     it does not require the ephemeral PR merge object to remain available.
     Publication must preserve that ancestor: no squash or rebase removing it.
  3. Language proves the displayed debug `pycc run` and independent CPython
     stdout `15\n`, exit 0, empty stderr. Separate existing dual-profile
     conformance is linked, not relabeled as the displayed execution.
     Diagnostics proves two stdout formats, exit 1 and empty stderr for one
     T0021 occurrence. JSON help is real; human help rendering and precise
     type-checker spans remain absent. Serialization and diagnostic-class
     correctness are different claims. Current PEP 526 scope is separately
     compared with the breadth owner; historical execution does not freeze
     current capability wording or promote matrix markers.
  4. HTML contains one visible hero with ordered parsed source, commands,
     outputs and statuses. Only one final fixture newline is omitted from a
     code pane; other whitespace drift fails. Provenance and limitations are
     visible in that hero, with exact artifact and five job links. Central
     Markdown and LLM projections carry the shared exact execution summaries,
     states, limitations and canonical detail links; they do not duplicate full
     page mirrors. JSON-LD and social metadata retain the same ID/kind/state.
     Existing context allocations remain unchanged.
  5. Seven canonical routes participate in navigation, sitemap freshness,
     performance, accessibility and the current visit-measurement contract.
     The separate 404 performance cohort remains separate. Empty visit
     observations and historical collection provenance remain unchanged;
     publication is not indexing, ranking, or traffic evidence.
- Alternatives: independent language and diagnostic mini-schemas would create
  two validator paths for the same execution facts. A site generator would
  replace hand-authored explanatory content without being needed to validate
  it. Full transcript duplication in the central maps would spend their
  bounded context allocation on repeated provenance; exact linked summaries
  preserve the material claims while detailed artifacts stay on their pages.
  Live GitHub fetches would make required PR validation depend on mutable
  provider state. Requiring the ephemeral merge object indefinitely would
  invalidate preserved evidence when GitHub replaces the merge ref. Each is
  rejected in favor of one offline model and a preserved source ancestor.
- Consequences: changing an accepted execution requires reviewed allowlist,
  manifest, artifact, test, projection, scope and mutation updates together.
  Landing and the five unavailable heroes remain intact. New public-CLI
  controls cover nested fields, immutable blobs, execution/provenance drift,
  hidden transcripts/H1s, scope/limitation drift and route cohorts. No compiler
  behavior, provider account, analytics or CI threshold changes are authorized
  by this decision.
