"""Architecture hero contract: a checked-in, re-derivable compiler pipeline trace.

The ``architecture`` record in ``site/evidence-heroes.json`` projects one
production fixture, ``tests/fixtures/quick_start.py``, through every implemented
compiler stage (D-243, Part 2 of #566).  This module owns the record's closed
shape, the stage vocabulary, the derived state, and the visible-projection
rules.  Nothing here contacts the network and nothing here runs the compiler:
``tests/architecture_trace.rs`` is the only party that re-derives the artifacts,
and it is checked in, so this module can validate identities offline against the
same bytes.

Division of labour with the other evidence modules:

* ``tests/architecture_trace.rs`` owns re-derivation (the artifacts are what
  today's compiler produces, the native build exits 0, the stdout is exact);
* this module owns the record shape, the SHA-256 identities and the page;
* ``tests/site_evidence.rs`` owns the per-hero manifest facts and never re-runs
  the pipeline.

Bootstrap note (the same rule Part 1 followed): the artifacts are read from the
working tree's ``evidence_root``, never through ``git show`` at a pinned commit.
A pin cannot name the commit that carries it, so no ``SOURCE``-style constant
pointing at an unborn commit is introduced here.  The commit this record *does*
pin, ``repository.commit``, is the already-published revision whose compiler
produced the bytes.
"""

import datetime
import hashlib
import json
import re
import subprocess
from pathlib import Path

import site_execution_evidence


REPO = "https://github.com/rotnov/pycc"
KIND = "compiler-pipeline-trace"
FIXTURE = "tests/fixtures/quick_start.py"
TEST = "tests/architecture_trace.rs"
TRACE = "tests/fixtures/architecture-trace/trace.json"
PARSER_ARTIFACT = "tests/fixtures/architecture-trace/parser-ast.txt"
HIR_ARTIFACT = "tests/fixtures/architecture-trace/hir-module.txt"
MIR_ARTIFACT = "tests/fixtures/architecture-trace/mir-items.txt"
# Kept in sync with ``EXCERPT_LINES`` in tests/architecture_trace.rs, which
# asserts that the same leading lines really are a prefix of each artifact.
EXCERPT_LINES = 18
ARGV = ["cargo", "test", "--test", "architecture_trace"]
REQUIRES = "a host LLVM 22 toolchain and linker; no network"
COLLECTION_METHOD = "cargo test --test architecture_trace (offline, host toolchain)"
STDOUT_TEXT = "0\n1\n1\n2\n3\n5\n8\n13\n21\n34\n55\n"
STDOUT_BYTES = 26

# The closed, ordered stage vocabulary.  ``evidence`` says what kind of proof a
# stage carries, and it is what makes the hero honestly ``partial`` rather than
# ``all-Tier-1``: exactly one stage carries ``none``.
#   artifact     -- a checked-in byte stream with its own SHA-256
#   identity     -- no artifact of its own; the stage returns the previous
#                   stage's bytes unchanged, and shares its SHA-256
#   exit-status  -- proved by a command's exit status, not by bytes
#   inline       -- the bytes are short enough to publish in full
#   none         -- no evidence exists; the stage is unavailable
EVIDENCE_KINDS = ("artifact", "identity", "exit-status", "inline", "none")
STAGES = [
    ("source", "01 Source", "tests/fixtures/quick_start.py", "artifact", FIXTURE),
    ("parser", "02 Parser", "pycc_parser::parse_all", "artifact", PARSER_ARTIFACT),
    ("hir", "03 HIR", "pycc_hir::lower_module, link, finalize", "artifact", HIR_ARTIFACT),
    ("type-check", "04 Type checking", "pycc_types::check_and_resolve_all_keyed", "identity", HIR_ARTIFACT),
    ("mir", "05 MIR", "pycc_mir::build", "artifact", MIR_ARTIFACT),
    ("llvm-ir", "06 LLVM IR", "pycc_codegen", "none", None),
    ("native", "07 Native executable", "pycc build", "exit-status", None),
    ("stdout", "08 Program stdout", "the linked executable", "inline", None),
]
# The stages whose bytes the page excerpts.  The source and stdout stages are
# short enough to publish whole, so they are not excerpted.
EXCERPTED = ("parser", "hir", "mir")
NOTES = {
    "source": "The one production fixture every later stage is derived from.",
    "parser": "Debug rendering of the returned ModModule.",
    "hir": "Debug rendering of the linked and finalized HirModule.",
    "type-check": "Accepted without rewriting a node: the returned HIR is byte-identical to stage 03, so the two stages share one SHA-256.",
    "mir": "Debug rendering of each item of the public mir.items field; MirModule itself derives no Debug.",
    "llvm-ir": "No artifact: pycc has no --emit flag, so LLVM IR cannot be captured through the public CLI. This stage is unavailable, which is why the hero is partial.",
    "native": "pycc build tests/fixtures/quick_start.py -o <scratch>/quick_start exits 0. No SHA-256: a linked binary is not reproducible across hosts.",
    "stdout": "26 bytes, the first eleven Fibonacci numbers one per line, exit 0 and empty stderr.",
}
TIER1 = [
    ("macos-14", "aarch64-apple-darwin"),
    ("macos-15-intel", "x86_64-apple-darwin"),
    ("ubuntu-latest", "x86_64-unknown-linux-gnu"),
    ("ubuntu-24.04-arm", "aarch64-unknown-linux-gnu"),
    ("windows-latest", "x86_64-pc-windows-msvc"),
]
CAPTURE_HOST = "macos-26.5.1 / aarch64-apple-darwin"
LIMITATIONS = (
    "One fixture is not the language. The LLVM IR stage carries no artifact at "
    "all, because pycc has no --emit flag, so this trace is partial and never "
    "all-Tier-1. The parser, HIR and MIR artifacts are Rust Debug renderings: a "
    "developer-facing format with no stability guarantee, so their SHA-256 "
    "identities change whenever those types change. The native build and its "
    "stdout were observed on one host; tests/architecture_trace.rs re-derives "
    "every stage wherever cargo test --workspace runs."
)
SHA_RE = re.compile(r"^[0-9a-f]{40}$")
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
TIME_RE = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$")


def fail(message):
    raise SystemExit(f"evidence-heroes.json: {message}")


def utc_now():
    """The current instant in the record's own RFC 3339 UTC shape."""
    return datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def is_utc_instant(value):
    """True when ``value`` is a real RFC 3339 UTC instant in the ``Z`` form."""
    if not isinstance(value, str) or not TIME_RE.match(value):
        return False
    try:
        datetime.datetime.strptime(value, "%Y-%m-%dT%H:%M:%SZ")
    except ValueError:
        return False
    return True


def require_exact_fields(value, expected, context):
    if not isinstance(value, dict):
        fail(f"{context} must be an object")
    actual = set(value)
    if actual != set(expected):
        missing = sorted(set(expected) - actual)
        extra = sorted(actual - set(expected))
        fail(f"{context} fields drifted; missing={missing}, extra={extra}")


def expected_shape():
    """Closed field sets for every nested object of a non-unavailable record."""
    return {
        "fixture": {"path", "sha256", "bytes"},
        "test": {"path", "sha256", "names"},
        "command": {"cwd", "argv", "requires"},
        "snapshot": {"trace", "stages"},
        "trace": {"path", "sha256", "bytes"},
        "stage": {"id", "label", "api", "evidence", "path", "sha256", "bytes", "note"},
        "repository": {"commit", "tree", "url"},
        "attestation": {"collected_at", "collection_method", "sanitized",
                        "compiler_commit", "verification_test"},
        "environment": {"rust", "llvm", "profile", "capture_host", "platforms"},
        "platform": {"runner", "architecture"},
    }


def canonical_sha256(data):
    return hashlib.sha256(data.replace(b"\r\n", b"\n")).hexdigest()


def canonical_bytes(data):
    return len(data.replace(b"\r\n", b"\n"))


def read_artifact(evidence_root, path, context):
    """Read one checked-in artifact from the working tree, refusing anything unsafe."""
    resolved = Path(evidence_root) / path
    if resolved.is_symlink() or not resolved.is_file() or not resolved.resolve().is_relative_to(Path(evidence_root)):
        fail(f"{context} file is missing or unsafe: {path}")
    return resolved.read_bytes().replace(b"\r\n", b"\n")


def derive_state(hero):
    """Closed mapping: ``partial`` while any stage carries no evidence at all.

    Every stage evidenced would be ``all-Tier-1``; a broken or absent stage list
    is ``unavailable``.  There is deliberately no path from this record to
    ``all-Tier-1`` while stage 06 exists: the LLVM stage cannot be evidenced
    without a compiler feature this part does not implement.
    """
    snapshot = hero.get("snapshot")
    stages = snapshot.get("stages") if isinstance(snapshot, dict) else None
    if not isinstance(stages, list) or len(stages) != len(STAGES):
        return "unavailable"
    kinds = [item.get("evidence") if isinstance(item, dict) else None for item in stages]
    if any(kind not in EVIDENCE_KINDS for kind in kinds):
        return "unavailable"
    if kinds.count("none") == 0:
        return "all-Tier-1"
    return "partial"


def expected_links(hero):
    commit = hero["repository"]["commit"]
    return {
        "commit": f"{REPO}/commit/{commit}",
        "tree": f"{REPO}/tree/{commit}",
        "fixture": f"{REPO}/blob/{commit}/{FIXTURE}",
        "owner": f"{REPO}/issues/566",
        "part": f"{REPO}/issues/1007",
    }


def excerpt_text(evidence_root, path):
    """The exact leading bytes of one artifact that the page may publish."""
    text = read_artifact(evidence_root, path, "architecture excerpt").decode()
    lines = text.split("\n")[:EXCERPT_LINES]
    if len(lines) != EXCERPT_LINES:
        fail(f"architecture artifact is shorter than its excerpt: {path}")
    return "\n".join(lines) + "\n"


def validate(hero, evidence_root, repo_root):
    """Structural validation plus record-internal invariants (offline)."""
    shape = expected_shape()
    evidence_root = Path(evidence_root).resolve()
    repo_root = Path(repo_root)
    page_id = hero["page_id"]
    if page_id != "architecture" or hero["kind"] != KIND:
        fail("pipeline trace validation applies only to the architecture hero")
    for field in ("fixture", "test", "command", "snapshot", "repository", "attestation", "environment"):
        require_exact_fields(hero[field], shape[field], f"architecture {field}")

    commit = hero["repository"]["commit"]
    tree = hero["repository"]["tree"]
    for name, value in (("commit", commit), ("tree", tree)):
        if not isinstance(value, str) or not SHA_RE.match(value):
            fail(f"architecture repository {name} must be a full lowercase SHA")
    if hero["repository"]["url"] != f"{REPO}/commit/{commit}":
        fail("architecture repository url must be the immutable commit URL")

    for field, expected_path in (("fixture", FIXTURE), ("test", TEST)):
        if hero[field]["path"] != expected_path:
            fail(f"architecture {field} path must be {expected_path}")
    fixture_bytes = read_artifact(evidence_root, FIXTURE, "architecture fixture")
    if hero["fixture"]["sha256"] != canonical_sha256(fixture_bytes) or hero["fixture"]["bytes"] != len(fixture_bytes):
        fail(f"architecture fixture sha256/bytes differ from {FIXTURE}")
    test_bytes = read_artifact(evidence_root, TEST, "architecture test")
    if hero["test"]["sha256"] != canonical_sha256(test_bytes):
        fail(f"architecture test sha256 differs from {TEST}")
    names = hero["test"]["names"]
    registered = re.findall(r"#\[test\]\nfn (\w+)\(\)", test_bytes.decode())
    if not isinstance(names, list) or not names or names != registered:
        fail("architecture test names must list every registered #[test] in source order")

    command = hero["command"]
    if command["cwd"] != "repository-root" or command["requires"] != REQUIRES or command["argv"] != ARGV:
        fail("architecture command drifted from the reviewed verification command")

    trace_bytes = read_artifact(evidence_root, TRACE, "architecture trace record")
    require_exact_fields(hero["snapshot"]["trace"], shape["trace"], "architecture snapshot trace")
    if hero["snapshot"]["trace"]["path"] != TRACE:
        fail(f"architecture snapshot trace path must be {TRACE}")
    if (hero["snapshot"]["trace"]["sha256"] != canonical_sha256(trace_bytes)
            or hero["snapshot"]["trace"]["bytes"] != len(trace_bytes)):
        fail(f"architecture snapshot trace sha256/bytes differ from {TRACE}")
    trace = json.loads(trace_bytes)
    if trace.get("compiler_commit") != commit or trace.get("compiler_tree") != tree:
        fail("architecture trace record names a different compiler revision than the hero record")
    if trace.get("excerpt_lines") != EXCERPT_LINES:
        fail(f"architecture trace record excerpt_lines must be {EXCERPT_LINES}")
    if trace.get("stdout", {}).get("text") != STDOUT_TEXT or trace.get("stdout", {}).get("bytes") != STDOUT_BYTES:
        fail("architecture trace record stdout drifted from the published transcript")
    if trace.get("native", {}).get("exit_status") != 0:
        fail("architecture trace record native exit status must be 0")

    stages = hero["snapshot"]["stages"]
    if not isinstance(stages, list) or len(stages) != len(STAGES):
        fail(f"architecture snapshot stages must be the {len(STAGES)} ordered pipeline stages")
    # Ahead of the closed-list comparison below, so an overstated LLVM IR stage
    # is rejected with the reason it is unevidenced rather than with the generic
    # drift message. `pycc` has no `--emit` flag, so the stage has no artifact.
    for item in stages:
        if isinstance(item, dict) and item.get("id") == "llvm-ir" and item.get("evidence") != "none":
            fail("architecture llvm-ir stage may not claim evidence; pycc has no --emit flag")
    for (identity, label, api, evidence, path), item in zip(STAGES, stages):
        require_exact_fields(item, shape["stage"], f"architecture stage {identity}")
        if (item["id"], item["label"], item["api"], item["evidence"]) != (identity, label, api, evidence):
            fail(f"architecture stage {identity} drifted from the closed reviewed stage list")
        if item["note"] != NOTES[identity]:
            fail(f"architecture stage {identity} note drifted from the reviewed text")
        if path is None:
            if (item["path"], item["sha256"], item["bytes"]) != (None, None, None):
                fail(f"architecture stage {identity} carries no artifact and must record null path/sha256/bytes")
            continue
        if item["path"] != path:
            fail(f"architecture stage {identity} path must be {path}")
        if not SHA256_RE.match(item["sha256"] or ""):
            fail(f"architecture stage {identity} sha256 must be a lowercase SHA-256")
        data = read_artifact(evidence_root, path, f"architecture stage {identity}")
        if item["sha256"] != canonical_sha256(data) or item["bytes"] != len(data):
            fail(f"architecture stage {identity} sha256/bytes differ from {path}")
        recorded = trace.get("artifacts", {}).get(identity)
        if not isinstance(recorded, dict) or (recorded.get("path"), recorded.get("sha256"), recorded.get("bytes")) != (
                item["path"], item["sha256"], item["bytes"]):
            fail(f"architecture stage {identity} differs from the checked-in trace record")
    by_id = {item["id"]: item for item in stages}
    if by_id["type-check"]["sha256"] != by_id["hir"]["sha256"]:
        fail("architecture type-check stage is an identity claim and must share the HIR stage's SHA-256")

    attestation = hero["attestation"]
    if not is_utc_instant(attestation["collected_at"]):
        fail("architecture attestation collected_at must be an RFC 3339 UTC timestamp")
    if attestation["collected_at"] > utc_now():
        fail("architecture attestation collected_at must not be later than the validation time")
    if attestation["collection_method"] != COLLECTION_METHOD or attestation["sanitized"] is not True:
        fail("architecture attestation collection_method/sanitized drifted")
    if attestation["compiler_commit"] != commit or attestation["verification_test"] != TEST:
        fail("architecture attestation must name the recorded compiler commit and the verification test")

    environment = hero["environment"]
    if (environment["rust"], environment["llvm"], environment["profile"], environment["capture_host"]) != (
            "1.97.1", "22", "debug", CAPTURE_HOST):
        fail("architecture environment drifted from the reviewed toolchain and capture host")
    rows = environment["platforms"]
    if not isinstance(rows, list) or len(rows) != len(TIER1):
        fail("architecture environment platforms must list the five Tier-1 targets")
    for (runner, architecture), row in zip(TIER1, rows):
        require_exact_fields(row, shape["platform"], f"architecture platform {runner}")
        if (row["runner"], row["architecture"]) != (runner, architecture):
            fail(f"architecture platform row drifted from the closed Tier-1 list: {row!r}")

    if hero["state"] != derive_state(hero) or hero["state"] != "partial":
        fail(f"architecture state {hero['state']!r} does not match the derived state {derive_state(hero)!r}")
    if hero["limitations"] != LIMITATIONS:
        fail("architecture limitations drifted from the reviewed text")
    if hero["stable_links"] != expected_links(hero):
        fail("architecture stable_links must be exactly the immutable commit, tree, fixture and issue links")
    verify_git(hero, repo_root)


def git(repo_root, *args):
    return subprocess.run(["git", "-C", str(repo_root), *args], capture_output=True, text=True)


def verify_git(hero, repo_root):
    """Prove the pinned compiler revision from Git objects a full checkout has."""
    commit = hero["repository"]["commit"]
    tree = git(repo_root, "rev-parse", f"{commit}^{{tree}}")
    if tree.returncode or tree.stdout.strip() != hero["repository"]["tree"]:
        fail(f"architecture compiler revision {commit} tree differs from the recorded tree")


def summary(hero):
    """Compact central projection; the full stage table stays on the linked page."""
    stages = {item["id"]: item for item in hero["snapshot"]["stages"]}
    return (
        f"{hero['evidence_id']} — {hero['state']}: {FIXTURE} traced through "
        f"parser, HIR, type checking, MIR and a native build to a {STDOUT_BYTES}-byte stdout; "
        f"LLVM IR unavailable. Re-derived by {TEST} "
        f"(parser {stages['parser']['sha256']}, HIR {stages['hir']['sha256']}, MIR {stages['mir']['sha256']}). "
        f"{hero['limitations']} [Exact stage artifacts, SHA-256 identities and excerpts]"
        f"(https://rotnov.github.io/pycc{hero['route']})."
    )


ROW_TAGS = {"dt", "dd", "li"}
HIDING_RULE = site_execution_evidence.HIDING_DECLARATION
BLOCK_TAGS = ROW_TAGS | {"p", "h1", "h2", "h3", "h4", "h5", "h6", "span", "summary"}
HERO_EYEBROW = "Architecture · Updated {date}"
PAGE_DATE = re.compile(r'"dateModified"\s*:\s*"(\d{4}-\d{2}-\d{2})"')
HERO_MASTHEAD = (
    "Python source to native code.",
    "pycc is a pre-alpha Rust compiler pipeline with an LLVM backend. The hero below is not a "
    "diagram: it is one production fixture carried through every implemented stage, with the "
    "bytes each stage produced checked into the repository and re-derived by a test.",
    "Frontend v0.1 checked",
    "Backend LLVM 22 · v0.1 implemented",
    "Output native executable",
)
HERO_DETAILS_TOGGLE = "Stage artifacts, SHA-256 identities and excerpts"
CSS_RULE = re.compile(r"([^{}]+)\{([^{}]*)\}")
GENERATED_CONTENT = re.compile(r"(?<![\w-])content\s*:\s*(?=\S)(?!(?:none|normal|\"\"|'')?\s*(?:;|!|$))", re.I)
COMBINATOR = re.compile(r"\s*[>+~]\s*|\s+")
PSEUDO = re.compile(r"::?[\w-]+(?:\([^)]*\))?")
# The two selector helpers are identical to the status hero's; import them
# rather than restate them, so one reviewed implementation covers both pages.
from site_status_evidence import compound_hooks, selector_hooks  # noqa: E402


class StageRowParser(site_execution_evidence.VisibleExecutionParser):
    """Keep every visible ``<dt>``/``<dd>``/``<li>`` row inside the hero, plus its links.

    Identical in shape to the status hero's ``ProofRowParser``: the flattened
    hero text proves a literal is visible somewhere, and the rows prove that a
    stage's path, SHA-256 and byte count sit in the same row as its own label,
    so two stages' evidence cannot be swapped without the gate noticing.
    """
    def reject(self, message):
        fail(f"architecture page {message}")

    def __init__(self):
        super().__init__()
        self.rows = []
        self.row_depth = None
        self.ancestry = []
        self.hero_hooks = set()
        self.ancestor_hooks = set()
        self.prose = []
        self.block_depth = None
        self.embedded_css = []
        self.stylesheets = []

    def handle_starttag(self, tag, attrs):
        links_before = len(self.links)
        starts_hero = dict(attrs).get("data-evidence-role") == "hero"
        if tag == "style":
            self.embedded_css.append("")
        if tag == "link" and "stylesheet" in dict(attrs).get("rel", "").lower().split():
            self.stylesheets.append(dict(attrs).get("href", ""))
        if starts_hero:
            self.ancestor_hooks.update(*self.ancestry, set())
        super().handle_starttag(tag, attrs)
        entry = self.stack[-1] if self.stack and tag not in self.VOID else None
        in_hero = starts_hero or bool(entry and entry[2]) or bool(self.stack and self.stack[-1][2] and tag in self.VOID)
        if in_hero:
            self.hero_hooks.update(selector_hooks(tag, dict(attrs)))
        if entry is not None:
            self.ancestry.append(selector_hooks(tag, dict(attrs)))
        is_summary = not starts_hero and "data-evidence-id" in dict(attrs)
        if (tag in ROW_TAGS or is_summary) and self.row_depth is None and entry is not None and entry[2] and not entry[1]:
            self.rows.append(["summary" if is_summary else tag, "", []])
            self.row_depth = len(self.stack)
        if self.row_depth is not None:
            self.rows[-1][2].extend(self.links[links_before:])
        elif tag in BLOCK_TAGS and self.block_depth is None and entry is not None and entry[2] and not entry[1]:
            self.prose.append("")
            self.block_depth = len(self.stack)

    def handle_endtag(self, tag):
        super().handle_endtag(tag)
        del self.ancestry[len(self.stack):]
        if self.row_depth is not None and len(self.stack) < self.row_depth:
            self.row_depth = None
        if self.block_depth is not None and len(self.stack) < self.block_depth:
            self.block_depth = None

    def handle_data(self, text):
        super().handle_data(text)
        if self.stack and self.stack[-1][0] == "style":
            self.embedded_css[-1] += text
        if not (self.stack and self.stack[-1][2] and not self.stack[-1][1]):
            return
        if self.row_depth is not None:
            self.rows[-1][1] += text
        elif self.block_depth is not None:
            self.prose[-1] += text
        elif text.strip():
            self.prose.append(text)


def hiding_rules(css, parser):
    """Stylesheet rules that hide the hero or add text to it (see the status hero's twin)."""
    found = []
    if site_execution_evidence.unterminated_css(css):
        found.append("unterminated CSS comment or string")
    css = site_execution_evidence.plain_css(css)
    reachable = parser.hero_hooks | parser.ancestor_hooks
    if re.search(r"@import\b", css, re.I):
        found.append("@import")
    for selectors, body in CSS_RULE.findall(css):
        if not (HIDING_RULE.search(body) or GENERATED_CONTENT.search(body)):
            continue
        for selector in selectors.split(","):
            selector = selector.strip()
            if not selector or selector.startswith("@"):
                continue
            compounds = [compound_hooks(part) for part in COMBINATOR.split(selector) if part]
            if not compounds or any(hooks is None for hooks in compounds):
                found.append(selector)
                continue
            if all(hooks <= reachable for hooks in compounds):
                found.append(selector)
    return found


def stage_rows(parser):
    """Bind each ``<dd>`` to the ``<dt>`` before it; list the ``<li>`` rows and the collapsed summaries."""
    labelled, platforms, summaries, label = {}, [], [], None
    for tag, text, links in parser.rows:
        text = " ".join(text.split())
        if tag == "summary":
            summaries.append((text, links))
        elif tag == "dt":
            if label is not None:
                fail("architecture stage rows must pair one visible label with one row each")
            label = text
        elif tag == "dd":
            if label is None or label in labelled:
                fail("architecture stage rows must pair one visible label with one row each")
            labelled[label] = (text, links)
            label = None
        else:
            platforms.append((text, links))
    if label is not None:
        fail("architecture stage rows must pair one visible label with one row each")
    return labelled, platforms, summaries


def stage_row_text(stage):
    """The exact visible text of one stage's row, from that stage's own fields."""
    if stage["evidence"] in ("artifact", "identity"):
        head = f"{stage['path']} · sha256 {stage['sha256']} · {stage['bytes']} bytes"
    elif stage["evidence"] == "none":
        head = "unavailable · no artifact"
    elif stage["evidence"] == "exit-status":
        head = "exit 0 · no artifact"
    else:
        head = f"{STDOUT_BYTES} bytes · published in full"
    return f"{head} · {stage['api']} · {stage['note']}"


def expected_rows(hero, evidence_root):
    """The exact visible text and links of every hero row, keyed by its label."""
    links = hero["stable_links"]
    rows = {}
    for stage in hero["snapshot"]["stages"]:
        rows[stage["label"]] = (stage_row_text(stage), [links["fixture"]] if stage["id"] == "source" else [])
    for stage in hero["snapshot"]["stages"]:
        if stage["id"] in EXCERPTED:
            rows[f"{stage['label']} · first {EXCERPT_LINES} lines"] = (
                " ".join(excerpt_text(evidence_root, stage["path"]).split()), [])
    rows["Compiler revision"] = (
        f"{hero['repository']['commit']} · tree {hero['repository']['tree']} · "
        f"re-derived by {TEST} · {' '.join(hero['command']['argv'])}",
        [links["commit"], links["tree"]])
    rows["Scope owner"] = ("#566 (parent) · #1007 (this part)", [links["owner"], links["part"]])
    rows["Verification targets"] = ("cargo test --workspace runs the trace on every Tier-1 target:", [])
    return rows


def expected_summary_line(hero):
    """The exact visible text of the collapsed hero summary."""
    return (f"Evidence hero {hero['state']} · {FIXTURE} traced through "
            f"{sum(1 for item in hero['snapshot']['stages'] if item['evidence'] != 'none')} of "
            f"{len(STAGES)} stages · LLVM IR unavailable · "
            f"compiler {hero['repository']['commit'][:8]} · captured {hero['attestation']['collected_at']}")


def expected_closing_paragraph(hero):
    """The exact visible text of the paragraph closing the hero's ``<details>``."""
    environment = hero["environment"]
    return (f"Captured {hero['attestation']['collected_at']} by {COLLECTION_METHOD} on "
            f"{environment['capture_host']} with Rust {environment['rust']}, LLVM {environment['llvm']}, "
            f"profile {environment['profile']}; {hero['command']['requires']}. {hero['limitations']}")


def check_prose_blocks(hero, prose, page_date):
    """Every visible hero text block outside the rows is one reviewed block, rendered once."""
    allowed = [HERO_EYEBROW.format(date=page_date), *HERO_MASTHEAD, HERO_DETAILS_TOGGLE,
               expected_closing_paragraph(hero)]
    seen = []
    for block in prose:
        text = " ".join(block.split())
        if text not in allowed:
            fail("architecture hero prose must be exactly the reviewed masthead, the details toggle and the "
                 f"record's closing paragraph; unexpected: {text}")
        if text in seen:
            fail(f"architecture hero prose block rendered twice: {text}")
        seen.append(text)
    missing = [item for item in allowed if item not in seen]
    if missing:
        fail("architecture hero must render every reviewed masthead block, the details toggle and the record's "
             "closing paragraph exactly once; missing: " + " | ".join(missing))


def check_summary_line(hero, summaries):
    if len(summaries) != 1:
        fail("architecture must render exactly one visible collapsed hero summary carrying the evidence id")
    if summaries[0] != (expected_summary_line(hero), []):
        fail("architecture collapsed hero summary must read exactly as the record's state, fixture, stage count, "
             "LLVM availability, compiler revision and capture time")


def check_stage_rows(hero, labelled, evidence_root):
    expected_by_label = expected_rows(hero, evidence_root)
    for label, expected in expected_by_label.items():
        if label not in labelled:
            fail(f"architecture stage row missing for {label}")
        if labelled[label] != expected:
            fail(f"architecture row for {label} must read exactly as that stage's own path, identity, byte count "
                 "and links")
    surplus = [label for label in labelled if label not in expected_by_label]
    if surplus:
        fail("architecture rows must be exactly the reviewed stage, excerpt, provenance and platform-heading rows; "
             "unexpected: " + ", ".join(surplus))


def check_platform_rows(hero, platforms):
    rows = hero["environment"]["platforms"]
    if len(platforms) != len(rows):
        fail("architecture must render exactly one visible row per Tier-1 target")
    for row in rows:
        expected = (f"{row['runner']} · {row['architecture']}", [])
        if platforms.count(expected) != 1:
            fail(f"architecture Tier-1 row for {row['runner']} must appear exactly once, reading exactly as its "
                 "own runner and target")


def check_excerpt_units(hero, parser, evidence_root):
    """The excerpts must be byte-exact prefixes of the artifacts, not paraphrases."""
    expected = [[f"{stage['id']}-excerpt", excerpt_text(evidence_root, stage["path"])]
                for stage in hero["snapshot"]["stages"] if stage["id"] in EXCERPTED]
    if parser.units != expected:
        fail("architecture visible stage excerpts must be the exact leading bytes of their artifacts, in stage order")


# A record that says ``unavailable`` while the page still renders stage rows is
# the contradiction this hero exists to make impossible.  These markers are what
# a stage row cannot be written without.
STAGE_ROW_MARKERS = ("sha256 ", "tests/fixtures/architecture-trace/", "-excerpt")


def validate_unavailable_projection(hero, repo_root, site_dir):
    """An unavailable record must leave the page carrying no stage rows at all."""
    page = (site_dir / hero["page_path"].removeprefix("site/")).read_text()
    present = [marker for marker in STAGE_ROW_MARKERS if marker in page]
    if present:
        fail("architecture record is unavailable but the page still renders stage rows: " + ", ".join(present))


def validate_projection(hero, repo_root, site_dir):
    """Visible stage rows bound to their stages, immutable links, locale and the shared summaries."""
    parser = StageRowParser()
    page = (site_dir / hero["page_path"].removeprefix("site/")).read_text()
    parser.feed(page)
    dates = PAGE_DATE.findall(page)
    if len(dates) != 1:
        fail("architecture page must declare exactly one JSON-LD dateModified")
    if parser.language != "en-US" or parser.locales != ["en_US"]:
        fail("architecture locale must be en-US / en_US")
    if parser.hero_count != 1:
        fail("architecture must render exactly one visible evidence hero")
    evidence_root = Path(repo_root)
    labelled, platforms, summaries = stage_rows(parser)
    check_summary_line(hero, summaries)
    check_prose_blocks(hero, parser.prose, dates[0])
    visible = " ".join("".join(parser.hero_text).split())
    literals = [hero["state"], hero["limitations"], hero["attestation"]["collected_at"],
                hero["repository"]["commit"], hero["repository"]["tree"], TEST, FIXTURE]
    for stage in hero["snapshot"]["stages"]:
        literals.extend(item for item in (stage["path"], stage["sha256"]) if item)
    for literal in literals:
        if " ".join(literal.split()) not in visible:
            fail(f"architecture visible stage row/limitation missing: {literal}")
    check_stage_rows(hero, labelled, evidence_root)
    check_platform_rows(hero, platforms)
    check_excerpt_units(hero, parser, evidence_root)
    links = list(hero["stable_links"].values())
    if not set(links) <= set(parser.links):
        fail("architecture visible immutable commit, tree, fixture and issue links missing")
    page_dir = (site_dir / hero["page_path"].removeprefix("site/")).parent
    stylesheet = (site_dir / "styles.css").resolve()
    for href in parser.stylesheets:
        if (page_dir / href.split("?", 1)[0].split("#", 1)[0]).resolve() != stylesheet:
            fail(f"architecture page may link no stylesheet but site/styles.css: {href}")
    hidden_by = hiding_rules("\n".join([stylesheet.read_text(), *parser.embedded_css]), parser)
    if hidden_by:
        fail("architecture stylesheet must not hide the evidence hero or its stage rows or add text to them: "
             + ", ".join(hidden_by))
    for surface in ("markdown", "llm"):
        text = (site_dir / hero["projections"][surface].removeprefix("site/")).read_text()
        if text.count(summary(hero)) != 1:
            fail(f"architecture {surface} pipeline summary/limitations drifted")
