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
    "base URL override": original.replace(
        "  <head>", '  <head>\n    <base href="https://example.com/">', 1
    ),
}

environment = dict(os.environ)
environment["SITE_DIR"] = str(site_dir)
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
  ai-native/index.html
do
  if [ ! -f "$fixture_root/site/$content_page" ]; then
    echo "Site fixture is missing required evidence page: $content_page" >&2
    exit 1
  fi
done

mv "$fixture_root/site/status/index.html" "$fixture_root/site/status/index.html.missing"
if SITE_DIR="$fixture_root/site" "$repo_root/scripts/check-site.sh" >/dev/null 2>&1; then
  echo "Validator accepted a site with a missing evidence page" >&2
  exit 1
fi
mv "$fixture_root/site/status/index.html.missing" "$fixture_root/site/status/index.html"

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
python3 - "$fixture_root/site/sitemap.xml" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
content = path.read_text()
required = """  <url>
    <loc>https://rotnov.github.io/pycc/status/</loc>"""
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
