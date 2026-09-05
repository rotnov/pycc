# 2026-09-05 — #565: immutable language and diagnostics publication

## Overall status

Local implementation, integration and review are complete on
`seo/organic-next-20260905`. This is the one current delivery snapshot for
the eventual issue #565 pull request; incoming main sessions remain intact.
The publication work made no push, PR/comment, remote merge, account, analytics
or repository-setting change. The orchestration session owns final remote
delivery through [PR #935](https://github.com/rotnov/pycc/pull/935).

The preparation commit `0d94ad8f30b27131a5da381a034d55165558e56a` remains
unchanged and reachable. Publication `8474efb4ded5317e659a3e02dfde08cab4026079`
and its records were followed by the first orchestration-owned harden batch
`08fe794368384db1fff33fe0d95a275466d7663d`. Main advanced during delivery;
three authorized local merges preserve the complete chain:

- `81f595183f14f133c5a0229b54378a2a78032f7a` integrates
  `51808cd8b1261d87900601679e33a8a5416ec6dc` (#938, issue #919).
- `2a6468bf08b364561b6fa555dddb284cd8122675` integrates
  `1ada30f8b3a6a79390d788f1e599343e1b288177` (#936) with its CI,
  governance, decision-index and retrospective changes preserved.
- `e3b46ddfdd43e76eaadf6e5086916cbbc07b3a6d` integrates
  `a275fbaa82dc2a463b8ba29a99433cf109abd6f9` (#939, issue #937).

Between the second and third merges,
`9547ba5087e159f9fbc180f0ac2b76daf419324f` supplies the independently
reviewed Pages dependency-routing correction. No rebase, squash or reset
rewrote preparation or incoming main.

## Delivered contract and evidence

[D-230](../decisions/D-230-bind-public-evidence-to-ordered-immutable-executions.md)
supersedes D-186's schema shape with version `2.0.0`: one ordered execution
model and projection path handles Language and Diagnostics. Our decision was
renumbered from D-229 after main accepted its own future-directive D-229;
the decision index was regenerated and checked. The public `check-site.sh`
entrypoint and environment overrides remain intact. Its cohesive evidence
block lives in `check_site_evidence.py`; shared ordered validation/projection
lives in `site_execution_evidence.py`. Canonical-LF artifact bytes must match
immutable preparation blobs, not merely refreshed hashes. Landing and all five
unavailable hero records remain identical to preparation.

The source tree `26bf9fe465c50d8065b1e0260e6100dc3e68f193` equals the complete
tree tested at PR merge `321e66ff71f1eb4dedcd34d606f98994ad198758` in
[CI run 33969157527](https://github.com/rotnov/pycc/actions/runs/33969157527).
Exact CPython 3.14.7 execution and both production evidence tests were confirmed
in all five successful jobs: macOS arm64 `101314505690`, macOS x64
`101314505701`, Linux x64 `101314505789`, Linux arm64 `101314505756`, and
Windows x64 `101314505712`. These are historical preparation-tree proofs,
not claims that the later publication tree already passed remote CI.

- `/language-support/` publishes the exact PEP 526 fixture, separate debug
  `pycc run` and CPython commands, stdout `15\n`, exit 0 and empty stderr.
  It proves the named subset, not whole Python 3.14 or whole-PEP compatibility.
- `/diagnostics/` publishes actual T0021 source and human/JSON stdout, exit 1
  and empty stderr. JSON help is real; human help remains absent, with the
  existing 1:1, zero-length placeholder span. No invented repair, explanation
  transcript, `--fix` support or all-input correctness is claimed.
- Seven canonical routes participate in navigation, sitemap, performance,
  accessibility and current visit-observation contracts. Central Markdown/llms
  link the evidence without enlarging context budgets. Visits remain empty;
  historical collection provenance and the no-analytics decision are unchanged.
- Current v0.4 projections say cross-file project `from` imports landed while
  bare/submodule imports, namespace handling, broader project CLI behavior and
  incremental compilation remain incomplete. Status's hero stays unavailable.
- Language and Status separately describe main's PEP 563 subset: the
  module-prologue directive is a compile-time no-op binding no feature name.
  Its fixture shows supported own-class method and already-defined-class
  function annotations working with that directive. Later-defined forward
  references, string annotations and runtime `__annotations__` introspection
  remain three core gaps. The separate evidence is
  [main CI 33972731538](https://github.com/rotnov/pycc/actions/runs/33972731538)
  at `51808cd8`, all five Tier-1 targets in debug/release; it does not enlarge
  the historical PEP 526 transcript. Current Status/ROADMAP totals are 39
  evidence-backed rows, 2 whole-PEP acceptances, 37 subsets and 40 distinct
  PEPs. Independent review retained the landing/README's dated v0.3 acceptance
  counts as historical context rather than attaching new evidence to that date.

## Review and routing decisions

Eight pinned independent rounds covered the full publication diff; round 8
read all 43 in-scope files through EOF and was clean across all eleven
categories. The [findings pile](../../.harden/findings/issue-565.jsonl) contains
the original seven fixed findings, three integration-review findings, the
caught resource-budget regression, and clean-round markers. The first seven
already received the batch recorded in `08fe7943`; the orchestration session
owns the remaining batch. No second batch was run by the publication writer.

Round 4 fixed the missing module-prologue qualification and rejection of
`noscript`-hidden source/limitations (four independent false-accept controls).
Round 5 was clean. Round 6 found missing Pages triggers; round 7 was clean
after the bounded routing fix. During final integration, the larger Status
note reached 25,761 bytes against its unchanged 25,600-byte limit. Three
existing Ruby healthy controls caught it. Removing a redundant historical
exception-row recap reduced the page to 25,442 bytes; no budget was relaxed.
The interrupted pre-fix performance capture is not accepted evidence.

The history-dependent suite `site_execution_evidence_test.py` runs explicitly
through the full-history Pages shell harness, outside shallow governance's
`test_*.py` discovery. The shallow-safe wiring suite protects that placement.
Independent D-127 review rejected a governance checkout mutation on the old
base; #936's subsequently merged governance changes remain untouched.
The later authorized fix enumerates exactly ten direct execution dependencies
in each Pages push and pull-request filter. Twenty initially failing positive
cases, twenty removals and twenty duplicates exercise each event independently;
Ruby YAML checks independently confirm all twenty exact-one event/path pairs.
Helper-only `ci.yml` classification intentionally stays EMPTY. Permissions,
checkout depth, thresholds, required checks, `ci.yml` and D-171 were not changed
by this routing fix. Actual remote Pages event runs remain a post-push check.

## Local verification

- Full workspace build and ordinary tests, fmt, clippy with `-D warnings`,
  and API docs passed after #938 integration. #936 and #939 add no compiler
  implementation. Focused final Rust reruns passed: future-import CLI 3/3;
  site evidence 5 passed/1 oracle-dependent ignored; conformance guard tests
  5 passed/56 oracle-dependent ignored. API docs and fmt passed again.
  Existing escaped-newline and seven private-link rustdoc warnings remain.
- The entire current governance run block passed (local LLVM already installed,
  so its apt installation step was not rerun): Python discovery 996 tests,
  six expected skips; permissions and roadmap checkers/self-tests; conformance
  breadth; decision-index freshness; scratch checks; README badge tests;
  workspace build; both offline Codex/Claude alpha evaluations.
- All 22 Ruby suites passed. The complete Pages `Validate website` block
  passed again after the final resource fix, including all 24 full-history
  execution tests (137.502 seconds), route/metadata/sitemap/Markdown tests and
  every observation/identity/workflow suite. The two new current-scope positive
  controls failed before implementation; positive and gap mutations then passed.
- Agent assets/policies, Codex marketplace discovery and the gh-status shell
  suite passed. The actual frontend throughput check passed at 38.68 ms against
  75 ms. Overall PR classification selects compiler, Pages and agent work.
  Branch protection matched strict administrator-enforced `audit`/`ci-gate`.
- Fresh Chrome 152 / Lighthouse 12.8.2: 35 reports, seven pages, median score
  100 throughout; LCP 900.7–1200.9 ms, CLS 0, TBT 0. Separate 404 HTTP/resource
  checks passed; no 404 Lighthouse result is claimed. Seven accessibility
  reports, HTTP identity, ARIA and reduced-motion checks passed.
- All 28 fresh 320/390-pixel JS/no-JS route cases passed: primary navigation
  visible, skip link first in keyboard order, no document-wide overflow,
  visible evidence/limitations and locally keyboard-scrollable execution panes.
  Fresh Status scope, Language viewport and Diagnostics source screenshots were
  visually inspected. Final artifacts are under
  `/tmp/pycc-565-merge.tCkaqN/`: `performance-pep563-final/`,
  `accessibility-pep563-final/`, `qa-pep563/` and matching logs. They are local
  acceptance evidence, not checked-in CI proof.

No local CPython 3.14.7 executable was established (3.14.6 was rejected, never
substituted), and no trusted passwordless-sudo/nobody coverage invocation was
established. Final PR CI must rerun the exact oracle and isolated 100% line and
region coverage. Ordinary workspace tests do not replace either gate.

## Remaining delivery

The orchestration session must complete its follow-up harden batch and publish
the preserved chain through PR #935, then inspect that exact head's `audit`,
`ci-gate`, Tier-1/oracle/coverage/frontend-performance and Pages checks,
including actual PR/push routing. Only #565 is intended to close; #563 and
coordination work remain open. Use a merge commit, never squash or rebase away
the accepted preparation ancestor.

After merge, verify push-to-main CI and successful Pages deployment, recording
deployment SHA separately from current main. Compare uploaded/live bodies for
the new routes, changed navigation pages, Markdown/llms, CSS and sitemap with
that tree; verify HTTP/media type, metadata, evidence, limitations, assets,
robots/header parity and live mobile usability. Inspect IndexNow's new-URL
receipt as notification evidence only. Publication establishes no indexing,
ranking, traffic or visit improvement.
