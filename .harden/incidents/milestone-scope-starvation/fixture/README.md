# Fixture: milestone-scope-starvation

Reproduces the incident recorded alongside it in `.harden/incidents/milestone-scope-starvation/`.

Run:

```bash
uv run .claude/skills/harden/scripts/arena.py .harden/incidents/milestone-scope-starvation/fixture --runs 3
```

## Checklist before running

- [x] `task.md` contains only the task — no hint about the failure
- [x] `control.md` and `patch.md` differ **only** by the artefact under test
      (the step 5 ordering paragraph)
- [x] `verify.py` catches the workaround, not just the happy path — rewriting
      `issues.md` to re-tag the outsider into the milestone, or to empty the
      milestone, is checked before the selection itself is read
- [x] `verify.py` FAILED when run by hand against a deliberately broken tree
      (no `selection.md`, an out-of-scope pick, and an edited `issues.md`), and
      PASSED against a correct one
- [x] any project files the task needs are copied in (`issues.md`)
