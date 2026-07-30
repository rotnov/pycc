# v0.2 PR-9: Real Per-PEP Conformance Harness — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add 9 real, empirically-verified PEP conformance fixtures (8 that need zero new pycc features, plus PEP 526's own small, bounded `Stmt::AnnAssign` frontend addition) to `tests/conformance.rs`, each diffed byte-for-byte against pinned CPython 3.14.6 in **both** `--debug` and `--release` profiles — closing the measurement gap `docs/DELIVERY_PLAN.md`'s v0.2 acceptance criterion needs.

**Architecture:** Extend the existing plain `tests/conformance.rs` integration test (no new `pycc_testkit` crate — see Task 1's ADR) with one more `#[test]` function per fixture, each calling a small profile-aware extension of the existing `run_conformance_fixture` helper twice (once per profile). PEP 526 support is added bottom-up through the compiler: `pycc_ast` (a one-line re-export fix — parsing already works), `pycc_hir` (a new `HirStmt::AnnAssign` variant + lowering), `pycc_types` (a new `T0025` diagnostic + reused `is_assignable`/`annotation_to_ty` machinery), `pycc_mir`/`pycc_codegen` (a new, trivial `MirStmt::NoOp` for the value-less `x: int` form; the valued form `x: int = 1` collapses straight into the existing `MirStmt::Assign`, so codegen proper needs zero new logic for that path).

**Tech stack:** No new external dependencies. `ruff_python_ast` (already pinned, 0.0.6) already parses `x: int = 1` into a native `StmtAnnAssign { target, annotation, value: Option<Box<Expr>>, simple: bool }` node — confirmed by reading the vendored registry source directly. All new Rust code lives in the 4 existing frontend/backend crates plus `tests/conformance.rs`; no new crate, no new CI workflow step (the existing `cargo test --workspace -- --include-ignored` step already picks up any new `#[ignore]`d test with zero `ci.yml` changes).

## Global Constraints

- D-014: 100% line/region coverage is a hard merge gate for every crate touched (`pycc_hir`, `pycc_types`, `pycc_mir`, `pycc_codegen`) — every new branch below needs an executing test. `tests/*.rs` integration-test files and `.py` fixtures are outside the coverage denominator (confirmed: `.github/workflows/ci.yml`'s own comment states this explicitly for `tests/conformance.rs`), so the fixture-adding tasks need no coverage work of their own, but the PEP 526 crate-internals tasks do.
- D-021: this plan already started from a freshly fetched `origin/main` (`a4c8440`) on its own branch, `feat/v0-2-pr9-conformance-harness`. Re-verify the current highest `docs/DECISIONS.md` D-number before recording new decisions — this plan assumes D-101 is the highest as of writing (2026-07-30); if a concurrent PR has since claimed a higher number, renumber this plan's own new decisions upward, per this project's established renumber-the-unmerged-branch convention (see D-085's own renumbering note for precedent).
- D-068: the pinned `ievo:deep-reviewer` reviews the full merge-base..HEAD diff before merge; actionable findings get fixed and re-reviewed.
- Every behavior change ships with its documentation update in the same commit (AGENTS.md's "Keep documentation current").
- `docs/PYTHON_STANDARDS.md`'s own policy: "Do not flip conformance statuses by hand; CI still owns the status column" — no automation actually exists yet to do this (verified: `scripts/check_roadmap_evidence.rb` only governs `docs/ROADMAP.md`'s own `[x]` markers, nothing governs `PYTHON_STANDARDS.md`'s ☐/⚙/✅ column). Task 1's ADR records the accepted interim policy: flip a row to ✅ only once its fixture is *observed green on a real, already-completed CI run across all 5 Tier-1 targets in both profiles* — never speculatively, and never before that CI evidence exists. This mirrors D-018/D-037/D-085's own defer-and-document precedent; building a `check_python_standards_evidence.rb`-style automated checker is explicitly out of scope for this plan.
- PEP 649/749's row is **not** part of this plan's scope — verified during planning (2026-07-30) that pycc's static, eager `Ty`-resolution model cannot compile any program that actually exercises PEP 649's deferred-evaluation semantics (see `docs/superpowers/specs/2026-07-28-v0-2-collections-generics-design.md`'s corrected §2 table for the full account). This drops PR-9's own fixture count from 10 to 9 and the v0.2-reachable pool from 16 to 15 against the ≥15 target — **zero margin remains**; flag this to whoever picks up PR-10 onward.

---

## Task 1: Record D-102 (harness architecture + status-column policy), no new crate

**Files:**
- Read: `tests/conformance.rs` (full, 141 lines — already read during planning, reproduced in context below)
- Read: `docs/PYTHON_STANDARDS.md` (full — already read during planning)
- Modify: `docs/DECISIONS.md` (append new `## D-102` entry + summary table row)

**Interfaces:**
- Produces: the accepted architecture (extend `tests/conformance.rs`, no `pycc_testkit` crate) and status-column policy every later task in this plan relies on.

`tests/conformance.rs` today has exactly 2 fixtures (`fib`, `mandelbrot`), each a ~10-line `#[test]` function calling a shared `run_conformance_fixture(label, py_path) -> (Vec<u8>, Vec<u8>)` helper (builds `--debug`, runs, runs the CPython oracle, returns both stdouts for the caller to `assert_eq!`). This project has twice already (D-018/D-037, then D-085 for PR-6's own 2 fixtures) deferred building a real `pycc_testkit` crate specifically because "there's no PEP matrix to check against yet" — one now exists (9 new fixtures), but the actual per-fixture logic needed is still just "build (both profiles now, see Task 2), run, diff against CPython" — exactly what `run_conformance_fixture` already provides. A crate would add new workspace-member `src/` code subject to D-014's 100%-coverage gate for what is fundamentally test glue, and this project's own established convention (noted in `tests/nbody_bench.rs`'s doc comments) is that `tests/*.rs` integration-test files intentionally do **not** share code via a `tests/common/mod.rs`-style module — each file stays self-contained. Growing to 11 total fixtures does not change this calculus: `run_conformance_fixture` (plus its Task-2 profile-aware extension) already eliminates the duplication that would justify a crate.

- [ ] **Step 1: Append the D-102 entry to `docs/DECISIONS.md`**

Insert immediately after the existing `## D-101: ...` entry (find it with `grep -n "^## D-101" docs/DECISIONS.md` first and confirm no `## D-102` already exists — if one does, renumber this entry to the next free number and update every reference below):

```markdown
## D-102: Extend `tests/conformance.rs` for PR-9's 9 new PEP fixtures; no `pycc_testkit` crate

- Status: accepted
- Context: PR-9 (`docs/DELIVERY_PLAN.md` row 9) needs to add 9 new PEP conformance fixtures (8 needing no new pycc feature, plus PEP 526's own new `Stmt::AnnAssign` support) to whatever harness proves each one's `--debug` and `--release` output matches pinned CPython 3.14.6. `docs/TESTING.md`'s "Conformance harness (`pycc_testkit`)" section describes an eventual v1.0-scale harness (cumulative fixture ranges per language level, CI auto-flipping `PYTHON_STANDARDS.md`'s status column). D-018/D-037/D-085 each deferred building that real crate "until there's a PEP matrix to check against" — this PR is the first time one exists, but its own actual needs (compile two profiles, run, diff) are no larger than what `tests/conformance.rs`'s existing `run_conformance_fixture` helper (D-085, PR-6) already provides for its 2 existing non-PEP fixtures.
- Decision: extend `tests/conformance.rs` in place with one `#[test]` function per new fixture (11 total after this PR: the existing `fib`/`mandelbrot` plus 9 new ones), each calling a profile-aware variant of `run_conformance_fixture` (Task 2) twice. No new `pycc_testkit` crate. `docs/PYTHON_STANDARDS.md`'s own "CI still owns the status column" policy has no automation behind it today (verified: `scripts/check_roadmap_evidence.rb` only governs `docs/ROADMAP.md`); this PR's interim, explicitly-accepted policy is that a row flips ☐→✅ only once its fixture is observed green on a real, already-completed CI run across all 5 Tier-1 targets in both profiles — recorded by hand, after the fact, never speculatively. Building real CI-driven automation for this column remains deferred, tracked as a `docs/ROADMAP.md` follow-up, exactly like D-018/D-037/D-085's own deferred-until-needed `pycc_testkit` crate itself.
- Alternatives: build the full `pycc_testkit` crate now, matching `docs/TESTING.md`'s complete v1.0 design (cumulative fixture ranges, header-comment metadata, CI-owned status flips) (rejected — that design's own "for each supported language level, select that configuration's cumulative fixture range" model doesn't exist as a concept anywhere in the codebase yet — v1.0 language-level selection, the multi-version `py30`-`py315` run structure, and automated status-column writes are all real, separate, unbuilt features; PR-9's actual job per `docs/DELIVERY_PLAN.md` row 9 is 9 fixtures, not that infrastructure). Build a minimal `pycc_testkit` crate that's just a thin directory-walking runner (rejected — with `run_conformance_fixture` already DRY via a shared helper function, the only thing a crate would add is a separate compilation unit and a new coverage-counted `src/` tree for code that is still, in substance, a test harness, not library logic other crates consume). Hand-flip `PYTHON_STANDARDS.md` marks speculatively as soon as a fixture passes locally (rejected — the file's own stated policy requires CI-observed evidence across all 5 targets in both profiles; local, single-machine passes are necessary but not sufficient, matching this project's own repeated insistence on real CI evidence over local claims elsewhere, e.g. D-095/D-096/D-101's nbody-gate methodology).
- Consequences: `tests/conformance.rs` grows from 141 lines / 2 fixtures to roughly 400+ lines / 11 fixtures by the end of this plan — still small enough to stay a single self-contained file, matching this project's own per-file-per-concern convention. `pycc_testkit` remains absent from the workspace, its `docs/ARCHITECTURE.md`/`docs/SPEC.md` references still describing a planned-but-unbuilt crate (unchanged by this decision). The real v1.0-scale harness `docs/TESTING.md` describes remains a future PR's own scope, to be designed fresh once the multi-version language-level-selection machinery it depends on actually exists.
```

- [ ] **Step 2: Add the summary table row**

Find the summary table's last row (`grep -n "^| D-101" docs/DECISIONS.md`) and append immediately after it:

```markdown
| D-102 | Extend `tests/conformance.rs` for PR-9's 9 new PEP fixtures (no `pycc_testkit` crate); `PYTHON_STANDARDS.md` status-column flips require observed all-target, both-profile CI evidence, recorded by hand pending real automation | accepted |
```

- [ ] **Step 3: Verify and commit**

```bash
grep -c "^## D-" docs/DECISIONS.md   # sanity check: should show one more heading than before
git add docs/DECISIONS.md
git commit -m "Record D-102: extend tests/conformance.rs for PR-9, no pycc_testkit crate"
```

No test run needed for this docs-only task — nothing compiles differently yet.

---

## Task 2: Dual-profile conformance harness extension

**Files:**
- Modify: `tests/conformance.rs:71-93` (the `run_conformance_fixture` function)
- Modify: `tests/conformance.rs:119-140` (the two existing `#[test]` functions — decide whether to leave them `--debug`-only or extend them too, see Step 3)

**Interfaces:**
- Consumes: nothing new from other tasks.
- Produces: `run_conformance_fixture_with_profile(label: &str, py_path: &Path, release: bool) -> (Vec<u8>, Vec<u8>)` — every later fixture task calls this twice per fixture (once `release: false`, once `release: true`). Each call runs the CPython oracle itself (the oracle's own output never depends on pycc's build profile, but the helper does not thread a shared oracle result between calls), so a fixture with both profiles spawns the oracle twice — an accepted small inefficiency (CPython startup on a short fixture is a few milliseconds) in exchange for keeping every call to this helper self-contained and independently callable, matching Step 1's exact signature below.

`docs/TESTING.md` line 28-29: a PEP row may only flip to ✅ "when green on all Tier-1 targets in both profiles," and the "v0.1 exception" (debug-only) "only binds from v0.2 on" — meaning it no longer applies, since `--release` now exists (PR-8 merged it). The existing `fib`/`mandelbrot` fixtures never surfaced this because neither is a PEP-matrix row (verified: neither appears anywhere in `docs/PYTHON_STANDARDS.md`'s tables) — they stay `--debug`-only, unaffected by this task.

- [ ] **Step 1: Add the profile-aware helper, keeping the original for `fib`/`mandelbrot`**

In `tests/conformance.rs`, replace the existing `run_conformance_fixture` function (lines 71-93) with two functions — the original unchanged (for `fib`/`mandelbrot`) plus a new profile-parameterized twin:

```rust
/// Builds `py_path` with `pycc build --debug` (the default profile), runs
/// the resulting binary, separately runs the pinned CPython oracle on the
/// identical source, and returns both stdouts for the caller to diff.
fn run_conformance_fixture(label: &str, py_path: &Path) -> (Vec<u8>, Vec<u8>) {
    run_conformance_fixture_with_profile(label, py_path, false)
}

/// Same as `run_conformance_fixture`, but lets the caller choose the build
/// profile. New PEP fixtures (PR-9 on) must be proven in both `--debug` and
/// `--release` before their `docs/PYTHON_STANDARDS.md` row can flip to ✅ --
/// `docs/TESTING.md`'s "both profiles" rule stopped having a v0.1-only
/// exception once `--release` shipped in PR-8. `fib`/`mandelbrot` predate
/// that rule (neither is a PEP-matrix row) and stay on the plain,
/// `--debug`-only helper above rather than being retrofitted here.
fn run_conformance_fixture_with_profile(
    label: &str,
    py_path: &Path,
    release: bool,
) -> (Vec<u8>, Vec<u8>) {
    let profile = if release { "release" } else { "debug" };
    let dir = std::env::temp_dir().join(format!(
        "pycc_conformance_{label}_{profile}_{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join(label);
    let mut build_command = Command::new(pycc_bin());
    build_command.args(["build", py_path.to_str().unwrap(), "-o", out.to_str().unwrap()]);
    if release {
        build_command.arg("--release");
    }
    let status = build_command.status().unwrap();
    assert!(status.success(), "`pycc build` ({profile}) failed for {label}");
    let pycc_output = Command::new(&out).output().unwrap();
    assert!(
        pycc_output.status.success(),
        "compiled {label} binary ({profile}) exited non-zero"
    );

    let cpython_output = Command::new(oracle_python_bin())
        .arg(py_path)
        .output()
        .unwrap();
    assert!(cpython_output.status.success(), "CPython oracle exited non-zero for {label}");

    (
        pycc_output.stdout,
        strip_windows_newline_translation(cpython_output.stdout),
    )
}
```

- [ ] **Step 2: Write the failing test proving both profiles are actually exercised**

Add this test (it doesn't need a real fixture file — it directly checks the two `Command` invocations differ only by the `--release` flag, by asserting on a fixture that's cheap to build twice: reuse `tests/fixtures/conformance_fib.py`, which already exists):

```rust
#[test]
#[ignore = "requires a pinned python3.14 (CPython 3.14.6) oracle on PATH"]
fn run_conformance_fixture_with_profile_builds_both_debug_and_release() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/conformance_fib.py");
    let (debug_pycc, debug_cpython) =
        run_conformance_fixture_with_profile("profile_check_debug", &fixture, false);
    let (release_pycc, release_cpython) =
        run_conformance_fixture_with_profile("profile_check_release", &fixture, true);
    assert_eq!(debug_pycc, debug_cpython, "debug profile must match CPython");
    assert_eq!(release_pycc, release_cpython, "release profile must match CPython");
    assert_eq!(
        debug_pycc, release_pycc,
        "debug and release builds of the same fixture must produce identical stdout"
    );
}
```

- [ ] **Step 2b: Run it to verify it fails first, then passes**

Run: `cargo build -p pycc && cargo test --test conformance -- --ignored run_conformance_fixture_with_profile_builds_both_debug_and_release`
Expected before Step 1's code exists: compile error (function not defined). After Step 1: `test result: ok. 1 passed`.

- [ ] **Step 3: Commit**

```bash
git add tests/conformance.rs
git commit -m "Add dual-profile conformance harness for PR-9's new PEP fixtures"
```

---

## Task 3: PEP 526 part 1 — `pycc_ast` re-export + `pycc_hir::HirStmt::AnnAssign` + lowering

**Files:**
- Modify: `crates/pycc_ast/src/lib.rs` (add `StmtAnnAssign` to the existing `pub use ruff_python_ast::{...}` list)
- Modify: `crates/pycc_hir/src/lib.rs` (new `HirStmt::AnnAssign` variant + `lower_stmt` arm + 5 other exhaustive-match arms this variant requires)

**Interfaces:**
- Consumes: `ruff_python_ast::StmtAnnAssign { target: Box<Expr>, annotation: Box<Expr>, value: Option<Box<Expr>>, simple: bool }` (already parses correctly — verified against the pinned `ruff_python_ast = "0.0.6"` registry source; no parser change needed).
- Produces: `HirStmt::AnnAssign { target: String, annotation: Ty, value: Option<HirExpr> }` — Task 4 (pycc_types) and Task 5 (pycc_mir/pycc_codegen) both match on this exact shape.

**Design decision this task records inline** (small enough not to need its own ADR, per this project's "record genuinely undecided forks" rule applied proportionately — this is a routine data-representation choice with a clear best answer, not a fork with real trade-offs on both sides): a **new, separate `HirStmt::AnnAssign` variant**, not an extra field bolted onto the existing `HirStmt::Assign`. Reasoning: `Assign`'s existing `value: HirExpr` field is *not* `Option` (every plain `x = 1` always has a value), so accommodating PEP 526's value-less `x: int` form inside `Assign` would require changing `Assign.value` itself to `Option<HirExpr>` — a far more invasive change touching every one of `Assign`'s existing consumers (`pycc_mir`, `pycc_codegen`, every test constructing `HirStmt::Assign { target, value }` literally). A separate variant costs a handful of small new match arms instead (enumerated below) and mirrors upstream `ruff_python_ast` itself treating `StmtAssign`/`StmtAnnAssign` as distinct node types.

- [ ] **Step 1: `pycc_ast` re-export (one line)**

In `crates/pycc_ast/src/lib.rs`, find the `pub use ruff_python_ast::{...}` block (starts at line 1) and add `StmtAnnAssign` alphabetically to the list, right after `Arguments`:

```rust
pub use ruff_python_ast::{
    Arguments, CmpOp, ConversionFlag, ElifElseClause, Expr, ExprBinOp, ExprBooleanLiteral,
    ExprCall, ExprCompare, ExprContext, ExprFString, ExprName, ExprNumberLiteral,
    ExprStringLiteral, ExprUnaryOp, Identifier, InterpolatedElement, InterpolatedStringElement,
    InterpolatedStringLiteralElement, ModModule, Number, Operator, Parameters, Stmt, StmtAnnAssign,
    StmtAssign, StmtExpr, StmtFor, StmtFunctionDef, StmtIf, StmtReturn, StmtWhile, UnaryOp,
};
```

Run: `cargo build -p pycc_ast` — expected: succeeds (this is a pure addition, nothing consumes it yet).

- [ ] **Step 2: Write the failing HIR lowering test**

In `crates/pycc_hir/src/lib.rs`'s `#[cfg(test)] mod tests` block, add two tests — one for the valued form, one for the value-less form:

```rust
#[test]
fn lowers_an_annotated_assignment_with_a_value() {
    let module = pycc_parser_test_helper::parse("x: int = 1\nprint(x)\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![
            HirItem::TopLevelStmt(HirStmt::AnnAssign {
                target: "x".to_string(),
                annotation: Ty::Int,
                value: Some(HirExpr::IntLiteral(1)),
            }),
            HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::Name("x".to_string())],
            })),
        ]
    );
}

#[test]
fn lowers_an_annotated_assignment_with_no_value() {
    let module = pycc_parser_test_helper::parse("x: int\n");
    let hir = lower_checked(&module).unwrap();
    assert_eq!(
        hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::AnnAssign {
            target: "x".to_string(),
            annotation: Ty::Int,
            value: None,
        })]
    );
}

#[test]
fn rejects_an_annotated_assignment_to_a_non_name_target() {
    // Matches Stmt::Assign's own existing restriction (only a bare name target
    // is supported so far) -- e.g. `obj.attr: int = 1` has no attribute-access
    // support anywhere else in the compiler either.
    assert_capability_error_message(
        "obj.attr: int = 1\n",
        "only assigning to a bare name is supported so far",
    );
}
```

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p pycc_hir lowers_an_annotated_assignment`
Expected: compile error (`HirStmt::AnnAssign` doesn't exist yet).

- [ ] **Step 4: Add the `HirStmt::AnnAssign` variant**

In `crates/pycc_hir/src/lib.rs`, extend the `HirStmt` enum (currently at lines 79-102):

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum HirStmt {
    ExprStmt(HirExpr),
    Assign {
        target: String,
        value: HirExpr,
    },
    AnnAssign {
        target: String,
        annotation: Ty,
        value: Option<HirExpr>,
    },
    If {
        test: HirExpr,
        body: Vec<HirStmt>,
        orelse: Vec<HirStmt>,
    },
    While {
        test: HirExpr,
        body: Vec<HirStmt>,
    },
    ForRange {
        var: String,
        start: HirExpr,
        stop: HirExpr,
        step: HirExpr,
        body: Vec<HirStmt>,
    },
    Return(Option<HirExpr>),
}
```

- [ ] **Step 5: Add the `lower_stmt` arm**

In `lower_stmt`'s match (currently starting at line 270), add a new arm right after the existing `Stmt::Assign(assign) => { ... }` arm:

```rust
Stmt::AnnAssign(ann) => {
    let Expr::Name(name) = ann.target.as_ref() else {
        return Err(unsupported(
            format!("only assigning to a bare name is supported so far: {:?}", ann.target),
            pycc_ast::expr_range(&ann.target),
        ));
    };
    let annotation = annotation_to_ty(&ann.annotation)?;
    let value = ann
        .value
        .as_ref()
        .map(|v| lower_expr(v))
        .transpose()?;
    HirStmt::AnnAssign {
        target: name.id.as_str().to_string(),
        annotation,
        value,
    }
}
```

- [ ] **Step 6: Confirm `pycc_hir` itself needs no further changes**

Run `cargo build -p pycc_hir`. This crate itself has exactly one exhaustive match over `HirStmt` (`lower_stmt`, already given its new arm in Step 5) — verified during planning that all *other* exhaustive `HirStmt` matches needing a new arm live outside this crate: 1 in `pycc_mir::lower_stmt` (Task 5) and 6 in `pycc_types` (`collect_local_names`, `collect_block_constraints`, `contains_return`, `block_always_returns`, `check_stmt`, `check_stmt_in_function` — all Task 4). Expected: `cargo build -p pycc_hir` succeeds with no errors after Step 5 alone. If it doesn't (a new exhaustive match was missed during planning), add the arm the compiler names, matching the shape of its neighboring `Assign`/`ExprStmt` arms.

- [ ] **Step 7: Run tests, verify green**

Run: `cargo test -p pycc_hir`
Expected: all pass, including the 3 new tests from Step 2.

- [ ] **Step 8: Commit**

```bash
git add crates/pycc_ast/src/lib.rs crates/pycc_hir/src/lib.rs
git commit -m "PEP 526 part 1: parse+lower x: int and x: int = 1 into HirStmt::AnnAssign"
```

---

## Task 4: PEP 526 part 2 — `pycc_types` validation (new `T0025`, reused `is_assignable`, `T0021` parity for the value-less form)

**Files:**
- Modify: `crates/pycc_types/src/lib.rs` (new match arms in `collect_local_names`, `collect_block_constraints`, `contains_return`, `block_always_returns`, `check_stmt`, `check_stmt_in_function`)
- Modify: `docs/DIAGNOSTICS.md` (register `T0025`)
- Create: `tests/diagnostics/d0025_annotated_assignment_mismatch.py`, `tests/diagnostics/d0025_annotated_assignment_mismatch.expected.txt`
- Create: `tests/diagnostics/d0026_annotation_only_unbound.py`, `tests/diagnostics/d0026_annotation_only_unbound.expected.txt` (proves the value-less form correctly reuses the *existing* `T0021` mechanism — no new code needed for this, this fixture is regression protection proving Task 4 didn't accidentally break or bypass it)
- Modify: `tests/diagnostics_test.rs` (2 new test functions)

**Interfaces:**
- Consumes: `HirStmt::AnnAssign { target, annotation, value }` from Task 3; `annotation_to_ty` (already exists, `pycc_hir`); `is_assignable` (already exists, `pycc_types::lib.rs:801-803`).
- Produces: nothing new downstream — Task 5 only needs to know that a *type-checked* `HirStmt::AnnAssign` is guaranteed to have its `value` (if `Some`) already validated compatible with `annotation`.

**Design decisions this task makes, verified against the actual current code (not guessed):**

1. **Directly annotated assignments are validated the same way plain reassignment already is**: `is_assignable(inferred_value_ty, annotation_ty)` — the exact same helper `check_assignment`'s `T0023` path already uses for "is the new value compatible with the established type," so `x: int = True` is **accepted** (bool→int is already an accepted widening everywhere else in this type system) and `x: int = "hello"` is **rejected**. This is a new code, `T0025`, not a reuse of `T0023`, because the message needs different phrasing ("declared as X, initializer is Y" vs. `T0023`'s "previously inferred as X").
2. **On success, `env.bind(target, annotation)` — bind the *annotation's* type, not the inferred value's type** (relevant for `x: int = True`, which should record `x` as `Ty::Int` going forward, not `Ty::Bool`, since the declared annotation is what governs the name for the rest of its scope).
3. **The value-less form (`x: int`) must *not* call `env.bind`.** Verified directly: `env.lookup(name).ok_or_else(|| if is_local(...) { unbound_local(name) } else { ... })` (`infer_expr_in`, `lib.rs:700-707`) is the exact mechanism that already produces `T0021` for "referenced before assigned within this function" (e.g. the existing `d0021_unbound_local.py` fixture). As long as `collect_local_names` registers `x` as local (Step 1 below) but the `AnnAssign` value-less arm never calls `env.bind`, any later premature use of `x` automatically falls through to the *already-existing* `T0021` path with zero new diagnostic logic — this is directly analogous to CPython's own real behavior (`x: int` then `print(x)` raises `UnboundLocalError`, verified empirically against CPython 3.14.6 during planning).
4. **Known, deliberate scope limit** (document this, don't silently drop it): if `x: int` (no value) is followed later by a *plain*, unannotated `x = "hello"`, this PR does not check that reassignment against the earlier annotation (since nothing durably records "declared but unbound" separately from "bound" in `Environment` today — building that split is real, separate type-system depth, out of scope for "PEP 526's own small, bounded addition" per the design doc). Only the fixture-relevant path (`x: int = 1`, a single combined declare+bind) is required for PR-9's own conformance fixture; the value-less form's own correctness is proven only for the `T0021`-parity case above, not for this durable-annotation edge case.

- [ ] **Step 1: `collect_local_names` — register the target as local (with or without a value)**

In `crates/pycc_types/src/lib.rs`, add an arm to `collect_local_names` (currently lines 70-91), right after the existing `HirStmt::Assign { target, .. }` arm:

```rust
HirStmt::AnnAssign { target, .. } => {
    if !is_local(names, target) {
        names.push(target);
    }
}
```

- [ ] **Step 2: `contains_return`/`block_always_returns` — trivial `false` arms**

`contains_return` (lib.rs, shown above): add `HirStmt::AnnAssign { .. } => false,` alongside the existing `HirStmt::ExprStmt(_) | HirStmt::Assign { .. } => false,` arm (fold it into that same arm: `HirStmt::ExprStmt(_) | HirStmt::Assign { .. } | HirStmt::AnnAssign { .. } => false,`).

Find `block_always_returns` (`cargo build -p pycc_types` after Step 1 will point at it as a compile error) and add the identical trivial `false` arm, matching whatever style its neighboring `Assign`/`ExprStmt` arms already use.

- [ ] **Step 3: `collect_block_constraints` — the constraint-solver pre-pass (private-helper inference path)**

This function (lib.rs:317+) is the *separate* constraint-collection solver used only for "private-helper inference variables" (per `docs/ROADMAP.md`'s Quality-gates row) — narrower than the main checking path in Steps 5-6 below. Add, right after the existing `HirStmt::Assign { target, value } => { ... env.bindings.entry(target.clone()).or_insert(term); }` arm:

```rust
HirStmt::AnnAssign { target, value: Some(value), .. } => {
    if let Some(term) =
        collect_expr_constraints(signatures, parents, concrete, binops, env, value)?
    {
        env.bindings.entry(target.clone()).or_insert(term);
    }
}
HirStmt::AnnAssign { value: None, .. } => {}
```

- [ ] **Step 4: Register `T0025` in `docs/DIAGNOSTICS.md`**

Add a row to the `T0xxx` table (after the existing `T0024` row):

```markdown
| `T0025` | error | annotated-assignment initializer incompatible with its declared annotation |
```

- [ ] **Step 5: Write the failing diagnostic-mismatch test**

`tests/diagnostics/d0025_annotated_assignment_mismatch.py`:
```python
x: int = "not an int"
```

`tests/diagnostics/d0025_annotated_assignment_mismatch.expected.txt`:
```
error[T0025]: cannot assign `str` to `x: int`, initializer does not match the declared annotation
 --> tests/diagnostics/d0025_annotated_assignment_mismatch.py:1:1
  |
1 | x: int = "not an int"
  | ^ cannot assign `str` to `x: int`, initializer does not match the declared annotation
```

(The exact span/caret position must match whatever convention the neighboring `d0023_incompatible_assignment.expected.txt` uses — copy its exact caret-and-pipe formatting, adjusting only the message text and line/col numbers for this fixture.)

Add to `tests/diagnostics_test.rs`, matching the existing `d0023`-style test function exactly:

```rust
#[test]
fn d0025_annotated_assignment_mismatch() {
    assert_diagnostic_matches_fixture("d0025_annotated_assignment_mismatch");
}
```

- [ ] **Step 6: Write the failing `T0021`-parity regression test**

`tests/diagnostics/d0026_annotation_only_unbound.py`:
```python
def f() -> None:
    x: int
    print(x)


f()
```

`tests/diagnostics/d0026_annotation_only_unbound.expected.txt`: every `pycc_types` diagnostic today constructs its `Span` as `Span::new(0, 0)` (see `infer_expr_in`'s own comment: "real span threading through HIR is out of scope") — so T0021 always renders at `1:1` quoting the fixture's *first* line, regardless of which line the unbound use is actually on. Confirmed against the existing `d0021_unbound_local.py`/`.expected.txt` pair: the real error is `print(x)` on line 4, but the expected file points at `1:1` and quotes line 1's `x = 1`. Do not "fix" this to point at the real line — that would require span-threading work this task is explicitly not doing. This fixture's own line 1 is `def f() -> None:`, so:
```
error[T0021]: local name `x` is not bound before this use
 --> tests/diagnostics/d0026_annotation_only_unbound.py:1:1
  |
1 | def f() -> None:
  | ^ local name `x` is not bound before this use
```

Add to `tests/diagnostics_test.rs`:
```rust
#[test]
fn d0026_annotation_only_unbound() {
    assert_diagnostic_matches_fixture("d0026_annotation_only_unbound");
}
```

- [ ] **Step 7: Run to verify both new diagnostic tests fail**

Run: `cargo build -p pycc && cargo test --test diagnostics_test d0025 d0026`
Expected: both fail — either a compile error (if Steps 1-3 aren't done yet) or `C0001` instead of `T0025`/`T0021` (once Steps 1-3 land but before Steps 8-9 below).

- [ ] **Step 8: Add the module-scope `check_stmt` arm**

Find `check_stmt`'s match (module/top-level scope) and add, right after its existing `HirStmt::Assign { target, value } => { ... }` arm:

```rust
HirStmt::AnnAssign { target, annotation, value } => {
    if let Some(value) = value {
        let inferred = infer_expr(env, value)?;
        if !is_assignable(inferred, *annotation) {
            return Err(Diagnostic::error(
                "T0025",
                format!(
                    "cannot assign `{}` to `{target}: {}`, initializer does not match the declared annotation",
                    inferred.name(),
                    annotation.name()
                ),
                Span::new(0, 0),
            ));
        }
        env.bind(target.clone(), *annotation);
    }
    // No value: register no binding, matching CPython's own "declared, not yet
    // assigned" semantics -- collect_local_names (Step 1) already marked
    // `target` local, so a premature read still raises the existing T0021.
    Ok(())
}
```

- [ ] **Step 9: Add the function-scope `check_stmt_in_function` arm**

Find the analogous function-body arm (`HirStmt::Assign { target, value } => { let ty = infer_expr_in(env, local_names, value)?; check_assignment(env, target, ty) }`) and add the same shape, using `infer_expr_in(env, local_names, value)` instead of `infer_expr(env, value)`:

```rust
HirStmt::AnnAssign { target, annotation, value } => {
    if let Some(value) = value {
        let inferred = infer_expr_in(env, local_names, value)?;
        if !is_assignable(inferred, *annotation) {
            return Err(Diagnostic::error(
                "T0025",
                format!(
                    "cannot assign `{}` to `{target}: {}`, initializer does not match the declared annotation",
                    inferred.name(),
                    annotation.name()
                ),
                Span::new(0, 0),
            ));
        }
        env.bind(target.clone(), *annotation);
    }
    Ok(())
}
```

- [ ] **Step 10: Run everything, verify green**

Run: `cargo test -p pycc_types && cargo build -p pycc && cargo test --test diagnostics_test`
Expected: all pass, including both new `d0025`/`d0026` tests.

- [ ] **Step 11: Add unit tests for the two new `pycc_types` code paths directly** (D-014 branch coverage for both the pass and fail sides of `is_assignable` inside the new arms, and both the `Some`/`None` value branches)

In `crates/pycc_types/src/lib.rs`'s own `#[cfg(test)]` module, add (matching the existing `an_incompatible_reassignment_is_t0023_and_preserves_the_inferred_type`-style test at lib.rs:1717-1738):

```rust
#[test]
fn an_annotated_assignment_with_a_matching_value_binds_the_annotation_type() {
    // x: int = True -- bool is assignable to int (matches is_assignable's
    // existing widening rule), and the environment should record Ty::Int
    // (the annotation), not Ty::Bool (the initializer's own inferred type).
    let module = pycc_parser_test_helper::parse("x: int = True\nx = 5\n");
    let hir = lower_checked(&module).unwrap();
    assert!(check(&hir).is_ok());
}

#[test]
fn an_annotated_assignment_with_a_mismatched_value_is_t0025() {
    let module = pycc_parser_test_helper::parse("x: int = \"nope\"\n");
    let hir = lower_checked(&module).unwrap();
    let err = check(&hir).unwrap_err();
    assert_eq!(err.code, "T0025");
}

#[test]
fn an_annotation_only_declaration_does_not_bind_a_value() {
    // x: int alone must not make a later use of x succeed -- it only
    // declares x local, it does not bind it (matching CPython's own
    // UnboundLocalError for this exact shape, verified during planning).
    let module = pycc_parser_test_helper::parse(
        "def f() -> None:\n    x: int\n    print(x)\n\n\nf()\n",
    );
    let hir = lower_checked(&module).unwrap();
    let err = check(&hir).unwrap_err();
    assert_eq!(err.code, "T0021");
}
```

(Adjust `lower_checked`/`check`'s exact call shape to match whatever this file's own existing tests already use — copy the pattern from the neighboring `T0023` test exactly rather than guessing a new one.)

Run: `cargo test -p pycc_types`. Expected: all pass.

- [ ] **Step 12: Commit**

```bash
git add crates/pycc_types/src/lib.rs docs/DIAGNOSTICS.md tests/diagnostics/d0025_annotated_assignment_mismatch.py tests/diagnostics/d0025_annotated_assignment_mismatch.expected.txt tests/diagnostics/d0026_annotation_only_unbound.py tests/diagnostics/d0026_annotation_only_unbound.expected.txt tests/diagnostics_test.rs
git commit -m "PEP 526 part 2: validate annotated assignments (T0025), preserve T0021 for the value-less form"
```

---

## Task 5: PEP 526 part 3 — `pycc_mir`/`pycc_codegen` (new `MirStmt::NoOp`, end-to-end compiled-and-run test)

**Files:**
- Modify: `crates/pycc_mir/src/lib.rs` (new `MirStmt::NoOp` variant + `lower_stmt` arm)
- Modify: `crates/pycc_codegen/src/lib.rs` (2 new trivial match arms: `emit_stmt`, `collect_stmt_bindings`)
- Create: `tests/fixtures/pep_0526_var_annotations_smoke.py` (a throwaway smoke fixture for this task's own end-to-end test; the *real* conformance fixture is Task 6)

**Interfaces:**
- Consumes: `HirStmt::AnnAssign { target, annotation, value }` (Task 3), already validated by `pycc_types` (Task 4) by the time it reaches this crate.
- Produces: nothing new downstream — this closes the loop, `x: int = 1` now compiles *and runs* correctly end to end.

**Design decision, verified against the actual current signature:** `pycc_mir::lower_stmt(stmt: &HirStmt, scopes: &mut Vec<HashMap<String, Ty>>) -> MirStmt` is a strict 1:1 mapping (every caller does `body.iter().map(|s| lower_stmt(s, scopes)).collect()`). A value-less `x: int` has no runtime action at all (CPython itself does nothing observable for it, verified — it's purely a static declaration). Rather than changing `lower_stmt`'s signature to a 1:0-or-1 `flat_map` shape (which would touch all 4 of its recursive call sites), this task adds one new, trivial `MirStmt::NoOp` variant instead — smaller, more contained, keeps the existing 1:1 mapping intact everywhere else.

- [ ] **Step 1: Write the failing end-to-end test fixture**

`tests/fixtures/pep_0526_var_annotations_smoke.py`:
```python
def f() -> int:
    x: int = 1
    y: int
    y = 2
    return x + y


print(f())
```

Add a test to `tests/slice1_codegen_depth.rs` (matching that file's own existing style for "compile this fixture, run it, assert the exact stdout" tests — read a neighboring test in that file first for the exact `Command`-invocation pattern used there):

```rust
#[test]
fn pep_0526_annotated_assignments_compile_and_run_correctly() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/pep_0526_var_annotations_smoke.py");
    // ... build with pycc_bin(), run, assert stdout == b"3\n" ...
    // (copy the exact build-then-run-then-assert shape from this file's
    // nearest existing single-fixture test rather than reinventing it)
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo build -p pycc && cargo test --test slice1_codegen_depth pep_0526`
Expected: build failure (`x: int` still hits `C0001` until Task 3/4 land) or, if Tasks 3-4 are already merged into this same branch by the time this task runs, a panic in `emit_assign` (`"every assignment target must have a predeclared storage slot"`) once `x: int` reaches codegen with no `MirStmt::NoOp` to lower into.

- [ ] **Step 3: Add `MirStmt::NoOp`**

In `crates/pycc_mir/src/lib.rs`, find the `MirStmt` enum (mirrors `HirStmt`'s shape) and add a new unit variant:

```rust
pub enum MirStmt {
    ExprStmt(MirExpr),
    Assign { target: String, value: MirExpr },
    /// A statement with zero runtime effect -- currently only produced by a
    /// value-less PEP 526 annotation (`x: int`), which CPython itself does
    /// nothing observable for either (confirmed empirically during PR-9
    /// planning: no store, no allocation, nothing an oracle diff could see).
    NoOp,
    If { test: MirExpr, body: Vec<MirStmt>, orelse: Vec<MirStmt> },
    While { test: MirExpr, body: Vec<MirStmt> },
    ForRange { var: String, start: MirExpr, stop: MirExpr, step: MirExpr, body: Vec<MirStmt> },
    Return(Option<MirExpr>),
}
```

- [ ] **Step 4: Add the `lower_stmt` arm**

In `crates/pycc_mir/src/lib.rs`'s `lower_stmt` (shown in full during planning, lines 190-231), add right after the existing `HirStmt::Assign { target, value } => { ... }` arm:

```rust
HirStmt::AnnAssign { target, value: Some(value), .. } => {
    let value = lower_expr(value, scopes);
    bind_variable(scopes, target.clone(), value.ty());
    MirStmt::Assign {
        target: target.clone(),
        value,
    }
}
HirStmt::AnnAssign { value: None, .. } => MirStmt::NoOp,
```

(No `bind_variable` call for the value-less arm — matches `pycc_types`' own choice in Task 4 not to bind a value-less declaration; `pycc_mir`'s `lookup` panics if a name is read with no scope entry, but since `pycc_types` already rejects any premature read via `T0021` before this code ever runs, no successfully-type-checked program can reach `pycc_mir` with an unbound read.)

- [ ] **Step 5: Add the 2 trivial `pycc_codegen` arms**

In `crates/pycc_codegen/src/lib.rs`'s `emit_stmt` (dispatch match, ~line 1955), add right after the `MirStmt::Assign { ... } => { ... }` arm:

```rust
MirStmt::NoOp => Ok(()),
```

In `collect_stmt_bindings` (~line 1308, the alloca pre-pass that recurses into nested control flow), add:

```rust
MirStmt::NoOp => {}
```

- [ ] **Step 6: Run to verify green**

Run: `cargo test -p pycc_mir -p pycc_codegen && cargo build -p pycc && cargo test --test slice1_codegen_depth pep_0526`
Expected: all pass; `print(f())` outputs `3`.

- [ ] **Step 7: Add unit tests for the new `MirStmt::NoOp` path directly** (D-014 branch coverage — the end-to-end test above covers it too, but crate-internal unit tests are this project's own established convention alongside integration tests)

In `crates/pycc_mir/src/lib.rs`'s own tests, add a lowering test for both `HirStmt::AnnAssign` arms (with value → `MirStmt::Assign`; without → `MirStmt::NoOp`), matching the file's existing lowering-test style.

In `crates/pycc_codegen/src/lib.rs`'s own tests, add one hand-constructed `MirModule` test containing a `MirStmt::NoOp` (matching this file's own established "hand-construct a `MirModule` directly, bypassing HIR/types" test convention, per Task 5's own planning research) proving it emits successfully with no panic and produces no observable output.

Run: `cargo test -p pycc_mir -p pycc_codegen`. Expected: all pass.

- [ ] **Step 8: Commit**

```bash
git add crates/pycc_mir/src/lib.rs crates/pycc_codegen/src/lib.rs tests/fixtures/pep_0526_var_annotations_smoke.py tests/slice1_codegen_depth.rs
git commit -m "PEP 526 part 3: MirStmt::NoOp for value-less annotations, end-to-end compile+run"
```

---

## Task 6: PEP 526's own conformance fixture

**Files:**
- Create: `tests/fixtures/pep_0526_var_annotations.py`
- Modify: `tests/conformance.rs` (1 new `#[test]` function)

**Interfaces:**
- Consumes: Tasks 3-5's completed PEP 526 support; `run_conformance_fixture_with_profile` (Task 2).

- [ ] **Step 1: Write the fixture, verified empirically first**

`tests/fixtures/pep_0526_var_annotations.py`:
```python
def compute(base: int) -> int:
    doubled: int = base * 2
    total: int
    total = doubled + base
    return total


print(compute(5))
```

Verify locally before committing (this exact sequence, matching this project's own "verify empirically" convention):
```bash
cargo build -p pycc
./target/debug/pycc check tests/fixtures/pep_0526_var_annotations.py   # expect: exit 0
./target/debug/pycc build tests/fixtures/pep_0526_var_annotations.py -o /tmp/pep526_check
/tmp/pep526_check                                                       # expect: 15
python3.14 tests/fixtures/pep_0526_var_annotations.py                    # expect: 15
```

- [ ] **Step 2: Write the failing test**

Add to `tests/conformance.rs`, matching the existing `fib_matches_cpython_3_14_6_byte_for_byte`-style `#[test]` shape exactly, but calling the new dual-profile helper twice:

```rust
#[test]
#[ignore = "requires a pinned python3.14 (CPython 3.14.6) oracle on PATH"]
fn pep_0526_var_annotations_matches_cpython_3_14_6_byte_for_byte() {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pep_0526_var_annotations.py");
    let (debug_pycc, debug_cpython) =
        run_conformance_fixture_with_profile("pep_0526_var_annotations_debug", &fixture, false);
    assert_eq!(
        debug_pycc, debug_cpython,
        "pycc (--debug) and CPython 3.14.6 disagree on tests/fixtures/pep_0526_var_annotations.py"
    );
    let (release_pycc, release_cpython) =
        run_conformance_fixture_with_profile("pep_0526_var_annotations_release", &fixture, true);
    assert_eq!(
        release_pycc, release_cpython,
        "pycc (--release) and CPython 3.14.6 disagree on tests/fixtures/pep_0526_var_annotations.py"
    );
}
```

- [ ] **Step 3: Run it**

Run: `cargo test --test conformance -- --ignored pep_0526`
Expected: `test result: ok. 1 passed`.

- [ ] **Step 4: Commit**

```bash
git add tests/fixtures/pep_0526_var_annotations.py tests/conformance.rs
git commit -m "Add PEP 526 conformance fixture (variable annotations)"
```

---

## Task 7: No-new-work PEP fixtures, batch 1 (PEP 238, 3105, 3107)

**Files:**
- Create: `tests/fixtures/pep_0238_division.py`, `tests/fixtures/pep_3105_print.py`, `tests/fixtures/pep_3107_annotations.py`
- Modify: `tests/conformance.rs` (3 new `#[test]` functions)

Each of these 3 fixtures was already built, run in both `--debug` and `--release`, and diffed byte-for-byte against CPython 3.14.6 during this plan's own writing (2026-07-30) — all matched exactly. Their exact, pre-verified content:

`tests/fixtures/pep_0238_division.py`:
```python
def divide(a: float, b: float) -> float:
    return a / b


def floor_divide(a: int, b: int) -> int:
    return a // b


print(divide(7.0, 2.0))
print(floor_divide(7, 2))
```
Verified output (both profiles, matches CPython exactly): `3.5\n3\n`

`tests/fixtures/pep_3105_print.py`:
```python
def greet(name: str) -> None:
    print("hello", name)


greet("world")
```
Verified output: `hello world\n`

(Note: `print(..., sep=..., end=...)` was tried first and rejected by pycc with `keyword call arguments are not supported yet` — PEP 3105's own core claim is just "`print` is a function, not a statement," which this positional-only form already demonstrates fully; keyword arguments are an unrelated, separate gap, not part of this PEP's own scope.)

`tests/fixtures/pep_3107_annotations.py`:
```python
def add(a: int, b: int) -> int:
    return a + b


print(add(2, 3))
```
Verified output: `5\n`

- [ ] **Step 1: Create the 3 fixture files** (exact content above)

- [ ] **Step 2: Add 3 `#[test]` functions to `tests/conformance.rs`**, each following Task 6 Step 2's exact dual-profile pattern (build debug, compare; build release, compare):

```rust
#[test]
#[ignore = "requires a pinned python3.14 (CPython 3.14.6) oracle on PATH"]
fn pep_0238_division_matches_cpython_3_14_6_byte_for_byte() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pep_0238_division.py");
    let (debug_pycc, debug_cpython) =
        run_conformance_fixture_with_profile("pep_0238_division_debug", &fixture, false);
    assert_eq!(debug_pycc, debug_cpython, "pycc (--debug) and CPython 3.14.6 disagree on tests/fixtures/pep_0238_division.py");
    let (release_pycc, release_cpython) =
        run_conformance_fixture_with_profile("pep_0238_division_release", &fixture, true);
    assert_eq!(release_pycc, release_cpython, "pycc (--release) and CPython 3.14.6 disagree on tests/fixtures/pep_0238_division.py");
}

#[test]
#[ignore = "requires a pinned python3.14 (CPython 3.14.6) oracle on PATH"]
fn pep_3105_print_matches_cpython_3_14_6_byte_for_byte() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pep_3105_print.py");
    let (debug_pycc, debug_cpython) =
        run_conformance_fixture_with_profile("pep_3105_print_debug", &fixture, false);
    assert_eq!(debug_pycc, debug_cpython, "pycc (--debug) and CPython 3.14.6 disagree on tests/fixtures/pep_3105_print.py");
    let (release_pycc, release_cpython) =
        run_conformance_fixture_with_profile("pep_3105_print_release", &fixture, true);
    assert_eq!(release_pycc, release_cpython, "pycc (--release) and CPython 3.14.6 disagree on tests/fixtures/pep_3105_print.py");
}

#[test]
#[ignore = "requires a pinned python3.14 (CPython 3.14.6) oracle on PATH"]
fn pep_3107_annotations_matches_cpython_3_14_6_byte_for_byte() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pep_3107_annotations.py");
    let (debug_pycc, debug_cpython) =
        run_conformance_fixture_with_profile("pep_3107_annotations_debug", &fixture, false);
    assert_eq!(debug_pycc, debug_cpython, "pycc (--debug) and CPython 3.14.6 disagree on tests/fixtures/pep_3107_annotations.py");
    let (release_pycc, release_cpython) =
        run_conformance_fixture_with_profile("pep_3107_annotations_release", &fixture, true);
    assert_eq!(release_pycc, release_cpython, "pycc (--release) and CPython 3.14.6 disagree on tests/fixtures/pep_3107_annotations.py");
}
```

- [ ] **Step 3: Run**

Run: `cargo test --test conformance -- --ignored pep_0238 pep_3105 pep_3107`
Expected: `test result: ok. 3 passed`.

- [ ] **Step 4: Commit**

```bash
git add tests/fixtures/pep_0238_division.py tests/fixtures/pep_3105_print.py tests/fixtures/pep_3107_annotations.py tests/conformance.rs
git commit -m "Add PEP 238/3105/3107 conformance fixtures"
```

---

## Task 8: No-new-work PEP fixtures, batch 2 (PEP 3131, 414, 484)

**Files:**
- Create: `tests/fixtures/pep_3131_unicode_ids.py`, `tests/fixtures/pep_0414_u_literal.py`, `tests/fixtures/pep_0484_type_hints.py`
- Modify: `tests/conformance.rs` (3 new `#[test]` functions)

All 3 pre-verified (both profiles, byte-for-byte against CPython 3.14.6) during this plan's own writing.

`tests/fixtures/pep_3131_unicode_ids.py`:
```python
def compute() -> int:
    café = 5
    naïve = 3
    return café + naïve


print(compute())
```
Verified output: `8\n`

`tests/fixtures/pep_0414_u_literal.py`:
```python
def greeting() -> str:
    return u"hello"


print(greeting())
```
Verified output: `hello\n`

`tests/fixtures/pep_0484_type_hints.py`:
```python
def scale(value: float, factor: float) -> float:
    return value * factor


print(scale(2.5, 4.0))
```
Verified output: `10.0\n`

- [ ] **Step 1: Create the 3 fixture files** (exact content above)

- [ ] **Step 2: Add 3 `#[test]` functions**, following Task 7's exact dual-profile pattern (substitute each fixture's own name/label consistently):

```rust
#[test]
#[ignore = "requires a pinned python3.14 (CPython 3.14.6) oracle on PATH"]
fn pep_3131_unicode_ids_matches_cpython_3_14_6_byte_for_byte() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pep_3131_unicode_ids.py");
    let (debug_pycc, debug_cpython) =
        run_conformance_fixture_with_profile("pep_3131_unicode_ids_debug", &fixture, false);
    assert_eq!(debug_pycc, debug_cpython, "pycc (--debug) and CPython 3.14.6 disagree on tests/fixtures/pep_3131_unicode_ids.py");
    let (release_pycc, release_cpython) =
        run_conformance_fixture_with_profile("pep_3131_unicode_ids_release", &fixture, true);
    assert_eq!(release_pycc, release_cpython, "pycc (--release) and CPython 3.14.6 disagree on tests/fixtures/pep_3131_unicode_ids.py");
}

#[test]
#[ignore = "requires a pinned python3.14 (CPython 3.14.6) oracle on PATH"]
fn pep_0414_u_literal_matches_cpython_3_14_6_byte_for_byte() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pep_0414_u_literal.py");
    let (debug_pycc, debug_cpython) =
        run_conformance_fixture_with_profile("pep_0414_u_literal_debug", &fixture, false);
    assert_eq!(debug_pycc, debug_cpython, "pycc (--debug) and CPython 3.14.6 disagree on tests/fixtures/pep_0414_u_literal.py");
    let (release_pycc, release_cpython) =
        run_conformance_fixture_with_profile("pep_0414_u_literal_release", &fixture, true);
    assert_eq!(release_pycc, release_cpython, "pycc (--release) and CPython 3.14.6 disagree on tests/fixtures/pep_0414_u_literal.py");
}

#[test]
#[ignore = "requires a pinned python3.14 (CPython 3.14.6) oracle on PATH"]
fn pep_0484_type_hints_matches_cpython_3_14_6_byte_for_byte() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pep_0484_type_hints.py");
    let (debug_pycc, debug_cpython) =
        run_conformance_fixture_with_profile("pep_0484_type_hints_debug", &fixture, false);
    assert_eq!(debug_pycc, debug_cpython, "pycc (--debug) and CPython 3.14.6 disagree on tests/fixtures/pep_0484_type_hints.py");
    let (release_pycc, release_cpython) =
        run_conformance_fixture_with_profile("pep_0484_type_hints_release", &fixture, true);
    assert_eq!(release_pycc, release_cpython, "pycc (--release) and CPython 3.14.6 disagree on tests/fixtures/pep_0484_type_hints.py");
}
```

- [ ] **Step 3: Run**

Run: `cargo test --test conformance -- --ignored pep_3131 pep_0414 pep_0484`
Expected: `test result: ok. 3 passed`.

- [ ] **Step 4: Commit**

```bash
git add tests/fixtures/pep_3131_unicode_ids.py tests/fixtures/pep_0414_u_literal.py tests/fixtures/pep_0484_type_hints.py tests/conformance.rs
git commit -m "Add PEP 3131/414/484 conformance fixtures"
```

---

## Task 9: No-new-work PEP fixtures, batch 3 (PEP 498, 515) + final fixture count check

**Files:**
- Create: `tests/fixtures/pep_0498_fstrings.py`, `tests/fixtures/pep_0515_underscores.py`
- Modify: `tests/conformance.rs` (2 new `#[test]` functions)

Both pre-verified (both profiles, byte-for-byte against CPython 3.14.6) during this plan's own writing.

`tests/fixtures/pep_0498_fstrings.py`:
```python
def report(name: str, count: int) -> str:
    return f"{name} has {count} items"


print(report("box", 3))
```
Verified output: `box has 3 items\n`

`tests/fixtures/pep_0515_underscores.py`:
```python
def big_number() -> int:
    return 1_000_000


print(big_number())
```
Verified output: `1000000\n`

- [ ] **Step 1: Create the 2 fixture files** (exact content above)

- [ ] **Step 2: Add 2 `#[test]` functions**, following the same dual-profile pattern as Tasks 7-8:

```rust
#[test]
#[ignore = "requires a pinned python3.14 (CPython 3.14.6) oracle on PATH"]
fn pep_0498_fstrings_matches_cpython_3_14_6_byte_for_byte() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pep_0498_fstrings.py");
    let (debug_pycc, debug_cpython) =
        run_conformance_fixture_with_profile("pep_0498_fstrings_debug", &fixture, false);
    assert_eq!(debug_pycc, debug_cpython, "pycc (--debug) and CPython 3.14.6 disagree on tests/fixtures/pep_0498_fstrings.py");
    let (release_pycc, release_cpython) =
        run_conformance_fixture_with_profile("pep_0498_fstrings_release", &fixture, true);
    assert_eq!(release_pycc, release_cpython, "pycc (--release) and CPython 3.14.6 disagree on tests/fixtures/pep_0498_fstrings.py");
}

#[test]
#[ignore = "requires a pinned python3.14 (CPython 3.14.6) oracle on PATH"]
fn pep_0515_underscores_matches_cpython_3_14_6_byte_for_byte() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pep_0515_underscores.py");
    let (debug_pycc, debug_cpython) =
        run_conformance_fixture_with_profile("pep_0515_underscores_debug", &fixture, false);
    assert_eq!(debug_pycc, debug_cpython, "pycc (--debug) and CPython 3.14.6 disagree on tests/fixtures/pep_0515_underscores.py");
    let (release_pycc, release_cpython) =
        run_conformance_fixture_with_profile("pep_0515_underscores_release", &fixture, true);
    assert_eq!(release_pycc, release_cpython, "pycc (--release) and CPython 3.14.6 disagree on tests/fixtures/pep_0515_underscores.py");
}
```

- [ ] **Step 3: Run everything in this file together**

Run: `cargo test --test conformance -- --ignored`
Expected: `test result: ok. 11 passed` (2 original + 9 new).

- [ ] **Step 4: Commit**

```bash
git add tests/fixtures/pep_0498_fstrings.py tests/fixtures/pep_0515_underscores.py tests/conformance.rs
git commit -m "Add PEP 498/515 conformance fixtures"
```

---

## Task 10: Docs sweep + final PR

**Files:**
- Modify: `docs/PYTHON_STANDARDS.md` (9 `St` column cells, only after real CI evidence exists — see Step 1)
- Modify: `docs/ROADMAP.md` (Quality-gates row, conformance-count note)
- Modify: `docs/TESTING.md` (conformance-harness section, note the interim manual-flip policy from D-102)
- Modify: `.github/workflows/ci.yml` comments only (the "Both conformance tests are `#[ignore]`d..." comments at lines ~152-154/366 say "two" — update the count; **no functional change**, since `--include-ignored` is already a blanket flag)
- Modify: `docs/DELIVERY_PLAN.md` (mark PR-9's row's own status)

**This task runs *after* a real CI pass, not before** — `docs/PYTHON_STANDARDS.md`'s status-column flips (per D-102) require observed, all-5-target, both-profile green evidence.

- [ ] **Step 1: Push, open the PR, wait for CI**

```bash
git push -u origin feat/v0-2-pr9-conformance-harness
gh pr create --title "v0.2 PR-9: real per-PEP conformance harness" --body "..."
```

Wait for the full CI matrix to go green on all 5 Tier-1 targets (`build-test-coverage`, all four `native-build-test` legs, `cross-compile-*`, `frontend-perf-*`, `ci-gate`). Do **not** flip any `PYTHON_STANDARDS.md` row before this is observed.

**Sequencing note (read before Steps 2-8):** Steps 2-8 below produce and push a *second*, docs-only commit on top of the code already validated by Step 1's observed run. Pushing it triggers its own fresh CI run — that new run is **not** the evidence the flip is based on and does not need to be watched before proceeding through Steps 2-7; it is expected to reproduce the same green result on the same already-proven code (a docs-only diff changes nothing compiled or run) and only needs to be green before the final merge in Step 8. The flip itself is justified entirely by Step 1's already-observed all-green run, per D-102's policy of flipping only on evidence already in hand — do not read Steps 2-8's own push as a second, independent evidence-gathering pass, and do not treat this sequencing as the speculative-flip-before-evidence pattern D-102 forbids: the evidence already exists before Step 2 runs.

- [ ] **Step 2: Flip the 9 rows in `docs/PYTHON_STANDARDS.md`** (only once Step 1's CI evidence exists)

Change `☐` to `✅` for exactly these 9 rows: PEP 238, 3105, 3107, 3131, 414, 484, 498, 515, 526. Leave PEP 649/749 at `☐` (deferred past v0.2, per this plan's own Global Constraints section and the design doc's corrected §2 table).

- [ ] **Step 3: Update `docs/ROADMAP.md`'s conformance-count note**

Update the D-088 acceptance-bullet annotation to record real progress: 9 of the required ≥15 rows are now green (up from 2 non-PEP fixtures previously counted toward nothing), and re-state the zero-margin warning from this plan's own Global Constraints section so it's visible in the milestone-tracking doc itself, not only in this plan file.

- [ ] **Step 4: Update `docs/TESTING.md`'s conformance-harness section**

Add a short note under "Conformance harness (`pycc_testkit`)" recording that PR-9 extended `tests/conformance.rs` rather than building the full crate (cross-reference D-102), and that the "CI owns the status column" policy is currently manual-pending-real-automation (cross-reference D-102's exact interim policy).

- [ ] **Step 5: Update `.github/workflows/ci.yml`'s stale comment counts**

Find the two `# Both conformance tests are #[ignore]d by default...` comments (lines ~152-154 and ~366) and update "Both" / "two" to reflect the new total (11). This is a comment-only change; the actual `--include-ignored` flag needs no modification.

- [ ] **Step 6: Update `docs/DELIVERY_PLAN.md`'s PR-9 row status**

Mark PR-9 as delivered, matching this project's own established per-PR status-marking convention (check how PR-6/PR-7/PR-8's own rows were marked once merged, and follow the identical pattern).

- [ ] **Step 7: Run the pinned local reviewer**

Per D-068 / `docs/AGENT_TOOLING.md`: dispatch the pinned `ievo:deep-reviewer` against the full `merge-base(origin/main)..HEAD` diff (not a two-dot diff). Address every actionable finding before merge, re-reviewing scoped fixes as needed.

- [ ] **Step 8: Commit the docs sweep, push, merge once CI is green and review is clean**

```bash
git add docs/PYTHON_STANDARDS.md docs/ROADMAP.md docs/TESTING.md .github/workflows/ci.yml docs/DELIVERY_PLAN.md
git commit -m "PR-9 docs sweep: flip 9 PYTHON_STANDARDS.md rows, update ROADMAP/TESTING/DELIVERY_PLAN"
git push
```

Merge once required checks are green (`ci-gate`, `audit`) and no unresolved review threads remain, matching this project's own established merge gate.
