# 2026-08-21-11 — issue #544 Part 3 merged; autopilot paused

## Repository state

- Default branch tip: `7db27a7c` ("refactor(types): extract the constraint
  solver into constraints.rs (Part 3 of #544) (#688)"), verified by fetch at
  the time this entry was committed.
- Open pull requests: none.
- Working tree: clean. The task branch `issue-544-part3` was merged and its
  remote ref deleted.

## What was delivered

PR #688, squash-merged as `7db27a7c`. One partial decomposition step against
the D-185 tracking issue #544.

- `crates/pycc_types/src/constraints.rs` is new, 1,975 lines: the type-term
  representation, constraint collection over expressions and blocks,
  unification, and signature checking.
- `crates/pycc_types/src/lib.rs` went from 6,485 to 4,594 lines.
- `crates/pycc_types/src/solver.rs` changed by two lines — a header sentence
  the move falsified, plus one link qualification.
- `docs/AGENT_RETROSPECTIVE.md` gained two entries (see below).

Purity was verified independently of the review, by diffing four ranges of the
pre-move file against the new module and against the four helpers deliberately
returned to `lib.rs`: 15 hunks, 40 changed lines, every one either `pub(crate)`
visibility widening or rustfmt reflow. No logic and no path changes. The two
byte-identical ranges diffed at `rc 0` including doc comments.

Gates on the merged head `8c7d592a`: `ci-gate` and `audit` both green, plus
coverage at 100.00% lines / 100.00% functions / 100.00% regions across 26,159
regions, native build/test on five targets, cross-compile, governance, and
the frontend performance pair. Locally: `check_roadmap_evidence`,
`check_ci_permissions`, the decisions-index freshness check, and the reviewer
binding all returned zero.

## #544 stays open

Its tracked file is 4,594 lines, still 4.6x the ~1,000-line threshold, so per
D-185 the issue is narrowed by comment rather than closed. The narrowing
comment records the current per-file inventory of `crates/pycc_types/src` and
names the next seam, with every line number re-derived against `7db27a7c`
rather than carried over from the branch. The stale line count in the issue
title was corrected from 31,673 to 4,594.

Next seam, for whoever picks this up: `infer_expr`/`infer_expr_in` at lines
947-1981 of `lib.rs`, 1,035 lines in one contiguous run, which would leave
`lib.rs` at about 3,560. Treat that as a starting point and re-derive it —
a published seam boundary was wrong by 45 lines earlier in this arc, because
items that looked like solver internals were general helpers imported by
`monomorphize.rs`.

Also worth noting and **not** covered by #544: `crates/pycc_types/src/tests.rs`
is 25,253 lines, by far the largest file in the crate. If it deserves its own
D-185 tracking issue, that should be filed deliberately.

## Process journal

Two D-066 entries were added, both about how this work was done rather than
about the code:

1. An append-only harden findings journal was destroyed by `cp`, permanently
   losing one refuted finding from the previous iteration. The rule stating the
   file is append-only had been read in the same session, so this is a
   compliance gap; it is now closed at the filesystem rung (the append-only
   flag, on all thirteen journals), proven in both directions against the exact
   command that caused the loss.
2. Issue #687 was filed with a "Current output" block composed from two
   separate command runs rather than pasted from one, reporting three
   diagnostics under a command that emits two. A correcting comment is posted
   there.

While proofreading entry 2 before merge, two further false claims were found
**inside it** — a cross-reference to "the two prose-drift entries below it"
(they are in a different journal) and a "second time in this journal" recurrence
count (no prior entry describes a comparable loss). Both were rhetorical
cross-references composed for weight. They were corrected by amend, at the cost
of a full CI re-run, and the entry now carries a postscript recording the
sharper lesson: a recurrence count is a claim about a file, and it is written by
counting.

## Follow-ups opened or advanced

- **#687** — three unresolvable intra-doc links in `pycc_types`, invisible to
  every gate because `cargo doc --workspace --no-deps` never documents private
  modules. Its corrected reproduction also showed that *every* crate in the
  workspace warns under `--document-private-items`, so promoting the
  documentation gate to that flag — the durable guard for the module-header
  prose drift this arc keeps producing — is workspace-scoped work blocked on
  clearing those warnings. That promotion is now recorded as the issue's
  motivating goal.

## Paused autopilot

The standing `/next-milestone` directive (no arguments) is **paused**, not
terminated. State to resume from:

- **Directive scope:** work the tracker on autopilot toward the active
  milestone.
- **Active milestone: v0.3, not met**, but **the gap figure must be
  re-derived, not carried forward.** `docs/ROADMAP.md`'s Accept bullet requires
  37 `PYTHON_STANDARDS.md` matrix rows at `◐`-or-better. Its progress note is
  dated 2026-08-19 and records 29 of 37 with an 8-row gap — but every issue that
  note names as tracking the gap (#572, #578, #579, #580) has since closed,
  verified at the time this entry was committed. So either the count has moved
  and the note is stale, or work closed without moving it. The next iteration's
  `next-milestone` step 2 evidence check must re-count the matrix against the
  tree rather than reuse the 29 figure; do not treat this bullet as the count.
- **Last iteration's outcome:** #544 Part 3 merged; #544 narrowed, not closed.
- **Next step:** re-enter `.claude/skills/issue-select/SKILL.md` at step 1 with
  a fresh baseline from `7db27a7c`.
- **In-run denylist, which must carry forward:** **#20, #631, #604**.

### Known follow-ups not yet actioned

- Narrowing candidates still needing their own cited evidence: #558 (narrow to
  the elapsed-window measurement), #162 (narrow to #397), #44 (re-describe with
  the accurate "downloaded but un-audited" gap).
- P1s never screened this run: #259, #563, #565, #566, #569.
- Stale decomposition-issue titles remaining: #545 says 17,665 (now 7,759),
  #549 says 4,701 (now 4,614). #663's 4,578 is accurate.
- #641's title still names only `macos-15-intel`.
- #162 and #397 both carry `milestone: null`.
- `docs/ROADMAP.md`'s v0.3 conformance-progress note cites four now-closed
  issues as the live trackers of its 8-row gap. Whatever the recount shows, that
  paragraph needs updating so it stops pointing at closed work.
- Other open items: #623, #676, #677, #685; D-171's stale lines 8 and 12; the
  2026-08-01 issue-109 plan document's line 50 / Task 5; the orphaned
  `tests/fixtures/policy-successors/`; `src/project_config.rs:116` citing a
  test gap that does not exist; and a mechanical CI guard over declared
  `closingIssuesReferences`.
