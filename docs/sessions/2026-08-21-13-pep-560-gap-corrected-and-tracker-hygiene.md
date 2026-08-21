# 2026-08-21-13 — PEP 560's recorded gap corrected, tracker hygiene, autopilot paused

## Baseline

Default branch tip at the time of writing: `99681477`
("docs(conformance): correct PEP 560's recorded core gap — annotation gating
shipped in #611 (#694)"). No open pull requests.

## Status

**v0.3 is not met.** `python3 scripts/check_conformance_breadth.py` reports
"32 evidence-backed rows, all declared (2 accepted as whole-PEP, 30 subset)"
against this tip; v0.3's **Accept:** requires 37 rows at `◐` or better, so the
gap is 5 rows. Nothing in this checkpoint's work changed that count — the
correction below moved gap *text*, not row status.

## Delivered this checkpoint

### PR [#694](https://github.com/rotnov/pycc/pull/694) — merged as `99681477`

Corrects a factually wrong `core` gap that [#691](https://github.com/rotnov/pycc/pull/691)
shipped for PEP 560. #691 recorded the gap as "annotation-position
`ClassName[type_arg]` is not gated on `__class_getitem__`", which had already
shipped in #611 (merged as PR #613). The genuine, narrower gap is that the
annotation subscript resolves to `Ty::Instance(ClassName)` and discards the type
argument — the accepting branch recurses on the base name alone and never reads
`sub.slice`.

Changed: `tests/fixtures/conformance-breadth-manifest.json` (PEP 560's
`not_proven` entry restated and re-pointed at issue 693; all 32 `matrix_line`
values recomputed through the checker's own `evidence_rows` parser rather than by
hand), `docs/PYTHON_STANDARDS.md`, `docs/ROADMAP.md`. Headline totals are
unchanged, so #691's own `--roadmap` guard still parses them. The row stays `◐`:
the narrower gap is still a `core` gap, and D-177 forces `◐` on any `core` gap.

`closingIssuesReferences.totalCount` was verified as `0` before merge — #586
needed the evidence-gated closure path, and #693 must stay open.

### Issues

- **[#586](https://github.com/rotnov/pycc/issues/586) closed** as completed, with
  a comment superseding this session's own earlier, wrong comment on it. Both
  halves are discharged: #610 (value-position dispatch plus fixture) and
  #611/PR #613 (annotation-position gating). Cited against tree state `cd78273c`:
  `crates/pycc_hir/src/func.rs:411-447`, `crates/pycc_hir/src/class.rs:961-984`,
  `docs/DIAGNOSTICS.md:48`, `crates/pycc_hir/src/tests.rs:1741-1865`.
- **[#693](https://github.com/rotnov/pycc/issues/693) open** — the real PEP 560
  gap, filed this session and now the manifest's referenced issue.
- **[#695](https://github.com/rotnov/pycc/issues/695) filed** (P1) — decompose
  `crates/pycc_types/src/tests.rs` (25,253 lines), the largest file in the
  repository and previously untracked.
- **[#696](https://github.com/rotnov/pycc/issues/696) filed** (P2) — decompose
  `crates/pycc_codegen/src/tests.rs` (11,645 lines).
- **#545 and #549 retitled** to their measured current sizes (7,759 and 4,614
  lines) with narrowing comments, per D-185's per-merge narrowing rule.
- **#545, #549, #663, #695, #696 left with no milestone.** They were assigned to
  v0.3 earlier in this same pass and then unassigned once v0.3's **Accept:**
  criteria were actually read: they scope conformance rows, the diagnostics
  registry, and `pycc explain` — not source decomposition. Two reasons, recorded
  on each issue: this is maintainability infrastructure rather than `vX.Y`
  compiler surface, and a D-185 issue closes only when its file is under the
  threshold, which takes many pull requests and would be forced out of a
  milestone at close time anyway.
- **#162 and #397 left with no milestone** with triage comments recording
  "no milestone — cross-cutting" reasoning, matching site issues #563-#569.
- **#641 deliberately not retitled.** Its title names only `macos-15-intel`; there
  is no evidence other targets showed the same behavior, and retitling on
  suspicion fails the same bar as closing on suspicion.

### Retrospective

One entry added to `docs/AGENT_RETROSPECTIVE.md` in this same commit: the #691
gap text was composed by quoting the referenced issue's body verbatim, without
opening the crate the claim was about. Lesson: a tracker issue is dated evidence
about a past tree; pin any durable claim about implementation behavior to lines
in the current tree before writing it.

## Paused autopilot

The standing `/next-milestone` directive (invoked with no arguments) is **paused**,
not finished.

- **Directive scope:** work the tracker autonomously toward the active milestone,
  looping `issue-select` → `issue-implement` until the milestone's Accept criteria
  are met.
- **Active milestone:** v0.3 — **not met**, 32 of 37 rows, 5-row gap.
- **Last iteration's outcome:** PEP 560 gap correction merged (#694); no code
  change, no row flip.
- **In-run denylist, which must carry forward:** **#20, #631, #604.** A resumed
  session's fresh inventory will otherwise re-select and re-fail them.
  `.claude/skills/issue-select/SKILL.md` asks for each entry's own reason, and
  **those per-issue reasons were not captured** — this session was compacted
  several times and only the numbers survived. Treat the list as complete but its
  rationale as lost: a resuming session should re-derive why each of the three
  stopped before deciding to lift any of them from the denylist, rather than
  assuming the omission means the reason was trivial.
- **Next step:** re-enter `issue-select` step 1 with a fresh baseline from
  `99681477` (or later), carrying that denylist into step 4's blocker screen.

## Known follow-ups

- Narrowing candidates still needing cited evidence: #558 (elapsed-window
  measurement), #44 (the "downloaded but un-audited" gap).
- P1 issues never screened this run: #259, #563, #565, #566, #569.
- `.claude/skills/issue-implement/SKILL.md` step 4 still describes D-103's
  retired exact-byte gate as live.
- Other open threads: #676, #677, #685, #687; D-171's stale lines 8 and 12; the
  2026-08-01 issue-109 plan document's line 50 / Task 5; the orphaned
  `tests/fixtures/policy-successors/` directory; `src/project_config.rs:116`'s
  citation of a test gap that does not exist; a mechanical CI guard over declared
  `closingIssuesReferences`.
