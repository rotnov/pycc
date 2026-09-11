"""Status hero contract: a checked-in commit-bound required-check snapshot.

The ``status`` record in ``site/evidence-heroes.json`` is a sanitized,
offline-refreshed observation of one default-branch revision (D-241).  This
module owns its closed shape, the state mapping, the record-internal
invariants, the Git-object checks that a full-history checkout can prove, and
the visible-projection rules.  Nothing here contacts GitHub: the collector
(``scripts/collect_status_snapshot.py``) is the only network-facing party and it
writes the record through ``expected_shape``/``derive_state`` so it cannot emit
what this module would reject.
"""

import hashlib
import json
import re
import subprocess
from pathlib import Path

import site_execution_evidence


REPO = "https://github.com/rotnov/pycc"
OWNER_ISSUE = f"{REPO}/issues/566"
KIND = "required-checks-snapshot"
APP_ID = 15368
REQUIRED_CONTEXTS = ["ci-gate", "audit"]
COLLECTOR = "scripts/collect_status_snapshot.py"
TEST = "scripts/test_check_status_snapshot.py"
COLLECTION_METHOD = "gh api check-runs, commits, pulls (read-only)"
REQUIRES = "gh auth (read-only), run offline by the refreshing agent; never in CI"
MILESTONE_MARKER = "**Current milestone:"
# Closed Tier-1 job list.  These are check-runs of the CI workflow run that
# ``ci-gate`` aggregates (not children of ``ci-gate``); ``build-test-coverage``
# runs on macos-14 without a target literal in ci.yml, so the
# ``aarch64-apple-darwin`` mapping is this list's, the same implicit mapping the
# landing allowlist documents.
TIER1 = [
    ("build-test-coverage", "macos-14", "aarch64-apple-darwin"),
    ("native-build-test (macos-15-intel, x86_64-apple-darwin)", "macos-15-intel", "x86_64-apple-darwin"),
    ("native-build-test (ubuntu-latest, x86_64-unknown-linux-gnu)", "ubuntu-latest", "x86_64-unknown-linux-gnu"),
    ("native-build-test (ubuntu-24.04-arm, aarch64-unknown-linux-gnu)", "ubuntu-24.04-arm", "aarch64-unknown-linux-gnu"),
    ("native-build-test (windows-latest, x86_64-pc-windows-msvc)", "windows-latest", "x86_64-pc-windows-msvc"),
]
SUBJECTS = [
    ("published-revision", "Published revision (default branch)", None),
    ("post-merge-ci-gate", "Post-merge CI gate", "ci-gate"),
    ("pre-merge-audit", "Pre-merge policy audit", "audit"),
]
LIMITATIONS = (
    "A point-in-time observation of one default-branch revision; later merges "
    "are not covered until the snapshot is refreshed. all-Tier-1 means the "
    "listed jobs concluded success in that run, not release readiness. The "
    "merged pull request's head commit is not reachable from a clean checkout, "
    "so its tree equality is proven from the collector's recorded fields, not "
    "from Git."
)
SHA_RE = re.compile(r"^[0-9a-f]{40}$")
TIME_RE = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$")
RUN_URL_RE = re.compile(r"^https://github\.com/rotnov/pycc/actions/runs/(\d+)$")
JOB_URL_RE = re.compile(r"^https://github\.com/rotnov/pycc/actions/runs/(\d+)/job/(\d+)$")


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


def expected_shape():
    """Closed field sets for every nested object of a non-unavailable record."""
    return {
        "fixture": {"path", "sha256"},
        "test": {"path", "sha256", "names"},
        "command": {"cwd", "argv", "requires"},
        "snapshot": {"subjects"},
        "subject": {"id", "label", "sha", "check", "app_id", "conclusion",
                    "completed_at", "run_id", "run_url", "job_url"},
        "published-revision": {"id", "label", "sha", "check", "app_id", "conclusion",
                               "completed_at", "run_id", "run_url", "job_url",
                               "tree", "parent_count", "merged_pull_request"},
        "merged_pull_request": {"number", "head_sha", "head_tree", "url"},
        "repository": {"commit", "tree", "url"},
        "attestation": {"collected_at", "collection_method", "sanitized",
                        "milestone_line", "required_contexts"},
        "environment": {"platforms"},
        "platform": {"runner", "architecture", "check_run_name", "conclusion", "job_url"},
    }


def canonical_sha256(data):
    return hashlib.sha256(data.replace(b"\r\n", b"\n")).hexdigest()


def milestone_lead(roadmap_text):
    """The bold lead of docs/ROADMAP.md's current-milestone line, without markers."""
    for line in roadmap_text.splitlines():
        if line.startswith(MILESTONE_MARKER):
            end = line.find("**", len(MILESTONE_MARKER))
            if end > 0:
                return line[2:end]
    return None


def subject_by_id(hero):
    subjects = hero["snapshot"]["subjects"] if isinstance(hero.get("snapshot"), dict) else None
    if not isinstance(subjects, list):
        return {}
    return {item.get("id"): item for item in subjects if isinstance(item, dict)}


def derive_state(hero):
    """Closed mapping: every required conclusion is success, or unavailable."""
    subjects = subject_by_id(hero)
    conclusions = [subjects.get(identity, {}).get("conclusion") for identity, _, check in SUBJECTS if check]
    platforms = hero.get("environment") or {}
    rows = platforms.get("platforms") if isinstance(platforms, dict) else None
    if not isinstance(rows, list) or len(rows) != len(TIER1):
        return "unavailable"
    conclusions.extend(row.get("conclusion") if isinstance(row, dict) else None for row in rows)
    return "all-Tier-1" if all(value == "success" for value in conclusions) else "unavailable"


def expected_links(hero):
    revision = subject_by_id(hero)["published-revision"]
    commit = hero["repository"]["commit"]
    links = {
        "commit": f"{REPO}/commit/{commit}",
        "tree": f"{REPO}/tree/{commit}",
        "merged_pull_request": revision["merged_pull_request"]["url"],
        "ci_gate_run": subject_by_id(hero)["post-merge-ci-gate"]["run_url"],
        "audit_run": subject_by_id(hero)["pre-merge-audit"]["run_url"],
    }
    for row in hero["environment"]["platforms"]:
        links[f"job_{row['runner']}"] = row["job_url"]
    return links


def check_run_fields(item, context):
    if item["app_id"] != APP_ID:
        fail(f"{context} app_id must be {APP_ID}")
    if item["conclusion"] != "success":
        fail(f"{context} conclusion must be success for a non-unavailable record")
    if not isinstance(item["completed_at"], str) or not TIME_RE.match(item["completed_at"]):
        fail(f"{context} completed_at must be an RFC 3339 UTC timestamp")
    if not isinstance(item["run_id"], int) or isinstance(item["run_id"], bool) or item["run_id"] <= 0:
        fail(f"{context} run_id must be a positive integer")
    run = RUN_URL_RE.match(item["run_url"] or "")
    if not run or int(run[1]) != item["run_id"]:
        fail(f"{context} run_url must be the immutable run URL for run_id {item['run_id']}")
    job = JOB_URL_RE.match(item["job_url"] or "")
    if not job or int(job[1]) != item["run_id"]:
        fail(f"{context} job_url must be an immutable job URL under run {item['run_id']}")


def validate(hero, evidence_root, repo_root):
    """Structural validation plus record-internal invariants (shallow-safe)."""
    shape = expected_shape()
    evidence_root = Path(evidence_root).resolve()
    repo_root = Path(repo_root)
    page_id = hero["page_id"]
    if page_id != "status" or hero["kind"] != KIND:
        fail("status snapshot validation applies only to the status hero")
    for field in ("fixture", "test", "command", "snapshot", "repository", "attestation", "environment"):
        require_exact_fields(hero[field], shape[field], f"status {field}")
    commit = hero["repository"]["commit"]
    tree = hero["repository"]["tree"]
    for name, value in (("commit", commit), ("tree", tree)):
        if not isinstance(value, str) or not SHA_RE.match(value):
            fail(f"status repository {name} must be a full lowercase SHA")
    if hero["repository"]["url"] != f"{REPO}/commit/{commit}":
        fail("status repository url must be the immutable commit URL")

    for field, expected_path in (("fixture", COLLECTOR), ("test", TEST)):
        if hero[field]["path"] != expected_path:
            fail(f"status {field} path must be {expected_path}")
        path = evidence_root / expected_path
        if path.is_symlink() or not path.is_file() or not path.resolve().is_relative_to(evidence_root):
            fail(f"status {field} file is missing or unsafe: {expected_path}")
        if hero[field]["sha256"] != canonical_sha256(path.read_bytes()):
            fail(f"status {field} sha256 differs from {expected_path}")
    names = hero["test"]["names"]
    if not isinstance(names, list) or not names or not all(isinstance(name, str) for name in names):
        fail("status test names must be a non-empty list of test names")
    test_source = (evidence_root / TEST).read_text()
    for name in names:
        if not re.search(r"^\s+def " + re.escape(name) + r"\(", test_source, re.M):
            fail(f"status test is not registered: {name}")
    if names != re.findall(r"^\s+def (test_\w+)\(", test_source, re.M):
        fail("status test names must list every registered test in source order")

    command = hero["command"]
    if command["cwd"] != "repository-root" or command["requires"] != REQUIRES:
        fail("status command cwd/requires drifted from the reviewed collector contract")
    if command["argv"] != ["python3", COLLECTOR, "--subject", commit]:
        fail("status command argv must invoke the collector on the recorded commit")

    subjects = hero["snapshot"]["subjects"]
    if not isinstance(subjects, list) or [item.get("id") if isinstance(item, dict) else None for item in subjects] != [identity for identity, _, _ in SUBJECTS]:
        fail("status snapshot subjects must be the three ordered subjects")
    by_id = {}
    for (identity, label, check), item in zip(SUBJECTS, subjects):
        shape_key = "published-revision" if check is None else "subject"
        require_exact_fields(item, shape[shape_key], f"status subject {identity}")
        if item["label"] != label:
            fail(f"status subject {identity} label must be {label!r}")
        if item["check"] != check:
            fail(f"status subject {identity} check must be {check!r}")
        if not isinstance(item["sha"], str) or not SHA_RE.match(item["sha"]):
            fail(f"status subject {identity} sha must be a full lowercase SHA")
        by_id[identity] = item
    revision = by_id["published-revision"]
    for field in ("app_id", "conclusion", "completed_at", "run_id", "run_url", "job_url"):
        if revision[field] is not None:
            fail(f"status published-revision {field} must be null; a revision is not a check")
    if revision["sha"] != commit or revision["tree"] != tree:
        fail("status published-revision must be the repository commit and tree")
    if revision["parent_count"] != 1:
        fail("status published-revision must record exactly one parent")
    merged = revision["merged_pull_request"]
    require_exact_fields(merged, shape["merged_pull_request"], "status merged_pull_request")
    if not isinstance(merged["number"], int) or isinstance(merged["number"], bool) or merged["number"] <= 0:
        fail("status merged_pull_request number must be a positive integer")
    if merged["url"] != f"{REPO}/pull/{merged['number']}":
        fail("status merged_pull_request url must be the immutable pull URL")
    if not isinstance(merged["head_sha"], str) or not SHA_RE.match(merged["head_sha"]):
        fail("status merged_pull_request head_sha must be a full lowercase SHA")
    if merged["head_sha"] == commit:
        fail("status merged_pull_request head_sha must differ from the merge commit")
    if merged["head_tree"] != tree:
        fail("status merged_pull_request head_tree must equal the published tree")
    gate = by_id["post-merge-ci-gate"]
    audit = by_id["pre-merge-audit"]
    if gate["sha"] != commit:
        fail("status post-merge-ci-gate must be observed on the repository commit")
    if audit["sha"] != merged["head_sha"]:
        fail("status pre-merge-audit must be observed on the merged pull request head")
    check_run_fields(gate, "status post-merge-ci-gate")
    check_run_fields(audit, "status pre-merge-audit")

    attestation = hero["attestation"]
    if not isinstance(attestation["collected_at"], str) or not TIME_RE.match(attestation["collected_at"]):
        fail("status attestation collected_at must be an RFC 3339 UTC timestamp")
    if attestation["collection_method"] != COLLECTION_METHOD or attestation["sanitized"] is not True:
        fail("status attestation collection_method/sanitized drifted")
    if attestation["required_contexts"] != REQUIRED_CONTEXTS:
        fail("status attestation required_contexts must be the protected-branch contexts")
    current = milestone_lead((repo_root / "docs" / "ROADMAP.md").read_text())
    if not isinstance(attestation["milestone_line"], str) or attestation["milestone_line"] != current:
        fail("status attestation milestone_line is stale against docs/ROADMAP.md; refresh the snapshot")

    rows = hero["environment"]["platforms"]
    if not isinstance(rows, list) or len(rows) != len(TIER1):
        fail("status environment platforms must list the five Tier-1 jobs")
    for (check_name, runner, architecture), row in zip(TIER1, rows):
        require_exact_fields(row, shape["platform"], f"status platform {runner}")
        if (row["check_run_name"], row["runner"], row["architecture"]) != (check_name, runner, architecture):
            fail(f"status platform row drifted from the closed Tier-1 list: {row!r}")
        if row["conclusion"] != "success":
            fail(f"status platform {runner} conclusion must be success for a non-unavailable record")
        job = JOB_URL_RE.match(row["job_url"] or "")
        if not job or int(job[1]) != gate["run_id"]:
            fail(f"status platform {runner} job_url must be an immutable job URL under the ci-gate run")
    if hero["state"] != derive_state(hero) or hero["state"] != "all-Tier-1":
        fail(f"status state {hero['state']!r} does not match the derived state {derive_state(hero)!r}")
    if hero["limitations"] != LIMITATIONS:
        fail("status limitations drifted from the reviewed text")
    if hero["stable_links"] != expected_links(hero):
        fail("status stable_links must be exactly the immutable commit, tree, pull, run and job links")


def git(repo_root, *args):
    return subprocess.run(["git", "-C", str(repo_root), *args], capture_output=True, text=True)


def verify_git(hero, repo_root, head="HEAD"):
    """Prove the subject side from Git objects a full-history checkout has."""
    commit = hero["repository"]["commit"]
    if git(repo_root, "merge-base", "--is-ancestor", commit, head).returncode:
        fail(f"status subject {commit} is not an ancestor of {head}")
    parents = git(repo_root, "rev-list", "--parents", "-n", "1", commit)
    if parents.returncode or len(parents.stdout.split()) != 2:
        fail(f"status subject {commit} must have exactly one parent")
    tree = git(repo_root, "rev-parse", f"{commit}^{{tree}}")
    if tree.returncode or tree.stdout.strip() != hero["repository"]["tree"]:
        fail(f"status subject {commit} tree differs from the recorded tree")


def status_record_at(repo_root, revision):
    shown = git(repo_root, "show", f"{revision}:site/evidence-heroes.json")
    if shown.returncode:
        return None
    try:
        heroes = json.loads(shown.stdout)["heroes"]
    except (ValueError, KeyError, TypeError):
        return None
    return next((hero for hero in heroes if isinstance(hero, dict) and hero.get("page_id") == "status"), None)


def currency_required(repo_root, base, head):
    """True when the range edits the status page or the status record."""
    changed = git(repo_root, "diff", "--name-only", f"{base}...{head}")
    if changed.returncode:
        fail(f"cannot diff {base}...{head}")
    if "site/status/index.html" in changed.stdout.split():
        return True
    return status_record_at(repo_root, base) != status_record_at(repo_root, head)


def check_currency(hero, repo_root, base, max_distance):
    commit = hero["repository"]["commit"]
    if git(repo_root, "merge-base", "--is-ancestor", commit, base).returncode:
        fail(f"status subject {commit} is not an ancestor of the base {base}; refresh the snapshot")
    count = git(repo_root, "rev-list", "--count", "--first-parent", f"{commit}..{base}")
    if count.returncode:
        fail(f"cannot count first-parent merges from {commit} to {base}")
    distance = int(count.stdout.strip())
    if distance > max_distance:
        fail(f"status subject is {distance} first-parent merges behind the base (limit {max_distance}); refresh the snapshot")
    return distance


def summary(hero):
    """Compact central projection; full proof rows stay on the linked page."""
    revision = subject_by_id(hero)["published-revision"]
    merged = revision["merged_pull_request"]
    return (
        f"{hero['evidence_id']} — {hero['state']}: main {hero['repository']['commit']} "
        f"(PR #{merged['number']} head {merged['head_sha']}) — ci-gate success, audit success, "
        f"five Tier-1 jobs success; captured {hero['attestation']['collected_at']}. "
        f"{hero['limitations']} [Exact subjects, run and job links]"
        f"(https://rotnov.github.io/pycc{hero['route']})."
    )


def validate_projection(hero, repo_root, site_dir):
    """Visible proof rows, immutable links, locale and the shared summaries."""
    parser = site_execution_evidence.VisibleExecutionParser()
    parser.feed((site_dir / hero["page_path"].removeprefix("site/")).read_text())
    if parser.language != "en-US" or parser.locales != ["en_US"]:
        fail("status locale must be en-US / en_US")
    if parser.hero_count != 1:
        fail("status must render exactly one visible evidence hero")
    visible = " ".join("".join(parser.hero_text).split())
    revision = subject_by_id(hero)["published-revision"]
    literals = [hero["state"], hero["limitations"], hero["attestation"]["collected_at"],
                hero["attestation"]["milestone_line"], str(APP_ID),
                f"#{revision['merged_pull_request']['number']}",
                revision["merged_pull_request"]["head_sha"], hero["repository"]["tree"]]
    for item in hero["snapshot"]["subjects"]:
        literals.extend([item["label"], item["sha"]])
        if item["check"] is not None:
            literals.extend([item["check"], item["conclusion"], item["completed_at"]])
    for row in hero["environment"]["platforms"]:
        literals.extend([row["check_run_name"], row["runner"], row["architecture"], row["conclusion"]])
    for literal in literals:
        if " ".join(literal.split()) not in visible:
            fail(f"status visible proof row/limitation missing: {literal}")
    links = list(hero["stable_links"].values())
    links.extend(item["job_url"] for item in hero["snapshot"]["subjects"] if item["job_url"])
    if not set(links) <= set(parser.links):
        fail("status visible immutable commit/tree/pull/run/job links missing")
    for surface in ("markdown", "llm"):
        text = (site_dir / hero["projections"][surface].removeprefix("site/")).read_text()
        if text.count(summary(hero)) != 1:
            fail(f"status {surface} snapshot summary/limitations drifted")
