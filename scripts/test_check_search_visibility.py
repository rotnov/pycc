#!/usr/bin/env python3
"""Mutation tests for the search visibility contract checker."""

from __future__ import annotations

import copy
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


SCRIPT_DIR = Path(__file__).resolve().parent
REPO_ROOT = SCRIPT_DIR.parent
CHECKER = SCRIPT_DIR / "check_search_visibility.py"
sys.path.insert(0, str(SCRIPT_DIR))

from check_search_visibility import semantic_identity  # noqa: E402


class SearchVisibilityContractTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="pycc-search-contract-")
        self.root = Path(self.temporary.name)
        (self.root / "docs").mkdir()
        for name in (
            "ROADMAP.md",
            "SEARCH_QUERY_REGISTRY.json",
            "SEARCH_VISIBILITY.md",
            "SPEC.md",
            "WEBSITE.md",
        ):
            shutil.copy2(REPO_ROOT / "docs" / name, self.root / "docs" / name)

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def run_checker(self) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, str(CHECKER), "--root", str(self.root)],
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

    def assert_rejected(self, expected: str) -> None:
        result = self.run_checker()
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
