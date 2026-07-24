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
- the project is created entirely by AI agents, while a human manages goals,
  constraints, priorities, and product decisions without writing project code;
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

Visible page copy must also state that AI agents create the entire project, the
human role is management, and no project code is handwritten by a human. This
development-model claim is part of the public project identity, not hidden
metadata.

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

Pull requests that change the website or either validator run the Pages build
and validation job without deploying. Pushes to `main` that change the website
validate the static files, upload only `site/`, and deploy through the protected
`github-pages` environment. A manual run performs validation and artifact
assembly only; deployment authority is reserved for a `push` to `main`. The
workflow grants `pages: write` and OIDC access only to that deployment job;
pull-request code runs with read-only repository access.

The site deliberately has no package manager, runtime JavaScript dependency,
analytics, cookies, or external font request. A small local script only powers
the copy-to-clipboard control. Relative asset paths keep local previews and the
`/pycc/` project-site base path working identically.

When the website's claims change, update the root README and the relevant
specification in the same patch. Search metadata must describe current or
explicitly labelled planned behavior; it must never turn roadmap goals into
present-tense product claims.
