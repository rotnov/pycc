# pycc Website and Search Discoverability

The public project website is a static, dependency-free landing page at
<https://rotnov.github.io/pycc/>. Its source lives in `site/`; GitHub Pages
publishes that directory through `.github/workflows/pages.yml`.

## Purpose

The website gives search engines and prospective contributors one stable,
indexable explanation of pycc:

- pycc is an ahead-of-time compiler for typed, standard Python 3.14;
- the intended output is a standalone native binary;
- the implementation is written in Rust and uses LLVM;
- the project is pre-alpha, and design targets are not presented as released
  features.

The canonical search phrase is “ahead-of-time compiler for typed Python”.
Copy may use close, natural variants such as “Python AOT compiler” and “compile
Python to a native binary”, but must not repeat phrases solely to manipulate
ranking.

## Search metadata contract

`site/index.html` must provide:

- the canonical URL `https://rotnov.github.io/pycc/`;
- a unique title and plain-language meta description;
- Open Graph and X card metadata;
- `SoftwareSourceCode` JSON-LD linked to the public repository;
- page-level crawl permission and a sitemap reference.

`site/robots.txt` and `site/sitemap.xml` must use the same canonical origin.
The social preview is `site/og.png`. `scripts/check-site.sh` enforces these
mechanical requirements locally and in the Pages workflow.
`scripts/test-check-site.sh` proves that the validator accepts the complete
site and rejects missing files or required metadata.

GitHub project Pages are served below `/pycc/`, while the robots exclusion
protocol only discovers `robots.txt` at the origin root. The page-level robots
meta tag is therefore the effective crawl directive on the default
`rotnov.github.io` domain. `site/robots.txt` remains ready for a future custom
domain, and the sitemap can be submitted directly to search consoles.

## Publication

Pushes to `main` that change the website trigger the Pages workflow. The
workflow validates the static files, uploads only `site/`, and deploys through
the protected `github-pages` environment. It can also be run manually; only a
run from `main` is allowed to reach the deployment job.

The site deliberately has no package manager, runtime JavaScript dependency,
analytics, cookies, or external font request. A small local script only powers
the copy-to-clipboard control. Relative asset paths keep local previews and the
`/pycc/` project-site base path working identically.

When the website's claims change, update the root README and the relevant
specification in the same patch. Search metadata must describe current or
explicitly labelled planned behavior; it must never turn roadmap goals into
present-tense product claims.
