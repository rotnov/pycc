# 2026-09-11-05 — Issue #1007: Architecture hero as a reproducible pipeline trace

## Overall status

Issue #1007 ("Part 2 of #566: Architecture hero as a reproducible pipeline
trace from `tests/fixtures/quick_start.py`", milestone v0.4) is implemented on
the task branch `feat/issue-1007-architecture-hero`, branched from
`b606062313e71f915d0a377acb548ce80c787940` — the exact baseline the
authoritative plan (issue comment 5638193866) was written against. The work is
committed on that branch only; at the time this snapshot was written nothing
had been pushed and no pull request existed. The orchestrating session owns the
push, the review rounds and the merge.

The Architecture record moves from `unavailable` to `partial` under
[D-243](../decisions/D-243-architecture-hero-is-a-checked-in-re-derivable-compiler-pipeline-trace.md).
It is `partial` and not `all-Tier-1` on purpose: pycc has no `--emit` flag, so
the `llvm-ir` stage carries no artifact, and one fixture is not the language.
Both limitations are published verbatim on every surface.

## What landed

- `tests/fixtures/architecture-trace/` — the pinned stage artifacts
  (`parser-ast.txt`, `hir-module.txt`, `mir-items.txt`) plus `trace.json`,
  which names the compiler commit and tree, the native `argv` and exit status,
  the exact stdout, and the excerpt line count.
- `tests/architecture_trace.rs` — the only party that re-derives the pipeline:
  public crate APIs plus a native build, byte-compared against the artifacts,
  with an omitted-stage negative control and a coverage-safe regeneration path.
- `scripts/site_pipeline_evidence.py` — the record shape, the closed
  eight-stage vocabulary, the derived state and the visible projection. Never
  runs the compiler.
- `scripts/site_pipeline_evidence_test.py` (public-CLI mutation battery, run by
  `scripts/test-check-site.sh`) and `scripts/test_site_pipeline_wiring.py`
  (fast record-internal and wiring controls, discovered by
  `unittest discover -s scripts -p 'test_*.py'`).
- `tests/architecture_manifest.rs` — the Rust-side manifest facts.
- `site/evidence-heroes.json` at schema `2.2.0`, `site/architecture/index.html`
  (21,057 bytes against the 25,600 budget), the Markdown and llms.txt
  summaries, and the four site pins rotated.
- Docs in the same commits: `docs/WEBSITE.md` (new "Architecture
  pipeline-trace record" section with the validation-layer table and the
  regeneration command), `docs/TESTING.md`, `docs/ROADMAP.md`, the D-243 entry
  and the regenerated decisions index.

## Deviations from the plan, and why

1. **`tests/site_evidence.rs` could not take the manifest-facts case.** The
   plan asked for it there; the D-230 language and diagnostics records pin that
   file byte-for-byte to a preserved source blob, and adding a case fails the
   site gate with "language artifact differs from preserved source blob". The
   case lives in a new `tests/architecture_manifest.rs` instead.
2. **No `sha2` crate exists in the workspace**, so `tests/architecture_trace.rs`
   compares stage bytes by equality rather than by digest; SHA-256 verification
   lives in `scripts/site_pipeline_evidence.py`, which reads the same bytes.
3. **The llms.txt per-resource budget was reallocated, not raised.** The
   architecture summary pushed `site/index.html.md` past its 13824-byte
   allocation. D-227 rejected a third `budget_kib` raise and `check-site.sh`
   enforces the partition invariant, so the Markdown landing rose to 15360 and
   `docs/PYTHON_STANDARDS.md` fell to 41472 (it carried the largest slack). The
   ceiling and the 512 unallocated bytes are unchanged. Recorded in D-243 and
   in `docs/WEBSITE.md`'s allocation paragraph.

## Known follow-ups

- `tests/fixtures/pages-performance-budget.json`'s `css.current_bytes` reads
  26356 while `site/styles.css` is 27780 bytes. Pre-existing drift, outside
  this issue's scope, and documentation-only — nothing under `scripts/` reads
  that field. The adjacent `html.current_max_bytes` was corrected here because
  the plan named it.
- The `llvm-ir` stage stays unevidenced until pycc grows an `--emit` flag.
  #566 remains open; this is Part 2 only.
- `docs/DELIVERY_PLAN.md` was reviewed and needs no change: it stays at
  milestone/PR granularity and names neither #566 nor #1007.

## Where a fresh session should resume

Start from `feat/issue-1007-architecture-hero`. `docs/WEBSITE.md`'s
"Architecture pipeline-trace record" section is the contract; D-243 is the
rationale including every alternative rejected. To change anything the trace
touches, regenerate with
`PYCC_ARCHITECTURE_TRACE_OUT=tests/fixtures/architecture-trace cargo test
--test architecture_trace regeneration`, then refresh the record's digests and
the page's excerpts and rotate the four date pins; the site gate fails until
all of them agree.
