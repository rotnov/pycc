# pycc Website

The public project website lives at <https://rotnov.github.io/pycc/>. It is
deliberately small: a landing page, a status page generated from the roadmap,
`llms.txt`, `sitemap.xml`, `robots.txt` and a 404 page.
[D-259](decisions/D-259-replace-the-pages-site-with-a-small-generated-site.md)
owns this design; it replaced an earlier seven-page site whose date pins,
evidence-hero manifest, performance and accessibility budgets, and wording
checkers had to be kept consistent by compiler pull requests.

## Sources and build

`site/` holds only hand-written static sources:

| File | Purpose |
|---|---|
| `index.html` | Landing page: what pycc is, the pre-alpha boundary, the checked quick-start example, and links into the repository's documents. |
| `404.html` | Not-found page. GitHub Pages serves it from the project root, so its links are absolute (`/pycc/...`). |
| `llms.txt` | [llms.txt](https://llmstxt.org/) summary for language models. It links to the status page and to the repository's Markdown documents and repeats no status claims, so it cannot drift from them. |
| `robots.txt`, `sitemap.xml` | Crawl policy and the two canonical URLs (`/` and `/status/`). The sitemap carries no `<lastmod>`. |
| `styles.css`, `favicon.svg`, `og.png` | One small stylesheet with light and dark schemes, the icon, and the 1280×640 social image also used as the GitHub repository social preview. |
| `3361fe03d0f44ab7cdbb1a3ce1461821.txt` | IndexNow key file. |
| `search-visibility/evidence.json` | Search-visibility evidence record read by `scripts/check_search_visibility.rb` (see [SEARCH_VISIBILITY.md](SEARCH_VISIBILITY.md)). |

No source file carries a hand-maintained date.

`python3 scripts/build_site.py` copies `site/` to `_site/` (gitignored) and
generates the rest:

- `status/index.html` from `docs/ROADMAP.md`: the bold "Current milestone"
  line and the Area and Status columns of the "Current delivery status"
  table, stamped with the commit SHA and commit date read from git. The
  roadmap's Evidence column stays in the roadmap, which the page links to.
  The build fails if the milestone line or the table is missing or
  malformed, and if `site/` contains a directory the build generates.
- `noindex` redirect stubs for the retired routes `/ai-native/`,
  `/architecture/`, `/diagnostics/`, `/language-support/` and
  `/python-aot-compilers/`, pointing to the landing page, so inbound links
  keep resolving while search engines drop the URLs.

Set `PYCC_SITE_COMMIT` and `PYCC_SITE_DATE` to build without git (the tests
do this).

## The site check

`python3 scripts/check_site.py _site` is the only site gate. It checks
structure, never wording, dates, sizes or performance:

- every HTML page starts with a doctype and has balanced tags (void elements
  excepted);
- every internal `href` and `src` resolves to a built file, and every
  same-page `#fragment` names an element `id`;
- every sitemap `<loc>` is unique, under `https://rotnov.github.io/pycc/`,
  and resolves;
- `robots.txt` names the sitemap;
- `llms.txt` starts with `# pycc` and a `> ` summary, its site links resolve,
  and its links into the repository's `main` branch name files that exist;
- `404.html` exists.

External links are not fetched. `scripts/test_check_site.py` holds the
positive control (the real site builds and passes) and one negative control
per rule; it runs in the governance job's unittest discovery and in the
`Pages` workflow.

To preview locally:

```console
$ python3 scripts/build_site.py && python3 scripts/check_site.py _site
$ python3 -m http.server -d _site 8000
```

Pages that use absolute `/pycc/` links (the 404 page) only resolve when
served under that prefix; the others use relative links.

## Search metadata

The landing page carries the canonical URL, a description, Open Graph and X
card metadata pointing at `og.png`, the Google Search Console verification
meta tag, and one JSON-LD `SoftwareSourceCode` entity named `pycc` with the
repository URL and MIT license. `scripts/check_package_identity.rb` checks
that the entity's `name` is `pycc` and that `llms.txt` is titled `# pycc`.
Search metadata must describe current behavior or clearly labelled plans;
it never turns roadmap goals into present-tense claims.

Search and traffic evidence (Search Console, GitHub traffic, engine
visibility, earned authority, Pages visits) is recorded in the `docs/*.json`
artifacts described in [SEARCH_VISIBILITY.md](SEARCH_VISIBILITY.md); their
checkers run in the `Pages` workflow. The site has no analytics script,
cookie or external beacon
([D-168](decisions/D-168-pages-visit-measurement-capability-contract.md)).

## Publication

`.github/workflows/pages.yml`:

- runs on pull requests and on pushes to `main` that change the site
  sources, `docs/ROADMAP.md`, `README.md`, `CITATION.cff`, the search
  evidence artifacts, or one of the scripts the workflow runs, and on manual
  dispatch;
- the `build` job builds `_site/`, runs `check_site.py`, the site tests and
  the kept README, citation, package-identity, search-evidence and IndexNow
  checkers, and uploads `_site/` as the Pages artifact;
- the `deploy` job runs only for a `push` to `refs/heads/main`, alone holds
  `pages: write` and OIDC access, and deploys through the protected
  `github-pages` environment;
- the read-only `notify-indexnow` job then submits the sitemap URLs to
  IndexNow on a best-effort basis (`scripts/notify-indexnow.sh`, contract
  checked by `scripts/check_pages_workflow.rb` and
  `scripts/test-notify-indexnow.py`).

Because a roadmap change triggers a deploy, the status page is always
generated from the latest roadmap on `main`. `Pages` is not a required
status check; a red `Pages` run never blocks a compiler pull request.

## Citation metadata

The root `CITATION.cff` provides CFF 1.2.0 citation metadata for GitHub's
"Cite this repository" panel. It uses the exact `rotnov/pycc` repository
identity, attributes authorship to the collective "pycc AI agents", and
omits release-bound fields until the release lifecycle is settled (#196).
`scripts/check_citation_cff.rb` validates it, with negative controls in
`scripts/test_check_citation_cff.rb`.
