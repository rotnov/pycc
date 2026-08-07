# pycc Website and Search Discoverability

The public project website is a static, dependency-free evidence hub rooted at
<https://rotnov.github.io/pycc/>. Its source lives in `site/`; GitHub Pages
publishes that directory through `.github/workflows/pages.yml`.

## Purpose

The website gives search engines and prospective contributors one stable,
indexable explanation of pycc:

- pycc is an ahead-of-time compiler for typed, standard Python 3.14;
- native and `--pure` output is a standalone native binary, while planned
  permitted CPython interop produces an autonomous bundle carrying the pinned
  interpreter and dependency closure (D-128);
- the implementation is written in Rust and uses LLVM;
- the project is created entirely by AI agents, while a human manages goals,
  constraints, priorities, and product decisions without writing project code;
- the project is pre-alpha, and design targets are not presented as released
  features.

The canonical landing page links to four crawlable evidence pages:

- `/status/` describes implemented behavior, enforced gates, remaining v0.1
  scope, and the next planned delivery slice;
- `/architecture/` separates the working compiler path and current crates from
  the target typed-Python architecture;
- `/python-aot-compilers/` maps pycc, LPython, Codon, Nuitka, mypyc, and
  Cython by language contract, output artifact, runtime model, and current
  positioning using each project's official documentation, with no unsupported
  benchmark claims;
- `/ai-native/` documents the AI-author/human-manager boundary, operating loop,
  safeguards, and public audit trail.

These pages turn repository evidence into useful, internally linked source
material for humans, conventional search engines, and retrieval systems. They
must remain concise projections of the repository specifications, not a second
independent source of product truth.

The current commit-relative boundary is deliberately explicit across the
landing, status, architecture, comparison, Markdown, and `llms.txt` surfaces:
`pycc check` owns the v0.1 parser → HIR → strict-type-checker path, including
stable human and JSON frontend diagnostics, while `pycc build` and `pycc run`
lower the same implemented language surface through MIR → LLVM → host linker →
native executable. `docs/ROADMAP.md`'s v0.1 and v0.2 acceptance criteria are
both met, and v0.3's class model core (#385) has landed; the project remains
pre-alpha because the documented representation and lifetime gaps, the full
multi-version conformance testkit, named demos, and the rest of v0.3's class
model (inheritance, `@property`, dataclasses, enums, protocols, structural
pattern matching, and custom exceptions) remain unfinished. These facts must
move with the authoritative roadmap whenever implementation depth changes; see
"Status-page freshness enforcement" below for how a stale claim like this one
is now caught mechanically instead of relying on a reviewer to notice.

The status page presents D-051/D-053/D-056/D-062's active fixed-replicate,
source-aware paired gate
independently of later compiler slices. It states that the frontend measurement
and regression gate are required through `ci-gate`, measure the exact
predecessor and candidate sequentially on one hosted runner, seal the
predecessor timing before candidate code runs, and classify complete
repository-owned executable inputs before that execution. Identical inputs
make timing non-blocking environment telemetry; changed source uses exactly
five complete runs per revision, compares the median of their per-run medians,
and keeps the hard greater-than-7.0% block (D-114). The gate fails closed on revision,
benchmark-contract, executable-input identity, artifact-identity, exact
ten-file evidence, or comparison drift.

The canonical search phrase is “ahead-of-time compiler for typed Python”.
Copy may use close, natural variants such as “Python AOT compiler” and “compile
Python to a native binary”, but must not repeat phrases solely to manipulate
ranking.

## Search metadata contract

Every canonical HTML page must provide:

- a unique title, canonical URL, and plain-language meta description;
- Open Graph and X card metadata;
- page-level crawl permission with unlimited text, image, and video previews;
- a sitemap reference.

The unsupported HTML `meta name="keywords"` field is forbidden on every
canonical page. Google and Bing do not use it as a ranking booster, and a
hidden list must not mix product-acquisition terms with AI-authorship
provenance. This zero-policy does not change the separately reviewed
`SoftwareSourceCode.keywords` property.

Descriptions are role-specific. The root is the product-acquisition landing:
its HTML, Open Graph, X, and JSON-LD descriptions put the AOT compiler for typed
Python and current pre-alpha maturity in the first semantic clause, with
AI-created/human-managed provenance second. Status, architecture, and
comparison pages describe their product-evidence role. The dedicated
authorship/provenance page may lead with the AI experiment in its title and
body, but its HTML, Open Graph, X, and JSON-LD descriptions must identify pycc
as a pre-alpha AOT compiler for typed Python before describing the authorship
experiment. These projections share material truth without requiring
byte-identical prose.

The landing page also carries the persistent Google Search Console verification
token for the `https://rotnov.github.io/pycc/` URL-prefix property and a
`WebPage` → `SoftwareSourceCode` JSON-LD graph whose descriptions match the
visible project and whose source entity links to the public repository.
Evidence pages carry a `WebPage` and two-level `BreadcrumbList`; each WebPage
references the landing-page project entity instead of duplicating it.
Structured data must match visible content and current status.

Visible page copy must also state that AI agents create the entire project, the
human role is management, and no project code is handwritten by a human. This
development-model claim is part of the public project identity, not hidden
metadata.

`site/robots.txt` and `site/sitemap.xml` must use the same canonical origin.
The sitemap lists the landing page plus `/status/`, `/architecture/`,
`/python-aot-compilers/`, and `/ai-native/`. Every sitemap URL entry contains
exactly one `lastmod`, equal to that page's JSON-LD `WebPage.dateModified`, and
both values advance when main content, structured data, or important links
materially change. The social preview is `site/og.png`.

The comparison page (`/python-aot-compilers/`) carries a structured
claim/source model in `site/python-aot-compilers/claims.json`. This JSON file
is the canonical, reviewable contract for every material comparison-table cell:
each entity record has `name`, `input_contract`, `html_output_cell`,
`positioning`, `maturity`, and a `sources` list of `{url, description}`
records. The HTML page is a human-readable projection of this model.
`scripts/check-site.sh` parses the HTML comparison table and validates every
cell against the corresponding field in `claims.json`: it verifies that the
HTML entity set exactly matches the model entity set, that each cell value
matches the expected text, that every source URL appears as an `<a href>` on
the page, that every entity has at least one source, and that `maturity` is a
non-empty string. The `maturity` field uses explicit labels from sources
(`pre-alpha`, `alpha`) or `unknown` when no explicit label is found—maturity
must never be inferred from URL segments, version numbers, or adoption. A
human or agent reviewer verifies the model against the cited sources at
model-authoring time; the validator then enforces that the HTML cannot drift
from the reviewed model. Binding is structural, not semantic: the validator
checks HTML↔model consistency and source-URL presence, not that source page
content supports each claim (semantic binding is #202's domain).

`scripts/check-site.sh` enforces these mechanical requirements locally and in
the Pages workflow.
`scripts/test-check-site.sh` proves that the validator accepts the complete
site and rejects missing evidence pages, wrong canonicals, incomplete sitemaps,
missing official comparison sources, missing LPython alpha-status evidence, a
missing pre-alpha comparison warning, or required metadata. The self-test
independently removes LPython's official project source and alpha positioning
so a newly covered compiler model cannot silently disappear. It rejects both a
duplicate sitemap `lastmod` and a value that disagrees with the corresponding
page's `WebPage.dateModified`. It also mutates
the landing, status, architecture, comparison, Markdown, and `llms.txt`
frontend/backend claims independently, preventing a structurally valid site
from silently describing a superseded compiler milestone. It also removes the
status page's paired same-runner measurement, exact-revision provenance,
predecessor-before-candidate sealing, executable-input classification,
conditional telemetry rule, changed-source hard threshold, and fail-closed
revision/contract/identity/artifact/comparison requirements independently so
the validator rejects a public performance-gate claim that no longer matches
CI. The comparison-page claim/source binding has its own mutation suite:
value-false mutations corrupt each external project's output or positioning
cell while keeping source URLs intact, proving that source-link presence alone
does not satisfy a value claim; model-HTML mismatch mutations change
`claims.json` without the HTML and vice versa, add or remove entities on one
side only, and relabel maturity in the model while the HTML disagrees; and
model integrity mutations delete `claims.json`, malform its JSON, remove an
entity's sources, and set an empty maturity. A positive control verifies that
minor whitespace changes in comparison cells do not break validation. The
landing-page contract also requires
exactly one
relative `styles.css` stylesheet link and exactly one deferred, executable
classic-script reference to relative `site.js` with no `type` override.
The stylesheet tag permits only `rel="stylesheet"` and `href="styles.css"`;
the non-self-closing script tag permits only `defer` and `src="site.js"`.
References inside inert `template` or `noscript` content do not satisfy the
contract. All `link` and `script` elements are rejected anywhere inside SVG or
MathML subtrees because foreign scripts use different URL attributes and HTML
integration points can change descendant namespaces. Valid self-closing void
and non-asset foreign-content elements remain accepted.
The same validator rejects the unsupported keywords field at head or body
depth and binds the root description family to the product, maturity, output,
and provenance tuple. Negative controls co-mutate HTML and JSON-LD
descriptions, corrupt social/source descriptions, reverse product/provenance
ordering, add unsupported benchmarks or readiness claims, and reintroduce the
keywords field on both landing and evidence pages.
Table-driven negative controls cover missing,
empty, duplicate, absolute, local-only, and differently targeted asset
references so the uploaded files cannot silently become browser-orphaned.
The validator also rejects suppressing or execution-changing asset attributes,
duplicate asset attributes, and HTML `base` elements, which could otherwise
make browser behavior disagree with the checked attribute values.
The landing-page hero grid lets both children shrink so the code example does
not widen the document beyond a narrow viewport. Browser QA at 320 and 390 CSS
pixels confirmed that the document width remains equal to the viewport while
wide comparison tables scroll only inside their containers. Prose inline code
uses `overflow-wrap: anywhere` so qualified identifiers such as
`pycc_types::check_and_resolve` cannot widen a narrow evidence page; fresh-page
browser QA confirms equal document and viewport widths for the landing, status,
architecture, and comparison pages at both representative widths. At
viewports up to 680 CSS pixels, the footer must stack into one grid column and
its navigation group must wrap within the available width; the validator and
an independent negative mutation preserve that footer contract as the
evidence-page link set grows.

## Status-page freshness enforcement

`site/status/index.html` and `site/index.html` restate `docs/ROADMAP.md`'s
current milestone and acceptance status in prose; nothing previously enforced
that they stayed in sync with it, and both pages drifted silently past the
v0.2 acceptance milestone and into v0.3 before #401 caught it (D-156).
`scripts/check_status_page_freshness.rb` closes that gap with a narrowly
scoped, two-signal check: it watches a pull request's or push's diff to
`docs/ROADMAP.md` for (1) the `**Current milestone: ...**` bold line changing,
or (2) a `<!-- roadmap-evidence: ... -->`-tagged checklist line's checked
state flipping or a new evidence-marker line being added, using the same
`EVIDENCE_MARKER` regex as `scripts/check_roadmap_evidence.rb` (duplicated
byte-for-byte, not shared via `require_relative`, so the two must be kept in
sync by hand). When
either signal fires and the same diff touches neither `site/status/index.html`
nor `site/index.html`, the check fails with a message pointing back at this
document and at [issue #401](https://github.com/rotnov/pycc/issues/401).
Full auto-generation of the status pages from `docs/ROADMAP.md` was
considered and rejected: the pages carry hand-tuned narrative (which v0.3
subset has landed, which gaps are real versus roadmap shorthand) that a
mechanical transform cannot reproduce without re-deriving the roadmap's own
editorial judgment in a second format. `scripts/test_check_status_page_freshness.rb`
is the validator's paired self-test, run directly rather than measured by
`llvm-cov`, mirroring `scripts/check_roadmap_evidence.rb`'s own pairing,
though it is wired into `.github/workflows/status-page-freshness.yml` itself
as a dedicated step rather than into `ci.yml`/`workflow-policy.yml` the way
`scripts/test_check_roadmap_evidence.rb` is. The freshness check is wired
into that same dedicated workflow, on ordinary `pull_request` and
`push`-to-`main` triggers with no `paths:` filter; see D-156 for why it is not
folded into `ci.yml` or `workflow-policy.yml`, and for the empirical
validation performed against this repository's own history before the check
was finalized. The workflow is not yet a required branch-protection check;
that registration is a deliberate, separate follow-up performed only after
the workflow is observed green on a real push-to-main run and red on a real
violating pull request.

GitHub project Pages are served below `/pycc/`, while the robots exclusion
protocol only discovers `robots.txt` at the origin root. The page-level robots
meta tag is therefore the effective crawl directive on the default
`rotnov.github.io` domain. `site/robots.txt` remains ready for a future custom
domain, and the sitemap can be submitted directly to search consoles.

Google site names are also domain- or subdomain-level and are not supported for
a project subdirectory. Do not add `WebSite` site-name markup while pycc uses
the shared `rotnov.github.io/pycc/` origin path. A dedicated custom domain is
the preferred infrastructure upgrade because it gives pycc control over the
origin-level robots policy, site identity, and search-console property.

## Generative search and LLM discovery

There is no separate markup that guarantees placement in Google AI Overviews,
AI Mode, ChatGPT, Claude, or Copilot. The project follows the public guidance
from the relevant providers:

- [Google's generative-search guide](https://developers.google.com/search/docs/fundamentals/ai-optimization-guide)
  says that core SEO, crawlability, indexability, useful original content, and
  page experience remain the ranking foundation. Google explicitly ignores
  `llms.txt` for ranking and requires no special AI schema.
- [OpenAI's publisher guidance](https://help.openai.com/en/articles/12627856-publishers-and-developers-faq)
  requires that `OAI-SearchBot` not be blocked for content to appear in
  ChatGPT summaries and snippets.
- [Bing's webmaster guidelines](https://www.bing.com/webmasters/help/webmaster-guidelines-30fba23a)
  apply the same crawl, indexing, content-quality, and authority principles to
  Bing Search, Copilot, and grounding citations.
- [Anthropic's crawler guidance](https://support.claude.com/en/articles/8896518-does-anthropic-crawl-data-from-the-web-and-how-can-site-owners-block-the-crawler)
  confirms that its bots honor origin-level robots directives.

`site/llms.txt` is maintained as a concise, experimental inference-time content
map for tools that choose to consume the emerging
[llms.txt proposal](https://llmstxt.org/). It is not described as a ranking
factor. `site/index.html.md` is the clean Markdown equivalent recommended by
that proposal. Both files link to the evidence pages, including the
source-backed compiler comparison, and must preserve the landing page's status
and AI-authorship disclosures.

The sitemap carries the standards-based discovery signal. After a successful
production deployment, `scripts/notify-indexnow.sh` parses that validated
sitemap and submits its complete canonical URL set in one IndexNow batch POST
so Bing and participating engines can discover material updates quickly. It
rejects empty, duplicate, out-of-scope, query-bearing, or fragment-bearing URLs
before making a request. Its public verification key is hosted below `/pycc/`,
which limits that key to URLs in the project path. The notification is
best-effort, has finite connection and request timeouts, and does not block a
successful Pages deployment.

`scripts/test-notify-indexnow.py` points the production notifier at a local HTTP
fixture and proves that the real non-dry-run path sends the expected JSON
payload and fails on an HTTP error without contacting a public search endpoint.
An accepted IndexNow response proves receipt only, not crawl or indexing.

Discovery is not ranking. Long-term visibility depends on publishing accurate,
non-commodity compiler evidence—implemented language features, architecture
decisions, diagnostics, benchmarks once reproducible, and release notes—and on
earning relevant references rather than manufacturing mentions, backlinks, or
search activity. Search Console, Bing Webmaster Tools, referral traffic, and
fixed query-position checks are the measurement surfaces; no position is
guaranteed. Operational monitoring must distinguish sitemap submission,
sitemap fetch/processing, URL indexing, impressions, clicks, and query position
instead of treating any one of them as proof of the others.
GitHub query and rolling traffic observations are preserved in
[SEARCH_VISIBILITY.md](./SEARCH_VISIBILITY.md) using a fixed top-50 ranking
contract and timestamped API snapshots so changes can be compared without
rewriting earlier responses or attributing automation traffic to SEO.
Its machine-readable query registry separates product acquisition from brand,
topic, competitive, and authorship diagnostics; provider-specific semantic
identities prevent aliases from inflating query coverage. The checker protects
each historical row prefix with an independently stored append-only checkpoint,
projects the checkpoint sequence into the roadmap, rejects backdated appends,
and keeps retired authorship experiments out of the product KPI. The required
base-owned workflow audits the complete protected bundle against the trusted
base without executing candidate code; the same
`scripts/check_search_visibility_audit.py` implementation and mutation suite
provide local base-versus-head validation without a weaker duplicate checker.
The same ledger records Search Console URL Inspection, sitemap-processing, and
performance-report states independently because none of those signals is a
substitute for the others.

## Publication

Pull requests that change the website or either validator run the Pages build
and validation job without deploying. Pushes to `main` that change the website
validate the static files, upload only `site/`, and deploy through the protected
`github-pages` environment. A manual run performs validation and artifact
assembly only; deployment authority is reserved for a `push` to `main`. The
workflow grants `pages: write` and OIDC access only to that deployment job;
pull-request code runs with read-only repository access. A separate read-only
post-deployment job sends the best-effort IndexNow notification.

The site deliberately has no package manager, runtime JavaScript dependency,
analytics, cookies, or external font request. A small local script only powers
the copy-to-clipboard control. Relative asset paths at both the root and
one-directory evidence-page depth keep local previews and the `/pycc/`
project-site base path working identically.

When the website's claims change, update the root README and the relevant
specification in the same patch. Search metadata must describe current or
explicitly labelled planned behavior; it must never turn roadmap goals into
present-tense product claims.
