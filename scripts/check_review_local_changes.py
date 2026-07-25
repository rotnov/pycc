#!/usr/bin/env python3
"""Exercise the shared local-review contract through either client entrypoint."""

from __future__ import annotations

import argparse
import base64
import hashlib
import importlib.util
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
CANONICAL = ROOT / ".claude/skills/review-local-changes/SKILL.md"
CODEX_WRAPPER = ROOT / ".agents/skills/review-local-changes/SKILL.md"
CLAUDE_METADATA = (
    ROOT / ".claude/skills/review-local-changes/agents/openai.yaml"
)
CODEX_METADATA = (
    ROOT / ".agents/skills/review-local-changes/agents/openai.yaml"
)
HELPER = (
    ROOT / ".claude/skills/review-local-changes/scripts/prepare_review.py"
)


def load_helper():
    spec = importlib.util.spec_from_file_location("prepare_review", HELPER)
    if spec is None or spec.loader is None:
        raise AssertionError("review preparation helper could not be loaded")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def git_output(repo: Path, *args: str, input_bytes: bytes | None = None) -> str:
    result = subprocess.run(
        ["git", "-C", str(repo), *args],
        check=True,
        input=input_bytes,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
    )
    return result.stdout.decode("utf-8").strip()


def git(repo: Path, *args: str) -> None:
    git_output(repo, *args)


def write_reviewer(path: Path, *, safe: bool) -> None:
    tools = "  - Read\n  - Grep\n" if safe else "  - Read\n  - Bash\n"
    path.write_text(
        "---\n"
        "name: deep-reviewer\n"
        "tools:\n"
        f"{tools}"
        "disallowedTools:\n"
        "  - Edit\n"
        "  - Write\n"
        "  - WebSearch\n"
        "---\n",
        encoding="utf-8",
    )


def build_fixture(root: Path) -> Path:
    repo = root / "repo"
    repo.mkdir()
    git(repo, "init", "-b", "main")
    git(repo, "config", "user.email", "review@example.invalid")
    git(repo, "config", "user.name", "Review Fixture")
    (repo / "tracked.txt").write_text("base\n", encoding="utf-8")
    (repo / "nested").mkdir()
    (repo / "nested/child.txt").write_text("nested\n", encoding="utf-8")
    marketplace = repo / ".agents" / "plugins"
    marketplace.mkdir(parents=True)
    (marketplace / "marketplace.json").write_text(
        json.dumps(
            {
                "plugins": [
                    {
                        "name": "ievo",
                        "source": {
                            "ref": (
                                "7d5f3e12d0556cb6c5df2974e2babe0433674186"
                            )
                        },
                    }
                ]
            }
        ),
        encoding="utf-8",
    )
    git(
        repo,
        "add",
        "tracked.txt",
        "nested/child.txt",
        ".agents/plugins/marketplace.json",
    )
    git(repo, "commit", "-m", "base")
    git(repo, "update-ref", "refs/remotes/origin/main", "HEAD")
    git(
        repo,
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/main",
    )
    git(repo, "switch", "-c", "feature")
    (repo / "committed.txt").write_text("committed\n", encoding="utf-8")
    git(repo, "add", "committed.txt")
    git(repo, "commit", "-m", "feature")
    (repo / "staged.txt").write_text("staged\n", encoding="utf-8")
    git(repo, "add", "staged.txt")
    (repo / "tracked.txt").write_text("working\n", encoding="utf-8")
    (repo / "untracked.txt").write_text("untracked\n", encoding="utf-8")
    return repo


def validate_entrypoint(
    client: str,
    installed_reviewer_manifest: Path | None = None,
) -> None:
    canonical_text = CANONICAL.read_text(encoding="utf-8")
    if client == "codex":
        wrapper = CODEX_WRAPPER.read_text(encoding="utf-8")
        assert ".claude/skills/review-local-changes/SKILL.md" in wrapper
        assert "exact merge-base commit" in wrapper
        assert "Never substitute the branch" in wrapper
    else:
        assert canonical_text.startswith("---\nname: review-local-changes\n")
    assert "scripts/prepare_review.py" in canonical_text
    assert "content_sources" in canonical_text
    assert "top-level `state`" in canonical_text
    assert "cat-file blob" in canonical_text
    assert "python3 -I -" in canonical_text
    assert 'cd "$trusted_cwd"' in canonical_text
    assert "working_content" in canonical_text
    assert "Never let the reviewer reopen a live working-tree path" in canonical_text
    assert (
        "python3 .claude/skills/review-local-changes/scripts/prepare_review.py"
        not in canonical_text
    )
    assert CODEX_METADATA.read_bytes() == CLAUDE_METADATA.read_bytes()
    helper_text = HELPER.read_text(encoding="utf-8")
    assert helper_text.count('"--no-textconv"') == 8
    assert '"GIT_OPTIONAL_LOCKS"] = "0"' in helper_text
    assert '"--no-optional-locks"' in helper_text

    helper = load_helper()
    helper.validate_repository_pin(ROOT)
    if installed_reviewer_manifest is not None:
        helper.validate_reviewer_manifest(installed_reviewer_manifest)

    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        repo = build_fixture(root)
        safe_manifest = root / "safe-reviewer.md"
        write_reviewer(safe_manifest, safe=True)
        safe_digest = hashlib.sha256(safe_manifest.read_bytes()).hexdigest()

        shadow_root = root / "python-shadow-fixture"
        shadow_root.mkdir()
        (shadow_root / "hashlib.py").write_text(
            "raise RuntimeError('reviewed branch module was imported')\n",
            encoding="utf-8",
        )
        isolation_probe = subprocess.run(
            [sys.executable, "-I", "-"],
            cwd=shadow_root,
            check=False,
            input=b"import argparse, hashlib, pathlib\nprint('isolated')\n",
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        assert isolation_probe.returncode == 0, isolation_probe.stderr.decode(
            "utf-8",
            errors="replace",
        )
        assert isolation_probe.stdout == b"isolated\n"

        index_path = repo / ".git/index"
        index_before = index_path.read_bytes()
        result = helper.prepare_review(
            repo,
            safe_manifest,
            expected_reviewer_sha256=safe_digest,
        )
        assert index_path.read_bytes() == index_before
        assert [scope["name"] for scope in result["scopes"]] == [
            "committed",
            "staged",
            "working",
        ]
        working = result["scopes"][2]
        assert working["changed_files"] == ["tracked.txt", "untracked.txt"]
        committed = result["scopes"][0]
        staged = result["scopes"][1]
        assert all(
            source["kind"] == "git-blob"
            for source in committed["content_sources"].values()
        )
        assert all(
            source["kind"] == "git-blob"
            for source in staged["content_sources"].values()
        )
        assert all(
            source["kind"] == "worktree"
            and len(source["sha256"]) == 64
            for source in working["content_sources"].values()
        )
        untracked_payload = working["working_content"]["untracked.txt"]
        assert untracked_payload["encoding"] == "base64"
        assert untracked_payload["status"] == "untracked"
        assert base64.b64decode(untracked_payload["data"]) == b"untracked\n"
        assert untracked_payload["sha256"] == hashlib.sha256(
            b"untracked\n"
        ).hexdigest()
        assert untracked_payload["size"] == len(b"untracked\n")
        assert (
            untracked_payload["sha256"]
            == working["content_sources"]["untracked.txt"]["sha256"]
        )
        tracked_payload = working["working_content"]["tracked.txt"]
        assert tracked_payload["status"] == "tracked"
        assert base64.b64decode(tracked_payload["data"]) == b"working\n"
        outside_secret = root / "outside-secret.txt"
        outside_secret.write_text("outside secret\n", encoding="utf-8")
        tracked_path = repo / "tracked.txt"
        tracked_path.unlink()
        try:
            tracked_path.symlink_to(outside_secret)
        except OSError:
            tracked_path.write_text("working\n", encoding="utf-8")
        else:
            assert base64.b64decode(tracked_payload["data"]) == b"working\n"
            tracked_path.unlink()
            tracked_path.write_text("working\n", encoding="utf-8")
        assert result["state"]["head"] == git_output(repo, "rev-parse", "HEAD")
        assert len(result["state"]["index_sha256"]) == 64
        assert len(result["state"]["status_sha256"]) == 64

        original_state = result["state"]
        tracked = repo / "tracked.txt"
        tracked.write_text("mutated\n", encoding="utf-8")
        same_status_mutation = helper.prepare_review(
            repo,
            safe_manifest,
            expected_reviewer_sha256=safe_digest,
        )
        assert (
            same_status_mutation["state"]["status_sha256"]
            == original_state["status_sha256"]
        )
        assert (
            same_status_mutation["state"]["working_content_sources"]
            != original_state["working_content_sources"]
        )
        tracked.write_text("working\n", encoding="utf-8")

        untracked = repo / "untracked.txt"
        untracked.write_text("secret-sentinel\n", encoding="utf-8")
        same_status_untracked = helper.prepare_review(
            repo,
            safe_manifest,
            expected_reviewer_sha256=safe_digest,
        )
        assert (
            same_status_untracked["state"]["status_sha256"]
            == original_state["status_sha256"]
        )
        changed_payload = same_status_untracked["state"][
            "working_content"
        ]["untracked.txt"]
        assert base64.b64decode(changed_payload["data"]) == b"secret-sentinel\n"
        assert (
            changed_payload
            != original_state["working_content"]["untracked.txt"]
        )
        untracked.write_text("untracked\n", encoding="utf-8")

        stable_state = helper.prepare_review(
            repo,
            safe_manifest,
            expected_reviewer_sha256=safe_digest,
        )["state"]
        staged_path = repo / "staged.txt"
        staged_path.write_text("replaced\n", encoding="utf-8")
        git(repo, "add", "staged.txt")
        replaced_index = helper.prepare_review(
            repo,
            safe_manifest,
            expected_reviewer_sha256=safe_digest,
        )["state"]
        assert replaced_index["status_sha256"] == stable_state["status_sha256"]
        assert replaced_index["index_sha256"] != stable_state["index_sha256"]
        staged_path.write_text("staged\n", encoding="utf-8")
        git(repo, "add", "staged.txt")

        stable_state = helper.prepare_review(
            repo,
            safe_manifest,
            expected_reviewer_sha256=safe_digest,
        )["state"]
        old_head = git_output(repo, "rev-parse", "HEAD")
        head_tree = git_output(repo, "rev-parse", "HEAD^{tree}")
        moved_head = git_output(
            repo,
            "commit-tree",
            head_tree,
            "-p",
            old_head,
            "-m",
            "identity-only head movement",
        )
        git(
            repo,
            "update-ref",
            "refs/heads/feature",
            moved_head,
            old_head,
        )
        moved_state = helper.prepare_review(
            repo,
            safe_manifest,
            expected_reviewer_sha256=safe_digest,
        )["state"]
        assert moved_state["status_sha256"] == stable_state["status_sha256"]
        assert moved_state["head"] != stable_state["head"]
        git(
            repo,
            "update-ref",
            "refs/heads/feature",
            old_head,
            moved_head,
        )

        flags_root = root / "index-flags-fixture"
        flags_root.mkdir()
        flags_repo = build_fixture(flags_root)
        flags_tracked = flags_repo / "tracked.txt"
        flags_tracked.write_text("base\n", encoding="utf-8")
        for enable, disable, label in [
            ("--assume-unchanged", "--no-assume-unchanged", "assume-unchanged"),
            ("--skip-worktree", "--no-skip-worktree", "skip-worktree"),
        ]:
            git(flags_repo, "update-index", enable, "tracked.txt")
            try:
                helper.prepare_review(
                    flags_repo,
                    safe_manifest,
                    expected_reviewer_sha256=safe_digest,
                )
            except helper.ReviewPreparationError as error:
                assert label in str(error)
            else:
                raise AssertionError(f"{label} index entry was accepted")
            git(flags_repo, "update-index", disable, "tracked.txt")

        unsafe_manifest = root / "unsafe-reviewer.md"
        write_reviewer(unsafe_manifest, safe=False)
        try:
            helper.prepare_review(
                repo,
                unsafe_manifest,
                expected_reviewer_sha256=safe_digest,
            )
        except helper.ReviewPreparationError:
            pass
        else:
            raise AssertionError("unsafe reviewer was accepted")

        nested = repo / "subdirectory"
        nested.mkdir()
        try:
            helper.prepare_review(
                nested,
                safe_manifest,
                expected_reviewer_sha256=safe_digest,
            )
        except helper.ReviewPreparationError:
            pass
        else:
            raise AssertionError("non-root review repository was accepted")

        reviewer_link = root / "reviewer-link.md"
        try:
            reviewer_link.symlink_to(safe_manifest)
        except OSError:
            pass
        else:
            try:
                helper.prepare_review(
                    repo,
                    reviewer_link,
                    expected_reviewer_sha256=safe_digest,
                )
            except helper.ReviewPreparationError:
                pass
            else:
                raise AssertionError("symlinked reviewer artifact was accepted")

        git(repo, "symbolic-ref", "--delete", "refs/remotes/origin/HEAD")
        try:
            helper.prepare_review(
                repo,
                safe_manifest,
                expected_reviewer_sha256=safe_digest,
            )
        except helper.ReviewPreparationError:
            pass
        else:
            raise AssertionError("missing origin/HEAD was accepted")

        try:
            helper.prepare_review(
                repo,
                safe_manifest,
                default_ref="origin/missing",
                expected_reviewer_sha256=safe_digest,
            )
        except helper.ReviewPreparationError:
            pass
        else:
            raise AssertionError("invalid explicit default ref was accepted")

        empty_tree = git_output(repo, "mktree", input_bytes=b"")
        unrelated = git_output(repo, "commit-tree", empty_tree, "-m", "unrelated")
        git(repo, "update-ref", "refs/remotes/origin/unrelated", unrelated)
        try:
            helper.prepare_review(
                repo,
                safe_manifest,
                default_ref="origin/unrelated",
                expected_reviewer_sha256=safe_digest,
            )
        except helper.ReviewPreparationError:
            pass
        else:
            raise AssertionError("missing merge base was accepted")

        git(
            repo,
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        )
        secret = root / "outside-secret.txt"
        secret.write_text("must-not-be-read\n", encoding="utf-8")
        symlink = repo / "untracked-link"
        try:
            symlink.symlink_to(secret)
        except OSError:
            pass
        else:
            result = helper.prepare_review(
                repo,
                safe_manifest,
                expected_reviewer_sha256=safe_digest,
            )
            encoded = json.dumps(result)
            assert "must-not-be-read" not in encoded
            assert any(
                entry["kind"] == "symlink"
                for scope in result["scopes"]
                for entry in scope["excluded_entries"]
            )

            staged_link = repo / "staged-link"
            staged_link.symlink_to(secret)
            git(repo, "add", "staged-link")
            result = helper.prepare_review(
                repo,
                safe_manifest,
                expected_reviewer_sha256=safe_digest,
            )
            staged_scope = next(
                scope for scope in result["scopes"] if scope["name"] == "staged"
            )
            assert "staged-link" not in staged_scope["changed_files"]
            assert any(
                entry["path"] == "staged-link"
                and entry["kind"] == "symlink"
                and "must-not-be-read" not in entry["target"]
                for entry in staged_scope["excluded_entries"]
            )

            git(repo, "commit", "-m", "commit staged symlink")
            result = helper.prepare_review(
                repo,
                safe_manifest,
                expected_reviewer_sha256=safe_digest,
            )
            committed_scope = next(
                scope for scope in result["scopes"] if scope["name"] == "committed"
            )
            assert "staged-link" not in committed_scope["changed_files"]
            assert any(
                entry["path"] == "staged-link" and entry["kind"] == "symlink"
                for entry in committed_scope["excluded_entries"]
            )

            tracked = repo / "tracked.txt"
            tracked.unlink()
            tracked.symlink_to(secret)
            result = helper.prepare_review(
                repo,
                safe_manifest,
                expected_reviewer_sha256=safe_digest,
            )
            working_scope = next(
                scope for scope in result["scopes"] if scope["name"] == "working"
            )
            assert "tracked.txt" not in working_scope["changed_files"]
            assert any(
                entry["path"] == "tracked.txt" and entry["kind"] == "symlink"
                for entry in working_scope["excluded_entries"]
            )

            outside_directory = root / "outside-directory"
            outside_directory.mkdir()
            (outside_directory / "child.txt").write_text(
                "outside child must-not-be-read\n",
                encoding="utf-8",
            )
            nested_child = repo / "nested/child.txt"
            nested_child.unlink()
            nested_directory = repo / "nested"
            nested_directory.rmdir()
            nested_directory.symlink_to(
                outside_directory,
                target_is_directory=True,
            )
            result = helper.prepare_review(
                repo,
                safe_manifest,
                expected_reviewer_sha256=safe_digest,
            )
            encoded = json.dumps(result)
            assert "outside child must-not-be-read" not in encoded
            working_scope = next(
                scope for scope in result["scopes"] if scope["name"] == "working"
            )
            assert "nested/child.txt" not in working_scope["changed_files"]
            assert any(
                entry["path"] == "nested/child.txt"
                and entry["kind"] == "symlink-component"
                for entry in working_scope["excluded_entries"]
            )

            git(
                repo,
                "--literal-pathspecs",
                "add",
                "--all",
                "--",
                "nested",
            )
            result = helper.prepare_review(
                repo,
                safe_manifest,
                expected_reviewer_sha256=safe_digest,
            )
            staged_scope = next(
                scope for scope in result["scopes"] if scope["name"] == "staged"
            )
            assert "nested/child.txt" not in staged_scope["changed_files"]
            assert any(
                entry["path"] == "nested/child.txt"
                and entry["kind"] == "symlink-component"
                and entry["component"] == "nested"
                for entry in staged_scope["excluded_entries"]
            )

            git(repo, "commit", "-m", "commit parent symlink")
            result = helper.prepare_review(
                repo,
                safe_manifest,
                expected_reviewer_sha256=safe_digest,
            )
            committed_scope = next(
                scope for scope in result["scopes"] if scope["name"] == "committed"
            )
            assert "nested/child.txt" not in committed_scope["changed_files"]
            assert any(
                entry["path"] == "nested/child.txt"
                and entry["kind"] == "symlink-component"
                and entry["component"] == "nested"
                for entry in committed_scope["excluded_entries"]
            )

        if os.name != "nt":
            magic = repo / ":(exclude)magic.txt"
            magic.write_text("literal pathspec\n", encoding="utf-8")
            git(
                repo,
                "--literal-pathspecs",
                "add",
                "--",
                magic.name,
            )
            result = helper.prepare_review(
                repo,
                safe_manifest,
                expected_reviewer_sha256=safe_digest,
            )
            staged_scope = next(
                scope for scope in result["scopes"] if scope["name"] == "staged"
            )
            assert magic.name in staged_scope["changed_files"]

            git(repo, "commit", "-m", "commit literal pathspec")
            result = helper.prepare_review(
                repo,
                safe_manifest,
                expected_reviewer_sha256=safe_digest,
            )
            committed_scope = next(
                scope for scope in result["scopes"] if scope["name"] == "committed"
            )
            assert magic.name in committed_scope["changed_files"]

            special = repo / "line\nquote\"backslash\\tab\t.txt"
            special.write_text("special\n", encoding="utf-8")
            result = helper.prepare_review(
                repo,
                safe_manifest,
                expected_reviewer_sha256=safe_digest,
            )
            assert any(
                special.name in scope["changed_files"]
                for scope in result["scopes"]
            )

        pin_root = root / "pin-file-fixture"
        pin_root.mkdir()
        pin_repo = build_fixture(pin_root)
        pin_path = pin_repo / ".agents/plugins/marketplace.json"
        pin_copy = pin_root / "marketplace.json"
        pin_copy.write_bytes(pin_path.read_bytes())
        for invalid_root in (b"[]", b"1", b"null"):
            pin_path.write_bytes(invalid_root)
            try:
                helper.validate_repository_pin(pin_repo)
            except helper.ReviewPreparationError:
                pass
            else:
                raise AssertionError(
                    "non-object repository pin JSON was accepted"
                )
        pin_path.write_bytes(pin_copy.read_bytes())
        pin_path.unlink()
        try:
            pin_path.symlink_to(pin_copy)
        except OSError:
            pass
        else:
            try:
                helper.validate_repository_pin(pin_repo)
            except helper.ReviewPreparationError:
                pass
            else:
                raise AssertionError("symlinked repository pin was accepted")

        parent_root = root / "pin-parent-fixture"
        parent_root.mkdir()
        parent_repo = build_fixture(parent_root)
        plugins = parent_repo / ".agents/plugins"
        outside_plugins = parent_root / "outside-plugins"
        outside_plugins.mkdir()
        marketplace = plugins / "marketplace.json"
        (outside_plugins / "marketplace.json").write_bytes(
            marketplace.read_bytes()
        )
        marketplace.unlink()
        plugins.rmdir()
        try:
            plugins.symlink_to(outside_plugins, target_is_directory=True)
        except OSError:
            pass
        else:
            try:
                helper.validate_repository_pin(parent_repo)
            except helper.ReviewPreparationError:
                pass
            else:
                raise AssertionError("symlinked repository pin parent was accepted")

        masked_root = root / "masked-pin-fixture"
        masked_root.mkdir()
        masked_repo = build_fixture(masked_root)
        masked_pin = masked_repo / ".agents/plugins/marketplace.json"
        safe_pin = masked_pin.read_bytes()
        bad_pin = json.dumps(
            {
                "plugins": [
                    {
                        "name": "ievo",
                        "source": {"ref": "f" * 40},
                    }
                ]
            }
        ).encode()
        masked_pin.write_bytes(bad_pin)
        git(masked_repo, "add", ".agents/plugins/marketplace.json")
        masked_pin.write_bytes(safe_pin)
        try:
            helper.prepare_review(
                masked_repo,
                safe_manifest,
                expected_reviewer_sha256=safe_digest,
            )
        except helper.ReviewPreparationError as error:
            assert "staged repository iEvo pin" in str(error)
        else:
            raise AssertionError("safe worktree masked a mismatched staged pin")

        git(masked_repo, "commit", "-m", "commit mismatched pin")
        git(masked_repo, "add", ".agents/plugins/marketplace.json")
        try:
            helper.prepare_review(
                masked_repo,
                safe_manifest,
                expected_reviewer_sha256=safe_digest,
            )
        except helper.ReviewPreparationError as error:
            assert "committed repository iEvo pin" in str(error)
        else:
            raise AssertionError("safe index masked a mismatched committed pin")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--client", choices=("codex", "claude"), required=True)
    parser.add_argument("--reviewer-manifest", type=Path)
    args = parser.parse_args()
    validate_entrypoint(args.client, args.reviewer_manifest)
    print(f"{args.client} review-local-changes success/failure paths: valid")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
