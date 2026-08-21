# 2026-08-21-09 — Issue #544 Part 1 merged; autopilot paused

## Repository state (re-resolved immediately before committing this entry)

- `origin/main` = `6a7d1561`
- Open pull requests: **0**
- Branch protection matches the documented baseline: `strict: true`,
  contexts `["audit", "ci-gate"]`, `enforce_admins: true`,
  `required_approving_review_count: 0`, `required_conversation_resolution: true`
- `tests/fixtures/policy-successor-manifest.json`: 49 targets, **0 mid-transition**
- Open issues: 103

## What this checkpoint delivered

**PR #681 — `refactor(types): extract the crate root's test module into tests.rs
(Part 1 of #544)`**, merged as `6a7d1561`.

`crates/pycc_types/src/lib.rs` went from **34,300 to 9,035 lines** (measured with
`wc -l`); its single trailing `#[cfg(test)] mod tests` block became a new sibling
`crates/pycc_types/src/tests.rs` (25,247 lines), following the template commit
`6c785332` set for `pycc_codegen`. Nothing needed visibility widening — a child
module reaches the crate root's private items through `use super::*` — and the
`pub`-bearing line count is unchanged under three different patterns.

The pull request carried three commits and no closing keyword;
`closingIssuesReferences.totalCount` was confirmed `0` before merge, so **#544
stays open** and was narrowed by comment
([#544#issuecomment-5370620819](https://github.com/rotnov/pycc/issues/544#issuecomment-5370620819))
with the residual seams renumbered against the merged tree.

**Issue #682 filed** — the README comparison table's ✅/❌ verdicts are bound by
nothing. `scripts/check-site.sh:2866` strips the glyph before comparing, and four
cells in `site/python-aot-compilers/claims.json` carry an empty expected label, so
the glyph is the whole claim and normalization reduces both sides to `""`.
Reproduced by mutation at `451268d1`: three verdicts flipped — including Codon's
`native_executable`, a factual claim about a third-party project — and both
`scripts/check-site.sh` and `scripts/test-check-site.sh` returned rc=0. The README
was restored with no diff. Its documented deferral target, #202, is closed, so this
was unowned.

## How the selection was made

The `issue-select` adversarial round **changed the pick**. The initial selection was
#545 Part 3; the advisor objected that #544 better satisfies the rule as written,
because "smaller win" is defined as *least effort, smallest blast radius, cleanest
scope* rather than line count. The deciding check was the shape of the file, not the
size of the diff: `pycc_types`' tests were a single trailing block, so the
extraction needed no visibility analysis at all, whereas #545 Part 3's production
seam would have required per-item judgment. The objection was accepted in full.

The advisor also flagged, non-blockingly, that the priority-marker ordering makes the
autopilot loop structurally unable to reach v0.3: every unmilestoned P1 outranks every
unmarked v0.3 issue, and conformance rows only come from the latter. That is a finding
about the skill's ranking rule versus the loop's exit condition. It was **not** acted
on as a scoring override — overriding the written ordering on an advisor's say-so is
the same move that was rejected for #82 — and it is recorded here as a `/harden` or
ADR candidate.

## Review outcome

The pinned reviewer (`ievo:deep-reviewer`, binding checked rc=0) returned nine
findings, all documentation accuracy, none functional. Seven rested on one premise —
that `tests.rs` is excluded from the coverage denominator — which was verified
independently against `docs/TESTING.md` before acting. All eight actionable findings
were fixed in a third commit; the ninth (that the second commit's message overstated
its own completeness) was addressed by saying so in the third commit's message rather
than by rewriting published history.

`AGENTS.md`'s decomposability rule was corrected in the same commit: its illustration
said "several `lib.rs` files here already reach 15-18k lines", which this change made
plainly false. Measured maxima are now `lib.rs` 9,035, with the largest files being
the extracted test modules at 11,645 and 25,247 lines.

## Paused autopilot

- **Directive scope:** project-local `/next-milestone` with no arguments — adopt the
  first `## vX.Y` roadmap section whose **Accept:** bullet is unmet on independently
  verified evidence, hand off to `issue-select`, and loop.
- **Active milestone:** v0.3. **Not met.** The Accept bullet carries no
  `Update (<date>): met.` note; the roadmap's own progress line records 29 of the
  required 37 `◐`-or-better matrix rows. The diagnostics-registry and `pycc explain`
  conjuncts have not been separately re-verified this session.
- **Last iteration's outcome:** #544 Part 1 merged, #544 narrowed and left open.
- **Next step:** re-enter `issue-select` step 1 from `6a7d1561`.
- **In-run denylist that must carry forward: #20, #631, #604.**

## Known follow-ups, not filed

- Narrowing candidates with evidence still to be gathered: #558 (narrow to the
  elapsed-window measurement), #162 (narrow to #397), #44 (re-describe as
  "downloaded but un-audited").
- Stale decomposition-issue titles, outside the autopilot's authorized write set:
  #544 says 31,673 (measured 34,300 before this change, 9,035 after), #549 says 4,701
  (measured 4,614). #545's title was 17,665 and its file is now 7,759.
- #641's title still names only `macos-15-intel`; that platform passed twice more on
  this pull request's two heads.
- `.claude/skills/issue-implement/SKILL.md` step 4 still describes D-103's retired
  exact-byte gate as live — a `/harden` candidate.
- Other carried items: #623, D-171's stale lines, the orphaned
  `tests/fixtures/policy-successors/`, and a mechanical CI guard over declared
  `closingIssuesReferences`.

## Where to resume

List `docs/sessions/` sorted by filename and read the last few entries; this is the
newest. The tree at `6a7d1561` is clean and has no open pull requests.
