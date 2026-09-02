---
id: D-217
title: "Report every frontend diagnostic per pass, with a byte-stable first diagnostic and JSON Lines output (issue #864, Part 1)"
status: accepted
---

## D-217: Report every frontend diagnostic per pass, with a byte-stable first diagnostic and JSON Lines output (issue #864, Part 1)

- Status: accepted
- Context: `pycc check` reported exactly one diagnostic per input file and stopped:
  the first frontend pass that failed (parser, HIR lowering, or the type checker)
  returned a single `Diagnostic`, and the driver's `FrontendFailure::Compile`
  carried a `Box<Diagnostic>` to `report_check_failure`/`report_build_failure`.
  Issue #864 makes that a v0.4 problem: the milestone's corpus-coverage acceptance
  ("Tier-2 corpus >= 80% files compile") and the meddylib coverage effort both
  need `check` to see past the first gap in a file, and a first-error-only sweep
  says nothing about the rest of a 58k-line codebase whose first blocker is
  almost always an `import` on line 1. The diagnostic contract lineage is D-043
  (per-diagnostic contract) -> D-083 (versioned JSON error format, one object
  per diagnostic); D-152 grew `Diagnostic` with a `help` field, which is what
  pushed the driver's error variant over clippy's `result_large_err` threshold
  and motivated the `Box` -- the box itself was never a recorded decision.

  Two facts measured against the tree shaped the decision. First,
  `ruff_python_parser 0.0.6`'s recovering parser reports errors in *discovery*
  order, not source order: for `x = (1 +\ny = 2\n  z = 3\ndef f():\nreturn 1\n`
  its `errors()` list starts with `11..12` and then `5..10`, and
  `Parsed::into_result` (what `pycc_parser::parse` called) returns `errors()[0]`.
  Sorting by span would therefore change the first diagnostic for such inputs.
  Second, the driver already prints one human render or one JSON object per
  failing *file* when several files are checked, and `tests/slice0.rs`'s
  `serde_json::from_str(stdout.trim())` pattern is valid only for
  single-diagnostic inputs; no in-tree consumer reads `check`'s stdout as one
  JSON document.

- Decision:
  1. Every frontend pass reports every diagnostic it can collect for a file, and
     the driver renders all of them. #864 lands this in three independently
     mergeable parts: Part 1 (this decision, #866) changes the driver's failure
     payload to `Vec<Diagnostic>` and fans out the parser; Part 2 (#867) collects
     per top-level item in HIR lowering with cascade suppression; Part 3 (#868)
     collects per function in the type checker. Until a part lands, its pass
     still contributes exactly one diagnostic, wrapped as a one-element vector.
  2. **The first diagnostic reported for any input is byte-stable across the
     #864 parts**: code, message, span, and position. The existing snapshot
     fixtures are the regression net for it; every new test asserts on the
     *additional* diagnostics.
  3. **Order is pass order, then that pass's own collection order.** Today only
     one pass fails per file (a file with syntax errors stops before HIR; ruff's
     recovered AST is not lowered). The parser's collection order is ruff's
     `Parsed::errors()` discovery order, kept verbatim precisely because it holds
     rule 2. No span-monotone order is promised across a file's diagnostics;
     consumers must not assume one, and Parts 2-3 must not re-sort across
     passes.
  4. **JSON Lines is the multi-diagnostic JSON shape**: one object per line, in
     report order, exactly the shape multi-file `check` already produced;
     `format_version` stays `1` because the per-object schema is unchanged.
     Human renders are concatenated with no separator, exactly as multi-file
     output already is.
  5. `pycc build`/`pycc run` print every collected diagnostic to stderr through
     the same rendering helper, then stop before MIR as before -- "fail-fast"
     means no codegen after a frontend failure, not hiding diagnostics the
     shared path already collected. They still have no `--error-format`.
  6. ruff's recovery cascades (e.g. the trailing `unexpected EOF while parsing`
     after a malformed parameter list) are reported verbatim.
  7. `pycc_parser` gains `parse_all(&str) -> Result<ModModule, Vec<Diagnostic>>`
     (never-empty `Err`, discovery order); `parse` stays as the first-element
     view so its many test and bench callers do not move.

- Alternatives:
  - *Sort diagnostics by source position.* Rejected: changes the first diagnostic
    on inputs where ruff's discovery order is not source order (measured above),
    breaking rule 2 and every snapshot that pins today's first diagnostic.
  - *A single JSON array per invocation.* Rejected: breaks the existing
    one-object-per-line multi-file shape and every consumer that reads a line at
    a time; JSON Lines is already what `check` emits for several files.
  - *First-only reporting on `build`/`run`.* Rejected: the diagnostics are already
    collected by the shared `FrontendFailure` path, and suppressing them there is
    an asymmetry the user cannot opt out of; one rendering rule also keeps one
    test surface.
  - *`debug_assert!(!diagnostics.is_empty())` or a non-empty newtype.* Rejected:
    the invariant already holds by construction (every constructor wraps one
    diagnostic or forwards `parse_all`'s non-empty `Err`, proven by the parser's
    unit tests), and either form adds an uncoverable in-crate region under
    D-014's 100%-region gate.
  - *Changing `parse`'s return type to `Vec`.* Rejected: ~75 call sites for no
    behaviour gain; the first-error view is exactly what those callers want.
  - *Filtering ruff's recovery cascades.* Deferred and unfiled: faithful
    reporting is the contract until a concrete corpus file shows the noise
    matters.
  - *`--error-format` for `build`/`run`.* Out of scope.

- Consequences:
  - `src/frontend.rs` is extracted from `src/main.rs` (which had crossed the
    ~1,000-line oversized-file threshold): `FrontendFailure`, the three
    `*_frontend` functions, both reporters, and the shared `render_all` helper
    live there; `main.rs` keeps command dispatch.
  - The `Box` around the failure payload is removed: a `Vec` keeps the variant
    well under clippy's `result_large_err` threshold. D-152 is cited only as the
    origin of the size pressure and is not superseded.
  - The planned corpus-bot fingerprinting (`docs/TESTING.md`, no workflow exists
    yet) will see several fingerprints per input that has several syntax errors;
    no document promised one fingerprint per file.
  - `docs/CLI_SPEC.md` now states the ordering rule, the JSON Lines shape, the
    exit-1 meaning ("at least one compile diagnostic in at least one file"), and
    the stdout-vs-stderr split between `check` and `build`/`run`; the pycc skill
    no longer describes `check` as reporting one diagnostic per input.
  - New snapshot fixtures pin both the fan-out (`l0001_two_syntax_errors`) and
    the Part 1 boundary (`c0001_issue_864_repro`: still one `C0001`); Parts 2-3
    regenerate the latter as their reviewable acceptance diff.
