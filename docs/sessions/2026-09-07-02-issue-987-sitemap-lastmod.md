# Session snapshot — 2026-09-07 (02): issue #987, the `/status/` sitemap lastmod

Autopilot iteration 28 under the standing D-127 directive ("fix all opened issues", milestone v0.4).

## Repository state at this checkpoint

- Base commit: `origin/main` = `74729513` — squash of PR #988, closing issue [#979](https://github.com/rotnov/pycc/issues/979) (dunder/sunder/name-mangled assignments in an `Enum` body).
- Previous merge: PR #985 = `3ba4a027`, closing issue [#977](https://github.com/rotnov/pycc/issues/977) (unrenderable instance and protocol string conversion, `C0001`).
- Work branch: `autopilot/iter-2026-09-07-28`, based on `74729513`. No other pull request was open when this iteration started.
- `python3 scripts/manage_ci_bypass.py status` reports branch protection **matches the documented baseline** (`strict`, contexts `audit` + `ci-gate`, admins enforced, zero required approving reviews, conversation resolution required). No `[ci-bypass]` incident is open.

### Post-merge workflow runs on `74729513`

| Workflow | Run | Result |
| --- | --- | --- |
| Status page freshness | [34119303266](https://github.com/rotnov/pycc/actions/runs/34119303266) | success |
| Main history audit | [34119303265](https://github.com/rotnov/pycc/actions/runs/34119303265) | success |
| Pages | [34119303281](https://github.com/rotnov/pycc/actions/runs/34119303281) | **failure** — this iteration's subject, issue #987 |
| CI | [34119303280](https://github.com/rotnov/pycc/actions/runs/34119303280) | **failure** — `native-build-test (ubuntu-latest, x86_64-unknown-linux-gnu)`, `nbody_release_binary_meets_required_speedup_over_cpython` (`tests/nbody_bench.rs:577`), which then failed `ci-gate` |

The CI failure is the already-tracked benchmark flake, not a regression from #988: it is the same measure-below-threshold-then-pass-on-re-run behavior recorded in [#414](https://github.com/rotnov/pycc/issues/414), [#416](https://github.com/rotnov/pycc/issues/416) and [#641](https://github.com/rotnov/pycc/issues/641), on one of the two legs those issues already name. No new issue was filed for it.

## What this iteration did

Issue [#987](https://github.com/rotnov/pycc/issues/987): the `Pages` workflow's `Validate website` step failed on `main`, so `deploy` and `notify-indexnow` were skipped and the site stopped publishing. `scripts/check_sitemap_lastmod.rb` exited 1 because the sitemap `<lastmod>` for `https://rotnov.github.io/pycc/status/` was `2026-09-06` while the last non-merge commit touching `site/status/index.html` (`3ba4a027`) has author date `2026-09-07`. `audit` and `ci-gate` were unaffected, so merges were never blocked — only publishing.

The fix rotates the four status-page pins to `2026-09-07`, byte-neutrally:

| Pin | Old | New |
| --- | --- | --- |
| `site/sitemap.xml`, `<lastmod>` of the `/status/` `<url>` entry | `2026-09-06` | `2026-09-07` |
| `site/status/index.html`, JSON-LD `"dateModified"` | `2026-09-06` | `2026-09-07` |
| `scripts/check-site.sh`, `PAGE_SPECS["status"]["date_modified"]` | `2026-09-06` | `2026-09-07` |
| `tests/fixtures/pages-performance-manifest.json`, the `status` page's `source_artifact_sha256` | `9536f540…29abff` | `6d203f04…c234e3` |

`site/status/index.html` stays 25591 bytes against the 25600-byte budget (`2026-09-06` and `2026-09-07` are the same length). The landing page's own `dateModified` assertion at `scripts/check-site.sh:523` and the other six pages' pins were deliberately **not** touched — their source files' last commit dates are all still `2026-09-06`.

## The layer-2 decision (D-127 judgment call)

The issue and the dispatch brief both floated relaxing `scripts/check_sitemap_lastmod.rb` in the same change. An independent adversarial reviewer was dispatched with the full context rather than asking the repository owner, and its verdict — which this session adopted — was **rotate the pins only, and file the recurrence as its own issue**.

The decisive point is that the proposed relaxation is directionally wrong. Accepting `lastmod >= git_date` instead of `lastmod == git_date` does not make the observed failure pass: the stale stamp (`2026-09-06`) is *earlier* than the commit date (`2026-09-07`), so `>=` rejects it exactly as equality does. Accepting `lastmod <= git_date` would instead permit the staleness the checker exists to catch (`scripts/check_sitemap_lastmod.rb:8-13`) and would require deleting the existing negative test `test_rejects_future_lastmod` (`scripts/test_check_sitemap_lastmod.rb:197`) — weakening a gate so a pull request passes, which `AGENTS.md` forbids. A content-aware comparison would not have helped either: PR #985 made a genuine content edit to the status page in the same squash.

**Rejected alternative:** relaxing the checker to `>=` (or to a one-day tolerance window) in this pull request.

The real cause is topological: the checker's `--no-merges` filter (`scripts/check_sitemap_lastmod.rb:41`) assumes a merge-commit history in which the content commit keeps its original author date, while GitHub squash-merge collapses the branch into one commit dated at merge time. That is a separate seam and is now tracked as [#990](https://github.com/rotnov/pycc/issues/990) with three candidate procedural fixes. The same failure already occurred once before, at `06ff4c0f` (2026-08-22).

`docs/ROADMAP.md` was **not** touched: this is publishing infrastructure, not language behavior or milestone evidence, and adding a new `**[#N](…) —` paragraph would itself trigger the status-page four-pin rotation for the wrong reason.

## This change carries the same race

The pins are stamped `2026-09-07` because this pull request's own squash commit is expected to land on `2026-09-07`. Recent squash commits carry a `+0100` offset (`74729513` 12:57:40 +0100, `3ba4a027` 11:02:55 +0100), and `%as` renders in that offset, so the deadline is roughly 23:00 UTC. If the merge slips past it, the pins must be re-stamped to the new date before merging — which is precisely what #990 exists to automate.

## Where a fresh session should resume

- Merge the open pull request for #987 once required checks are green, re-verifying the stamped date against the actual merge date first.
- Next candidates in the v0.4 loop after #987: [#974](https://github.com/rotnov/pycc/issues/974), [#981](https://github.com/rotnov/pycc/issues/981) (needs a design pass first), [#982](https://github.com/rotnov/pycc/issues/982), then [#908](https://github.com/rotnov/pycc/issues/908), [#952](https://github.com/rotnov/pycc/issues/952), [#954](https://github.com/rotnov/pycc/issues/954), [#932](https://github.com/rotnov/pycc/issues/932), [#798](https://github.com/rotnov/pycc/issues/798), [#768](https://github.com/rotnov/pycc/issues/768), [#606](https://github.com/rotnov/pycc/issues/606), [#903](https://github.com/rotnov/pycc/issues/903), [#893](https://github.com/rotnov/pycc/issues/893). Newly filed: [#990](https://github.com/rotnov/pycc/issues/990).
- [#927](https://github.com/rotnov/pycc/issues/927) stays blocked on [#918](https://github.com/rotnov/pycc/issues/918). [#958](https://github.com/rotnov/pycc/issues/958) and [#965](https://github.com/rotnov/pycc/issues/965) are decision-bearing and remain deferred.
- The P-tier convention is the issue **title** prefix (`P1:`/`P2:`/`P3:`), not a label.
