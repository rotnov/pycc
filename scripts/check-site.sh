#!/usr/bin/env sh
set -eu

canonical='https://rotnov.github.io/pycc/'
site_dir=${SITE_DIR:-site}

for required_file in \
  index.html \
  styles.css \
  site.js \
  og.png \
  robots.txt \
  sitemap.xml \
  llms.txt \
  404.html
do
  test -f "$site_dir/$required_file"
done

test -s "$site_dir/og.png"

assert_once() {
  expected=$1
  file=$2
  count=$(grep -Fc "$expected" "$file" || true)
  if [ "$count" -ne 1 ]; then
    echo "Expected exactly one occurrence in $file: $expected" >&2
    exit 1
  fi
}

index="$site_dir/index.html"
python3 - "$index" "$canonical" <<'PY'
from html.parser import HTMLParser
import json
from pathlib import Path
import sys


class MetadataParser(HTMLParser):
    def __init__(self):
        super().__init__()
        self.in_title = False
        self.in_json_ld = False
        self.in_body = False
        self.hidden_body_depth = 0
        self.titles = []
        self.current_title = []
        self.links = []
        self.metas = []
        self.json_ld = []
        self.current_json_ld = []
        self.visible_body_text = []

    def handle_starttag(self, tag, attrs):
        attributes = dict(attrs)
        if tag == "body":
            self.in_body = True
        elif self.in_body and tag in {"script", "style", "template", "noscript"}:
            self.hidden_body_depth += 1
        elif tag == "title":
            self.in_title = True
            self.current_title = []
        elif tag == "link":
            self.links.append(attributes)
        elif tag == "meta":
            self.metas.append(attributes)
        elif tag == "script" and attributes.get("type") == "application/ld+json":
            self.in_json_ld = True
            self.current_json_ld = []

    def handle_endtag(self, tag):
        if tag == "body":
            self.in_body = False
        elif self.in_body and tag in {"script", "style", "template", "noscript"}:
            self.hidden_body_depth -= 1
        elif tag == "title" and self.in_title:
            self.titles.append("".join(self.current_title).strip())
            self.in_title = False
        elif tag == "script" and self.in_json_ld:
            self.json_ld.append("".join(self.current_json_ld))
            self.in_json_ld = False

    def handle_data(self, data):
        if self.in_title:
            self.current_title.append(data)
        if self.in_json_ld:
            self.current_json_ld.append(data)
        if self.in_body and self.hidden_body_depth == 0:
            self.visible_body_text.append(data)


def require_one(items, description):
    if len(items) != 1:
        raise SystemExit(f"Expected exactly one {description}; found {len(items)}")
    return items[0]


index_path = Path(sys.argv[1])
canonical = sys.argv[2]
parser = MetadataParser()
parser.feed(index_path.read_text())

title = require_one(parser.titles, "page title")
if title != "pycc — AOT compiler for typed Python to native binaries":
    raise SystemExit(f"Unexpected page title: {title!r}")

metadata = {}
for meta in parser.metas:
    key = meta.get("name") or meta.get("property")
    if key:
        metadata.setdefault(key, []).append(meta)

required_metadata = (
    "description",
    "robots",
    "og:type",
    "og:site_name",
    "og:locale",
    "og:url",
    "og:title",
    "og:description",
    "og:image",
    "og:image:alt",
    "twitter:card",
    "twitter:title",
    "twitter:description",
    "twitter:image",
    "twitter:image:alt",
)
for key in required_metadata:
    meta = require_one(metadata.get(key, []), f"{key!r} metadata field")
    if not meta.get("content", "").strip():
        raise SystemExit(f"Metadata field {key!r} must have nonempty content")

expected_values = {
    "robots": "index, follow, max-image-preview:large",
    "og:type": "website",
    "og:url": canonical,
    "og:image": f"{canonical}og.png",
    "twitter:card": "summary_large_image",
    "twitter:image": f"{canonical}og.png",
}
for key, expected in expected_values.items():
    actual = metadata[key][0]["content"]
    if actual != expected:
        raise SystemExit(f"Metadata field {key!r}: expected {expected!r}, found {actual!r}")

canonical_link = require_one(
    [
        link
        for link in parser.links
        if "canonical" in link.get("rel", "").split()
    ],
    "canonical link",
)
if canonical_link.get("href") != canonical:
    raise SystemExit("Canonical link does not match the canonical origin")

sitemap_link = require_one(
    [
        link
        for link in parser.links
        if "sitemap" in link.get("rel", "").split()
    ],
    "sitemap link",
)
if sitemap_link.get("href") != "sitemap.xml":
    raise SystemExit("Sitemap link must reference sitemap.xml")

software_sources = []
for source in parser.json_ld:
    document = json.loads(source)
    candidates = document.get("@graph", []) if isinstance(document, dict) else []
    candidates = [document, *candidates]
    software_sources.extend(
        candidate
        for candidate in candidates
        if isinstance(candidate, dict) and candidate.get("@type") == "SoftwareSourceCode"
    )

software_source = require_one(software_sources, "SoftwareSourceCode JSON-LD object")
if software_source.get("codeRepository") != "https://github.com/rotnov/pycc":
    raise SystemExit("SoftwareSourceCode JSON-LD must link to the public repository")

visible_body_text = " ".join(" ".join(parser.visible_body_text).split())
required_disclosures = (
    "Built entirely by AI.",
    "Managed by a human.",
    "No project code is handwritten by a human.",
)
for disclosure in required_disclosures:
    if disclosure not in visible_body_text:
        raise SystemExit(f"Missing visible AI authorship disclosure: {disclosure}")
PY

assert_once "Sitemap: ${canonical}sitemap.xml" "$site_dir/robots.txt"
assert_once "<loc>${canonical}</loc>" "$site_dir/sitemap.xml"

if grep -R -nE '(localhost|127\.0\.0\.1|file://)' "$site_dir"; then
  echo "Website contains a local-only URL" >&2
  exit 1
fi

echo "Website checks passed."
