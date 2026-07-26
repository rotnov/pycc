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
Expected: confirms the next free ID. This plan was drafted when D-047 was the highest existing entry (D-046 covers the frontend-perf-gate cache lifecycle; D-047 covers reverting that gate to a single job matching the upstream trust-anchor validator), so it uses D-048 through D-051 below -- but this repo's `docs/DECISIONS.md` has been extended and renumbered multiple times during PR-4's own review cycle (parallel work landing on `main`/this branch). **Re-verify the actual next-free ID at execution time and renumber every reference in this task (and any place elsewhere in this plan that cites one of these four IDs) if the real repo state differs.**

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

By construction, every `Ty` reaching this module is concrete: the build path's
`resolve_frontend` stage calls `pycc_types::check_and_resolve` before
`pycc_mir::build`. The validation-only `check_frontend` path calls
`pycc_types::check` without materializing a resolved HIR and never feeds its
result to MIR. Therefore `build` can assert that `Ty::Infer` is absent rather
than handle it as a real case.

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
    if left == Ty::Float || right == Ty::Float {
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
