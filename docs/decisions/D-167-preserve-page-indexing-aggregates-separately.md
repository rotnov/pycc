---
id: D-167
title: "Preserve Page Indexing aggregates separately from per-URL evidence"
status: accepted
---

## D-167: Preserve Page Indexing aggregates separately from per-URL evidence
- Status: accepted
- Context: Google Search Console's Page Indexing report shows aggregate-level
  indexing states that lag behind per-URL URL Inspection evidence. The
  comparison page `/python-aot-compilers/` is indexed per a fresh URL
  Inspection, but the Page Indexing aggregate (report last updated 2026-07-24)
  still lists it under "Crawled — currently not indexed" with 4 indexed and 1
  not indexed. The repository had no structured capture for the aggregate, its
  report-update date, reason rows, example URLs, or its reconciliation with
  newer per-URL and performance evidence. Without one, automation could
  overwrite the five positive per-URL inspections with the aggregate count of
  four, mark the comparison page absent, or treat report lag as a live-site
  regression.
- Decision: Add a distinct `page_indexing_observations` append-only series to
  `docs/SEARCH_CONSOLE_OBSERVATIONS.json`, reusing the provenance-bearing
  envelope from #163 but separate from the per-URL `observations` array. Each
  snapshot preserves the verified property, collection timestamp, transport,
  locale, page scope, sitemap filter, report last-updated date, report-level
  totals, filters, data freshness state, and each reason row independently
  (exact reason, source, validation state, affected-page count, first-detected
  date, displayed examples with last-crawl dates, row/example limits, and
  sampled/truncated flags). The `latest_projection` carries the latest
  aggregate's report date, totals, freshness, first reason, and first example.
  The validator rejects conflating the aggregate with per-URL evidence: a
  disagreeing aggregate marked `fresh` is rejected; it must be
  `report_lag_or_unreconciled`. Binding phrases in `SEARCH_VISIBILITY.md` and
  `ROADMAP.md` keep both the five positive inspections and the lagging 4/1
  aggregate visible.
- Alternatives: (a) Store the aggregate inside the per-URL `observations`
  array — rejected because it conflates two report cadences and lets a single
  timestamp collapse report, crawl, inspection, and performance dates. (b) A
  separate artifact — rejected because #163 requires reusing the provenance
  envelope, not creating a second incompatible owner-evidence format. (c)
  Prose-only — rejected because the prose ledger cannot safely represent the
  contradiction or prevent automation from selecting one number.
- Consequences: The Page Indexing aggregate is preserved as an immutable
  append-only baseline. Future refreshes append a new snapshot rather than
  overwriting the 2026-07-29 baseline. The validator enforces the
  reconciliation contract in `pages.yml` CI. The roadmap honestly notes both
  the five positive per-URL inspections and the lagging 4/1 aggregate. "Validate
  fix" is not started unless a deployed site-side change is identified and the
  action is bounded and recorded.
