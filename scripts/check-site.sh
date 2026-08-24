#!/usr/bin/env sh
set -eu

canonical='https://rotnov.github.io/pycc/'
site_dir=${SITE_DIR:-site}
indexnow_key='3361fe03d0f44ab7cdbb1a3ce1461821'
repo_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
readme_path=${README_PATH:-"$repo_root/README.md"}
quick_start_fixture=${QUICK_START_FIXTURE_PATH:-"$repo_root/tests/fixtures/quick_start.py"}
quick_start_expected=${QUICK_START_EXPECTED_PATH:-"$repo_root/tests/fixtures/quick_start.expected.txt"}
quick_start_diagnostic=${QUICK_START_DIAGNOSTIC_PATH:-"$repo_root/tests/diagnostics/quick_start_type_error.expected.txt"}
quick_start_diagnostic_source=${QUICK_START_DIAGNOSTIC_SOURCE_PATH:-"$repo_root/tests/diagnostics/quick_start_type_error.py"}
website_md_path=${WEBSITE_MD_PATH:-"$repo_root/docs/WEBSITE.md"}
evidence_root=${EVIDENCE_ROOT_PATH:-"$repo_root"}
evidence_manifest=${EVIDENCE_HERO_MANIFEST_PATH:-"$site_dir/evidence-heroes.json"}

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
  evidence-heroes.json \
  "${indexnow_key}.txt" \
  404.html \
  status/index.html \
  architecture/index.html \
  python-aot-compilers/index.html \
  python-aot-compilers/claims.json \
  ai-native/index.html
do
  test -f "$site_dir/$required_file"
done

test -s "$site_dir/og.png"

# Issue #564: one offline, versioned contract owns every evidence-page hero.
# The checked-in manifest is public projection data, while this allowlist is
# the reviewed acceptance boundary: a coordinated edit cannot silently replace
# an accepted commit/run/platform tuple with decorative values.  No provider is
# contacted here; exact source bytes are resolved from the local Git object
# database, and the immutable GitHub URLs are checked structurally.
python3 - \
  "$evidence_manifest" \
  "$evidence_root" \
  "$repo_root" \
  "$site_dir" <<'PY'
import hashlib
import json
import re
import subprocess
import sys
from html.parser import HTMLParser
from pathlib import Path, PurePosixPath


manifest_path = Path(sys.argv[1])
evidence_root = Path(sys.argv[2]).resolve()
repo_root = Path(sys.argv[3]).resolve()
site_dir = Path(sys.argv[4]).resolve()

SCHEMA_VERSION = "1.0.0"
EVIDENCE_STATES = [
    "all-Tier-1",
    "partial",
    "experimental",
    "unavailable",
    "superseded",
]
ROOT_FIELDS = {"schema_version", "evidence_states", "heroes"}
HERO_FIELDS = {
    "page_id",
    "evidence_id",
    "kind",
    "route",
    "page_path",
    "fixture",
    "test",
    "command",
    "snapshot",
    "repository",
    "attestation",
    "environment",
    "state",
    "limitations",
    "stable_links",
    "projections",
}
PAGE_ALLOWLIST = {
    "landing": {
        "evidence_id": "landing-quick-start-v1",
        "kind": "native-build-output",
        "route": "/",
        "page_path": "site/index.html",
        "owner": None,
        "projections": {
            "html": "site/index.html",
            "markdown": "site/index.html.md",
            "llm": "site/llms.txt",
            "structured_data": "site/index.html",
            "social": "site/index.html",
        },
    },
    "language": {
        "evidence_id": "language-support-v1",
        "kind": "language-conformance",
        "route": "/language-support/",
        "page_path": None,
        "owner": "https://github.com/rotnov/pycc/issues/565",
        "projections": {
            "markdown": "site/index.html.md",
            "llm": "site/llms.txt",
        },
    },
    "diagnostics": {
        "evidence_id": "diagnostics-v1",
        "kind": "compiler-diagnostic",
        "route": "/diagnostics/",
        "page_path": None,
        "owner": "https://github.com/rotnov/pycc/issues/565",
        "projections": {
            "markdown": "site/index.html.md",
            "llm": "site/llms.txt",
        },
    },
    "performance": {
        "evidence_id": "performance-v1",
        "kind": "benchmark",
        "route": "/performance/",
        "page_path": None,
        "owner": "https://github.com/rotnov/pycc/issues/567",
        "projections": {
            "markdown": "site/index.html.md",
            "llm": "site/llms.txt",
        },
    },
    "architecture": {
        "evidence_id": "architecture-trace-v1",
        "kind": "compiler-pipeline-trace",
        "route": "/architecture/",
        "page_path": "site/architecture/index.html",
        "owner": "https://github.com/rotnov/pycc/issues/566",
        "projections": {
            "html": "site/architecture/index.html",
            "markdown": "site/index.html.md",
            "llm": "site/llms.txt",
            "structured_data": "site/architecture/index.html",
            "social": "site/architecture/index.html",
        },
    },
    "status": {
        "evidence_id": "status-snapshot-v1",
        "kind": "required-checks-snapshot",
        "route": "/status/",
        "page_path": "site/status/index.html",
        "owner": "https://github.com/rotnov/pycc/issues/566",
        "projections": {
            "html": "site/status/index.html",
            "markdown": "site/index.html.md",
            "llm": "site/llms.txt",
            "structured_data": "site/status/index.html",
            "social": "site/status/index.html",
        },
    },
    "comparison": {
        "evidence_id": "comparison-sources-v1",
        "kind": "source-backed-comparison",
        "route": "/python-aot-compilers/",
        "page_path": "site/python-aot-compilers/index.html",
        "owner": "https://github.com/rotnov/pycc/issues/563",
        "projections": {
            "html": "site/python-aot-compilers/index.html",
            "markdown": "site/index.html.md",
            "llm": "site/llms.txt",
            "structured_data": "site/python-aot-compilers/index.html",
            "social": "site/python-aot-compilers/index.html",
        },
    },
    "provenance": {
        "evidence_id": "ai-provenance-v1",
        "kind": "authorship-attestation",
        "route": "/ai-native/",
        "page_path": "site/ai-native/index.html",
        "owner": "https://github.com/rotnov/pycc/issues/217",
        "projections": {
            "html": "site/ai-native/index.html",
            "markdown": "site/index.html.md",
            "llm": "site/llms.txt",
            "structured_data": "site/ai-native/index.html",
            "social": "site/ai-native/index.html",
        },
    },
}
LANDING_ALLOWLIST = {
    "commit": "8ccc05b51477c23711d2cefce3eb9128aab1162a",
    "run_id": 32419646184,
    "platforms": [
        {
            "runner": "macos-14",
            "architecture": "aarch64-apple-darwin",
            "job_url": "https://github.com/rotnov/pycc/actions/runs/32419646184/job/96588686638",
        },
        {
            "runner": "macos-15-intel",
            "architecture": "x86_64-apple-darwin",
            "job_url": "https://github.com/rotnov/pycc/actions/runs/32419646184/job/96588686654",
        },
        {
            "runner": "ubuntu-latest",
            "architecture": "x86_64-unknown-linux-gnu",
            "job_url": "https://github.com/rotnov/pycc/actions/runs/32419646184/job/96588686701",
        },
        {
            "runner": "ubuntu-24.04-arm",
            "architecture": "aarch64-unknown-linux-gnu",
            "job_url": "https://github.com/rotnov/pycc/actions/runs/32419646184/job/96588686726",
        },
        {
            "runner": "windows-latest",
            "architecture": "x86_64-pc-windows-msvc",
            "job_url": "https://github.com/rotnov/pycc/actions/runs/32419646184/job/96588687208",
        },
    ],
}


def fail(message):
    raise SystemExit(f"evidence-heroes.json: {message}")


def require_exact_fields(value, expected, context):
    if not isinstance(value, dict):
        fail(f"{context} must be an object")
    actual = set(value)
    if actual != set(expected):
        missing = sorted(set(expected) - actual)
        extra = sorted(actual - set(expected))
        fail(f"{context} fields drifted; missing={missing}, extra={extra}")


def canonical_bytes(data):
    # The repository has no .gitattributes line-ending pin.  Match the existing
    # quick-start contract by hashing canonical LF bytes on Windows too.
    return data.replace(b"\r\n", b"\n")


def safe_file(relative_path, context):
    if not isinstance(relative_path, str) or not relative_path:
        fail(f"{context} path must be a non-empty string")
    pure = PurePosixPath(relative_path)
    if pure.is_absolute() or ".." in pure.parts or "\\" in relative_path:
        fail(f"{context} path must be repository-relative: {relative_path!r}")
    candidate = evidence_root.joinpath(*pure.parts)
    if candidate.is_symlink() or not candidate.is_file():
        fail(f"{context} file is missing or not a regular file: {relative_path}")
    try:
        candidate.resolve().relative_to(evidence_root)
    except ValueError:
        fail(f"{context} path escapes the evidence root: {relative_path}")
    return candidate


def sha256(data):
    return hashlib.sha256(canonical_bytes(data)).hexdigest()


def git_blob(commit, path, context):
    result = subprocess.run(
        ["git", "-C", str(repo_root), "show", f"{commit}:{path}"],
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        check=False,
    )
    if result.returncode != 0:
        fail(f"{context} is absent from repository commit {commit}: {path}")
    return canonical_bytes(result.stdout)


try:
    document = json.loads(manifest_path.read_text())
except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
    fail(f"cannot read valid UTF-8 JSON: {error}")

require_exact_fields(document, ROOT_FIELDS, "root")
if document["schema_version"] != SCHEMA_VERSION:
    fail(
        f"schema_version must be {SCHEMA_VERSION!r}, "
        f"found {document['schema_version']!r}"
    )
if document["evidence_states"] != EVIDENCE_STATES:
    fail("evidence_states must equal the version-1 ordered vocabulary")
heroes = document["heroes"]
if not isinstance(heroes, list):
    fail("heroes must be an array")
page_ids = [hero.get("page_id") for hero in heroes if isinstance(hero, dict)]
if page_ids != list(PAGE_ALLOWLIST):
    fail(
        "hero page inventory/order must be exactly "
        f"{list(PAGE_ALLOWLIST)!r}, found {page_ids!r}"
    )

for hero in heroes:
    page_id = hero.get("page_id") if isinstance(hero, dict) else None
    require_exact_fields(hero, HERO_FIELDS, f"hero {page_id!r}")
    expected = PAGE_ALLOWLIST[page_id]
    for field in ("evidence_id", "kind", "route", "page_path", "projections"):
        if hero[field] != expected[field]:
            fail(
                f"hero {page_id!r} {field} must be "
                f"{expected[field]!r}, found {hero[field]!r}"
            )
    if not isinstance(hero["limitations"], str) or not hero["limitations"].strip():
        fail(f"hero {page_id!r} limitations must be non-empty human-readable text")
    if hero["state"] not in EVIDENCE_STATES:
        fail(f"hero {page_id!r} has unsupported evidence state {hero['state']!r}")
    if hero["page_path"] is not None:
        page_path = PurePosixPath(hero["page_path"])
        if not page_path.parts or page_path.parts[0] != "site":
            fail(f"hero {page_id!r} page_path must live under site/")
        projected_page = site_dir.joinpath(*page_path.parts[1:])
        if not projected_page.is_file():
            fail(f"hero {page_id!r} projected page is missing: {hero['page_path']}")

    if page_id != "landing":
        if hero["state"] != "unavailable":
            fail(
                f"hero {page_id!r} has no accepted artifact and must remain "
                "explicitly unavailable"
            )
        for field in (
            "fixture",
            "test",
            "command",
            "snapshot",
            "repository",
            "attestation",
            "environment",
        ):
            if hero[field] is not None:
                fail(
                    f"unavailable hero {page_id!r} must keep {field}=null; "
                    "decorative partial evidence is forbidden"
                )
        if hero["stable_links"] != {"owner": expected["owner"]}:
            fail(
                f"unavailable hero {page_id!r} must link only to its canonical "
                f"owner {expected['owner']!r}"
            )
        overstatement = re.search(
            r"\b(all[- ]Tier[- ]1|verified|proven|passing)\b",
            hero["limitations"],
            re.IGNORECASE,
        )
        if overstatement:
            fail(
                f"unavailable hero {page_id!r} limitations overstate evidence "
                f"with {overstatement.group(0)!r}"
            )
        continue

    if hero["state"] != "all-Tier-1":
        fail("landing hero accepted artifact must remain all-Tier-1")
    require_exact_fields(hero["fixture"], {"path", "sha256"}, "landing fixture")
    require_exact_fields(hero["test"], {"path", "name", "sha256"}, "landing test")
    require_exact_fields(
        hero["command"], {"build", "run", "compiler_flags"}, "landing command"
    )
    require_exact_fields(
        hero["snapshot"], {"path", "stream", "text", "sha256"}, "landing snapshot"
    )
    require_exact_fields(hero["repository"], {"commit", "url"}, "landing repository")
    require_exact_fields(
        hero["attestation"], {"workflow", "run_id", "run_url"}, "landing attestation"
    )
    require_exact_fields(
        hero["environment"],
        {"python", "rust", "llvm", "profile", "platforms"},
        "landing environment",
    )
    require_exact_fields(
        hero["stable_links"],
        {"fixture", "test", "snapshot", "commit", "run"},
        "landing stable_links",
    )

    expected_paths = {
        "fixture": "tests/fixtures/quick_start.py",
        "test": "tests/quick_start.rs",
        "snapshot": "tests/fixtures/quick_start.expected.txt",
    }
    for field, expected_path in expected_paths.items():
        if hero[field]["path"] != expected_path:
            fail(
                f"landing {field}.path must be {expected_path!r}; cross-fixture "
                f"mixing is forbidden, found {hero[field]['path']!r}"
            )
        local_data = canonical_bytes(safe_file(expected_path, f"landing {field}").read_bytes())
        actual_sha = sha256(local_data)
        if hero[field]["sha256"] != actual_sha:
            fail(
                f"landing {field}.sha256 does not match canonical local bytes: "
                f"expected {actual_sha}, found {hero[field]['sha256']!r}"
            )

    if hero["test"]["name"] != "quick_start_fixture_builds_and_prints_the_documented_sequence":
        fail("landing test.name is not the allowlisted integration test")
    test_text = safe_file(hero["test"]["path"], "landing test").read_text()
    if not re.search(rf"\bfn\s+{re.escape(hero['test']['name'])}\s*\(", test_text):
        fail("landing test.name is absent from the declared test file")
    if hero["command"] != {
        "build": "pycc build hello.py -o hello",
        "run": "./hello",
        "compiler_flags": [],
    }:
        fail("landing command must equal the tested public quick-start command tuple")
    if hero["snapshot"]["stream"] != "stdout":
        fail("landing snapshot.stream must be stdout")
    local_snapshot = canonical_bytes(
        safe_file(hero["snapshot"]["path"], "landing snapshot").read_bytes()
    ).decode("utf-8")
    if hero["snapshot"]["text"] != local_snapshot:
        fail("landing snapshot.text must byte-match the canonical stdout fixture")

    commit = hero["repository"]["commit"]
    if commit != LANDING_ALLOWLIST["commit"] or not re.fullmatch(r"[0-9a-f]{40}", commit):
        fail("landing repository.commit is not the reviewed evidence commit")
    commit_url = f"https://github.com/rotnov/pycc/commit/{commit}"
    if hero["repository"]["url"] != commit_url:
        fail("landing repository.url must point to the exact evidence commit")
    for field, path in expected_paths.items():
        at_commit = git_blob(commit, path, f"landing {field}")
        local_data = canonical_bytes(safe_file(path, f"landing {field}").read_bytes())
        if at_commit != local_data:
            fail(
                f"landing {field} local bytes drifted from exact evidence commit {commit}"
            )

    run_id = hero["attestation"]["run_id"]
    run_url = f"https://github.com/rotnov/pycc/actions/runs/{run_id}"
    if hero["attestation"]["workflow"] != "CI":
        fail("landing attestation.workflow must be CI")
    if run_id != LANDING_ALLOWLIST["run_id"]:
        fail("landing attestation.run_id is not the reviewed accepted run")
    if hero["attestation"]["run_url"] != run_url:
        fail("landing attestation.run_url must match the accepted numeric run id")

    environment = hero["environment"]
    if environment["python"] != "3.14.7":
        fail("landing environment.python must match the accepted CI oracle")
    if environment["rust"] != "1.97.1":
        fail("landing environment.rust must match the evidence commit toolchain")
    if environment["llvm"] != "22" or environment["profile"] != "debug":
        fail("landing LLVM/profile tuple must remain LLVM 22 debug")
    if environment["platforms"] != LANDING_ALLOWLIST["platforms"]:
        fail("landing Tier-1 platform/job tuple drifted from the reviewed run")
    for platform in environment["platforms"]:
        require_exact_fields(
            platform, {"runner", "architecture", "job_url"}, "landing platform"
        )
    toolchain = git_blob(commit, "rust-toolchain.toml", "landing toolchain").decode()
    if f'channel = "{environment["rust"]}"' not in toolchain:
        fail("landing Rust version is absent from the evidence commit toolchain")
    ci_workflow = git_blob(commit, ".github/workflows/ci.yml", "landing CI workflow").decode()
    for platform in environment["platforms"]:
        # The coverage leg is a native macos-14 arm64 runner and therefore has
        # no explicit target literal; the four matrix legs spell out both
        # runner and target.  The reviewed tuple above owns that one implicit
        # host-architecture mapping.
        architecture_missing = (
            platform["runner"] != "macos-14"
            and platform["architecture"] not in ci_workflow
        )
        if platform["runner"] not in ci_workflow or architecture_missing:
            fail(
                "landing platform tuple is absent from the exact evidence-commit CI workflow: "
                f"{platform['runner']}/{platform['architecture']}"
            )
    if "CPython 3.14.7" not in ci_workflow or "LLVM 22" not in ci_workflow:
        fail("landing Python/LLVM versions are absent from the evidence-commit CI workflow")

    stable_links = hero["stable_links"]
    expected_links = {
        "fixture": f"https://github.com/rotnov/pycc/blob/{commit}/{expected_paths['fixture']}",
        "test": f"https://github.com/rotnov/pycc/blob/{commit}/{expected_paths['test']}",
        "snapshot": f"https://github.com/rotnov/pycc/blob/{commit}/{expected_paths['snapshot']}",
        "commit": commit_url,
        "run": run_url,
    }
    if stable_links != expected_links:
        fail(
            "landing stable_links must point to the exact commit/run artifacts; "
            "moving branch URLs are forbidden"
        )


class EvidenceProjectionParser(HTMLParser):
    def __init__(self):
        super().__init__()
        self.hero_roots = []
        self.visible_markers = []
        self.named_meta = {}
        self.social_meta = {}
        self.json_ld = []
        self._in_json_ld = False
        self._json_ld_text = []
        self.visible_text = []

    def handle_starttag(self, tag, attrs):
        attributes = dict(attrs)
        evidence_tuple = (
            attributes.get("data-evidence-id"),
            attributes.get("data-evidence-kind"),
            attributes.get("data-evidence-state"),
        )
        if attributes.get("data-evidence-role") == "hero":
            self.hero_roots.append(evidence_tuple)
        if tag not in {"meta", "script"} and any(evidence_tuple):
            self.visible_markers.append(evidence_tuple)
        if tag == "meta":
            name = attributes.get("name")
            if name in {
                "pycc:evidence-id",
                "pycc:evidence-kind",
                "pycc:evidence-state",
            }:
                self.named_meta.setdefault(name, []).append(attributes.get("content"))
            social_name = attributes.get("property") or name
            if social_name in {"og:description", "twitter:description"}:
                self.social_meta.setdefault(social_name, []).append(evidence_tuple)
        if tag == "script" and attributes.get("type") == "application/ld+json":
            self._in_json_ld = True
            self._json_ld_text = []

    def handle_endtag(self, tag):
        if tag == "script" and self._in_json_ld:
            self._in_json_ld = False
            self.json_ld.append("".join(self._json_ld_text))

    def handle_data(self, data):
        if self._in_json_ld:
            self._json_ld_text.append(data)
        else:
            self.visible_text.append(data)


def projection_file(relative_path, context):
    pure = PurePosixPath(relative_path)
    if not pure.parts or pure.parts[0] != "site" or ".." in pure.parts:
        fail(f"{context} must be a safe site/ path")
    path = site_dir.joinpath(*pure.parts[1:])
    if not path.is_file():
        fail(f"{context} is missing: {relative_path}")
    return path


def normalized_text(value):
    return " ".join(value.replace("`", "").split())


hero_by_page = {hero["page_id"]: hero for hero in heroes}
for page_id, hero in hero_by_page.items():
    expected_tuple = (hero["evidence_id"], hero["kind"], hero["state"])
    html_projection = hero["projections"].get("html")
    if html_projection is not None:
        html_path = projection_file(html_projection, f"hero {page_id!r} HTML projection")
        html_text = html_path.read_text()
        parser = EvidenceProjectionParser()
        parser.feed(html_text)
        if parser.hero_roots != [expected_tuple]:
            fail(
                f"hero {page_id!r} HTML must carry exactly one matching "
                f"data-evidence-role='hero' tuple; found {parser.hero_roots!r}"
            )
        if len(parser.visible_markers) < 2 or any(
            marker != expected_tuple for marker in parser.visible_markers
        ):
            fail(
                f"hero {page_id!r} visible evidence markers must all equal "
                f"{expected_tuple!r}; found {parser.visible_markers!r}"
            )
        page_visible_text = normalized_text("".join(parser.visible_text))
        if hero["state"] not in page_visible_text:
            fail(f"hero {page_id!r} HTML does not visibly render state {hero['state']!r}")
        expected_meta = {
            "pycc:evidence-id": [hero["evidence_id"]],
            "pycc:evidence-kind": [hero["kind"]],
            "pycc:evidence-state": [hero["state"]],
        }
        if parser.named_meta != expected_meta:
            fail(
                f"hero {page_id!r} structured meta tuple drifted: "
                f"expected {expected_meta!r}, found {parser.named_meta!r}"
            )
        expected_social = {
            "og:description": [expected_tuple],
            "twitter:description": [expected_tuple],
        }
        if parser.social_meta != expected_social:
            fail(
                f"hero {page_id!r} social descriptions must carry the exact "
                f"evidence tuple; found {parser.social_meta!r}"
            )
        if len(parser.json_ld) != 1:
            fail(f"hero {page_id!r} must carry exactly one JSON-LD projection")
        try:
            structured = json.loads(parser.json_ld[0])
        except json.JSONDecodeError as error:
            fail(f"hero {page_id!r} JSON-LD is invalid: {error}")
        graph = structured.get("@graph") if isinstance(structured, dict) else None
        if not isinstance(graph, list):
            fail(f"hero {page_id!r} JSON-LD must carry an @graph array")
        webpages = [node for node in graph if node.get("@type") == "WebPage"]
        if len(webpages) != 1:
            fail(f"hero {page_id!r} JSON-LD must carry exactly one WebPage")
        properties = webpages[0].get("additionalProperty")
        expected_properties = [
            {
                "@type": "PropertyValue",
                "propertyID": "pycc:evidence-id",
                "value": hero["evidence_id"],
            },
            {
                "@type": "PropertyValue",
                "propertyID": "pycc:evidence-kind",
                "value": hero["kind"],
            },
            {
                "@type": "PropertyValue",
                "propertyID": "pycc:evidence-state",
                "value": hero["state"],
            },
        ]
        if properties != expected_properties:
            fail(
                f"hero {page_id!r} JSON-LD evidence properties drifted: "
                f"expected {expected_properties!r}, found {properties!r}"
            )

# Markdown and LLM clients get the complete inventory, including unavailable
# routes, rather than inferring proof from the existence of an explanatory
# HTML page.  Exact comment records make the state machine-readable without a
# second independently-maintained JSON object.
for surface_name in ("markdown", "llm"):
    projection_paths = {hero["projections"].get(surface_name) for hero in heroes}
    if None in projection_paths or len(projection_paths) != 1:
        fail(
            f"all heroes must share one {surface_name} inventory projection; "
            f"found {sorted(str(path) for path in projection_paths)!r}"
        )
    projection_path = projection_file(
        projection_paths.pop(), f"shared {surface_name} projection"
    )
    projection_text = projection_path.read_text()
    for hero in heroes:
        marker = (
            f"<!-- evidence-hero: {hero['page_id']} | {hero['evidence_id']} | "
            f"{hero['kind']} | {hero['state']} | {hero['route']} -->"
        )
        if projection_text.count(marker) != 1:
            fail(
                f"{surface_name} projection must contain exactly one marker "
                f"for hero {hero['page_id']!r}: {marker}"
            )

landing = hero_by_page["landing"]
landing_surface_paths = {
    "html": landing["projections"]["html"],
    "markdown": landing["projections"]["markdown"],
    "llm": landing["projections"]["llm"],
}
for surface_name, relative_path in landing_surface_paths.items():
    text = projection_file(relative_path, f"landing {surface_name}").read_text()
    if surface_name == "html":
        text_parser = EvidenceProjectionParser()
        text_parser.feed(text)
        normalized = normalized_text("".join(text_parser.visible_text))
    else:
        normalized = normalized_text(text)
    required_literals = [
        landing["evidence_id"],
        landing["kind"],
        landing["state"],
        landing["fixture"]["path"],
        landing["fixture"]["sha256"],
        landing["test"]["path"],
        landing["test"]["name"],
        landing["test"]["sha256"],
        landing["command"]["build"],
        landing["command"]["run"],
        landing["snapshot"]["path"],
        landing["snapshot"]["sha256"],
        landing["repository"]["commit"],
        str(landing["attestation"]["run_id"]),
        landing["attestation"]["run_url"],
        landing["environment"]["python"],
        landing["environment"]["rust"],
        landing["environment"]["llvm"],
        landing["environment"]["profile"],
        "no extra compiler flags",
    ]
    required_literals.extend(landing["stable_links"].values())
    for platform in landing["environment"]["platforms"]:
        required_literals.extend(platform.values())
    for literal in required_literals:
        if str(literal) not in text and normalized_text(str(literal)) not in normalized:
            fail(
                f"landing {surface_name} projection is missing manifest field "
                f"value {literal!r}"
            )
    if normalized_text(landing["limitations"]) not in normalized:
        fail(f"landing {surface_name} projection limitations drifted")
    snapshot_without_terminator = landing["snapshot"]["text"].removesuffix("\n")
    if snapshot_without_terminator not in text.replace("\r\n", "\n"):
        fail(f"landing {surface_name} projection exact stdout drifted")
PY

# Issue #200: validate the social preview image for format, dimensions, and
# file size so it meets GitHub's repository social-preview upload constraints
# (PNG/JPG/GIF under 1 MB, at least 640x320, 1280x640 recommended) and remains
# a single canonical asset shared by the website and the repository.
python3 - "$site_dir/og.png" <<'PY'
import struct
import sys
from pathlib import Path

og_path = Path(sys.argv[1])
data = og_path.read_bytes()
size = len(data)

# GitHub requires under 1 MB; enforce a safety margin (960 KiB ceiling).
GITHUB_LIMIT = 1_048_576
SAFETY_CEILING = 960 * 1024
if size >= GITHUB_LIMIT:
    raise SystemExit(
        f"og.png must be under 1 MB for GitHub upload; got {size} bytes"
    )
if size >= SAFETY_CEILING:
    raise SystemExit(
        f"og.png must be under 960 KiB safety margin; got {size} bytes"
    )

# Validate PNG signature and read IHDR for dimensions and format.
PNG_SIG = b"\x89PNG\r\n\x1a\n"
if data[:8] != PNG_SIG:
    raise SystemExit("og.png must be a PNG file")
if data[12:16] != b"IHDR":
    raise SystemExit("og.png IHDR chunk must appear first")
width, height = struct.unpack(">II", data[16:24])
if width < 640 or height < 320:
    raise SystemExit(
        f"og.png dimensions must be at least 640x320; got {width}x{height}"
    )
if width != 1280 or height != 640:
    raise SystemExit(
        f"og.png dimensions must be exactly 1280x640; got {width}x{height}"
    )
PY

python3 - "$site_dir/favicon.svg" <<'PY'
from pathlib import Path
import sys
import xml.etree.ElementTree as ET

favicon_path = Path(sys.argv[1])
if favicon_path.stat().st_size >= 1024:
    raise SystemExit("favicon.svg must be under 1KB")
root = ET.parse(favicon_path).getroot()
if root.tag != "{http://www.w3.org/2000/svg}svg":
    raise SystemExit("favicon.svg root element must be <svg>")
PY

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
        self.keyword_metas = []

    def handle_starttag(self, tag, attrs):
        if tag in {"base", "link", "meta", "script"}:
            attribute_names = [name for name, _ in attrs]
            if len(attribute_names) != len(set(attribute_names)):
                raise SystemExit(f"Duplicate attributes are not allowed on <{tag}>")
        attributes = dict(attrs)
        if (
            tag == "meta"
            and attributes.get("name", "").strip().lower() == "keywords"
        ):
            self.keyword_metas.append(attributes)
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
if parser.keyword_metas:
    raise SystemExit("HTML meta keywords are unsupported and must not be published")

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

expected_description = (
    "pycc is a pre-alpha ahead-of-time compiler for typed Python 3.14 with "
    "an implemented native-binary path through Rust and LLVM; AI-created "
    "and human-managed."
)
expected_social_description = (
    "A pre-alpha AOT compiler for typed Python 3.14 with an implemented "
    "native-binary path through Rust and LLVM, created by AI and managed "
    "by a human."
)
if metadata["description"][0]["content"] != expected_description:
    raise SystemExit("Landing description must be product-first and pre-alpha")
for key in ("og:description", "twitter:description"):
    if metadata[key][0]["content"] != expected_social_description:
        raise SystemExit(f"{key} must preserve the product-first social description")

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

# `<link rel="sitemap">` is not a registered IANA link relation and is not a
# documented sitemap-discovery mechanism. The XML sitemap referenced from
# robots.txt is the standards-based discovery surface; the HTML link relation
# must not be re-introduced as a discovery channel.
if any(
    "sitemap" in link.get("rel", "").lower().split() for link in parser.links
):
    raise SystemExit(
        "Pages must not use the unregistered rel=sitemap link relation; "
        "the XML sitemap referenced from robots.txt is the discovery surface"
    )

favicon_link = require_one(
    [
        link
        for link in parser.links
        if "icon" in link.get("rel", "").lower().split()
    ],
    "favicon link",
)
if favicon_link.get("href") != "favicon.svg":
    raise SystemExit("Favicon link must reference favicon.svg relatively")
if favicon_link.get("type") != "image/svg+xml":
    raise SystemExit("Favicon link must use the image/svg+xml type")
if set(favicon_link) != {"href", "rel", "type"}:
    raise SystemExit("favicon.svg must use only href, rel, and type attributes")

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
if web_page.get("dateModified") != "2026-08-21":
    raise SystemExit("Landing WebPage dateModified is stale")
if web_page.get("mainEntity") != {"@id": project_id}:
    raise SystemExit("WebPage JSON-LD must identify the pycc project as its main entity")

if software_source.get("@id") != project_id:
    raise SystemExit("SoftwareSourceCode JSON-LD must use the canonical project ID")
if software_source.get("mainEntityOfPage") != {"@id": web_page_id}:
    raise SystemExit("SoftwareSourceCode JSON-LD must point back to the webpage")
if software_source.get("codeRepository") != "https://github.com/rotnov/pycc":
    raise SystemExit("SoftwareSourceCode JSON-LD must link to the public repository")
expected_source_description = (
    "pycc is a pre-alpha ahead-of-time compiler for typed Python 3.14 with "
    "an implemented path to standalone native binaries; AI-created and "
    "human-managed."
)
if software_source.get("description") != expected_source_description:
    raise SystemExit("SoftwareSourceCode description must preserve product-first truth")

# --- Issue #203: bind SoftwareSourceCode semantics to visible facts ---
if software_source.get("name") != "pycc":
    raise SystemExit("SoftwareSourceCode JSON-LD name must be 'pycc'")
if software_source.get("alternateName") != "pycc Python compiler":
    raise SystemExit(
        "SoftwareSourceCode JSON-LD alternateName must be 'pycc Python compiler'"
    )
if software_source.get("url") != canonical:
    raise SystemExit("SoftwareSourceCode JSON-LD url must be the canonical URL")
if software_source.get("license") != "https://opensource.org/license/mit":
    raise SystemExit("SoftwareSourceCode JSON-LD license must be the MIT URL")
if software_source.get("programmingLanguage") != "Rust":
    raise SystemExit("SoftwareSourceCode JSON-LD programmingLanguage must be 'Rust'")
# runtimePlatform must NOT be present — LLVM is the compiler backend,
# not a runtime platform.  schema.org defines runtimePlatform as the
# runtime/interpreter dependency; pycc's output is standalone native
# binaries with no runtime platform.  Setting it to "LLVM" would
# misleadingly imply generated programs run on an LLVM platform.
if "runtimePlatform" in software_source:
    raise SystemExit(
        "SoftwareSourceCode JSON-LD must not declare runtimePlatform — "
        "LLVM is the compiler backend, not a runtime platform (issue #203)"
    )
# Keywords must describe a typed-Python AOT compiler and must not
# describe pycc as an AI or ML compiler.
keywords = software_source.get("keywords")
if not isinstance(keywords, list) or not keywords:
    raise SystemExit("SoftwareSourceCode JSON-LD must have a non-empty keywords array")
kw_text = " ".join(keywords).lower()
if "python" not in kw_text or "compiler" not in kw_text:
    raise SystemExit("SoftwareSourceCode keywords must describe a Python compiler")
if "ai compiler" in kw_text or "machine learning" in kw_text:
    raise SystemExit(
        "SoftwareSourceCode keywords must not describe pycc as an AI or ML compiler"
    )
# Must not claim production readiness.
source_text = json.dumps(software_source).lower()
for false_claim in ("production-ready", "production ready", "stable release", "ga "):
    if false_claim in source_text:
        raise SystemExit(
            f"SoftwareSourceCode JSON-LD must not claim '{false_claim}' — "
            "pycc is pre-alpha"
        )

visible_body_text = " ".join(" ".join(parser.visible_body_text).split())
required_disclosures = (
    "Built entirely by AI.",
    "Managed by a human.",
    "No project code is handwritten by a human.",
    "pycc check now runs the v0.1 frontend",
    "v0.1 native backend with documented gaps",
    "v0.1 acceptance criteria met (conformance verified on all five Tier-1 targets)",
    "v0.2 acceptance criteria also met; v0.3's class model core has landed",
    "The full conformance matrix and the rest of v0.3 are next",
    (
        "Native and pure builds emit standalone executables; planned permitted "
        "CPython interop emits a self-contained bundle with its pinned runtime."
    ),
)
for disclosure in required_disclosures:
    if disclosure not in visible_body_text:
        raise SystemExit(f"Missing visible AI authorship disclosure: {disclosure}")
product_phrase = (
    "pycc is an open-source ahead-of-time compiler project for standard "
    "Python 3.14."
)
provenance_phrase = "AI agents create the entire project"
if product_phrase not in visible_body_text or provenance_phrase not in visible_body_text:
    raise SystemExit("Landing body must state both product and provenance roles")
if visible_body_text.index(product_phrase) > visible_body_text.index(provenance_phrase):
    raise SystemExit("Landing body must explain the product before provenance")
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
        "date_modified": "2026-08-24",
        "title": "pycc status — what the Python AOT compiler can do today",
        "description": (
            "See what pycc, the AI-created AOT compiler for typed Python, "
            "implements today, what's complete for v0.1 and what's planned "
            "next, and the CI evidence behind each claim."
        ),
        "required_visible_text": (
            "pycc check runs the parser → HIR → strict type checker",
            "Strict checking and inference",
            "Implemented v0.1 subset",
            "Validated frontend codes",
            "byte-exact CLI snapshots cover",
            "Custom exceptions and the full conformance matrix are next.",
            "v0.1 and v0.2's acceptance criteria are both met, and v0.3's class model core has landed",
            "v0.2 acceptance criteria met",
            "v0.3 in progress",
            "user-defined exception class remains planned.",
            "Still ahead for v0.3: raising and catching a",
            "the full multi-version conformance matrix, differential fuzzing, and corpus testing remain planned test-depth work carried over from v0.1 and v0.2.",
            "Unary operators are rejected earlier by HIR lowering with a spanned C0001 capability diagnostic, including under pycc check",
            "The 100% line/region coverage gate and the required",
            "currently a greater-than-7.0% regression floor, enforced by a paired predecessor/candidate measurement",
            "stay required through all of this work.",
            "The implementation gaps listed above are accepted boundaries",
        ),
    },
    "architecture": {
        "canonical": f"{ROOT}architecture/",
        "date_modified": "2026-08-21",
        "title": "pycc architecture — typed Python to LLVM native binaries",
        "description": (
            "Explore pycc's implemented Rust and LLVM compiler pipeline, "
            "current crate boundaries, and the planned path from typed "
            "Python 3.14 to native binaries."
        ),
        "required_visible_text": (
            "Frontend v0.1 checked",
            "Resolve and type-check",
            "pycc_types::check_and_resolve",
            "MIR and code generation cover the implemented v0.1 surface",
            "Strict checking and inference",
            (
                "Planned permitted CPython interop instead adds the pinned "
                "interpreter and dependency closure to an autonomous "
                "application bundle."
            ),
        ),
    },
    "python-aot-compilers": {
        "canonical": f"{ROOT}python-aot-compilers/",
        "date_modified": "2026-08-21",
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
            (
                "Standalone native executable without CPython for native "
                "and pure builds; planned permitted interop bundles a "
                "pinned CPython runtime"
            ),
            (
                "Native and pure pycc builds target a standalone runtime; "
                "planned permitted interop carries a pinned CPython runtime "
                "in the application bundle."
            ),
            (
                "v0.1 frontend and native backend implemented with "
                "documented gaps; not production-ready"
            ),
            "Do not choose pycc for production today.",
            "Benchmarks none claimed",
        ),
    },
    "ai-native": {
        "canonical": f"{ROOT}ai-native/",
        "date_modified": "2026-08-21",
        "title": (
            "pycc AI-native experiment — software built entirely by AI"
        ),
        "description": (
            "pycc is a pre-alpha AOT compiler for typed Python 3.14 and an "
            "AI-native development experiment: AI agents create the project "
            "while a human manages direction."
        ),
        "social_description": (
            "pycc is a pre-alpha AOT compiler for typed Python 3.14, created "
            "entirely by AI agents and managed by a human."
        ),
        "required_visible_text": (
            "AI-native” does not mean that pycc compiles AI models or an AI-specific language.",
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
        self.keyword_metas = []
        self.in_table = False
        self.in_tr = False
        self.in_th = False
        self.in_td = False
        self.current_cell_text = []
        self.current_row_header = None
        self.table_rows = {}

    def handle_starttag(self, tag, attrs):
        if tag == "meta":
            attribute_names = [name for name, _ in attrs]
            if len(attribute_names) != len(set(attribute_names)):
                raise SystemExit("Duplicate attributes are not allowed on <meta>")
        attributes = dict(attrs)
        if (
            tag == "meta"
            and attributes.get("name", "").strip().lower() == "keywords"
        ):
            self.keyword_metas.append(attributes)
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
            if tag == "table":
                self.in_table = True
            elif tag == "tr" and self.in_table:
                self.in_tr = True
                self.current_row_header = None
            elif tag == "th" and self.in_tr:
                scope = attributes.get("scope", "")
                if scope == "row":
                    self.in_th = True
                    self.current_cell_text = []
            elif tag == "td" and self.in_tr:
                self.in_td = True
                self.current_cell_text = []
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
            if tag == "table":
                self.in_table = False
            elif tag == "tr" and self.in_tr:
                self.in_tr = False
            elif tag == "th" and self.in_th:
                self.in_th = False
                self.current_row_header = "".join(
                    self.current_cell_text
                ).strip()
                self.current_cell_text = []
            elif tag == "td" and self.in_td:
                self.in_td = False
                cell_text = " ".join(
                    "".join(self.current_cell_text).split()
                )
                if self.current_row_header is not None:
                    self.table_rows.setdefault(
                        self.current_row_header, []
                    ).append(cell_text)
                self.current_cell_text = []
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
        if self.in_th or self.in_td:
            self.current_cell_text.append(data)


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

    if parser.keyword_metas:
        raise SystemExit(
            f"{slug} publishes unsupported HTML meta keywords"
        )

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

    if "social_description" in spec:
        for key in ("og:description", "twitter:description"):
            if metadata[key][0]["content"] != spec["social_description"]:
                raise SystemExit(
                    f"Unexpected {slug} {key}: "
                    f"{metadata[key][0]['content']!r}"
                )

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

    if any(
        "sitemap" in link.get("rel", "").lower().split() for link in parser.links
    ):
        raise SystemExit(
            f"{slug} must not use the unregistered rel=sitemap link "
            "relation; the XML sitemap referenced from robots.txt is the "
            "discovery surface"
        )

    favicon = require_one(
        [
            link for link in parser.links
            if "icon" in link.get("rel", "").lower().split()
        ],
        f"{slug} favicon link",
    )
    if favicon.get("href") != "../favicon.svg":
        raise SystemExit(f"{slug} favicon link must be ../favicon.svg")
    if favicon.get("type") != "image/svg+xml":
        raise SystemExit(f"{slug} favicon link must use the image/svg+xml type")
    if set(favicon) != {"href", "rel", "type"}:
        raise SystemExit(
            f"{slug} favicon link must use only href, rel, and type attributes"
        )

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
    expected_modified = spec.get("date_modified", "2026-07-25")
    if web_page.get("dateModified") != expected_modified:
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

    if slug == "python-aot-compilers":
        claims_path = path.parent / "claims.json"
        claims = json.loads(claims_path.read_text())
        if "entities" not in claims:
            raise SystemExit("claims.json must contain an 'entities' list")
        model_entities = claims["entities"]
        if not isinstance(model_entities, list) or not model_entities:
            raise SystemExit("claims.json entities must be a non-empty list")

        column_names = ["Input contract", "Output and runtime", "Current positioning"]
        html_entity_map = {}
        for entity_name, cells in parser.table_rows.items():
            if len(cells) != len(column_names):
                raise SystemExit(
                    f"comparison table row '{entity_name}' has "
                    f"{len(cells)} cells; expected {len(column_names)}"
                )
            html_entity_map[entity_name] = dict(zip(column_names, cells))

        model_names = [e["name"] for e in model_entities]
        html_names = list(html_entity_map.keys())
        if set(html_names) != set(model_names):
            missing = set(model_names) - set(html_names)
            extra = set(html_names) - set(model_names)
            detail = []
            if missing:
                detail.append(f"missing from HTML: {sorted(missing)}")
            if extra:
                detail.append(f"extra in HTML: {sorted(extra)}")
            raise SystemExit(
                f"comparison entity set mismatch: {'; '.join(detail)}"
            )

        for entity in model_entities:
            name = entity["name"]
            html_cells = html_entity_map[name]
            for field, column in (
                ("input_contract", "Input contract"),
                ("html_output_cell", "Output and runtime"),
                ("positioning", "Current positioning"),
            ):
                expected = entity[field]
                actual = html_cells[column]
                if actual != expected:
                    raise SystemExit(
                        f"comparison cell mismatch for {name} "
                        f"{column!r}: expected {expected!r}, "
                        f"found {actual!r}"
                    )
            maturity = entity.get("maturity", "")
            if not maturity or not maturity.strip():
                raise SystemExit(
                    f"claims.json entity {name!r} has empty maturity"
                )
            sources = entity.get("sources", [])
            if not sources:
                raise SystemExit(
                    f"claims.json entity {name!r} has no sources"
                )
            for source in sources:
                url = source.get("url", "")
                if not url:
                    raise SystemExit(
                        f"claims.json entity {name!r} has a source with no URL"
                    )
                if url not in parser.anchors:
                    raise SystemExit(
                        f"comparison page is missing source link for "
                        f"{name!r}: {url}"
                    )
PY

python3 - \
  "$site_dir/index.html" \
  "$site_dir/python-aot-compilers/claims.json" <<'PY'
from html.parser import HTMLParser
from pathlib import Path
import json
import sys


landing_path = Path(sys.argv[1])
claims_path = Path(sys.argv[2])
claims = json.loads(claims_path.read_text())


class LandingTableParser(HTMLParser):
    def __init__(self):
        super().__init__()
        self.in_table = False
        self.in_tr = False
        self.in_th = False
        self.in_td = False
        self.in_col_header = False
        self.in_mini_mark = False
        self.current_cell_text = []
        self.current_row_header = None
        self.table_rows = {}
        self.column_headers = []

    def handle_starttag(self, tag, attrs):
        attributes = dict(attrs)
        if tag == "table":
            self.in_table = True
        elif tag == "tr" and self.in_table:
            self.in_tr = True
            self.current_row_header = None
        elif tag == "th" and self.in_tr:
            scope = attributes.get("scope", "")
            if scope == "row":
                self.in_th = True
                self.current_cell_text = []
            elif scope == "col":
                self.in_col_header = True
                self.current_cell_text = []
        elif tag == "td" and self.in_tr:
            self.in_td = True
            self.current_cell_text = []
        elif tag == "span" and self.in_th:
            if attributes.get("class", "") == "mini-mark":
                self.in_mini_mark = True

    def handle_endtag(self, tag):
        if tag == "table":
            self.in_table = False
        elif tag == "tr" and self.in_tr:
            self.in_tr = False
        elif tag == "th" and self.in_th:
            self.in_th = False
            self.current_row_header = "".join(
                self.current_cell_text
            ).strip()
            self.current_cell_text = []
        elif tag == "th" and self.in_col_header:
            self.in_col_header = False
            header_text = " ".join(
                "".join(self.current_cell_text).split()
            )
            self.column_headers.append(header_text)
            self.current_cell_text = []
        elif tag == "td" and self.in_td:
            self.in_td = False
            cell_text = " ".join(
                "".join(self.current_cell_text).split()
            )
            if self.current_row_header is not None:
                self.table_rows.setdefault(
                    self.current_row_header, []
                ).append(cell_text)
            self.current_cell_text = []
        elif tag == "span" and self.in_mini_mark:
            self.in_mini_mark = False

    def handle_data(self, data):
        if self.in_th and not self.in_mini_mark:
            self.current_cell_text.append(data)
        if self.in_col_header:
            self.current_cell_text.append(data)
        if self.in_td:
            self.current_cell_text.append(data)


if "landing_projection" not in claims:
    raise SystemExit(
        "claims.json must contain a 'landing_projection' block"
    )
projection = claims["landing_projection"]
for sub in ("column_sources", "labels", "anchors"):
    if sub not in projection or not projection[sub]:
        raise SystemExit(
            f"landing_projection.{sub} must be present and non-empty"
        )
column_sources = projection["column_sources"]
labels = projection["labels"]
anchors = projection["anchors"]

model_entities = claims["entities"]
model_names = [e["name"] for e in model_entities]
model_fields = set(model_entities[0].keys())
for entity in model_entities:
    model_fields &= set(entity.keys())

for column, field in column_sources.items():
    if field not in model_fields:
        raise SystemExit(
            f"landing_projection.column_sources.{column} maps to "
            f"unknown claim field {field!r}"
        )

parser = LandingTableParser()
parser.feed(landing_path.read_text())
landing_rows = parser.table_rows

expected_headers = ["Tool", "Static model", "Output artifact", "Language contract"]
if parser.column_headers != expected_headers:
    raise SystemExit(
        f"landing table column headers mismatch: "
        f"expected {expected_headers!r}, "
        f"found {parser.column_headers!r}"
    )

landing_keys = set(landing_rows.keys())
label_keys = set(labels.keys())
if landing_keys != label_keys:
    missing = label_keys - landing_keys
    extra = landing_keys - label_keys
    detail = []
    if missing:
        detail.append(f"missing from landing HTML: {sorted(missing)}")
    if extra:
        detail.append(f"extra in landing HTML: {sorted(extra)}")
    raise SystemExit(
        f"landing entity set mismatch: {'; '.join(detail)}"
    )
if not label_keys <= set(model_names):
    raise SystemExit(
        "landing_projection.labels entities must be a subset of "
        "the model entities"
    )

columns = ["static_model", "output_artifact", "language_contract"]
for entity_name, cells in landing_rows.items():
    if len(cells) != len(columns):
        raise SystemExit(
            f"landing table row '{entity_name}' has {len(cells)} cells; "
            f"expected {len(columns)}"
        )
    for column, cell_text in zip(columns, cells):
        expected = labels[entity_name][column]
        if cell_text != expected:
            raise SystemExit(
                f"landing cell mismatch for {entity_name} "
                f"{column!r}: expected {expected!r}, "
                f"found {cell_text!r}"
            )

for entity_name in labels:
    if entity_name not in anchors:
        raise SystemExit(
            f"landing_projection.anchors missing entity {entity_name!r}"
        )
    if set(anchors[entity_name].keys()) != set(labels[entity_name].keys()):
        raise SystemExit(
            f"landing_projection anchors/labels column mismatch for "
            f"{entity_name!r}"
        )
    entity_record = next(
        e for e in model_entities if e["name"] == entity_name
    )
    for column in columns:
        anchor = anchors[entity_name][column]
        if not anchor or not anchor.strip():
            raise SystemExit(
                f"landing_projection.anchors.{entity_name}.{column} "
                f"must be a non-empty, non-whitespace token"
            )
        field = column_sources[column]
        claim_value = entity_record[field]
        if anchor.lower() not in claim_value.lower():
            raise SystemExit(
                f"landing anchor for {entity_name} {column!r} "
                f"({anchor!r}) not found in claim field "
                f"{field!r}: {claim_value!r}"
            )
PY

assert_once "Sitemap: ${canonical}sitemap.xml" "$site_dir/robots.txt"

python3 - \
  "$site_dir/sitemap.xml" \
  "$site_dir/llms.txt" \
  "$site_dir/index.html.md" \
  "$site_dir/index.html" \
  "$site_dir/status/index.html" \
  "$site_dir/architecture/index.html" \
  "$site_dir/python-aot-compilers/index.html" \
  "$site_dir/ai-native/index.html" \
  "$canonical" \
  "$repo_root" <<'PY'
from datetime import date
from html.parser import HTMLParser
import json
from pathlib import Path
import sys
import xml.etree.ElementTree as ET


sitemap_path = Path(sys.argv[1])
llms_path = Path(sys.argv[2])
markdown_path = Path(sys.argv[3])
page_paths = [Path(argument) for argument in sys.argv[4:9]]
canonical = sys.argv[9]
repo_root = Path(sys.argv[10])
site_dir = llms_path.parent


class JsonLdParser(HTMLParser):
    def __init__(self):
        super().__init__()
        self.in_json_ld = False
        self.current = []
        self.documents = []

    def handle_starttag(self, tag, attrs):
        attributes = dict(attrs)
        if tag == "script" and attributes.get("type") == "application/ld+json":
            self.in_json_ld = True
            self.current = []

    def handle_data(self, data):
        if self.in_json_ld:
            self.current.append(data)

    def handle_endtag(self, tag):
        if tag == "script" and self.in_json_ld:
            self.documents.append(json.loads("".join(self.current)))
            self.in_json_ld = False


def webpage_modified(path):
    parser = JsonLdParser()
    parser.feed(path.read_text())
    pages = []
    for document in parser.documents:
        candidates = document.get("@graph", []) if isinstance(document, dict) else []
        candidates = [document, *candidates]
        pages.extend(
            candidate
            for candidate in candidates
            if isinstance(candidate, dict) and candidate.get("@type") == "WebPage"
        )
    if len(pages) != 1 or not pages[0].get("url") or not pages[0].get("dateModified"):
        raise SystemExit(f"Expected one dated WebPage JSON-LD object in {path}")
    return pages[0]["url"], pages[0]["dateModified"]


expected_last_modified = dict(webpage_modified(path) for path in page_paths)

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
    location = (entry.findtext("s:loc", namespaces=namespace) or "").strip()
    last_modified_nodes = entry.findall("s:lastmod", namespace)
    if len(last_modified_nodes) != 1:
        raise SystemExit(
            "Each sitemap URL entry must contain exactly one lastmod"
        )
    last_modified = (last_modified_nodes[0].text or "").strip()
    try:
        last_modified_date = date.fromisoformat(last_modified or "")
    except ValueError as error:
        raise SystemExit(
            "Sitemap lastmod must be an ISO 8601 calendar date"
        ) from error
    if last_modified_date > date.today():
        raise SystemExit("Sitemap lastmod cannot be in the future")
    if last_modified != expected_last_modified.get(location):
        raise SystemExit(
            f"Sitemap lastmod for {location} must match its WebPage dateModified"
        )

llms = llms_path.read_text()
if not llms.startswith("# pycc\n\n> "):
    raise SystemExit("llms.txt must start with the project H1 and blockquote summary")
if not llms.startswith(
    "# pycc\n\n> pycc is a pre-alpha strict ahead-of-time compiler for typed, standard Python"
):
    raise SystemExit("llms.txt summary must put the product before provenance")
# --- Issue #207: the blockquote summary must be a single physical line
# so reference consumers (llms_txt2ctx) do not truncate it after the
# first line and push the rest into the info field with literal > prefixes.
first_blockquote_line = llms.split("\n")[2]  # line 0: H1, line 1: blank, line 2: > ...
if not first_blockquote_line.startswith("> "):
    raise SystemExit("llms.txt must have a blockquote summary on line 3")
second_line = llms.split("\n")[3]
if second_line.startswith(">"):
    raise SystemExit(
        "llms.txt blockquote summary must be a single physical line (issue #207) — "
        "reference consumers truncate multi-line blockquotes"
    )
for heading in ("## Project", "## Specifications", "## Optional"):
    if heading not in llms:
        raise SystemExit(f"llms.txt must contain a {heading!r} section")
for required_link in (
    f"[Canonical website]({canonical})",
    f"[Markdown landing]({canonical}index.html.md)",
    f"[Current implementation status]({canonical}status/)",
    f"[Compiler architecture]({canonical}architecture/)",
    f"[Python AOT compiler comparison]({canonical}python-aot-compilers/)",
    f"[AI-native experiment]({canonical}ai-native/)",
    "[Source repository](https://github.com/rotnov/pycc)",
    "[Specification index](https://raw.githubusercontent.com/rotnov/pycc/main/docs/SPEC.md)",
    "[README](https://raw.githubusercontent.com/rotnov/pycc/main/README.md)",
):
    if required_link not in llms:
        raise SystemExit(f"llms.txt is missing required link: {required_link}")

# --- Issue #207: bounded Markdown-first llms.txt expansion ---
# The non-optional (Project and Specifications) sections must expand to a
# bounded context of Markdown/plain-text documents, never GitHub blob
# application-shell HTML. A hermetic manifest
# (site/llms-txt-context-manifest.json) binds each non-optional link to its
# local source file, representation role, and per-resource byte budget. The
# validator computes actual byte counts from the checked-out repository so the
# 256 KiB aggregate ceiling and per-resource budgets are enforced without any
# live network fetch. See docs/WEBSITE.md "llms.txt bounded expansion contract"
# for the full consumer contract.
import re as _re
manifest_path = site_dir / "llms-txt-context-manifest.json"
if not manifest_path.is_file():
    raise SystemExit("site/llms-txt-context-manifest.json is required (issue #207)")
manifest = json.loads(manifest_path.read_text())
if manifest.get("schema") != "pycc-llms-txt-context-manifest/v1":
    raise SystemExit("llms.txt context manifest has an unexpected schema")
budget_kib = manifest.get("budget_kib")
if not isinstance(budget_kib, int) or budget_kib <= 0:
    raise SystemExit("llms.txt context manifest budget_kib must be a positive integer")
manifest_docs = manifest.get("non_optional_documents")
if not isinstance(manifest_docs, list) or not manifest_docs:
    raise SystemExit("llms.txt context manifest must list non_optional_documents")


def _parse_llms_txt_sections(text):
    """Return {section_heading: [(label, url), ...]} for non-optional sections.

    Non-optional sections are every ## heading except ## Optional. Links are
    parsed from Markdown list items: ``- [label](url): description``.
    """
    sections = {}
    current = None
    link_re = _re.compile(r"^- \[([^\]]+)\]\(([^)]+)\)")
    for line in text.splitlines():
        if line.startswith("## "):
            current = line[3:].strip()
            sections.setdefault(current, [])
            continue
        if current is None:
            continue
        match = link_re.match(line)
        if match:
            sections[current].append((match.group(1), match.group(2)))
    return sections


def _non_optional_links(text):
    sections = _parse_llms_txt_sections(text)
    links = []
    for heading, items in sections.items():
        if heading.lower().startswith("optional"):
            continue
        links.extend(items)
    return links


non_optional = _non_optional_links(llms)
if not non_optional:
    raise SystemExit("llms.txt must have non-optional Project/Specifications links")
manifest_by_url = {doc["url"]: doc for doc in manifest_docs}
manifest_urls = set(manifest_by_url)
llms_urls = {url for _, url in non_optional}
if llms_urls != manifest_urls:
    missing = manifest_urls - llms_urls
    extra = llms_urls - manifest_urls
    detail = []
    if missing:
        detail.append(f"missing from llms.txt: {sorted(missing)}")
    if extra:
        detail.append(f"missing from manifest: {sorted(extra)}")
    raise SystemExit(
        "llms.txt non-optional links must match the context manifest (issue #207) — "
        + "; ".join(detail)
    )

# Reject GitHub blob/application URLs and duplicate representations in the
# non-optional expansion. Enforce that every non-optional URL is Markdown or
# plain-text, not HTML UI.
seen_local = set()
total_bytes = 0
for label, url in non_optional:
    doc = manifest_by_url[url]
    representation = doc.get("representation", "")
    if representation not in ("markdown", "plain-text"):
        raise SystemExit(
            f"llms.txt non-optional link {label!r} must be markdown/plain-text, "
            f"not {representation!r} (issue #207)"
        )
    if "github.com/" in url and "/blob/" in url:
        raise SystemExit(
            f"llms.txt non-optional link {label!r} uses a GitHub blob URL "
            f"({url}); use raw.githubusercontent.com for tracked Markdown "
            f"documents (issue #207)"
        )
    if representation == "markdown" and not (
        url.startswith("https://raw.githubusercontent.com/") or url.endswith(".md")
    ):
        raise SystemExit(
            f"llms.txt non-optional link {label!r} declares markdown but its URL "
            f"({url}) is not a raw Markdown source or .md page (issue #207)"
        )
    local_path = repo_root / doc["local_path"]
    if not local_path.is_file():
        raise SystemExit(
            f"llms.txt context manifest local_path {doc['local_path']!r} "
            f"for {label!r} does not exist in the repository (issue #207)"
        )
    actual_bytes = local_path.stat().st_size
    budget_bytes = doc.get("budget_bytes")
    if not isinstance(budget_bytes, int) or budget_bytes <= 0:
        raise SystemExit(
            f"llms.txt context manifest budget_bytes for {label!r} "
            f"must be a positive integer (issue #207)"
        )
    if actual_bytes > budget_bytes:
        raise SystemExit(
            f"llms.txt non-optional document {label!r} is {actual_bytes} bytes, "
            f"exceeding its {budget_bytes}-byte per-resource budget (issue #207)"
        )
    if doc["local_path"] in seen_local:
        raise SystemExit(
            f"llms.txt non-optional expansion duplicates local representation "
            f"{doc['local_path']!r} (issue #207)"
        )
    seen_local.add(doc["local_path"])
    total_bytes += actual_bytes

budget_ceiling = budget_kib * 1024
if total_bytes > budget_ceiling:
    raise SystemExit(
        f"llms.txt non-optional expansion is {total_bytes} bytes, exceeding the "
        f"{budget_ceiling}-byte ({budget_kib} KiB) aggregate budget (issue #207)"
    )

# Reject duplicate HTML+Markdown representations of the same page in the
# non-optional expansion (e.g. both the canonical landing HTML and its Markdown
# equivalent). The Markdown landing is the clean representation; the canonical
# HTML landing must stay in the Optional section.
non_optional_canonical_html = canonical.rstrip("/") + "/"
non_optional_html_pages = {
    url for _, url in non_optional
    if url.endswith("/") and "rotnov.github.io/pycc" in url
}
if non_optional_canonical_html in non_optional_html_pages:
    raise SystemExit(
        "llms.txt non-optional expansion must not include the canonical HTML "
        "landing; use the Markdown landing and keep the HTML page in Optional "
        "(issue #207)"
    )

markdown = markdown_path.read_text()
if not markdown.startswith("# pycc — AOT compiler for typed Python to native binaries"):
    raise SystemExit("index.html.md must start with the canonical page title")
if not markdown.startswith(
    "# pycc — AOT compiler for typed Python to native binaries\n\n"
    "> pycc is a pre-alpha ahead-of-time compiler for typed, standard Python"
):
    raise SystemExit("index.html.md summary must put the product before provenance")
for disclosure in (
    "AI agents create",
    "a human manages",
    "A human only manages goals, constraints,",
    "No project code is handwritten by a human.",
    "pycc is not an AI\nor machine-learning compiler.",
):
    if disclosure not in llms or disclosure not in markdown:
        raise SystemExit(f"LLM-readable files are missing disclosure: {disclosure}")
current_status = "`pycc check` now parses and type-checks the v0.1"
if current_status not in llms or current_status not in markdown:
    raise SystemExit(
        "LLM-readable files are missing the current frontend status"
    )
backend_status = "`pycc build` and `pycc run` compile that implemented surface"
if backend_status not in llms or backend_status not in markdown:
    raise SystemExit(
        "LLM-readable files are missing the current backend status"
    )
artifact_contract = (
    "Native and pure builds emit standalone executables; planned permitted CPython\n"
    "interop emits a self-contained bundle with its pinned runtime."
)
if artifact_contract not in markdown:
    raise SystemExit(
        "Markdown website is missing the native-versus-interop artifact contract"
    )
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

# --- Issue #206: bind Markdown landing to HTML page semantic contract ---
# The Markdown must cover the same key facts as the HTML page so that
# one-sided semantic drift is detected.

# Must mention the same authoritative resources as the HTML page.
for resource in (
    "https://github.com/rotnov/pycc",
    "https://github.com/rotnov/pycc/blob/main/docs/SPEC.md",
    "https://github.com/rotnov/pycc/blob/main/docs/ROADMAP.md",
):
    if resource not in markdown:
        raise SystemExit(
            f"Markdown website is missing authoritative resource: {resource}"
        )

# Must mention the conformance evidence (matching the HTML page).
for conformance_claim in (
    "fib",
    "mandelbrot-ascii",
    "five Tier-1 targets",
):
    if conformance_claim not in markdown:
        raise SystemExit(
            f"Markdown website is missing conformance claim: {conformance_claim}"
        )

# Must mention the code example features (matching the HTML hero).
for code_feature in (
    "statement-form",
    "recursive_fibonacci_matches_the_well_known_sequence",
):
    if code_feature not in markdown:
        raise SystemExit(
            f"Markdown website is missing code example feature: {code_feature}"
        )

# Must not claim production readiness (matching the HTML page's pre-alpha status).
md_lower = markdown.lower()
for false_claim in ("production-ready", "production ready", "stable release"):
    if false_claim in md_lower:
        raise SystemExit(
            f"Markdown website must not claim '{false_claim}' — pycc is pre-alpha"
        )

# Must mention the v0.1/v0.2 acceptance status (matching the HTML page).
if "v0.1" not in markdown or "acceptance criteria" not in markdown:
    raise SystemExit(
        "Markdown website must mention v0.1 acceptance criteria status"
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

python3 - "$site_dir/404.html" <<'PY'
from html.parser import HTMLParser
from pathlib import Path
import sys


class NotFoundParser(HTMLParser):
    void_tags = {
        "area", "base", "br", "col", "embed", "hr", "img", "input",
        "link", "meta", "param", "source", "track", "wbr",
    }

    def __init__(self):
        super().__init__()
        self.lang = None
        self.in_title = False
        self.titles = []
        self.current_title = []
        self.metas = []
        self.links = []
        self.stylesheets = []
        self.in_body = False
        self.hidden_body_depth = 0
        self.body_element_stack = []
        self.visible_headings = []
        self.current_heading = []
        self.in_heading = False
        self.anchor_hrefs = []
        self.in_anchor = False
        self.charset_seen = False
        self.viewport_seen = False

    def handle_starttag(self, tag, attrs):
        attributes = dict(attrs)
        if tag == "html":
            self.lang = attributes.get("lang", "")
        if tag == "meta" and attributes.get("charset", "").lower() == "utf-8":
            self.charset_seen = True
        if (
            tag == "meta"
            and attributes.get("name", "").lower() == "viewport"
        ):
            self.viewport_seen = True
        if tag == "meta":
            self.metas.append(attributes)
        if tag == "link":
            self.links.append(attributes)
            if "stylesheet" in attributes.get("rel", "").lower().split():
                self.stylesheets.append(attributes.get("href", ""))
        if tag == "title":
            self.in_title = True
            self.current_title = []
        if tag == "body":
            self.in_body = True
            return
        if self.in_body:
            inline_style = attributes.get("style", "").replace(" ", "").lower()
            is_hidden = (
                self.hidden_body_depth > 0
                or tag in {"script", "style", "template", "noscript"}
                or "hidden" in attributes
                or attributes.get("aria-hidden", "").lower() == "true"
                or "display:none" in inline_style
                or "visibility:hidden" in inline_style
            )
            if tag not in self.void_tags:
                self.body_element_stack.append((tag, is_hidden))
                if is_hidden:
                    self.hidden_body_depth += 1
            if tag in {"h1", "h2", "h3"} and not is_hidden:
                self.in_heading = True
                self.current_heading = []
            if tag == "a" and not is_hidden:
                self.in_anchor = True
                self.anchor_hrefs.append(attributes.get("href", ""))
            return

    def handle_startendtag(self, tag, attrs):
        if tag not in self.void_tags:
            raise SystemExit(f"<{tag}> must use an explicit closing tag")
        self.handle_starttag(tag, attrs)

    def handle_endtag(self, tag):
        if tag == "body":
            self.in_body = False
            self.body_element_stack = []
            self.hidden_body_depth = 0
            return
        if self.in_body:
            if tag == "a":
                self.in_anchor = False
            if tag in {"h1", "h2", "h3"} and self.in_heading:
                self.visible_headings.append(
                    "".join(self.current_heading).strip()
                )
                self.in_heading = False
            if self.body_element_stack:
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

    def handle_data(self, data):
        if self.in_title:
            self.current_title.append(data)
        if self.in_body and self.hidden_body_depth == 0:
            if self.in_heading:
                self.current_heading.append(data)


path = Path(sys.argv[1])
content = path.read_text()
if not content.strip():
    raise SystemExit("404.html must not be empty")
parser = NotFoundParser()
parser.feed(content)

if not parser.lang:
    raise SystemExit("404.html must declare a lang attribute")
if not parser.charset_seen:
    raise SystemExit("404.html must declare a UTF-8 charset")
if not parser.viewport_seen:
    raise SystemExit("404.html must declare a viewport meta tag")

if len(parser.titles) != 1:
    raise SystemExit(
        f"404.html must have exactly one title; found {len(parser.titles)}"
    )
title = parser.titles[0]
if not title:
    raise SystemExit("404.html title must not be empty")

robots_metas = [
    meta for meta in parser.metas
    if meta.get("name", "").lower() == "robots"
]
if len(robots_metas) != 1:
    raise SystemExit(
        "404.html must have exactly one robots meta directive"
    )
if robots_metas[0].get("content", "").strip().lower() != "noindex":
    raise SystemExit("404.html robots directive must be noindex")

not_found_headings = [
    heading for heading in parser.visible_headings
    if "not found" in heading.lower()
]
if not not_found_headings:
    raise SystemExit(
        "404.html must have a visible not-found heading"
    )

stylesheet_hrefs = parser.stylesheets
if len(stylesheet_hrefs) != 1:
    raise SystemExit(
        "404.html must link exactly one stylesheet"
    )
if stylesheet_hrefs[0] != "/pycc/styles.css":
    raise SystemExit(
        "404.html stylesheet must use the absolute /pycc/styles.css path"
    )

canonical_links = [
    link for link in parser.links
    if "canonical" in link.get("rel", "").split()
]
if len(canonical_links) != 1:
    raise SystemExit("404.html must have exactly one canonical link")
if canonical_links[0].get("href") != "https://rotnov.github.io/pycc/":
    raise SystemExit("404.html canonical link must point to the pycc origin")

required_recovery = {
    "/pycc/": "home",
    "/pycc/status/": "status",
    "/pycc/architecture/": "architecture",
    "/pycc/python-aot-compilers/": "python-aot-compilers",
}
recovery_hrefs = set(parser.anchor_hrefs)
for href, label in required_recovery.items():
    if href not in recovery_hrefs:
        raise SystemExit(
            f"404.html must link the {label} recovery route ({href})"
        )

evidence_routes = {
    "/pycc/status/",
    "/pycc/architecture/",
    "/pycc/python-aot-compilers/",
}
if not evidence_routes <= recovery_hrefs:
    raise SystemExit(
        "404.html must link at least three evidence pages"
    )

for href in parser.anchor_hrefs:
    if href.startswith("http"):
        continue
    if not href.startswith("/pycc/"):
        raise SystemExit(
            f"404.html navigation link must use an absolute /pycc/ path: {href}"
        )
for href in stylesheet_hrefs:
    if href.startswith("http"):
        continue
    if not href.startswith("/pycc/"):
        raise SystemExit(
            f"404.html asset URL must use an absolute /pycc/ path: {href}"
        )
PY

# --- README comparison table binding (Part 2 of #162) ---
# Validate the root README's compiler comparison table against the
# readme_projection block in claims.json.

# `readme_path` was already resolved at the top of this script, honoring an
# explicit README_PATH override. Only derive it from `site_dir` when no
# override was given, so the override reaches every consumer below.
if [ -z "${README_PATH:-}" ]; then
  readme_path="${site_dir%/site}/README.md"
  if [ ! -f "$readme_path" ]; then
    readme_path="README.md"
  fi
fi

python3 - \
  "$readme_path" \
  "$site_dir/python-aot-compilers/claims.json" <<'PY'
import json
import re
import sys
from pathlib import Path


readme_path = Path(sys.argv[1])
claims_path = Path(sys.argv[2])
claims = json.loads(claims_path.read_text())

if "readme_projection" not in claims:
    raise SystemExit("claims.json must contain a 'readme_projection' block")

projection = claims["readme_projection"]
column_sources = projection.get("column_sources", {})
labels = projection.get("labels", {})
row_order = projection.get("row_order", [])

if not column_sources or not labels or not row_order:
    raise SystemExit(
        "readme_projection.column_sources, .labels, and .row_order "
        "must be present and non-empty"
    )

readme_text = readme_path.read_text()

# Find the comparison table. It starts with a header row containing
# "Enforces types at compile time" and ends at the next blank line.
table_lines = []
in_table = False
for line in readme_text.splitlines():
    if "Enforces types at compile time" in line:
        in_table = True
        table_lines.append(line)
        continue
    if in_table:
        if not line.strip() or not line.strip().startswith("|"):
            break
        table_lines.append(line)

if not table_lines:
    raise SystemExit("README comparison table not found")

# Parse the table. Skip header and separator rows.
# Expected columns: empty first col (entity name), type_enforcement, native_executable, standard_python
readme_rows = {}
for line in table_lines:
    if "Enforces types at compile time" in line:
        continue
    if re.match(r"^\|[\s\-|]+\|$", line):
        continue
    cells = [c.strip() for c in line.split("|")]
    # Remove empty leading/trailing cells from | ... | format
    cells = [c for c in cells if c != ""]
    if len(cells) < 4:
        continue
    entity_name = cells[0].rstrip(".")
    # Remove markdown bold markers
    entity_name = entity_name.replace("**", "")
    # Handle "pycc (v1.0 design target)" -> "pycc"
    entity_name = entity_name.split("(")[0].strip()
    # Handle "mypy / pyright" -> split into two entities
    if "/" in entity_name and entity_name not in labels:
        parts = [p.strip() for p in entity_name.split("/")]
        for part in parts:
            readme_rows[part] = cells[1:]
    else:
        readme_rows[entity_name] = cells[1:]

# Check row order matches
readme_names = list(readme_rows.keys())
expected_names = row_order

# The README may combine mypy/pyright into one row; allow that
if set(readme_names) != set(expected_names):
    # Check if mypy and pyright are combined
    if "mypy" in readme_names and "pyright" not in readme_names:
        # The combined row covers both
        if set(readme_names) | {"pyright"} == set(expected_names) or set(readme_names) == set(expected_names) - {"pyright"}:
            pass  # Acceptable
        else:
            missing = set(expected_names) - set(readme_names)
            extra = set(readme_names) - set(expected_names)
            detail = []
            if missing:
                detail.append(f"missing from README: {sorted(missing)}")
            if extra:
                detail.append(f"extra in README: {sorted(extra)}")
            raise SystemExit(
                f"README comparison entity set mismatch: {'; '.join(detail)}"
            )
    else:
        missing = set(expected_names) - set(readme_names)
        extra = set(readme_names) - set(expected_names)
        detail = []
        if missing:
            detail.append(f"missing from README: {sorted(missing)}")
        if extra:
            detail.append(f"extra in README: {sorted(extra)}")
        raise SystemExit(
            f"README comparison entity set mismatch: {'; '.join(detail)}"
        )

# Check each cell matches the projection labels
columns = ["type_enforcement", "native_executable", "standard_python"]
for entity_name in readme_names:
    if entity_name not in labels:
        # If this is a combined mypy/pyright row, check against mypy
        if entity_name == "mypy":
            continue
        raise SystemExit(
            f"README comparison row {entity_name!r} has no "
            f"readme_projection.labels entry"
        )
    expected_labels = labels[entity_name]
    actual_cells = readme_rows[entity_name]
    for i, column in enumerate(columns):
        if i >= len(actual_cells):
            raise SystemExit(
                f"README comparison row {entity_name!r} has too few cells"
            )
        expected = expected_labels[column]
        actual = actual_cells[i]
        # Normalize: remove emoji indicators for comparison
        actual_normalized = re.sub(r"[✅❌⚠️`]", "", actual).strip()
        if actual_normalized != expected:
            raise SystemExit(
                f"README comparison cell mismatch for {entity_name} "
                f"{column!r}: expected {expected!r}, "
                f"found {actual_normalized!r}"
            )
PY

# --- Issue #197: bind the public quick-start example across README, site,
# and the canonical fixture. The fixture is the single source of truth; the
# README `cat hello.py` block, the site hero `<pre><code>` source, the
# copy-button command, and the documented output are all bound to it and to
# each other so cross-file drift is detected.

python3 - \
  "$index" \
  "$readme_path" \
  "$quick_start_fixture" \
  "$website_md_path" \
  "$quick_start_expected" \
  "$quick_start_diagnostic" \
  "$quick_start_diagnostic_source" <<'PY'
from html.parser import HTMLParser
from pathlib import Path
import re
import sys


def normalize_source(text):
    """Normalize Python source for semantic comparison.

    Preserves leading indentation (semantically significant in Python) while
    normalizing line endings, stripping trailing whitespace, and collapsing
    blank lines. This is stricter than whitespace collapsing.
    """
    lines = text.replace("\r\n", "\n").replace("\r", "\n").split("\n")
    stripped = [line.rstrip() for line in lines]
    # Drop leading and trailing blank lines, collapse internal blank runs.
    result = []
    for line in stripped:
        if line == "" and (not result or result[-1] == ""):
            continue
        result.append(line)
    while result and result[-1] == "":
        result.pop()
    while result and result[0] == "":
        result.pop(0)
    return "\n".join(result)


def read_fixture(path):
    """Read a fixture and drop its single trailing newline.

    Every fixture compared here is a POSIX text file ending in exactly one
    `\n`; the published copies (a README fence body, an HTML `<pre><code>`
    pane) have no such terminator because the fence/tag closes on the last
    content line. Stripping exactly one trailing newline -- rather than
    `.strip()`-ing or collapsing whitespace -- keeps the comparison
    byte-exact in both directions: a fixture that gains a blank final line,
    or a published copy that gains or loses trailing whitespace, still fails.
    Windows checkouts under the default `core.autocrlf` also carry `\r\n`,
    so line endings are normalized first (there is no `.gitattributes` to
    pin them, and workflow-policy.yml rejects adding one).
    """
    text = path.read_text().replace("\r\n", "\n")
    if text.endswith("\n"):
        text = text[:-1]
    return text


# The evidence-state vocabulary is shared with the generated evidence-hero
# contract (#564). A marker outside this set is drift, not a new state.
EVIDENCE_STATES = {
    "all-Tier-1",
    "partial",
    "experimental",
    "unavailable",
    "superseded",
}

DIAGNOSTIC_FIXTURE_REL = "tests/diagnostics/quick_start_type_error.expected.txt"
DIAGNOSTIC_SOURCE_REL = "tests/diagnostics/quick_start_type_error.py"
HERO_COMMAND = "pycc build hello.py -o hello"


class HeroCodeParser(HTMLParser):
    def __init__(self):
        super().__init__()
        self.in_hero_code = False
        self.hero_div_depth = 0
        # The hero carries two independent `<pre><code>` panes: the program
        # source in `.code-window` and that program's stdout in
        # `.output-window`. They are accumulated into separate buffers and
        # each is compared against its own fixture. A parser that lumped
        # every `<pre><code>` inside `.hero-code` into one buffer would
        # concatenate source and output, so neither could be compared -- the
        # gate would still pass while checking nothing.
        self.in_code_window = False
        self.code_window_depth = 0
        self.in_output_window = False
        self.output_window_depth = 0
        self.in_pre = False
        self.in_pre_code = False
        self.hero_code_text = []
        self.hero_output_text = []
        self.in_command = False
        self.command_div_depth = 0
        self.command_in_code = False
        self.command_text = []
        self.copy_button_data = None
        self.in_command_note = False
        self.command_note_text = []
        self.in_provenance = False
        self.provenance_text = []
        self.provenance_hrefs = []
        self.in_evidence_state = False
        self.evidence_state_attr = None
        self.evidence_state_text = []

    def handle_starttag(self, tag, attrs):
        attributes = dict(attrs)
        classes = attributes.get("class", "").split()
        if tag == "div" and "hero-code" in classes:
            self.in_hero_code = True
            self.hero_div_depth = 1
            return
        if tag == "div" and "command" in classes:
            self.in_command = True
            self.command_div_depth = 1
            return
        if tag == "p" and "command-note" in classes:
            self.in_command_note = True
            self.command_note_text = []
            return
        if tag == "p" and "hero-provenance" in classes:
            self.in_provenance = True
            self.provenance_text = []
            return
        if "evidence-state" in classes:
            self.in_evidence_state = True
            self.evidence_state_attr = attributes.get("data-evidence-state")
        if self.in_provenance and tag == "a":
            self.provenance_hrefs.append(attributes.get("href", ""))
        if self.in_command:
            if tag == "div":
                self.command_div_depth += 1
            if tag == "code":
                self.command_in_code = True
        if self.in_hero_code:
            if tag == "div":
                self.hero_div_depth += 1
                if self.in_code_window:
                    self.code_window_depth += 1
                elif self.in_output_window:
                    self.output_window_depth += 1
                elif "code-window" in classes:
                    self.in_code_window = True
                    self.code_window_depth = 1
                elif "output-window" in classes:
                    self.in_output_window = True
                    self.output_window_depth = 1
            if tag == "pre":
                self.in_pre = True
            if tag == "code" and self.in_pre:
                self.in_pre_code = True
        if tag == "button" and "copy-button" in classes:
            self.copy_button_data = attributes.get("data-copy", "")

    def handle_endtag(self, tag):
        if self.in_command_note and tag == "p":
            self.in_command_note = False
        if self.in_provenance and tag == "p":
            self.in_provenance = False
        if self.in_evidence_state and tag == "span":
            self.in_evidence_state = False
        if self.in_command:
            if tag == "code":
                self.command_in_code = False
            if tag == "div":
                self.command_div_depth -= 1
                if self.command_div_depth <= 0:
                    self.in_command = False
        if self.in_hero_code:
            if tag == "code":
                self.in_pre_code = False
            if tag == "pre":
                self.in_pre = False
            if tag == "div":
                if self.in_code_window:
                    self.code_window_depth -= 1
                    if self.code_window_depth <= 0:
                        self.in_code_window = False
                elif self.in_output_window:
                    self.output_window_depth -= 1
                    if self.output_window_depth <= 0:
                        self.in_output_window = False
                self.hero_div_depth -= 1
                if self.hero_div_depth <= 0:
                    self.in_hero_code = False

    def handle_data(self, data):
        if self.in_hero_code and self.in_pre_code:
            if self.in_code_window:
                self.hero_code_text.append(data)
            elif self.in_output_window:
                self.hero_output_text.append(data)
        if self.in_command and self.command_in_code:
            self.command_text.append(data)
        if self.in_command_note:
            self.command_note_text.append(data)
        if self.in_provenance:
            self.provenance_text.append(data)
        if self.in_evidence_state:
            self.evidence_state_text.append(data)


index_path = Path(sys.argv[1])
readme_path = Path(sys.argv[2])
fixture_path = Path(sys.argv[3])
website_md_path = Path(sys.argv[4])
expected_output_path = Path(sys.argv[5])
diagnostic_fixture_path = Path(sys.argv[6])
diagnostic_source_path = Path(sys.argv[7])

fixture_source = read_fixture(fixture_path)
expected_output = read_fixture(expected_output_path)
expected_diagnostic = read_fixture(diagnostic_fixture_path)
diagnostic_source = read_fixture(diagnostic_source_path)

# The published diagnostic is bound to the compiler's own generated fixture,
# so the fixture itself must still be the placeholder-span shape D-083
# describes and D-043 owns fixing. If a renderer change moves the span, this
# fires before any published copy is compared.
diagnostic_path_line = f" --> {DIAGNOSTIC_SOURCE_REL}:1:1"
if diagnostic_path_line not in expected_diagnostic:
    raise SystemExit(
        f"{DIAGNOSTIC_FIXTURE_REL} must contain the exact path line "
        f"{diagnostic_path_line!r}; the README copy is generated from it by "
        f"substituting the path, so a changed span silently falsifies the "
        f"published example"
    )

# --- README quick-start extraction ---
readme_text = readme_path.read_text().replace("\r\n", "\n")
quick_start_match = re.search(
    r"^## Quick start\s*$",
    readme_text,
    re.MULTILINE,
)
if not quick_start_match:
    raise SystemExit("README is missing a '## Quick start' heading")

console_match = re.search(
    r"```console\n(.*?)\n```",
    readme_text,
    re.DOTALL,
)
if not console_match:
    raise SystemExit("README Quick start is missing a console fence block")
console_block = console_match.group(1)
console_lines = console_block.splitlines()

# Extract the Python source: lines between `$ cat hello.py` and the next
# line beginning with `$ `.
cat_marker = None
for idx, line in enumerate(console_lines):
    if line.strip() == "$ cat hello.py":
        cat_marker = idx
        break
if cat_marker is None:
    raise SystemExit("README Quick start console block is missing '$ cat hello.py'")
source_lines = []
end_idx = None
for idx in range(cat_marker + 1, len(console_lines)):
    if console_lines[idx].startswith("$ "):
        end_idx = idx
        break
    source_lines.append(console_lines[idx])
if end_idx is None:
    raise SystemExit("README Quick start console block has no command after cat")
readme_source = "\n".join(source_lines)

# Extract the expected output: lines after `$ ./hello` to the end of the
# console block.
hello_marker = None
for idx, line in enumerate(console_lines):
    if line.strip() == "$ ./hello":
        hello_marker = idx
        break
if hello_marker is None:
    raise SystemExit("README Quick start console block is missing '$ ./hello'")
output_lines = []
for idx in range(hello_marker + 1, len(console_lines)):
    output_lines.append(console_lines[idx])
readme_output = "\n".join(output_lines)

# Extract README quick-start command lines for the no-global-install guard.
readme_command_lines = [
    line for line in console_lines if line.startswith("$ ")
]

# --- README diagnostic-example extraction ---
# The published diagnostic block is anchored by an explicit HTML comment
# naming the fixture it is generated from, so the extraction cannot silently
# retarget onto some other fenced block if the surrounding prose is reworded.
diagnostic_match = re.search(
    r"<!-- #197: generated from (\S+) -->\n```\n(.*?)\n```",
    readme_text,
    re.DOTALL,
)
if not diagnostic_match:
    raise SystemExit(
        "README is missing the '<!-- #197: generated from ... -->' anchor "
        "immediately followed by the generated diagnostic fence block"
    )
declared_fixture = diagnostic_match.group(1)
if declared_fixture != DIAGNOSTIC_FIXTURE_REL:
    raise SystemExit(
        f"README diagnostic anchor must name {DIAGNOSTIC_FIXTURE_REL!r}, "
        f"found {declared_fixture!r}"
    )
readme_diagnostic = diagnostic_match.group(2)

# The README copy is the fixture with the fixture's own source path replaced
# by the `hello.py` the surrounding quick start uses -- nothing else.
substituted = expected_diagnostic.replace(DIAGNOSTIC_SOURCE_REL, "hello.py")
readme_diagnostic_path_line = " --> hello.py:1:1"
if readme_diagnostic_path_line not in readme_diagnostic:
    raise SystemExit(
        f"README diagnostic block must contain the exact path line "
        f"{readme_diagnostic_path_line!r}"
    )
if readme_diagnostic != substituted:
    raise SystemExit(
        "README diagnostic block is not the compiler-generated "
        f"{DIAGNOSTIC_FIXTURE_REL} with its source path substituted for "
        f"'hello.py':\nexpected:\n{substituted}\nfound:\n{readme_diagnostic}"
    )

# --- Diagnostic-source binding ---
# The README states the type-error example is the quick-start file "above"
# with one line appended. Nothing else enforces that sentence: the path-line
# checks above only compare `DIAGNOSTIC_SOURCE_REL` as a string and never read
# that file's body. Without this binding, editing tests/fixtures/quick_start.py
# drags every other published surface along through its own binding while this
# claim silently goes false. Bind all three: the README names the appended
# line, and the diagnostic source must be the canonical fixture plus exactly
# that one line.
appended_match = re.search(
    r"appending\s+`([^`]+)`\s+to\s+the\s+file\s+above",
    readme_text,
)
if not appended_match:
    raise SystemExit(
        "README must describe the type-error example as appending a named "
        "line to the quick-start file, in the form: appending `<line>` to "
        "the file above"
    )
appended_line = appended_match.group(1)
if "\n" in appended_line or not appended_line.strip():
    raise SystemExit(
        f"README's appended-line claim must name exactly one non-empty "
        f"source line, found {appended_line!r}"
    )
expected_diagnostic_source = fixture_source + "\n" + appended_line
if diagnostic_source != expected_diagnostic_source:
    raise SystemExit(
        f"{DIAGNOSTIC_SOURCE_REL} must be tests/fixtures/quick_start.py with "
        f"exactly the one line the README names ({appended_line!r}) appended, "
        f"so the README's 'the file above' claim stays true:\n"
        f"expected:\n{expected_diagnostic_source}\n"
        f"found:\n{diagnostic_source}"
    )

# --- Site hero extraction ---
index_text = index_path.read_text().replace("\r\n", "\n")
parser = HeroCodeParser()
parser.feed(index_text)

hero_source = "".join(parser.hero_code_text)
# HTMLParser already unescapes entities in handle_data, so hero_source is
# already unescaped. Normalize whitespace for comparison.
if normalize_source(hero_source) != normalize_source(fixture_source):
    raise SystemExit(
        "site/index.html hero <pre><code> source does not match "
        "tests/fixtures/quick_start.py (indentation-preserving)"
    )

if normalize_source(readme_source) != normalize_source(fixture_source):
    raise SystemExit(
        "README Quick start 'cat hello.py' source does not match "
        "tests/fixtures/quick_start.py (indentation-preserving)"
    )

# --- Documented output binding ---
# Both published copies are compared byte-for-byte against the fixture the
# `quick_start` integration test asserts the real binary produces.
if readme_output != expected_output:
    raise SystemExit(
        f"README Quick start '$ ./hello' output does not match "
        f"tests/fixtures/quick_start.expected.txt: "
        f"expected {expected_output!r}, found {readme_output!r}"
    )

hero_output = "".join(parser.hero_output_text)
if not hero_output:
    raise SystemExit(
        "site/index.html hero is missing an .output-window <pre><code> pane "
        "carrying the quick-start program's documented stdout"
    )
if hero_output != expected_output:
    raise SystemExit(
        f"site/index.html hero .output-window pane does not match "
        f"tests/fixtures/quick_start.expected.txt: "
        f"expected {expected_output!r}, found {hero_output!r} "
        f"(write the pane as `<pre><code>0` with no newline after <code>, "
        f"and close it on the last output line)"
    )

# --- Copy-button binding ---
command_displayed = "".join(parser.command_text).strip()
if command_displayed.startswith("$ "):
    command_displayed = command_displayed[2:]
if command_displayed != HERO_COMMAND:
    raise SystemExit(
        f"site hero displayed command must be {HERO_COMMAND!r}, "
        f"found {command_displayed!r}"
    )
if parser.copy_button_data is None:
    raise SystemExit("site hero is missing a .copy-button with data-copy")

# --- No-global-install guard ---
# These checks run on the raw data-copy value BEFORE the equality check below,
# so a copy-button carrying an install command or @<version> suffix is rejected
# on its own merit rather than only being caught by the displayed-command
# mismatch.
install_patterns = (
    "pip install",
    "brew install",
    "npm install",
    "cargo install",
    "apt install",
)
version_suffix = re.compile(r"@\d")
for line in readme_command_lines:
    stripped = line[2:] if line.startswith("$ ") else line
    for pattern in install_patterns:
        if pattern in stripped:
            raise SystemExit(
                f"README Quick start command must not assume a package-manager "
                f"install: found {pattern!r} in {stripped!r}"
            )
    if version_suffix.search(stripped):
        raise SystemExit(
            f"README Quick start command must not use a @<version> suffix: "
            f"found in {stripped!r}"
        )
copy_command = parser.copy_button_data
for pattern in install_patterns:
    if pattern in copy_command:
        raise SystemExit(
            f"site hero copy-button command must not assume a package-manager "
            f"install: found {pattern!r} in {copy_command!r}"
        )
if version_suffix.search(copy_command):
    raise SystemExit(
        f"site hero copy-button command must not use a @<version> suffix: "
        f"found in {copy_command!r}"
    )

if parser.copy_button_data != command_displayed:
    raise SystemExit(
        f"site hero copy-button data-copy ({parser.copy_button_data!r}) "
        f"does not match the displayed command ({command_displayed!r})"
    )

# --- No-"planned"-relapse guard ---
command_note = "".join(parser.command_note_text)
if "planned" in command_note.lower():
    raise SystemExit(
        "site hero .command-note must not contain 'planned' "
        "(the quick-start example is tested, not planned)"
    )

# --- Evidence-state marker ---
if parser.evidence_state_attr is None:
    raise SystemExit(
        "site hero is missing an .evidence-state marker with a "
        "data-evidence-state attribute"
    )
if parser.evidence_state_attr not in EVIDENCE_STATES:
    raise SystemExit(
        f"site hero data-evidence-state must be one of "
        f"{sorted(EVIDENCE_STATES)}, found {parser.evidence_state_attr!r}"
    )
evidence_state_text = "".join(parser.evidence_state_text).strip()
if evidence_state_text != parser.evidence_state_attr:
    raise SystemExit(
        f"site hero .evidence-state text ({evidence_state_text!r}) must match "
        f"its data-evidence-state attribute "
        f"({parser.evidence_state_attr!r})"
    )

# --- Provenance and limitations ---
provenance = " ".join("".join(parser.provenance_text).split())
if not provenance:
    raise SystemExit(
        "site hero is missing a .hero-provenance paragraph naming the example's "
        "source fixture, its verifying test, and its limitations"
    )
for required in ("tests/fixtures/quick_start.py", "tests/quick_start.rs"):
    if required not in provenance:
        raise SystemExit(
            f"site hero .hero-provenance must name {required!r}"
        )
if "Limitations:" not in provenance:
    raise SystemExit(
        "site hero .hero-provenance must carry a 'Limitations:' statement "
        "bounding what the compiled example demonstrates"
    )
for required in ("tests/fixtures/quick_start.py", "tests/quick_start.rs"):
    if not any(href.endswith(required) for href in parser.provenance_hrefs):
        raise SystemExit(
            f"site hero .hero-provenance must link to {required!r}, "
            f"found hrefs {parser.provenance_hrefs!r}"
        )

# --- Negative control: the fabricated diagnostic card must stay deleted ---
# The removed `.diagnostic-card` published a diagnostic no compiler version
# ever emitted (a `help:` line pycc's `render_human` does not render, and a
# span D-043 has not implemented). Reintroducing either shape is a relapse.
if "diagnostic-card" in index_text:
    raise SystemExit(
        "site/index.html must not carry a .diagnostic-card: the hero's "
        "diagnostic example was fabricated, and the real one lives in "
        "README.md bound to a compiler-generated fixture (#197)"
    )
if "help:" in index_text:
    raise SystemExit(
        "site/index.html must not publish a 'help:' diagnostic line: "
        "pycc's render_human does not emit one (D-083; D-043 owns the work)"
    )

# --- WEBSITE.md currency ---
website_md = website_md_path.read_text()
binding_phrase = "tested, executable v0.1 example"
if binding_phrase not in website_md:
    raise SystemExit(
        f"docs/WEBSITE.md must contain the binding phrase "
        f"{binding_phrase!r} identifying the quick start as a tested example"
    )
if "tests/fixtures/quick_start.py" not in website_md:
    raise SystemExit(
        "docs/WEBSITE.md must reference tests/fixtures/quick_start.py"
    )
if "tests/fixtures/quick_start.expected.txt" not in website_md:
    raise SystemExit(
        "docs/WEBSITE.md must reference tests/fixtures/quick_start.expected.txt "
        "as the single source of truth for the documented stdout"
    )
PY

if grep -R -nE '(localhost|127\.0\.0\.1|file://)' "$site_dir"; then
  echo "Website contains a local-only URL" >&2
  exit 1
fi

echo "Website checks passed."
