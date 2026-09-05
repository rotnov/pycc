# 2026-09-05 — #565: immutable language and diagnostics publication

## Overall status

Local implementation and verification are complete on
`seo/organic-next-20260905`, based on remote default-branch commit
`4eca5e24e09d6972b5717f35652e5201dde2a02f` (unchanged at final fetch).
Publication commit `8474efb4ded5317e659a3e02dfde08cab4026079` follows the
preserved preparation commit `0d94ad8f30b27131a5da381a034d55165558e56a`.
This is the one delivery snapshot for the eventual issue #565 pull request.
The existing draft [PR #935](https://github.com/rotnov/pycc/pull/935) still
contains the preparation head; this publication phase made no push, PR/comment,
merge, account, analytics or repository-setting change.

## Delivered contract and evidence

[D-230](../decisions/D-230-bind-public-evidence-to-ordered-immutable-executions.md)
supersedes D-186's schema shape with version `2.0.0`: one ordered execution
model and projection path handles Language and Diagnostics. The public
`check-site.sh` entrypoint and environment overrides remain intact; its
cohesive evidence block now lives in `check_site_evidence.py`, with the shared
execution implementation in `site_execution_evidence.py`. Local canonical-LF
artifact bytes must match immutable preparation blobs, not merely refreshed
local hashes. The landing record and five unavailable records are unchanged.

The preserved source tree is `26bf9fe465c50d8065b1e0260e6100dc3e68f193`.
It equals the complete tree tested at PR merge
`321e66ff71f1eb4dedcd34d606f98994ad198758` in successful
[CI run 33969157527](https://github.com/rotnov/pycc/actions/runs/33969157527).
Exact CPython 3.14.7 execution and both production evidence tests were
confirmed in all five successful jobs: macOS arm64 `101314505690`, macOS x64
`101314505701`, Linux x64 `101314505789`, Linux arm64 `101314505756`, and
Windows x64 `101314505712`. These are historical preparation-tree proofs,
not claims that the later publication tree already passed remote CI.

- `/language-support/` publishes the exact PEP 526 fixture, separate debug
  `pycc run` and CPython commands, stdout `15\n`, exit 0 and empty stderr.
  Its current scope is the named tested subset, never whole Python 3.14
  compatibility or whole-PEP acceptance.
- `/diagnostics/` publishes the exact occurred T0021 source and human/JSON
  stdout, exit 1 and empty stderr. JSON help is real; human help remains absent
  and the type span remains the 1:1, zero-length placeholder. It invents no
  repair, explanation transcript, `--fix` support or all-input correctness.
- Seven canonical routes participate in navigation, sitemap, performance,
  accessibility and the current visit-observation contract. Central Markdown
  and llms use exact shared summaries and canonical detail links without
  enlarging context budgets. Visit observations remain empty and historical
  collection provenance remains unchanged.
- Newly republished current v0.4 projections now say cross-file project
  `from` imports landed while bare/submodule imports, namespace handling,
  broader project CLI behavior and incremental compilation remain incomplete.
  This is current contract repair, not a milestone acceptance claim.

## Review decisions and verification

Three pinned independent review rounds resolved seven findings; the final
38-file snapshot was clean across all eleven categories. The complete pile
is [issue-565.jsonl](../../.harden/findings/issue-565.jsonl), with real fix
commit references. The orchestration session owns the subsequent harden batch.

The history-dependent mutation suite is deliberately named
`site_execution_evidence_test.py` and runs explicitly through the existing
full-history Pages `test-check-site.sh` harness. A shallow-safe discovered
wiring suite protects that placement. A depth-1/full-history A/B reproduction
proved the original failure; final full discovery passed in a fresh depth-1
clone. Independent review rejected a governance checkout edit because the
base-owned D-171 policy and concurrent PR #936 own that seam. No workflow or
path-filter change was made. Other bounded review fixes strengthened primary
navigation separately from footer links, keyboard scroll ownership, the
mutation matrix and current measurement/status projections.

Completed local gates:

- Workspace build and ordinary workspace tests; formatting, clippy with
  `-D warnings`, and workspace API documentation. Clippy still emitted the
  existing escaped-newline warnings in `slice1_codegen_depth`; rustdoc emitted
  seven existing private-link warnings. All commands exited successfully.
- Python discovery: 991 tests, six expected skips (46.370 seconds on the final
  publication tree). Fresh shallow-clone discovery: the same 991 tests and six
  skips, successful. Full-history execution mutations: 21 tests, including
  nested-field and per-artifact/projection subtests (113.982 seconds).
- All 22 Ruby self-test suites, all shell suites, and the complete Pages
  `Validate website` command block. The two sitemap commands ran after the
  publication commit so they checked actual Git author dates; all passed.
- Workflow permissions, roadmap evidence and self-tests, conformance breadth,
  scratch usage, decision-index freshness, agent policies/assets and both
  offline Codex/Claude alpha evaluations. The final path classifier selects
  compiler, Pages and agent work. Status freshness passed on the actual
  default-base-to-publication range. Branch protection matched its baseline.
- Fresh Chrome 152 / Lighthouse 12.8.2 evidence: 35 performance reports across
  seven pages, all median scores 100; LCP 900.7–1201.0 ms, CLS 0 and TBT 0.
  Separate 404 HTTP/resource checks passed; no 404 Lighthouse claim is made.
  Seven fresh accessibility reports, HTTP identity, ARIA and reduced-motion
  evaluations passed.
- All 28 fresh route/viewport/script combinations passed: seven pages at
  320/390 CSS pixels with JavaScript disabled/enabled, required primary links
  visible, first keyboard focus on the skip link, and no document-wide
  horizontal overflow. New execution panes support local keyboard scrolling.
  Source/viewport screenshots and the corrected comparison navigation were
  visually inspected. Local logs and browser artifacts remain under
  `/tmp/pycc-565-verification.wJthWp/`; they are not checked-in CI evidence.

Two local limitations remain explicit: no CPython 3.14.7 executable was
established locally (3.14.6 was rejected, never substituted), and no trusted
passwordless-sudo/nobody coverage invocation was established. Therefore the
exact oracle rerun and isolated 100% line/region coverage gate remain required
on the final PR CI head. Ordinary workspace tests are not replacements.

## Next actions

Run the orchestration-owned harden batch, then publish the preserved commit
chain through PR #935 and inspect the exact current head's `audit`, `ci-gate`,
Tier-1/oracle/coverage/frontend-performance and Pages validation/browser checks.
Keep issue #563 and coordination work open; the intended closing reference is
only #565. Merge with a merge commit, never squash or rebase away the accepted
preparation ancestor.

After merge, verify push-to-main CI and successful Pages deployment, recording
the deployment SHA separately from current main. Compare uploaded and live
bodies for both new routes, changed navigation pages, Markdown/llms, CSS and
sitemap against that deployment tree; verify statuses, media types, canonical
metadata, evidence, limitations, assets, robots/header parity and live mobile
usability. Inspect IndexNow's receipt for the new URLs as notification evidence
only. No indexing, rankings, traffic or visit improvement follows from this
local publication work.
