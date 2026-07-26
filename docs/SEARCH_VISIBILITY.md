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
- Before URL Inspection or an indexing request, verify that the candidate
  returns `200`, declares the expected canonical URL, and appears in the
  current sitemap. Record an inspected stale or mistyped path as its own
  resource, never as evidence for the live canonical page.

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
| 2026-07-25T14:54:05Z | `pycc` | >50 | — | 50 | 363 |
| 2026-07-25T14:54:05Z | `Python 3.14 compiler in:description` | 12 | 0 | 16 | 16 |
| 2026-07-25T14:54:05Z | `ahead-of-time compiler python` | 8 | 0 | 11 | 11 |
| 2026-07-25T14:54:05Z | `typed Python compiler` | >50 | — | 50 | 316 |
| 2026-07-25T14:54:05Z | `python aot compiler` | 19 | 0 | 28 | 28 |
| 2026-07-25T14:54:05Z | `compile python native binary` | 22 | 0 | 38 | 38 |
| 2026-07-25T14:54:05Z | `AI-native compiler` | 48 | 0 | 50 | 94 |
| 2026-07-25T14:54:05Z | `topic:aot-compiler` | 16 | -2 | 18 | 18 |
| 2026-07-25T14:54:05Z | `topic:python-compiler` | 43 | -2 | 44 | 44 |
| 2026-07-25T15:43:31Z | `pycc` | >50 | — | 50 | 363 |
| 2026-07-25T15:43:31Z | `Python 3.14 compiler in:description` | 12 | 0 | 16 | 16 |
| 2026-07-25T15:43:31Z | `ahead-of-time compiler python` | 8 | 0 | 11 | 11 |
| 2026-07-25T15:43:31Z | `typed Python compiler` | >50 | — | 50 | 316 |
| 2026-07-25T15:43:31Z | `python aot compiler` | 19 | 0 | 28 | 28 |
| 2026-07-25T15:43:31Z | `compile python native binary` | 22 | 0 | 38 | 38 |
| 2026-07-25T15:43:31Z | `AI-native compiler` | 48 | 0 | 50 | 94 |
| 2026-07-25T15:43:31Z | `topic:aot-compiler` | 14 | +2 | 18 | 18 |
| 2026-07-25T15:43:31Z | `topic:python-compiler` | 41 | +2 | 44 | 44 |
| 2026-07-25T16:18:15Z | `pycc` | >50 | — | 50 | 363 |
| 2026-07-25T16:18:15Z | `Python 3.14 compiler in:description` | 12 | 0 | 16 | 16 |
| 2026-07-25T16:18:15Z | `ahead-of-time compiler python` | 8 | 0 | 11 | 11 |
| 2026-07-25T16:18:15Z | `typed Python compiler` | >50 | — | 50 | 316 |
| 2026-07-25T16:18:15Z | `python aot compiler` | 19 | 0 | 28 | 28 |
| 2026-07-25T16:18:15Z | `compile python native binary` | 22 | 0 | 38 | 38 |
| 2026-07-25T16:18:15Z | `AI-native compiler` | 48 | 0 | 50 | 94 |
| 2026-07-25T16:18:15Z | `topic:aot-compiler` | 16 | -2 | 18 | 18 |
| 2026-07-25T16:18:15Z | `topic:python-compiler` | 43 | -2 | 44 | 44 |
| 2026-07-25T16:52:38Z | `pycc` | >50 | — | 50 | 363 |
| 2026-07-25T16:52:38Z | `Python 3.14 compiler in:description` | 12 | 0 | 16 | 16 |
| 2026-07-25T16:52:38Z | `ahead-of-time compiler python` | 8 | 0 | 11 | 11 |
| 2026-07-25T16:52:38Z | `typed Python compiler` | >50 | — | 50 | 316 |
| 2026-07-25T16:52:38Z | `python aot compiler` | 19 | 0 | 28 | 28 |
| 2026-07-25T16:52:38Z | `compile python native binary` | 22 | 0 | 38 | 38 |
| 2026-07-25T16:52:38Z | `AI-native compiler` | 48 | 0 | 50 | 94 |
| 2026-07-25T16:52:38Z | `topic:aot-compiler` | 15 | +1 | 18 | 18 |
| 2026-07-25T16:52:38Z | `topic:python-compiler` | 43 | 0 | 44 | 44 |
| 2026-07-25T17:47:48Z | `pycc` | >50 | — | 50 | 363 |
| 2026-07-25T17:47:48Z | `Python 3.14 compiler in:description` | 12 | 0 | 16 | 16 |
| 2026-07-25T17:47:48Z | `ahead-of-time compiler python` | 8 | 0 | 11 | 11 |
| 2026-07-25T17:47:48Z | `typed Python compiler` | >50 | — | 50 | 316 |
| 2026-07-25T17:47:48Z | `python aot compiler` | 19 | 0 | 28 | 28 |
| 2026-07-25T17:47:48Z | `compile python native binary` | 22 | 0 | 38 | 38 |
| 2026-07-25T17:47:48Z | `AI-native compiler` | 48 | 0 | 50 | 94 |
| 2026-07-25T17:47:48Z | `topic:aot-compiler` | 15 | 0 | 18 | 18 |
| 2026-07-25T17:47:48Z | `topic:python-compiler` | 43 | 0 | 44 | 44 |
| 2026-07-25T19:07:45Z | `pycc` | >50 | — | 50 | 363 |
| 2026-07-25T19:07:45Z | `Python 3.14 compiler in:description` | 12 | 0 | 16 | 16 |
| 2026-07-25T19:07:45Z | `ahead-of-time compiler python` | 8 | 0 | 11 | 11 |
| 2026-07-25T19:07:45Z | `typed Python compiler` | >50 | — | 50 | 316 |
| 2026-07-25T19:07:45Z | `python aot compiler` | 19 | 0 | 28 | 28 |
| 2026-07-25T19:07:45Z | `compile python native binary` | 22 | 0 | 38 | 38 |
| 2026-07-25T19:07:45Z | `AI-native compiler` | 48 | 0 | 50 | 94 |
| 2026-07-25T19:07:45Z | `topic:aot-compiler` | 15 | 0 | 18 | 18 |
| 2026-07-25T19:07:45Z | `topic:python-compiler` | 43 | 0 | 44 | 44 |

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
| 2026-07-25T14:54:05Z | 2026-07-24 | 74 / 1 | 1,444 / 349 | 0 / 1 / 0 | `github.com`: 5 / 1 unique; `rotnov.github.io`: 2 / 1 unique |
| 2026-07-25T15:43:17Z | 2026-07-24 | 74 / 1 | 1,444 / 349 | 0 / 1 / 0 | `github.com`: 5 / 1 unique; `rotnov.github.io`: 2 / 1 unique |
| 2026-07-25T16:18:15Z | 2026-07-24 | 74 / 1 | 1,444 / 349 | 0 / 1 / 0 | `github.com`: 5 / 1 unique; `rotnov.github.io`: 2 / 1 unique |
| 2026-07-25T16:52:38Z | 2026-07-24 | 74 / 1 | 1,444 / 349 | 0 / 1 / 0 | `github.com`: 5 / 1 unique; `rotnov.github.io`: 2 / 1 unique |
| 2026-07-25T17:47:48Z | 2026-07-24 | 74 / 1 | 1,444 / 349 | 0 / 1 / 0 | `github.com`: 5 / 1 unique; `rotnov.github.io`: 2 / 1 unique |
| 2026-07-25T19:08:00Z | 2026-07-24 | 74 / 1 | 1,444 / 349 | 0 / 1 / 0 | `github.com`: 5 / 1 unique; `rotnov.github.io`: 2 / 1 unique |

## Google Search Console history

URL Inspection is the authoritative owner-facing index check. A separate
sitemap row can fail or lag even when an individually inspected URL is already
indexed, so this ledger records those states independently.

| Observed at (UTC) | URL Inspection | Sitemap | Performance |
|---|---|---|---|
| 2026-07-25T10:29:08Z | All 4 canonical URLs report “URL is on Google”; the 3 evidence pages also report one valid breadcrumb item each | Public `/sitemap.xml` returns `200 application/xml` with 4 valid canonical URLs. Search Console still reports that it could not process the sitemap and 0 discovered pages; a new submission was accepted for periodic processing | Report still processing; no clicks, impressions, or query rows available |
| 2026-07-25T12:54:10Z | The new comparison URL reports “URL is not on Google” because it is unknown to Google; a request was accepted into the priority crawl queue | Public `/sitemap.xml` returns `200 application/xml` with 5 valid canonical URLs. Search Console still reports “Couldn’t fetch” and 0 discovered pages; another submission was accepted for periodic processing | Report still processing; no clicks, impressions, or query rows available |
| 2026-07-25T16:15:43Z | Correction: the inspected `/compare/python-compilers/` path still reports “URL is not on Google” because it is unknown, but an independent fetch proves that path is a noncanonical `404`; the live `/python-aot-compilers/` comparison URL was not inspected in this snapshot, so its individual index state remains unknown | Public `/sitemap.xml` returns `200 application/xml` with 5 canonical URLs including `/python-aot-compilers/`. Search Console still reports “Couldn’t fetch,” no processing date, and 0 discovered pages | The processed 3-month web report is updated about 5.5 hours before this observation and reports 0 clicks, 0 impressions, and no query rows; therefore no Google query position exists yet |
| 2026-07-25T16:43:18Z | The live `/python-aot-compilers/` URL reports “URL is on Google,” HTTPS valid, and one valid breadcrumb item. Googlebot Smartphone last crawled it at 13:58:20 UTC; fetch and indexing permission succeeded, and Google selected the inspected canonical | Public `/sitemap.xml` still returns `200 application/xml` with all 5 canonical URLs. Search Console still reports “Couldn’t fetch,” 0 discovered pages, and a temporary processing error for the page's sitemap discovery field | The processed 3-month web report is updated about 5 hours before this observation and still reports 0 clicks, 0 impressions, and no query rows; therefore no Google query position exists yet |
| 2026-07-25T17:49:31Z | No new URL Inspection was run; the latest authoritative evidence remains the positive inspection state for all 5 canonical URLs recorded above | Public `/sitemap.xml` returns `200 application/xml` with all 5 canonical URLs. Search Console still reports “Couldn’t fetch,” no processing date, and 0 discovered pages | The processed 3-month web report is updated about 4.5 hours before this observation and still reports 0 clicks, 0 impressions, and no query rows; therefore no Google query position exists yet |
| 2026-07-25T19:10:03Z | No new URL Inspection was run; the latest authoritative evidence remains the positive inspection state for all 5 canonical URLs recorded above | Public `/sitemap.xml` returns `200 application/xml` with all 5 canonical URLs. Search Console still reports “Couldn’t fetch,” no processing date, and 0 discovered pages | The processed 3-month web report is updated about 5 hours before this observation and still reports 0 clicks, 0 impressions, and no query rows; therefore no Google query position exists yet |

## Change log

Record meaningful discovery-surface changes separately from observations so a
later movement can be correlated without presenting correlation as causation.

| Changed at (UTC) | Surface | Change |
|---|---|---|
| 2026-07-25T10:28:25Z | GitHub repository metadata | Kept the honest ahead-of-time positioning, added the standard `AOT` abbreviation to the description, and added the focused `typed-python`, `python-314`, and `llvm-compiler` topics |
| 2026-07-25T12:04:00Z | Public evidence site | Added a crawlable, source-backed Python AOT compiler comparison that distinguishes language, artifact, runtime, and maturity models while warning that pycc is not production-ready |
| 2026-07-25T13:21:53Z | Public evidence site | Expanded the source-backed comparison to LPython after current public web results exposed a missing typed-Python/AOT model; the new claims cite official project and repository documentation, and the landing-page hero now stays within narrow mobile viewports |
| 2026-07-25T14:58:50Z | Repository and public evidence site | Synchronized README, landing, status, architecture, comparison, Markdown, and `llms.txt` with the implemented v0.1 frontend from PR #86 while preserving the slice-only native backend boundary; added validator mutations so a structurally valid site cannot silently retain superseded compiler-status copy, and made qualified prose identifiers wrap after browser QA exposed 320-pixel document overflow |

## Current interpretation

At 19:07 UTC, all nine measured GitHub Search positions and their result totals
were unchanged from the 17:47 UTC snapshot. The repository remains rank 12 for
the Python 3.14 description query, rank 8 for
`ahead-of-time compiler python`, rank 19 for `python aot compiler`, rank 15 for
`topic:aot-compiler`, and rank 43 for `topic:python-compiler`. The project
remains outside the top 50 for the ambiguous exact name and broad typed-Python
query. The absence of movement over this interval does not justify a content
or metadata change.
All five canonical URLs now have positive URL Inspection evidence: the four
pre-existing URLs were on Google at 10:29 UTC, and the live
`/python-aot-compilers/` page was on Google at 16:43 UTC with a successful
mobile crawl, matching canonical, valid HTTPS, and valid breadcrumb. The
earlier accepted request for the noncanonical `404` path remains only a
historical correction and is not credited for this result. Search Console
still reports 0 impressions and 0 clicks, so there are no Google query
positions. At 19:10 UTC, sitemap processing remained unsuccessful even though
the public sitemap returned `200` with the five canonical URLs, reinforcing
that indexing and sitemap processing are independent signals. The unchanged
traffic window remains too automation-heavy and low-uniqueness to attribute to
SEO. Search Console's overview still says that indexing data is processing; its
separate HTTPS aggregate currently shows 4 valid pages and 0 non-HTTPS pages,
which neither replaces nor contradicts the five per-URL inspection results.
