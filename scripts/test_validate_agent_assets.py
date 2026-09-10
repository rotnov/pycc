#!/usr/bin/env python3
"""Regression tests for repository agent-asset validation."""

from __future__ import annotations

import codecs
import hashlib
import json
import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import validate_agent_assets as validator


FEATURE_DEV = "feature" + "-dev"
CLAUDE_PLUGIN_MARKETPLACE = "claude-plugins" + "-official"
TDD_WORKFLOWS = "tdd" + "-workflows"
CLAUDE_WORKFLOW_MARKETPLACE = "claude-code" + "-workflows"
MUTABLE_HELPER = "mutable" + "-helper"
CODE_REVIEW = "code" + "-review"
PR_REVIEW_TOOLKIT = "pr-review" + "-toolkit"


class AgentAssetValidationTests(unittest.TestCase):
    def fence_result(self, contents: str) -> str | None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "fixture.md"
            path.write_text(contents, encoding="utf-8")
            return validator.fence_error(path)

    def test_balanced_fence_is_accepted(self) -> None:
        self.assertIsNone(self.fence_result("```markdown\n# Example\n```\n"))

    def test_unclosed_fence_is_rejected(self) -> None:
        self.assertIn(
            "unclosed",
            self.fence_result("```markdown\n# Example\n") or "",
        )

    def test_nested_fence_is_rejected(self) -> None:
        self.assertIn(
            "nested",
            self.fence_result("```markdown\n```json\n{}\n```\n") or "",
        )

    def test_absolute_repository_output_is_detected(self) -> None:
        line = "The ADR must be saved in the `/docs/adr/` directory."
        match = validator.ABSOLUTE_OUTPUT.search(line)
        self.assertIsNotNone(match)
        self.assertEqual(match.group(1), "/docs/adr/")

    def test_slash_skill_dependency_is_detected(self) -> None:
        self.assertEqual(
            validator.SLASH_SKILL.findall(
                "Run a `/grilling` session using the `/domain-modeling` skill."
            ),
            ["grilling", "domain-modeling"],
        )

    def test_relative_link_target_drops_fragment(self) -> None:
        self.assertEqual(
            validator.link_target("../skill/SKILL.md#workflow"),
            "../skill/SKILL.md",
        )

    def test_absolute_link_target_stays_absolute_for_rejection(self) -> None:
        self.assertEqual(validator.link_target("/spec/"), "/spec/")

    def test_immutable_pin_requires_a_full_commit_sha(self) -> None:
        self.assertIsNotNone(
            validator.IMMUTABLE_SHA.fullmatch(
                "7d5f3e12d0556cb6c5df2974e2babe0433674186"
            )
        )
        self.assertIsNone(validator.IMMUTABLE_SHA.fullmatch("v0.58.1"))

    def test_skill_folder_hash_matches_skills_cli_ordering(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "agents").mkdir()
            (root / "agents" / "openai.yaml").write_bytes(b"agent")
            (root / "SKILL.md").write_bytes(b"skill")

            expected = hashlib.sha256()
            expected.update(b"agents/openai.yaml")
            expected.update(b"agent")
            expected.update(b"SKILL.md")
            expected.update(b"skill")
            self.assertEqual(
                validator.compute_skill_folder_hash(
                    root,
                    [root / "SKILL.md", root / "agents" / "openai.yaml"],
                ),
                expected.hexdigest(),
            )

    def test_skill_folder_hash_hashes_every_listed_file(self) -> None:
        # The hash function no longer filters bytecode: rejection is the
        # payload validator's job, and upstream hashes every regular file.
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "SKILL.md").write_bytes(b"skill")
            (root / "payload.pyc").write_bytes(b"bytecode")

            expected = hashlib.sha256()
            expected.update(b"payload.pyc")
            expected.update(b"bytecode")
            expected.update(b"SKILL.md")
            expected.update(b"skill")
            self.assertEqual(
                validator.compute_skill_folder_hash(
                    root, [root / "SKILL.md", root / "payload.pyc"]
                ),
                expected.hexdigest(),
            )

    def skill_lock_failures(
        self,
        *,
        entry_overrides: dict[str, object] | None = None,
        canonical_present: bool = True,
    ) -> list[str]:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "docs").mkdir()
            (root / "docs" / "AGENT_TOOLING.md").write_text(
                (validator.ROOT / "docs" / "AGENT_TOOLING.md").read_text(
                    encoding="utf-8"
                ),
                encoding="utf-8",
            )
            lock = json.loads(
                (validator.ROOT / "skills-lock.json").read_text(encoding="utf-8")
            )
            if entry_overrides:
                lock["skills"]["i-have-an-issue"].update(entry_overrides)
            (root / "skills-lock.json").write_text(
                json.dumps(lock),
                encoding="utf-8",
            )
            skills_root = validator.SKILLS_ROOT
            if not canonical_present:
                skills_root = root / "empty-skills"
                skills_root.mkdir()
            # The lock and policy live in the temp root while the payload is
            # the real vendored copy, so enumerate it from the real repository.
            payload_entries = validator.skill_payload_entries(
                validator.SKILLS_ROOT / "i-have-an-issue", validator.ROOT
            )
            failures: list[str] = []
            validator.validate_skill_lock(
                failures,
                root=root,
                skills_root=skills_root,
                payload_entries=payload_entries,
            )
            return failures

    def test_skill_lock_binds_reviewed_provenance_and_hash(self) -> None:
        self.assertEqual(self.skill_lock_failures(), [])

    def test_skill_lock_rejects_mismatched_content_hash(self) -> None:
        failures = self.skill_lock_failures(
            entry_overrides={"computedHash": "0" * 64}
        )
        self.assertTrue(any(".computedHash must be" in item for item in failures))

    def test_skill_lock_rejects_changed_upstream_source(self) -> None:
        failures = self.skill_lock_failures(
            entry_overrides={"source": "attacker/skills"}
        )
        self.assertTrue(any(".source must be" in item for item in failures))

    def test_skill_lock_requires_canonical_skill(self) -> None:
        failures = self.skill_lock_failures(canonical_present=False)
        self.assertTrue(any("has no canonical" in item for item in failures))

    VENDORED_SKILL = "i-have-an-issue"
    LOCK_LABEL = f"skills-lock.json: skills.{VENDORED_SKILL}"
    HASH_MISMATCH = ".computedHash does not match"

    @staticmethod
    def copy_lock_fixtures(root: Path) -> None:
        (root / "docs").mkdir(parents=True, exist_ok=True)
        shutil.copyfile(
            validator.ROOT / "docs" / "AGENT_TOOLING.md",
            root / "docs" / "AGENT_TOOLING.md",
        )
        shutil.copyfile(
            validator.ROOT / "skills-lock.json", root / "skills-lock.json"
        )

    @classmethod
    def copy_vendored_skill(
        cls, destination: Path
    ) -> list[validator.SkillPayloadEntry]:
        """Copy the vendored skill's tracked files and return their entries.

        Both the fixture's file set and the returned entry list derive from
        the repository index, exactly as ``validate_skill_lock`` enumerates
        them, so an untracked stray inside the real skill directory (a
        ``.DS_Store``, a bytecode cache) can never enter the fixture or be
        synthesized as a tracked entry.
        """
        source = validator.SKILLS_ROOT / cls.VENDORED_SKILL
        entries = validator.skill_payload_entries(source, validator.ROOT)
        for entry in entries:
            target = destination / entry.relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source / entry.relative, target)
        return entries

    def mutated_skill_lock_failures(
        self,
        mutate=None,
        entries: list[tuple[str, str, int]] | None = None,
    ) -> list[str]:
        """Run the lock against a mutated temp copy with injected entries."""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            self.copy_lock_fixtures(root)
            skills_root = root / ".claude" / "skills"
            skill_root = skills_root / self.VENDORED_SKILL
            payload = self.copy_vendored_skill(skill_root)
            if mutate is not None:
                mutate(skill_root)
            payload.extend(
                validator.SkillPayloadEntry(*entry) for entry in entries or []
            )
            failures: list[str] = []
            validator.validate_skill_lock(
                failures,
                root=root,
                skills_root=skills_root,
                payload_entries=payload,
            )
            return failures

    def assert_payload_rejected(
        self,
        failures: list[str],
        relative: str,
        reason: str,
    ) -> None:
        self.assertIn(f"{self.LOCK_LABEL}: {relative}: {reason}", failures)
        self.assertFalse(
            any(self.HASH_MISMATCH in item for item in failures), failures
        )

    def test_skill_lock_accepts_tracked_copy_of_vendored_payload(self) -> None:
        self.assertEqual(self.mutated_skill_lock_failures(), [])

    def test_skill_lock_rejects_tracked_bytecode_at_skill_root(self) -> None:
        def mutate(skill_root: Path) -> None:
            (skill_root / "payload.pyc").write_bytes(b"\x00bytecode")

        failures = self.mutated_skill_lock_failures(
            mutate, [("payload.pyc", "100644", 0)]
        )
        self.assert_payload_rejected(
            failures,
            "payload.pyc",
            "Python bytecode is not part of the reviewed vendored copy",
        )

    def test_skill_lock_rejects_tracked_pycache_payload(self) -> None:
        def mutate(skill_root: Path) -> None:
            (skill_root / "__pycache__").mkdir()
            (skill_root / "__pycache__" / "payload.bin").write_bytes(b"x")

        failures = self.mutated_skill_lock_failures(
            mutate, [("__pycache__/payload.bin", "100644", 0)]
        )
        self.assert_payload_rejected(
            failures,
            "__pycache__/payload.bin",
            "__pycache__/ is not part of the reviewed vendored copy",
        )

    def test_skill_lock_rejects_nested_pycache_component(self) -> None:
        def mutate(skill_root: Path) -> None:
            (skill_root / "scripts" / "__pycache__").mkdir()
            (skill_root / "scripts" / "__pycache__" / "x.py").write_bytes(b"x")

        failures = self.mutated_skill_lock_failures(
            mutate, [("scripts/__pycache__/x.py", "100644", 0)]
        )
        self.assert_payload_rejected(
            failures,
            "scripts/__pycache__/x.py",
            "__pycache__/ is not part of the reviewed vendored copy",
        )

    def test_skill_lock_rejects_upstream_skipped_directories(self) -> None:
        for directory in ("__pypackages__", "node_modules", ".git"):
            with self.subTest(directory=directory):

                def mutate(skill_root: Path) -> None:
                    (skill_root / directory).mkdir()
                    (skill_root / directory / "x").write_bytes(b"x")

                failures = self.mutated_skill_lock_failures(
                    mutate, [(f"{directory}/x", "100644", 0)]
                )
                self.assert_payload_rejected(
                    failures,
                    f"{directory}/x",
                    f"{directory}/ is not part of the reviewed vendored copy",
                )

    def test_skill_lock_rejects_tracked_symlinks(self) -> None:
        def mutate(skill_root: Path) -> None:
            os.symlink("missing-target", skill_root / "dangling")
            os.symlink("SKILL.md", skill_root / "alias.md")

        failures = self.mutated_skill_lock_failures(
            mutate,
            [("dangling", "120000", 0), ("alias.md", "120000", 0)],
        )
        for relative in ("dangling", "alias.md"):
            self.assert_payload_rejected(
                failures,
                relative,
                "tracked symlinks are not part of the reviewed vendored copy",
            )

    def test_skill_lock_rejects_tracked_gitlink(self) -> None:
        def mutate(skill_root: Path) -> None:
            (skill_root / "vendor").mkdir()

        failures = self.mutated_skill_lock_failures(
            mutate, [("vendor", "160000", 0)]
        )
        self.assert_payload_rejected(
            failures,
            "vendor",
            "tracked mode 160000 is not a regular file blob",
        )

    def test_skill_lock_rejects_tracked_entry_missing_from_tree(self) -> None:
        failures = self.mutated_skill_lock_failures(
            None, [("ghost.md", "100644", 0)]
        )
        self.assert_payload_rejected(
            failures,
            "ghost.md",
            "tracked entry is missing from the working tree",
        )

    def test_skill_lock_rejects_tracked_entry_under_a_file(self) -> None:
        failures = self.mutated_skill_lock_failures(
            None, [("SKILL.md/x", "100644", 0)]
        )
        self.assertTrue(
            any(
                item.startswith(
                    f"{self.LOCK_LABEL}: SKILL.md/x: tracked entry could not "
                    "be inspected: "
                )
                for item in failures
            ),
            failures,
        )
        self.assertFalse(any(self.HASH_MISMATCH in item for item in failures))

    @unittest.skipUnless(hasattr(os, "mkfifo"), "FIFOs are not supported")
    def test_skill_lock_rejects_tracked_fifo(self) -> None:
        def mutate(skill_root: Path) -> None:
            os.mkfifo(skill_root / "pipe")

        failures = self.mutated_skill_lock_failures(
            mutate, [("pipe", "100644", 0)]
        )
        self.assert_payload_rejected(
            failures, "pipe", "tracked entry is not a regular file"
        )

    def test_skill_lock_never_hashes_through_a_file_symlink(self) -> None:
        # A blob-mode index entry whose checkout is a symlink to a regular
        # file used to be hashed by accident (Path.is_file follows links).
        def mutate(skill_root: Path) -> None:
            os.symlink("SKILL.md", skill_root / "alias.md")

        failures = self.mutated_skill_lock_failures(
            mutate, [("alias.md", "100644", 0)]
        )
        self.assert_payload_rejected(
            failures, "alias.md", "tracked entry is not a regular file"
        )

    def test_skill_lock_rejects_non_utf8_tracked_path(self) -> None:
        failures = self.mutated_skill_lock_failures(
            None, [("bad\udcff", "100644", 0)]
        )
        self.assert_payload_rejected(
            failures, "bad\udcff", "tracked path is not valid UTF-8"
        )

    def test_skill_lock_rejects_symlinked_skill_root_entry(self) -> None:
        for relative in (".", ""):
            with self.subTest(relative=repr(relative)):
                failures: list[str] = []
                validator.validate_skill_payload(
                    self.LOCK_LABEL,
                    validator.SKILLS_ROOT / self.VENDORED_SKILL,
                    [validator.SkillPayloadEntry(relative, "120000", 0)],
                    failures,
                )
                self.assertEqual(
                    failures,
                    [
                        f"{self.LOCK_LABEL}: .: tracked symlinks are not "
                        "part of the reviewed vendored copy"
                    ],
                )

    def test_skill_lock_rejects_empty_tracked_payload(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            self.copy_lock_fixtures(root)
            skills_root = root / ".claude" / "skills"
            self.copy_vendored_skill(skills_root / self.VENDORED_SKILL)
            failures: list[str] = []
            validator.validate_skill_lock(
                failures, root=root, skills_root=skills_root, payload_entries=[]
            )
        self.assertEqual(len(failures), 1, failures)
        self.assertTrue(
            failures[0].startswith(f"{self.LOCK_LABEL}: no tracked payload under ")
        )

    def test_skill_lock_rejects_unmerged_tracked_entry(self) -> None:
        failures = self.mutated_skill_lock_failures(
            None, [("SKILL.md", "100644", 2)]
        )
        self.assert_payload_rejected(
            failures, "SKILL.md", "tracked entry is unmerged"
        )

    def test_skill_lock_emits_first_rejection_reason_only(self) -> None:
        # A bytecode file inside __pycache__ that is also missing from the
        # working tree produces exactly one line: the suffix rule wins.
        failures = self.mutated_skill_lock_failures(
            None, [("__pycache__/x.pyc", "100644", 0)]
        )
        offending = [item for item in failures if "__pycache__/x.pyc" in item]
        self.assertEqual(
            offending,
            [
                f"{self.LOCK_LABEL}: __pycache__/x.pyc: Python bytecode is "
                "not part of the reviewed vendored copy"
            ],
        )

    def test_skill_lock_keeps_policy_check_after_payload_rejection(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            self.copy_lock_fixtures(root)
            (root / "docs" / "AGENT_TOOLING.md").write_text("", encoding="utf-8")
            skills_root = root / ".claude" / "skills"
            payload = self.copy_vendored_skill(skills_root / self.VENDORED_SKILL)
            payload.append(validator.SkillPayloadEntry("ghost.md", "100644", 0))
            failures: list[str] = []
            validator.validate_skill_lock(
                failures,
                root=root,
                skills_root=skills_root,
                payload_entries=payload,
            )
        self.assertIn(
            f"{self.LOCK_LABEL}: ghost.md: tracked entry is missing from the "
            "working tree",
            failures,
        )
        self.assertIn(
            "docs/AGENT_TOOLING.md: missing computedHash for i-have-an-issue",
            failures,
        )
        self.assertFalse(any(self.HASH_MISMATCH in item for item in failures))

    def test_skill_lock_hashes_extra_tracked_regular_file(self) -> None:
        def mutate(skill_root: Path) -> None:
            (skill_root / "evil.py").write_bytes(b"print('evil')\n")

        failures = self.mutated_skill_lock_failures(
            mutate, [("evil.py", "100644", 0)]
        )
        self.assertEqual(len(failures), 1, failures)
        self.assertIn(self.HASH_MISMATCH, failures[0])

    def test_skill_lock_ignores_untracked_bytecode_noise(self) -> None:
        def mutate(skill_root: Path) -> None:
            cache = skill_root / "scripts" / "__pycache__"
            cache.mkdir()
            (cache / "search_github.cpython-313.pyc").write_bytes(b"\x00")

        self.assertEqual(self.mutated_skill_lock_failures(mutate), [])

    def test_parse_git_ls_files_stage_records(self) -> None:
        sha = "0" * 40
        output = (
            f"100644 {sha} 0\tSKILL.md\0"
            f"120000 {sha} 0\tlink\0"
            f"160000 {sha} 0\tvendor\0"
            f"100644 {sha} 2\tconflict.md\0"
        ).encode("utf-8") + b"100644 " + sha.encode() + b" 0\tbad\xff\0"
        records = validator.parse_git_ls_files_stage(output)
        self.assertEqual(
            records,
            [
                validator.TrackedIndexRecord("SKILL.md", "100644", 0),
                validator.TrackedIndexRecord("link", "120000", 0),
                validator.TrackedIndexRecord("vendor", "160000", 0),
                validator.TrackedIndexRecord("conflict.md", "100644", 2),
                validator.TrackedIndexRecord("bad\udcff", "100644", 0),
            ],
        )
        for malformed in (b"100644 " + sha.encode() + b"\tno-stage\0", b"junk\0"):
            with self.subTest(record=malformed):
                with self.assertRaises(RuntimeError):
                    validator.parse_git_ls_files_stage(malformed)
        with self.assertRaises(RuntimeError):
            validator.parse_git_ls_files_stage(
                b"100644 " + sha.encode() + b" x\tSKILL.md\0"
            )

    def test_skill_lock_reports_unenumerable_payload(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            self.copy_lock_fixtures(root)
            skills_root = root / "skills"
            self.copy_vendored_skill(skills_root / self.VENDORED_SKILL)
            failures: list[str] = []
            validator.validate_skill_lock(
                failures, root=root, skills_root=skills_root
            )
        self.assertEqual(len(failures), 1, failures)
        self.assertTrue(
            failures[0].startswith(
                f"{self.LOCK_LABEL}: could not enumerate tracked payload: "
            ),
            failures,
        )
        self.assertFalse(any(self.HASH_MISMATCH in item for item in failures))

    def test_skill_payload_entries_rejects_root_outside_repository(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory).resolve()
            repo = base / "repo"
            skill_root = repo / ".claude" / "skills" / self.VENDORED_SKILL
            self.copy_vendored_skill(skill_root)
            subprocess.run(["git", "init", "--quiet", str(repo)], check=True)
            os.symlink(repo, base / "link")
            spelled = base / "link" / ".claude" / "skills" / self.VENDORED_SKILL
            with self.assertRaisesRegex(
                RuntimeError, "skill root is outside the repository root"
            ):
                validator.skill_payload_entries(spelled, repo)

    def test_skill_payload_entries_rejects_records_outside_prefix(self) -> None:
        # Anchoring below the git toplevel makes ``--full-name`` records
        # disagree with the root-relative prefix; that must fail closed.
        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory).resolve()
            anchor = repo / "sub"
            skill_root = anchor / self.VENDORED_SKILL
            self.copy_vendored_skill(skill_root)
            subprocess.run(["git", "init", "--quiet", str(repo)], check=True)
            subprocess.run(
                ["git", "-C", str(repo), "add", "-f", "--", "sub"], check=True
            )
            with self.assertRaisesRegex(RuntimeError, "outside"):
                validator.skill_payload_entries(skill_root, anchor)

    def test_skill_lock_rejects_wrong_case_rejected_directory(self) -> None:
        def mutate(skill_root: Path) -> None:
            (skill_root / "__PYCACHE__").mkdir()
            (skill_root / "__PYCACHE__" / "note.txt").write_bytes(b"x")

        failures = self.mutated_skill_lock_failures(
            mutate, [("__PYCACHE__/note.txt", "100644", 0)]
        )
        self.assert_payload_rejected(
            failures,
            "__PYCACHE__/note.txt",
            "__PYCACHE__/ is not part of the reviewed vendored copy",
        )

    def test_skill_lock_rejects_wrong_case_bytecode_suffix(self) -> None:
        def mutate(skill_root: Path) -> None:
            (skill_root / "payload.PYC").write_bytes(b"\x00bytecode")

        failures = self.mutated_skill_lock_failures(
            mutate, [("payload.PYC", "100644", 0)]
        )
        self.assert_payload_rejected(
            failures,
            "payload.PYC",
            "Python bytecode is not part of the reviewed vendored copy",
        )

    def test_skill_lock_rejects_tracked_git_component_beside_nested_repo(
        self,
    ) -> None:
        def mutate(skill_root: Path) -> None:
            subprocess.run(
                ["git", "init", "--quiet", str(skill_root)], check=True
            )

        failures = self.mutated_skill_lock_failures(
            mutate, [(".git/config", "100644", 0)]
        )
        self.assert_payload_rejected(
            failures,
            ".git/config",
            ".git/ is not part of the reviewed vendored copy",
        )

    def test_skill_lock_end_to_end_ignores_nested_repository_boundary(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory).resolve()
            self.copy_lock_fixtures(repo)
            skills_root = repo / ".claude" / "skills"
            skill_root = skills_root / self.VENDORED_SKILL
            expected = self.copy_vendored_skill(skill_root)
            subprocess.run(["git", "init", "--quiet", str(repo)], check=True)
            subprocess.run(
                ["git", "-C", str(repo), "add", "-f", "--", ".claude"],
                check=True,
            )
            # A nested repository boundary appears inside the skill after the
            # outer index was populated; its empty index must not answer.
            subprocess.run(
                ["git", "init", "--quiet", str(skill_root)], check=True
            )
            (skill_root / "untracked-anywhere.txt").write_bytes(b"x")
            self.assertEqual(
                subprocess.run(
                    ["git", "-C", str(skill_root), "ls-files", "--stage"],
                    check=True,
                    stdout=subprocess.PIPE,
                ).stdout,
                b"",
            )

            entries = validator.skill_payload_entries(skill_root, repo)
            self.assertEqual(
                sorted(entry.relative for entry in entries),
                sorted(entry.relative for entry in expected),
            )
            failures: list[str] = []
            validator.validate_skill_lock(
                failures, root=repo, skills_root=skills_root
            )
        self.assertEqual(failures, [])

    def test_skill_lock_end_to_end_rejects_tracked_nested_repository(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory).resolve()
            self.copy_lock_fixtures(repo)
            skills_root = repo / ".claude" / "skills"
            skill_root = skills_root / self.VENDORED_SKILL
            self.copy_vendored_skill(skill_root)
            nested = skill_root / "vendor"
            nested.mkdir()
            (nested / "x").write_bytes(b"x")
            subprocess.run(["git", "init", "--quiet", str(nested)], check=True)
            subprocess.run(
                ["git", "-C", str(nested), "add", "--", "x"], check=True
            )
            subprocess.run(
                [
                    "git",
                    "-C",
                    str(nested),
                    "-c",
                    "user.name=test",
                    "-c",
                    "user.email=test@example.invalid",
                    "commit",
                    "--quiet",
                    "-m",
                    "nested",
                ],
                check=True,
            )
            subprocess.run(["git", "init", "--quiet", str(repo)], check=True)
            subprocess.run(
                ["git", "-C", str(repo), "add", "-f", "--", ".claude"],
                check=True,
            )

            entries = validator.skill_payload_entries(skill_root, repo)
            self.assertIn(
                validator.SkillPayloadEntry("vendor", "160000", 0), entries
            )
            failures: list[str] = []
            validator.validate_skill_lock(
                failures, root=repo, skills_root=skills_root
            )
        self.assert_payload_rejected(
            failures, "vendor", "tracked mode 160000 is not a regular file blob"
        )
        self.assertEqual(len(failures), 1, failures)

    def test_skill_lock_end_to_end_rejects_force_added_payload(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory).resolve()
            self.copy_lock_fixtures(repo)
            skills_root = repo / ".claude" / "skills"
            skill_root = skills_root / self.VENDORED_SKILL
            self.copy_vendored_skill(skill_root)
            (skill_root / "payload.pyc").write_bytes(b"\x00bytecode")
            os.symlink("SKILL.md", skill_root / "alias.md")
            subprocess.run(["git", "init", "--quiet", str(repo)], check=True)
            subprocess.run(
                ["git", "-C", str(repo), "add", "-f", "--", ".claude"],
                check=True,
            )

            entries = validator.skill_payload_entries(skill_root, repo)
            self.assertIn(
                validator.SkillPayloadEntry("payload.pyc", "100644", 0), entries
            )
            self.assertIn(
                validator.SkillPayloadEntry("alias.md", "120000", 0), entries
            )
            self.assertIn(
                validator.SkillPayloadEntry("SKILL.md", "100644", 0), entries
            )

            failures: list[str] = []
            validator.validate_skill_lock(
                failures, root=repo, skills_root=skills_root
            )
        self.assert_payload_rejected(
            failures,
            "payload.pyc",
            "Python bytecode is not part of the reviewed vendored copy",
        )
        self.assert_payload_rejected(
            failures,
            "alias.md",
            "tracked symlinks are not part of the reviewed vendored copy",
        )
        self.assertEqual(len(failures), 2, failures)

    def test_skill_lock_end_to_end_rejects_symlinked_skill_root(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory).resolve()
            self.copy_lock_fixtures(repo)
            self.copy_vendored_skill(repo / "vendored" / self.VENDORED_SKILL)
            skills_root = repo / ".claude" / "skills"
            skills_root.mkdir(parents=True)
            os.symlink(
                Path("..") / ".." / "vendored" / self.VENDORED_SKILL,
                skills_root / self.VENDORED_SKILL,
            )
            subprocess.run(["git", "init", "--quiet", str(repo)], check=True)
            subprocess.run(
                ["git", "-C", str(repo), "add", "-f", "--", ".claude", "vendored"],
                check=True,
            )
            self.assertTrue((skills_root / self.VENDORED_SKILL / "SKILL.md").is_file())

            failures: list[str] = []
            validator.validate_skill_lock(
                failures, root=repo, skills_root=skills_root
            )
        self.assertEqual(
            failures,
            [
                f"{self.LOCK_LABEL}: .: tracked symlinks are not part of the "
                "reviewed vendored copy"
            ],
        )

    def test_skill_lock_end_to_end_accepts_clean_tracked_copy(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory).resolve()
            self.copy_lock_fixtures(repo)
            skills_root = repo / ".claude" / "skills"
            skill_root = skills_root / self.VENDORED_SKILL
            self.copy_vendored_skill(skill_root)
            subprocess.run(["git", "init", "--quiet", str(repo)], check=True)
            subprocess.run(
                ["git", "-C", str(repo), "add", "-f", "--", ".claude"],
                check=True,
            )
            # Untracked bytecode next to the tracked payload must not matter.
            (skill_root / "scripts" / "__pycache__").mkdir()
            (skill_root / "scripts" / "__pycache__" / "x.pyc").write_bytes(b"\x00")

            failures: list[str] = []
            validator.validate_skill_lock(
                failures, root=repo, skills_root=skills_root
            )
        self.assertEqual(failures, [])

    ALPHA_PROMOTION_CANDIDATES = (
        "issue-implement",
        "issue-select",
        "issue-to-plan",
        "next-milestone",
        "pycc",
        "pycc-feedback",
        "ultra-review",
    )

    def test_alpha_promotion_requires_both_authenticated_client_evals(
        self,
    ) -> None:
        for name in sorted(validator.ALPHA_EVAL_RUNNERS):
            shapes = {
                "absent": {},
                "codex-only": {
                    name: {"codex": "https://example.test/codex-eval"}
                },
                "claude-only": {
                    name: {"claude": "https://example.test/claude-eval"}
                },
            }
            for shape, evidence in shapes.items():
                with self.subTest(name=name, shape=shape), mock.patch.object(
                    validator, "AUTHENTICATED_MODEL_EVAL_EVIDENCE", evidence
                ):
                    failures: list[str] = []
                    validator.validate_alpha_promotion_gate(
                        {name: {"source": "future"}},
                        failures,
                    )
                    self.assertEqual(len(failures), 1)
                    self.assertIn(name, failures[0])
                    self.assertIn(
                        "authenticated Codex and Claude model-eval evidence",
                        failures[0],
                    )

        failures = []
        validator.validate_alpha_promotion_gate(
            {name: {"source": "future"} for name in validator.ALPHA_EVAL_RUNNERS},
            failures,
        )
        self.assertEqual(
            failures,
            [
                f"skills-lock.json: {name} cannot be promoted without "
                "authenticated Codex and Claude model-eval evidence"
                for name in sorted(validator.ALPHA_EVAL_RUNNERS)
            ],
        )

    def test_alpha_promotion_accepts_complete_authenticated_evidence(
        self,
    ) -> None:
        for name in sorted(validator.ALPHA_EVAL_RUNNERS):
            evidence = {
                name: {
                    "codex": "https://example.test/codex-eval",
                    "claude": "https://example.test/claude-eval",
                }
            }
            with self.subTest(name=name), mock.patch.object(
                validator, "AUTHENTICATED_MODEL_EVAL_EVIDENCE", evidence
            ):
                failures: list[str] = []
                validator.validate_alpha_promotion_gate(
                    {name: {"source": "future"}},
                    failures,
                )
                self.assertEqual(failures, [])

    def test_alpha_promotion_rejects_non_https_evidence(self) -> None:
        evidence = {
            "pycc": {
                "codex": "http://example.test/codex-eval",
                "claude": "https://example.test/claude-eval",
            }
        }
        with mock.patch.object(
            validator, "AUTHENTICATED_MODEL_EVAL_EVIDENCE", evidence
        ):
            failures: list[str] = []
            validator.validate_alpha_promotion_gate(
                {"pycc": {"source": "future"}},
                failures,
            )
        self.assertEqual(len(failures), 1)
        self.assertIn("pycc cannot be promoted", failures[0])

    def test_alpha_promotion_set_is_derived_from_the_eval_runner_table(
        self,
    ) -> None:
        """The promotion gate reads ``ALPHA_EVAL_RUNNERS`` at call time.

        ``ALPHA_PROMOTION_CANDIDATES`` below is a test-side pin, not a second
        production copy of the inventory: its only job is to turn an inventory
        change into a reviewable test diff, so it must be edited whenever a
        skill is added to or removed from ``ALPHA_EVAL_RUNNERS``.
        """
        self.assertEqual(
            tuple(sorted(validator.ALPHA_EVAL_RUNNERS)),
            self.ALPHA_PROMOTION_CANDIDATES,
        )
        locked = {"future-alpha-skill": {"source": "future"}}

        failures: list[str] = []
        validator.validate_alpha_promotion_gate(locked, failures)
        self.assertEqual(failures, [])

        with mock.patch.dict(
            validator.ALPHA_EVAL_RUNNERS,
            {"future-alpha-skill": {"future_runner"}},
        ):
            failures = []
            validator.validate_alpha_promotion_gate(locked, failures)
        self.assertEqual(len(failures), 1)
        self.assertIn("future-alpha-skill cannot be promoted", failures[0])

        failures = []
        validator.validate_alpha_promotion_gate(locked, failures)
        self.assertEqual(failures, [])

    def test_alpha_skill_count_prose_rejects_stale_spelled_out_count(
        self,
    ) -> None:
        failures: list[str] = []
        validator.validate_alpha_skill_count_prose(
            "intro line.\n"
            "`EXPECTED_RUNNERS` in that script names all six alpha skills.\n",
            failures,
        )
        expected = len(validator.ALPHA_EVAL_RUNNERS)
        self.assertEqual(
            failures,
            [
                "docs/AGENT_TOOLING.md:2: literal alpha-skill count 6 "
                f"disagrees with ALPHA_EVAL_RUNNERS ({expected})"
            ],
        )

    def test_alpha_skill_count_prose_rejects_stale_count_after_table_mention(
        self,
    ) -> None:
        failures: list[str] = []
        validator.validate_alpha_skill_count_prose(
            "covers every skill in `ALPHA_EVAL_RUNNERS` "
            "(6 at the time of writing), and none of them can enter.\n",
            failures,
        )
        expected = len(validator.ALPHA_EVAL_RUNNERS)
        self.assertEqual(
            failures,
            [
                "docs/AGENT_TOOLING.md:1: literal alpha-skill count 6 "
                f"disagrees with ALPHA_EVAL_RUNNERS ({expected})"
            ],
        )

    def test_alpha_skill_count_prose_accepts_the_table_length(self) -> None:
        failures: list[str] = []
        with mock.patch.dict(
            validator.ALPHA_EVAL_RUNNERS, {"a": set(), "b": set()}, clear=True
        ):
            validator.validate_alpha_skill_count_prose(
                "names all two alpha skills. `ALPHA_EVAL_RUNNERS` "
                "(2 at the time of writing) lists 2 skills.\n",
                failures,
            )
        self.assertEqual(failures, [])

    def test_alpha_skill_count_prose_ignores_unrelated_numerals(self) -> None:
        failures: list[str] = []
        with mock.patch.dict(
            validator.ALPHA_EVAL_RUNNERS, {"only": set()}, clear=True
        ):
            validator.validate_alpha_skill_count_prose(
                "enforcing at least two evals on every alpha skill's file "
                "in `ALPHA_EVAL_RUNNERS`. Tier-1 coverage since PR #255 "
                "(2026-08-23) is 100% and one thing stays deferred.\n"
                "The v0.3 release lists 12 project-wide checks.\n",
                failures,
            )
        self.assertEqual(failures, [])

    def test_alpha_skill_count_prose_ignores_numerals_that_are_not_counts(
        self,
    ) -> None:
        failures: list[str] = []
        with mock.patch.dict(
            validator.ALPHA_EVAL_RUNNERS, {"only": set()}, clear=True
        ):
            validator.validate_alpha_skill_count_prose(
                "The two clients cover all one alpha skills.\n"
                "Issue #260 covers every alpha skill.\n"
                "PR 255 landed the evals of the one project-local alpha "
                "skill.\n",
                failures,
            )
        self.assertEqual(failures, [])

    def test_alpha_skill_count_prose_accepts_the_tracked_policy(self) -> None:
        failures: list[str] = []
        validator.validate_alpha_skill_count_prose(
            (validator.ROOT / "docs" / "AGENT_TOOLING.md").read_text(
                encoding="utf-8"
            ),
            failures,
        )
        self.assertEqual(failures, [])

    def test_alpha_promotion_ignores_vendored_non_alpha_skills(self) -> None:
        failures: list[str] = []
        validator.validate_alpha_promotion_gate(
            {"i-have-an-issue": {"source": "vercel-labs/skills"}},
            failures,
        )
        self.assertEqual(failures, [])

    def alpha_contract_failures(
        self,
        *,
        remove_feedback_text: str | None = None,
        pycc_eval_count: int | None = None,
        remove_pycc_runner: bool = False,
        remove_feedback_runner: bool = False,
    ) -> list[str]:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in ("pycc", "pycc-feedback"):
                shutil.copytree(validator.SKILLS_ROOT / name, root / name)
            if remove_feedback_text is not None:
                feedback_path = root / "pycc-feedback" / "SKILL.md"
                feedback_path.write_text(
                    feedback_path.read_text(encoding="utf-8").replace(
                        remove_feedback_text,
                        "",
                    ),
                    encoding="utf-8",
                )
            if pycc_eval_count is not None:
                evals_path = root / "pycc" / "evals" / "evals.json"
                evals = json.loads(evals_path.read_text(encoding="utf-8"))
                evals["evals"] = evals["evals"][:pycc_eval_count]
                evals_path.write_text(json.dumps(evals), encoding="utf-8")
            if remove_pycc_runner:
                evals_path = root / "pycc" / "evals" / "evals.json"
                evals = json.loads(evals_path.read_text(encoding="utf-8"))
                evals["evals"][0].pop("runner")
                evals_path.write_text(json.dumps(evals), encoding="utf-8")
            if remove_feedback_runner:
                evals_path = root / "pycc-feedback" / "evals" / "evals.json"
                evals = json.loads(evals_path.read_text(encoding="utf-8"))
                evals["evals"][0].pop("runner")
                evals_path.write_text(json.dumps(evals), encoding="utf-8")
            failures: list[str] = []
            validator.validate_alpha_skill_contracts(
                root,
                failures,
                root=root,
            )
            return failures

    def test_feedback_skill_requires_outbound_query_sanitization(self) -> None:
        failures = self.alpha_contract_failures(
            remove_feedback_text="sanitize every outbound query"
        )
        self.assertTrue(
            any("sanitize every outbound query" in item for item in failures)
        )

    def test_feedback_skill_preserves_the_accepted_pr5_boundary(self) -> None:
        for contract in (
            "using print()'s result as a nested expression is not supported yet",
            "intentional temporary",
        ):
            with self.subTest(contract=contract):
                failures = self.alpha_contract_failures(
                    remove_feedback_text=contract
                )
                self.assertTrue(any(contract in item for item in failures))

    def test_alpha_skill_requires_multiple_evals(self) -> None:
        failures = self.alpha_contract_failures(pycc_eval_count=1)
        self.assertTrue(any("at least two evals" in item for item in failures))

    def test_alpha_skill_requires_executable_eval_runners(self) -> None:
        for option in ("remove_pycc_runner", "remove_feedback_runner"):
            with self.subTest(option=option):
                failures = self.alpha_contract_failures(**{option: True})
                self.assertTrue(
                    any("malformed eval" in item for item in failures)
                )

    def test_validate_alpha_skill_contracts_covers_issue_implement(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in (
                "pycc",
                "pycc-feedback",
                "issue-to-plan",
                "issue-implement",
                "issue-select",
            ):
                shutil.copytree(validator.SKILLS_ROOT / name, root / name)
            evals_path = root / "issue-implement" / "evals" / "evals.json"
            evals_data = json.loads(evals_path.read_text(encoding="utf-8"))
            evals_data["evals"][1]["id"] = evals_data["evals"][0]["id"]
            evals_path.write_text(json.dumps(evals_data), encoding="utf-8")

            failures: list[str] = []
            validator.validate_alpha_skill_contracts(root, failures, root=root)
            self.assertTrue(any("eval ids must be unique" in item for item in failures))

    def write_skill(
        self,
        root: Path,
        name: str,
        description: str,
        body: str,
        *,
        extra_frontmatter: str = "",
    ) -> Path:
        skill_root = root / name
        skill_root.mkdir(parents=True)
        path = skill_root / "SKILL.md"
        path.write_text(
            f"---\nname: {name}\ndescription: {description}\n"
            f"{extra_frontmatter}---\n\n{body}\n",
            encoding="utf-8",
        )
        return path

    def parity_failures(
        self,
        wrapper_body: str,
        *,
        wrapper_name: str = "example",
        wrapper_description: str = "Example workflow.",
    ) -> list[str]:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            canonical_root = root / "canonical"
            codex_root = root / "codex"
            self.write_skill(
                canonical_root,
                "example",
                "Example workflow.",
                "# Canonical",
            )
            self.write_skill(
                codex_root,
                wrapper_name,
                wrapper_description,
                wrapper_body,
            )
            failures: list[str] = []
            validator.validate_skill_parity(
                canonical_root,
                codex_root,
                failures,
            )
            return failures

    def test_codex_wrapper_loads_the_complete_canonical_skill(self) -> None:
        self.assertEqual(
            self.parity_failures(
                "Read `.claude/skills/example/SKILL.md` completely as the "
                "canonical workflow."
            ),
            [],
        )

    def test_codex_wrapper_with_wrong_canonical_target_is_rejected(self) -> None:
        failures = self.parity_failures(
            "Read `.claude/skills/other/SKILL.md` completely as the canonical workflow."
        )
        self.assertTrue(
            any("must reference exactly" in failure for failure in failures)
        )

    def test_codex_and_claude_skill_descriptions_must_match(self) -> None:
        failures = self.parity_failures(
            "Read `.claude/skills/example/SKILL.md` completely as the "
            "canonical workflow.",
            wrapper_description="Different triggers.",
        )
        self.assertTrue(
            any("description must match" in failure for failure in failures)
        )

    def test_codex_wrapper_set_must_match_canonical_skill_set(self) -> None:
        failures = self.parity_failures(
            "Read `.claude/skills/other/SKILL.md` completely as the "
            "canonical workflow.",
            wrapper_name="other",
        )
        self.assertTrue(any("missing canonical" in failure for failure in failures))
        self.assertTrue(any("without canonical" in failure for failure in failures))

    def test_explicit_only_canonical_skill_requires_a_codex_guard(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            canonical_root = root / "canonical"
            codex_root = root / "codex"
            self.write_skill(
                canonical_root,
                "example",
                "Explicit invocation only; never select this skill implicitly. "
                "Example workflow.",
                "# Canonical",
                extra_frontmatter="disable-model-invocation: true\n",
            )
            self.write_skill(
                codex_root,
                "example",
                "Explicit invocation only; never select this skill implicitly. "
                "Example workflow.",
                "Read `.claude/skills/example/SKILL.md` completely as the "
                "canonical workflow.",
            )
            failures: list[str] = []
            validator.validate_skill_parity(
                canonical_root,
                codex_root,
                failures,
            )
            self.assertTrue(
                any("explicit-only invocation gate" in failure for failure in failures)
            )

    def test_explicit_only_codex_guard_is_accepted(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            canonical_root = root / "canonical"
            codex_root = root / "codex"
            self.write_skill(
                canonical_root,
                "example",
                "Explicit invocation only; never select this skill implicitly. "
                "Example workflow.",
                "# Canonical",
                extra_frontmatter="disable-model-invocation: true\n",
            )
            self.write_skill(
                codex_root,
                "example",
                "Explicit invocation only; never select this skill implicitly. "
                "Example workflow.",
                "The canonical workflow is explicit-only. Continue only when "
                "the user names `$example`. If selected implicitly, stop without "
                "writing files. Read `.claude/skills/example/SKILL.md` completely "
                "as the canonical workflow.",
            )
            failures: list[str] = []
            validator.validate_skill_parity(
                canonical_root,
                codex_root,
                failures,
            )
            self.assertEqual(failures, [])

    def dispatch_failures(
        self,
        wrapper_body: str,
        *,
        canonical_requires_dispatch: bool = True,
    ) -> list[str]:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            canonical_root = root / "canonical"
            codex_root = root / "codex"
            self.write_skill(
                canonical_root,
                "example",
                "Example workflow.",
                "# Canonical",
                extra_frontmatter=(
                    "requires-agent-dispatch: true\n"
                    if canonical_requires_dispatch
                    else ""
                ),
            )
            self.write_skill(
                codex_root,
                "example",
                "Example workflow.",
                wrapper_body,
            )
            failures: list[str] = []
            validator.validate_skill_parity(
                canonical_root,
                codex_root,
                failures,
            )
            return failures

    def dispatch_wrapper_body(self) -> str:
        return (
            "Read `.claude/skills/example/SKILL.md` completely as the canonical "
            "workflow. This skill explicitly asks for sub-agents, delegation, and "
            "parallel agent work, so use `spawn_agent` and join with `wait_agent`. "
            "If sub-agent dispatch is unavailable in this session, run the step "
            "inline and say so."
        )

    def test_dispatch_mapping_in_the_codex_wrapper_is_accepted(self) -> None:
        self.assertEqual(self.dispatch_failures(self.dispatch_wrapper_body()), [])

    def test_dispatch_wrapper_without_a_named_fallback_is_rejected(self) -> None:
        body = self.dispatch_wrapper_body()
        truncated = body[: body.index("If sub-agent dispatch is unavailable")].rstrip()
        failures = self.dispatch_failures(truncated)
        self.assertTrue(
            any(
                "must map the canonical sub-agent dispatch step to a Codex "
                "capability with a named fallback" in failure
                for failure in failures
            )
        )

    def test_dispatch_wrapper_missing_any_required_literal_is_rejected(self) -> None:
        for literal in (
            "explicitly asks for sub-agents, delegation, and parallel agent work",
            "`spawn_agent`",
            "`wait_agent`",
            "If sub-agent dispatch is unavailable",
        ):
            with self.subTest(literal=literal):
                body = self.dispatch_wrapper_body().replace(literal, "REDACTED")
                failures = self.dispatch_failures(body)
                self.assertTrue(
                    any("named fallback" in failure for failure in failures)
                )

    def test_unmarked_canonical_skill_needs_no_dispatch_mapping(self) -> None:
        self.assertEqual(
            self.dispatch_failures(
                "Read `.claude/skills/example/SKILL.md` completely as the "
                "canonical workflow.",
                canonical_requires_dispatch=False,
            ),
            [],
        )

    def instruction_parity_failures(
        self,
        claude_text: str,
        *,
        include_agents: bool = True,
        agents_text: str | None = None,
    ) -> list[str]:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            if include_agents:
                if agents_text is None:
                    agents_text = self.valid_agent_instructions()
                (root / "AGENTS.md").write_text(
                    agents_text,
                    encoding="utf-8",
                )
            (root / "CLAUDE.md").write_text(claude_text, encoding="utf-8")
            failures: list[str] = []
            validator.validate_instruction_parity(failures, root)
            return failures

    @staticmethod
    def valid_agent_instructions() -> str:
        instructions = validator.REQUIRED_LIVE_MONITORING_INSTRUCTIONS
        return (
            f"# Shared instructions\n\n{instructions[0]}\n\n"
            + "\n".join(f"- {instruction}" for instruction in instructions[1:])
        )

    def test_claude_instructions_import_the_canonical_agents_file(self) -> None:
        self.assertEqual(
            self.instruction_parity_failures("@AGENTS.md\n"),
            [],
        )

    def test_divergent_claude_instructions_are_rejected(self) -> None:
        for claude_text in (
            "# Independent Claude rules\n",
            "@AGENTS.md\nAdditional Claude-only rule.\n",
        ):
            with self.subTest(claude_text=claude_text):
                failures = self.instruction_parity_failures(claude_text)
                self.assertEqual(len(failures), 1)
                self.assertIn("must contain exactly @AGENTS.md", failures[0])

    def test_claude_import_requires_the_canonical_agents_file(self) -> None:
        failures = self.instruction_parity_failures(
            "@AGENTS.md\n",
            include_agents=False,
        )
        self.assertEqual(
            failures,
            ["AGENTS.md: canonical shared instructions are required"],
        )

    def test_live_monitoring_instructions_are_required(self) -> None:
        complete = self.valid_agent_instructions()
        for instruction in validator.REQUIRED_LIVE_MONITORING_INSTRUCTIONS:
            with self.subTest(instruction=instruction):
                failures = self.instruction_parity_failures(
                    "@AGENTS.md\n",
                    agents_text=complete.replace(instruction, "missing", 1),
                )
                self.assertEqual(
                    failures,
                    [
                        "AGENTS.md: missing required live-monitoring instruction: "
                        f"{instruction}"
                    ],
                )

    def test_retired_live_monitoring_policy_in_a_fence_is_rejected(self) -> None:
        fenced = (
            "# Shared instructions\n\nThe former policy is retained for history.\n\n"
            "```text\n"
            + self.valid_agent_instructions()
            + "\n```\n"
        )
        failures = self.instruction_parity_failures(
            "@AGENTS.md\n",
            agents_text=fenced,
        )
        self.assertEqual(
            failures,
            [
                "AGENTS.md: missing required live-monitoring instruction: "
                f"{validator.LIVE_MONITORING_HEADING}"
            ],
        )

    def test_unicode_whitespace_does_not_end_hidden_blocks(self) -> None:
        policy = validator.LIVE_MONITORING_HEADING + "\n" + "\n".join(
            f"- {instruction}"
            for instruction in validator.REQUIRED_LIVE_MONITORING_BULLETS
        )
        for whitespace in ("\u00a0", "\u2003", "\v", "\f"):
            for agents_text in (
                f"```text\nliteral\n```{whitespace}\n{policy}\n```\n",
                f"<agent-policy>\n{whitespace}\n{policy}\n</agent-policy>\n",
            ):
                with self.subTest(whitespace=ascii(whitespace)):
                    failures = self.instruction_parity_failures(
                        "@AGENTS.md\n",
                        agents_text=agents_text,
                    )
                    self.assertEqual(
                        failures,
                        [
                            "AGENTS.md: missing required live-monitoring instruction: "
                            f"{validator.LIVE_MONITORING_HEADING}"
                        ],
                    )

    def test_commented_live_monitoring_policy_is_rejected(self) -> None:
        for commented in (
            f"# Shared instructions\n\n<!--\n{self.valid_agent_instructions()}\n-->\n",
            f"paragraph\n+ <!--\n{self.valid_agent_instructions()}\n-->\n",
            f"- - <!--\n{self.valid_agent_instructions()}\n-->\n",
            f"- > <!--\n{self.valid_agent_instructions()}\n-->\n",
        ):
            with self.subTest(first_line=commented.splitlines()[0]):
                failures = self.instruction_parity_failures(
                    "@AGENTS.md\n",
                    agents_text=commented,
                )
                self.assertEqual(
                    failures,
                    [
                        "AGENTS.md: missing required live-monitoring instruction: "
                        f"{validator.LIVE_MONITORING_HEADING}"
                    ],
                )

    def test_leading_html_comments_cannot_reveal_policy_tokens(self) -> None:
        for prefix in ("<!-- retired -->", "<!--\n-->"):
            with self.subTest(prefix=prefix):
                prefixed = "\n".join(
                    f"{prefix}{line}"
                    for line in self.valid_agent_instructions().splitlines()
                )
                failures = self.instruction_parity_failures(
                    "@AGENTS.md\n",
                    agents_text=prefixed,
                )
                self.assertEqual(
                    failures,
                    [
                        "AGENTS.md: missing required live-monitoring instruction: "
                        f"{validator.LIVE_MONITORING_HEADING}"
                    ],
                )

    def test_raw_html_blocks_cannot_supply_active_policy(self) -> None:
        policy = validator.LIVE_MONITORING_HEADING + "\n" + "\n".join(
            f"- {instruction}"
            for instruction in validator.REQUIRED_LIVE_MONITORING_BULLETS
        )
        for opening, closing in (
            ("<pre>", "</pre>"),
            ("<script>", "</script>"),
            ("<style>", "</style>"),
            ("<div>", "</div>"),
            ("<agent-policy>", "</agent-policy>"),
            ('<agent-policy data-x=">">', "</agent-policy>"),
        ):
            with self.subTest(opening=opening):
                failures = self.instruction_parity_failures(
                    "@AGENTS.md\n",
                    agents_text=f"{opening}\n{policy}\n{closing}\n",
                )
                self.assertEqual(
                    failures,
                    [
                        "AGENTS.md: missing required live-monitoring instruction: "
                        f"{validator.LIVE_MONITORING_HEADING}"
                    ],
                )

    def test_policy_after_raw_html_block_is_active(self) -> None:
        complete = self.valid_agent_instructions()
        for prefix in (
            "<pre>literal</pre>\n",
            "<div>\nliteral\n\n",
            "<agent-policy>\nliteral\n\n",
            "<agent-policy ???>\n",
        ):
            with self.subTest(prefix=prefix.splitlines()[0]):
                self.assertEqual(
                    self.instruction_parity_failures(
                        "@AGENTS.md\n",
                        agents_text=prefix + complete,
                    ),
                    [],
                )

    def test_type_seven_html_blocks_start_after_other_blocks(self) -> None:
        policy = validator.LIVE_MONITORING_HEADING + "\n" + "\n".join(
            f"- {instruction}"
            for instruction in validator.REQUIRED_LIVE_MONITORING_BULLETS
        )
        for prefix in (
            "<pre>literal</pre>\n",
            "```text\nliteral\n```\n",
            "# Prior heading\n",
            "- Prior list item\n",
            "> Prior quote\n",
            "    prior code\n",
            "paragraph\n_ \t_ \t_\n",
            "paragraph\n+ <!--x-->\n",
            "paragraph\n1. <!--x-->\n",
            "- item\nplain\n",
            "> item\nplain\n",
            "- item\n  continued\nplain\n",
            "paragraph\n+     <!--\n",
            "paragraph\n- -     <!--\n",
        ):
            with self.subTest(prefix=prefix.splitlines()[0]):
                failures = self.instruction_parity_failures(
                    "@AGENTS.md\n",
                    agents_text=(
                        prefix
                        + "<agent-policy>\n"
                        + policy
                        + "\n</agent-policy>\n"
                    ),
                )
                self.assertEqual(
                    failures,
                    [
                        "AGENTS.md: missing required live-monitoring instruction: "
                        f"{validator.LIVE_MONITORING_HEADING}"
                    ],
                )

    def test_type_seven_tag_after_paragraph_continuation_is_inline(self) -> None:
        complete = self.valid_agent_instructions()
        for continuation in (
            "2. continuation",
            "1. ",
            "* ",
            "+ ",
            "    code",
            "\tcode",
            "+ <!--x-->\nparagraph",
            "+ <!--\n-->\nparagraph",
        ):
            with self.subTest(continuation=repr(continuation)):
                agents_text = (
                    "paragraph\n"
                    f"{continuation}\n"
                    "<agent-policy>\n"
                    f"{complete}\n"
                    "</agent-policy>\n"
                )
                self.assertEqual(
                    self.instruction_parity_failures(
                        "@AGENTS.md\n",
                        agents_text=agents_text,
                    ),
                    [],
                )

    def test_completed_block_clears_lazy_container_state(self) -> None:
        complete = self.valid_agent_instructions()
        for prefix in (
            "- item\nplain\n```text\ncode\n```\nparagraph\n",
            "- item\nplain\n<div>\ncode\n\nparagraph\n",
            "- # heading\nparagraph\n",
            "- ---\nparagraph\n",
            "> <!--x-->\nparagraph\n",
            "> ```text\n> code\n> ```\nparagraph\n",
            "- - \nparagraph\n",
            "- 1. \nparagraph\n",
            "> - \nparagraph\n",
            "> 1. \nparagraph\n",
            "- - # heading\nparagraph\n",
            "- - <!--x-->\nparagraph\n",
            "> - # heading\nparagraph\n",
        ):
            with self.subTest(prefix=prefix):
                agents_text = (
                    prefix
                    + "<agent-policy>\n"
                    + complete
                    + "\n</agent-policy>\n"
                )
                self.assertEqual(
                    self.instruction_parity_failures(
                        "@AGENTS.md\n",
                        agents_text=agents_text,
                    ),
                    [],
                )

    def test_hidden_fragments_do_not_complete_the_active_section(self) -> None:
        fragments = "\n".join(
            f"- {instruction}"
            for instruction in validator.REQUIRED_LIVE_MONITORING_BULLETS
        )
        for hidden in (
            f"```text\n{fragments}\n```",
            f"<!--\n{fragments}\n-->",
            "\n".join(f"    {line}" for line in fragments.splitlines()),
            "\n".join(f"> {line}" for line in fragments.splitlines()),
        ):
            with self.subTest(hidden=hidden.splitlines()[0]):
                agents_text = (
                    "# Shared instructions\n\n"
                    f"{validator.LIVE_MONITORING_HEADING}\n\n"
                    "This policy is obsolete.\n\n"
                    f"{hidden}\n"
                )
                failures = self.instruction_parity_failures(
                    "@AGENTS.md\n",
                    agents_text=agents_text,
                )
                self.assertIn(
                    "AGENTS.md: missing required live-monitoring instruction: "
                    f"{validator.REQUIRED_LIVE_MONITORING_BULLETS[0]}",
                    failures,
                )

    def test_duplicate_active_live_monitoring_sections_are_rejected(self) -> None:
        complete = self.valid_agent_instructions()
        duplicate = complete + "\n\n" + complete
        failures = self.instruction_parity_failures(
            "@AGENTS.md\n",
            agents_text=duplicate,
        )
        self.assertEqual(
            failures,
            [
                "AGENTS.md: live-monitoring policy must contain exactly one "
                "active section"
            ],
        )

    def test_fragments_outside_the_active_section_are_rejected(self) -> None:
        fragments = "\n".join(
            f"- {instruction}"
            for instruction in validator.REQUIRED_LIVE_MONITORING_BULLETS
        )
        agents_text = (
            "# Shared instructions\n\n"
            "## Retired monitoring rules\n\n"
            f"{fragments}\n\n"
            f"{validator.LIVE_MONITORING_HEADING}\n\n"
            "This policy is obsolete.\n"
        )
        failures = self.instruction_parity_failures(
            "@AGENTS.md\n",
            agents_text=agents_text,
        )
        self.assertIn(
            "AGENTS.md: missing required live-monitoring instruction: "
            f"{validator.REQUIRED_LIVE_MONITORING_BULLETS[0]}",
            failures,
        )

    def test_setext_heading_ends_the_active_section(self) -> None:
        bullets = "\n".join(
            f"- {instruction}"
            for instruction in validator.REQUIRED_LIVE_MONITORING_BULLETS
        )
        for paragraph, underline in (
            ("obsolete\nOther section", "====="),
            ("obsolete\nOther section", "-----"),
            ("paragraph\n2. continuation", "---"),
            ("paragraph\n1. ", "---"),
            ("paragraph\n* ", "---"),
            ("paragraph\n+ ", "---"),
            ("[foo]:", "---"),
            ("paragraph\n[foo]: /url", "---"),
            ("[   ]: /url", "---"),
            ("[foo[bar]: /url", "---"),
            (f"[{'x' * 1000}]: /url", "---"),
            ("[" + r"\*" * 500 + "]: /url", "---"),
            ("[foo]: foo)", "---"),
            ("[foo]: " + "(" * 33 + "url" + ")" * 33, "---"),
            ("[foo]: /url\x01tail", "---"),
            ("[foo]: /url\rtail", "---"),
            ("[foo]: /url\x7ftail", "---"),
            ("[foo]: <a\rb>", "---"),
            ("[foo]: <a\\\rb>", "---"),
            (r"[foo]: foo\ bar", "---"),
            ('[foo]: /url "title\n\nend"', "---"),
        ):
            with self.subTest(paragraph=paragraph, underline=underline):
                agents_text = (
                    f"{validator.LIVE_MONITORING_HEADING}\n\n"
                    f"{paragraph}\n"
                    f"{underline}\n"
                    f"{bullets}\n"
                )
                failures = self.instruction_parity_failures(
                    "@AGENTS.md\n",
                    agents_text=agents_text,
                )
                self.assertIn(
                    "AGENTS.md: missing required live-monitoring instruction: "
                    f"{validator.REQUIRED_LIVE_MONITORING_BULLETS[0]}",
                    failures,
                )

    def test_thematic_break_after_link_definition_stays_in_section(self) -> None:
        for definition in (
            "[foo]: /url",
            r"[foo\]]: /url",
            "[foo]:\n  /url",
            "[foo\nbar]: /url",
            "[foo\n bar]: /url",
            '[foo]: /url\n  "title"',
            '[foo]: /url "multi\n  line"',
            "[foo]: " + "(" * 32 + "url" + ")" * 32,
            "[" + r"\*" * 499 + "]: /url",
        ):
            with self.subTest(definition=definition):
                agents_text = (
                    f"{validator.LIVE_MONITORING_HEADING}\n\n"
                    f"{definition}\n"
                    "---\n"
                    + "\n".join(
                        f"- {instruction}"
                        for instruction in validator.REQUIRED_LIVE_MONITORING_BULLETS
                    )
                )
                self.assertEqual(
                    self.instruction_parity_failures(
                        "@AGENTS.md\n",
                        agents_text=agents_text,
                    ),
                    [],
                )

    def test_angle_reference_destination_rejects_line_endings(self) -> None:
        for destination in (
            "<a\rb>",
            "<a\nb>",
            "<a\\\rb>",
            "<a\\\nb>",
        ):
            with self.subTest(destination=repr(destination)):
                self.assertEqual(
                    validator.markdown_reference_destination_state(destination),
                    (False, None),
                )

    def test_unterminated_reference_state_cannot_supply_policy(self) -> None:
        bullets = "\n".join(
            f"- {instruction}"
            for instruction in validator.REQUIRED_LIVE_MONITORING_BULLETS
        )
        for reference in (
            "[foo\n---",
            "[foo\nbar",
            "[foo\\]\n---",
            '[foo]: /url "title\n---',
            "[foo\n---\n[",
            '[foo]: /url "title\n---\n" trailing',
        ):
            with self.subTest(reference=reference):
                agents_text = (
                    f"{validator.LIVE_MONITORING_HEADING}\n"
                    f"{reference}\n"
                    f"{bullets}\n"
                )
                failures = self.instruction_parity_failures(
                    "@AGENTS.md\n",
                    agents_text=agents_text,
                )
                self.assertIn(
                    "AGENTS.md: missing required live-monitoring instruction: "
                    f"{validator.REQUIRED_LIVE_MONITORING_BULLETS[0]}",
                    failures,
                )

    def test_empty_atx_heading_ends_the_active_section(self) -> None:
        bullets = "\n".join(
            f"- {instruction}"
            for instruction in validator.REQUIRED_LIVE_MONITORING_BULLETS
        )
        agents_text = (
            f"{validator.LIVE_MONITORING_HEADING}\n\n"
            "obsolete\n"
            "##\n"
            f"{bullets}\n"
        )
        failures = self.instruction_parity_failures(
            "@AGENTS.md\n",
            agents_text=agents_text,
        )
        self.assertIn(
            "AGENTS.md: missing required live-monitoring instruction: "
            f"{validator.REQUIRED_LIVE_MONITORING_BULLETS[0]}",
            failures,
        )

    def test_plain_prose_does_not_replace_the_active_instruction_list(self) -> None:
        instructions = validator.REQUIRED_LIVE_MONITORING_INSTRUCTIONS
        agents_text = (
            f"# Shared instructions\n\n{instructions[0]}\n\n"
            + "\n".join(instructions[1:])
        )
        failures = self.instruction_parity_failures(
            "@AGENTS.md\n",
            agents_text=agents_text,
        )
        self.assertIn(
            "AGENTS.md: missing required live-monitoring instruction: "
            f"{validator.REQUIRED_LIVE_MONITORING_BULLETS[0]}",
            failures,
        )

    def test_nested_markdown_containers_cannot_supply_instruction_items(self) -> None:
        bullets = validator.REQUIRED_LIVE_MONITORING_BULLETS
        for hidden in (
            "- ```text\n"
            + "\n".join(f"  - {instruction}" for instruction in bullets)
            + "\n  ```",
            "- ~~~text\n"
            + "\n".join(f"  - {instruction}" for instruction in bullets)
            + "\n  ~~~",
            "\n".join(f"- > {instruction}" for instruction in bullets),
            "\n".join(f" \t- {instruction}" for instruction in bullets),
        ):
            with self.subTest(hidden=hidden.splitlines()[0]):
                agents_text = (
                    "# Shared instructions\n\n"
                    f"{validator.LIVE_MONITORING_HEADING}\n\n"
                    f"{hidden}\n"
                )
                failures = self.instruction_parity_failures(
                    "@AGENTS.md\n",
                    agents_text=agents_text,
                )
                self.assertIn(
                    "AGENTS.md: missing required live-monitoring instruction: "
                    f"{bullets[0]}",
                    failures,
                )

    def test_literal_comment_tokens_in_code_do_not_hide_active_policy(self) -> None:
        complete = self.valid_agent_instructions()
        for prefix in (
            "```text\n<!--\n```\n\n",
            "~~~text\n<!--\n~~~\n\n",
            "```text <!--\nignored\n```\n\n",
            "    <!--\n\n",
            "- ```text\n  <!--\n  ```\n\n",
            "- ~~~text\n  <!--\n  ~~~\n\n",
            "- ```text <!--\n  ignored\n  ```\n\n",
        ):
            with self.subTest(prefix=prefix.splitlines()[0]):
                failures = self.instruction_parity_failures(
                    "@AGENTS.md\n",
                    agents_text=prefix + complete,
                )
                self.assertEqual(failures, [])

    def test_inline_code_and_escapes_do_not_open_html_comments(self) -> None:
        complete = self.valid_agent_instructions()
        for prefix in (
            "`<!--`\n\n",
            "`` embedded ` <!-- ``\n\n",
            "\\<!-- escaped\n\n",
            "before `<!--` after\n\n",
        ):
            with self.subTest(prefix=prefix.strip()):
                failures = self.instruction_parity_failures(
                    "@AGENTS.md\n",
                    agents_text=prefix + complete,
                )
                self.assertEqual(failures, [])

    def test_list_fence_must_close_inside_its_container(self) -> None:
        bullets = "\n".join(
            f"- {instruction}"
            for instruction in validator.REQUIRED_LIVE_MONITORING_BULLETS
        )
        for close in ("```", " ```"):
            with self.subTest(close=close):
                agents_text = (
                    "# Shared instructions\n\n"
                    f"{validator.LIVE_MONITORING_HEADING}\n\n"
                    "- ```text\n"
                    f"{close}\n"
                    f"{bullets}\n"
                )
                failures = self.instruction_parity_failures(
                    "@AGENTS.md\n",
                    agents_text=agents_text,
                )
                self.assertIn(
                    "AGENTS.md: missing required live-monitoring instruction: "
                    f"{validator.REQUIRED_LIVE_MONITORING_BULLETS[0]}",
                    failures,
                )

    def test_list_continuation_fence_keeps_its_container_boundary(self) -> None:
        bullets = "\n".join(
            f"- {instruction}"
            for instruction in validator.REQUIRED_LIVE_MONITORING_BULLETS
        )
        for prefix in (
            "- item\n  ```text\n```\n",
            "1. item\n   ```text\n```\n",
            "- item\n\n  ```text\n```\n",
            "- - item\n    ```text\n  ```\n```\n",
            "-\n  ```text\n```\n",
            "*\n  ```text\n```\n",
            "+\n  ```text\n```\n",
            "1.\n   ```text\n```\n",
            "- \n  ```text\n```\n",
            "- # heading\n  ```text\n```\n",
            "- # heading\n\n  ```text\n```\n",
            "- <!--x-->\n  ```text\n```\n",
            "- <!--x-->\n\n  ```text\n```\n",
            "- <div>\n\n  ```text\n```\n",
            "-     code\n\n  ```text\n```\n",
        ):
            with self.subTest(prefix=prefix):
                agents_text = (
                    f"{validator.LIVE_MONITORING_HEADING}\n\n"
                    f"{prefix}"
                    f"{bullets}\n"
                )
                failures = self.instruction_parity_failures(
                    "@AGENTS.md\n",
                    agents_text=agents_text,
                )
                self.assertIn(
                    "AGENTS.md: missing required live-monitoring instruction: "
                    f"{validator.REQUIRED_LIVE_MONITORING_BULLETS[0]}",
                    failures,
                )

        for prefix in (
            "-\n\n  ```text\n```\n",
            "- \n\n  ```text\n```\n",
            "1.\n\n   ```text\n```\n",
        ):
            with self.subTest(empty_prefix=prefix):
                agents_text = prefix + self.valid_agent_instructions()
                self.assertEqual(
                    self.instruction_parity_failures(
                        "@AGENTS.md\n",
                        agents_text=agents_text,
                    ),
                    [],
                )

        for thematic_break in (
            "- - -",
            "- --",
            "- ---",
            "* * *",
            "* \t* \t*",
            "  - - -",
        ):
            with self.subTest(thematic_break=repr(thematic_break)):
                active = (
                    f"{validator.LIVE_MONITORING_HEADING}\n"
                    f"{thematic_break}\n"
                    "  ```text\n"
                    "```\n"
                    f"{bullets}\n"
                )
                self.assertEqual(
                    self.instruction_parity_failures(
                        "@AGENTS.md\n",
                        agents_text=active,
                    ),
                    [],
                )
                hidden = active.replace(
                    "```\n" + bullets,
                    "```\n```\n" + bullets,
                    1,
                )
                failures = self.instruction_parity_failures(
                    "@AGENTS.md\n",
                    agents_text=hidden,
                )
                self.assertIn(
                    "AGENTS.md: missing required live-monitoring instruction: "
                    f"{validator.REQUIRED_LIVE_MONITORING_BULLETS[0]}",
                    failures,
                )

    def test_list_fence_ends_at_a_blank_container_boundary(self) -> None:
        for separator in ("", "\n"):
            with self.subTest(separator=repr(separator)):
                agents_text = (
                    "- ```text\n"
                    "  literal\n"
                    + separator
                    + self.valid_agent_instructions()
                )
                self.assertEqual(
                    self.instruction_parity_failures(
                        "@AGENTS.md\n",
                        agents_text=agents_text,
                    ),
                    [],
                )

    def test_list_indented_code_does_not_open_a_fence(self) -> None:
        agents_text = "-     ```text\n\n" + self.valid_agent_instructions()
        self.assertEqual(
            self.instruction_parity_failures(
                "@AGENTS.md\n",
                agents_text=agents_text,
            ),
            [],
        )

    def test_inline_comment_state_ends_at_a_blank_block_boundary(self) -> None:
        for separator in ("", "\n"):
            with self.subTest(separator=repr(separator)):
                agents_text = (
                    "literal <!--\n" + separator + self.valid_agent_instructions()
                )
                self.assertEqual(
                    self.instruction_parity_failures(
                        "@AGENTS.md\n",
                        agents_text=agents_text,
                    ),
                    [],
                )

    def test_retired_heading_suffix_is_not_the_canonical_section(self) -> None:
        agents_text = self.valid_agent_instructions().replace(
            validator.LIVE_MONITORING_HEADING,
            "## Monitor only live repository events (retired)",
            1,
        )
        self.assertEqual(
            self.instruction_parity_failures(
                "@AGENTS.md\n",
                agents_text=agents_text,
            ),
            [
                "AGENTS.md: missing required live-monitoring instruction: "
                f"{validator.LIVE_MONITORING_HEADING}"
            ],
        )

    def test_backtick_in_fence_info_does_not_hide_active_policy(self) -> None:
        agents_text = "``` bad`info\n\n" + self.valid_agent_instructions()
        self.assertEqual(
            self.instruction_parity_failures(
                "@AGENTS.md\n",
                agents_text=agents_text,
            ),
            [],
        )

    def test_invalid_claude_instruction_encoding_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "AGENTS.md").write_text(
                self.valid_agent_instructions(),
                encoding="utf-8",
            )
            (root / "CLAUDE.md").write_bytes(b"\xffinvalid")
            failures: list[str] = []
            validator.validate_instruction_parity(failures, root)
            self.assertEqual(len(failures), 1)
            self.assertIn("could not read instruction import", failures[0])

    def test_invalid_agents_instruction_encoding_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "AGENTS.md").write_bytes(b"\xffinvalid")
            (root / "CLAUDE.md").write_text("@AGENTS.md\n", encoding="utf-8")
            failures: list[str] = []
            validator.validate_instruction_parity(failures, root)
            self.assertEqual(len(failures), 1)
            self.assertIn("could not read canonical instructions", failures[0])

    def claude_settings(self) -> dict:
        return {
            "enabledPlugins": {
                "ievo@ievo-skills": True,
            },
            "extraKnownMarketplaces": {
                "ievo-skills": {
                    "source": {
                        "source": "settings",
                        "name": "ievo-skills",
                        "plugins": [
                            {
                                "name": "ievo",
                                "source": {
                                    "source": "git-subdir",
                                    "url": validator.IEVO_REPOSITORY_URL,
                                    "path": validator.IEVO_PLUGIN_PATH,
                                },
                            }
                        ],
                    },
                    "autoUpdate": False,
                }
            }
        }

    def validate_claude_settings(self, settings: dict) -> list[str]:
        failures: list[str] = []
        validator.validate_claude_ievo_marketplace(
            settings,
            failures,
        )
        return failures

    def optional_boundary_failures(
        self,
        settings: dict,
        root: Path,
        repository_files: list[tuple[Path, str]] | None = None,
    ) -> list[str]:
        if repository_files is None:
            repository_files = [
                (path, "100644")
                for path in root.rglob("*")
                if path.is_file()
            ]
        failures: list[str] = []
        validator.validate_optional_plugin_boundary(
            settings,
            failures,
            root,
            repository_files,
        )
        return failures

    def test_inline_claude_marketplace_without_pin_is_accepted(
        self,
    ) -> None:
        self.assertEqual(self.validate_claude_settings(self.claude_settings()), [])

    def test_inline_claude_marketplace_rejects_every_sibling_entry(self) -> None:
        settings = self.claude_settings()
        plugins = settings["extraKnownMarketplaces"]["ievo-skills"]["source"][
            "plugins"
        ]
        plugins.append(
            {
                "name": "sibling",
                "source": {
                    "source": "git",
                    "url": "https://example.com/sibling.git",
                },
            }
        )

        failures = self.validate_claude_settings(settings)

        self.assertTrue(
            any(
                "must contain only the pinned ievo plugin" in failure
                for failure in failures
            )
        )

    def test_inline_claude_marketplace_rejects_duplicate_ievo_entries(
        self,
    ) -> None:
        settings = self.claude_settings()
        plugins = settings["extraKnownMarketplaces"]["ievo-skills"]["source"][
            "plugins"
        ]
        plugins.append(json.loads(json.dumps(plugins[0])))

        failures = self.validate_claude_settings(settings)

        self.assertTrue(
            any(
                "must contain only the pinned ievo plugin" in failure
                for failure in failures
            )
        )

    def test_inline_claude_marketplace_rejects_a_non_ievo_only_entry(
        self,
    ) -> None:
        invalid_entries = (
            "not-an-ievo-plugin",
            {
                "name": "sibling",
                "source": {
                    "source": "git",
                    "url": "https://example.com/sibling.git",
                },
            },
        )
        for invalid_entry in invalid_entries:
            with self.subTest(entry=invalid_entry):
                settings = self.claude_settings()
                settings["extraKnownMarketplaces"]["ievo-skills"]["source"][
                    "plugins"
                ] = [invalid_entry]

                failures = self.validate_claude_settings(settings)

                self.assertTrue(
                    any(
                        "plugin must be the pinned ievo plugin" in failure
                        for failure in failures
                    )
                )

    def test_claude_marketplace_ref_is_rejected(self) -> None:
        settings = self.claude_settings()
        marketplace = settings["extraKnownMarketplaces"]["ievo-skills"]
        marketplace["source"] = {
            "source": "github",
            "repo": "ievo-ai/skills",
            "ref": "7d5f3e12d0556cb6c5df2974e2babe0433674186",
        }
        failures = self.validate_claude_settings(settings)
        self.assertTrue(
            any("inline settings marketplace" in failure for failure in failures)
        )

    def test_claude_plugin_source_rejects_a_sha_pin(self) -> None:
        settings = self.claude_settings()
        plugin_source = settings["extraKnownMarketplaces"]["ievo-skills"]["source"][
            "plugins"
        ][0]["source"]
        plugin_source["sha"] = "7d5f3e12d0556cb6c5df2974e2babe0433674186"
        failures = self.validate_claude_settings(settings)
        self.assertTrue(
            any("must not pin sha or ref" in failure for failure in failures)
        )

    def test_claude_plugin_source_rejects_a_ref_pin(self) -> None:
        settings = self.claude_settings()
        plugin_source = settings["extraKnownMarketplaces"]["ievo-skills"]["source"][
            "plugins"
        ][0]["source"]
        plugin_source["ref"] = "main"
        failures = self.validate_claude_settings(settings)
        self.assertTrue(
            any("must not pin sha or ref" in failure for failure in failures)
        )

    def test_claude_ievo_plugin_must_be_enabled(self) -> None:
        settings = self.claude_settings()
        settings["enabledPlugins"]["ievo@ievo-skills"] = False
        failures = self.validate_claude_settings(settings)
        self.assertTrue(any("must enable ievo@ievo-skills" in failure for failure in failures))

    def test_required_asset_cannot_reference_an_optional_claude_plugin(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            skill = root / ".claude" / "skills" / "example" / "SKILL.md"
            skill.parent.mkdir(parents=True)
            skill.write_text(
                f"Run `/{FEATURE_DEV}` before implementing the task.\n",
                encoding="utf-8",
            )
            settings = {
                "enabledPlugins": {
                    f"{FEATURE_DEV}@{CLAUDE_PLUGIN_MARKETPLACE}": True,
                    "ievo@ievo-skills": True,
                }
            }

            failures = self.optional_boundary_failures(
                settings,
                root,
            )

            self.assertEqual(len(failures), 1)
            self.assertIn(
                f"{FEATURE_DEV}@{CLAUDE_PLUGIN_MARKETPLACE}",
                failures[0],
            )

    def test_required_workflow_cannot_reference_an_optional_marketplace(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            workflow = root / ".github" / "workflows" / "required.yml"
            workflow.parent.mkdir(parents=True)
            workflow.write_text(
                f"# Requires {CLAUDE_WORKFLOW_MARKETPLACE} at runtime.\n",
                encoding="utf-8",
            )
            settings = {
                "enabledPlugins": {
                    f"{TDD_WORKFLOWS}@{CLAUDE_WORKFLOW_MARKETPLACE}": True,
                }
            }

            failures = self.optional_boundary_failures(
                settings,
                root,
            )

            self.assertEqual(len(failures), 1)
            self.assertIn(
                f"marketplace {CLAUDE_WORKFLOW_MARKETPLACE}",
                failures[0],
            )

    def test_declared_marketplace_is_optional_before_a_plugin_is_enabled(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            agents = root / "AGENTS.md"
            marketplace = "new" + "-market"
            agents.write_text(
                f"Use `helper@{marketplace}` for every task.\n",
                encoding="utf-8",
            )
            settings = {
                "enabledPlugins": {
                    "ievo@ievo-skills": True,
                },
                "extraKnownMarketplaces": {
                    "ievo-skills": {},
                    marketplace: {},
                },
            }

            failures = self.optional_boundary_failures(
                settings,
                root,
            )

            self.assertEqual(len(failures), 1)
            self.assertIn(f"marketplace {marketplace}", failures[0])

    def test_declared_marketplace_repository_source_is_optional(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            agents = root / "AGENTS.md"
            marketplace = "new" + "-market"
            source_repo = "wshobson" + "/agents"
            agents.write_text(
                f"Use workflows from `{source_repo}` for every task.\n",
                encoding="utf-8",
            )
            settings = self.claude_settings()
            settings["extraKnownMarketplaces"][marketplace] = {
                "source": {
                    "source": "github",
                    "repo": source_repo,
                }
            }

            failures = self.optional_boundary_failures(
                settings,
                root,
            )

            self.assertEqual(len(failures), 1)
            self.assertIn(f"marketplace source {source_repo}", failures[0])
            self.assertIn(f"({marketplace})", failures[0])

    def test_github_marketplace_coordinates_are_case_insensitive(self) -> None:
        cases = (
            (
                "github",
                "repo",
                "wshobson" + "/agents",
                "WSHOBSON" + "/AGENTS",
            ),
            (
                "url",
                "url",
                "https://github.com/" + "wshobson" + "/agents.git",
                "HTTPS://GITHUB.COM/" + "WSHOBSON" + "/AGENTS",
            ),
            (
                "url",
                "url",
                "git@github.com:" + "wshobson" + "/agents.git",
                "github.com/" + "WSHOBSON" + "/AGENTS",
            ),
        )
        for source_type, source_key, source_value, required_reference in cases:
            with self.subTest(source=source_value):
                with tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    agents = root / "AGENTS.md"
                    agents.write_text(
                        f"Use `{required_reference}` for every task.\n",
                        encoding="utf-8",
                    )
                    marketplace = "github" + "-case-market"
                    settings = self.claude_settings()
                    settings["extraKnownMarketplaces"][marketplace] = {
                        "source": {
                            "source": source_type,
                            source_key: source_value,
                        }
                    }

                    failures = self.optional_boundary_failures(
                        settings,
                        root,
                    )

                    self.assertEqual(len(failures), 1)
                    self.assertIn("marketplace source", failures[0])
                    self.assertIn(f"({marketplace})", failures[0])

    def test_shared_repository_tokens_preserve_every_marketplace_source(
        self,
    ) -> None:
        github_alias = "github" + "-source"
        hosted_alias = "hosted" + "-source"
        repository = "exampleowner" + "/toolkit"
        declarations = {
            github_alias: {
                "source": {
                    "source": "github",
                    "repo": repository,
                }
            },
            hosted_alias: {
                "source": {
                    "source": "url",
                    "url": "https://example.com/" + repository,
                }
            },
        }
        for aliases in (
            (github_alias, hosted_alias),
            (hosted_alias, github_alias),
        ):
            with self.subTest(order=aliases):
                with tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    agents = root / "AGENTS.md"
                    agents.write_text(
                        "Use `EXAMPLEOWNER/TOOLKIT` for every task.\n",
                        encoding="utf-8",
                    )
                    settings = self.claude_settings()
                    settings["extraKnownMarketplaces"].update(
                        {
                            alias: declarations[alias]
                            for alias in aliases
                        }
                    )

                    failures = self.optional_boundary_failures(
                        settings,
                        root,
                    )

                    self.assertEqual(len(failures), 1)
                    self.assertIn(f"({github_alias})", failures[0])

    def test_declared_marketplace_url_source_coordinates_are_optional(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            agents = root / "AGENTS.md"
            marketplace = "url" + "-market"
            source_repo = "example" + "/agent-tools"
            source_url = "https://github.com/" + source_repo + ".git"
            normalized_source = "github.com/" + source_repo
            agents.write_text(
                f"Use workflows from `{normalized_source}` for every task.\n",
                encoding="utf-8",
            )
            settings = self.claude_settings()
            settings["extraKnownMarketplaces"][marketplace] = {
                "source": {
                    "source": "url",
                    "url": source_url,
                }
            }

            failures = self.optional_boundary_failures(
                settings,
                root,
            )

            self.assertEqual(len(failures), 1)
            self.assertIn(f"marketplace source {normalized_source}", failures[0])
            self.assertIn(f"({marketplace})", failures[0])

    def test_marketplace_source_coordinates_are_canonicalized(self) -> None:
        cases = (
            (
                "repo",
                "example/agent-tools" + ".git/",
                "example/agent-tools",
            ),
            (
                "url",
                "https://github.com/example/agent%2Dtools.git",
                "example/agent-tools",
            ),
            (
                "url",
                "git@github.com:example/agent-tools.git",
                "example/agent-tools",
            ),
        )
        for source_key, source_value, required_reference in cases:
            with self.subTest(source=source_value):
                with tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    agents = root / "AGENTS.md"
                    marketplace = "normalized" + "-market"
                    agents.write_text(
                        f"Use `{required_reference}` for every task.\n",
                        encoding="utf-8",
                    )
                    settings = self.claude_settings()
                    settings["extraKnownMarketplaces"][marketplace] = {
                        "source": {
                            "source": source_key,
                            source_key: source_value,
                        }
                    }

                    failures = self.optional_boundary_failures(
                        settings,
                        root,
                    )

                    self.assertEqual(len(failures), 1)
                    self.assertIn(
                        f"marketplace source {required_reference}",
                        failures[0],
                    )

    def test_single_segment_url_path_does_not_create_a_generic_token(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            agents = root / "AGENTS.md"
            agents.write_text(
                "All agents must preserve user changes.\n",
                encoding="utf-8",
            )
            marketplace = "single" + "-segment"
            settings = self.claude_settings()
            settings["extraKnownMarketplaces"][marketplace] = {
                "source": {
                    "source": "url",
                    "url": "https://example.com/agents",
                }
            }

            failures = self.optional_boundary_failures(
                settings,
                root,
            )

            self.assertEqual(failures, [])

    def test_url_scheme_and_host_are_normalized_but_path_stays_case_sensitive(
        self,
    ) -> None:
        source_url = "https://example.com/agents"
        cases = (
            ("HTTPS://EXAMPLE.COM/agents", True),
            ("https://example.com/Agents", False),
        )
        for required_reference, rejected in cases:
            with self.subTest(reference=required_reference):
                with tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    agents = root / "AGENTS.md"
                    agents.write_text(
                        f"Use `{required_reference}` for every task.\n",
                        encoding="utf-8",
                    )
                    marketplace = "case" + "-market"
                    settings = self.claude_settings()
                    settings["extraKnownMarketplaces"][marketplace] = {
                        "source": {
                            "source": "url",
                            "url": source_url,
                        }
                    }

                    failures = self.optional_boundary_failures(
                        settings,
                        root,
                    )

                    if rejected:
                        self.assertEqual(len(failures), 1)
                        self.assertIn("marketplace source", failures[0])
                        self.assertIn("example.com/agents", failures[0])
                    else:
                        self.assertEqual(failures, [])

    def test_default_url_ports_match_their_implicit_forms(self) -> None:
        cases = (
            (
                "https://example.com:443/agents",
                "https://example.com/agents",
                True,
            ),
            (
                "https://example.com/agents",
                "HTTPS://EXAMPLE.COM:443/agents",
                True,
            ),
            (
                "https://example.com:443/agents",
                "example.com:443/agents",
                True,
            ),
            (
                "https://example.com/agents",
                "example.com:443/agents",
                True,
            ),
            (
                "ssh://example.com:22/agents",
                "example.com:22/agents",
                True,
            ),
            (
                "ssh://example.com/agents",
                "example.com:22/agents",
                True,
            ),
            (
                "https://example.com:8443/agents",
                "https://example.com/agents",
                False,
            ),
        )
        for source_url, required_reference, rejected in cases:
            with self.subTest(
                source=source_url,
                reference=required_reference,
            ):
                with tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    agents = root / "AGENTS.md"
                    agents.write_text(
                        f"Use `{required_reference}` for every task.\n",
                        encoding="utf-8",
                    )
                    marketplace = "port" + "-market"
                    settings = self.claude_settings()
                    settings["extraKnownMarketplaces"][marketplace] = {
                        "source": {
                            "source": "url",
                            "url": source_url,
                        }
                    }

                    failures = self.optional_boundary_failures(
                        settings,
                        root,
                    )

                    if rejected:
                        self.assertEqual(len(failures), 1)
                        self.assertIn("marketplace source", failures[0])
                        self.assertIn("example.com/agents", failures[0])
                    else:
                        self.assertEqual(failures, [])

    def test_ipv6_host_path_forms_match_url_source_identity(self) -> None:
        cases = (
            ("[2001:DB8::1]/agents", True),
            ("[2001:DB8::1]:443/agents", True),
            ("[2001:db8::1]/agents", True),
            ("HTTPS://[2001:DB8::1]:443/agents", True),
            ("[2001:DB8::1]/Agents", False),
        )
        for required_reference, rejected in cases:
            with self.subTest(reference=required_reference):
                with tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    agents = root / "AGENTS.md"
                    agents.write_text(
                        f"Use `{required_reference}` for every task.\n",
                        encoding="utf-8",
                    )
                    marketplace = "ipv6" + "-market"
                    settings = self.claude_settings()
                    settings["extraKnownMarketplaces"][marketplace] = {
                        "source": {
                            "source": "url",
                            "url": "https://[2001:db8::1]/agents",
                        }
                    }

                    failures = self.optional_boundary_failures(
                        settings,
                        root,
                    )

                    if rejected:
                        self.assertEqual(len(failures), 1)
                        self.assertIn(
                            "marketplace source",
                            failures[0],
                        )
                        self.assertIn("[2001:db8::1]/agents", failures[0])
                    else:
                        self.assertEqual(failures, [])

    def test_url_identity_normalization_handles_authority_edges(self) -> None:
        cases = (
            (
                "HTTPS://EXAMPLE.COM:8443/Path",
                "https://example.com:8443/Path",
            ),
            (
                "HTTPS://EXAMPLE.COM:443/Path",
                "https://example.com/Path",
            ),
            (
                "HTTP://EXAMPLE.COM:80/Path",
                "http://example.com/Path",
            ),
            (
                "ssh://EXAMPLE.COM:22/Path",
                "ssh://example.com/Path",
            ),
            (
                "git://EXAMPLE.COM:9418/Path",
                "git://example.com/Path",
            ),
            (
                "HTTPS://[2001:DB8::1]:8443/Path",
                "https://[2001:db8::1]:8443/Path",
            ),
            (
                "HTTPS://user@EXAMPLE.COM/Path",
                "https://example.com/Path",
            ),
            (
                "https://[invalid/Path",
                "https://[invalid/Path",
            ),
            (
                "https://EXAMPLE.COM:notaport/Path",
                "https://EXAMPLE.COM:notaport/Path",
            ),
            (
                "https://:443/Path",
                "https://:443/Path",
            ),
            (
                "All agents preserve path Case.",
                "All agents preserve path Case.",
            ),
        )
        for source, expected in cases:
            with self.subTest(source=source):
                self.assertEqual(
                    validator.normalize_url_identity_components(source),
                    expected,
                )

    def test_scp_and_host_path_hosts_are_normalized_but_paths_are_not(
        self,
    ) -> None:
        source_url = "git@EXAMPLE.COM:agents.git"
        cases = (
            ("git@example.com:agents.git", True),
            ("EXAMPLE.COM/agents", True),
            ("git@example.com:Agents.git", False),
        )
        for required_reference, rejected in cases:
            with self.subTest(reference=required_reference):
                with tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    agents = root / "AGENTS.md"
                    agents.write_text(
                        f"Use `{required_reference}` for every task.\n",
                        encoding="utf-8",
                    )
                    marketplace = "scp-case" + "-market"
                    settings = self.claude_settings()
                    settings["extraKnownMarketplaces"][marketplace] = {
                        "source": {
                            "source": "url",
                            "url": source_url,
                        }
                    }

                    failures = self.optional_boundary_failures(
                        settings,
                        root,
                    )

                    if rejected:
                        self.assertEqual(len(failures), 1)
                        self.assertIn(
                            "marketplace source example.com/agents",
                            failures[0],
                        )
                    else:
                        self.assertEqual(failures, [])

    def test_bracketed_ipv6_scp_marketplace_sources_are_normalized(
        self,
    ) -> None:
        source_url = "git@[2001:DB8::1]:org/agents.git"
        cases = (
            ("git@[2001:db8::1]:org/agents.git", True),
            ("[2001:db8::1]/org/agents", True),
            ("git@[2001:db8::1]:org/Agents.git", False),
        )
        for required_reference, rejected in cases:
            with self.subTest(reference=required_reference):
                with tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    agents = root / "AGENTS.md"
                    agents.write_text(
                        f"Use `{required_reference}` for every task.\n",
                        encoding="utf-8",
                    )
                    marketplace = "ipv6-scp" + "-market"
                    settings = self.claude_settings()
                    settings["extraKnownMarketplaces"][marketplace] = {
                        "source": {
                            "source": "url",
                            "url": source_url,
                        }
                    }

                    failures = self.optional_boundary_failures(
                        settings,
                        root,
                    )

                    if rejected:
                        self.assertEqual(len(failures), 1)
                        self.assertIn(
                            "marketplace source [2001:db8::1]/org/agents",
                            failures[0],
                        )
                    else:
                        self.assertEqual(failures, [])

    def test_invalid_bracketed_ipv6_scp_host_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            agents = root / "AGENTS.md"
            agents.write_text(
                "Use repository-owned tools.\n",
                encoding="utf-8",
            )
            marketplace = "invalid-ipv6-scp" + "-market"
            settings = self.claude_settings()
            settings["extraKnownMarketplaces"][marketplace] = {
                "source": {
                    "source": "url",
                    "url": "git@[2001:::1]:org/agents.git",
                }
            }

            failures = self.optional_boundary_failures(settings, root)

            self.assertEqual(len(failures), 1)
            self.assertIn("bracketed IPv6 SCP host", failures[0])
            self.assertNotIn("2001", failures[0])

    def test_percent_encoded_paths_compose_with_host_normalization(self) -> None:
        cases = (
            (
                "HTTPS://EXAMPLE.COM/agent%2Dtools.git",
                "https://example.com/agent%2Dtools.git",
            ),
            (
                "git@EXAMPLE.COM:agent%2Dtools.git",
                "git@example.com:agent%2Dtools.git",
            ),
        )
        for source_url, required_reference in cases:
            with self.subTest(source=source_url):
                with tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    agents = root / "AGENTS.md"
                    agents.write_text(
                        f"Use `{required_reference}` for every task.\n",
                        encoding="utf-8",
                    )
                    marketplace = "encoded-case" + "-market"
                    settings = self.claude_settings()
                    settings["extraKnownMarketplaces"][marketplace] = {
                        "source": {
                            "source": "url",
                            "url": source_url,
                        }
                    }

                    failures = self.optional_boundary_failures(
                        settings,
                        root,
                    )

                    self.assertEqual(len(failures), 1)
                    self.assertIn(
                        "marketplace source",
                        failures[0],
                    )
                    self.assertIn(f"({marketplace})", failures[0])

    def test_single_label_scp_marketplace_sources_are_rejected(self) -> None:
        for host in ("github", "GITHUB"):
            with self.subTest(host=host):
                with tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    agents = root / "AGENTS.md"
                    agents.write_text(
                        "Use repository-owned tools.\n",
                        encoding="utf-8",
                    )
                    marketplace = "single-label" + "-market"
                    settings = self.claude_settings()
                    settings["extraKnownMarketplaces"][marketplace] = {
                        "source": {
                            "source": "url",
                            "url": f"git@{host}:agents.git",
                        }
                    }

                    failures = self.optional_boundary_failures(
                        settings,
                        root,
                    )

                    self.assertEqual(len(failures), 1)
                    self.assertIn(marketplace, failures[0])
                    self.assertIn(
                        "fully qualified dotted or bracketed IPv6 SCP host",
                        failures[0],
                    )
                    self.assertNotIn(host, failures[0])

    def test_malformed_marketplace_url_reports_a_controlled_failure(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            agents = root / "AGENTS.md"
            agents.write_text("Use repository-owned tools.\n", encoding="utf-8")
            marketplace = "malformed" + "-market"
            settings = self.claude_settings()
            settings["extraKnownMarketplaces"][marketplace] = {
                "source": {
                    "source": "url",
                    "url": "https://[invalid/repo.git",
                }
            }

            failures = self.optional_boundary_failures(
                settings,
                root,
            )

            self.assertEqual(len(failures), 1)
            self.assertIn(marketplace, failures[0])
            self.assertIn("source.url", failures[0])
            self.assertNotIn("[invalid", failures[0])

    def test_pinned_marketplace_source_is_not_treated_as_optional(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            agents = root / "AGENTS.md"
            source_repo = "example" + "/pinned-tools"
            agents.write_text(
                f"Use the reviewed `{source_repo}` baseline.\n",
                encoding="utf-8",
            )
            settings = {
                "enabledPlugins": {
                    "ievo@ievo-skills": True,
                },
                "extraKnownMarketplaces": {
                    "ievo-skills": {
                        "source": {
                            "source": "github",
                            "repo": source_repo,
                        }
                    }
                },
            }

            failures = self.optional_boundary_failures(
                settings,
                root,
            )

            self.assertEqual(failures, [])

    def test_unvalidated_sibling_in_pinned_marketplace_is_optional(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            agents = root / "AGENTS.md"
            agents.write_text(
                f"Use `{MUTABLE_HELPER}@ievo-skills` for every change.\n",
                encoding="utf-8",
            )
            settings = {
                "enabledPlugins": {
                    "ievo@ievo-skills": True,
                    f"{MUTABLE_HELPER}@ievo-skills": True,
                }
            }

            failures = self.optional_boundary_failures(
                settings,
                root,
            )

            self.assertEqual(len(failures), 1)
            self.assertIn(f"{MUTABLE_HELPER}@ievo-skills", failures[0])

    def test_unconfigured_sibling_in_pinned_marketplace_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            agents = root / "AGENTS.md"
            agents.write_text(
                f"Use `{MUTABLE_HELPER}@ievo-skills` for every change.\n",
                encoding="utf-8",
            )

            failures = self.optional_boundary_failures(
                {
                    "enabledPlugins": {
                        "ievo@ievo-skills": True,
                    }
                },
                root,
            )

            self.assertEqual(len(failures), 1)
            self.assertIn(f"{MUTABLE_HELPER}@ievo-skills", failures[0])

    def test_scoped_instruction_files_reject_optional_plugin_references(self) -> None:
        for filename in ("AGENTS.md", "CLAUDE.md"):
            with self.subTest(filename=filename):
                with tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    instructions = root / "nested" / "subdir" / filename
                    instructions.parent.mkdir(parents=True)
                    instructions.write_text(
                        f"Use `/{FEATURE_DEV}` for every task.\n",
                        encoding="utf-8",
                    )
                    settings = {
                        "enabledPlugins": {
                            f"{FEATURE_DEV}@{CLAUDE_PLUGIN_MARKETPLACE}": True,
                            "ievo@ievo-skills": True,
                        }
                    }

                    failures = self.optional_boundary_failures(
                        settings,
                        root,
                    )

                    self.assertEqual(len(failures), 1)
                    self.assertIn(f"nested/subdir/{filename}", failures[0])

    def test_all_tracked_agent_asset_formats_are_scanned(self) -> None:
        settings = {
            "enabledPlugins": {
                f"{FEATURE_DEV}@{CLAUDE_PLUGIN_MARKETPLACE}": True,
                "ievo@ievo-skills": True,
            }
        }
        fixtures = (
            (".claude/skills/demo/scripts/run.sh", "100644"),
            (".claude/skills/demo/agents/openai.yaml", "100644"),
            (".ievo/evolution/project.md", "100644"),
            (".github/actions/demo/action.yml", "100644"),
            ("ci/check/action.yml", "100644"),
            ("ci/check/action.yaml", "100644"),
            ("tests/agent_smoke.py", "100644"),
            ("scripts/check_ci_permissions.rb", "100644"),
            ("crates/demo/src/lib.rs", "100644"),
            ("tools/agent-adapter", "100755"),
        )

        for relative, mode in fixtures:
            with self.subTest(relative=relative):
                with tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    asset = root / relative
                    asset.parent.mkdir(parents=True)
                    asset.write_text(
                        f"Run /{FEATURE_DEV} before continuing.\n",
                        encoding="utf-8",
                    )

                    failures = self.optional_boundary_failures(
                        settings,
                        root,
                        [(asset, mode)],
                    )

                    self.assertEqual(len(failures), 1)
                    self.assertIn(relative, failures[0])

    def test_extensionless_scripts_invoked_by_required_assets_are_scanned(
        self,
    ) -> None:
        consumers = (
            (
                ".github/workflows/check.yml",
                "jobs:\n  check:\n    steps:\n"
                "      - run: python -X dev tools/helper\n",
            ),
            (
                ".claude/settings.json",
                json.dumps(
                    {
                        "hooks": {
                            "PreToolUse": [
                                {
                                    "hooks": [
                                        {
                                            "type": "command",
                                            "command": (
                                                "python3 -u "
                                                '"./tools/helper"'
                                            ),
                                        }
                                    ]
                                }
                            ]
                        }
                    }
                ),
            ),
        )
        settings = {
            "enabledPlugins": {
                f"{FEATURE_DEV}@{CLAUDE_PLUGIN_MARKETPLACE}": True,
                "ievo@ievo-skills": True,
            }
        }
        for consumer_relative, consumer_text in consumers:
            with self.subTest(consumer=consumer_relative):
                with tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    consumer = root / consumer_relative
                    consumer.parent.mkdir(parents=True)
                    consumer.write_text(consumer_text, encoding="utf-8")
                    helper = root / "tools" / "helper"
                    helper.parent.mkdir()
                    helper.write_text(
                        f"Run /{FEATURE_DEV} before continuing.\n",
                        encoding="utf-8",
                    )

                    failures = self.optional_boundary_failures(
                        settings,
                        root,
                        [
                            (consumer, "100644"),
                            (helper, "100644"),
                        ],
                    )

                    self.assertEqual(len(failures), 1)
                    self.assertIn("tools/helper", failures[0])

    def test_workflow_working_directories_resolve_extensionless_scripts(
        self,
    ) -> None:
        cases = (
            (
                "defaults:\n"
                "  run:\n"
                "    working-directory: tools\n"
                "jobs:\n"
                "  check:\n"
                "    steps:\n"
                "      - run: python helper\n",
                "tools/helper",
            ),
            (
                "jobs:\n"
                "  check:\n"
                "    defaults:\n"
                "      run:\n"
                "        working-directory: tools\n"
                "    steps:\n"
                "      - run: python helper\n",
                "tools/helper",
            ),
            (
                "jobs:\n"
                "  check:\n"
                "    steps:\n"
                "      - working-directory: tools\n"
                "        run: >2\n"
                "          python helper\n",
                "tools/helper",
            ),
            (
                "jobs:\n"
                "  check:\n"
                "    steps:\n"
                "      - working-directory: tools\n"
                "        run: >2-\n"
                "          python\n"
                "          helper\n",
                "tools/helper",
            ),
            (
                "jobs:\n"
                "  check:\n"
                "    steps:\n"
                "      - working-directory: tools\n"
                "        run: |2\n"
                "          python helper\n",
                "tools/helper",
            ),
            (
                "jobs:\n"
                "  check:\n"
                "    defaults:\n"
                "      run:\n"
                "        working-directory: ignored\n"
                "    steps:\n"
                "      - working-directory: tools\n"
                "        run: >+\n"
                "          python\n"
                "          helper\n",
                "tools/helper",
            ),
            (
                "jobs:\n"
                "  'quoted-check':\n"
                "    steps:\n"
                "      - working-directory: tools\n"
                "        run: python helper\n",
                "tools/helper",
            ),
            (
                "jobs:\n"
                "  check:\n"
                "    steps:\n"
                "      -\n"
                "        working-directory: tools\n"
                "        run: python helper\n",
                "tools/helper",
            ),
        )
        settings = {
            "enabledPlugins": {
                f"{FEATURE_DEV}@{CLAUDE_PLUGIN_MARKETPLACE}": True,
                "ievo@ievo-skills": True,
            }
        }
        for workflow_text, violating_path in cases:
            with self.subTest(path=violating_path, workflow=workflow_text):
                with tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    workflow = root / ".github" / "workflows" / "check.yml"
                    workflow.parent.mkdir(parents=True)
                    workflow.write_text(workflow_text, encoding="utf-8")
                    helper = root / violating_path
                    helper.parent.mkdir(parents=True)
                    helper.write_text(
                        f"Run /{FEATURE_DEV} before continuing.\n",
                        encoding="utf-8",
                    )

                    failures = self.optional_boundary_failures(
                        settings,
                        root,
                        [
                            (workflow, "100644"),
                            (helper, "100644"),
                        ],
                    )

                    self.assertEqual(len(failures), 1)
                    self.assertIn(violating_path, failures[0])

    def test_composite_action_working_directory_resolves_extensionless_script(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            action = root / "ci" / "check" / "action.yml"
            action.parent.mkdir(parents=True)
            action.write_text(
                "name: check\n"
                "runs:\n"
                "  using: composite\n"
                "  steps:\n"
                "    - shell: bash\n"
                "      working-directory: tools/demo\n"
                "      run: python helper\n",
                encoding="utf-8",
            )
            root_helper = root / "helper"
            root_helper.write_text(
                "Use only repository-owned tooling.\n",
                encoding="utf-8",
            )
            effective_helper = root / "tools" / "demo" / "helper"
            effective_helper.parent.mkdir(parents=True)
            effective_helper.write_text(
                f"Run /{FEATURE_DEV} before continuing.\n",
                encoding="utf-8",
            )

            failures = self.optional_boundary_failures(
                {
                    "enabledPlugins": {
                        f"{FEATURE_DEV}@{CLAUDE_PLUGIN_MARKETPLACE}": True,
                        "ievo@ievo-skills": True,
                    }
                },
                root,
                [
                    (action, "100644"),
                    (root_helper, "100644"),
                    (effective_helper, "100644"),
                ],
            )

            self.assertEqual(len(failures), 1)
            self.assertIn("tools/demo/helper", failures[0])

    def test_non_composite_action_entrypoints_are_scanned_relative_to_manifest(
        self,
    ) -> None:
        cases = (
            (
                "  using: node20\n"
                "  main: main\n",
                "ci/check/main",
                "main",
            ),
            (
                "  using: node20\n"
                "  main: dist/index.js\n"
                "  pre: setup\n",
                "ci/check/setup",
                "setup",
            ),
            (
                "  using: node20\n"
                "  main: dist/index.js\n"
                "  post: cleanup\n",
                "ci/check/cleanup",
                "cleanup",
            ),
            (
                "  using: docker\n"
                "  image: Containerfile\n",
                "ci/check/Containerfile",
                "Containerfile",
            ),
            (
                "  using: node20\n"
                "  main: ../main\n",
                "ci/main",
                "main",
            ),
        )
        for runs_body, entrypoint_path, shadow_name in cases:
            with self.subTest(entrypoint=entrypoint_path):
                with tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    action = root / "ci" / "check" / "action.yml"
                    action.parent.mkdir(parents=True)
                    action.write_text(
                        "name: check\n"
                        "runs:\n"
                        f"{runs_body}",
                        encoding="utf-8",
                    )
                    root_shadow = root / shadow_name
                    root_shadow.write_text(
                        "Use only repository-owned tooling.\n",
                        encoding="utf-8",
                    )
                    entrypoint = root / entrypoint_path
                    entrypoint.parent.mkdir(parents=True, exist_ok=True)
                    entrypoint.write_text(
                        f"Run /{FEATURE_DEV} before continuing.\n",
                        encoding="utf-8",
                    )

                    failures = self.optional_boundary_failures(
                        {
                            "enabledPlugins": {
                                (
                                    f"{FEATURE_DEV}@"
                                    f"{CLAUDE_PLUGIN_MARKETPLACE}"
                                ): True,
                                "ievo@ievo-skills": True,
                            }
                        },
                        root,
                        [
                            (action, "100644"),
                            (root_shadow, "100644"),
                            (entrypoint, "100644"),
                        ],
                    )

                    self.assertEqual(len(failures), 1)
                    self.assertIn(entrypoint_path, failures[0])

    def test_external_docker_action_image_is_not_a_repository_path(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            action = root / "ci" / "check" / "action.yaml"
            action.parent.mkdir(parents=True)
            action.write_text(
                "name: check\n"
                "runs:\n"
                "  using: docker\n"
                "  image: docker://example.com/optional-image:latest\n",
                encoding="utf-8",
            )

            failures = self.optional_boundary_failures(
                self.claude_settings(),
                root,
                [(action, "100644")],
            )

            self.assertEqual(failures, [])

    def test_action_input_names_do_not_become_execution_fields(self) -> None:
        for input_name in ("main", "pre", "post", "image"):
            with self.subTest(input_name=input_name):
                with tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    action = root / "ci" / "check" / "action.yml"
                    action.parent.mkdir(parents=True)
                    action.write_text(
                        "name: check\n"
                        "inputs:\n"
                        f"  {input_name}: "
                        '{description: "input", required: false}\n'
                        "runs:\n"
                        "  using: node20\n"
                        "  main: dist/index.js\n",
                        encoding="utf-8",
                    )

                    failures = self.optional_boundary_failures(
                        self.claude_settings(),
                        root,
                        [(action, "100644")],
                    )

                    self.assertEqual(failures, [])

    def test_action_execution_fields_reject_flow_or_alias_values(
        self,
    ) -> None:
        cases = (
            ("main", "{path: main}", "flow-style"),
            ("pre", "{path: setup}", "flow-style"),
            ("post", "{path: cleanup}", "flow-style"),
            ("image", "*container", "aliased"),
        )
        for field, value, message in cases:
            with self.subTest(field=field):
                with tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    action = root / "ci" / "check" / "action.yml"
                    action.parent.mkdir(parents=True)
                    action.write_text(
                        "name: check\n"
                        "container: &container Dockerfile\n"
                        "runs:\n"
                        "  using: node20\n"
                        f"  {field}: {value}\n",
                        encoding="utf-8",
                    )

                    failures = self.optional_boundary_failures(
                        self.claude_settings(),
                        root,
                        [(action, "100644")],
                    )

                    self.assertEqual(len(failures), 1)
                    self.assertIn(message, failures[0])

    def test_non_composite_entrypoint_inherits_repository_runtime_cwd(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            action = root / "ci" / "check" / "action.yml"
            action.parent.mkdir(parents=True)
            action.write_text(
                "name: check\n"
                "runs:\n"
                "  using: node20\n"
                "  main: main\n",
                encoding="utf-8",
            )
            entrypoint = root / "ci" / "check" / "main"
            entrypoint.write_text("python nested\n", encoding="utf-8")
            action_relative_nested = root / "ci" / "check" / "nested"
            action_relative_nested.write_text(
                "Use only repository-owned tooling.\n",
                encoding="utf-8",
            )
            repository_nested = root / "nested"
            repository_nested.write_text(
                f"Run /{FEATURE_DEV} before continuing.\n",
                encoding="utf-8",
            )

            failures = self.optional_boundary_failures(
                {
                    "enabledPlugins": {
                        f"{FEATURE_DEV}@{CLAUDE_PLUGIN_MARKETPLACE}": True,
                        "ievo@ievo-skills": True,
                    }
                },
                root,
                [
                    (action, "100644"),
                    (entrypoint, "100644"),
                    (action_relative_nested, "100644"),
                    (repository_nested, "100644"),
                ],
            )

            self.assertEqual(len(failures), 1)
            self.assertIn("nested", failures[0])
            self.assertNotIn("ci/check/nested", failures[0])

    def test_dynamic_composite_action_working_directory_fails_closed(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            action = root / "ci" / "check" / "action.yaml"
            action.parent.mkdir(parents=True)
            action.write_text(
                "name: check\n"
                "runs:\n"
                "  using: composite\n"
                "  steps:\n"
                "    - shell: bash\n"
                "      working-directory: ${{ inputs.directory }}\n"
                "      run: python helper\n",
                encoding="utf-8",
            )

            failures = self.optional_boundary_failures(
                self.claude_settings(),
                root,
                [(action, "100644")],
            )

            self.assertEqual(len(failures), 1)
            self.assertIn("action line 6", failures[0])
            self.assertIn("dynamic or non-repository", failures[0])

    def test_parent_script_path_inside_repository_is_resolved(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            parent_helper = ".." + "/helper"
            workflow = root / ".github" / "workflows" / "check.yml"
            workflow.parent.mkdir(parents=True)
            workflow.write_text(
                "jobs:\n"
                "  check:\n"
                "    steps:\n"
                "      - working-directory: tools/sub\n"
                f"        run: python {parent_helper}\n",
                encoding="utf-8",
            )
            helper = root / "tools" / "helper"
            helper.parent.mkdir(parents=True)
            helper.write_text(
                f"Run /{FEATURE_DEV} before continuing.\n",
                encoding="utf-8",
            )

            failures = self.optional_boundary_failures(
                {
                    "enabledPlugins": {
                        f"{FEATURE_DEV}@{CLAUDE_PLUGIN_MARKETPLACE}": True,
                        "ievo@ievo-skills": True,
                    }
                },
                root,
                [
                    (workflow, "100644"),
                    (helper, "100644"),
                ],
            )

            self.assertEqual(len(failures), 1)
            self.assertIn("tools/helper", failures[0])

    def test_parent_script_path_escaping_repository_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            escaping_path = ".." + "/../outside"
            workflow = root / ".github" / "workflows" / "check.yml"
            workflow.parent.mkdir(parents=True)
            workflow.write_text(
                "jobs:\n"
                "  check:\n"
                "    steps:\n"
                "      - working-directory: tools\n"
                f"        run: python {escaping_path}\n",
                encoding="utf-8",
            )

            failures = self.optional_boundary_failures(
                self.claude_settings(),
                root,
                [(workflow, "100644")],
            )

            self.assertEqual(len(failures), 1)
            self.assertIn("escapes the repository", failures[0])

    def test_workflow_working_directory_is_scoped_to_its_step(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            workflow = root / ".github" / "workflows" / "check.yml"
            workflow.parent.mkdir(parents=True)
            workflow.write_text(
                "jobs:\n"
                "  check:\n"
                "    steps:\n"
                "      - run: python helper\n"
                "      - working-directory: tools\n"
                "        run: echo done\n"
                "  sibling:\n"
                "    steps:\n"
                "      - working-directory: elsewhere\n"
                "        run: echo done\n",
                encoding="utf-8",
            )
            helper = root / "tools" / "helper"
            helper.parent.mkdir()
            helper.write_text(
                f"Run /{FEATURE_DEV} before continuing.\n",
                encoding="utf-8",
            )

            failures = self.optional_boundary_failures(
                {
                    "enabledPlugins": {
                        f"{FEATURE_DEV}@{CLAUDE_PLUGIN_MARKETPLACE}": True,
                        "ievo@ievo-skills": True,
                    }
                },
                root,
                [
                    (workflow, "100644"),
                    (helper, "100644"),
                ],
            )

            self.assertEqual(failures, [])

    def test_workflow_working_directory_does_not_select_shadowed_root_script(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            workflow = root / ".github" / "workflows" / "check.yml"
            workflow.parent.mkdir(parents=True)
            workflow.write_text(
                "jobs:\n"
                "  check:\n"
                "    steps:\n"
                "      - working-directory: tools\n"
                "        run: python helper\n",
                encoding="utf-8",
            )
            root_helper = root / "helper"
            root_helper.write_text(
                f"Run /{FEATURE_DEV} before continuing.\n",
                encoding="utf-8",
            )
            effective_helper = root / "tools" / "helper"
            effective_helper.parent.mkdir()
            effective_helper.write_text(
                "Use only repository-owned tooling.\n",
                encoding="utf-8",
            )

            failures = self.optional_boundary_failures(
                {
                    "enabledPlugins": {
                        f"{FEATURE_DEV}@{CLAUDE_PLUGIN_MARKETPLACE}": True,
                        "ievo@ievo-skills": True,
                    }
                },
                root,
                [
                    (workflow, "100644"),
                    (root_helper, "100644"),
                    (effective_helper, "100644"),
                ],
            )

            self.assertEqual(failures, [])

    def test_dynamic_working_directory_with_relative_script_fails_closed(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            workflow = root / ".github" / "workflows" / "check.yml"
            workflow.parent.mkdir(parents=True)
            workflow.write_text(
                "jobs:\n"
                "  check:\n"
                "    strategy:\n"
                "      matrix:\n"
                "        directory: [tools]\n"
                "    steps:\n"
                "      - working-directory: ${{ matrix.directory }}\n"
                "        run: python helper\n",
                encoding="utf-8",
            )
            helper = root / "tools" / "helper"
            helper.parent.mkdir()
            helper.write_text(
                f"Run /{FEATURE_DEV} before continuing.\n",
                encoding="utf-8",
            )

            failures = self.optional_boundary_failures(
                {
                    "enabledPlugins": {
                        f"{FEATURE_DEV}@{CLAUDE_PLUGIN_MARKETPLACE}": True,
                        "ievo@ievo-skills": True,
                    }
                },
                root,
                [
                    (workflow, "100644"),
                    (helper, "100644"),
                ],
            )

            self.assertEqual(len(failures), 1)
            self.assertIn("dynamic or non-repository", failures[0])

    def test_flow_style_workflow_structure_fails_closed(self) -> None:
        workflows = (
            (
                "jobs:\n"
                "  check:\n"
                "    steps: "
                "[{run: python helper, working-directory: tools}]\n"
            ),
            (
                "jobs:\n"
                "  check:\n"
                "    steps:\n"
                "      - {run: python helper, working-directory: tools}\n"
            ),
            (
                "jobs: "
                "{check: {steps: [{run: python helper, "
                "working-directory: tools}]}}\n"
            ),
            (
                '{"jobs": {"check": {"steps": '
                '[{"run": "python helper", '
                '"working-directory": "tools"}]}}}\n'
            ),
            (
                '--- {"jobs": {"check": {"steps": '
                '[{"run": "python helper", '
                '"working-directory": "tools"}]}}}\n'
            ),
        )
        for workflow_text in workflows:
            with self.subTest(workflow=workflow_text):
                with tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    workflow = root / ".github" / "workflows" / "check.yml"
                    workflow.parent.mkdir(parents=True)
                    workflow.write_text(workflow_text, encoding="utf-8")

                    failures = self.optional_boundary_failures(
                        self.claude_settings(),
                        root,
                        [(workflow, "100644")],
                    )

                    self.assertEqual(len(failures), 1)
                    self.assertIn("flow-style", failures[0])

    def test_workflow_aliases_and_merge_keys_fail_closed(self) -> None:
        cases = (
            (
                "shared: &shared\n"
                "  run:\n"
                "    working-directory: tools\n"
                "defaults: *shared\n"
                "jobs:\n"
                "  check:\n"
                "    steps:\n"
                "      - run: python helper\n",
                "aliased",
            ),
            (
                "shared: &shared\n"
                "  defaults:\n"
                "    run:\n"
                "      working-directory: tools\n"
                "jobs:\n"
                "  check:\n"
                "    <<: *shared\n"
                "    steps:\n"
                "      - run: python helper\n",
                "merge keys",
            ),
        )
        for workflow_text, message in cases:
            with self.subTest(message=message):
                with tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    workflow = root / ".github" / "workflows" / "check.yml"
                    workflow.parent.mkdir(parents=True)
                    workflow.write_text(workflow_text, encoding="utf-8")

                    failures = self.optional_boundary_failures(
                        self.claude_settings(),
                        root,
                        [(workflow, "100644")],
                    )

                    self.assertEqual(len(failures), 1)
                    self.assertIn(message, failures[0])

    def test_recursive_helper_discovery_inherits_working_directory(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            workflow = root / ".github" / "workflows" / "check.yml"
            workflow.parent.mkdir(parents=True)
            workflow.write_text(
                "jobs:\n"
                "  check:\n"
                "    defaults:\n"
                "      run:\n"
                "        working-directory: tools\n"
                "    steps:\n"
                "      - run: python helper\n",
                encoding="utf-8",
            )
            helper = root / "tools" / "helper"
            helper.parent.mkdir()
            helper.write_text("python nested\n", encoding="utf-8")
            nested = root / "tools" / "nested"
            nested.write_text(
                f"Run /{FEATURE_DEV} before continuing.\n",
                encoding="utf-8",
            )

            failures = self.optional_boundary_failures(
                {
                    "enabledPlugins": {
                        f"{FEATURE_DEV}@{CLAUDE_PLUGIN_MARKETPLACE}": True,
                        "ievo@ievo-skills": True,
                    }
                },
                root,
                [
                    (workflow, "100644"),
                    (helper, "100644"),
                    (nested, "100644"),
                ],
            )

            self.assertEqual(len(failures), 1)
            self.assertIn("tools/nested", failures[0])

    def test_interpreter_script_discovery_is_recursive(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            workflow = root / ".github" / "workflows" / "check.yml"
            workflow.parent.mkdir(parents=True)
            workflow.write_text(
                "jobs:\n  check:\n    steps:\n      - run: python tools/helper\n",
                encoding="utf-8",
            )
            helper = root / "tools" / "helper"
            helper.parent.mkdir()
            helper.write_text("python tools/nested\n", encoding="utf-8")
            nested = root / "tools" / "nested"
            nested.write_text(
                f"Run /{FEATURE_DEV} before continuing.\n",
                encoding="utf-8",
            )

            failures = self.optional_boundary_failures(
                {
                    "enabledPlugins": {
                        f"{FEATURE_DEV}@{CLAUDE_PLUGIN_MARKETPLACE}": True,
                        "ievo@ievo-skills": True,
                    }
                },
                root,
                [
                    (workflow, "100644"),
                    (helper, "100644"),
                    (nested, "100644"),
                ],
            )

            self.assertEqual(len(failures), 1)
            self.assertIn("tools/nested", failures[0])

    def test_bash_rcfile_discovery_continues_to_the_main_script(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            workflow = root / ".github" / "workflows" / "check.yml"
            workflow.parent.mkdir(parents=True)
            workflow.write_text(
                "jobs:\n  check:\n    steps:\n"
                "      - run: bash --rcfile tools/rc tools/helper\n",
                encoding="utf-8",
            )
            rc_file = root / "tools" / "rc"
            rc_file.parent.mkdir()
            rc_file.write_text(
                "export AGENT_POLICY=enabled\n",
                encoding="utf-8",
            )
            helper = root / "tools" / "helper"
            helper.write_text(
                f"Run /{FEATURE_DEV} before continuing.\n",
                encoding="utf-8",
            )

            failures = self.optional_boundary_failures(
                {
                    "enabledPlugins": {
                        f"{FEATURE_DEV}@{CLAUDE_PLUGIN_MARKETPLACE}": True,
                        "ievo@ievo-skills": True,
                    }
                },
                root,
                [
                    (workflow, "100644"),
                    (rc_file, "100644"),
                    (helper, "100644"),
                ],
            )

            self.assertEqual(len(failures), 1)
            self.assertIn("tools/helper", failures[0])

    def test_inline_commands_do_not_select_their_arguments_as_scripts(
        self,
    ) -> None:
        for command in (
            "fish --command=echo tools/helper",
            "pwsh -c echo tools/helper",
            "powershell.exe -C echo tools/helper",
        ):
            with self.subTest(command=command):
                with tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    workflow = root / ".github" / "workflows" / "check.yml"
                    workflow.parent.mkdir(parents=True)
                    workflow.write_text(
                        "jobs:\n  check:\n    steps:\n"
                        f"      - run: {command}\n",
                        encoding="utf-8",
                    )
                    helper = root / "tools" / "helper"
                    helper.parent.mkdir()
                    helper.write_text(
                        f"Run /{FEATURE_DEV} before continuing.\n",
                        encoding="utf-8",
                    )

                    failures = self.optional_boundary_failures(
                        {
                            "enabledPlugins": {
                                (
                                    f"{FEATURE_DEV}@"
                                    f"{CLAUDE_PLUGIN_MARKETPLACE}"
                                ): True,
                                "ievo@ievo-skills": True,
                            }
                        },
                        root,
                        [
                            (workflow, "100644"),
                            (helper, "100644"),
                        ],
                    )

                    self.assertEqual(failures, [])

    def test_interpreter_variants_reach_tracked_scripts_in_full_boundary(
        self,
    ) -> None:
        cases = (
            ("node -C development tools/helper", "tools/helper", False),
            ('python -W "" tools/helper', "tools/helper", False),
            ('python -W "" tools/helper', "tools/helper", True),
            ("ruby --encoding UTF-8 tools/helper", "tools/helper", False),
            ("ruby -E UTF-8 tools/helper", "tools/helper", False),
            ("ruby -EUTF-8 tools/helper", "tools/helper", False),
            (
                "PowerShell -ExecutionPolicy Bypass -File tools/helper",
                "tools/helper",
                False,
            ),
            (r'python "tools\helper"', "tools/helper", False),
            (r"python tools\helper", "tools/helper", False),
            (r'python "tools\helper"', "tools/helper", True),
            (r"python tools\helper", "tools/helper", True),
            ("powershell.exe -File tools/helper", "tools/helper", False),
            ("python.exe -X dev tools/helper", "tools/helper", False),
            ("node.exe -C development tools/helper", "tools/helper", False),
            (
                "node --experimental-loader tools/loader tools/helper",
                "tools/loader",
                False,
            ),
            (
                "node --experimental-loader=tools/loader tools/helper",
                "tools/helper",
                False,
            ),
            (
                "python --future-option arbitrary tools/helper",
                "tools/helper",
                False,
            ),
            (
                "node --future-flag --require=tools/preload tools/helper",
                "tools/preload",
                False,
            ),
            (
                "bash --future-flag --rcfile=tools/rc tools/helper",
                "tools/rc",
                False,
            ),
            (
                "node --future-flag -rtools/preload tools/helper",
                "tools/preload",
                False,
            ),
            (
                "ruby --future-flag -rtools/preload tools/helper",
                "tools/preload",
                False,
            ),
            (
                "python --future-option -- -script",
                "-script",
                False,
            ),
        )
        settings = {
            "enabledPlugins": {
                f"{FEATURE_DEV}@{CLAUDE_PLUGIN_MARKETPLACE}": True,
                "ievo@ievo-skills": True,
            }
        }
        for command, violating_path, use_settings in cases:
            with self.subTest(
                command=command,
                violating_path=violating_path,
                use_settings=use_settings,
            ):
                with tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    if use_settings:
                        consumer = root / ".claude" / "settings.json"
                        consumer.parent.mkdir(parents=True)
                        consumer.write_text(
                            json.dumps(
                                {
                                    "hooks": {
                                        "PreToolUse": [
                                            {
                                                "hooks": [
                                                    {
                                                        "type": "command",
                                                        "command": command,
                                                    }
                                                ]
                                            }
                                        ]
                                    }
                                }
                            ),
                            encoding="utf-8",
                        )
                    else:
                        consumer = root / ".github" / "workflows" / "check.yml"
                        consumer.parent.mkdir(parents=True)
                        consumer.write_text(
                            "jobs:\n  check:\n    steps:\n"
                            f"      - run: {command}\n",
                            encoding="utf-8",
                        )
                    tools = root / "tools"
                    tools.mkdir()
                    repository_scripts = (
                        tools / "helper",
                        tools / "loader",
                        tools / "preload",
                        tools / "rc",
                        root / "-script",
                    )
                    for path in repository_scripts:
                        relative = path.relative_to(root).as_posix()
                        contents = (
                            f"Run /{FEATURE_DEV} before continuing.\n"
                            if relative == violating_path
                            else "Use repository-owned tools.\n"
                        )
                        path.write_text(contents, encoding="utf-8")

                    failures = self.optional_boundary_failures(
                        settings,
                        root,
                        [
                            (consumer, "100644"),
                            *[
                                (path, "100644")
                                for path in repository_scripts
                            ],
                        ],
                    )

                    self.assertEqual(len(failures), 1)
                    self.assertIn(violating_path, failures[0])

    def test_command_boundaries_reach_scripts_in_full_boundary(self) -> None:
        cases = (
            (
                ".github/workflows/check.yml",
                "jobs:\n  check:\n    steps:\n"
                "      - run: >\n"
                "          python\n"
                "          tools/helper\n"
                "        shell: bash\n"
                "      - run: echo done\n",
                "tools/helper",
            ),
            (
                ".github/workflows/check.yml",
                "jobs:\n  check:\n    steps:\n"
                "      - run: >-\n"
                "          python\n"
                "          tools/helper\n"
                "        shell: bash\n",
                "tools/helper",
            ),
            (
                ".github/workflows/check.yml",
                "jobs:\n  check:\n    steps:\n"
                "      - run: >+\n"
                "          python\n"
                "          tools/helper\n"
                "        shell: bash\n",
                "tools/helper",
            ),
            (
                ".github/workflows/check.yml",
                "jobs:\n  check:\n    steps:\n"
                "      - run: python \\\n"
                "          tools/helper\n",
                "tools/helper",
            ),
            (
                ".github/workflows/check.yml",
                "jobs:\n  check:\n    steps:\n"
                "      - run: node --require=tools/preload \\\n"
                "          tools/helper\n",
                "tools/helper",
            ),
            (
                "AGENTS.md",
                "Run `python tools/helper` before continuing.\n",
                "tools/helper",
            ),
            (
                "AGENTS.md",
                "Run $(python tools/helper) before continuing.\n",
                "tools/helper",
            ),
            (
                ".github/workflows/check.yml",
                "jobs:\n  check:\n    steps:\n"
                "      - run: bash < tools/helper\n",
                "tools/helper",
            ),
            (
                ".github/workflows/check.yml",
                "jobs:\n  check:\n    steps:\n"
                "      - run: python -X dev -- < tools/helper\n",
                "tools/helper",
            ),
            (
                ".github/workflows/check.yml",
                "jobs:\n  check:\n    steps:\n"
                r"      - run: python tools/my\ helper"
                "\n",
                "tools/my helper",
            ),
        )
        settings = {
            "enabledPlugins": {
                f"{FEATURE_DEV}@{CLAUDE_PLUGIN_MARKETPLACE}": True,
                "ievo@ievo-skills": True,
            }
        }
        for consumer_relative, consumer_text, violating_path in cases:
            with self.subTest(
                consumer=consumer_relative,
                violating_path=violating_path,
                text=consumer_text,
            ):
                with tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    consumer = root / consumer_relative
                    consumer.parent.mkdir(parents=True, exist_ok=True)
                    consumer.write_text(consumer_text, encoding="utf-8")
                    tools = root / "tools"
                    tools.mkdir()
                    repository_scripts = (
                        tools / "helper",
                        tools / "preload",
                        tools / "my helper",
                    )
                    for path in repository_scripts:
                        relative = path.relative_to(root).as_posix()
                        contents = (
                            f"Run /{FEATURE_DEV} before continuing.\n"
                            if relative == violating_path
                            else "Use repository-owned tools.\n"
                        )
                        path.write_text(contents, encoding="utf-8")

                    failures = self.optional_boundary_failures(
                        settings,
                        root,
                        [
                            (consumer, "100644"),
                            *[
                                (path, "100644")
                                for path in repository_scripts
                            ],
                        ],
                    )

                    self.assertEqual(len(failures), 1)
                    self.assertIn(violating_path, failures[0])

    def test_unreferenced_extensionless_file_is_not_a_required_asset(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            agents = root / "AGENTS.md"
            agents.write_text(
                "Use repository-owned tools.\n",
                encoding="utf-8",
            )
            helper = root / "tools" / "helper"
            helper.parent.mkdir()
            helper.write_text(
                f"Run /{FEATURE_DEV} before continuing.\n",
                encoding="utf-8",
            )

            failures = self.optional_boundary_failures(
                {
                    "enabledPlugins": {
                        f"{FEATURE_DEV}@{CLAUDE_PLUGIN_MARKETPLACE}": True,
                        "ievo@ievo-skills": True,
                    }
                },
                root,
                [
                    (agents, "100644"),
                    (helper, "100644"),
                ],
            )

            self.assertEqual(failures, [])

    def test_inline_interpreter_modes_do_not_select_a_file(self) -> None:
        for invocation in (
            "python -c tools/helper",
            "python -cprint('tools/helper')",
            "python -m tools/helper",
            "python -mtools.helper tools/helper",
            "node --eval tools/helper",
            "node --eval=code tools/helper",
            "ruby -e tools/helper",
            "pwsh -Command tools/helper",
            "pwsh -c echo tools/helper",
            "powershell --command tools/helper",
            "powershell.exe -C echo tools/helper",
            "fish --command echo tools/helper",
            "fish --command=echo tools/helper",
        ):
            with self.subTest(invocation=invocation):
                self.assertEqual(
                    validator.referenced_interpreter_scripts(invocation),
                    set(),
                )

    def test_interpreter_options_select_the_actual_script(self) -> None:
        cases = (
            ("python -X dev tools/helper", {"tools/helper"}),
            ("python -Xdev tools/helper", {"tools/helper"}),
            ("python -W ignore tools/helper", {"tools/helper"}),
            ('python -W "" tools/helper', {"tools/helper"}),
            ('python -X "" tools/helper', {"tools/helper"}),
            ("bash -O extglob tools/helper", {"tools/helper"}),
            (
                "bash --rcfile tools/rc tools/helper",
                {"tools/rc", "tools/helper"},
            ),
            (
                "bash --init-file=tools/rc tools/helper",
                {"tools/rc", "tools/helper"},
            ),
            ("pwsh -ExecutionPolicy Bypass -File tools/helper", {"tools/helper"}),
            ("powershell -File:tools/helper", {"tools/helper"}),
            (
                "node --require tools/preload tools/helper",
                {"tools/preload", "tools/helper"},
            ),
            (
                "node --require=tools/preload tools/helper",
                {"tools/preload", "tools/helper"},
            ),
            ("node -C development tools/helper", {"tools/helper"}),
            ("node --title review-probe tools/helper", {"tools/helper"}),
            ("node --input-type module tools/helper", {"tools/helper"}),
            ("node --inspect-port 9330 tools/helper", {"tools/helper"}),
            (
                "node --experimental-loader tools/loader tools/helper",
                {"tools/loader", "tools/helper"},
            ),
            (
                "node --experimental-loader=tools/loader tools/helper",
                {"tools/loader", "tools/helper"},
            ),
            ("ruby --encoding UTF-8 tools/helper", {"tools/helper"}),
            ("ruby -E UTF-8 tools/helper", {"tools/helper"}),
            ("ruby -EUTF-8 tools/helper", {"tools/helper"}),
            (
                "ruby --external-encoding UTF-8 tools/helper",
                {"tools/helper"},
            ),
            ("ruby --disable gems tools/helper", {"tools/helper"}),
        )
        for invocation, expected in cases:
            with self.subTest(invocation=invocation):
                self.assertEqual(
                    validator.referenced_interpreter_scripts(invocation),
                    expected,
                )

    def test_interpreter_stdin_redirection_selects_the_input_script(self) -> None:
        cases = (
            ("bash < tools/helper", {"tools/helper"}),
            ("python <tools/helper", {"tools/helper"}),
            ('ruby 0<"tools/helper"', {"tools/helper"}),
            ("node < tools/my\\ helper", {"tools/my helper"}),
            ("python -- < tools/helper", {"tools/helper"}),
            ("python -X dev -- < tools/helper", {"tools/helper"}),
        )
        for invocation, expected in cases:
            with self.subTest(invocation=invocation):
                self.assertEqual(
                    validator.referenced_interpreter_scripts(invocation),
                    expected,
                )

    def test_windows_interpreter_names_and_paths_are_canonicalized(self) -> None:
        cases = (
            (
                "PowerShell -ExecutionPolicy Bypass -File tools/helper",
                {"tools/helper"},
            ),
            ("powershell.exe -File tools/helper", {"tools/helper"}),
            ("pwsh.exe -File tools/helper", {"tools/helper"}),
            ("python.exe -X dev tools/helper", {"tools/helper"}),
            ("node.exe -C development tools/helper", {"tools/helper"}),
            (r"python tools\helper", {"tools/helper"}),
            (r'python "tools\helper"', {"tools/helper"}),
        )
        for invocation, expected in cases:
            with self.subTest(invocation=invocation):
                self.assertEqual(
                    validator.referenced_interpreter_scripts(invocation),
                    expected,
                )

    def test_absolute_command_paths_are_not_repository_references(self) -> None:
        for path in (
            "/tmp/helper",
            r"C:\tools\helper",
            r"\\server\share\helper",
        ):
            with self.subTest(path=path):
                self.assertIsNone(validator.local_script_reference(path))

    def test_parent_command_paths_are_retained_for_contextual_resolution(
        self,
    ) -> None:
        for path in ("../tools/helper", "tools/../helper"):
            with self.subTest(path=path):
                self.assertEqual(
                    validator.local_script_reference(path),
                    path,
                )

    def test_unknown_interpreter_options_fail_closed(self) -> None:
        cases = (
            (
                "python --future-option arbitrary tools/helper",
                {"arbitrary", "tools/helper"},
            ),
            (
                "node --future-loader=tools/loader tools/helper",
                {"tools/loader", "tools/helper"},
            ),
            (
                "node --future-flag --require=tools/preload tools/helper",
                {"tools/preload", "tools/helper"},
            ),
            (
                "node --future-flag "
                "--experimental-loader=tools/loader tools/helper",
                {"tools/loader", "tools/helper"},
            ),
            (
                "bash --future-flag --rcfile=tools/rc tools/helper",
                {"tools/rc", "tools/helper"},
            ),
            (
                "node --future-flag -rtools/preload tools/helper",
                {"tools/preload", "tools/helper"},
            ),
            (
                "ruby --future-flag -rtools/preload tools/helper",
                {"tools/preload", "tools/helper"},
            ),
            (
                "python --future-option value -- -script",
                {"value", "-script"},
            ),
            ("python --future-option -- -script", {"-script"}),
        )
        for invocation, expected in cases:
            with self.subTest(invocation=invocation):
                self.assertEqual(
                    validator.referenced_interpreter_scripts(invocation),
                    expected,
                )

    def test_command_boundaries_preserve_repository_script_paths(self) -> None:
        cases = (
            ("python \\\n  tools/helper", {"tools/helper"}),
            (
                "node --require=tools/preload \\\n tools/helper",
                {"tools/preload", "tools/helper"},
            ),
            ("Run `python tools/helper`.", {"tools/helper"}),
            ("$(python tools/helper)", {"tools/helper"}),
            (r"python tools/my\ helper", {"tools/my helper"}),
        )
        for invocation, expected in cases:
            with self.subTest(invocation=invocation):
                self.assertEqual(
                    validator.referenced_interpreter_scripts(invocation),
                    expected,
                )

    def test_claude_behavioral_settings_reject_optional_plugin_references(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            settings_path = root / ".claude" / "settings.json"
            settings_path.parent.mkdir(parents=True)
            settings = self.claude_settings()
            settings["enabledPlugins"][
                f"{FEATURE_DEV}@{CLAUDE_PLUGIN_MARKETPLACE}"
            ] = True
            settings["hooks"] = {
                "PreToolUse": [
                    {
                        "hooks": [
                            {
                                "type": "command",
                                "command": f"/{FEATURE_DEV}",
                            }
                        ]
                    }
                ]
            }
            settings_path.write_text(
                json.dumps(settings),
                encoding="utf-8",
            )

            failures = self.optional_boundary_failures(
                settings,
                root,
            )

            self.assertEqual(len(failures), 1)
            self.assertIn(".claude/settings.json", failures[0])
            self.assertIn(
                f"{FEATURE_DEV}@{CLAUDE_PLUGIN_MARKETPLACE}",
                failures[0],
            )

    def test_claude_marketplace_declarations_are_not_required_consumers(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            settings_path = root / ".claude" / "settings.json"
            settings_path.parent.mkdir(parents=True)
            settings = self.claude_settings()
            settings["enabledPlugins"][
                f"{FEATURE_DEV}@{CLAUDE_PLUGIN_MARKETPLACE}"
            ] = True
            settings["extraKnownMarketplaces"][
                CLAUDE_PLUGIN_MARKETPLACE
            ] = {
                "source": {
                    "source": "github",
                    "repo": "example" + "/optional-tools",
                }
            }
            settings_path.write_text(
                json.dumps(settings),
                encoding="utf-8",
            )

            failures = self.optional_boundary_failures(
                settings,
                root,
            )

            self.assertEqual(failures, [])

    def test_invalid_claude_settings_shapes_are_scanned_as_raw_text(self) -> None:
        settings = {
            "enabledPlugins": {
                f"{FEATURE_DEV}@{CLAUDE_PLUGIN_MARKETPLACE}": True,
                "ievo@ievo-skills": True,
            }
        }
        cases = (
            f'{{"hooks": ["/{FEATURE_DEV}"]',
            json.dumps([f"/{FEATURE_DEV}"]),
        )
        for contents in cases:
            with self.subTest(contents=contents):
                with tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    settings_path = root / ".claude" / "settings.json"
                    settings_path.parent.mkdir(parents=True)
                    settings_path.write_text(contents, encoding="utf-8")

                    self.assertEqual(
                        validator.required_asset_body(
                            Path(".claude/settings.json"),
                            contents,
                        ),
                        contents,
                    )
                    failures = self.optional_boundary_failures(
                        settings,
                        root,
                    )

                    self.assertEqual(len(failures), 1)
                    self.assertIn(".claude/settings.json", failures[0])
                    self.assertIn(
                        f"{FEATURE_DEV}@{CLAUDE_PLUGIN_MARKETPLACE}",
                        failures[0],
                    )

    def test_ignored_instruction_file_is_not_a_repository_asset(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            tracked = root / "AGENTS.md"
            tracked.write_text("Use repository-owned tools.\n", encoding="utf-8")
            ignored = root / "cache" / "AGENTS.md"
            ignored.parent.mkdir()
            ignored.write_text(
                f"Use /{FEATURE_DEV}.\n",
                encoding="utf-8",
            )
            (root / ".gitignore").write_text("cache/\n", encoding="utf-8")
            subprocess.run(
                ["git", "init", "--quiet", str(root)],
                check=True,
            )
            subprocess.run(
                ["git", "-C", str(root), "add", ".gitignore", "AGENTS.md"],
                check=True,
            )

            repository_files = validator.tracked_repository_files(root)
            failures = self.optional_boundary_failures(
                {
                    "enabledPlugins": {
                        f"{FEATURE_DEV}@{CLAUDE_PLUGIN_MARKETPLACE}": True,
                        "ievo@ievo-skills": True,
                    }
                },
                root,
                repository_files,
            )

            self.assertEqual(failures, [])
            self.assertNotIn(
                ignored,
                [path for path, _ in repository_files],
            )

    def test_ievo_overlay_provenance_is_not_a_required_dependency(self) -> None:
        for newline in ("\n", "\r\n"):
            with self.subTest(newline=repr(newline)):
                with tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    overlay = root / ".ievo" / "evolution" / "skills" / "demo.md"
                    overlay.parent.mkdir(parents=True)
                    provenance = newline.join(
                        (
                            "---",
                            "source:",
                            f"  path: plugins/{FEATURE_DEV}/skills/demo",
                            "---",
                            "",
                        )
                    )
                    overlay.write_text(
                        provenance + "# Local vendored overlay" + newline,
                        encoding="utf-8",
                    )
                    settings = {
                        "enabledPlugins": {
                            f"{FEATURE_DEV}@{CLAUDE_PLUGIN_MARKETPLACE}": True,
                            "ievo@ievo-skills": True,
                        }
                    }

                    failures = self.optional_boundary_failures(
                        settings,
                        root,
                        [(overlay, "100644")],
                    )

                    self.assertEqual(failures, [])
                    overlay.write_text(
                        provenance
                        + f"Run /{FEATURE_DEV} before continuing."
                        + newline,
                        encoding="utf-8",
                    )

                    failures = self.optional_boundary_failures(
                        settings,
                        root,
                        [(overlay, "100644")],
                    )

                    self.assertEqual(len(failures), 1)
                    self.assertIn(
                        overlay.relative_to(root).as_posix(),
                        failures[0],
                    )

    def test_dated_ievo_vendoring_heading_is_provenance_only(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            overlay = root / ".ievo" / "evolution" / "skills" / "demo.md"
            overlay.parent.mkdir(parents=True)
            source_repo = "example" + "/agent-tools"
            heading = f"## 2026-07-24 — Vendored from {source_repo}\n"
            provenance = (
                "---\n"
                "source:\n"
                f"  repo: {source_repo}\n"
                "  commit_sha: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n"
                "---\n\n"
            )
            overlay.write_text(
                provenance + "# demo\n\n" + heading + "Initial copy.\n",
                encoding="utf-8",
            )
            marketplace = "overlay" + "-source"
            settings = self.claude_settings()
            settings["extraKnownMarketplaces"][marketplace] = {
                "source": {
                    "source": "github",
                    "repo": source_repo,
                }
            }

            failures = self.optional_boundary_failures(
                settings,
                root,
                [(overlay, "100644")],
            )

            self.assertEqual(failures, [])

            overlay.write_text(
                provenance
                + "# demo\n\n"
                + heading
                + f"Use {source_repo} at runtime.\n",
                encoding="utf-8",
            )
            failures = self.optional_boundary_failures(
                settings,
                root,
                [(overlay, "100644")],
            )

            self.assertEqual(len(failures), 1)
            self.assertIn(f"marketplace source {source_repo}", failures[0])

    def test_nested_ievo_repo_metadata_does_not_authorize_provenance(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            overlay = root / ".ievo" / "evolution" / "skills" / "demo.md"
            overlay.parent.mkdir(parents=True)
            source_repo = "example" + "/agent-tools"
            overlay.write_text(
                "---\n"
                "source:\n"
                "  metadata:\n"
                f"    repo: {source_repo}\n"
                "---\n\n"
                "# demo\n\n"
                f"## 2026-07-24 — Vendored from {source_repo}\n"
                "Initial copy.\n",
                encoding="utf-8",
            )
            marketplace = "nested" + "-source"
            settings = self.claude_settings()
            settings["extraKnownMarketplaces"][marketplace] = {
                "source": {
                    "source": "github",
                    "repo": source_repo,
                }
            }

            failures = self.optional_boundary_failures(
                settings,
                root,
                [(overlay, "100644")],
            )

            self.assertEqual(len(failures), 1)
            self.assertIn(f"marketplace source {source_repo}", failures[0])

    def test_required_agent_asset_symlink_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            subprocess.run(
                ["git", "init", "--quiet", str(root)],
                check=True,
            )
            object_id = subprocess.run(
                ["git", "-C", str(root), "hash-object", "-w", "--stdin"],
                check=True,
                input=b"target.md",
                stdout=subprocess.PIPE,
            ).stdout.decode("ascii").strip()
            subprocess.run(
                [
                    "git",
                    "-C",
                    str(root),
                    "update-index",
                    "--add",
                    "--cacheinfo",
                    f"120000,{object_id},AGENTS.md",
                ],
                check=True,
            )
            failures: list[str] = []
            validator.validate_optional_plugin_boundary(
                {
                    "enabledPlugins": {
                        f"{FEATURE_DEV}@{CLAUDE_PLUGIN_MARKETPLACE}": True,
                        "ievo@ievo-skills": True,
                    }
                },
                failures,
                root,
            )

            self.assertEqual(len(failures), 1)
            self.assertIn("must not be symlinks", failures[0])

    def test_unreadable_required_asset_reports_a_controlled_failure(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            missing = root / "AGENTS.md"

            failures = self.optional_boundary_failures(
                {
                    "enabledPlugins": {
                        f"{FEATURE_DEV}@{CLAUDE_PLUGIN_MARKETPLACE}": True,
                        "ievo@ievo-skills": True,
                    }
                },
                root,
                [(missing, "100644")],
            )

            self.assertEqual(len(failures), 1)
            self.assertIn("unable to read tracked required agent asset", failures[0])

    def test_utf16_powershell_asset_is_scanned(self) -> None:
        source = f"Run /{FEATURE_DEV} before continuing.\n"
        payloads = (
            codecs.BOM_UTF16_LE + source.encode("utf-16-le"),
            codecs.BOM_UTF16_BE + source.encode("utf-16-be"),
        )
        for payload in payloads:
            with self.subTest(bom=payload[:2]):
                with tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    script = root / "scripts" / "agent-check.ps1"
                    script.parent.mkdir()
                    script.write_bytes(payload)

                    failures = self.optional_boundary_failures(
                        {
                            "enabledPlugins": {
                                f"{FEATURE_DEV}@{CLAUDE_PLUGIN_MARKETPLACE}": True,
                                "ievo@ievo-skills": True,
                            }
                        },
                        root,
                        [(script, "100644")],
                    )

                    self.assertEqual(len(failures), 1)
                    self.assertIn(script.relative_to(root).as_posix(), failures[0])

    def test_unknown_required_asset_encoding_fails_closed(self) -> None:
        source = f"Run /{FEATURE_DEV} before continuing.\n"
        payloads = (
            b"\xffinvalid",
            source.encode("utf-16-le"),
            source.encode("utf-16-be"),
            codecs.BOM_UTF32_LE + source.encode("utf-32-le"),
            codecs.BOM_UTF32_BE + source.encode("utf-32-be"),
        )
        for payload in payloads:
            with self.subTest(prefix=payload[:4]):
                with tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    script = root / "scripts" / "agent-check.ps1"
                    script.parent.mkdir()
                    script.write_bytes(payload)

                    failures = self.optional_boundary_failures(
                        {
                            "enabledPlugins": {
                                f"{FEATURE_DEV}@{CLAUDE_PLUGIN_MARKETPLACE}": True,
                                "ievo@ievo-skills": True,
                            }
                        },
                        root,
                        [(script, "100644")],
                    )

                    self.assertEqual(len(failures), 1)
                    self.assertIn("must be UTF-8 or BOM-tagged UTF-16", failures[0])

    def test_exact_identity_reports_the_matching_shared_marketplace_plugin(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            agents = root / "AGENTS.md"
            agents.write_text(
                f"Use `{CODE_REVIEW}@{CLAUDE_PLUGIN_MARKETPLACE}`.\n",
                encoding="utf-8",
            )
            settings = {
                "enabledPlugins": {
                    f"{FEATURE_DEV}@{CLAUDE_PLUGIN_MARKETPLACE}": True,
                    f"{CODE_REVIEW}@{CLAUDE_PLUGIN_MARKETPLACE}": True,
                    f"{PR_REVIEW_TOOLKIT}@{CLAUDE_PLUGIN_MARKETPLACE}": True,
                }
            }

            failures = self.optional_boundary_failures(
                settings,
                root,
            )

            self.assertEqual(len(failures), 1)
            self.assertIn(
                f"{CODE_REVIEW}@{CLAUDE_PLUGIN_MARKETPLACE}",
                failures[0],
            )
            self.assertNotIn(PR_REVIEW_TOOLKIT, failures[0])

    def test_pinned_baseline_plugin_is_not_treated_as_optional(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            agents = root / "AGENTS.md"
            agents.write_text("Use `ievo@ievo-skills`.\n", encoding="utf-8")
            settings = {
                "enabledPlugins": {
                    "ievo@ievo-skills": True,
                }
            }

            failures = self.optional_boundary_failures(
                settings,
                root,
            )

            self.assertEqual(failures, [])

    def test_pinned_identity_stays_exempt_with_optional_marketplace_sibling(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            agents = root / "AGENTS.md"
            agents.write_text("Use `ievo@ievo-skills`.\n", encoding="utf-8")
            settings = {
                "enabledPlugins": {
                    "ievo@ievo-skills": True,
                    f"{MUTABLE_HELPER}@ievo-skills": True,
                }
            }

            failures = self.optional_boundary_failures(
                settings,
                root,
            )

            self.assertEqual(failures, [])

    def test_disabled_or_missing_pinned_identity_is_not_exempt(self) -> None:
        for enabled_plugins in (
            {"ievo@ievo-skills": False},
            {},
        ):
            with self.subTest(enabled_plugins=enabled_plugins):
                with tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    agents = root / "AGENTS.md"
                    agents.write_text(
                        "Use `ievo@ievo-skills`.\n",
                        encoding="utf-8",
                    )

                    failures = self.optional_boundary_failures(
                        {"enabledPlugins": enabled_plugins},
                        root,
                    )

                    self.assertEqual(len(failures), 1)
                    self.assertIn("ievo@ievo-skills", failures[0])


if __name__ == "__main__":
    unittest.main()
