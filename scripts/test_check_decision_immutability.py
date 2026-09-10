"""Tests for scripts/check_decision_immutability.py (issue #1000, Part 2 of #77).

The rule tests drive ``check_file`` on strings and never touch git; the
plumbing tests build tiny throwaway repositories (two decision files, two
commits) and drive ``main`` with ``--base``/``--head`` or a fake GitHub
Actions event. ``RealCorpusTest`` runs the rule over every current decision
file with itself as the head, so stub detection and the walk are exercised on
real bodies, and ``CiWiringTest`` mirrors ``test_generate_decisions_index``'s
binding of the step into the ``governance`` job ``ci-gate`` requires.
"""

from __future__ import annotations

import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import check_decision_immutability as cdi

REPOSITORY_ROOT = Path(__file__).resolve().parent.parent
DECISIONS_DIR = REPOSITORY_ROOT / "docs" / "decisions"


def frontmatter(id_, title, status):
    return f'---\nid: {id_}\ntitle: "{title}"\nstatus: {status}\n---\n'


def long_form(id_="D-101", title="Long form", status="accepted", body=None):
    if body is None:
        body = (
            f"## {id_}: {title}\n\n"
            f"- Status: {status}\n"
            "- Context: what forces the choice\n"
            "- Decision: what we do\n"
            "- Alternatives: what we rejected\n"
            "- Consequences: what changes\n"
        )
    return frontmatter(id_, title, status) + "\n" + body


def stub(id_="D-001", title="Stub title", status="accepted", tail=""):
    return (
        frontmatter(id_, title, status)
        + f"\n# {id_}\n\n{cdi.STUB_MARKER}\n\n{title}\n"
        + tail
    )


PATH = "docs/decisions/D-101-long-form.md"
D032_PATH = "docs/decisions/D-032-branch-protection-requires-one-aggregate-ci-gate.md"


class RuleTests(unittest.TestCase):
    def assertPasses(self, base, head, path=PATH):
        self.assertEqual(cdi.check_file(path, base, head), [])

    def assertFails(self, base, head, fragment, path=PATH):
        violations = cdi.check_file(path, base, head)
        self.assertEqual(len(violations), 1, violations)
        self.assertIn(fragment, violations[0])
        self.assertTrue(violations[0].startswith(f"{path}: "), violations[0])

    # -- passing shapes ---------------------------------------------------

    def test_untouched_file_passes(self):
        self.assertPasses(long_form(), long_form())

    def test_new_file_is_unconstrained(self):
        # A path absent at the base never reaches check_file with a base
        # text; the closest string-level analogue is a proposed base.
        self.assertPasses(long_form(status="proposed"), "anything at all\n")

    def test_proposed_file_may_be_rewritten_freely(self):
        self.assertPasses(long_form(status="proposed"), long_form(status="proposed", body="rewritten\n"))

    def test_proposed_to_accepted_with_body_rewrite_passes(self):
        self.assertPasses(long_form(status="proposed"), long_form(status="accepted", body="- Status: accepted\nnew body\n"))

    def test_insert_only_dated_amendment_passes(self):
        head = long_form().replace(
            "- Context:", "- Amendment (2026-09-10): the switch happened, see D-033.\n- Context:"
        )
        self.assertPasses(long_form(), head)

    def test_accepted_to_superseded_with_multi_line_status_passes(self):
        head = long_form(status="accepted").replace(
            "status: accepted\n", "status: superseded\n"
        ).replace(
            "- Status: accepted\n",
            "- Status: superseded by D-200\n  (the continuation explains why)\n",
        )
        self.assertPasses(long_form(), head)

    def test_accepted_stub_may_receive_its_long_form_entry(self):
        base_paths = sorted(DECISIONS_DIR.glob("D-001-*.md"))
        self.assertEqual(len(base_paths), 1)
        base_text = base_paths[0].read_text(encoding="utf-8")
        self.assertTrue(cdi.is_index_only_stub(base_text.splitlines()))
        head = base_text.split("\n---\n")[0] + "\n---\n\n" + (
            "## D-001: hybrid int\n\n- Status: accepted\n- Context: filled in\n"
        )
        self.assertPasses(base_text, head, path=f"docs/decisions/{base_paths[0].name}")

    def test_insert_only_edit_to_a_stub_that_keeps_the_stub_text_passes(self):
        head = stub() + "\n- Amendment (2026-09-10): a note.\n"
        self.assertPasses(stub(), head)

    def test_superseded_stub_with_appended_note_may_replace_stub_text(self):
        base_paths = sorted(DECISIONS_DIR.glob("D-005-*.md"))
        self.assertEqual(len(base_paths), 1)
        base_text = base_paths[0].read_text(encoding="utf-8")
        lines = base_text.splitlines()
        self.assertTrue(cdi.is_index_only_stub(lines))
        self.assertTrue(lines[12].startswith("**Superseded by"))
        head_lines = lines[:6] + ["## D-005: Exceptions", "", "- Status: superseded"] + lines[11:]
        self.assertPasses(base_text, "\n".join(head_lines) + "\n", path=f"docs/decisions/{base_paths[0].name}")

    def test_trailing_newline_only_change_passes(self):
        self.assertPasses(long_form(), long_form().rstrip("\n"))
        self.assertPasses(long_form(), long_form() + "\n")

    def test_amendment_inserted_directly_above_a_transitioning_status_line_passes(self):
        head = long_form().replace(
            "status: accepted\n", "status: superseded\n"
        ).replace(
            "- Status: accepted\n",
            "- Amendment (2026-09-10): superseded by D-200.\n- Status: superseded by D-200\n",
        )
        self.assertPasses(long_form(), head)

    def test_repeated_lines_receiving_insertions_pass(self):
        # The shape on which difflib.SequenceMatcher(autojunk=False)
        # misreports a pure insertion as a delete opcode.
        body = "- Status: accepted\n  z\n  z\n- x\n  z\n- y\n"
        base = long_form(body=body)
        head = long_form(body="- Status: accepted\n  z\n- n1\n  z\n- x\n- n2\n  z\n- n3\n- y\n")
        self.assertPasses(base, head)

    def test_narrowing_annotation_on_first_status_line_passes(self):
        head = long_form().replace(
            "- Status: accepted\n",
            "- Status: accepted (one clause is narrowly superseded by D-200)\n",
        )
        self.assertPasses(long_form(), head)

    def test_superseding_a_stub_without_a_body_status_line_passes(self):
        self.assertPasses(stub(), stub(status="superseded"))

    def test_unparseable_base_frontmatter_is_not_frozen(self):
        self.assertPasses("no frontmatter\n", "anything\n")

    # -- failing shapes ---------------------------------------------------

    def test_pr_74_same_line_append_on_d032_fails(self):
        base_paths = sorted(DECISIONS_DIR.glob("D-032-*.md"))
        self.assertEqual(len(base_paths), 1)
        base_text = base_paths[0].read_text(encoding="utf-8")
        lines = base_text.splitlines()
        self.assertTrue(lines[14].endswith("does not by itself block a merge into `main`."))
        lines[14] += " (The switch has since happened -- see D-033.)"
        self.assertFails(
            base_text,
            "\n".join(lines) + "\n",
            "base line 15 removed or changed",
            path=f"docs/decisions/{base_paths[0].name}",
        )

    def test_in_place_reword_fails(self):
        head = long_form().replace("- Decision: what we do", "- Decision: what we now do")
        self.assertFails(long_form(), head, "base line 11 removed or changed: - Decision: what we do")

    def test_deleted_line_fails(self):
        head = long_form().replace("- Alternatives: what we rejected\n", "")
        self.assertFails(long_form(), head, "base line 12 removed or changed: - Alternatives")

    def test_deleted_file_fails(self):
        self.assertFails(long_form(), None, "accepted decision deleted or renamed")
        self.assertFails(long_form(status="superseded"), None, "superseded decision deleted or renamed")

    def test_accepted_to_proposed_fails(self):
        head = long_form().replace("status: accepted\n", "status: proposed\n")
        self.assertFails(long_form(), head, "status accepted -> proposed is not a permitted transition")

    def test_superseded_to_accepted_fails(self):
        base = long_form(status="superseded")
        head = base.replace("status: superseded\n", "status: accepted\n")
        self.assertFails(base, head, "status superseded -> accepted is not a permitted transition")

    def test_head_frontmatter_removed_fails(self):
        head = long_form().split("\n---\n", 1)[1]
        self.assertFails(long_form(), head, "head frontmatter is missing or malformed")

    def test_head_frontmatter_with_crlf_fails(self):
        self.assertFails(long_form(), long_form().replace("\n", "\r\n"), "head frontmatter is missing or malformed")

    def test_frontmatter_title_reworded_fails(self):
        head = long_form().replace('title: "Long form"', 'title: "Long form, reworded"')
        self.assertFails(long_form(), head, "base line 3 removed or changed")

    def test_stub_with_title_changed_fails(self):
        head = stub().replace('title: "Stub title"', 'title: "Other title"')
        self.assertFails(stub(), head, "base line 3 removed or changed")

    def test_stub_with_frontmatter_removed_fails_without_raising(self):
        self.assertFails(stub(), "# D-001\n", "head frontmatter is missing or malformed")
        self.assertFails(stub(), "", "head frontmatter is missing or malformed")

    def test_superseded_stub_with_its_note_reworded_or_deleted_fails(self):
        base_paths = sorted(DECISIONS_DIR.glob("D-005-*.md"))
        base_text = base_paths[0].read_text(encoding="utf-8")
        lines = base_text.splitlines()
        path = f"docs/decisions/{base_paths[0].name}"
        reworded = lines[:12] + ["**Superseded by D-173** -- reworded."]
        self.assertFails(base_text, "\n".join(reworded) + "\n", "base line 13 removed or changed", path=path)
        deleted = lines[:11]
        self.assertFails(base_text, "\n".join(deleted) + "\n", "base line 13 removed or changed", path=path)

    def test_stub_marker_inside_a_long_form_entry_does_not_unlock_the_body(self):
        # Step one (an earlier PR) inserted the marker; step two rewrites the
        # body. Because a `- Status:` line is present the exemption is off.
        base = long_form(body=f"## D-101: Long form\n\n{cdi.STUB_MARKER}\n\n- Status: accepted\n- Context: original\n")
        self.assertFalse(cdi.is_index_only_stub(base.splitlines()))
        head = long_form(body="## D-101: Long form\n\n- Status: accepted\n- Context: rewritten\n")
        self.assertFails(base, head, "removed or changed")

    def test_body_line_starting_with_status_colon_is_not_exempt(self):
        base = long_form(body="- Status: accepted\nstatus: literal body text\n")
        head = long_form(body="- Status: accepted\nstatus: something else\n")
        self.assertFails(base, head, "base line 8 removed or changed: status: literal body text")

    def test_second_body_status_line_is_not_exempt(self):
        base = long_form(body="- Status: accepted\n- Status: a second one\n")
        head = long_form(body="- Status: accepted (annotated)\n- Status: a rewritten second one\n")
        self.assertFails(base, head, "base line 8 removed or changed: - Status: a second one")

    def test_rewriting_the_context_line_together_with_the_status_line_fails(self):
        head = long_form().replace(
            "- Status: accepted\n- Context: what forces the choice\n",
            "- Status: superseded by D-200\n- Context: rewritten\n",
        ).replace("status: accepted\n", "status: superseded\n")
        self.assertFails(long_form(), head, "base line 10 removed or changed: - Context:")

    def test_non_utf8_byte_in_the_head_body_is_a_violation_not_a_traceback(self):
        head_bytes = long_form().encode("utf-8").replace(b"what we do", b"what we d\xff")
        head = head_bytes.decode("utf-8", "surrogateescape")
        self.assertFails(long_form(), head, "base line 11 removed or changed")

    def test_decision_path_filter(self):
        self.assertTrue(cdi.is_decision_path("docs/decisions/D-240-anything.md"))
        self.assertFalse(cdi.is_decision_path("docs/decisions/README.md"))
        self.assertFalse(cdi.is_decision_path("docs/decisions/TEMPLATE.md"))
        self.assertFalse(cdi.is_decision_path("docs/decisions/D-999-x.md/inner.md"))
        self.assertFalse(cdi.is_decision_path("other/docs/decisions/D-240-anything.md"))


class RealCorpusTest(unittest.TestCase):
    def test_every_current_decision_file_passes_against_itself(self):
        files = sorted(DECISIONS_DIR.glob("D-*.md"))
        self.assertGreater(len(files), 200)
        frozen = 0
        stubs = 0
        for path in files:
            text = path.read_text(encoding="utf-8")
            relative = f"docs/decisions/{path.name}"
            self.assertEqual(cdi.check_file(relative, text, text), [], relative)
            if cdi.frozen_status(text) is not None:
                frozen += 1
                if cdi.is_index_only_stub(text.splitlines()):
                    stubs += 1
        self.assertGreater(frozen, 0)
        self.assertGreater(stubs, 0)


def git(repo, *args, **kwargs):
    return subprocess.run(
        ["git", "-C", str(repo), *args],
        check=True,
        capture_output=True,
        text=True,
        **kwargs,
    ).stdout.strip()


def make_repo(root):
    """Initialise a repo with one accepted and one proposed decision."""
    subprocess.run(["git", "init", "-q", "-b", "main", str(root)], check=True)
    git(root, "config", "user.email", "test@example.test")
    git(root, "config", "user.name", "Immutability Test")
    git(root, "config", "commit.gpgsign", "false")
    decisions = root / "docs" / "decisions"
    decisions.mkdir(parents=True)
    (decisions / "D-101-long-form.md").write_text(long_form(), encoding="utf-8")
    (decisions / "D-102-proposed.md").write_text(long_form("D-102", "Proposed", "proposed"), encoding="utf-8")
    (decisions / "README.md").write_text("index\n", encoding="utf-8")
    git(root, "add", ".")
    git(root, "commit", "-qm", "base")
    return git(root, "rev-parse", "HEAD")


def commit_all(root, message="head"):
    git(root, "add", "-A")
    git(root, "commit", "-qm", message)
    return git(root, "rev-parse", "HEAD")


class PlumbingTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name) / "repo"
        self.base = make_repo(self.root)
        self.decisions = self.root / "docs" / "decisions"

    def tearDown(self):
        self._tmp.cleanup()

    def run_main(self, *argv, env=None):
        clean = {k: v for k, v in os.environ.items() if not k.startswith("GITHUB_")}
        clean.update(env or {})
        with mock.patch.dict(os.environ, clean, clear=True):
            with mock.patch("sys.stdout") as out, mock.patch("sys.stderr") as err:
                code = cdi.main(["--root", str(self.root), *argv])
        stdout = "".join(str(c.args[0]) for c in out.write.call_args_list)
        stderr = "".join(str(c.args[0]) for c in err.write.call_args_list)
        return code, stdout, stderr

    def test_explicit_range_passes_on_insert_only_amendment(self):
        path = self.decisions / "D-101-long-form.md"
        path.write_text(path.read_text().replace("- Context:", "- Amendment (2026-09-10): note.\n- Context:"))
        (self.decisions / "D-102-proposed.md").write_text("rewritten proposed\n")
        (self.decisions / "D-103-new.md").write_text(long_form("D-103", "New", "accepted"))
        (self.decisions / "README.md").write_text("regenerated index\n")
        head = commit_all(self.root)
        code, stdout, stderr = self.run_main("--base", self.base, "--head", head)
        self.assertEqual(code, 0, stderr)
        self.assertIn("passed (2 decision files compared)", stdout)

    def test_explicit_range_fails_on_in_place_edit_with_context(self):
        path = self.decisions / "D-101-long-form.md"
        path.write_text(path.read_text().replace("what we do", "what we now do"))
        head = commit_all(self.root)
        code, _stdout, stderr = self.run_main("--base", self.base, "--head", head)
        self.assertEqual(code, 1)
        self.assertIn("D-101-long-form.md: base line 11 removed or changed", stderr)
        self.assertIn("-- Decision: what we do\n+- Decision: what we now do", stderr)
        self.assertIn("1 violation(s) in 1 decision file(s) compared", stderr)

    def test_renamed_frozen_file_fails_as_a_deletion(self):
        git(self.root, "mv", "docs/decisions/D-101-long-form.md", "docs/decisions/D-101-renamed.md")
        head = commit_all(self.root)
        code, _stdout, stderr = self.run_main("--base", self.base, "--head", head)
        self.assertEqual(code, 1)
        self.assertIn("D-101-long-form.md: accepted decision deleted or renamed", stderr)
        self.assertNotIn("--- base/", stderr)

    def test_frozen_file_replaced_by_a_symlink_fails(self):
        path = self.decisions / "D-101-long-form.md"
        path.unlink()
        path.symlink_to("D-102-proposed.md")
        head = commit_all(self.root)
        self.assertEqual(
            cdi.changed_decision_files(self.root, self.base, head),
            [("T", "docs/decisions/D-101-long-form.md")],
        )
        code, _stdout, stderr = self.run_main("--base", self.base, "--head", head)
        self.assertEqual(code, 1)
        self.assertIn("diff status 'T'", stderr)

    def test_type_change_of_a_proposed_file_is_ignored(self):
        path = self.decisions / "D-102-proposed.md"
        path.unlink()
        path.symlink_to("D-101-long-form.md")
        head = commit_all(self.root)
        code, stdout, _stderr = self.run_main("--base", self.base, "--head", head)
        self.assertEqual(code, 0)
        self.assertIn("(1 decision files compared)", stdout)

    def test_non_decision_paths_and_nested_lookalikes_are_ignored(self):
        (self.decisions / "README.md").write_text("changed\n")
        (self.decisions / "TEMPLATE.md").write_text("new template\n")
        nested = self.decisions / "D-999-x.md"
        nested.mkdir()
        (nested / "D-101-long-form.md").write_text("not a decision\n")
        head = commit_all(self.root)
        code, stdout, _stderr = self.run_main("--base", self.base, "--head", head)
        self.assertEqual(code, 0)
        self.assertIn("(0 decision files compared)", stdout)

    def test_pull_request_event_resolves_base_sha_and_github_sha(self):
        path = self.decisions / "D-101-long-form.md"
        path.write_text(path.read_text().replace("what we do", "what we now do"))
        head = commit_all(self.root)
        event = Path(self._tmp.name) / "event.json"
        event.write_text(json.dumps({"pull_request": {"base": {"sha": self.base}}}))
        code, _stdout, stderr = self.run_main(env={
            "GITHUB_EVENT_NAME": "pull_request",
            "GITHUB_EVENT_PATH": str(event),
            "GITHUB_SHA": head,
        })
        self.assertEqual(code, 1)
        self.assertIn("base line 11 removed or changed", stderr)

    def test_push_event_resolves_before_and_github_sha(self):
        path = self.decisions / "D-101-long-form.md"
        path.write_text(path.read_text().replace("- Context:", "- Amendment (2026-09-10): note.\n- Context:"))
        head = commit_all(self.root)
        event = Path(self._tmp.name) / "event.json"
        event.write_text(json.dumps({"before": self.base, "after": head}))
        code, stdout, _stderr = self.run_main(env={
            "GITHUB_EVENT_NAME": "push",
            "GITHUB_EVENT_PATH": str(event),
            "GITHUB_SHA": head,
        })
        self.assertEqual(code, 0)
        self.assertIn("passed (1 decision files compared)", stdout)

    def test_push_event_with_all_zero_before_skips(self):
        event = Path(self._tmp.name) / "event.json"
        event.write_text(json.dumps({"before": cdi.ZERO_SHA}))
        code, stdout, _stderr = self.run_main(env={
            "GITHUB_EVENT_NAME": "push",
            "GITHUB_EVENT_PATH": str(event),
            "GITHUB_SHA": self.base,
        })
        self.assertEqual(code, 0)
        self.assertIn("skipping", stdout)

    def test_unknown_event_exits_2(self):
        event = Path(self._tmp.name) / "event.json"
        event.write_text("{}")
        code, _stdout, stderr = self.run_main(env={
            "GITHUB_EVENT_NAME": "workflow_dispatch",
            "GITHUB_EVENT_PATH": str(event),
            "GITHUB_SHA": self.base,
        })
        self.assertEqual(code, 2)
        self.assertIn("unsupported GITHUB_EVENT_NAME 'workflow_dispatch'", stderr)

    def test_event_without_path_or_sha_exits_2(self):
        code, _stdout, stderr = self.run_main(env={"GITHUB_EVENT_NAME": "push"})
        self.assertEqual(code, 2)
        self.assertIn("needs GITHUB_EVENT_PATH and GITHUB_SHA", stderr)

    def test_no_flags_and_no_event_exits_2(self):
        code, _stdout, stderr = self.run_main()
        self.assertEqual(code, 2)
        self.assertIn("pass --base REV --head REV", stderr)

    def test_base_without_head_exits_2(self):
        code, _stdout, stderr = self.run_main("--base", self.base)
        self.assertEqual(code, 2)
        self.assertIn("--base and --head must be given together", stderr)

    def test_missing_revision_triggers_exactly_one_shallow_fetch(self):
        path = self.decisions / "D-101-long-form.md"
        path.write_text(path.read_text().replace("what we do", "what we now do"))
        head = commit_all(self.root)
        clone = Path(self._tmp.name) / "clone"
        subprocess.run(
            ["git", "clone", "-q", "--depth=1", "--no-tags", f"file://{self.root}", str(clone)],
            check=True,
            capture_output=True,
        )
        self.assertNotEqual(
            subprocess.run(["git", "-C", str(clone), "cat-file", "-e", f"{self.base}^{{commit}}"]).returncode,
            0,
        )
        with mock.patch.object(cdi.subprocess, "run", wraps=subprocess.run) as spy:
            with mock.patch("sys.stdout"), mock.patch("sys.stderr") as err:
                code = cdi.main(["--root", str(clone), "--base", self.base, "--head", head])
        fetches = [c for c in spy.call_args_list if c.args[0][1] == "fetch"]
        self.assertEqual(len(fetches), 1)
        self.assertEqual(fetches[0].args[0], ["git", "fetch", "--no-tags", "--depth=1", "origin", self.base])
        self.assertEqual(code, 1)
        stderr = "".join(str(c.args[0]) for c in err.write.call_args_list)
        self.assertIn("base line 11 removed or changed", stderr)

    def test_unresolvable_revision_exits_2(self):
        code, _stdout, stderr = self.run_main("--base", "0" * 39 + "1", "--head", self.base)
        self.assertEqual(code, 2)
        self.assertIn("could not resolve revision", stderr)
        self.assertIn("git fetch --no-tags --depth=1 origin", stderr)


class CiWiringTest(unittest.TestCase):
    """The guard is only a merge gate while required CI actually runs it.

    Mirrors ``test_generate_decisions_index.CiWiringTest``: the step lives
    in ``governance``, which ``ci-gate`` -- the required branch-protection
    check -- needs unconditionally, and it is not advisory. The base-owned
    ``audit`` binds the same step through
    ``D171_GOVERNANCE_POLICY_STEPS`` in ``scripts/check_roadmap_evidence.rb``
    once this change is on ``main``.
    """

    WORKFLOW = REPOSITORY_ROOT / ".github" / "workflows" / "ci.yml"
    STEP_NAME = "Check accepted-decision immutability (issue 1000)"
    STEP_RUN = "python3 -B scripts/check_decision_immutability.py"

    def setUp(self):
        self.text = self.WORKFLOW.read_text(encoding="utf-8")

    def _job_body(self, name):
        start = self.text.index(f"\n  {name}:\n")
        rest = self.text[start + 1 :]
        lines = rest.split("\n")
        body = [lines[0]]
        for line in lines[1:]:
            if line.startswith("  ") and not line.startswith("   ") and line.rstrip().endswith(":"):
                break
            body.append(line)
        return "\n".join(body)

    def test_the_governance_job_runs_the_guard_with_no_flags(self):
        body = self._job_body("governance")
        self.assertIn(f"- name: {self.STEP_NAME}\n        run: {self.STEP_RUN}\n", body)

    def test_the_step_name_carries_no_hash_sign(self):
        # An unquoted `#` in a YAML scalar starts a comment, which is why the
        # coverage-badge step's key in D171_GOVERNANCE_POLICY_STEPS is
        # truncated; this step avoids the trap so its key is the full name.
        self.assertNotIn("#", self.STEP_NAME)

    def test_ci_gate_requires_the_governance_job(self):
        gate = self._job_body("ci-gate")
        self.assertIn("- governance", gate)
        self.assertIn("needs.governance.result != 'success'", gate)

    def test_the_guard_is_not_wired_as_advisory(self):
        self.assertNotIn("continue-on-error", self._job_body("governance"))

    def test_the_d171_fixture_and_policy_table_carry_the_step(self):
        fixture = REPOSITORY_ROOT / "tests" / "fixtures" / "policy-successors" / "ci-d171.yml"
        self.assertIn(f"- name: {self.STEP_NAME}\n        run: {self.STEP_RUN}\n", fixture.read_text(encoding="utf-8"))
        checker = (REPOSITORY_ROOT / "scripts" / "check_roadmap_evidence.rb").read_text(encoding="utf-8")
        self.assertIn(f'"{self.STEP_NAME}" =>\n    "{self.STEP_RUN}"', checker)


if __name__ == "__main__":
    unittest.main()
