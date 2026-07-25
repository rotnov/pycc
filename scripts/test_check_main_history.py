#!/usr/bin/env python3
"""Regression tests for the protected-main history audit."""

from __future__ import annotations

import io
import json
import subprocess
import unittest
from collections.abc import Callable
from contextlib import redirect_stdout
from unittest import mock

import check_main_history as audit


Result = subprocess.CompletedProcess[str]


def result(
    arguments: list[str],
    returncode: int = 0,
    stdout: str = "",
    stderr: str = "",
) -> Result:
    return subprocess.CompletedProcess(arguments, returncode, stdout, stderr)


def runner(
    responses: list[Result],
) -> Callable[..., Result]:
    pending = list(responses)

    def run(arguments: list[str], **_: object) -> Result:
        current = pending.pop(0)
        current.args = arguments
        return current

    return run


class MainHistoryAuditTests(unittest.TestCase):
    def test_zero_sha_fails_closed_without_revision_enumeration(self) -> None:
        commit = "a" * 40
        self.assertEqual(
            audit.audit_main_history("owner/repo", audit.ZERO_SHA, commit),
            [
                (
                    "Unexpected main branch creation",
                    "A zero before SHA cannot prove the complete introduced history",
                )
            ],
        )

    def test_created_and_forced_events_fail_before_revision_enumeration(self) -> None:
        self.assertEqual(
            audit.audit_main_history(
                "owner/repo",
                "a" * 40,
                "b" * 40,
                created=True,
                forced=True,
            ),
            [
                (
                    "Unexpected main branch creation",
                    "The protected main branch was reported as newly created",
                ),
                (
                    "Forced main update",
                    "The protected main branch was updated non-fast-forward",
                ),
            ],
        )

    def test_revision_enumeration_failure_is_reported(self) -> None:
        fake = runner([result([], returncode=1, stderr="bad revision")])
        self.assertEqual(
            audit.audit_main_history("owner/repo", "a" * 40, "b" * 40, fake),
            [
                (
                    "Main history audit unavailable",
                    "Could not enumerate pushed commits",
                )
            ],
        )

    def test_empty_revision_range_is_reported(self) -> None:
        fake = runner([result([], stdout="\n")])
        self.assertEqual(
            audit.audit_main_history("owner/repo", "a" * 40, "b" * 40, fake),
            [
                (
                    "Main history audit unavailable",
                    "No pushed commits were found to audit",
                )
            ],
        )

    def test_revision_enumeration_uses_the_forward_push_range(self) -> None:
        calls: list[tuple[list[str], dict[str, object]]] = []

        def fake(arguments: list[str], **kwargs: object) -> Result:
            calls.append((arguments, kwargs))
            return result(arguments, stdout=f"{'b' * 40}\n")

        commits, error = audit.pushed_commits("a" * 40, "b" * 40, fake)
        self.assertEqual(commits, ["b" * 40])
        self.assertIsNone(error)
        self.assertEqual(
            calls,
            [
                (
                    ["git", "rev-list", f"{'a' * 40}..{'b' * 40}"],
                    {
                        "check": False,
                        "capture_output": True,
                        "text": True,
                    },
                )
            ],
        )

    def test_api_failure_is_reported(self) -> None:
        commit = "b" * 40
        fake = runner(
            [
                result([], stdout=f"{commit}\n"),
                result([], returncode=1, stderr="rate limited"),
            ]
        )
        self.assertEqual(
            audit.audit_main_history("owner/repo", "a" * 40, commit, fake),
            [
                (
                    "Main history audit unavailable",
                    f"GitHub API failed while checking {commit}",
                )
            ],
        )

    def test_malformed_api_response_is_reported(self) -> None:
        commit = "b" * 40
        fake = runner(
            [
                result([], stdout=f"{commit}\n"),
                result([], stdout="{not-json"),
            ]
        )
        self.assertEqual(
            audit.audit_main_history("owner/repo", "a" * 40, commit, fake),
            [
                (
                    "Invalid main history audit response",
                    f"Expected pull-request associations for {commit}",
                )
            ],
        )

    def test_malformed_association_fields_fail_closed(self) -> None:
        commit = "b" * 40
        for merged_at in (False, 0, "", {}, []):
            with self.subTest(merged_at=merged_at):
                fake = runner(
                    [
                        result([], stdout=f"{commit}\n"),
                        result(
                            [],
                            stdout=json.dumps(
                                [
                                    {
                                        "merged_at": merged_at,
                                        "base": {"ref": "main"},
                                        "merge_commit_sha": commit,
                                    }
                                ]
                            ),
                        ),
                    ]
                )
                self.assertEqual(
                    audit.audit_main_history(
                        "owner/repo",
                        "a" * 40,
                        commit,
                        fake,
                    ),
                    [
                        (
                            "Invalid main history audit response",
                            (f"Expected pull-request associations for {commit}"),
                        )
                    ],
                )

        for base in ({}, {"ref": None}, {"ref": 42}):
            with self.subTest(base=base):
                fake = runner(
                    [
                        result([], stdout=f"{commit}\n"),
                        result(
                            [],
                            stdout=json.dumps([{"merged_at": None, "base": base}]),
                        ),
                    ]
                )
                self.assertEqual(
                    audit.audit_main_history(
                        "owner/repo",
                        "a" * 40,
                        commit,
                        fake,
                    ),
                    [
                        (
                            "Invalid main history audit response",
                            (f"Expected pull-request associations for {commit}"),
                        )
                    ],
                )

        for merge_commit_sha in (None, False, 0, "", {}, []):
            with self.subTest(merge_commit_sha=merge_commit_sha):
                fake = runner(
                    [
                        result([], stdout=f"{commit}\n"),
                        result(
                            [],
                            stdout=json.dumps(
                                [
                                    {
                                        "merged_at": "2026-07-24T20:00:00Z",
                                        "base": {"ref": "main"},
                                        "merge_commit_sha": merge_commit_sha,
                                    }
                                ]
                            ),
                        ),
                    ]
                )
                self.assertEqual(
                    audit.audit_main_history(
                        "owner/repo",
                        "a" * 40,
                        commit,
                        fake,
                    ),
                    [
                        (
                            "Invalid main history audit response",
                            (f"Expected pull-request associations for {commit}"),
                        )
                    ],
                )

    def test_main_rejects_missing_environment_with_annotation(self) -> None:
        output = io.StringIO()
        with (
            mock.patch.dict("os.environ", {}, clear=True),
            redirect_stdout(output),
        ):
            status = audit.main()
        self.assertEqual(status, 1)
        self.assertEqual(
            output.getvalue(),
            (
                "::error title=Main history audit unavailable::"
                "AUDIT_REPOSITORY, AUDIT_BEFORE, AUDIT_AFTER, AUDIT_CREATED, "
                "and AUDIT_FORCED are required with boolean event flags\n"
            ),
        )

    def test_main_returns_success_without_annotations(self) -> None:
        output = io.StringIO()
        environment = {
            "AUDIT_REPOSITORY": "owner/repo",
            "AUDIT_BEFORE": "a" * 40,
            "AUDIT_AFTER": "b" * 40,
            "AUDIT_CREATED": "false",
            "AUDIT_FORCED": "false",
        }
        with (
            mock.patch.dict("os.environ", environment, clear=True),
            mock.patch.object(audit, "audit_main_history", return_value=[]) as check,
            redirect_stdout(output),
        ):
            status = audit.main()
        self.assertEqual(status, 0)
        self.assertEqual(output.getvalue(), "")
        check.assert_called_once_with(
            environment["AUDIT_REPOSITORY"],
            environment["AUDIT_BEFORE"],
            environment["AUDIT_AFTER"],
            created=False,
            forced=False,
        )

    def test_merged_main_association_passes(self) -> None:
        commit = "b" * 40
        associations = [
            {"merged_at": None, "base": {"ref": "main"}},
            {
                "merged_at": "2026-07-24T20:00:00Z",
                "base": {"ref": "release"},
                "merge_commit_sha": commit,
            },
            {
                "merged_at": "2026-07-24T20:00:00Z",
                "base": {"ref": "main"},
                "merge_commit_sha": commit,
            },
        ]
        fake = runner(
            [
                result([], stdout=f"{commit}\n"),
                result([], stdout=json.dumps(associations)),
            ]
        )
        self.assertEqual(
            audit.audit_main_history("owner/repo", "a" * 40, commit, fake),
            [],
        )

    def test_later_api_page_can_prove_merged_main_association(self) -> None:
        commit = "b" * 40
        pages = [
            [
                {
                    "merged_at": "2026-07-24T20:00:00Z",
                    "base": {"ref": "release"},
                    "merge_commit_sha": commit,
                }
            ],
            [
                {
                    "merged_at": "2026-07-24T20:05:00Z",
                    "base": {"ref": "main"},
                    "merge_commit_sha": commit,
                }
            ],
        ]
        fake = runner(
            [
                result([], stdout=f"{commit}\n"),
                result([], stdout=json.dumps(pages)),
            ]
        )
        self.assertEqual(
            audit.audit_main_history("owner/repo", "a" * 40, commit, fake),
            [],
        )

    def test_multiple_commits_are_all_audited_and_failures_are_aggregated(
        self,
    ) -> None:
        first = "b" * 40
        second = "c" * 40
        fake = runner(
            [
                result([], stdout=f"{first}\n{second}\n"),
                result(
                    [],
                    stdout=json.dumps(
                        [
                            {
                                "merged_at": "2026-07-24T20:00:00Z",
                                "base": {"ref": "main"},
                                "merge_commit_sha": first,
                            }
                        ]
                    ),
                ),
                result([], stdout=json.dumps([])),
            ]
        )
        self.assertEqual(
            audit.audit_main_history("owner/repo", "a" * 40, second, fake),
            [
                (
                    "Uncorrelated main commit",
                    (
                        f"{second} has no merged-main pull request whose "
                        "merge commit was introduced by this push"
                    ),
                )
            ],
        )

    def test_source_commits_correlate_with_merge_commit_in_same_push(self) -> None:
        source = "b" * 40
        merge = "c" * 40
        association = [
            {
                "merged_at": "2026-07-24T20:00:00Z",
                "base": {"ref": "main"},
                "merge_commit_sha": merge,
            }
        ]
        fake = runner(
            [
                result([], stdout=f"{source}\n{merge}\n"),
                result([], stdout=json.dumps(association)),
                result([], stdout=json.dumps(association)),
            ]
        )
        self.assertEqual(
            audit.audit_main_history("owner/repo", "a" * 40, merge, fake),
            [],
        )

    def test_api_request_uses_the_association_endpoint_and_media_type(self) -> None:
        calls: list[tuple[list[str], dict[str, object]]] = []

        def fake(arguments: list[str], **kwargs: object) -> Result:
            calls.append((arguments, kwargs))
            return result(arguments, stdout="[]")

        merge_shas, error = audit.merged_main_pr_merge_shas(
            "owner/repo",
            "b" * 40,
            fake,
        )
        self.assertEqual(merge_shas, set())
        self.assertIsNone(error)
        self.assertEqual(
            calls,
            [
                (
                    [
                        "gh",
                        "api",
                        "--paginate",
                        "--slurp",
                        "-H",
                        "Accept: application/vnd.github+json",
                        (f"repos/owner/repo/commits/{'b' * 40}/pulls?per_page=100"),
                    ],
                    {
                        "check": False,
                        "capture_output": True,
                        "text": True,
                    },
                )
            ],
        )

    def test_historical_merge_association_does_not_prove_this_push(self) -> None:
        commit = "b" * 40
        historical_merge = "d" * 40
        fake = runner(
            [
                result([], stdout=f"{commit}\n"),
                result(
                    [],
                    stdout=json.dumps(
                        [
                            {
                                "merged_at": "2026-07-24T20:00:00Z",
                                "base": {"ref": "main"},
                                "merge_commit_sha": historical_merge,
                            }
                        ]
                    ),
                ),
            ]
        )
        self.assertEqual(
            audit.audit_main_history("owner/repo", "a" * 40, commit, fake),
            [
                (
                    "Uncorrelated main commit",
                    (
                        f"{commit} has no merged-main pull request whose "
                        "merge commit was introduced by this push"
                    ),
                )
            ],
        )


if __name__ == "__main__":
    unittest.main()
