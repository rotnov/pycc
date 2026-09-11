---
id: D-243
title: "Architecture hero is a checked-in, re-derivable compiler pipeline trace"
status: accepted
---

## D-243: Architecture hero is a checked-in, re-derivable compiler pipeline trace
- Status: accepted
- Context: The Architecture page was the last evidence-first page whose first
  screen showed a diagram of a pipeline rather than proof that the pipeline
  runs. Under the #564 contract (D-186/D-230) its record stayed explicitly
  `unavailable`, and #566 owned the missing artifact. The four remaining
  options for closing it differ in what they actually prove. A live build at
  deploy time proves the most and is unavailable: the Pages job has no LLVM
  toolchain, and a hero that depends on a toolchain at publish time fails
  closed on an unrelated infrastructure change. A hand-written stage table
  proves nothing and is exactly the decorative evidence the contract exists to
  reject. A CI-run attestation in the D-230 shape proves one historical run but
  publishes no stage bytes a reader can compare. Checking the stage bytes in
  and re-deriving them in an ordinary test proves the claim continuously, on
  every Tier-1 target, at the cost of artifacts that churn.
- Decision: The `architecture` record projects one production fixture,
  `tests/fixtures/quick_start.py`, through eight ordered stages -- `source`,
  `parser`, `hir`, `type-check`, `mir`, `llvm-ir`, `native`, `stdout` -- with
  each stage's bytes checked in under `tests/fixtures/architecture-trace/` and
  pinned by canonical LF SHA-256. `tests/architecture_trace.rs` is the only
  party that re-derives them: it drives the public crate APIs and a native
  build, byte-compares every stage, asserts the native exit status and exact
  stdout, and carries an omitted-stage negative control.
  `scripts/site_pipeline_evidence.py` owns the record shape, the closed stage
  vocabulary, the derived state and the visible projection, and never runs the
  compiler. Schema version becomes `2.2.0` and the kind vocabulary gains
  `compiler-pipeline-trace`. The state is derived by a closed mapping from the
  stage evidence kinds and is `partial`: pycc has no `--emit` flag, so the
  `llvm-ir` stage carries no artifact at all, and there is deliberately no path
  from this record to `all-Tier-1` while that stage exists.
- Decision: The architecture page is deliberately *not* added to
  `check_site_evidence.py`'s `CURRENT_FUTURE_SCOPE`. That constant marks pages
  whose prose describes work not yet done, so a page in it may not carry a
  proof hero. The architecture page now carries one; putting it in both places
  would make the two halves of the contract contradict each other, and the
  honest statement of what is unproven belongs in the record's `limitations`,
  which every surface republishes verbatim, rather than in a page-level
  future-scope flag.
- Decision: The record's Rust-side manifest facts live in a new
  `tests/architecture_manifest.rs` rather than in `tests/site_evidence.rs`,
  which the implementation plan named. The D-230 language and diagnostics
  records pin `tests/site_evidence.rs` byte-for-byte to a preserved source
  blob, so adding a case there fails the site gate with "language artifact
  differs from preserved source blob". This is a recorded deviation from the
  plan, resolved in favour of the immutability contract.
- Decision: The architecture summary line pushed `site/index.html.md` past its
  13824-byte per-resource allocation in `site/llms-txt-context-manifest.json`.
  D-227 rejected raising `budget_kib` a third time and `scripts/check-site.sh`
  enforces the partition invariant `sum(budget_bytes) <= budget_kib * 1024`
  with only 512 deliberately unallocated bytes, so the growth is absorbed by
  reallocation: the Markdown landing rises 13824 -> 15360 and
  `docs/PYTHON_STANDARDS.md` falls 43008 -> 41472, it having carried the
  largest slack of any document in the manifest (38851 actual). The ceiling and
  the 512 unallocated bytes are unchanged. This was a judgment call taken under
  D-127 rather than referred to the repository owner; the alternative --
  shortening the published summary until it fit -- would have traded a
  reviewable allocation for a less honest projection.
- Alternatives: A live build at deploy time (no LLVM toolchain in the Pages
  job, and a publish-time toolchain dependency makes the hero fail closed on
  unrelated infrastructure changes). A hand-authored stage table (decorative
  evidence; rejected by the contract's own purpose). A D-230-shaped CI-run
  attestation (proves one historical run, publishes no comparable stage bytes,
  and cannot be re-derived by a reader). Emitting LLVM IR to close the eighth
  stage (needs an `--emit` compiler flag that does not exist; adding one is a
  separate change with its own CLI contract, and inventing an artifact to reach
  `all-Tier-1` is the failure mode this contract exists to prevent). Raising
  the llms.txt ceiling a third time (explicitly rejected by D-227).
- Consequences: Easier -- the Architecture hero is now continuously true rather
  than true as of one run: any change to the traced pipeline's output fails
  `cargo test --workspace` on every Tier-1 target immediately, and a reader can
  compare the published excerpts against the checked-in bytes without running
  anything. The closed stage list makes adding, removing or rebinding a stage a
  reviewed change rather than an edit. Harder -- the parser, HIR and MIR
  artifacts are Rust `Debug` renderings, a developer-facing format with no
  stability guarantee, so their SHA-256 identities churn whenever those types
  change; every such change must regenerate the artifacts with
  `PYCC_ARCHITECTURE_TRACE_OUT=tests/fixtures/architecture-trace cargo test
  --test architecture_trace regeneration` and refresh the record and the page
  in the same pull request. That churn is accepted deliberately: a stable but
  unverifiable rendering would prove less. Irreversible -- schema `2.2.0`, the
  `compiler-pipeline-trace` kind and the eight-stage vocabulary are now part of
  the published contract, and the llms.txt allocation between the Markdown
  landing and `docs/PYTHON_STANDARDS.md` is now the reviewed split.
