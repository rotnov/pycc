from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

import rewrite_decisions_references as rewrite


class BuildSlugMapTests(unittest.TestCase):
    def test_maps_lowercase_id_to_filename(self):
        with tempfile.TemporaryDirectory() as directory:
            d = Path(directory)
            (d / "D-021-agent-task-preflight.md").write_text("x", encoding="utf-8")
            (d / "D-142-dispatched-agent.md").write_text("x", encoding="utf-8")
            mapping = rewrite.build_slug_map(d)
            self.assertEqual(
                mapping,
                {
                    "d-021": "D-021-agent-task-preflight.md",
                    "d-142": "D-142-dispatched-agent.md",
                },
            )


class RewriteTextTests(unittest.TestCase):
    def setUp(self):
        self.slug_map = {"d-021": "D-021-agent-task-preflight.md"}

    def test_rewrites_plain_reference(self):
        text = "See [D-021](docs/DECISIONS.md#d-021-agent-task-preflight-and-documentation-refresh) for details."
        new_text, unresolved = rewrite.rewrite_text(text, self.slug_map)
        self.assertEqual(unresolved, [])
        self.assertEqual(
            new_text,
            "See [D-021](docs/decisions/D-021-agent-task-preflight.md) for details.",
        )

    def test_rewrites_relative_reference_preserving_prefix_depth(self):
        text = "[D-021](../../../docs/DECISIONS.md#d-021-agent-task-preflight-and-documentation-refresh)"
        new_text, unresolved = rewrite.rewrite_text(text, self.slug_map)
        self.assertEqual(unresolved, [])
        self.assertEqual(
            new_text, "[D-021](../../../docs/decisions/D-021-agent-task-preflight.md)"
        )

    def test_rewrites_same_directory_reference_with_no_docs_prefix(self):
        # Real shape from docs/PYTHON_STANDARDS.md and docs/DELIVERY_PLAN.md,
        # both already inside docs/ themselves: a same-directory reference
        # has no literal "docs/" segment at all, just "./DECISIONS.md".
        text = "[D-021](./DECISIONS.md#d-021-agent-task-preflight-and-documentation-refresh)"
        new_text, unresolved = rewrite.rewrite_text(text, self.slug_map)
        self.assertEqual(unresolved, [])
        self.assertEqual(new_text, "[D-021](./decisions/D-021-agent-task-preflight.md)")

    def test_rewrites_one_level_up_reference_with_no_docs_prefix(self):
        # Real shape from docs/sessions/README.md and other files one level
        # below docs/: "../DECISIONS.md" with no "docs/" segment, since the
        # referring file is already inside the docs/ tree.
        text = "[D-021](../DECISIONS.md#d-021-agent-task-preflight-and-documentation-refresh)"
        new_text, unresolved = rewrite.rewrite_text(text, self.slug_map)
        self.assertEqual(unresolved, [])
        self.assertEqual(new_text, "[D-021](../decisions/D-021-agent-task-preflight.md)")

    def test_rewrites_two_levels_up_reference_with_no_docs_prefix(self):
        # Real shape from docs/superpowers/specs/*.md: "../../DECISIONS.md"
        # with no "docs/" segment.
        text = "[D-021](../../DECISIONS.md#d-021-agent-task-preflight-and-documentation-refresh)"
        new_text, unresolved = rewrite.rewrite_text(text, self.slug_map)
        self.assertEqual(unresolved, [])
        self.assertEqual(new_text, "[D-021](../../decisions/D-021-agent-task-preflight.md)")

    def test_rewrites_reference_with_underscore_in_slug(self):
        # Real shape from D-102's own anchor (title mentions `pycc_testkit`):
        # GitHub's slugifier keeps underscores verbatim -- the character
        # class must include '_' or the match truncates mid-anchor, leaving
        # a dangling "_testkit-crate)" fragment in the rewritten text.
        slug_map = {"d-102": "D-102-extend-tests-conformance.md"}
        text = "[D-102](./DECISIONS.md#d-102-extend-testsconformancers-for-pr-9s-9-new-pep-fixtures-no-pycc_testkit-crate)"
        new_text, unresolved = rewrite.rewrite_text(text, slug_map)
        self.assertEqual(unresolved, [])
        self.assertEqual(new_text, "[D-102](./decisions/D-102-extend-tests-conformance.md)")

    def test_rewrites_multiple_occurrences(self):
        text = (
            "First: docs/DECISIONS.md#d-021-agent-task-preflight-and-documentation-refresh\n"
            "Second: docs/DECISIONS.md#d-021-agent-task-preflight-and-documentation-refresh\n"
        )
        new_text, unresolved = rewrite.rewrite_text(text, self.slug_map)
        self.assertEqual(unresolved, [])
        self.assertEqual(new_text.count("docs/decisions/D-021-agent-task-preflight.md"), 2)

    def test_leaves_unresolved_id_untouched_and_reports_it(self):
        text = "docs/DECISIONS.md#d-999-does-not-exist"
        new_text, unresolved = rewrite.rewrite_text(text, self.slug_map)
        self.assertEqual(new_text, text)
        self.assertEqual(unresolved, ["d-999"])

    def test_text_with_no_references_is_unchanged(self):
        text = "Nothing to rewrite here.\n"
        new_text, unresolved = rewrite.rewrite_text(text, self.slug_map)
        self.assertEqual(new_text, text)
        self.assertEqual(unresolved, [])


if __name__ == "__main__":
    unittest.main()
