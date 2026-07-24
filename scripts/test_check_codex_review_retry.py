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
        "comments": [],
        "reviews": [],
        "statusCheckRollup": [],
    }


def comment(
    body: str,
    created_at: str,
    *,
    author: str = "owner",
    head: str = HEAD,
    url: str | None = None,
) -> dict:
    value = {
        "author": {"login": author},
        "body": body,
        "createdAt": created_at,
        "headRefOid": head,
    }
    if url is not None:
        value["url"] = url
    return value


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

    def test_timeline_comments_follow_commit_and_force_push_order(self) -> None:
        old_head = "b" * 40
        events = [
            {"event": "committed", "sha": old_head},
            {
                "event": "commented",
                "user": {"login": "owner"},
                "body": "@codex review",
                "created_at": "2026-07-24T20:05:00Z",
                "html_url": "https://example.test/old",
            },
            {
                "event": "head_ref_force_pushed",
                "after_commit": {"sha": HEAD},
            },
            {
                "event": "commented",
                "user": {"login": gate.CODEX_LOGIN},
                "body": "Unable to start the review.",
                "created_at": "2026-07-24T20:06:00Z",
                "html_url": "https://example.test/new",
            },
        ]
        comments = gate.timeline_comments(events)
        self.assertEqual(comments[0]["headRefOid"], old_head)
        self.assertEqual(comments[1]["headRefOid"], HEAD)
        self.assertEqual(comments[1]["author"]["login"], gate.CODEX_LOGIN)

    def test_old_request_does_not_consume_new_head_budget(self) -> None:
        data = payload()
        data["comments"] = [
            comment(
                "@codex review",
                "2026-07-24T20:05:00Z",
                head="b" * 40,
            )
        ]
        self.assertEqual(gate.classify(data, NOW)[0], "REQUEST_ALLOWED")

    def test_unthreaded_failure_comment_does_not_allow_early_retry(self) -> None:
        data = payload()
        data["comments"] = [
            comment(
                "@codex review",
                "2026-07-24T20:05:00Z",
                url="https://example.test/request",
            ),
            comment(
                "To use Codex here, create an environment for this repo.",
                "2026-07-24T20:06:00Z",
                author=gate.CODEX_LOGIN,
                url="https://example.test/failure",
            ),
        ]
        self.assertEqual(gate.classify(data, NOW)[0], "ARTIFACT_EXISTS")

    def test_second_attempt_reaches_retry_limit(self) -> None:
        data = payload()
        data["comments"] = [
            comment("@codex review", "2026-07-24T20:05:00Z"),
            comment("@codex review", "2026-07-24T20:10:00Z"),
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
            comment("@codex review", "2026-07-24T20:05:00Z"),
            comment(
                "Codex Review: no major issues.",
                "2026-07-24T20:06:00Z",
                author=gate.CODEX_LOGIN,
                url="https://example.test/review",
            ),
        ]
        self.assertEqual(
            gate.classify(data, NOW)[0],
            "ARTIFACT_EXISTS",
        )

    def test_success_comment_that_mentions_no_errors_is_not_a_failure(self) -> None:
        data = payload()
        data["comments"] = [
            comment("@codex review", "2026-07-24T20:05:00Z"),
            comment(
                "Codex Review: no errors found.",
                "2026-07-24T20:06:00Z",
                author=gate.CODEX_LOGIN,
                url="https://example.test/review",
            ),
        ]
        self.assertEqual(
            gate.classify(data, NOW)[0],
            "ARTIFACT_EXISTS",
        )

    def test_bot_suffixed_failure_comment_does_not_allow_early_retry(self) -> None:
        data = payload()
        data["comments"] = [
            comment("@codex review", "2026-07-24T20:05:00Z"),
            comment(
                "Unable to start the review.",
                "2026-07-24T20:06:00Z",
                author=f"{gate.CODEX_LOGIN}[bot]",
                url="https://example.test/failure",
            ),
        ]
        self.assertEqual(gate.classify(data, NOW)[0], "ARTIFACT_EXISTS")

    def test_delayed_old_head_failure_cannot_unlock_new_head_retry(self) -> None:
        data = payload()
        data["comments"] = [
            comment("@codex review", "2026-07-24T20:05:00Z"),
            comment(
                "Unable to start the review.",
                "2026-07-24T20:06:00Z",
                author=gate.CODEX_LOGIN,
                url="https://example.test/delayed-old-head-failure",
            ),
        ]
        state, evidence = gate.classify(
            data,
            dt.datetime(2026, 7, 24, 20, 10, tzinfo=dt.UTC),
        )
        self.assertEqual(state, "ARTIFACT_EXISTS")
        self.assertIn("cannot be safely correlated", evidence)

    def test_timeout_without_artifact_allows_retry(self) -> None:
        data = payload()
        data["comments"] = [
            comment(
                "@codex review",
                "2026-07-24T20:05:00Z",
                url="https://example.test/request",
            )
        ]
        self.assertEqual(gate.classify(data, NOW)[0], "RETRY_ALLOWED")

    def test_recent_request_waits(self) -> None:
        data = payload()
        data["comments"] = [comment("@codex review", "2026-07-24T20:55:00Z")]
        self.assertEqual(gate.classify(data, NOW)[0], "WAIT")

    def test_human_failure_comment_does_not_block_timeout(self) -> None:
        data = payload()
        data["comments"] = [
            comment(
                "@codex review",
                "2026-07-24T20:05:00Z",
                url="https://example.test/request",
            ),
            comment(
                "The build failed.",
                "2026-07-24T20:06:00Z",
                author="reviewer",
            ),
        ]
        self.assertEqual(gate.classify(data, NOW)[0], "RETRY_ALLOWED")


if __name__ == "__main__":
    unittest.main()
