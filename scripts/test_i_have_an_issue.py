from __future__ import annotations

import argparse
import importlib.util
import io
import os
import sys
import unittest
import urllib.error
from pathlib import Path
from unittest import mock


SCRIPT = (
    Path(__file__).resolve().parents[1]
    / ".claude"
    / "skills"
    / "i-have-an-issue"
    / "scripts"
    / "search_github.py"
)
SPEC = importlib.util.spec_from_file_location("project_search_github", SCRIPT)
assert SPEC and SPEC.loader
search_github = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(search_github)


def issue_args(**overrides: object) -> argparse.Namespace:
    values: dict[str, object] = {
        "query": '"stale cache"',
        "repo": "owner/project",
        "kind": "issues",
        "item_type": "issue",
        "state": "closed",
        "limit": 1,
        "body_chars": 80,
        "format": "json",
    }
    values.update(overrides)
    return argparse.Namespace(**values)


class IHaveAnIssueHelperTests(unittest.TestCase):
    def test_search_success_normalizes_public_issue(self) -> None:
        payload = {
            "total_count": 1,
            "incomplete_results": False,
            "items": [
                {
                    "repository_url": "https://api.github.com/repos/owner/project",
                    "number": 42,
                    "title": "Fix stale cache",
                    "state": "closed",
                    "html_url": "https://github.com/owner/project/issues/42",
                    "user": {"login": "maintainer"},
                    "labels": [{"name": "regression"}],
                    "body": "A   reproducible\nregression",
                    "score": 1.0,
                }
            ],
        }
        headers = {
            "x-ratelimit-remaining": "9",
            "x-ratelimit-reset": "123",
        }

        with (
            mock.patch.dict(os.environ, {}, clear=True),
            mock.patch.object(
                search_github,
                "request_json",
                return_value=(payload, headers),
            ) as request,
        ):
            result = search_github.search(issue_args())

        self.assertEqual(result["returned_count"], 1)
        self.assertEqual(result["items"][0]["repository"], "owner/project")
        self.assertEqual(
            result["qualified_query"],
            '"stale cache" repo:owner/project is:issue state:closed',
        )
        request_url, token = request.call_args.args
        self.assertIn("/search/issues?", request_url)
        self.assertIsNone(token)

    def test_main_reports_api_failure_without_traceback(self) -> None:
        stderr = io.StringIO()
        with (
            mock.patch.object(search_github, "parse_args", return_value=issue_args()),
            mock.patch.object(
                search_github,
                "search",
                side_effect=search_github.SearchError("rate limited"),
            ),
            mock.patch.object(sys, "stderr", stderr),
        ):
            self.assertEqual(search_github.main(), 1)

        self.assertEqual(stderr.getvalue(), "error: rate limited\n")

    def test_http_error_is_converted_to_search_error(self) -> None:
        error = urllib.error.HTTPError(
            "https://api.github.com/search/issues",
            403,
            "Forbidden",
            {
                "X-RateLimit-Remaining": "0",
                "X-RateLimit-Reset": "456",
            },
            io.BytesIO(b'{"message":"API rate limit exceeded"}'),
        )
        with mock.patch.object(
            search_github.urllib.request,
            "urlopen",
            side_effect=error,
        ):
            with self.assertRaisesRegex(
                search_github.SearchError,
                r"HTTP 403: API rate limit exceeded.*remaining=0.*reset=456",
            ):
                search_github.request_json(
                    "https://api.github.com/search/issues?q=x",
                    None,
                )

    def test_network_error_is_converted_to_search_error(self) -> None:
        with mock.patch.object(
            search_github.urllib.request,
            "urlopen",
            side_effect=urllib.error.URLError("offline"),
        ):
            with self.assertRaisesRegex(
                search_github.SearchError,
                "Could not reach GitHub API: offline",
            ):
                search_github.request_json(
                    "https://api.github.com/search/issues?q=x",
                    None,
                )

    def test_search_paginates_until_requested_limit(self) -> None:
        def issue(number: int) -> dict[str, object]:
            return {
                "repository_url": "https://api.github.com/repos/owner/project",
                "number": number,
                "title": f"Issue {number}",
                "state": "open",
                "html_url": f"https://github.com/owner/project/issues/{number}",
                "user": {"login": "maintainer"},
                "labels": [],
                "body": "",
            }

        responses = [
            (
                {
                    "total_count": 101,
                    "incomplete_results": False,
                    "items": [issue(number) for number in range(1, 101)],
                },
                {},
            ),
            (
                {
                    "total_count": 101,
                    "incomplete_results": False,
                    "items": [issue(101)],
                },
                {},
            ),
        ]
        with (
            mock.patch.dict(os.environ, {}, clear=True),
            mock.patch.object(
                search_github,
                "request_json",
                side_effect=responses,
            ) as request,
        ):
            result = search_github.search(issue_args(limit=101, state="open"))

        self.assertEqual(result["returned_count"], 101)
        self.assertIn("page=1", request.call_args_list[0].args[0])
        self.assertIn("page=2", request.call_args_list[1].args[0])

    def test_commit_and_code_results_are_normalized(self) -> None:
        commit = search_github.normalize_item(
            "commits",
            {
                "sha": "abc",
                "html_url": "https://github.com/owner/project/commit/abc",
                "repository": {"full_name": "owner/project"},
                "commit": {
                    "message": "Fix bug",
                    "author": {"name": "A", "date": "2026-07-24"},
                },
            },
            0,
        )
        code = search_github.normalize_item(
            "code",
            {
                "name": "main.rs",
                "path": "src/main.rs",
                "sha": "def",
                "html_url": "https://github.com/owner/project/blob/def/src/main.rs",
                "repository": {"full_name": "owner/project"},
            },
            0,
        )
        self.assertEqual(commit["kind"], "commit")
        self.assertEqual(commit["message"], "Fix bug")
        self.assertEqual(code["kind"], "code")
        self.assertEqual(code["path"], "src/main.rs")

    def test_jsonl_output_contains_metadata_then_items(self) -> None:
        output = io.StringIO()
        result = {
            "kind": "code",
            "query": "parser",
            "items": [{"kind": "code", "path": "src/parser.rs"}],
        }
        with mock.patch.object(sys, "stdout", output):
            search_github.emit(result, "jsonl")
        lines = output.getvalue().splitlines()
        self.assertEqual(len(lines), 2)
        self.assertEqual(
            search_github.json.loads(lines[0])["metadata"]["query"],
            "parser",
        )
        self.assertEqual(
            search_github.json.loads(lines[1])["path"],
            "src/parser.rs",
        )

    def test_repository_argument_rejects_invalid_owner_repo(self) -> None:
        with (
            mock.patch.object(
                sys,
                "argv",
                ["search_github.py", "--query", "x", "--repo", "invalid"],
            ),
            mock.patch.object(sys, "stderr", io.StringIO()),
            self.assertRaises(SystemExit) as error,
        ):
            search_github.parse_args()
        self.assertEqual(error.exception.code, 2)

    def test_result_limit_enforces_github_search_cap(self) -> None:
        self.assertEqual(search_github.search_result_limit("1000"), 1000)
        with self.assertRaises(argparse.ArgumentTypeError):
            search_github.search_result_limit("1001")


if __name__ == "__main__":
    unittest.main()
