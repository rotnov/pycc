"""Shallow-safe binding for the Pages-owned status-snapshot test suite (D-241).

Mirrors ``test_site_status_wiring``'s sibling for the execution heroes: the
Pages harness must run the status suite, every collector/validator input must
trigger both Pages events exactly once, and the pull-request leg must run the
snapshot currency check.
"""

from pathlib import Path
import re
import unittest

import test_site_execution_wiring as execution_wiring


ROOT = Path(__file__).resolve().parents[1]
SUITE = "site_status_evidence_test.py"
COMMAND = 'python3 -B "$repo_root/scripts/' + SUITE + '"'
CURRENCY = ('python3 scripts/check_status_snapshot.py --currency --base "${BASE_SHA}" --head "${HEAD_SHA}" \\\n'
            '            --max-first-parent-distance 20 site/evidence-heroes.json . .')
STATUS_INPUTS = (
    "scripts/site_status_evidence.py",
    "scripts/site_status_evidence_test.py",
    "scripts/check_status_snapshot.py",
    "scripts/test_check_status_snapshot.py",
    "scripts/collect_status_snapshot.py",
)


class StatusEvidenceWiringTests(unittest.TestCase):
    helper = execution_wiring.ExecutionEvidenceWiringTests()

    def setUp(self):
        self.shell = (ROOT / "scripts/test-check-site.sh").read_text()
        self.workflow = (ROOT / ".github/workflows/pages.yml").read_text()

    def assert_binding(self, shell, workflow):
        self.assertRegex(shell, re.compile("^" + re.escape(COMMAND) + "$", re.M))
        build = re.search(r"\n  build:\n(.*?)(?=\n  [a-z][^ ]*:|\Z)", workflow, re.S)
        self.assertIsNotNone(build, "Pages build job is missing")
        step = re.search(
            r"- name: Check the status snapshot's currency against the pull request \(issue #1006\)\n"
            r"(.*?)(?=\n      - name:|\Z)", build.group(1), re.S)
        self.assertIsNotNone(step, "Pages status currency step is missing")
        self.assertRegex(step.group(1), r"(?m)^          BASE_SHA: \$\{\{ github\.event\.pull_request\.base\.sha \}\}$")
        self.assertRegex(step.group(1), r"(?m)^          HEAD_SHA: \$\{\{ github\.event\.pull_request\.head\.sha \}\}$")
        self.assertIn("set -euo pipefail", step.group(1))
        self.assertIn(CURRENCY, step.group(1))
        for event in ("push", "pull_request"):
            for path in STATUS_INPUTS:
                self.helper.assert_dependency_trigger(workflow, event, path)

    def test_full_history_pages_harness_owns_the_suite(self):
        self.assertTrue((ROOT / "scripts" / SUITE).is_file())
        self.assertFalse(SUITE.startswith("test_"))
        self.assertFalse((ROOT / "scripts/test_site_status_evidence.py").exists())
        self.assert_binding(self.shell, self.workflow)

    def test_missing_shell_invocation_is_rejected(self):
        with self.assertRaises(AssertionError):
            self.assert_binding(self.shell.replace(COMMAND, "# omitted"), self.workflow)

    def test_missing_currency_step_is_rejected(self):
        for old, new in (
            ("Check the status snapshot's currency against the pull request (issue #1006)", "Renamed step"),
            (CURRENCY, "true"),
            ("--max-first-parent-distance 20", "--max-first-parent-distance 200"),
            ("          BASE_SHA: ${{ github.event.pull_request.base.sha }}\n          HEAD_SHA: ${{ github.event.pull_request.head.sha }}\n"
             "        run: |\n          set -euo pipefail\n          if [ -z \"${BASE_SHA}\" ]",
             "          BASE_SHA: ${{ github.event.pull_request.base.sha }}\n          HEAD_SHA: ${{ github.event.pull_request.head.sha }}\n"
             "        run: |\n          if [ -z \"${BASE_SHA}\" ]"),
        ):
            with self.subTest(mutation=new):
                self.assertIn(old, self.workflow)
                with self.assertRaises(AssertionError):
                    self.assert_binding(self.shell, self.workflow.replace(old, new))

    def test_each_status_dependency_triggers_each_pages_event(self):
        for event in ("push", "pull_request"):
            for path in STATUS_INPUTS:
                with self.subTest(event=event, path=path):
                    self.helper.assert_dependency_trigger(self.workflow, event, path)

    def test_each_dependency_trigger_rejects_absence_and_duplication(self):
        for event in ("push", "pull_request"):
            block = self.helper.event_block(self.workflow, event)
            for path in STATUS_INPUTS:
                line = f'      - "{path}"\n'
                self.assertIn(line, block.group(1))
                for replacement in ("", line + line):
                    with self.subTest(event=event, path=path, replacement=replacement):
                        changed = block.group(1).replace(line, replacement, 1)
                        workflow = self.workflow[:block.start(1)] + changed + self.workflow[block.end(1):]
                        with self.assertRaisesRegex(AssertionError, re.escape(
                                f"Pages {event} paths must list {path} exactly once")):
                            self.assert_binding(self.shell, workflow)


if __name__ == "__main__":
    unittest.main()
