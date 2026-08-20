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
multi-version conformance testkit, named demos, and the last of v0.3's class
model (custom exceptions) remain unfinished. These facts must
move with the authoritative roadmap whenever implementation depth changes; see
"Status-page freshness enforcement" below for how a stale claim like this one
is now caught mechanically instead of relying on a reviewer to notice.

The status page's "Planned next" section states, in one sentence, that the
`frontend-perf-gate`'s greater-than-7.0% regression floor (D-051/D-053/D-056/
D-062/D-114) stays required through all of this remaining work (v0.3's
class-model items plus the conformance-matrix, fuzzing, and corpus-testing
work carried over from v0.1/v0.2), enforced by a paired predecessor/candidate
measurement, and links to `docs/ROADMAP.md` for the gate's full mechanics
instead of restating them on the page. The detailed mechanics themselves —
same-runner sequencing, sealed predecessor timing, executable-input
classification, the fixed five-run plan, the ten-file evidence set, and the
fail-closed identity checks — live only in the roadmap entry cited above and
its decision records, not duplicated on the public page.

The canonical search phrase is “ahead-of-time compiler for typed Python”.
Copy may use close, natural variants such as “Python AOT compiler” and “compile
Python to a native binary”, but must not repeat phrases solely to manipulate
ranking.

## Search metadata contract

Every canonical HTML page must provide:

- a unique title, canonical URL, and plain-language meta description;
- Open Graph and X card metadata;
- page-level crawl permission with unlimited text, image, and video previews.

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

### SoftwareSourceCode entity model

The landing page's `SoftwareSourceCode` entity (`@id`:
`https://rotnov.github.io/pycc/#project`) is the machine-readable projection of
the project's authoritative facts. `scripts/check-site.sh` binds every material
field to a visible or repository-authoritative value so the JSON-LD cannot
silently claim a different project identity, license, language, runtime
semantics, or maturity:

- **`name`** must be `pycc`, matching the visible brand and page title.
- **`alternateName`** must be `pycc Python compiler`, a canonical
  human-readable alias that identifies the product category without
  misclassifying it as an AI or ML compiler.
- **`url`** must be the canonical landing-page URL
  (`https://rotnov.github.io/pycc/`), not the repository URL or any other
  origin.
- **`description`** must match the product-first pre-alpha source description
  that puts the AOT compiler for typed Python and current maturity first, with
  AI-created/human-managed provenance second.
- **`codeRepository`** must link to `https://github.com/rotnov/pycc`.
- **`license`** must be the MIT license URL
  (`https://opensource.org/license/mit`), matching the root `LICENSE` file.
  A non-MIT license (e.g. GPL) is rejected.
- **`programmingLanguage`** must be `Rust`, matching the Cargo workspace that
  implements the compiler. A false value (e.g. `Python`) is rejected.
- **`runtimePlatform`** must not be present. schema.org defines
  `runtimePlatform` as the runtime or interpreter dependency for the software
  being described; pycc's native and pure builds emit standalone executables
  with no runtime platform, and LLVM is the compiler backend, not a runtime
  platform for generated programs. Setting `runtimePlatform: "LLVM"` would
  misleadingly imply that pycc's output runs on an LLVM platform.
- **`keywords`** must be a non-empty array that includes compiler-category
  terms (`python`, `compiler`) and must not describe pycc as an AI or ML
  compiler (`ai compiler`, `machine learning`). AI-authorship terms
  (`AI-created software`, `AI agents`, `autonomous software development`)
  describe the development model, not the product intent.
- The entity must not claim production readiness (`production-ready`,
  `production ready`, `stable release`, `ga`), matching the visible pre-alpha
  status.

Each binding has a negative mutation test in `scripts/test-check-site.sh` that
corrupts the field and verifies the validator rejects the change.

Visible page copy must also state that AI agents create the entire project, the
human role is management, and no project code is handwritten by a human. This
development-model claim is part of the public project identity, not hidden
metadata.

`site/robots.txt` and `site/sitemap.xml` must use the same canonical origin.
The sitemap lists the landing page plus `/status/`, `/architecture/`,
`/python-aot-compilers/`, and `/ai-native/`. Every sitemap URL entry contains
exactly one `lastmod`, equal to that page's JSON-LD `WebPage.dateModified`, and
both values advance when main content, structured data, or important links
materially change. `scripts/check_sitemap_lastmod.rb` enforces a deterministic
binding between each canonical page's sitemap `lastmod` and the last non-merge
git commit (author date) that modified that page's source file, so a visible
content edit that leaves the `lastmod` stale is caught before merge. The
`check-site.sh` validator independently enforces that `lastmod` equals the
page's JSON-LD `dateModified`, so the two checks together guarantee
`lastmod` == `dateModified` == last content-change commit date.

### Repository social preview contract (issue #200)

The social preview is `site/og.png`, a single canonical project-owned visual
asset shared by the website Open Graph/X card metadata and the GitHub
repository social preview. GitHub generates a default owner-avatar/counter
card when no custom image is uploaded; the project replaces that generated
card with the same `og.png` the website serves so repository shares and
website shares present one consistent project identity.

GitHub documents its upload constraints at
<https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/customizing-your-repository/customizing-your-repositorys-social-media-preview>:
PNG, JPG, or GIF under 1 MB, at least 640×320, with 1280×640 recommended for
best display. The canonical asset is therefore PNG, exactly 1280×640, and
held under a 960 KiB safety margin so routine recompression or metadata
stripping cannot silently push it over GitHub's 1 MB ceiling. The asset
leads with the product name `pycc`, the promise "Typed Python in. Native
binaries out.", the "Rust + LLVM · pre-alpha" label, and the compiler
pipeline graphic; it contains no volatile stars/issues/contributor counts
and no personal owner imagery, preserving the product/provenance distinction
(product: AOT compiler for typed Python; provenance: built by AI, managed by
a human) owned jointly with #192.

`scripts/check-site.sh` enforces the asset contract deterministically: it
validates the PNG signature, IHDR dimensions (exactly 1280×640, rejecting
both undersize images below 640×320 and valid PNGs at the wrong
dimensions), and file size (rejecting anything at or above GitHub's 1 MB
limit and anything at or above the 960 KiB safety margin). The validator
also binds the `og:image` and `twitter:image` metadata to the canonical
`{canonical}og.png` URL so the HTML cannot reference a different or missing
asset. `scripts/test-check-site.sh` provides paired negative controls
(issue #200): an oversize image at or above 1 MB, an undersize image below
640×320, a valid PNG at wrong dimensions (640×320 instead of 1280×640), a
non-PNG file (JPEG magic bytes), a wrong `og:image` target, a wrong
`twitter:image` target, and a missing `og.png` that the metadata still
references.

The upload itself is performed through Repository Settings → Social preview
on GitHub; it is a one-time manual action that cannot be automated through
the standard repository API. After upload, the external setting is verified
via GraphQL: `usesCustomOpenGraphImage` must report `true` and
`openGraphImageUrl` must resolve successfully. The observation timestamp,
source asset SHA-256, and setting verification are recorded without treating
social-card publication as traffic or ranking evidence. The source asset
SHA-256 for the current `site/og.png` is
`79f47b25e40e4cc82d0d15a53fbf0828f3581942b4574be4294f053ba41a1ad7`
(740,052 bytes, 1280×640, PNG).

Each canonical page
carries an SVG favicon (`site/favicon.svg`, `>_` brand mark) linked with
`rel="icon"` and `type="image/svg+xml"`; the validator checks the SVG root
element, size limit, and link attributes. The 404 error page
(`site/404.html`) is a useful recovery page with the site's visual identity,
a visible "Page not found" heading, and absolute `/pycc/`-prefixed links to
the home page and three evidence pages; the validator checks its structure,
noindex directive, recovery links, and absolute paths.

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

`claims.json` also carries a `landing_projection` block that binds the
landing-page comparison table (`/`) to the same claim/source model. It has
three sub-objects: `column_sources` maps each landing column
(`static_model`, `output_artifact`, `language_contract`) to the single claim
field it projects from (`input_contract` or `html_output_cell`); `labels`
holds the exact short strings rendered in the landing `<td>` cells, keyed by
entity name then column; and `anchors` is the cross-projection link—a token
that must appear as a case-insensitive substring of the source claim field.
The landing table is a curated subset: it lists pycc, LPython, Codon, Nuitka,
and mypyc, and intentionally omits Cython (the page caption reads "Design
targets, not release claims"). `scripts/check-site.sh` parses the landing
table (keying rows by `<th scope="row">` text, skipping the pycc row header's
`mini-mark` span so the key is `"pycc"` not `">_ pycc"`, and validating the
`<th scope="col">` column headers against the expected names and order),
verifies that the landing row keys exactly equal `labels`'s entity set and
that this set is a subset of the model entities, checks that each row has
exactly three `<td>` cells whose whitespace-normalized text equals the
corresponding `labels` value, and enforces cross-projection anchor
consistency: for each entity and
column, `anchors[entity][column]` must be a non-empty, non-whitespace token
that appears as a case-insensitive substring of the entity's
`column_sources[column]` claim field. This anchor rule is a
structural token-presence invariant, not semantic content analysis: it catches
claim-side drift (a detailed claim field edited to drop the anchor token while
the landing projection keeps its label) and landing-side drift (an HTML cell or
projection label that no longer matches), but a coordinated five-field edit
can still evade it; full semantic contradiction detection remains #202's
domain, the same structural-binding limitation Part 1 documents.

`scripts/check-site.sh` enforces these mechanical requirements locally and in
the Pages workflow.
`scripts/test-check-site.sh` proves that the validator accepts the complete
site and rejects missing evidence pages, wrong canonicals, incomplete sitemaps,
missing official comparison sources, missing LPython alpha-status evidence, a
missing pre-alpha comparison warning, or required metadata. Table-driven
negative controls remove each required file and each required metadata key
individually, so deleting any entry from the validator's required-file or
required-metadata check list causes the self-test to fail. Additional negative
controls verify the landing-page canonical URL, the sitemap origin in
`robots.txt`, the JSON-LD `codeRepository` link, and the local-only-URL grep
check. The self-test
independently removes LPython's official project source and alpha positioning
so a newly covered compiler model cannot silently disappear. It rejects both a
duplicate sitemap `lastmod` and a value that disagrees with the corresponding
page's `WebPage.dateModified`. It also mutates
the landing, status, architecture, comparison, Markdown, and `llms.txt`
frontend/backend claims independently, preventing a structurally valid site
from silently describing a superseded compiler milestone. It also weakens the
status page's perf-gate enforcement claim (the greater-than-7.0% regression
floor enforced by a paired predecessor/candidate measurement) so the
validator rejects a public performance-gate claim that no longer matches
CI; the gate's detailed mechanics are documented only in `docs/ROADMAP.md`
and its cited decision records, not asserted against the status page's own
text, so no mutation targets them there. The comparison-page claim/source binding has its own mutation suite:
value-false mutations corrupt each external project's output or positioning
cell while keeping source URLs intact, proving that source-link presence alone
does not satisfy a value claim; model-HTML mismatch mutations change
`claims.json` without the HTML and vice versa, add or remove entities on one
side only, and relabel maturity in the model while the HTML disagrees; and
model integrity mutations delete `claims.json`, malform its JSON, remove an
entity's sources, and set an empty maturity. A positive control verifies that
minor whitespace changes in comparison cells do not break validation. The
landing-page projection has its own mutation suite: a binding mutation
corrupts the pycc `Output artifact` landing cell; entity-set mutations remove
the mypyc landing row, add an extra `ExtraTool` landing row, and add Cython to
the projection `labels`/`anchors` without a landing row; a label-drift
mutation changes `labels.pycc.output_artifact` without updating the landing
HTML; an anchor mutation sets `anchors.pycc.output_artifact` to a token absent
from `html_output_cell`; a cross-projection contradiction mutation drops
"Standalone" from the pycc `html_output_cell` claim field while co-updating
the detailed HTML cell to match, so both HTML tables still agree with their
model fields but the landing projection and the detailed claim now contradict
and the anchor rule catches it; and a mini-mark mutation removes the
`mini-mark` class from the pycc row-header span so the row key becomes
`">_ pycc"` and the entity-set check rejects. A blank-anchor mutation sets
an anchor to the empty string, which would otherwise be a substring of every
claim field and bypass the cross-projection check. A column-header mutation
swaps two `scope="col"` headers while leaving the cells in place, catching a
reordered table that presents every validated value under the wrong meaning.
A positive control verifies that minor whitespace changes in landing cells do
not break validation. The
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

## Tested quick-start example binding

The landing-page and README quick-start example is a tested, executable v0.1 example, not a design mock. Its source is bound to a single canonical fixture
(`tests/fixtures/quick_start.py`) and a CLI regression test
(`tests/quick_start.rs`) that builds and runs it through the real
`pycc build` → native binary path. That test no longer carries its own copy of
the expected stdout: the documented output lives in
`tests/fixtures/quick_start.expected.txt`, the single source of truth the test,
the README `$ ./hello` block, and the site hero output pane are all compared
against byte-for-byte (#197). `scripts/check-site.sh` binds the README
`cat hello.py` source, the site hero `<pre><code>` source, the copy-button
command, the site hero `.output-window` pane, and the README output to those
fixtures and to each other, with mutation tests in
`scripts/test-check-site.sh`. A deliberate coordinated change that updates the
fixtures, the README, and the site consistently is the intended update path;
the binding prevents inconsistent drift, not deliberate coordinated updates.
Other examples and CLI commands shown on the website are design targets unless
explicitly identified as implemented behavior.

### The published diagnostic example

The hero previously carried a `.diagnostic-card` showing a `T0021` error with a
precise multi-character span and a `help:` suggestion line. No pycc version has
ever emitted that output: `render_human` emits no `help:` line at all, and
every `T0xxx` span is still the `Span::new(0, 0)` placeholder, so the real
diagnostic points at line 1, column 1 with a one-character caret whose label
repeats the full message (D-083; D-043 owns the deferred span and help work).
The card is deleted, and `scripts/check-site.sh` fails if either
`diagnostic-card` or a published `help:` line reappears in `site/index.html`.

The honest replacement lives in `README.md`, generated from
`tests/diagnostics/quick_start_type_error.expected.txt` — the quick-start
program plus a call that passes a `str` to its `int` parameter, whose expected
output `tests/diagnostics_test.rs` checks against the real binary on every
test run — verifying the checked-in fixture, never rewriting it, so a renderer
change surfaces as a failing test and the fixture is updated by hand.
`scripts/check-site.sh` extracts the README block through an explicit
`<!-- #197: generated from ... -->` anchor and requires it to equal that
fixture with only the source path substituted for `hello.py`, so a renderer
change that would falsify the published example fails the gate rather than
silently shipping.

The hero output pane and the fixture it is compared against differ by exactly
one byte: the fixture is a POSIX text file ending in a single newline, while
the pane's text ends on its last output line because `</code></pre>` closes
there. `check-site.sh` strips exactly that one trailing newline from each
fixture before comparing, rather than collapsing whitespace, so trailing-space
or blank-line drift on either side still fails. For the same reason the pane's
markup must open as `<pre><code>0` with no newline after `<code>`: HTML
preserves it, and it would become a leading blank line the fixture does not
have.

Full HTML/Markdown parity for the new output pane and provenance note is
[#206](https://github.com/rotnov/pycc/issues/206)'s scope; the generated
evidence-hero contract that owns the `all-Tier-1` / `partial` / `experimental`
/ `unavailable` / `superseded` evidence-state vocabulary reused by the hero
marker is [#564](https://github.com/rotnov/pycc/issues/564)'s, and a dedicated
Diagnostics evidence page is
[#565](https://github.com/rotnov/pycc/issues/565)'s.

## Status-page freshness enforcement

`site/status/index.html` and `site/index.html` restate `docs/ROADMAP.md`'s
current milestone and acceptance status in prose; nothing previously enforced
that they stayed in sync with it, and both pages drifted silently past the
v0.2 acceptance milestone and into v0.3 before #401 caught it (D-156).
`scripts/check_status_page_freshness.rb` closes that gap with a narrowly
scoped, three-signal check: it watches a pull request's or push's diff to
`docs/ROADMAP.md` for (1) the `**Current milestone: ...**` bold line changing,
(2) a `<!-- roadmap-evidence: ... -->`-tagged checklist line's checked
state flipping or a new evidence-marker line being added, using the same
`EVIDENCE_MARKER` regex as `scripts/check_roadmap_evidence.rb` (duplicated
byte-for-byte, not shared via `require_relative`, so the two must be kept in
sync by hand), or (3) a new feature-landing paragraph
(`**[#NNN](https://github.com/rotnov/pycc/issues/NNN) — <feature>:**`) being
added under an existing `## vX.Y` heading, or an existing one being removed
(D-170, #522). When
any signal fires and the same diff touches neither `site/status/index.html`
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

Because the workflow runs on a shallow checkout, the base revision it must
diff against is usually absent locally and has to be fetched. That fetch
shares the checkout with whatever else `actions/checkout` is still doing,
and on 2026-08-18 it lost that race on a content-correct head: `git fetch`
aborted with `fatal: shallow file has changed since we read it`, so the
check failed before evaluating any signal at all. `ensure_revision_available`
therefore retries the fetch up to `FETCH_ATTEMPTS` times with a short
backoff, re-checking whether a concurrent operation landed the object
between attempts. The retry is deliberately narrow: only the transient
failures listed in `TRANSIENT_FETCH_PATTERNS` are retried, so a genuinely
unresolvable revision or a missing `origin` remote still fails on the first
attempt rather than costing the run three fetches before reporting the same
misconfiguration.

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

### llms.txt bounded expansion contract (issue #207)

The live `llms.txt` is a hand-read content map and an experimental
inference-time artifact. A reference consumer that expands its non-optional
links (`llms_txt2ctx`) previously assembled a **2,171,690-byte** context
dominated by GitHub application-shell HTML and duplicated Pages HTML, because
every tracked Markdown document was linked to its GitHub blob page (hundreds of
kilobytes of navigation and UI chrome) instead of its raw Markdown body, and
the canonical landing was expanded as both HTML and Markdown. The same
reference parser also truncated the three-line blockquote summary after its
first physical line. This defeated the file's inference-time purpose: a
constrained client received more than 2 MB of irrelevant markup before any
bounded project context.

The supported consumer contract is now explicit and enforced:

- **Hand-read content-map behavior.** A human or agent reading `llms.txt`
  directly sees a concise project summary, AI-authorship disclosure, current
  status, and a categorized link list. The file remains valid, readable
  Markdown.
- **Parser grammar/compatibility target.** The H1 title and blockquote summary
  are single physical lines so the proposal's reference parser
  (`llms_txt2ctx`) returns the complete intended summary in its `summary`
  field rather than truncating after the first line and pushing the remainder
  into `info` with literal `>` prefixes. `scripts/check-site.sh` rejects a
  multi-line blockquote summary.
- **Default versus optional expansion semantics.** Only the `## Project` and
  `## Specifications` sections are non-optional (expanded by a default
  consumer). The `## Optional` section holds human-navigation and larger
  resources (the canonical HTML landing, the GitHub repository UI page, the
  four evidence-page HTML representations, and the MIT license) that a client
  expands only when it explicitly requests more context.
- **Representation preference.** Non-optional inference links are
  Markdown/plain-text-first: tracked Markdown documents use
  `raw.githubusercontent.com` URLs so the fetched body is the intended
  Markdown artifact, not a GitHub blob/application page. GitHub blob URLs
  (`github.com/.../blob/...`) are rejected in non-optional sections. The
  Markdown landing (`index.html.md`) is the clean first-party representation of
  the landing page; the canonical HTML landing is kept in Optional so the
  default expansion does not download both representations of the same page.
- **Explicit context-size budget.** A reviewed **256 KiB** ceiling bounds the
  complete non-optional expanded context. The ceiling is enforced hermetically
  by `scripts/check-site.sh` against `site/llms-txt-context-manifest.json`, a
  reviewed fixture that binds each non-optional link to its local source file,
  representation role, and per-resource byte budget. The validator computes
  actual byte counts from the checked-out repository (the same content
  `raw.githubusercontent.com` serves for `main`) and fails closed when the
  aggregate or any per-resource budget is breached. No pull-request CI fetches
  the live site, GitHub, PyPI, or the mutable reference parser; the budget is
  pinned to the commit's content. The manifest is the deterministic record of
  per-document and total byte counts.
- **Human-navigation discoverability.** The public repository, canonical site,
  and human-readable GitHub links remain discoverable in the Optional section.
  Optimizing inference representation does not remove navigation or source
  provenance.

`scripts/check-site.sh` enforces the contract: it parses the non-optional
sections, matches their link set against the manifest, rejects GitHub blob
URLs, rejects HTML representations, rejects duplicate HTML+Markdown
representations of the same page in the default expansion, computes actual
byte counts from the checked-out files, and fails closed on per-resource or
aggregate budget breaches. `scripts/test-check-site.sh` provides negative
controls for each failure mode: a GitHub blob URL in a non-optional section, a
large human-navigation-only resource moved into the default set, a duplicate
HTML+Markdown landing in the default expansion, an oversized document
breaching its per-resource budget, an aggregate expansion breaching the total
budget, a representation changed from Markdown to HTML, and a non-optional
link removed so the manifest and file drift. Positive controls retain the
complete project summary, the authorship and current-status boundaries, and
human-navigation URLs remaining discoverable in Optional.

### Markdown landing semantic contract (issue #206)

`site/index.html.md` is publicly described (in `site/llms.txt` and above) as
the "clean text equivalent" of the landing page. Without a binding contract,
the Markdown could omit entire prominent visible sections of
`site/index.html` — the design-contract pillars, the "why pycc" rationale,
the comparison table, the compiler pipeline, and the final call to action —
while the HTML kept them, producing one-sided semantic drift that no validator
caught. `scripts/check_markdown_landing.rb` closes that gap with a
section-coverage contract: a `SECTION_CONTRACT` table pairs each prominent
visible section of the HTML page with (a) an HTML anchor substring that must
still exist in `site/index.html` and (b) a set of Markdown marker phrases
(and, for headed sections, the Markdown heading) that must appear in
`site/index.html.md`. The validator first confirms each HTML anchor still
exists in the live HTML page — so the contract cannot silently describe a
section that was removed from the HTML — then confirms each Markdown marker
and heading is present — so the Markdown cannot silently drop a section the
HTML still shows. Either side drifting alone fails the validator. The
validator also re-checks the pre-alpha no-production-readiness invariant so it
is self-contained. Its paired mutation suite
(`scripts/test_check_markdown_landing.rb`) provides negative controls for
each direction of drift: removing an HTML anchor, dropping a Markdown section
heading, removing a specific marker, corrupting the title, and adding a
production-ready claim. Both are wired into the Pages workflow alongside the
existing `check-site.sh` Markdown fact checks.

The XML sitemap (`site/sitemap.xml`, referenced from `site/robots.txt`) is the
standards-based discovery surface. The HTML `<link rel="sitemap">` link
relation is not used: it is not a registered IANA link relation and is not a
documented sitemap-discovery mechanism, so emitting it would create false
confidence about discovery without any standards, Google, or Bing backing.
After a successful production deployment, `scripts/notify-indexnow.sh` parses
that validated sitemap and submits its complete canonical URL set in one
IndexNow batch POST so Bing and participating engines can discover material
updates quickly. It rejects empty, duplicate, out-of-scope, query-bearing, or
fragment-bearing URLs before making a request. Its public verification key is
hosted below `/pycc/`, which limits that key to URLs in the project path. The
notification is best-effort, has finite connection and request timeouts, and
does not block a successful Pages deployment. Redirects (3xx) are fail-closed:
the notifier does not follow them, because a redirected request may not carry
the original JSON payload and a final 200 from a different origin would be a
false delivery claim. Instead of opaque curl retry, the notifier uses an
explicit per-attempt loop that records each attempt's HTTP status and uploaded
byte count. On every terminal path the notifier emits a structured one-line
result record containing the UTC submission timestamp, endpoint host, URL
count, sitemap payload SHA-256, deployed commit, final HTTP status, accepted
class, delivery state, attempt count, possible-duplicate-delivery flag,
sanitized failure class (distinguishing DNS errors, connection errors,
timeouts, TLS errors, rate limiting, scope mismatches, redirect rejections,
and HTTP errors), and retry configuration. The delivery state machine
distinguishes: `submitted` (direct 200 with no earlier ambiguity),
`key_validation_pending` (direct 202 with no earlier ambiguity),
`submitted_after_ambiguous_retry` (200 after an earlier attempt sent the body
but received no accepted response), `key_validation_pending_after_ambiguous_retry`
(the corresponding 202 state), `delivery_unknown_after_payload_write` (no
accepted response, but one or more attempts may have delivered bytes),
`failed_before_payload_write` (no accepted response and no request body
crossed the wire), `redirect_rejected` (3xx response), and `http_rejected`
(4xx/5xx response). When `possible_duplicate_delivery=true`, the notifier is
an at-least-once transport, not exactly-once: an earlier attempt may have been
accepted before its response was lost. The production workflow step has a
stable `id` and an `if: always()` follow-up that inspects the step's raw
`outcome` before `continue-on-error` normalization, so a failed notification
is externally visible as a workflow warning without rolling back the completed
deployment. The notifier also writes a concise `GITHUB_STEP_SUMMARY` table so
a reader can identify the trusted commit, URL count, sitemap digest, response
class, delivery state, attempt count, and delivery outcome without downloading
raw logs.

`scripts/test-notify-indexnow.py` points the production notifier at a local HTTP
fixture and proves that the real non-dry-run path sends the expected JSON
payload, emits the structured result record with all required fields on every
terminal path, distinguishes 200 from 202, classifies failures by sanitized
failure class, rejects redirects (301/302/303/307/308) as `redirect_rejected`,
verifies ambiguous retry states (`submitted_after_ambiguous_retry`,
`key_validation_pending_after_ambiguous_retry`,
`delivery_unknown_after_payload_write`), confirms
`possible_duplicate_delivery` is set only after an ambiguous body-bearing
attempt, writes the `GITHUB_STEP_SUMMARY` file, and fails on an HTTP
error without contacting a public search endpoint. The pages workflow
structural invariants (step `id`, `continue-on-error`, `if: always()`
observer, `outcome` not `conclusion`, unprivileged permissions, push-only
after deploy) are independently validated by
`scripts/check_pages_workflow.rb` with its own mutation suite
(`scripts/test_check_pages_workflow.rb`). The binding of this hermetic test
to the Pages workflow's build job — so that removing or weakening the only
real HTTP-fixture invocation causes a CI failure — is enforced by
`scripts/check_indexnow_test_binding.rb`, which semantically parses
`pages.yml` and rejects deletion of the fixture invocation, push-only job-
or step-level `if` conditions, job- or step-level `continue-on-error`, and
shell control flow that absorbs the fixture's exit status (`|| true`, `; true`,
`&& true`, background `&`, piping into `true`). Its mutation suite is
`scripts/test_check_indexnow_test_binding.rb`.
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

## Pages visit measurement gap (issue #208)

The site deliberately has no project-selected analytics script, cookie,
or external beacon (see [D-168](./decisions/D-168-pages-visit-measurement-capability-contract.md)).
This is an explicit, reviewed decision, not an oversight. The three
existing discovery signals — Google Search Console (Google Search
clicks/impressions), GitHub repository traffic (repository
views/clones), and engine-qualified visibility (SERP/answer citations) —
do not measure visits to the GitHub Pages site from Yandex, DuckDuckGo,
Perplexity, ChatGPT, other answer engines, ordinary referrals, or direct
navigation. Non-Google landing visits remain unobservable, and the
roadmap and SEO reports state this.

The [machine-readable Pages visit artifact](./PAGES_VISIT_OBSERVATIONS.json)
is a template with no observations. It defines the measurement contract
(reporting timezone, canonical pages, primary conversion, source-class
vocabulary, collection-status vocabulary, data-minimization boundary, and
separation rules) so a future PR that activates analytics can append real
observations without a schema redesign. The contract's primary conversion
is a click from a canonical Pages page to
`https://github.com/rotnov/pycc` — the only instrumented interaction. The
data-minimization boundary forbids names, email/account identifiers, form
contents, full IP addresses, full user agents, cookies, fingerprints,
persistent cross-site IDs, session replay, arbitrary query
strings/fragments, and raw search queries unless a separately accepted
decision justifies them. A non-zeroable collection status (`blocked`,
`delayed`, `unauthorized`, `provider_error`, `unknown`) must never be
converted to zero; unavailable data is `null`, not `0`.

`scripts/check_pages_visit_observations.py` validates the artifact schema
and prose bindings. It rejects prose that conflates repository views with
Pages visits, Search Console clicks with all-provider visits, or
analytics with a ranking factor. Its mutation suite is
`scripts/test_check_pages_visit_observations.py`.

A future PR that activates analytics must: record a superseding ADR;
add an accurate public privacy/analytics disclosure before or with
collection; load the provider script non-blockingly on every canonical
HTML page exactly once; keep content, navigation, and local `site.js`
functional when the provider is unavailable or blocked; not expose
dashboard/read/export credentials in the published artifact; and
establish the activation baseline at deployment time without
synthesizing pre-installation history. The issue explicitly does not
request adding analytics scripts to the site in this change.

## Pages performance budget gate

The website is protected by a hermetic Pages performance budget gate
(`scripts/check_pages_performance_budget.rb`) that runs in CI as the
`pages-performance` job and is bound to `ci-gate` -- a pull request cannot
merge unless the gate passes. The gate is a source-artifact check, not a
field-data observation: it measures the published HTML/CSS/JS/image bytes
under controlled lab conditions, not real-user field data.

### Lighthouse configuration

The gate runs Lighthouse 12.8.2 (pinned) in mobile emulation mode against
a hermetic local server (`scripts/serve_pages_fixture.py`) that serves the
checked-out `site/` directory. No external network requests are made during
the check -- the server, Lighthouse, and Chrome are all local to the CI
runner, making the gate fully reproducible.

### Canonical pages and 404 cohort

The gate checks the 5 canonical pages (landing, status, architecture,
python-aot-compilers, AI-native experiment) plus the 404 error page. Each
page is measured with 5 Lighthouse replicates, and the median of each
metric across the replicates is compared against the budget thresholds.
The 5-replicate median strategy dampens single-run variance inherent to
lab-based browser performance measurement.

### Performance thresholds

Each page must meet all of the following Lighthouse metric thresholds:

- **LCP** (Largest Contentful Paint): within budget
- **CLS** (Cumulative Layout Shift): within budget
- **TBT** (Total Blocking Time): within budget
- **Performance** score: at or above the threshold

TBT is a lab metric that measures main-thread blocking during page load.
It is **not** INP (Interaction to Next Paint), which is a field-data metric
measuring real user interaction latency. The gate enforces TBT as a
source-artifact proxy for responsiveness, not as a claim about real-user
interaction latency.

### Resource budgets

In addition to Lighthouse metrics, the gate enforces byte-count budgets
for each resource type:

- **HTML**: within budget per page
- **CSS**: within budget per page
- **JS**: within budget per page
- **Image**: within budget per page

These budgets are defined in `tests/fixtures/pages-performance-budget.json`
and the page-to-URL mapping in
`tests/fixtures/pages-performance-manifest.json`.

### CI binding

The `pages-performance` job runs on every pull request and push. It is
listed in `ci-gate.needs`, so `ci-gate` fails unless `pages-performance`
succeeds. The job has `contents: read` permission only, no
`continue-on-error`, and is not push-only -- the
`scripts/check_roadmap_evidence.rb` lifecycle validator enforces these
structural invariants on every pull request.

### Lab vs field distinction

The Pages performance budget gate is a **lab** measurement: it runs
Lighthouse in a controlled CI environment against the source artifacts.
It is not **field** data from real users. Lighthouse scores can differ
from Chrome User Experience Report (CrUX) field data because field data
reflects real devices, networks, and usage patterns. The gate's purpose is
to prevent regressions in the source artifacts, not to claim specific
search ranking outcomes. Page experience is one of many signals search
engines use; this gate does not directly affect search ranking.

## Site accessibility gate

The website is protected by a hermetic site accessibility gate that
runs in CI as the `pages-accessibility` job and is bound to `ci-gate` --
a pull request cannot merge unless the gate passes. The gate was
activated through a D-103 two-merge sequence: the first merge staged
the CI workflow, checker, and tests as inert policy successors; the
second merge activated them alongside the accessibility source fix.
The gate uses three evaluators, each targeting a
distinct accessibility surface:

### Lighthouse accessibility

The gate runs Lighthouse 12.8.2 (pinned) in mobile emulation mode
(412×823 CSS pixels, device scale factor 1.75) with
`--only-categories=accessibility` against the hermetic local server
(`scripts/serve_pages_fixture.py`). One Lighthouse Result (LHR) JSON
is collected per canonical page. The checker
(`scripts/check_site_accessibility.rb`) validates that the
`aria-allowed-role` and `color-contrast` audits each pass with score 1
and zero failing items, and that expected audits are present and not
`notApplicable`.

### W3C Nu ARIA conformance

The ARIA conformance evaluator (`scripts/check_site_aria_conformance.py`)
parses each served HTML body and rejects any `<div>` element carrying
`aria-label` or `aria-labelledby` without an explicit non-generic
`role` that permits naming. This catches the W3C Nu validator's
prohibition on naming generic-role elements without requiring a
network request to validator.w3.org.

### Reduced-motion computed-style

The reduced-motion evaluator (`scripts/check_site_reduced_motion.js`)
uses Puppeteer to emulate `prefers-reduced-motion: reduce` and
`no-preference`, reading computed `scroll-behavior` (root) and
`transition-duration` (skip-link, nav-link) to assert that
reduced-motion suppresses nonessential motion to ≤ 0.02ms while normal
motion remains available under no-preference. Puppeteer is a CI tool
(like `npx lighthouse`), not a site dependency; it uses the system
Chrome via `PUPPETEER_EXECUTABLE_PATH`.

### CI binding

The `pages-accessibility` job runs on every pull request and push. It
has `contents: read` permission only, no `continue-on-error`, and is
listed in `ci-gate.needs` alongside `pages-performance`. The
`ci-gate` fail-closed aggregate condition requires
`needs.pages-accessibility.result == 'success'`.

### Scope and limitations

The three evaluators are deliberately separate: Lighthouse
accessibility scoring, Nu ARIA conformance, and reduced-motion
computed-style checks each cover a distinct surface and must not be
collapsed into a single check. The ARIA conformance evaluator uses
Python's `html.parser` rather than a full browser parser, so it
catches structural ARIA violations but not computed-style or
rendering-dependent issues. The reduced-motion evaluator checks the
documented motion surfaces but does not enumerate every animated
element; the CSS `@media (prefers-reduced-motion: reduce)` block with
universal selector provides the global override that the evaluator
verifies.

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

## Citation metadata

The root `CITATION.cff` provides machine-readable software citation metadata
conforming to CFF 1.2.0. GitHub renders a "Cite this repository" panel from
this file. The citation identity uses the exact `rotnov/pycc` repository
identity rather than the bare product name, which collides with unrelated
projects. Authorship is attributed to a collective entity ("pycc AI agents")
that truthfully describes the autonomous AI coding agent development model;
the human maintainer is not listed as a software author, only as the
repository owner and manager. Release-bound fields (`version`,
`date-released`, `commit`, DOI) are omitted until the release lifecycle
becomes coherent (see #196); a future release will derive those fields from
the accepted structured release state rather than duplicating literals by
hand. No DOI or `preferred-citation` is declared because no external
archival record exists. The citation file is validated by
`scripts/check_citation_cff.rb`, which rejects wrong repository URLs,
non-MIT licenses, "AI compiler" product semantics, human authorship
inferred from repository ownership, release-bound fields before #196
resolves, and the presence of `.zenodo.json` (Zenodo is a separate,
explicitly approved step). Its mutation suite
(`scripts/test_check_citation_cff.rb`) provides negative controls for
each material field.

## Source link monitoring (issue #202)

The Python AOT compiler comparison page cites external project
documentation as evidence for each comparison-table cell. These source
links can break when upstream projects reorganize their docs, but
required PR CI must not depend on network calls to external sites.

The contract has two layers:

1. **Hermetic registry validator** (PR-required, in `pages.yml`):
   `scripts/check_source_links_registry.rb` validates that
   `site/python-aot-compilers/source-link-registry.json` covers every
   external URL in `claims.json`, that each entry has a valid status
   (`ok`, `broken`, or `redirect`) and `last_checked` date, and that no
   entry has status `broken` (a known-broken link must be updated, not
   silently kept). This check performs no network requests.

2. **Scheduled live link check** (non-blocking, separate workflow):
   `.github/workflows/link-check.yml` runs daily at 06:00 UTC and
   executes `scripts/check_source_links_live.py` against the registry.
   It uses `continue-on-error` so a failure surfaces a workflow warning
   for human triage without blocking PR merges. The live checker
   classifies each URL as `healthy`, `confirmed_missing`,
   `blocked_or_rate_limited`, or `unknown_error`.

The registry is the bridge: the hermetic validator ensures it is
complete and well-formed, the live checker updates it, and a broken
status in the registry blocks the hermetic validator until the link is
fixed or the claim is updated.
