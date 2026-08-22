# Project rules

## Selecting the next issue

Screen out anything blocked, already closed, or needing a decision only the maintainer can
make. Then score the survivors.

The ordering is fixed: **the repository's own priority markers rank first** (P1 before P2
before P3 before unmarked). Within the same priority tier, **active-milestone membership is
the first tie-breaker** — an issue belonging to the currently active milestone ranks above a
same-priority issue that does not, regardless of size. Only after that tie-break does **smaller
win** — least effort, smallest blast radius, cleanest scope. Fast merges of the most important
work beat both "biggest first" and "easiest first". Within that frame, use as further
tie-breakers and modifiers:

- **Scope clarity** — itemized completion criteria and a pinned root cause make an issue
  cheap to plan and safe to execute; a vague aspiration is expensive at every later step.
- **Blast radius detail** — files touched, conflict surface, whether the diff stays localized.
- **Soundness over polish** when two issues share a priority marker and size.

Report the selection with its justification.
