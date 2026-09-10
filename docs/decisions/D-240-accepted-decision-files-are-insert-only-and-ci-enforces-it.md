---
id: D-240
title: "Accepted decision files are insert-only, and CI enforces it"
status: accepted
---

## D-240: Accepted decision files are insert-only, and CI enforces it

- Status: accepted (Part 2 of #77, issue #1000, is the pull request that
  depends on it)
- Context: `AGENTS.md` has said since the decisions log existed that an
  accepted decision is never rewritten -- a change of mind is a new entry
  that supersedes the old one. `docs/decisions/README.md`'s preamble and
  `docs/decisions/TEMPLATE.md` restate it, and D-151 (the per-file split)
  relies on it for the convention of dated inline correction notes. Nothing
  enforced it. PR #74 appended ` (The switch has since happened -- see
  D-033.)` to the accepted Consequences text of D-032, and the edit reached
  `main` with every required check green; Part 1 of #77 (#999, PR #1001)
  restored the original line by hand. A replay of a prototype guard over
  the 35 first-parent commits in `7ae33ae1..4b317abe` that modified an
  existing `docs/decisions/D-*.md` found 19 that would have failed it --
  including Part 1's own restoring merge, which is itself an in-place
  replacement of an accepted line. The rule as written was too coarse to
  enforce, because the tree legitimately changes accepted files in four
  shapes: a status transition (`accepted` to `superseded` in both the
  frontmatter `status:` line and the body `- Status:` line, with
  continuation lines as in D-141 and D-178), a narrowing annotation that
  keeps the frontmatter `accepted` while rewriting the first `- Status:`
  line (D-024, D-066, D-130, D-185), an inserted dated correction note
  (D-086, D-109, D-116, D-119, D-127), and the filling-in of one of the
  eighteen D-151 index-only stubs (`Index-only: no long-form entry recorded
  yet.`), eight of which are already `accepted`.
- Decision: An accepted or superseded decision file's *original lines are
  immutable*. Concretely, comparing the file at a pull request's base
  revision with the file at its head:
  - Every base line must reappear in the head verbatim and in the same
    order. Whatever else the head contains is an insertion, and inserted
    lines are the only way to amend an accepted entry: a dated line such as
    `- Amendment (YYYY-MM-DD): ...` or a dated note block, placed by
    convention after the `- Status:` line (style, not enforced). A typo in
    an accepted entry is corrected the same way -- an inserted dated
    correction, not an edit -- so the first such pull request does not
    "fix" the guard instead. A same-line append (the PR #74 shape) changes
    the line and is therefore never an insertion.
  - Exactly two base lines may be *replaced* rather than kept, and each
    only by a line of its documented shape: the frontmatter `status:` line
    (line 4, fixed there by the frontmatter grammar) by `status: accepted`
    or `status: superseded`, and the *first* body line starting with
    `- Status:` by a line that starts with `- Status: accepted` or
    `- Status: superseded`, optionally followed by whitespace and an
    annotation (`- Status: superseded by D-NNN`, `- Status: accepted (one
    clause is narrowly superseded by D-NNN)`). The frontmatter status may
    move from `accepted` to `superseded` and never back to `accepted` or to
    `proposed`; the body line may carry either that transition or a
    narrowing annotation while the frontmatter stays `accepted`. A
    replacement carrying any other value (`proposed`, `rejected`, a blank
    `- Status: `) is a violation that names the offending head line. Any
    other `- Status:` line, and any body line that merely starts with
    `status:`, is ordinary frozen text.
  - A D-151 index-only stub -- a base file whose ninth line (0-based
    `splitlines()` index 8) is exactly
    `Index-only: no long-form entry recorded yet.` *and* which has no
    `- Status:` line -- may replace its five stub body lines with the
    long-form entry, and only with one: the exemption applies only when the
    replaced stub body itself carries a well-formed body status line
    (`- Status: accepted` or `- Status: superseded`, with an optional
    annotation) -- the first such line after the frontmatter and its
    closing blank line, with every frozen base line after the stub
    reappearing in order after it. A status line placed after the frozen
    tail, or inside the frontmatter, is not part of the replaced body and
    unlocks nothing. A head that deletes the stub body outright, or
    replaces it with text that has no such line in that region, is judged
    under the strict walk with the stub body frozen, and the violation says
    so (`index-only stub replaced without a long-form entry: no
    '- Status: accepted' or '- Status: superseded' line in the replaced
    stub body`). The marker test is positional, not membership: the
    exemption unfreezes lines 7-11 by number, so a file carrying the marker
    anywhere else is not the modelled shape and stays under the strict walk.
    Its frontmatter and every base line after the stub (D-005's appended
    supersession paragraph) stay frozen. Both conditions are required so the
    marker cannot be inserted into a long-form entry in one pull request and
    used to unlock a rewrite in the next: every long-form entry has a
    `- Status:` line, and under this rule that line can never be removed.
  - Deleting or renaming a frozen file is a violation; so is replacing it
    with a symlink or any other diff shape the guard does not model (it
    fails closed). A `proposed` file and a file that is new in the pull
    request are unconstrained. A trailing-newline-only change is not a
    violation.
  - The guard is *base-relative*: `scripts/check_decision_immutability.py`
    diffs only the event's own `base..head` range, so pre-existing text is
    never re-judged and the 19 historical in-place edits are neither
    retrofitted nor reverted. It runs as the `Check accepted-decision
    immutability (issue 1000)` step of `.github/workflows/ci.yml`'s
    `governance` job, which the required `ci-gate` context needs
    unconditionally, and the step is bound by name and command in
    `D171_GOVERNANCE_POLICY_STEPS` so the base-owned `audit` rejects a head
    that drops, conditions, or replaces it. Locally the same check is
    `python3 -B scripts/check_decision_immutability.py --base "$(git
    merge-base origin/main HEAD)" --head HEAD`.
  - The verdict is an exact greedy subsequence walk, not
    `difflib.SequenceMatcher` opcodes: on repetitive ADR-like line
    sequences the matcher (even with `autojunk=False`) reports pure
    insertions as deletions in 303 of 20,000 fuzzed cases, while the walk
    is exact for the subsequence test. `difflib` is used only to print
    context after a violation.
- Alternatives:
  - *A checked-in digest table of accepted files.* Rejected: the pull
    request that edits an entry also updates the digest, so the table
    proves nothing a reviewer could not already see.
  - *Placing the guard in `workflow-policy.yml`'s base-owned `audit`.*
    Rejected: that job downloads only workflow YAML, the manifest and a
    fixed protected-path set as data, never a checkout; judging 235
    decision files would need per-blob downloads, and the `governance` +
    `D171_GOVERNANCE_POLICY_STEPS` binding already gives the base-owned
    side a hold on the step once it is on `main`.
  - *Diffing `base.sha..pull_request.head.sha` instead of the merge
    commit.* Rejected: a branch behind `main` would see `main`'s own
    inserted amendments to a frozen file as deletions. On `pull_request`
    the checkout is `refs/pull/N/merge`, whose first parent is `base.sha`,
    so a two-dot diff against it is exact; the local form uses the merge
    base for the same reason.
  - *Allowing a same-line append.* Rejected: it is exactly the defect the
    guard exists for.
  - *Repairing the eight files whose `](#d-...)` anchors predate the
    per-file split (D-022, D-024, D-054, D-066, D-127, D-130, D-185,
    D-192) in place.* Rejected: a link-target-only exemption would let a
    pull request silently re-point a reference. The anchors stay as they
    are.
  - *Retrofitting the 19 historical in-place edits.* Rejected: they are
    history; the guard judges only the range in front of it.
- Consequences: D-151's "dated inline correction notes" convention
  continues as *inserted* notes, and D-236's reference to "this project's
  append-only rule" now has a stating document. Superseding or narrowing an
  accepted decision touches at most the two status lines of the old file;
  the substance goes in the new entry. Filling in an accepted stub is
  allowed and expected. Because the step, its policy-table entry, the
  `tests/fixtures/policy-successors/ci-d171.yml` update and the digest
  rotation all ship in one pull request, only the head-controlled `ci-gate`
  enforces the step during that pull request; the base-owned `audit` starts
  binding it with the next pull request after the merge (the same window
  #936 accepted for the decisions-index step; D-172's two-pull-request
  counterexample does not apply because no existing frozen constant changes
  meaning). A pull request that edits
  `scripts/check_decision_immutability.py` alongside `docs/decisions/` must
  be read as one unit by the reviewer -- the head controls the script, so
  the script is a trust boundary in the same sense as every other
  governance checker (see the closing paragraph of the 2026-09-05 "Filed an
  issue without searching the open list" entry in
  `docs/AGENT_RETROSPECTIVE.md`). This decision supersedes nothing: D-151
  and D-033 stand unchanged.
