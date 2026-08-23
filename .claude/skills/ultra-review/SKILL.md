---
name: ultra-review
description: Use this alpha project skill when the user wants a periodic, evidence-gated codebase review that files prioritized (`P1`/`P2`/`P3`), milestone-scoped GitHub issues for what it finds — "run an ultra review", "do a periodic code review and file issues", a standing recurring-review directive, or a scheduled/automated invocation with no issue named. Reads a GitHub-native checkpoint to review only the diff since the last run, dispatches the pinned D-068 deep-reviewer once, maps its `blocker`/`warning`/`note` findings to `P1`/`P2`/`P3`, deduplicates against already-filed `ultra-review`-labeled issues, and files the rest autonomously within a bounded evidence bar — without a human approving each payload. Does not implement anything itself and does not pick an issue to work (`issue-select`'s job) or plan one (`issue-to-plan`'s job).
---

# ultra-review (Alpha)

Periodically re-review the codebase for the class of problem that survives any single
pull request's own D-068 pre-merge gate — drift that only shows up once several merges
have accumulated, or a doc/config file nobody happened to touch in the diff that
introduced the drift — and turn confirmed findings into prioritized, milestone-scoped
GitHub issues. This skill mutates no tracked file and writes no implementation code; its
only public writes are the GitHub issues (and the checkpoint issue) described below.

This project-local skill is alpha, matching `issue-select`, `issue-to-plan`,
`issue-implement`, and `next-milestone`'s own alpha status.

Design background, including the empirical comparison that ruled out a broader
two-pass review architecture in favor of the single pinned pass this skill actually
uses, is recorded in
[`docs/superpowers/specs/2026-08-05-ultra-review-skill-design.md`](../../../docs/superpowers/specs/2026-08-05-ultra-review-skill-design.md).

## Scope

Use it when asked to run a periodic/ultra code review, when a standing recurring-review
directive is in effect, or when an external scheduler (Claude Code Routines, a Codex
cron equivalent) invokes it. Do not use it to plan or implement anything found —
`issue-to-plan`/`issue-implement` pick those issues up separately, the same way they
pick up any other filed issue. This skill does not schedule its own recurrence; wiring
an actual recurring trigger is a separate setup step using the existing `schedule`
mechanism, outside this skill's own definition.

## Workflow

### 1. Baseline

Fetch and record the default-branch tip, per
[D-021](../../../docs/decisions/D-021-agent-task-preflight-and-documentation-refresh.md).
Confirm `gh` resolves the target repository. No Rust build or `cargo doc` step is needed
— this skill makes no code changes.

### 2. Read the checkpoint

Ensure the `ultra-review-checkpoint` and `ultra-review` labels exist
(`gh label list --repo <owner>/<repo> --limit 200 --json name`; create either missing
one with `gh label create`), the same way `next-milestone`'s own step 3 ensures its
GitHub milestone exists before using it. Two `gh` behaviors this step depends on,
both confirmed against the live repository on this skill's first run:

- pass `--limit` explicitly — bare `gh label list` returns only the first 30 labels,
  so a repository whose label set has grown past that could report an existing label
  as missing;
- `gh label create` **exits 1** on a label that already exists
  (`label with name "..." already exists`). That is this step's success condition, not
  a failure: the goal is "the label exists", so treat an already-exists error as
  satisfied and continue. Only an error of any other kind stops the run.

This label check must run before the checkpoint search below, not be folded into it:
`gh issue list --label <name>` returns an empty list and exit 0 for a label that does
not exist at all, so it cannot distinguish "no checkpoint yet" from "the label is
missing".

Search for the checkpoint tracking issue:
`gh issue list --repo <owner>/<repo> --label ultra-review-checkpoint --state all --json number,body`.

- **Exactly one found:** parse its body's `Last reviewed commit: <sha>` line, and, when
  present, the `Counting started`, `Cumulative`, and `Cumulative by model` lines (the
  fixed, machine-parseable block described in step 9). A checkpoint body that predates
  this format carries none of those three lines — step 9's migration rule covers that
  case. A checkpoint that already has them but omits `Cumulative by model` (a bootstrap,
  or a history with nothing filed yet) has every bucket at zero; that is the expected
  steady state, not a parse error.
- **None found (first run ever):** create it —
  `gh issue create --repo <owner>/<repo> --title "Ultra-review checkpoint — do not close" --label ultra-review-checkpoint --body "<bootstrap block, below>"`.
  Step 9's block has fields with no meaning on a bootstrap, where no review has run, so
  the bootstrap body is fixed here rather than left to invention: keep step 9's exact
  delimiters and `Last reviewed commit:` line (set to the current default-branch tip),
  set `Reviewed at:` to the bootstrap timestamp, write `Last run:` literally as
  `bootstrap -- no review performed; checkpoint initialized at the current default-branch tip`,
  and write the two new counters in their steady-state shape immediately:
  `Counting started: <same tip sha> at <same bootstrap timestamp>` and
  `Cumulative: 0 runs, 0 findings, 0 filed, 0 deduped-as-existing`, with no
  `Cumulative by model` line (no bucket has ever gone nonzero). This means a
  bootstrapping repository and a repository migrating an existing pre-format checkpoint
  both land on the identical steady-state shape from their very next real run, with no
  separate transitional format to reason about.
  Report this bootstrap explicitly: the very first run reviews an empty range and is a
  deliberate one-time no-op, not a full historical sweep — reviewing the repository's
  entire history in one pass was explicitly rejected in the design doc's alternatives.
- **More than one found:** stop and report — a race between two concurrent runs (this
  project has a documented concurrent actor that can push to `main` mid-session, so
  concurrent ultra-review runs are a real possibility). Do not create a third
  checkpoint issue or silently pick one as authoritative.

This is a **GitHub-native checkpoint**, not a tracked file: a local checkpoint file
would not survive this project's own ephemeral-worktree lifecycle, and committing a
checkpoint bump would need its own pull request every run purely to move a counter.

### 3. Compute the incremental diff

`git diff <checkpoint-sha>..<default-branch-tip>`. If empty, report "nothing new since
`<checkpoint-sha>`" and stop — no dispatch, no checkpoint update; advancing the
checkpoint on an empty diff would be a no-op that only obscures the timestamp of the
last real review.

Save the diff to a scratch file (outside the working tree) rather than inlining it
directly into the dispatch prompt below — a large diff embedded as literal prompt text
is unreliable to construct correctly; handing the pinned reviewer a file path and
letting it `Read` the file, exactly as this skill's own design bake-off did, is the
proven approach.

### 4. Review dispatch

Dispatch the pinned deep reviewer from a structurally verified `ievo@ievo-skills`
install (`scripts/check_claude_reviewer_binding.py`, per D-155) — the exact one
`issue-implement`'s own review loop uses; if no such install can be bound, the review
gate is unavailable — report that and stop rather than substituting a weaker reviewer.
Give it the input shape the pinned `deep-review/SKILL.md` entrypoint itself defines
(`diff`, `changed_files`, `repo_context`), which `docs/AGENT_TOOLING.md`'s "Local review
workflow" section is the policy record for:

```
Review the following diff for gaps, drift, and consistency issues.

## Repo context
<one-line repo description>

## Changed files
<git diff <checkpoint-sha>..<default-branch-tip> --name-only>

## Diff
The full diff is saved at <scratch file path> (git diff <checkpoint-sha>..<default-branch-tip>). Read that file with your Read tool to get the complete diff text before starting the 11-point checklist.
```

Add two short, explicit reminders on top of the standard 11 points — informational
context supplied by this skill's own dispatch prompt, not a modification of the pinned
agent artifact itself — learned directly from this skill's own design bake-off, where
both compared review variants missed the same real bug:

- when a changed line updates a status/summary statement, check whether adjacent
  *unchanged* prose still describes the prior state;
- check whether files under `docs/sessions/` were updated to checkpoint the merges in
  this diff range, per that convention's own non-blocking note in `AGENTS.md`.

### 5. Triage findings into candidate issues

For each finding the reviewer reports, in severity order (`blocker` first):

- **Severity → priority:** `blocker` → `P1:`, `warning` → `P2:`, `note` → `P3:`. Every
  filed issue must cite a concrete `file:line` — a finding without one is never filed,
  regardless of severity.
- **Milestone:** read `docs/ROADMAP.md`'s ordered `## vX.Y` sections the same way
  `next-milestone` does, to find the currently active milestone, then apply
  `AGENTS.md`'s D-021 step 9 milestone-at-filing rule unchanged: assign that milestone.
  Under [D-192](../../../docs/decisions/D-192-bound-the-tracker-with-milestone-at-filing-a.md)
  a milestone is required at filing and the former "no milestone — cross-cutting" escape
  hatch is closed — a finding that fits no milestone is not filed as its own issue. A
  cross-cutting finding (CI governance, website, agent tooling — those three exact area
  names, since umbrella discovery matches on the title string) becomes
  a checklist item on that area's one standing umbrella issue instead, and a finding about
  the project's own apparatus must first clear D-192's filing bar: it becomes tracked work
  only if it can cause an incorrect merge decision or hide a compiler defect. Everything
  else is a `docs/AGENT_RETROSPECTIVE.md` line, not an issue.
- **Title:** `<P-prefix>: <one-line summary>`.
- **Body:** the finding's file:line, the concrete issue description, the reviewer's own
  suggestion, the diff range reviewed (`<checkpoint-sha>..<default-branch-tip>`), the
  umbrella issue this finding was added to whenever it was routed there instead of filed
  on its own (D-192 permits no other milestone-less destination), and a fixed footer:
  `Filed by the ultra-review skill from range <checkpoint-sha>..<tip>.` — so a human
  or a later `issue-select` run can trace which review pass produced it.
- **Label:** every candidate issue gets `ultra-review`.

### 6. Deduplicate before publishing

For each candidate, before filing:
`gh issue list --repo <owner>/<repo> --label ultra-review --state all --search "<file path>"`,
narrowed further by a keyword from the finding's own description. A match against the
same file with a materially overlapping description is already tracked — skip filing
it; leave a `gh issue comment` on the existing issue only if the new finding adds
concrete new evidence (a different line, a recurrence after the file changed again),
otherwise skip silently. This mirrors the duplicate-search discipline `pycc-feedback`
and [D-022](../../../docs/decisions/D-022-autonomous-public-ievo-bug-reporting.md)
already require for public writes in this repository.

### 7. Publish (autonomous, bounded)

File every candidate that survives step 6 without per-payload human confirmation,
mirroring D-022's precedent for the one other standing autonomous-write authority this
project has already accepted (`gh issue create`, `P1:`/`P2:`/`P3:` title prefix,
`ultra-review` label, milestone from step 5 when assigned). The bound that keeps this
safe, mirroring D-022's own evidence bar:

- every filed issue cites a concrete file:line (step 5);
- the dedup pass (step 6) always runs first;
- secrets are never included verbatim — the pinned reviewer's own point 11 already
  flags and redacts credential-shaped content before it reaches the finding text;
- if the dedup-survived candidate count in one run exceeds roughly 15 (a first-run
  bootstrap gone wrong, a reviewer mis-triggering), stop short of filing any of them
  and report the batch for a human to look at once, rather than auto-filing a flood.

### 8. Attribute filed findings to a model

For every finding **filed** in step 7 (never for a deduped or dropped candidate — only
a filed finding has a public, citable file:line worth attributing):

1. **Range-validity precondition**, checked once per run, not once per finding: confirm
   `git merge-base --is-ancestor <checkpoint-sha-read-in-step-2> <default-branch-tip>`
   succeeds. If it fails (a force-push or history rewrite upstream since the checkpoint
   was recorded), skip attribution entirely for every finding filed this run, bucket
   every one of them as `unattributed (history check failed)` in this run's own
   report — a report-only annotation, not a new checkpoint bucket — and continue with
   step 9's checkpoint write otherwise unaffected.
2. For the finding's cited `file:line` (step 5 already guarantees every filed finding
   has one), run
   `git blame -L <line>,<line> --porcelain <default-branch-tip> -- <file>` to get the
   blamed commit sha. Pin these exact flags — no `-w`, no `-M`/`-C`. Copy-detection
   (`-M`/`-C`) is deliberately excluded: it would attribute a copied-in defective line
   to whoever originally wrote it elsewhere, answering "who wrote this text first"
   rather than the defect-attribution question this step asks — whose commit is
   responsible for this line existing in its current, flagged form here.
   - `git blame` itself can fail: the reviewer's cited line can be stale, off-by-one,
     or past the file's current length, and the cited file can have been deleted since.
     On any `git blame` failure for a filed finding, bucket that finding as
     `unattributed (blame failed)` in this run's own report and continue — never let
     one unresolvable citation stop the run or step 9's checkpoint write.
3. **Diff-range-relative check:**
   `git merge-base --is-ancestor <blamed-sha> <checkpoint-sha>`.
   - If true, the blamed line predates this run's reviewed range — it is pre-existing
     code the diff merely referenced. Bucket as `unattributed` and credit no model.
     Never blame whoever last touched a pre-existing line for a defect the current diff
     merely exposed.
   - If false, the blamed line was introduced within
     `<checkpoint-sha>..<default-branch-tip>` — isolate its actual trailer block first,
     rather than scanning the whole commit body, with
     `git log -1 --format=%B <blamed-sha> | git interpret-trailers --parse --no-divider`.
     `--parse` is Git's documented alias for `--only-trailers --only-input --unfold`: it
     emits only the message's real trailing trailer lines and drops any trailer-shaped
     text that merely appears earlier in the body as quoted prose or an example — this
     repository's own commit messages routinely quote
     `Co-Authored-By: <Model> <email>` as documentation, which scanning the full body
     would misattribute. `--no-divider` is required in addition to `--parse`: without it,
     `git interpret-trailers` treats a bare `---` line as a `format-patch`-style divider
     and stops parsing there, and this repository's own `SKILL.md`/agent front matter —
     quoted verbatim in commit bodies just as often as the trailer syntax itself — opens
     and closes with exactly that `---` line, which would silently truncate the scan
     before it ever reaches the real trailing trailer block.
     - This `git log | git interpret-trailers` pipeline can also fail outright — not
       merely return zero trailers, but exit non-zero, for any reason (for example
       `--no-divider` being unrecognized by a `git` older than the version that
       introduced it). On any non-zero exit from this pipeline for a filed finding,
       bucket that finding as `unattributed (extraction failed)` in this run's own
       report and continue — never let one unresolvable citation stop the run or
       step 9's checkpoint write.

     From that already-trailer-scoped output, select the
     `Co-Authored-By`-keyed line(s) case-insensitively — a real trailer key can be
     spelled in any case, e.g. `CO-AUTHORED-BY:`, and `git interpret-trailers`
     preserves the key's original casing rather than normalizing it — with
     `re.compile(r'^co-authored-by:\s*(.+?)\s*<', re.MULTILINE | re.IGNORECASE)`, and
     collect the **distinct** names found (a squash-merged commit can repeat the
     identical trailer line several times; that is one distinct name, not several):
     - Zero names → `unattributed`. **Never fall back to the commit's own author or
       committer identity** — most commits in this repository carry no trailer at all,
       and falling back to `%an`/`%cn` would silently attribute most defects to the
       repository owner rather than reporting them as genuinely unattributed.
     - Exactly one distinct name → attribute to that name, unless it contains a
       literal `,` or `:`, which cannot serialize into the `Cumulative by model` line's
       `<name>: <count>` pairs — bucket as `ambiguous` instead of `unattributed` in that
       case, since the name *was* resolved, just not safely serializable.
     - Two or more distinct names (the squash-merge-concatenation case, several
       different collaborators' trailers on one landed commit) → `ambiguous`. Never
       guess between them.

### 9. Advance the checkpoint

**Concurrency safety.** This step has two independent race hazards, both from the
project's documented concurrent actor that can push to the checkpoint issue or to
`main` mid-session:

- **Lost update:** re-read the checkpoint issue immediately before writing —
  `gh issue view <number> --repo <owner>/<repo> --json body` — and add this run's own
  contribution on top of *that* fresh read's `Cumulative` values, not the values read
  back in step 2. This narrows, though does not eliminate, the window in which a
  concurrent run's own write could be silently overwritten (`gh issue edit` has no
  conditional-write/`If-Match` support to close it completely).
- **Overlapping range:** compare the fresh read's `Last reviewed commit` to the sha this
  run read in step 2.
  - **Unchanged:** proceed with the fresh-read-and-add write below.
  - **Changed:** a concurrent run already advanced the checkpoint since this run started.
    Do not write the checkpoint at all — leave it exactly as the fresh read found it.
    This is a deliberate, permanent undercount of the `Cumulative` counters, not a
    retry: the reviewed ranges may overlap in that case, so adding this run's own
    counts on top would double-count whatever the concurrent run already reviewed.
    Report it plainly — the filed issues from this run are already real and public, and
    the next run's own diff against the (unmoved) checkpoint will simply re-review a
    range that step 6's dedup pass is already positioned to absorb without refiling
    anything.

Edit the checkpoint tracking issue's body
(`gh issue edit <number> --repo <owner>/<repo> --body "<new block>"`) to:

```
<!-- ultra-review-checkpoint -->
Last reviewed commit: <new default-branch tip sha>
Reviewed at: <ISO 8601 timestamp>
Last run: <N findings, M filed, K deduped-as-existing>
Counting started: <sha> at <ISO 8601 timestamp>
Cumulative: <R> runs, <F> findings, <M> filed, <K> deduped-as-existing
Cumulative by model: <name>: <count>, <name>: <count>, unattributed: <count>, ambiguous: <count>
<!-- /ultra-review-checkpoint -->
```

The `runs` label in the `Cumulative` line is a fixed literal regardless of count
(`1 runs`, `12 runs`) — deliberate, to keep the field a stable, greppable string rather
than adding conditional singular/plural grammar to a machine-parsed line.

- **`Counting started`** is written once — the first time a run writes this new format
  — and never changes again. It records the sha and timestamp this same write is using
  for `Last reviewed commit`/`Reviewed at`, not the repository's first-ever commit; see
  the no-backfill rule below.
- **`Cumulative`** counts only runs whose step 9 actually wrote the checkpoint: an
  empty-diff run (step 3 already stops before reaching here) and a concurrency-aborted
  run (the "Changed" branch above) both never increment it. Add this run's own
  `R=1, F=<findings>, M=<filed>, K=<deduped>` to the fresh-read values.
- **Migration:** when the fresh read (like step 2's own earlier read) predates this
  format — it carries none of the `Counting started`/`Cumulative`/`Cumulative by model`
  lines at all, not merely zero values for them — treat every cumulative counter as
  absent/zero for this run's own computation, so "add this run's own contribution to the
  fresh-read values" reduces to writing this run's own `R=1, F=<findings>, M=<filed>,
  K=<deduped>` outright. This single write is simultaneously the migration and the first
  counted run: `Counting started` is set as described above (this write's own new
  `Last reviewed commit`/`Reviewed at`, not the old checkpoint's sha), never to the old
  body's sha. There is no separate transitional format between the old three-line body
  and this step's steady-state shape — matching step 2's bootstrap case exactly.
- **`Cumulative by model`** accumulates the same way `Cumulative` does, one bucket at a
  time: each bucket's new count is that bucket's fresh-read count plus this run's own
  step 8 contribution to that bucket (a bucket absent from the fresh read, including
  every bucket on a migration write per the Migration bullet above, starts from zero).
  Never compute this line from this run's own step 8 results alone — doing so would
  discard every prior run's accumulated per-model counts on the shared checkpoint.
  Once accumulated, list a bucket only once its cumulative count is nonzero; omit the
  whole line when every bucket is still zero. Bucket key is the trailer's name string
  exactly as it appears (no normalization). Sort buckets alphabetically, case-insensitive,
  by name, with `unattributed` and `ambiguous` always last in that fixed order — each
  still shown only when nonzero, same as any other bucket.
- **No backfill:** this feature counts only runs from its own first write forward.
  Do not backfill counts for issues #422/#423/#424 or for any run that predates this
  format — those earlier runs' individual finding/filed/deduped counts are not
  reconstructible from the checkpoint issue's edit history, and guessing at them would
  misrepresent real counts as if they were measured.

No repository commit, no pull request. If the edit above fails after findings were
already filed, retry once; if it still fails, report the exact new tip sha in this
run's final report — the filed issues are already real and public, and the next run's
own diff against the stale checkpoint would only redundantly re-review already-triaged
commits, which step 6's dedup pass is specifically there to absorb. This retry-once
rule is independent of the concurrency-abort rule above: retry applies to an ordinary
`gh` failure (network, transient API error), while the concurrency abort is a
deliberate one-time non-write triggered by detecting a concurrent run, never retried.

## Output

The checkpoint range reviewed, the reviewer's raw findings, which were filed (with
issue numbers and URLs) versus deduped as already-tracked versus dropped for missing
evidence, the new checkpoint state — including the updated `Cumulative` totals and the
`Cumulative by model` per-model breakdown — and, if the batch-size guard tripped, the
full undispatched batch for a human to review. Report this run's own three report-only
attribution annotation counts (findings bucketed `unattributed (history check failed)`,
findings bucketed `unattributed (blame failed)`, and findings bucketed
`unattributed (extraction failed)`, step 8) alongside the checkpoint's
persisted buckets — they explain any gap between this run's filed-finding count and the
sum of buckets the checkpoint actually recorded for it, without being counted in the
checkpoint themselves. If step 9's concurrency check aborted the write, report that
explicitly too, including the fresh `Last reviewed commit` sha it found instead.
