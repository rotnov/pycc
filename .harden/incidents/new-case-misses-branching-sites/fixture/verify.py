#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Objective check. Exit 0 = the task was solved correctly.

The task hands over a document that branches on a change's class in eight
places and names only two of them. The measured property is whether the plan
decides the six dispatch sites the task never mentions -- the two it does
mention are not evidence of anything.

The workaround for a rule that constrains what a plan must enumerate is to
edit the source document instead of planning against it, or to emit no plan
at all, so both are checked.
"""
import re
import sys
from pathlib import Path


def fail(msg: str) -> None:
    print(f"FAIL: {msg}")
    sys.exit(1)


# --- the input must be intact --------------------------------------------
handbook = Path("handbook.md")
if not handbook.is_file():
    fail("handbook.md is missing")
handbook_text = handbook.read_text(encoding="utf-8")
for anchor in (
    "## Change classes",
    "## Authorized edits",
    "### 2. Classify, then decide whether an entry is owed",
    "### 4. Format the entry by class",
    "### 5. Bump and tag",
    "### 6. Verify the release artefacts",
    "## Reporting",
    "## Appendix: worked examples",
):
    if anchor not in handbook_text:
        fail(f"handbook.md was edited: {anchor!r} is gone -- the task said to plan, not to change it")
if "security" in handbook_text.lower():
    fail("handbook.md was edited: the new class was written into it instead of into the plan")

# --- the plan ------------------------------------------------------------
plan = Path("plan.md")
if not plan.is_file():
    fail("plan.md was not created")
text = plan.read_text(encoding="utf-8")
if len(text.strip()) < 200:
    fail("plan.md is too short to be a plan")
if not re.search(r"security", text, re.I):
    fail("plan.md never mentions the new class")

# Each site is matched by phrases the task itself never uses, so a plan cannot
# satisfy a site by echoing the prompt. The match must also be *local*: the
# site's own words and the new class have to appear within a two-line window,
# or a throwaway sentence elsewhere in the plan credits a site the plan
# explicitly declines to touch.
SITES = {
    "Change classes (the prefix enumeration the rest of the document branches on)":
        r"prefix|enumerat|declared classes|list of classes|change[- ]classes",
    "Authorized edits (which writes the new class authorizes)":
        r"authoriz|writes list|write list|permitted (release )?(writes|actions)|__about__|internal\.md",
    "Step 2, Classify (the per-class outcome list that decides whether an entry is owed)":
        r"classif|step\s*2|section\s*2|entry is owed|\bowed\b",
    "Step 5, Bump and tag (which bump, and whether a tag is cut)":
        r"\bbump|\btag\b|\btags\b|major|minor",
    "Reporting (the monthly summary's per-class migration column)":
        r"report|monthly|summary (table|row)|migration column",
    "Appendix (the worked example per class)":
        r"appendix|worked example",
}
CLASS = re.compile(r"security|new class", re.I)

lines = text.splitlines()
windows = [" ".join(lines[i : i + 2]) for i in range(len(lines))] or [text]
covered = [w for w in windows if CLASS.search(w)]

missing = [
    name
    for name, pattern in SITES.items()
    if not any(re.search(pattern, w, re.I) for w in covered)
]
if missing:
    print("FAIL: the plan covers only some sites; these dispatch sites are undecided:")
    for name in missing:
        print(f"  - {name}")
    sys.exit(1)

print("OK: each of the six dispatch sites the task did not name is decided in the plan")
