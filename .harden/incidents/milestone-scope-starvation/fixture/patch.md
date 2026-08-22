# Project rules

## Selecting the next issue

Screen out anything blocked, already closed, or needing a decision only the maintainer can
make. Then score the survivors.

When a **milestone scope is in effect** — a standing directive naming an active milestone —
**membership in that scope ranks first**, ahead of every other signal. Inside the scope, order
by the repository's own priority markers (P1 before P2 before P3 before unmarked), then by
size, smaller first. Issues outside the scope keep that same marker-then-size order among
themselves, but they sort below every in-scope survivor, so the scope is exhausted before any
of them is reached. Reaching outside the scope is therefore permitted only when the scope
contributes no survivor at all — never merely because an out-of-scope issue looks more
attractive. One consequence is deliberate: under a scope, an *unmarked* in-scope issue
outranks an out-of-scope P1.

With **no milestone scope in effect**, the ordering is fixed: **the repository's own priority
markers rank first** (P1 before P2 before P3 before unmarked), and only then does **smaller
win** — least effort, smallest blast radius, cleanest scope. Fast merges of the most important
work beat both "biggest first" and "easiest first". In either case, use as further
tie-breakers and modifiers:

- **Scope clarity** — itemized completion criteria and a pinned root cause make an issue
  cheap to plan and safe to execute; a vague aspiration is expensive at every later step.
- **Blast radius detail** — files touched, conflict surface, whether the diff stays localized.
- **Soundness over polish** when two issues share a priority marker and size.

Report the selection with its justification.
