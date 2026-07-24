#!/usr/bin/env python3
"""Tests for the Codex review retry classifier."""

from __future__ import annotations

import datetime as dt
import unittest

import check_codex_review_retry as gate


NOW = dt.datetime(2026, 7, 24, 21, 0, tzinfo=dt.UTC)
HEAD = "a" * 40


def payload() -> dict:
    return {
        "headRefOid": HEAD,
        "commits": [{"committedDate": "2026-07-24T20:00:00Z"}],
        "comments": [],
        "reviews": [],
        "statusCheckRollup": [],
    }


class CodexReviewRetryTests(unittest.TestCase):
    def test_flattens_all_array_pages(self) -> None:
        self.assertEqual(
            gate.flatten_pages([[{"id": 1}], [{"id": 2}]]),
            [{"id": 1}, {"id": 2}],
        )

    def test_flattens_all_check_run_pages(self) -> None:
        self.assertEqual(
            gate.flatten_pages(
                [
                    {"check_runs": [{"id": 1}]},
                    {"check_runs": [{"id": 2}]},
                ],
                "check_runs",
            ),
            [{"id": 1}, {"id": 2}],
        )

    def test_first_request_is_allowed(self) -> None:
        self.assertEqual(gate.classify(payload(), NOW)[0], "REQUEST_ALLOWED")

    def test_explicit_failed_request_allows_one_retry(self) -> None:
        data = payload()
        data["comments"] = [
            {
                "author": {"login": "owner"},
                "body": "@codex review",
                "createdAt": "2026-07-24T20:05:00Z",
                "url": "https://example.test/request",
            },
            {
                "author": {"login": gate.CODEX_LOGIN},
                "body": ("To use Codex here, create an environment for this repo."),
                "createdAt": "2026-07-24T20:06:00Z",
                "url": "https://example.test/failure",
            },
        ]
        self.assertEqual(gate.classify(data, NOW)[0], "RETRY_ALLOWED")

    def test_second_attempt_reaches_retry_limit(self) -> None:
        data = payload()
        data["comments"] = [
            {
                "author": {"login": "owner"},
                "body": "@codex review",
                "createdAt": "2026-07-24T20:05:00Z",
            },
            {
                "author": {"login": "owner"},
                "body": "@codex review",
                "createdAt": "2026-07-24T20:10:00Z",
            },
        ]
        self.assertEqual(
            gate.classify(data, NOW)[0],
            "RETRY_LIMIT_REACHED",
        )

    def test_current_head_review_blocks_duplicate(self) -> None:
        data = payload()
        data["reviews"] = [
            {
                "author": {"login": gate.CODEX_LOGIN},
                "commit": {"oid": HEAD},
            }
        ]
        self.assertEqual(gate.classify(data, NOW)[0], "ARTIFACT_EXISTS")

    def test_bot_suffixed_review_blocks_duplicate(self) -> None:
        data = payload()
        data["reviews"] = [
            {
                "author": {"login": f"{gate.CODEX_LOGIN}[bot]"},
                "commit": {"oid": HEAD},
            }
        ]
        self.assertEqual(gate.classify(data, NOW)[0], "ARTIFACT_EXISTS")

    def test_completed_codex_check_blocks_duplicate(self) -> None:
        data = payload()
        data["statusCheckRollup"] = [
            {
                "name": "Codex review",
                "status": "COMPLETED",
                "conclusion": "SUCCESS",
            }
        ]
        self.assertEqual(
            gate.classify(data, NOW)[0],
            "ARTIFACT_EXISTS",
        )

    def test_codex_success_comment_blocks_duplicate(self) -> None:
        data = payload()
        data["comments"] = [
            {
                "author": {"login": "owner"},
                "body": "@codex review",
                "createdAt": "2026-07-24T20:05:00Z",
            },
            {
                "author": {"login": gate.CODEX_LOGIN},
                "body": "Codex Review: no major issues.",
                "createdAt": "2026-07-24T20:06:00Z",
                "url": "https://example.test/review",
            },
        ]
        self.assertEqual(
            gate.classify(data, NOW)[0],
            "ARTIFACT_EXISTS",
        )

    def test_success_comment_that_mentions_no_errors_is_not_a_failure(self) -> None:
        data = payload()
        data["comments"] = [
            {
                "author": {"login": "owner"},
                "body": "@codex review",
                "createdAt": "2026-07-24T20:05:00Z",
            },
            {
                "author": {"login": gate.CODEX_LOGIN},
                "body": "Codex Review: no errors found.",
                "createdAt": "2026-07-24T20:06:00Z",
                "url": "https://example.test/review",
            },
        ]
        self.assertEqual(
            gate.classify(data, NOW)[0],
            "ARTIFACT_EXISTS",
        )

    def test_bot_suffixed_failure_comment_allows_retry(self) -> None:
        data = payload()
        data["comments"] = [
            {
                "author": {"login": "owner"},
                "body": "@codex review",
                "createdAt": "2026-07-24T20:05:00Z",
            },
            {
                "author": {"login": f"{gate.CODEX_LOGIN}[bot]"},
                "body": "Unable to start the review.",
                "createdAt": "2026-07-24T20:06:00Z",
                "url": "https://example.test/failure",
            },
        ]
        self.assertEqual(gate.classify(data, NOW)[0], "RETRY_ALLOWED")

    def test_timeout_without_artifact_allows_retry(self) -> None:
        data = payload()
        data["comments"] = [
            {
                "author": {"login": "owner"},
                "body": "@codex review",
                "createdAt": "2026-07-24T20:05:00Z",
                "url": "https://example.test/request",
            }
        ]
        self.assertEqual(gate.classify(data, NOW)[0], "RETRY_ALLOWED")

    def test_recent_request_waits(self) -> None:
        data = payload()
        data["comments"] = [
            {
                "author": {"login": "owner"},
                "body": "@codex review",
                "createdAt": "2026-07-24T20:55:00Z",
            }
        ]
        self.assertEqual(gate.classify(data, NOW)[0], "WAIT")

    def test_human_failure_comment_does_not_block_timeout(self) -> None:
        data = payload()
        data["comments"] = [
            {
                "author": {"login": "owner"},
                "body": "@codex review",
                "createdAt": "2026-07-24T20:05:00Z",
                "url": "https://example.test/request",
            },
            {
                "author": {"login": "reviewer"},
                "body": "The build failed.",
                "createdAt": "2026-07-24T20:06:00Z",
            },
        ]
        self.assertEqual(gate.classify(data, NOW)[0], "RETRY_ALLOWED")


if __name__ == "__main__":
    unittest.main()
