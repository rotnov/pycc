# Fixture: new-case-misses-branching-sites

Reproduces the incident recorded alongside it in `.harden/incidents/new-case-misses-branching-sites/`.

Run:

```bash
uv run .claude/skills/harden/scripts/arena.py .harden/incidents/new-case-misses-branching-sites/fixture --runs 3
```

## Checklist before running

- [ ] `task.md` contains only the task — no hint about the failure
- [ ] `control.md` and `patch.md` differ **only** by the artefact under test
- [ ] `verify.py` catches the workaround, not just the happy path
- [ ] `verify.py` FAILED when run by hand against a deliberately broken tree —
      a control that passes under a broken implementation is not a control
- [ ] any project files the task needs are copied in
