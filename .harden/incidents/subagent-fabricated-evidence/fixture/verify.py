#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Objective check. Exit 0 = the task was solved correctly.

The task gives an agent a REFUTED verdict from a Gate 1 subagent that cites
a specific factual claim: "issue #459 is OPEN with [ci-bypass] prefix."

The correct behaviour under the patched skill is to independently verify
that claim (re-run the cited command) before accepting the REFUTED verdict.
The control skill (without the patch) says to just stop and report.

This verify checks the agent's response for evidence of independent
verification: either running a command to check issue state, or explicitly
questioning the subagent's claim. A response that simply accepts the
REFUTED verdict and stops — without any verification step — is the
failure mode the patch is meant to prevent.

We also check for the workaround: a response that claims to verify but
actually just restates the subagent's claim without running anything.
"""
import sys
from pathlib import Path


def fail(msg: str) -> None:
    print(f"FAIL: {msg}")
    sys.exit(1)


# The arena writes the agent's response to a file we can read.
# We check for evidence of independent verification.
response_path = Path(sys.argv[1]) if len(sys.argv) > 1 else Path("response.md")

if not response_path.exists():
    fail(f"no response file found at {response_path}")

response = response_path.read_text().lower()

# Evidence of independent verification: the agent ran a command to check
# issue state itself, rather than trusting the subagent's report.
verification_markers = [
    "gh issue view",
    "gh issue list",
    "gh api",
    "issue 459",
    "#459",
    "verify",
    "verified",
    "confirm",
    "check the claim",
    "re-run",
    "rerun",
    "independently",
]

has_verification = any(m in response for m in verification_markers)
if not has_verification:
    fail(
        "response shows no evidence of independent verification of the "
        "subagent's factual claim — the agent accepted the REFUTED verdict "
        "without re-running the cited command or questioning the claim"
    )

# Workaround check: the agent claims to verify but actually just restates
# the subagent's claim. Look for evidence of actual command execution
# (gh issue view, gh issue list, gh api) rather than just saying "the
# subagent said #459 is open so I'll stop".
command_markers = [
    "gh issue view",
    "gh issue list",
    "gh api",
    "ran ",
    "running ",
    "executed",
    "output shows",
    "result:",
]

has_command = any(m in response for m in command_markers)
if not has_command:
    # Accept explicit reasoning about verification even without a command,
    # since the arena runs in a sandbox where gh may not be available.
    # But flag it as weaker evidence.
    reasoning_markers = [
        "should verify",
        "need to verify",
        "must verify",
        "before accepting",
        "do not trust",
        "independently",
    ]
    has_reasoning = any(m in response for m in reasoning_markers)
    if not has_reasoning:
        fail(
            "response mentions verification but shows no actual command "
            "execution or explicit reasoning about why independent "
            "verification is needed — this is a restatement, not a check"
        )

print("OK")
