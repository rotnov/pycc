#!/usr/bin/env sh
set -eu

repo_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
fixture_root=$(mktemp -d "${TMPDIR:-/tmp}/pycc-site-check.XXXXXX")

cleanup() {
  rm -rf "$fixture_root"
}
trap cleanup EXIT HUP INT TERM

cp -R "$repo_root/site" "$fixture_root/site"

SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null

python3 - "$fixture_root/site/styles.css" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
content = path.read_text()
responsive_footer = """  footer > div {
    min-width: 0;
    flex-wrap: wrap;
    justify-content: flex-start;
    justify-self: stretch;
  }"""
assert responsive_footer in content
path.write_text(
    content.replace(
        responsive_footer,
        responsive_footer.replace("flex-wrap: wrap", "flex-wrap: nowrap"),
        1,
    )
)
PY

if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted non-wrapping narrow footer links" >&2
  exit 1
fi

cp "$repo_root/site/styles.css" "$fixture_root/site/styles.css"

python3 - "$fixture_root/site/styles.css" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
content = path.read_text()
mobile_footer = """  footer {
    grid-template-columns: 1fr;
    gap: 18px;
    padding: 24px 0;
  }"""
assert mobile_footer in content
path.write_text(
    content.replace(
        mobile_footer,
        mobile_footer.replace(
            "grid-template-columns: 1fr",
            "grid-template-columns: 1fr auto",
        ),
        1,
    )
)
PY

if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a two-column narrow footer" >&2
  exit 1
fi

cp "$repo_root/site/styles.css" "$fixture_root/site/styles.css"

python3 - "$repo_root" "$fixture_root/site" <<'PY'
from pathlib import Path
import os
import subprocess
import sys


repo_root = Path(sys.argv[1])
site_dir = Path(sys.argv[2])
index_path = site_dir / "index.html"
checker = repo_root / "scripts" / "check-site.sh"
original = index_path.read_text()
stylesheet = '    <link rel="stylesheet" href="styles.css">'
script = '    <script defer src="site.js"></script>'
assert stylesheet in original
assert script in original

mutations = {
    "missing stylesheet": original.replace(stylesheet, "", 1),
    "empty stylesheet target": original.replace(
        'href="styles.css"', 'href=""', 1
    ),
    "duplicate stylesheet outside the head": original.replace(
        "  <body>", f"  <body>\n{stylesheet}", 1
    ),
    "duplicate stylesheet href attribute": original.replace(
        'href="styles.css"', 'href="other.css" href="styles.css"', 1
    ),
    "duplicate stylesheet rel attribute": original.replace(
        'rel="stylesheet"', 'rel="alternate" rel="stylesheet"', 1
    ),
    "absolute stylesheet target": original.replace(
        'href="styles.css"',
        'href="https://rotnov.github.io/pycc/styles.css"',
        1,
    ),
    "local-only stylesheet target": original.replace(
        'href="styles.css"', 'href="http://127.0.0.1/styles.css"', 1
    ),
    "different stylesheet target": original.replace(
        'href="styles.css"', 'href="other.css"', 1
    ),
    "disabled stylesheet": original.replace(
        '<link rel="stylesheet" href="styles.css">',
        '<link rel="stylesheet" href="styles.css" disabled>',
        1,
    ),
    "non-applying stylesheet media": original.replace(
        '<link rel="stylesheet" href="styles.css">',
        '<link rel="stylesheet" href="styles.css" media="not all">',
        1,
    ),
    "incompatible stylesheet type": original.replace(
        '<link rel="stylesheet" href="styles.css">',
        '<link rel="stylesheet" href="styles.css" type="text/plain">',
        1,
    ),
    "alternate stylesheet relationship": original.replace(
        'rel="stylesheet"', 'rel="alternate stylesheet"', 1
    ),
    "stylesheet inside an inert template": original.replace(
        stylesheet,
        "",
        1,
    ).replace(
        "  <body>",
        f"  <body>\n    <template>\n{stylesheet}\n    </template>",
        1,
    ),
    "stylesheet inside an inert noscript": original.replace(
        stylesheet,
        "",
        1,
    ).replace(
        "  <body>",
        f"  <body>\n    <noscript>\n{stylesheet}\n    </noscript>",
        1,
    ),
    "stylesheet inside foreign SVG content": original.replace(
        stylesheet,
        "",
        1,
    ).replace(
        "  <body>",
        f"  <body>\n    <svg>\n{stylesheet}\n    </svg>",
        1,
    ),
    "additional stylesheet inside an SVG HTML integration point": (
        original.replace(
            "  <body>",
            "  <body>\n"
            "    <svg><foreignObject>"
            '<link rel="stylesheet" href="other.css">'
            "</foreignObject></svg>",
            1,
        )
    ),
    "additional SVG href script": original.replace(
        "  <body>",
        '  <body>\n    <svg><script href="other.js"></script></svg>',
        1,
    ),
    "additional self-closing SVG href script": original.replace(
        "  <body>",
        '  <body>\n    <svg><script href="other.js" /></svg>',
        1,
    ),
    "missing script": original.replace(script, "", 1),
    "empty script target": original.replace('src="site.js"', 'src=""', 1),
    "duplicate script outside the head": original.replace(
        "  <body>", f"  <body>\n{script}", 1
    ),
    "duplicate script src attribute": original.replace(
        'src="site.js"', 'src="other.js" src="site.js"', 1
    ),
    "absolute script target": original.replace(
        'src="site.js"',
        'src="https://rotnov.github.io/pycc/site.js"',
        1,
    ),
    "local-only script target": original.replace(
        'src="site.js"', 'src="http://localhost/site.js"', 1
    ),
    "different script target": original.replace(
        'src="site.js"', 'src="other.js"', 1
    ),
    "non-deferred script": original.replace(
        '<script defer src="site.js">', '<script src="site.js">', 1
    ),
    "async script": original.replace(
        '<script defer src="site.js">',
        '<script async defer src="site.js">',
        1,
    ),
    "non-executable script type": original.replace(
        '<script defer src="site.js">',
        '<script type="text/plain" defer src="site.js">',
        1,
    ),
    "script inside an inert template": original.replace(
        script,
        "",
        1,
    ).replace(
        "  <body>",
        f"  <body>\n    <template>\n{script}\n    </template>",
        1,
    ),
    "script inside an inert noscript": original.replace(
        script,
        "",
        1,
    ).replace(
        "  <body>",
        f"  <body>\n    <noscript>\n{script}\n    </noscript>",
        1,
    ),
    "script inside foreign MathML content": original.replace(
        script,
        "",
        1,
    ).replace(
        "  <body>",
        f"  <body>\n    <math>\n{script}\n    </math>",
        1,
    ),
    "additional script inside a MathML HTML integration point": (
        original.replace(
            "  <body>",
            "  <body>\n"
            '    <math><annotation-xml encoding="text/html">'
            '<script src="other.js"></script>'
            "</annotation-xml></math>",
            1,
        )
    ),
    "self-closing external script": original.replace(
        script,
        '    <script defer src="site.js" />',
        1,
    ),
    "base URL override": original.replace(
        "  <head>", '  <head>\n    <base href="https://example.com/">', 1
    ),
}

environment = dict(os.environ)
environment["SITE_DIR"] = str(site_dir)
index_path.write_text(original.replace("<br>", "<br />", 1))
void_result = subprocess.run(
    [str(checker)],
    cwd=repo_root,
    env=environment,
    capture_output=True,
    text=True,
    check=False,
    timeout=10,
)
if void_result.returncode != 0:
    raise SystemExit("Validator rejected a self-closing void element")

index_path.write_text(
    original.replace(
        "  <body>",
        '  <body>\n    <svg><path d="M0 0h1v1z" /></svg>',
        1,
    )
)
foreign_result = subprocess.run(
    [str(checker)],
    cwd=repo_root,
    env=environment,
    capture_output=True,
    text=True,
    check=False,
    timeout=10,
)
if foreign_result.returncode != 0:
    raise SystemExit("Validator rejected a self-closing foreign element")

for description, mutated in mutations.items():
    index_path.write_text(mutated)
    result = subprocess.run(
        [str(checker)],
        cwd=repo_root,
        env=environment,
        capture_output=True,
        text=True,
        check=False,
        timeout=10,
    )
    if result.returncode == 0:
        raise SystemExit(
            f"Validator accepted an entry point with {description}"
        )

index_path.write_text(original)
PY

rm "$fixture_root/site/robots.txt"
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a site with a missing required file" >&2
  exit 1
fi

cp "$repo_root/site/robots.txt" "$fixture_root/site/robots.txt"
python3 - "$fixture_root/site/index.html" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
content = path.read_text()
required = (
    'content="index, follow, max-image-preview:large, '
    'max-snippet:-1, max-video-preview:-1"'
)
assert required in content
path.write_text(content.replace(required, "", 1))
PY

if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a site without the required robots directive" >&2
  exit 1
fi

cp "$repo_root/site/index.html" "$fixture_root/site/index.html"
python3 - "$fixture_root/site/index.html" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
content = path.read_text()
marker = 'name="description"'
marker_offset = content.index(marker)
content_offset = content.index('content="', marker_offset) + len('content="')
content_end = content.index('"', content_offset)
assert content_end > content_offset
path.write_text(content[:content_offset] + content[content_end:])
PY

if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted an empty required metadata value" >&2
  exit 1
fi

cp "$repo_root/site/index.html" "$fixture_root/site/index.html"
python3 - "$repo_root" "$fixture_root/site" <<'PY'
from pathlib import Path
import os
import subprocess
import sys


repo_root = Path(sys.argv[1])
site_dir = Path(sys.argv[2])
checker = repo_root / "scripts" / "check-site.sh"
index_path = site_dir / "index.html"
ai_path = site_dir / "ai-native" / "index.html"
original_index = index_path.read_text()
original_ai = ai_path.read_text()
head_marker = "  </head>"
body_marker = "  <body>"

keyword_mutations = (
    (
        index_path,
        original_index.replace(
            'name="description"',
            'name="keywords" name="description"',
            1,
        ),
        "landing duplicate meta name attribute",
    ),
    (
        index_path,
        original_index.replace(
            head_marker,
            '    <meta name="keywords" content="python compiler">\n' + head_marker,
            1,
        ),
        "landing head keywords metadata",
    ),
    (
        index_path,
        original_index.replace(
            body_marker,
            body_marker + '\n    <meta name=" KEYWORDS " content="python compiler">',
            1,
        ),
        "landing body mixed-case keywords metadata",
    ),
    (
        ai_path,
        original_ai.replace(
            head_marker,
            '    <meta name="keywords" content="AI compiler">\n' + head_marker,
            1,
        ),
        "evidence-page keywords metadata",
    ),
    (
        ai_path,
        original_ai.replace(
            'name="description"',
            'name="keywords" name="description"',
            1,
        ),
        "evidence-page duplicate meta name attribute",
    ),
)
environment = dict(os.environ)
environment["SITE_DIR"] = str(site_dir)

for path, mutation, description in keyword_mutations:
    original = path.read_text()
    path.write_text(mutation)
    result = subprocess.run(
        [str(checker)],
        cwd=repo_root,
        env=environment,
        capture_output=True,
        text=True,
        check=False,
        timeout=15,
    )
    path.write_text(original)
    if result.returncode == 0:
        raise SystemExit(f"Validator accepted {description}")

root_description = (
    "pycc is a pre-alpha ahead-of-time compiler for typed Python 3.14 with "
    "an implemented native-binary path through Rust and LLVM; AI-created "
    "and human-managed."
)
social_description = (
    "A pre-alpha AOT compiler for typed Python 3.14 with an implemented "
    "native-binary path through Rust and LLVM, created by AI and managed "
    "by a human."
)
source_description = (
    "pycc is a pre-alpha ahead-of-time compiler for typed Python 3.14 with "
    "an implemented path to standalone native binaries; AI-created and "
    "human-managed."
)
description_mutations = (
    (
        "A fully AI-created project, human-managed, building a pre-alpha "
        "ahead-of-time compiler for typed Python 3.14 with Rust and LLVM.",
        "AI-created software shared by agents and independently verified by humans.",
        "AI-created software shared by agents and independently verified by humans.",
        "provenance-first acquisition descriptions",
    ),
    (
        "A production-ready AI compiler that replaces CPython and makes typed "
        "Python native binaries 100x faster.",
        "The production-ready AI compiler that replaces CPython with 100x speedups.",
        "The production-ready AI compiler that replaces CPython with 100x speedups.",
        "unsupported product claims",
    ),
)
for root_replacement, social_replacement, source_replacement, description in description_mutations:
    mutation = original_index
    assert mutation.count(root_description) == 2
    assert mutation.count(social_description) == 2
    assert mutation.count(source_description) == 1
    mutation = mutation.replace(root_description, root_replacement)
    mutation = mutation.replace(social_description, social_replacement)
    mutation = mutation.replace(source_description, source_replacement)
    index_path.write_text(mutation)
    result = subprocess.run(
        [str(checker)],
        cwd=repo_root,
        env=environment,
        capture_output=True,
        text=True,
        check=False,
        timeout=15,
    )
    index_path.write_text(original_index)
    if result.returncode == 0:
        raise SystemExit(f"Validator accepted {description}")

ai_description = (
    "pycc is a pre-alpha AOT compiler for typed Python 3.14 and an AI-native "
    "development experiment: AI agents create the project while a human "
    "manages direction."
)
ai_social_description = (
    "pycc is a pre-alpha AOT compiler for typed Python 3.14, created entirely "
    "by AI agents and managed by a human."
)
assert original_ai.count(ai_description) == 2
assert original_ai.count(ai_social_description) == 2
ai_path.write_text(
    original_ai.replace(
        ai_description,
        "AI agents create every artifact while a human manages the experiment.",
    ).replace(
        ai_social_description,
        "An experiment created by AI agents and managed by a human.",
    )
)
result = subprocess.run(
    [str(checker)],
    cwd=repo_root,
    env=environment,
    capture_output=True,
    text=True,
    check=False,
    timeout=15,
)
ai_path.write_text(original_ai)
if result.returncode == 0:
    raise SystemExit(
        "Validator accepted provenance metadata without compiler context"
    )
PY

cp "$repo_root/site/index.html" "$fixture_root/site/index.html"
python3 - "$fixture_root/site/index.html" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
content = path.read_text()
required = "No project code is handwritten by a human."
assert required in content
replacement = f"Human authorship is not disclosed.<!-- {required} -->"
path.write_text(content.replace(required, replacement, 1))
PY

if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a site without the required AI authorship disclosure" >&2
  exit 1
fi

cp "$repo_root/site/index.html" "$fixture_root/site/index.html"
python3 - "$fixture_root/site/index.html" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
content = path.read_text()
visible_note = '<p class="ai-built-note">'
assert visible_note in content
path.write_text(content.replace(visible_note, '<p class="ai-built-note" hidden>', 1))
PY

if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted an AI authorship disclosure in a hidden subtree" >&2
  exit 1
fi

cp "$repo_root/site/index.html" "$fixture_root/site/index.html"
python3 - "$fixture_root/site/index.html" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
content = path.read_text()
project_id = "https://rotnov.github.io/pycc/#project"
assert project_id in content
path.write_text(content.replace(project_id, f"{project_id}-wrong"))
PY

if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted disconnected JSON-LD project entities" >&2
  exit 1
fi

cp "$repo_root/site/index.html" "$fixture_root/site/index.html"
python3 - "$fixture_root/site/3361fe03d0f44ab7cdbb1a3ce1461821.txt" <<'PY'
from pathlib import Path
import sys

Path(sys.argv[1]).write_text("wrong-key\n")
PY

if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted an invalid IndexNow ownership key" >&2
  exit 1
fi

cp "$repo_root/site/3361fe03d0f44ab7cdbb1a3ce1461821.txt" \
  "$fixture_root/site/3361fe03d0f44ab7cdbb1a3ce1461821.txt"

# <link rel="sitemap"> is not a registered IANA link relation and is not a
# documented sitemap-discovery mechanism. The validator must reject any page
# that re-introduces it, so a standards/Google/Bing-backed submission claim
# cannot silently return as an HTML link relation.
python3 - "$fixture_root/site/index.html" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
content = path.read_text()
canonical = '    <link rel="canonical" href="https://rotnov.github.io/pycc/">'
assert canonical in content
path.write_text(
    content.replace(
        canonical,
        canonical + '\n    <link rel="sitemap" type="application/xml" '
        'href="sitemap.xml">',
        1,
    )
)
PY

if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted the unregistered rel=sitemap link relation on the landing page" >&2
  exit 1
fi

cp "$repo_root/site/index.html" "$fixture_root/site/index.html"

python3 - "$fixture_root/site/architecture/index.html" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
content = path.read_text()
canonical = '    <link rel="canonical" href="https://rotnov.github.io/pycc/architecture/">'
assert canonical in content
path.write_text(
    content.replace(
        canonical,
        canonical + '\n    <link rel="sitemap" type="application/xml" '
        'href="../sitemap.xml">',
        1,
    )
)
PY

if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted the unregistered rel=sitemap link relation on a sub-page" >&2
  exit 1
fi

cp "$repo_root/site/architecture/index.html" \
  "$fixture_root/site/architecture/index.html"

# Case-insensitive rel=sitemap must also be rejected (HTML rel values are
# case-insensitive per the HTML specification).
python3 - "$fixture_root/site/index.html" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
content = path.read_text()
canonical = '    <link rel="canonical" href="https://rotnov.github.io/pycc/">'
assert canonical in content
path.write_text(
    content.replace(
        canonical,
        canonical + '\n    <link rel="Sitemap" type="application/xml" '
        'href="sitemap.xml">',
        1,
    )
)
PY

if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a case-variant rel=Sitemap link relation" >&2
  exit 1
fi

cp "$repo_root/site/index.html" "$fixture_root/site/index.html"

for content_page in \
  status/index.html \
  architecture/index.html \
  python-aot-compilers/index.html \
  ai-native/index.html
do
  if [ ! -f "$fixture_root/site/$content_page" ]; then
    echo "Site fixture is missing required evidence page: $content_page" >&2
    exit 1
  fi
done

mv "$fixture_root/site/python-aot-compilers/index.html" \
  "$fixture_root/site/python-aot-compilers/index.html.missing"
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a site with a missing evidence page" >&2
  exit 1
fi
mv "$fixture_root/site/python-aot-compilers/index.html.missing" \
  "$fixture_root/site/python-aot-compilers/index.html"

python3 - "$fixture_root/site/architecture/index.html" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
content = path.read_text()
canonical = "https://rotnov.github.io/pycc/architecture/"
assert content.count(canonical) >= 1
path.write_text(content.replace(canonical, "https://rotnov.github.io/pycc/status/"))
PY

if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted an evidence page with the wrong canonical URL" >&2
  exit 1
fi

cp "$repo_root/site/architecture/index.html" \
  "$fixture_root/site/architecture/index.html"

python3 - "$repo_root" "$fixture_root/site" <<'PY'
from pathlib import Path
import os
import subprocess
import sys


repo_root = Path(sys.argv[1])
site_dir = Path(sys.argv[2])
checker = repo_root / "scripts" / "check-site.sh"
mutations = (
    (
        site_dir / "index.html",
        "planned permitted CPython interop emits a self-contained",
        "permitted interop is rejected instead of bundled in a",
        "landing page without the native-versus-interop artifact contract",
    ),
    (
        site_dir / "index.html",
        "<code>pycc check</code> now runs the v0.1 frontend",
        "<code>pycc check</code> is not implemented",
        "landing page without the current frontend status",
    ),
    (
        site_dir / "index.html",
        "v0.1 native backend with documented gaps",
        "narrow native backend only",
        "landing page with a superseded backend status",
    ),
    (
        site_dir / "index.html",
        "v0.1 acceptance criteria met (conformance verified on all five Tier-1 targets)",
        "v0.1 acceptance criteria remain unmet",
        "landing page with a superseded acceptance status",
    ),
    (
        site_dir / "index.html",
        "v0.2 acceptance criteria also met; v0.3's class model core has landed",
        "v0.2 acceptance criteria remain unmet",
        "landing page with a superseded v0.2/v0.3 status",
    ),
    (
        site_dir / "status" / "index.html",
        "<strong>Strict checking and inference</strong>",
        "<strong>Type checker placeholder</strong>",
        "status page without the implemented type-system boundary",
    ),
    (
        site_dir / "status" / "index.html",
        "<strong>Implemented v0.1 subset</strong>",
        "<strong>Full v0.1 grammar</strong>",
        "status page that overclaims complete grammar coverage",
    ),
    (
        site_dir / "status" / "index.html",
        "v0.1 and v0.2's acceptance criteria are both met, and v0.3's\n              class model core has landed",
        "v0.1's acceptance criteria remain unmet, and v0.3's\n              class model core has landed",
        "status page that understates v0.1/v0.2 acceptance-checklist completion",
    ),
    (
        site_dir / "status" / "index.html",
        (
            "full multi-version conformance matrix, differential fuzzing,\n"
            "              and corpus testing remain planned test-depth work carried over\n"
            "              from v0.1 and v0.2."
        ),
        "The full multi-version conformance matrix is already complete.",
        "status page that overclaims conformance-matrix completion",
    ),
    (
        site_dir / "status" / "index.html",
        "Unary operators are rejected earlier by\n              HIR lowering with a spanned <code>C0001</code> capability diagnostic,\n              including under <code>pycc check</code>.",
        "Unary operators are rejected only by the backend.",
        "status page that misattributes the unary-expression boundary",
    ),
    (
        site_dir / "status" / "index.html",
        "Parser failures use <code>L0001</code>; byte-exact CLI",
        "Parser failures use <code>L0001</code>; output checks only",
        "status page that overstates diagnostic snapshot coverage",
    ),
    (
        site_dir / "status" / "index.html",
        (
            "currently a\n              greater-than-7.0% regression floor, "
            "enforced by a paired\n              predecessor/candidate measurement"
        ),
        "currently a non-blocking telemetry signal, not enforced",
        "status page that understates perf-gate enforcement",
    ),
    (
        site_dir / "architecture" / "index.html",
        "Planned permitted CPython interop instead adds the",
        "No interop artifact path exists beyond the",
        "architecture page without the planned interop artifact path",
    ),
    (
        site_dir / "architecture" / "index.html",
        "<strong>Resolve and type-check</strong>",
        "<strong>Pass through the type stage</strong>",
        "architecture page without the implemented checker stage",
    ),
    (
        site_dir / "architecture" / "index.html",
        "MIR and code generation cover the implemented v0.1",
        "MIR and code generation remain slice-only",
        "architecture page with a superseded backend boundary",
    ),
    (
        site_dir / "python-aot-compilers" / "index.html",
        (
            "Native and pure pycc builds target a standalone runtime; "
            "planned permitted interop carries a pinned CPython runtime "
            "in the application bundle."
        ),
        "pycc targets only a standalone runtime without CPython.",
        "comparison positioning with an unconditional native-only runtime",
    ),
    (
        site_dir / "python-aot-compilers" / "index.html",
        (
            "Standalone native executable without CPython for native and "
            "pure builds; planned permitted interop bundles a pinned "
            "CPython runtime"
        ),
        "Standalone native executable without CPython is the design target",
        "comparison page with an unconditional CPython-free artifact claim",
    ),
    (
        site_dir / "python-aot-compilers" / "index.html",
        (
            "v0.1 frontend and native backend implemented with documented "
            "gaps; not production-ready"
        ),
        "type checker is a stub and the backend status is unknown",
        "comparison page with superseded pycc positioning",
    ),
    (
        site_dir / "llms.txt",
        "`pycc check` now parses and type-checks the v0.1",
        "`pycc check` is not implemented for the v0.1",
        "llms.txt without the current frontend status",
    ),
    (
        site_dir / "llms.txt",
        "`pycc build` and `pycc run` compile that implemented surface",
        "`pycc build` and `pycc run` compile only a narrow slice",
        "llms.txt with a superseded backend status",
    ),
    (
        site_dir / "llms.txt",
        "pycc is not an AI\nor machine-learning compiler.",
        "pycc is an AI\nand machine-learning compiler.",
        "llms.txt that misclassifies the product as an AI compiler",
    ),
    (
        site_dir / "index.html.md",
        (
            "Native and pure builds emit standalone executables; planned "
            "permitted CPython\ninterop emits a self-contained bundle with "
            "its pinned runtime."
        ),
        "Every build emits only a standalone native executable.",
        "Markdown landing page without the interop artifact distinction",
    ),
    (
        site_dir / "index.html.md",
        "`pycc check` now parses and type-checks the v0.1",
        "`pycc check` is not implemented for the v0.1",
        "Markdown landing page without the current frontend status",
    ),
    (
        site_dir / "index.html.md",
        "`pycc build` and `pycc run` compile that implemented surface",
        "`pycc build` and `pycc run` compile only a narrow slice",
        "Markdown landing page with a superseded backend status",
    ),
    (
        site_dir / "index.html.md",
        "pycc is not an AI\nor machine-learning compiler.",
        "pycc is an AI\nand machine-learning compiler.",
        "Markdown landing page that misclassifies the product as an AI compiler",
    ),
)
environment = dict(os.environ)
environment["SITE_DIR"] = str(site_dir)

for path, required, replacement, description in mutations:
    original = path.read_text()
    if required not in original:
        raise SystemExit(
            f"Fixture is missing current-status mutation target: {required}"
        )
    path.write_text(original.replace(required, replacement, 1))
    result = subprocess.run(
        [str(checker)],
        cwd=repo_root,
        env=environment,
        capture_output=True,
        text=True,
        check=False,
        timeout=15,
    )
    path.write_text(original)
    if result.returncode == 0:
        raise SystemExit(f"Validator accepted a {description}")
PY

python3 - "$fixture_root/site/python-aot-compilers/index.html" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
content = path.read_text()
source = "https://docs.exaloop.io/language/overview/"
assert source in content
path.write_text(content.replace(source, "https://example.com/codon/", 1))
PY

if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a comparison page without its official source link" >&2
  exit 1
fi

cp "$repo_root/site/python-aot-compilers/index.html" \
  "$fixture_root/site/python-aot-compilers/index.html"

python3 - "$fixture_root/site/python-aot-compilers/index.html" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
content = path.read_text()
source = "https://lpython.org/"
assert source in content
path.write_text(content.replace(source, "https://example.com/lpython/", 1))
PY

if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a comparison page without its LPython project source" >&2
  exit 1
fi

cp "$repo_root/site/python-aot-compilers/index.html" \
  "$fixture_root/site/python-aot-compilers/index.html"
python3 - "$fixture_root/site/python-aot-compilers/index.html" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
content = path.read_text()
status = "Alpha; focused on numerical and array-oriented typed Python"
assert status in content
path.write_text(content.replace(status, "Available typed Python compiler", 1))
PY

if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a comparison page without LPython maturity evidence" >&2
  exit 1
fi

cp "$repo_root/site/python-aot-compilers/index.html" \
  "$fixture_root/site/python-aot-compilers/index.html"
python3 - "$fixture_root/site/python-aot-compilers/index.html" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
content = path.read_text()
disclosure = "Do not choose pycc for production today."
assert disclosure in content
path.write_text(content.replace(disclosure, "Choose pycc for production today.", 1))
PY

if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a comparison page without its pre-alpha warning" >&2
  exit 1
fi

cp "$repo_root/site/python-aot-compilers/index.html" \
  "$fixture_root/site/python-aot-compilers/index.html"
python3 - "$fixture_root/site/python-aot-compilers/index.html" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
content = path.read_text()
disclosure = "Benchmarks</strong> none claimed"
assert disclosure in content
path.write_text(content.replace(disclosure, "Benchmarks</strong> not disclosed", 1))
PY

if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a comparison page without its no-benchmark disclosure" >&2
  exit 1
fi

cp "$repo_root/site/python-aot-compilers/index.html" \
  "$fixture_root/site/python-aot-compilers/index.html"
cp "$repo_root/site/python-aot-compilers/claims.json" \
  "$fixture_root/site/python-aot-compilers/claims.json"
python3 - "$repo_root" "$fixture_root/site" <<'PY'
from pathlib import Path
import json
import os
import subprocess
import sys


repo_root = Path(sys.argv[1])
site_dir = Path(sys.argv[2])
checker = repo_root / "scripts" / "check-site.sh"
comp_html = site_dir / "python-aot-compilers" / "index.html"
claims_json = site_dir / "python-aot-compilers" / "claims.json"
original_html = comp_html.read_text()
original_claims = claims_json.read_text()
environment = dict(os.environ)
environment["SITE_DIR"] = str(site_dir)


def run_checker():
    return subprocess.run(
        [str(checker)],
        cwd=repo_root,
        env=environment,
        capture_output=True,
        text=True,
        check=False,
        timeout=15,
    )


def restore():
    comp_html.write_text(original_html)
    claims_json.write_text(original_claims)


def expect_reject(description, fn):
    fn()
    result = run_checker()
    restore()
    if result.returncode == 0:
        raise SystemExit(
            f"Validator accepted {description}"
        )


def expect_accept(description, fn):
    fn()
    result = run_checker()
    restore()
    if result.returncode != 0:
        raise SystemExit(
            f"Validator rejected {description}"
        )


# --- Value-false mutations (corrupt HTML cell, keep sources intact) ---

codon_output = "Native machine code without interpreter runtime overhead"
codon_false = "Requires the CPython interpreter at runtime"
expect_reject(
    "Codon output cell corrupted to require CPython",
    lambda: comp_html.write_text(
        original_html.replace(codon_output, codon_false, 1)
    ),
)

nuitka_output = (
    "Executable or extension; standalone and onefile modes "
    "package required runtime dependencies"
)
nuitka_false = (
    "Pure standalone native executable; no CPython or runtime dependencies"
)
expect_reject(
    "Nuitka output cell corrupted to claim no CPython dependency",
    lambda: comp_html.write_text(
        original_html.replace(nuitka_output, nuitka_false, 1)
    ),
)

mypyc_output = "Native C extension imported and run by CPython"
mypyc_false = "Standalone native executable that does not use CPython"
expect_reject(
    "mypyc output cell corrupted to claim standalone executable",
    lambda: comp_html.write_text(
        original_html.replace(mypyc_output, mypyc_false, 1)
    ),
)

cython_output = (
    "Generated C or C++; commonly a CPython extension, with a "
    "documented CPython-embedding executable path"
)
cython_false = (
    "Pure standalone C/C++ with no Python interpreter dependency"
)
expect_reject(
    "Cython output cell corrupted to claim no Python interpreter dependency",
    lambda: comp_html.write_text(
        original_html.replace(cython_output, cython_false, 1)
    ),
)

lpython_output = (
    "AOT binary via LLVM; C, C++, and WASM are also documented "
    "backends, with optional CPython interop"
)
lpython_false = "CPython C extension only; no native executable backend"
expect_reject(
    "LPython output cell corrupted to claim CPython extension only",
    lambda: comp_html.write_text(
        original_html.replace(lpython_output, lpython_false, 1)
    ),
)

mypyc_positioning = (
    "Alpha; accelerating typed Python modules while keeping "
    "CPython interoperability"
)
mypyc_no_alpha = (
    "Accelerating typed Python modules while keeping CPython interoperability"
)
expect_reject(
    "mypyc positioning with alpha label removed",
    lambda: comp_html.write_text(
        original_html.replace(mypyc_positioning, mypyc_no_alpha, 1)
    ),
)

mypyc_stable = (
    "Stable; accelerating typed Python modules while keeping "
    "CPython interoperability"
)
expect_reject(
    "mypyc positioning relabeled as Stable",
    lambda: comp_html.write_text(
        original_html.replace(mypyc_positioning, mypyc_stable, 1)
    ),
)

# --- Model-HTML mismatch mutations ---

# Change claims.json without changing HTML
def corrupt_claims_codon():
    data = json.loads(original_claims)
    for e in data["entities"]:
        if e["name"] == "Codon":
            e["html_output_cell"] = "Requires the CPython interpreter at runtime"
    claims_json.write_text(json.dumps(data, indent=2))

expect_reject(
    "claims.json Codon value changed without HTML update",
    corrupt_claims_codon,
)

# Change HTML cell without changing claims.json
def corrupt_html_codon():
    comp_html.write_text(
        original_html.replace(codon_output, "Different output text", 1)
    )

expect_reject(
    "HTML Codon cell changed without claims.json update",
    corrupt_html_codon,
)

# Add entity to HTML not in claims.json
def add_html_entity():
    new_row = (
        "                  <tr>\n"
        '                    <th scope="row">ExtraTool</th>\n'
        "                    <td>Some language</td>\n"
        "                    <td>Some output</td>\n"
        "                    <td>Some positioning</td>\n"
        "                  </tr>\n"
    )
    comp_html.write_text(
        original_html.replace(
            "                </tbody>\n",
            new_row + "                </tbody>\n",
            1,
        )
    )

expect_reject(
    "extra HTML entity not in claims.json",
    add_html_entity,
)

# Remove entity from HTML still in claims.json
def remove_html_entity():
    start = original_html.index('<tr>\n                    <th scope="row">Codon</th>')
    end = original_html.index("                  </tr>\n", start) + len("                  </tr>\n")
    comp_html.write_text(original_html[:start] + original_html[end:])

expect_reject(
    "HTML entity removed but still in claims.json",
    remove_html_entity,
)

# --- Model integrity mutations ---

# Delete claims.json
def delete_claims():
    claims_json.unlink()

expect_reject(
    "missing claims.json",
    delete_claims,
)

# Malform claims.json
def malform_claims():
    claims_json.write_text("{ invalid json ]")

expect_reject(
    "malformed claims.json",
    malform_claims,
)

# Entity with no sources
def no_sources():
    data = json.loads(original_claims)
    for e in data["entities"]:
        if e["name"] == "Codon":
            e["sources"] = []
    claims_json.write_text(json.dumps(data, indent=2))

expect_reject(
    "claims.json entity with no sources",
    no_sources,
)

# Maturity mismatch: model says stable, HTML says Alpha
def maturity_mismatch():
    data = json.loads(original_claims)
    for e in data["entities"]:
        if e["name"] == "mypyc":
            e["positioning"] = (
                "Stable; accelerating typed Python modules while keeping "
                "CPython interoperability"
            )
    claims_json.write_text(json.dumps(data, indent=2))

expect_reject(
    "claims.json maturity mismatch with HTML positioning",
    maturity_mismatch,
)

# Empty maturity
def empty_maturity():
    data = json.loads(original_claims)
    for e in data["entities"]:
        if e["name"] == "Codon":
            e["maturity"] = ""
    claims_json.write_text(json.dumps(data, indent=2))

expect_reject(
    "claims.json entity with empty maturity",
    empty_maturity,
)

# --- Positive control: minor whitespace change should still pass ---

def whitespace_tolerant():
    comp_html.write_text(
        original_html.replace(
            "<td>Native machine code without interpreter runtime overhead</td>",
            "<td>  Native machine code without interpreter runtime overhead  </td>",
            1,
        )
    )

expect_accept(
    "minor whitespace change in comparison cell",
    whitespace_tolerant,
)

# --- Landing-page projection mutations ---

landing_html = site_dir / "index.html"
original_landing = landing_html.read_text()


def restore_landing():
    landing_html.write_text(original_landing)
    claims_json.write_text(original_claims)


def expect_reject_landing(description, fn):
    fn()
    result = run_checker()
    restore_landing()
    if result.returncode == 0:
        raise SystemExit(
            f"Validator accepted {description}"
        )


def expect_accept_landing(description, fn):
    fn()
    result = run_checker()
    restore_landing()
    if result.returncode != 0:
        raise SystemExit(
            f"Validator rejected {description}"
        )


# A: Landing HTML pycc Output artifact corrupted (binding)
def landing_pycc_output_corrupt():
    landing_html.write_text(
        original_landing.replace(
            '<td><span class="yes">Standalone target</span></td>',
            '<td><span class="yes">C extension</span></td>',
            1,
        )
    )

expect_reject_landing(
    "landing HTML pycc Output artifact corrupted to C extension",
    landing_pycc_output_corrupt,
)

# B: Landing HTML remove mypyc <tr> (entity set)
def landing_remove_mypyc():
    start = original_landing.index(
        '              <tr>\n                <th scope="row">mypyc</th>'
    )
    end = original_landing.index("              </tr>\n", start) + len(
        "              </tr>\n"
    )
    landing_html.write_text(original_landing[:start] + original_landing[end:])

expect_reject_landing(
    "landing HTML mypyc row removed",
    landing_remove_mypyc,
)

# C: Landing HTML add extra ExtraTool <tr> (entity set)
def landing_add_extra_entity():
    new_row = (
        "              <tr>\n"
        '                <th scope="row">ExtraTool</th>\n'
        "                <td>Some language</td>\n"
        "                <td>Some output</td>\n"
        "                <td>Some contract</td>\n"
        "              </tr>\n"
    )
    landing_html.write_text(
        original_landing.replace(
            "            </tbody>\n",
            new_row + "            </tbody>\n",
            1,
        )
    )

expect_reject_landing(
    "landing HTML extra ExtraTool entity not in projection",
    landing_add_extra_entity,
)

# D: claims.json labels.pycc.output_artifact changed without HTML (binding)
def landing_label_drift():
    data = json.loads(original_claims)
    data["landing_projection"]["labels"]["pycc"]["output_artifact"] = (
        "C extension"
    )
    claims_json.write_text(json.dumps(data, indent=2))

expect_reject_landing(
    "claims.json labels.pycc.output_artifact changed without landing HTML",
    landing_label_drift,
)

# E: claims.json anchors.pycc.output_artifact changed to absent token (anchor)
def landing_anchor_absent():
    data = json.loads(original_claims)
    data["landing_projection"]["anchors"]["pycc"]["output_artifact"] = (
        "NonexistentToken"
    )
    claims_json.write_text(json.dumps(data, indent=2))

expect_reject_landing(
    "claims.json anchors.pycc.output_artifact set to absent token",
    landing_anchor_absent,
)

# F: claims.json pycc html_output_cell drops "Standalone" AND co-updates
#    the detailed HTML cell to match (cross-projection contradiction)
def landing_cross_projection_contradiction():
    data = json.loads(original_claims)
    for e in data["entities"]:
        if e["name"] == "pycc":
            e["html_output_cell"] = e["html_output_cell"].replace(
                "Standalone ", "", 1
            )
    claims_json.write_text(json.dumps(data, indent=2))
    comp_html.write_text(
        original_html.replace(
            "Standalone native executable without CPython for native and pure builds; planned permitted interop bundles a pinned CPython runtime",
            "native executable without CPython for native and pure builds; planned permitted interop bundles a pinned CPython runtime",
            1,
        )
    )

expect_reject_landing(
    "cross-projection contradiction: pycc html_output_cell drops Standalone "
    "with co-updated detailed HTML",
    landing_cross_projection_contradiction,
)
comp_html.write_text(original_html)

# G: claims.json add Cython to labels/anchors without landing <tr> (entity set)
def landing_add_cython_projection():
    data = json.loads(original_claims)
    data["landing_projection"]["labels"]["Cython"] = {
        "static_model": "Python superset",
        "output_artifact": "C or C++",
        "language_contract": "Python superset",
    }
    data["landing_projection"]["anchors"]["Cython"] = {
        "static_model": "superset",
        "output_artifact": "C",
        "language_contract": "Python",
    }
    claims_json.write_text(json.dumps(data, indent=2))

expect_reject_landing(
    "claims.json Cython added to projection without landing <tr>",
    landing_add_cython_projection,
)

# H: Landing HTML remove mini-mark class so >_ is no longer skipped (entity set)
def landing_remove_mini_mark():
    landing_html.write_text(
        original_landing.replace(
            '<span class="mini-mark">&gt;_</span> pycc',
            '<span>&gt;_</span> pycc',
            1,
        )
    )

expect_reject_landing(
    "landing HTML mini-mark class removed, row key becomes '>_ pycc'",
    landing_remove_mini_mark,
)

# I: Positive control — minor extra whitespace inside a landing <td> (accept)
def landing_whitespace_tolerant():
    landing_html.write_text(
        original_landing.replace(
            "<td>Typed subset</td>",
            "<td>  Typed subset  </td>",
            1,
        )
    )

expect_accept_landing(
    "minor whitespace change in landing cell",
    landing_whitespace_tolerant,
)

# J: claims.json anchors.pycc.output_artifact set to empty string (blank anchor)
def landing_blank_anchor():
    data = json.loads(original_claims)
    data["landing_projection"]["anchors"]["pycc"]["output_artifact"] = ""
    claims_json.write_text(json.dumps(data, indent=2))

expect_reject_landing(
    "claims.json anchors.pycc.output_artifact set to empty string",
    landing_blank_anchor,
)

# K: Landing HTML swap Static model and Output artifact column headers (header order)
def landing_swap_col_headers():
    landing_html.write_text(
        original_landing.replace(
            '<th scope="col">Static model</th>\n                <th scope="col">Output artifact</th>',
            '<th scope="col">Output artifact</th>\n                <th scope="col">Static model</th>',
            1,
        )
    )

expect_reject_landing(
    "landing HTML column headers swapped (Static model <-> Output artifact)",
    landing_swap_col_headers,
)
PY

cp "$repo_root/site/python-aot-compilers/index.html" \
  "$fixture_root/site/python-aot-compilers/index.html"
cp "$repo_root/site/python-aot-compilers/claims.json" \
  "$fixture_root/site/python-aot-compilers/claims.json"
cp "$repo_root/site/sitemap.xml" "$fixture_root/site/sitemap.xml"
python3 - "$fixture_root/site/sitemap.xml" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
content = path.read_text()
entry = """    <loc>https://rotnov.github.io/pycc/</loc>
    <lastmod>2026-08-14</lastmod>"""
assert entry in content
path.write_text(content.replace(entry, entry + "\n    <lastmod>2026-07-30</lastmod>", 1))
PY

if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a sitemap URL entry with duplicate lastmod elements" >&2
  exit 1
fi

cp "$repo_root/site/sitemap.xml" "$fixture_root/site/sitemap.xml"
python3 - "$fixture_root/site/sitemap.xml" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
content = path.read_text()
lastmod = "<lastmod>2026-08-14</lastmod>"
assert lastmod in content
path.write_text(content.replace(lastmod, "<lastmod>not-a-date</lastmod>", 1))
PY

if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a malformed sitemap lastmod" >&2
  exit 1
fi

cp "$repo_root/site/sitemap.xml" "$fixture_root/site/sitemap.xml"
python3 - "$fixture_root/site/sitemap.xml" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
content = path.read_text()
lastmod = "<lastmod>2026-08-14</lastmod>"
assert lastmod in content
path.write_text(content.replace(lastmod, "<lastmod>9999-12-31</lastmod>", 1))
PY

if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a future sitemap lastmod" >&2
  exit 1
fi

cp "$repo_root/site/sitemap.xml" "$fixture_root/site/sitemap.xml"
python3 - "$fixture_root/site/sitemap.xml" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
content = path.read_text()
entry = """    <loc>https://rotnov.github.io/pycc/python-aot-compilers/</loc>
    <lastmod>2026-08-14</lastmod>"""
assert entry in content
path.write_text(content.replace(entry, entry.replace("2026-08-14", "2026-07-27"), 1))
PY

if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a sitemap lastmod that disagrees with page dateModified" >&2
  exit 1
fi

cp "$repo_root/site/sitemap.xml" "$fixture_root/site/sitemap.xml"
python3 - "$fixture_root/site/sitemap.xml" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
content = path.read_text()
required = """  <url>
    <loc>https://rotnov.github.io/pycc/python-aot-compilers/</loc>"""
assert required in content
start = content.index(required)
end = content.index("  </url>", start) + len("  </url>\n")
path.write_text(content[:start] + content[end:])
PY

if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a sitemap that omitted an evidence page" >&2
  exit 1
fi

cp "$repo_root/site/sitemap.xml" "$fixture_root/site/sitemap.xml"
python3 - "$fixture_root/site/sitemap.xml" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
content = path.read_text()
required = "    <loc>https://rotnov.github.io/pycc/</loc>"
assert required in content
path.write_text(
    content.replace(
        required,
        required + "\n"
        "    <loc>https://rotnov.github.io/pycc/unexpected/</loc>",
        1,
    )
)
PY

if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a sitemap URL entry with multiple loc elements" >&2
  exit 1
fi

cp "$repo_root/site/sitemap.xml" "$fixture_root/site/sitemap.xml"
python3 - "$fixture_root/site/sitemap.xml" <<'PY'
from pathlib import Path
import sys
import xml.etree.ElementTree as ET

path = Path(sys.argv[1])
namespace = "http://www.sitemaps.org/schemas/sitemap/0.9"
ET.register_namespace("", namespace)
root = ET.parse(path).getroot()
root[:] = list(reversed(root[:]))
ET.ElementTree(root).write(path, encoding="unicode", xml_declaration=True)
PY

if ! SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator rejected a complete sitemap solely because URL order changed" >&2
  exit 1
fi

# --- Favicon mutations ---

rm "$fixture_root/site/favicon.svg"
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a site with a missing favicon.svg asset" >&2
  exit 1
fi

cp "$repo_root/site/favicon.svg" "$fixture_root/site/favicon.svg"

python3 - "$fixture_root/site/favicon.svg" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
path.write_text("<svg><rect></svg>")
PY
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a malformed favicon.svg" >&2
  exit 1
fi

cp "$repo_root/site/favicon.svg" "$fixture_root/site/favicon.svg"

python3 - "$fixture_root/site/index.html" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
content = path.read_text()
favicon = '<link rel="icon" type="image/svg+xml" href="favicon.svg">'
assert favicon in content
path.write_text(content.replace(favicon, "", 1))
PY
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a landing page without a favicon link" >&2
  exit 1
fi

cp "$repo_root/site/index.html" "$fixture_root/site/index.html"

python3 - "$fixture_root/site/architecture/index.html" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
content = path.read_text()
favicon = '<link rel="icon" type="image/svg+xml" href="../favicon.svg">'
assert favicon in content
path.write_text(content.replace(favicon, "", 1))
PY
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted an evidence page without a favicon link" >&2
  exit 1
fi

cp "$repo_root/site/architecture/index.html" \
  "$fixture_root/site/architecture/index.html"

python3 - "$fixture_root/site/architecture/index.html" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
content = path.read_text()
path.write_text(
    content.replace(
        'href="../favicon.svg"',
        'href="favicon.svg"',
        1,
    )
)
PY
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted an evidence page with the wrong favicon path" >&2
  exit 1
fi

cp "$repo_root/site/architecture/index.html" \
  "$fixture_root/site/architecture/index.html"

# Wrong type attribute on favicon link
python3 - "$fixture_root/site/index.html" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
content = path.read_text()
path.write_text(
    content.replace(
        'type="image/svg+xml"',
        'type="image/png"',
        1,
    )
)
PY
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a favicon link with the wrong type attribute" >&2
  exit 1
fi

cp "$repo_root/site/index.html" "$fixture_root/site/index.html"

# Extra attributes on favicon link
python3 - "$fixture_root/site/index.html" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
content = path.read_text()
path.write_text(
    content.replace(
        '<link rel="icon" type="image/svg+xml" href="favicon.svg">',
        '<link rel="icon" type="image/svg+xml" href="favicon.svg" sizes="any">',
        1,
    )
)
PY
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a favicon link with extra attributes" >&2
  exit 1
fi

cp "$repo_root/site/index.html" "$fixture_root/site/index.html"

# Oversized favicon.svg
python3 - "$fixture_root/site/favicon.svg" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
# Valid SVG root but exceeding the 1KB limit
path.write_text(
    '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1 1">'
    + '<text>' + 'x' * 2048 + '</text>'
    + '</svg>'
)
PY
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted an oversized favicon.svg" >&2
  exit 1
fi

cp "$repo_root/site/favicon.svg" "$fixture_root/site/favicon.svg"

# Valid XML but wrong root tag
python3 - "$fixture_root/site/favicon.svg" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
path.write_text('<html xmlns="http://www.w3.org/1999/xhtml"></html>')
PY
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a favicon.svg with a non-SVG root element" >&2
  exit 1
fi

cp "$repo_root/site/favicon.svg" "$fixture_root/site/favicon.svg"

# Duplicate favicon link on landing page
python3 - "$fixture_root/site/index.html" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
content = path.read_text()
favicon = '<link rel="icon" type="image/svg+xml" href="favicon.svg">'
assert favicon in content
path.write_text(content.replace(favicon, favicon + "\n    " + favicon, 1))
PY
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a landing page with a duplicate favicon link" >&2
  exit 1
fi

cp "$repo_root/site/index.html" "$fixture_root/site/index.html"

# Wrong favicon path on landing page (using nested path)
python3 - "$fixture_root/site/index.html" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
content = path.read_text()
path.write_text(
    content.replace(
        'href="favicon.svg"',
        'href="../favicon.svg"',
        1,
    )
)
PY
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a landing page with a nested favicon path" >&2
  exit 1
fi

cp "$repo_root/site/index.html" "$fixture_root/site/index.html"

# --- 404 page mutations ---

cp "$repo_root/site/404.html" "$fixture_root/site/404.html"

printf '' > "$fixture_root/site/404.html"
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted an empty 404.html" >&2
  exit 1
fi

cp "$repo_root/site/404.html" "$fixture_root/site/404.html"
python3 - "$fixture_root/site/404.html" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
content = path.read_text()
required = '<meta name="robots" content="noindex">'
assert required in content
path.write_text(content.replace(required, "", 1))
PY

if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a 404 page without the noindex robots directive" >&2
  exit 1
fi

cp "$repo_root/site/404.html" "$fixture_root/site/404.html"
python3 - "$fixture_root/site/404.html" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
content = path.read_text()
required = "<h1 id=\"not-found-title\">Page not found</h1>"
assert required in content
path.write_text(content.replace(required, "<h1 id=\"not-found-title\">Welcome</h1>", 1))
PY

if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a 404 page without a not-found heading" >&2
  exit 1
fi

cp "$repo_root/site/404.html" "$fixture_root/site/404.html"
python3 - "$fixture_root/site/404.html" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
content = path.read_text()
assert content.count('href="/pycc/"') >= 2
path.write_text(content.replace('href="/pycc/"', 'href="/pycc/status/"'))
PY

if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a 404 page without a home link" >&2
  exit 1
fi

cp "$repo_root/site/404.html" "$fixture_root/site/404.html"
python3 - "$fixture_root/site/404.html" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
content = path.read_text()
required = '<a href="/pycc/status/">Status</a>'
assert required in content
path.write_text(content.replace(required, "", 1))
PY

if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a 404 page without an evidence page link" >&2
  exit 1
fi

cp "$repo_root/site/404.html" "$fixture_root/site/404.html"
python3 - "$fixture_root/site/404.html" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
content = path.read_text()
required = 'href="/pycc/styles.css"'
assert required in content
path.write_text(content.replace(required, 'href="styles.css"', 1))
PY

if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a 404 page with a relative asset URL" >&2
  exit 1
fi

cp "$repo_root/site/404.html" "$fixture_root/site/404.html"
python3 - "$fixture_root/site/404.html" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
content = path.read_text()
required = '<a href="/pycc/architecture/">Architecture</a>'
assert required in content
path.write_text(content.replace(required, '<a href="architecture/">Architecture</a>', 1))
PY

if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a 404 page with a relative navigation link" >&2
  exit 1
fi

cp "$repo_root/site/404.html" "$fixture_root/site/404.html"

# --- README comparison table mutation tests (Part 2 of #162) ---

# Value-false mutation: corrupt a README comparison cell while keeping
# the claims.json model intact. The validator must reject the mismatch.
cp "$repo_root/README.md" "$fixture_root/README.md"
python3 - "$fixture_root/README.md" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
content = path.read_text()
# Corrupt the Codon type_enforcement cell
content = content.replace(
    "| Codon | ✅ static language |",
    "| Codon | ❌ no type enforcement |",
    1
)
path.write_text(content)
PY

if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a corrupted README comparison cell (Codon type_enforcement)" >&2
  exit 1
fi

# Model-HTML mismatch: change the claims.json readme_projection label
# without changing the README. The validator must reject the mismatch.
cp "$repo_root/README.md" "$fixture_root/README.md"
cp "$repo_root/site/python-aot-compilers/claims.json" "$fixture_root/site/python-aot-compilers/claims.json"
python3 - "$fixture_root/site/python-aot-compilers/claims.json" <<'PY'
from pathlib import Path
import sys
import json

path = Path(sys.argv[1])
data = json.loads(path.read_text())
# Corrupt the pycc native_executable label
data["readme_projection"]["labels"]["pycc"]["native_executable"] = "CPython interpreter required"
path.write_text(json.dumps(data, indent=2) + "\n")
PY

if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a corrupted claims.json readme_projection label" >&2
  exit 1
fi

# Entity-set mutation: add an extra entity to the README table.
cp "$repo_root/README.md" "$fixture_root/README.md"
python3 - "$fixture_root/README.md" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
content = path.read_text()
# Add a fake row after Cython
content = content.replace(
    "| Cython | ⚠️ optional | ❌ extension or embedded CPython | ⚠️ Python superset |",
    "| Cython | ⚠️ optional | ❌ extension or embedded CPython | ⚠️ Python superset |\n| FakeCompiler | ✅ | ✅ | ✅ |",
    1
)
path.write_text(content)
PY

if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted an extra entity in the README comparison table" >&2
  exit 1
fi

cp "$repo_root/README.md" "$fixture_root/README.md"
cp "$repo_root/site/python-aot-compilers/claims.json" "$fixture_root/site/python-aot-compilers/claims.json"

# --- Issue #203: SoftwareSourceCode JSON-LD semantic bindings ---

# Mutation: change programmingLanguage to Python (false).
cp "$repo_root/site/index.html" "$fixture_root/site/index.html"
python3 - "$fixture_root/site/index.html" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
content = path.read_text()
content = content.replace(
    '"programmingLanguage": "Rust"',
    '"programmingLanguage": "Python"',
    1
)
path.write_text(content)
PY
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted programmingLanguage=Python (issue #203)" >&2
  exit 1
fi

# Mutation: add runtimePlatform back (misleading).
cp "$repo_root/site/index.html" "$fixture_root/site/index.html"
python3 - "$fixture_root/site/index.html" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
content = path.read_text()
content = content.replace(
    '"programmingLanguage": "Rust",',
    '"programmingLanguage": "Rust",\n            "runtimePlatform": "LLVM",',
    1
)
path.write_text(content)
PY
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted runtimePlatform=LLVM (issue #203)" >&2
  exit 1
fi

# Mutation: change license to GPL (false).
cp "$repo_root/site/index.html" "$fixture_root/site/index.html"
python3 - "$fixture_root/site/index.html" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
content = path.read_text()
content = content.replace(
    '"license": "https://opensource.org/license/mit"',
    '"license": "https://www.gnu.org/licenses/gpl-3.0.html"',
    1
)
path.write_text(content)
PY
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted license=GPL (issue #203)" >&2
  exit 1
fi

# Mutation: add "AI compiler" to keywords.
cp "$repo_root/site/index.html" "$fixture_root/site/index.html"
python3 - "$fixture_root/site/index.html" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
content = path.read_text()
content = content.replace(
    '"autonomous software development"',
    '"autonomous software development",\n              "AI compiler"',
    1
)
path.write_text(content)
PY
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted 'AI compiler' keyword (issue #203)" >&2
  exit 1
fi

# Mutation: change name to something false.
cp "$repo_root/site/index.html" "$fixture_root/site/index.html"
python3 - "$fixture_root/site/index.html" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
content = path.read_text()
content = content.replace(
    '"@type": "SoftwareSourceCode",\n            "@id": "https://rotnov.github.io/pycc/#project",\n            "name": "pycc"',
    '"@type": "SoftwareSourceCode",\n            "@id": "https://rotnov.github.io/pycc/#project",\n            "name": "pycc-compiler"'
)
path.write_text(content)
PY
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted name=pycc-compiler (issue #203)" >&2
  exit 1
fi

# Mutation: change alternateName to something false.
cp "$repo_root/site/index.html" "$fixture_root/site/index.html"
python3 - "$fixture_root/site/index.html" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
content = path.read_text()
content = content.replace(
    '"alternateName": "pycc Python compiler"',
    '"alternateName": "pycc AI compiler"',
    1
)
path.write_text(content)
PY
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted alternateName=pycc AI compiler (issue #203)" >&2
  exit 1
fi

# Mutation: change SoftwareSourceCode url to a non-canonical URL.
cp "$repo_root/site/index.html" "$fixture_root/site/index.html"
python3 - "$fixture_root/site/index.html" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
content = path.read_text()
content = content.replace(
    '"url": "https://rotnov.github.io/pycc/",\n            "mainEntityOfPage"',
    '"url": "https://github.com/rotnov/pycc",\n            "mainEntityOfPage"',
    1
)
path.write_text(content)
PY
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted non-canonical SoftwareSourceCode url (issue #203)" >&2
  exit 1
fi

# Mutation: add production-ready claim to description.
cp "$repo_root/site/index.html" "$fixture_root/site/index.html"
python3 - "$fixture_root/site/index.html" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
content = path.read_text()
content = content.replace(
    'AI-created and human-managed."',
    'AI-created and human-managed. Production-ready."',
    1
)
path.write_text(content)
PY
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted production-ready claim in JSON-LD (issue #203)" >&2
  exit 1
fi

cp "$repo_root/site/index.html" "$fixture_root/site/index.html"

# --- Issue #206: Markdown landing semantic contract mutations ---

# Mutation: remove the ROADMAP link from the Markdown.
cp "$repo_root/site/index.html.md" "$fixture_root/site/index.html.md"
python3 - "$fixture_root/site/index.html.md" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
content = path.read_text()
content = content.replace(
    "- [Roadmap](https://github.com/rotnov/pycc/blob/main/docs/ROADMAP.md)\n",
    ""
)
path.write_text(content)
PY
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted Markdown without ROADMAP link (issue #206)" >&2
  exit 1
fi

# Mutation: remove the conformance claim from the Markdown.
cp "$repo_root/site/index.html.md" "$fixture_root/site/index.html.md"
python3 - "$fixture_root/site/index.html.md" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
content = path.read_text()
content = content.replace("mandelbrot-ascii", "another-test")
path.write_text(content)
PY
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted Markdown without mandelbrot-ascii claim (issue #206)" >&2
  exit 1
fi

# Mutation: add production-ready claim to the Markdown.
cp "$repo_root/site/index.html.md" "$fixture_root/site/index.html.md"
python3 - "$fixture_root/site/index.html.md" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
content = path.read_text()
content = content.replace(
    "pycc is pre-alpha and is not ready for production.",
    "pycc is pre-alpha and is not ready for production. It is production-ready."
)
path.write_text(content)
PY
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted Markdown with production-ready claim (issue #206)" >&2
  exit 1
fi

# Mutation: remove the code example feature mention.
cp "$repo_root/site/index.html.md" "$fixture_root/site/index.html.md"
python3 - "$fixture_root/site/index.html.md" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
content = path.read_text()
content = content.replace(
    "recursive_fibonacci_matches_the_well_known_sequence",
    "some_other_test"
)
path.write_text(content)
PY
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted Markdown without conformance test reference (issue #206)" >&2
  exit 1
fi

cp "$repo_root/site/index.html.md" "$fixture_root/site/index.html.md"

# --- Issue #207: llms.txt bounded and Markdown-first ---

# Mutation: split the single-line blockquote summary into multiple lines.
cp "$repo_root/site/llms.txt" "$fixture_root/site/llms.txt"
python3 - "$fixture_root/site/llms.txt" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
content = path.read_text()
single_line = (
    "> pycc is a pre-alpha strict ahead-of-time compiler for typed, standard Python 3.14 "
    "with an implemented native-binary path through Rust and LLVM. AI agents create it, "
    "and a human manages it."
)
multi_line = (
    "> pycc is a pre-alpha strict ahead-of-time compiler for typed, standard Python\n"
    "> 3.14 with an implemented native-binary path through Rust and LLVM. AI agents\n"
    "> create it, and a human manages it."
)
content = content.replace(single_line, multi_line)
path.write_text(content)
PY
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted multi-line blockquote summary (issue #207)" >&2
  exit 1
fi

# Mutation: remove the Markdown landing link.
cp "$repo_root/site/llms.txt" "$fixture_root/site/llms.txt"
python3 - "$fixture_root/site/llms.txt" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
content = path.read_text()
content = content.replace(
    "- [Markdown landing](https://rotnov.github.io/pycc/index.html.md): "
    "Clean text equivalent of the landing page for agents and constrained clients.\n",
    ""
)
path.write_text(content)
PY
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted llms.txt without Markdown landing link (issue #207)" >&2
  exit 1
fi

cp "$repo_root/site/llms.txt" "$fixture_root/site/llms.txt"
cp "$repo_root/site/llms-txt-context-manifest.json" "$fixture_root/site/llms-txt-context-manifest.json"

# --- Issue #207: bounded Markdown-first expansion mutation tests ---

# Mutation: replace a raw.githubusercontent.com URL with a GitHub blob URL in a
# non-optional section. The validator must reject application-shell HTML where
# a raw tracked Markdown document is expected.
cp "$repo_root/site/llms.txt" "$fixture_root/site/llms.txt"
python3 - "$fixture_root/site/llms.txt" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
content = path.read_text()
content = content.replace(
    "[README](https://raw.githubusercontent.com/rotnov/pycc/main/README.md)",
    "[README](https://github.com/rotnov/pycc/blob/main/README.md)",
)
path.write_text(content)
PY
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted GitHub blob URL in non-optional section (issue #207)" >&2
  exit 1
fi

# Mutation: move the Source repository (a large GitHub UI page) from Optional
# into the non-optional Project section. The validator must reject a large
# human-navigation-only resource that breaches the bounded default expansion.
cp "$repo_root/site/llms.txt" "$fixture_root/site/llms.txt"
python3 - "$fixture_root/site/llms.txt" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
content = path.read_text()
content = content.replace(
    "- [Source repository](https://github.com/rotnov/pycc): Public source, "
    "tests, issues, pull requests, and development history.\n",
    "",
)
content = content.replace(
    "- [Markdown landing](https://rotnov.github.io/pycc/index.html.md): "
    "Clean text equivalent of the landing page for agents and constrained clients.\n",
    "- [Markdown landing](https://rotnov.github.io/pycc/index.html.md): "
    "Clean text equivalent of the landing page for agents and constrained clients.\n"
    "- [Source repository](https://github.com/rotnov/pycc): Public source, "
    "tests, issues, pull requests, and development history.\n",
)
path.write_text(content)
PY
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a non-optional link absent from the manifest (issue #207)" >&2
  exit 1
fi

# Mutation: add the canonical HTML landing to the non-optional Project section,
# duplicating the Markdown landing representation. The validator must reject
# duplicate default HTML+Markdown representations of the same page.
cp "$repo_root/site/llms.txt" "$fixture_root/site/llms.txt"
python3 - "$fixture_root/site/llms.txt" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
content = path.read_text()
content = content.replace(
    "- [Markdown landing](https://rotnov.github.io/pycc/index.html.md): "
    "Clean text equivalent of the landing page for agents and constrained clients.\n",
    "- [Canonical website](https://rotnov.github.io/pycc/): Human-readable project overview.\n"
    "- [Markdown landing](https://rotnov.github.io/pycc/index.html.md): "
    "Clean text equivalent of the landing page for agents and constrained clients.\n",
)
path.write_text(content)
PY
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted duplicate HTML+Markdown landing in non-optional section (issue #207)" >&2
  exit 1
fi

# Mutation: shrink a per-resource budget below the actual file size so the
# validator rejects an oversized document breaching its per-resource budget.
cp "$repo_root/site/llms-txt-context-manifest.json" "$fixture_root/site/llms-txt-context-manifest.json"
python3 - "$fixture_root/site/llms-txt-context-manifest.json" <<'PY'
from pathlib import Path
import sys
import json
path = Path(sys.argv[1])
data = json.loads(path.read_text())
for doc in data["non_optional_documents"]:
    if doc["label"] == "README":
        doc["budget_bytes"] = 1
path.write_text(json.dumps(data, indent=2) + "\n")
PY
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted an oversized document breaching its per-resource budget (issue #207)" >&2
  exit 1
fi
cp "$repo_root/site/llms-txt-context-manifest.json" "$fixture_root/site/llms-txt-context-manifest.json"

# Mutation: shrink the aggregate budget below the actual total so the
# validator rejects an expansion breaching the reviewed total budget.
cp "$repo_root/site/llms-txt-context-manifest.json" "$fixture_root/site/llms-txt-context-manifest.json"
python3 - "$fixture_root/site/llms-txt-context-manifest.json" <<'PY'
from pathlib import Path
import sys
import json
path = Path(sys.argv[1])
data = json.loads(path.read_text())
data["budget_kib"] = 1
path.write_text(json.dumps(data, indent=2) + "\n")
PY
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted aggregate expansion breaching the total budget (issue #207)" >&2
  exit 1
fi
cp "$repo_root/site/llms-txt-context-manifest.json" "$fixture_root/site/llms-txt-context-manifest.json"

# Mutation: declare a non-optional document's representation as HTML. The
# validator must reject a representation that changed from Markdown/plain text
# to HTML UI.
cp "$repo_root/site/llms-txt-context-manifest.json" "$fixture_root/site/llms-txt-context-manifest.json"
python3 - "$fixture_root/site/llms-txt-context-manifest.json" <<'PY'
from pathlib import Path
import sys
import json
path = Path(sys.argv[1])
data = json.loads(path.read_text())
for doc in data["non_optional_documents"]:
    if doc["label"] == "README":
        doc["representation"] = "html"
path.write_text(json.dumps(data, indent=2) + "\n")
PY
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted HTML representation for a non-optional document (issue #207)" >&2
  exit 1
fi
cp "$repo_root/site/llms-txt-context-manifest.json" "$fixture_root/site/llms-txt-context-manifest.json"

# Mutation: remove a non-optional link from llms.txt so the manifest and the
# file drift. The validator must reject a missing clean evidence representation.
cp "$repo_root/site/llms.txt" "$fixture_root/site/llms.txt"
python3 - "$fixture_root/site/llms.txt" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
content = path.read_text()
content = content.replace(
    "- [Roadmap](https://raw.githubusercontent.com/rotnov/pycc/main/docs/ROADMAP.md): "
    "Delivery stages and acceptance criteria toward v1.0.\n",
    "",
)
path.write_text(content)
PY
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted llms.txt with a non-optional link missing from the manifest (issue #207)" >&2
  exit 1
fi
cp "$repo_root/site/llms.txt" "$fixture_root/site/llms.txt"

# --- Issue #39: table-driven mutation tests for required files ---
# For each required file in the validator's required_file list, remove it
# and verify the validator rejects. Each file is restored immediately after
# its test so the fixture stays intact for the next iteration. This catches
# deletion of any required-file check from scripts/check-site.sh: if a file
# is removed from the validator's required_file loop, the corresponding
# self-test case stops failing.
indexnow_key='3361fe03d0f44ab7cdbb1a3ce1461821'
for required_file in \
  index.html \
  index.html.md \
  styles.css \
  site.js \
  og.png \
  favicon.svg \
  robots.txt \
  sitemap.xml \
  llms.txt \
  llms-txt-context-manifest.json \
  "${indexnow_key}.txt" \
  404.html \
  status/index.html \
  architecture/index.html \
  python-aot-compilers/index.html \
  python-aot-compilers/claims.json \
  ai-native/index.html
do
  target="$fixture_root/site/$required_file"
  rm -f "$target"
  if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
    echo "Validator accepted missing required file: $required_file (issue #39)" >&2
    exit 1
  fi
  mkdir -p "$(dirname "$target")"
  cp "$repo_root/site/$required_file" "$target"
done

# --- Issue #39: table-driven mutation tests for required metadata ---
# For each required metadata key in the validator's required_metadata list,
# remove the corresponding meta tag from index.html and verify the validator
# rejects. index.html is restored from the pristine repo copy before each
# iteration so only the targeted meta tag is missing. This catches deletion
# of any required-metadata check from scripts/check-site.sh: if a key is
# removed from the validator's required_metadata loop, the corresponding
# self-test case stops failing.
for meta_key in \
  description \
  google-site-verification \
  robots \
  og:type \
  og:site_name \
  og:locale \
  og:url \
  og:title \
  og:description \
  og:image \
  og:image:alt \
  twitter:card \
  twitter:title \
  twitter:description \
  twitter:image \
  twitter:image:alt
do
  cp "$repo_root/site/index.html" "$fixture_root/site/index.html"
  python3 - "$fixture_root/site/index.html" "$meta_key" <<'PY'
from pathlib import Path
import re
import sys
path = Path(sys.argv[1])
meta_key = sys.argv[2]
content = path.read_text()
# Remove the meta tag with this property/name. [^>] matches newlines so
# multi-line meta tags (e.g. name="description") are handled correctly.
pattern = re.compile(
    r'<meta\s+(?:property|name)="' + re.escape(meta_key) + r'"[^>]*/?>',
    re.IGNORECASE
)
content, count = pattern.subn("", content)
if count == 0:
    print(f"WARNING: could not find meta tag for {meta_key}", file=sys.stderr)
path.write_text(content)
PY
  if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
    echo "Validator accepted missing required metadata: $meta_key (issue #39)" >&2
    exit 1
  fi
done

cp "$repo_root/site/index.html" "$fixture_root/site/index.html"

# --- Issue #39: negative controls for other explicit contract checks ---
# These verify that deleting any of the validator's explicit contract checks
# (beyond the required-file and required-metadata loops) causes the self-test
# to fail.

# Canonical URL on the landing page: the validator must reject a landing page
# whose canonical link href does not match the canonical origin.
python3 - "$fixture_root/site/index.html" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
content = path.read_text()
canonical = '    <link rel="canonical" href="https://rotnov.github.io/pycc/">'
assert canonical in content
path.write_text(
    content.replace(
        canonical,
        '    <link rel="canonical" href="https://rotnov.github.io/pycc/wrong/">',
        1,
    )
)
PY
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a landing page with the wrong canonical URL (issue #39)" >&2
  exit 1
fi
cp "$repo_root/site/index.html" "$fixture_root/site/index.html"

# Sitemap origin in robots.txt: the validator must reject a robots.txt that
# does not declare the canonical sitemap URL.
python3 - "$fixture_root/site/robots.txt" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
content = path.read_text()
sitemap_line = "Sitemap: https://rotnov.github.io/pycc/sitemap.xml"
assert sitemap_line in content
path.write_text(content.replace(sitemap_line, "Sitemap: https://example.com/sitemap.xml", 1))
PY
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a robots.txt with the wrong sitemap origin (issue #39)" >&2
  exit 1
fi
cp "$repo_root/site/robots.txt" "$fixture_root/site/robots.txt"

# JSON-LD repository link: the validator must reject a SoftwareSourceCode
# JSON-LD object whose codeRepository does not link to the public repository.
python3 - "$fixture_root/site/index.html" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
content = path.read_text()
content = content.replace(
    '"codeRepository": "https://github.com/rotnov/pycc"',
    '"codeRepository": "https://github.com/wrong/pycc"',
    1,
)
path.write_text(content)
PY
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a JSON-LD codeRepository pointing to the wrong repository (issue #39)" >&2
  exit 1
fi
cp "$repo_root/site/index.html" "$fixture_root/site/index.html"

# Local-only URLs: the validator must reject any site file containing a
# localhost, 127.0.0.1, or file:// URL. The URL is inserted as an HTML
# comment so the metadata parser does not reject the page for structural
# reasons before the local-only-URL grep check runs.
python3 - "$fixture_root/site/index.html" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
content = path.read_text()
content = content.replace(
    "  </head>",
    '  <!-- http://127.0.0.1/local -->\n  </head>',
    1,
)
path.write_text(content)
PY
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a site containing a local-only URL (issue #39)" >&2
  exit 1
fi
cp "$repo_root/site/index.html" "$fixture_root/site/index.html"

# --- Issue #197: quick-start example binding mutation tests ---
# Execution-level negative controls (expression-form reintroduction, missing
# top-level invocation, fixture output drift) are covered by the CLI regression
# test (tests/quick_start.rs), not by test-check-site.sh — they require running
# the compiler, which the site validator does not do.

# Mutation: change README `cat hello.py` source so it differs from the fixture.
cp "$repo_root/README.md" "$fixture_root/README.md"
python3 - "$fixture_root/README.md" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
content = path.read_text()
content = content.replace(
    "def fib(n: int) -> int:\n    if n < 2:",
    "def fib(n: int) -> int :\n    if n < 2:",
    1,
)
path.write_text(content)
PY
if README_PATH="$fixture_root/README.md" SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted README source diverging from fixture (issue #197)" >&2
  exit 1
fi

# Mutation: dedent the README `return n` line (syntactically invalid Python)
# to verify the validator preserves indentation rather than collapsing it.
cp "$repo_root/README.md" "$fixture_root/README.md"
python3 - "$fixture_root/README.md" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
content = path.read_text()
content = content.replace(
    "def fib(n: int) -> int:\n    if n < 2:\n        return n",
    "def fib(n: int) -> int:\n    if n < 2:\n    return n",
    1,
)
path.write_text(content)
PY
if README_PATH="$fixture_root/README.md" SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted README source with indentation drift (issue #197)" >&2
  exit 1
fi

# Mutation: change site hero `<pre><code>` text so it differs from the fixture.
cp "$repo_root/site/index.html" "$fixture_root/site/index.html"
python3 - "$fixture_root/site/index.html" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
content = path.read_text()
content = content.replace(
    '<span class="code-keyword">def</span> <span class="code-function">fib</span>',
    '<span class="code-keyword">def</span> <span class="code-function">fib2</span>',
    1,
)
path.write_text(content)
PY
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted site hero source diverging from fixture (issue #197)" >&2
  exit 1
fi

# Mutation: change the fixture source itself so the README no longer matches.
cp "$repo_root/tests/fixtures/quick_start.py" "$fixture_root/quick_start.py"
python3 - "$fixture_root/quick_start.py" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
content = path.read_text()
content = content.replace(
    "def fib(n: int) -> int:",
    "def fib(n: int) -> int :",
    1,
)
path.write_text(content)
PY
if QUICK_START_FIXTURE_PATH="$fixture_root/quick_start.py" SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted fixture source diverging from README (issue #197)" >&2
  exit 1
fi

# Mutation: change README `$ ./hello` output block (drop the final `55`).
cp "$repo_root/README.md" "$fixture_root/README.md"
python3 - "$fixture_root/README.md" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
content = path.read_text()
content = content.replace(
    "$ ./hello\n0\n1\n1\n2\n3\n5\n8\n13\n21\n34\n55\n```",
    "$ ./hello\n0\n1\n1\n2\n3\n5\n8\n13\n21\n34\n```",
    1,
)
path.write_text(content)
PY
if README_PATH="$fixture_root/README.md" SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted README output diverging from canonical stdout (issue #197)" >&2
  exit 1
fi

# Mutation: change `data-copy` to differ from the displayed command.
cp "$repo_root/site/index.html" "$fixture_root/site/index.html"
python3 - "$fixture_root/site/index.html" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
content = path.read_text()
content = content.replace(
    'data-copy="pycc check hello.py"',
    'data-copy="pycc build hello.py"',
    1,
)
path.write_text(content)
PY
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted copy-button data-copy diverging from displayed command (issue #197)" >&2
  exit 1
fi

# Mutation: change copy-button command to a pip install command.
cp "$repo_root/site/index.html" "$fixture_root/site/index.html"
python3 - "$fixture_root/site/index.html" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
content = path.read_text()
content = content.replace(
    'data-copy="pycc check hello.py"',
    'data-copy="pip install pycc && pycc check hello.py"',
    1,
)
path.write_text(content)
PY
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a package-manager install command in copy-button (issue #197)" >&2
  exit 1
fi

# Mutation: add `planned` to the hero `.command-note`.
cp "$repo_root/site/index.html" "$fixture_root/site/index.html"
python3 - "$fixture_root/site/index.html" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
content = path.read_text()
content = content.replace(
    '<p class="command-note">Type-check typed Python · pre-alpha</p>',
    '<p class="command-note">Type-check typed Python · planned CLI · pre-alpha</p>',
    1,
)
path.write_text(content)
PY
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted 'planned' in hero command-note (issue #197)" >&2
  exit 1
fi

# Mutation: rename README `## Quick start` to `## Quick start (planned CLI)`.
# This is caught by the heading-existence guard (regex no longer matches),
# not by a 'planned'-in-heading guard.
cp "$repo_root/README.md" "$fixture_root/README.md"
python3 - "$fixture_root/README.md" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
content = path.read_text()
content = content.replace(
    "## Quick start\n",
    "## Quick start (planned CLI)\n",
    1,
)
path.write_text(content)
PY
if README_PATH="$fixture_root/README.md" SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted 'planned' in README Quick start heading (issue #197)" >&2
  exit 1
fi

# Mutation: remove the WEBSITE.md binding phrase.
cp "$repo_root/docs/WEBSITE.md" "$fixture_root/WEBSITE.md"
python3 - "$fixture_root/WEBSITE.md" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
content = path.read_text()
content = content.replace(
    "tested, executable v0.1 example",
    "design-target v0.1 example",
    1,
)
path.write_text(content)
PY
if WEBSITE_MD_PATH="$fixture_root/WEBSITE.md" SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted WEBSITE.md without the binding phrase (issue #197)" >&2
  exit 1
fi

# Mutation: change the displayed <code> command text (not data-copy) so it
# differs from the canonical 'pycc check hello.py'.
cp "$repo_root/site/index.html" "$fixture_root/site/index.html"
python3 - "$fixture_root/site/index.html" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
content = path.read_text()
content = content.replace(
    '<code><span>$</span> pycc check hello.py</code>',
    '<code><span>$</span> pycc run hello.py</code>',
    1,
)
path.write_text(content)
PY
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a displayed command other than 'pycc check hello.py' (issue #197)" >&2
  exit 1
fi

# Mutation: add a pip install command to the README quick-start console block.
cp "$repo_root/README.md" "$fixture_root/README.md"
python3 - "$fixture_root/README.md" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
content = path.read_text()
content = content.replace(
    "$ pycc check hello.py\n",
    "$ pip install pycc\n$ pycc check hello.py\n",
    1,
)
path.write_text(content)
PY
if README_PATH="$fixture_root/README.md" SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a package-manager install command in README (issue #197)" >&2
  exit 1
fi

# Mutation: add a @<version> suffix to a README quick-start command.
cp "$repo_root/README.md" "$fixture_root/README.md"
python3 - "$fixture_root/README.md" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
content = path.read_text()
content = content.replace(
    "$ pycc check hello.py\n",
    "$ pycc@0.1.0 check hello.py\n",
    1,
)
path.write_text(content)
PY
if README_PATH="$fixture_root/README.md" SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a @<version> suffix in README command (issue #197)" >&2
  exit 1
fi

# Mutation: add a @<version> suffix to the copy-button command.
cp "$repo_root/site/index.html" "$fixture_root/site/index.html"
python3 - "$fixture_root/site/index.html" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
content = path.read_text()
content = content.replace(
    'data-copy="pycc check hello.py"',
    'data-copy="pycc@0.1.0 check hello.py"',
    1,
)
path.write_text(content)
PY
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a @<version> suffix in copy-button command (issue #197)" >&2
  exit 1
fi

# Mutation: remove the tests/fixtures/quick_start.py reference from WEBSITE.md
# (keeping the binding phrase intact) so the fixture-path guard is exercised.
cp "$repo_root/docs/WEBSITE.md" "$fixture_root/WEBSITE.md"
python3 - "$fixture_root/WEBSITE.md" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
content = path.read_text()
content = content.replace(
    "tests/fixtures/quick_start.py",
    "tests/fixtures/example_fixture.py",
)
path.write_text(content)
PY
if WEBSITE_MD_PATH="$fixture_root/WEBSITE.md" SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted WEBSITE.md without tests/fixtures/quick_start.py reference (issue #197)" >&2
  exit 1
fi

# --- Issue #200: social preview image negative tests ---
# Each mutation replaces og.png in the fixture with a deliberately invalid
# image and verifies the validator rejects it. The fixture is restored from
# the pristine repo copy after each test.

# Mutation: oversize image (>= 1 MB GitHub limit).
python3 - "$fixture_root/site/og.png" <<'PY'
import struct
import sys
from pathlib import Path

path = Path(sys.argv[1])
# Build a valid 1280x640 PNG whose uncompressed pixel data pushes the file
# over 1 MB. We write a minimal PNG with a large IDAT of random-ish bytes.
width, height = 1280, 640
sig = b"\x89PNG\r\n\x1a\n"

def chunk(tag, data):
    return (
        struct.pack(">I", len(data))
        + tag
        + data
        + struct.pack(">I", 0)
    )

ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
# Raw row data: each row starts with filter byte 0, then RGB pixels.
row = b"\x00" + b"\xaa\xbb\xcc" * width
raw = row * height
# Use stored (no) compression to inflate size past 1 MB.
idat = b"x\x01\x01\x00" + struct.pack(">H", len(raw) & 0xFFFF) + struct.pack(">H", ~len(raw) & 0xFFFF) + raw + b"\x00\x00\x00\x00"
png = sig + chunk(b"IHDR", ihdr) + chunk(b"IDAT", idat) + chunk(b"IEND", b"")
path.write_bytes(png)
if len(png) < 1_048_576:
    # Fallback: pad with an extra ancillary chunk to exceed 1 MB.
    pad = b"\x00" * (1_048_576 - len(png) + 1024)
    png = sig + chunk(b"IHDR", ihdr) + chunk(b"zTXt", pad) + chunk(b"IDAT", idat) + chunk(b"IEND", b"")
    path.write_bytes(png)
PY
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted an og.png at or above 1 MB (issue #200)" >&2
  exit 1
fi
cp "$repo_root/site/og.png" "$fixture_root/site/og.png"

# Mutation: dimensions below 640x320 minimum.
python3 - "$fixture_root/site/og.png" <<'PY'
import struct
import sys
import zlib
from pathlib import Path

path = Path(sys.argv[1])
width, height = 320, 160
sig = b"\x89PNG\r\n\x1a\n"

def chunk(tag, data):
    import zlib as _z
    return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", _z.crc32(tag + data) & 0xFFFFFFFF)

ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
row = b"\x00" + b"\xaa\xbb\xcc" * width
raw = row * height
idat_data = zlib.compress(raw)
png = sig + chunk(b"IHDR", ihdr) + chunk(b"IDAT", idat_data) + chunk(b"IEND", b"")
path.write_bytes(png)
PY
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted og.png with dimensions below 640x320 (issue #200)" >&2
  exit 1
fi
cp "$repo_root/site/og.png" "$fixture_root/site/og.png"

# Mutation: wrong dimensions (valid PNG but not 1280x640).
python3 - "$fixture_root/site/og.png" <<'PY'
import struct
import sys
import zlib
from pathlib import Path

path = Path(sys.argv[1])
width, height = 640, 320
sig = b"\x89PNG\r\n\x1a\n"

def chunk(tag, data):
    return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
row = b"\x00" + b"\xaa\xbb\xcc" * width
raw = row * height
idat_data = zlib.compress(raw)
png = sig + chunk(b"IHDR", ihdr) + chunk(b"IDAT", idat_data) + chunk(b"IEND", b"")
path.write_bytes(png)
PY
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted og.png with wrong dimensions (not 1280x640) (issue #200)" >&2
  exit 1
fi
cp "$repo_root/site/og.png" "$fixture_root/site/og.png"

# Mutation: non-PNG format (JPEG magic bytes).
python3 - "$fixture_root/site/og.png" <<'PY'
import sys
from pathlib import Path

path = Path(sys.argv[1])
# Write a minimal JPEG-like file (JPEG SOI + EOI markers).
path.write_bytes(b"\xff\xd8\xff\xe0" + b"\x00" * 100 + b"\xff\xd9")
PY
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a non-PNG og.png (issue #200)" >&2
  exit 1
fi
cp "$repo_root/site/og.png" "$fixture_root/site/og.png"

# Mutation: wrong og:image target (points to a different file).
cp "$repo_root/site/index.html" "$fixture_root/site/index.html"
python3 - "$fixture_root/site/index.html" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
content = path.read_text()
content = content.replace(
    'property="og:image" content="https://rotnov.github.io/pycc/og.png"',
    'property="og:image" content="https://rotnov.github.io/pycc/wrong-card.png"',
)
path.write_text(content)
PY
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a wrong og:image target (issue #200)" >&2
  exit 1
fi
cp "$repo_root/site/index.html" "$fixture_root/site/index.html"

# Mutation: wrong twitter:image target (points to a different file).
cp "$repo_root/site/index.html" "$fixture_root/site/index.html"
python3 - "$fixture_root/site/index.html" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
content = path.read_text()
content = content.replace(
    'name="twitter:image" content="https://rotnov.github.io/pycc/og.png"',
    'name="twitter:image" content="https://rotnov.github.io/pycc/wrong-card.png"',
)
path.write_text(content)
PY
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a wrong twitter:image target (issue #200)" >&2
  exit 1
fi
cp "$repo_root/site/index.html" "$fixture_root/site/index.html"

# Mutation: preview asset absent from the site directory (og.png deleted but
# og:image still references it). This is already covered by the required-file
# loop above, but we add an explicit confirmation that removing og.png while
# keeping the metadata reference is rejected.
rm -f "$fixture_root/site/og.png"
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a missing og.png referenced by metadata (issue #200)" >&2
  exit 1
fi
cp "$repo_root/site/og.png" "$fixture_root/site/og.png"

echo "Website validator self-tests passed."
