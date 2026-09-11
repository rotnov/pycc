# 2026-09-11 (03) — #1006 (Part 1 of #566): Status hero as a commit-bound required-check snapshot

Status: draft; to be delivered by the pull request that carries this file;
base `origin/main` `4111208c` (re-fetched before the final commit; unchanged
throughout the session). Branch `feat/issue-1006-status-hero`.

## What changed

The Status page's evidence hero was the last first-screen claim about the
project's CI state and was still explicitly `unavailable` under D-186. It is
now a checked-in, sanitized snapshot of the required checks for exactly one
default-branch revision, refreshed offline by an agent and validated by the
same hermetic Pages gate as the other heroes (D-241). Nothing at build or
deploy time contacts GitHub.

- Contract and validator: `scripts/site_status_evidence.py` (`expected_shape`,
  `derive_state`, `validate`, `validate_projection`, `summary`) beside the
  execution module; `scripts/check_status_snapshot.py` CLI (`--verify-git`
  for the Pages full-history checkout, `--currency` for the pull-request leg,
  structural-only otherwise); `scripts/test_check_status_snapshot.py`
  (shallow-safe, synthetic `git init` repositories, stubbed collector shapes)
  discovered by the `governance` job. `scripts/check_site_evidence.py`
  dispatches the non-`unavailable` `status` record to the new module and the
  schema is `2.1.0`.
- Collector: `scripts/collect_status_snapshot.py` over read-only `gh api`
  (commits, pulls, paginated check-runs); unknown-not-green; rewrites only the
  `status` record; `scripts/test_collect_status_snapshot.py` stubs `gh`.
- Record: `site/evidence-heroes.json` `status` is `all-Tier-1` for main
  `4111208c0910c0a7588408a6ac7d2b2bad20592c` (PR #1005, head `984eb6f2…`,
  same tree, one parent): `ci-gate` success (run `34552229293`, completed
  2026-09-11T02:10:46Z), `audit` success on the head (completed
  2026-09-11T01:50:30Z), five Tier-1 jobs success; captured
  2026-09-11T12:35:04Z. The record pins the collector's and its suite's
  canonical SHA-256, so it was re-collected with
  `--collected-at 2026-09-11T12:35:04Z` after the suite changed.
- Gate wiring: `scripts/check-site.sh` runs `check_status_snapshot.py
  --verify-git` after `check_site_evidence.py`; `scripts/test-check-site.sh`
  stages the two pinned scripts, retargets every status-as-`unavailable`
  anchor, adds `status_mutation()` (commit/tree/parent-count co-mutations),
  and runs the new Pages-only `scripts/site_status_evidence_test.py`;
  `scripts/site_execution_evidence_test.py` tolerates a snapshot without
  `artifacts`; `.github/workflows/pages.yml` lists the five new inputs on
  both events and runs the currency step on the pull-request leg only;
  `scripts/test_site_status_wiring.py` binds all of it.
- Page and projections: `site/status/index.html` (`en-US`, `all-Tier-1`,
  one `page-meta` summary line, a `<details class="hero-evidence-details">`
  with the three subject rows and five job rows, the milestone line and the
  limitations) trimmed to fit the 25,600-byte budget; `site/index.html.md` and
  `site/llms.txt` carry the one-line `summary(hero)`; the Markdown landing's
  context budget rose 13312 -> 13824 bytes (aggregate 278016 of 278528);
  sitemap `lastmod`, JSON-LD `dateModified`, `check-site.sh` `date_modified`
  and the performance-manifest SHA were rotated.
- Documentation: D-241 (narrowly supersedes D-230's "five unavailable heroes"
  consequence), decisions index, `docs/WEBSITE.md` (schema `2.1.0`, the
  status record shape, the gate/convention table, the four remaining
  `unavailable` heroes, the context-budget allocation sentence),
  `docs/ROADMAP.md`, `docs/SPEC.md`.

## Forks resolved (D-127)

- Nested-field-required mirror would not reject deleting one `test.names`
  entry, because the validator only checked that each listed name exists.
  Resolved by tightening the validator to require the full ordered `def
  test_*` list (unit-tested, record re-collected) rather than weakening the
  mirror test.
- `site_execution_evidence_test.py`/`site_status_evidence_test.py` stage
  `snapshot.artifacts` for every non-null-fixture hero; the landing snapshot
  is a single `path` and the status snapshot holds subjects. Resolved by a
  narrow guard on the artifact line only (plan item 6a), so the landing hero
  is still staged.
- The 25,600-byte page budget (29,069 bytes after the proof rows) was met by
  four rounds of prose trims that keep all 18 `PAGE_SPECS` literals, the
  current-scope literals and the disclosure phrases, rather than by a budget
  raise (the plan's stated precedent).

## Gates at the delivering tree (all exit 0; only this file changed after the run)

`bash scripts/check-site.sh` ("Website checks passed."); `bash
scripts/test-check-site.sh` ("Website validator self-tests passed.");
`ruby scripts/check_pages_performance_budget.rb --skip-lighthouse`
(`site/status/index.html` 25,532 of 25,600 bytes); the `scripts/` unittest
suite (1199 tests, OK, 6 skipped); `scripts/site_status_evidence_test.py`
(10 tests, 111 nested-field subtests, OK); `check_site_evidence.py`;
`check_status_snapshot.py --verify-git` and `--currency --base origin/main
--head HEAD` (0 first-parent merges behind); `check_ci_permissions.rb`
(9 files); `check_roadmap_evidence.rb` and its 247-run suite;
`check_status_page_freshness.rb origin/main HEAD`; `validate_agent_policies.py`;
`validate_agent_assets.py`; `generate_decisions_index.py --check`;
`check_decision_immutability.py`; `classify_ci_changes.py --event-name
pull_request` (compiler=true, pages=true, agent=true; no Rust touched, so
D-014 is unaffected); `check_site_pin_merge_currency.rb`; the execution and
status wiring suites (17 tests). `site/index.html.md` is 13,760 of 13,824
bytes; the context aggregate is 278,016 of 278,528.

## Review rounds

Five D-068 `ievo:deep-reviewer` rounds over the full merge-base range; every
verdict is a row in `.harden/findings/issue-1006.jsonl`.

1. One warning: `paginated_check_runs` accepted a non-dict check-run entry
   and died with `AttributeError` instead of the collector's `Unavailable`
   path. Fixed in `c3f929a5` (guard plus test; snapshot re-collected because
   the record pins the collector's bytes).
2. One warning: the `parent_count` invariant used `!= 1`, which `True` and
   `1.0` satisfy. One note: an unused `verify_git` flag on the test helper.
   Fixed in `8e5adcf5` (int-not-bool check with a subTest over `True`, `1.0`,
   `"1"`; flag removed; snapshot re-collected).
3. One warning: `scripts/test_site_status_wiring.py` imported a `TestCase`
   by name, so `unittest discover` ran its six tests twice. Fixed in
   `48d9b813` (qualified module import; 1182 collected became 1176).
4. One warning: `validate()` compared `attestation.milestone_line` with
   `docs/ROADMAP.md` at `HEAD` while the subject must be a `main` commit, so a
   milestone-transition pull request could never re-collect a green record.
   The plan prescribed the `HEAD` comparison; an independent advisor round
   (D-127) chose to prove the line against the subject commit in
   `verify_git`, exactly as the subject's tree is proved, over hand-edited
   records or an accepted red leg, and D-241 records the bounded window in
   which the hero shows the previous milestone. One note: the pin prose
   called the validator suite the collector's suite. Fixed in `c2d8cf79`
   (snapshot re-collected at `2026-09-11T04:38:32Z`).
5. Clean.
6. External, on PR #1008: a Codex P2 — the RFC 3339 predicate was a shape
   regex, so `2026-99-99T99:99:99Z` passed both the collector's `--collected-at`
   guard and the validator. Fixed: `site_status_evidence.is_utc_instant` parses
   the instant after the shape check; the collector's `completed_at` and
   `--collected-at` guards use it; impossible-instant cases added to both
   suites. The pinned files changed, so the snapshot was re-collected at
   `2026-09-11T05:19:28Z` and the pins rotated.
7. External, on PR #1008, two more Codex P2s: platform rows could share a
   `job_url` (only the enclosing run id was checked), and a valid but earlier
   `--collected-at` could claim capture before completion. Fixed in the
   validator: the five platform job links plus the `ci-gate` job must be six
   distinct URLs, and `collected_at` must be no earlier than either recorded
   `completed_at`; cases in both suites, `docs/WEBSITE.md` gate row updated,
   snapshot re-collected at `2026-09-11T05:40:28Z`, pins rotated.
8. External, on PR #1008, a third Codex P2: the audit subject could reuse
   the ci-gate run and job. Fixed: the audit `run_id` must differ from the
   ci-gate `run_id`, and the audit job joins the distinct-job set (seven
   URLs); cases added, gate row updated, snapshot re-collected at
   `2026-09-11T06:00:33Z`, pins rotated.
9. External, on PR #1008, a fourth Codex P2: the projection check searched
   one flattened hero string and compared links as an unordered set, so
   swapping the `ci-gate` and `audit` run links, shas or completion times
   between the two visible rows still passed. Fixed: `ProofRowParser` keeps
   each visible `<dt>`/`<dd>`/`<li>` row inside the hero with its own text
   and links, and the validator binds every subject's full tuple (sha,
   check, App id, conclusion, completion time, run and job links) to the one
   row carrying its label, and every Tier-1 platform's runner, target and
   conclusion to the one row linking its job; swap, drop, duplicate, hidden
   and out-of-hero cases in the synthetic suite, swap cases in the public-CLI
   suite, gate row updated, snapshot re-collected at `2026-09-11T06:25:14Z`,
   pins rotated.
10. External, on PR #1008, three more Codex P2s: a row could carry a
    contradictory value beside the expected token (`failure (recorded
    success)`) because the row check was substring presence; a stylesheet
    rule such as `.hero-evidence-details { display: none }` could hide the
    proof outside the parser's HTML-only visibility model; and a valid but
    future `--collected-at` was accepted. Fixed: each subject and platform
    row must equal its exact normalised text and link list
    (`expected_rows`, `platform_row_text`); `hiding_rules` rejects any
    `site/styles.css` rule setting `display: none`/`visibility: hidden`
    whose subject compound can match a hero element through hero or
    hero-ancestor compounds; `collected_at` must not be later than the
    validating clock and the collector refuses a future override. Cases in
    the synthetic, collector and public-CLI suites, gate row updated,
    snapshot re-collected at `2026-09-11T06:48:31Z`, pins rotated.
11. External, on PR #1008, three more Codex P2s: a successful `audit`
    rerun that completed after the pull request merged would have been
    published as the pre-merge audit because `merged_pull_request()`
    discarded `merged_at`; the collapsed `page-meta` summary (`ci-gate
    success · audit success (PR #1005)`) was not checked at all; and
    `HIDING_RULE` matched `display: none` case-sensitively although CSS
    declarations are not. Fixed: the record now carries
    `merged_pull_request.merged_at` (a provider timestamp of the same class
    as `completed_at`; the sanitization note lists it), the collector refuses
    an `audit` completed after the merge or a `ci-gate` completed before it,
    and the validator enforces both orderings offline; `ProofRowParser`
    keeps the visible element repeating the hero's `data-evidence-id` as a
    `summary` row that must equal `expected_summary_line` exactly and appear
    exactly once; `HIDING_RULE` is case-insensitive. Cases in the synthetic,
    collector and public-CLI suites, gate row and record description
    updated, snapshot re-collected at `2026-09-11T07:16:10Z`, pins rotated.
12. External, on PR #1008, one Codex P2: an inline `style="DISPLAY: NONE"`
    on a proof container or row still counted as visible because the
    inherited `VisibleExecutionParser` matched inline declarations
    case-sensitively. Fixed: one shared case-insensitive
    `site_execution_evidence.HIDING_DECLARATION` serves both the inline
    visibility model and the stylesheet scan; upper- and mixed-case inline
    cases in the execution, synthetic and public-CLI suites, snapshot
    re-collected at `2026-09-11T07:43:45Z`, pins rotated.
13. External, on PR #1008, two Codex P2s: `opacity: 0` (and other
    visibility-removing declarations) bypassed the stylesheet scan, which
    knew only `display` and `visibility`; and a surplus labelled row such as
    `<dt>Current gate result</dt><dd>ci-gate failure</dd>` was accepted
    because only the expected labels were checked. Fixed: the shared
    `HIDING_DECLARATION` enumerates `display: none`, `visibility:
    hidden|collapse`, `opacity: 0`, `content-visibility: hidden`,
    `font-size: 0` and a zero `transform` scale (off-screen positioning and
    overlays stay outside the model, recorded in `docs/WEBSITE.md`); the
    labelled rows must be exactly the three subject rows plus the `Tier-1
    jobs` heading row, itself matched exactly. Cases in the execution,
    synthetic and public-CLI suites, snapshot re-collected at
    `2026-09-11T08:02:33Z`, pins rotated.
14. External, on PR #1008, four Codex P2s: `verify_git` and `--currency`
    accepted any ancestor, so a one-parent commit merged through a merge
    commit's second parent passed as the published revision; the stylesheet
    scan compared the subject compound with hero hooks only, so
    `.content-page { display: none }` on a hero ancestor was invisible to it;
    the zero pattern missed `.0`/`.00` spellings; and unlabelled hero prose
    (`<p>Current gate result: ci-gate failure</p>`) was checked only for the
    required literals. Fixed: `on_first_parent_history` (`git rev-list
    --first-parent`) replaces `merge-base --is-ancestor` in both checks;
    every compound is matched against hero and ancestor hooks; `ZERO` accepts
    leading-dot zeros; and the hero's visible text outside the proof rows is
    closed — the parser collects every text block, and each must equal one of
    the reviewed masthead lines, the `<details>` toggle or the record-built
    closing paragraph, each at most once. The design fork (a vocabulary filter
    on free prose versus a closed enumeration) was resolved for the closed
    enumeration: a filter is a model with a boundary that each further review
    round chips at, while the masthead is reviewed text pinned exactly as
    `LIMITATIONS` already is. Cases in the execution, synthetic (a `--no-ff`
    merge fixture) and public-CLI suites, snapshot re-collected at
    `2026-09-11T08:32:49Z`, pins rotated.
15. External, on PR #1008, three Codex P2s: a `<dt>` whose row is hidden or
    missing, or two consecutive `<dt>`s, never entered the labelled set, so a
    dangling visible label such as `<dt>Current gate result: ci-gate
    failure</dt>` passed; the page's own `<style>` elements were a third
    hiding source the scan never read (the parser marks `style` hidden and
    drops its text); and the closed prose enumeration still let the
    `<details>` toggle or the closing paragraph disappear. Fixed: a pending
    label on the next `<dt>` or at the end of the list fails; the parser keeps
    every `<style>` body and every `<link rel="stylesheet">` href, the hiding
    scan runs over `site/styles.css` plus the embedded CSS (any `@import` is a
    hit), and a stylesheet link other than `site/styles.css` fails; the prose
    check requires the toggle and the closing paragraph exactly once (the
    masthead stays optional). Cases in the synthetic and public-CLI suites,
    snapshot re-collected at `2026-09-11T09:17:02Z` with `--subject 4111208c`:
    the branch merged `origin/main` (`747a5677`) through a second parent, so
    the newer tip is not on the branch's own first-parent history and the
    round-14 check rejects it locally (the pull-request merge ref CI checks out
    has the base tip as its first parent); `docs/WEBSITE.md` records the
    `--subject` route. Pins rotated.
16. External, on PR #1008, three Codex P2s: only the toggle and the closing
    paragraph were mandatory, so the H1, lede, milestone, acceptance,
    readiness or eyebrow could vanish; the eyebrow date was shape-checked
    only (`Updated 9999-99-99` passed beside a `2026-09-11` JSON-LD
    `dateModified`); and the stylesheet scan looked for hiding declarations
    only, so `.page-meta span:first-child::after { content: " · ci-gate
    failure" }` rendered contradicting text the HTML parser never saw. Fixed:
    every reviewed block is required exactly once; the page must declare one
    JSON-LD `dateModified` and the eyebrow is bound to it; a `content`
    declaration rendering text (anything but `none`/`normal`/empty) on a
    hero-reachable selector is rejected with the hiding rules (the
    `justify-content` family is excluded by the word guard; `site/styles.css`'s
    own `content` rules are empty or off the hero). Cases in the synthetic and
    public-CLI suites, snapshot re-collected at `2026-09-11T10:03:39Z` with
    `--subject 4111208c` (now two first-parent merges behind `origin/main`
    after merging #1009), pins rotated.
17. External, on PR #1008, three Codex P2s: `opacity: -0` and `opacity: 0e0`
    (signed and exponent-form zeros, which compute to zero) escaped the
    enumerated zero spellings; an `inert` attribute removed a proof row or the
    disclosure from view without being treated as hiding; and D-241's own text
    said "ancestor" where the gate checks first-parent history. Fixed: the
    zero pattern accepts an optional sign and exponent; `inert` joins
    `hidden`/`aria-hidden="true"` in the subtree-hiding attributes; D-241
    reads "on the first-parent history". The same thread's escaped-identifier
    case (`d\69 splay: none`) is refuted and recorded as a model boundary in
    `docs/WEBSITE.md`: the scanned CSS is the repository's own reviewed
    stylesheet, where an escaped property name is deliberate obfuscation, not
    the accidental hiding the scan exists to catch. Cases in the execution,
    synthetic and public-CLI suites, snapshot re-collected at `2026-09-11T10:59:43Z`
    with `--subject 4111208c`, pins rotated.
18. External, on PR #1008, two Codex P2s: a `<dialog>` without `open` is
    hidden by user-agent styling but was not in the hidden-element set, and a
    repeated attribute (`<a href="wrong" href="expected">`) is read first-wins
    by browsers but last-wins by the `dict(attrs)` collapse, so a visible job
    link could navigate elsewhere. Fixed in the shared visibility parser: a
    closed `dialog` hides its subtree, and any element repeating an attribute
    is rejected before the collapse (no page in `site/` repeats one). Cases in
    all three suites, snapshot re-collected at `2026-09-11T11:25:30Z` with
    `--subject 4111208c`, pins rotated.
19. External, on PR #1008, two Codex P2s: the `transform` match read only
    the first `scale` argument, so `scale(1, 0)` collapsed the hero unseen;
    and selector attribute names kept their casing while `HTMLParser`
    lowercases them, so `[DATA-EVIDENCE-ROLE="hero"] { display: none }`
    hid the hero unseen. Fixed: any zero argument of `scale`/`scaleX`/
    `scaleY`/`scaleZ`/`scale3d` counts, and attribute-name hooks are
    lower-cased on the selector side. Cases in all three suites, snapshot
    re-collected at `2026-09-11T11:54:15Z` with `--subject 4111208c`, pins rotated.
20. External, on PR #1008, one Codex P2: the collector bound `ci-gate` and
    `audit` by check-run name and App 15368 only, and App 15368 is GitHub
    Actions as a whole, so a job named `audit` from any other workflow
    (including a pull request's own YAML under `pull_request`) would have
    been published as the D-172 audit. Fixed: the collector reads the
    workflow run behind each check's job URL (`actions/runs/<id>`, one
    read per run) and requires the closed `EXPECTED_WORKFLOWS` binding --
    `ci.yml` under `push` for `ci-gate`, `workflow-policy.yml` under
    `pull_request_target` for `audit` -- on the observed commit; the record
    gains `workflow_path`/`event` per subject (null on the revision row) and
    the validator enforces the same table. Cases in all three suites,
    snapshot re-collected at `2026-09-11T12:35:04Z` with `--subject 4111208c`, pins rotated.

Harden batch over the pile: four classes, all recorded as open counters
under `.harden/incidents/` in this pull request — `new-case-misses-branching-sites`
(rows 1-2, review is the right rung), `plan-contradicts-its-own-constraint`
(rows 5-6, file 4; the step-7 reviewer-brief line is routed to the agent-tooling
umbrella #806), `test-seam-widens-public-api` (row 3, second file) and the new
`testcase-import-rediscovered-by-unittest` (row 4, fails the D-192 filing bar).

## Follow-ups

- #1007 (Part 2 of #566): the Architecture hero. #566 stays open.
- #408: `status-page-freshness` is still not a required context; the new
  currency step is deliberately Pages-only and not a merge gate either.
- The #802 checklist tick for the compiler-driver card (`init`/`explain` now
  listed as implemented on the Status page) is a `gh` write left for the
  orchestrating session.

## Where to resume

`git log --oneline -1 origin/main`; `python3 scripts/collect_status_snapshot.py`
against the current tip if `site/status/index.html` or the `status` record is
edited again; open issues in milestone v0.4.
