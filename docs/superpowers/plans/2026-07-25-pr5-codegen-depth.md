# PR-5: Codegen Depth Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace every `panic!("...codegen lands in PR-5...")` in `pycc_mir`/`pycc_codegen` with real MIR lowering and LLVM codegen for PR-4's full HIR grammar: arithmetic, comparisons, local variables, `if`/`while`/`for`+`range`, functions with real parameters/return values/recursion, `float`/`bool`/`str`/`None`, string concatenation, basic f-strings, and `print` for every v0.1 scalar type. Runtime gets real `int` overflow-to-bigint (D-001) and `str` small-string optimization (D-007).

**Architecture:** MIR grows into a straightforward *typed* structural mirror of HIR (every node keeps its shape, gains a `Ty`) rather than a strict SSA form — LLVM's own `alloca`+`load`/`store` pattern (relying on LLVM's `mem2reg`, which runs automatically at any optimization level `> None`... actually see Task 1's decision: v0.1 stays un-optimized, allocas are simply left as allocas, which is correct and simple for a `--debug`-only profile) replaces real SSA construction. Every scalar keeps its current unboxed representation (`i64`, `f64`, `i1`) until `int` overflows `i64` or a `str` needs heap storage, at which point a minimal, hand-rolled runtime type (not a full arbitrary-precision library) backs it — the simplest correct choice given `pycc_own`/real ownership inference is out of scope until v0.5.

**Tech Stack:** Rust 1.97+ (edition 2024), `inkwell` 0.9 / LLVM 22 (D-015), no new external crates (bigint and small-string logic are hand-rolled in `pycc_rt`, matching D-001/D-007's "simplest correct" spirit and avoiding an undecided-dependency fork).

## Global Constraints

- 100% line and region coverage is a hard merge invariant (D-014) — `cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100` must pass after every single task, not just at the PR boundary.
- `cargo clippy --workspace --all-targets -- -D warnings` must stay clean after every task.
- `cargo doc --workspace --no-deps` must stay clean after any public API change.
- Every out-of-v0.1-scope construct still gets an explicit, clearly-worded `panic!("pycc_mir/pycc_codegen: X is not supported yet")` — never a silently wrong result (this exact convention was the subject of a self-review-caught bug in PR-4: `pycc_hir::lower_params` silently dropping unsupported parameter kinds instead of panicking).
- `--debug` profile only — `--release`/LTO optimization is out of scope (v0.2 item per `docs/DELIVERY_PLAN.md`).
- `pycc_own` (ownership/escape/RC-elision inference) is out of scope — deferred to v0.5 per `docs/DELIVERY_PLAN.md`'s crate-scope section. Use the simplest safe default (always-heap-allocate, always-refcount for `str`) instead of building real analysis.
- Record any genuinely-undecided implementation-fork decision as a new `docs/DECISIONS.md` entry (re-check the current highest `D-0NN` ID in the actual file before picking a number — it has been renumbered/extended multiple times during PR-4's own review cycle; as of this plan's writing the highest is D-045, but re-verify at execution time), picking the most conservative actually-available option rather than stopping to ask.
- Follow the existing TDD-per-task discipline used throughout PR-1 through PR-4: write failing test, verify it fails, implement, verify it passes, full workspace test+clippy+coverage, commit, push, then a separate docs-only commit flipping that task's plan checkboxes.

---

## Task 1: Record PR-5 scope decisions in DECISIONS.md

**Files:**
- Modify: `docs/DECISIONS.md`

**Interfaces:**
- Produces: the accepted design this whole plan builds against — MIR representation, bigint mechanism, small-string mechanism, `pycc_own` deferral confirmation.

- [ ] **Step 1: Re-check the current highest ADR ID**

Run: `grep -n "^| D-0" docs/DECISIONS.md | tail -3`
Expected: confirms the next free ID. This plan was drafted when D-047 was the highest existing entry (D-046 covers the frontend-perf-gate cache lifecycle; D-047 defers the whole frontend-perf-gate feature to PR-6, reverting PR-4's `ci.yml` to be byte-identical with `origin/main`'s -- corrected here from an earlier draft of this step that misdescribed D-047's content as "reverting that gate to a single job matching the upstream trust-anchor validator," which is not what D-047's actual text says), so it uses D-048 through D-051 below -- but this repo's `docs/DECISIONS.md` has been extended and renumbered multiple times during PR-4's own review cycle (parallel work landing on `main`/this branch). **Re-verify the actual next-free ID at execution time and renumber every reference in this task (and any place elsewhere in this plan that cites one of these four IDs) if the real repo state differs.** (Verified against the actual `docs/DECISIONS.md` on this branch as of this plan's Task 3-11 authoring pass: D-047 is still the highest entry and D-048 is still free, so the numbering below is current -- but re-check anyway, since this branch keeps integrating `main`.)

- [ ] **Step 2: Append four new ADR entries**

Add to the table (after the last row):

```markdown
| D-048 | PR-5's MIR stays a typed structural mirror of HIR (not real SSA); LLVM codegen uses one `alloca` per local/parameter + `load`/`store`, relying on no optimization pass (correct and simplest for a `--debug`-only v0.1 profile per D-034/DELIVERY_PLAN.md) | accepted |
| D-049 | `int` overflow-to-bigint (D-001) is a minimal hand-rolled sign-magnitude `Vec<u32>` limb representation in `pycc_rt`, not an external bigint crate — v0.1 only needs overflow-safe arithmetic + `print`, not a general-purpose bignum API surface | accepted |
| D-050 | `str` small-string optimization (D-007) inlines up to 22 UTF-8 bytes directly in the runtime string header (no heap allocation); longer strings heap-allocate with a refcount, no interning, no rope/cow structure -- the simplest representation matching D-007's own stated `≤ 22 bytes inline` threshold | accepted |
| D-051 | `pycc_own` (ownership/escape/RC-elision) is confirmed out of scope for PR-5, per DELIVERY_PLAN.md's v0.1 crate scope; every heap-allocated `str` this PR creates is unconditionally refcounted and freed on refcount reaching zero, with no cycle collector (D-004) since no v0.1 construct can form a reference cycle without classes/containers | accepted |
```

- [ ] **Step 3: Graduate D-001 and D-007 from `proposed` to `accepted`**

In the table, change D-001's status cell from `proposed` to `accepted`, and D-007's from `proposed` to `accepted` (matching the PR-3 precedent of graduating D-014/D-015/D-016/D-017 once a PR actually implements and tests the decision — do not edit either decision's own descriptive text, only the status column).

- [ ] **Step 4: Commit**

```bash
git add docs/DECISIONS.md
git commit -m "docs: record PR-5 scope decisions (D-048 through D-051), graduate D-001/D-007"
```

---

## Task 2: MIR grows into a typed structural mirror of HIR

**Files:**
- Modify: `crates/pycc_mir/src/lib.rs`

**Interfaces:**
- Consumes: `pycc_hir::{HirExpr, HirStmt, HirItem, HirModule, Ty, BinOpKind, CmpOpKind, FStringPart}` (all already exist, unchanged by this task).
- Produces: `MirExpr` (all HIR expression kinds, each carrying its resolved `Ty`), `MirStmt` (all HIR statement kinds), `MirItem::Function { name: String, params: Vec<(String, Ty)>, return_ty: Ty, body: Vec<MirStmt> }`, `MirItem::TopLevelStmt(MirStmt)`, `MirModule { items: Vec<MirItem> }`, `pub fn build(hir: &HirModule) -> MirModule`. Every later task's codegen work matches these exact names/shapes.

By construction, every `Ty` reaching this module is concrete (`pycc_types::check` fully resolves `Ty::Infer` before returning `Ok`, and `try_check`/`try_build` in `src/main.rs` never call `pycc_mir::build` on a HIR that failed `check`) -- so `build` can assert this rather than handle `Ty::Infer` as a real case.

- [ ] **Step 1: Write the failing tests for the new MIR shape**

```rust
// crates/pycc_mir/src/lib.rs, replacing the existing `tests` module's fixtures
#[test]
fn builds_an_assignment_and_a_later_name_reference() {
    let hir = HirModule {
        items: vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            }),
            HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::Name("x".to_string())],
            })),
        ],
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items,
        vec![
            MirItem::TopLevelStmt(MirStmt::Assign {
                target: "x".to_string(),
                value: MirExpr::IntLiteral(1),
            }),
            MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                callee: "print".to_string(),
                args: vec![MirExpr::Name { name: "x".to_string(), ty: Ty::Int }],
                ty: Ty::None,
            })),
        ]
    );
}

#[test]
fn builds_a_function_with_typed_params_and_return() {
    let hir = HirModule {
        items: vec![HirItem::Function {
            name: "add".to_string(),
            params: vec![("a".to_string(), Ty::Int), ("b".to_string(), Ty::Int)],
            return_ty: Ty::Int,
            body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(HirExpr::Name("a".to_string())),
                right: Box::new(HirExpr::Name("b".to_string())),
            }))],
        }],
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items,
        vec![MirItem::Function {
            name: "add".to_string(),
            params: vec![("a".to_string(), Ty::Int), ("b".to_string(), Ty::Int)],
            return_ty: Ty::Int,
            body: vec![MirStmt::Return(Some(MirExpr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(MirExpr::Name { name: "a".to_string(), ty: Ty::Int }),
                right: Box::new(MirExpr::Name { name: "b".to_string(), ty: Ty::Int }),
                ty: Ty::Int,
            }))],
        }]
    );
}
```

- [ ] **Step 2: Run to verify these fail**

Run: `cargo test -p pycc_mir`
Expected: FAIL to compile (`MirStmt` doesn't exist yet, `MirExpr::Name`/`Call`/`BinOp` don't have the new shape).

- [ ] **Step 3: Implement the new MIR types and a type-carrying `build`**

Replace `crates/pycc_mir/src/lib.rs`'s type definitions and `build`/`lower_instr`:

```rust
use pycc_hir::{BinOpKind, CmpOpKind, FStringPart, HirExpr, HirItem, HirModule, HirStmt, Ty};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum MirExpr {
    IntLiteral(i64),
    FloatLiteral(f64),
    BoolLiteral(bool),
    StringLiteral(String),
    Name { name: String, ty: Ty },
    Call { callee: String, args: Vec<MirExpr>, ty: Ty },
    BinOp { op: BinOpKind, left: Box<MirExpr>, right: Box<MirExpr>, ty: Ty },
    Compare { op: CmpOpKind, left: Box<MirExpr>, right: Box<MirExpr>, ty: Ty },
    FString(Vec<MirFStringPart>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum MirFStringPart {
    Literal(String),
    Interpolation(Box<MirExpr>),
}

impl MirExpr {
    pub fn ty(&self) -> Ty {
        match self {
            MirExpr::IntLiteral(_) => Ty::Int,
            MirExpr::FloatLiteral(_) => Ty::Float,
            MirExpr::BoolLiteral(_) => Ty::Bool,
            MirExpr::StringLiteral(_) | MirExpr::FString(_) => Ty::Str,
            MirExpr::Name { ty, .. }
            | MirExpr::Call { ty, .. }
            | MirExpr::BinOp { ty, .. }
            | MirExpr::Compare { ty, .. } => *ty,
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum MirStmt {
    ExprStmt(MirExpr),
    Assign { target: String, value: MirExpr },
    If { test: MirExpr, body: Vec<MirStmt>, orelse: Vec<MirStmt> },
    While { test: MirExpr, body: Vec<MirStmt> },
    ForRange { var: String, start: MirExpr, stop: MirExpr, step: MirExpr, body: Vec<MirStmt> },
    Return(Option<MirExpr>),
}

#[derive(Debug, PartialEq)]
pub enum MirItem {
    Function { name: String, params: Vec<(String, Ty)>, return_ty: Ty, body: Vec<MirStmt> },
    TopLevelStmt(MirStmt),
}

pub struct MirModule {
    pub items: Vec<MirItem>,
}

pub fn build(hir: &HirModule) -> MirModule {
    let mut scopes: Vec<HashMap<String, Ty>> = vec![HashMap::new()];
    let items = hir
        .items
        .iter()
        .map(|item| lower_item(item, &mut scopes))
        .collect();
    MirModule { items }
}

fn lower_item(item: &HirItem, scopes: &mut Vec<HashMap<String, Ty>>) -> MirItem {
    match item {
        HirItem::Function { name, params, return_ty, body } => {
            scopes.push(params.iter().cloned().collect());
            let body = body.iter().map(|s| lower_stmt(s, scopes)).collect();
            scopes.pop();
            MirItem::Function { name: name.clone(), params: params.clone(), return_ty: *return_ty, body }
        }
        HirItem::TopLevelStmt(stmt) => MirItem::TopLevelStmt(lower_stmt(stmt, scopes)),
    }
}

fn bind(scopes: &mut [HashMap<String, Ty>], name: String, ty: Ty) {
    scopes.last_mut().expect("at least one scope is always present").insert(name, ty);
}

fn lookup(scopes: &[HashMap<String, Ty>], name: &str) -> Ty {
    scopes
        .iter()
        .rev()
        .find_map(|scope| scope.get(name).copied())
        .unwrap_or_else(|| panic!("pycc_mir: internal error: `{name}` has no recorded type -- pycc_types::check should have rejected this HIR before it reached pycc_mir"))
}

fn lower_stmt(stmt: &HirStmt, scopes: &mut Vec<HashMap<String, Ty>>) -> MirStmt {
    match stmt {
        HirStmt::ExprStmt(expr) => MirStmt::ExprStmt(lower_expr(expr, scopes)),
        HirStmt::Assign { target, value } => {
            let value = lower_expr(value, scopes);
            bind(scopes, target.clone(), value.ty());
            MirStmt::Assign { target: target.clone(), value }
        }
        HirStmt::If { test, body, orelse } => MirStmt::If {
            test: lower_expr(test, scopes),
            body: body.iter().map(|s| lower_stmt(s, scopes)).collect(),
            orelse: orelse.iter().map(|s| lower_stmt(s, scopes)).collect(),
        },
        HirStmt::While { test, body } => MirStmt::While {
            test: lower_expr(test, scopes),
            body: body.iter().map(|s| lower_stmt(s, scopes)).collect(),
        },
        HirStmt::ForRange { var, start, stop, step, body } => {
            let start = lower_expr(start, scopes);
            let stop = lower_expr(stop, scopes);
            let step = lower_expr(step, scopes);
            bind(scopes, var.clone(), Ty::Int);
            let body = body.iter().map(|s| lower_stmt(s, scopes)).collect();
            MirStmt::ForRange { var: var.clone(), start, stop, step, body }
        }
        HirStmt::Return(value) => MirStmt::Return(value.as_ref().map(|v| lower_expr(v, scopes))),
    }
}

fn lower_expr(expr: &HirExpr, scopes: &[HashMap<String, Ty>]) -> MirExpr {
    match expr {
        HirExpr::IntLiteral(n) => MirExpr::IntLiteral(*n),
        HirExpr::FloatLiteral(f) => MirExpr::FloatLiteral(*f),
        HirExpr::BoolLiteral(b) => MirExpr::BoolLiteral(*b),
        HirExpr::StringLiteral(s) => MirExpr::StringLiteral(s.clone()),
        HirExpr::Name(name) => MirExpr::Name { name: name.clone(), ty: lookup(scopes, name) },
        HirExpr::Call { callee, args } => {
            let args: Vec<MirExpr> = args.iter().map(|a| lower_expr(a, scopes)).collect();
            let ty = if callee == "print" {
                Ty::None
            } else {
                lookup(scopes, &format!("$fn:{callee}"))
            };
            MirExpr::Call { callee: callee.clone(), args, ty }
        }
        HirExpr::BinOp { op, left, right } => {
            let left = lower_expr(left, scopes);
            let right = lower_expr(right, scopes);
            let ty = binop_result_ty(*op, left.ty(), right.ty());
            MirExpr::BinOp { op: *op, left: Box::new(left), right: Box::new(right), ty }
        }
        HirExpr::Compare { op, left, right } => MirExpr::Compare {
            op: *op,
            left: Box::new(lower_expr(left, scopes)),
            right: Box::new(lower_expr(right, scopes)),
            ty: Ty::Bool,
        },
        HirExpr::FString(parts) => MirExpr::FString(
            parts
                .iter()
                .map(|p| match p {
                    FStringPart::Literal(s) => MirFStringPart::Literal(s.clone()),
                    FStringPart::Interpolation(e) => MirFStringPart::Interpolation(Box::new(lower_expr(e, scopes))),
                })
                .collect(),
        ),
    }
}

fn binop_result_ty(op: BinOpKind, left: Ty, right: Ty) -> Ty {
    if left == Ty::Str && right == Ty::Str && op == BinOpKind::Add {
        return Ty::Str;
    }
    // True division always produces `float`, even for two `int`/`bool`
    // operands -- this must match `pycc_types::numeric_result_type`'s own
    // rule (`(Some(_), Some(_)) if op == BinOpKind::Div => Ok(Ty::Float)`)
    // exactly, since `pycc_types` already accepted this program on that
    // promise; a mismatch here would make MIR's `ty` lie about what
    // codegen must produce (self-review correction: an earlier draft of
    // this function returned `Ty::Int` for `int / int`, which is simply
    // wrong -- `5 / 2` is `2.5`, not `2`).
    if op == BinOpKind::Div || left == Ty::Float || right == Ty::Float {
        return Ty::Float;
    }
    Ty::Int
}
```

`lookup`'s `$fn:{callee}` convention needs every function's signature registered before any body is lowered, exactly like `pycc_types::check`'s own D-038/D-039 two-pass fix. Add this as `build`'s first pass, before the existing per-item loop:

```rust
pub fn build(hir: &HirModule) -> MirModule {
    let mut scopes: Vec<HashMap<String, Ty>> = vec![HashMap::new()];
    for item in &hir.items {
        if let HirItem::Function { name, return_ty, .. } = item {
            bind(&mut scopes, format!("$fn:{name}"), *return_ty);
        }
    }
    let items = hir.items.iter().map(|item| lower_item(item, &mut scopes)).collect();
    MirModule { items }
}
```

- [ ] **Step 4: Run to verify pycc_mir's tests pass**

Run: `cargo test -p pycc_mir`
Expected: PASS.

- [ ] **Step 5: Update `pycc_codegen` to compile against the new MIR shape (mechanical, no new codegen)**

`crates/pycc_codegen/src/lib.rs`'s `emit_instr` only handled `MirInstr::{CallPrint, CallUserFunction}`; both are gone. Replace every `MirInstr`/`emit_instr` reference with a temporary, minimal `emit_stmt` that only handles the two shapes the *existing* tests exercise (a `print(<int literal>)` `ExprStmt` and a zero-arg `Call` `ExprStmt`), panicking explicitly for everything else -- later tasks in this plan replace this panic arm by arm:

```rust
use pycc_mir::{MirExpr, MirItem, MirModule, MirStmt};
// ...
fn emit_stmt<'ctx>(
    builder: &inkwell::builder::Builder<'ctx>,
    print_fn: FunctionValue<'ctx>,
    user_functions: &HashMap<&str, FunctionValue<'ctx>>,
    i64_type: IntType<'ctx>,
    stmt: &MirStmt,
) -> Result<(), String> {
    match stmt {
        MirStmt::ExprStmt(MirExpr::Call { callee, args, .. }) if callee == "print" => match args.as_slice() {
            [MirExpr::IntLiteral(n)] => {
                let arg_value = i64_type.const_int(*n as u64, true);
                builder
                    .build_call(print_fn, &[arg_value.into()], "call_print")
                    .expect("build_call should not fail for a well-formed print call");
                Ok(())
            }
            _ => panic!("pycc_codegen: this print() argument shape is not supported yet"),
        },
        MirStmt::ExprStmt(MirExpr::Call { callee, args, .. }) if args.is_empty() => {
            let f = user_functions
                .get(callee.as_str())
                .ok_or_else(|| format!("pycc_codegen v0.1: call to undefined function `{callee}`"))?;
            builder
                .build_call(*f, &[], "call_user_fn")
                .expect("build_call should not fail for a well-formed zero-arg call");
            Ok(())
        }
        other => panic!("pycc_codegen: this statement kind's codegen is not supported yet: {other:?}"),
    }
}
```

Update every call site (`compile_to_object`'s two loops) from `emit_instr(..., instr)` to `emit_stmt(..., stmt)`, and every test fixture from `MirInstr::CallPrint { arg: MirExpr::IntLiteral(n) }` to `MirStmt::ExprStmt(MirExpr::Call { callee: "print".to_string(), args: vec![MirExpr::IntLiteral(n)], ty: Ty::None })`, and `MirInstr::CallUserFunction { name }` to `MirStmt::ExprStmt(MirExpr::Call { callee: name, args: vec![], ty: Ty::None })`. `MirItem::Function { body, .. }`'s `body: Vec<MirInstr>` becomes `body: Vec<MirStmt>` at every construction site (add `params: vec![], return_ty: Ty::None` too, since `MirItem::Function` gained those fields in Step 3).

- [ ] **Step 6: Run the full workspace test suite**

Run: `cargo test --workspace`
Expected: PASS (every `tests/slice0.rs` e2e test and every crate's own unit tests).

- [ ] **Step 7: Run clippy and the coverage gate**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo llvm-cov clean --workspace && cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/pycc_mir/src/lib.rs crates/pycc_codegen/src/lib.rs
git commit -m "feat(pycc_mir,pycc_codegen): MIR becomes a typed structural mirror of HIR (Task 2, D-048)"
```

---

## Task 3: Integer arithmetic, comparisons, and local variables in codegen

**Order/scope note (task-ordering deviation, per this plan's own instructions):** the original brainstorm scope described this task as "the `i64` fast path only," treating `int` overflow-to-bigint (Task 9) as a strictly later, separately-layered concern. Re-reading `pycc_own`'s deferral and D-001 together during repo research surfaced a real design hazard in that split: with no ownership/escape analysis, codegen can never statically know *which* `int` values might later overflow (a loop accumulator, a function parameter, a recursive return value are all runtime-unknown), so *every* `int` value -- not just the ones a later task happens to touch -- must share one stable storage representation from the moment `int` codegen exists at all. If this task stored `int` as a bare `i64` and Task 9 later had to widen that to a tagged/boxed shape, every `alloca`/parameter/return/print/compare this task (and Tasks 4/5/10) write would need rewriting in Task 9 -- a representation migration disguised as a bolt-on, which this plan's own "type consistency across tasks" self-review would have to reject. This task therefore fixes `int`'s *final* v0.1 storage layout now (D-052 below); Task 9 only ever replaces an internal fallback function body behind that fixed layout, exactly the same "replace this arm later" pattern Task 2 already established for `emit_stmt`'s temporary panics. Also, since `pycc_types::infer_expr`'s `HirExpr::Compare` arm already accepts *any* v0.1 grammar reachable today (`x = 1 < 2`) needs somewhere to store the resulting `bool`, "local variables" in this task's title necessarily includes minimal `bool` storage (from comparisons and `bool` literals) alongside `int` -- not the full `float`/`bool`/`None` *arithmetic promotion* work, which stays Task 6's job as originally scoped.

**Files:**
- Modify: `crates/pycc_rt/src/lib.rs`
- Modify: `crates/pycc_codegen/src/lib.rs`
- Modify: `docs/DECISIONS.md`

**Interfaces:**
- Consumes: `pycc_mir::{MirModule, MirItem, MirStmt, MirExpr}`, `pycc_hir::{Ty, BinOpKind, CmpOpKind}` (all from Task 2, unchanged).
- Produces (used unchanged by every later task in this plan):
  - `pycc_rt`: `pycc_rt_int_add/sub/mul/floordiv/floormod/pow(i64, i64) -> i64`, `pycc_rt_int_cmp(i64, i64) -> i32` (Rust `Ordering` encoded as `-1/0/1`), `pycc_rt_int_print(i64)`. All take/return the *tagged* `i64` representation (D-052) -- never a raw untagged value -- so every call site, in this task and every later one, passes/receives tagged values uniformly.
  - `pycc_codegen`: a `Scalar<'ctx>` enum (`Int(IntValue<'ctx>)`, `Bool(IntValue<'ctx>)` in this task; `Float`/`Str` variants are *added*, not replaced, by Tasks 6/7) representing one MIR-level value during codegen; an `emit_expr(context, builder, module, rt, user_functions, locals, expr: &MirExpr) -> Scalar<'ctx>` function (panics -- never a `Result` -- on every internal-error/not-yet-supported path, matching `pycc_mir::lookup`'s own established convention) that every later task's codegen extends with new `MirExpr` arms; a `RtFns<'ctx>` struct caching every declared `pycc_rt` `FunctionValue`, extended (not replaced) by Tasks 6/7/8/9/10 as they add more runtime declarations; a `locals: &HashMap<String, (PointerValue<'ctx>, Ty)>` (read-only in `emit_expr`) / `&mut HashMap<...>` (mutated in `emit_stmt`/`emit_assign`) convention threaded through both, reused unchanged by Tasks 4/5.

### Step 1: Record D-052 (int representation) in DECISIONS.md

- [ ] Re-run `grep -n "^| D-0" docs/DECISIONS.md | tail -3` to reconfirm D-051 is still the highest entry (Task 1 already claimed D-048-D-051; this is the next free ID). Append to the table:

```markdown
| D-052 | `int`'s single `i64` storage slot (alloca/parameter/return/print) is a tagged 63-bit fixnum -- `(n << 1) \| 1` -- for the fast path; an *even* bit pattern in that same slot is a real heap `BigInt` pointer (D-049), never a raw untagged value. Every `pycc_rt_int_*` function's signature is fixed from this decision onward; only their internal fallback bodies change when Task 9 adds bigint promotion. Arithmetic/comparison codegen calls these `pycc_rt` functions rather than emitting raw LLVM int ops directly (simplest-correct for a `--debug`-only, no-perf-requirement v0.1 profile; direct-intrinsic codegen is a documented, non-blocking future optimization) | accepted |
```

- [ ] Add the corresponding long-form section at the end of the file (after D-047's section):

```markdown
## D-052: `int`'s fast-path `i64` slot is a low-bit-tagged 63-bit fixnum

- Status: accepted (PR-5 Task 3 is the task that depends on it)
- Context: D-001 says "`i64` fast path, overflow promotes to heap bigint" without fixing a concrete bit-level representation. With `pycc_own` deferred to v0.5 (no escape/ownership analysis), codegen cannot statically distinguish an `int` value that will always stay small from one that might later overflow -- a loop accumulator or a function parameter is runtime-unknown either way. Every `int` value must therefore share one storage layout from the first codegen task that touches `int` at all, or a later task migrating the layout would have to rewrite every earlier task's `alloca`/parameter/return/print/compare codegen -- exactly the kind of silent representation drift this plan's own type-consistency self-review exists to catch. Two alternatives were weighed against this constraint (see Alternatives).
- Decision: represent every `Ty::Int` value, in every context (local `alloca`, function parameter, function return, a literal constant), as one LLVM `i64`. Its low bit is the discriminant: `1` means the remaining 63 bits (arithmetic-shift-recovered: `tagged >> 1`) are the real value (`tag_smallint(n) = (n << 1) | 1`); `0` means the full 64 bits are a real heap pointer to a `BigInt` (D-049), which Rust's global allocator guarantees is at least 2-byte aligned on every Tier-1 target, so a genuine heap pointer's low bit is always `0` -- disjoint from a tagged fixnum's low bit, which is always `1`. Every arithmetic/comparison/print operation on `int` is a call to a `pycc_rt` function taking/returning this exact tagged `i64` (never a raw untagged value) -- `pycc_codegen` itself never inspects the tag bit or constructs a `BigInt` pointer; all of that logic (and 100%-coverage-friendly unit testing of it) lives in `pycc_rt`. Task 9 (bigint promotion) only ever changes what these same functions do internally on overflow -- their signatures, and every call site this task (and Tasks 4/5/10) write, stay fixed.
- Alternatives: a 16-byte `{ tag: i64, payload: i64 }` struct passed by value -- rejected: this project has already hit real, expensive-to-diagnose cross-platform ABI mismatches for far simpler cases (D-027/D-028/D-029's ~3 CI round-trips over Windows linking alone), and Windows x64's C ABI passes aggregates larger than 8 bytes *by hidden reference* rather than in two registers, unlike System V -- a fresh source of exactly that kind of bug, for no benefit this project's stated v0.1 perf bar requires. Always-boxed (every `int`, even a small one, is a heap `BigInt` from the start) -- rejected: contradicts this plan's own header ("every scalar keeps its unboxed representation... until `int` overflows") and would require building the bigint runtime (Task 9's job) before Task 3 could emit any arithmetic at all, inverting this plan's dependency order for no correctness benefit. Emitting raw LLVM `add`/`sub`/`mul`/overflow-intrinsic instructions directly in `pycc_codegen` instead of calling `pycc_rt` functions -- rejected for v0.1 only: it avoids a function-call's overhead (irrelevant, since v0.1 has no generated-code speed requirement -- see TESTING.md Layer 7, a v0.2+ item) at the cost of moving all the tagging/overflow/floor-division-semantics logic into hand-built LLVM IR, which is far harder to get right and to unit-test in isolation than equivalent Rust code in `pycc_rt`; revisit once a real perf bar exists.
- Consequences: `RUNTIME.md`'s "scalars... unboxed, never touch the runtime" describes the eventual, optimized steady state (once direct-intrinsic codegen replaces these calls, a documented future item) -- v0.1's actual `int` fast path does call into `pycc_rt` for every arithmetic/comparison/print operation; `RUNTIME.md` is annotated to say so honestly rather than silently contradicted (see Task 11). `bool` gets its own, simpler, untagged `i8` representation (0/1) -- not folded into this tagging scheme, since Python `bool` never needs bigint promotion. A **known, load-bearing gap accepted for the rest of PR-5**: `pycc_rt_int_cmp` (and any future bigint-comparison need) explicitly panics if either operand's tag bit shows it's already a heap `BigInt` -- comparing a bigint-valued `int` is out of scope for v0.1 and stays an honest, named limitation (like D-026's cross-OS gap), not a silent wrong answer.
```

- [ ] **Step 1 commit:**

```bash
git add docs/DECISIONS.md
git commit -m "docs: record D-052 (int's tagged-fixnum representation)"
```

### Step 2: Write failing `pycc_rt` tests for the tagging primitives and checked fast-path arithmetic

Append to `crates/pycc_rt/src/lib.rs`'s `#[cfg(test)] mod tests`:

```rust
#[test]
fn tagging_a_small_value_and_untagging_it_round_trips() {
    for n in [0i64, 1, -1, 42, -42, i64::MAX >> 1, -(i64::MAX >> 1)] {
        let tagged = tag_smallint(n);
        assert!(is_smallint(tagged), "expected {n} to be tagged as small");
        assert_eq!(untag_smallint(tagged), n);
    }
}

#[test]
fn a_value_needing_the_full_64_bits_does_not_fit_the_tagged_range() {
    assert_eq!(fits_smallint(i64::MAX), None);
    assert_eq!(fits_smallint(i64::MIN), None);
}

#[test]
fn pycc_rt_int_add_computes_the_correct_tagged_sum() {
    let a = tag_smallint(2);
    let b = tag_smallint(3);
    assert_eq!(untag_smallint(pycc_rt_int_add(a, b)), 5);
}

#[test]
#[should_panic(expected = "integer overflow")]
fn pycc_rt_int_add_panics_on_overflow_before_bigint_promotion_exists() {
    pycc_rt_int_add(tag_smallint(i64::MAX >> 1), tag_smallint(1));
}

#[test]
fn pycc_rt_int_sub_computes_the_correct_tagged_difference() {
    let a = tag_smallint(5);
    let b = tag_smallint(3);
    assert_eq!(untag_smallint(pycc_rt_int_sub(a, b)), 2);
}

#[test]
fn pycc_rt_int_mul_computes_the_correct_tagged_product() {
    let a = tag_smallint(6);
    let b = tag_smallint(7);
    assert_eq!(untag_smallint(pycc_rt_int_mul(a, b)), 42);
}

#[test]
fn pycc_rt_int_floordiv_matches_python_floor_semantics() {
    // Python: -7 // 2 == -4 (floors toward negative infinity), not -3
    // (truncation toward zero, which is what a raw LLVM/Rust `/` gives).
    assert_eq!(untag_smallint(pycc_rt_int_floordiv(tag_smallint(-7), tag_smallint(2))), -4);
    assert_eq!(untag_smallint(pycc_rt_int_floordiv(tag_smallint(7), tag_smallint(2))), 3);
    assert_eq!(untag_smallint(pycc_rt_int_floordiv(tag_smallint(7), tag_smallint(-2))), -4);
}

#[test]
fn pycc_rt_int_floormod_matches_python_floor_semantics() {
    // Python: -7 % 2 == 1 (result takes the divisor's sign), not -1.
    assert_eq!(untag_smallint(pycc_rt_int_floormod(tag_smallint(-7), tag_smallint(2))), 1);
    assert_eq!(untag_smallint(pycc_rt_int_floormod(tag_smallint(7), tag_smallint(2))), 1);
    assert_eq!(untag_smallint(pycc_rt_int_floormod(tag_smallint(7), tag_smallint(-2))), -1);
}

#[test]
#[should_panic(expected = "division by zero")]
fn pycc_rt_int_floordiv_by_zero_panics() {
    pycc_rt_int_floordiv(tag_smallint(1), tag_smallint(0));
}

#[test]
#[should_panic(expected = "modulo by zero")]
fn pycc_rt_int_floormod_by_zero_panics() {
    pycc_rt_int_floormod(tag_smallint(1), tag_smallint(0));
}

#[test]
fn pycc_rt_int_floordiv_handles_i64_min_divided_by_negative_one_without_trapping() {
    // Rust's own `/`/`%` panic (hardware trap) on this exact pair -- the
    // mathematical quotient (2^63) doesn't fit `i64` at all, so this must
    // go through bigint promotion once Task 9 exists; for now it's a
    // documented overflow (same "not yet" panic as any other overflow).
    let min = i64::MIN >> 1; // largest-magnitude value that still tags
    assert_eq!(untag_smallint(pycc_rt_int_floordiv(tag_smallint(min), tag_smallint(1))), min);
}

#[test]
fn pycc_rt_int_pow_computes_the_correct_tagged_power() {
    assert_eq!(untag_smallint(pycc_rt_int_pow(tag_smallint(2), tag_smallint(10))), 1024);
    assert_eq!(untag_smallint(pycc_rt_int_pow(tag_smallint(5), tag_smallint(0))), 1);
}

#[test]
#[should_panic(expected = "negative exponent")]
fn pycc_rt_int_pow_with_a_negative_exponent_panics() {
    pycc_rt_int_pow(tag_smallint(2), tag_smallint(-1));
}

#[test]
fn pycc_rt_int_cmp_reports_less_equal_and_greater() {
    assert_eq!(pycc_rt_int_cmp(tag_smallint(1), tag_smallint(2)), -1);
    assert_eq!(pycc_rt_int_cmp(tag_smallint(2), tag_smallint(2)), 0);
    assert_eq!(pycc_rt_int_cmp(tag_smallint(3), tag_smallint(2)), 1);
}

#[test]
#[should_panic(expected = "bigint-valued")]
fn pycc_rt_int_cmp_on_a_bigint_tagged_operand_panics() {
    // Bit pattern `0` (even) is what D-052 reserves for a heap `BigInt`
    // pointer -- no real allocation needed to exercise this rejection.
    pycc_rt_int_cmp(0, tag_smallint(1));
}

#[test]
fn pycc_rt_int_print_prints_the_untagged_decimal_value() {
    // stdout is captured by the test harness; this exercises
    // `pycc_rt_int_print` itself (not just `pycc_rt_print_i64`) for the
    // D-014 gate, same rationale as this file's existing
    // `extern_c_entry_point_runs_for_positive_negative_and_zero` test.
    pycc_rt_int_print(tag_smallint(42));
    pycc_rt_int_print(tag_smallint(-7));
}

#[test]
#[should_panic(expected = "bigint-valued")]
fn pycc_rt_int_print_on_a_bigint_tagged_value_panics() {
    pycc_rt_int_print(0);
}
```

- [ ] **Step 2 run:** `cargo test -p pycc_rt`
Expected: FAIL to compile (`tag_smallint`, `untag_smallint`, `is_smallint`, `fits_smallint`, `pycc_rt_int_add`, and every other new function don't exist yet).

### Step 3: Implement the tagging primitives and checked fast-path arithmetic in `pycc_rt`

Add to `crates/pycc_rt/src/lib.rs` (above the existing `format_i64_line`/`pycc_rt_print_i64`, which stay unchanged):

```rust
/// See D-052: every `Ty::Int` value is one LLVM `i64`. Its low bit is the
/// discriminant -- `1` means the high 63 bits (arithmetic-shift-recovered)
/// are the real value; `0` means the full 64 bits are a heap `BigInt`
/// pointer (Task 9). This module never constructs the `0` case yet.
const TAG_BIT: i64 = 1;

fn tag_smallint(value: i64) -> i64 {
    (value << 1) | TAG_BIT
}

fn untag_smallint(tagged: i64) -> i64 {
    tagged >> 1 // arithmetic (sign-extending) shift for `i64`
}

fn is_smallint(tagged: i64) -> bool {
    tagged & TAG_BIT == TAG_BIT
}

/// `None` when `value` needs the full 64 bits (including sign) to
/// represent -- i.e. tagging then untagging would not round-trip.
fn fits_smallint(value: i64) -> Option<i64> {
    let tagged = tag_smallint(value);
    (untag_smallint(tagged) == value).then_some(tagged)
}

fn require_smallint(tagged: i64, context: &str) {
    if !is_smallint(tagged) {
        panic!("pycc_rt: {context} a bigint-valued `int` is not supported yet");
    }
}

/// # Safety (panic-across-FFI note, applies to every `pycc_rt_int_*`
/// function below)
/// These are plain `extern "C" fn`s, not `extern "C-unwind"`. Since Rust
/// 1.71, a panic that would otherwise unwind past an ordinary
/// `extern "C"` function's boundary is caught at that boundary and turned
/// into a process abort instead of continuing to unwind into a foreign
/// (non-Rust, no unwind tables) caller -- which is exactly what happens
/// here when pycc-generated LLVM code calls one of these and it panics.
/// This is a real, stable Rust guarantee (not assumed UB-avoidance), and
/// it is *also* what makes these functions directly unit-testable with
/// ordinary `#[should_panic]` below: a panic raised while calling one of
/// these from *this crate's own* Rust test code is an ordinary,
/// same-binary unwind the test harness catches -- no FFI boundary is
/// crossed during the test itself.
#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_int_add(a: i64, b: i64) -> i64 {
    require_smallint(a, "adding");
    require_smallint(b, "adding");
    untag_smallint(a)
        .checked_add(untag_smallint(b))
        .and_then(fits_smallint)
        .unwrap_or_else(|| panic!("pycc_rt: integer overflow (bigint promotion is not implemented yet)"))
}

#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_int_sub(a: i64, b: i64) -> i64 {
    require_smallint(a, "subtracting");
    require_smallint(b, "subtracting");
    untag_smallint(a)
        .checked_sub(untag_smallint(b))
        .and_then(fits_smallint)
        .unwrap_or_else(|| panic!("pycc_rt: integer overflow (bigint promotion is not implemented yet)"))
}

#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_int_mul(a: i64, b: i64) -> i64 {
    require_smallint(a, "multiplying");
    require_smallint(b, "multiplying");
    untag_smallint(a)
        .checked_mul(untag_smallint(b))
        .and_then(fits_smallint)
        .unwrap_or_else(|| panic!("pycc_rt: integer overflow (bigint promotion is not implemented yet)"))
}

#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_int_floordiv(a: i64, b: i64) -> i64 {
    require_smallint(a, "dividing");
    require_smallint(b, "dividing");
    let (a, b) = (untag_smallint(a), untag_smallint(b));
    if b == 0 {
        panic!("pycc_rt: integer division by zero");
    }
    if a == i64::MIN && b == -1 {
        panic!("pycc_rt: integer overflow (bigint promotion is not implemented yet)");
    }
    let q = a / b;
    let r = a % b;
    let floored = if r != 0 && (r < 0) != (b < 0) { q - 1 } else { q };
    fits_smallint(floored)
        .unwrap_or_else(|| panic!("pycc_rt: integer overflow (bigint promotion is not implemented yet)"))
}

#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_int_floormod(a: i64, b: i64) -> i64 {
    require_smallint(a, "computing the modulo of");
    require_smallint(b, "computing the modulo of");
    let (a, b) = (untag_smallint(a), untag_smallint(b));
    if b == 0 {
        panic!("pycc_rt: integer modulo by zero");
    }
    if a == i64::MIN && b == -1 {
        // Mathematically `i64::MIN % -1 == 0` (anything mod +/-1 is 0),
        // and 0 always fits -- unlike floordiv, this case never actually
        // needs bigint promotion, but Rust's own `%` still traps on the
        // identical hardware instruction floordiv uses, so it needs the
        // same explicit bypass.
        return tag_smallint(0);
    }
    let r = a % b;
    let floored = if r != 0 && (r < 0) != (b < 0) { r + b } else { r };
    fits_smallint(floored)
        .unwrap_or_else(|| panic!("pycc_rt: integer overflow (bigint promotion is not implemented yet)"))
}

#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_int_pow(base: i64, exp: i64) -> i64 {
    require_smallint(base, "exponentiating");
    require_smallint(exp, "exponentiating");
    let mut exp = untag_smallint(exp);
    if exp < 0 {
        panic!(
            "pycc_rt: negative exponent for `int ** int` is not supported \
             (the real result would need to be `float`, matching CPython's \
             own `int ** int` rule -- a pre-existing pycc_types simplification, \
             not a new PR-5 gap: pycc_types::numeric_result_type always types \
             `**` as `int`-returning)"
        );
    }
    let mut result = tag_smallint(1);
    let mut base = base;
    while exp > 0 {
        if exp & 1 == 1 {
            result = pycc_rt_int_mul(result, base);
        }
        exp >>= 1;
        if exp > 0 {
            base = pycc_rt_int_mul(base, base);
        }
    }
    result
}

#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_int_cmp(a: i64, b: i64) -> i32 {
    require_smallint(a, "comparing");
    require_smallint(b, "comparing");
    match untag_smallint(a).cmp(&untag_smallint(b)) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_int_print(tagged: i64) {
    require_smallint(tagged, "printing");
    pycc_rt_print_i64(untag_smallint(tagged));
}
```

- [ ] **Step 3 run:** `cargo test -p pycc_rt`
Expected: PASS.

### Step 4: Write a failing `pycc_codegen` test for local-variable integer arithmetic end to end

Append to `crates/pycc_codegen/src/lib.rs`'s `#[cfg(test)] mod tests` (this crate's tests already compile+link+run a real binary via `link_object_with_runtime`, established in Task 2/earlier PRs -- this test follows the exact same pattern):

```rust
#[test]
fn compiles_local_variable_arithmetic_comparisons_and_floor_division() {
    // `x = 7; y = 2; print(x // y)` at the MIR level, exercising: a fresh
    // `alloca` per local, `BinOp::FloorDiv` codegen, and reading a `Name`
    // back out of its local for a later statement -- everything Task 2's
    // temporary `emit_stmt` explicitly could not do yet.
    let mir = MirModule {
        items: vec![
            MirItem::TopLevelStmt(MirStmt::Assign {
                target: "x".to_string(),
                value: MirExpr::IntLiteral(7),
            }),
            MirItem::TopLevelStmt(MirStmt::Assign {
                target: "y".to_string(),
                value: MirExpr::IntLiteral(2),
            }),
            MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                callee: "print".to_string(),
                args: vec![MirExpr::BinOp {
                    op: pycc_hir::BinOpKind::FloorDiv,
                    left: Box::new(MirExpr::Name { name: "x".to_string(), ty: pycc_hir::Ty::Int }),
                    right: Box::new(MirExpr::Name { name: "y".to_string(), ty: pycc_hir::Ty::Int }),
                    ty: pycc_hir::Ty::Int,
                }],
                ty: pycc_hir::Ty::None,
            })),
        ],
    };
    let dir = tempfile_dir("locals_arith");
    let obj_path = dir.join("locals_arith.o");
    compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
    let bin_path = dir.join("locals_arith");
    link_object_with_runtime(&obj_path, &bin_path);
    let output = Command::new(&bin_path).output().expect("binary should run");
    assert_eq!(output.stdout, b"3\n");
}

#[test]
fn compiles_a_comparison_result_stored_in_a_bool_local() {
    // `b = 1 < 2` -- exercises `Compare` codegen and a `bool`-typed
    // (`i8`) local's own `alloca`, distinct from `int`'s tagged `i64`.
    // Nothing yet reads `b` back out (print(bool) is Task 10's job), so
    // this only proves the assignment itself doesn't crash/miscompile;
    // `verify_module`'s `module.verify()` call (non-Windows) is the
    // actual proof the generated IR is well-formed.
    let mir = MirModule {
        items: vec![MirItem::TopLevelStmt(MirStmt::Assign {
            target: "b".to_string(),
            value: MirExpr::Compare {
                op: pycc_hir::CmpOpKind::Lt,
                left: Box::new(MirExpr::IntLiteral(1)),
                right: Box::new(MirExpr::IntLiteral(2)),
                ty: pycc_hir::Ty::Bool,
            },
        })],
    };
    let dir = tempfile_dir("bool_local");
    let obj_path = dir.join("bool_local.o");
    compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
}

#[test]
fn reassigning_a_local_reuses_its_existing_alloca() {
    // `x = 1; x = 2; print(x)` -- the second `Assign` must reuse `x`'s
    // existing slot (not allocate a second, shadowing one), matching
    // ordinary Python rebinding semantics.
    let mir = MirModule {
        items: vec![
            MirItem::TopLevelStmt(MirStmt::Assign {
                target: "x".to_string(),
                value: MirExpr::IntLiteral(1),
            }),
            MirItem::TopLevelStmt(MirStmt::Assign {
                target: "x".to_string(),
                value: MirExpr::IntLiteral(2),
            }),
            MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                callee: "print".to_string(),
                args: vec![MirExpr::Name { name: "x".to_string(), ty: pycc_hir::Ty::Int }],
                ty: pycc_hir::Ty::None,
            })),
        ],
    };
    let dir = tempfile_dir("reassign_local");
    let obj_path = dir.join("reassign_local.o");
    compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
    let bin_path = dir.join("reassign_local");
    link_object_with_runtime(&obj_path, &bin_path);
    let output = Command::new(&bin_path).output().expect("binary should run");
    assert_eq!(output.stdout, b"2\n");
}
```

- [ ] **Step 4 run:** `cargo test -p pycc_codegen`
Expected: FAIL to compile (`Scalar`, the new `locals`-aware `emit_expr`, and `BinOp`/`Compare`/general `Name`-reading codegen don't exist yet; the existing `emit_stmt` still only recognizes the two Task-2 shapes).

### Step 5: Implement `Scalar`, `RtFns`, and general local/arithmetic/comparison codegen

Add near the top of `crates/pycc_codegen/src/lib.rs` (after the existing `use` block -- add `use inkwell::IntPredicate;` and `use inkwell::values::{IntValue, PointerValue};` to it, and change `use pycc_mir::{MirExpr, MirItem, MirModule};` to `use pycc_mir::{MirExpr, MirItem, MirModule, MirStmt};`):

```rust
/// One MIR-level value during codegen. Extended (never replaced) by
/// later tasks: `Float` in Task 6, `Str` in Task 7. `Ty::None` never
/// needs a variant here -- no v0.1 `MirExpr` can actually construct a
/// `None` *value* (see Task 6's note).
enum Scalar<'ctx> {
    /// Tagged per D-052. Always LLVM `i64`.
    Int(IntValue<'ctx>),
    /// `0`/`1`, LLVM `i8` -- not `i1` (D-052's ABI note: this project has
    /// already hit real cross-platform storage/parameter footguns for
    /// sub-byte types, see D-027/D-028/D-029; `i1` is used only
    /// transiently for a `br` condition or an `icmp`/`fcmp` result,
    /// immediately zero-extended to `i8` before it's stored anywhere).
    Bool(IntValue<'ctx>),
}

/// Every `pycc_rt` function this crate calls, declared once in
/// `compile_to_object` and threaded through `emit_stmt`/`emit_expr`.
/// Extended (never replaced) by Tasks 6/7/8/9/10 as they add more
/// `pycc_rt` declarations.
struct RtFns<'ctx> {
    int_add: FunctionValue<'ctx>,
    int_sub: FunctionValue<'ctx>,
    int_mul: FunctionValue<'ctx>,
    int_floordiv: FunctionValue<'ctx>,
    int_floormod: FunctionValue<'ctx>,
    int_pow: FunctionValue<'ctx>,
    int_cmp: FunctionValue<'ctx>,
    int_print: FunctionValue<'ctx>,
}

fn declare_rt_functions<'ctx>(
    context: &'ctx Context,
    module: &inkwell::module::Module<'ctx>,
) -> RtFns<'ctx> {
    let i64_type = context.i64_type();
    let i32_type = context.i32_type();
    let void_type = context.void_type();
    let declare = |name: &str, fn_type: inkwell::types::FunctionType<'ctx>| {
        module.add_function(name, fn_type, Some(Linkage::External))
    };
    RtFns {
        int_add: declare(
            "pycc_rt_int_add",
            i64_type.fn_type(&[i64_type.into(), i64_type.into()], false),
        ),
        int_sub: declare(
            "pycc_rt_int_sub",
            i64_type.fn_type(&[i64_type.into(), i64_type.into()], false),
        ),
        int_mul: declare(
            "pycc_rt_int_mul",
            i64_type.fn_type(&[i64_type.into(), i64_type.into()], false),
        ),
        int_floordiv: declare(
            "pycc_rt_int_floordiv",
            i64_type.fn_type(&[i64_type.into(), i64_type.into()], false),
        ),
        int_floormod: declare(
            "pycc_rt_int_floormod",
            i64_type.fn_type(&[i64_type.into(), i64_type.into()], false),
        ),
        int_pow: declare(
            "pycc_rt_int_pow",
            i64_type.fn_type(&[i64_type.into(), i64_type.into()], false),
        ),
        int_cmp: declare(
            "pycc_rt_int_cmp",
            i32_type.fn_type(&[i64_type.into(), i64_type.into()], false),
        ),
        int_print: declare("pycc_rt_int_print", void_type.fn_type(&[i64_type.into()], false)),
    }
}

fn tag_smallint_const(context: &Context, n: i64) -> IntValue<'_> {
    // Mirrors `pycc_rt::tag_smallint` exactly (compile-time constant
    // folding of the same encoding) -- an `int` literal whose magnitude
    // doesn't fit the tagged 63-bit range needs a real bigint *literal*,
    // which doesn't exist until Task 9; this is a narrow, honest,
    // compile-time "not supported yet" (not a silent truncation).
    let tagged = (n << 1) | 1;
    if (tagged >> 1) != n {
        panic!(
            "pycc_codegen: integer literal {n} is too large for the v0.1 fast \
             path (bigint literal support lands in a later task)"
        );
    }
    context.i64_type().const_int(tagged as u64, true)
}

fn emit_expr<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    // Unused by every arm this task adds -- prefixed to satisfy
    // `unused_variables`/clippy's `-D warnings` gate until Task 7 adds a
    // `MirExpr::StringLiteral` arm that needs it to build a constant
    // global (Task 7 renames this to `module`, dropping the underscore).
    _module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    // Unused by every arm this task adds -- prefixed to satisfy
    // `unused_variables`/clippy's `-D warnings` gate until Task 5 adds a
    // `MirExpr::Call` arm that needs it (Task 5 renames this to
    // `user_functions`, dropping the underscore, at that point; every
    // call site below already passes its own local `user_functions`
    // variable through regardless of this parameter's own name).
    _user_functions: &HashMap<&str, FunctionValue<'ctx>>,
    locals: &HashMap<String, (PointerValue<'ctx>, pycc_hir::Ty)>,
    expr: &MirExpr,
) -> Scalar<'ctx> {
    use pycc_hir::Ty;
    match expr {
        MirExpr::IntLiteral(n) => Scalar::Int(tag_smallint_const(context, *n)),
        MirExpr::Name { name, ty } => {
            let (ptr, local_ty) = locals
                .get(name)
                .unwrap_or_else(|| panic!("pycc_codegen: internal error: `{name}` has no local slot"));
            debug_assert_eq!(local_ty, ty, "pycc_codegen: internal error: local type drifted");
            let load_ty = match ty {
                Ty::Int => context.i64_type().as_basic_type_enum(),
                Ty::Bool => context.i8_type().as_basic_type_enum(),
                other => panic!("pycc_codegen: reading a `{other:?}`-typed local is not supported yet"),
            };
            let loaded = builder
                .build_load(load_ty, *ptr, "load")
                .expect("build_load should not fail for a slot this function itself allocated");
            match ty {
                Ty::Int => Scalar::Int(loaded.into_int_value()),
                Ty::Bool => Scalar::Bool(loaded.into_int_value()),
                _ => unreachable!("handled above"),
            }
        }
        MirExpr::BinOp { op, left, right, ty: Ty::Int } => {
            let Scalar::Int(l) = emit_expr(context, builder, module, rt, user_functions, locals, left) else {
                panic!("pycc_codegen: internal error: `int` BinOp operand did not evaluate to `int`");
            };
            let Scalar::Int(r) = emit_expr(context, builder, module, rt, user_functions, locals, right) else {
                panic!("pycc_codegen: internal error: `int` BinOp operand did not evaluate to `int`");
            };
            let rt_fn = match op {
                pycc_hir::BinOpKind::Add => rt.int_add,
                pycc_hir::BinOpKind::Sub => rt.int_sub,
                pycc_hir::BinOpKind::Mul => rt.int_mul,
                pycc_hir::BinOpKind::FloorDiv => rt.int_floordiv,
                pycc_hir::BinOpKind::Mod => rt.int_floormod,
                pycc_hir::BinOpKind::Pow => rt.int_pow,
                pycc_hir::BinOpKind::Div => panic!(
                    "pycc_codegen: true division (always `float`) is not supported yet"
                ),
            };
            let result = builder
                .build_call(rt_fn, &[l.into(), r.into()], "int_binop")
                .expect("build_call should not fail for a well-formed int binop")
                .try_as_basic_value()
                .left()
                .expect("pycc_rt_int_* functions all return a non-void `i64`");
            Scalar::Int(result.into_int_value())
        }
        MirExpr::Compare { op, left, right, .. } => {
            let Scalar::Int(l) = emit_expr(context, builder, module, rt, user_functions, locals, left) else {
                panic!("pycc_codegen: comparing a non-`int` operand is not supported yet");
            };
            let Scalar::Int(r) = emit_expr(context, builder, module, rt, user_functions, locals, right) else {
                panic!("pycc_codegen: comparing a non-`int` operand is not supported yet");
            };
            let ordering = builder
                .build_call(rt.int_cmp, &[l.into(), r.into()], "int_cmp")
                .expect("build_call should not fail for a well-formed comparison")
                .try_as_basic_value()
                .left()
                .expect("pycc_rt_int_cmp returns a non-void `i32`")
                .into_int_value();
            let zero = context.i32_type().const_int(0, false);
            let predicate = match op {
                pycc_hir::CmpOpKind::Eq => IntPredicate::EQ,
                pycc_hir::CmpOpKind::NotEq => IntPredicate::NE,
                pycc_hir::CmpOpKind::Lt => IntPredicate::SLT,
                pycc_hir::CmpOpKind::LtE => IntPredicate::SLE,
                pycc_hir::CmpOpKind::Gt => IntPredicate::SGT,
                pycc_hir::CmpOpKind::GtE => IntPredicate::SGE,
            };
            let cond = builder
                .build_int_compare(predicate, ordering, zero, "cmp")
                .expect("build_int_compare should not fail for two i32 operands");
            let as_bool = builder
                .build_int_z_extend(cond, context.i8_type(), "bool_from_cmp")
                .expect("build_int_z_extend should not fail widening i1 to i8");
            Scalar::Bool(as_bool)
        }
        MirExpr::BoolLiteral(b) => {
            Scalar::Bool(context.i8_type().const_int(u64::from(*b), false))
        }
        other => panic!("pycc_codegen: this expression kind's codegen is not supported yet: {other:?}"),
    }
}

/// Allocates (on first assignment) or reuses (on reassignment) the
/// `alloca` backing `target`, stores `value` into it, and records/updates
/// its entry in `locals`. A local's `Ty` never changes across
/// reassignment (`pycc_types` ties one static type to each binding), so
/// reusing an existing slot never needs a type check beyond the
/// `debug_assert_eq!` in `emit_expr`'s `Name` arm above.
fn emit_assign<'ctx>(
    builder: &inkwell::builder::Builder<'ctx>,
    locals: &mut HashMap<String, (PointerValue<'ctx>, pycc_hir::Ty)>,
    target: &str,
    ty: pycc_hir::Ty,
    value: Scalar<'ctx>,
) {
    let ptr = match locals.get(target) {
        Some((ptr, _)) => *ptr,
        None => {
            let alloca_ty: inkwell::types::BasicTypeEnum = match &value {
                Scalar::Int(v) => v.get_type().into(),
                Scalar::Bool(v) => v.get_type().into(),
            };
            let ptr = builder
                .build_alloca(alloca_ty, target)
                .expect("build_alloca should not fail for a supported scalar type");
            locals.insert(target.to_string(), (ptr, ty));
            ptr
        }
    };
    let basic_value: inkwell::values::BasicValueEnum = match value {
        Scalar::Int(v) => v.into(),
        Scalar::Bool(v) => v.into(),
    };
    builder
        .build_store(ptr, basic_value)
        .expect("build_store should not fail for a slot this function itself allocated");
}
```

Note: `Scalar::Int`/`Scalar::Bool`'s `.get_type()`/`.into()` calls need `inkwell::types::BasicType` and `inkwell::values::BasicValue` in scope -- add `use inkwell::types::BasicType;` and `use inkwell::values::BasicValue;` to the existing `use` block alongside the others. `build_load`'s first argument (the type to load) needs `use inkwell::types::BasicTypeEnum;` and `as_basic_type_enum()` needs `use inkwell::types::AnyType;`/`BasicType` in scope too depending on the exact inkwell 0.9 trait layout on this toolchain -- if `cargo build -p pycc_codegen` reports a missing-trait-method error for any of these, add the specific trait `use` it names; this is ordinary, expected TDD friction (Step 6 below), not a design gap.

Also add a shared statement-sequence helper, used by every task from here on (Task 4's `if`/`while`/`for` bodies, Task 5's function bodies) instead of a raw `for stmt in body { emit_stmt(...)?; }` loop:

```rust
/// Emits every statement in `body` in order, stopping early the moment
/// the current block already ends in a terminator. A `return` nested
/// inside an `if`/`while`/`for` body (ordinary, reachable v0.1 Python --
/// `pycc_types`' fallthrough check (T0024) rejects a non-`None` function
/// that *doesn't* return on every path, not a function that returns
/// early and then has trailing dead code after it) creates exactly this
/// situation: any `body` statement after that point is unreachable, and
/// LLVM basic blocks may not contain instructions after their terminator
/// -- so skipping the rest is both correct (matches what CPython itself
/// would ever execute) and required for valid IR, not just an
/// optimization.
fn emit_body<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    user_functions: &HashMap<&str, FunctionValue<'ctx>>,
    locals: &mut HashMap<String, (PointerValue<'ctx>, pycc_hir::Ty)>,
    body: &[MirStmt],
) -> Result<(), String> {
    for stmt in body {
        emit_stmt(context, builder, module, rt, user_functions, locals, stmt)?;
        if builder.get_insert_block().unwrap().get_terminator().is_some() {
            break;
        }
    }
    Ok(())
}
```

### Step 6: Replace `emit_stmt` to use `emit_expr`/`emit_assign` for every statement shape

Replace the whole `emit_stmt` function (Task 2's temporary version) with:

```rust
fn emit_stmt<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    user_functions: &HashMap<&str, FunctionValue<'ctx>>,
    locals: &mut HashMap<String, (inkwell::values::PointerValue<'ctx>, pycc_hir::Ty)>,
    stmt: &MirStmt,
) -> Result<(), String> {
    match stmt {
        MirStmt::ExprStmt(MirExpr::Call { callee, args, .. }) if callee == "print" => {
            match args.as_slice() {
                [expr] if expr.ty() == pycc_hir::Ty::Int => {
                    let Scalar::Int(v) = emit_expr(context, builder, module, rt, user_functions, locals, expr) else {
                        unreachable!("Ty::Int always evaluates to Scalar::Int")
                    };
                    builder
                        .build_call(rt.int_print, &[v.into()], "print_int")
                        .expect("build_call should not fail for a well-formed print call");
                    Ok(())
                }
                _ => panic!(
                    "pycc_codegen: this print() argument shape is not supported yet \
                     (multi-arg / non-int print lands in Task 10)"
                ),
            }
        }
        MirStmt::ExprStmt(MirExpr::Call { callee, args, .. }) if args.is_empty() => {
            let f = user_functions
                .get(callee.as_str())
                .ok_or_else(|| format!("pycc_codegen v0.1: call to undefined function `{callee}`"))?;
            builder
                .build_call(*f, &[], "call_user_fn")
                .expect("build_call should not fail for a well-formed zero-arg call");
            Ok(())
        }
        MirStmt::ExprStmt(expr) => {
            emit_expr(context, builder, module, rt, user_functions, locals, expr);
            Ok(())
        }
        MirStmt::Assign { target, value } => {
            let ty = value.ty();
            let scalar = emit_expr(context, builder, module, rt, user_functions, locals, value);
            emit_assign(builder, locals, target, ty, scalar);
            Ok(())
        }
        other => panic!("pycc_codegen: this statement kind's codegen is not supported yet: {other:?}"),
    }
}
```

`locals`'s tuple order is `(PointerValue<'ctx>, Ty)` everywhere in this plan -- matching `emit_assign`'s signature (Step 5 below), `emit_expr`'s `Name` arm, and every other function that threads a `locals` map -- so `emit_stmt`'s own `HashMap<String, (PointerValue<'ctx>, pycc_hir::Ty)>` above already matches; there is no second convention to reconcile.

Update `compile_to_object`: replace the `print_fn`/`i64_type`-based setup with `RtFns`, thread a fresh `locals: HashMap::new()` per compiled function body (top-level statements share one `locals` map across the synthetic `main` entry block; each user function gets its own, fresh map, since Python function bodies don't see each other's locals), and update every `emit_instr(...)` call site to `emit_stmt(context, &builder, &module, &rt, &user_functions, &mut locals, stmt)`:

```rust
pub fn compile_to_object(
    mir: &MirModule,
    output_path: &Path,
    target_triple: Option<&str>,
) -> Result<(), String> {
    let context = Context::create();
    let module = context.create_module("pycc_module");
    let builder = context.create_builder();
    let i64_type = context.i64_type();
    let rt = declare_rt_functions(&context, &module);

    let no_arg_void_fn_type = context.void_type().fn_type(&[], false);
    let mut user_functions: HashMap<&str, FunctionValue> = HashMap::new();
    for item in &mir.items {
        if let MirItem::Function { name, .. } = item {
            let mangled = format!("pyfn_{name}");
            let f = module.add_function(&mangled, no_arg_void_fn_type, None);
            user_functions.insert(name.as_str(), f);
        }
    }

    let entry_fn_type = i64_type.fn_type(&[], false);
    let entry_fn = module.add_function("main", entry_fn_type, None);
    let entry_block = context.append_basic_block(entry_fn, "entry");
    builder.position_at_end(entry_block);
    let mut top_level_locals = HashMap::new();
    for item in &mir.items {
        if let MirItem::TopLevelStmt(stmt) = item {
            emit_stmt(&context, &builder, &module, &rt, &user_functions, &mut top_level_locals, stmt)?;
        }
    }
    builder
        .build_return(Some(&i64_type.const_int(0, false)))
        .expect(
            "build_return should not fail: builder is always freshly positioned before this call",
        );

    for item in &mir.items {
        if let MirItem::Function { name, body, .. } = item {
            let f = user_functions[name.as_str()];
            let block = context.append_basic_block(f, "entry");
            builder.position_at_end(block);
            let mut fn_locals = HashMap::new();
            emit_body(&context, &builder, &module, &rt, &user_functions, &mut fn_locals, body)?;
            builder
                .build_return(None)
                .expect("build_return should not fail: builder is always freshly positioned before this call");
        }
    }

    verify_module(&module);

    // initialize_all (not initialize_native): a requested target_triple may
    // not match the host's own architecture, and LLVM only has codegen
    // support for a target's backend if that backend was initialized.
    Target::initialize_all(&InitializationConfig::default());
    // ManuallyDrop, not a plain value: see D-029. TargetTriple wraps an
    // LLVMString (inkwell's own message wrapper around LLVMCreateMessage /
    // LLVMGetDefaultTargetTriple), whose Drop calls LLVMDisposeMessage --
    // this crashes on Windows against the official prebuilt LLVM 22.1.1
    // release. Suppressing the drop here, at the point of creation, covers
    // every exit path uniformly (the early `?` below included), not just
    // the success path a trailing forget would. Leaks one small string per
    // compile on every platform -- negligible in a short-lived CLI process,
    // and simpler than cfg-gating a type difference for a Windows-only leak.
    let triple = std::mem::ManuallyDrop::new(match target_triple {
        Some(t) => TargetTriple::create(t),
        None => TargetMachine::get_default_triple(),
    });
    let target = Target::from_triple(&triple).map_err(|e| {
        format!(
            "pycc_codegen: `{}` is not a target LLVM knows how to generate code for: {}",
            triple.as_str().to_string_lossy(),
            llvm_string_to_owned(e)
        )
    })?;
    let target_machine = target
        .create_target_machine(
            &triple,
            "generic",
            "",
            OptimizationLevel::None,
            RelocMode::Default,
            CodeModel::Default,
        )
        .expect(
            "creating a target machine with generic CPU/features should never fail for a \
             triple Target::from_triple has already accepted",
        );
    target_machine
        .write_to_file(&module, FileType::Object, output_path)
        .map_err(llvm_string_to_owned)
}
```

Update every EXISTING test fixture in `crates/pycc_codegen/src/lib.rs` that still constructs `MirItem::Function { name, body }` without `params`/`return_ty` (Task 2 already required adding those two fields at every construction site -- if any were missed, add `params: vec![], return_ty: pycc_hir::Ty::None` now) and every `MirStmt::ExprStmt(MirExpr::Call { callee: "print".to_string(), args: vec![MirExpr::IntLiteral(n)], ty: pycc_hir::Ty::None })` fixture keeps compiling unchanged -- its *output* (`"42\n"` etc.) must stay identical, since tagging is purely an internal representation change invisible to a program's stdout.

- [ ] **Step 6 run:** `cargo test -p pycc_codegen`
Expected: PASS, including every pre-existing test from Task 2/earlier PRs (their expected `stdout` bytes are unaffected by tagging) and the three new tests from Step 4.

### Step 7: Run the full workspace test suite

Run: `cargo test --workspace`
Expected: PASS (includes `tests/slice0.rs`'s e2e tests, still exercising only `print(<i64 literal>)`/zero-arg-call shapes -- their observable output is unaffected by this task's internal representation change).

### Step 8: Run clippy and the coverage gate

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo llvm-cov clean --workspace && cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`
Expected: PASS. If `require_smallint`'s panic branch or `emit_expr`'s various `panic!`/`unreachable!` arms show as uncovered, add a targeted test exercising that exact arm (e.g. a direct `#[should_panic]` unit test calling `emit_expr`/`compile_to_object` with a MIR shape that hits it) rather than weakening the check.

### Step 9: Commit

```bash
git add crates/pycc_rt/src/lib.rs crates/pycc_codegen/src/lib.rs
git commit -m "feat(pycc_rt,pycc_codegen): int/bool locals, arithmetic, and comparisons (Task 3, D-052)"
```

---

## Task 4: `if`/`while`/`for`+`range` control-flow codegen

**Files:**
- Modify: `crates/pycc_rt/src/lib.rs`
- Modify: `crates/pycc_codegen/src/lib.rs`

**Interfaces:**
- Consumes: `Scalar`, `RtFns`, `emit_expr`, `emit_assign`, the `locals` convention (all Task 3, unchanged).
- Produces: a `truthy(...)` helper turning any supported `Scalar` into an LLVM `i1` (extended by Tasks 6/7 as `Float`/`Str` truthiness is added); `pycc_rt_int_truthy(i64) -> i8` and `pycc_rt_range_continue(i64, i64, i64) -> i8` in `pycc_rt`; real `If`/`While`/`ForRange` arms in `emit_stmt`, replacing Task 2's temporary panic for these three shapes. No `pycc_mir` changes -- Task 2's `MirStmt::If/While/ForRange` shapes are already exactly what this task needs.

### Step 1: Write failing `pycc_rt` tests for truthiness and range-continuation

Append to `crates/pycc_rt/src/lib.rs`'s tests:

```rust
#[test]
fn pycc_rt_int_truthy_is_false_only_for_zero() {
    assert_eq!(pycc_rt_int_truthy(tag_smallint(0)), 0);
    assert_eq!(pycc_rt_int_truthy(tag_smallint(1)), 1);
    assert_eq!(pycc_rt_int_truthy(tag_smallint(-1)), 1);
}

#[test]
fn pycc_rt_range_continue_handles_positive_step() {
    assert_eq!(pycc_rt_range_continue(tag_smallint(0), tag_smallint(3), tag_smallint(1)), 1);
    assert_eq!(pycc_rt_range_continue(tag_smallint(3), tag_smallint(3), tag_smallint(1)), 0);
}

#[test]
fn pycc_rt_range_continue_handles_negative_step() {
    assert_eq!(pycc_rt_range_continue(tag_smallint(3), tag_smallint(0), tag_smallint(-1)), 1);
    assert_eq!(pycc_rt_range_continue(tag_smallint(0), tag_smallint(0), tag_smallint(-1)), 0);
}

#[test]
#[should_panic(expected = "must not be zero")]
fn pycc_rt_range_continue_with_a_zero_step_panics() {
    pycc_rt_range_continue(tag_smallint(0), tag_smallint(3), tag_smallint(0));
}
```

- [ ] **Step 1 run:** `cargo test -p pycc_rt`
Expected: FAIL to compile (`pycc_rt_int_truthy`, `pycc_rt_range_continue` don't exist yet).

### Step 2: Implement `pycc_rt_int_truthy`/`pycc_rt_range_continue`

Add to `crates/pycc_rt/src/lib.rs`:

```rust
#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_int_truthy(tagged: i64) -> i8 {
    // A value tagged as a heap `BigInt` (Task 9) is, by construction,
    // only ever created because it *didn't* fit the smallint range --
    // which excludes zero -- so it's always truthy without needing to
    // inspect it further.
    if !is_smallint(tagged) {
        return 1;
    }
    i8::from(untag_smallint(tagged) != 0)
}

#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_range_continue(i: i64, stop: i64, step: i64) -> i8 {
    require_smallint(i, "iterating");
    require_smallint(stop, "iterating");
    require_smallint(step, "iterating");
    let (i, stop, step) = (untag_smallint(i), untag_smallint(stop), untag_smallint(step));
    match step.cmp(&0) {
        std::cmp::Ordering::Greater => i8::from(i < stop),
        std::cmp::Ordering::Less => i8::from(i > stop),
        std::cmp::Ordering::Equal => panic!("pycc_rt: range() arg 3 must not be zero"),
    }
}
```

- [ ] **Step 2 run:** `cargo test -p pycc_rt`
Expected: PASS.

### Step 3: Write failing `pycc_codegen` tests for `if`, `while`, and `for`+`range`

Append to `crates/pycc_codegen/src/lib.rs`'s tests:

```rust
#[test]
fn compiles_an_if_else_choosing_the_correct_branch_at_runtime() {
    // `x = 1; if x < 2: print(10) else: print(20)` -- must print 10.
    let mir = MirModule {
        items: vec![
            MirItem::TopLevelStmt(MirStmt::Assign {
                target: "x".to_string(),
                value: MirExpr::IntLiteral(1),
            }),
            MirItem::TopLevelStmt(MirStmt::If {
                test: MirExpr::Compare {
                    op: pycc_hir::CmpOpKind::Lt,
                    left: Box::new(MirExpr::Name { name: "x".to_string(), ty: pycc_hir::Ty::Int }),
                    right: Box::new(MirExpr::IntLiteral(2)),
                    ty: pycc_hir::Ty::Bool,
                },
                body: vec![MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::IntLiteral(10)],
                    ty: pycc_hir::Ty::None,
                })],
                orelse: vec![MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::IntLiteral(20)],
                    ty: pycc_hir::Ty::None,
                })],
            }),
        ],
    };
    let dir = tempfile_dir("if_else");
    let obj_path = dir.join("if_else.o");
    compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
    let bin_path = dir.join("if_else");
    link_object_with_runtime(&obj_path, &bin_path);
    let output = Command::new(&bin_path).output().expect("binary should run");
    assert_eq!(output.stdout, b"10\n");
}

#[test]
fn compiles_an_if_with_no_else_and_a_false_test_prints_nothing() {
    let mir = MirModule {
        items: vec![MirItem::TopLevelStmt(MirStmt::If {
            test: MirExpr::BoolLiteral(false),
            body: vec![MirStmt::ExprStmt(MirExpr::Call {
                callee: "print".to_string(),
                args: vec![MirExpr::IntLiteral(1)],
                ty: pycc_hir::Ty::None,
            })],
            orelse: vec![],
        })],
    };
    let dir = tempfile_dir("if_no_else");
    let obj_path = dir.join("if_no_else.o");
    compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
    let bin_path = dir.join("if_no_else");
    link_object_with_runtime(&obj_path, &bin_path);
    let output = Command::new(&bin_path).output().expect("binary should run");
    assert_eq!(output.stdout, b"");
}

#[test]
fn compiles_a_while_loop_that_counts_down() {
    // `i = 3; while i > 0: print(i); i = i - 1` -- prints 3, 2, 1.
    let mir = MirModule {
        items: vec![
            MirItem::TopLevelStmt(MirStmt::Assign {
                target: "i".to_string(),
                value: MirExpr::IntLiteral(3),
            }),
            MirItem::TopLevelStmt(MirStmt::While {
                test: MirExpr::Compare {
                    op: pycc_hir::CmpOpKind::Gt,
                    left: Box::new(MirExpr::Name { name: "i".to_string(), ty: pycc_hir::Ty::Int }),
                    right: Box::new(MirExpr::IntLiteral(0)),
                    ty: pycc_hir::Ty::Bool,
                },
                body: vec![
                    MirStmt::ExprStmt(MirExpr::Call {
                        callee: "print".to_string(),
                        args: vec![MirExpr::Name { name: "i".to_string(), ty: pycc_hir::Ty::Int }],
                        ty: pycc_hir::Ty::None,
                    }),
                    MirStmt::Assign {
                        target: "i".to_string(),
                        value: MirExpr::BinOp {
                            op: pycc_hir::BinOpKind::Sub,
                            left: Box::new(MirExpr::Name { name: "i".to_string(), ty: pycc_hir::Ty::Int }),
                            right: Box::new(MirExpr::IntLiteral(1)),
                            ty: pycc_hir::Ty::Int,
                        },
                    },
                ],
            }),
        ],
    };
    let dir = tempfile_dir("while_countdown");
    let obj_path = dir.join("while_countdown.o");
    compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
    let bin_path = dir.join("while_countdown");
    link_object_with_runtime(&obj_path, &bin_path);
    let output = Command::new(&bin_path).output().expect("binary should run");
    assert_eq!(output.stdout, b"3\n2\n1\n");
}

#[test]
fn compiles_a_for_range_loop_with_a_positive_step() {
    // `for i in range(0, 6, 2): print(i)` -- prints 0, 2, 4.
    let mir = MirModule {
        items: vec![MirItem::TopLevelStmt(MirStmt::ForRange {
            var: "i".to_string(),
            start: MirExpr::IntLiteral(0),
            stop: MirExpr::IntLiteral(6),
            step: MirExpr::IntLiteral(2),
            body: vec![MirStmt::ExprStmt(MirExpr::Call {
                callee: "print".to_string(),
                args: vec![MirExpr::Name { name: "i".to_string(), ty: pycc_hir::Ty::Int }],
                ty: pycc_hir::Ty::None,
            })],
        })],
    };
    let dir = tempfile_dir("for_range_pos");
    let obj_path = dir.join("for_range_pos.o");
    compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
    let bin_path = dir.join("for_range_pos");
    link_object_with_runtime(&obj_path, &bin_path);
    let output = Command::new(&bin_path).output().expect("binary should run");
    assert_eq!(output.stdout, b"0\n2\n4\n");
}

#[test]
fn compiles_a_for_range_loop_with_a_negative_step() {
    // `for i in range(3, 0, -1): print(i)` -- prints 3, 2, 1.
    let mir = MirModule {
        items: vec![MirItem::TopLevelStmt(MirStmt::ForRange {
            var: "i".to_string(),
            start: MirExpr::IntLiteral(3),
            stop: MirExpr::IntLiteral(0),
            step: MirExpr::IntLiteral(-1),
            body: vec![MirStmt::ExprStmt(MirExpr::Call {
                callee: "print".to_string(),
                args: vec![MirExpr::Name { name: "i".to_string(), ty: pycc_hir::Ty::Int }],
                ty: pycc_hir::Ty::None,
            })],
        })],
    };
    let dir = tempfile_dir("for_range_neg");
    let obj_path = dir.join("for_range_neg.o");
    compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
    let bin_path = dir.join("for_range_neg");
    link_object_with_runtime(&obj_path, &bin_path);
    let output = Command::new(&bin_path).output().expect("binary should run");
    assert_eq!(output.stdout, b"3\n2\n1\n");
}
```

- [ ] **Step 3 run:** `cargo test -p pycc_codegen`
Expected: FAIL (`If`/`While`/`ForRange` still hit `emit_stmt`'s catch-all panic).

### Step 4: Implement `truthy`, and the `If`/`While`/`ForRange` arms

Extend `RtFns`/`declare_rt_functions` with two new fields, both returning `i8`:

```rust
// RtFns gains:
int_truthy: FunctionValue<'ctx>,
range_continue: FunctionValue<'ctx>,

// declare_rt_functions gains (inside the `RtFns { ... }` literal):
int_truthy: declare("pycc_rt_int_truthy", context.i8_type().fn_type(&[i64_type.into()], false)),
range_continue: declare(
    "pycc_rt_range_continue",
    context.i8_type().fn_type(&[i64_type.into(), i64_type.into(), i64_type.into()], false),
),
```

Add near `emit_expr` in `crates/pycc_codegen/src/lib.rs`:

```rust
fn truthy<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    rt: &RtFns<'ctx>,
    scalar: Scalar<'ctx>,
) -> inkwell::values::IntValue<'ctx> {
    let as_i8 = match scalar {
        Scalar::Bool(v) => v,
        Scalar::Int(v) => builder
            .build_call(rt.int_truthy, &[v.into()], "int_truthy")
            .expect("build_call should not fail for a well-formed truthiness check")
            .try_as_basic_value()
            .left()
            .expect("pycc_rt_int_truthy returns a non-void i8")
            .into_int_value(),
    };
    builder
        .build_int_compare(IntPredicate::NE, as_i8, context.i8_type().const_int(0, false), "truthy")
        .expect("build_int_compare should not fail comparing two i8 operands")
}

/// Emits `body` (via Task 3's `emit_body`, which already stops early at
/// a terminator), then -- only if the current block still does not end
/// in a terminator -- an unconditional branch to `dest`.
fn emit_body_then_branch<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    user_functions: &HashMap<&str, FunctionValue<'ctx>>,
    locals: &mut HashMap<String, (PointerValue<'ctx>, pycc_hir::Ty)>,
    body: &[MirStmt],
    dest: inkwell::basic_block::BasicBlock<'ctx>,
) -> Result<(), String> {
    emit_body(context, builder, module, rt, user_functions, locals, body)?;
    if builder.get_insert_block().unwrap().get_terminator().is_none() {
        builder
            .build_unconditional_branch(dest)
            .expect("build_unconditional_branch should not fail on a block with no terminator yet");
    }
    Ok(())
}
```

Add three new `emit_stmt` match arms (replacing the `other => panic!(...)` catch-all's coverage of these three shapes -- keep the catch-all itself for anything still unhandled):

```rust
MirStmt::If { test, body, orelse } => {
    let function = builder.get_insert_block().unwrap().get_parent().unwrap();
    let cond = {
        let scalar = emit_expr(context, builder, module, rt, user_functions, locals, test);
        truthy(context, builder, rt, scalar)
    };
    let then_bb = context.append_basic_block(function, "if_then");
    let merge_bb = context.append_basic_block(function, "if_merge");
    let else_bb = if orelse.is_empty() { merge_bb } else { context.append_basic_block(function, "if_else") };
    builder
        .build_conditional_branch(cond, then_bb, else_bb)
        .expect("build_conditional_branch should not fail for a well-formed i1 condition");

    builder.position_at_end(then_bb);
    emit_body_then_branch(context, builder, module, rt, user_functions, locals, body, merge_bb)?;

    if !orelse.is_empty() {
        builder.position_at_end(else_bb);
        emit_body_then_branch(context, builder, module, rt, user_functions, locals, orelse, merge_bb)?;
    }

    builder.position_at_end(merge_bb);
    Ok(())
}
MirStmt::While { test, body } => {
    let function = builder.get_insert_block().unwrap().get_parent().unwrap();
    let test_bb = context.append_basic_block(function, "while_test");
    let body_bb = context.append_basic_block(function, "while_body");
    let after_bb = context.append_basic_block(function, "while_after");

    builder
        .build_unconditional_branch(test_bb)
        .expect("build_unconditional_branch should not fail entering the loop test");
    builder.position_at_end(test_bb);
    let cond = {
        let scalar = emit_expr(context, builder, module, rt, user_functions, locals, test);
        truthy(context, builder, rt, scalar)
    };
    builder
        .build_conditional_branch(cond, body_bb, after_bb)
        .expect("build_conditional_branch should not fail for a well-formed i1 condition");

    builder.position_at_end(body_bb);
    emit_body_then_branch(context, builder, module, rt, user_functions, locals, body, test_bb)?;

    builder.position_at_end(after_bb);
    Ok(())
}
MirStmt::ForRange { var, start, stop, step, body } => {
    let function = builder.get_insert_block().unwrap().get_parent().unwrap();
    let Scalar::Int(start_v) = emit_expr(context, builder, module, rt, user_functions, locals, start) else {
        panic!("pycc_codegen: internal error: range() start did not evaluate to int")
    };
    let Scalar::Int(stop_v) = emit_expr(context, builder, module, rt, user_functions, locals, stop) else {
        panic!("pycc_codegen: internal error: range() stop did not evaluate to int")
    };
    let Scalar::Int(step_v) = emit_expr(context, builder, module, rt, user_functions, locals, step) else {
        panic!("pycc_codegen: internal error: range() step did not evaluate to int")
    };
    emit_assign(builder, locals, var, pycc_hir::Ty::Int, Scalar::Int(start_v));

    let test_bb = context.append_basic_block(function, "for_test");
    let body_bb = context.append_basic_block(function, "for_body");
    let after_bb = context.append_basic_block(function, "for_after");

    builder
        .build_unconditional_branch(test_bb)
        .expect("build_unconditional_branch should not fail entering the loop test");
    builder.position_at_end(test_bb);
    let (var_ptr, _) = *locals.get(var).expect("range() var was just bound above");
    let current = builder
        .build_load(context.i64_type(), var_ptr, "for_var")
        .expect("build_load should not fail for this function's own alloca")
        .into_int_value();
    let cont = builder
        .build_call(rt.range_continue, &[current.into(), stop_v.into(), step_v.into()], "range_continue")
        .expect("build_call should not fail for a well-formed range_continue check")
        .try_as_basic_value()
        .left()
        .expect("pycc_rt_range_continue returns a non-void i8")
        .into_int_value();
    let cont_i1 = builder
        .build_int_compare(IntPredicate::NE, cont, context.i8_type().const_int(0, false), "for_cont")
        .expect("build_int_compare should not fail comparing two i8 operands");
    builder
        .build_conditional_branch(cont_i1, body_bb, after_bb)
        .expect("build_conditional_branch should not fail for a well-formed i1 condition");

    builder.position_at_end(body_bb);
    emit_body(context, builder, module, rt, user_functions, locals, body)?;
    if builder.get_insert_block().unwrap().get_terminator().is_none() {
        let current = builder
            .build_load(context.i64_type(), var_ptr, "for_var_reload")
            .expect("build_load should not fail for this function's own alloca")
            .into_int_value();
        let next = builder
            .build_call(rt.int_add, &[current.into(), step_v.into()], "for_next")
            .expect("build_call should not fail for a well-formed int add")
            .try_as_basic_value()
            .left()
            .expect("pycc_rt_int_add returns a non-void i64")
            .into_int_value();
        builder
            .build_store(var_ptr, next)
            .expect("build_store should not fail for this function's own alloca");
        builder
            .build_unconditional_branch(test_bb)
            .expect("build_unconditional_branch should not fail on a block with no terminator yet");
    }

    builder.position_at_end(after_bb);
    Ok(())
}
```

`emit_stmt`'s own signature must now accept `context: &'ctx Context` if it doesn't already (Task 3's Step 6 version already does). Reload `var`'s current value from its `alloca` (`for_var_reload`) rather than reusing the SSA value loaded before the loop body ran, since the body may itself reassign `var` -- unusual Python, but not statically excluded, and re-reading from memory is what makes this correct either way (this is exactly why `--debug`/no-`mem2reg` unoptimized `alloca`s, per this plan's own header, are the simplest correct choice: always go through memory, never assume an SSA value survives across a block the body may have mutated).

- [ ] **Step 4 run:** `cargo test -p pycc_codegen`
Expected: PASS.

### Step 5: Run the full workspace test suite

Run: `cargo test --workspace`
Expected: PASS.

### Step 6: Run clippy and the coverage gate

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo llvm-cov clean --workspace && cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`
Expected: PASS.

### Step 7: Commit

```bash
git add crates/pycc_rt/src/lib.rs crates/pycc_codegen/src/lib.rs
git commit -m "feat(pycc_rt,pycc_codegen): if/while/for-range control-flow codegen (Task 4)"
```

---

## Task 5: Function calls with real parameters/return values, and recursion

**Files:**
- Modify: `crates/pycc_codegen/src/lib.rs`

**Interfaces:**
- Consumes: `Scalar`, `RtFns`, `emit_expr`, `emit_stmt`, `emit_body`, `ty_to_basic_type` (new in this task; Tasks 6/7 extend it), `locals` (Tasks 3/4, unchanged).
- Produces: real (non-zero-arg, non-void-only) function declarations; a `MirExpr::Call` arm in `emit_expr` handling *any* user function call as an expression (superseding `emit_stmt`'s old zero-arg-only special case, which this task removes as redundant); a `MirStmt::Return` arm in `emit_stmt`. `emit_expr`'s signature gains real use of its (previously `_user_functions`-prefixed, unused) function-table parameter -- rename it back to `user_functions` here.

### Step 1: Write a failing test for a function with real parameters and a return value

Append to `crates/pycc_codegen/src/lib.rs`'s tests:

```rust
#[test]
fn compiles_a_function_call_with_real_arguments_and_a_return_value() {
    // `def add(a: int, b: int) -> int: return a + b` ; `print(add(2, 3))`
    let mir = MirModule {
        items: vec![
            MirItem::Function {
                name: "add".to_string(),
                params: vec![("a".to_string(), pycc_hir::Ty::Int), ("b".to_string(), pycc_hir::Ty::Int)],
                return_ty: pycc_hir::Ty::Int,
                body: vec![MirStmt::Return(Some(MirExpr::BinOp {
                    op: pycc_hir::BinOpKind::Add,
                    left: Box::new(MirExpr::Name { name: "a".to_string(), ty: pycc_hir::Ty::Int }),
                    right: Box::new(MirExpr::Name { name: "b".to_string(), ty: pycc_hir::Ty::Int }),
                    ty: pycc_hir::Ty::Int,
                }))],
            },
            MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                callee: "print".to_string(),
                args: vec![MirExpr::Call {
                    callee: "add".to_string(),
                    args: vec![MirExpr::IntLiteral(2), MirExpr::IntLiteral(3)],
                    ty: pycc_hir::Ty::Int,
                }],
                ty: pycc_hir::Ty::None,
            })),
        ],
    };
    let dir = tempfile_dir("call_with_args");
    let obj_path = dir.join("call_with_args.o");
    compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
    let bin_path = dir.join("call_with_args");
    link_object_with_runtime(&obj_path, &bin_path);
    let output = Command::new(&bin_path).output().expect("binary should run");
    assert_eq!(output.stdout, b"5\n");
}

#[test]
fn compiles_a_recursive_function_with_an_early_return() {
    // `def fact(n: int) -> int:\n    if n <= 1:\n        return 1\n    return n * fact(n - 1)`
    // `print(fact(5))` -- exercises recursion (calling `fact` from inside
    // its own not-yet-fully-emitted body works because the two-pass
    // declare-then-define structure already declares every function
    // before any body is compiled), a return nested inside an `if` with
    // no `else`, and a second `return` reached only via that `if`'s false
    // edge (Task 4's `merge_bb` handling).
    let fact_body = vec![
        MirStmt::If {
            test: MirExpr::Compare {
                op: pycc_hir::CmpOpKind::LtE,
                left: Box::new(MirExpr::Name { name: "n".to_string(), ty: pycc_hir::Ty::Int }),
                right: Box::new(MirExpr::IntLiteral(1)),
                ty: pycc_hir::Ty::Bool,
            },
            body: vec![MirStmt::Return(Some(MirExpr::IntLiteral(1)))],
            orelse: vec![],
        },
        MirStmt::Return(Some(MirExpr::BinOp {
            op: pycc_hir::BinOpKind::Mul,
            left: Box::new(MirExpr::Name { name: "n".to_string(), ty: pycc_hir::Ty::Int }),
            right: Box::new(MirExpr::Call {
                callee: "fact".to_string(),
                args: vec![MirExpr::BinOp {
                    op: pycc_hir::BinOpKind::Sub,
                    left: Box::new(MirExpr::Name { name: "n".to_string(), ty: pycc_hir::Ty::Int }),
                    right: Box::new(MirExpr::IntLiteral(1)),
                    ty: pycc_hir::Ty::Int,
                }],
                ty: pycc_hir::Ty::Int,
            }),
            ty: pycc_hir::Ty::Int,
        })),
    ];
    let mir = MirModule {
        items: vec![
            MirItem::Function {
                name: "fact".to_string(),
                params: vec![("n".to_string(), pycc_hir::Ty::Int)],
                return_ty: pycc_hir::Ty::Int,
                body: fact_body,
            },
            MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                callee: "print".to_string(),
                args: vec![MirExpr::Call {
                    callee: "fact".to_string(),
                    args: vec![MirExpr::IntLiteral(5)],
                    ty: pycc_hir::Ty::Int,
                }],
                ty: pycc_hir::Ty::None,
            })),
        ],
    };
    let dir = tempfile_dir("recursive_fact");
    let obj_path = dir.join("recursive_fact.o");
    compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
    let bin_path = dir.join("recursive_fact");
    link_object_with_runtime(&obj_path, &bin_path);
    let output = Command::new(&bin_path).output().expect("binary should run");
    assert_eq!(output.stdout, b"120\n");
}

#[test]
fn a_non_none_function_falling_through_is_an_internal_error_not_bad_ir() {
    // `pycc_types`' T0024 fallthrough check should have rejected this
    // HIR already -- this proves codegen fails loudly (a clear panic)
    // rather than emitting an invalid `ret` from a function declared to
    // return `int`, if that check is ever somehow bypassed.
    let mir = MirModule {
        items: vec![MirItem::Function {
            name: "broken".to_string(),
            params: vec![],
            return_ty: pycc_hir::Ty::Int,
            body: vec![],
        }],
    };
    let dir = tempfile_dir("fallthrough_internal_error");
    let obj_path = dir.join("fallthrough_internal_error.o");
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        compile_to_object(&mir, &obj_path, None)
    }));
    assert!(result.is_err(), "expected a panic, not a successfully-compiled object");
}
```

- [ ] **Step 1 run:** `cargo test -p pycc_codegen`
Expected: FAIL (functions are still declared zero-arg/void regardless of `params`/`return_ty`; `MirStmt::Return` and a general `MirExpr::Call` still hit `emit_stmt`/`emit_expr`'s catch-all panics).

### Step 2: Implement real function signatures, parameter binding, `Return`, and general `Call`

Add `ty_to_basic_type` near `emit_expr` in `crates/pycc_codegen/src/lib.rs` (extended by Tasks 6/7 with `Float`/`Str` arms):

```rust
fn ty_to_basic_type(context: &Context, ty: pycc_hir::Ty) -> inkwell::types::BasicTypeEnum<'_> {
    match ty {
        pycc_hir::Ty::Int => context.i64_type().into(),
        pycc_hir::Ty::Bool => context.i8_type().into(),
        other => panic!("pycc_codegen: a `{other:?}`-typed parameter/return value is not supported yet"),
    }
}
```

Rename `emit_expr`'s `_user_functions` parameter (Task 3) to `user_functions` (it's used below now), and add a `MirExpr::Call` arm:

```rust
MirExpr::Call { callee, args, ty } => {
    if callee == "print" {
        panic!("pycc_codegen: using print()'s result as a nested expression is not supported yet");
    }
    let f = *user_functions.get(callee.as_str()).unwrap_or_else(|| {
        panic!(
            "pycc_codegen: internal error: call to undefined function `{callee}` \
             should have been rejected by pycc_types before reaching codegen"
        )
    });
    let arg_values: Vec<inkwell::values::BasicMetadataValueEnum> = args
        .iter()
        .map(|a| match emit_expr(context, builder, module, rt, user_functions, locals, a) {
            Scalar::Int(v) => v.into(),
            Scalar::Bool(v) => v.into(),
        })
        .collect();
    let call_site = builder
        .build_call(f, &arg_values, "call_user_fn")
        .expect("build_call should not fail for a well-formed user function call");
    match ty {
        pycc_hir::Ty::Int => Scalar::Int(
            call_site
                .try_as_basic_value()
                .left()
                .expect("this function is declared to return int")
                .into_int_value(),
        ),
        pycc_hir::Ty::Bool => Scalar::Bool(
            call_site
                .try_as_basic_value()
                .left()
                .expect("this function is declared to return bool")
                .into_int_value(),
        ),
        other => panic!("pycc_codegen: a `{other:?}`-typed call result is not supported yet"),
    }
}
```

Add a `MirStmt::Return` arm to `emit_stmt` (replacing its coverage in the catch-all):

```rust
MirStmt::Return(value) => {
    match value {
        Some(expr) => {
            let scalar = emit_expr(context, builder, module, rt, user_functions, locals, expr);
            let basic_value: inkwell::values::BasicValueEnum = match scalar {
                Scalar::Int(v) => v.into(),
                Scalar::Bool(v) => v.into(),
            };
            builder
                .build_return(Some(&basic_value))
                .expect("build_return should not fail for a well-formed return value");
        }
        None => {
            builder
                .build_return(None)
                .expect("build_return should not fail for a bare `return`");
        }
    }
    Ok(())
}
```

Remove `emit_stmt`'s now-redundant `MirStmt::ExprStmt(MirExpr::Call { callee, args, .. }) if args.is_empty()` arm entirely -- the plain `MirStmt::ExprStmt(expr) => { emit_expr(...); Ok(()) }` arm already below it now handles every call (zero-arg or not) via `emit_expr`'s new `Call` arm, so the dedicated arm is dead weight, not dead code (it would never be reached differently, but keeping both invites drift).

Replace the whole `compile_to_object` function (Task 3/4's version) with:

```rust
pub fn compile_to_object(
    mir: &MirModule,
    output_path: &Path,
    target_triple: Option<&str>,
) -> Result<(), String> {
    let context = Context::create();
    let module = context.create_module("pycc_module");
    let builder = context.create_builder();
    let i64_type = context.i64_type();
    let rt = declare_rt_functions(&context, &module);

    let mut user_functions: HashMap<&str, FunctionValue> = HashMap::new();
    for item in &mir.items {
        if let MirItem::Function { name, params, return_ty, .. } = item {
            let param_types: Vec<inkwell::types::BasicMetadataTypeEnum> = params
                .iter()
                .map(|(_, ty)| ty_to_basic_type(&context, *ty).into())
                .collect();
            let fn_type = match return_ty {
                pycc_hir::Ty::None => context.void_type().fn_type(&param_types, false),
                other => ty_to_basic_type(&context, *other).fn_type(&param_types, false),
            };
            let mangled = format!("pyfn_{name}");
            let f = module.add_function(&mangled, fn_type, None);
            user_functions.insert(name.as_str(), f);
        }
    }

    let entry_fn_type = i64_type.fn_type(&[], false);
    let entry_fn = module.add_function("main", entry_fn_type, None);
    let entry_block = context.append_basic_block(entry_fn, "entry");
    builder.position_at_end(entry_block);
    let mut top_level_locals = HashMap::new();
    for item in &mir.items {
        if let MirItem::TopLevelStmt(stmt) = item {
            emit_stmt(&context, &builder, &module, &rt, &user_functions, &mut top_level_locals, stmt)?;
        }
    }
    builder
        .build_return(Some(&i64_type.const_int(0, false)))
        .expect(
            "build_return should not fail: builder is always freshly positioned before this call",
        );

    for item in &mir.items {
        if let MirItem::Function { name, params, return_ty, body } = item {
            let f = user_functions[name.as_str()];
            let block = context.append_basic_block(f, "entry");
            builder.position_at_end(block);
            let mut fn_locals = HashMap::new();
            for (i, (param_name, ty)) in params.iter().enumerate() {
                let incoming = f.get_nth_param(i as u32).unwrap_or_else(|| {
                    panic!("pycc_codegen: internal error: missing parameter {i} for `{name}`")
                });
                let ptr = builder
                    .build_alloca(ty_to_basic_type(&context, *ty), param_name)
                    .expect("build_alloca should not fail for a supported scalar type");
                builder
                    .build_store(ptr, incoming)
                    .expect("build_store should not fail for a slot this function itself allocated");
                fn_locals.insert(param_name.clone(), (ptr, *ty));
            }
            emit_body(&context, &builder, &module, &rt, &user_functions, &mut fn_locals, body)?;
            match return_ty {
                pycc_hir::Ty::None => {
                    if builder.get_insert_block().unwrap().get_terminator().is_none() {
                        builder.build_return(None).expect(
                            "build_return should not fail: builder is always freshly positioned before this call",
                        );
                    }
                }
                _ if builder.get_insert_block().unwrap().get_terminator().is_none() => {
                    panic!(
                        "pycc_codegen: internal error: `{name}` is declared to return a \
                         non-`None` value but fell through without a `return` -- \
                         pycc_types::check (T0024) should have rejected this HIR before \
                         it reached codegen"
                    );
                }
                _ => {}
            }
        }
    }

    verify_module(&module);

    Target::initialize_all(&InitializationConfig::default());
    let triple = std::mem::ManuallyDrop::new(match target_triple {
        Some(t) => TargetTriple::create(t),
        None => TargetMachine::get_default_triple(),
    });
    let target = Target::from_triple(&triple).map_err(|e| {
        format!(
            "pycc_codegen: `{}` is not a target LLVM knows how to generate code for: {}",
            triple.as_str().to_string_lossy(),
            llvm_string_to_owned(e)
        )
    })?;
    let target_machine = target
        .create_target_machine(
            &triple,
            "generic",
            "",
            OptimizationLevel::None,
            RelocMode::Default,
            CodeModel::Default,
        )
        .expect(
            "creating a target machine with generic CPU/features should never fail for a \
             triple Target::from_triple has already accepted",
        );
    target_machine
        .write_to_file(&module, FileType::Object, output_path)
        .map_err(llvm_string_to_owned)
}
```

- [ ] **Step 2 run:** `cargo test -p pycc_codegen`
Expected: PASS.

### Step 3: Run the full workspace test suite

Run: `cargo test --workspace`
Expected: PASS.

### Step 4: Run clippy and the coverage gate

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo llvm-cov clean --workspace && cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`
Expected: PASS.

### Step 5: Commit

```bash
git add crates/pycc_codegen/src/lib.rs
git commit -m "feat(pycc_codegen): real function parameters, return values, and recursion (Task 5)"
```

---

## Task 6: `float`/`bool` runtime representation and arithmetic promotion codegen

**Scope note on `None`:** the original brainstorm scope named this task "`float`/`bool`/`None` runtime representation." Repo research found there is nothing to build for `None` as a *value*: `pycc_hir::HirExpr` has no `NoneLiteral` variant at all (confirmed by reading `crates/pycc_hir/src/lib.rs` in full -- `annotation_to_ty` handles `Expr::NoneLiteral` only in *type-annotation* position, e.g. `def f(x: None) -> None`, and `lower_expr`'s catch-all panics for a bare `None` used as a value). `Ty::None` only ever appears as a function's return type or a parameter's declared type; the only way a `Ty::None`-typed *value* can flow anywhere is a call to a function whose `return_ty` is `None` (including `print` itself). This task therefore covers `float` and `bool` promotion for real; `None` handling is folded into Task 10 (`print`'s dispatch), which is the only place a `None`-typed expression is ever actually reachable (e.g. `print(some_void_function())`).

**Files:**
- Modify: `crates/pycc_rt/src/lib.rs`
- Modify: `crates/pycc_codegen/src/lib.rs`

**Interfaces:**
- Consumes: `Scalar`, `RtFns`, `emit_expr`, `emit_assign`, `truthy`, `ty_to_basic_type` (Tasks 3-5).
- Produces: `Scalar::Float(FloatValue<'ctx>)` (new variant, added alongside `Int`/`Bool`, never replacing them); `pycc_rt_int_to_float(i64) -> f64`, `pycc_rt_float_floordiv/floormod/pow(f64, f64) -> f64` in `pycc_rt`; `to_tagged_int`/`to_float` codegen helpers coercing a `Scalar` to the domain a given operation needs (per `pycc_types::numeric_result_type`/`numeric_or_bool_compatible`'s existing promotion rules -- `bool` is an `int` subtype, and any `float` operand promotes the whole expression to `float`); `MirExpr::BinOp`/`MirExpr::Compare`'s arms in `emit_expr` are **replaced** (not added alongside) with versions that dispatch on the actual operand/result types instead of assuming `int`-only.

### Step 1: Write failing `pycc_rt` tests for float conversion and floor/pow helpers

Append to `crates/pycc_rt/src/lib.rs`'s tests:

```rust
#[test]
fn pycc_rt_int_to_float_converts_the_untagged_value() {
    assert_eq!(pycc_rt_int_to_float(tag_smallint(5)), 5.0);
    assert_eq!(pycc_rt_int_to_float(tag_smallint(-3)), -3.0);
}

#[test]
fn pycc_rt_float_floordiv_matches_python_floor_semantics() {
    assert_eq!(pycc_rt_float_floordiv(7.0, 2.0), 3.0);
    assert_eq!(pycc_rt_float_floordiv(-7.0, 2.0), -4.0);
}

#[test]
fn pycc_rt_float_floormod_matches_python_floor_semantics() {
    assert_eq!(pycc_rt_float_floormod(-7.0, 2.0), 1.0);
    assert_eq!(pycc_rt_float_floormod(7.0, 2.0), 1.0);
}

#[test]
fn pycc_rt_float_pow_computes_the_correct_power() {
    assert_eq!(pycc_rt_float_pow(2.0, 10.0), 1024.0);
    assert_eq!(pycc_rt_float_pow(9.0, 0.5), 3.0);
}
```

- [ ] **Step 1 run:** `cargo test -p pycc_rt`
Expected: FAIL to compile.

### Step 2: Implement the float helpers in `pycc_rt`

```rust
#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_int_to_float(tagged: i64) -> f64 {
    require_smallint(tagged, "converting");
    untag_smallint(tagged) as f64
}

#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_float_floordiv(a: f64, b: f64) -> f64 {
    (a / b).floor()
}

#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_float_floormod(a: f64, b: f64) -> f64 {
    let r = a % b;
    if r != 0.0 && (r < 0.0) != (b < 0.0) { r + b } else { r }
}

#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_float_pow(a: f64, b: f64) -> f64 {
    a.powf(b)
}
```

- [ ] **Step 2 run:** `cargo test -p pycc_rt`
Expected: PASS.

### Step 3: Write failing `pycc_codegen` tests for float/bool arithmetic, comparisons, and truthiness

Append to `crates/pycc_codegen/src/lib.rs`'s tests (these use a small helper that isn't written yet -- `print_float` doesn't exist until Task 10, so these tests only prove codegen succeeds and the generated IR verifies, following the same style as Task 3's `compiles_a_comparison_result_stored_in_a_bool_local`):

```rust
#[test]
fn compiles_true_division_of_two_ints_as_float_arithmetic() {
    // `x = 7 / 2` -- must promote both operands to float and use
    // `fdiv`, not integer division (`pycc_types` already types this
    // `Ty::Float`; this proves codegen honors that, not `int`'s own
    // `//`).
    let mir = MirModule {
        items: vec![MirItem::TopLevelStmt(MirStmt::Assign {
            target: "x".to_string(),
            value: MirExpr::BinOp {
                op: pycc_hir::BinOpKind::Div,
                left: Box::new(MirExpr::IntLiteral(7)),
                right: Box::new(MirExpr::IntLiteral(2)),
                ty: pycc_hir::Ty::Float,
            },
        })],
    };
    let dir = tempfile_dir("true_div");
    let obj_path = dir.join("true_div.o");
    compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
}

#[test]
fn compiles_mixed_int_and_float_addition() {
    // `y = 1 + 1.5` -- promotes the `int` operand to `float`.
    let mir = MirModule {
        items: vec![MirItem::TopLevelStmt(MirStmt::Assign {
            target: "y".to_string(),
            value: MirExpr::BinOp {
                op: pycc_hir::BinOpKind::Add,
                left: Box::new(MirExpr::IntLiteral(1)),
                right: Box::new(MirExpr::FloatLiteral(1.5)),
                ty: pycc_hir::Ty::Float,
            },
        })],
    };
    let dir = tempfile_dir("mixed_add");
    let obj_path = dir.join("mixed_add.o");
    compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
}

#[test]
fn compiles_bool_arithmetic_promoted_to_int() {
    // `z = True + True` -- Python's `bool` is an `int` subtype; the
    // result is `2` (`int`), not a `bool`.
    let mir = MirModule {
        items: vec![
            MirItem::TopLevelStmt(MirStmt::Assign {
                target: "z".to_string(),
                value: MirExpr::BinOp {
                    op: pycc_hir::BinOpKind::Add,
                    left: Box::new(MirExpr::BoolLiteral(true)),
                    right: Box::new(MirExpr::BoolLiteral(true)),
                    ty: pycc_hir::Ty::Int,
                },
            }),
            MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                callee: "print".to_string(),
                args: vec![MirExpr::Name { name: "z".to_string(), ty: pycc_hir::Ty::Int }],
                ty: pycc_hir::Ty::None,
            })),
        ],
    };
    let dir = tempfile_dir("bool_arith");
    let obj_path = dir.join("bool_arith.o");
    compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
    let bin_path = dir.join("bool_arith");
    link_object_with_runtime(&obj_path, &bin_path);
    let output = Command::new(&bin_path).output().expect("binary should run");
    assert_eq!(output.stdout, b"2\n");
}

#[test]
fn compiles_a_float_comparison() {
    let mir = MirModule {
        items: vec![MirItem::TopLevelStmt(MirStmt::Assign {
            target: "b".to_string(),
            value: MirExpr::Compare {
                op: pycc_hir::CmpOpKind::Lt,
                left: Box::new(MirExpr::FloatLiteral(1.5)),
                right: Box::new(MirExpr::FloatLiteral(2.5)),
                ty: pycc_hir::Ty::Bool,
            },
        })],
    };
    let dir = tempfile_dir("float_cmp");
    let obj_path = dir.join("float_cmp.o");
    compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
}

#[test]
fn compiles_an_if_test_on_a_float_expression() {
    // `if 0.0: print(1)` -- must print nothing (`0.0` is falsy).
    // `if 1.5: print(1)` -- must print `1`.
    for (test, expected) in [(0.0, ""), (1.5, "1\n")] {
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::If {
                test: MirExpr::FloatLiteral(test),
                body: vec![MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::IntLiteral(1)],
                    ty: pycc_hir::Ty::None,
                })],
                orelse: vec![],
            })],
        };
        let dir = tempfile_dir(&format!("float_truthy_{test}"));
        let obj_path = dir.join("float_truthy.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("float_truthy");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, expected.as_bytes(), "test value {test}");
    }
}
```

- [ ] **Step 3 run:** `cargo test -p pycc_codegen`
Expected: FAIL (`FloatLiteral` and float-typed `BinOp`/`Compare` still hit `emit_expr`'s catch-all; `truthy` has no `Scalar::Float` arm).

### Step 4: Add `Scalar::Float`, `FloatLiteral`, and the coercion helpers

Add the `Float` variant to `Scalar` (declared in Task 3):

```rust
enum Scalar<'ctx> {
    Int(IntValue<'ctx>),
    Bool(IntValue<'ctx>),
    Float(inkwell::values::FloatValue<'ctx>),
}
```

Add two coercion helpers and extend `RtFns`/`declare_rt_functions` with four new fields -- `int_to_float` (`i64 -> f64`), `float_floordiv`, `float_floormod`, `float_pow` (each `(f64, f64) -> f64`) -- declared the same way as Task 3's `int_*` fields, using `context.f64_type()` for the float-typed parameter/return positions:

```rust
// RtFns gains:
int_to_float: FunctionValue<'ctx>,
float_floordiv: FunctionValue<'ctx>,
float_floormod: FunctionValue<'ctx>,
float_pow: FunctionValue<'ctx>,

// declare_rt_functions gains (inside the `RtFns { ... }` literal), using
// `let f64_type = context.f64_type();` alongside its existing bindings:
int_to_float: declare("pycc_rt_int_to_float", f64_type.fn_type(&[i64_type.into()], false)),
float_floordiv: declare(
    "pycc_rt_float_floordiv",
    f64_type.fn_type(&[f64_type.into(), f64_type.into()], false),
),
float_floormod: declare(
    "pycc_rt_float_floormod",
    f64_type.fn_type(&[f64_type.into(), f64_type.into()], false),
),
float_pow: declare(
    "pycc_rt_float_pow",
    f64_type.fn_type(&[f64_type.into(), f64_type.into()], false),
),
```

```rust
/// `bool` is an `int` subtype (Python/`pycc_types`'
/// `numeric_or_bool_compatible`) -- widens a `Bool` scalar to a tagged
/// `int` (D-052) via two trivial, unambiguous LLVM instructions (a
/// zero-extend then a shift-and-or matching `pycc_rt::tag_smallint`
/// exactly); an existing `Int` scalar passes through unchanged. Panics
/// for `Float`, which is never `int`-coercible.
fn to_tagged_int<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    scalar: Scalar<'ctx>,
) -> IntValue<'ctx> {
    match scalar {
        Scalar::Int(v) => v,
        Scalar::Bool(v) => {
            let widened = builder
                .build_int_z_extend(v, context.i64_type(), "bool_to_i64")
                .expect("build_int_z_extend should not fail widening i8 to i64");
            let shifted = builder
                .build_left_shift(widened, context.i64_type().const_int(1, false), "tag_shl")
                .expect("build_left_shift should not fail for a constant shift amount");
            builder
                .build_or(shifted, context.i64_type().const_int(1, false), "tag_or")
                .expect("build_or should not fail for two i64 operands")
        }
        Scalar::Float(_) => panic!("pycc_codegen: internal error: expected an int-or-bool operand, got float"),
    }
}

/// Promotes any numeric `Scalar` to `f64`: an existing `Float` passes
/// through; `Int` goes through `pycc_rt_int_to_float` (never a raw LLVM
/// cast -- the value is D-052-tagged, so only `pycc_rt` may interpret
/// its bits); `Bool` uses a plain unsigned-int-to-float conversion
/// (unambiguous for a 0/1 value, no tagging involved).
fn to_float<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    rt: &RtFns<'ctx>,
    scalar: Scalar<'ctx>,
) -> inkwell::values::FloatValue<'ctx> {
    match scalar {
        Scalar::Float(v) => v,
        Scalar::Int(v) => builder
            .build_call(rt.int_to_float, &[v.into()], "int_to_float")
            .expect("build_call should not fail for a well-formed conversion")
            .try_as_basic_value()
            .left()
            .expect("pycc_rt_int_to_float returns a non-void f64")
            .into_float_value(),
        Scalar::Bool(v) => builder
            .build_unsigned_int_to_float(v, context.f64_type(), "bool_to_float")
            .expect("build_unsigned_int_to_float should not fail for an i8 0/1 value"),
    }
}
```

Add `MirExpr::FloatLiteral` to `emit_expr`:

```rust
MirExpr::FloatLiteral(f) => Scalar::Float(context.f64_type().const_float(*f)),
```

### Step 5: Replace `emit_expr`'s `BinOp` arm to dispatch on the result type

Replace the whole `MirExpr::BinOp { op, left, right, ty: Ty::Int } => { ... }` arm from Task 3 with:

```rust
MirExpr::BinOp { op, left, right, ty } => {
    let l = emit_expr(context, builder, module, rt, user_functions, locals, left);
    let r = emit_expr(context, builder, module, rt, user_functions, locals, right);
    match ty {
        Ty::Int => {
            let l = to_tagged_int(context, builder, l);
            let r = to_tagged_int(context, builder, r);
            let rt_fn = match op {
                pycc_hir::BinOpKind::Add => rt.int_add,
                pycc_hir::BinOpKind::Sub => rt.int_sub,
                pycc_hir::BinOpKind::Mul => rt.int_mul,
                pycc_hir::BinOpKind::FloorDiv => rt.int_floordiv,
                pycc_hir::BinOpKind::Mod => rt.int_floormod,
                pycc_hir::BinOpKind::Pow => rt.int_pow,
                pycc_hir::BinOpKind::Div => {
                    unreachable!("pycc_types/pycc_mir always type true division as Ty::Float")
                }
            };
            let result = builder
                .build_call(rt_fn, &[l.into(), r.into()], "int_binop")
                .expect("build_call should not fail for a well-formed int binop")
                .try_as_basic_value()
                .left()
                .expect("pycc_rt_int_* functions all return a non-void i64");
            Scalar::Int(result.into_int_value())
        }
        Ty::Float => {
            let l = to_float(context, builder, rt, l);
            let r = to_float(context, builder, rt, r);
            match op {
                pycc_hir::BinOpKind::Add => Scalar::Float(
                    builder.build_float_add(l, r, "fadd").expect("build_float_add should not fail for two f64 operands"),
                ),
                pycc_hir::BinOpKind::Sub => Scalar::Float(
                    builder.build_float_sub(l, r, "fsub").expect("build_float_sub should not fail for two f64 operands"),
                ),
                pycc_hir::BinOpKind::Mul => Scalar::Float(
                    builder.build_float_mul(l, r, "fmul").expect("build_float_mul should not fail for two f64 operands"),
                ),
                pycc_hir::BinOpKind::Div => Scalar::Float(
                    builder.build_float_div(l, r, "fdiv").expect("build_float_div should not fail for two f64 operands"),
                ),
                pycc_hir::BinOpKind::FloorDiv | pycc_hir::BinOpKind::Mod | pycc_hir::BinOpKind::Pow => {
                    let rt_fn = match op {
                        pycc_hir::BinOpKind::FloorDiv => rt.float_floordiv,
                        pycc_hir::BinOpKind::Mod => rt.float_floormod,
                        pycc_hir::BinOpKind::Pow => rt.float_pow,
                        _ => unreachable!("guarded by the outer match arm"),
                    };
                    let result = builder
                        .build_call(rt_fn, &[l.into(), r.into()], "float_binop")
                        .expect("build_call should not fail for a well-formed float binop")
                        .try_as_basic_value()
                        .left()
                        .expect("pycc_rt_float_* functions all return a non-void f64");
                    Scalar::Float(result.into_float_value())
                }
            }
        }
        other => panic!("pycc_codegen: a `{other:?}`-result BinOp is not supported yet"),
    }
}
```

### Step 6: Replace `emit_expr`'s `Compare` arm to promote per operand type

Replace the whole `MirExpr::Compare { op, left, right, .. } => { ... }` arm from Task 3 with:

```rust
MirExpr::Compare { op, left, right, .. } => {
    let left_ty = left.ty();
    let right_ty = right.ty();
    let l = emit_expr(context, builder, module, rt, user_functions, locals, left);
    let r = emit_expr(context, builder, module, rt, user_functions, locals, right);
    let as_bool = if left_ty == Ty::Float || right_ty == Ty::Float {
        let l = to_float(context, builder, rt, l);
        let r = to_float(context, builder, rt, r);
        let predicate = match op {
            pycc_hir::CmpOpKind::Eq => inkwell::FloatPredicate::OEQ,
            // `UNE` ("unordered or not equal"), not `ONE` -- CPython's
            // `float('nan') != float('nan')` is `True`, and `NaN`
            // involves an *unordered* comparison, not an ordered
            // not-equal one. The other five predicates below correctly
            // stay "ordered" (`O*`): Python's `<`/`<=`/`>`/`>=`/`==` on
            // `float` are all `False` whenever `NaN` is involved, which
            // is exactly what the ordered forms give.
            pycc_hir::CmpOpKind::NotEq => inkwell::FloatPredicate::UNE,
            pycc_hir::CmpOpKind::Lt => inkwell::FloatPredicate::OLT,
            pycc_hir::CmpOpKind::LtE => inkwell::FloatPredicate::OLE,
            pycc_hir::CmpOpKind::Gt => inkwell::FloatPredicate::OGT,
            pycc_hir::CmpOpKind::GtE => inkwell::FloatPredicate::OGE,
        };
        let cond = builder
            .build_float_compare(predicate, l, r, "fcmp")
            .expect("build_float_compare should not fail for two f64 operands");
        builder
            .build_int_z_extend(cond, context.i8_type(), "bool_from_fcmp")
            .expect("build_int_z_extend should not fail widening i1 to i8")
    } else {
        let l = to_tagged_int(context, builder, l);
        let r = to_tagged_int(context, builder, r);
        let ordering = builder
            .build_call(rt.int_cmp, &[l.into(), r.into()], "int_cmp")
            .expect("build_call should not fail for a well-formed comparison")
            .try_as_basic_value()
            .left()
            .expect("pycc_rt_int_cmp returns a non-void i32")
            .into_int_value();
        let zero = context.i32_type().const_int(0, false);
        let predicate = match op {
            pycc_hir::CmpOpKind::Eq => IntPredicate::EQ,
            pycc_hir::CmpOpKind::NotEq => IntPredicate::NE,
            pycc_hir::CmpOpKind::Lt => IntPredicate::SLT,
            pycc_hir::CmpOpKind::LtE => IntPredicate::SLE,
            pycc_hir::CmpOpKind::Gt => IntPredicate::SGT,
            pycc_hir::CmpOpKind::GtE => IntPredicate::SGE,
        };
        let cond = builder
            .build_int_compare(predicate, ordering, zero, "cmp")
            .expect("build_int_compare should not fail for two i32 operands");
        builder
            .build_int_z_extend(cond, context.i8_type(), "bool_from_cmp")
            .expect("build_int_z_extend should not fail widening i1 to i8")
    };
    Scalar::Bool(as_bool)
}
```

Don't run `cargo test -p pycc_codegen` yet -- `Scalar` gained a third variant in Step 4, so every other `match` over `Scalar` in this file (`emit_assign`, `emit_expr`'s `Name` arm) is non-exhaustive and won't compile until Step 7 extends them too. The first real compile/verify checkpoint is at the end of Step 7.

### Step 7: Extend `truthy`, `ty_to_basic_type`, and `emit_assign` for `Scalar::Float`

`truthy` (Task 4) gains a `Scalar::Float` arm. Python's `bool(x)` for a `float` is `False` only for exactly `0.0`/`-0.0` -- `NaN` is truthy -- so this needs the *unordered*-or-not-equal predicate, `UNE`, not `ONE` (same distinction as Step 6's fix):

```rust
fn truthy<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    rt: &RtFns<'ctx>,
    scalar: Scalar<'ctx>,
) -> IntValue<'ctx> {
    let as_i8 = match scalar {
        Scalar::Bool(v) => v,
        Scalar::Int(v) => builder
            .build_call(rt.int_truthy, &[v.into()], "int_truthy")
            .expect("build_call should not fail for a well-formed truthiness check")
            .try_as_basic_value()
            .left()
            .expect("pycc_rt_int_truthy returns a non-void i8")
            .into_int_value(),
        Scalar::Float(v) => {
            let zero = context.f64_type().const_float(0.0);
            let cond = builder
                .build_float_compare(inkwell::FloatPredicate::UNE, v, zero, "float_truthy")
                .expect("build_float_compare should not fail for two f64 operands");
            builder
                .build_int_z_extend(cond, context.i8_type(), "bool_from_float_truthy")
                .expect("build_int_z_extend should not fail widening i1 to i8")
        }
    };
    builder
        .build_int_compare(IntPredicate::NE, as_i8, context.i8_type().const_int(0, false), "truthy")
        .expect("build_int_compare should not fail comparing two i8 operands")
}
```

`Scalar` gained a variant, so every `match` over a `Scalar` value written in Tasks 3-5 is now non-exhaustive and needs a `Float` arm before any of this compiles. Every one of the following gets `Scalar::Float(v) => v.into()` (or the `get_type()`/pointer-extraction equivalent shown), alongside its existing `Int`/`Bool` arms -- **add, never replace**:

- `emit_assign`'s (Task 3) two internal `match` expressions: `Scalar::Float(v) => v.get_type().into()` (the alloca-type-selection match) and `Scalar::Float(v) => v.into()` (the store-value match).
- `emit_expr`'s `Name` arm (Task 3): `Ty::Float => context.f64_type().as_basic_type_enum()` in its `load_ty` match, and `Ty::Float => Scalar::Float(loaded.into_float_value())` in its post-load match.
- `emit_expr`'s `Call` arm (Task 5): the argument-marshaling `match emit_expr(...) { Scalar::Int(v) => v.into(), Scalar::Bool(v) => v.into() }` gains `Scalar::Float(v) => v.into()`; the call-result `match ty { Ty::Int => ..., Ty::Bool => ..., other => panic!(...) }` gains `pycc_hir::Ty::Float => Scalar::Float(call_site.try_as_basic_value().left().expect("this function is declared to return float").into_float_value()),` (a function can now be declared with a `float` parameter/return once `ty_to_basic_type` above supports it).
- `emit_stmt`'s `MirStmt::Return` arm (Task 5): its `let basic_value: BasicValueEnum = match scalar { Scalar::Int(v) => v.into(), Scalar::Bool(v) => v.into() }` gains `Scalar::Float(v) => v.into()`.

Compile errors naming exactly these four match sites (by their now-missing-arm diagnostics) are the expected way to rediscover this list while implementing -- Rust's own exhaustiveness checker is the actual source of truth here, this list is just a shortcut to it.

- [ ] **Step 7 run:** `cargo test -p pycc_codegen`
Expected: PASS, including every earlier test (Tasks 3-5's `int`-only fixtures are untouched by these additive `Float` match arms).

### Step 8: Run the full workspace test suite

Run: `cargo test --workspace`
Expected: PASS.

### Step 9: Run clippy and the coverage gate

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo llvm-cov clean --workspace && cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`
Expected: PASS.

### Step 10: Commit

```bash
git add crates/pycc_rt/src/lib.rs crates/pycc_codegen/src/lib.rs
git commit -m "feat(pycc_rt,pycc_codegen): float/bool arithmetic promotion codegen (Task 6)"
```

---

## Task 7: `str` runtime (small-string optimization) and codegen -- literals, concatenation, comparisons

**Scope note (deviation from the original brainstorm):** the brainstorm scoped this task to "literals, concatenation" only, leaving `str` comparison (`==`/`!=`/`<`/etc., lexicographic ordering) undelivered anywhere in this plan. Repo research found `pycc_types::infer_expr`'s `Compare` arm (and its own test `comparing_two_strings_infers_bool`) already accepts all six comparison operators on two `str` operands today -- a real, reachable v0.1 program (`"a" < "b"`) that this plan's own self-review would otherwise flag as a spec-coverage gap with no task. String comparison is folded into this task rather than a new one, since it needs exactly the runtime this task already builds and nothing from Task 8/9/10.

**Files:**
- Modify: `crates/pycc_rt/src/lib.rs`
- Modify: `crates/pycc_codegen/src/lib.rs`

**Interfaces:**
- Consumes: `Scalar`, `RtFns`, `emit_expr`, `emit_assign`, `truthy`, `ty_to_basic_type`, `to_tagged_int`, `to_float` (Tasks 3-6).
- Produces: `Scalar::Str(PointerValue<'ctx>)` (new variant); `pycc_rt`'s `PyStrObj` (D-050's inline-≤22-bytes/heap representation, opaque to `pycc_codegen`) and its C ABI: `pycc_rt_str_from_literal(*const u8, i64) -> *mut PyStrObj`, `pycc_rt_str_concat`, `pycc_rt_str_cmp`, `pycc_rt_str_truthy`, `pycc_rt_str_incref`, `pycc_rt_str_decref`; every `Scalar`-exhaustiveness call site from Tasks 3-6 gains a `Str` arm; `MirExpr::StringLiteral`, a `Ty::Str` arm in `BinOp` (concatenation) and a str-aware `Compare` dispatch; a `str_value_is_a_duplicate_reference`/`incref_if_str_duplicate`/`decref_old_str_if_reassigning` refcounting convention used at every point a `str` value is bound into a new owning slot.

### Step 1: Record the `str` refcounting scope limitation

This task implements D-051's "unconditionally refcounted and freed on refcount reaching zero" for the case that's both realistic and tractable without `pycc_own`: a `str`-typed local variable reassigned within the same scope (e.g. a loop rebinding a string each iteration) decrefs its previous value, and top-level `str` locals (which have exactly one exit point -- module-level code has no `return`, per `T0024`) are decrefed at program completion. A **known, accepted limitation**, honest per this project's documented-gap convention (same spirit as D-026/D-043): a `str`-typed function **parameter or local that is never reassigned again before that function returns is not decrefed at the return site** in this task -- a function can have multiple `return` statements (an early return inside an `if`, for instance), and decrefing every live `str` local at *every* return site is real, separate scope this task does not attempt. This never causes a use-after-free or double-free (nothing is freed early) -- only an additional accepted leak, matching this project's "simplest safe default instead of building real analysis" brief for `pycc_own`'s absence. Record this in `docs/DECISIONS.md` as part of D-052's family rather than a new ID (it is a refinement of the same "simplest safe default" call, not a separate irreversible choice) -- add one sentence to D-052's own Consequences paragraph:

- [ ] Append to D-052's "Consequences" paragraph in `docs/DECISIONS.md`: `" \`str\`'s own refcounting (Task 7) follows the same honesty convention: reassignment-in-scope and top-level completion are handled; a function-scoped `str` local not reassigned before its function returns is an accepted, documented leak until \`pycc_own\` (v0.5) makes real liveness tracking possible."`

- [ ] **Step 1 commit:**

```bash
git add docs/DECISIONS.md
git commit -m "docs: record str refcounting scope limitation under D-052"
```

### Step 2: Write failing `pycc_rt` tests for the `str` representation and refcounting

Append to `crates/pycc_rt/src/lib.rs`'s tests:

```rust
#[test]
fn a_short_literal_round_trips_through_the_inline_representation() {
    let bytes = b"hi";
    let s = pycc_rt_str_from_literal(bytes.as_ptr(), bytes.len() as i64);
    assert_eq!(unsafe { &*s }.bytes(), b"hi");
    pycc_rt_str_decref(s);
}

#[test]
fn a_long_literal_round_trips_through_the_heap_representation() {
    let long = "x".repeat(23); // one byte past the 22-byte inline cap (D-050)
    let s = pycc_rt_str_from_literal(long.as_ptr(), long.len() as i64);
    assert_eq!(unsafe { &*s }.bytes(), long.as_bytes());
    pycc_rt_str_decref(s);
}

#[test]
fn concat_joins_bytes_from_both_operands() {
    let a = pycc_rt_str_from_literal(b"foo".as_ptr(), 3);
    let b = pycc_rt_str_from_literal(b"bar".as_ptr(), 3);
    let joined = pycc_rt_str_concat(a, b);
    assert_eq!(unsafe { &*joined }.bytes(), b"foobar");
    pycc_rt_str_decref(a);
    pycc_rt_str_decref(b);
    pycc_rt_str_decref(joined);
}

#[test]
fn cmp_orders_strings_lexicographically() {
    let a = pycc_rt_str_from_literal(b"apple".as_ptr(), 5);
    let b = pycc_rt_str_from_literal(b"banana".as_ptr(), 6);
    assert_eq!(pycc_rt_str_cmp(a, a), 0);
    assert_eq!(pycc_rt_str_cmp(a, b), -1);
    assert_eq!(pycc_rt_str_cmp(b, a), 1);
    pycc_rt_str_decref(a);
    pycc_rt_str_decref(b);
}

#[test]
fn truthy_is_false_only_for_the_empty_string() {
    let empty = pycc_rt_str_from_literal(b"".as_ptr(), 0);
    let non_empty = pycc_rt_str_from_literal(b"x".as_ptr(), 1);
    assert_eq!(pycc_rt_str_truthy(empty), 0);
    assert_eq!(pycc_rt_str_truthy(non_empty), 1);
    pycc_rt_str_decref(empty);
    pycc_rt_str_decref(non_empty);
}

#[test]
fn incref_then_decref_survives_until_the_final_decref() {
    let s = pycc_rt_str_from_literal(b"hi".as_ptr(), 2);
    pycc_rt_str_incref(s); // rc 1 -> 2
    pycc_rt_str_decref(s); // rc 2 -> 1, must NOT free yet
    assert_eq!(pycc_rt_str_cmp(s, s), 0); // still safe to read
    pycc_rt_str_decref(s); // rc 1 -> 0, frees
}

#[test]
fn incref_and_decref_on_a_null_pointer_are_safe_no_ops() {
    pycc_rt_str_incref(std::ptr::null_mut());
    pycc_rt_str_decref(std::ptr::null_mut());
}
```

- [ ] **Step 2 run:** `cargo test -p pycc_rt`
Expected: FAIL to compile.

### Step 3: Implement the `str` representation and its C ABI in `pycc_rt`

```rust
use std::cell::Cell;

/// D-050: up to 22 bytes inline (no heap allocation for the byte
/// payload itself); longer strings heap-allocate. Either way, the
/// *object* (`PyStrObj`, including its refcount) is always exactly one
/// heap allocation -- `pycc_codegen` only ever sees an opaque pointer to
/// it (D-052's ABI-avoidance principle, applied to `str` too: no struct
/// ever crosses the LLVM/Rust boundary by value).
enum PyStrPayload {
    Inline([u8; 22], u8), // bytes (only the first `len` are meaningful), len
    Heap(Box<[u8]>),
}

struct PyStrObj {
    rc: Cell<u32>,
    payload: PyStrPayload,
}

impl PyStrObj {
    fn bytes(&self) -> &[u8] {
        match &self.payload {
            PyStrPayload::Inline(buf, len) => &buf[..*len as usize],
            PyStrPayload::Heap(b) => b,
        }
    }
}

fn new_pystr(bytes: &[u8]) -> *mut PyStrObj {
    let payload = if bytes.len() <= 22 {
        let mut buf = [0u8; 22];
        buf[..bytes.len()].copy_from_slice(bytes);
        PyStrPayload::Inline(buf, bytes.len() as u8)
    } else {
        PyStrPayload::Heap(bytes.to_vec().into_boxed_slice())
    };
    Box::into_raw(Box::new(PyStrObj { rc: Cell::new(1), payload }))
}

/// # Safety
/// `ptr` must point to at least `len` readable bytes (true for every
/// `pycc_codegen`-emitted call site, which always passes a compile-time
/// string literal's own constant global and byte length together).
#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_str_from_literal(ptr: *const u8, len: i64) -> *mut PyStrObj {
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len as usize) };
    new_pystr(bytes)
}

/// # Safety
/// `a`/`b` must be live `PyStrObj` pointers (every `pycc_codegen` call
/// site only ever passes a value it just evaluated from a well-typed
/// `Ty::Str` expression).
#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_str_concat(a: *mut PyStrObj, b: *mut PyStrObj) -> *mut PyStrObj {
    let a_bytes = unsafe { &*a }.bytes();
    let b_bytes = unsafe { &*b }.bytes();
    let mut combined = Vec::with_capacity(a_bytes.len() + b_bytes.len());
    combined.extend_from_slice(a_bytes);
    combined.extend_from_slice(b_bytes);
    new_pystr(&combined)
}

/// # Safety
/// Same as `pycc_rt_str_concat`.
#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_str_cmp(a: *mut PyStrObj, b: *mut PyStrObj) -> i32 {
    match unsafe { &*a }.bytes().cmp(unsafe { &*b }.bytes()) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

/// # Safety
/// `s` must be a live `PyStrObj` pointer.
#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_str_truthy(s: *mut PyStrObj) -> i8 {
    i8::from(!unsafe { &*s }.bytes().is_empty())
}

#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_str_incref(s: *mut PyStrObj) {
    if s.is_null() {
        return;
    }
    let obj = unsafe { &*s };
    obj.rc.set(obj.rc.get() + 1);
}

#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_str_decref(s: *mut PyStrObj) {
    if s.is_null() {
        return;
    }
    let new_rc = unsafe { &*s }.rc.get() - 1;
    if new_rc == 0 {
        drop(unsafe { Box::from_raw(s) });
    } else {
        unsafe { &*s }.rc.set(new_rc);
    }
}
```

- [ ] **Step 3 run:** `cargo test -p pycc_rt`
Expected: PASS.

### Step 4: Write failing `pycc_codegen` tests for `str` literals, concatenation, comparison, and reassignment

Append to `crates/pycc_codegen/src/lib.rs`'s tests:

```rust
#[test]
fn compiles_string_concatenation_and_a_reassignment_that_frees_the_old_value() {
    // `x = "foo"; x = x + "bar"` -- the second `Assign` reads the
    // existing `x` (needs an incref before rebinding) and overwrites
    // `x`'s slot (must decref the *original* `"foo"` first). Nothing
    // observes the refcounting directly; this proves it doesn't crash
    // and that codegen for the whole sequence succeeds.
    let mir = MirModule {
        items: vec![
            MirItem::TopLevelStmt(MirStmt::Assign {
                target: "x".to_string(),
                value: MirExpr::StringLiteral("foo".to_string()),
            }),
            MirItem::TopLevelStmt(MirStmt::Assign {
                target: "x".to_string(),
                value: MirExpr::BinOp {
                    op: pycc_hir::BinOpKind::Add,
                    left: Box::new(MirExpr::Name { name: "x".to_string(), ty: pycc_hir::Ty::Str }),
                    right: Box::new(MirExpr::StringLiteral("bar".to_string())),
                    ty: pycc_hir::Ty::Str,
                },
            }),
        ],
    };
    let dir = tempfile_dir("str_concat_reassign");
    let obj_path = dir.join("str_concat_reassign.o");
    compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
    let bin_path = dir.join("str_concat_reassign");
    link_object_with_runtime(&obj_path, &bin_path);
    let output = Command::new(&bin_path).output().expect("binary should run");
    assert!(output.status.success(), "should run without crashing");
}

#[test]
fn compiles_a_string_comparison() {
    let mir = MirModule {
        items: vec![MirItem::TopLevelStmt(MirStmt::Assign {
            target: "b".to_string(),
            value: MirExpr::Compare {
                op: pycc_hir::CmpOpKind::Lt,
                left: Box::new(MirExpr::StringLiteral("apple".to_string())),
                right: Box::new(MirExpr::StringLiteral("banana".to_string())),
                ty: pycc_hir::Ty::Bool,
            },
        })],
    };
    let dir = tempfile_dir("str_cmp");
    let obj_path = dir.join("str_cmp.o");
    compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
}

#[test]
fn compiles_an_if_test_on_a_string_expression() {
    // `if "": print(1)` prints nothing; `if "x": print(1)` prints `1`.
    for (test, expected) in [("", ""), ("x", "1\n")] {
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::If {
                test: MirExpr::StringLiteral(test.to_string()),
                body: vec![MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::IntLiteral(1)],
                    ty: pycc_hir::Ty::None,
                })],
                orelse: vec![],
            })],
        };
        let dir = tempfile_dir(&format!("str_truthy_{}", test.len()));
        let obj_path = dir.join("str_truthy.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("str_truthy");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, expected.as_bytes(), "test value {test:?}");
    }
}

#[test]
fn compiles_a_string_literal_longer_than_the_inline_cap() {
    let long = "y".repeat(30); // exceeds D-050's 22-byte inline threshold
    let mir = MirModule {
        items: vec![MirItem::TopLevelStmt(MirStmt::Assign {
            target: "s".to_string(),
            value: MirExpr::StringLiteral(long),
        })],
    };
    let dir = tempfile_dir("str_long_literal");
    let obj_path = dir.join("str_long_literal.o");
    compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
}
```

- [ ] **Step 4 run:** `cargo test -p pycc_codegen`
Expected: FAIL (`Scalar::Str` doesn't exist; `StringLiteral` and `Ty::Str` still hit catch-alls).

### Step 5: Add `Scalar::Str`, `RtFns`'s new fields, and `MirExpr::StringLiteral`

Add the `Str` variant to `Scalar`:

```rust
enum Scalar<'ctx> {
    Int(IntValue<'ctx>),
    Bool(IntValue<'ctx>),
    Float(inkwell::values::FloatValue<'ctx>),
    Str(PointerValue<'ctx>),
}
```

Extend `RtFns` with six new fields (`str_from_literal`, `str_concat`, `str_cmp`, `str_truthy`, `str_incref`, `str_decref`) and `declare_rt_functions` with their declarations -- `str_from_literal` takes `(ptr, i64) -> ptr`, `str_concat` takes `(ptr, ptr) -> ptr`, `str_cmp` takes `(ptr, ptr) -> i32`, `str_truthy`/`str_incref`/`str_decref` take/return `ptr`/`i8`/`void` as appropriate, using `context.ptr_type(inkwell::AddressSpace::default())` for every pointer type:

```rust
// RtFns gains:
str_from_literal: FunctionValue<'ctx>,
str_concat: FunctionValue<'ctx>,
str_cmp: FunctionValue<'ctx>,
str_truthy: FunctionValue<'ctx>,
str_incref: FunctionValue<'ctx>,
str_decref: FunctionValue<'ctx>,

// declare_rt_functions gains (inside the `RtFns { ... }` literal):
str_from_literal: declare(
    "pycc_rt_str_from_literal",
    ptr_type.fn_type(&[ptr_type.into(), i64_type.into()], false),
),
str_concat: declare(
    "pycc_rt_str_concat",
    ptr_type.fn_type(&[ptr_type.into(), ptr_type.into()], false),
),
str_cmp: declare(
    "pycc_rt_str_cmp",
    i32_type.fn_type(&[ptr_type.into(), ptr_type.into()], false),
),
str_truthy: declare(
    "pycc_rt_str_truthy",
    context.i8_type().fn_type(&[ptr_type.into()], false),
),
str_incref: declare("pycc_rt_str_incref", void_type.fn_type(&[ptr_type.into()], false)),
str_decref: declare("pycc_rt_str_decref", void_type.fn_type(&[ptr_type.into()], false)),
```

(`declare_rt_functions` needs a `let ptr_type = context.ptr_type(inkwell::AddressSpace::default());` binding alongside its existing `i64_type`/`i32_type`/`void_type` ones.)

Add `MirExpr::StringLiteral` to `emit_expr`, embedding the literal's bytes as a constant global (never null-terminated -- a Python `str` can contain an embedded `\0`, so the byte length is always passed explicitly rather than relying on termination):

```rust
MirExpr::StringLiteral(s) => {
    let bytes = s.as_bytes();
    let array_ty = context.i8_type().array_type(bytes.len() as u32);
    let global = module.add_global(array_ty, None, "str_lit");
    global.set_initializer(&context.const_string(bytes, false));
    global.set_constant(true);
    global.set_linkage(Linkage::Private);
    let ptr = global.as_pointer_value();
    let len = context.i64_type().const_int(bytes.len() as u64, false);
    let call_site = builder
        .build_call(rt.str_from_literal, &[ptr.into(), len.into()], "str_lit_obj")
        .expect("build_call should not fail for a well-formed string literal construction");
    Scalar::Str(
        call_site
            .try_as_basic_value()
            .left()
            .expect("pycc_rt_str_from_literal returns a non-void pointer")
            .into_pointer_value(),
    )
}
```

`emit_expr` needs `module: &inkwell::module::Module<'ctx>` to reach `module.add_global` for a string literal's constant. This plan already threads it everywhere it needs to go, matching Task 5's `user_functions` precedent exactly: Task 3's original `emit_expr` signature declared it as `_module` (unused-until-now, same reasoning as `_user_functions` was in that same task) -- rename it to `module` here, dropping the underscore. Every one of Tasks 3-6's `emit_expr(context, builder, module, rt, user_functions, locals, ...)` call sites, and `emit_body`/`emit_stmt`/`emit_body_then_branch`/`compile_to_object`'s own signatures, already carry a `module` parameter threaded through the same way `rt` is (this plan's Tasks 3-6 text already shows it in place, not as a separate retrofit step here) -- `truthy` does *not* need it (it never touches the module, only an already-evaluated `Scalar`).

- [ ] **Step 5 run:** do not run yet -- `Scalar`'s new variant makes every match from Tasks 3-6 non-exhaustive; Step 6 finishes those before anything compiles.

### Step 6: Extend every existing match/helper for `Scalar::Str`

Exactly like Task 6's Step 7, extend every site that matches `Scalar` or dispatches on `Ty`, adding a `Str` arm alongside the existing ones (never replacing):

- `emit_assign`'s two internal matches: `Scalar::Str(v) => v.get_type().into()` (alloca-type) and `Scalar::Str(v) => v.into()` (store-value).
- `emit_expr`'s `Name` arm: `Ty::Str => context.ptr_type(inkwell::AddressSpace::default()).as_basic_type_enum()` in `load_ty`; `Ty::Str => Scalar::Str(loaded.into_pointer_value())` in the post-load match.
- `emit_expr`'s `Call` arm: `Scalar::Str(v) => v.into()` in the argument-marshaling match; `pycc_hir::Ty::Str => Scalar::Str(call_site.try_as_basic_value().left().expect("this function is declared to return str").into_pointer_value()),` in the call-result match.
- `emit_stmt`'s `Return` arm: `Scalar::Str(v) => v.into()` in its `basic_value` match.
- `ty_to_basic_type`: `pycc_hir::Ty::Str => context.ptr_type(inkwell::AddressSpace::default()).into(),`.
- `to_tagged_int` (Task 6): `Scalar::Str(_) => panic!("pycc_codegen: internal error: expected an int-or-bool operand, got str"),`.
- `to_float` (Task 6): `Scalar::Str(_) => panic!("pycc_codegen: internal error: expected a numeric operand, got str"),`.
- `truthy` (Task 4/6) gains:

```rust
Scalar::Str(v) => {
    let result = builder
        .build_call(rt.str_truthy, &[v.into()], "str_truthy")
        .expect("build_call should not fail for a well-formed truthiness check")
        .try_as_basic_value()
        .left()
        .expect("pycc_rt_str_truthy returns a non-void i8")
        .into_int_value();
    result
}
```

### Step 7: Replace `emit_expr`'s `BinOp` arm's outer `match ty` to add string concatenation

Add a `Ty::Str` arm to the `match ty { Ty::Int => ..., Ty::Float => ..., other => panic!(...) }` from Task 6 (the `Int`/`Float` arms are unchanged):

```rust
Ty::Str => {
    let Scalar::Str(l) = l else {
        panic!("pycc_codegen: internal error: str BinOp operand did not evaluate to str")
    };
    let Scalar::Str(r) = r else {
        panic!("pycc_codegen: internal error: str BinOp operand did not evaluate to str")
    };
    if *op != pycc_hir::BinOpKind::Add {
        panic!("pycc_codegen: `str {op:?} str` is not supported yet (only concatenation is)");
    }
    let result = builder
        .build_call(rt.str_concat, &[l.into(), r.into()], "str_concat")
        .expect("build_call should not fail for a well-formed concatenation")
        .try_as_basic_value()
        .left()
        .expect("pycc_rt_str_concat returns a non-void pointer");
    Scalar::Str(result.into_pointer_value())
}
```

### Step 8: Replace `emit_expr`'s `Compare` arm to add string comparison

Replace Task 6's `Compare` arm's dispatch (`if left_ty == Ty::Float || right_ty == Ty::Float { ... } else { ... int path ... }`) with a three-way dispatch:

```rust
MirExpr::Compare { op, left, right, .. } => {
    let left_ty = left.ty();
    let right_ty = right.ty();
    let l = emit_expr(context, builder, module, rt, user_functions, locals, left);
    let r = emit_expr(context, builder, module, rt, user_functions, locals, right);
    let as_bool = if left_ty == Ty::Float || right_ty == Ty::Float {
        let l = to_float(context, builder, rt, l);
        let r = to_float(context, builder, rt, r);
        let predicate = match op {
            pycc_hir::CmpOpKind::Eq => inkwell::FloatPredicate::OEQ,
            // `UNE` ("unordered or not equal"), not `ONE` -- CPython's
            // `float('nan') != float('nan')` is `True`, and `NaN`
            // involves an *unordered* comparison, not an ordered
            // not-equal one. The other five predicates below correctly
            // stay "ordered" (`O*`): Python's `<`/`<=`/`>`/`>=`/`==` on
            // `float` are all `False` whenever `NaN` is involved, which
            // is exactly what the ordered forms give.
            pycc_hir::CmpOpKind::NotEq => inkwell::FloatPredicate::UNE,
            pycc_hir::CmpOpKind::Lt => inkwell::FloatPredicate::OLT,
            pycc_hir::CmpOpKind::LtE => inkwell::FloatPredicate::OLE,
            pycc_hir::CmpOpKind::Gt => inkwell::FloatPredicate::OGT,
            pycc_hir::CmpOpKind::GtE => inkwell::FloatPredicate::OGE,
        };
        let cond = builder
            .build_float_compare(predicate, l, r, "fcmp")
            .expect("build_float_compare should not fail for two f64 operands");
        builder
            .build_int_z_extend(cond, context.i8_type(), "bool_from_fcmp")
            .expect("build_int_z_extend should not fail widening i1 to i8")
    } else if left_ty == Ty::Str || right_ty == Ty::Str {
        let Scalar::Str(l) = l else {
            panic!("pycc_codegen: internal error: str Compare operand did not evaluate to str")
        };
        let Scalar::Str(r) = r else {
            panic!("pycc_codegen: internal error: str Compare operand did not evaluate to str")
        };
        let ordering = builder
            .build_call(rt.str_cmp, &[l.into(), r.into()], "str_cmp")
            .expect("build_call should not fail for a well-formed comparison")
            .try_as_basic_value()
            .left()
            .expect("pycc_rt_str_cmp returns a non-void i32")
            .into_int_value();
        let zero = context.i32_type().const_int(0, false);
        let predicate = match op {
            pycc_hir::CmpOpKind::Eq => IntPredicate::EQ,
            pycc_hir::CmpOpKind::NotEq => IntPredicate::NE,
            pycc_hir::CmpOpKind::Lt => IntPredicate::SLT,
            pycc_hir::CmpOpKind::LtE => IntPredicate::SLE,
            pycc_hir::CmpOpKind::Gt => IntPredicate::SGT,
            pycc_hir::CmpOpKind::GtE => IntPredicate::SGE,
        };
        let cond = builder
            .build_int_compare(predicate, ordering, zero, "str_cmp_pred")
            .expect("build_int_compare should not fail for two i32 operands");
        builder
            .build_int_z_extend(cond, context.i8_type(), "bool_from_str_cmp")
            .expect("build_int_z_extend should not fail widening i1 to i8")
    } else {
        let l = to_tagged_int(context, builder, l);
        let r = to_tagged_int(context, builder, r);
        let ordering = builder
            .build_call(rt.int_cmp, &[l.into(), r.into()], "int_cmp")
            .expect("build_call should not fail for a well-formed comparison")
            .try_as_basic_value()
            .left()
            .expect("pycc_rt_int_cmp returns a non-void i32")
            .into_int_value();
        let zero = context.i32_type().const_int(0, false);
        let predicate = match op {
            pycc_hir::CmpOpKind::Eq => IntPredicate::EQ,
            pycc_hir::CmpOpKind::NotEq => IntPredicate::NE,
            pycc_hir::CmpOpKind::Lt => IntPredicate::SLT,
            pycc_hir::CmpOpKind::LtE => IntPredicate::SLE,
            pycc_hir::CmpOpKind::Gt => IntPredicate::SGT,
            pycc_hir::CmpOpKind::GtE => IntPredicate::SGE,
        };
        let cond = builder
            .build_int_compare(predicate, ordering, zero, "cmp")
            .expect("build_int_compare should not fail for two i32 operands");
        builder
            .build_int_z_extend(cond, context.i8_type(), "bool_from_cmp")
            .expect("build_int_z_extend should not fail widening i1 to i8")
    };
    Scalar::Bool(as_bool)
}
```

- [ ] **Step 8 run:** `cargo test -p pycc_codegen`
Expected: FAIL still -- reassignment refcounting (Step 9) is needed for `compiles_string_concatenation_and_a_reassignment_that_frees_the_old_value` to be *correct* (it will currently compile and run without the refcounting calls, since nothing yet inserts them -- add Step 9 before trusting this test proves anything beyond "codegen doesn't crash").

### Step 9: Add `str` refcounting at binding sites

```rust
/// Whether evaluating `expr` produces a *duplicate* reference to an
/// already-owned `str` (a bare variable read) rather than a fresh
/// object owning exactly one reference from its own construction.
/// v0.1's grammar makes this purely syntactic: every str-producing
/// expression other than a bare `Name` (`StringLiteral`, string
/// concatenation, an f-string (Task 8), or a `Call`'s return value)
/// freshly constructs its result and already owns exactly one reference.
fn str_value_is_a_duplicate_reference(expr: &MirExpr) -> bool {
    matches!(expr, MirExpr::Name { .. })
}

fn incref_if_str_duplicate<'ctx>(
    builder: &inkwell::builder::Builder<'ctx>,
    rt: &RtFns<'ctx>,
    source_expr: &MirExpr,
    scalar: Scalar<'ctx>,
) -> Scalar<'ctx> {
    if let Scalar::Str(ptr) = scalar {
        if str_value_is_a_duplicate_reference(source_expr) {
            builder
                .build_call(rt.str_incref, &[ptr.into()], "str_incref")
                .expect("build_call should not fail for a well-formed incref");
        }
        Scalar::Str(ptr)
    } else {
        scalar
    }
}

/// Only meaningful for `Ty::Str` targets: if `target` already has a slot
/// in `locals` (this `Assign` is a reassignment, not a first binding),
/// loads its current value and decrefs it before the new value
/// overwrites it -- otherwise reassigning a `str` local in a loop would
/// leak its previous value every iteration.
fn decref_old_str_if_reassigning<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    rt: &RtFns<'ctx>,
    locals: &HashMap<String, (PointerValue<'ctx>, pycc_hir::Ty)>,
    target: &str,
) {
    if let Some((slot_ptr, pycc_hir::Ty::Str)) = locals.get(target) {
        let old = builder
            .build_load(context.ptr_type(inkwell::AddressSpace::default()), *slot_ptr, "old_str")
            .expect("build_load should not fail for this function's own alloca")
            .into_pointer_value();
        builder
            .build_call(rt.str_decref, &[old.into()], "str_decref_old")
            .expect("build_call should not fail for a well-formed decref");
    }
}
```

Update `emit_stmt`'s `MirStmt::Assign` arm (Task 3) to use both:

```rust
MirStmt::Assign { target, value } => {
    let ty = value.ty();
    let scalar = emit_expr(context, builder, module, rt, user_functions, locals, value);
    let scalar = incref_if_str_duplicate(builder, rt, value, scalar);
    if ty == pycc_hir::Ty::Str {
        decref_old_str_if_reassigning(context, builder, rt, locals, target);
    }
    emit_assign(builder, locals, target, ty, scalar);
    Ok(())
}
```

Update `emit_expr`'s `Call` arm's argument-marshaling (Task 5) to incref a duplicated `str` argument:

```rust
let arg_values: Vec<inkwell::values::BasicMetadataValueEnum> = args
    .iter()
    .map(|a| {
        let scalar = emit_expr(context, builder, module, rt, user_functions, locals, a);
        let scalar = incref_if_str_duplicate(builder, rt, a, scalar);
        match scalar {
            Scalar::Int(v) => v.into(),
            Scalar::Bool(v) => v.into(),
            Scalar::Float(v) => v.into(),
            Scalar::Str(v) => v.into(),
        }
    })
    .collect();
```

Update `emit_stmt`'s `MirStmt::Return` arm (Task 5) the same way:

```rust
MirStmt::Return(value) => {
    match value {
        Some(expr) => {
            let scalar = emit_expr(context, builder, module, rt, user_functions, locals, expr);
            let scalar = incref_if_str_duplicate(builder, rt, expr, scalar);
            let basic_value: inkwell::values::BasicValueEnum = match scalar {
                Scalar::Int(v) => v.into(),
                Scalar::Bool(v) => v.into(),
                Scalar::Float(v) => v.into(),
                Scalar::Str(v) => v.into(),
            };
            builder
                .build_return(Some(&basic_value))
                .expect("build_return should not fail for a well-formed return value");
        }
        None => {
            builder
                .build_return(None)
                .expect("build_return should not fail for a bare `return`");
        }
    }
    Ok(())
}
```

Finally, `compile_to_object`'s top-level statement loop decrefs every top-level `str` local once, right before `main`'s own final `build_return`: after the top-level loop, add

```rust
for (ptr, ty) in top_level_locals.values() {
    if *ty == pycc_hir::Ty::Str {
        let value = builder
            .build_load(context.ptr_type(inkwell::AddressSpace::default()), *ptr, "final_str")
            .expect("build_load should not fail for this function's own alloca")
            .into_pointer_value();
        builder
            .build_call(rt.str_decref, &[value.into()], "str_decref_final")
            .expect("build_call should not fail for a well-formed decref");
    }
}
```

immediately before the existing `builder.build_return(Some(&i64_type.const_int(0, false)))` call.

- [ ] **Step 9 run:** `cargo test -p pycc_codegen`
Expected: PASS.

### Step 10: Run the full workspace test suite

Run: `cargo test --workspace`
Expected: PASS.

### Step 11: Run clippy and the coverage gate

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo llvm-cov clean --workspace && cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`
Expected: PASS. `PyStrObj`'s `unsafe` blocks need every branch exercised (both `Inline`/`Heap` payload variants, both the "not yet zero" and "reaches zero" `decref` branches, the null-pointer no-op path) -- Step 2's tests already cover all of these; if coverage still reports a gap, add the missing fixture rather than an exemption.

### Step 12: Commit

```bash
git add crates/pycc_rt/src/lib.rs crates/pycc_codegen/src/lib.rs docs/DECISIONS.md
git commit -m "feat(pycc_rt,pycc_codegen): str runtime (SSO), literals, concatenation, comparisons (Task 7)"
```

---

## Task 8: f-string codegen

**Order note:** the original brainstorm scope put `print`'s full formatting in Task 10, after this task. F-string interpolation and `print` both need the exact same "format a scalar as a `str`" logic (`f"{n}"` and `print(n)` must produce identical text for the same `n`) -- building it twice, or building it in Task 10 and then discovering Task 8 already needed it, would either duplicate logic or invert this plan's own dependency order. This task therefore introduces the `int`/`float`/`bool` → `str` conversion primitives (per-type formatting rules, matching CPython's `str()`) that both f-strings *and* Task 10's `print` reuse unchanged.

**Files:**
- Modify: `crates/pycc_rt/src/lib.rs`
- Modify: `crates/pycc_codegen/src/lib.rs`

**Interfaces:**
- Consumes: `Scalar`, `RtFns`, `emit_expr`, `pycc_rt_str_concat` (Task 7).
- Produces: `pycc_rt_int_to_str(i64) -> *mut PyStrObj`, `pycc_rt_float_to_str(f64) -> *mut PyStrObj`, `pycc_rt_bool_to_str(i8) -> *mut PyStrObj` in `pycc_rt` (Task 9 extends `int_to_str`'s bigint handling; Task 10 reuses all three unchanged); a `MirExpr::FString` arm in `emit_expr`.

### Step 1: Write failing `pycc_rt` tests for the scalar-to-`str` conversions

Append to `crates/pycc_rt/src/lib.rs`'s tests:

```rust
#[test]
fn pycc_rt_int_to_str_formats_the_untagged_decimal_value() {
    let s = pycc_rt_int_to_str(tag_smallint(42));
    assert_eq!(unsafe { &*s }.bytes(), b"42");
    pycc_rt_str_decref(s);
    let s = pycc_rt_int_to_str(tag_smallint(-7));
    assert_eq!(unsafe { &*s }.bytes(), b"-7");
    pycc_rt_str_decref(s);
}

#[test]
fn pycc_rt_bool_to_str_matches_python_s_capitalized_spelling() {
    let s = pycc_rt_bool_to_str(1);
    assert_eq!(unsafe { &*s }.bytes(), b"True");
    pycc_rt_str_decref(s);
    let s = pycc_rt_bool_to_str(0);
    assert_eq!(unsafe { &*s }.bytes(), b"False");
    pycc_rt_str_decref(s);
}

#[test]
fn pycc_rt_float_to_str_always_shows_a_decimal_point() {
    // CPython: `str(3.0) == "3.0"`, not `"3"` -- unlike Rust's own `{}`
    // `Display` for `f64`, which omits the fractional part entirely for
    // a whole-number value.
    let s = pycc_rt_float_to_str(3.0);
    assert_eq!(unsafe { &*s }.bytes(), b"3.0");
    pycc_rt_str_decref(s);
    let s = pycc_rt_float_to_str(2.5);
    assert_eq!(unsafe { &*s }.bytes(), b"2.5");
    pycc_rt_str_decref(s);
    let s = pycc_rt_float_to_str(-0.5);
    assert_eq!(unsafe { &*s }.bytes(), b"-0.5");
    pycc_rt_str_decref(s);
}

#[test]
fn pycc_rt_float_to_str_handles_infinity_and_nan_like_cpython() {
    // CPython: `str(float('inf')) == "inf"`, `str(float('nan')) == "nan"`
    // -- lowercase, unlike Rust's own `{}` (`"inf"`/`"NaN"`, capitalized
    // for `NaN`).
    let s = pycc_rt_float_to_str(f64::INFINITY);
    assert_eq!(unsafe { &*s }.bytes(), b"inf");
    pycc_rt_str_decref(s);
    let s = pycc_rt_float_to_str(f64::NEG_INFINITY);
    assert_eq!(unsafe { &*s }.bytes(), b"-inf");
    pycc_rt_str_decref(s);
    let s = pycc_rt_float_to_str(f64::NAN);
    assert_eq!(unsafe { &*s }.bytes(), b"nan");
    pycc_rt_str_decref(s);
}

#[test]
#[should_panic(expected = "not supported yet")]
fn pycc_rt_float_to_str_rejects_magnitudes_needing_scientific_notation() {
    // CPython's `repr(float)` switches to scientific notation outside a
    // specific decimal-exponent range (verified against `python3.14`:
    // `repr(1e17)` is `'1e+17'`, not the full 18-digit expansion) --
    // reproducing that exact algorithm is out of scope for this task;
    // this is an honest, loud "not supported yet" for that narrow range
    // (never a silently wrong digit string), not silently accepted.
    pycc_rt_float_to_str(1e17);
}

#[test]
fn pycc_rt_float_to_str_accepts_the_boundary_just_inside_the_supported_range() {
    let s = pycc_rt_float_to_str(1e16);
    assert_eq!(unsafe { &*s }.bytes(), b"10000000000000000.0");
    pycc_rt_str_decref(s);
}
```

- [ ] **Step 1 run:** `cargo test -p pycc_rt`
Expected: FAIL to compile.

### Step 2: Implement the conversions in `pycc_rt`

```rust
#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_int_to_str(tagged: i64) -> *mut PyStrObj {
    require_smallint(tagged, "formatting");
    new_pystr(format_i64_line(untag_smallint(tagged)).trim_end().as_bytes())
}

#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_bool_to_str(value: i8) -> *mut PyStrObj {
    new_pystr(if value != 0 { b"True" } else { b"False" })
}

/// Verified against `python3.14`'s actual `repr(float)`/`str(float)`
/// (identical since Python 3.1): CPython switches to scientific
/// notation once the value's magnitude is `>= 1e16` or (nonzero and)
/// `< 1e-4`; within that range it always shows at least one digit after
/// the decimal point (`3.0`, never bare `3`, unlike Rust's own `{}`
/// `Display` for `f64`), and `inf`/`-inf`/`nan` are lowercase (Rust's
/// own `Display` capitalizes `NaN`). Reproducing CPython's scientific
/// notation formatting exactly is out of scope for this task -- an
/// honest panic for that narrow range, not a silently wrong digit
/// string (a documented, named gap, same convention as D-026/D-043).
#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_float_to_str(value: f64) -> *mut PyStrObj {
    if value.is_nan() {
        return new_pystr(b"nan");
    }
    if value.is_infinite() {
        return new_pystr(if value > 0.0 { b"inf" } else { b"-inf" });
    }
    let magnitude = value.abs();
    if magnitude != 0.0 && (magnitude >= 1e16 || magnitude < 1e-4) {
        panic!(
            "pycc_rt: formatting a float this large or small ({value}) needs \
             scientific notation, which is not supported yet"
        );
    }
    let text = format!("{value}");
    let text = if text.contains('.') { text } else { format!("{text}.0") };
    new_pystr(text.as_bytes())
}
```

- [ ] **Step 2 run:** `cargo test -p pycc_rt`
Expected: PASS.

### Step 3: Write failing `pycc_codegen` tests for f-string interpolation

Append to `crates/pycc_codegen/src/lib.rs`'s tests:

```rust
#[test]
fn compiles_an_f_string_interpolating_an_int_between_literal_parts() {
    // `x = 5; print(f"n={x}!")` -- prints `n=5!`.
    let mir = MirModule {
        items: vec![
            MirItem::TopLevelStmt(MirStmt::Assign {
                target: "x".to_string(),
                value: MirExpr::IntLiteral(5),
            }),
            MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                callee: "print".to_string(),
                args: vec![MirExpr::FString(vec![
                    pycc_mir::MirFStringPart::Literal("n=".to_string()),
                    pycc_mir::MirFStringPart::Interpolation(Box::new(MirExpr::Name {
                        name: "x".to_string(),
                        ty: pycc_hir::Ty::Int,
                    })),
                    pycc_mir::MirFStringPart::Literal("!".to_string()),
                ])],
                ty: pycc_hir::Ty::None,
            })),
        ],
    };
    let dir = tempfile_dir("fstring_int");
    let obj_path = dir.join("fstring_int.o");
    compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
}

#[test]
fn compiles_an_f_string_interpolating_a_float_and_a_bool() {
    let mir = MirModule {
        items: vec![MirItem::TopLevelStmt(MirStmt::Assign {
            target: "s".to_string(),
            value: MirExpr::FString(vec![
                pycc_mir::MirFStringPart::Interpolation(Box::new(MirExpr::FloatLiteral(2.5))),
                pycc_mir::MirFStringPart::Literal(" ".to_string()),
                pycc_mir::MirFStringPart::Interpolation(Box::new(MirExpr::BoolLiteral(true))),
            ]),
        })],
    };
    let dir = tempfile_dir("fstring_float_bool");
    let obj_path = dir.join("fstring_float_bool.o");
    compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
}

#[test]
fn compiles_an_f_string_with_only_literal_parts() {
    let mir = MirModule {
        items: vec![MirItem::TopLevelStmt(MirStmt::Assign {
            target: "s".to_string(),
            value: MirExpr::FString(vec![pycc_mir::MirFStringPart::Literal(
                "no interpolation".to_string(),
            )]),
        })],
    };
    let dir = tempfile_dir("fstring_literal_only");
    let obj_path = dir.join("fstring_literal_only.o");
    compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
}
```

- [ ] **Step 3 run:** `cargo test -p pycc_codegen`
Expected: FAIL (`MirExpr::FString` still hits `emit_expr`'s catch-all).

### Step 4: Extend `RtFns` and implement the `FString` arm

Add `int_to_str`, `float_to_str`, `bool_to_str` fields to `RtFns`:

```rust
// RtFns gains:
int_to_str: FunctionValue<'ctx>,
float_to_str: FunctionValue<'ctx>,
bool_to_str: FunctionValue<'ctx>,

// declare_rt_functions gains (inside the `RtFns { ... }` literal):
int_to_str: declare("pycc_rt_int_to_str", ptr_type.fn_type(&[i64_type.into()], false)),
float_to_str: declare("pycc_rt_float_to_str", ptr_type.fn_type(&[f64_type.into()], false)),
bool_to_str: declare("pycc_rt_bool_to_str", ptr_type.fn_type(&[context.i8_type().into()], false)),
```

Add `MirExpr::FString` to `emit_expr`:

```rust
MirExpr::FString(parts) => {
    let mut acc: Option<PointerValue> = None;
    for part in parts {
        let part_str = match part {
            pycc_mir::MirFStringPart::Literal(s) => {
                let Scalar::Str(ptr) = emit_expr(
                    context,
                    builder,
                    module,
                    rt,
                    user_functions,
                    locals,
                    &MirExpr::StringLiteral(s.clone()),
                ) else {
                    unreachable!("StringLiteral always evaluates to Scalar::Str")
                };
                ptr
            }
            pycc_mir::MirFStringPart::Interpolation(inner) => {
                let scalar = emit_expr(context, builder, module, rt, user_functions, locals, inner);
                let scalar = incref_if_str_duplicate(builder, rt, inner, scalar);
                to_str(context, builder, rt, scalar)
            }
        };
        acc = Some(match acc {
            None => part_str,
            Some(prev) => {
                let joined = builder
                    .build_call(rt.str_concat, &[prev.into(), part_str.into()], "fstring_concat")
                    .expect("build_call should not fail for a well-formed concatenation")
                    .try_as_basic_value()
                    .left()
                    .expect("pycc_rt_str_concat returns a non-void pointer")
                    .into_pointer_value();
                builder
                    .build_call(rt.str_decref, &[prev.into()], "fstring_decref_prev")
                    .expect("build_call should not fail for a well-formed decref");
                builder
                    .build_call(rt.str_decref, &[part_str.into()], "fstring_decref_part")
                    .expect("build_call should not fail for a well-formed decref");
                joined
            }
        });
    }
    Scalar::Str(acc.unwrap_or_else(|| {
        // An empty `FString(vec![])` never actually reaches this arm --
        // `pycc_hir`'s own f-string lowering always produces at least
        // one `Literal` part (an empty Python f-string `f""` still
        // lowers to `FString(vec![FStringPart::Literal("")])`, never a
        // truly empty `Vec`) -- but guard it explicitly rather than
        // silently returning a dangling/null pointer if that assumption
        // is ever wrong.
        panic!("pycc_codegen: internal error: an f-string with zero parts should not be reachable")
    }))
}
```

Add the `to_str` coercion helper (dispatches any `Scalar` to a fresh, owned `Scalar::Str`/`PointerValue` -- reused unchanged by Task 10):

```rust
/// Converts any scalar to a fresh, owned `str` object matching CPython's
/// `str(x)` for that value -- reused unchanged by Task 10's `print`.
/// `str` itself passes through (already a `str`); every other type goes
/// through its own `pycc_rt_*_to_str` conversion.
fn to_str<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    rt: &RtFns<'ctx>,
    scalar: Scalar<'ctx>,
) -> PointerValue<'ctx> {
    let (rt_fn, arg): (FunctionValue<'ctx>, inkwell::values::BasicMetadataValueEnum) = match scalar {
        Scalar::Str(v) => return v,
        Scalar::Int(v) => (rt.int_to_str, v.into()),
        Scalar::Float(v) => (rt.float_to_str, v.into()),
        Scalar::Bool(v) => (rt.bool_to_str, v.into()),
    };
    builder
        .build_call(rt_fn, &[arg], "to_str")
        .expect("build_call should not fail for a well-formed conversion")
        .try_as_basic_value()
        .left()
        .expect("every pycc_rt_*_to_str function returns a non-void pointer")
        .into_pointer_value()
}
```

`emit_expr`'s `FString` arm's own result is always a fresh, owned object built entirely from `str_concat` calls (or a single fresh conversion, for a one-part f-string) -- it never needs its own incref, matching every other str-producing expression (`StringLiteral`, `BinOp` concatenation) already established in Task 7.

- [ ] **Step 4 run:** `cargo test -p pycc_codegen`
Expected: PASS.

### Step 5: Run the full workspace test suite

Run: `cargo test --workspace`
Expected: PASS.

### Step 6: Run clippy and the coverage gate

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo llvm-cov clean --workspace && cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`
Expected: PASS.

### Step 7: Commit

```bash
git add crates/pycc_rt/src/lib.rs crates/pycc_codegen/src/lib.rs
git commit -m "feat(pycc_rt,pycc_codegen): f-string codegen and scalar-to-str conversions (Task 8)"
```

---

## Task 9: `int` overflow-to-bigint runtime

**Scope note:** per D-052's own design, this task touches only `pycc_rt` function *bodies* -- every `pycc_rt_int_*` signature is already fixed (Task 3), and `pycc_codegen` needs zero changes: `pycc_rt_int_add`/`pycc_rt_int_sub`/`pycc_rt_int_to_str`/`pycc_rt_int_print` gain real bigint handling; `pycc_rt_int_mul`/`floordiv`/`floormod`/`pow` keep their existing `require_smallint`-triggered panic for a bigint operand (multiplication/division/power on an already-promoted value is a documented, out-of-scope gap for v0.1 -- D-049 itself scopes the bigint mechanism to "overflow-safe arithmetic + `print`, not a general-purpose bignum API surface," and `pycc_rt_int_cmp` keeps D-052's already-recorded bigint-comparison gap unchanged). This keeps the scope to exactly what a realistic `fib`-style program (Task 11) needs: unbounded addition, with everything else either already correct (the fast path, for values that never overflow) or an honest, named "not supported yet."

**Files:**
- Modify: `crates/pycc_rt/src/lib.rs`

**Interfaces:**
- Consumes: `tag_smallint`/`untag_smallint`/`is_smallint`/`fits_smallint` (Task 3).
- Produces: a private `BigIntObj` type (sign-magnitude `Vec<u32>` limbs, D-049) and `tag_bigint`/`decode_int` helpers, internal to `pycc_rt`; real bigint-aware bodies for `pycc_rt_int_add`, `pycc_rt_int_sub`, `pycc_rt_int_to_str`, `pycc_rt_int_print`, `pycc_rt_int_truthy` -- no new `pub extern "C"` functions, no `pycc_codegen` changes.

### Step 1: Write failing `pycc_rt` tests for bigint promotion, addition, and formatting

Append to `crates/pycc_rt/src/lib.rs`'s tests:

```rust
#[test]
fn adding_past_i64_range_now_promotes_instead_of_panicking() {
    // This is the exact fixture Task 3's
    // `pycc_rt_int_add_panics_on_overflow_before_bigint_promotion_exists`
    // used to require a panic for -- it must now succeed and print the
    // exact mathematical sum.
    let huge = pycc_rt_int_add(tag_smallint(i64::MAX >> 1), tag_smallint(1));
    let s = pycc_rt_int_to_str(huge);
    assert_eq!(unsafe { &*s }.bytes(), b"4611686018427387904");
    pycc_rt_str_decref(s);
}

#[test]
fn subtracting_past_the_negative_range_promotes_correctly() {
    let huge = pycc_rt_int_sub(tag_smallint(i64::MIN >> 1), tag_smallint(1));
    let s = pycc_rt_int_to_str(huge);
    assert_eq!(unsafe { &*s }.bytes(), b"-4611686018427387905");
    pycc_rt_str_decref(s);
}

#[test]
fn repeated_addition_exercises_the_general_bigint_plus_smallint_path() {
    // Simulates unbounded `fib`-style growth. The first `pycc_rt_int_add`
    // call overflows the tagged range (`i64::MAX >> 1` is exactly the
    // largest value that still round-trips through tagging, so adding
    // it to itself does not fit) and promotes via the one-time
    // `i128`-widening path. Every one of the 20 subsequent additions
    // then has a `BigIntObj` as its left operand and a tagged smallint
    // as its right operand -- `is_smallint(a) && is_smallint(b)` is
    // false for all of them, so they all go through
    // `to_sign_and_magnitude`/`bigint_add_signed`'s general limb
    // arithmetic, not the fast path or the one-time promotion shortcut.
    // (The running total here stays well under 128 bits -- about 67 at
    // the end -- this test is not about exceeding `i128`'s own range,
    // only about exercising the general bigint-arithmetic code path
    // repeatedly rather than just once.)
    let mut acc = pycc_rt_int_add(tag_smallint(i64::MAX >> 1), tag_smallint(i64::MAX >> 1));
    for _ in 0..20 {
        acc = pycc_rt_int_add(acc, tag_smallint(i64::MAX >> 1));
    }
    assert!(!is_smallint(acc));
    let s = pycc_rt_int_to_str(acc);
    let text = String::from_utf8(unsafe { &*s }.bytes().to_vec()).unwrap();
    assert_eq!(text, (bigint_reference_sum()).to_string());
    pycc_rt_str_decref(s);
}

/// Independent reference computation for the test above (22 additions
/// of `i64::MAX >> 1`, well within `i128`'s own range) -- this doesn't
/// exercise `pycc_rt`'s bigint code at all, so it's a trustworthy oracle
/// for what the *correct* sum is, independent of any bug the code under
/// test might have.
fn bigint_reference_sum() -> i128 {
    let step = (i64::MAX >> 1) as i128;
    let mut acc = step + step;
    for _ in 0..20 {
        acc += step;
    }
    acc
}

#[test]
fn a_bigint_that_would_fit_back_in_smallint_range_still_formats_correctly() {
    // Two already-promoted values that sum back to something small
    // (mathematically representable as a smallint) are not required to
    // shrink back down (D-052/this task's own "simplest correct" choice
    // -- once a value touches the bigint path, it stays represented as
    // one) -- but the printed *value* must still be exactly right.
    let a = pycc_rt_int_add(tag_smallint(i64::MAX >> 1), tag_smallint(1)); // a bigint
    let b = pycc_rt_int_sub(tag_smallint(0), a); // -a, also a bigint (sub promotes too)
    let zero = pycc_rt_int_add(a, b);
    let s = pycc_rt_int_to_str(zero);
    assert_eq!(unsafe { &*s }.bytes(), b"0");
    pycc_rt_str_decref(s);
}

#[test]
fn a_bigint_zero_is_falsy() {
    let a = pycc_rt_int_add(tag_smallint(i64::MAX >> 1), tag_smallint(1));
    let b = pycc_rt_int_sub(tag_smallint(0), a);
    let zero = pycc_rt_int_add(a, b);
    assert_eq!(pycc_rt_int_truthy(zero), 0);
}

#[test]
fn a_bigint_prints_with_a_trailing_newline_like_a_smallint() {
    let huge = pycc_rt_int_add(tag_smallint(i64::MAX >> 1), tag_smallint(1));
    pycc_rt_int_print(huge); // stdout captured by the test harness
}
```

- [ ] **Step 1 run:** `cargo test -p pycc_rt`
Expected: FAIL (`adding_past_i64_range_now_promotes_instead_of_panicking` and its neighbors fail -- the current body still panics on overflow; `Task 3`'s own `pycc_rt_int_add_panics_on_overflow_before_bigint_promotion_exists` test, still present, now needs removing since that exact behavior is what this task replaces -- delete it as part of Step 2's implementation, not before, so the removal is reviewable in the same diff as the behavior change).

### Step 2: Implement the bigint type and promote `int_add`/`int_sub`/`int_to_str`/`int_print`/`int_truthy`

Remove Task 3's `pycc_rt_int_add_panics_on_overflow_before_bigint_promotion_exists` test (it asserted exactly the behavior this task replaces).

Add the bigint type and its arithmetic helpers to `crates/pycc_rt/src/lib.rs`:

```rust
/// D-049: hand-rolled sign-magnitude limbs, base 2^32, little-endian,
/// no trailing zero limbs except a single `[0]` representing zero
/// itself. Never freed (leaked) -- unlike `PyStrObj`, D-051 only commits
/// `str` to real refcounting; a bigint is a rare, overflow-only path
/// with no v0.1 construct that could leak it in a hot loop the way an
/// unbounded string-building loop could (this is a deliberate, narrower
/// "simplest safe default" than `str`'s, recorded alongside D-052).
struct BigIntObj {
    negative: bool,
    limbs: Vec<u32>,
}

fn trim(limbs: &[u32]) -> Vec<u32> {
    let mut end = limbs.len();
    while end > 1 && limbs[end - 1] == 0 {
        end -= 1;
    }
    limbs[..end].to_vec()
}

fn magnitude_cmp(a: &[u32], b: &[u32]) -> std::cmp::Ordering {
    let (a, b) = (trim(a), trim(b));
    if a.len() != b.len() {
        return a.len().cmp(&b.len());
    }
    for i in (0..a.len()).rev() {
        if a[i] != b[i] {
            return a[i].cmp(&b[i]);
        }
    }
    std::cmp::Ordering::Equal
}

fn magnitude_add(a: &[u32], b: &[u32]) -> Vec<u32> {
    let mut result = Vec::with_capacity(a.len().max(b.len()) + 1);
    let mut carry: u64 = 0;
    for i in 0..a.len().max(b.len()) {
        let av = *a.get(i).unwrap_or(&0) as u64;
        let bv = *b.get(i).unwrap_or(&0) as u64;
        let sum = av + bv + carry;
        result.push((sum & 0xFFFF_FFFF) as u32);
        carry = sum >> 32;
    }
    if carry > 0 {
        result.push(carry as u32);
    }
    result
}

/// Requires `a >= b` (checked by every caller via `magnitude_cmp` first).
fn magnitude_sub(a: &[u32], b: &[u32]) -> Vec<u32> {
    let mut result = Vec::with_capacity(a.len());
    let mut borrow: i64 = 0;
    for i in 0..a.len() {
        let av = a[i] as i64;
        let bv = *b.get(i).unwrap_or(&0) as i64;
        let mut diff = av - bv - borrow;
        if diff < 0 {
            diff += 1i64 << 32;
            borrow = 1;
        } else {
            borrow = 0;
        }
        result.push(diff as u32);
    }
    result
}

fn bigint_add_signed(a_neg: bool, a_mag: &[u32], b_neg: bool, b_mag: &[u32]) -> BigIntObj {
    if a_neg == b_neg {
        BigIntObj { negative: a_neg, limbs: trim(&magnitude_add(a_mag, b_mag)) }
    } else {
        match magnitude_cmp(a_mag, b_mag) {
            std::cmp::Ordering::Equal => BigIntObj { negative: false, limbs: vec![0] },
            std::cmp::Ordering::Greater => {
                BigIntObj { negative: a_neg, limbs: trim(&magnitude_sub(a_mag, b_mag)) }
            }
            std::cmp::Ordering::Less => {
                BigIntObj { negative: b_neg, limbs: trim(&magnitude_sub(b_mag, a_mag)) }
            }
        }
    }
}

fn bigint_from_i128(v: i128) -> BigIntObj {
    let negative = v < 0;
    let mut mag = v.unsigned_abs();
    let mut limbs = Vec::new();
    while mag > 0 {
        limbs.push((mag & 0xFFFF_FFFF) as u32);
        mag >>= 32;
    }
    if limbs.is_empty() {
        limbs.push(0);
    }
    BigIntObj { negative, limbs }
}

fn tag_bigint(b: BigIntObj) -> i64 {
    Box::into_raw(Box::new(b)) as i64
}

/// # Safety
/// `tagged` must be a `BigIntObj` pointer (an even bit pattern -- D-052);
/// every call site below checks `!is_smallint(tagged)` first.
unsafe fn bigint_ref<'a>(tagged: i64) -> &'a BigIntObj {
    unsafe { &*(tagged as *const BigIntObj) }
}

fn to_sign_and_magnitude(tagged: i64) -> (bool, Vec<u32>) {
    if is_smallint(tagged) {
        let v = untag_smallint(tagged);
        let negative = v < 0;
        let mag = v.unsigned_abs();
        (negative, trim(&[(mag & 0xFFFF_FFFF) as u32, (mag >> 32) as u32]))
    } else {
        let b = unsafe { bigint_ref(tagged) };
        (b.negative, b.limbs.clone())
    }
}

fn divmod_small(limbs: &[u32], divisor: u32) -> (Vec<u32>, u32) {
    let mut quotient = vec![0u32; limbs.len()];
    let mut remainder: u64 = 0;
    for i in (0..limbs.len()).rev() {
        let acc = (remainder << 32) | limbs[i] as u64;
        quotient[i] = (acc / divisor as u64) as u32;
        remainder = acc % divisor as u64;
    }
    (quotient, remainder as u32)
}

fn bigint_to_decimal_string(negative: bool, limbs: &[u32]) -> String {
    let mut limbs = limbs.to_vec();
    let mut digits = Vec::new();
    loop {
        let (q, r) = divmod_small(&limbs, 10);
        digits.push(std::char::from_digit(r, 10).expect("a remainder of division by 10 is always 0-9"));
        limbs = trim(&q);
        if limbs.len() == 1 && limbs[0] == 0 {
            break;
        }
    }
    if negative {
        digits.push('-');
    }
    digits.iter().rev().collect()
}
```

Replace `pycc_rt_int_add`'s body:

```rust
#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_int_add(a: i64, b: i64) -> i64 {
    if is_smallint(a) && is_smallint(b) {
        if let Some(result) = untag_smallint(a).checked_add(untag_smallint(b)).and_then(fits_smallint) {
            return result;
        }
        // Both operands fit 63 bits, so their true sum always fits i128
        // with room to spare -- exact, no further bigint math needed
        // for this specific promotion step.
        return tag_bigint(bigint_from_i128(untag_smallint(a) as i128 + untag_smallint(b) as i128));
    }
    let (a_neg, a_mag) = to_sign_and_magnitude(a);
    let (b_neg, b_mag) = to_sign_and_magnitude(b);
    tag_bigint(bigint_add_signed(a_neg, &a_mag, b_neg, &b_mag))
}
```

Replace `pycc_rt_int_sub`'s body (reuses the same `bigint_add_signed`: `a - b == a + (-b)`):

```rust
#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_int_sub(a: i64, b: i64) -> i64 {
    if is_smallint(a) && is_smallint(b) {
        if let Some(result) = untag_smallint(a).checked_sub(untag_smallint(b)).and_then(fits_smallint) {
            return result;
        }
        return tag_bigint(bigint_from_i128(untag_smallint(a) as i128 - untag_smallint(b) as i128));
    }
    let (a_neg, a_mag) = to_sign_and_magnitude(a);
    let (b_neg, b_mag) = to_sign_and_magnitude(b);
    tag_bigint(bigint_add_signed(a_neg, &a_mag, !b_neg, &b_mag))
}
```

Replace `pycc_rt_int_to_str`'s body:

```rust
#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_int_to_str(tagged: i64) -> *mut PyStrObj {
    if is_smallint(tagged) {
        return new_pystr(format_i64_line(untag_smallint(tagged)).trim_end().as_bytes());
    }
    let b = unsafe { bigint_ref(tagged) };
    new_pystr(bigint_to_decimal_string(b.negative, &b.limbs).as_bytes())
}
```

Replace `pycc_rt_int_print`'s body:

```rust
#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_int_print(tagged: i64) {
    if is_smallint(tagged) {
        pycc_rt_print_i64(untag_smallint(tagged));
        return;
    }
    let s = pycc_rt_int_to_str(tagged);
    println!("{}", String::from_utf8_lossy(unsafe { &*s }.bytes()));
    pycc_rt_str_decref(s);
}
```

Replace `pycc_rt_int_truthy`'s body (a bigint can now legitimately be zero -- `bigint_add_signed`'s "equal magnitude, opposite sign" case -- so the old "any bigint tag is truthy" shortcut from Task 4 is no longer correct):

```rust
#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_int_truthy(tagged: i64) -> i8 {
    if is_smallint(tagged) {
        return i8::from(untag_smallint(tagged) != 0);
    }
    let b = unsafe { bigint_ref(tagged) };
    i8::from(!(b.limbs.len() == 1 && b.limbs[0] == 0))
}
```

`pycc_rt_int_mul`/`pycc_rt_int_floordiv`/`pycc_rt_int_floormod`/`pycc_rt_int_pow`/`pycc_rt_int_cmp` are **unchanged** -- their existing `require_smallint(...)` calls already reject a bigint operand with a clear panic, which is exactly this task's intended scope boundary, not an oversight.

- [ ] **Step 2 run:** `cargo test -p pycc_rt`
Expected: PASS.

### Step 3: Write a failing `pycc_codegen`/e2e-style test proving unbounded addition works through real compiled code

Append to `crates/pycc_codegen/src/lib.rs`'s tests:

```rust
#[test]
fn compiles_a_loop_whose_accumulator_overflows_into_a_bigint() {
    // `i = 0; acc = 4611686018427387903; while i < 3: acc = acc + acc; i = i + 1`
    // `print(acc)` -- starts at `i64::MAX >> 1` and doubles 3 times,
    // overflowing well past `i64::MAX` partway through; must print the
    // exact mathematical result via real bigint arithmetic, not a
    // wrapped/truncated one.
    let start: i64 = i64::MAX >> 1;
    let expected = (start as i128) * 8; // doubled 3 times
    let mir = MirModule {
        items: vec![
            MirItem::TopLevelStmt(MirStmt::Assign {
                target: "acc".to_string(),
                value: MirExpr::IntLiteral(start),
            }),
            MirItem::TopLevelStmt(MirStmt::ForRange {
                var: "i".to_string(),
                start: MirExpr::IntLiteral(0),
                stop: MirExpr::IntLiteral(3),
                step: MirExpr::IntLiteral(1),
                body: vec![MirStmt::Assign {
                    target: "acc".to_string(),
                    value: MirExpr::BinOp {
                        op: pycc_hir::BinOpKind::Add,
                        left: Box::new(MirExpr::Name { name: "acc".to_string(), ty: pycc_hir::Ty::Int }),
                        right: Box::new(MirExpr::Name { name: "acc".to_string(), ty: pycc_hir::Ty::Int }),
                        ty: pycc_hir::Ty::Int,
                    },
                }],
            }),
            MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                callee: "print".to_string(),
                args: vec![MirExpr::Name { name: "acc".to_string(), ty: pycc_hir::Ty::Int }],
                ty: pycc_hir::Ty::None,
            })),
        ],
    };
    let dir = tempfile_dir("bigint_overflow_loop");
    let obj_path = dir.join("bigint_overflow_loop.o");
    compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
    let bin_path = dir.join("bigint_overflow_loop");
    link_object_with_runtime(&obj_path, &bin_path);
    let output = Command::new(&bin_path).output().expect("binary should run");
    assert_eq!(output.stdout, format!("{expected}\n").into_bytes());
}
```

- [ ] **Step 3 run:** `cargo test -p pycc_codegen`
Expected: PASS already (this task changes only `pycc_rt`, which the already-existing `int_add`/`int_print` codegen calls unchanged) -- this step is a *proof*, not a new implementation step; if it fails, the bug is in Step 2's `pycc_rt` changes, not in codegen.

### Step 4: Run the full workspace test suite

Run: `cargo test --workspace`
Expected: PASS.

### Step 5: Run clippy and the coverage gate

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo llvm-cov clean --workspace && cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`
Expected: PASS.

### Step 6: Commit

```bash
git add crates/pycc_rt/src/lib.rs crates/pycc_codegen/src/lib.rs
git commit -m "feat(pycc_rt): int overflow-to-bigint promotion (Task 9, D-049)"
```

---

## Task 10: `print`'s full type-aware, multi-argument formatting

**Files:**
- Modify: `crates/pycc_rt/src/lib.rs`
- Modify: `crates/pycc_codegen/src/lib.rs`

**Interfaces:**
- Consumes: `to_str`, `incref_if_str_duplicate`, `pycc_rt_int_to_str`/`float_to_str`/`bool_to_str` (Task 8), `pycc_rt_str_decref` (Task 7).
- Produces: `pycc_rt_print_write_str(*mut PyStrObj)` (writes bytes, no newline -- distinct from Task 3's `pycc_rt_int_print`/newline-inclusive helper, which stays as-is and unused by this task's new dispatch), `pycc_rt_print_space()`, `pycc_rt_print_newline()`, `pycc_rt_print_none()` in `pycc_rt`; `emit_stmt`'s print-handling arm becomes fully general: any number of arguments (including zero), any of `int`/`float`/`bool`/`str`, space-separated, one trailing newline, matching CPython's `print(*args)` (no `sep`/`end` keyword support -- `pycc_hir` never lowers keyword call arguments at all, see its `a_keyword_call_argument_panics_instead_of_being_erased` test, so this is not a reachable gap).

**Scope note on `None`:** per Task 6's finding, a `Ty::None`-typed *value* is only ever reachable as a `Call`'s own result (e.g. `print(some_void_function())`) -- there is no `HirExpr`/`MirExpr::NoneLiteral`, and a `Name` bound to a `None`-typed variable (`x = some_void_function()` then `print(x)`) is legal Python but stays an explicit, narrow "not supported yet" here: `emit_expr`'s `Name` arm has no `Ty::None` case (and this task does not add one), so only the direct, common shape `print(f(...))` for a `None`-returning `f` is handled.

### Step 1: Write failing `pycc_rt` tests for the new print primitives

Append to `crates/pycc_rt/src/lib.rs`'s tests:

```rust
#[test]
fn print_write_str_writes_bytes_with_no_trailing_newline() {
    // stdout is captured by the test harness; this only proves the call
    // itself doesn't panic/crash (same rationale as this file's other
    // direct extern-fn exercises).
    let s = pycc_rt_str_from_literal(b"hi".as_ptr(), 2);
    pycc_rt_print_write_str(s);
    pycc_rt_str_decref(s);
}

#[test]
fn print_space_and_newline_and_none_do_not_panic() {
    pycc_rt_print_space();
    pycc_rt_print_newline();
    pycc_rt_print_none();
}
```

- [ ] **Step 1 run:** `cargo test -p pycc_rt`
Expected: FAIL to compile.

### Step 2: Implement the print primitives

```rust
#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_print_write_str(s: *mut PyStrObj) {
    print!("{}", String::from_utf8_lossy(unsafe { &*s }.bytes()));
}

#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_print_space() {
    print!(" ");
}

#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_print_newline() {
    println!();
}

#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_print_none() {
    print!("None");
}
```

- [ ] **Step 2 run:** `cargo test -p pycc_rt`
Expected: PASS.

### Step 3: Write failing `pycc_codegen` tests for multi-argument and mixed-type `print`

Append to `crates/pycc_codegen/src/lib.rs`'s tests:

```rust
#[test]
fn compiles_a_zero_argument_print_producing_just_a_newline() {
    let mir = MirModule {
        items: vec![MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
            callee: "print".to_string(),
            args: vec![],
            ty: pycc_hir::Ty::None,
        }))],
    };
    let dir = tempfile_dir("print_zero_args");
    let obj_path = dir.join("print_zero_args.o");
    compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
    let bin_path = dir.join("print_zero_args");
    link_object_with_runtime(&obj_path, &bin_path);
    let output = Command::new(&bin_path).output().expect("binary should run");
    assert_eq!(output.stdout, b"\n");
}

#[test]
fn compiles_a_multi_argument_print_with_mixed_types_space_separated() {
    // `print(1, 2.5, True, "hi")` -- prints `1 2.5 True hi\n`.
    let mir = MirModule {
        items: vec![MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
            callee: "print".to_string(),
            args: vec![
                MirExpr::IntLiteral(1),
                MirExpr::FloatLiteral(2.5),
                MirExpr::BoolLiteral(true),
                MirExpr::StringLiteral("hi".to_string()),
            ],
            ty: pycc_hir::Ty::None,
        }))],
    };
    let dir = tempfile_dir("print_mixed_multi");
    let obj_path = dir.join("print_mixed_multi.o");
    compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
    let bin_path = dir.join("print_mixed_multi");
    link_object_with_runtime(&obj_path, &bin_path);
    let output = Command::new(&bin_path).output().expect("binary should run");
    assert_eq!(output.stdout, b"1 2.5 True hi\n");
}

#[test]
fn compiles_print_of_a_bool_false() {
    let mir = MirModule {
        items: vec![MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
            callee: "print".to_string(),
            args: vec![MirExpr::BoolLiteral(false)],
            ty: pycc_hir::Ty::None,
        }))],
    };
    let dir = tempfile_dir("print_false");
    let obj_path = dir.join("print_false.o");
    compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
    let bin_path = dir.join("print_false");
    link_object_with_runtime(&obj_path, &bin_path);
    let output = Command::new(&bin_path).output().expect("binary should run");
    assert_eq!(output.stdout, b"False\n");
}

#[test]
fn compiles_print_of_a_void_returning_call_as_none() {
    // `def f() -> None: return` ; `print(f())` -- prints `None`.
    let mir = MirModule {
        items: vec![
            MirItem::Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: pycc_hir::Ty::None,
                body: vec![MirStmt::Return(None)],
            },
            MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                callee: "print".to_string(),
                args: vec![MirExpr::Call {
                    callee: "f".to_string(),
                    args: vec![],
                    ty: pycc_hir::Ty::None,
                }],
                ty: pycc_hir::Ty::None,
            })),
        ],
    };
    let dir = tempfile_dir("print_none_from_call");
    let obj_path = dir.join("print_none_from_call.o");
    compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
    let bin_path = dir.join("print_none_from_call");
    link_object_with_runtime(&obj_path, &bin_path);
    let output = Command::new(&bin_path).output().expect("binary should run");
    assert_eq!(output.stdout, b"None\n");
}
```

- [ ] **Step 3 run:** `cargo test -p pycc_codegen`
Expected: FAIL (`emit_stmt`'s print arm still only accepts exactly one `int`-typed argument).

### Step 4: Add the `Ty::None` call-result placeholder and replace `emit_stmt`'s print arm

`emit_expr`'s `Call` arm (Task 5) gains a `pycc_hir::Ty::None` case in its call-result `match ty { ... }`:

```rust
pycc_hir::Ty::None => {
    // A `None`-returning call's LLVM function returns `void` -- there is
    // no value to extract, and this crate has no `Scalar::None` (Task
    // 6's finding: no v0.1 expression can construct a `None` *value*
    // other than this exact call-result shape). The only caller that
    // ever evaluates a `Ty::None`-typed expression is `print`'s
    // dispatch below, which discards this placeholder and prints the
    // literal `"None"` instead of using it.
    Scalar::Bool(context.i8_type().const_int(0, false))
}
```

Replace `emit_stmt`'s `MirStmt::ExprStmt(MirExpr::Call { callee, args, .. }) if callee == "print"` arm:

```rust
MirStmt::ExprStmt(MirExpr::Call { callee, args, .. }) if callee == "print" => {
    for (i, arg) in args.iter().enumerate() {
        if i > 0 {
            builder
                .build_call(rt.print_space, &[], "print_sep")
                .expect("build_call should not fail for a well-formed print separator");
        }
        if arg.ty() == pycc_hir::Ty::None {
            let MirExpr::Call { .. } = arg else {
                panic!(
                    "pycc_codegen: printing a `None`-typed value that isn't a direct \
                     call result is not supported yet"
                );
            };
            emit_expr(context, builder, module, rt, user_functions, locals, arg); // evaluate for side effects
            builder
                .build_call(rt.print_none, &[], "print_none")
                .expect("build_call should not fail for a well-formed print of None");
        } else {
            let scalar = emit_expr(context, builder, module, rt, user_functions, locals, arg);
            let scalar = incref_if_str_duplicate(builder, rt, arg, scalar);
            let str_ptr = to_str(context, builder, rt, scalar);
            builder
                .build_call(rt.print_write_str, &[str_ptr.into()], "print_write")
                .expect("build_call should not fail for a well-formed print write");
            builder
                .build_call(rt.str_decref, &[str_ptr.into()], "print_decref_temp")
                .expect("build_call should not fail for a well-formed decref");
        }
    }
    builder
        .build_call(rt.print_newline, &[], "print_end")
        .expect("build_call should not fail for a well-formed print newline");
    Ok(())
}
```

Extend `RtFns`/`declare_rt_functions` with four new fields:

```rust
// RtFns gains:
print_write_str: FunctionValue<'ctx>,
print_space: FunctionValue<'ctx>,
print_newline: FunctionValue<'ctx>,
print_none: FunctionValue<'ctx>,

// declare_rt_functions gains (inside the `RtFns { ... }` literal):
print_write_str: declare("pycc_rt_print_write_str", void_type.fn_type(&[ptr_type.into()], false)),
print_space: declare("pycc_rt_print_space", void_type.fn_type(&[], false)),
print_newline: declare("pycc_rt_print_newline", void_type.fn_type(&[], false)),
print_none: declare("pycc_rt_print_none", void_type.fn_type(&[], false)),
```

Remove the now-unused `int_print` field from `RtFns`'s struct declaration and from the `RtFns { ... }` literal in `declare_rt_functions` (both added in Task 3) -- this task's new print dispatch above calls `rt.print_write_str` for every argument, `int` included (via `to_str`), so `rt.int_print` has no remaining call site in `pycc_codegen`, and an unread struct field fails `-D warnings`' `dead_code` lint. `pycc_rt_int_print` itself (the underlying Rust `extern "C" fn`, Task 3) is untouched and stays covered by its own existing unit tests -- only its `pycc_codegen`-side `RtFns` declaration, which nothing calls anymore, is removed.

- [ ] **Step 4 run:** `cargo test -p pycc_codegen`
Expected: PASS, including every earlier `print(<single int expression>)` test from Tasks 3-5 (the new dispatch produces byte-identical output for that case: `to_str` on an `Int` scalar calls the same `pycc_rt_int_to_str` Task 8/9 already built and tests, and a single argument has no separator to add).

### Step 5: Run the full workspace test suite

Run: `cargo test --workspace`
Expected: PASS.

### Step 6: Run clippy and the coverage gate

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo llvm-cov clean --workspace && cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`
Expected: PASS. `pycc_rt_int_print` (Task 3's original newline-inclusive helper) stays covered by its own existing direct unit tests even though `pycc_codegen` no longer calls it from this task onward -- `#[unsafe(no_mangle)] pub extern "C" fn`s are never flagged as dead code by `-D warnings` (rustc treats every exported C symbol as a potential external caller), so this is not a coverage or lint regression.

### Step 7: Commit

```bash
git add crates/pycc_rt/src/lib.rs crates/pycc_codegen/src/lib.rs
git commit -m "feat(pycc_rt,pycc_codegen): print's full type-aware, multi-argument formatting (Task 10)"
```

---

## Task 11: End-to-end sweep and documentation sync

**Files:**
- Create: `tests/slice1_codegen_depth.rs`
- Modify: `docs/ROADMAP.md`
- Modify: `docs/ARCHITECTURE.md`
- Modify: `docs/RUNTIME.md`

**Interfaces:**
- Consumes: the real `pycc` CLI binary (`build`/`run`), exactly like `tests/slice0.rs` -- these are real `.py` source fixtures compiled through the whole pipeline (parser → HIR → types → MIR → LLVM → link → run), not hand-built MIR like `pycc_codegen`'s own tests.
- Produces: no new public API.

### Step 1: Create `tests/slice1_codegen_depth.rs` with the shared test helpers

`tests/slice0.rs` is already 767 lines covering PR-1 through PR-4's slice-0 CLI/build/run/check surface; per this plan's own file-structure guidance ("split by responsibility... if a file has grown unwieldy, a split is reasonable"), PR-5's new full-language e2e fixtures get their own file rather than growing `slice0.rs` further.

```rust
use std::io::Write;
use std::process::Command;

fn pycc_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_pycc"))
}

fn write_fixture(dir: &std::path::Path, name: &str, source: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(source.as_bytes()).unwrap();
    path
}

fn build_and_run(label: &str, source: &str) -> std::process::Output {
    let dir = std::env::temp_dir().join(format!("pycc_slice1_{label}_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, &format!("{label}.py"), source);
    let out = dir.join(label);
    let status = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(status.success(), "`pycc build` failed for {label}");
    Command::new(&out).output().unwrap()
}
```

- [ ] **Step 1 commit:**

```bash
git add tests/slice1_codegen_depth.rs
git commit -m "test: scaffold tests/slice1_codegen_depth.rs for PR-5 e2e coverage"
```

### Step 2: Add the recursive `fib` test (exact output, well-known values)

```rust
#[test]
fn recursive_fibonacci_matches_the_well_known_sequence() {
    let source = "\
def fib(n: int) -> int:
    if n < 2:
        return n
    return fib(n - 1) + fib(n - 2)

i = 0
while i < 11:
    print(fib(i))
    i = i + 1
";
    let output = build_and_run("fib_recursive", source);
    assert!(output.status.success());
    assert_eq!(
        output.stdout,
        b"0\n1\n1\n2\n3\n5\n8\n13\n21\n34\n55\n"
    );
}
```

- [ ] **Step 2 run:** `cargo test --test slice1_codegen_depth recursive_fibonacci_matches_the_well_known_sequence`
Expected: PASS.

### Step 3: Add the iterative-`fib`-overflowing-into-bigint test (structural assertion, not a hand-verified exact digit string)

```rust
#[test]
fn iterative_fibonacci_overflows_into_a_bigint_and_prints_only_decimal_digits() {
    // `fib(100)` genuinely exceeds `i64::MAX` (19 decimal digits) -- this
    // asserts the *shape* of the result (more digits than `i64::MAX` can
    // hold, an optional leading `-` aside, no digits lost/garbled)
    // rather than a hand-computed 21-digit reference value, which this
    // plan has no way to verify independently without executing Python.
    let source = "\
def fib_iter(n: int) -> int:
    a = 0
    b = 1
    i = 0
    while i < n:
        temp = a + b
        a = b
        b = temp
        i = i + 1
    return a

print(fib_iter(100))
";
    let output = build_and_run("fib_iterative_bigint", source);
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).expect("output should be valid UTF-8");
    let digits = text.trim_end_matches('\n');
    assert!(
        digits.chars().all(|c| c.is_ascii_digit()),
        "expected only decimal digits, got {digits:?}"
    );
    assert!(
        digits.len() > 19, // i64::MAX ("9223372036854775807") is 19 digits
        "expected a value exceeding i64::MAX's own digit count, got {digits:?}"
    );
}
```

- [ ] **Step 3 run:** `cargo test --test slice1_codegen_depth iterative_fibonacci_overflows_into_a_bigint_and_prints_only_decimal_digits`
Expected: PASS.

### Step 4: Add the FizzBuzz combined-features test (exact output, well-known values)

```rust
#[test]
fn fizzbuzz_exercises_int_arithmetic_modulo_elif_chains_and_mixed_print_types() {
    let source = "\
def fizzbuzz(n: int) -> None:
    i = 1
    while i <= n:
        if i % 15 == 0:
            print(\"FizzBuzz\")
        elif i % 3 == 0:
            print(\"Fizz\")
        elif i % 5 == 0:
            print(\"Buzz\")
        else:
            print(i)
        i = i + 1

fizzbuzz(15)
";
    let output = build_and_run("fizzbuzz", source);
    assert!(output.status.success());
    assert_eq!(
        output.stdout,
        b"1\n2\nFizz\n4\nBuzz\nFizz\n7\n8\nFizz\nBuzz\n11\nFizz\n13\n14\nFizzBuzz\n"
    );
}
```

- [ ] **Step 4 run:** `cargo test --test slice1_codegen_depth fizzbuzz_exercises_int_arithmetic_modulo_elif_chains_and_mixed_print_types`
Expected: PASS.

### Step 5: Add a small `mandelbrot-ascii`-style test (structural assertion: dimensions and character palette)

```rust
#[test]
fn mandelbrot_ascii_produces_a_grid_of_the_expected_dimensions_and_palette() {
    // A first-cut, deliberately small (20x40) rendering exercising
    // nested `while` loops, `float` arithmetic (including true
    // division), a cascading `if`/`elif`/`else` shade lookup, `str`
    // concatenation building a line character by character, and a
    // recursion-free numeric function. Not a byte-exact CPython
    // differential -- that is `pycc_testkit`'s job (PR-6, per
    // DELIVERY_PLAN.md); this only proves the full v0.1 feature
    // combination compiles and runs to a plausible result.
    let source = "\
def mandel_escape(cx: float, cy: float, max_iter: int) -> int:
    x = 0.0
    y = 0.0
    i = 0
    while i < max_iter:
        x2 = x * x
        y2 = y * y
        if x2 + y2 > 4.0:
            return i
        y = 2.0 * x * y + cy
        x = x2 - y2 + cx
        i = i + 1
    return max_iter

def shade_char(level: int) -> str:
    if level <= 0:
        return \" \"
    if level == 1:
        return \".\"
    if level == 2:
        return \":\"
    if level == 3:
        return \"-\"
    if level == 4:
        return \"=\"
    if level == 5:
        return \"+\"
    if level == 6:
        return \"*\"
    if level == 7:
        return \"#\"
    if level == 8:
        return \"%\"
    return \"@\"

height = 20
width = 40
max_iter = 20
row = 0
while row < height:
    line = \"\"
    col = 0
    while col < width:
        cx = -2.0 + (col / width) * 3.0
        cy = -1.0 + (row / height) * 2.0
        iters = mandel_escape(cx, cy, max_iter)
        level = (iters * 9) // max_iter
        line = line + shade_char(level)
        col = col + 1
    print(line)
    row = row + 1
";
    let output = build_and_run("mandelbrot_ascii", source);
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).expect("output should be valid UTF-8");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 20, "expected exactly `height` printed lines");
    let palette: &[char] = &[' ', '.', ':', '-', '=', '+', '*', '#', '%', '@'];
    for (row_index, line) in lines.iter().enumerate() {
        assert_eq!(
            line.chars().count(),
            40,
            "row {row_index} should be exactly `width` characters wide"
        );
        assert!(
            line.chars().all(|c| palette.contains(&c)),
            "row {row_index} contained a character outside the shading palette: {line:?}"
        );
    }
}
```

- [ ] **Step 5 run:** `cargo test --test slice1_codegen_depth mandelbrot_ascii_produces_a_grid_of_the_expected_dimensions_and_palette`
Expected: PASS.

### Step 6: Run the full workspace test suite

Run: `cargo test --workspace`
Expected: PASS.

### Step 7: Run clippy and the coverage gate

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo llvm-cov clean --workspace && cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`
Expected: PASS.

### Step 8: Update `docs/ROADMAP.md`'s "Compiler pipeline" and "Language surface" rows

In the "Current delivery status" table (do not touch the acceptance checklist below it yet -- Step 9 handles that):

- [ ] Replace the "Compiler pipeline" row's evidence text (currently: `"pycc check` now runs the broadened parser → HIR → strict type checker, while [tests/slice0.rs] still proves only the original source → MIR → LLVM object → host linker → native executable subset.") with: `` `pycc check` runs the full parser → HIR → strict type checker pipeline, and `build`/`run` now compile the complete v0.1 language surface (arithmetic, comparisons, `if`/`while`/`for`+`range`, functions with real parameters/return values and recursion, `int`/`float`/`bool`/`str`, string concatenation, basic f-strings, and type-aware multi-argument `print`) through MIR → LLVM object → host linker → native executable, proven by [`tests/slice0.rs`](../tests/slice0.rs) and [`tests/slice1_codegen_depth.rs`](../tests/slice1_codegen_depth.rs). `` (do not claim `--release`/LTO, which stays a v0.2 item, or cross-target CPython-differential conformance, which stays `pycc_testkit`'s PR-6 job).
- [ ] Replace the "Language surface" row's evidence text similarly: `` `pycc check` accepts and type-checks the full v0.1 grammar; `build`/`run` now implement it too -- arithmetic (including `int` overflow-to-bigint per D-001/D-049/D-052 and true-division-always-`float` promotion), comparisons (`int`/`float`/`bool`/`str`, lexicographic for `str`), `if`/`while`/`for`+`range`, functions/recursion, `str` (small-string optimization per D-007/D-050, concatenation, refcounted for named locals), basic f-strings, and `print` for every v0.1 scalar type. Known, accepted gaps (see D-052's own Consequences and Task 7/9's scope notes in [`docs/superpowers/plans/2026-07-25-pr5-codegen-depth.md`](./superpowers/plans/2026-07-25-pr5-codegen-depth.md)): comparing an already-promoted bigint `int`, `int` multiplication/floor-division/modulo/power on a bigint operand, `float` formatting outside CPython's own non-scientific-notation range, and a `str` local not reassigned before its function returns are all honest "not supported yet" boundaries, not silent wrong answers. ``

- [ ] **Step 8 commit:**

```bash
git add docs/ROADMAP.md
git commit -m "docs: update ROADMAP.md's pipeline/language-surface rows for PR-5"
```

### Step 9: Update `docs/ARCHITECTURE.md`'s pipeline diagram and `docs/RUNTIME.md`'s object-model note

`docs/ARCHITECTURE.md`'s pipeline diagram describes the v1.0 target end state (`THIR`, `pycc_own`, a `MIR (typed SSA, ownership-annotated)`, and an `optimizations` stage) -- none of which exist after PR-5. Add a note directly beneath the diagram rather than rewriting the diagram itself (the diagram is the target architecture this project is building toward, and staying honest about "not yet" is this project's own established convention, not a reason to erase the target):

- [ ] Insert after the pipeline code block in `docs/ARCHITECTURE.md`:

```markdown
**Current state (through PR-5):** the diagram above is the v1.0 target. As of PR-5, `pycc_own` does not exist (deferred to v0.5, per DELIVERY_PLAN.md's crate scope), so there is no separate ownership-analysis stage and no THIR; `pycc_types` produces a checked HIR directly, and `pycc_mir`'s `MIR` is a typed *structural mirror* of HIR (D-048), not the ownership-annotated SSA form shown above -- LLVM codegen uses one `alloca` per local/parameter and relies on no optimization pass, matching this project's `--debug`-only v0.1 profile. The `optimizations` stage does not exist yet either. This is a deliberate, currently-accepted gap between the target architecture and today's implementation, not an unplanned deviation.
```

`docs/RUNTIME.md`'s object model says scalars are "unboxed, never touch the runtime." As of PR-5, `int`'s fast path is a low-bit-tagged `i64` (D-052) whose arithmetic/comparison/formatting operations are `pycc_rt` function calls, not raw LLVM instructions -- a real, documented deviation from that steady-state description, not an oversight:

- [ ] Replace `docs/RUNTIME.md`'s `- Scalars (`int` i64-path, `float`, `bool`, `None`) — unboxed, never touch the runtime.` line with:

```markdown
- Scalars (`int` i64-path, `float`, `bool`, `None`) — unboxed (one machine word each, no heap allocation). **Current state (through PR-5):** `int`'s fast path additionally low-bit-tags that word (D-052) and every arithmetic/comparison/formatting operation on it is a `pycc_rt` function call rather than a raw LLVM instruction -- simplest-correct for a `--debug`-only, no-generated-code-perf-requirement v0.1 profile (see D-052's Alternatives); replacing these calls with direct LLVM-intrinsic codegen once a real perf bar exists (v0.2+) is a documented future item, not a v0.1 requirement.
```

Also add one line documenting `str`'s actual PR-5 shape, since the existing `str:` bullet describes only the target layout without noting refcounting's real scope:

- [ ] Append to `docs/RUNTIME.md`'s `str:` bullet: `` **Current state (through PR-5):** every `str` value is a pointer to a refcounted heap object (small-string bytes inline in that same allocation, per D-050); reassigning a named `str` local, and top-level program completion, both decref the previous value -- a `str` bound to a function parameter/local that is never reassigned before that function returns is not yet decrefed at the return site (an accepted, documented leak until `pycc_own`, v0.5, makes real liveness tracking possible -- see Task 7's scope note). ``

- [ ] **Step 9 commit:**

```bash
git add docs/ARCHITECTURE.md docs/RUNTIME.md
git commit -m "docs: reconcile ARCHITECTURE.md/RUNTIME.md with PR-5's actual implementation"
```

### Step 10: Confirm the roadmap acceptance checklist is not overclaimed

- [ ] Re-read `docs/ROADMAP.md`'s "v0.1 acceptance checklist". Confirm `- [ ] fib and mandelbrot-ascii compile and match CPython output on all five Tier-1 targets.` **stays unchecked** -- this plan's Task 11 e2e tests prove the feature set compiles and runs correctly on the local host (and, via the existing CI matrix, on whichever Tier-1 targets `cargo test --workspace` runs on in CI), but the checklist item specifically requires `pycc_testkit`'s pinned-CPython-oracle differential comparison across all five Tier-1 targets, which remains PR-6's deliverable per DELIVERY_PLAN.md's PR breakdown table -- do not flip this box, and do not add a `roadmap-evidence` marker for it (per `scripts/check_roadmap_evidence.rb`'s registered-identifier requirement, only `ci-tier1-cross-compile` and `ci-build-test-coverage-100` are registered, and this item has no registered identifier yet regardless).
- [ ] Confirm `- [ ] pycc check processes 1k LOC in under 50 ms.` and `- [ ] The error demonstration matches the stable CLI specification output.` also stay unchecked -- neither is this task's or this plan's deliverable.

No file changes in this step -- it is a verification-only gate before the final commit.

### Step 11: Regenerate and check the Rust API documentation

Run: `cargo doc --workspace --no-deps`
Expected: succeeds with no warnings on any public item this plan's tasks touched (`pycc_rt`'s and `pycc_codegen`'s newly `pub`/`#[unsafe(no_mangle)] pub extern "C"` functions in particular -- most are crate-internal (`fn`, not `pub fn`) except the `pycc_rt_*` C-ABI surface, which already carries doc comments throughout this plan's tasks). Per `AGENTS.md`, do not commit `target/doc/`.

### Step 12: Final full-workspace verification and commit

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo llvm-cov clean --workspace && cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`
Expected: PASS -- this is the whole PR-5 branch's final gate, not just this task's.

```bash
git add tests/slice1_codegen_depth.rs docs/ROADMAP.md docs/ARCHITECTURE.md docs/RUNTIME.md
git commit -m "test,docs: PR-5 end-to-end sweep (fib, FizzBuzz, mandelbrot-ascii) and doc sync (Task 11)"
```

---
