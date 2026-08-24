#!/usr/bin/env python3
"""Regression test for check-reflexive-stop.py, run directly (`python3
test_check_reflexive_stop.py`) or with any unittest runner.

Transcript fixtures below mirror the real Claude Code JSONL shape observed
on this machine: a tool_use block is always followed, after the tool_result
round-trip, by a *separate* assistant message for any further text — an
assistant message with tool_use cannot also carry trailing text in the same
message. Each user-role tool-result entry carries both a top-level
"toolUseResult" field and a message.content block of type "tool_result",
which is exactly what the hook's is_tool_result_entry() keys on.
"""
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HOOK = Path(__file__).resolve().parent.parent / "check-reflexive-stop.py"


def assistant(*, tool_use=None, text=None):
    content = []
    if tool_use is not None:
        content.append({"type": "tool_use", "name": tool_use, "id": "toolu_1", "input": {}})
    if text is not None:
        content.append({"type": "text", "text": text})
    return {"message": {"role": "assistant", "content": content}}


def tool_result(text="ok"):
    return {
        "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_1", "content": text}]},
        "toolUseResult": {"stdout": text, "stderr": "", "interrupted": False},
    }


def human(text):
    return {"message": {"role": "user", "content": [{"type": "text", "text": text}]}}


def meta_reminder(text="<system-reminder>...</system-reminder>"):
    # A mid-turn system-reminder injection: role:"user", isMeta:true, not a
    # tool result. Must not be mistaken for the true turn boundary.
    return {"message": {"role": "user", "content": [{"type": "text", "text": text}]}, "isMeta": True}


def write_transcript(path, entries):
    with open(path, "w", encoding="utf-8") as fh:
        for entry in entries:
            fh.write(json.dumps(entry) + "\n")


def run_hook(transcript_path, stop_hook_active=False):
    payload = {"transcript_path": str(transcript_path), "stop_hook_active": stop_hook_active}
    proc = subprocess.run(
        [sys.executable, str(HOOK)],
        input=json.dumps(payload),
        capture_output=True,
        text=True,
        timeout=10,
    )
    blocked = False
    if proc.stdout.strip():
        try:
            blocked = json.loads(proc.stdout)["decision"] == "block"
        except (json.JSONDecodeError, KeyError):
            pass
    return proc.returncode, blocked


class TestCheckReflexiveStop(unittest.TestCase):
    def setUp(self):
        self.tmpdir = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmpdir.cleanup)
        self.transcript = Path(self.tmpdir.name) / "transcript.jsonl"

    def test_dispatch_then_reflexive_wait_is_blocked(self):
        # The exact real-transcript shape: dispatch tool_use, its tool_result
        # round-trip, then a *separate* assistant message announcing waiting.
        write_transcript(self.transcript, [
            human("do the thing"),
            assistant(tool_use="Agent"),
            tool_result(),
            assistant(text="Запустил трассировщик, жду его отчёта."),
        ])
        code, blocked = run_hook(self.transcript)
        self.assertEqual(code, 0)
        self.assertTrue(blocked)

    def test_dispatch_then_verification_after_is_allowed(self):
        write_transcript(self.transcript, [
            human("do the thing"),
            assistant(tool_use="Agent"),
            tool_result(),
            assistant(tool_use="Bash"),
            tool_result(),
            assistant(text="Checked PR state directly while the agent runs; ждём отчёта."),
        ])
        code, blocked = run_hook(self.transcript)
        self.assertEqual(code, 0)
        self.assertFalse(blocked)

    def test_verification_only_before_dispatch_still_blocks(self):
        # A Read/Grep done while deciding whether to dispatch must not
        # excuse skipping verification after the dispatch itself.
        write_transcript(self.transcript, [
            human("do the thing"),
            assistant(tool_use="Read"),
            tool_result(),
            assistant(tool_use="Agent"),
            tool_result(),
            assistant(text="Dispatched the tracer. Waiting for the report."),
        ])
        code, blocked = run_hook(self.transcript)
        self.assertEqual(code, 0)
        self.assertTrue(blocked)

    def test_no_dispatch_is_allowed(self):
        write_transcript(self.transcript, [
            human("do the thing"),
            assistant(tool_use="Bash"),
            tool_result(),
            assistant(text="Done, жду дальнейших указаний."),
        ])
        code, blocked = run_hook(self.transcript)
        self.assertEqual(code, 0)
        self.assertFalse(blocked)

    def test_stop_hook_active_never_reblocks(self):
        write_transcript(self.transcript, [
            human("do the thing"),
            assistant(tool_use="Agent"),
            tool_result(),
            assistant(text="Жду отчёта."),
        ])
        code, blocked = run_hook(self.transcript, stop_hook_active=True)
        self.assertEqual(code, 0)
        self.assertFalse(blocked)

    def test_malformed_stdin_fails_open(self):
        proc = subprocess.run(
            [sys.executable, str(HOOK)],
            input="not json",
            capture_output=True,
            text=True,
            timeout=10,
        )
        self.assertEqual(proc.returncode, 0)
        self.assertEqual(proc.stdout.strip(), "")

    def test_missing_transcript_fails_open(self):
        code, blocked = run_hook(Path(self.tmpdir.name) / "does-not-exist.jsonl")
        self.assertEqual(code, 0)
        self.assertFalse(blocked)

    def test_invalid_utf8_transcript_fails_open(self):
        with open(self.transcript, "wb") as fh:
            fh.write(b'{"message": {"role": "user", "content": [{"type": "text", "text": "\xff\xfe broken"}]}}\n')
        code, blocked = run_hook(self.transcript)
        self.assertEqual(code, 0)
        self.assertFalse(blocked)

    def test_mid_turn_system_reminder_does_not_truncate_the_walk(self):
        # A system-reminder (role:"user", isMeta:true) landing between the
        # dispatch call and the final text must not be mistaken for the true
        # turn boundary — the walk must still reach back to the dispatch.
        write_transcript(self.transcript, [
            human("do the thing"),
            assistant(tool_use="Agent"),
            tool_result(),
            meta_reminder(),
            assistant(text="Дождался напоминания и всё равно жду отчёта."),
        ])
        code, blocked = run_hook(self.transcript)
        self.assertEqual(code, 0)
        self.assertTrue(blocked)

    def test_malformed_transcript_lines_are_skipped(self):
        with open(self.transcript, "w", encoding="utf-8") as fh:
            fh.write("not json at all\n")
            fh.write("[1, 2, 3]\n")  # valid JSON, not a dict — must not crash
            fh.write(json.dumps(human("do the thing")) + "\n")
            fh.write(json.dumps(assistant(tool_use="Agent")) + "\n")
            fh.write(json.dumps(tool_result()) + "\n")
            fh.write("\n")  # blank line
            fh.write(json.dumps(assistant(text="Жду отчёта.")) + "\n")
        code, blocked = run_hook(self.transcript)
        self.assertEqual(code, 0)
        self.assertTrue(blocked)

    def test_message_field_not_a_dict_does_not_crash(self):
        with open(self.transcript, "w", encoding="utf-8") as fh:
            fh.write(json.dumps(human("do the thing")) + "\n")
            fh.write('{"message": "not-a-dict"}\n')
            fh.write(json.dumps(assistant(tool_use="Agent")) + "\n")
            fh.write(json.dumps(tool_result()) + "\n")
            fh.write(json.dumps(assistant(text="Жду отчёта.")) + "\n")
        code, blocked = run_hook(self.transcript)
        self.assertEqual(code, 0)
        self.assertTrue(blocked)


if __name__ == "__main__":
    unittest.main()
