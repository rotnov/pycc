#!/usr/bin/env python3
"""Tests for scripts/check_corpus_compile_rate.py.

The metric is driven by a fake ``pycc`` shim, never the real compiler: the point
under test is the reporting and exit-status contract, and a test that depended on
what pycc currently accepts would change its verdict every time the compiler
improved. Both directions of that contract are proven -- every measurement
outcome exits 0 (including a zero compile rate, no qualifying speedup sample and
``--max-seconds`` exhaustion), and every broken-harness case exits non-zero.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import stat
import subprocess
import sys
import tempfile
import unittest
import unittest.mock
from pathlib import Path

METRIC_PATH = Path(__file__).with_name("check_corpus_compile_rate.py")
METRIC_SPEC = importlib.util.spec_from_file_location("corpus_compile_rate", METRIC_PATH)
if METRIC_SPEC is None or METRIC_SPEC.loader is None:
    raise RuntimeError("could not load scripts/check_corpus_compile_rate.py")
METRIC = importlib.util.module_from_spec(METRIC_SPEC)
METRIC_SPEC.loader.exec_module(METRIC)

# A shim standing in for `pycc build <source> -o <out>`. It succeeds when the
# solution's first line is a `# ok` marker and otherwise prints pycc-shaped
# diagnostics taken from that marker line, so a test can script any tally.
PYCC_SHIM = '''#!/usr/bin/env python3
import os, stat, sys
source, out = sys.argv[2], sys.argv[4]
lines = open(source).read().splitlines()
marker = lines[0] if lines else ""
if not marker.startswith("# ok"):
    for code in marker.removeprefix("# fail ").split("|"):
        print("error[" + code + "]: shim diagnostic", file=sys.stderr)
    sys.exit(1)
body = "\\n".join(lines[1:])
with open(out, "w") as handle:
    handle.write("#!" + sys.executable + "\\n" + body + "\\n")
os.chmod(out, os.stat(out).st_mode | stat.S_IEXEC)
'''

# A shim that reports no recognizable diagnostic at all.
SILENT_SHIM = '''#!/usr/bin/env python3
import sys
print("something went wrong", file=sys.stderr)
sys.exit(1)
'''


def write_executable(path: Path, text: str) -> Path:
    path.write_text(text, encoding="utf-8")
    path.chmod(path.stat().st_mode | stat.S_IEXEC)
    return path


class CorpusBuilder:
    """Build a throwaway corpus tree with a consistent manifest."""

    def __init__(self, root: Path) -> None:
        self.root = root
        self.root.mkdir(parents=True, exist_ok=True)
        self.problems: list[dict] = []
        self.files: dict[str, dict] = {}
        self.max_bytes = 16 * 1024 * 1024

    def add_root_file(self, name: str, text: str) -> None:
        data = text.encode()
        (self.root / name).write_bytes(data)
        self.files[name] = {"bytes": len(data), "sha256": METRIC.sha256_bytes(data)}

    def add(
        self,
        index: int,
        solution: str,
        cases: list[dict[str, str]],
        set_name: str = "steering",
    ) -> dict:
        subdir = "problems" if set_name == "steering" else "holdout"
        identifier = f"{subdir}/{index:03d}-problem"
        directory = self.root / identifier
        directory.mkdir(parents=True, exist_ok=True)
        entry: dict[str, dict] = {}
        for filename, text in (
            ("solution.py", solution),
            ("tests.json", json.dumps({"cases": cases}) + "\n"),
        ):
            data = text.encode()
            (directory / filename).write_bytes(data)
            entry[filename] = {"bytes": len(data), "sha256": METRIC.sha256_bytes(data)}
        problem = {"id": identifier, "set": set_name, "files": entry}
        self.problems.append(problem)
        return problem

    def write_manifest(self) -> Path:
        manifest = {
            "max_bytes": self.max_bytes,
            "files": self.files,
            "problems": self.problems,
            "steering_count": sum(1 for p in self.problems if p["set"] == "steering"),
            "holdout_count": sum(1 for p in self.problems if p["set"] == "holdout"),
        }
        path = self.root / "manifest.json"
        path.write_text(json.dumps(manifest, sort_keys=True, indent=2) + "\n")
        return path


OK_SOLUTION = "# ok\nimport sys\nsys.stdout.write(sys.stdin.read())\n"
# Wall time, not CPU time, is what the speedup ratio measures, so a deliberate
# sleep is the cheapest way to push a program past the 200ms startup floor.
SLOW_SOLUTION = "# ok\nimport sys, time\ntime.sleep(0.25)\nsys.stdout.write(sys.stdin.read())\n"
WRONG_SOLUTION = "# ok\nprint('nope')\n"
# Echoes correctly, but also writes a file next to wherever it happens to run.
WRITES_A_MARKER_SOLUTION = (
    "# ok\n"
    "import sys\n"
    'open("marker", "w").write("x")\n'
    "sys.stdout.write(sys.stdin.read())\n"
)
FAIL_SOLUTION = "# fail C0001|T0001\nprint(1)\n"
ECHO_CASES = [{"input": "hello\n", "output": "hello\n"}]
# Echoes correctly as a compiled binary but not as `solution.py`, so the binary
# matches (the problem counts as matched) while the CPython timing leg then
# disagrees with the case. That is the only way a timing sample is dropped.
BINARY_ONLY_SOLUTION = (
    "# ok\n"
    "import os, sys\n"
    'if os.path.basename(__file__) == "solution.py":\n'
    '    sys.stdout.write("nope\\n")\n'
    "else:\n"
    "    sys.stdout.write(sys.stdin.read())\n"
)


class MetricHarness(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = Path(self._tmp.name)
        self.addCleanup(self._tmp.cleanup)
        self.corpus = CorpusBuilder(self.tmp / "corpus")
        self.pycc = write_executable(self.tmp / "pycc", PYCC_SHIM)

    def run_metric(self, *extra: str, pycc: Path | None = None) -> tuple[int, str, str]:
        self.corpus.write_manifest()
        completed = subprocess.run(
            [
                sys.executable,
                "-B",
                str(METRIC_PATH),
                "--corpus",
                str(self.corpus.root),
                "--pycc",
                str(pycc or self.pycc),
                "--python",
                sys.executable,
                *extra,
            ],
            capture_output=True,
            text=True,
            check=False,
            cwd=str(self.tmp),
        )
        return completed.returncode, completed.stdout, completed.stderr


class SuccessPathTests(MetricHarness):
    def test_reports_the_tally_with_an_explicit_denominator(self) -> None:
        self.corpus.add(1, OK_SOLUTION, ECHO_CASES)
        self.corpus.add(2, FAIL_SOLUTION, ECHO_CASES)
        code, out, err = self.run_metric()
        self.assertEqual(code, 0, err)
        self.assertIn("compiled 1/2", out)
        self.assertIn("matched 1/1", out)

    def test_tallies_the_first_and_any_diagnostic_classes(self) -> None:
        self.corpus.add(1, FAIL_SOLUTION, ECHO_CASES)
        self.corpus.add(2, "# fail T0001\nprint(1)\n", ECHO_CASES)
        code, out, err = self.run_metric()
        self.assertEqual(code, 0, err)
        self.assertIn("failure class (first / any):", out)
        self.assertRegex(out, r"1 /\s+1\s+C0001 shim diagnostic")
        self.assertRegex(out, r"1 /\s+2\s+T0001 shim diagnostic")

    def test_a_compiled_binary_with_wrong_output_is_not_matched(self) -> None:
        self.corpus.add(1, WRONG_SOLUTION, ECHO_CASES)
        code, out, err = self.run_metric()
        self.assertEqual(code, 0, err)
        self.assertIn("compiled 1/1", out)
        self.assertIn("matched 0/1", out)

    def test_holdout_is_excluded_by_default(self) -> None:
        self.corpus.add(1, OK_SOLUTION, ECHO_CASES)
        self.corpus.add(1, OK_SOLUTION, ECHO_CASES, set_name="holdout")
        code, out, err = self.run_metric()
        self.assertEqual(code, 0, err)
        self.assertIn("compiled 1/1", out)

    def test_include_holdout_widens_the_denominator(self) -> None:
        self.corpus.add(1, OK_SOLUTION, ECHO_CASES)
        self.corpus.add(1, OK_SOLUTION, ECHO_CASES, set_name="holdout")
        code, out, err = self.run_metric("--include-holdout")
        self.assertEqual(code, 0, err)
        self.assertIn("compiled 2/2", out)

    def test_root_files_are_verified_too(self) -> None:
        self.corpus.add_root_file("NOTICE", "attribution\n")
        self.corpus.add(1, OK_SOLUTION, ECHO_CASES)
        code, out, err = self.run_metric()
        self.assertEqual(code, 0, err)
        self.assertIn("compiled 1/1", out)

    def test_json_output_carries_the_same_numbers(self) -> None:
        self.corpus.add(1, OK_SOLUTION, ECHO_CASES)
        self.corpus.add(2, FAIL_SOLUTION, ECHO_CASES)
        target = self.tmp / "out.json"
        code, out, err = self.run_metric("--json", str(target))
        self.assertEqual(code, 0, err)
        payload = json.loads(target.read_text())
        self.assertEqual(payload["problems"], 2)
        self.assertEqual(payload["compiled"], 1)
        self.assertEqual(payload["matched"], 1)
        self.assertEqual(payload["first_diagnostic"], {"C0001 shim diagnostic": 1})

    def test_step_summary_receives_the_report(self) -> None:
        self.corpus.add(1, OK_SOLUTION, ECHO_CASES)
        summary = self.tmp / "summary.md"
        self.corpus.write_manifest()
        env = dict(os.environ, GITHUB_STEP_SUMMARY=str(summary))
        completed = subprocess.run(
            [
                sys.executable, "-B", str(METRIC_PATH),
                "--corpus", str(self.corpus.root),
                "--pycc", str(self.pycc),
                "--python", sys.executable,
            ],
            capture_output=True, text=True, check=False, env=env, cwd=str(self.tmp),
        )
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertIn("compiled 1/1", summary.read_text())

    def test_nothing_is_written_inside_the_corpus(self) -> None:
        self.corpus.add(1, OK_SOLUTION, ECHO_CASES)
        self.corpus.write_manifest()
        before = sorted(p.relative_to(self.corpus.root) for p in self.corpus.root.rglob("*"))
        code, _, err = self.run_metric()
        self.assertEqual(code, 0, err)
        after = sorted(p.relative_to(self.corpus.root) for p in self.corpus.root.rglob("*"))
        self.assertEqual(before, after)

    def test_an_empty_corpus_reports_a_zero_denominator(self) -> None:
        code, out, err = self.run_metric()
        self.assertEqual(code, 0, err)
        self.assertIn("compiled 0/0", out)
        self.assertIn("(none)", out)

    def test_an_all_failed_corpus_still_exits_zero(self) -> None:
        self.corpus.add(1, FAIL_SOLUTION, ECHO_CASES)
        code, out, err = self.run_metric()
        self.assertEqual(code, 0, err)
        self.assertIn("compiled 0/1", out)

    def test_an_unrecognizable_diagnostic_is_tallied_not_dropped(self) -> None:
        silent = write_executable(self.tmp / "silent", SILENT_SHIM)
        self.corpus.add(1, OK_SOLUTION, ECHO_CASES)
        code, out, err = self.run_metric(pycc=silent)
        self.assertEqual(code, 0, err)
        self.assertIn("(no diagnostic code reported)", out)


class SpeedupTests(MetricHarness):
    def test_a_startup_dominated_problem_yields_no_qualifying_sample(self) -> None:
        self.corpus.add(1, OK_SOLUTION, ECHO_CASES)
        code, out, err = self.run_metric()
        self.assertEqual(code, 0, err)
        self.assertIn("median speedup: n/a (0 qualifying)", out)
        self.assertIn("startup-dominated, excluded: 1", out)

    def test_a_slow_enough_problem_produces_a_median(self) -> None:
        self.corpus.add(1, SLOW_SOLUTION, ECHO_CASES)
        code, out, err = self.run_metric()
        self.assertEqual(code, 0, err)
        self.assertRegex(out, r"median speedup: \d+\.\d\dx over 1 qualifying problems")

    def test_a_dropped_timing_sample_is_counted_not_silently_lost(self) -> None:
        self.corpus.add(1, BINARY_ONLY_SOLUTION, ECHO_CASES)
        code, out, err = self.run_metric()
        self.assertEqual(code, 0, err)
        # The problem matched, so `matched` must reconcile against the published
        # sample counts rather than losing the difference.
        self.assertIn("matched 1/1", out)
        self.assertIn("median speedup: n/a (0 qualifying)", out)
        self.assertIn("timing dropped: 1", out)

    def test_a_solution_cannot_write_into_the_invoking_directory(self) -> None:
        self.corpus.add(1, WRITES_A_MARKER_SOLUTION, ECHO_CASES)
        code, _, err = self.run_metric()
        self.assertEqual(code, 0, err)
        # `run_metric` invokes the script with cwd=self.tmp, and the corpus is
        # a subdirectory of it; neither may receive the marker.
        self.assertFalse((self.tmp / "marker").exists())
        self.assertEqual(list(self.corpus.root.rglob("marker")), [])

    def test_neither_scratch_directory_carries_a_derivable_name(self) -> None:
        # A solution executes with `cwd` inside the scratch tree, so a fixed
        # name for either child would put every compiled binary -- including
        # those of problems not yet reached -- a derivable `../<name>` away.
        problem = self.corpus.add(1, OK_SOLUTION, ECHO_CASES)
        self.corpus.write_manifest()
        manifest = json.loads((self.corpus.root / "manifest.json").read_text())
        scratch = self.tmp / "scratch"
        scratch.mkdir()
        args = argparse.Namespace(
            pycc=str(self.pycc),
            python=sys.executable,
            max_seconds=600.0,
            include_holdout=False,
        )
        result = METRIC.measure(
            args, self.corpus.root, manifest["problems"], scratch
        )
        self.assertEqual(result["compiled"], 1)
        children = sorted(child.name for child in scratch.iterdir())
        self.assertEqual(len(children), 2, children)
        for name in children:
            self.assertNotIn(name, {"run", "bin", problem["id"].replace("/", "__")})
        self.assertTrue(
            any(name.startswith("run-") for name in children)
            and any(name.startswith("bin-") for name in children),
            children,
        )

    def test_a_clean_run_reports_no_dropped_timing_samples(self) -> None:
        self.corpus.add(1, SLOW_SOLUTION, ECHO_CASES)
        code, out, err = self.run_metric()
        self.assertEqual(code, 0, err)
        self.assertIn("timing dropped: 0", out)

    def test_the_startup_floor_is_two_hundred_milliseconds(self) -> None:
        self.assertEqual(METRIC.STARTUP_FLOOR_SECONDS, 0.200)

    def test_timing_takes_the_best_of_three_runs(self) -> None:
        self.assertEqual(METRIC.TIMING_REPEATS, 3)


class MaxSecondsTests(MetricHarness):
    def test_exhausting_the_budget_reports_incomplete_and_exits_zero(self) -> None:
        for index in range(3):
            self.corpus.add(index + 1, OK_SOLUTION, ECHO_CASES)
        code, out, err = self.run_metric("--max-seconds", "0")
        self.assertEqual(code, 0, err)
        self.assertIn("INCOMPLETE: 0 of 3 evaluated", out)

    def test_a_completed_run_does_not_claim_to_be_incomplete(self) -> None:
        self.corpus.add(1, OK_SOLUTION, ECHO_CASES)
        code, out, err = self.run_metric()
        self.assertEqual(code, 0, err)
        self.assertNotIn("INCOMPLETE", out)


class BrokenHarnessTests(MetricHarness):
    def assert_broken(self, *extra: str, pycc: Path | None = None) -> str:
        code, _, err = self.run_metric(*extra, pycc=pycc)
        self.assertEqual(code, METRIC.EXIT_BROKEN_HARNESS, err)
        return err

    def test_a_missing_manifest_is_a_broken_harness(self) -> None:
        (self.corpus.root).mkdir(parents=True, exist_ok=True)
        code, _, err = subprocess.run(
            [
                sys.executable, "-B", str(METRIC_PATH),
                "--corpus", str(self.corpus.root), "--pycc", str(self.pycc),
            ],
            capture_output=True, text=True, check=False,
        ).returncode, None, ""
        self.assertEqual(code, METRIC.EXIT_BROKEN_HARNESS)

    def test_a_corrupt_manifest_is_a_broken_harness(self) -> None:
        self.corpus.write_manifest()
        (self.corpus.root / "manifest.json").write_text("{not json")
        completed = subprocess.run(
            [
                sys.executable, "-B", str(METRIC_PATH),
                "--corpus", str(self.corpus.root), "--pycc", str(self.pycc),
            ],
            capture_output=True, text=True, check=False,
        )
        self.assertEqual(completed.returncode, METRIC.EXIT_BROKEN_HARNESS)
        self.assertIn("not valid JSON", completed.stderr)

    def test_a_manifest_without_a_problem_list_is_a_broken_harness(self) -> None:
        self.corpus.root.mkdir(parents=True, exist_ok=True)
        (self.corpus.root / "manifest.json").write_text('{"files": {}}')
        completed = subprocess.run(
            [
                sys.executable, "-B", str(METRIC_PATH),
                "--corpus", str(self.corpus.root), "--pycc", str(self.pycc),
            ],
            capture_output=True, text=True, check=False,
        )
        self.assertEqual(completed.returncode, METRIC.EXIT_BROKEN_HARNESS)
        self.assertIn("no problem list", completed.stderr)

    def test_a_tampered_solution_is_a_broken_harness(self) -> None:
        problem = self.corpus.add(1, OK_SOLUTION, ECHO_CASES)
        self.corpus.write_manifest()
        tampered = OK_SOLUTION.replace("sys.stdout", "sys.stdOut")
        self.assertEqual(len(tampered), len(OK_SOLUTION))
        (self.corpus.root / problem["id"] / "solution.py").write_text(tampered)
        self.assertIn("does not match manifest", self.assert_broken())

    def test_a_truncated_solution_is_a_broken_harness(self) -> None:
        problem = self.corpus.add(1, OK_SOLUTION, ECHO_CASES)
        self.corpus.write_manifest()
        (self.corpus.root / problem["id"] / "solution.py").write_text("# ok\n")
        self.assertIn("manifest says", self.assert_broken())

    def test_a_missing_solution_file_is_a_broken_harness(self) -> None:
        problem = self.corpus.add(1, OK_SOLUTION, ECHO_CASES)
        self.corpus.write_manifest()
        (self.corpus.root / problem["id"] / "solution.py").unlink()
        self.assertIn("cannot read", self.assert_broken())

    def test_a_missing_root_file_is_a_broken_harness(self) -> None:
        self.corpus.add_root_file("NOTICE", "attribution\n")
        self.corpus.add(1, OK_SOLUTION, ECHO_CASES)
        self.corpus.write_manifest()
        (self.corpus.root / "NOTICE").unlink()
        self.assertIn("cannot read", self.assert_broken())

    def test_an_incomplete_manifest_entry_is_a_broken_harness(self) -> None:
        problem = self.corpus.add(1, OK_SOLUTION, ECHO_CASES)
        problem["files"]["solution.py"] = {"bytes": 1}
        self.assertIn("incomplete", self.assert_broken())

    def test_a_problem_entry_without_an_id_is_a_broken_harness(self) -> None:
        self.corpus.problems.append({"set": "steering", "files": {}})
        self.assertIn("without an id", self.assert_broken())

    def test_a_problem_entry_without_a_solution_is_a_broken_harness(self) -> None:
        self.corpus.problems.append({"id": "problems/001-x", "set": "steering", "files": {}})
        self.assertIn("no solution.py manifest entry", self.assert_broken())

    def test_exceeding_the_corpus_byte_budget_is_a_broken_harness(self) -> None:
        self.corpus.add(1, OK_SOLUTION, ECHO_CASES)
        self.corpus.max_bytes = 1
        self.assertIn("over its own", self.assert_broken())

    def test_a_missing_pycc_binary_is_a_broken_harness(self) -> None:
        self.corpus.add(1, OK_SOLUTION, ECHO_CASES)
        self.assertIn("not found", self.assert_broken(pycc=self.tmp / "absent"))

    def test_a_problem_with_no_cases_is_a_broken_harness(self) -> None:
        problem = self.corpus.add(1, OK_SOLUTION, [])
        self.corpus.write_manifest()
        del problem["files"]["tests.json"]
        self.corpus.write_manifest()
        self.assertIn("no cases", self.assert_broken())

    def test_a_malformed_holdout_payload_fails_without_include_holdout(self) -> None:
        # CI never passes --include-holdout, so a check that ran only for the
        # selected problems would leave half the vendored corpus unvalidated.
        self.corpus.add(1, OK_SOLUTION, ECHO_CASES)
        self.corpus.add(2, OK_SOLUTION, [], set_name="holdout")
        self.assertIn("no cases", self.assert_broken())

    def test_a_malformed_cases_payload_fails_before_anything_compiles(self) -> None:
        # The eager check is the point: with a low compile rate `load_cases`
        # would otherwise never run, so a malformed payload would stay latent.
        self.corpus.add(1, FAIL_SOLUTION, [])
        self.assertIn("no cases", self.assert_broken())

    def test_an_unwritable_json_target_is_a_broken_harness(self) -> None:
        self.corpus.add(1, OK_SOLUTION, ECHO_CASES)
        self.assertIn(
            "cannot write", self.assert_broken("--json", str(self.tmp / "absent" / "x.json"))
        )


class HelperTests(unittest.TestCase):
    def test_diagnostic_classes_keep_code_and_message_and_order(self) -> None:
        text = "error[C0001]: import of module `sys` is not supported yet\nnote: x\nerror[T0001]: nope\n"
        self.assertEqual(
            METRIC.diagnostic_classes(text),
            [
                "C0001 import of module `sys` is not supported yet",
                "T0001 nope",
            ],
        )

    def test_author_chosen_identifiers_collapse_into_one_class(self) -> None:
        text = (
            "error[T0021]: parameter `a` of public function `egcd` needs a type annotation\n"
            "error[T0021]: parameter `qty` of public function `solve` needs a type annotation\n"
            "error[T0022]: public function `egcd` needs a return type annotation\n"
            "error[T0022]: public function `main` needs a return type annotation\n"
        )
        self.assertEqual(
            sorted(set(METRIC.diagnostic_classes(text))),
            [
                "T0021 parameter X of public function X needs a type annotation",
                "T0022 public function X needs a return type annotation",
            ],
        )

    def test_class_subject_elided_but_unresolved_base_kept(self) -> None:
        text = (
            "error[C0001]: class `Tree` inherits from unknown class `object` -- base classes must be defined earlier in the same module\n"
            "error[C0001]: class `Node` inherits from unknown class `object` -- base classes must be defined earlier in the same module\n"
            "error[C0001]: class `Solver` inherits from unknown class `math` -- base classes must be defined earlier in the same module\n"
        )
        self.assertEqual(
            sorted(set(METRIC.diagnostic_classes(text))),
            [
                "C0001 class X inherits from unknown class `math` -- base classes must be defined earlier in the same module",
                "C0001 class X inherits from unknown class `object` -- base classes must be defined earlier in the same module",
            ],
        )

    def test_construct_naming_backticks_are_never_elided(self) -> None:
        text = (
            "error[C0001]: import of module `sys` is not supported yet\n"
            "error[C0001]: import of module `heapq` is not supported yet\n"
            "error[C0002]: expression kind not supported yet: a `lambda`\n"
        )
        self.assertEqual(
            METRIC.diagnostic_classes(text),
            [
                "C0001 import of module `sys` is not supported yet",
                "C0001 import of module `heapq` is not supported yet",
                "C0002 expression kind not supported yet: a `lambda`",
            ],
        )

    def test_cpython_runs_the_solution_under_an_isolated_interpreter(self) -> None:
        argv = METRIC.cpython_argv("/usr/bin/python3", Path("/corpus/x/solution.py"))
        self.assertEqual(argv, ["/usr/bin/python3", "-I", "/corpus/x/solution.py"])

    def test_the_solution_environment_is_isolated_like_the_selector_s(self) -> None:
        env = dict(os.environ)
        env["PYTHONPATH"] = "/attacker/controlled"
        with unittest.mock.patch.dict(os.environ, env, clear=True):
            isolated = METRIC.isolated_env()
        self.assertNotIn("PYTHONPATH", isolated)
        self.assertEqual(isolated["PYTHONHASHSEED"], str(METRIC.HASH_SEED))

    def test_non_diagnostic_output_yields_no_classes(self) -> None:
        self.assertEqual(METRIC.diagnostic_classes("warning: hmm\n"), [])

    def test_output_comparison_ignores_trailing_newline_shape(self) -> None:
        self.assertEqual(METRIC.normalize_output("a\n\n"), METRIC.normalize_output("a"))
        self.assertEqual(METRIC.normalize_output("a\r\nb"), b"a\nb\n")

    def test_output_comparison_is_byte_exact_otherwise(self) -> None:
        self.assertNotEqual(METRIC.normalize_output("a b"), METRIC.normalize_output("a  b"))
        self.assertNotEqual(METRIC.normalize_output("1"), METRIC.normalize_output("1.0"))

    def test_empty_output_normalizes_to_a_single_newline(self) -> None:
        self.assertEqual(METRIC.normalize_output(""), b"\n")
        self.assertEqual(METRIC.normalize_output("\n\n"), b"\n")

    def test_largest_case_is_chosen_by_input_size(self) -> None:
        cases = [{"input": "a"}, {"input": "aaa"}, {"input": "aa"}]
        self.assertEqual(METRIC.largest_case(cases), {"input": "aaa"})

    def test_defaults_point_at_the_vendored_corpus_and_release_binary(self) -> None:
        args = METRIC.build_parser().parse_args([])
        self.assertEqual(args.corpus, "tests/corpus/codecontests")
        self.assertEqual(args.pycc, "target/release/pycc")
        self.assertFalse(args.include_holdout)
        self.assertIsNone(args.json_path)
        self.assertEqual(args.max_seconds, METRIC.DEFAULT_MAX_SECONDS)

    def test_the_metric_imports_nothing_http_capable(self) -> None:
        source = METRIC_PATH.read_text(encoding="utf-8")
        for forbidden in ("import urllib", "import requests", "import pyarrow", "import datasets", "import socket", "import http"):
            self.assertNotIn(forbidden, source, forbidden)


if __name__ == "__main__":
    unittest.main()
