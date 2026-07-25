#!/usr/bin/env sh
set -eu

canonical='https://rotnov.github.io/pycc/'
site_dir=${SITE_DIR:-site}
indexnow_key='3361fe03d0f44ab7cdbb1a3ce1461821'
repo_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)

for required_file in \
  index.html \
  index.html.md \
  styles.css \
  site.js \
  og.png \
  robots.txt \
  sitemap.xml \
  llms.txt \
  "${indexnow_key}.txt" \
  404.html \
  status/index.html \
  architecture/index.html \
  python-aot-compilers/index.html \
  ai-native/index.html
do
  test -f "$site_dir/$required_file"
done

test -s "$site_dir/og.png"

python3 - "$site_dir/styles.css" <<'PY'
from pathlib import Path
import sys


def block_after(source, selector, *, last=False):
    start = source.rfind(selector) if last else source.find(selector)
    if start == -1:
        raise SystemExit(f"Missing responsive CSS selector: {selector.strip()}")
    opening = source.find("{", start)
    if opening == -1:
        raise SystemExit(f"Missing CSS block for selector: {selector.strip()}")
    depth = 1
    cursor = opening + 1
    while cursor < len(source) and depth:
        if source[cursor] == "{":
            depth += 1
        elif source[cursor] == "}":
            depth -= 1
        cursor += 1
    if depth:
        raise SystemExit(f"Unclosed responsive CSS selector: {selector.strip()}")
    return source[opening + 1:cursor - 1]


def declarations(block):
    return {
        line.strip().removesuffix(";")
        for line in block.splitlines()
        if ":" in line
    }


css = Path(sys.argv[1]).read_text()
mobile = block_after(css, "@media (max-width: 680px)")
footer = declarations(block_after(mobile, "\n  footer {", last=True))
footer_links = declarations(block_after(mobile, "\n  footer > div {"))

required_footer = {
    "grid-template-columns: 1fr",
}
required_footer_links = {
    "min-width: 0",
    "flex-wrap: wrap",
    "justify-content: flex-start",
    "justify-self: stretch",
}
if not required_footer <= footer:
    raise SystemExit("Narrow footer must stack into one grid column")
if not required_footer_links <= footer_links:
    raise SystemExit("Narrow footer links must wrap inside the available width")
PY

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
    hidden_body_tags = {"script", "style", "template", "noscript"}
    inert_asset_tags = {"template", "noscript"}
    foreign_root_tags = {"math", "svg"}
    void_tags = {
        "area",
        "base",
        "br",
        "col",
        "embed",
        "hr",
        "img",
        "input",
        "link",
        "meta",
        "param",
        "source",
        "track",
        "wbr",
    }

    def __init__(self):
        super().__init__()
        self.in_title = False
        self.in_json_ld = False
        self.in_body = False
        self.inert_element_stack = []
        self.foreign_root_stack = []
        self.hidden_body_depth = 0
        self.body_element_stack = []
        self.titles = []
        self.current_title = []
        self.links = []
        self.metas = []
        self.base_elements = []
        self.asset_links = []
        self.external_scripts = []
        self.json_ld = []
        self.current_json_ld = []
        self.visible_body_text = []

    def handle_starttag(self, tag, attrs):
        if tag in {"base", "link", "script"}:
            attribute_names = [name for name, _ in attrs]
            if len(attribute_names) != len(set(attribute_names)):
                raise SystemExit(f"Duplicate attributes are not allowed on <{tag}>")
        attributes = dict(attrs)
        if tag in self.inert_asset_tags:
            self.inert_element_stack.append(tag)
        if tag in self.foreign_root_tags:
            self.foreign_root_stack.append(tag)
        if self.foreign_root_stack and tag in {"link", "script"}:
            raise SystemExit(
                "Link and script elements are not allowed inside SVG or MathML"
            )
        if tag == "base":
            self.base_elements.append(attributes)
        elif (
            tag == "link"
            and not self.inert_element_stack
            and not self.foreign_root_stack
        ):
            self.asset_links.append(attributes)
        elif (
            tag == "script"
            and "src" in attributes
            and not self.inert_element_stack
            and not self.foreign_root_stack
        ):
            self.external_scripts.append(attributes)
        if tag == "body":
            self.in_body = True
            return
        if self.in_body:
            inline_style = attributes.get("style", "").replace(" ", "").lower()
            is_hidden = (
                self.hidden_body_depth > 0
                or tag in self.hidden_body_tags
                or "hidden" in attributes
                or attributes.get("aria-hidden", "").lower() == "true"
                or "display:none" in inline_style
                or "visibility:hidden" in inline_style
            )
            if tag not in self.void_tags:
                self.body_element_stack.append((tag, is_hidden))
                if is_hidden:
                    self.hidden_body_depth += 1
            return
        if tag == "title":
            self.in_title = True
            self.current_title = []
        elif tag == "link":
            self.links.append(attributes)
        elif tag == "meta":
            self.metas.append(attributes)
        elif tag == "script":
            if attributes.get("type") == "application/ld+json":
                self.in_json_ld = True
                self.current_json_ld = []

    def handle_startendtag(self, tag, attrs):
        if self.foreign_root_stack and tag in {"link", "script"}:
            raise SystemExit(
                "Link and script elements are not allowed inside SVG or MathML"
            )
        if self.foreign_root_stack or tag in self.foreign_root_tags:
            return
        if tag not in self.void_tags:
            raise SystemExit(f"<{tag}> must use an explicit closing tag")
        self.handle_starttag(tag, attrs)

    def handle_endtag(self, tag):
        if tag in self.foreign_root_tags:
            if not self.foreign_root_stack:
                raise SystemExit(f"Unexpected closing foreign root: {tag}")
            expected_tag = self.foreign_root_stack.pop()
            if tag != expected_tag:
                raise SystemExit(
                    f"Mismatched foreign roots: expected {expected_tag}, found {tag}"
                )
        if tag in self.inert_asset_tags:
            if not self.inert_element_stack:
                raise SystemExit(f"Unexpected closing {tag} tag")
            expected_tag = self.inert_element_stack.pop()
            if tag != expected_tag:
                raise SystemExit(
                    f"Mismatched inert tags: expected {expected_tag}, found {tag}"
                )
        if tag == "body":
            self.in_body = False
            self.body_element_stack = []
            self.hidden_body_depth = 0
            return
        if self.in_body:
            if not self.body_element_stack:
                raise SystemExit(f"Unexpected closing body tag: {tag}")
            started_tag, was_hidden = self.body_element_stack.pop()
            if started_tag != tag:
                raise SystemExit(
                    f"Mismatched body tags: expected {started_tag}, found {tag}"
                )
            if was_hidden:
                self.hidden_body_depth -= 1
            return
        if tag == "title" and self.in_title:
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

if parser.inert_element_stack:
    raise SystemExit(f"Unclosed inert element: {parser.inert_element_stack[-1]}")
if parser.foreign_root_stack:
    raise SystemExit(f"Unclosed foreign root: {parser.foreign_root_stack[-1]}")
if parser.base_elements:
    raise SystemExit("Base elements are not allowed because assets must resolve locally")

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
    "google-site-verification",
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
    "google-site-verification": "JYWBkUpaYuJgPDksjf5oGOn49o8X41PqUxS--u-eF24",
    "robots": (
        "index, follow, max-image-preview:large, "
        "max-snippet:-1, max-video-preview:-1"
    ),
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

stylesheet_link = require_one(
    [
        link
        for link in parser.asset_links
        if "stylesheet" in link.get("rel", "").lower().split()
    ],
    "stylesheet link",
)
if stylesheet_link.get("href") != "styles.css":
    raise SystemExit("Stylesheet link must reference styles.css relatively")
if stylesheet_link.get("rel", "").lower().split() != ["stylesheet"]:
    raise SystemExit("styles.css must use only the stylesheet relationship")
if set(stylesheet_link) != {"href", "rel"}:
    raise SystemExit("styles.css must use only href and rel attributes")

external_script = require_one(
    parser.external_scripts,
    "external script",
)
if external_script.get("src") != "site.js":
    raise SystemExit("External script must reference site.js relatively")
if set(external_script) != {"defer", "src"}:
    raise SystemExit("site.js must use only defer and src attributes")

software_sources = []
web_pages = []
for source in parser.json_ld:
    document = json.loads(source)
    candidates = document.get("@graph", []) if isinstance(document, dict) else []
    candidates = [document, *candidates]
    software_sources.extend(
        candidate
        for candidate in candidates
        if isinstance(candidate, dict) and candidate.get("@type") == "SoftwareSourceCode"
    )
    web_pages.extend(
        candidate
        for candidate in candidates
        if isinstance(candidate, dict) and candidate.get("@type") == "WebPage"
    )

software_source = require_one(software_sources, "SoftwareSourceCode JSON-LD object")
web_page = require_one(web_pages, "WebPage JSON-LD object")
project_id = f"{canonical}#project"
web_page_id = f"{canonical}#webpage"

if web_page.get("@id") != web_page_id:
    raise SystemExit("WebPage JSON-LD must use the canonical webpage ID")
if web_page.get("url") != canonical:
    raise SystemExit("WebPage JSON-LD must use the canonical URL")
if web_page.get("name") != title:
    raise SystemExit("WebPage JSON-LD name must match the page title")
if web_page.get("description") != metadata["description"][0]["content"]:
    raise SystemExit("WebPage JSON-LD description must match the meta description")
if web_page.get("mainEntity") != {"@id": project_id}:
    raise SystemExit("WebPage JSON-LD must identify the pycc project as its main entity")

if software_source.get("@id") != project_id:
    raise SystemExit("SoftwareSourceCode JSON-LD must use the canonical project ID")
if software_source.get("mainEntityOfPage") != {"@id": web_page_id}:
    raise SystemExit("SoftwareSourceCode JSON-LD must point back to the webpage")
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

python3 - \
  "$site_dir/status/index.html" \
  "$site_dir/architecture/index.html" \
  "$site_dir/python-aot-compilers/index.html" \
  "$site_dir/ai-native/index.html" <<'PY'
from html.parser import HTMLParser
import json
from pathlib import Path
import sys


ROOT = "https://rotnov.github.io/pycc/"
ROBOTS = (
    "index, follow, max-image-preview:large, "
    "max-snippet:-1, max-video-preview:-1"
)
PAGE_SPECS = {
    "status": {
        "canonical": f"{ROOT}status/",
        "title": "pycc status — what the Python AOT compiler can do today",
        "description": (
            "See what pycc, the AI-created AOT compiler for typed Python, "
            "implements today, what remains planned for v0.1, and the CI "
            "evidence behind each claim."
        ),
    },
    "architecture": {
        "canonical": f"{ROOT}architecture/",
        "title": "pycc architecture — typed Python to LLVM native binaries",
        "description": (
            "Explore pycc's implemented Rust and LLVM compiler pipeline, "
            "current crate boundaries, and the planned path from typed "
            "Python 3.14 to native binaries."
        ),
    },
    "python-aot-compilers": {
        "canonical": f"{ROOT}python-aot-compilers/",
        "title": "Python AOT compilers compared — where pycc fits",
        "description": (
            "Compare pycc, LPython, Codon, Nuitka, mypyc, and Cython from "
            "official docs: language contract, output artifact, runtime "
            "model, and current positioning."
        ),
        "required_hrefs": (
            "https://lpython.org/",
            "https://github.com/lcompilers/lpython",
            "https://docs.exaloop.io/language/overview/",
            "https://nuitka.net/user-documentation/use-cases.html",
            "https://mypyc.readthedocs.io/en/stable/introduction.html",
            (
                "https://docs.cython.org/en/latest/src/quickstart/"
                "overview.html"
            ),
            (
                "https://docs.cython.org/en/latest/src/tutorial/"
                "embedding.html"
            ),
        ),
        "required_visible_text": (
            "Tools six projects",
            "Alpha; focused on numerical and array-oriented typed Python",
            "Do not choose pycc for production today.",
            "Benchmarks none claimed",
        ),
    },
    "ai-native": {
        "canonical": f"{ROOT}ai-native/",
        "title": (
            "pycc AI-native experiment — software built entirely by AI"
        ),
        "description": (
            "See how AI agents create pycc's specifications, code, tests, "
            "reviews, documentation, and automation while a human only "
            "manages direction and constraints."
        ),
    },
}


class PageParser(HTMLParser):
    hidden_tags = {"script", "style", "template", "noscript"}
    void_tags = {
        "area", "base", "br", "col", "embed", "hr", "img", "input",
        "link", "meta", "param", "source", "track", "wbr",
    }

    def __init__(self):
        super().__init__()
        self.in_title = False
        self.in_json_ld = False
        self.in_body = False
        self.hidden_depth = 0
        self.body_stack = []
        self.titles = []
        self.current_title = []
        self.metas = []
        self.links = []
        self.json_ld = []
        self.current_json_ld = []
        self.visible_text = []
        self.anchors = []

    def handle_starttag(self, tag, attrs):
        attributes = dict(attrs)
        if tag == "body":
            self.in_body = True
            return
        if self.in_body:
            inline_style = attributes.get("style", "").replace(" ", "").lower()
            hidden = (
                self.hidden_depth > 0
                or tag in self.hidden_tags
                or "hidden" in attributes
                or attributes.get("aria-hidden", "").lower() == "true"
                or "display:none" in inline_style
                or "visibility:hidden" in inline_style
            )
            if tag == "a" and not hidden and attributes.get("href"):
                self.anchors.append(attributes["href"])
            if tag not in self.void_tags:
                self.body_stack.append((tag, hidden))
                if hidden:
                    self.hidden_depth += 1
            return
        if tag == "title":
            self.in_title = True
            self.current_title = []
        elif tag == "meta":
            self.metas.append(attributes)
        elif tag == "link":
            self.links.append(attributes)
        elif tag == "script" and attributes.get("type") == "application/ld+json":
            self.in_json_ld = True
            self.current_json_ld = []

    def handle_endtag(self, tag):
        if tag == "body":
            self.in_body = False
            return
        if self.in_body:
            if tag in self.void_tags:
                return
            if not self.body_stack:
                raise SystemExit(f"Unexpected closing body tag: {tag}")
            started_tag, hidden = self.body_stack.pop()
            if started_tag != tag:
                raise SystemExit(
                    f"Mismatched body tags: expected {started_tag}, found {tag}"
                )
            if hidden:
                self.hidden_depth -= 1
            return
        if tag == "title" and self.in_title:
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
        if self.in_body and self.hidden_depth == 0:
            self.visible_text.append(data)


def require_one(items, description):
    if len(items) != 1:
        raise SystemExit(
            f"Expected exactly one {description}; found {len(items)}"
        )
    return items[0]


seen_titles = set()
seen_descriptions = set()
for path_value in sys.argv[1:]:
    path = Path(path_value)
    slug = path.parent.name
    spec = PAGE_SPECS[slug]
    parser = PageParser()
    parser.feed(path.read_text())

    title = require_one(parser.titles, f"{slug} title")
    if title != spec["title"]:
        raise SystemExit(f"Unexpected {slug} title: {title!r}")
    if title in seen_titles:
        raise SystemExit(f"Evidence page title is not unique: {title!r}")
    seen_titles.add(title)

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
        meta = require_one(
            metadata.get(key, []),
            f"{slug} {key!r} metadata field",
        )
        if not meta.get("content", "").strip():
            raise SystemExit(
                f"{slug} metadata field {key!r} must have nonempty content"
            )

    description = metadata["description"][0]["content"]
    if description != spec["description"]:
        raise SystemExit(f"Unexpected {slug} description: {description!r}")
    if description in seen_descriptions:
        raise SystemExit(
            f"Evidence page description is not unique: {description!r}"
        )
    seen_descriptions.add(description)

    expected_metadata = {
        "robots": ROBOTS,
        "og:type": "website",
        "og:url": spec["canonical"],
        "og:image": f"{ROOT}og.png",
        "twitter:card": "summary_large_image",
        "twitter:image": f"{ROOT}og.png",
    }
    for key, expected in expected_metadata.items():
        actual = metadata[key][0]["content"]
        if actual != expected:
            raise SystemExit(
                f"{slug} metadata {key!r}: "
                f"expected {expected!r}, found {actual!r}"
            )

    canonical = require_one(
        [
            link for link in parser.links
            if "canonical" in link.get("rel", "").split()
        ],
        f"{slug} canonical link",
    )
    if canonical.get("href") != spec["canonical"]:
        raise SystemExit(f"{slug} canonical link does not match its URL")

    sitemap = require_one(
        [
            link for link in parser.links
            if "sitemap" in link.get("rel", "").split()
        ],
        f"{slug} sitemap link",
    )
    if sitemap.get("href") != "../sitemap.xml":
        raise SystemExit(f"{slug} sitemap link must be ../sitemap.xml")

    stylesheet = require_one(
        [
            link for link in parser.links
            if "stylesheet" in link.get("rel", "").split()
        ],
        f"{slug} stylesheet link",
    )
    if stylesheet.get("href") != "../styles.css":
        raise SystemExit(f"{slug} stylesheet link must be ../styles.css")

    web_pages = []
    breadcrumbs = []
    for source in parser.json_ld:
        document = json.loads(source)
        candidates = (
            document.get("@graph", [])
            if isinstance(document, dict)
            else []
        )
        candidates = [document, *candidates]
        web_pages.extend(
            item for item in candidates
            if isinstance(item, dict) and item.get("@type") == "WebPage"
        )
        breadcrumbs.extend(
            item for item in candidates
            if isinstance(item, dict)
            and item.get("@type") == "BreadcrumbList"
        )

    web_page = require_one(web_pages, f"{slug} WebPage JSON-LD object")
    breadcrumb = require_one(
        breadcrumbs,
        f"{slug} BreadcrumbList JSON-LD object",
    )
    if web_page.get("@id") != f"{spec['canonical']}#webpage":
        raise SystemExit(f"{slug} WebPage has the wrong @id")
    if web_page.get("url") != spec["canonical"]:
        raise SystemExit(f"{slug} WebPage has the wrong URL")
    if web_page.get("name") != title:
        raise SystemExit(f"{slug} WebPage name must match the title")
    if web_page.get("description") != description:
        raise SystemExit(
            f"{slug} WebPage description must match meta description"
        )
    if web_page.get("dateModified") != "2026-07-25":
        raise SystemExit(f"{slug} WebPage dateModified is stale")
    if web_page.get("isPartOf") != {"@id": f"{ROOT}#webpage"}:
        raise SystemExit(f"{slug} WebPage must reference the root WebPage")
    if web_page.get("about") != {"@id": f"{ROOT}#project"}:
        raise SystemExit(f"{slug} WebPage must reference the pycc project")
    if web_page.get("breadcrumb") != {"@id": breadcrumb.get("@id")}:
        raise SystemExit(f"{slug} WebPage must reference its breadcrumb")

    items = breadcrumb.get("itemListElement", [])
    if [item.get("position") for item in items] != [1, 2]:
        raise SystemExit(f"{slug} breadcrumb must contain positions 1 and 2")
    if [item.get("item") for item in items] != [ROOT, spec["canonical"]]:
        raise SystemExit(f"{slug} breadcrumb URLs do not match the page")

    visible_text = " ".join(" ".join(parser.visible_text).split())
    for disclosure in (
        "Built entirely by AI.",
        "Managed by a human.",
        "No project code is handwritten by a human.",
        "pre-alpha",
    ):
        if disclosure not in visible_text:
            raise SystemExit(
                f"{slug} is missing visible disclosure: {disclosure}"
            )

    for required_text in spec.get("required_visible_text", ()):
        if required_text not in visible_text:
            raise SystemExit(
                f"{slug} is missing required visible text: {required_text}"
            )

    for required_href in (
        "../",
        "../status/",
        "../architecture/",
        "../python-aot-compilers/",
        "../ai-native/",
        "https://github.com/rotnov/pycc",
    ):
        if required_href not in parser.anchors:
            raise SystemExit(
                f"{slug} is missing internal navigation link: {required_href}"
            )

    for required_href in spec.get("required_hrefs", ()):
        if required_href not in parser.anchors:
            raise SystemExit(
                f"{slug} is missing required source link: {required_href}"
            )
PY

assert_once "Sitemap: ${canonical}sitemap.xml" "$site_dir/robots.txt"

python3 - \
  "$site_dir/sitemap.xml" \
  "$site_dir/llms.txt" \
  "$site_dir/index.html.md" \
  "$canonical" <<'PY'
from datetime import date
from pathlib import Path
import sys
import xml.etree.ElementTree as ET


sitemap_path = Path(sys.argv[1])
llms_path = Path(sys.argv[2])
markdown_path = Path(sys.argv[3])
canonical = sys.argv[4]

namespace = {"s": "http://www.sitemaps.org/schemas/sitemap/0.9"}
root = ET.parse(sitemap_path).getroot()
urls = root.findall("s:url", namespace)
expected_locations = {
    canonical,
    f"{canonical}status/",
    f"{canonical}architecture/",
    f"{canonical}python-aot-compilers/",
    f"{canonical}ai-native/",
}
locations = []
for entry in urls:
    location_nodes = entry.findall("s:loc", namespace)
    if len(location_nodes) != 1:
        raise SystemExit(
            "Each sitemap URL entry must contain exactly one loc"
        )
    locations.append((location_nodes[0].text or "").strip())
if len(locations) != len(expected_locations):
    raise SystemExit(
        f"Expected {len(expected_locations)} sitemap URLs; "
        f"found {len(locations)}"
    )
if set(locations) != expected_locations:
    raise SystemExit(
        f"Sitemap URLs do not match the canonical page set: {locations!r}"
    )
if len(locations) != len(set(locations)):
    raise SystemExit("Sitemap contains a duplicate canonical URL")

for entry in urls:
    last_modified = entry.findtext("s:lastmod", namespaces=namespace)
    try:
        last_modified_date = date.fromisoformat(last_modified or "")
    except ValueError as error:
        raise SystemExit(
            "Sitemap lastmod must be an ISO 8601 calendar date"
        ) from error
    if last_modified_date > date.today():
        raise SystemExit("Sitemap lastmod cannot be in the future")

llms = llms_path.read_text()
if not llms.startswith("# pycc\n\n> "):
    raise SystemExit("llms.txt must start with the project H1 and blockquote summary")
for heading in ("## Project", "## Specifications", "## Optional"):
    if llms.count(heading) != 1:
        raise SystemExit(f"llms.txt must contain exactly one {heading!r} section")
for required_link in (
    f"[Canonical website]({canonical})",
    f"[Markdown website]({canonical}index.html.md)",
    f"[Current implementation status]({canonical}status/)",
    f"[Compiler architecture]({canonical}architecture/)",
    f"[Python AOT compiler comparison]({canonical}python-aot-compilers/)",
    f"[AI-native experiment]({canonical}ai-native/)",
    "[Source repository](https://github.com/rotnov/pycc)",
    "[Specification index](https://github.com/rotnov/pycc/blob/main/docs/SPEC.md)",
):
    if required_link not in llms:
        raise SystemExit(f"llms.txt is missing required link: {required_link}")

markdown = markdown_path.read_text()
if not markdown.startswith("# pycc — AOT compiler for typed Python to native binaries"):
    raise SystemExit("index.html.md must start with the canonical page title")
for disclosure in (
    "fully AI-created, human-managed",
    "A human only manages goals, constraints,",
    "No project code is handwritten by a human.",
):
    if disclosure not in llms or disclosure not in markdown:
        raise SystemExit(f"LLM-readable files are missing disclosure: {disclosure}")
for evidence_link in (
    f"[Current implementation status]({canonical}status/)",
    f"[Compiler architecture]({canonical}architecture/)",
    f"[Python AOT compiler comparison]({canonical}python-aot-compilers/)",
    f"[AI-native experiment]({canonical}ai-native/)",
):
    if evidence_link not in markdown:
        raise SystemExit(
            f"Markdown website is missing evidence link: {evidence_link}"
        )
PY

key_file="$site_dir/${indexnow_key}.txt"
if [ "$(tr -d '\r\n' < "$key_file")" != "$indexnow_key" ]; then
  echo "IndexNow key file must contain the key from its filename" >&2
  exit 1
fi

notify_script="$repo_root/scripts/notify-indexnow.sh"
test -x "$notify_script"
sh -n "$notify_script"
actual_notification=$(
  INDEXNOW_DRY_RUN=1 \
    INDEXNOW_SITEMAP="$site_dir/sitemap.xml" \
    "$notify_script"
)
python3 - \
  "$site_dir/sitemap.xml" \
  "$canonical" \
  "$indexnow_key" \
  "$actual_notification" <<'PY'
import json
from pathlib import Path
import sys
from urllib.parse import urlsplit
import xml.etree.ElementTree as ET

sitemap_path = Path(sys.argv[1])
canonical = sys.argv[2]
key = sys.argv[3]
payload = json.loads(sys.argv[4])
namespace = {"s": "http://www.sitemaps.org/schemas/sitemap/0.9"}
root = ET.parse(sitemap_path).getroot()
expected_urls = [
    (node.text or "").strip()
    for node in root.findall("s:url/s:loc", namespace)
]
expected = {
    "host": urlsplit(canonical).hostname,
    "key": key,
    "keyLocation": f"{canonical}{key}.txt",
    "urlList": expected_urls,
}
if payload != expected:
    raise SystemExit(
        "IndexNow notifier payload does not match the canonical sitemap set"
    )
PY

if grep -R -nE '(localhost|127\.0\.0\.1|file://)' "$site_dir"; then
  echo "Website contains a local-only URL" >&2
  exit 1
fi

echo "Website checks passed."
