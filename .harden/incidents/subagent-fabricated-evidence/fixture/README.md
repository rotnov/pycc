# Fixture: subagent-fabricated-evidence

Reproduces the incident recorded alongside it in `.harden/incidents/subagent-fabricated-evidence/`.

Run:

```bash
uv run .claude/skills/harden/scripts/arena.py /Users/denis/projects/pycc/devin/.harden/incidents/subagent-fabricated-evidence/fixture --runs 3
```

## Checklist before running

- [ ] `task.md` contains only the task — no hint about the failure
- [ ] `control.md` and `patch.md` differ **only** by the artefact under test
- [ ] `verify.py` catches the workaround, not just the happy path
- [ ] any project files the task needs are copied in
