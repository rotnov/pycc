"""Shallow-safe binding for the Pages-owned historical-evidence test suite."""

from pathlib import Path
import re
import unittest


ROOT = Path(__file__).resolve().parents[1]
SUITE = "site_execution_evidence_test.py"
COMMAND = 'python3 -B "$repo_root/scripts/' + SUITE + '"'


class ExecutionEvidenceWiringTests(unittest.TestCase):
    def assert_binding(self, shell, workflow):
        self.assertIn("set -eu", shell)
        self.assertRegex(shell, re.compile("^" + re.escape(COMMAND) + "$", re.M))
        build = re.search(r"\n  build:\n(.*?)(?=\n  [a-z][^ ]*:|\Z)", workflow, re.S)
        self.assertIsNotNone(build, "Pages build job is missing")
        checkout = re.search(
            r"- name: Check out repository\n(.*?)(?=\n      - name:)",
            build.group(1), re.S,
        )
        self.assertIsNotNone(checkout, "Pages checkout is missing")
        self.assertIn("uses: actions/checkout@", checkout.group(1))
        self.assertRegex(checkout.group(1), r"(?m)^          fetch-depth: 0$")
        self.assertRegex(checkout.group(1), r"(?m)^          persist-credentials: false$")
        validation = re.search(
            r"- name: Validate website\n(.*?)(?=\n      - name:|\Z)",
            build.group(1), re.S,
        )
        self.assertIsNotNone(validation, "Pages validation is missing")
        self.assertRegex(validation.group(1), r"(?m)^          ./scripts/test-check-site\.sh$")

    def setUp(self):
        self.shell = (ROOT / "scripts/test-check-site.sh").read_text()
        self.workflow = (ROOT / ".github/workflows/pages.yml").read_text()

    def test_full_history_pages_harness_owns_the_suite(self):
        self.assertTrue((ROOT / "scripts" / SUITE).is_file())
        self.assertFalse(SUITE.startswith("test_"))
        self.assertFalse((ROOT / "scripts/test_site_execution_evidence.py").exists())
        self.assert_binding(self.shell, self.workflow)

    def test_missing_shell_invocation_is_rejected(self):
        with self.assertRaises(AssertionError):
            self.assert_binding(self.shell.replace(COMMAND, "# omitted"), self.workflow)

    def test_missing_pages_harness_is_rejected(self):
        with self.assertRaises(AssertionError):
            self.assert_binding(self.shell, self.workflow.replace("./scripts/test-check-site.sh", "# omitted"))

    def test_shallow_pages_checkout_is_rejected(self):
        with self.assertRaises(AssertionError):
            self.assert_binding(self.shell, self.workflow.replace("fetch-depth: 0", "fetch-depth: 1"))


if __name__ == "__main__":
    unittest.main()
