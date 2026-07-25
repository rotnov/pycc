#!/usr/bin/env python3
"""Regression tests for the shared local-review entrypoint contract."""

from __future__ import annotations

import unittest

import check_review_local_changes as checker


class ReviewLocalChangesTests(unittest.TestCase):
    def test_codex_entrypoint_success_and_safe_failure(self) -> None:
        checker.validate_entrypoint("codex")

    def test_claude_entrypoint_success_and_safe_failure(self) -> None:
        checker.validate_entrypoint("claude")


if __name__ == "__main__":
    unittest.main()
