# pycc v0.1 PR-4: Frontend Depth Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [x]`) syntax for tracking.

**Goal:** Grow pycc's frontend (parser → HIR → types → diagnostics) to accept and strictly type-check the full v0.1 grammar (arithmetic, comparisons, `if`/`while`/`for`+`range`, functions with arguments/return values/recursion, `bool`/`float`/`str` literals, basic f-strings, local variables), implement real T0001 (public-signature annotation requirement) and T0002 (`Any` forbidden) diagnostics with a real human/JSON renderer and real spans, stand up `pycc check` for real, add `tests/diagnostics/` negative fixtures with snapshot-style assertions, and wire the first frontend performance-gate benchmark, per `docs/DELIVERY_PLAN.md`'s PR-4 row and Performance gate section.

**Architecture:** `pycc_mir`/`pycc_codegen` are **not** extended with new lowering logic in this PR — per DELIVERY_PLAN.md's own row split, that is PR-5 ("Codegen depth"). `pycc_hir` grows a real, general small-IR shape (`HirExpr` tree + a wider `HirStmt`) instead of the old two special-cased variants; `pycc_mir` gets a **mechanical** update so it still compiles (exhaustive match) against the grown `HirStmt`, but every new construct it can't lower yet fails with a single, clearly-labeled `todo!("codegen for X lands in PR-5")`-style panic — a deliberate, unchanged-behavior boundary, not new codegen work. `pycc check` (parse → HIR → types, no MIR/codegen at all) is the only consumer of the new frontend depth for this PR; `pycc build`/`pycc run` keep working exactly as before, on exactly the same subset they already support.

**Tech Stack:** Same as PR-1 through PR-3 (Rust 1.97.1 edition 2024, `ruff_python_parser`/`ruff_python_ast` 0.0.6 pinned, `clap`, `cargo-llvm-cov`). No new external crate dependency is added for diagnostic snapshot testing (D-036 below) or for the performance gate (D-038 below) — both are hand-rolled with `std`/existing deps, the most conservative option.

## Global Constraints

- Rust edition 2024, toolchain `1.97.1`, unchanged from prior PRs.
- `cargo llvm-cov --fail-under-lines 100 --fail-under-regions 100` gates every PR (D-014) — every new function needs a test in the same commit that adds it, including every new match arm and every new diagnostic code's success *and* failure path.
- `ruff_python_ast = "0.0.6"` is the exact pinned version this plan's code is verified against (see Task 4's verified-API appendix) — do not assume newer/older ruff API shapes.
- `pycc_mir`/`pycc_codegen` receive **no new lowering logic** in this PR (see Architecture above) — if a task description asks you to make `print(x + 1)` actually *run*, stop: that is out of scope, re-read the Architecture section.
- `pycc_own`/`pycc_std`/`pycc_lexer` remain deferred (D-017, unchanged).
- `pycc_testkit` and `tests/conformance/` remain deferred to PR-6 (D-018, reaffirmed as D-037 below) — this PR only adds `tests/diagnostics/`, not `tests/conformance/`.
- Every new `Diagnostic` code must actually be producible by a real, reachable code path added in this same PR — no stub codes nothing constructs (per DIAGNOSTICS.md's own registry being aspirational; only implement what's real).
- Diagnostics are byte-identical across Tier-1 platforms (DIAGNOSTICS.md's quality bar) — never format a path with a platform-specific separator in test-asserted output; use forward slashes or file-name-only in fixtures.

## Post-implementation review corrections

This checked plan records the original execution sequence; it is not the
current normative contract where a later review correction differs. The
containing commit incorporates these corrections:

- D-045 replaces the plan's temporary use of `Ty::None` for missing private
  annotations with explicit inference variables and a module-local monomorphic
  solver. Private identity, arithmetic, `range`, recursive, and implicit-`None`
  helpers therefore use real constraints; unresolved or conflicting variables
  request annotations instead of silently becoming `None`.
- The checker applies Python true-division semantics, validates every
  `range` operand, rejects incompatible reassignment with `T0023`, emits
  `T0022` for return mismatches, and rejects a non-`None` function whenever
  the v0.1 conservative control-flow analysis finds a fallthrough path.
- The diagnostic snapshot suite contains five fixtures: `T0001`, `T0002`,
  `T0021`, `T0022`, and `T0023`. D-043 remains the owner of the documented
  placeholder-span and missing-safe-help gaps.
- D-044 supersedes D-042 and Task 14's informational-only recommendation:
  `frontend-perf-gate` is a required `ci-gate` dependency, with the
  first-baseline bootstrap handled inside the required job.

When this historical plan conflicts with `docs/SPEC.md`, its linked
specifications, or the later ADRs, those current sources win.

---

## File Structure

```
pycc/
├── docs/
│   └── DECISIONS.md                     # D-035..D-038 appended (Task 1)
├── crates/
│   ├── pycc_diag/src/lib.rs              # Span gets real line/col; new render module (Task 2, 3)
│   ├── pycc_ast/src/lib.rs                # new re-exports (Task 4)
│   ├── pycc_parser/src/lib.rs             # threads real spans from ruff's error (Task 2)
│   ├── pycc_hir/src/lib.rs                # HirExpr tree + wider HirStmt; one task per grammar slice (Tasks 5-11)
│   ├── pycc_types/src/lib.rs              # real environment + local inference + T0001/T0002 (grows alongside Tasks 5-11)
│   └── pycc_mir/src/lib.rs                # mechanical exhaustive-match update only (Task 5, then untouched)
├── src/
│   ├── main.rs                           # `pycc check` subcommand wired for real (Task 12)
│   └── cli.rs                            # `Check` variant already exists; confirm its exact shape before Task 12
├── tests/
│   └── diagnostics/                      # NEW directory: dNNNN_slug.py fixtures + expected .txt (Task 13)
├── benches/
│   └── check_bench.rs                    # NEW: criterion benchmark for `pycc check` (Task 14)
└── .github/workflows/ci.yml               # new perf-gate job (Task 14)
```

---

## Task 1: Record PR-4 scope decisions in DECISIONS.md

**Files:**
- Modify: `docs/DECISIONS.md`

**Interfaces:**
- Produces: nothing code-facing. This task exists so every later task can cite a settled, written decision instead of re-litigating scope mid-implementation.

- [x] **Step 1: Append four new ADR entries**

Add to the summary table (after the last existing row) and as full sections at the end of the file, following the exact format every existing entry already uses:

```markdown
| D-035 | PR-4 is frontend-only: `pycc_mir`/`pycc_codegen` gain no new lowering logic; new HIR constructs get a clear "not implemented until PR-5" panic in MIR's one exhaustive match, not new codegen work | accepted |
| D-036 | Diagnostic snapshot tests are hand-rolled (`assert_eq!` against a checked-in expected-output file per fixture), not the `insta` crate — TESTING.md's "insta-style" describes the *approach*, not a dependency requirement, and this avoids a new external dependency for a mechanism `std` already covers | accepted |
| D-037 | `pycc_testkit` and `tests/conformance/` remain deferred past PR-4 to PR-6, per D-018's original "PR-4/PR-6" window — PR-4 adds only `tests/diagnostics/` (negative fixtures), which DIAGNOSTICS.md and PYTHON_STANDARDS.md already specify independently of the conformance harness | accepted |
| D-038 | A top-level function is "public" for T0001 purposes iff its name does not start with `_`, matching ordinary Python convention — TYPE_SYSTEM.md says "locals and private helpers" are inferred but never defines "private" for module-level defs; this is the standard, least-surprising reading | accepted |
```

```markdown
## D-035: PR-4 is frontend-only; MIR gains no new lowering logic

- Status: accepted (PR-4 is the PR that depends on it)
- Context: DELIVERY_PLAN.md's PR table splits "PR-4: Frontend depth" from "PR-5: Codegen depth: full v0.1 feature set (int/float/str/bool, arithmetic, control flow, recursion, f-strings)" — the *same* feature list PR-4's own row implies growing (via "full v0.1 grammar"). Read literally, PR-4 grows the grammar the *frontend* accepts and type-checks; PR-5 grows what the *backend* can actually compile and run. `pycc check` (CLI_SPEC.md: "frontend only: parse + types + ownership; ruff-fast, no codegen") is the concrete CLI surface that only needs the frontend, confirming this split is intentional, not an oversight.
- Decision: `pycc_hir` grows a real small-IR shape for the full v0.1 grammar (Tasks 5-11). `pycc_mir::build()` is updated only mechanically, so it still compiles against the wider `HirStmt`/`HirExpr` enums (Rust's exhaustive matching forces this) — every new construct gets an explicit `panic!("pycc_mir: <construct> codegen lands in PR-5")` arm, not real lowering. `pycc build`/`pycc run` therefore keep working on exactly the subset they already support (module-level `print(<i64 literal>)`, zero-arg function definitions/calls) and fail loudly, with a clear message naming the reason, on anything new PR-4's frontend now accepts but PR-5 hasn't implemented lowering for yet.
- Alternatives: extend `pycc_mir`/`pycc_codegen` in lockstep with every new HIR construct so `pycc build` supports the full grammar immediately (rejected — this is literally PR-5's scope restated, and doing it here would make PR-5 an empty PR while making PR-4 unboundedly large). Build a second, parallel expression tree used only by `pycc check` so `pycc_mir` never needs touching at all (rejected — creates two divergent ASTs that must be reconciled again in PR-5 anyway, redundant work for no real benefit given the mechanical MIR update is small).
- Consequences: `pycc build`/`pycc run` on any of PR-4's new grammar (arithmetic, control flow, functions with arguments, f-strings, etc.) panics with a clear, PR-5-referencing message rather than silently miscompiling or producing wrong output — a real, intentional, and temporary gap, not a silent one. Every `tests/slice0.rs` CLI-level test added in PR-4 for new grammar must go through `pycc check`, not `pycc build`/`pycc run`.

## D-036: Diagnostic snapshot tests are hand-rolled, not the `insta` crate

- Status: accepted (PR-4 is the PR that depends on it)
- Context: TESTING.md's Layer 3 row says diagnostic fixtures use "insta-style snapshots." DELIVERY_PLAN.md's PR-4 row says "first diagnostic codes with snapshot tests." Neither document requires the literal `insta` crate; "insta-style" describes comparing rendered output against a checked-in expected file and failing on any diff, which `std::fs::read_to_string` + `assert_eq!` already does without a new dependency.
- Decision: each `tests/diagnostics/dNNNN_slug.py` fixture pairs with a `tests/diagnostics/dNNNN_slug.expected.txt` file holding the exact expected human-format diagnostic output (CLI_SPEC.md's format). The test harness (`tests/diagnostics_test.rs`) runs `pycc check` on the fixture and asserts the captured stdout/stderr equals the expected file's contents exactly.
- Alternatives: add `insta` as a dependency now (rejected — `insta`'s own workflow (`.snap.new` files, `cargo insta review`) is a genuine ergonomic win at scale, but is a new external dependency this PR doesn't need to introduce yet; revisit if/when the fixture count grows large enough that manual maintenance becomes the bottleneck, as its own new ADR entry).
- Consequences: adding a new diagnostic fixture means hand-writing its exact expected output once, verified by running `pycc check` on it and copying the real output in (never hand-typed from imagination) — slightly more manual than `insta`'s auto-generate-and-review flow, acceptable at this fixture count.

## D-037: `pycc_testkit`/`tests/conformance/` remain deferred to PR-6

- Status: accepted (reaffirms D-018 for PR-4 specifically)
- Context: D-018 already deferred `pycc_testkit` "past PR-1/PR-2... built for real at PR-4/PR-6," leaving genuine ambiguity about which of those two PRs. PYTHON_STANDARDS.md's PEP matrix is still 100% `☐` (unstarted) and DELIVERY_PLAN.md's own task breakdown names PR-6 "Conformance + benchmark gate" — a dedicated PR for exactly this harness.
- Decision: PR-4 does not create `pycc_testkit` or `tests/conformance/`. It creates only `tests/diagnostics/` (D-036), which DIAGNOSTICS.md and PYTHON_STANDARDS.md's "rejected by design" table already specify as its own, separate concern from PEP conformance fixtures.
- Alternatives: stand up a minimal `pycc_testkit` now too (rejected — no PEP matrix exists yet for it to check against, the same YAGNI reasoning D-017/D-018 already used; would be structure without function).
- Consequences: PR-6 is where the PEP-by-PEP conformance harness and its `tests/conformance/pyXY/` fixtures actually get built; PR-4's diagnostic fixtures are unaffected by that later work since they live in a separate directory with a separate, already-real purpose.

## D-038: A leading underscore marks a top-level function "private" for T0001

- Status: accepted (PR-4 is the PR that depends on it)
- Context: TYPE_SYSTEM.md's strictness rule 1 requires annotations on "every public function/method," rule 2 says "locals and private helpers" are inferred — but the document never defines what makes a *module-level function* public or private (there is no Python keyword for this, unlike a class's `_`-prefixed attribute convention, which IS an established Python-ecosystem norm this extends).
- Decision: a module-level function whose name does not start with `_` is public and requires a fully annotated signature (T0001 on any missing parameter or return annotation); a leading `_` marks it private, eligible for local inference instead.
- Alternatives: treat every top-level function as public, always requiring annotations regardless of naming (rejected — forecloses the "private helper" case TYPE_SYSTEM.md's rule 2 explicitly names, with no way to opt out); require an explicit `pycc.toml`-level allowlist (rejected — no `pycc.toml` support exists yet in v0.1, per CLI_SPEC.md's own `[project]`/`[build]` sections being unimplemented; premature complexity).
- Consequences: `def _helper(x): ...` (unannotated) type-checks via inference; `def helper(x): ...` (unannotated) raises `T0001`. Revisit if a real `pycc.toml` visibility mechanism is designed later — this convention becomes the default, not a permanent rule.
```

- [x] **Step 2: Commit**

```bash
git add docs/DECISIONS.md
git commit -m "docs: record PR-4 scope decisions (D-035 through D-038)"
```

---

## Task 2: Real spans end to end (parser → diag)

**Files:**
- Modify: `crates/pycc_diag/src/lib.rs`
- Modify: `crates/pycc_parser/src/lib.rs`
- Test: inline `#[cfg(test)]` in both files

**Interfaces:**
- Consumes: nothing new from earlier tasks.
- Produces: `pycc_diag::Span { start: u32, end: u32 }` unchanged in shape (byte offsets — already correct for JSON's `col`/`len`, just never populated with real values before); a new `pycc_diag::LineCol { line: u32, column: u32 }` and `pycc_diag::byte_offset_to_line_col(source: &str, offset: u32) -> LineCol` function every later renderer/test uses to turn a byte offset into a 1-indexed line/column pair for the human-format output.

- [x] **Step 1: Write the failing test for `byte_offset_to_line_col`**

Add to `crates/pycc_diag/src/lib.rs`'s existing `#[cfg(test)] mod tests`:

```rust
#[test]
fn byte_offset_to_line_col_finds_first_line_first_column() {
    assert_eq!(byte_offset_to_line_col("print(42)\n", 0), LineCol { line: 1, column: 1 });
}

#[test]
fn byte_offset_to_line_col_finds_a_later_line() {
    let source = "def f():\n    print(42)\n";
    // offset 13 is the 'p' in "print", on line 2, column 5 (1-indexed, after 4 spaces)
    assert_eq!(byte_offset_to_line_col(source, 13), LineCol { line: 2, column: 5 });
}

#[test]
fn byte_offset_to_line_col_at_a_newline_byte_stays_on_the_line_it_ends() {
    let source = "ab\ncd";
    // offset 2 is the '\n' itself -- still counted as the end of line 1, column 3
    assert_eq!(byte_offset_to_line_col(source, 2), LineCol { line: 1, column: 3 });
}
```

- [x] **Step 2: Run to verify it fails**

Run: `cargo test -p pycc_diag byte_offset_to_line_col`
Expected: FAIL with `cannot find function 'byte_offset_to_line_col'` / `cannot find type 'LineCol'`.

- [x] **Step 3: Implement `LineCol` and `byte_offset_to_line_col`**

Add to `crates/pycc_diag/src/lib.rs` (near the existing `Span` definition):

```rust
/// 1-indexed line and column, computed from a byte offset into `source` --
/// CLI_SPEC.md's human format shows `src/main.py:5:15` (1-indexed line:col),
/// and the JSON format's `spans[{line,col,...}]` needs the same. Hand-rolled
/// rather than pulling in `ruff_source_file`'s `LineIndex` (a separate crate
/// this workspace doesn't otherwise depend on) for one small function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineCol {
    pub line: u32,
    pub column: u32,
}

pub fn byte_offset_to_line_col(source: &str, offset: u32) -> LineCol {
    let offset = offset as usize;
    let mut line = 1u32;
    let mut last_newline_end = 0usize;
    for (i, b) in source.bytes().enumerate() {
        if i >= offset {
            break;
        }
        if b == b'\n' {
            line += 1;
            last_newline_end = i + 1;
        }
    }
    let column = (offset.saturating_sub(last_newline_end)) as u32 + 1;
    LineCol { line, column }
}
```

- [x] **Step 4: Run to verify it passes**

Run: `cargo test -p pycc_diag byte_offset_to_line_col`
Expected: PASS (3 tests).

- [x] **Step 5: Write the failing test proving the parser threads a real span**

Add to `crates/pycc_parser/src/lib.rs`'s existing tests:

```rust
#[test]
fn syntax_error_carries_the_real_byte_span_not_a_placeholder() {
    let err = parse("def main(:\n").unwrap_err();
    // "def main(:\n" -- ruff's parser fails at the malformed parameter list;
    // this must no longer be Span::new(0, 0) for every input regardless of
    // where the error actually is.
    assert_ne!(err.span, Some(pycc_diag::Span::new(0, 0)));
}
```

- [x] **Step 6: Run to verify it fails**

Run: `cargo test -p pycc_parser syntax_error_carries_the_real_byte_span`
Expected: FAIL (current code always produces `Span::new(0, 0)`).

- [x] **Step 7: Thread the real span from ruff's error**

Modify `crates/pycc_parser/src/lib.rs`'s `parse` function. Ruff's `ParseError` (returned inside the `Err` ruff hands back) carries a `location: TextRange` field with `.start()`/`.end()` methods returning `TextSize` (convertible to `u32` via `.to_u32()` or `Into<u32>` — confirm the exact accessor by checking `ruff_python_parser::ParseError`'s definition at `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/ruff_python_parser-0.0.6/src/error.rs` before writing this; it is `pub struct ParseError { pub error: ParseErrorType, pub location: TextRange }`, and `TextRange` (from `ruff_text_size`, a transitive dependency already in `Cargo.lock`) has `.start() -> TextSize` / `.end() -> TextSize`, and `TextSize` implements `From<TextSize> for u32`). Replace:

```rust
Err(e) => Err(Diagnostic::error("L0001", e.to_string(), Span::new(0, 0))),
```

with:

```rust
Err(e) => {
    let span = Span::new(e.location.start().to_u32(), e.location.end().to_u32());
    Err(Diagnostic::error("L0001", e.to_string(), span))
}
```

- [x] **Step 8: Run to verify it passes**

Run: `cargo test -p pycc_parser -p pycc_diag`
Expected: PASS, all tests including the new one.

- [x] **Step 9: Run the full existing suite to confirm nothing regressed**

Run: `cargo test --workspace`
Expected: PASS — `tests/slice0.rs`'s `a_syntax_error_is_a_compile_error_exit_code_1` only checks the diagnostic *code* appears in stderr (`contains("L0001")`), not the exact span text, so it is unaffected by this change.

- [x] **Step 10: Commit**

```bash
git add crates/pycc_diag/src/lib.rs crates/pycc_parser/src/lib.rs
git commit -m "feat(pycc_diag,pycc_parser): thread real byte spans, add line/col conversion"
```

---

## Task 3: Diagnostic renderers (human format + JSON format)

**Files:**
- Modify: `crates/pycc_diag/src/lib.rs`
- Test: inline

**Interfaces:**
- Consumes: `Diagnostic { code, severity, message, span: Option<Span> }`, `byte_offset_to_line_col` (Task 2).
- Produces: `pycc_diag::render_human(diag: &Diagnostic, file_path: &str, source: &str) -> String` and `pycc_diag::render_json(diag: &Diagnostic, file_path: &str, source: &str) -> String` — every later diagnostic-emitting call site (T0001, T0002, `pycc check`) renders through these instead of hand-formatting `eprintln!` text, so all diagnostics look identical regardless of which crate raised them.

- [x] **Step 1: Write the failing test for `render_human`**

```rust
#[test]
fn render_human_matches_cli_spec_format() {
    let source = "def fib(n):\n    print(fib(\"35\"))\n";
    let diag = Diagnostic::error(
        "T0021",
        "argument 1 of `fib` expects `int`, got `str`".to_string(),
        Span::new(24, 28), // the "\"35\"" token, byte-verified against `source` above
    );
    let rendered = render_human(&diag, "src/main.py", source);
    let expected = "\
error[T0021]: argument 1 of `fib` expects `int`, got `str`
 --> src/main.py:2:16
  |
2 |     print(fib(\"35\"))
  |                ^^^^
";
    assert_eq!(rendered, expected);
}
```

(Verify the byte offsets 24/28 and column 16 actually match that exact source string before trusting this test — count bytes by hand or with a quick `python3 -c "s='def fib(n):\n    print(fib(\"35\"))\n'; print(s[24:28])"` to confirm it slices out `"35"` before running the test, since a wrong offset here would make the test self-consistently wrong.)

- [x] **Step 2: Run to verify it fails**

Run: `cargo test -p pycc_diag render_human_matches_cli_spec_format`
Expected: FAIL with `cannot find function 'render_human'`.

- [x] **Step 3: Implement `render_human`**

```rust
/// CLI_SPEC.md's human diagnostic format, reproduced byte-for-byte:
/// `error[CODE]: message` / ` --> file:line:col` / a blank gutter line / the
/// source line prefixed with its line number / a caret-underline beneath the
/// span. `help:` lines are added by callers that have one to add (Task 5+);
/// this function only renders the primary error + location, matching every
/// code path that doesn't yet have a suggestion to attach.
pub fn render_human(diag: &Diagnostic, file_path: &str, source: &str) -> String {
    let mut out = String::new();
    let severity_word = match diag.severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
    };
    out.push_str(&format!("{severity_word}[{}]: {}\n", diag.code, diag.message));
    let Some(span) = diag.span else {
        return out;
    };
    let start = byte_offset_to_line_col(source, span.start);
    let end = byte_offset_to_line_col(source, span.end);
    out.push_str(&format!(" --> {file_path}:{}:{}\n", start.line, start.column));
    let line_num_width = start.line.to_string().len();
    out.push_str(&" ".repeat(line_num_width));
    out.push_str("  |\n");
    let source_line = source.lines().nth((start.line - 1) as usize).unwrap_or("");
    out.push_str(&format!("{} | {source_line}\n", start.line));
    out.push_str(&" ".repeat(line_num_width));
    out.push_str("  | ");
    out.push_str(&" ".repeat((start.column - 1) as usize));
    let caret_len = if end.line == start.line {
        (end.column.saturating_sub(start.column)).max(1) as usize
    } else {
        1
    };
    out.push_str(&"^".repeat(caret_len));
    out.push('\n');
    out
}
```

- [x] **Step 4: Run to verify it passes**

Run: `cargo test -p pycc_diag render_human_matches_cli_spec_format`
Expected: PASS. If it fails on exact whitespace, print both strings with `{:?}` to see the byte-for-byte diff and fix the format string, not the test.

- [x] **Step 5: Write the failing test for `render_json`**

```rust
#[test]
fn render_json_matches_the_versioned_schema() {
    let source = "print(1)\n";
    let diag = Diagnostic::error("T0001", "missing annotation".to_string(), Span::new(0, 5));
    let rendered = render_json(&diag, "src/main.py", source);
    let parsed: serde_json::Value = serde_json::from_str(&rendered).unwrap();
    assert_eq!(parsed["format_version"], 1);
    assert_eq!(parsed["code"], "T0001");
    assert_eq!(parsed["severity"], "error");
    assert_eq!(parsed["message"], "missing annotation");
    assert_eq!(parsed["spans"][0]["file"], "src/main.py");
    assert_eq!(parsed["spans"][0]["line"], 1);
    assert_eq!(parsed["spans"][0]["col"], 1);
    assert_eq!(parsed["spans"][0]["len"], 5);
}
```

- [x] **Step 6: Add `serde`/`serde_json` to `pycc_diag`'s `Cargo.toml`**

```toml
[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

Run: `curl -sA "pycc-build/0.1" https://crates.io/api/v1/crates/serde | python3 -c "import json,sys;print(json.load(sys.stdin)['crate']['newest_version'])"` (and the same for `serde_json`) first, and pin whatever comes back instead of a bare `"1"` if a specific patch matters — `"1"` (major-only) is acceptable here since `serde`/`serde_json` follow strict semver and this is a first-time addition, not a version already pinned elsewhere in the workspace.

- [x] **Step 7: Run to verify the JSON test fails**

Run: `cargo test -p pycc_diag render_json_matches_the_versioned_schema`
Expected: FAIL with `cannot find function 'render_json'`.

- [x] **Step 8: Implement `render_json`**

```rust
pub fn render_json(diag: &Diagnostic, file_path: &str, source: &str) -> String {
    let severity_word = match diag.severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
    };
    let spans = if let Some(span) = diag.span {
        let start = byte_offset_to_line_col(source, span.start);
        serde_json::json!([{
            "file": file_path,
            "line": start.line,
            "col": start.column,
            "len": span.end.saturating_sub(span.start),
            "label": serde_json::Value::Null,
        }])
    } else {
        serde_json::json!([])
    };
    let value = serde_json::json!({
        "format_version": 1,
        "code": diag.code,
        "severity": severity_word,
        "message": diag.message,
        "spans": spans,
        "help": [],
    });
    value.to_string()
}
```

- [x] **Step 9: Run to verify it passes**

Run: `cargo test -p pycc_diag`
Expected: PASS, all tests in the crate.

- [x] **Step 10: Commit**

```bash
git add crates/pycc_diag/src/lib.rs crates/pycc_diag/Cargo.toml
git commit -m "feat(pycc_diag): human and JSON diagnostic renderers per CLI_SPEC.md"
```

---

## Task 4: Grow `pycc_ast`'s re-exports

**Files:**
- Modify: `crates/pycc_ast/src/lib.rs`
- Test: inline (a compile-time-only test that constructs one value of each new re-exported type from a parsed fixture, proving the re-export path actually resolves)

**Interfaces:**
- Produces: every type listed below, re-exported from `pycc_ast` exactly as `ruff_python_ast` defines it (no wrapping) — Task 5 onward import these from `pycc_ast::*`, never `ruff_python_ast::*` directly, preserving the existing "thin, stable re-export boundary" pattern (D-017's stated rationale for why `pycc_ast` exists as its own crate).

**Verified against `ruff_python_ast` 0.0.6** (see the research this plan is built from — every shape below was read directly from `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/ruff_python_ast-0.0.6/src/{generated.rs,nodes.rs}`, not assumed):

- `StmtAssign { targets: Vec<Expr>, value: Box<Expr>, .. }`
- `ExprBinOp { left: Box<Expr>, op: Operator, right: Box<Expr>, .. }`, `Operator { Add, Sub, Mult, MatMult, Div, Mod, Pow, LShift, RShift, BitOr, BitXor, BitAnd, FloorDiv }` — note `Mult` not `Mul`.
- `ExprCompare { left: Box<Expr>, ops: Box<[CmpOp]>, comparators: Box<[Expr]>, .. }`, `CmpOp { Eq, NotEq, Lt, LtE, Gt, GtE, Is, IsNot, In, NotIn }`.
- `StmtIf { test: Box<Expr>, body: ThinVec<Stmt>, elif_else_clauses: Vec<ElifElseClause>, .. }`, `ElifElseClause { test: Option<Expr>, body: Suite, .. }` (a `None` test is the trailing `else:`).
- `StmtWhile { test: Box<Expr>, body: ThinVec<Stmt>, orelse: ThinVec<Stmt>, .. }`.
- `StmtFor { is_async: bool, target: Box<Expr>, iter: Box<Expr>, body: ThinVec<Stmt>, orelse: ThinVec<Stmt>, .. }`.
- `ExprBooleanLiteral { value: bool, .. }`.
- `ExprStringLiteral { value: StringLiteralValue, .. }`, `StringLiteralValue` (use its `.to_str()` method).
- `ExprFString { value: FStringValue, .. }`, `FStringValue` (use its `.elements()` method), `InterpolatedStringElement { Interpolation(InterpolatedElement), Literal(InterpolatedStringLiteralElement) }`, `InterpolatedElement { expression: Box<Expr>, .. }`, `InterpolatedStringLiteralElement { value: Box<str>, .. }`.
- `Number { Int(int::Int), Float(f64), Complex { real: f64, imag: f64 } }` (already partly re-exported; confirm `Float` variant is reachable).
- `ExprUnaryOp { op: UnaryOp, operand: Box<Expr>, .. }`, `UnaryOp { Invert, Not, UAdd, USub }`.
- `ExprContext { Load, Store, Del, Invalid }` (distinguishes an `ExprName` used as a value vs. an assignment target).

- [x] **Step 1: Write the failing compile-check test**

```rust
#[test]
fn re_exported_grammar_types_resolve_and_have_the_expected_shape() {
    let module = pycc_parser_test_helper_parse(
        "x = 1\ny = x + 2\nif y == 3:\n    pass\nelif True:\n    pass\nelse:\n    pass\nwhile y < 10:\n    pass\nfor i in range(3):\n    pass\nz = -y\ns = \"hi\"\nf = f\"{y}\"\n",
    );
    // This test only needs to compile and run without panicking -- it exists
    // to prove every re-exported type name/field below actually resolves
    // against the pinned ruff_python_ast 0.0.6, catching a typo'd re-export
    // at test time rather than at every downstream crate's build.
    assert!(!module.body.is_empty());
}

fn pycc_parser_test_helper_parse(source: &str) -> ModModule {
    ruff_python_parser::parse_module(source).unwrap().into_syntax()
}
```

(This helper duplicates one line of `pycc_parser`'s own logic rather than depending on that crate from `pycc_ast`, since `pycc_ast` sits *below* `pycc_parser` in the dependency graph — `pycc_parser` depends on `pycc_ast`, not the reverse. `ruff_python_parser` becomes a `[dev-dependencies]`-only addition to `pycc_ast/Cargo.toml` for this test.)

- [x] **Step 2: Add the dev-dependency**

In `crates/pycc_ast/Cargo.toml`:

```toml
[dev-dependencies]
ruff_python_parser = "0.0.6"
```

- [x] **Step 3: Run to verify it fails**

Run: `cargo test -p pycc_ast re_exported_grammar_types_resolve`
Expected: FAIL to compile (none of the new types are re-exported yet).

- [x] **Step 4: Add the re-exports**

Replace `crates/pycc_ast/src/lib.rs`'s existing `pub use` line with the widened list (keep every currently-re-exported name, add the new ones):

```rust
pub use ruff_python_ast::{
    Arguments, CmpOp, ElifElseClause, Expr, ExprBinOp, ExprBooleanLiteral, ExprCall, ExprCompare,
    ExprContext, ExprFString, ExprName, ExprNumberLiteral, ExprStringLiteral, ExprUnaryOp,
    Identifier, InterpolatedElement, InterpolatedStringElement, InterpolatedStringLiteralElement,
    ModModule, Number, Operator, Parameters, Stmt, StmtAssign, StmtExpr, StmtFor,
    StmtFunctionDef, StmtIf, StmtReturn, StmtWhile, UnaryOp,
};
```

- [x] **Step 5: Run to verify it passes**

Run: `cargo test -p pycc_ast`
Expected: PASS.

- [x] **Step 6: Run the full workspace test suite**

Run: `cargo test --workspace`
Expected: PASS — this task only widens re-exports, nothing downstream consumes them yet.

- [x] **Step 7: Commit**

```bash
git add crates/pycc_ast/src/lib.rs crates/pycc_ast/Cargo.toml
git commit -m "feat(pycc_ast): re-export the full v0.1 grammar's AST node types"
```

---

## Task 5: `pycc_hir` grows a real expression tree; `pycc_mir` mechanical update

This is the structural task every later vertical slice depends on — it does not yet add any *new* grammar support beyond what already works, it only reshapes the existing two constructs (`print(<int literal>)`, zero-arg function calls) into the new general shape, proving the refactor is behavior-preserving before any new feature is layered on.

**Files:**
- Modify: `crates/pycc_hir/src/lib.rs`
- Modify: `crates/pycc_mir/src/lib.rs`
- Test: inline in both

**Interfaces:**
- Produces (from `pycc_hir`):
  ```rust
  pub enum HirExpr {
      IntLiteral(i64),
      Name(String),
      Call { callee: String, args: Vec<HirExpr> },
  }
  pub enum HirStmt {
      ExprStmt(HirExpr),
  }
  pub enum HirItem {
      Function { name: String, body: Vec<HirStmt> },
      TopLevelStmt(HirStmt),
  }
  pub struct HirModule { pub items: Vec<HirItem> }
  pub fn lower(module: &ModModule) -> HirModule
  ```
  (`CallPrint`/`CallUserFunction` are gone — both are now `HirExpr::Call { callee, args }`, with `print` no longer special-cased at the HIR level; that distinction moves to `pycc_types`/`pycc_mir` as needed.)
- Produces (from `pycc_mir`, mechanically updated to match):
  ```rust
  pub enum MirExpr { IntLiteral(i64) }
  pub enum MirInstr { CallPrint { arg: MirExpr }, CallUserFunction { name: String } }
  pub enum MirItem { Function { name: String, body: Vec<MirInstr> }, TopLevelStmt(MirInstr) }
  pub struct MirModule { pub items: Vec<MirItem> }
  pub fn build(hir: &HirModule) -> MirModule
  ```

- [x] **Step 1: Write the failing tests proving the new shape still handles the old two cases**

Replace `crates/pycc_hir/src/lib.rs`'s existing tests with (same assertions, new shape):

```rust
#[test]
fn lowers_top_level_print_with_no_main() {
    let module = parse_test_source("print(42)\n");
    let hir = lower(&module);
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::IntLiteral(42)],
        }))]
    );
}

#[test]
fn lowers_a_call_to_a_user_defined_function() {
    let module = parse_test_source("def helper():\n    print(1)\nhelper()\n");
    let hir = lower(&module);
    assert_eq!(hir.items.len(), 2);
    assert!(matches!(
        hir.items[1],
        HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call { ref callee, ref args }))
            if callee == "helper" && args.is_empty()
    ));
}

#[test]
fn lowers_a_function_definition_without_calling_it() {
    let module = parse_test_source("def helper():\n    print(1)\n");
    let hir = lower(&module);
    assert_eq!(hir.items.len(), 1);
    assert!(matches!(hir.items[0], HirItem::Function { ref name, .. } if name == "helper"));
}

#[test]
fn calling_a_zero_arg_function_other_than_print_is_supported() {
    let module = parse_test_source("def helper():\n    print(1)\nhelper()\n");
    let hir = lower(&module);
    // covered by lowers_a_call_to_a_user_defined_function above; this name
    // is kept because an existing test of this exact name already exists
    // and downstream tooling may reference it by name -- verify no such
    // reference exists before deleting rather than renaming duplicates.
}

fn parse_test_source(source: &str) -> ModModule {
    ruff_python_parser::parse_module(source).unwrap().into_syntax()
}
```

(Add `ruff_python_parser = "0.0.6"` under `[dev-dependencies]` in `crates/pycc_hir/Cargo.toml` if not already present — check first with `grep ruff_python_parser crates/pycc_hir/Cargo.toml`.)

The four existing "unsupported" panic tests (`non_call_expression_statement_is_unsupported`, `non_name_callee_is_unsupported`, `calling_a_non_print_function_with_arguments_is_unsupported`, `print_with_wrong_argument_count_is_unsupported`, `print_with_an_integer_too_large_for_i64_is_unsupported`, `print_with_a_float_argument_is_unsupported`, `non_expr_statement_is_unsupported`) are **deleted** in this task, not kept: the whole point of Tasks 6+ is to make several of these no longer panic (a non-`print` call *with* arguments becomes legal in Task 8's function-arguments slice; a float argument to `print` becomes legal once `HirExpr` gains a float variant in Task 6). Keeping stale "this panics" tests that Task 6/8 then have to delete anyway just adds churn — delete them now, and each later task adds its own precise test for the new, non-panicking behavior it introduces.

- [x] **Step 2: Run to verify the new tests fail**

Run: `cargo test -p pycc_hir`
Expected: FAIL to compile (`HirExpr`, new `HirStmt` shape don't exist yet).

- [x] **Step 3: Implement the new `HirExpr`/`HirStmt`/`lower`**

Replace `crates/pycc_hir/src/lib.rs`'s type definitions and `lower`/`lower_stmt` functions:

```rust
use pycc_ast::{Expr, ModModule, Stmt, StmtExpr, StmtFunctionDef};

#[derive(Debug, Clone, PartialEq)]
pub enum HirExpr {
    IntLiteral(i64),
    Name(String),
    Call { callee: String, args: Vec<HirExpr> },
}

#[derive(Debug, Clone, PartialEq)]
pub enum HirStmt {
    ExprStmt(HirExpr),
}

#[derive(Debug, Clone, PartialEq)]
pub enum HirItem {
    Function { name: String, body: Vec<HirStmt> },
    TopLevelStmt(HirStmt),
}

#[derive(Debug, Clone, PartialEq)]
pub struct HirModule {
    pub items: Vec<HirItem>,
}

pub fn lower(module: &ModModule) -> HirModule {
    let items = module
        .body
        .iter()
        .map(|stmt| match stmt {
            Stmt::FunctionDef(StmtFunctionDef { name, body, .. }) => HirItem::Function {
                name: name.to_string(),
                body: body.iter().map(lower_stmt).collect(),
            },
            other => HirItem::TopLevelStmt(lower_stmt(other)),
        })
        .collect();
    HirModule { items }
}

fn lower_stmt(stmt: &Stmt) -> HirStmt {
    match stmt {
        Stmt::Expr(StmtExpr { value, .. }) => HirStmt::ExprStmt(lower_expr(value)),
        other => panic!("pycc_hir: statement kind not supported yet: {other:?}"),
    }
}

fn lower_expr(expr: &Expr) -> HirExpr {
    match expr {
        Expr::NumberLiteral(n) => match &n.value {
            pycc_ast::Number::Int(i) => HirExpr::IntLiteral(
                i.as_i64()
                    .unwrap_or_else(|| panic!("pycc_hir: integer literal does not fit in i64: {i:?}")),
            ),
            other => panic!("pycc_hir: numeric literal kind not supported yet: {other:?}"),
        },
        Expr::Name(name) => HirExpr::Name(name.id.to_string()),
        Expr::Call(call) => {
            let Expr::Name(callee) = call.func.as_ref() else {
                panic!("pycc_hir: only calling a bare name is supported so far: {:?}", call.func);
            };
            let args = call.arguments.args.iter().map(lower_expr).collect();
            HirExpr::Call { callee: callee.id.to_string(), args }
        }
        other => panic!("pycc_hir: expression kind not supported yet: {other:?}"),
    }
}
```

`i.as_i64()`: confirm this exact method name exists on `ruff_python_ast::int::Int` by checking `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/ruff_python_ast-0.0.6/src/int.rs` before running — if the real accessor has a different name (e.g. `as_int()`, `to_i64()`), use that instead; the existing pre-Task-5 code already extracted an `i64` from this exact type somehow (check `git log -p -- crates/pycc_hir/src/lib.rs` for the original `lower_stmt`'s int-literal-extraction line and reuse its exact method call).

- [x] **Step 4: Run `pycc_hir`'s tests**

Run: `cargo test -p pycc_hir`
Expected: PASS.

- [x] **Step 5: Update `pycc_mir` to match, mechanically**

Replace `crates/pycc_mir/src/lib.rs`'s types and `build`/`lower_instr`:

```rust
use pycc_hir::{HirExpr, HirItem, HirModule, HirStmt};

#[derive(Debug, Clone, PartialEq)]
pub enum MirExpr {
    IntLiteral(i64),
}

#[derive(Debug, Clone, PartialEq)]
pub enum MirInstr {
    CallPrint { arg: MirExpr },
    CallUserFunction { name: String },
}

#[derive(Debug, Clone, PartialEq)]
pub enum MirItem {
    Function { name: String, body: Vec<MirInstr> },
    TopLevelStmt(MirInstr),
}

#[derive(Debug, Clone, PartialEq)]
pub struct MirModule {
    pub items: Vec<MirItem>,
}

pub fn build(hir: &HirModule) -> MirModule {
    let items = hir
        .items
        .iter()
        .map(|item| match item {
            HirItem::Function { name, body } => MirItem::Function {
                name: name.clone(),
                body: body.iter().map(lower_instr).collect(),
            },
            HirItem::TopLevelStmt(stmt) => MirItem::TopLevelStmt(lower_instr(stmt)),
        })
        .collect();
    MirModule { items }
}

/// Only the two constructs pycc_codegen can already emit LLVM IR for are
/// lowered here -- everything HIR's wider grammar (Tasks 6-11) can now
/// represent but this crate can't yet compile panics with a message naming
/// PR-5 explicitly (D-035): a deliberate, temporary boundary, not new
/// codegen work landing in this PR.
fn lower_instr(stmt: &HirStmt) -> MirInstr {
    let HirStmt::ExprStmt(expr) = stmt;
    match expr {
        HirExpr::Call { callee, args } if callee == "print" => match args.as_slice() {
            [HirExpr::IntLiteral(n)] => MirInstr::CallPrint { arg: MirExpr::IntLiteral(*n) },
            _ => panic!("pycc_mir: print() with this argument shape lands in PR-5"),
        },
        HirExpr::Call { callee, args } if args.is_empty() => {
            MirInstr::CallUserFunction { name: callee.clone() }
        }
        _ => panic!("pycc_mir: this construct's codegen lands in PR-5"),
    }
}
```

- [x] **Step 6: Update `pycc_mir`'s existing tests to the new `MirExpr` shape**

Every existing test's `MirInstr::CallPrint { arg: 42 }`-style construction becomes `MirInstr::CallPrint { arg: MirExpr::IntLiteral(42) }`. Run `grep -n "CallPrint { arg:" crates/pycc_mir/src/lib.rs crates/pycc_codegen/src/lib.rs` first to find every call site needing this mechanical edit (both crates' tests construct `MirInstr`/`MirModule` values directly).

- [x] **Step 7: Update `pycc_codegen` to match the new `MirExpr` shape**

`crates/pycc_codegen/src/lib.rs`'s `emit_instr` currently does `i64_type.const_int(*arg as u64, true)` for a `MirInstr::CallPrint { arg }` where `arg: i64`. Update the match to destructure the new shape:

```rust
MirInstr::CallPrint { arg: MirExpr::IntLiteral(n) } => {
    let arg_value = i64_type.const_int(*n as u64, true);
    builder
        .build_call(print_fn, &[arg_value.into()], "call_print")
        .expect("build_call should not fail for a well-formed print call");
    Ok(())
}
```

(Add `use pycc_mir::MirExpr;` to the imports at the top of the file alongside the existing `pycc_mir::{MirInstr, MirItem, MirModule}` import.)

- [x] **Step 8: Run the full workspace test suite**

Run: `cargo test --workspace`
Expected: PASS, all 16 `tests/slice0.rs` CLI tests plus every crate's unit tests — this task changes internal shapes only, never observable CLI behavior.

- [x] **Step 9: Run clippy and the coverage gate**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo build --workspace && cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`
Expected: PASS. Pay particular attention to `pycc_mir`'s new panic arms and `pycc_hir`'s new panic arms — each needs a test that actually triggers it (`#[should_panic]`) to stay covered; port the old `#[should_panic]` tests forward for whichever panic messages still exist after Step 1's deletions (e.g. keep a `calling_a_non_name_callee_is_unsupported`-equivalent test for `Expr::Call` whose `func` isn't `Expr::Name`, since nothing in Tasks 6-11 makes that legal).

- [x] **Step 10: Commit**

```bash
git add crates/pycc_hir/src/lib.rs crates/pycc_hir/Cargo.toml crates/pycc_mir/src/lib.rs crates/pycc_codegen/src/lib.rs
git commit -m "refactor(pycc_hir,pycc_mir): general HirExpr/MirExpr tree, behavior-preserving (D-035)"
```

---

## Task 6: Assignment, local variables, and arithmetic (frontend only)

**Files:**
- Modify: `crates/pycc_hir/src/lib.rs`
- Modify: `crates/pycc_types/src/lib.rs`
- Test: inline in both

**Interfaces:**
- Consumes: `HirExpr`/`HirStmt` (Task 5).
- Produces: `HirExpr` gains `FloatLiteral(f64)`, `BinOp { op: BinOpKind, left: Box<HirExpr>, right: Box<HirExpr> }`, `BinOpKind { Add, Sub, Mul, Div, FloorDiv, Mod, Pow }` (a pycc-owned enum, not `ruff_python_ast::Operator` directly — HIR should not leak the parser's exact vocabulary, e.g. `Mult`→`Mul` renamed to the more common spelling now that it's ours to name). `HirStmt` gains `Assign { target: String, value: HirExpr }`. `pycc_types` gains a real `Ty` enum (`Int, Float, Bool, Str, None`), an `Environment` (a `HashMap<String, Ty>` scoped per function/module), and `infer_expr(env: &Environment, expr: &HirExpr) -> Result<Ty, Diagnostic>` / `check_stmt(env: &mut Environment, stmt: &HirStmt) -> Result<(), Diagnostic>`.

- [x] **Step 1: Write the failing HIR test for assignment and arithmetic**

```rust
#[test]
fn lowers_an_assignment_and_a_later_reference_to_it() {
    let module = parse_test_source("x = 1\nprint(x)\n");
    let hir = lower(&module);
    assert_eq!(
        hir.items,
        vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            }),
            HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::Name("x".to_string())],
            })),
        ]
    );
}

#[test]
fn lowers_a_binary_addition() {
    let module = parse_test_source("x = 1 + 2\n");
    let hir = lower(&module);
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::Assign {
            target: "x".to_string(),
            value: HirExpr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(HirExpr::IntLiteral(1)),
                right: Box::new(HirExpr::IntLiteral(2)),
            },
        })]
    );
}

#[test]
fn lowers_a_float_literal() {
    let module = parse_test_source("x = 1.5\n");
    let hir = lower(&module);
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::Assign {
            target: "x".to_string(),
            value: HirExpr::FloatLiteral(1.5),
        })]
    );
}
```

- [x] **Step 2: Run to verify these fail**

Run: `cargo test -p pycc_hir lowers_an_assignment_and_a_later_reference lowers_a_binary_addition lowers_a_float_literal`
Expected: FAIL to compile (`HirStmt::Assign`, `HirExpr::BinOp`/`FloatLiteral`, `BinOpKind` don't exist yet).

- [x] **Step 3: Add `BinOpKind`, extend `HirExpr`/`HirStmt`, extend `lower_expr`/`lower_stmt`**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOpKind {
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv,
    Mod,
    Pow,
}

// HirExpr gains:
//     FloatLiteral(f64),
//     BinOp { op: BinOpKind, left: Box<HirExpr>, right: Box<HirExpr> },

// HirStmt gains:
//     Assign { target: String, value: HirExpr },
```

In `lower_stmt`, add a case before the final `other => panic!` arm:

```rust
Stmt::Assign(pycc_ast::StmtAssign { targets, value, .. }) => {
    let [target] = targets.as_slice() else {
        panic!("pycc_hir: only a single assignment target is supported so far: {targets:?}");
    };
    let pycc_ast::Expr::Name(name) = target else {
        panic!("pycc_hir: only assigning to a bare name is supported so far: {target:?}");
    };
    HirStmt::Assign { target: name.id.to_string(), value: lower_expr(value) }
}
```

In `lower_expr`, add before the final `other => panic!` arm:

```rust
Expr::NumberLiteral(n) => match &n.value {
    pycc_ast::Number::Int(i) => HirExpr::IntLiteral(/* same as Task 5 */ i.as_i64().unwrap_or_else(|| panic!("pycc_hir: integer literal does not fit in i64: {i:?}"))),
    pycc_ast::Number::Float(f) => HirExpr::FloatLiteral(*f),
    other => panic!("pycc_hir: numeric literal kind not supported yet: {other:?}"),
},
Expr::BinOp(pycc_ast::ExprBinOp { left, op, right, .. }) => {
    let op = match op {
        pycc_ast::Operator::Add => BinOpKind::Add,
        pycc_ast::Operator::Sub => BinOpKind::Sub,
        pycc_ast::Operator::Mult => BinOpKind::Mul,
        pycc_ast::Operator::Div => BinOpKind::Div,
        pycc_ast::Operator::FloorDiv => BinOpKind::FloorDiv,
        pycc_ast::Operator::Mod => BinOpKind::Mod,
        pycc_ast::Operator::Pow => BinOpKind::Pow,
        other => panic!("pycc_hir: binary operator not supported yet: {other:?}"),
    };
    HirExpr::BinOp { op, left: Box::new(lower_expr(left)), right: Box::new(lower_expr(right)) }
}
```

(Note this *replaces* Task 5's `Expr::NumberLiteral` arm with the widened version shown here — don't leave two arms for the same pattern.)

- [x] **Step 4: Run to verify HIR tests pass**

Run: `cargo test -p pycc_hir`
Expected: PASS.

- [x] **Step 5: Write the failing type-check tests**

```rust
#[test]
fn infers_an_int_literal_as_int() {
    let env = Environment::new();
    assert_eq!(infer_expr(&env, &HirExpr::IntLiteral(1)), Ok(Ty::Int));
}

#[test]
fn infers_a_float_literal_as_float() {
    let env = Environment::new();
    assert_eq!(infer_expr(&env, &HirExpr::FloatLiteral(1.5)), Ok(Ty::Float));
}

#[test]
fn an_assignment_binds_the_inferred_type_in_the_environment() {
    let mut env = Environment::new();
    check_stmt(&mut env, &HirStmt::Assign { target: "x".to_string(), value: HirExpr::IntLiteral(1) }).unwrap();
    assert_eq!(env.lookup("x"), Some(Ty::Int));
}

#[test]
fn referencing_an_assigned_name_infers_its_bound_type() {
    let mut env = Environment::new();
    check_stmt(&mut env, &HirStmt::Assign { target: "x".to_string(), value: HirExpr::IntLiteral(1) }).unwrap();
    assert_eq!(infer_expr(&env, &HirExpr::Name("x".to_string())), Ok(Ty::Int));
}

#[test]
fn adding_two_ints_infers_int() {
    let env = Environment::new();
    let expr = HirExpr::BinOp {
        op: BinOpKind::Add,
        left: Box::new(HirExpr::IntLiteral(1)),
        right: Box::new(HirExpr::IntLiteral(2)),
    };
    assert_eq!(infer_expr(&env, &expr), Ok(Ty::Int));
}

#[test]
fn adding_an_int_and_a_str_is_a_type_error() {
    // no str literal HIR node exists until Task 10 -- this test moves there
    // once HirExpr::StringLiteral exists; skip it in this task's commit.
}

#[test]
fn referencing_an_undefined_name_is_a_clean_error_not_a_panic() {
    let env = Environment::new();
    let err = infer_expr(&env, &HirExpr::Name("undefined".to_string())).unwrap_err();
    assert_eq!(err.code, "T0021");
    assert!(err.message.contains("undefined"));
}
```

(Delete the placeholder `adding_an_int_and_a_str_is_a_type_error` stub before committing — it's a forward note for Task 10, not a real test; leaving an empty `#[test] fn` with no body would itself be a coverage/lint issue. Move the *idea* into Task 10's own step instead.)

- [x] **Step 6: Run to verify these fail**

Run: `cargo test -p pycc_types`
Expected: FAIL to compile (`Ty`, `Environment`, `infer_expr`, `check_stmt` don't exist yet).

- [x] **Step 7: Implement `Ty`, `Environment`, `infer_expr`, `check_stmt`**

Replace `crates/pycc_types/src/lib.rs`'s content (keeping the crate's existing `pub fn check(hir: &HirModule) -> Result<(), Diagnostic>` entry point, now implemented for real rather than a no-op — Task 8 wires the "must be annotated" T0001 check into it; this task only builds the inference core it will call):

```rust
use pycc_diag::{Diagnostic, Span};
use pycc_hir::{BinOpKind, HirExpr, HirStmt};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ty {
    Int,
    Float,
    Bool,
    Str,
    None,
}

impl Ty {
    fn name(self) -> &'static str {
        match self {
            Ty::Int => "int",
            Ty::Float => "float",
            Ty::Bool => "bool",
            Ty::Str => "str",
            Ty::None => "None",
        }
    }
}

#[derive(Debug, Default)]
pub struct Environment {
    bindings: HashMap<String, Ty>,
}

impl Environment {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn lookup(&self, name: &str) -> Option<Ty> {
        self.bindings.get(name).copied()
    }

    pub fn bind(&mut self, name: String, ty: Ty) {
        self.bindings.insert(name, ty);
    }
}

pub fn infer_expr(env: &Environment, expr: &HirExpr) -> Result<Ty, Diagnostic> {
    match expr {
        HirExpr::IntLiteral(_) => Ok(Ty::Int),
        HirExpr::FloatLiteral(_) => Ok(Ty::Float),
        HirExpr::Name(name) => env.lookup(name).ok_or_else(|| {
            Diagnostic::error(
                "T0021",
                format!("name `{name}` is not defined"),
                Span::new(0, 0), // real span threading through HIR is out of scope for this task -- see Task 15's follow-up note
            )
        }),
        HirExpr::BinOp { op, left, right } => {
            let left_ty = infer_expr(env, left)?;
            let right_ty = infer_expr(env, right)?;
            numeric_result_type(*op, left_ty, right_ty)
        }
        HirExpr::Call { .. } => {
            // Call type-checking (arguments/return) lands in Task 9 alongside
            // real function signatures; until then, treat any call as
            // producing an unconstrained placeholder the caller can't yet
            // misuse, since nothing consumes a call's result type before Task 9.
            Ok(Ty::None)
        }
    }
}

fn numeric_result_type(op: BinOpKind, left: Ty, right: Ty) -> Result<Ty, Diagnostic> {
    match (left, right) {
        (Ty::Int, Ty::Int) => Ok(Ty::Int),
        (Ty::Float, Ty::Float) | (Ty::Int, Ty::Float) | (Ty::Float, Ty::Int) => Ok(Ty::Float),
        _ => Err(Diagnostic::error(
            "T0021",
            format!(
                "operator {op:?} is not defined for `{}` and `{}`",
                left.name(),
                right.name()
            ),
            Span::new(0, 0),
        )),
    }
}

pub fn check_stmt(env: &mut Environment, stmt: &HirStmt) -> Result<(), Diagnostic> {
    match stmt {
        HirStmt::Assign { target, value } => {
            let ty = infer_expr(env, value)?;
            env.bind(target.clone(), ty);
            Ok(())
        }
        HirStmt::ExprStmt(expr) => infer_expr(env, expr).map(|_| ()),
    }
}

pub fn check(hir: &pycc_hir::HirModule) -> Result<(), Diagnostic> {
    let mut env = Environment::new();
    for item in &hir.items {
        match item {
            pycc_hir::HirItem::TopLevelStmt(stmt) => check_stmt(&mut env, stmt)?,
            pycc_hir::HirItem::Function { .. } => {
                // Function-body checking (its own scope, T0001 on the
                // signature) lands in Task 9 -- until then, a function's
                // body is not yet type-checked at all, matching this crate's
                // pre-existing behavior of never failing.
            }
        }
    }
    Ok(())
}
```

`#[derive(Debug)]` on `BinOpKind` (added in Task 6's HIR step) is required for the `{op:?}` format above — confirm it's present.

- [x] **Step 8: Run to verify pycc_types tests pass**

Run: `cargo test -p pycc_types`
Expected: PASS.

- [x] **Step 9: Fix the call site in `src/main.rs`**

`try_build`'s existing line `pycc_types::check(&hir).expect("v0.1's type checker is a no-op passthrough; it never fails")` now has a real, potentially-`Err` `check` behind it. Change to:

```rust
pycc_types::check(&hir).map_err(|diag| {
    eprintln!("error[{}]: {}", diag.code, diag.message);
    ExitCode::from(1)
})?;
```

(Matches the existing pattern used two lines above for `pycc_parser::parse`'s error handling.)

- [x] **Step 10: Run the full workspace test suite**

Run: `cargo test --workspace`
Expected: PASS. Confirm specifically that `tests/slice0.rs`'s existing tests (which only use `print(<int literal>)` and zero-arg function calls, never assignment or undefined names) still pass unchanged — `check()` never rejects anything they do.

- [x] **Step 11: Run clippy and the coverage gate**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`
Expected: PASS. `numeric_result_type`'s error arm needs a test (`adding_an_int_and_a_str_is_a_type_error`'s *real* version can't exist until Task 10 adds `Ty::Str`, but `adding_an_int_and_a_bool`-style mismatches aren't real either until Task 7 adds bools — for Task 6 specifically, test the error arm via two `Ty` values that are both already real and incompatible in a way that doesn't require a new HIR node: there isn't one yet with only `Int`/`Float`, since Task 6 alone can't construct an `HirExpr` producing `Ty::Bool`/`Ty::Str`. Add a unit test calling `numeric_result_type` *directly* instead of only through `infer_expr`, to exercise the error arm before Task 7/10 make it reachable end-to-end:

```rust
#[test]
fn numeric_result_type_rejects_a_hypothetical_incompatible_pair() {
    let err = numeric_result_type(BinOpKind::Add, Ty::Int, Ty::None).unwrap_err();
    assert_eq!(err.code, "T0021");
}
```

Make `numeric_result_type` `pub(crate)` rather than private if the test module is a sibling `mod tests` in the same file (it already is, per this crate's existing pattern) — no visibility change needed, `#[cfg(test)] mod tests { use super::*; ... }` already sees private items.)

- [x] **Step 12: Commit**

```bash
git add crates/pycc_hir/src/lib.rs crates/pycc_types/src/lib.rs src/main.rs
git commit -m "feat(pycc_hir,pycc_types): assignment, local variables, arithmetic type inference"
```

---

## Task 7: Comparisons and `bool`

**Files:**
- Modify: `crates/pycc_hir/src/lib.rs`
- Modify: `crates/pycc_types/src/lib.rs`

**Interfaces:**
- Produces: `HirExpr` gains `BoolLiteral(bool)`, `Compare { op: CmpOpKind, left: Box<HirExpr>, right: Box<HirExpr> }` (single comparison only — chained comparisons like `a < b < c` are explicitly out of v0.1 scope; panic with a clear message for a chain of length > 1, per `ExprCompare.ops`/`.comparators` potentially holding more than one element). `CmpOpKind { Eq, NotEq, Lt, LtE, Gt, GtE }` (pycc-owned enum; `Is`/`IsNot`/`In`/`NotIn` are out of v0.1 scope — panic with a clear message).

- [x] **Step 1: Write the failing HIR tests**

```rust
#[test]
fn lowers_a_boolean_literal() {
    let module = parse_test_source("x = True\n");
    let hir = lower(&module);
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::Assign {
            target: "x".to_string(),
            value: HirExpr::BoolLiteral(true),
        })]
    );
}

#[test]
fn lowers_a_single_comparison() {
    let module = parse_test_source("x = 1 < 2\n");
    let hir = lower(&module);
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::Assign {
            target: "x".to_string(),
            value: HirExpr::Compare {
                op: CmpOpKind::Lt,
                left: Box::new(HirExpr::IntLiteral(1)),
                right: Box::new(HirExpr::IntLiteral(2)),
            },
        })]
    );
}

#[test]
#[should_panic(expected = "chained comparisons")]
fn a_chained_comparison_is_not_supported_yet() {
    let module = parse_test_source("x = 1 < 2 < 3\n");
    lower(&module);
}
```

- [x] **Step 2: Run to verify these fail**

Run: `cargo test -p pycc_hir lowers_a_boolean_literal lowers_a_single_comparison a_chained_comparison_is_not_supported_yet`
Expected: FAIL to compile.

- [x] **Step 3: Add `CmpOpKind`, extend `HirExpr`, extend `lower_expr`**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOpKind {
    Eq,
    NotEq,
    Lt,
    LtE,
    Gt,
    GtE,
}

// HirExpr gains:
//     BoolLiteral(bool),
//     Compare { op: CmpOpKind, left: Box<HirExpr>, right: Box<HirExpr> },
```

In `lower_expr`, add before the final `other => panic!` arm:

```rust
Expr::BooleanLiteral(pycc_ast::ExprBooleanLiteral { value, .. }) => HirExpr::BoolLiteral(*value),
Expr::Compare(pycc_ast::ExprCompare { left, ops, comparators, .. }) => {
    if ops.len() != 1 {
        panic!("pycc_hir: chained comparisons are not supported yet: {ops:?}");
    }
    let op = match ops[0] {
        pycc_ast::CmpOp::Eq => CmpOpKind::Eq,
        pycc_ast::CmpOp::NotEq => CmpOpKind::NotEq,
        pycc_ast::CmpOp::Lt => CmpOpKind::Lt,
        pycc_ast::CmpOp::LtE => CmpOpKind::LtE,
        pycc_ast::CmpOp::Gt => CmpOpKind::Gt,
        pycc_ast::CmpOp::GtE => CmpOpKind::GtE,
        other => panic!("pycc_hir: comparison operator not supported yet: {other:?}"),
    };
    HirExpr::Compare { op, left: Box::new(lower_expr(left)), right: Box::new(lower_expr(&comparators[0])) }
}
```

- [x] **Step 4: Run to verify pycc_hir tests pass**

Run: `cargo test -p pycc_hir`
Expected: PASS.

- [x] **Step 5: Write the failing pycc_types tests**

```rust
#[test]
fn infers_a_bool_literal_as_bool() {
    let env = Environment::new();
    assert_eq!(infer_expr(&env, &HirExpr::BoolLiteral(true)), Ok(Ty::Bool));
}

#[test]
fn comparing_two_ints_infers_bool() {
    let env = Environment::new();
    let expr = HirExpr::Compare {
        op: CmpOpKind::Lt,
        left: Box::new(HirExpr::IntLiteral(1)),
        right: Box::new(HirExpr::IntLiteral(2)),
    };
    assert_eq!(infer_expr(&env, &expr), Ok(Ty::Bool));
}

#[test]
fn comparing_incompatible_types_is_a_clean_type_error() {
    let env = Environment::new();
    let expr = HirExpr::Compare {
        op: CmpOpKind::Eq,
        left: Box::new(HirExpr::IntLiteral(1)),
        right: Box::new(HirExpr::BoolLiteral(true)),
    };
    // int == bool: bool IS a subtype of int per TYPE_SYSTEM.md's own
    // representation table ("bool: subtype of int") -- this must succeed,
    // not error. Test the genuinely-incompatible case in Task 10 once Str
    // exists (comparing int to str has no such subtype relationship).
}
```

(Same pattern as Task 6, Step 5: delete the placeholder-comment test before committing; its real assertion needs `Ty::Str`, which doesn't exist until Task 10. Replace it with a real assertion using what already exists: `bool == int` succeeding, since `Ty::Bool` is a subtype of `Ty::Int` per the type-representation table.)

```rust
#[test]
fn comparing_a_bool_and_an_int_succeeds_since_bool_is_a_subtype_of_int() {
    let env = Environment::new();
    let expr = HirExpr::Compare {
        op: CmpOpKind::Eq,
        left: Box::new(HirExpr::IntLiteral(1)),
        right: Box::new(HirExpr::BoolLiteral(true)),
    };
    assert_eq!(infer_expr(&env, &expr), Ok(Ty::Bool));
}
```

- [x] **Step 6: Run to verify these fail**

Run: `cargo test -p pycc_types infers_a_bool_literal comparing_two_ints comparing_a_bool_and_an_int`
Expected: FAIL to compile (`CmpOpKind` not imported/used, comparison inference doesn't exist).

- [x] **Step 7: Implement comparison inference with the bool-subtype-of-int rule**

Add to `pycc_types/src/lib.rs`'s `infer_expr` match, and add a shared subtype-compatibility helper both `numeric_result_type` and the new comparison function use:

```rust
// In infer_expr's match:
HirExpr::BoolLiteral(_) => Ok(Ty::Bool),
HirExpr::Compare { op: _, left, right } => {
    let left_ty = infer_expr(env, left)?;
    let right_ty = infer_expr(env, right)?;
    if numeric_or_bool_compatible(left_ty, right_ty) {
        Ok(Ty::Bool)
    } else {
        Err(Diagnostic::error(
            "T0021",
            format!("cannot compare `{}` and `{}`", left_ty.name(), right_ty.name()),
            Span::new(0, 0),
        ))
    }
}

fn numeric_or_bool_compatible(a: Ty, b: Ty) -> bool {
    let is_numeric_like = |t: Ty| matches!(t, Ty::Int | Ty::Float | Ty::Bool);
    is_numeric_like(a) && is_numeric_like(b)
}
```

Update `numeric_result_type` (Task 6) to also accept a `Bool` operand as an `Int` for arithmetic (`True + 1 == 2` is legal Python), widening its match:

```rust
fn numeric_result_type(op: BinOpKind, left: Ty, right: Ty) -> Result<Ty, Diagnostic> {
    let as_numeric = |t: Ty| match t {
        Ty::Bool => Some(Ty::Int),
        Ty::Int => Some(Ty::Int),
        Ty::Float => Some(Ty::Float),
        _ => None,
    };
    match (as_numeric(left), as_numeric(right)) {
        (Some(Ty::Int), Some(Ty::Int)) => Ok(Ty::Int),
        (Some(_), Some(_)) => Ok(Ty::Float), // either operand float -> float
        _ => Err(Diagnostic::error(
            "T0021",
            format!("operator {op:?} is not defined for `{}` and `{}`", left.name(), right.name()),
            Span::new(0, 0),
        )),
    }
}
```

(Re-run Task 6's `numeric_result_type_rejects_a_hypothetical_incompatible_pair` test after this change — `(BinOpKind::Add, Ty::Int, Ty::None)` still correctly errors, since `Ty::None` isn't numeric-like.)

- [x] **Step 8: Run to verify pycc_types tests pass**

Run: `cargo test -p pycc_types`
Expected: PASS.

- [x] **Step 9: Run the full workspace suite, clippy, coverage**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`
Expected: PASS. Ensure the `Is`/`IsNot`/`In`/`NotIn` panic arm and the chained-comparison panic arm each have a `#[should_panic]` test (add one for at least one of `Is`/`In` if missing — reuse the `a_chained_comparison_is_not_supported_yet` pattern).

- [x] **Step 10: Commit**

```bash
git add crates/pycc_hir/src/lib.rs crates/pycc_types/src/lib.rs
git commit -m "feat(pycc_hir,pycc_types): comparisons and bool, with bool-is-subtype-of-int arithmetic"
```

---

## Task 8: `if`/`while`, and `for`+`range`

**Files:**
- Modify: `crates/pycc_hir/src/lib.rs`
- Modify: `crates/pycc_types/src/lib.rs`

**Interfaces:**
- Produces: `HirStmt` gains `If { test: HirExpr, body: Vec<HirStmt>, orelse: Vec<HirStmt> }`, `While { test: HirExpr, body: Vec<HirStmt> }`, `ForRange { var: String, start: HirExpr, stop: HirExpr, step: HirExpr, body: Vec<HirStmt> }` (only `for x in range(...)` is in v0.1 scope per DELIVERY_PLAN.md's own TDD sequence "if/while/for+range" — iterating any other iterable panics with a clear message).

- [x] **Step 1: Write the failing HIR tests**

```rust
#[test]
fn lowers_an_if_with_no_else() {
    let module = parse_test_source("if True:\n    print(1)\n");
    let hir = lower(&module);
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::If {
            test: HirExpr::BoolLiteral(true),
            body: vec![HirStmt::ExprStmt(HirExpr::Call { callee: "print".to_string(), args: vec![HirExpr::IntLiteral(1)] })],
            orelse: vec![],
        })]
    );
}

#[test]
fn lowers_an_if_with_an_else() {
    let module = parse_test_source("if True:\n    print(1)\nelse:\n    print(2)\n");
    let hir = lower(&module);
    let HirItem::TopLevelStmt(HirStmt::If { orelse, .. }) = &hir.items[0] else {
        panic!("expected an If statement");
    };
    assert_eq!(orelse.len(), 1);
}

#[test]
fn lowers_an_elif_as_a_nested_if_in_orelse() {
    let module = parse_test_source("if False:\n    print(1)\nelif True:\n    print(2)\nelse:\n    print(3)\n");
    let hir = lower(&module);
    let HirItem::TopLevelStmt(HirStmt::If { orelse, .. }) = &hir.items[0] else {
        panic!("expected an If statement");
    };
    assert_eq!(orelse.len(), 1);
    assert!(matches!(orelse[0], HirStmt::If { .. }));
}

#[test]
fn lowers_a_while_loop() {
    let module = parse_test_source("while True:\n    print(1)\n");
    let hir = lower(&module);
    assert!(matches!(hir.items[0], HirItem::TopLevelStmt(HirStmt::While { .. })));
}

#[test]
fn lowers_a_for_range_loop_with_one_argument() {
    let module = parse_test_source("for i in range(3):\n    print(i)\n");
    let hir = lower(&module);
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::ForRange {
            var: "i".to_string(),
            start: HirExpr::IntLiteral(0),
            stop: HirExpr::IntLiteral(3),
            step: HirExpr::IntLiteral(1),
            body: vec![HirStmt::ExprStmt(HirExpr::Call { callee: "print".to_string(), args: vec![HirExpr::Name("i".to_string())] })],
        })]
    );
}

#[test]
fn lowers_a_for_range_loop_with_start_and_stop() {
    let module = parse_test_source("for i in range(1, 3):\n    print(i)\n");
    let hir = lower(&module);
    let HirItem::TopLevelStmt(HirStmt::ForRange { start, stop, step, .. }) = &hir.items[0] else {
        panic!("expected a ForRange statement");
    };
    assert_eq!(*start, HirExpr::IntLiteral(1));
    assert_eq!(*stop, HirExpr::IntLiteral(3));
    assert_eq!(*step, HirExpr::IntLiteral(1));
}

#[test]
#[should_panic(expected = "range")]
fn iterating_a_non_range_call_is_not_supported_yet() {
    let module = parse_test_source("for i in [1, 2, 3]:\n    print(i)\n");
    lower(&module);
}
```

- [x] **Step 2: Run to verify these fail**

Run: `cargo test -p pycc_hir lowers_an_if lowers_an_elif lowers_a_while lowers_a_for_range iterating_a_non_range`
Expected: FAIL to compile.

- [x] **Step 3: Extend `HirStmt`, `lower_stmt`, and add a `lower_body` helper**

```rust
// HirStmt gains:
//     If { test: HirExpr, body: Vec<HirStmt>, orelse: Vec<HirStmt> },
//     While { test: HirExpr, body: Vec<HirStmt> },
//     ForRange { var: String, start: HirExpr, stop: HirExpr, step: HirExpr, body: Vec<HirStmt> },
```

```rust
fn lower_body(body: &[pycc_ast::Stmt]) -> Vec<HirStmt> {
    body.iter().map(lower_stmt).collect()
}
```

In `lower_stmt`, add before the final `other => panic!` arm:

```rust
Stmt::If(pycc_ast::StmtIf { test, body, elif_else_clauses, .. }) => {
    HirStmt::If {
        test: lower_expr(test),
        body: lower_body(body),
        orelse: lower_elif_else_clauses(elif_else_clauses),
    }
}
Stmt::While(pycc_ast::StmtWhile { test, body, orelse, .. }) => {
    if !orelse.is_empty() {
        panic!("pycc_hir: while/else is not supported yet");
    }
    HirStmt::While { test: lower_expr(test), body: lower_body(body) }
}
Stmt::For(pycc_ast::StmtFor { is_async, target, iter, body, orelse, .. }) => {
    if *is_async {
        panic!("pycc_hir: async for is not supported yet");
    }
    if !orelse.is_empty() {
        panic!("pycc_hir: for/else is not supported yet");
    }
    let pycc_ast::Expr::Name(var) = target.as_ref() else {
        panic!("pycc_hir: only a bare name for-target is supported so far: {target:?}");
    };
    let pycc_ast::Expr::Call(call) = iter.as_ref() else {
        panic!("pycc_hir: only `for x in range(...)` is supported so far: {iter:?}");
    };
    let pycc_ast::Expr::Name(callee) = call.func.as_ref() else {
        panic!("pycc_hir: only `for x in range(...)` is supported so far: {:?}", call.func);
    };
    if callee.id.as_str() != "range" {
        panic!("pycc_hir: only iterating over `range(...)` is supported so far, got `{}`", callee.id);
    }
    let (start, stop, step) = match call.arguments.args.as_slice() {
        [stop] => (HirExpr::IntLiteral(0), lower_expr(stop), HirExpr::IntLiteral(1)),
        [start, stop] => (lower_expr(start), lower_expr(stop), HirExpr::IntLiteral(1)),
        [start, stop, step] => (lower_expr(start), lower_expr(stop), lower_expr(step)),
        other => panic!("pycc_hir: range() with {} arguments is not supported", other.len()),
    };
    HirStmt::ForRange { var: var.id.to_string(), start, stop, step, body: lower_body(body) }
}
```

Add the `elif`-flattening helper (an `ElifElseClause` with `test: Some(_)` becomes a nested `If` inside `orelse`; `test: None` is the trailing bare `else:`):

```rust
fn lower_elif_else_clauses(clauses: &[pycc_ast::ElifElseClause]) -> Vec<HirStmt> {
    let Some((first, rest)) = clauses.split_first() else {
        return vec![];
    };
    match &first.test {
        Some(test) => vec![HirStmt::If {
            test: lower_expr(test),
            body: lower_body(&first.body),
            orelse: lower_elif_else_clauses(rest),
        }],
        None => {
            assert!(rest.is_empty(), "pycc_hir: an else clause must be the last elif_else_clause");
            lower_body(&first.body)
        }
    }
}
```

- [x] **Step 4: Run to verify pycc_hir tests pass**

Run: `cargo test -p pycc_hir`
Expected: PASS.

- [x] **Step 5: Write the failing pycc_types tests**

```rust
#[test]
fn an_if_s_test_must_be_bool_like_and_both_branches_are_checked() {
    let mut env = Environment::new();
    let stmt = HirStmt::If {
        test: HirExpr::BoolLiteral(true),
        body: vec![HirStmt::Assign { target: "x".to_string(), value: HirExpr::IntLiteral(1) }],
        orelse: vec![HirStmt::Assign { target: "y".to_string(), value: HirExpr::IntLiteral(2) }],
    };
    check_stmt(&mut env, &stmt).unwrap();
    // Both branches ran in the same (single, unscoped-per-branch) environment
    // for v0.1's simplified model -- neither branch's bindings are undone,
    // matching "no real per-branch scoping yet" as an explicit, acceptable
    // v0.1 simplification (real flow-sensitive narrowing is TYPE_SYSTEM.md's
    // post-v0.1-depth territory).
    assert_eq!(env.lookup("x"), Some(Ty::Int));
    assert_eq!(env.lookup("y"), Some(Ty::Int));
}

#[test]
fn a_while_loop_s_test_and_body_are_checked() {
    let mut env = Environment::new();
    let stmt = HirStmt::While {
        test: HirExpr::BoolLiteral(true),
        body: vec![HirStmt::Assign { target: "x".to_string(), value: HirExpr::IntLiteral(1) }],
    };
    check_stmt(&mut env, &stmt).unwrap();
    assert_eq!(env.lookup("x"), Some(Ty::Int));
}

#[test]
fn a_for_range_loop_binds_its_variable_as_int_and_checks_its_body() {
    let mut env = Environment::new();
    let stmt = HirStmt::ForRange {
        var: "i".to_string(),
        start: HirExpr::IntLiteral(0),
        stop: HirExpr::IntLiteral(3),
        step: HirExpr::IntLiteral(1),
        body: vec![HirStmt::Assign { target: "x".to_string(), value: HirExpr::Name("i".to_string()) }],
    };
    check_stmt(&mut env, &stmt).unwrap();
    assert_eq!(env.lookup("i"), Some(Ty::Int));
    assert_eq!(env.lookup("x"), Some(Ty::Int));
}
```

- [x] **Step 6: Run to verify these fail**

Run: `cargo test -p pycc_types an_if_s_test a_while_loop_s_test a_for_range_loop_binds`
Expected: FAIL to compile.

- [x] **Step 7: Implement `check_stmt`'s new arms**

Add to `check_stmt`'s match in `pycc_types/src/lib.rs`:

```rust
HirStmt::If { test, body, orelse } => {
    infer_expr(env, test)?; // any type is accepted as truthy for v0.1 -- Python's own truthiness has no static type restriction
    for stmt in body {
        check_stmt(env, stmt)?;
    }
    for stmt in orelse {
        check_stmt(env, stmt)?;
    }
    Ok(())
}
HirStmt::While { test, body } => {
    infer_expr(env, test)?;
    for stmt in body {
        check_stmt(env, stmt)?;
    }
    Ok(())
}
HirStmt::ForRange { var, start, stop, step, body } => {
    infer_expr(env, start)?;
    infer_expr(env, stop)?;
    infer_expr(env, step)?;
    env.bind(var.clone(), Ty::Int);
    for stmt in body {
        check_stmt(env, stmt)?;
    }
    Ok(())
}
```

- [x] **Step 8: Run to verify pycc_types tests pass**

Run: `cargo test -p pycc_types`
Expected: PASS.

- [x] **Step 9: Run the full workspace suite, clippy, coverage**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`
Expected: PASS. Every new panic arm (`while/else`, `for/else`, `async for`, non-range-call for-target, non-bare-name for-target, wrong range() arg count) needs its own `#[should_panic]` test — add the missing ones now (this task's Step 1 only wrote one; add at minimum one more covering `while True:\n    pass\nelse:\n    pass\n` for the `while/else` arm, and one for `for i in range(1, 2, 3, 4):` for the wrong-arg-count arm).

- [x] **Step 10: Commit**

```bash
git add crates/pycc_hir/src/lib.rs crates/pycc_types/src/lib.rs
git commit -m "feat(pycc_hir,pycc_types): if/elif/else, while, for-range"
```

---

## Task 9: Function arguments, return values, recursion, and T0001

**Files:**
- Modify: `crates/pycc_hir/src/lib.rs`
- Modify: `crates/pycc_types/src/lib.rs`
- Test: inline in both

**Interfaces:**
- Produces: `HirItem::Function` gains `params: Vec<(String, Ty)>` and `return_ty: Ty` (both required — this is where T0001 actually fires, at lowering time if unannotated on a public function, per D-038's naming convention). `HirStmt` gains `Return(Option<HirExpr>)`. `pycc_types::check_function(env: &Environment, function: &HirItem) -> Result<(), Diagnostic>` type-checks a function's body against its declared parameter types and verifies every `Return` matches the declared `return_ty`.

- [x] **Step 1: Write the failing HIR tests**

```rust
#[test]
fn lowers_a_fully_annotated_public_function_with_params_and_return() {
    let module = parse_test_source("def add(a: int, b: int) -> int:\n    return a + b\n");
    let hir = lower(&module);
    let HirItem::Function { name, params, return_ty, body } = &hir.items[0] else {
        panic!("expected a Function item");
    };
    assert_eq!(name, "add");
    assert_eq!(params, &vec![("a".to_string(), Ty::Int), ("b".to_string(), Ty::Int)]);
    assert_eq!(*return_ty, Ty::Int);
    assert_eq!(body.len(), 1);
    assert!(matches!(body[0], HirStmt::Return(Some(_))));
}

#[test]
fn lowers_a_return_with_no_value() {
    let module = parse_test_source("def f() -> None:\n    return\n");
    let hir = lower(&module);
    let HirItem::Function { body, .. } = &hir.items[0] else {
        panic!("expected a Function item");
    };
    assert_eq!(body[0], HirStmt::Return(None));
}

#[test]
fn an_unannotated_public_function_produces_t0001_at_lowering_time() {
    let module = parse_test_source("def add(a, b):\n    return a + b\n");
    let diag = lower_checked(&module).unwrap_err();
    assert_eq!(diag.code, "T0001");
}

#[test]
fn an_unannotated_private_function_is_allowed() {
    let module = parse_test_source("def _add(a, b):\n    return a + b\n");
    let hir = lower_checked(&module).unwrap();
    let HirItem::Function { params, return_ty, .. } = &hir.items[0] else {
        panic!("expected a Function item");
    };
    // A private helper's unannotated params/return infer as a placeholder
    // until real Hindley-Milner unification is added -- Task 16's follow-up
    // widens this; for now, an unannotated private function's declared
    // params/return are simply absent (empty params, Ty::None return),
    // proving the T0001 gate is skipped without yet claiming real inference
    // exists for this case.
    assert!(params.is_empty());
    assert_eq!(*return_ty, Ty::None);
}
```

(`lower` becomes fallible in this task — rename the existing infallible `lower(module: &ModModule) -> HirModule` to a new `lower_checked(module: &ModModule) -> Result<HirModule, Diagnostic>`, and every existing test calling `lower(&module)` and pattern-matching/asserting on its `HirModule` directly needs updating to `lower_checked(&module).unwrap()` — this is a breaking signature change to `pycc_hir`'s public API, done deliberately in this task since T0001 is a lowering-time error, not a later type-checking-phase error, given HIR's `Function` item now carries the annotation requirement directly in its shape.)

- [x] **Step 2: Run to verify these fail**

Run: `cargo test -p pycc_hir`
Expected: FAIL to compile (signature/shape changes ripple through every existing test in the file).

- [x] **Step 3: Rename `lower` to `lower_checked`, returning `Result`; update `HirItem::Function`; implement T0001**

```rust
// HirItem::Function gains:
//     params: Vec<(String, Ty)>,
//     return_ty: Ty,
// (name and body fields stay as they are)

// HirStmt gains:
//     Return(Option<HirExpr>),
```

```rust
pub fn lower_checked(module: &ModModule) -> Result<HirModule, Diagnostic> {
    let items = module
        .body
        .iter()
        .map(|stmt| match stmt {
            Stmt::FunctionDef(def) => lower_function(def),
            other => Ok(HirItem::TopLevelStmt(lower_stmt(other))),
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(HirModule { items })
}

fn lower_function(def: &pycc_ast::StmtFunctionDef) -> Result<HirItem, Diagnostic> {
    let is_public = !def.name.as_str().starts_with('_'); // D-038
    let params = lower_params(&def.parameters, is_public, &def.name)?;
    let return_ty = lower_return_annotation(def.returns.as_deref(), is_public, &def.name)?;
    let body = def.body.iter().map(lower_stmt).collect();
    Ok(HirItem::Function { name: def.name.to_string(), params, return_ty, body })
}

fn lower_params(
    parameters: &pycc_ast::Parameters,
    is_public: bool,
    fn_name: &str,
) -> Result<Vec<(String, Ty)>, Diagnostic> {
    parameters
        .args
        .iter()
        .map(|param| {
            let name = param.parameter.name.as_str();
            match &param.parameter.annotation {
                Some(ann) => Ok((name.to_string(), annotation_to_ty(ann))),
                None if is_public => Err(pycc_diag::Diagnostic::error(
                    "T0001",
                    format!("parameter `{name}` of public function `{fn_name}` needs a type annotation"),
                    pycc_diag::Span::new(0, 0),
                )),
                None => Ok((name.to_string(), Ty::None)), // private helper: unannotated is allowed; real inference is a later task's job
            }
        })
        .collect()
}
```

Hold on — the `an_unannotated_private_function_is_allowed` test above asserts `params.is_empty()` for `def _add(a, b): ...`, which has two params — that assertion is wrong given `lower_params` (as just written) still produces two `(name, Ty::None)` entries, not an empty vec. Fix the test instead of the code (the code's behavior — "params exist, with a placeholder `Ty::None` type until real inference lands" — is the correct, more useful one to keep):

```rust
#[test]
fn an_unannotated_private_function_is_allowed() {
    let module = parse_test_source("def _add(a, b):\n    return a + b\n");
    let hir = lower_checked(&module).unwrap();
    let HirItem::Function { params, return_ty, .. } = &hir.items[0] else {
        panic!("expected a Function item");
    };
    assert_eq!(params, &vec![("a".to_string(), Ty::None), ("b".to_string(), Ty::None)]);
    assert_eq!(*return_ty, Ty::None);
}
```

```rust
fn lower_return_annotation(
    returns: Option<&pycc_ast::Expr>,
    is_public: bool,
    fn_name: &str,
) -> Result<Ty, Diagnostic> {
    match returns {
        Some(ann) => Ok(annotation_to_ty(ann)),
        None if is_public => Err(pycc_diag::Diagnostic::error(
            "T0001",
            format!("public function `{fn_name}` needs a return type annotation"),
            pycc_diag::Span::new(0, 0),
        )),
        None => Ok(Ty::None),
    }
}

fn annotation_to_ty(annotation: &pycc_ast::Expr) -> Ty {
    let pycc_ast::Expr::Name(name) = annotation else {
        panic!("pycc_hir: only a bare name type annotation is supported so far: {annotation:?}");
    };
    match name.id.as_str() {
        "int" => Ty::Int,
        "float" => Ty::Float,
        "bool" => Ty::Bool,
        "str" => Ty::Str,
        "None" => Ty::None,
        other => panic!("pycc_hir: type annotation `{other}` is not supported yet"),
    }
}
```

`Ty` must move from `pycc_types` into `pycc_hir` (or a new shared location) for this to compile, since `HirItem::Function`'s shape now names it directly and `pycc_hir` cannot depend on `pycc_types` (the dependency graph runs the other way: `pycc_types` depends on `pycc_hir`, confirmed by `pycc_types/src/lib.rs`'s existing `use pycc_hir::HirModule`). Move the `Ty` enum's definition into `pycc_hir/src/lib.rs`, and have `pycc_types/src/lib.rs` do `pub use pycc_hir::Ty;` at its top instead of defining `Ty` itself, so every earlier task's `pycc_types::Ty`-qualified test code keeps compiling unchanged.

Add `Return` handling to `lower_stmt`:

```rust
Stmt::Return(pycc_ast::StmtReturn { value, .. }) => {
    HirStmt::Return(value.as_deref().map(lower_expr))
}
```

- [x] **Step 4: Update every existing test in `pycc_hir` calling `lower(&module)`**

Run: `grep -n "lower(&module)" crates/pycc_hir/src/lib.rs` — change each to `lower_checked(&module).unwrap()`, keeping every existing assertion identical (they all use module-level `print`/function-call statements, none currently trigger T0001, so `.unwrap()` is safe for all of them). Do **not** change any assertion's expected value.

- [x] **Step 5: Update `src/main.rs`'s call site**

`try_build` currently calls `pycc_parser::parse` then `pycc_hir::lower(&module)`. Change to:

```rust
let hir = pycc_hir::lower_checked(&module).map_err(|diag| {
    eprintln!("error[{}]: {}", diag.code, diag.message);
    ExitCode::from(1)
})?;
```

- [x] **Step 6: Run to verify pycc_hir tests pass**

Run: `cargo test -p pycc_hir`
Expected: PASS.

- [x] **Step 7: Write the failing pycc_types test for function body checking**

```rust
#[test]
fn a_function_s_body_is_checked_against_its_declared_param_types() {
    let function = HirItem::Function {
        name: "add".to_string(),
        params: vec![("a".to_string(), Ty::Int), ("b".to_string(), Ty::Int)],
        return_ty: Ty::Int,
        body: vec![HirStmt::Return(Some(HirExpr::BinOp {
            op: BinOpKind::Add,
            left: Box::new(HirExpr::Name("a".to_string())),
            right: Box::new(HirExpr::Name("b".to_string())),
        }))],
    };
    check_function(&function).unwrap();
}

#[test]
fn a_return_type_mismatch_is_a_clean_error() {
    let function = HirItem::Function {
        name: "f".to_string(),
        params: vec![],
        return_ty: Ty::Str,
        body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
    };
    let err = check_function(&function).unwrap_err();
    assert_eq!(err.code, "T0023");
}

#[test]
fn recursion_is_supported_since_the_function_s_own_signature_is_in_scope() {
    let function = HirItem::Function {
        name: "count".to_string(),
        params: vec![("n".to_string(), Ty::Int)],
        return_ty: Ty::Int,
        body: vec![HirStmt::Return(Some(HirExpr::Call {
            callee: "count".to_string(),
            args: vec![HirExpr::Name("n".to_string())],
        }))],
    };
    check_function(&function).unwrap();
}
```

- [x] **Step 8: Run to verify these fail**

Run: `cargo test -p pycc_types a_function_s_body_is_checked a_return_type_mismatch recursion_is_supported`
Expected: FAIL to compile (`check_function` doesn't exist).

- [x] **Step 9: Implement `check_function`**

Calling a function needs its signature visible for both external call-sites (Task 6's placeholder `HirExpr::Call => Ok(Ty::None)` was deliberately incomplete) and recursive self-calls. Add a `FunctionSignature` registry to `Environment`:

```rust
#[derive(Debug, Default)]
pub struct Environment {
    bindings: HashMap<String, Ty>,
    functions: HashMap<String, (Vec<Ty>, Ty)>, // name -> (param types, return type)
}

impl Environment {
    // ...existing new/lookup/bind...

    pub fn bind_function(&mut self, name: String, param_tys: Vec<Ty>, return_ty: Ty) {
        self.functions.insert(name, (param_tys, return_ty));
    }

    pub fn lookup_function(&self, name: &str) -> Option<&(Vec<Ty>, Ty)> {
        self.functions.get(name)
    }
}
```

Update `infer_expr`'s `HirExpr::Call` arm to use this instead of the Task 6 placeholder:

```rust
HirExpr::Call { callee, args } => {
    let arg_tys = args.iter().map(|a| infer_expr(env, a)).collect::<Result<Vec<_>, _>>()?;
    if callee == "print" {
        return Ok(Ty::None); // print's own signature isn't user-declarable in v0.1
    }
    let Some((param_tys, return_ty)) = env.lookup_function(callee) else {
        return Err(Diagnostic::error(
            "T0021",
            format!("call to undefined function `{callee}`"),
            Span::new(0, 0),
        ));
    };
    if arg_tys.len() != param_tys.len() {
        return Err(Diagnostic::error(
            "T0021",
            format!(
                "`{callee}` expects {} argument(s), got {}",
                param_tys.len(),
                arg_tys.len()
            ),
            Span::new(0, 0),
        ));
    }
    for (i, (arg_ty, param_ty)) in arg_tys.iter().zip(param_tys.iter()).enumerate() {
        if !is_assignable(*arg_ty, *param_ty) {
            return Err(Diagnostic::error(
                "T0021",
                format!(
                    "argument {} of `{callee}` expects `{}`, got `{}`",
                    i + 1,
                    param_ty.name(),
                    arg_ty.name()
                ),
                Span::new(0, 0),
            ));
        }
    }
    Ok(*return_ty)
}
```

```rust
fn is_assignable(from: Ty, to: Ty) -> bool {
    from == to || (from == Ty::Bool && to == Ty::Int) // bool is a subtype of int, TYPE_SYSTEM.md's representation table
}
```

```rust
pub fn check_function(function: &pycc_hir::HirItem) -> Result<(), Diagnostic> {
    let pycc_hir::HirItem::Function { name, params, return_ty, body } = function else {
        panic!("check_function called with a non-Function HirItem");
    };
    let mut env = Environment::new();
    env.bind_function(name.clone(), params.iter().map(|(_, ty)| *ty).collect(), *return_ty);
    for (param_name, param_ty) in params {
        env.bind(param_name.clone(), *param_ty);
    }
    for stmt in body {
        check_stmt_in_function(&mut env, stmt, *return_ty)?;
    }
    Ok(())
}

fn check_stmt_in_function(env: &mut Environment, stmt: &HirStmt, return_ty: Ty) -> Result<(), Diagnostic> {
    match stmt {
        HirStmt::Return(None) => {
            if return_ty != Ty::None {
                return Err(Diagnostic::error(
                    "T0023",
                    format!("expected a return value of type `{}`, got none", return_ty.name()),
                    Span::new(0, 0),
                ));
            }
            Ok(())
        }
        HirStmt::Return(Some(expr)) => {
            let actual = infer_expr(env, expr)?;
            if !is_assignable(actual, return_ty) {
                return Err(Diagnostic::error(
                    "T0023",
                    format!("expected return type `{}`, got `{}`", return_ty.name(), actual.name()),
                    Span::new(0, 0),
                ));
            }
            Ok(())
        }
        HirStmt::If { body, orelse, .. } | HirStmt::While { body, orelse: _, .. } => {
            // shares check_stmt's own test/branch checking below via a small
            // recursive helper -- reuse check_stmt for the non-Return cases
            // and only special-case Return above, since Return is the only
            // construct check_stmt (module scope) doesn't already know how
            // to type-check correctly.
            check_stmt(env, stmt)?;
            for s in body.iter().chain(orelse.iter()) {
                check_stmt_in_function(env, s, return_ty)?;
            }
            Ok(())
        }
        other => check_stmt(env, other),
    }
}
```

The `HirStmt::If { .. } | HirStmt::While { .. }` combined pattern above will not compile as written (their field sets differ — `While` has no `orelse`). Split them:

```rust
HirStmt::If { test, body, orelse } => {
    infer_expr(env, test)?;
    for s in body {
        check_stmt_in_function(env, s, return_ty)?;
    }
    for s in orelse {
        check_stmt_in_function(env, s, return_ty)?;
    }
    Ok(())
}
HirStmt::While { test, body } => {
    infer_expr(env, test)?;
    for s in body {
        check_stmt_in_function(env, s, return_ty)?;
    }
    Ok(())
}
HirStmt::ForRange { var, start, stop, step, body } => {
    infer_expr(env, start)?;
    infer_expr(env, stop)?;
    infer_expr(env, step)?;
    env.bind(var.clone(), Ty::Int);
    for s in body {
        check_stmt_in_function(env, s, return_ty)?;
    }
    Ok(())
}
other => check_stmt(env, other),
```

(This duplicates `If`/`While`/`ForRange` traversal between `check_stmt` (module scope, no `Return` allowed) and `check_stmt_in_function` (function scope, `Return` allowed) — an acceptable, explicit duplication for v0.1 rather than a premature unifying abstraction; revisit if a third scope kind is ever added.)

Update `pycc_types::check`'s existing `HirItem::Function { .. }` arm (Task 6 left it as a no-op comment) to actually call `check_function`:

```rust
pycc_hir::HirItem::Function { .. } => check_function(item)?,
```

- [x] **Step 10: Run to verify pycc_types tests pass**

Run: `cargo test -p pycc_types`
Expected: PASS.

- [x] **Step 11: Run the full workspace suite, clippy, coverage**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`
Expected: PASS. Note `tests/slice0.rs`'s existing `defining_main_without_calling_it_produces_no_output` and similar fixtures use `def main() -> None:` — already fully annotated, so T0001 doesn't fire for them; confirm this by re-reading those exact fixture strings in `tests/slice0.rs` before assuming. Cover every new panic arm (multi-target assign already covered in Task 6; unsupported annotation types like `def f(x: list) -> None`) with a `#[should_panic]` test.

- [x] **Step 12: Commit**

```bash
git add crates/pycc_hir/src/lib.rs crates/pycc_types/src/lib.rs src/main.rs
git commit -m "feat(pycc_hir,pycc_types): function params/return/recursion, T0001 at lowering time"
```

---

## Task 10: `str` literals, `T0002` (`Any` forbidden)

**Files:**
- Modify: `crates/pycc_hir/src/lib.rs`
- Modify: `crates/pycc_types/src/lib.rs`

**Interfaces:**
- Produces: `HirExpr` gains `StringLiteral(String)`. `annotation_to_ty` (Task 9) rejects a bare `Any` annotation with `T0002` instead of the generic "not supported yet" panic, since `Any` is a real, named, deliberately-rejected case per TYPE_SYSTEM.md rule 3 — not merely unimplemented.

- [x] **Step 1: Write the failing HIR test**

```rust
#[test]
fn lowers_a_plain_string_literal() {
    let module = parse_test_source("x = \"hi\"\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::Assign {
            target: "x".to_string(),
            value: HirExpr::StringLiteral("hi".to_string()),
        })]
    );
}

#[test]
fn an_any_annotation_produces_t0002() {
    let module = parse_test_source("def f(x: Any) -> None:\n    pass\n");
    let diag = lower_checked(&module).unwrap_err();
    assert_eq!(diag.code, "T0002");
}
```

- [x] **Step 2: Run to verify these fail**

Run: `cargo test -p pycc_hir lowers_a_plain_string_literal an_any_annotation_produces_t0002`
Expected: FAIL to compile / FAIL (wrong error code, since `Any` currently panics rather than erroring).

- [x] **Step 3: Add `HirExpr::StringLiteral`, extend `lower_expr`, make `annotation_to_ty` fallible**

```rust
// HirExpr gains:
//     StringLiteral(String),
```

In `lower_expr`:

```rust
Expr::StringLiteral(pycc_ast::ExprStringLiteral { value, .. }) => {
    HirExpr::StringLiteral(value.to_str().to_string())
}
```

`annotation_to_ty` must become fallible (`Result<Ty, Diagnostic>`) to produce `T0002` instead of panicking for `Any` specifically, while still panicking for genuinely-unimplemented annotations (`list`, `dict`, etc. stay out of v0.1 scope and stay panics, per this task's narrow goal):

```rust
fn annotation_to_ty(annotation: &pycc_ast::Expr) -> Result<Ty, Diagnostic> {
    let pycc_ast::Expr::Name(name) = annotation else {
        panic!("pycc_hir: only a bare name type annotation is supported so far: {annotation:?}");
    };
    match name.id.as_str() {
        "int" => Ok(Ty::Int),
        "float" => Ok(Ty::Float),
        "bool" => Ok(Ty::Bool),
        "str" => Ok(Ty::Str),
        "None" => Ok(Ty::None),
        "Any" => Err(pycc_diag::Diagnostic::error(
            "T0002",
            "`Any` is not permitted in pycc code outside a declared interop boundary".to_string(),
            pycc_diag::Span::new(0, 0),
        )),
        other => panic!("pycc_hir: type annotation `{other}` is not supported yet"),
    }
}
```

Update `lower_params`/`lower_return_annotation` (Task 9) to propagate this `Result` with `?` instead of calling `annotation_to_ty` infallibly:

```rust
// lower_params, inside the `Some(ann) =>` arm:
Some(ann) => Ok((name.to_string(), annotation_to_ty(ann)?)),

// lower_return_annotation:
Some(ann) => annotation_to_ty(ann),
```

(Both functions already return `Result<_, Diagnostic>`, so this is a small, mechanical `?`-propagation change, not a new signature.)

- [x] **Step 4: Run to verify pycc_hir tests pass**

Run: `cargo test -p pycc_hir`
Expected: PASS.

- [x] **Step 5: Write the failing pycc_types tests**

```rust
#[test]
fn infers_a_string_literal_as_str() {
    let env = Environment::new();
    assert_eq!(infer_expr(&env, &HirExpr::StringLiteral("hi".to_string())), Ok(Ty::Str));
}

#[test]
fn adding_an_int_and_a_str_is_a_clean_type_error() {
    let env = Environment::new();
    let expr = HirExpr::BinOp {
        op: BinOpKind::Add,
        left: Box::new(HirExpr::IntLiteral(1)),
        right: Box::new(HirExpr::StringLiteral("x".to_string())),
    };
    let err = infer_expr(&env, &expr).unwrap_err();
    assert_eq!(err.code, "T0021");
}

#[test]
fn adding_two_strings_infers_str() {
    let env = Environment::new();
    let expr = HirExpr::BinOp {
        op: BinOpKind::Add,
        left: Box::new(HirExpr::StringLiteral("a".to_string())),
        right: Box::new(HirExpr::StringLiteral("b".to_string())),
    };
    assert_eq!(infer_expr(&env, &expr), Ok(Ty::Str));
}
```

- [x] **Step 6: Run to verify these fail**

Run: `cargo test -p pycc_types infers_a_string_literal adding_an_int_and_a_str adding_two_strings`
Expected: FAIL (no `Ty::Str` case in `infer_expr`, `numeric_result_type` doesn't know about string concatenation).

- [x] **Step 7: Implement**

Add to `infer_expr`'s match:

```rust
HirExpr::StringLiteral(_) => Ok(Ty::Str),
```

`numeric_result_type` needs a string-concatenation special case for `BinOpKind::Add` specifically (Python doesn't allow `"a" - "b"` or any other operator between strings):

```rust
fn numeric_result_type(op: BinOpKind, left: Ty, right: Ty) -> Result<Ty, Diagnostic> {
    if left == Ty::Str && right == Ty::Str {
        return if op == BinOpKind::Add {
            Ok(Ty::Str)
        } else {
            Err(Diagnostic::error(
                "T0021",
                format!("operator {op:?} is not defined for `str` and `str`"),
                Span::new(0, 0),
            ))
        };
    }
    let as_numeric = |t: Ty| match t {
        Ty::Bool => Some(Ty::Int),
        Ty::Int => Some(Ty::Int),
        Ty::Float => Some(Ty::Float),
        _ => None,
    };
    match (as_numeric(left), as_numeric(right)) {
        (Some(Ty::Int), Some(Ty::Int)) => Ok(Ty::Int),
        (Some(_), Some(_)) => Ok(Ty::Float),
        _ => Err(Diagnostic::error(
            "T0021",
            format!("operator {op:?} is not defined for `{}` and `{}`", left.name(), right.name()),
            Span::new(0, 0),
        )),
    }
}
```

- [x] **Step 8: Run to verify pycc_types tests pass**

Run: `cargo test -p pycc_types`
Expected: PASS.

- [x] **Step 9: Run the full workspace suite, clippy, coverage**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`
Expected: PASS.

- [x] **Step 10: Commit**

```bash
git add crates/pycc_hir/src/lib.rs crates/pycc_types/src/lib.rs
git commit -m "feat(pycc_hir,pycc_types): str literals and concatenation, T0002 for Any"
```

---

## Task 11: Basic f-strings

**Files:**
- Modify: `crates/pycc_hir/src/lib.rs`
- Modify: `crates/pycc_types/src/lib.rs`

**Interfaces:**
- Produces: `HirExpr` gains `FString(Vec<FStringPart>)`, `pub enum FStringPart { Literal(String), Interpolation(Box<HirExpr>) }` (a pycc-owned, flattened shape — deliberately not threading `ruff`'s own nested `FStringValue`/`FStringPart`/`InterpolatedElement` types any further than this one lowering function, keeping HIR's vocabulary its own). Only the simple case is in v0.1 scope: no `format_spec` (`{x:.2f}`-style), no `conversion` (`{x!r}`-style), no nested f-strings — each panics with a clear message.

- [x] **Step 1: Write the failing HIR tests**

```rust
#[test]
fn lowers_a_basic_f_string_with_one_interpolation() {
    let module = parse_test_source("x = 1\ny = f\"value: {x}\"\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items[1],
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: "y".to_string(),
            value: HirExpr::FString(vec![
                FStringPart::Literal("value: ".to_string()),
                FStringPart::Interpolation(Box::new(HirExpr::Name("x".to_string()))),
            ]),
        })
    );
}

#[test]
#[should_panic(expected = "format spec")]
fn an_f_string_with_a_format_spec_is_not_supported_yet() {
    let module = parse_test_source("x = 1.5\ny = f\"{x:.2f}\"\n");
    lower_checked(&module).unwrap();
}

#[test]
#[should_panic(expected = "conversion")]
fn an_f_string_with_a_conversion_flag_is_not_supported_yet() {
    let module = parse_test_source("x = 1\ny = f\"{x!r}\"\n");
    lower_checked(&module).unwrap();
}
```

(Verify `!r` actually parses to a non-`ConversionFlag::None` value in ruff 0.0.6 before trusting this test — check `nodes.rs`'s `ConversionFlag` enum definition directly if uncertain.)

- [x] **Step 2: Run to verify these fail**

Run: `cargo test -p pycc_hir lowers_a_basic_f_string an_f_string_with_a_format_spec an_f_string_with_a_conversion_flag`
Expected: FAIL to compile.

- [x] **Step 3: Add `FStringPart`, extend `HirExpr`, implement lowering**

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum FStringPart {
    Literal(String),
    Interpolation(Box<HirExpr>),
}

// HirExpr gains:
//     FString(Vec<FStringPart>),
```

In `lower_expr`:

```rust
Expr::FString(pycc_ast::ExprFString { value, .. }) => {
    let parts = value
        .elements()
        .map(|element| match element {
            pycc_ast::InterpolatedStringElement::Literal(lit) => {
                FStringPart::Literal(lit.value.to_string())
            }
            pycc_ast::InterpolatedStringElement::Interpolation(interp) => {
                if interp.conversion != Default::default() {
                    panic!("pycc_hir: f-string conversion flags (!r/!s/!a) are not supported yet");
                }
                if interp.format_spec.is_some() {
                    panic!("pycc_hir: f-string format spec ({{x:...}}) is not supported yet");
                }
                FStringPart::Interpolation(Box::new(lower_expr(&interp.expression)))
            }
        })
        .collect();
    HirExpr::FString(parts)
}
```

Confirm `ConversionFlag` implements `Default` (defaulting to "no conversion") and `PartialEq` by checking its definition in `nodes.rs` before trusting `!= Default::default()` compiles — if it doesn't implement both, match on its variant explicitly instead (e.g. `if !matches!(interp.conversion, pycc_ast::ConversionFlag::None) { panic!(...) }`, adding `ConversionFlag` to `pycc_ast`'s re-export list in that case, which Task 4 did not anticipate needing).

- [x] **Step 4: Run to verify pycc_hir tests pass**

Run: `cargo test -p pycc_hir`
Expected: PASS.

- [x] **Step 5: Write the failing pycc_types test**

```rust
#[test]
fn an_f_string_always_infers_str_regardless_of_interpolated_types() {
    let env = Environment::new();
    let expr = HirExpr::FString(vec![
        FStringPart::Literal("n=".to_string()),
        FStringPart::Interpolation(Box::new(HirExpr::IntLiteral(1))),
    ]);
    assert_eq!(infer_expr(&env, &expr), Ok(Ty::Str));
}

#[test]
fn an_f_string_still_type_checks_its_interpolated_expressions() {
    let env = Environment::new();
    let expr = HirExpr::FString(vec![FStringPart::Interpolation(Box::new(HirExpr::Name("undefined".to_string())))]);
    let err = infer_expr(&env, &expr).unwrap_err();
    assert_eq!(err.code, "T0021");
}
```

- [x] **Step 6: Run to verify these fail**

Run: `cargo test -p pycc_types an_f_string`
Expected: FAIL to compile (no `HirExpr::FString` arm in `infer_expr`).

- [x] **Step 7: Implement**

```rust
HirExpr::FString(parts) => {
    for part in parts {
        if let FStringPart::Interpolation(expr) = part {
            infer_expr(env, expr)?; // any interpolatable type is allowed; Python str()-coerces at runtime
        }
    }
    Ok(Ty::Str)
}
```

- [x] **Step 8: Run to verify pycc_types tests pass**

Run: `cargo test -p pycc_types`
Expected: PASS.

- [x] **Step 9: Run the full workspace suite, clippy, coverage**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`
Expected: PASS.

- [x] **Step 10: Commit**

```bash
git add crates/pycc_hir/src/lib.rs crates/pycc_types/src/lib.rs
git commit -m "feat(pycc_hir,pycc_types): basic f-strings (no format spec/conversion yet)"
```

---

## Task 12: Wire `pycc check` for real

**Files:**
- Modify: `src/main.rs`
- Modify: `src/cli.rs` (only if `Check`'s existing shape needs a field — check first)
- Test: `tests/slice0.rs`

**Interfaces:**
- Consumes: `pycc_parser::parse`, `pycc_hir::lower_checked`, `pycc_types::check`, `pycc_diag::render_human`/`render_json` (all prior tasks).
- Produces: `fn try_check(path: &str, format: CheckFormat) -> Result<(), ExitCode>` in `src/main.rs`, wired to `Command::Check`. Exit codes match CLI_SPEC.md's contract: `0` on a clean check, `1` on any diagnostic.

- [x] **Step 1: Read `src/cli.rs`'s current `Check` variant shape**

Run: `grep -n "Check" src/cli.rs`
Expected output describes whatever fields already exist (likely just `{ path: Option<String> }`, possibly no `--error-format` flag yet per CLI_SPEC.md's `--error-format human|json` key flag, which is currently unimplemented workspace-wide). If `--error-format` isn't already a field, add it:

```rust
Check {
    path: Option<String>,
    #[arg(long, default_value = "human")]
    error_format: String,
},
```

- [x] **Step 2: Write the failing CLI test**

Add to `tests/slice0.rs`:

```rust
#[test]
fn check_subcommand_reports_no_issues_on_valid_code() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_check_ok_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "ok.py", "def main() -> None:\n    print(42)\n\nmain()\n");

    let output = Command::new(pycc_bin()).args(["check", src.to_str().unwrap()]).output().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"");
}

#[test]
fn check_subcommand_reports_t0001_on_an_unannotated_public_function() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_check_t0001_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "bad.py", "def add(a, b):\n    return a + b\n");

    let output = Command::new(pycc_bin()).args(["check", src.to_str().unwrap()]).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stdout).contains("T0001"));
}

#[test]
fn check_subcommand_supports_json_error_format() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_check_json_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "bad.py", "def add(a, b):\n    return a + b\n");

    let output = Command::new(pycc_bin())
        .args(["check", src.to_str().unwrap(), "--error-format", "json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(parsed["code"], "T0001");
}
```

(`serde_json` becomes a `[dev-dependencies]` addition to the root `pycc` binary crate's `Cargo.toml` for this test — it isn't already a dependency there, only in `pycc_diag`.)

- [x] **Step 3: Run to verify these fail**

Run: `cargo test --test slice0 check_subcommand`
Expected: FAIL — `check` currently prints "pycc: this subcommand is not yet implemented" and exits 2, not the behavior these tests expect.

- [x] **Step 4: Implement `try_check` and wire it into `main`**

In `src/main.rs`, replace `Command::Check { .. }`'s current arm (which currently falls into the shared "not yet implemented" branch alongside `Test`/`Explain`/`Init`/`Clean`) with its own dedicated case:

```rust
Command::Check { path, error_format } => match try_check(path.as_deref().unwrap_or("."), &error_format) {
    Ok(()) => ExitCode::SUCCESS,
    Err(code) => code,
},
```

(Confirm `path`'s exact type from Step 1 — if it's `Option<String>` as assumed, `.as_deref().unwrap_or(".")` is correct; adjust if it's a plain `String` already defaulting elsewhere.)

Add `try_check` near `try_build`:

```rust
fn try_check(path: &str, error_format: &str) -> Result<(), ExitCode> {
    let source = std::fs::read_to_string(path).map_err(|e| {
        eprintln!("error: could not read `{path}`: {e}");
        ExitCode::from(2)
    })?;
    let report = |diag: pycc_diag::Diagnostic| -> ExitCode {
        let rendered = if error_format == "json" {
            pycc_diag::render_json(&diag, path, &source)
        } else {
            pycc_diag::render_human(&diag, path, &source)
        };
        println!("{rendered}");
        ExitCode::from(1)
    };
    let module = match pycc_parser::parse(&source) {
        Ok(m) => m,
        Err(diag) => return Err(report(diag)),
    };
    let hir = match pycc_hir::lower_checked(&module) {
        Ok(h) => h,
        Err(diag) => return Err(report(diag)),
    };
    match pycc_types::check(&hir) {
        Ok(()) => Ok(()),
        Err(diag) => Err(report(diag)),
    }
}
```

- [x] **Step 5: Run to verify the CLI tests pass**

Run: `cargo test --test slice0 check_subcommand`
Expected: PASS.

- [x] **Step 6: Run the full workspace suite, clippy, coverage**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`
Expected: PASS. Confirm `unimplemented_subcommands_exit_with_code_2` (an existing `tests/slice0.rs` test asserting `pycc clean` exits 2) still passes — `Check` moving out of that shared match arm must not affect the other still-unimplemented subcommands (`Test`, `Explain`, `Init`, `Clean`).

- [x] **Step 7: Commit**

```bash
git add src/main.rs src/cli.rs tests/slice0.rs Cargo.toml
git commit -m "feat(pycc): wire pycc check for real (parse+HIR+types, human/JSON output)"
```

---

## Task 13: `tests/diagnostics/` fixtures with hand-rolled snapshot assertions (D-036)

**Files:**
- Create: `tests/diagnostics/d0001_missing_public_annotation.py`
- Create: `tests/diagnostics/d0001_missing_public_annotation.expected.txt`
- Create: `tests/diagnostics/d0002_any_forbidden.py`
- Create: `tests/diagnostics/d0002_any_forbidden.expected.txt`
- Create: `tests/diagnostics_test.rs`
- Modify: `docs/PYTHON_STANDARDS.md` (mark the `T0001`/`E0102` rows in the "Rejected by design" table's status, if that table has a status column already populated for others — check first; if the table's design predates a status column, add one consistently for every row, not just these two)

**Interfaces:**
- Consumes: `pycc` binary (via `Command::new(pycc_bin())`, same pattern as `tests/slice0.rs`).
- Produces: nothing new code-facing — this task is pure test/fixture infrastructure.

- [x] **Step 1: Write the fixture files**

`tests/diagnostics/d0001_missing_public_annotation.py`:
```python
def add(a, b):
    return a + b
```

`tests/diagnostics/d0002_any_forbidden.py`:
```python
from typing import Any


def f(x: Any) -> None:
    pass
```

(Confirm `pycc_parser`/`pycc_hir` actually handle a bare `from typing import Any` import statement without panicking before trusting this fixture — if `Stmt::ImportFrom` isn't lowered at all yet (it isn't, per Task 5's `lower_stmt`'s catch-all panic, and no task in this plan adds import handling), this fixture will panic on the import line before ever reaching the `Any` annotation. Rewrite it to avoid the import, since imports are out of this PR's scope entirely:

```python
def f(x) -> None:
    pass
```

Wait — this no longer has an `Any` annotation to trigger `T0002` at all; it would trigger `T0001` (missing param annotation) instead, duplicating `d0001`'s fixture. Given imports are genuinely out of scope and `Any` can only appear as a bare name in an annotation position (which ruff parses as `Expr::Name` regardless of whether `Any` was ever imported — pycc's own `annotation_to_ty` only pattern-matches the *string* `"Any"`, it doesn't check whether the name was actually imported from `typing`, since real name resolution against `typing`'s exports is out of v0.1 scope), the import line can simply be omitted and the bare name still triggers T0002 correctly:

```python
def f(x: Any) -> None:
    pass
```

This is correct as pycc's own `annotation_to_ty` doesn't care about `Any`'s import provenance — only its literal spelling. Use this version, no import line.)

`tests/diagnostics/d0001_missing_public_annotation.expected.txt` — run `cargo run --bin pycc -- check tests/diagnostics/d0001_missing_public_annotation.py` locally once Task 12 lands, and copy its *actual* stdout verbatim into this file (do not hand-write the expected text from imagination — D-036's whole point is comparing against real, verified output). Example of the shape to expect (verify exactly, byte for byte, before committing):

```
error[T0001]: parameter `a` of public function `add` needs a type annotation
```

(The exact rendered text depends on `render_human`'s real output for a `Span::new(0, 0)` diagnostic against this exact source — Task 6 onward's diagnostics all use a placeholder `Span::new(0, 0)` rather than a real span threaded from HIR, an acknowledged, tracked gap this plan's Task 15 follow-up section calls out; the expected `.txt` file will therefore show `-->` pointing at line 1, column 1 regardless of which token is actually wrong, which is misleading but consistent — do not treat this as a bug to silently fix mid-task; it's the honest, already-flagged state of span-precision in this PR.)

`tests/diagnostics/d0002_any_forbidden.expected.txt` — same process: run `pycc check` on the real fixture and copy its real output.

- [x] **Step 2: Write the failing test harness**

Create `tests/diagnostics_test.rs`:

```rust
use std::path::Path;
use std::process::Command;

fn pycc_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_pycc"))
}

fn assert_diagnostic_matches_fixture(fixture_stem: &str) {
    let fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/diagnostics");
    let py_path = fixture_dir.join(format!("{fixture_stem}.py"));
    let expected_path = fixture_dir.join(format!("{fixture_stem}.expected.txt"));
    let expected = std::fs::read_to_string(&expected_path)
        .unwrap_or_else(|e| panic!("could not read {}: {e}", expected_path.display()));

    let output = Command::new(pycc_bin()).args(["check", py_path.to_str().unwrap()]).output().unwrap();
    let actual = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        actual.trim_end(),
        expected.trim_end(),
        "diagnostic output for {fixture_stem} did not match its .expected.txt fixture"
    );
    assert_eq!(output.status.code(), Some(1), "{fixture_stem} should be a compile error");
}

#[test]
fn d0001_missing_public_annotation() {
    assert_diagnostic_matches_fixture("d0001_missing_public_annotation");
}

#[test]
fn d0002_any_forbidden() {
    assert_diagnostic_matches_fixture("d0002_any_forbidden");
}
```

- [x] **Step 3: Run to verify it fails, then generate the real fixtures**

Run: `cargo test --test diagnostics_test`
Expected: initially FAILS (the `.expected.txt` files don't exist yet, or don't match if hand-guessed). Run `cargo build --bin pycc` then manually: `./target/debug/pycc check tests/diagnostics/d0001_missing_public_annotation.py` and `./target/debug/pycc check tests/diagnostics/d0002_any_forbidden.py`, and paste each command's *actual* stdout into the corresponding `.expected.txt` file exactly.

- [x] **Step 4: Run to verify it passes**

Run: `cargo test --test diagnostics_test`
Expected: PASS.

- [x] **Step 5: Run the full workspace suite, clippy, coverage**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`
Expected: PASS. `tests/diagnostics_test.rs` is itself a `tests/` file, excluded from the coverage denominator by cargo-llvm-cov's default (per D-014's own documented behavior) — no coverage concern from this file itself, only from whether it actually exercises real product-code paths (it does, via the real `pycc` binary).

- [x] **Step 6: Commit**

```bash
git add tests/diagnostics/ tests/diagnostics_test.rs
git commit -m "test: add tests/diagnostics/ negative fixtures for T0001 and T0002 (D-036)"
```

---

## Task 14: Frontend performance gate

**Files:**
- Create: `benches/check_bench.rs`
- Modify: `Cargo.toml` (root workspace manifest, or the `pycc` binary package's own manifest — confirm which one currently owns `[[bin]]`/build config before adding `[[bench]]`)
- Modify: `.github/workflows/ci.yml`
- Modify: `docs/DELIVERY_PLAN.md` (mark the Performance gate section's "not yet wired" status, if such wording exists, as done — read the exact current text first)

**Interfaces:**
- Produces: a `criterion` benchmark measuring `pycc check`'s wall-clock time on a representative fixture, checked into CI, with the *first* recorded run establishing the baseline (nothing to compare against yet, per DELIVERY_PLAN.md's literal "every *subsequent* PR" wording) and every later PR's CI run failing if its own `pycc check` timing regresses >2% against the immediately preceding recorded baseline.

- [x] **Step 1: Add the `criterion` dependency**

Run: `curl -sA "pycc-build/0.1" https://crates.io/api/v1/crates/criterion | python3 -c "import json,sys;print(json.load(sys.stdin)['crate']['newest_version'])"` and use whatever version comes back (verify, don't assume `0.5` or any other specific number from memory).

Add to the root `pycc` binary package's `Cargo.toml` (the crate with `src/main.rs`, not `crates/pycc_hir` etc. — this benchmark measures the whole `pycc check` pipeline, which only the binary crate assembles end to end):

```toml
[dev-dependencies]
criterion = { version = "<verified version>", features = ["html_reports"] }

[[bench]]
name = "check_bench"
harness = false
```

(If the root manifest doesn't currently have a `[package]`/`[[bin]]` section of its own — recall PR-1's plan explicitly deferred adding one until its own Task 8 — confirm it exists now, post PR-2/PR-3, before assuming this structure; `src/main.rs` already exists and is built as a binary today, so a `[package]` section must already exist somewhere. Run `grep -n "\[package\]\|\[\[bin\]\]" Cargo.toml` to locate it.)

- [x] **Step 2: Write the benchmark**

Since `pycc check`'s actual logic lives in `src/main.rs`'s private `try_check` function, and a `benches/` file can't call a binary crate's private functions directly, the benchmark measures the same pipeline `try_check` calls, assembled directly from the public library crates it depends on (`pycc_parser`, `pycc_hir`, `pycc_types`) rather than shelling out to the compiled binary (spawning a subprocess would measure process-startup overhead, not the frontend's own work, and be far noisier run to run):

```rust
use criterion::{criterion_group, criterion_main, Criterion};

const FIXTURE: &str = r#"
def fib(n: int) -> int:
    if n < 2:
        return n
    return fib(n - 1) + fib(n - 2)

def main() -> None:
    x = 0
    for i in range(10):
        x = x + fib(i)
    print(x)

main()
"#;

fn bench_check(c: &mut Criterion) {
    c.bench_function("pycc_check_frontend_fixture", |b| {
        b.iter(|| {
            let module = pycc_parser::parse(FIXTURE).unwrap();
            let hir = pycc_hir::lower_checked(&module).unwrap();
            pycc_types::check(&hir).unwrap();
        });
    });
}

criterion_group!(benches, bench_check);
criterion_main!(benches);
```

(Verify this exact `FIXTURE` string actually type-checks cleanly with zero diagnostics once Tasks 6-11 land — `fib`/`main` are both public, both fully annotated, using arithmetic/comparisons/if/for-range/recursion/assignment, exercising a real cross-section of this PR's new grammar without tripping any of its diagnostics, which would make the benchmark measure an early-exit error path instead of full frontend work.)

- [x] **Step 3: Run the benchmark locally to confirm it executes**

Run: `cargo bench --bench check_bench`
Expected: completes, printing a criterion timing report (e.g. "time: [12.3 µs 12.5 µs 12.8 µs]" — exact numbers don't matter, only that it runs without erroring).

- [x] **Step 4: Add the CI job**

Add a new job to `.github/workflows/ci.yml` (after `ci-gate`, or wherever a new job reads best given the file's existing structure — check the file's current end before appending):

```yaml
  # Performance gate (resolves DELIVERY_PLAN.md issue #12): fails a PR whose
  # `pycc check` frontend timing regresses >2% against the immediately
  # preceding PR's recorded baseline. Runs on the same macOS runner as the
  # coverage gate for a stable, single-machine timing baseline -- comparing
  # across different runner hardware would be noise, not signal.
  frontend-perf-gate:
    runs-on: macos-14
    steps:
      - uses: actions/checkout@v4

      - name: Show pinned toolchain
        run: rustup show

      - name: Install LLVM 22 (D-015)
        run: brew install llvm@22

      - name: Export LLVM_SYS_221_PREFIX
        run: echo "LLVM_SYS_221_PREFIX=$(brew --prefix llvm@22)" >> "$GITHUB_ENV"

      - name: Run frontend benchmark
        run: cargo bench --bench check_bench -- --save-baseline current

      # Compares against the previous run's cached baseline; a fresh cache
      # (this PR's first run, or the very first time this job exists at all)
      # has nothing to compare against, so the comparison step is skipped
      # cleanly rather than failing on a missing baseline (D-014-style
      # honesty: absence of a prior baseline is a real, temporary state, not
      # an error to paper over).
      - name: Cache criterion baseline
        uses: actions/cache@v4
        with:
          path: target/criterion
          key: criterion-baseline-${{ github.event.pull_request.base.sha || github.sha }}
          restore-keys: criterion-baseline-

      - name: Compare against previous baseline (if one exists)
        run: |
          if [ -d target/criterion/pycc_check_frontend_fixture/previous ]; then
            cargo bench --bench check_bench -- --baseline previous
          else
            echo "no previous baseline cached yet -- this run establishes it"
          fi
```

(This cache-key/comparison design is a reasonable first cut, not a fully-proven CI pattern — `actions/cache@v4`'s exact semantics for a criterion-baseline workflow across PRs deserve a real CI round-trip to confirm before treating this step as load-bearing; if the comparison step doesn't actually fail the job on a >2% regression as written, that is a known follow-up, not a silent gap, since criterion's own `--baseline`/`--save-baseline` CLI flags don't themselves set a process exit code on regression — confirm this by reading `criterion`'s own CLI docs for the pinned version, and if it doesn't exit non-zero on regression, add a small script parsing criterion's JSON output (`target/criterion/*/estimates.json`) and comparing the two runs' mean estimates directly, failing with `exit 1` if the current run's mean exceeds the previous by >2%.)

- [x] **Step 5: Validate the new workflow YAML**

Run: `ruby -ryaml -e "YAML.load_file('.github/workflows/ci.yml'); puts 'valid YAML'"` and `actionlint .github/workflows/ci.yml` and `ruby scripts/check_ci_permissions.rb`
Expected: all three pass — the new job inherits the workflow's top-level `contents: read` permission (it only checks out and benchmarks, no elevated access needed, no `permissions:` override required).

- [x] **Step 6: Update `ci-gate`'s `needs:` list**

`.github/workflows/ci.yml`'s `ci-gate` job (D-032) currently `needs: [build-test-coverage, native-build-test, cross-compile-build, cross-compile-verify]` — decide whether `frontend-perf-gate` should also gate merges (add it to `ci-gate`'s `needs:` and its `if:` condition's explicit checks) or stay informational-only for now (since its regression-detection mechanism is unproven per Step 4's caveat). Given the mechanism isn't yet verified to actually fail correctly, the conservative choice is: do **not** add it to `ci-gate` in this task — record that decision explicitly:

Note: this snippet's `D-039` label is a stale placeholder from when this plan was written — D-039 and D-040 are both already taken (D-040 closed the sibling-function-call gap D-039 itself deferred). Re-check `docs/DECISIONS.md`'s actual highest existing ID when Task 14 is executed and use the next free one instead.

```markdown
## D-039: `frontend-perf-gate` is not yet a required check

- Status: accepted (PR-4 is the PR that depends on it)
- Context: Task 14 stood up a `criterion`-based frontend benchmark and a CI job intended to fail on a >2% regression, but whether criterion's own CLI actually sets a failing exit code on regression (vs. just printing a report) for the pinned version was not verified against a live CI run before this PR needed to land -- see the job definition's own comment in `ci.yml`.
- Decision: `frontend-perf-gate` runs on every PR and reports its result, but is **not** added to `ci-gate`'s `needs:` list yet, so it cannot block a merge on a false negative (silently never failing) or false positive (failing on cache-key mismatches unrelated to a real regression) while its mechanism is unproven.
- Alternatives: add it to `ci-gate` immediately, matching DELIVERY_PLAN.md's "every subsequent PR... fails if it regresses" wording literally (rejected -- an unverified merge-blocking gate is worse than an honestly-optional one; a gate nobody trusts gets disabled at the first false positive, which is a worse outcome than shipping it optional and hardening it once its first few real runs are observed).
- Consequences: a real regression could merge without being blocked until this gate is verified and promoted into `ci-gate`'s `needs:` list in a focused follow-up PR (tracked, not silent).
```

Append this to `docs/DECISIONS.md` (table row + full section, same format as every other entry) as part of this task's commit.

- [x] **Step 7: Run the full workspace suite one more time**

Run: `cargo build --workspace && cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`
Expected: PASS. `benches/check_bench.rs` is a `[[bench]]` target, not a `[[test]]` — confirm `cargo llvm-cov`'s denominator excludes it the same way it excludes `tests/` (check `cargo llvm-cov`'s own docs for whether `benches/` needs an explicit `--ignore-filename-regex` or is excluded by default; if it's *not* excluded by default and the benchmark file itself shows as uncovered, add a documented exemption entry to `docs/TESTING.md`'s exemption table for `benches/check_bench.rs` specifically, following D-014's existing whole-file-exemption mechanism).

- [x] **Step 8: Commit**

```bash
git add benches/check_bench.rs Cargo.toml .github/workflows/ci.yml docs/DECISIONS.md docs/TESTING.md
git commit -m "feat(ci): frontend performance gate (criterion benchmark for pycc check, D-0NN)"  # use the actual next-free ADR ID, not a hardcoded D-039 -- see the note above Step 6's snippet
```

---

## Self-Review

**1. Spec coverage:**
- TYPE_SYSTEM.md rule 1 (T0001) — Task 9. ✅
- TYPE_SYSTEM.md rule 2 (local inference for private helpers) — Task 9 partially (private functions skip T0001; real Hindley-Milner *unification* for genuinely ambiguous inference cases, beyond the "infer from literal/binop shape" already built, is a real gap — flagged explicitly in Task 9's own `an_unannotated_private_function_is_allowed` test comment as a placeholder pending a later task; **this plan does not fully close rule 2** for every conceivable private-helper shape, only the ones this plan's own grammar subset can construct. Acceptable for v0.1's scope, but should be named here rather than silently assumed complete.)
- TYPE_SYSTEM.md rule 3 (`Any` forbidden, T0002) — Task 10. ✅
- TYPE_SYSTEM.md rule 4 (no implicit Optional/narrowing/untyped containers) — **not covered by this plan.** v0.1's grammar subset (per DELIVERY_PLAN.md's own TDD sequence: arithmetic → comparisons → if/while/for+range → functions/recursion → f-strings) never introduces `Optional`, containers, or narrowing constructs at all, so this rule has no surface to violate yet — not a gap in this plan, a gap in what v0.1's grammar itself includes. Flag this explicitly rather than silently passing over it.
- TYPE_SYSTEM.md rule 5 (unreachable-after-`match`/`Never`) — **not covered.** `match` isn't in v0.1's grammar per DELIVERY_PLAN.md's own scope (PEP 634 `match` is PR-... actually, checking ROADMAP.md's v0.1 accept criteria again: only `if/while/for+range`, no `match` — confirmed out of scope, not a gap).
- Type↔representation table (int/float/bool/str/None) — Task 6, 7, 10. ✅ (class/Protocol/enum/generics explicitly out of v0.1 scope, correctly excluded)
- DIAGNOSTICS.md quality bar (span, label, help) — Task 3 covers span; **`help` suggestions are never populated** (every `render_human` call renders an empty help section implicitly, since no task ever constructs one) — flagged as a real, tracked gap, not fixed in this plan; add a follow-up task or DECISIONS.md note if this matters before PR-4 ships. **Action: add this as a documented, explicit follow-up in Task 14's own ADR entry (next free ID, not literally D-039 — that and D-040 are both already taken).**
- CLI_SPEC.md human/JSON format — Task 3. ✅
- PYTHON_STANDARDS.md `tests/diagnostics/` convention — Task 13. ✅
- DELIVERY_PLAN.md Performance gate — Task 14. ✅ (with the honest caveat, recorded under whatever ADR ID is next-free at that time, about its unverified regression-detection mechanism)
- DELIVERY_PLAN.md's exact TDD sequence "arithmetic → comparisons → if/while/for+range → functions/recursion → basic f-strings" — Tasks 6, 7, 8, 9, 11 in that exact order. ✅

**Gap found during self-review, fixed inline:** TYPE_SYSTEM.md rule 4 and the missing `help:` suggestions are real, named gaps this plan doesn't close. Add one more task before considering PR-4 complete:

## Task 15: Document known gaps, don't ship them silently

**Files:**
- Modify: `docs/DECISIONS.md`

- [x] **Step 1: Add D-040 recording the real, deliberate gaps this PR leaves**

```markdown
## D-040: PR-4's known gaps -- Optional/narrowing/containers, help-text suggestions, real spans

- Status: accepted (PR-4 is the PR that depends on it)
- Context: self-review of this PR's own implementation plan against TYPE_SYSTEM.md and DIAGNOSTICS.md in full found three things this PR does not close: (1) TYPE_SYSTEM.md rule 4 (no implicit Optional, no untyped containers) has no surface to violate yet, since v0.1's grammar (this PR's own scope) never introduces `Optional`/containers/narrowing constructs at all; (2) DIAGNOSTICS.md's quality bar calls for a `help:` suggestion "when one is safe" on every diagnostic, but no diagnostic this PR produces ever populates one (`render_human`/`render_json` support it structurally, nothing calls the codepath that would fill it in); (3) every diagnostic's `Span` is `Span::new(0, 0)` regardless of where the real error is (Task 2 only fixed the *parser's own* L0001 span; every T0001/T0002/T0021/T0023 diagnostic Task 6 onward constructs still hardcodes a placeholder span, since threading a real span through the whole HIR-lowering/type-inference call chain -- carrying a `Span` alongside every `HirExpr`/`HirStmt` node -- is a structural change this plan deliberately didn't take on given its already-large scope).
- Decision: ship PR-4 with these three gaps named here rather than silently implied-complete. (1) needs no action until a later PR's grammar actually introduces `Optional`/containers/narrowing. (2) and (3) are real, valuable, and tracked as explicit follow-up work, not blocking PR-4's own merge, since every diagnostic this PR produces is still correct and useful without a real span or a help suggestion -- just less precise than the spec's eventual bar.
- Alternatives: block PR-4 until all three are fully closed (rejected -- (1) is not actionable yet given v0.1's own grammar scope, and (3) alone is large enough to be its own PR-sized effort spanning every HIR node gaining a `span: Span` field, which this plan's already-substantial scope should not also absorb).
- Consequences: a future PR (naturally, whichever PR next touches `pycc_hir`/`pycc_types` significantly, likely PR-5 or a dedicated diagnostics-quality PR) should thread real spans through HIR before DIAGNOSTICS.md's quality bar can be honestly claimed as met; until then, every diagnostic's `-->` line points at line 1 column 1 regardless of the real error location, a known, user-visible imprecision.
```

- [x] **Step 2: Commit**

```bash
git add docs/DECISIONS.md
git commit -m "docs: record PR-4's known gaps honestly (D-040) rather than leaving them silently implied"
```

**2. Placeholder scan:** no "TBD"/"implement later" strings remain in any step's code (searched: every code block above is complete and would compile as written, modulo the explicitly-flagged verification points calling out exact method names to confirm against the pinned `ruff_python_ast`/`ruff_python_parser` 0.0.6 source before trusting them — e.g. `Int::as_i64`, `ParseError::location`'s exact field name, `ConversionFlag`'s trait derives — each of these is a "verify this specific claim against the real source" instruction, not a placeholder for missing design).

**3. Type consistency:** `Ty` is defined once (moved into `pycc_hir` in Task 9, re-exported from `pycc_types`) and used with the same name/variants throughout Tasks 6-14. `HirExpr`/`HirStmt`/`HirItem` grow additively across Tasks 5-11 with no renamed fields once introduced (`params`/`return_ty`/`body` on `HirItem::Function` stay those names from Task 9 onward; `target`/`value` on `HirStmt::Assign` stay those names from Task 6 onward). `Environment::lookup`/`bind`/`lookup_function`/`bind_function` are introduced once (Tasks 6, 9) and never renamed later.

---

**Plan complete and saved to `docs/superpowers/plans/2026-07-25-pr4-frontend-depth.md`.**
