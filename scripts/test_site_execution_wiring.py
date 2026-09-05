"""Shallow-safe binding for the Pages-owned historical-evidence test suite."""

from pathlib import Path
import re
import unittest


ROOT = Path(__file__).resolve().parents[1]
SUITE = "site_execution_evidence_test.py"
COMMAND = 'python3 -B "$repo_root/scripts/' + SUITE + '"'
EXECUTION_INPUTS = (
    "scripts/check_site_evidence.py",
    "scripts/site_execution_evidence.py",
    "scripts/site_execution_evidence_test.py",
    "tests/site_evidence.rs",
    "tests/fixtures/pep_0526_var_annotations.py",
    "tests/fixtures/pep_0526_var_annotations.expected.txt",
    "tests/diagnostics/d0021_range_argument_type.py",
    "tests/diagnostics/d0021_range_argument_type.expected.txt",
    "tests/diagnostics/d0021_range_argument_type.expected.json",
    "tests/fixtures/conformance-breadth-manifest.json",
)


class ExecutionEvidenceWiringTests(unittest.TestCase):
    def event_block(self, workflow, event):
        block = re.search(rf"(?ms)^  {event}:\n(.*?)(?=^  \S|\Z)", workflow)
        self.assertIsNotNone(block, f"Pages {event} event is missing")
        return block

    def event_paths(self, workflow, event):
        block = self.event_block(workflow, event)
        paths = re.search(r"(?ms)^    paths:\n(.*?)(?=^    \S|\Z)", block.group(1))
        self.assertIsNotNone(paths, f"Pages {event} paths are missing")
        entries = []
        for line in paths.group(1).splitlines():
            if not line.strip() or line.lstrip().startswith("#"):
                continue
            # Model the workflow's literal quoted-list form, failing closed on
            # unmodeled YAML syntax instead of overlooking duplicate aliases.
            entry = re.fullmatch(r'      - "([^"\\]+)"\s*(?:#.*)?', line)
            self.assertIsNotNone(entry, f"Pages {event} paths require literal quoted entries")
            entries.append(entry.group(1))
        return entries

    def assert_dependency_trigger(self, workflow, event, path):
        self.assertEqual(self.event_paths(workflow, event).count(path), 1,
                         f"Pages {event} paths must list {path} exactly once")

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
        for event in ("push", "pull_request"):
            for path in EXECUTION_INPUTS:
                self.assert_dependency_trigger(workflow, event, path)

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

    def test_each_execution_dependency_triggers_each_pages_event(self):
        for event in ("push", "pull_request"):
            for path in EXECUTION_INPUTS:
                with self.subTest(event=event, path=path):
                    self.assert_dependency_trigger(self.workflow, event, path)

    def test_each_dependency_trigger_rejects_absence_and_duplication(self):
        for event in ("push", "pull_request"):
            block = self.event_block(self.workflow, event)
            for path in EXECUTION_INPUTS:
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
