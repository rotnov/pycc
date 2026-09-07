# 2026-09-07 (04) — issue #682: binding the README comparison table's verdicts

## Overall status

Issue #682 is implemented on the task branch `fix/issue-682`, rebased onto
`origin/main` at `aa43fd30` ("fix(pycc_types, pycc_mir): resolve an inherited
class attribute through the class name (#974) (#992)"). Two commits — `8af3c690`
(the fix) and `6639305a` (a reviewer finding) — are pushed as pull request
[#994](https://github.com/rotnov/pycc/pull/994). Nothing is merged as this
snapshot is written; CI is still running.

The issue was assigned to milestone **v0.4** when the work started; the
planning run deliberately made no tracker mutation, so that assignment was the
orchestrating session's to make.

## Correcting the previous checkpoint's open state

The `2026-09-07-03` snapshot describes pull request #993 as open. It has since
merged as `ebdedd1d`, which is now an ancestor of `origin/main`, and issue #990
is closed. D-130 forbids editing a prior session's file, so the correction is
recorded here instead.

## What the change does

`scripts/check-site.sh` carries a `readme_projection` binding that is supposed
to hold the README's compiler comparison table to the model in
`site/python-aot-compilers/claims.json`. It was fail-open in five ways, three
of them wider than the issue itself reported. Each was measured on the pre-fix
tree rather than inferred:

- **All 24 verdict glyphs were unbound.** Flipping `| Codon | ✅ static
  language |` to `❌ static language` exited 0.
- **`mypy` was skipped outright** by an `if entity_name == "mypy": continue`.
- **A permissive branch let `pyright` vanish** from the README without failing.
- **The per-cell loop was driven by the README's own entity names** rather than
  the model's `row_order`, so an entity dropped from the README was never
  checked at all. Replacing the combined row with `| mypy | ❌ | ❌ checker
  only | ❌ |` exited 0 — and that mutation survives a pure `{verdict, text}`
  schema change, which is why the loop driver had to be part of the fix rather
  than a follow-up.
- **The row parser dropped empty cells wholesale** and then applied
  `if len(cells) < 4: continue`, a silent-skip hole.

Landed in this change:

- `site/python-aot-compilers/claims.json` — all 24 `readme_projection.labels`
  cells converted from bare strings to `{verdict, text}` objects. Six were
  empty strings on `main`; the issue named four, omitting `pyright`'s two.
- `scripts/check-site.sh` — the binding rewritten: two-way
  `set(labels) == set(row_order)`, an exact row arity check as a hard error,
  ordered README-exhaustiveness over a list, the per-cell loop driven off
  `row_order`, a glyph parser matching `⚠️` as `U+26A0(U+FE0F)?`, backtick
  stripping confined to the remainder text, `column_sources` bound, and both
  dead fail-open branches plus the `mypy`-skip wrapper deleted. The row
  parser's own `mypy / pyright` fan-out guard is kept — the two occurrences of
  `entity_name not in labels` are not the same check.
- `scripts/test-check-site.sh` — 17 new mutation controls plus one existing
  control updated to the object cell shape.
- `docs/WEBSITE.md` — a `readme_projection` contract paragraph.

No new decision entry: zero of 235 `docs/decisions/D-*.md` mention
`claims.json`. `docs/ROADMAP.md` was deliberately left untouched — it stands at
168947 of its 168960-byte budget, 13 bytes of headroom.

## The finding that should outlive this issue

**No required check exercises this change.** `check-site.sh` and
`test-check-site.sh` run only in `pages.yml`'s `Pages / build` job, which is
not one of branch protection's two required contexts (`audit`, `ci-gate`);
`pages-performance` and `pages-accessibility` run Lighthouse only. So a red
mutation suite still yields a green `ci-gate`, and merging on `ci-gate` alone
would merge an unproven change. The acceptance proof for this pull request is
therefore local evidence plus the `Pages / build` result read directly, not the
required-context rollup.

Making the site checkers gating was considered and deliberately left out of
scope: it is a branch-protection change, which is maintainer authority under
D-024 and outside what an agent session may do.

## Acceptance evidence

Every gate captured as `cmd > log 2>&1; echo $?` — a pipeline destroys the exit
status. `RUBYOPT="-E UTF-8"` is required locally for `scripts/check-site.sh`
(a pre-existing machine-local US-ASCII locale issue, not a property of the
change).

| Gate | Exit |
| --- | --- |
| `bash scripts/check-site.sh` | 0 |
| `bash scripts/test-check-site.sh` | 0 (24 tests, `OK`) |
| `python3 -B -m unittest discover -s scripts -p 'test_*.py'` | 1 |

The unittest failure is `test_check_harden_findings.test_real_repository_piles_conform`,
pre-existing and machine-local: `/Users/denis/projects/pycc-proto/.git/info/exclude`
carries a `.harden/` line that hides untracked findings files. This diff
touches no `.harden` file. The coverage gate is not selected for this change
(`compiler=false, pages=true, agent=false`).

Both mutations that measurably passed before the fix now fail:

| Mutation | Before | After |
| --- | --- | --- |
| Glyph-only flip of Codon `type_enforcement` | 0 | 1 |
| `pyright` dropped from the combined `mypy / pyright` row | 0 | 1 |

## Review

The local pinned reviewer (`ievo@ievo-skills` 0.78.8, binding confirmed by
`scripts/check_claude_reviewer_binding.py`) returned three `note`-severity
findings and no P0/P1. One was actionable and became commit `6639305a`: the
rewritten row parser dereferenced cell zero before applying its arity check, so
a degenerate line carrying no closing pipe raised an uncaught `IndexError`
instead of a diagnostic. It failed closed, but with a traceback — the opposite
of what this change exists to produce.

That fix carries a mutation control asserting the diagnostic text rather than
merely a non-zero exit, because a control checking only the exit status would
have passed against the unfixed code too. The control was negative-tested: with
the guard removed in place, the run emits an `IndexError` and no `has no cells`
message, so the control genuinely detects the regression. `scripts/check-site.sh`
was then restored and verified byte-identical by SHA-256.

The two findings left as-is: a `row_order` duplicate check that is redundant
with the downstream ordered-equality check (fail-closed either way), and
`readme_projection.column_sources` being a self-validating identity map, which
`docs/WEBSITE.md` already documents as such.

## Planning note

The delegated `issue-to-plan` run (D-143) reported "Review rounds: 5 (stop
condition reached)". Read literally that is `issue-implement`'s per-issue stop
condition. It was resolved against the independent advisor rather than the
repository owner, per D-127, and the resolution was that the stop condition did
not fire in substance: round 5 raised no reviewer finding at all — it
discovered that three round-4 edits had never reached disk because a script
aborted before its `write_text`, and re-applied them. That is repair of a
tooling failure inside round 4, not a review round's own output. The loop
converged at round 4 with no open disagreements, and the published plan resolves
every fork explicitly. #682 was therefore not denylisted.

## Follow-ups this change deliberately did not take

- Extending `scripts/check_source_links_registry.rb` to cover the URLs in
  `claims.json`'s `readme_entities` block. That block has **zero readers**
  anywhere in the tree — worth an issue of its own, including the question of
  whether it should exist at all.
- Rewriting `docs/WEBSITE.md`'s two stale "#202's domain" deferrals.
- Making `check-site.sh` a required CI context (see above — maintainer
  authority).

## Paused autopilot

- **Directive scope:** open-ended — `/goal fix all opened issues`, currently
  scoped to milestone **v0.4**. The loop does not stop when v0.4 completes.
- **Active milestone:** v0.4.
- **Last iteration's outcome:** #682 implemented, pull request #994 opened and
  awaiting CI.
- **Next autopilot step:** land #994 (watch `Pages / build` specifically, not
  just the required contexts), then re-enter `issue-select` step 1 with a fresh
  baseline.
- **In-run denylist:** empty. No issue reached a per-issue stop condition this
  run.

## Where a fresh session should look

- The published plan: <https://github.com/rotnov/pycc/issues/682#issuecomment-5573130115>
- The pull request: <https://github.com/rotnov/pycc/pull/994>
- The rewritten binding: `scripts/check-site.sh`, under the "README comparison
  table binding (Part 2 of #162)" banner.
