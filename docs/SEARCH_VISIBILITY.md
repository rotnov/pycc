# Search visibility and launch readiness evidence

This document is the human-readable projection of the structured
external-mention evidence record and the launch readiness gate defined
in issue #209. It is generated from `site/search-visibility/evidence.json`
and must stay consistent with it.

## Evidence model

Every external mention observation is recorded as an append-only entry
in the structured evidence file. Entries are never deleted or rewritten;
later observations add new entries that may supersede earlier ones.

Each entry records:

- `observed_at` — UTC timestamp of the observation.
- `discovery_surface` — where the observation was made (e.g. `github_code_search`, `github_issue_search`, `google_web_search`, `search_console_links`, `github_traffic_referrers`).
- `query` — the exact bounded query or export used.
- `result_count` — number of rows returned (0 is a window result, not a universal zero).
- `result_type` — `rows`, `empty_window`, `processing`, or `unavailable`.
- `entries` — individual mention records (when rows were returned).
- `sample_limitation` — known limitations of the observation (e.g. "sampled, non-comprehensive").
- `source_limitation` — provider-stated limitations (e.g. "Links report is sampled, grouped, and truncated").

## Mention classification

Each mention is classified as exactly one of:

- `owned` — on a project-owned surface (repository, Pages site, etc.).
- `self_authored_external` — written by the repository owner outside the project (e.g. upstream bug reports).
- `independent_editorial` — an independent third-party review, article, or recommendation.
- `community` — a community forum, Q&A, or discussion post.
- `directory` — a directory or package index listing.
- `automated_mirror` — an automated mirror or copied README.
- `spam` — a link placed primarily to manipulate rankings.
- `unknown` — classification not yet determined.

Self-authored external references (including upstream bug reports by
the repository owner) are **not** independent editorial citations and
must not increment an earned-authority count.

## Launch readiness gate

The launch gate is `closed` until **all** of the following are true:

1. Release/tag/roadmap status is reconciled (no open release-consistency issue).
2. The flagship quick start is executable and tested.
3. The landing and status pages agree on current behavior and pre-alpha limitations.
4. Canonical URLs and live links pass their existing checks.
5. The launch text names the actual product category (AOT compiler for typed Python) and does not position pycc as an AI compiler.
6. The message discloses pre-alpha readiness and AI-created/human-managed provenance without turning provenance into the product keyword.

The gate status is recorded in the structured evidence file and must
not be opened by an automated agent without explicit human authorization.

## Current state (2026-08-12)

- **Launch gate**: `closed`
- **Independent editorial mentions observed**: 0
- **Self-authored external references observed**: 3 (upstream `ievo-ai/skills` bug reports #432, #446, #449)
- **Search Console Links report**: processing (sampled, non-comprehensive)
- **GitHub Traffic referrers**: only `github.com` and `rotnov.github.io` (rolling top list, not exhaustive)

These are bounded window results, not universal zeros. The absence of
observed independent mentions does not prove zero backlinks exist.

## Distribution checklist

Every candidate community, directory, or publication must have:

- An audience fit explanation.
- Current posting/submission rules.
- A useful audience-facing artifact to share.
- A named human approval point before any external action.
- A non-promotional fallback when self-submission is prohibited.

Automated agents must not post, comment, vote, star, review, or create
accounts. The repository may prepare evidence and copy, but external
publication requires explicit human authorization for that destination
and message.
