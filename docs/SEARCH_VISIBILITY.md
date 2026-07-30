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
- Ledger and registry timestamps use exact second-precision UTC in
  `YYYY-MM-DDTHH:MM:SSZ` form.
- GitHub history timestamps are nondecreasing. Appending an older timestamp
  after a newer observation is forbidden even when the query predates the
  registry contract.
- GitHub's index and best-match ordering are volatile. Preserve every timestamp
  and exact query; never present one observation as a stable rank.
- Google positions are added only from verified Search Console performance
  data. URL Inspection, sitemap acceptance, crawl status, and `site:` searches
  are different signals and are not substituted for query position.
- Before URL Inspection or an indexing request, verify that the candidate
  returns `200`, declares the expected canonical URL, and appears in the
  current sitemap. Record an inspected stale or mistyped path as its own
  resource, never as evidence for the live canonical page.

## Query registry

The [machine-readable query registry](./SEARCH_QUERY_REGISTRY.json) is the
source of truth for exact query text, provider surface, intent, lifecycle,
semantic identity, and KPI role. Product-acquisition positions are reported
separately from brand, metadata, topic, competitive, and authorship
diagnostics. A diagnostic can remain useful without increasing an acquisition
"N of M" count.

The registry uses provider-specific semantic identities. GitHub's current
version treats case, repeated whitespace, and the order of simple unquoted
terms as equivalent while preserving repeated terms. Quoted phrases,
qualifiers, exclusions, Boolean syntax, punctuation, and unreviewed Unicode
remain syntax-distinct. At most one active product KPI query may use a given
semantic identity on one provider surface. Raw query text always remains
append-only replay evidence. A GitHub raw query cannot contain a backtick,
because the ledger uses a single-backtick code span to project it exactly, or a
pipe or line separator, because the ledger uses unescaped pipes and one physical
line per table row. An existing active or diagnostic query may make only one
lifecycle transition: to `retired`, with a non-null retirement timestamp
strictly after its final observation on that provider surface. Every identity,
intent, KPI, alias, rationale, activation, and surface field remains unchanged
during retirement.

The active GitHub product-acquisition set is:

- `ahead-of-time compiler python`;
- `typed Python compiler`;
- `python aot compiler`;
- `compile python native binary`;
- `typed python aot compiler`;
- `python 3.14 compiler`;
- `compile python to native binary`.

`pycc`, the explicit description/topic queries, and `python llvm compiler`
remain diagnostics. `AI-native compiler` is a retired authorship diagnostic:
its old rows are preserved, but it is not polled or counted as compiler-product
acquisition. On Google, the processed Search Console row for
`python aot compiler` is product-intent evidence on the separate `google_web`
surface; it is never combined with GitHub rank.

Starting with registry-era GitHub repository observations, each snapshot must
record the query ID, raw request parameters, provider/surface, result window
and sort contract, returned/API totals, target rank, `incomplete_results`, and
an `ordered-corpus SHA-256` in the registry's `measurements` array. The checker
requires an exact structured measurement for every history row at or after
`registry_activated_at`, rejects orphaned metadata, and verifies the projected
rank and totals. A measurement must be at or after its query's activation and
strictly before retirement; a retired query cannot be polled again. Every
accepted append must advance the protected row count and digest to the complete
committed history by appending a checkpoint to
[SEARCH_VISIBILITY_CHECKPOINTS.json](./SEARCH_VISIBILITY_CHECKPOINTS.json).
Existing checkpoints are never replaced: each one verifies its own historical
prefix before a later full-history digest can be accepted. The same checkpoint
sequence is projected into [ROADMAP.md](./ROADMAP.md): regular head CI binds the
projection to this ledger through the staged checker. After trust-anchor
activation, the required base-owned `pull_request_target` audit rejects
deletion or rewriting of any checkpoint already trusted on the base branch. A
pre-registry snapshot may state that a field was not captured;
it must not invent one. All imported rows below retain their original
six-column shape and are protected as immutable prefixes by the checker.

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
| 2026-07-25T20:21:57Z | `pycc` | >50 | — | 50 | 363 |
| 2026-07-25T20:21:57Z | `Python 3.14 compiler in:description` | 12 | 0 | 16 | 16 |
| 2026-07-25T20:21:57Z | `ahead-of-time compiler python` | 8 | 0 | 11 | 11 |
| 2026-07-25T20:21:57Z | `typed Python compiler` | >50 | — | 50 | 316 |
| 2026-07-25T20:21:57Z | `python aot compiler` | 19 | 0 | 28 | 28 |
| 2026-07-25T20:21:57Z | `compile python native binary` | 22 | 0 | 38 | 38 |
| 2026-07-25T20:21:57Z | `AI-native compiler` | 48 | 0 | 50 | 95 |
| 2026-07-25T20:21:57Z | `topic:aot-compiler` | 14 | +1 | 18 | 18 |
| 2026-07-25T20:21:57Z | `topic:python-compiler` | 43 | 0 | 44 | 44 |
| 2026-07-29T15:46:34Z | `typed python aot compiler` | 2 | — | 3 | 3 |
| 2026-07-29T15:46:34Z | `python 3.14 compiler` | 12 | — | 17 | 17 |
| 2026-07-29T15:46:34Z | `ahead-of-time compiler python` | 8 | 0 | 11 | 11 |
| 2026-07-29T15:46:34Z | `python aot compiler` | 19 | 0 | 28 | 28 |
| 2026-07-29T15:46:34Z | `compile python to native binary` | >50 | — | 24 | 24 |
| 2026-07-29T15:46:34Z | `compile python native binary` | 22 | 0 | 40 | 40 |
| 2026-07-29T15:46:34Z | `typed Python compiler` | >50 | — | 50 | 325 |
| 2026-07-29T15:46:34Z | `pycc` | >50 | — | 50 | 364 |
| 2026-07-29T15:46:34Z | `python llvm compiler` | >50 | — | 50 | 219 |
| 2026-07-29T15:46:34Z | `topic:aot-compiler` | 9 | +5 | 17 | 17 |
| 2026-07-29T15:46:34Z | `topic:python-compiler` | 31 | +12 | 44 | 44 |
| 2026-07-30T00:52:41Z | `typed python aot compiler` | 2 | 0 | 3 | 3 |
| 2026-07-30T00:52:41Z | `python 3.14 compiler` | 12 | 0 | 17 | 17 |
| 2026-07-30T00:52:41Z | `ahead-of-time compiler python` | 8 | 0 | 11 | 11 |
| 2026-07-30T00:52:41Z | `python aot compiler` | 19 | 0 | 28 | 28 |
| 2026-07-30T00:52:41Z | `compile python to native binary` | >50 | — | 24 | 24 |
| 2026-07-30T00:52:41Z | `compile python native binary` | 22 | 0 | 40 | 40 |
| 2026-07-30T00:52:41Z | `typed Python compiler` | >50 | — | 50 | 326 |
| 2026-07-30T00:52:41Z | `pycc` | >50 | — | 50 | 364 |
| 2026-07-30T00:52:41Z | `python llvm compiler` | >50 | — | 50 | 218 |
| 2026-07-30T00:52:41Z | `topic:aot-compiler` | 8 | +1 | 17 | 17 |
| 2026-07-30T00:52:41Z | `topic:python-compiler` | 31 | 0 | 44 | 44 |

The two 2026-07-29/30 snapshots used the public REST endpoint, default
best-match order, `per_page=50`, no `user:` or `repo:` qualifier, and returned
`incomplete_results=false` for every request. Their ordered-corpus fingerprints
were not retained before the registry contract existed and are therefore
recorded as unavailable rather than reconstructed.

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
| 2026-07-25T20:23:25Z | 2026-07-24 | 74 / 1 | 1,444 / 349 | 0 / 1 / 0 | `github.com`: 5 / 1 unique; `rotnov.github.io`: 2 / 1 unique |

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
| 2026-07-29T10:25:27Z | No new URL Inspection was run; all 5 canonical URLs retain the latest positive per-URL evidence above | Search Console still reports the submitted `/sitemap.xml` as failed to process with 0 discovered pages, while the public resource remains independently valid; investigation continues in #193 | Maintainer-attested processed web data for 2026-07-23 through 2026-07-26 reports 15 impressions, 2 clicks, 13.3% CTR, and average position 5.7. The only disclosed query row is `python aot compiler`: 0 clicks, 3 impressions, average position 6.3. Low-volume rows are withheld, so the clicks cannot be attributed to named queries; #163 owns stronger immutable evidence. |

## Change log

Record meaningful discovery-surface changes separately from observations so a
later movement can be correlated without presenting correlation as causation.

| Changed at (UTC) | Surface | Change |
|---|---|---|
| 2026-07-25T10:28:25Z | GitHub repository metadata | Kept the honest ahead-of-time positioning, added the standard `AOT` abbreviation to the description, and added the focused `typed-python`, `python-314`, and `llvm-compiler` topics |
| 2026-07-25T12:04:00Z | Public evidence site | Added a crawlable, source-backed Python AOT compiler comparison that distinguishes language, artifact, runtime, and maturity models while warning that pycc is not production-ready |
| 2026-07-25T13:21:53Z | Public evidence site | Expanded the source-backed comparison to LPython after current public web results exposed a missing typed-Python/AOT model; the new claims cite official project and repository documentation, and the landing-page hero now stays within narrow mobile viewports |
| 2026-07-25T14:58:50Z | Repository and public evidence site | Synchronized README, landing, status, architecture, comparison, Markdown, and `llms.txt` with the implemented v0.1 frontend from PR #86 while preserving the slice-only native backend boundary; added validator mutations so a structurally valid site cannot silently retain superseded compiler-status copy, and made qualified prose identifiers wrap after browser QA exposed 320-pixel document overflow |
| 2026-07-30T14:14:24Z | Search measurement contract | Separated product-acquisition queries from brand/topic/authorship diagnostics, retired `AI-native compiler` without deleting its history, added provider-specific semantic identities and append-only prefix checkpoints, and staged the byte-pinned evidence bootstrap before trust-anchor activation |

## Current interpretation

At the latest comparable GitHub observation, product acquisition is rank 2 of
3 for `typed python aot compiler`, rank 12 of 17 for `python 3.14 compiler`,
rank 8 of 11 for `ahead-of-time compiler python`, rank 19 of 28 for
`python aot compiler`, and rank 22 of 40 for `compile python native binary`.
The repository is absent from the complete 24-result
`compile python to native binary` corpus and remains outside the first 50 for
the broad `typed Python compiler` query. Those corpora and ranks are reported
together; rank 2 of 3 is not treated as broad demand.

Brand and metadata diagnostics remain separate: `pycc` and
`python llvm compiler` are outside the first 50, while the topic diagnostics
are ranks 8 and 31. `AI-native compiler` is a retired authorship diagnostic and
was intentionally not polled in the 2026-07-29/30 product series. Historical
authorship rows remain evidence of the old experiment, not product progress.
All five canonical URLs now have positive URL Inspection evidence: the four
pre-existing URLs were on Google at 10:29 UTC, and the live
`/python-aot-compilers/` page was on Google at 16:43 UTC with a successful
mobile crawl, matching canonical, valid HTTPS, and valid breadcrumb. The
earlier accepted request for the noncanonical `404` path remains only a
historical correction and is not credited for this result. Processed Search
Console data now contains 15 impressions and 2 clicks; its only disclosed
query row, `python aot compiler`, has 3 impressions, 0 clicks, and average
position 6.3. That small disclosed row is product-intent evidence, not a stable
rank or proof of demand, and the withheld rows prevent attribution of the two
clicks. Sitemap processing remains unsuccessful even though the public sitemap
is valid, reinforcing that query performance, per-URL indexing, and sitemap
processing are independent signals. The traffic window remains too
automation-heavy and low-uniqueness to attribute to SEO.
