# Batch: one harden pass over a pile of review findings

Review-fix loops catch failures one at a time; the class only exists in the
pile. A single finding is n=1 and traces to "we erred" — six findings across
four rounds, three of them the same missed check, is a recurrence counter
nobody had to wait months for. So findings are collected during the loops and
harden runs ONCE, at the end.

## The findings file — the whole interface

The host project's own workflow appends to `.harden/findings/<task>.jsonl`,
one JSON object per finding per round, at the moment the round's verdicts are
known. Append-only; never rewritten — and committed on the task branch before
its pull request merges, or the pile dies with the branch:

```json
{"round": 2, "file": "src/x.py", "line": 41, "category": "error-handling",
 "summary": "bare except swallows the timeout the caller distinguishes",
 "disposition": "fixed", "fix_commit": "abc1234", "note": ""}
```

`disposition` is `fixed` or `refuted` — refuted findings are data too: they
accumulate into reviewer-error classes, and those route to the reviewer's own
artefact. `note` carries the refutation reason when there is one. Collection
must not interrupt the loop: write the lines and keep fixing.

## The pass

Invoked as `/harden batch <findings.jsonl>` after the final review round.

1. **Cluster into root-cause classes.** One batched tracer dispatch — the
   subagent receives every finding and returns classes: member findings, the
   shared cause, a candidate termination point each. Never trace findings one
   by one; the clustering IS the value of batching.
2. **Count recurrence twice.** Within the batch (class size), and against the
   journal (`ls .harden/incidents/` for matching topics). An intra-batch count
   is early recurrence — the same signal, bought before production.
3. **Threshold — frequency OR cost.** Class size ≥ 2, or any journal match →
   the full cycle for that class: step 3.5 ladder, artefact, arena, journal.
   A **blocker-severity finding escalates alone**: frequency measures how
   often, not how much, and a catastrophic class must not wait for its second
   occurrence — run the inline mini-trace (what would have caught this before
   review?) and either take the ladder or record "the fix's own tests are the
   guard — build nothing". Remaining singletons → an incident entry with
   `verdict: pending`, no artefact — a counter seed the next batch or the
   journal check will find. This is the spam guard: fifteen findings should
   yield one to three artefacts and a row of counters, never fifteen
   paragraphs.
4. **Expect promotions, not prose.** Every finding here was ALREADY CAUGHT —
   by review. "What detects this class" is answered before the ladder starts,
   so the real question is whether a cheaper, earlier rung can catch it:
   review-tier → static-tier promotions are the main product. A class the
   review is the right rung for gets `build nothing`, recorded.
5. **Artefacts land through the normal steps 5–7**, proposed together as one
   per-class batch. One pass per task; the next pile is the next task's.

## What this is not

Not a repair loop — fixes happened in the review cycles, before this runs.
Not a replacement for the journal — classes that ship artefacts are journaled
exactly as single incidents are, and singletons enter as open counters.
