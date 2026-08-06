from __future__ import annotations

import unittest

import migrate_decisions_log as migrate


FIXTURE = '''# Test Decisions

Format: one entry per irreversible-ish call.

| ID | Decision | Status |
|---|---|---|
| D-001 | First decision, index-only | proposed |
| D-002 | Second decision | accepted |
| D-003 | Third decision, has a nested code block | accepted |

## Template

```
## D-0XX: Title
- Status: proposed
```

Entries D-001 gets its long-form section once it graduates.

## D-002: Second decision

- Status: accepted
- Context: something happened.
- Decision: we did X.
- Alternatives: Y (rejected).
- Consequences: Z.

## D-003: Third decision, has a nested code block

- Status: accepted (closes #7)
- Context: needs an example.
- Decision: use this snippet:

```python
def f():
    return 1
```

- Alternatives: none.
- Consequences: none.
'''


class SplitEntriesTests(unittest.TestCase):
    def test_finds_only_real_headings_outside_fences(self):
        entries = migrate.split_entries(FIXTURE)
        ids = [id_ for id_, _, _ in entries]
        self.assertEqual(ids, ["D-002", "D-003"])

    def test_entry_body_includes_nested_fence_content(self):
        entries = migrate.split_entries(FIXTURE)
        _, _, body = entries[1]
        self.assertIn("def f():", body)
        self.assertIn("Alternatives: none.", body)


class ParseIndexTableTests(unittest.TestCase):
    def test_parses_all_rows_in_order(self):
        rows = migrate.parse_index_table(FIXTURE)
        self.assertEqual(
            rows,
            [
                ("D-001", "First decision, index-only", "proposed"),
                ("D-002", "Second decision", "accepted"),
                ("D-003", "Third decision, has a nested code block", "accepted"),
            ],
        )


class ExtractStatusTests(unittest.TestCase):
    def test_plain_status(self):
        self.assertEqual(
            migrate.extract_status("- Status: accepted\n- Context: x"), "accepted"
        )

    def test_status_with_parenthetical_detail(self):
        self.assertEqual(
            migrate.extract_status("- Status: accepted (closes #7)\n- Context: x"),
            "accepted",
        )

    def test_missing_status_raises(self):
        with self.assertRaises(ValueError):
            migrate.extract_status("- Context: no status line here")


class SlugifyTests(unittest.TestCase):
    def test_basic(self):
        self.assertEqual(migrate.slugify("Second decision"), "second-decision")

    def test_truncates_at_word_boundary(self):
        long_title = "a very long title that keeps going and going and going and going"
        slug = migrate.slugify(long_title, max_len=20)
        self.assertLessEqual(len(slug), 20)
        self.assertFalse(slug.endswith("-"))
        words = slug.split("-")
        title_words = long_title.lower().split()
        self.assertEqual(words, title_words[: len(words)])


class BuildFilesTests(unittest.TestCase):
    def test_builds_one_file_per_long_form_entry_and_one_stub(self):
        files = migrate.build_files(FIXTURE)
        self.assertEqual(
            set(files.keys()),
            {
                "D-001-first-decision-index-only.md",
                "D-002-second-decision.md",
                "D-003-third-decision-has-a-nested-code-block.md",
            },
        )

    def test_long_form_file_has_frontmatter_and_body(self):
        files = migrate.build_files(FIXTURE)
        content = files["D-002-second-decision.md"]
        self.assertTrue(content.startswith('---\nid: D-002\n'))
        self.assertIn('status: accepted', content)
        self.assertIn("## D-002: Second decision", content)
        self.assertIn("- Context: something happened.", content)

    def test_index_only_stub_has_the_fixed_note(self):
        files = migrate.build_files(FIXTURE)
        content = files["D-001-first-decision-index-only.md"]
        self.assertIn("status: proposed", content)
        self.assertIn("Index-only: no long-form entry recorded yet.", content)

    def test_status_comes_from_body_not_index_row_when_they_disagree(self):
        drifted = FIXTURE.replace(
            "| D-002 | Second decision | accepted |",
            "| D-002 | Second decision | proposed |",
        )
        files = migrate.build_files(drifted)
        content = files["D-002-second-decision.md"]
        self.assertIn("status: accepted", content)
        self.assertNotIn("status: proposed", content)


class VerifyRoundTripTests(unittest.TestCase):
    def test_ok_on_well_formed_fixture(self):
        ok, message = migrate.verify_round_trip(FIXTURE)
        self.assertTrue(ok, message)
        self.assertIn("2 entries", message)

    def test_fails_when_a_heading_has_no_index_row(self):
        orphan = FIXTURE.replace(
            "## D-003: Third decision, has a nested code block",
            "## D-999: Orphan heading with no index row\n\n## D-003: Third decision, has a nested code block",
        )
        ok, message = migrate.verify_round_trip(orphan)
        self.assertFalse(ok)
        self.assertIn("D-999", message)

    def test_fails_on_empty_input(self):
        ok, message = migrate.verify_round_trip("# Nothing here\n")
        self.assertFalse(ok)
        self.assertEqual(message, "no entries found")


if __name__ == "__main__":
    unittest.main()
