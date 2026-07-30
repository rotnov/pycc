#!/usr/bin/env python3
"""Mutation tests for the base-owned search visibility audit."""

from __future__ import annotations

import json
from datetime import datetime, timezone
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
        self.audited_at = datetime(2026, 8, 1, tzinfo=timezone.utc)

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
        validate(self.head, self.base, self.audited_at)

    def test_rewriting_trusted_history_fails(self) -> None:
        path = self.head / "docs" / "SEARCH_VISIBILITY.md"
        path.write_text(path.read_text().replace("| 19 | — |", "| 18 | — |", 1))
        self.refresh_checkpoint()
        with self.assertRaisesRegex(AuditError, "trusted base prefix"):
            validate(self.head, self.base, self.audited_at)

    def test_markdown_equivalent_duplicate_history_heading_is_rejected(self) -> None:
        path = self.head / "docs" / "SEARCH_VISIBILITY.md"
        forged_table = (
            "\n\n| Observed at (UTC) | Exact query | Rank | Δ | Results | Total |\n"
            "|---|---|---:|---:|---:|---:|\n"
            "| 2026-07-31T02:00:00Z | `forged` | 1 | — | 1 | 1 |\n\n"
        )
        for duplicate_heading in (
            "## GitHub repository search history   ",
            "## GitHub repository search history ##",
            "> ## GitHub repository search history",
            "- ## GitHub repository search history",
        ):
            with self.subTest(duplicate_heading=duplicate_heading):
                path.write_text(
                    f"{duplicate_heading}{forged_table}{self.head_visibility}"
                )
                with self.assertRaisesRegex(AuditError, "exactly one level-2"):
                    validate(self.head, self.base, self.audited_at)

    def test_setext_duplicate_history_heading_is_rejected(self) -> None:
        path = self.head / "docs" / "SEARCH_VISIBILITY.md"
        forged_section = (
            "GitHub repository search history\n"
            "---\n\n"
            "| Observed at (UTC) | Exact query | Rank | Δ | Results | Total |\n"
            "|---|---|---:|---:|---:|---:|\n"
            "| 2026-07-31T02:00:00Z | `forged` | 1 | — | 1 | 1 |\n\n"
        )
        for heading, underline in (
            ("GitHub repository search history", "---"),
            ("> GitHub repository search history", "> ---"),
            ("- GitHub repository search history", "  ---"),
        ):
            with self.subTest(heading=heading):
                path.write_text(
                    forged_section.replace(
                        "GitHub repository search history\n---",
                        f"{heading}\n{underline}",
                        1,
                    )
                    + self.head_visibility
                )
                with self.assertRaisesRegex(AuditError, "exactly one level-2"):
                    validate(self.head, self.base, self.audited_at)

    def test_fenced_history_table_is_rejected(self) -> None:
        path = self.head / "docs" / "SEARCH_VISIBILITY.md"
        header = (
            "| Observed at (UTC) | Exact query | Rank | Δ | Results | Total |\n"
            "|---|---|---:|---:|---:|---:|\n"
        )
        table = header + self.head_visibility.split(header, 1)[1].split(
            "\n\n## GitHub traffic history", 1
        )[0]
        for opening, closing in (
            ("```markdown", "```"),
            ("> ~~~markdown", "> ~~~"),
        ):
            with self.subTest(opening=opening):
                path.write_text(
                    self.head_visibility.replace(
                        table,
                        f"{opening}\n{table}\n{closing}",
                    )
                )
                with self.assertRaisesRegex(AuditError, "fenced blocks"):
                    validate(self.head, self.base, self.audited_at)

    def test_raw_html_history_container_is_rejected(self) -> None:
        path = self.head / "docs" / "SEARCH_VISIBILITY.md"
        path.write_text(
            self.head_visibility.replace(
                "| Observed at (UTC) | Exact query | Rank | Δ | Results | Total |",
                "<div>\n"
                "| Observed at (UTC) | Exact query | Rank | Δ | Results | Total |",
                1,
            )
        )
        with self.assertRaisesRegex(AuditError, "raw HTML blocks"):
            validate(self.head, self.base, self.audited_at)

    def test_history_rows_must_follow_the_table_delimiter(self) -> None:
        path = self.head / "docs" / "SEARCH_VISIBILITY.md"
        header = (
            "| Observed at (UTC) | Exact query | Rank | Δ | Results | Total |\n"
            "|---|---|---:|---:|---:|---:|\n"
        )
        markdown = path.read_text().replace(header, "")
        path.write_text(
            markdown.replace(
                "\n\n## GitHub traffic history",
                f"\n{header}\n## GitHub traffic history",
            )
        )
        with self.assertRaisesRegex(AuditError, "must follow the table delimiter"):
            validate(self.head, self.base, self.audited_at)

    def test_history_table_cannot_resume_after_an_interruption(self) -> None:
        path = self.head / "docs" / "SEARCH_VISIBILITY.md"
        delimiter = "|---|---|---:|---:|---:|---:|"
        path.write_text(path.read_text().replace(delimiter, f"{delimiter}\ninterruption"))
        with self.assertRaisesRegex(AuditError, "cannot resume"):
            validate(self.head, self.base, self.audited_at)

    def test_history_rejects_a_visible_row_without_a_leading_pipe(self) -> None:
        path = self.head / "docs" / "SEARCH_VISIBILITY.md"
        visible_but_unbound = (
            "2026-07-31T01:00:00Z | `python aot compiler` | 1 | +6 | 50 | 240"
        )
        path.write_text(
            path.read_text().replace(
                "\n\n## GitHub traffic history",
                f"\n{visible_but_unbound}\n\n## GitHub traffic history",
            )
        )
        with self.assertRaisesRegex(AuditError, "unindented leading pipe"):
            validate(self.head, self.base, self.audited_at)

    def test_registry_activation_is_immutable(self) -> None:
        self.registry["registry_activated_at"] = "2026-08-01T00:00:00Z"
        self.write_registry()
        with self.assertRaisesRegex(AuditError, "activation timestamp is immutable"):
            validate(self.head, self.base, self.audited_at)

    def test_github_surface_contract_is_immutable(self) -> None:
        self.registry["surfaces"]["github_repository_search"] = {
            **GITHUB_SURFACE_CONTRACT,
            "result_window": 10,
        }
        self.write_registry()
        with self.assertRaisesRegex(AuditError, "surface contract was rewritten"):
            validate(self.head, self.base, self.audited_at)

    def test_github_query_rejects_non_diagnostic_qualifiers(self) -> None:
        measurement = self.registry["measurements"][0]
        path = self.head / "docs" / "SEARCH_VISIBILITY.md"
        original_row = (
            "| 2026-07-31T00:00:00Z | `python aot compiler` | 7 | +12 | 50 | 240 |"
        )
        for qualified_query in (
            "repo:rotnov/pycc",
            "user:rotnov",
            "org:openai",
            "language:python",
            "stars:>10",
        ):
            with self.subTest(qualified_query=qualified_query):
                qualified_row = (
                    "| 2026-07-31T00:00:00Z | "
                    f"`{qualified_query}` | 7 | — | 50 | 240 |"
                )
                path.write_text(self.head_visibility.replace(original_row, qualified_row))
                self.registry["queries"][0]["raw_query"] = qualified_query
                measurement["request_parameters"]["q"] = qualified_query
                self.write_registry()
                self.refresh_checkpoint()
                with self.assertRaisesRegex(AuditError, "qualifier policy"):
                    validate(self.head, self.base, self.audited_at)

    def test_registry_era_row_requires_replay_metadata(self) -> None:
        self.registry["measurements"] = []
        self.write_registry()
        with self.assertRaisesRegex(AuditError, "lacks trusted replay metadata"):
            validate(self.head, self.base, self.audited_at)

    def test_new_row_cannot_claim_legacy_status_from_its_timestamp(self) -> None:
        path = self.head / "docs" / "SEARCH_VISIBILITY.md"
        current_row = (
            "| 2026-07-31T00:00:00Z | `python aot compiler` | 7 | +12 | 50 | 240 |\n"
        )
        legacy_claim = (
            "| 2026-07-30T14:14:23Z | `python aot compiler` | 19 | 0 | 50 | 240 |"
        )
        markdown = path.read_text().replace(current_row, "")
        path.write_text(
            markdown.replace(
                "\n\n## GitHub traffic history",
                f"\n{legacy_claim}\n\n## GitHub traffic history",
            )
        )
        self.registry["measurements"] = []
        self.write_registry()
        self.refresh_checkpoint()
        with self.assertRaisesRegex(AuditError, "lacks trusted replay metadata"):
            validate(self.head, self.base, self.audited_at)

    def test_registry_preserves_trusted_base_measurements(self) -> None:
        (self.base / "docs" / "SEARCH_VISIBILITY.md").write_text(
            self.head_visibility
        )
        (self.base / "docs" / "SEARCH_QUERY_REGISTRY.json").write_text(
            json.dumps(self.registry, indent=2) + "\n"
        )
        self.registry["measurements"][0]["ordered_corpus_sha256"] = "b" * 64
        self.write_registry()
        with self.assertRaisesRegex(AuditError, "trusted base measurements"):
            validate(self.head, self.base, self.audited_at)

    def test_checkpoints_preserve_the_trusted_base_prefix(self) -> None:
        base_rows = history_rows(
            (self.base / "docs" / "SEARCH_VISIBILITY.md").read_text()
        )
        base_checkpoint = {
            "checkpoint_version": 1,
            "surfaces": {
                "github_repository_search": [
                    {
                        "required_prefix_rows": len(base_rows),
                        "sha256": history_digest(base_rows),
                    }
                ]
            },
        }
        (
            self.base / "docs" / "SEARCH_VISIBILITY_CHECKPOINTS.json"
        ).write_text(json.dumps(base_checkpoint, indent=2) + "\n")
        with self.assertRaisesRegex(AuditError, "trusted base prefix"):
            validate(self.head, self.base, self.audited_at)

    def test_checkpoint_version_rejects_json_boolean(self) -> None:
        path = self.head / "docs" / "SEARCH_VISIBILITY_CHECKPOINTS.json"
        document = json.loads(path.read_text())
        document["checkpoint_version"] = True
        path.write_text(json.dumps(document, indent=2) + "\n")
        with self.assertRaisesRegex(AuditError, "checkpoint_version must be an integer"):
            validate(self.head, self.base, self.audited_at)

    def test_registry_era_row_requires_correct_rank_delta(self) -> None:
        path = self.head / "docs" / "SEARCH_VISIBILITY.md"
        path.write_text(path.read_text().replace("| 7 | +12 |", "| 7 | nonsense |"))
        self.refresh_checkpoint()
        with self.assertRaisesRegex(AuditError, "rank delta disagrees"):
            validate(self.head, self.base, self.audited_at)

    def test_registry_replay_metadata_requires_valid_types_and_ranges(self) -> None:
        mutations = {
            "returned_results type": ("returned_results", False, "must be an integer"),
            "returned_results range": (
                "returned_results",
                51,
                "outside its result window",
            ),
            "api_total relationship": (
                "api_total",
                49,
                "smaller than returned_results",
            ),
            "target_rank type": ("target_rank", False, "must be an integer"),
            "target_rank lower bound": (
                "target_rank",
                0,
                "outside returned results",
            ),
            "target_rank upper bound": (
                "target_rank",
                51,
                "outside returned results",
            ),
            "incomplete_results type": (
                "incomplete_results",
                0,
                "must be boolean",
            ),
            "corpus digest": (
                "ordered_corpus_sha256",
                "not-a-digest",
                "must be lowercase SHA-256",
            ),
        }
        original = dict(self.registry["measurements"][0])
        for name, (field, value, message) in mutations.items():
            with self.subTest(name=name):
                self.registry["measurements"][0] = {**original, field: value}
                self.write_registry()
                with self.assertRaisesRegex(AuditError, message):
                    validate(self.head, self.base, self.audited_at)

    def test_complete_measurement_requires_the_complete_result_window(self) -> None:
        measurement = self.registry["measurements"][0]
        measurement["returned_results"] = 1
        measurement["target_rank"] = None
        self.write_registry()
        with self.assertRaisesRegex(AuditError, "incomplete result window"):
            validate(self.head, self.base, self.audited_at)

    def test_incomplete_response_cannot_produce_rank_evidence(self) -> None:
        self.registry["measurements"][0]["incomplete_results"] = True
        self.write_registry()
        with self.assertRaisesRegex(AuditError, "cannot produce rank evidence"):
            validate(self.head, self.base, self.audited_at)

    def test_future_observation_is_rejected(self) -> None:
        future = "9999-12-31T23:59:59Z"
        path = self.head / "docs" / "SEARCH_VISIBILITY.md"
        path.write_text(path.read_text().replace("2026-07-31T00:00:00Z", future))
        self.registry["measurements"][0]["observed_at"] = future
        self.write_registry()
        self.refresh_checkpoint()
        with self.assertRaisesRegex(AuditError, "cannot be in the future"):
            validate(self.head, self.base, self.audited_at)

    def test_duplicate_observation_key_is_rejected(self) -> None:
        path = self.head / "docs" / "SEARCH_VISIBILITY.md"
        duplicate = (
            "| 2026-07-31T00:00:00Z | `python aot compiler` | 7 | 0 | 50 | 240 |"
        )
        path.write_text(
            path.read_text().replace(
                "\n\n## GitHub traffic history",
                f"\n{duplicate}\n\n## GitHub traffic history",
            )
        )
        self.refresh_checkpoint()
        with self.assertRaisesRegex(AuditError, "observation keys must be unique"):
            validate(self.head, self.base, self.audited_at)

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
            validate(self.head, self.base, self.audited_at)

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
            validate(self.head, self.base, self.audited_at)


if __name__ == "__main__":
    unittest.main()
