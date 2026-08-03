# 2026-07-27 — PR #157 integrates new live `main` events

**Advanced monitoring checkpoint:** while draft
[PR #157](https://github.com/rotnov/pycc/pull/157) was completing review, the
refreshed default branch advanced first to
`6f541c5974930d4a6271092f6797439e043915ed`, the merge of
[PR #158](https://github.com/rotnov/pycc/pull/158) at source head
`b153d4dd41c57c494a99d0f76fb68bcc7eeeab2e`, and then to
`3a5662c180ac1c6c7028331f8323f73a7d365ce8`, the merge of
[PR #159](https://github.com/rotnov/pycc/pull/159) at source head
`3e3a9623b53d7e9ee2f7403d1887457763adf8c2`. Both introduced ranges were
inspected: PR #158 supplies the 12-file iEvo lifecycle hardening and PR #159
stages D-080's five-file conformance-oracle workflow fixture and trust-anchor
evidence without activating it. PR #158's exact-merge workflows all passed.
For PR #159's exact merge, `Agent assets`, `Agent policy`, and `Main history
audit` passed. Its
[exact-merge `CI`](https://github.com/rotnov/pycc/actions/runs/30255055309)
also completed successfully, including hard 100% line/region coverage, the
complete Tier-1 matrix, cross compilation, frontend performance measurement
and gate, and aggregate `ci-gate`. The verified current default-branch
checkpoint is therefore
`3a5662c180ac1c6c7028331f8323f73a7d365ce8`.

**Remote PR inventory at the refreshed checkpoint:** the complete open set,
with the baseline fields required by D-078, is #36 (`OPEN`, draft,
`8d1f7a252c75d7c6858bef00ab7e07b48422a361`), #59 (`OPEN`, draft,
`e9a0e3828e25cb7695bd180234083948a98385ab`), #91 (`OPEN`, draft,
`c4833fd9b03538d7eab885d7447576e7037d5be5`), #92 (`OPEN`, draft,
`2cd67390b8f8903e2cd01b32e6056438d27ccdd5`), #112 (`OPEN`, ready,
`6f4f4f50db9878bf39e8f2043c14e1c631df5de6`), #153 (`OPEN`, ready,
`74a355f86613da346c10ba83cc62d521eb984679`), and #157 (`OPEN`, draft and
task-active, `6e951e2563d1cd05850c0564c79ac975d4780de7`). PR #159's merge was
evaluated once and it is no longer in the live PR set. Inventory membership
alone does not make a historical open PR live; an eligible field transition
relative to this baseline does. The reviewed published PR #157 head was
`MERGEABLE` with `mergeStateStatus=BLOCKED` against exact base `3a5662c`.
Both local Git and the GitHub commit API show `6e951e2` has parent `9bcf0ae`,
whose parents are `2efdfbd` and `3a5662c`; the review claim that those
integration commits were not ancestors was factually incorrect. The previous
three Codex threads are resolved. The `6e951e2` review added two unresolved P2
threads for the
[handoff snapshot](https://github.com/rotnov/pycc/pull/157#discussion_r3656142150)
and [active-section validation](https://github.com/rotnov/pycc/pull/157#discussion_r3656142157).
Its required-check baseline had `audit: SUCCESS`; hard coverage also passed,
but the aggregate `ci-gate` had not completed when the actionable review made
that head ineligible to authorize merge. Its remaining checks will not be used
as evidence for the forthcoming repair head.

**Integrated scope:** the containing merges preserve PR #158's D-081 lifecycle
hardening and PR #159's staged D-080 artifacts while adding PR #157's D-078
event-driven monitoring contract. Canonical `AGENTS.md` now limits the live set
to eligible post-checkpoint default-branch and pull-request field transitions;
`docs/REPOSITORY_GOVERNANCE.md`, the ADR map, roadmap, and fail-closed
agent-asset validator agree. Claude Code receives the same rule through the
exact `CLAUDE.md` import. D-054's issue #125 and PR #119 remain historical
evidence only and must not become recurring polling targets.

**Superseded and current evidence:** PR #157's previous exact head
`f5bd5d49bc46b5459f42689b98a2516850bdbfcd` passed every required job,
including hard coverage and `ci-gate`, and received a clean user-requested
GitHub Codex review with no inline comments or unresolved threads. Those checks
do not authorize the integration head after `main` advanced. The integrated
repair passes all 298 Python discovery tests (four platform-only skips),
including a warnings-as-errors run, both agent validators, Ruff, 100
roadmap-policy tests with 434 assertions,
roadmap evidence, `cargo fmt`, workspace build and all 581 Rust tests, clippy
with warnings denied, fresh Rust API documentation, and `git diff --check`.

**Current review repair and remaining gates:** the exact-head Codex review of
`2efdfbd` found three actionable P2 threads; `6e951e2` fixed them, closed the
mocked `HTTPError` that caused Python 3.14 warning leakage, and integrated
`main` through `9bcf0ae`. The exact-head review of `6e951e2` then found the two
threads recorded above. The handoff finding's ancestry premise was wrong, but
the published-head baseline still needed this refresh. The validator finding
was correct: raw substring matching allowed retired rules inside Markdown
fences or HTML comments to satisfy CI. The containing follow-up requires exact,
unindented list items in exactly one active level-two monitoring section; plain
prose, fenced, commented, indented, blockquoted, nested-container, duplicate,
and out-of-section copies cannot satisfy it. Fence-before-comment recognition
and state, leading HTML-comment blocks, CommonMark tab stops, and invalid
backtick-info handling prevent the three additional review reproductions. The
final pinned pass also found list-container close indentation, list-indented
code, and escaped or inline-code comment tokens; its next pass found raw HTML,
list-container termination, inline-comment block boundaries, and renamed-heading
cases. Exact regressions cover each repair, including quoted type-7 attributes
and peer boundaries without a blank. Tab-separated thematic breaks and
non-interrupting list-like paragraph lines have exact regressions as well.
List-contained HTML comments and Unicode whitespace block terminators have exact
regressions as well; Setext and empty ATX H1/H2 headings terminate the active
section without misclassifying link-reference definitions. The final
container-state regressions distinguish lazy list and blockquote paragraphs,
indented paragraph continuations, and five-space list code from inline comments.
Completed fenced and raw-HTML blocks clear stale lazy-container state, while fences
opened on list continuation lines retain each active list indentation boundary.
Thematic breaks take precedence over otherwise list-like marker sequences.
Reference-definition regressions cover escaped and multiline labels, the raw
999-character limit, balanced destinations through CommonMark's 32-level parenthesis
limit, rejection at level 33, ASCII-control rejection in bare destinations,
line-ending rejection in angle-bracket destinations, multiline titles, and
fail-closed invalidation or end-of-file state. Negative regressions cover every
bypass class.
The code-bearing repair commit `3d3c985d4050b32e210355c78c42778437acdfa5`
was published only after pinned deep review returned zero findings across all 11
points; its additional adversarial comparison covered 418 generated Markdown cases
against cmark without a false acceptance. After publishing this handoff refresh,
request exactly one new `@codex review` for its exact head; keep the PR draft until
hard 100% coverage and every other required check pass, resolve all actionable
threads, re-confirm fresh `main`, and merge only through branch protection.
