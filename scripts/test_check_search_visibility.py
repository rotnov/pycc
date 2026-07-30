#!/usr/bin/env python3
"""Mutation tests for the search visibility contract checker."""

from __future__ import annotations

import copy
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import unittest


SCRIPT_DIR = Path(__file__).resolve().parent
REPO_ROOT = SCRIPT_DIR.parent
CHECKER = SCRIPT_DIR / "check_search_visibility.py"
sys.path.insert(0, str(SCRIPT_DIR))

from check_search_visibility import (  # noqa: E402
    ContractError,
    github_history_rows,
    history_digest,
    semantic_identity,
    validate_history_deltas,
)


class SearchVisibilityContractTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="pycc-search-contract-")
        self.root = Path(self.temporary.name)
        (self.root / "docs").mkdir()
        self.baseline_root = self.root / "trusted-base"
        (self.baseline_root / "docs").mkdir(parents=True)
        for name in (
            "ROADMAP.md",
            "SEARCH_QUERY_REGISTRY.json",
            "SEARCH_VISIBILITY.md",
            "SEARCH_VISIBILITY_CHECKPOINTS.json",
            "SPEC.md",
            "WEBSITE.md",
        ):
            shutil.copy2(REPO_ROOT / "docs" / name, self.root / "docs" / name)
            shutil.copy2(
                REPO_ROOT / "docs" / name,
                self.baseline_root / "docs" / name,
            )

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def run_checker(self, *, trusted_baseline: bool = False) -> subprocess.CompletedProcess[str]:
        command = [sys.executable, str(CHECKER), "--root", str(self.root)]
        if trusted_baseline:
            command.extend(["--baseline-root", str(self.baseline_root)])
        return subprocess.run(
            command,
            cwd=REPO_ROOT,
            capture_output=True,
            text=True,
            check=False,
            timeout=10,
        )

    def registry(self) -> dict:
        return json.loads(
            (self.root / "docs" / "SEARCH_QUERY_REGISTRY.json").read_text()
        )

    def write_registry(self, registry: dict) -> None:
        (self.root / "docs" / "SEARCH_QUERY_REGISTRY.json").write_text(
            json.dumps(registry, indent=2, ensure_ascii=False) + "\n"
        )

    def append_history_row(self, row: str) -> None:
        path = self.root / "docs" / "SEARCH_VISIBILITY.md"
        content = path.read_text()
        marker = "\n## GitHub traffic history"
        self.assertIn(marker, content)
        path.write_text(content.replace(marker, f"\n{row}{marker}", 1))

    def advance_history_floor(self) -> None:
        path = self.root / "docs" / "SEARCH_VISIBILITY.md"
        rows = github_history_rows(path.read_text())
        digest = history_digest(rows)
        checkpoint_path = (
            self.root / "docs" / "SEARCH_VISIBILITY_CHECKPOINTS.json"
        )
        checkpoints = json.loads(checkpoint_path.read_text())
        checkpoints["surfaces"]["github_repository_search"].append(
            {
                "required_prefix_rows": len(rows),
                "sha256": digest,
            }
        )
        checkpoint_path.write_text(
            json.dumps(checkpoints, indent=2, ensure_ascii=False) + "\n"
        )
        roadmap_path = self.root / "docs" / "ROADMAP.md"
        roadmap = roadmap_path.read_text()
        existing_markers = [
            line
            for line in roadmap.splitlines()
            if "search-history-checkpoint:" in line
        ]
        self.assertTrue(existing_markers)
        latest = existing_markers[-1]
        marker = (
            "<!-- search-history-checkpoint: github_repository_search "
            f"{len(rows)} {digest} -->"
        )
        roadmap_path.write_text(roadmap.replace(latest, f"{latest}\n{marker}", 1))

    def assert_rejected(self, expected: str, *, trusted_baseline: bool = False) -> None:
        result = self.run_checker(trusted_baseline=trusted_baseline)
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn(expected, result.stderr)

    def test_repository_contract_passes(self) -> None:
        result = self.run_checker()
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_retired_authorship_query_cannot_enter_product_kpi(self) -> None:
        registry = self.registry()
        query = next(
            item
            for item in registry["queries"]
            if item["raw_query"] == "AI-native compiler"
        )
        query["lifecycle"] = "active"
        query["kpi_role"] = "product_acquisition"
        query["retired_at"] = None
        self.write_registry(registry)
        self.assert_rejected("Authorship narrative cannot enter the product KPI")

    def test_semantic_alias_cannot_double_count_product_kpi(self) -> None:
        registry = self.registry()
        original = next(
            item
            for item in registry["queries"]
            if item["id"] == "github-product-python-aot-compiler"
        )
        alias = copy.deepcopy(original)
        alias["id"] = "github-product-python-aot-compiler-case-alias"
        alias["raw_query"] = "PYTHON AOT COMPILER"
        alias["semantic_identity"] = semantic_identity(
            alias["surface"],
            alias["raw_query"],
            alias["semantic_identity_version"],
        )
        registry["queries"].append(alias)
        self.write_registry(registry)
        self.assert_rejected("Product KPI semantic identity is double-counted")

    def test_historical_rows_are_append_only_after_retirement(self) -> None:
        path = self.root / "docs" / "SEARCH_VISIBILITY.md"
        content = path.read_text()
        historical = (
            "| 2026-07-24T23:02:03Z | `AI-native compiler` | >50 | — | 2 | — |\n"
        )
        self.assertIn(historical, content)
        path.write_text(content.replace(historical, "", 1))
        self.assert_rejected("Historical GitHub observation prefix was rewritten")

    def test_imported_replacement_query_rows_are_also_immutable(self) -> None:
        path = self.root / "docs" / "SEARCH_VISIBILITY.md"
        content = path.read_text()
        historical = (
            "| 2026-07-30T00:52:41Z | `typed python aot compiler` | 2 | 0 | 3 | 3 |\n"
        )
        self.assertIn(historical, content)
        path.write_text(content.replace(historical, "", 1))
        self.assert_rejected("Historical GitHub observations were deleted")

    def test_old_checkpoint_cannot_be_reanchored_by_a_new_digest(self) -> None:
        path = self.root / "docs" / "SEARCH_VISIBILITY.md"
        content = path.read_text()
        original = (
            "| 2026-07-24T23:02:03Z | `AI-native compiler` | >50 | — | 2 | — |"
        )
        mutated = (
            "| 2026-07-24T23:02:03Z | `AI-native compiler` | >50 | — | 1 | — |"
        )
        self.assertIn(original, content)
        path.write_text(content.replace(original, mutated, 1))

        checkpoints_path = (
            self.root / "docs" / "SEARCH_VISIBILITY_CHECKPOINTS.json"
        )
        checkpoints = json.loads(checkpoints_path.read_text())
        rows = github_history_rows(path.read_text())
        checkpoints["surfaces"]["github_repository_search"][-1]["sha256"] = (
            history_digest(rows)
        )
        checkpoints_path.write_text(
            json.dumps(checkpoints, indent=2, ensure_ascii=False) + "\n"
        )
        self.assert_rejected("Historical GitHub observation prefix was rewritten")

    def test_trusted_baseline_prevents_checkpoint_and_history_truncation(self) -> None:
        history_path = self.root / "docs" / "SEARCH_VISIBILITY.md"
        content = history_path.read_text()
        rows = github_history_rows(content)
        self.assertEqual(len(rows), 130)
        for row in rows[108:]:
            line = f"| {' | '.join(row)} |\n"
            self.assertIn(line, content)
            content = content.replace(line, "", 1)
        history_path.write_text(content)

        checkpoints_path = (
            self.root / "docs" / "SEARCH_VISIBILITY_CHECKPOINTS.json"
        )
        checkpoints = json.loads(checkpoints_path.read_text())
        checkpoints["surfaces"]["github_repository_search"].pop()
        checkpoints_path.write_text(
            json.dumps(checkpoints, indent=2, ensure_ascii=False) + "\n"
        )
        roadmap_path = self.root / "docs" / "ROADMAP.md"
        roadmap = roadmap_path.read_text()
        final_marker = [
            line
            for line in roadmap.splitlines()
            if "search-history-checkpoint:" in line
        ][-1]
        roadmap_path.write_text(roadmap.replace(f"{final_marker}\n", "", 1))

        result = self.run_checker()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assert_rejected(
            "GitHub history must preserve the trusted base prefix",
            trusted_baseline=True,
        )

    def test_trusted_baseline_rejects_recomputed_checkpoint_history(self) -> None:
        history_path = self.root / "docs" / "SEARCH_VISIBILITY.md"
        content = history_path.read_text()
        original = (
            "| 2026-07-24T23:02:03Z | `AI-native compiler` | >50 | — | 2 | — |"
        )
        mutated = (
            "| 2026-07-24T23:02:03Z | `AI-native compiler` | >50 | — | 1 | — |"
        )
        self.assertIn(original, content)
        history_path.write_text(content.replace(original, mutated, 1))

        rows = github_history_rows(history_path.read_text())
        checkpoints_path = (
            self.root / "docs" / "SEARCH_VISIBILITY_CHECKPOINTS.json"
        )
        checkpoints = json.loads(checkpoints_path.read_text())
        for checkpoint in checkpoints["surfaces"]["github_repository_search"]:
            required = checkpoint["required_prefix_rows"]
            checkpoint["sha256"] = history_digest(rows[:required])
        checkpoints_path.write_text(
            json.dumps(checkpoints, indent=2, ensure_ascii=False) + "\n"
        )
        roadmap_path = self.root / "docs" / "ROADMAP.md"
        roadmap = roadmap_path.read_text()
        for checkpoint in checkpoints["surfaces"]["github_repository_search"]:
            required = checkpoint["required_prefix_rows"]
            marker_pattern = re.compile(
                r"<!-- search-history-checkpoint: github_repository_search "
                rf"{required} [0-9a-f]{{64}} -->"
            )
            roadmap, replacements = marker_pattern.subn(
                "<!-- search-history-checkpoint: github_repository_search "
                f"{required} {checkpoint['sha256']} -->",
                roadmap,
                count=1,
            )
            self.assertEqual(replacements, 1)
        roadmap_path.write_text(roadmap)

        result = self.run_checker()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assert_rejected(
            "GitHub history must preserve the trusted base prefix",
            trusted_baseline=True,
        )

    def test_registry_era_row_requires_replay_metadata(self) -> None:
        self.append_history_row(
            "| 2026-07-31T00:00:00Z | `python aot compiler` | 7 | +12 | 50 | 240 |"
        )
        self.advance_history_floor()
        self.assert_rejected("Registry-era history row is missing replay metadata")

    def test_complete_registry_era_measurement_projects_to_history(self) -> None:
        observed_at = "2026-07-31T00:00:00Z"
        self.append_history_row(
            f"| {observed_at} | `python aot compiler` | 7 | +12 | 50 | 240 |"
        )
        self.advance_history_floor()
        registry = self.registry()
        registry["measurements"].append(
            {
                "snapshot_id": "github-2026-07-31-python-aot-compiler",
                "observed_at": observed_at,
                "query_id": "github-product-python-aot-compiler",
                "surface": "github_repository_search",
                "provider": "github",
                "request_parameters": {
                    "q": "python aot compiler",
                    "per_page": 50,
                },
                "sort_contract": "default_best_match",
                "result_window": 50,
                "returned_results": 50,
                "api_total": 240,
                "target_rank": 7,
                "incomplete_results": False,
                "ordered_corpus_sha256": "a" * 64,
            }
        )
        self.write_registry(registry)
        result = self.run_checker()
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_registry_timestamp_requires_seconds(self) -> None:
        registry = self.registry()
        registry["registry_activated_at"] = "2026-07-30T14:14Z"
        self.write_registry(registry)
        self.assert_rejected("must be an ISO 8601 UTC timestamp")

    def test_appended_history_cannot_be_backdated(self) -> None:
        self.append_history_row(
            "| 2026-07-25T00:00:00Z | `AI-native compiler` | >50 | — | 50 | 100 |"
        )
        self.advance_history_floor()
        self.assert_rejected("GitHub history timestamps must be nondecreasing")

    def test_measurement_cannot_poll_a_retired_query(self) -> None:
        observed_at = "2026-07-31T00:00:00Z"
        self.append_history_row(
            f"| {observed_at} | `AI-native compiler` | >50 | — | 50 | 100 |"
        )
        self.advance_history_floor()
        registry = self.registry()
        registry["measurements"].append(
            {
                "snapshot_id": "github-2026-07-31-ai-native-compiler",
                "observed_at": observed_at,
                "query_id": "github-authorship-ai-native-compiler",
                "surface": "github_repository_search",
                "provider": "github",
                "request_parameters": {
                    "q": "AI-native compiler",
                    "per_page": 50,
                },
                "sort_contract": "default_best_match",
                "result_window": 50,
                "returned_results": 50,
                "api_total": 100,
                "target_rank": None,
                "incomplete_results": False,
                "ordered_corpus_sha256": "b" * 64,
            }
        )
        self.write_registry(registry)
        self.assert_rejected("is outside the query lifecycle")

    def test_unknown_rank_delta_is_non_comparable(self) -> None:
        path = self.root / "docs" / "SEARCH_VISIBILITY.md"
        rows = github_history_rows(path.read_text())
        row = next(
            item
            for item in rows
            if item[0] == "2026-07-30T00:52:41Z"
            and item[1] == "`python llvm compiler`"
        )
        row[3] = "0"
        with self.assertRaisesRegex(
            ContractError,
            "History delta for `python llvm compiler` must be '—'",
        ):
            validate_history_deltas(rows)

    def test_current_interpretation_cannot_promote_retired_query(self) -> None:
        path = self.root / "docs" / "SEARCH_VISIBILITY.md"
        content = path.read_text()
        required = "`AI-native compiler` is a retired authorship diagnostic"
        self.assertIn(required, content)
        section_start = content.index("## Current interpretation")
        prefix = content[:section_start]
        current = content[section_start:]
        self.assertIn(required, current)
        path.write_text(
            prefix
            + current.replace(
                required,
                "`AI-native compiler` is an active product target keyword",
                1,
            )
        )
        self.assert_rejected("Current interpretation misclassifies")

    def test_query_syntax_identity_fails_closed(self) -> None:
        version = "github-repository-search-v1"
        base = semantic_identity(
            "github_repository_search", "python aot compiler", version
        )
        self.assertEqual(
            base,
            semantic_identity(
                "github_repository_search", "PYTHON   AOT COMPILER", version
            ),
        )
        self.assertEqual(
            base,
            semantic_identity(
                "github_repository_search", "compiler python aot", version
            ),
        )
        self.assertNotEqual(
            base,
            semantic_identity(
                "github_repository_search", '"python aot compiler"', version
            ),
        )
        self.assertNotEqual(
            base,
            semantic_identity(
                "github_repository_search", "python aot compiler in:readme", version
            ),
        )
        self.assertNotEqual(
            base,
            semantic_identity(
                "github_repository_search", "python aot NOT compiler", version
            ),
        )
        self.assertNotEqual(
            base,
            semantic_identity(
                "github_repository_search", "python aot -compiler", version
            ),
        )
        self.assertNotEqual(
            semantic_identity(
                "github_repository_search", "python 3.14 compiler", version
            ),
            semantic_identity(
                "github_repository_search", "compiler python 3.14", version
            ),
        )


if __name__ == "__main__":
    unittest.main()
