#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Scaffolds an arena fixture for ONE incident.

Why a script and not an instruction: step 6.2 requires a fixture that reproduces
the incident, and requiring something the skill cannot produce is how the earlier
version of this gap was "fixed" — with a sentence, which did not hold. A fixture
per incident is the point; the one shipped under fixtures/ is a worked example,
not a thing to reuse.

Usage: uv run scripts/new-fixture.py <topic-slug> [target-dir]
  default target: .harden/incidents/<topic-slug>/fixture
"""
import re
import subprocess
import sys
from pathlib import Path

TASK = """<!-- The prompt the agent receives. Nothing else — no hints about the failure,
     no "be careful with X", no meta-instructions. If the task has to mention
     the trap, the fixture is testing the prompt, not the artefact. -->
"""

CONTROL = """<!-- CONTROL: the project rules WITHOUT the artefact under test.
     Copy the real AGENTS.md the incident happened under, minus the new rule.
     Named control.md, not AGENTS.md, on purpose: a stray AGENTS.md inside the
     repo can be picked up as if it were the project's actual rules. The arena
     renames it when copying into the throwaway run directory. -->
"""

PATCH = """<!-- PATCH: identical to control.md plus the artefact under test.
     The only difference between the two files is what is being proven. -->
"""

VERIFY = '''#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Objective check. Exit 0 = the task was solved correctly.

Check the RESULT, not the process. And check for the WORKAROUND, not just the
happy path: a model that cannot satisfy a rule tends to weaken the constraint
instead — in one real run it lowered a version requirement rather than build the
environment the rule demanded. A verify that only asks "was there output" passes
that cheat.

The contract: this script runs in the run WORKDIR with no arguments — the
arena passes nothing else. Interrogate the workspace: files on disk, git
state, executed effects. The agent's prose lives in `_agent-output.txt`
beside you, but grading prose is grading the story, and stories are gameable
— measured: an agent reworded its answer specifically to beat a phrase grep,
and three field fixtures graded a `response.md` no arena ever writes, so
only agents that happened to author that filename could pass at all.
"""
import sys
from pathlib import Path


def fail(msg: str) -> None:
    print(f"FAIL: {msg}")
    sys.exit(1)


# fail("not implemented")

print("OK")
'''

README = """# Fixture: {topic}

Reproduces the incident recorded alongside it in `.harden/incidents/{topic}/`.

Run:

```bash
uv run .claude/skills/harden/scripts/arena.py {dir} --runs 3
```

## Checklist before running

- [ ] `task.md` contains only the task — no hint about the failure
- [ ] `control.md` and `patch.md` differ **only** by the artefact under test
- [ ] `verify.py` catches the workaround, not just the happy path
- [ ] `verify.py` FAILED when run by hand against a deliberately broken tree —
      a control that passes under a broken implementation is not a control
- [ ] any project files the task needs are copied in
"""


MIN_ARGS = 2  # script name + topic slug


def main() -> int:
    if len(sys.argv) < MIN_ARGS:
        print("usage: new-fixture.py <topic-slug> [target-dir]")
        return 2

    topic = sys.argv[1]
    if not re.fullmatch(r"[a-z0-9]+(-[a-z0-9]+)*", topic):
        print(f"topic must be lowercase-with-hyphens: {topic!r}")
        return 2

    # Anchor on the repository being hardened, not on the current directory. An
    # agent that has wandered into the skill's own source tree would otherwise
    # file the fixture there — observed live: an incident was recorded in one
    # repository while its fixture and arena report landed in another, which by
    # this skill's own rule makes the incident unreproducible.
    root = Path(
        subprocess.run(["git", "rev-parse", "--show-toplevel"],
                       capture_output=True, text=True, check=False).stdout.strip()
        or ".",
    )
    default = root / ".harden/incidents" / topic / "fixture"
    d = Path(sys.argv[2]) if len(sys.argv) > MIN_ARGS else default
    if d.exists():
        print(f"already exists: {d}")
        return 2
    d.mkdir(parents=True)

    (d / "task.md").write_text(TASK, encoding="utf-8")
    (d / "control.md").write_text(CONTROL, encoding="utf-8")
    (d / "patch.md").write_text(PATCH, encoding="utf-8")
    (d / "README.md").write_text(README.format(topic=topic, dir=d), encoding="utf-8")

    verify = d / "verify.py"
    verify.write_text(VERIFY, encoding="utf-8")
    verify.chmod(0o755)   # no-op on Windows, harmless

    print(f"created: {d.resolve()}")
    print("  (confirm this is the project being hardened, not the skill's own repository)\n")
    print("next:")
    print("  1. write task.md       — the prompt, and nothing but the prompt")
    print("  2. control.md/patch.md — they must differ only by the artefact")
    print("  2b. optional setup.py — for state copying cannot carry (git repo, venv);")
    print("      runs with the working copy as cwd, and is not copied into the run")
    print("  3. write verify.py     — the result, and the workaround")
    print("  4. copy in whatever project files the task needs")
    print(f"  5. uv run .claude/skills/harden/scripts/arena.py {d} --runs 3")
    return 0


if __name__ == "__main__":
    sys.exit(main())
