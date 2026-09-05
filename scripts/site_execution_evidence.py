"""Reviewed ordered execution records; no compiler or provider is invoked here.

The source commit is deliberately an ancestor, not the publication commit.
Git's immutable blobs establish historical bytes; scope is checked separately
against the current breadth declaration. D-230 owns this incompatible v2 shape.
"""

import hashlib
import json
import re
import subprocess
from html.parser import HTMLParser


SOURCE = "0d94ad8f30b27131a5da381a034d55165558e56a"
TREE = "26bf9fe465c50d8065b1e0260e6100dc3e68f193"
TESTED = "321e66ff71f1eb4dedcd34d606f98994ad198758"
BASE = "4eca5e24e09d6972b5717f35652e5201dde2a02f"
RUN = 33969157527
REPO = "https://github.com/rotnov/pycc"
CURRENT_FUTURE_SCOPE = (
    "In the module prologue, from __future__ import annotations is accepted as a "
    "compile-time no-op and binds no feature name. String annotations and "
    "references to classes defined later remain unsupported. PEP 563 "
    "acceptance remains pending in #937."
)
RUN_URL = f"{REPO}/actions/runs/{RUN}"
PLATFORMS = [
    ("macos-14", "aarch64-apple-darwin", 101314505690),
    ("macos-15-intel", "x86_64-apple-darwin", 101314505701),
    ("ubuntu-latest", "x86_64-unknown-linux-gnu", 101314505789),
    ("ubuntu-24.04-arm", "aarch64-unknown-linux-gnu", 101314505756),
    ("windows-latest", "x86_64-pc-windows-msvc", 101314505712),
]
LIMITATIONS = {
    "language": "One passing fixture does not establish full Python 3.14 compatibility. all-Tier-1 means platform coverage for this fixture, not whole-language acceptance. The displayed pycc run uses debug, not release.",
    "diagnostics": "Human output has no help line. The type checker uses a placeholder 1:1, zero-length span; the caret does not precisely highlight the argument. Exact serialization for this fixture does not establish diagnostic-class correctness for all inputs.",
}
SCOPES = {
    "language": {
        "source": "tests/fixtures/conformance-breadth-manifest.json",
        "proven": [
            "annotated assignment with an initializer (`doubled: int = ...`)",
            "a bare annotation without an initializer (`total: int`)",
        ],
        "not_proven": [
            {"category": "parenthesized annotated targets (`(x): int = 1`)", "kind": "core"},
            {"category": "module- and class-level `__annotations__` recording", "kind": "core"},
            {"category": "annotated attribute and subscript targets (`obj.x: int`, `d['k']: int`)", "kind": "core"},
        ],
    },
    "diagnostics": {
        "source": "docs/DIAGNOSTICS.md",
        "proven": ["T0021 range argument rejection; exact human and JSON serialization"],
        "not_proven": [
            {"category": "precise type-checker spans and human help rendering", "kind": "core"},
            {"category": "diagnostic-class correctness for all inputs", "kind": "core"},
        ],
    },
}
SPECS = {
    "language": {
        "fixture": "tests/fixtures/pep_0526_var_annotations.py",
        "test": "language_commands_match_cpython_3_14_7_and_canonical_stdout",
        "artifacts": [("stdout", "tests/fixtures/pep_0526_var_annotations.expected.txt", "text")],
        "commands": [
            ("pycc", ["pycc", "run", "tests/fixtures/pep_0526_var_annotations.py"], "debug", [], 0, "stdout"),
            ("cpython", ["python3.14", "tests/fixtures/pep_0526_var_annotations.py"], "not-applicable", [], 0, "stdout"),
        ],
    },
    "diagnostics": {
        "fixture": "tests/diagnostics/d0021_range_argument_type.py",
        "test": "diagnostics_commands_match_human_and_json_snapshots",
        "artifacts": [
            ("human", "tests/diagnostics/d0021_range_argument_type.expected.txt", "text"),
            ("json", "tests/diagnostics/d0021_range_argument_type.expected.json", "json"),
        ],
        "commands": [
            ("human", ["pycc", "check", "tests/diagnostics/d0021_range_argument_type.py"], "frontend", [], 1, "human"),
            ("json", ["pycc", "check", "tests/diagnostics/d0021_range_argument_type.py", "--error-format", "json"], "frontend", ["--error-format", "json"], 1, "json"),
        ],
    },
}


def fail(message):
    raise SystemExit(f"execution evidence: {message}")


def blob(repo_root, path):
    result = subprocess.run(["git", "-C", str(repo_root), "show", f"{SOURCE}:{path}"], capture_output=True)
    if result.returncode:
        fail(f"missing preserved source blob: {path}")
    return result.stdout.replace(b"\r\n", b"\n")


def expected_record(page_id, repo_root):
    """Construct the closed reviewed shape from immutable source blobs only."""
    spec = SPECS[page_id]
    def artifact(path):
        return {"path": path, "sha256": hashlib.sha256(blob(repo_root, path)).hexdigest()}
    snapshots = []
    for identity, path, format_name in spec["artifacts"]:
        snapshots.append({"id": identity, **artifact(path), "text": blob(repo_root, path).decode(), "format": format_name})
    commands = [
        {"id": identity, "argv": argv, "profile": profile, "flags": flags,
         "exit_status": exit_status, "stdout": {"artifact": stdout}, "stderr": {"empty": True}}
        for identity, argv, profile, flags, exit_status, stdout in spec["commands"]
    ]
    links = {
        "fixture": f"{REPO}/blob/{SOURCE}/{spec['fixture']}",
        "test": f"{REPO}/blob/{SOURCE}/tests/site_evidence.rs",
        "commit": f"{REPO}/commit/{SOURCE}",
        "run": RUN_URL,
    }
    for snapshot in snapshots:
        links[snapshot["id"]] = f"{REPO}/blob/{SOURCE}/{snapshot['path']}"
    return {
        "fixture": {**artifact(spec["fixture"]), "scope": SCOPES[page_id]},
        "test": {**artifact("tests/site_evidence.rs"), "names": [spec["test"]]},
        "command": {"cwd": "repository-root", "executions": commands},
        "snapshot": {"artifacts": snapshots},
        "repository": {"commit": SOURCE, "tree": TREE, "url": links["commit"]},
        "attestation": {"workflow": "CI", "run_id": RUN, "run_url": RUN_URL,
                        "source_head": SOURCE, "tested_commit": TESTED, "tested_tree": TREE, "base_commit": BASE},
        "environment": {"python": "3.14.7", "rust": "1.97.1", "llvm": "22", "profile": "debug",
                        "platforms": [{"runner": runner, "architecture": target, "job_url": f"{RUN_URL}/job/{job}"}
                                      for runner, target, job in PLATFORMS]},
        "state": "all-Tier-1", "limitations": LIMITATIONS[page_id], "stable_links": links,
    }


def validate(hero, evidence_root, repo_root):
    page_id = hero["page_id"]
    expected = expected_record(page_id, repo_root)
    # JSON serialization retains bool versus int, unlike Python value equality.
    for field, value in expected.items():
        if json.dumps(hero[field], sort_keys=True) != json.dumps(value, sort_keys=True):
            fail(f"{page_id} {field} differs from the reviewed execution record")
    tree = subprocess.run(["git", "-C", str(repo_root), "rev-parse", f"{SOURCE}^{{tree}}"], capture_output=True, text=True)
    if tree.returncode or tree.stdout.strip() != TREE:
        fail(f"{page_id} source tree differs from the attested tested tree")
    artifacts = [hero["fixture"], hero["test"], *hero["snapshot"]["artifacts"]]
    for artifact in artifacts:
        path = evidence_root / artifact["path"]
        if path.is_symlink() or not path.is_file() or not path.resolve().is_relative_to(evidence_root):
            fail(f"{page_id} artifact missing or unsafe: {artifact['path']}")
        if path.read_bytes().replace(b"\r\n", b"\n") != blob(repo_root, artifact["path"]):
            fail(f"{page_id} artifact differs from preserved source blob: {artifact['path']}")
    test = (evidence_root / hero["test"]["path"]).read_text()
    for name in hero["test"]["names"]:
        if not re.search(r"#\[test\]\s*(?:#\[[^\n]*\]\s*)*fn\s+" + re.escape(name) + r"\s*\(", test):
            fail(f"{page_id} test is not registered: {name}")
    if page_id == "language":
        breadth = json.loads((evidence_root / SCOPES[page_id]["source"]).read_text())
        rows = [row for row in breadth["rows"] if row["fixtures"] == ["pep_0526_var_annotations.py"]]
        if len(rows) != 1:
            fail("language current breadth row is missing or ambiguous")
        row = rows[0]
        if ([item["category"] for item in row["proven"]] != SCOPES[page_id]["proven"] or
            [{key: item[key] for key in ("category", "kind")} for item in row["not_proven"]] != SCOPES[page_id]["not_proven"]):
            fail("language current breadth scope drifted; review public scope before accepting")
    else:
        data = json.loads(hero["snapshot"]["artifacts"][1]["text"])
        if (data["code"], data["message"], data["help"], data["spans"][0]["line"], data["spans"][0]["col"], data["spans"][0]["len"]) != (
            "T0021", "range stop expects `int`, got `str`", ["pass an `int` value"], 1, 1, 0
        ):
            fail("diagnostics structured fields differ from the occurrence")


def code_units(hero, repo_root):
    """One ordered transcript projection for either evidence kind."""
    units = [("source", blob(repo_root, hero["fixture"]["path"]).decode().removesuffix("\n"))]
    snapshots = {item["id"]: item for item in hero["snapshot"]["artifacts"]}
    for execution in hero["command"]["executions"]:
        identity = execution["id"]
        units.append((f"{identity}-command", " ".join(execution["argv"])))
        for stream in ("stdout", "stderr"):
            output = execution[stream]
            text = "" if output.get("empty") else snapshots[output["artifact"]]["text"].removesuffix("\n")
            units.append((f"{identity}-{stream}", text))
        units.append((f"{identity}-exit", str(execution["exit_status"])))
    return units


def summary(hero):
    """Compact central projection; complete artifacts stay on the linked page."""
    page_id = hero["page_id"]
    executions = "; ".join(f"`{' '.join(item['argv'])}` (exit {item['exit_status']}, empty stderr)"
                           for item in hero["command"]["executions"])
    result = "Both stdout streams: `15\\n`." if page_id == "language" else "Human stdout: T0021; JSON stdout includes ``\"help\":[\"pass an `int` value\"]``."
    return f"{hero['evidence_id']} — all-Tier-1: {executions}. {result} {hero['limitations']} [Exact source, snapshots, SHA-256 identities, toolchain and five jobs](https://rotnov.github.io/pycc{hero['route']})."


class VisibleExecutionParser(HTMLParser):
    """Parse visible hero code and primary-navigation links independently."""
    VOID = {"area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source", "track", "wbr"}
    def __init__(self):
        super().__init__()
        self.stack = []
        self.units = []
        self.hero_count = 0
        self.hero_text = []
        self.visible_text = []
        self.links = []
        self.primary_navs = 0
        self.navigation = []
        self.h1s = []
        self.h1_count = 0
        self.language = None
        self.locales = []
    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if tag == "html":
            self.language = attrs.get("lang")
        if tag == "meta" and attrs.get("property") == "og:locale":
            self.locales.append(attrs.get("content"))
        hidden = (self.stack and self.stack[-1][1]) or tag in {"head", "script", "style", "template", "noscript"} or "hidden" in attrs or attrs.get("aria-hidden") == "true" or bool(re.search(r"display\s*:\s*none|visibility\s*:\s*hidden", attrs.get("style", "")))
        in_hero = bool(self.stack and self.stack[-1][2]) or attrs.get("data-evidence-role") == "hero"
        starts_nav = tag == "nav" and "site-nav" in attrs.get("class", "").split()
        in_nav = bool(self.stack and self.stack[-1][4]) or starts_nav
        if starts_nav and not hidden:
            self.primary_navs += 1
        if tag == "h1":
            self.h1_count += 1
            if not hidden:
                self.h1s.append("")
        if attrs.get("data-evidence-role") == "hero" and not hidden:
            self.hero_count += 1
        unit = self.stack[-1][3] if self.stack else None
        if tag == "code" and attrs.get("data-execution") and not hidden and in_hero:
            self.units.append([attrs["data-execution"], ""])
            unit = len(self.units) - 1
        if tag == "a" and not hidden and in_hero:
            self.links.append(attrs.get("href"))
        if tag == "a" and not hidden and in_nav:
            self.navigation.append(attrs.get("href"))
        if tag not in self.VOID:
            self.stack.append((tag, hidden, in_hero, unit, in_nav))
    def handle_endtag(self, tag):
        for index in range(len(self.stack) - 1, -1, -1):
            if self.stack[index][0] == tag:
                del self.stack[index:]
                break
    def handle_data(self, text):
        if self.stack and not self.stack[-1][1]:
            if any(item[0] == "h1" for item in self.stack):
                self.h1s[-1] += text
            self.visible_text.append(text)
            if self.stack[-1][2]:
                self.hero_text.append(text)
                if self.stack[-1][3] is not None:
                    self.units[self.stack[-1][3]][1] += text


def validate_projection(hero, repo_root, site_dir):
    css = re.sub(r"/\*.*?\*/", "", (site_dir / "styles.css").read_text(), flags=re.S)
    provenance = re.search(r"\.hero-provenance\s*\{([^}]+)\}", css)
    mobile = css.split("@media (max-width: 980px)", 1)[-1]
    navigation = re.search(r"\.site-nav\s*\{([^}]+)\}", mobile)
    if (not provenance or not re.search(r"overflow-wrap\s*:\s*anywhere\s*;", provenance[1]) or
        not navigation or not re.search(r"flex-wrap\s*:\s*wrap\s*;", navigation[1]) or
        re.search(r"\.site-nav a:not\([^}]+display\s*:\s*none", mobile)):
        fail("mobile evidence navigation and provenance wrapping are required")
    parser = VisibleExecutionParser()
    parser.feed((site_dir / hero["page_path"].removeprefix("site/")).read_text())
    expected_h1 = {"language": "Python language support, with evidence.",
                   "diagnostics": "Compiler diagnostics, exactly as emitted."}[hero["page_id"]]
    if parser.h1_count != 1 or [" ".join(value.split()) for value in parser.h1s] != [expected_h1]:
        fail(f"{hero['page_id']} must have exactly one meaningful visible H1")
    if parser.language != "en-US" or parser.locales != ["en_US"]:
        fail(f"{hero['page_id']} locale must be en-US / en_US")
    body = " ".join("".join(parser.visible_text).split())
    if re.search(r"fully supports Python|full Python 3\.14 compatibility is (?:proven|guaranteed)|supports all (?:Python|of PEP)|production.ready|int\([\"']three[\"']\)", body, re.I):
        fail(f"{hero['page_id']} contains an unsupported claim or repair")
    required_scope = {
        "language": ["A curated, non-exhaustive support map", "Implemented: named tested subsets",
                     "Partial: core gaps remain", "Experimental", "Not yet supported",
                     "PEP 526 remains a subset row (◐), not whole-PEP acceptance (D-177).",
                     "parenthesized annotated targets, module- and class-level __annotations__ recording, or annotated attribute and subscript targets",
                     "Permanent non-goals are different from missing implementation"],
        "diagnostics": ['"help":["pass an `int` value"]', "--fix is not implemented",
                        "pycc explain T0021", "not another occurred diagnostic"],
    }
    for required in required_scope[hero["page_id"]]:
        if required not in body:
            fail(f"{hero['page_id']} current scope is missing: {required}")
    if parser.hero_count != 1 or parser.units != [list(unit) for unit in code_units(hero, repo_root)]:
        fail(f"{hero['page_id']} visible ordered source/command/output/exit transcript drifted")
    visible = " ".join("".join(parser.hero_text).split())
    literals = [hero["limitations"], SOURCE, TREE, TESTED, BASE, str(RUN), "3.14.7", "1.97.1", "LLVM 22", "repository-root", "no extra compiler flags"]
    for artifact in [hero["fixture"], hero["test"], *hero["snapshot"]["artifacts"]]:
        literals.extend([artifact["path"], artifact["sha256"]])
    literals.extend(hero["test"]["names"])
    for runner, target, _ in PLATFORMS:
        literals.extend([runner, target])
    for literal in literals:
        if " ".join(literal.split()) not in visible:
            fail(f"{hero['page_id']} visible provenance/limitation missing: {literal}")
    links = list(hero["stable_links"].values()) + [item["job_url"] for item in hero["environment"]["platforms"]]
    if not set(links) <= set(parser.links):
        fail(f"{hero['page_id']} visible immutable source/job links missing")
    for surface in ("markdown", "llm"):
        text = (site_dir / hero["projections"][surface].removeprefix("site/")).read_text()
        if text.count(summary(hero)) != 1:
            fail(f"{hero['page_id']} {surface} execution summary/limitations drifted")
