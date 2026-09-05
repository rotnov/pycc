"""Offline evidence manifest validation, invoked only through check-site.sh."""

import hashlib
import json
import re
import subprocess
import sys
from html.parser import HTMLParser
from pathlib import Path, PurePosixPath
import site_execution_evidence


manifest_path = Path(sys.argv[1])
evidence_root = Path(sys.argv[2]).resolve()
repo_root = Path(sys.argv[3]).resolve()
site_dir = Path(sys.argv[4]).resolve()

SCHEMA_VERSION = "2.0.0"
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
        "page_path": "site/language-support/index.html",
        "owner": "https://github.com/rotnov/pycc/issues/565",
        "projections": {
            "html": "site/language-support/index.html",
            "structured_data": "site/language-support/index.html",
            "social": "site/language-support/index.html",
            "markdown": "site/index.html.md",
            "llm": "site/llms.txt",
        },
    },
    "diagnostics": {
        "evidence_id": "diagnostics-v1",
        "kind": "compiler-diagnostic",
        "route": "/diagnostics/",
        "page_path": "site/diagnostics/index.html",
        "owner": "https://github.com/rotnov/pycc/issues/565",
        "projections": {
            "html": "site/diagnostics/index.html",
            "structured_data": "site/diagnostics/index.html",
            "social": "site/diagnostics/index.html",
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
    "commit": "8324332d5ea713bd8a56f4d08bf7e0120757d66b",
    "run_id": 33198103510,
    "platforms": [
        {
            "runner": "macos-14",
            "architecture": "aarch64-apple-darwin",
            "job_url": "https://github.com/rotnov/pycc/actions/runs/33198103510/job/98940383105",
        },
        {
            "runner": "macos-15-intel",
            "architecture": "x86_64-apple-darwin",
            "job_url": "https://github.com/rotnov/pycc/actions/runs/33198103510/job/98940383070",
        },
        {
            "runner": "ubuntu-latest",
            "architecture": "x86_64-unknown-linux-gnu",
            "job_url": "https://github.com/rotnov/pycc/actions/runs/33198103510/job/98940382966",
        },
        {
            "runner": "ubuntu-24.04-arm",
            "architecture": "aarch64-unknown-linux-gnu",
            "job_url": "https://github.com/rotnov/pycc/actions/runs/33198103510/job/98940383014",
        },
        {
            "runner": "windows-latest",
            "architecture": "x86_64-pc-windows-msvc",
            "job_url": "https://github.com/rotnov/pycc/actions/runs/33198103510/job/98940382973",
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

    if page_id in site_execution_evidence.SPECS:
        site_execution_evidence.validate(hero, evidence_root, repo_root)
        continue

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
    if page_id in site_execution_evidence.SPECS:
        site_execution_evidence.validate_projection(hero, repo_root, site_dir)
    expected_tuple = (hero["evidence_id"], hero["kind"], hero["state"])
    html_projection = hero["projections"].get("html")
    if html_projection is not None:
        html_path = projection_file(html_projection, f"hero {page_id!r} HTML projection")
        html_text = html_path.read_text()
        navigation = site_execution_evidence.VisibleExecutionParser()
        navigation.feed(html_text)
        prefix = "" if page_id == "landing" else "../"
        expected_navigation = [
            prefix + item["route"].lstrip("/")
            for identity, item in PAGE_ALLOWLIST.items()
            if identity != "landing" and item["page_path"] is not None
        ] + ["https://github.com/rotnov/pycc"]
        if (navigation.primary_navs != 1 or len(navigation.navigation) != len(expected_navigation)
                or set(navigation.navigation) != set(expected_navigation)):
            fail(f"hero {page_id!r} primary navigation must visibly link every evidence route exactly once")
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
        if page_id in site_execution_evidence.SPECS and webpages[0].get("inLanguage") != "en-US":
            fail(f"hero {page_id!r} JSON-LD inLanguage must be en-US")
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
