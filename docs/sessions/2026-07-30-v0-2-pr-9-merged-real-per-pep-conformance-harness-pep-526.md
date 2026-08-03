# 2026-07-30 — v0.2 PR-9 merged (real per-PEP conformance harness + PEP 526)

**Authoritative checkpoint:** `main`'s tip is
[`3b38fe6`](https://github.com/rotnov/pycc/commit/3b38fe6) — [PR #234](https://github.com/rotnov/pycc/pull/234)
(v0.2 PR-9), squash-merged directly onto `a4c8440` (PR-8's own merge
commit) with no intervening `main` activity, so PR-9 needed none of
PR-8's merge-conflict/rebase overhead. Delivered via
`superpowers:subagent-driven-development`, 10 tasks: D-102 (Task 1, the
decision to extend `tests/conformance.rs` in place rather than build
`pycc_testkit`, superseding D-018/D-037/D-085), a dual-profile
debug/release fixture runner (Task 2), the bare-name subset of PEP 526
(variable annotations, `x: int = 1` and value-less `x: int`) across the
full pipeline — `pycc_ast`/`pycc_hir`/`pycc_types` (new `T0025`
diagnostic)/`pycc_mir` (new `MirStmt::NoOp`)/`pycc_codegen` (Tasks 3-5).
Deliberately out of scope, documented in the plan: parenthesized or
subscript annotation targets, and durably tracking a value-less `x: int`
declaration against a later plain, unannotated reassignment (nothing in
`Environment` today distinguishes "declared but unbound" from "bound").
9 new conformance fixtures needing no new language feature
(PEP 238/3105/3107/3131/414/484/498/515 plus PEP 526's own, Tasks 6-9),
and a docs sweep + final pinned review + merge (Task 10).

**Three real bugs found and fixed during the per-task review loop, all
independently verified rather than taken on the implementer's word:**
a parser gap silently dropping `StmtAnnAssign`'s `simple` field (Task 3,
confirmed against the pinned `ruff_python_parser` 0.0.6 vendored source);
an `env.bind` call in `pycc_types` that would let a re-annotated binding
silently change representation, fixed to `check_assignment` instead
(Task 4); and a silent miscompilation where an annotated assignment bound
the initializer's type instead of the annotation's, undersizing the
codegen slot (Task 5) — the reviewer reproduced the bug-before/fix-after
behavior in an isolated git worktree by hand-applying the buggy code to
the pre-fix commit, the strongest verification technique used this
session. One deliberately deferred gap remains, documented inline in
`crates/pycc_types/src/lib.rs` and cross-referenced from `docs/DECISIONS.md`
D-102's own entry: `collect_block_constraints`'s `AnnAssign` arm discards
the annotation in favor of the initializer's inferred term, confirmed
real but narrow (only reachable via underscore-prefixed private helpers
with `Ty::Infer` signatures) and unreachable by any fixture this PR
ships.

**Self-inflicted CI break, caught and reverted rather than worked
around.** The Task 10 docs sweep's own comment-count fix to
`.github/workflows/ci.yml` (correcting a stale "two"/"eleven" test count
to the real count of 12) broke this repo's D-100 whole-file digest pin —
`scripts/check_roadmap_evidence.rb` hashes `ci.yml`'s exact bytes as a
security trust anchor with no carve-out for comment-only edits, a
distinction the plan's own task text had wrongly assumed exempted it.
Both edits (`6af3638`, then a same-day undercount fix `3cf234f`) were
reverted in `963e5af`, restoring the exact pinned blob byte-for-byte
(verified via `git rev-parse` blob-hash equality with `origin/main` plus
a clean local `check_roadmap_evidence.rb`/`test_check_roadmap_evidence.rb`
run). The stale comment counts remain in `ci.yml` as a deliberately
deferred cosmetic gap for whichever future PR next legitimately re-stages
that file's digest. Full writeup in the plan's own Task 10 Step 5
correction note and a new `docs/AGENT_RETROSPECTIVE.md` entry
("A digest-pinned file has no 'comment-only, no functional change'
exemption").

**`frontend-perf-gate` flagged noise twice on this branch, both
confirmed via a fresh full CI re-run rather than dismissed on the first
failure**, per this project's own D-095/D-096/D-101 methodology: once
early in the branch's life (2.9395% reported, -0.1886% on rerun) and
once immediately after the digest-pin revert above (10.5447% reported
against a commit whose only diff from the last cleanly-passing commit
was a comment plus an equivalent pattern-binding rewrite — zero
behavioral change — then a clean pass on rerun). Neither incident
changed any perf-gate threshold or mechanism.

**Merged.** The pinned `ievo:deep-reviewer` reviewed the full committed
range (`a4c8440`..head, the exact merge-base with `main`) twice — once
mid-branch (Task 10 Step 7, one finding: an "11" vs. "12" test-count
comment, fixed), once as the final whole-branch gate (two doc-drift
notes: `docs/PYTHON_STANDARDS.md` and the v0.2 design doc's own PR-9
bullet both lacked a D-102 cross-reference, fixed in `a6e1243`). No
correctness, security, or contract-fidelity findings either time. A
`chatgpt-codex-connector` bot review independently flagged the same
`collect_block_constraints` gap already tracked above; replied with the
existing rationale and resolved the thread (GitHub's `resolved
conversations` branch-protection rule was otherwise blocking the merge).
Final CI run before merge was fully green (all four `native-build-test`
legs, `build-test-coverage`, both `cross-compile-*` jobs,
`frontend-perf-measure`/`frontend-perf-gate`, `audit`, `ci-gate`),
`mergeStateStatus: CLEAN`. [PR #234](https://github.com/rotnov/pycc/pull/234)
squash-merged as [`3b38fe6`](https://github.com/rotnov/pycc/commit/3b38fe6);
its now-fully-merged remote branch was deleted, matching PR-8's own
precedent (not every prior PR's branch was deleted, so this is
convention, not a hard rule).

Two unrelated open PRs exist on `main` from a separate concurrent actor
(`codex/stage-search-ledger-audit` #232, `codex/fix-seo-query-intent`
#230) — untouched by this session, noted here only so a fresh session
doesn't mistake them for PR-9 follow-up work.

Next up per `docs/DELIVERY_PLAN.md`'s v0.2 breakdown: PR-10 (`Ty`
representation migration per D-089, ~729 call sites; monomorphization
foundation; `list[T]` thin slice) — flagged in PR-8's own handoff note
below as the highest-risk remaining PR in v0.2.
