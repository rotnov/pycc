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

## GitHub traffic history

Traffic API totals cover overlapping rolling 14-day windows and must not be
added together. GitHub may backfill a window after an earlier request; preserve
both observations instead of rewriting the earlier response. Clone activity can
include CI, agents, and other automation and is not treated as human discovery.

| Collected at (UTC) | API data through | Views / unique | Clones / unique | Stars / forks / watchers | Referrers |
|---|---|---:|---:|---:|---|
| 2026-07-24T23:08:42Z | 2026-07-24 | 0 / 0 | 0 / 0 | 0 / 0 / 0 | none returned |
| 2026-07-25T09:08:07Z | 2026-07-24 | 74 / 1 | 1,444 / 349 | 0 / 1 / 0 | `github.com`: 5 / 1 unique; `rotnov.github.io`: 2 / 1 unique |

## Current interpretation

The repository is visible for the specific Python 3.14 description query and
has entered the measured window for the ahead-of-time, native-binary, and
AI-native intents. It is still outside the top 50 for the ambiguous exact name
and the broad typed-Python/AOT queries. These are discovery observations only;
they do not demonstrate crawl coverage or search-engine indexing. The traffic
backfill confirms repository activity but is too automation-heavy and
low-uniqueness to attribute to SEO.
