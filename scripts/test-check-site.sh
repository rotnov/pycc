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
required = (
    'content="pycc is a fully AI-created, human-managed pre-alpha project '
    'building an ahead-of-time compiler for typed Python 3.14 with Rust and LLVM."'
)
assert required in content
path.write_text(content.replace(required, 'content=""', 1))
PY

if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted an empty required metadata value" >&2
  exit 1
fi

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
        "<code>pycc check</code> now runs the broadened v0.1 frontend",
        "<code>pycc check</code> is not implemented",
        "landing page without the current frontend status",
    ),
    (
        site_dir / "status" / "index.html",
        "<strong>Strict checking and inference</strong>",
        "<strong>Type checker placeholder</strong>",
        "status page without the implemented type-system boundary",
    ),
    (
        site_dir / "status" / "index.html",
        "Parser failures use <code>L0001</code>; byte-exact CLI",
        "Parser failures use <code>L0001</code>; output checks only",
        "status page that overstates diagnostic snapshot coverage",
    ),
    (
        site_dir / "status" / "index.html",
        "greater-than-2% regression gate run independently",
        "The frontend performance gate follows the compiler work",
        "status page with stale performance-gate sequencing",
    ),
    (
        site_dir / "status" / "index.html",
        "During the staged D-051",
        "Without the staged D-051",
        "status page without the digest-selected performance transition",
    ),
    (
        site_dir / "status" / "index.html",
        "The D-048 digest requires the",
        "The D-048 digest optionally uses the",
        "status page without D-048 exact-predecessor provenance",
    ),
    (
        site_dir / "status" / "index.html",
        "The D-051 digest\n              seals the exact-predecessor measurement before current source\n              executes and compares both measurements from the same runner.",
        "The D-051 digest\n              reuses a measurement from another runner.",
        "status page without D-051 same-run predecessor provenance",
    ),
    (
        site_dir / "status" / "index.html",
        "Both modes fail closed when exact predecessor evidence is\n              unavailable.",
        "Both modes continue when exact predecessor evidence is\n              unavailable.",
        "status page without fail-closed predecessor handling",
    ),
    (
        site_dir / "architecture" / "index.html",
        "<strong>Resolve and type-check</strong>",
        "<strong>Pass through the type stage</strong>",
        "architecture page without the implemented checker stage",
    ),
    (
        site_dir / "python-aot-compilers" / "index.html",
        (
            "frontend checker implemented for the v0.1 subset, while "
            "native code generation remains slice-only"
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
        site_dir / "index.html.md",
        "`pycc check` now parses and type-checks the v0.1",
        "`pycc check` is not implemented for the v0.1",
        "Markdown landing page without the current frontend status",
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

echo "Website validator self-tests passed."
