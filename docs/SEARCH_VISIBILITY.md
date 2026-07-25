# Search Visibility Measurements

This is the chronological evidence ledger for pycc search discovery. It records
observations, not ranking guarantees or product capabilities. Add new snapshots
without replacing earlier measurements so changes remain comparable.

## Measurement contract

- GitHub repository search uses the public REST
  `GET /search/repositories` endpoint with its default best-match ordering.
- Queries are unqualified except where the query itself deliberately measures a
  topic or description field. No `user:` or `repo:` qualifier is added.
- Each measurement requests the first 50 results and records the 1-based
  position of `rotnov/pycc`; `>50` means absent from that measured window.
- `Δ` is positive when the repository moved closer to rank 1, `new` when it
  entered the measured window, and `—` when no comparable change exists.
- GitHub's index and best-match ordering are volatile. Preserve every timestamp
  and exact query; never present one observation as a stable rank.
- Google positions are added only from verified Search Console performance
  data. URL Inspection, sitemap acceptance, crawl status, and `site:` searches
  are different signals and are not substituted for query position.

## GitHub repository search history

The first snapshot below was preserved from the project monitor. Its `Results`
value is the number of rows returned by that measurement; its API-wide total
was not recorded. Later snapshots record both returned rows and `Total`.

| Observed at (UTC) | Exact query | Rank | Δ | Results | Total |
|---|---|---:|---:|---:|---:|
| 2026-07-24T23:02:03Z | `pycc` | >50 | — | 50 | — |
| 2026-07-24T23:02:03Z | `Python 3.14 compiler in:description` | 12 | — | 16 | — |
| 2026-07-24T23:02:03Z | `ahead-of-time compiler python` | >50 | — | 0 | — |
| 2026-07-24T23:02:03Z | `typed Python compiler` | >50 | — | 3 | — |
| 2026-07-24T23:02:03Z | `python aot compiler` | >50 | — | 1 | — |
| 2026-07-24T23:02:03Z | `compile python native binary` | >50 | — | 0 | — |
| 2026-07-24T23:02:03Z | `AI-native compiler` | >50 | — | 2 | — |
| 2026-07-24T23:02:03Z | `topic:aot-compiler` | 12 | — | 18 | — |
| 2026-07-24T23:02:03Z | `topic:python-compiler` | 41 | — | 44 | — |
| 2026-07-25T08:49:50Z | `pycc` | >50 | — | 50 | 363 |
| 2026-07-25T08:49:50Z | `Python 3.14 compiler in:description` | 12 | 0 | 16 | 16 |
| 2026-07-25T08:49:50Z | `ahead-of-time compiler python` | 8 | new | 11 | 11 |
| 2026-07-25T08:49:50Z | `typed Python compiler` | >50 | — | 50 | 316 |
| 2026-07-25T08:49:50Z | `python aot compiler` | >50 | — | 27 | 27 |
| 2026-07-25T08:49:50Z | `compile python native binary` | 22 | new | 38 | 38 |
| 2026-07-25T08:49:50Z | `AI-native compiler` | 48 | new | 50 | 94 |
| 2026-07-25T08:49:50Z | `topic:aot-compiler` | 10 | +2 | 18 | 18 |
| 2026-07-25T08:49:50Z | `topic:python-compiler` | 39 | +2 | 45 | 45 |
| 2026-07-25T10:23:18Z | `pycc` | >50 | — | 50 | 363 |
| 2026-07-25T10:23:18Z | `Python 3.14 compiler in:description` | 12 | 0 | 16 | 16 |
| 2026-07-25T10:23:18Z | `ahead-of-time compiler python` | 8 | 0 | 11 | 11 |
| 2026-07-25T10:23:18Z | `typed Python compiler` | >50 | — | 50 | 316 |
| 2026-07-25T10:23:18Z | `python aot compiler` | >50 | — | 27 | 27 |
| 2026-07-25T10:23:18Z | `compile python native binary` | 22 | 0 | 38 | 38 |
| 2026-07-25T10:23:18Z | `AI-native compiler` | 48 | 0 | 50 | 94 |
| 2026-07-25T10:23:18Z | `topic:aot-compiler` | 12 | -2 | 18 | 18 |
| 2026-07-25T10:23:18Z | `topic:python-compiler` | 42 | -3 | 45 | 45 |
| 2026-07-25T11:58:38Z | `pycc` | >50 | — | 50 | 363 |
| 2026-07-25T11:58:38Z | `Python 3.14 compiler in:description` | 12 | 0 | 16 | 16 |
| 2026-07-25T11:58:38Z | `ahead-of-time compiler python` | 8 | 0 | 11 | 11 |
| 2026-07-25T11:58:38Z | `typed Python compiler` | >50 | — | 50 | 316 |
| 2026-07-25T11:58:38Z | `python aot compiler` | 19 | new | 28 | 28 |
| 2026-07-25T11:58:38Z | `compile python native binary` | 22 | 0 | 38 | 38 |
| 2026-07-25T11:58:38Z | `AI-native compiler` | 48 | 0 | 50 | 94 |
| 2026-07-25T11:58:38Z | `topic:aot-compiler` | 14 | -2 | 18 | 18 |
| 2026-07-25T11:58:38Z | `topic:python-compiler` | 41 | +1 | 44 | 44 |
| 2026-07-25T13:18:56Z | `pycc` | >50 | — | 50 | 363 |
| 2026-07-25T13:18:56Z | `Python 3.14 compiler in:description` | 12 | 0 | 16 | 16 |
| 2026-07-25T13:18:56Z | `ahead-of-time compiler python` | 8 | 0 | 11 | 11 |
| 2026-07-25T13:18:56Z | `typed Python compiler` | >50 | — | 50 | 316 |
| 2026-07-25T13:18:56Z | `python aot compiler` | 19 | 0 | 28 | 28 |
| 2026-07-25T13:18:56Z | `compile python native binary` | 22 | 0 | 38 | 38 |
| 2026-07-25T13:18:56Z | `AI-native compiler` | 48 | 0 | 50 | 94 |
| 2026-07-25T13:18:56Z | `topic:aot-compiler` | 14 | 0 | 18 | 18 |
| 2026-07-25T13:18:56Z | `topic:python-compiler` | 41 | 0 | 44 | 44 |

## GitHub traffic history

Traffic API totals cover overlapping rolling 14-day windows and must not be
added together. GitHub may backfill a window after an earlier request; preserve
both observations instead of rewriting the earlier response. Clone activity can
include CI, agents, and other automation and is not treated as human discovery.

| Collected at (UTC) | API data through | Views / unique | Clones / unique | Stars / forks / watchers | Referrers |
|---|---|---:|---:|---:|---|
| 2026-07-24T23:08:42Z | 2026-07-24 | 0 / 0 | 0 / 0 | 0 / 0 / 0 | none returned |
| 2026-07-25T09:08:07Z | 2026-07-24 | 74 / 1 | 1,444 / 349 | 0 / 1 / 0 | `github.com`: 5 / 1 unique; `rotnov.github.io`: 2 / 1 unique |
| 2026-07-25T10:23:18Z | 2026-07-24 | 74 / 1 | 1,444 / 349 | 0 / 1 / 0 | `github.com`: 5 / 1 unique; `rotnov.github.io`: 2 / 1 unique |
| 2026-07-25T11:58:38Z | 2026-07-24 | 74 / 1 | 1,444 / 349 | 0 / 1 / 0 | `github.com`: 5 / 1 unique; `rotnov.github.io`: 2 / 1 unique |
| 2026-07-25T13:18:56Z | 2026-07-24 | 74 / 1 | 1,444 / 349 | 0 / 1 / 0 | `github.com`: 5 / 1 unique; `rotnov.github.io`: 2 / 1 unique |

## Google Search Console history

URL Inspection is the authoritative owner-facing index check. A separate
sitemap row can fail or lag even when an individually inspected URL is already
indexed, so this ledger records those states independently.

| Observed at (UTC) | URL Inspection | Sitemap | Performance |
|---|---|---|---|
| 2026-07-25T10:29:08Z | All 4 canonical URLs report “URL is on Google”; the 3 evidence pages also report one valid breadcrumb item each | Public `/sitemap.xml` returns `200 application/xml` with 4 valid canonical URLs. Search Console still reports that it could not process the sitemap and 0 discovered pages; a new submission was accepted for periodic processing | Report still processing; no clicks, impressions, or query rows available |
| 2026-07-25T12:54:10Z | The new comparison URL reports “URL is not on Google” because it is unknown to Google; a request was accepted into the priority crawl queue | Public `/sitemap.xml` returns `200 application/xml` with 5 valid canonical URLs. Search Console still reports “Couldn’t fetch” and 0 discovered pages; another submission was accepted for periodic processing | Report still processing; no clicks, impressions, or query rows available |

## Change log

Record meaningful discovery-surface changes separately from observations so a
later movement can be correlated without presenting correlation as causation.

| Changed at (UTC) | Surface | Change |
|---|---|---|
| 2026-07-25T10:28:25Z | GitHub repository metadata | Kept the honest ahead-of-time positioning, added the standard `AOT` abbreviation to the description, and added the focused `typed-python`, `python-314`, and `llvm-compiler` topics |
| 2026-07-25T12:04:00Z | Public evidence site | Added a crawlable, source-backed Python AOT compiler comparison that distinguishes language, artifact, runtime, and maturity models while warning that pycc is not production-ready |
| 2026-07-25T13:21:53Z | Public evidence site | Expanded the source-backed comparison to LPython after current public web results exposed a missing typed-Python/AOT model; the new claims cite official project and repository documentation, and the landing-page hero now stays within narrow mobile viewports |

## Current interpretation

All nine measured GitHub positions were unchanged at 13:18 UTC. The repository
remains visible for the specific Python 3.14 description query and within the
measured window for every tracked AOT/native/AI intent, including rank 19 for
`python aot compiler`. It is still outside the top 50 for the ambiguous exact
name and broad typed-Python query. Current public web research exposed LPython
as a missing comparison model; that content gap is not a Google rank or
impression claim. The latest URL Inspection evidence has two timestamps: the
four pre-existing canonical URLs were on Google at 10:29 UTC, while the new
comparison URL was unknown to Google at 12:54 UTC and now has an accepted
priority indexing request. Google query performance is not available, and
sitemap processing remains unsuccessful after another accepted resubmission.
The unchanged traffic window remains too automation-heavy and low-uniqueness
to attribute to SEO.
