# Session handoff: issue #732 — v0.3's distinct-PEP-count Accept clause was tracked by nothing

- Status: implementation, docs, and two post-review fix rounds are complete
  on branch `issue-732-pep-count-accept-clause`, based on `origin/main` tip
  `ef581ad1`. PR [#750](https://github.com/rotnov/pycc/pull/750) is open;
  both automated-review threads are resolved. CI was still running against
  the final push at the point this entry was written — merge is the
  remaining step, gated on green checks per D-024/D-014.
- What shipped: `scripts/check_conformance_breadth.py` gained
  `ROADMAP_PEP_FIGURES` (parses the progress headline's "encompass N of the
  required M distinct PEP numbers, leaving a G-PEP gap" clause) and
  `ACCEPT_CLAUSE_FIGURES` (parses the milestone's own `**Accept:** conformance
  ≥ N ... matrix rows ... encompassing M distinct PEP numbers` bullet), both
  wired into `check_roadmap_counts` so the headline's PEP totals must agree
  both with the matrix's own `distinct_pep_count` and with the Accept
  clause's stated targets — closing the gap issue #732 named: v0.3's stated
  "39 distinct PEP numbers" target had drifted to a matrix-derived 34 with no
  mechanical check holding the two in sync. `docs/ROADMAP.md`'s v0.3
  progress paragraph gained a dated addendum recording the mechanically
  checked 34-of-39 figure and correcting a stale `#248` citation for the PEP
  681 row (superseded by D-196 and #749).
- Review loop (D-068/D-155 plus two bot-authored rounds):
  - The pinned local `ievo:deep-reviewer` pass ran against the initial
    implementation commit and found no blocking issues; a stale `#248`
    citation was fixed as a small follow-up commit before pushing.
  - After the branch was pushed and PR #750 opened, an automated
    `chatgpt-codex-connector[bot]` review left two threads:
    1. `ACCEPT_CLAUSE_FIGURES` originally used unbounded `.*?` with
       `re.DOTALL`, risking a silent cross-line match into the
       checker-summary sentence two lines below the real Accept bullet
       (which also contains an "encompassing N distinct PEP numbers" phrase,
       but for the *achieved* figure, not the target). Fixed by scoping the
       inter-group gap to `[^\n]*?` (single-line, no `DOTALL`).
    2. Asked whether PEP 681's row could be flipped instead of only
       re-pointing its stale citation. Answered by quoting D-196's own
       "Alternatives" section, which already explicitly considers and
       rejects that: a fixture that never instantiates the
       `@dataclass_transform()`-decorated class "proves nothing about
       `dataclass_transform` beyond 'the decorator parses'" under D-177's
       proven-subset bar. No code or doc change needed.
  - A second, self-initiated review pass (this session, dispatching a fresh
    `ievo:deep-reviewer` specifically against the Finding-1 fix commit, since
    it was a real logic change and not covered by the no-behavior-change
    exception) caught a further robustness gap the bot review did not: the
    single-line scoping closed the cross-line risk but the regex still had
    no uniqueness check analogous to `ROADMAP_HEADLINE`'s `len(headlines) !=
    1` guard, and the match was not provably anchored to v0.3 specifically
    rather than "whichever bullet happens to match this exact phrasing
    first." Fixed by adding `accept_matches = list(ACCEPT_CLAUSE_FIGURES
    .finditer(roadmap))` with a `len(accept_matches) != 1` fail-closed
    check, plus a new test
    (`test_a_duplicated_accept_clause_is_a_failure`) asserting a second
    bullet matching the same phrasing (e.g. a hypothetical future milestone
    reusing this wording) is rejected rather than silently bound to the
    wrong milestone. Verified empirically against the real `docs/ROADMAP.md`
    that v0.2's own Accept bullet (which contains an unrelated "conformance
    ≥ 15 ... encompassing 17 distinct PEP numbers" clause later in its
    prose) does not collide, because the regex requires `**Accept:**` to be
    immediately followed by `conformance ≥` and v0.2's bullet leads with
    different prose.
  - Module and function docstrings were updated in the same round to
    describe the new Accept-clause binding (a doc-drift note the internal
    reviewer also flagged, non-blocking but folded in per this repo's
    "docs updated in the same patch" rule).
- Verification run this session: `python3
  scripts/test_check_conformance_breadth.py` (67 tests, all pass — one new
  test added this round), `python3 scripts/check_conformance_breadth.py`
  against the real file (33 evidence-backed rows, 34 distinct PEP numbers,
  unchanged), `ruby scripts/test_check_roadmap_evidence.rb` and `ruby
  scripts/check_roadmap_evidence.rb` under an explicit UTF-8 locale (both
  pass — the ambient shell's unset `LANG` otherwise trips an unrelated Ruby
  `US-ASCII` encoding error in `check_roadmap_evidence.rb`'s blockquote
  scanner, reproduced and confirmed unrelated to this change before
  discounting it), `python3 scripts/generate_decisions_index.py
  docs/decisions docs/decisions/README.md --check` (up to date).
- Next step for whoever resumes: confirm PR #750's CI finished green,
  re-verify `closingIssuesReferences.totalCount == 1` naming #732 via the
  GraphQL query in `AGENTS.md`'s PR-creation section, re-check
  `reviewThreads` for any new bot thread opened against the latest push (the
  two known threads were resolved before this session's final push, so a
  fresh bot pass on the new head has not yet been observed), then merge with
  a merge commit, delete the branch, and confirm #732 closes.
