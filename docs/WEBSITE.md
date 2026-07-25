# pycc Website and Search Discoverability

The public project website is a static, dependency-free evidence hub rooted at
<https://rotnov.github.io/pycc/>. Its source lives in `site/`; GitHub Pages
publishes that directory through `.github/workflows/pages.yml`.

## Purpose

The website gives search engines and prospective contributors one stable,
indexable explanation of pycc:

- pycc is an ahead-of-time compiler for typed, standard Python 3.14;
- the intended output is a standalone native binary;
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
`/python-aot-compilers/`, and `/ai-native/`; it records `lastmod` when main
content, structured data, or important links materially change. The social
preview is `site/og.png`.
`scripts/check-site.sh` enforces these mechanical requirements locally and in
the Pages workflow.
`scripts/test-check-site.sh` proves that the validator accepts the complete
site and rejects missing evidence pages, wrong canonicals, incomplete sitemaps,
missing official comparison sources, missing LPython alpha-status evidence, a
missing pre-alpha comparison warning, or required metadata. The self-test
independently removes LPython's official project source and alpha positioning
so a newly covered compiler model cannot silently disappear. The landing-page
contract also requires exactly one
relative `styles.css` stylesheet link and exactly one deferred, executable
classic-script reference to relative `site.js` with no `type` override.
The stylesheet tag permits only `rel="stylesheet"` and `href="styles.css"`;
the non-self-closing script tag permits only `defer` and `src="site.js"`.
References inside inert `template` or `noscript` content do not satisfy the
contract. All `link` and `script` elements are rejected anywhere inside SVG or
MathML subtrees because foreign scripts use different URL attributes and HTML
integration points can change descendant namespaces. Valid self-closing void
and non-asset foreign-content elements remain accepted.
Table-driven negative controls cover missing,
empty, duplicate, absolute, local-only, and differently targeted asset
references so the uploaded files cannot silently become browser-orphaned.
The validator also rejects suppressing or execution-changing asset attributes,
duplicate asset attributes, and HTML `base` elements, which could otherwise
make browser behavior disagree with the checked attribute values.
The landing-page hero grid lets both children shrink so the code example does
not widen the document beyond a narrow viewport. Browser QA at 320 and 390 CSS
pixels confirmed that the document width remains equal to the viewport while
wide comparison tables scroll only inside their containers. At viewports up to
680 CSS pixels, the footer must stack into one grid column and its navigation
group must wrap within the available width; the validator and an independent
negative mutation preserve that footer contract as the evidence-page link set
grows.

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
