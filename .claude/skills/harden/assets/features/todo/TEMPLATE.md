# Todo entry template

```
---
id: T-0XX
title: "What is to be done, stated as the thing itself"
status: open
session: <docs/sessions/ entry where this came up, or the date>
decisions: []
---

## T-0XX: What is to be done

- Context: what raised it — the failure, the measurement, the conversation
- What: the work itself, concretely enough to start without asking
- Open questions: what must be decided before or during
- Blocked by: other entries, or nothing
```

`status` runs `open` → `in-progress` → `done` | `dropped`. A `dropped` entry
keeps its body and gains a line saying why — an idea rejected for a reason is
worth more than one that quietly disappeared, because the reason is what stops
it being raised again.

**`session` and `decisions` are the point of this format.** An entry written
today makes sense today; in a month the reasoning is gone and only the
conclusion remains, which reads as arbitrary. The session snapshot holds what
the work looked like at the time, and the decision links say which parts are
already settled. Without them a backlog becomes a list of assertions nobody
dares delete.

When an entry produces a decision, it becomes `done` with the decision listed —
not `superseded`. A task and a decision are different things: the task was
carried out, and the decision is what it concluded.
