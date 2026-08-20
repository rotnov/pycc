# Session checkpoint — 2026-08-20-11

## Status

`main` is at `ffc585d5` (merge commit for PR [#648](https://github.com/rotnov/pycc/pull/648),
merged 2026-08-20T22:07:13Z). This checkpoint records the end of a task that amended
`AGENTS.md`'s "Keep source files decomposable" wording so the nine oversized-file tracking
issues filed earlier in the day (#544-#552) can be picked up as standalone units of work, and
then closed a gap the change itself introduced in `.claude/skills/issue-implement/SKILL.md`.

## What happened this task

- The repository owner asked (chat instruction, not a GitHub issue) whether a tracking issue
  existed for decomposing `lib.rs` and this project's other oversized Rust source files. None
  did; nine were filed (#544-#552), one per file over the ~1,000-line threshold. Filing them
  exposed a real tension: `AGENTS.md`'s decomposition rule required the work to ride along with
  an unrelated task ("not as a separate dedicated refactor task"), which left no policy basis
  for `issue-select`/`issue-implement` to ever pick one of the nine on its own.
- Fixed by adding an explicit carve-out to `AGENTS.md` and recording the reasoning in
  [D-185](../decisions/D-185-permit-a-dedicated-tracking-issue-per-oversized.md): a dedicated
  tracking issue for one oversized file is itself a legitimate justification for touching that
  file, each pull request against it still extracts only cohesion-driven submodules, and the
  issue is narrowed by comment (never closed per PR) until the file drops under threshold.
- The pinned D-068 reviewer (`ievo:deep-reviewer`) reviewed the change in two rounds; both
  rounds' findings were addressed before merge (a D-021 step 10 paraphrase correction; a spot
  check of issue bodies in round one — see below for round two).
- Opened PR #648. An automated review from `chatgpt-codex-connector` (a `Bot`-type GitHub
  reviewer) correctly flagged that `.claude/skills/issue-implement/SKILL.md`'s workflow had no
  accommodation for D-185's multi-PR, non-closing model: unconditionally requiring `Fixes #N`
  on every PR would let the first partial-decomposition PR against a D-185 issue wrongly close
  the tracker instead of narrowing it. Fixed by extending `issue-implement`'s step 4 with a
  D-185 branch modeled on the file's existing D-080/D-103 stage-then-activate patterns, but
  open-ended rather than fixed-count: every partial-decomposition PR omits `Fixes #N` and is
  followed by a narrowing comment (an authorized write); only the PR that finally brings the
  file under threshold carries the real `Fixes #N`.
- Re-ran the pinned reviewer on that extension; it returned two more findings (one warning, one
  note) — a dispatch-brief sentence in step 4 that hadn't been extended to mention the new D-185
  branch alongside D-080/D-103, and a missing verification-scope/stop-condition parity sentence
  for the D-185 pattern compared to its D-080/D-103 siblings. Both fixed before merge.
- Replied to and resolved the bot's review thread (authorized under `issue-implement`'s
  "Authorized writes" #4 — the author is a confirmed `Bot`-type GitHub account).
- The feature branch needed two rebases during this task against a moving `origin/main` (the
  project's documented concurrent background actor pushed twice: once a session-checkpoint doc
  commit, once PR #649/#652's `README_PATH` fix) — both handled via fetch, rebase, and
  `--force-with-lease` push, never a bare `--force`.
- Merged PR #648 (merge commit `ffc585d5`, `fix/agents-decomposable-wording` branch deleted)
  once all required checks were green, `mergeStateStatus` was `CLEAN`, and no review thread
  remained unresolved. Re-read the full final diff end to end immediately before merging.

## Known follow-ups

- Issues #544-#552 are now selectable by `issue-select` as ordinary standalone candidates (D-185
  already states no issue body needs editing — each already carries the "no milestone —
  cross-cutting" note and description of the tension D-185 now resolves at the policy level).
  Nothing further is required to unblock them; the next `issue-select` pass should simply see
  them as normal candidates.
- `.claude/skills/issue-implement/SKILL.md` now documents the D-185 narrowing-PR pattern but has
  not yet been exercised against a real oversized-file issue. The first session that picks up
  one of #544-#552 is the first live test of that new workflow branch — watch for gaps the two
  D-068 reviewer rounds and the bot review did not catch (this is inherent to documentation
  written before its first real use, not a known defect).

## Where to resume

Nothing is in flight from this task — PR #648 is merged, the review thread is resolved, and the
worktree used for it has been (or should be) removed. A fresh session should re-enter
`issue-select`'s normal workflow at its own step 1 baseline; #544-#552 are now legitimate
candidates for that pass, alongside whatever else is open in the tracker.
