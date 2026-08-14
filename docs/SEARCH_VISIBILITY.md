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
version normalizes ASCII case and repeated whitespace before classifying every
query, including Boolean operators, and also treats the order of simple
unquoted terms as equivalent while preserving repeated terms. Quoted phrases,
qualifiers, exclusions, Boolean syntax,
punctuation, and unreviewed Unicode remain syntax-distinct after that baseline
normalization. At most one active product KPI query may use a given
semantic identity on one provider surface. Raw query text always remains
append-only replay evidence. A GitHub raw query cannot contain a backtick,
because the ledger uses a single-backtick code span to project it exactly, or a
pipe or line separator, because the ledger uses unescaped pipes and one physical
line per table row. HTML tag and comment syntax is also forbidden in raw GitHub
queries because the fail-closed ledger parser rejects those constructs before
reading its machine-owned Markdown table. Unicode control and formatting
characters in the `Cc` and `Cf` categories, including NUL, tabs, bidirectional
overrides, and zero-width formatting, are forbidden because CommonMark can
replace, expand, reorder, or invisibly render them instead of preserving exact
raw query text. An existing active or diagnostic query may make only one
lifecycle transition: to `retired`, with a non-null retirement timestamp
value. For GitHub, that timestamp must be strictly after the final history
observation and structured measurement. The registry has no Google snapshot
series yet, so Google retirement has only activation-order and trusted audit
clock bounds. Every identity, intent, KPI, alias, rationale, activation, and
surface field remains unchanged during retirement.

Intent and KPI roles use one closed compatibility matrix. `product_category`,
`category_version`, and `task_output` are `product_acquisition`; `brand`,
`metadata_diagnostic`, `topic_diagnostic`, and `competitive_category` are
`diagnostic`; `authorship_narrative` is `excluded`. An unrecognized intent or
any other pairing is invalid and cannot enter the acquisition denominator.

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

The [machine-readable GitHub traffic artifact](./GITHUB_TRAFFIC_OBSERVATIONS.json)
is the structured source of truth for every traffic observation. It reuses the
same sanitized immutable evidence envelope as the Search Console artifact:
`artifact_version`, `provenance`, `observations`, and `latest_projection`. Each
observation preserves `observed_at` in UTC, the requested GitHub API version,
the data-through date, endpoint totals for views and clones with `count` and
`uniques` separate, every returned daily views and clones row, returned popular
referrers and popular paths, repository stars/forks/watchers as separate
point-in-time counters, and an explicit `collection_status` so unavailable or
unauthorized data is `unknown`, never zero. Observations are append-only: if
GitHub later backfills a previously returned day, a new snapshot is stored and
the older response is preserved. Comparisons are derived by matching identical
UTC calendar dates across snapshots — never by adding or subtracting aggregate
rolling totals to infer lifetime or daily traffic. Daily `count` rows reconcile
to the endpoint total; daily `uniques` are not additive because the same actor
can appear on multiple days, so the endpoint's rolling `uniques` value remains
authoritative. The current clone series is labeled `automation-heavy /
unattributed` and is not claimed as SEO lift without query or referrer evidence
that supports it. `scripts/check_github_traffic_observations.py` (with mutation
tests in `scripts/test_check_github_traffic_observations.py`) validates the
artifact format, the history table binding, and the prose projection, and
rejects forbidden wording that equates clones with humans, visits, clicks, or
SEO acquisition.

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
| 2026-07-29T10:25:27Z | No new URL Inspection was run; all 5 canonical URLs retain the latest positive per-URL evidence above | Search Console still reports the submitted `/sitemap.xml` as failed to process with 0 discovered pages, while the public resource remains independently valid; investigation continues in #193 | Maintainer-attested processed web data for 2026-07-23 through 2026-07-26 reports 15 impressions, 2 clicks, 13.3% CTR, and average position 5.7. The only disclosed query row is `python aot compiler`: 0 clicks, 3 impressions, average position 6.3. Low-volume rows are withheld, so the clicks cannot be attributed to named queries; #163 owns stronger immutable evidence. The structured artifact now preserves page, country, device, date, and search-appearance dimension tables separately from the property aggregate: the page table displays 19 page-level impressions across 4 rows (property total 15), the device table shows 6 mobile and 9 desktop impressions, the country table shows 5 US impressions plus 10 across 8 other countries, the date table shows 0 impressions on 2026-07-23, 1 on 2026-07-24, 5 on 2026-07-25, and 9 on 2026-07-26, and search appearance reports no data. These marginals are independent observations and must not be joined into synthetic multi-dimensional rows. |

## Page indexing aggregate history

The Search Console **Page indexing** report is a report-level dataset, distinct
from URL Inspection, live URL tests, sitemap processing, HTTPS aggregates,
performance data, and search appearance. Google documents that the report's
totals describe the report state, not the current index status of any specific
URL, and warns not to expect immediate indexing. The report exposes a
report-level "last updated" date and example-level dates that must be preserved
exactly as displayed; they are never collapsed into a single timestamp.

The [machine-readable artifact](./SEARCH_CONSOLE_OBSERVATIONS.json) preserves
each Page indexing aggregate snapshot in a separate append-only
`page_indexing_observations` series, reusing the same sanitized provenance
envelope as the per-URL observation series. Each snapshot records the verified
property, collection timestamp, UI/API transport and locale, selected page
scope or sitemap filter, report last-updated date, report-level totals,
filters, data freshness state, and each reason row independently — exact
reason, source, validation state, affected-page count, first-detected date,
displayed examples with their last-crawl dates, and row/example limits with
sampled/truncated flags.

Cross-report reconciliation rules:

- A Page indexing aggregate does not overwrite a newer per-URL inspection.
- A per-URL inspection does not rewrite the historical aggregate snapshot.
- Performance proves an appearance in its measured period, not permanent
  current index membership.
- Public HTTP/canonical/sitemap checks are implementation evidence, not
  substitutes for owner-facing index state.
- A stale contradiction is represented explicitly as
  `report_lag_or_unreconciled`, not silently "fixed" by selecting one number.
- The Page indexing aggregate is separate from #193's sitemap-processing
  state; neither report proves or causes the other.
- "Validate fix" is not started for a Google-systems reason unless a current
  diagnosis identifies a deployed site-side change and the validation action
  is bounded and recorded.

| Collected at (UTC) | Report last updated | Indexed | Not indexed | Data freshness | Reason | Source | Validation | Affected | First detected | Example URL | Example last crawl |
|---|---|---:|---:|---|---|---|---|---:|---|---|---|
| 2026-07-29T11:59:42Z | 2026-07-24 | 4 | 1 | report_lag_or_unreconciled | Crawled — currently not indexed | Google systems | not_started | 1 | 2026-07-25 | `https://rotnov.github.io/pycc/python-aot-compilers/` | 2026-07-25 |

The 2026-07-29 baseline preserves the lagging 4 indexed / 1 not indexed
aggregate exactly as displayed. The example URL is the comparison page whose
fresh URL Inspection reports it as indexed, so the aggregate lags behind the
per-URL evidence; this is recorded as `report_lag_or_unreconciled`, not as a
live-site regression. The displayed examples are a sample, not a complete
lifetime URL inventory.

## Engine-qualified visibility

Indexability, public result visibility, and answer-engine citation are
different signals and must remain separate. A successful HTTP fetch proves
crawl access, not ranking. Search Console owner data proves Google indexing
and performance, not ChatGPT citation. `llms.txt` is a concise
machine-readable content map, not a claimed ranking lever. One volatile
answer-engine result is not a stable position.

The [machine-readable engine-visibility artifact](./ENGINE_VISIBILITY_OBSERVATIONS.json)
is the structured source of truth for engine-qualified web-search and
LLM answer-engine visibility observations. It reuses the same sanitized
immutable evidence envelope as the Search Console and GitHub traffic
artifacts: `artifact_version`, `provenance`, `observations`, and
`latest_projection`. It also defines a `query_suite` with a stable
prompt/query set and a `surfaces` map with explicitly qualified engine
contracts.

### Surfaces

Each observation must name an explicitly qualified surface. The supported
surfaces are:

- `chatgpt_search` — the ChatGPT web-search surface (OpenAI);
- `google_web_search` — Google web search (distinct from Search Console
  owner data);
- `bing_web_search` — Bing web search (Microsoft);
- `perplexity_search` — Perplexity answer engine;
- `unknown_web_provider` — an unidentified web-search provider, used only
  when provenance is not disclosed. `unknown_web_provider` must never be
  labeled as Google, Bing, or another named engine.

A web observation without a defined surface/provider is invalid.

### Query suite

The stable prompt/query suite covers five intent classes:

- exact entity (`rotnov/pycc`) — diagnostic, never a product-acquisition KPI;
- named product (`pycc Python AOT compiler`) — product acquisition;
- product category (`python aot compiler`) — product acquisition;
- feature intent (`typed Python compiler native binary`) — product
  acquisition;
- AI-authorship diagnostic (`"fully AI-created" "pycc"`) — excluded from
  the product-acquisition KPI, never counted as product progress.

The intent/KPI compatibility matrix is closed: an unrecognized intent or
any other pairing is invalid and cannot enter the acquisition denominator.

### Outcome vocabulary

Each observation records one outcome from a separate vocabulary that
distinguishes indexability from visibility from citation:

- `crawlable` — the resource is reachable by a crawler;
- `indexed_owner_visible` — owner-visible indexing evidence (e.g. Search
  Console URL Inspection);
- `returned_in_result_set` — the project appeared in the observed result
  window;
- `cited_in_answer` — an answer engine cited the project with a captured
  URL;
- `not_surfaced_in_observed_window` — the project did not appear in the
  observed result or answer window;
- `unknown` — the outcome could not be determined.

`crawlable` and `indexed_owner_visible` are indexability signals, not
citation evidence. They must never be treated as `cited_in_answer`. A
`cited_in_answer` outcome requires at least one captured cited URL.

### Observation fields

Every answer/search observation is stored append-only with:

- `observed_at` UTC timestamp;
- `query_suite_id` referencing the stable query suite;
- `surface` naming the explicitly qualified engine;
- `prompt` — the exact user prompt or query;
- `model_product_version` — model/product/version when exposed, or
  `unknown`;
- `locale` and `country` — when known, or `unknown`;
- `personalization_state` — signed-in/personalization state when known, or
  `unknown`;
- `result_window` — description of the result/citation window observed;
- `outcome` — one of the vocabulary above;
- `pycc_surfaced` — whether pycc appeared in the observed window;
- `cited_urls` — cited URL(s) when surfaced with a citation outcome;
- `citation_order` — citation/source order when surfaced, or `null`;
- `competing_entities` — returned competing entities when absent;
- `transport_tool` — transport/tool used for the observation;
- `query_rewriting_disclosed` — `yes`, `no`, or `unknown`.

Unavailable fields use `unknown` (or `null` for `citation_order`) rather
than invented values. Observations are append-only: timestamps must be
nondecreasing and earlier observations are never rewritten or deleted.

### Current state

The artifact is a template with no engine-qualified visibility observations
recorded yet. The 2026-07-29 ChatGPT search baseline described in issue #195
is preserved in that issue's body, not in this artifact, because it predates
the contract and was not captured under this schema. When real manual
captures are appended, they must pass the same local schema validator
(`scripts/check_engine_visibility_observations.py`) before they enter the
ledger. Repeated observations on a bounded cadence are required before
changing public positioning solely because of answer-engine output.

## Pages visit measurement

The three discovery signals above — GitHub repository search, GitHub
Traffic, Google Search Console, and engine-qualified visibility — do not
measure visits to the **GitHub Pages site** from Yandex, DuckDuckGo,
Perplexity, ChatGPT, other answer engines, ordinary referrals, or direct
navigation. The five-page production artifact has no pageview/visit
analytics integration, and the project has no owner-facing Pages request
log. This leaves a material measurement gap: the project cannot tell
whether a Yandex rank or an answer-engine citation produced a visit,
which page became the entry page, or whether a visitor continued to
GitHub (issue #208, [D-168](./decisions/D-168-pages-visit-measurement-capability-contract.md)).

The [machine-readable Pages visit artifact](./PAGES_VISIT_OBSERVATIONS.json)
is the structured source of truth for owner-facing GitHub Pages visit
analytics. It reuses the same sanitized immutable evidence envelope as
the Search Console, GitHub traffic, and engine-visibility artifacts:
`artifact_version`, `provenance`, `observations`, and
`latest_projection`. It also carries a `measurement_contract` block that
defines the reporting timezone, canonical pages, primary conversion,
source-class vocabulary, collection-status vocabulary, data-minimization
boundary, and separation rules.

### Current analytics decision

The current explicit analytics decision is **keep no site analytics**
(D-168). The site has no project-selected analytics script, cookie, or
external beacon. The roadmap and this document state that non-Google
landing visits remain unobservable. "Keep no site analytics" is a valid
explicit decision; a future PR that activates analytics must record a
superseding ADR that justifies the visitor-data, retention, and
external-service policy, add an accurate public privacy/analytics
disclosure before or with collection, and establish the activation
baseline at deployment time without synthesizing pre-installation
history.

### Measurement contract

The minimum measurement contract, defined before choosing fields:

- **Timestamp/date and reporting timezone**: `observed_at` in UTC and
  `data_through_date` in `YYYY-MM-DD` form; `reporting_timezone` is UTC.
- **Canonical entry page**: each observation records a `per_page`
  breakdown keyed by canonical URL.
- **Pageviews and visit/unique as separate metrics**: `pageviews`,
  `visits`, and `uniques` are separate fields; `unique_definition`
  records the provider's exact unique-visitor definition so it is not
  silently compared with GitHub's `uniques` or another provider's.
- **Coarse referrer/source class**: `source_classes` uses a closed
  vocabulary (`google`, `yandex`, `bing`, `duckduckgo`, `perplexity`,
  `chatgpt`, `other_answer_engine`, `other_search`, `referral`,
  `direct_or_unattributed`, `unknown`). A referrer domain supports a
  coarse source classification; it does not expose the search query.
  Missing referrer data is `direct_or_unattributed`, not proof of direct
  navigation.
- **Coarse country and device**: `country` and `device` dimensions are
  permitted only if supplied by the provider without expanding the
  chosen privacy boundary.
- **Primary conversion**: a click from a canonical Pages page to
  `https://github.com/rotnov/pycc`, recorded as
  `primary_conversion_clicks`. This is the only instrumented interaction;
  no speculative per-interaction tracking.
- **Collection status**: `available`, `delayed`, `blocked`,
  `unauthorized`, `provider_error`, or `unknown`. A non-zeroable status
  (`blocked`, `delayed`, `unauthorized`, `provider_error`, `unknown`)
  must never be converted to zero pageviews or zero visits; unavailable
  data is `null`, not `0`.

### Data-minimization boundary

Unless a separately accepted decision justifies them, the artifact must
not collect or retain: names, email/account identifiers, form contents,
full IP addresses, full user agents, cookies, fingerprints, persistent
cross-site IDs, session replay, arbitrary query strings/fragments, or
raw search queries not supplied by a provider-owned search report. The
`data_minimization_boundary` block in the artifact records this
forbidden-fields list and a note explaining the scope.

### Separation rules

This source is separate from Google Search Console, GitHub repository
traffic, and direct-provider SERP/answer observations. Those systems
must never be joined at a person/session level, and no cross-provider
funnel may be manufactured from marginal totals. A provider-specific
unique visitor is not comparable with GitHub's `uniques` or another
provider unless definitions are proven equivalent. A Pages visit after a
Yandex or other engine observation is correlation only unless the
analytics source exposes a corresponding referrer or UTM signal.
Browser blocking, script failure, network loss, and provider filtering
make client-side analytics incomplete; counts must not be presented as a
census. Analytics must not be described as a ranking factor or as
evidence that `llms.txt`, schema, or a content edit caused visibility.

### Current state

The artifact is a template with no Pages visit observations recorded
yet. When a privacy-scoped analytics source is activated and real data
is collected, observations must pass the local schema validator
(`scripts/check_pages_visit_observations.py`) before they enter the
ledger. The validator rejects a non-zeroable collection status converted
to zero, an invalid source class, a non-append-only observation
sequence, a `latest_projection` that disagrees with the latest
observation, and prose that conflates repository views with Pages visits
or Search Console clicks with all-provider visits.

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

| Exact query | Latest observed at (UTC) | Rank | Results | Total |
|---|---|---:|---:|---:|
| `ahead-of-time compiler python` | 2026-07-30T00:52:41Z | 8 | 11 | 11 |
| `AI-native compiler` | 2026-07-25T20:21:57Z | 48 | 50 | 95 |
| `compile python native binary` | 2026-07-30T00:52:41Z | 22 | 40 | 40 |
| `compile python to native binary` | 2026-07-30T00:52:41Z | >50 | 24 | 24 |
| `pycc` | 2026-07-30T00:52:41Z | >50 | 50 | 364 |
| `python 3.14 compiler` | 2026-07-30T00:52:41Z | 12 | 17 | 17 |
| `Python 3.14 compiler in:description` | 2026-07-25T20:21:57Z | 12 | 16 | 16 |
| `python aot compiler` | 2026-07-30T00:52:41Z | 19 | 28 | 28 |
| `python llvm compiler` | 2026-07-30T00:52:41Z | >50 | 50 | 218 |
| `topic:aot-compiler` | 2026-07-30T00:52:41Z | 8 | 17 | 17 |
| `topic:python-compiler` | 2026-07-30T00:52:41Z | 31 | 44 | 44 |
| `typed python aot compiler` | 2026-07-30T00:52:41Z | 2 | 3 | 3 |
| `typed Python compiler` | 2026-07-30T00:52:41Z | >50 | 50 | 326 |

The table is a machine-checked projection of the latest accepted history row
for every exact GitHub query; a new snapshot cannot merge while this view is
stale. Product-acquisition and diagnostic corpora remain separate in the
registry even though the projection reports them together. A high rank in a
narrow corpus is not treated as broad demand. `AI-native compiler` remains a
retired authorship diagnostic: its historical row is evidence of the old
experiment, not product progress, and it is not polled in the current product
series.
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
clicks. The structured artifact preserves 19 page-level impressions across 4
page rows separately from the 15-impression property total: Search Console
counts the property-level result once, while page-level aggregation counts
each unique URL, so the page sum legitimately exceeds the property total. The
device marginal shows both clicks on mobile (6 mobile impressions, 9 desktop
impressions), and the country marginal shows both clicks from the United States
(5 US impressions, 10 across 8 other countries). The date marginal shows 0
impressions on 2026-07-23, 1 on 2026-07-24, 5 on 2026-07-25, and 9 on
2026-07-26. Search appearance reports no data, which is categorical absence
rather than a numeric zero. These dimension tables are independent marginal
observations and must not be joined into synthetic page×country,
page×device, or query×country rows; the individual page, query, country, and
device identity of each click remains unknown. Sitemap processing remains unsuccessful even though the public sitemap is valid, reinforcing that query
performance, per-URL indexing, and sitemap processing are independent signals.
The lagging Page indexing aggregate (report last updated 2026-07-24, collected
2026-07-29T11:59:42Z) still shows 4 indexed and 1 not indexed, with the single
not-indexed reason row "Crawled — currently not indexed" (source: Google
systems, validation not started, 1 affected page, first detected 2026-07-25)
whose displayed example is the comparison page
`https://rotnov.github.io/pycc/python-aot-compilers/` (example last crawl
2026-07-25). That example URL is the same page whose fresh URL Inspection
reports it as indexed, so the aggregate lags behind the per-URL evidence; this
is recorded as `report_lag_or_unreconciled`, not as a live-site regression or a
reason to request indexing again or click "Validate fix". The aggregate count
of 4 indexed does not overwrite the five positive per-URL inspections, and the
per-URL inspection does not rewrite the historical aggregate snapshot. The
displayed examples are a sample, not a complete lifetime URL inventory, and the
report, crawl, inspection, and performance dates are preserved separately
rather than collapsed into one timestamp.
The traffic window remains too
automation-heavy and low-uniqueness to attribute to SEO. The structured
GitHub traffic artifact ([`GITHUB_TRAFFIC_OBSERVATIONS.json`](./GITHUB_TRAFFIC_OBSERVATIONS.json))
preserves daily views and clones rows alongside the rolling 14-day endpoint
totals; clone activity is not treated as human discovery and is not claimed as
SEO lift without query or referrer evidence that supports it.
