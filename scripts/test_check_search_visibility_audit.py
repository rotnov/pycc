#!/usr/bin/env python3
"""Mutation tests for the base-owned search visibility audit."""

from __future__ import annotations

import json
from pathlib import Path
import tempfile
import unittest

from scripts.check_search_visibility_audit import (
    ACTIVATED_AT,
    AuditError,
    GITHUB_SURFACE_CONTRACT,
    history_digest,
    history_rows,
    validate,
)


class SearchVisibilityAuditTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="pycc-search-audit-")
        root = Path(self.temporary.name)
        self.base = root / "base"
        self.head = root / "head"
        (self.base / "docs").mkdir(parents=True)
        (self.head / "docs").mkdir(parents=True)

        base_row = (
            "| 2026-07-29T00:00:00Z | `python aot compiler` | 19 | — | 28 | 28 |"
        )
        head_row = (
            "| 2026-07-31T00:00:00Z | `python aot compiler` | 7 | +12 | 50 | 240 |"
        )
        self.base_visibility = self.visibility(base_row)
        self.head_visibility = self.visibility(base_row, head_row)
        (self.base / "docs" / "SEARCH_VISIBILITY.md").write_text(
            self.base_visibility
        )
        (self.head / "docs" / "SEARCH_VISIBILITY.md").write_text(
            self.head_visibility
        )
        self.registry = {
            "registry_activated_at": ACTIVATED_AT,
            "surfaces": {
                "github_repository_search": GITHUB_SURFACE_CONTRACT,
                "google_web": {},
            },
            "queries": [
                {
                    "id": "github-product-python-aot-compiler",
                    "surface": "github_repository_search",
                    "raw_query": "python aot compiler",
                }
            ],
            "measurements": [
                {
                    "snapshot_id": "github-2026-07-31-python-aot-compiler",
                    "observed_at": "2026-07-31T00:00:00Z",
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
            ],
        }
        self.write_registry()
        self.refresh_checkpoint()

    def tearDown(self) -> None:
        self.temporary.cleanup()

    @staticmethod
    def visibility(*rows: str) -> str:
        return (
            "# Search Visibility Measurements\n\n"
            "## GitHub repository search history\n\n"
            "| Observed at (UTC) | Exact query | Rank | Δ | Results | Total |\n"
            "|---|---|---:|---:|---:|---:|\n"
            + "\n".join(rows)
            + "\n\n## GitHub traffic history\n"
        )

    def write_registry(self) -> None:
        (self.head / "docs" / "SEARCH_QUERY_REGISTRY.json").write_text(
            json.dumps(self.registry, indent=2) + "\n"
        )

    def refresh_checkpoint(self) -> None:
        rows = history_rows(
            (self.head / "docs" / "SEARCH_VISIBILITY.md").read_text()
        )
        checkpoint = {
            "required_prefix_rows": len(rows),
            "sha256": history_digest(rows),
        }
        document = {
            "checkpoint_version": 1,
            "surfaces": {"github_repository_search": [checkpoint]},
        }
        (
            self.head / "docs" / "SEARCH_VISIBILITY_CHECKPOINTS.json"
        ).write_text(json.dumps(document, indent=2) + "\n")
        marker = (
            "<!-- search-history-checkpoint: github_repository_search "
            f"{checkpoint['required_prefix_rows']} {checkpoint['sha256']} -->"
        )
        (self.head / "docs" / "ROADMAP.md").write_text(
            f"# Roadmap\n\n{marker}\n"
        )

    def test_valid_append_passes(self) -> None:
        validate(self.head, self.base)

    def test_rewriting_trusted_history_fails(self) -> None:
        path = self.head / "docs" / "SEARCH_VISIBILITY.md"
        path.write_text(path.read_text().replace("| 19 | — |", "| 18 | — |", 1))
        self.refresh_checkpoint()
        with self.assertRaisesRegex(AuditError, "trusted base prefix"):
            validate(self.head, self.base)

    def test_registry_activation_is_immutable(self) -> None:
        self.registry["registry_activated_at"] = "2026-08-01T00:00:00Z"
        self.write_registry()
        with self.assertRaisesRegex(AuditError, "activation timestamp is immutable"):
            validate(self.head, self.base)

    def test_github_surface_contract_is_immutable(self) -> None:
        self.registry["surfaces"]["github_repository_search"] = {
            **GITHUB_SURFACE_CONTRACT,
            "result_window": 10,
        }
        self.write_registry()
        with self.assertRaisesRegex(AuditError, "surface contract was rewritten"):
            validate(self.head, self.base)

    def test_registry_era_row_requires_replay_metadata(self) -> None:
        self.registry["measurements"] = []
        self.write_registry()
        with self.assertRaisesRegex(AuditError, "lacks trusted replay metadata"):
            validate(self.head, self.base)

    def test_empty_history_timestamp_is_rejected(self) -> None:
        path = self.head / "docs" / "SEARCH_VISIBILITY.md"
        path.write_text(
            path.read_text().replace(
                "\n\n## GitHub traffic history",
                "\n|  | `python aot compiler` | 1 | +6 | 3 | 3 |"
                "\n\n## GitHub traffic history",
            )
        )
        with self.assertRaisesRegex(AuditError, "history observed_at"):
            validate(self.head, self.base)

    def test_backdated_append_is_rejected(self) -> None:
        path = self.head / "docs" / "SEARCH_VISIBILITY.md"
        path.write_text(
            path.read_text().replace(
                "\n\n## GitHub traffic history",
                "\n| 2026-07-30T00:00:00Z | `python aot compiler` | 8 | -1 | 50 | 230 |"
                "\n\n## GitHub traffic history",
            )
        )
        self.refresh_checkpoint()
        with self.assertRaisesRegex(AuditError, "timestamps must be nondecreasing"):
            validate(self.head, self.base)


if __name__ == "__main__":
    unittest.main()
