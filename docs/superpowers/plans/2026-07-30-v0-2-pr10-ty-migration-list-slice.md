# v0.2 PR-10: `Ty` Representation Migration + Monomorphization Foundation + `list[T]` Thin Slice Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Migrate `Ty` from a flat six-variant `Copy` enum to a recursive, heap-boxed enum (D-089) capable of representing `list[T]`/`dict[K,V]`/`set[T]`/`tuple[...]`, then implement `list[int]` end-to-end (literal, read-indexing, `len()`, iteration, `.append()`) as v0.2's proof that the new representation actually drives real codegen — closing with `list[int]`'s own PEP-585 conformance fixture.

**Architecture:** The `Ty` migration is four sequential per-crate compile-fix passes (`pycc_hir` → `pycc_types` → `pycc_mir` → `pycc_codegen`), each ending in a green `cargo build -p <crate>`. `pycc_codegen`'s existing `Ty` dispatch is a set of **catch-all** matches (`other => panic!(...)`), not exhaustive ones — adding variants there is *not* compiler-enforced, so that crate's task explicitly enumerates and touches every catch-all site by hand. `list[int]` then lands as five further passes: HIR (new expression/statement forms, narrowly shaped — no general method-call or iterator-protocol machinery), type-checking (homogeneous-element inference, a new "only `list[int]` has codegen so far" diagnostic), MIR, a new `pycc_rt` runtime list object, and `pycc_codegen`'s real LLVM lowering for that one runtime object.

**Tech Stack:** Rust workspace (existing `pycc_ast`/`pycc_hir`/`pycc_types`/`pycc_mir`/`pycc_codegen`/`pycc_rt`/`pycc_diag` crates), `inkwell`/LLVM codegen (existing), `ruff_python_ast`/`ruff_python_parser` 0.0.6 (pinned, already vendored — no new dependency).

## Global Constraints

- D-014: 100% line/region coverage is a hard merge gate for every crate touched in this plan (`pycc_hir`, `pycc_types`, `pycc_mir`, `pycc_codegen`, `pycc_rt`). Every new branch needs an executing test; `tests/*.rs` integration files and `.py` fixtures are outside the coverage denominator.
- This plan needs **zero changes to `.github/workflows/ci.yml`**: it adds no new CI job, only new tests inside the existing `cargo test --workspace` step and the existing conformance harness (D-102). That file is a whole-file digest-pinned security trust anchor (D-100, `scripts/check_roadmap_evidence.rb`) — if any task believes it needs to touch it, stop and re-read `docs/AGENT_RETROSPECTIVE.md`'s 2026-07-30 entry ("A digest-pinned file has no 'comment-only, no functional change' exemption") before doing so.
- D-102's dual-profile conformance-harness shape applies to this plan's own fixture: extend `tests/conformance.rs` in place (no `pycc_testkit` crate), fixture lives flat at `tests/fixtures/pep_0585_builtin_generics.py` (never the stale `tests/conformance/py39/...` path some docs tables still show).
- Every genuinely-undecided implementation fork is recorded as its own accepted `docs/DECISIONS.md` entry with alternatives considered, exactly like every prior PR in this repo (Task 1 records two such entries before any code is touched).
- Runtime error convention (verified in `crates/pycc_rt/src/lib.rs`, e.g. lines 350/393/583): unrecoverable runtime conditions are `panic!("pycc_rt: <description>")`, tested via `#[should_panic(expected = "...")]`. A condition a *type-checked, valid-looking* Python program can actually reach must be a proper `pycc_types` diagnostic (`Txxxx`), never a runtime panic discovered only at codegen — panics are reserved for conditions that are unreachable once type-checking has run correctly.
- Diagnostic-code convention (verified in `docs/DIAGNOSTICS.md`): highest code in use today is `T0031`; codes are narrow and situation-specific (one code per distinct failure shape), never reused. This plan's new codes are `T0032`, `T0033`, `T0034` (assigned in Task 8/9 below — do not renumber if another PR lands a `T0032`+ first; re-check `docs/DIAGNOSTICS.md`'s highest code at the start of Task 8 and shift these three plan-local numbers up together if so).
  - **`T0021` is deliberately reused, not freshly minted, for three of Task 8's failure paths**: the empty-list-literal (`[]`) element-type-unknown case, the non-`int` list-index case, and the `.append()` argument-type-mismatch case. This is not an oversight against the "never reused" rule above — `docs/TYPE_SYSTEM.md`'s v0.1 inference section already documents `T0021` as the general code for "an unconstrained parameter or return variable" *and* "conflicting call-site constraints," and all three of these are instances of that same existing shape (an unconstrained or conflicting type-inference variable), not a new distinct failure shape. `T0032`/`T0033`/`T0034` remain genuinely new codes for genuinely new failure shapes (heterogeneous list-literal elements, non-subscriptable value, non-`int` list element type reaching codegen).
- The pinned `ievo:deep-reviewer` review (D-068, `docs/AGENT_TOOLING.md`) runs on the full branch diff before merge (Task 14).
- Keep `docs/ROADMAP.md`/`docs/DELIVERY_PLAN.md`/`docs/PYTHON_STANDARDS.md`/`docs/TYPE_SYSTEM.md`/`docs/RUNTIME.md`/`docs/TESTING.md`/`docs/SPEC.md` current in the same commit as the behavior change each describes, not deferred to a trailing "docs sweep" commit that never happens.
- v1.0 invariant (docs/SPEC.md's own "Invariants" list, item 2): "Strict types are the only mode. Untyped public API doesn't compile." This plan's own Task 1 D-104 entry records that `list[T]` is **not** annotate-able in v0.2 (no subscripted type-annotation syntax support), so every `list[int]` value in this PR's own fixture/tests must live in a **private helper** (name starts with `_`, per D-038's existing private-helper local-inference convention already used by `docs/DECISIONS.md`'s prior PRs) or at module scope, never as a public function's annotated parameter or return type.

---

## Task 1: Record D-103 and D-104; re-verify `Ty` call-site counts

**Files:**
- Modify: `docs/DECISIONS.md` (two new entries: D-103, D-104)

**Interfaces:**
- Produces: the two accepted decisions every later task cites by number. If `docs/DECISIONS.md`'s highest entry is no longer D-102 when this task runs (another PR may have landed one), number these D-<next> and D-<next+1> instead and use that numbering consistently in your own commit — do not renumber anything already in the file.

- [ ] **Step 1: Re-verify the real `Ty::` call-site counts**

Run, from the repo root:

```bash
for c in pycc_ast pycc_hir pycc_types pycc_mir pycc_codegen pycc_parser pycc_rt; do
  n=$(rg -o 'Ty::' "crates/$c" 2>/dev/null | wc -l | tr -d ' ')
  echo "$c: $n"
done
```

As of this plan's own writing (2026-07-30, HEAD `0598071`), this printed: `pycc_ast: 0`, `pycc_hir: 36`, `pycc_types: 335`, `pycc_mir: 111`, `pycc_codegen: 372`, `pycc_parser: 0`, `pycc_rt: 3` — total 857, up from D-089's own 729 figure. Re-run it yourself; if the numbers differ from these (they will, if any other PR landed `Ty`-touching code since), use your own freshly-measured numbers in the D-103/D-104 text you write below, not these.

- [ ] **Step 2: Confirm `docs/DECISIONS.md`'s current highest entry**

```bash
grep -n "^## D-1" docs/DECISIONS.md | tail -5
```

Expected (as of this plan's writing): highest is `## D-102: Extend \`tests/conformance.rs\`...`. If a higher one exists now, use the next two free numbers after it throughout this step instead of D-103/D-104.

- [ ] **Step 3: Write the D-103 entry (defer the generic-type-parameter placeholder to PR-13)**

Append to `docs/DECISIONS.md` (before its closing, following the exact format of every other entry — `Status`/`Context`/`Decision`/`Alternatives`/`Consequences`):

```markdown
## D-103: Defer the generic function type-parameter `Ty` placeholder to PR-13

- Status: accepted
- Context: D-089 decided `Ty`'s new container variants (`List`/`Dict`/`Set`/`Tuple`) but explicitly left "whatever case PR-13 needs for a generic function's type parameter (a placeholder/parameter marker distinct from `Infer`)" unresolved, naming both PR-10 and PR-13 as candidate owners. PR-10's own actual scope (`docs/DELIVERY_PLAN.md` row 10; `docs/superpowers/specs/2026-07-28-v0-2-collections-generics-design.md`'s PR breakdown item 3) is `Ty` migration + monomorphization foundation + a `list[T]` thin slice with concrete, already-known element types (a literal's element type, a function argument's already-annotated type) — it never emits or consumes a type *parameter* (`def f[T](x: T) -> T`'s `T`), only concrete types. PEP 695 user-defined generic functions are PR-13's own scope (`docs/DELIVERY_PLAN.md` row 13).
- Decision: PR-10 adds no `Ty` variant for generic type parameters. `Ty`'s new recursive variants are exactly `List(Box<Ty>)`, `Dict(Box<Ty>, Box<Ty>)`, `Set(Box<Ty>)`, `Tuple(Vec<Ty>)` — nothing else. PR-13's own implementer designs the type-parameter placeholder against the real constraint-solving code (`collect_expr_constraints`/`infer_expr_in` in `pycc_types`) once PEP 695 syntax parsing actually exists to feed it, rather than PR-10 guessing its shape with no call site to exercise it.
- Alternatives: add a speculative `Ty::TypeParam(String)`-style variant now, unused until PR-13 (rejected — this project's own D-057 "simplest correct thing for the stated scope" precedent argues against speculative variants with no code path exercising them; an unexercised variant is also invisible to every exhaustive match this plan's own Task 2-5 fix, meaning it would silently need re-auditing anyway once PR-13 actually uses it, buying nothing over adding it in PR-13 directly).
- Consequences: `Ty`'s recursive shape after this PR is exactly the four container variants plus the six pre-existing scalars — ten variants total, all fully concrete. PR-13 supersedes nothing here; it simply adds its own variant when it has a real consumer for it.
```

- [ ] **Step 4: Write the D-104 entry (v0.2's `list[T]` scope cuts)**

Append immediately after D-103, using your own Step 1 call-site counts in place of the `<N>` placeholders below:

```markdown
## D-104: v0.2's `list[T]` thin slice — scope cuts and runtime-representation choice

- Status: accepted
- Context: PR-10 (`docs/DELIVERY_PLAN.md` row 10) must migrate `Ty` to a recursive enum (<N total call sites across pycc_hir/pycc_types/pycc_mir/pycc_codegen/pycc_rt as of this decision) and then implement `list[T]` "end-to-end thin slice (literal, indexing, `len()`, iteration, `.append()`)" per the v0.2 design doc's own PR-10 line. Reading the actual current frontend (`crates/pycc_hir/src/lib.rs`): `Stmt::Assign` only accepts a bare-name target (line ~288, "only assigning to a bare name is supported so far") — no subscript-assignment target exists; `Stmt::For` only accepts `for x in range(...)` (line ~369-395) via a dedicated `HirStmt::ForRange { var, start, stop, step, body }` node with no general iterator/iterable abstraction anywhere downstream; `lower_expr`'s `Expr::Call` arm (line ~485) only accepts a bare-name callee, no attribute/method call exists at all today; `annotation_to_ty` (line ~249) only accepts a bare name, so a subscripted annotation like `list[int]` cannot be written today and none of `pycc_types`/`pycc_mir`/`pycc_codegen`'s existing `Ty` dispatch sites are exhaustive matches (they are catch-all `other => panic!(...)` arms — see `ty_to_basic_type`, the local-slot read match, the `BinOp` result match, `collect_module_bindings`'s inner match, all in `crates/pycc_codegen/src/lib.rs`), so adding new `Ty` variants is not compiler-enforced there. Separately, `docs/RUNTIME.md`'s only container-layout line ("`list[T]`: growable vec of unboxed `T`... `list[int]` is literally `Vec<i64>`-shaped") already conflicts with its own generic-heap-object-header line a few lines above (16-byte header with `rc`/`type_id`/`flags`) — the actual, shipped `PyStrObj` (`crates/pycc_rt/src/lib.rs`, struct at line ~702) has neither `type_id` nor `flags`, just `rc: Cell<u32>` plus payload.
- Decision, five scope cuts:
  1. **No annotation syntax for `list[T]` in v0.2.** `list[int]` values exist only via bare-name local inference (`x = [1, 2, 3]`) inside module scope or a private helper (D-038 convention: name starts with `_`), never as a public function's annotated parameter or return type. `annotation_to_ty` is unchanged by this plan; `x: list[int] = []` continues to be rejected by its existing "only a bare name type annotation is supported so far" error, now covered by an explicit regression test asserting this is still true on purpose, not by accident.
  2. **Indexing is read-only.** `x[0]` is a new r-value expression form; `x[0] = value` (subscript as an assignment target) is out of scope — `Stmt::Assign`'s existing bare-name-only restriction is untouched.
  3. **`.append()` and `len()` are narrowly-recognized special forms, not general method/attribute dispatch.** `lst.append(value)` is recognized directly inside `lower_expr`'s existing `Expr::Call` arm when `call.func` is an `Expr::Attribute` whose attribute name is exactly `append` and whose value is a bare `Expr::Name` — producing a new `HirExpr::ListAppend { list: String, value: Box<HirExpr> }` node. `len(lst)` needs **no new HIR node at all**: it is already fully expressible as the existing `HirExpr::Call { callee: "len", args: [lst] }` shape (a bare-name callee), so only `pycc_types`/`pycc_mir`/`pycc_codegen`'s call-dispatch needs a new `"len"` arm alongside their existing `"print"` arm. Neither of these builds general attribute access or a general builtin-function table; both are single, hand-recognized shapes, exactly mirroring how `for x in range(...)` is already hand-recognized in `Stmt::For` today rather than backed by any general iterable/callable abstraction.
  4. **Iteration desugars to an index-counted loop, reusing `ForRange`'s existing lowering shape.** A new `HirStmt::ForList { var: String, list: String, body: Vec<HirStmt> }` (parallel structure to the existing `HirStmt::ForRange`) lowers through `pycc_mir` into a new `MirStmt::ForList { var: String, list: String, body: Vec<MirStmt> }`, which `pycc_codegen` compiles by reusing `MirStmt::ForRange`'s existing loop/branch-building basic-block infrastructure (`crates/pycc_codegen/src/lib.rs` around line 2055), parametrized over a runtime length call instead of a static bound, plus one indexed read per iteration. No user-visible iterator protocol (`__iter__`/`__next__`) is introduced.
  5. **Codegen ships `list[int]` only; every other element type stays a clean, pre-codegen diagnostic, not a runtime panic.** `Ty::List(Box<Ty>)` itself is fully general at the type-representation and type-checking level (a list of `str`, `float`, `bool`, or even a nested list type-checks correctly), but `pycc_types` rejects any `list[T]` where `T != Ty::Int` with a new diagnostic (`T0034`, "list codegen only supports `list[int]` in v0.2") **before** codegen ever runs — this keeps `pycc_codegen`'s existing "should be unreachable" panic convention (per this plan's own Global Constraints) actually true, rather than a type-checked `list[str]` program silently reaching a codegen-level Rust panic. The new runtime object (`crates/pycc_rt`) is therefore exactly one concrete type: a growable, unboxed `i64` buffer with a `PyStrObj`-shaped header (`rc: Cell<u32>` plus `len`/`cap`/data-pointer fields — no `type_id`/`flags`, following the real shipped precedent, not `docs/RUNTIME.md`'s stale, self-inconsistent spec). `docs/RUNTIME.md` itself is corrected in this plan's own Task 12 to describe what's actually built rather than repeating its pre-existing inconsistency.
- Alternatives: build general attribute/method-call dispatch now (rejected — no other PR-10 feature needs it, and it is strictly larger than the two hand-recognized shapes `.append()`/`len()` actually require; YAGNI per this project's own D-057 precedent). Build a real iterator protocol now (rejected — same reasoning; `ForList`'s index-counted desugaring is fully suf'ficient for `list[T]` and is the only container v0.2 iterates over). Implement codegen for all scalar element types immediately (rejected — multiplies `pycc_rt`'s new runtime-object surface by 4-5x for zero benefit to this PR's own stated "thin slice" scope; each additional element type is better sized as its own later PR once list[int]'s pattern is proven end-to-end). Follow `docs/RUNTIME.md`'s existing 16-byte generic-header spec literally (rejected — it is unimplemented by every existing runtime object including `PyStrObj`, and inventing a second, more elaborate header shape used by nothing else in the runtime creates exactly the doc-vs-reality drift this project's own AGENTS.md "keep documentation honest about what exists now" rule forbids; corrected in Task 12 instead).
- Consequences: `T0034` lives in `pycc_types`, at the point a `list[T]` literal's element type is inferred (Task 8), where a real source span is naturally available for the diagnostic — not as a separate pre-codegen validation pass over already-checked MIR/HIR, which would need to reconstruct a span it may no longer have. This means a future PR extending `list[T]` to `str`/`float`/`bool`/nested-list elements needs **two** changes, not one: (a) relax or remove this specific `T0034` gate in `pycc_types` (a small, isolated check — the homogeneity-inference logic around it, and the `Subscript`/`ForList`/`ListAppend` inference it feeds, are already written generically over any scalar `Ty`, not hardcoded to `Int`, and need no changes themselves), and (b) add the new `pycc_rt` runtime object plus `pycc_codegen` dispatch arms for that element type. "Only codegen changes" would be inaccurate; `pycc_mir`'s lowering is already generic and genuinely needs no changes either way. `docs/TYPE_SYSTEM.md`'s "Generics" section describing full PEP 695 monomorphization remains the v1.0 target, unaffected by this narrower interim state.
```

- [ ] **Step 5: Commit**

```bash
git add docs/DECISIONS.md
git commit -m "Record D-103 (defer generic type-param placeholder to PR-13) and D-104 (v0.2 list[T] scope cuts)"
```

---

## Task 2: `Ty` migration — `pycc_hir` (definition + this crate's own internal matches)

**Files:**
- Modify: `crates/pycc_hir/src/lib.rs`

**Interfaces:**
- Consumes: nothing from earlier tasks (this is the first code task).
- Produces: the new `Ty` shape every later task's crate depends on:
  ```rust
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub enum Ty {
      Int,
      Float,
      Bool,
      Str,
      None,
      Infer,
      List(Box<Ty>),
      Dict(Box<Ty>, Box<Ty>),
      Set(Box<Ty>),
      Tuple(Vec<Ty>),
  }
  ```
  and `Ty::name(&self) -> String` (signature changed from `self` to `&self` and from `-> &'static str` to `-> String`, since a boxed inner type can't produce a `'static` string for e.g. `"list[int]"`).

- [ ] **Step 1: Write the failing test for the new variants existing and being structurally comparable**

Add to `crates/pycc_hir/src/lib.rs`'s existing `#[cfg(test)] mod tests` block:

```rust
#[test]
fn ty_list_variant_is_structurally_comparable_and_not_copy() {
    let a = Ty::List(Box::new(Ty::Int));
    let b = Ty::List(Box::new(Ty::Int));
    let c = Ty::List(Box::new(Ty::Str));
    assert_eq!(a, b);
    assert_ne!(a, c);
    // This line only compiles if `Ty` is NOT `Copy` -- moving `a` here and
    // using it again below would be a compile error under `Copy`'s absence,
    // which is exactly what this test wants to lock in. `.clone()` is
    // required precisely because Copy is gone.
    let d = a.clone();
    assert_eq!(a, d);
}

#[test]
fn ty_name_describes_nested_container_types() {
    assert_eq!(Ty::Int.name(), "int");
    assert_eq!(Ty::List(Box::new(Ty::Int)).name(), "list[int]");
    assert_eq!(
        Ty::Dict(Box::new(Ty::Str), Box::new(Ty::Float)).name(),
        "dict[str, float]"
    );
    assert_eq!(Ty::Set(Box::new(Ty::Bool)).name(), "set[bool]");
    assert_eq!(
        Ty::Tuple(vec![Ty::Int, Ty::Str]).name(),
        "tuple[int, str]"
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p pycc_hir ty_list_variant_is_structurally_comparable_and_not_copy ty_name_describes_nested_container_types
```

Expected: compile failure — `Ty::List` doesn't exist yet, and `Ty::name(self)` takes `self` by value with a `&'static str` return that can't produce `"list[int]".to_string()`.

- [ ] **Step 3: Update `Ty`'s definition and derive list**

In `crates/pycc_hir/src/lib.rs`, replace:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ty {
    Int,
    Float,
    Bool,
    Str,
    None,
    Infer,
}
```

with:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ty {
    Int,
    Float,
    Bool,
    Str,
    None,
    Infer,
    /// `list[T]`. Type-checking (Task 8) accepts any scalar `T`; only
    /// `T = Ty::Int` has real codegen in v0.2 (D-104) -- codegen for
    /// anything else is rejected earlier by a `pycc_types` diagnostic
    /// (T0034), never reached as an unhandled codegen case.
    List(Box<Ty>),
    /// `dict[K, V]`. No v0.2 code path constructs this yet (PR-11's own
    /// scope per `docs/DELIVERY_PLAN.md`) -- the variant exists now only
    /// because D-089 decided `Ty`'s full recursive shape up front, so
    /// every later PR's match arms are additive, not migratory again.
    Dict(Box<Ty>, Box<Ty>),
    /// `set[T]`. Same status as `Dict` above -- PR-11's own scope.
    Set(Box<Ty>),
    /// `tuple[A, B, ...]`. Same status as `Dict` above -- PR-11's own scope.
    Tuple(Vec<Ty>),
}
```

- [ ] **Step 4: Update `Ty::name`**

Replace:

```rust
impl Ty {
    pub fn name(self) -> &'static str {
        match self {
            Ty::Int => "int",
            Ty::Float => "float",
            Ty::Bool => "bool",
            Ty::Str => "str",
            Ty::None => "None",
            Ty::Infer => "<inferred>",
        }
    }
}
```

with:

```rust
impl Ty {
    pub fn name(&self) -> String {
        match self {
            Ty::Int => "int".to_string(),
            Ty::Float => "float".to_string(),
            Ty::Bool => "bool".to_string(),
            Ty::Str => "str".to_string(),
            Ty::None => "None".to_string(),
            Ty::Infer => "<inferred>".to_string(),
            Ty::List(elem) => format!("list[{}]", elem.name()),
            Ty::Dict(key, value) => format!("dict[{}, {}]", key.name(), value.name()),
            Ty::Set(elem) => format!("set[{}]", elem.name()),
            Ty::Tuple(elems) => format!(
                "tuple[{}]",
                elems.iter().map(Ty::name).collect::<Vec<_>>().join(", ")
            ),
        }
    }
}
```

- [ ] **Step 5: Fix every call site the signature change breaks, within this crate only**

```bash
cargo build -p pycc_hir 2>&1 | rg "^error" -A 5
```

Every error here is one of two shapes:
1. `Ty::name(x)` called with an owned `Ty` where `x` is later used again → change the call to `x.name()` (the new `&self` signature borrows, so this alone usually fixes it — no `.clone()` needed at a `.name()` call site specifically, since it no longer takes ownership).
2. A place expecting `&'static str` from `.name()` (e.g. a `Diagnostic` message field typed `&'static str`) → widen that field/parameter to `String` (`format!`'s output is already `String`; do not fight this by trying to leak a `String` into `'static` — that would be a real behavior/memory-profile change this task doesn't need).

Do not touch any file outside `crates/pycc_hir/src/lib.rs` in this task — every other crate's own build breakage is Tasks 3-5's job, not this one's. If `cargo build -p pycc_hir` still fails after fixing this crate's own internal call sites, the remaining errors are in a downstream crate reported through `cargo build -p pycc_hir`'s dependency chain; confirm this with `cargo build -p pycc_hir --lib` (library-only, no downstream) before concluding this task is done.

- [ ] **Step 6: Run the tests to verify they pass**

```bash
cargo test -p pycc_hir ty_list_variant_is_structurally_comparable_and_not_copy ty_name_describes_nested_container_types
cargo build -p pycc_hir
```

Expected: both tests PASS, `cargo build -p pycc_hir` succeeds (this crate has no other exhaustive match over `Ty` besides `name` — confirmed during planning by `rg -n 'match .*Ty' crates/pycc_hir/src/lib.rs`; if your own run of that command finds another exhaustive match this plan didn't anticipate, fix it the same way: add the four new arms, matching this task's own style).

- [ ] **Step 7: Commit**

```bash
git add crates/pycc_hir/src/lib.rs
git commit -m "Ty migration part 1: recursive List/Dict/Set/Tuple variants, drop Copy, Ty::name returns String (D-089)"
```

---

## Task 3: `Ty` migration — `pycc_types`

**Files:**
- Modify: `crates/pycc_types/src/lib.rs`

**Interfaces:**
- Consumes: `pycc_hir::Ty` from Task 2 (10 variants, not `Copy`).
- Produces: `pycc_types` compiling cleanly against the new `Ty`, with **zero behavior change** for the 6 pre-existing scalar variants (verified by the crate's own existing test suite staying green with no test bodies changed).

- [ ] **Step 1: Attempt the build to enumerate every break**

```bash
cargo build -p pycc_types 2>&1 | rg "^error" | wc -l
cargo build -p pycc_types 2>&1 | tee /tmp/pycc_types_ty_migration_errors.txt | rg "^error" -A 3
```

Expected: dozens of errors, overwhelmingly one shape: "cannot move out of `*ty` which is behind a shared reference" or "use of moved value" wherever code previously relied on `Ty` being implicitly copied (e.g. `let t = *some_ty_ref;` or passing `ty` by value into a function while also using `ty` afterward).

- [ ] **Step 2: Fix every break by adding `.clone()` at the exact site the compiler flags — never by restructuring logic**

This is mechanical, not a design task: for each compiler error, add `.clone()` at the flagged expression (or change a `*ty` dereference-copy to `ty.clone()`), then rebuild. Do not change which function owns which data, do not introduce new indirection, do not "optimize away" a clone the compiler didn't ask for — every extra clone beyond what the compiler flags is unnecessary allocation this task doesn't need to fix, and every skipped clone the compiler does flag is a build that doesn't finish. Iterate:

```bash
cargo build -p pycc_types 2>&1 | rg "^error" -A 5
# fix the first reported error
cargo build -p pycc_types 2>&1 | rg "^error" -A 5
# repeat until:
cargo build -p pycc_types
# exits 0
```

- [ ] **Step 3: Confirm zero behavior change for the 6 pre-existing scalar variants**

```bash
cargo test -p pycc_types
```

Expected: every existing test in this crate passes unchanged — no test body in this file should need editing in this task. If any existing test fails, you have changed behavior, not just fixed a compile error; find the actual compile fix that doesn't require a behavior change (the almost-always-correct fix is an additional `.clone()`, not a logic change).

- [ ] **Step 4: Commit**

```bash
git add crates/pycc_types/src/lib.rs
git commit -m "Ty migration part 2: pycc_types compiles against the recursive Ty (D-089), zero behavior change"
```

---

## Task 4: `Ty` migration — `pycc_mir`

**Files:**
- Modify: `crates/pycc_mir/src/lib.rs`

**Interfaces:**
- Consumes: `pycc_hir::Ty` (re-exported as `pycc_mir::Ty` per the existing `pub use pycc_hir::{..., Ty};` at `crates/pycc_mir/src/lib.rs:12`) from Task 2; `pycc_types` compiling from Task 3 (this crate's own tests exercise `pycc_types`' checker output).
- Produces: `pycc_mir` compiling cleanly, zero behavior change for the 6 pre-existing scalar variants.

- [ ] **Step 1: Attempt the build to enumerate every break**

```bash
cargo build -p pycc_mir 2>&1 | rg "^error" -A 5
```

- [ ] **Step 2: Fix every break the same way as Task 3 — add `.clone()` exactly where the compiler flags it, no logic changes**

```bash
cargo build -p pycc_mir 2>&1 | rg "^error" -A 5
# fix, repeat, until:
cargo build -p pycc_mir
# exits 0
```

- [ ] **Step 3: Confirm zero behavior change**

```bash
cargo test -p pycc_mir
```

Expected: every existing test passes unchanged, no test body edited.

- [ ] **Step 4: Commit**

```bash
git add crates/pycc_mir/src/lib.rs
git commit -m "Ty migration part 3: pycc_mir compiles against the recursive Ty (D-089), zero behavior change"
```

---

## Task 5: `Ty` migration — `pycc_codegen` (mechanical fixes + explicit catch-all audit)

**Files:**
- Modify: `crates/pycc_codegen/src/lib.rs`

**Interfaces:**
- Consumes: `pycc_hir::Ty`/`pycc_mir::Ty` (Task 2), `pycc_mir` compiling (Task 4).
- Produces: `pycc_codegen` compiling cleanly, zero behavior change for the 6 pre-existing scalar variants, **and** every one of its catch-all `Ty` dispatch sites now has an explicit, honest "not yet supported" arm for the 4 new variants — so a `List`/`Dict`/`Set`/`Tuple` value reaching any of these sites today (before Tasks 7-11 add real `list[int]` support) fails loudly with a clear message, never silently, and never by falling through an existing generic `other => panic!(...)` arm that wasn't written with these variants in mind.

**This task's risk is different from Tasks 3-4's**: `pycc_codegen`'s `Ty` dispatch is **catch-all, not exhaustive** (verified during planning — every site below is `other => panic!(...)` or a `matches!` allow-list, not a `match` the compiler would refuse to build without a new arm). This means `cargo build -p pycc_codegen` alone will **not** tell you every place that needs attention for the new variants — it only tells you where the borrow checker broke on the dropped `Copy`. You must also visit each of the specific sites below by hand.

- [ ] **Step 1: Fix the mechanical (compiler-flagged) breaks first**

```bash
cargo build -p pycc_codegen 2>&1 | rg "^error" -A 5
# fix each with .clone() exactly where flagged, same discipline as Tasks 3-4
cargo build -p pycc_codegen
# exits 0
```

- [ ] **Step 2: Audit and update every catch-all `Ty` dispatch site by hand**

Visit each site below (line numbers are from this plan's own research pass against HEAD `0598071` — re-locate by the quoted context if they've shifted) and add an explicit arm for the 4 new variants that panics with a message naming the specific gap, replacing the generic `other => panic!(...)` catch-all's silent generality with a message that will actually help whoever hits it before Task 11 lands real `list[int]` codegen:

1. **`ty_to_basic_type`** (~line 232-247, decides LLVM type for locals/params/returns):
   ```rust
   fn ty_to_basic_type<'ctx>(context: &'ctx Context, ty: &Ty) -> BasicTypeEnum<'ctx> {
       match ty {
           Ty::Int => context.i64_type().into(),
           Ty::Bool => context.i8_type().into(),
           Ty::Float => context.f64_type().into(),
           Ty::Str => context.ptr_type(AddressSpace::default()).into(),
           Ty::List(_) => context.ptr_type(AddressSpace::default()).into(),
           other => panic!(
               "pycc_codegen: {} has no LLVM representation yet (only int/float/bool/str/list[int] do)",
               other.name()
           ),
       }
   }
   ```
   (Use the exact existing variable names/parameter list from the real function — this snippet shows the *shape* of the fix: a `Ty::List(_)` arm producing a pointer type, since Task 11's runtime list object is heap-allocated and referenced by pointer just like `Str` already is, plus a catch-all whose message now names the actual type via `.name()` instead of a bare `{:?}`.)

2. **Local-slot read match** (~line 543-593): same pattern — add `Ty::List(_) => /* read the pointer slot, matching Str's existing pointer-slot-read arm exactly */`, catch-all message updated to use `.name()`.

3. **`BinOp` result match** (~line 609-710): add an explicit catch-all arm covering `Ty::List(_) | Ty::Dict(..) | Ty::Set(_) | Ty::Tuple(_)` that panics `"pycc_codegen: binary operators are not supported on {} yet"` — no container type supports any `BinOpKind` in this plan's own scope (not even `+` for list concatenation; that's out of scope per D-104), so this arm is a pure diagnostic-message improvement over today's already-correct rejection, not new capability.

4. **`emit_expr`'s `MirExpr::Call` result match** (~line 835-869; Task 5's implementer confirmed this brief's original "`pycc_rt_int_to_str`-style conversion match" label was wrong — the actual code at this line range is the `Call`-result-unpacking match, not a conversion match): same pattern, explicit catch-all naming the type.

5. **`declare_module_globals`'s inner match** (~line 1363-1384; Task 5's implementer confirmed this brief's original "`collect_module_bindings`" label was wrong — `collect_module_bindings` is a different, nearby function with no `Ty` match at all; the actual code at this line range is inside `declare_module_globals`): same pattern.

6. **`collect_stmt_bindings`'s allow-list filter** (~line 1308-1318, currently `matches!(ty, Ty::Int | Ty::Bool | Ty::Float | Ty::Str)`): change to `matches!(ty, Ty::Int | Ty::Bool | Ty::Float | Ty::Str | Ty::List(_))` — a `list[int]` local **does** need its binding collected (Task 11 needs this), so this specific site is not a "panic louder" fix but a real, deliberate inclusion; everything else (`Dict`/`Set`/`Tuple`) stays excluded, since PR-11 owns those.

7. **Return-type → LLVM `fn_type`** (~line 1465-1466): this one already delegates to `ty_to_basic_type` for anything but `Ty::None`, so fixing site 1 above fixes this transitively — confirm with a quick read that no separate `Ty` match exists here beyond the delegation.

- [ ] **Step 3: Write a regression test per audited site confirming the new panic message, for the 3 non-list container variants (Dict/Set/Tuple stay fully unimplemented in this PR)**

Add to `crates/pycc_codegen/src/lib.rs`'s test module:

```rust
#[test]
#[should_panic(expected = "has no LLVM representation yet")]
fn ty_to_basic_type_panics_clearly_for_dict() {
    let context = Context::create();
    ty_to_basic_type(&context, &Ty::Dict(Box::new(Ty::Str), Box::new(Ty::Int)));
}

#[test]
#[should_panic(expected = "has no LLVM representation yet")]
fn ty_to_basic_type_panics_clearly_for_tuple() {
    let context = Context::create();
    ty_to_basic_type(&context, &Ty::Tuple(vec![Ty::Int, Ty::Str]));
}
```

(Match these to the actual test-harness conventions already present in this file for constructing an LLVM `Context` in a unit test — copy the setup from a neighboring existing test rather than inventing new scaffolding.)

- [ ] **Step 4: Run the full existing test suite to confirm zero behavior change for the 6 scalars**

```bash
cargo test -p pycc_codegen
```

Expected: every pre-existing test passes unchanged, plus the two new panic-message tests above pass.

- [ ] **Step 5: Commit**

```bash
git add crates/pycc_codegen/src/lib.rs
git commit -m "Ty migration part 4: pycc_codegen compiles against the recursive Ty, audit every catch-all dispatch site (D-089)"
```

---

## Task 6: Whole-workspace verification — zero behavior change

**Files:** none modified; verification only.

**Interfaces:**
- Consumes: Tasks 2-5's combined result.
- Produces: confidence that the migration is complete and behavior-preserving before any new `list[T]` capability is added on top of it.

- [ ] **Step 1: Full workspace build**

```bash
cargo build --workspace
```

Expected: succeeds with zero errors, zero warnings introduced by this migration (run `cargo build --workspace 2>&1 | rg warning` and confirm any warnings present also existed on `origin/main` before this branch — do not introduce new ones).

- [ ] **Step 2: Full existing test suite**

```bash
cargo test --workspace
```

Expected: every test across every crate passes — this is the load-bearing check that four sequential per-crate mechanical migrations (Tasks 2-5) didn't silently break cross-crate behavior for the 6 pre-existing scalar `Ty` variants.

- [ ] **Step 3: Full conformance suite (requires the pinned CPython 3.14.6 oracle on PATH)**

```bash
cargo test --workspace -- --include-ignored
```

Expected: all pre-existing conformance fixtures (fib, mandelbrot, the 9 PEP fixtures from PR-9) still match CPython byte-for-byte in both debug and release profiles — this is the strongest possible confirmation that the `Ty` migration changed representation, not behavior.

- [ ] **Step 4: `cargo doc` freshness (per this project's own D-021 preflight convention)**

```bash
cargo doc --workspace --no-deps
```

Expected: succeeds with no new broken doc-links; do not commit `target/doc/`.

No commit for this task — it produces no file changes, only verification. If any step above fails, return to the task that introduced the regression (most likely Task 5, given its non-exhaustive-match risk) and fix it there before proceeding.

---

## Task 7: `list[int]` frontend — HIR expression/statement forms

**Files:**
- Modify: `crates/pycc_hir/src/lib.rs`

**Interfaces:**
- Consumes: the 10-variant `Ty` from Task 2.
- Produces (exact names every later task depends on):
  ```rust
  pub enum HirExpr {
      // ...existing variants unchanged...
      ListLiteral(Vec<HirExpr>),
      Subscript { base: Box<HirExpr>, index: Box<HirExpr> },
      ListAppend { list: String, value: Box<HirExpr> },
  }

  pub enum HirStmt {
      // ...existing variants unchanged...
      ForList { var: String, list: String, body: Vec<HirStmt> },
  }
  ```

- [ ] **Step 1: Write the failing tests**

Add to `crates/pycc_hir/src/lib.rs`'s test module:

```rust
#[test]
fn lowers_a_list_literal() {
    let hir = lower_module_from_source("x = [1, 2, 3]\n").unwrap();
    let HirItem::TopLevelStmt(HirStmt::Assign { target, value }) = &hir.items[0] else {
        panic!("expected a top-level Assign, got {:?}", hir.items[0]);
    };
    assert_eq!(target, "x");
    assert_eq!(
        *value,
        HirExpr::ListLiteral(vec![
            HirExpr::IntLiteral(1),
            HirExpr::IntLiteral(2),
            HirExpr::IntLiteral(3),
        ])
    );
}

#[test]
fn lowers_a_read_subscript() {
    let hir = lower_module_from_source("x = [1, 2, 3]\ny = x[0]\n").unwrap();
    let HirItem::TopLevelStmt(HirStmt::Assign { target, value }) = &hir.items[1] else {
        panic!("expected the second top-level Assign, got {:?}", hir.items[1]);
    };
    assert_eq!(target, "y");
    assert_eq!(
        *value,
        HirExpr::Subscript {
            base: Box::new(HirExpr::Name("x".to_string())),
            index: Box::new(HirExpr::IntLiteral(0)),
        }
    );
}

#[test]
fn lowers_append_as_a_dedicated_hir_node_not_a_generic_call() {
    let hir = lower_module_from_source("x = [1]\nx.append(2)\n").unwrap();
    let HirItem::TopLevelStmt(HirStmt::ExprStmt(expr)) = &hir.items[1] else {
        panic!("expected the second top-level ExprStmt, got {:?}", hir.items[1]);
    };
    assert_eq!(
        *expr,
        HirExpr::ListAppend {
            list: "x".to_string(),
            value: Box::new(HirExpr::IntLiteral(2)),
        }
    );
}

#[test]
fn lowers_for_over_a_list_name_to_for_list() {
    let hir = lower_module_from_source("x = [1, 2, 3]\nfor v in x:\n    print(v)\n").unwrap();
    let HirItem::TopLevelStmt(HirStmt::ForList { var, list, body }) = &hir.items[1] else {
        panic!("expected a top-level ForList, got {:?}", hir.items[1]);
    };
    assert_eq!(var, "v");
    assert_eq!(list, "x");
    assert_eq!(body.len(), 1);
}

#[test]
fn subscripted_type_annotation_is_still_rejected_on_purpose() {
    // D-104: v0.2 adds no annotation-syntax support for list[T]. This test
    // locks in that `x: list[int] = []` keeps failing today's existing
    // "only a bare name type annotation" capability error, so a future
    // change to `annotation_to_ty` doesn't silently start accepting this
    // without its own deliberate decision.
    assert_capability_error_message(
        "x: list[int] = []\n",
        "only a bare name type annotation is supported so far",
    );
}
```

(Match `lower_module_from_source`/`assert_capability_error_message`'s exact existing signatures from this file's own test helpers — do not invent new ones; these two helper names are taken from this crate's own existing test module, confirmed present during planning.)

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p pycc_hir lowers_a_list_literal lowers_a_read_subscript lowers_append_as_a_dedicated_hir_node_not_a_generic_call lowers_for_over_a_list_name_to_for_list subscripted_type_annotation_is_still_rejected_on_purpose
```

Expected: the first four fail (missing variants / compile errors), the fifth already passes today (it's a regression-lock test, not new behavior) — confirm it passes before your other changes and stays passing after.

- [ ] **Step 3: Add the new `HirExpr`/`HirStmt` variants**

In `crates/pycc_hir/src/lib.rs`, add to the existing `HirExpr` enum:

```rust
    /// `[e1, e2, ...]`. Element homogeneity is `pycc_types`' job (Task 8),
    /// not this lowering step's -- HIR only records the syntactic shape.
    ListLiteral(Vec<HirExpr>),
    /// `base[index]`, read-only (D-104 -- no subscript assignment target
    /// exists in v0.2).
    Subscript { base: Box<HirExpr>, index: Box<HirExpr> },
    /// `list.append(value)`, recognized as a single dedicated node rather
    /// than through any general method-call mechanism (D-104).
    ListAppend { list: String, value: Box<HirExpr> },
```

and to the existing `HirStmt` enum:

```rust
    /// `for var in list:`, parallel to the existing `ForRange` -- desugars
    /// to an index-counted loop starting in pycc_mir (Task 10), not here.
    ForList { var: String, list: String, body: Vec<HirStmt> },
```

- [ ] **Step 4: Recognize list literals in `lower_expr`**

Add this arm to `lower_expr`'s `match expr` (before the trailing `other => ...` catch-all):

```rust
        Expr::List(list) => HirExpr::ListLiteral(
            list.elts
                .iter()
                .map(lower_expr)
                .collect::<Result<Vec<_>, _>>()?,
        ),
```

- [ ] **Step 5: Recognize read-subscript in `lower_expr`**

Add this arm (also before the catch-all):

```rust
        Expr::Subscript(sub) => {
            if sub.ctx != ExprContext::Load {
                return Err(unsupported(
                    "subscript assignment is not supported yet (D-104: indexing is read-only in v0.2)",
                    sub.range,
                ));
            }
            HirExpr::Subscript {
                base: Box::new(lower_expr(&sub.value)?),
                index: Box::new(lower_expr(&sub.slice)?),
            }
        }
```

(Add `ExprContext` and `ruff_python_ast::ExprSubscript`'s field names to `pycc_ast`'s re-export list at the top of `crates/pycc_ast/src/lib.rs` only if they are not already visible through the existing `Expr` re-export — confirm by attempting the build; per this plan's own research, `Expr::Subscript(sub)` pattern-matching needs no new re-export since `Expr` itself is already re-exported, but `sub.ctx`'s type `ExprContext` needs to be nameable, and `ExprContext` is already in the existing re-export list — verify this compiles before assuming a new re-export is needed.)

- [ ] **Step 6: Recognize `.append()` inside the existing `Expr::Call` arm**

`lower_expr`'s existing `Expr::Call(call)` arm currently does:

```rust
        Expr::Call(call) => {
            if !call.arguments.keywords.is_empty() {
                return Err(unsupported(
                    "keyword call arguments are not supported yet",
                    call.range,
                ));
            }
            let Expr::Name(callee) = call.func.as_ref() else {
                return Err(unsupported(
                    format!(
                        "only calling a bare name is supported so far: {:?}",
                        call.func
                    ),
                    pycc_ast::expr_range(&call.func),
                ));
            };
            let args = call
                .arguments
                .args
                .iter()
                .map(lower_expr)
                .collect::<Result<Vec<_>, _>>()?;
            HirExpr::Call {
                callee: callee.id.as_str().to_string(),
                args,
            }
        }
```

Change the `else` branch's fallthrough to check for the `.append()` shape **before** rejecting a non-bare-name callee:

```rust
        Expr::Call(call) => {
            if !call.arguments.keywords.is_empty() {
                return Err(unsupported(
                    "keyword call arguments are not supported yet",
                    call.range,
                ));
            }
            if let Expr::Attribute(attr) = call.func.as_ref() {
                if attr.attr.as_str() == "append" {
                    let Expr::Name(list_name) = attr.value.as_ref() else {
                        return Err(unsupported(
                            "`.append()` is only supported on a bare-name list so far",
                            pycc_ast::expr_range(&attr.value),
                        ));
                    };
                    let [value] = call.arguments.args.as_slice() else {
                        return Err(unsupported(
                            format!(
                                "list.append() takes exactly one argument, got {}",
                                call.arguments.args.len()
                            ),
                            call.range,
                        ));
                    };
                    return Ok(HirExpr::ListAppend {
                        list: list_name.id.as_str().to_string(),
                        value: Box::new(lower_expr(value)?),
                    });
                }
                return Err(unsupported(
                    format!(
                        "only the `.append()` method is supported so far, got `.{}(...)`",
                        attr.attr
                    ),
                    call.range,
                ));
            }
            let Expr::Name(callee) = call.func.as_ref() else {
                return Err(unsupported(
                    format!(
                        "only calling a bare name is supported so far: {:?}",
                        call.func
                    ),
                    pycc_ast::expr_range(&call.func),
                ));
            };
            let args = call
                .arguments
                .args
                .iter()
                .map(lower_expr)
                .collect::<Result<Vec<_>, _>>()?;
            HirExpr::Call {
                callee: callee.id.as_str().to_string(),
                args,
            }
        }
```

(This changes `lower_expr`'s `match` body for this arm into an early-`return Ok(...)` style for the `.append()` case specifically, since it needs to skip the rest of the arm's logic and the outer `match`'s trailing `Ok(lowered)` — check the exact existing function structure and adapt the control flow to fit; the exact mechanism, whether an early return or a nested `let lowered = ...`, is a mechanical choice, but the recognized shape and produced `HirExpr::ListAppend` must match exactly.)

- [ ] **Step 7: Recognize `for var in list_name:` in `lower_stmt`'s existing `Stmt::For` arm**

The existing `Stmt::For` arm rejects any non-`range(...)` iterable with `"only for x in range(...) is supported so far"`. Change the rejection point so a bare-name iterable is instead lowered to `HirStmt::ForList`, and only a non-bare-name, non-`range(...)` iterable still gets the original rejection:

```rust
            let Expr::Name(var) = for_stmt.target.as_ref() else {
                return Err(unsupported(
                    format!(
                        "only a bare name for-target is supported so far: {:?}",
                        for_stmt.target
                    ),
                    pycc_ast::expr_range(&for_stmt.target),
                ));
            };
            // A bare-name iterable is `for v in some_list:` (D-104) --
            // resolved to `Ty::List` or rejected by pycc_types (Task 9),
            // not here; HIR only records the syntactic shape.
            if let Expr::Name(list_name) = for_stmt.iter.as_ref() {
                return Ok(HirStmt::ForList {
                    var: var.id.to_string(),
                    list: list_name.id.as_str().to_string(),
                    body: lower_body(&for_stmt.body)?,
                });
            }
            let Expr::Call(call) = for_stmt.iter.as_ref() else {
                return Err(unsupported(
                    format!(
                        "only `for x in range(...)` or `for x in <list>` is supported so far: {:?}",
                        for_stmt.iter
                    ),
                    pycc_ast::expr_range(&for_stmt.iter),
                ));
            };
            // ...existing range(...) handling continues unchanged below...
```

(Same note as Step 6: adapt this early-return into whatever control-flow shape `lower_stmt`'s own function structure actually uses — the recognized shapes and produced HIR nodes are what must match exactly, not the exact Rust control-flow idiom.)

- [ ] **Step 8: Run the tests to verify they pass**

```bash
cargo test -p pycc_hir lowers_a_list_literal lowers_a_read_subscript lowers_append_as_a_dedicated_hir_node_not_a_generic_call lowers_for_over_a_list_name_to_for_list subscripted_type_annotation_is_still_rejected_on_purpose
cargo build -p pycc_hir
```

- [ ] **Step 9: Commit**

```bash
git add crates/pycc_hir/src/lib.rs crates/pycc_ast/src/lib.rs
git commit -m "list[int] part 1: HIR forms for list literal, read-subscript, .append(), for-over-list (D-104)"
```

---

## Task 8: `list[int]` type-checking — homogeneous inference, `T0032`/`T0033`/`T0034`

**Files:**
- Modify: `crates/pycc_types/src/lib.rs`
- Modify: `docs/DIAGNOSTICS.md` (register the 3 new codes)
- Test: `tests/diagnostics/d0032_heterogeneous_list_literal.py` + `.expected.txt`
- Test: `tests/diagnostics/d0033_subscript_on_non_list.py` + `.expected.txt`
- Test: `tests/diagnostics/d0034_list_element_type_not_int.py` + `.expected.txt`

**Interfaces:**
- Consumes: `HirExpr::ListLiteral`/`Subscript`/`ListAppend`, `HirStmt::ForList` from Task 7.
- Produces: `pycc_types` inferring `Ty::List(Box<Ty>)` for list literals and rejecting the 3 new failure shapes with the diagnostics below; every later task (Task 10 MIR, Task 11 codegen) can assume a `list[int]`-typed value reaching them has already been confirmed homogeneous, `int`-elemented, and correctly indexed.

- [ ] **Step 1: Re-confirm `docs/DIAGNOSTICS.md`'s current highest code**

```bash
grep -oE "T00[0-9]{2}" docs/DIAGNOSTICS.md | sort -u | tail -3
```

Expected (as of this plan's writing): `T0031` is highest. If a higher one now exists, shift this task's `T0032`/`T0033`/`T0034` up together to the next three free numbers and use those consistently across this task's diagnostics, tests, and `docs/DIAGNOSTICS.md` entries.

- [ ] **Step 2: Write the failing diagnostic fixture tests**

Create `tests/diagnostics/d0032_heterogeneous_list_literal.py`:

```python
x = [1, "two", 3]
```

Create `tests/diagnostics/d0032_heterogeneous_list_literal.expected.txt` (match this repo's existing diagnostic-expected-file format exactly — copy the header/span-rendering style from `tests/diagnostics/d0025_annotated_assignment_mismatch.expected.txt`, PR-9's own precedent, substituting this fixture's own message and span):

```
error[T0032]: list literal elements must all share one type
 --> tests/diagnostics/d0032_heterogeneous_list_literal.py:1:1
  |
1 | x = [1, "two", 3]
  | ^^^^^^^^^^^^^^^^^ list element type mismatch: expected int (from the first element), found str
  |
```

Create `tests/diagnostics/d0033_subscript_on_non_list.py`:

```python
x = 5
y = x[0]
```

Create `tests/diagnostics/d0033_subscript_on_non_list.expected.txt`:

```
error[T0033]: value is not subscriptable
 --> tests/diagnostics/d0033_subscript_on_non_list.py:2:5
  |
2 | y = x[0]
  |     ^^^^ `int` does not support indexing
  |
```

Create `tests/diagnostics/d0034_list_element_type_not_int.py`:

```python
x = ["a", "b"]
```

Create `tests/diagnostics/d0034_list_element_type_not_int.expected.txt`:

```
error[T0034]: list codegen only supports list[int] in v0.2
 --> tests/diagnostics/d0034_list_element_type_not_int.py:1:1
  |
1 | x = ["a", "b"]
  | ^^^^^^^^^^^^^^ list[str] is not compiled yet (D-104) -- only list[int] is
  |
```

Add the 3 matching test functions to `tests/diagnostics_test.rs`, copying the exact existing pattern used for `d0025_annotated_assignment_mismatch` (PR-9) — same helper function, same assertion shape, just new file names.

- [ ] **Step 3: Run the tests to verify they fail**

```bash
cargo test --test diagnostics_test d0032_heterogeneous_list_literal d0033_subscript_on_non_list d0034_list_element_type_not_int
```

Expected: FAIL (no such diagnostic code exists yet).

- [ ] **Step 4: Register the 3 codes in `docs/DIAGNOSTICS.md`**

Add 3 rows to the registry table, following its existing format exactly (copy a neighboring row's structure, e.g. `T0025`'s row from PR-9):

```markdown
| `T0032` | error | list literal elements must all share one type |
| `T0033` | error | value is not subscriptable |
| `T0034` | error | list codegen only supports `list[int]` in v0.2 (D-104) |
```

- [ ] **Step 5: Implement homogeneous list-literal inference and the 3 diagnostics**

In `crates/pycc_types/src/lib.rs`, find the function that type-checks expressions (the one already handling `HirExpr::Call`/`HirExpr::BinOp`/etc. — locate it via `rg -n "HirExpr::BinOp" crates/pycc_types/src/lib.rs` to find the right match) and add:

```rust
        HirExpr::ListLiteral(elements) => {
            if elements.is_empty() {
                // An empty list literal has no element to infer a type
                // from. v0.2 has no annotation syntax for list[T] (D-104),
                // so an empty list literal with no later-inferable use has
                // no way to recover its element type -- reject it plainly
                // rather than silently guessing `Ty::Infer` all the way
                // through codegen.
                return Err(diagnostic(
                    "T0021",
                    "an empty list literal's element type cannot be inferred without an annotation (list[T] annotations are not supported yet, D-104)",
                    span_of(expr),
                ));
            }
            let mut elem_ty = None;
            for element in elements {
                let this_ty = check_expr(element, /* ...existing checker-state params... */)?;
                match &elem_ty {
                    None => elem_ty = Some(this_ty),
                    Some(expected) if *expected == this_ty => {}
                    Some(expected) => {
                        return Err(diagnostic(
                            "T0032",
                            format!(
                                "list element type mismatch: expected {} (from the first element), found {}",
                                expected.name(),
                                this_ty.name()
                            ),
                            span_of(expr),
                        ));
                    }
                }
            }
            let elem_ty = elem_ty.expect("checked non-empty above");
            if elem_ty != Ty::Int {
                return Err(diagnostic(
                    "T0034",
                    format!(
                        "list[{}] is not compiled yet (D-104) -- only list[int] is",
                        elem_ty.name()
                    ),
                    span_of(expr),
                ));
            }
            Ty::List(Box::new(elem_ty))
        }
        HirExpr::Subscript { base, index } => {
            let base_ty = check_expr(base, /* ... */)?;
            let index_ty = check_expr(index, /* ... */)?;
            if index_ty != Ty::Int {
                return Err(diagnostic(
                    "T0021",
                    format!("list index must be int, found {}", index_ty.name()),
                    span_of(index),
                ));
            }
            match base_ty {
                Ty::List(elem_ty) => *elem_ty,
                other => {
                    return Err(diagnostic(
                        "T0033",
                        format!("`{}` does not support indexing", other.name()),
                        span_of(base),
                    ));
                }
            }
        }
        HirExpr::ListAppend { list, value } => {
            let list_ty = /* ...look up `list`'s already-bound type from the checker's environment, same lookup HirExpr::Name already uses... */;
            let Ty::List(elem_ty) = &list_ty else {
                return Err(diagnostic(
                    "T0033",
                    format!("`{}` does not support `.append()`", list_ty.name()),
                    span_of(expr),
                ));
            };
            let value_ty = check_expr(value, /* ... */)?;
            if value_ty != **elem_ty {
                return Err(diagnostic(
                    "T0021",
                    format!(
                        "cannot append {} to a list of {}",
                        value_ty.name(),
                        elem_ty.name()
                    ),
                    span_of(value),
                ));
            }
            Ty::None
        }
```

(The exact parameter names/types passed to `check_expr` and however this crate looks up a bare name's already-bound type — match this file's own existing internal API precisely; the snippet above shows the required control flow and diagnostic shapes, not this crate's exact private helper signatures, which you have direct read access to confirm.)

Add the equivalent handling for `HirStmt::ForList` wherever `HirStmt::ForRange` is currently type-checked (`rg -n "HirStmt::ForRange" crates/pycc_types/src/lib.rs` to find every site — there were 7+ found during planning, each needs a parallel `ForList` arm): the loop variable (`var`) gets bound to the list's element type (extracted the same way `Subscript`'s base-type check does above), the body is checked the same way `ForRange`'s body already is.

- [ ] **Step 6: Run the tests to verify they pass**

```bash
cargo test --test diagnostics_test d0032_heterogeneous_list_literal d0033_subscript_on_non_list d0034_list_element_type_not_int
cargo test -p pycc_types
```

- [ ] **Step 7: Commit**

```bash
git add crates/pycc_types/src/lib.rs docs/DIAGNOSTICS.md tests/diagnostics/d0032_heterogeneous_list_literal.py tests/diagnostics/d0032_heterogeneous_list_literal.expected.txt tests/diagnostics/d0033_subscript_on_non_list.py tests/diagnostics/d0033_subscript_on_non_list.expected.txt tests/diagnostics/d0034_list_element_type_not_int.py tests/diagnostics/d0034_list_element_type_not_int.expected.txt tests/diagnostics_test.rs
git commit -m "list[int] part 2: pycc_types homogeneous-list inference, T0032/T0033/T0034 (D-104)"
```

**Post-implementation addendum (added during Task 8, in a follow-up commit, not originally in this task's Step list):** D-104 point 3 requires `len(lst)` to type-check via a hand-recognized `"len"` arm in `pycc_types`' call dispatch, alongside the existing `"print"` special-case, at both the `collect_expr_constraints` (solver) and `infer_expr_in` (real check) sites — Task 11's own end-to-end fixture (`print(len(x))`) assumes this already type-checks by the time codegen runs. Added: exactly-one-argument + `Ty::List(_)`-argument validation (generic over any element type, matching `Subscript`/`ListAppend`/`ForList`'s existing genericity), returning `Ty::Int`; both failure shapes reuse `T0033` (`docs/DIAGNOSTICS.md`'s row broadened accordingly), with corresponding unit tests at both dispatch sites.

---

## Task 9: `list[int]` MIR lowering

**Files:**
- Modify: `crates/pycc_mir/src/lib.rs`

**Interfaces:**
- Consumes: `HirExpr::ListLiteral`/`Subscript`/`ListAppend`, `HirStmt::ForList` (Task 7), type-checked as `Ty::List(Box::new(Ty::Int))` only (Task 8 already rejected anything else before MIR ever sees it).
- Produces:
  ```rust
  pub enum MirExpr {
      // ...existing variants unchanged...
      ListLiteral(Vec<MirExpr>),
      Subscript { base: Box<MirExpr>, index: Box<MirExpr> },
      ListAppend { list: String, value: Box<MirExpr> },
  }

  pub enum MirStmt {
      // ...existing variants unchanged...
      ForList { var: String, list: String, body: Vec<MirStmt> },
  }
  ```

- [ ] **Step 1: Write the failing tests**

Add to `crates/pycc_mir/src/lib.rs`'s test module, following this file's own existing test pattern for `lower_stmt`/`lower_expr` (e.g. the `ForRange` test near line 1177, or the module-level test near line 562):

```rust
#[test]
fn lowers_list_literal_to_mir() {
    let hir_module = /* ...construct a minimal HIR module the same way this file's own existing ForRange test at line ~562 does, with one top-level Assign to a ListLiteral... */;
    let mir_module = lower_module(&hir_module).unwrap();
    let MirItem::TopLevelStmt(MirStmt::Assign { value, .. }) = &mir_module.items[0] else {
        panic!("expected a top-level Assign");
    };
    assert_eq!(
        *value,
        MirExpr::ListLiteral(vec![MirExpr::IntLiteral(1), MirExpr::IntLiteral(2)])
    );
}

#[test]
fn lowers_for_list_to_mir_for_list() {
    let hir_module = /* ...a module with HirStmt::ForList, mirroring this file's own existing ForRange construction test... */;
    let mir_module = lower_module(&hir_module).unwrap();
    let MirItem::TopLevelStmt(MirStmt::ForList { var, list, .. }) = &mir_module.items[1] else {
        panic!("expected a top-level ForList");
    };
    assert_eq!(var, "v");
    assert_eq!(list, "x");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p pycc_mir lowers_list_literal_to_mir lowers_for_list_to_mir_for_list
```

- [ ] **Step 3: Add the MIR variants and lowering arms**

Add `ListLiteral`/`Subscript`/`ListAppend` to `MirExpr` and `ForList` to `MirStmt`, matching Task 7's `HirExpr`/`HirStmt` field shapes exactly. Add lowering arms in whichever function already lowers `HirExpr::BinOp`/`HirStmt::ForRange` into their `Mir*` equivalents (`rg -n "HirStmt::ForRange =>" crates/pycc_mir/src/lib.rs` to find the exact spot), following the same recursive-lowering pattern already used for every other expression/statement kind in this file (each `Hir*` variant maps to exactly one `Mir*` variant with each of its own sub-expressions/sub-statements recursively lowered the same way).

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p pycc_mir lowers_list_literal_to_mir lowers_for_list_to_mir_for_list
cargo build -p pycc_mir
```

- [ ] **Step 5: Commit**

```bash
git add crates/pycc_mir/src/lib.rs
git commit -m "list[int] part 3: pycc_mir lowering for list literal, subscript, append, for-list (D-104)"
```

**Note added during execution:** this task's actual scope grew by one required fix beyond the steps above, found by the controller before dispatch (mirroring the `len()` gap Task 8 found and fixed for `pycc_types`): `pycc_mir`'s existing `HirExpr::Call` lowering arm (`crates/pycc_mir/src/lib.rs:298-309`) special-cases `callee == "print"` to produce `Ty::None` directly, falling back to `lookup(scopes, "$fn:{callee}")` (which panics on a missing key) for every other callee. Without a parallel `callee == "len"` branch producing `Ty::Int`, `len(lst)` — already accepted by Task 8's `pycc_types` as valid — would panic during MIR lowering with `lookup`'s own internal-error message. This task's implementer added `else if callee == "len" { Ty::Int }` alongside the existing `"print"` case, plus a covering test, as part of the same commit. Task 11's own `"len"` addition (`pycc_codegen`'s call-dispatch) is unaffected by this note — codegen still needs its own arm, independent of this MIR-level fix.

---

## Task 10: `list[int]` runtime object in `pycc_rt`

**Files:**
- Modify: `crates/pycc_rt/src/lib.rs`

**Interfaces:**
- Consumes: nothing from earlier tasks (pure runtime addition, callable independently).
- Produces the extern "C" ABI Task 11's codegen calls directly:
  ```rust
  #[repr(C)]
  pub struct PyIntListObj { /* rc, len, cap, data ptr -- see Step 3 */ }

  pub extern "C" fn pycc_rt_int_list_new() -> *mut PyIntListObj;
  pub extern "C" fn pycc_rt_int_list_append(list: *mut PyIntListObj, value: i64);
  pub extern "C" fn pycc_rt_int_list_get(list: *mut PyIntListObj, index: i64) -> i64;
  pub extern "C" fn pycc_rt_int_list_len(list: *mut PyIntListObj) -> i64;
  pub extern "C" fn pycc_rt_int_list_incref(list: *mut PyIntListObj);
  pub extern "C" fn pycc_rt_int_list_decref(list: *mut PyIntListObj);
  ```

- [ ] **Step 1: Write the failing tests**

Add to `crates/pycc_rt/src/lib.rs`'s test module, following the exact style of this file's existing `PyStrObj` tests (e.g. its `incref`/`decref`/construction tests):

```rust
#[test]
fn int_list_new_starts_empty() {
    let list = pycc_rt_int_list_new();
    assert_eq!(pycc_rt_int_list_len(list), 0);
    pycc_rt_int_list_decref(list);
}

#[test]
fn int_list_append_then_get_round_trips() {
    let list = pycc_rt_int_list_new();
    pycc_rt_int_list_append(list, 10);
    pycc_rt_int_list_append(list, 20);
    pycc_rt_int_list_append(list, 30);
    assert_eq!(pycc_rt_int_list_len(list), 3);
    assert_eq!(pycc_rt_int_list_get(list, 0), 10);
    assert_eq!(pycc_rt_int_list_get(list, 1), 20);
    assert_eq!(pycc_rt_int_list_get(list, 2), 30);
    pycc_rt_int_list_decref(list);
}

#[test]
fn int_list_grows_past_its_initial_capacity() {
    let list = pycc_rt_int_list_new();
    for i in 0..1000 {
        pycc_rt_int_list_append(list, i);
    }
    assert_eq!(pycc_rt_int_list_len(list), 1000);
    assert_eq!(pycc_rt_int_list_get(list, 999), 999);
    pycc_rt_int_list_decref(list);
}

#[test]
#[should_panic(expected = "pycc_rt: list index out of range")]
fn int_list_get_out_of_range_panics_honestly() {
    let list = pycc_rt_int_list_new();
    pycc_rt_int_list_append(list, 1);
    pycc_rt_int_list_get(list, 5);
}

#[test]
fn int_list_incref_decref_round_trip_does_not_free_early() {
    let list = pycc_rt_int_list_new();
    pycc_rt_int_list_append(list, 1);
    pycc_rt_int_list_incref(list);
    pycc_rt_int_list_decref(list);
    // still alive after one incref/decref pair -- one decref remains
    assert_eq!(pycc_rt_int_list_get(list, 0), 1);
    pycc_rt_int_list_decref(list);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p pycc_rt int_list_new_starts_empty int_list_append_then_get_round_trips int_list_grows_past_its_initial_capacity int_list_get_out_of_range_panics_honestly int_list_incref_decref_round_trip_does_not_free_early
```

Expected: compile failure — none of these functions/types exist yet.

- [ ] **Step 3: Implement `PyIntListObj` and its extern "C" functions**

Add to `crates/pycc_rt/src/lib.rs`, following `PyStrObj`'s exact real header shape (`rc: Cell<u32>` plus payload — no `type_id`/`flags`, per D-104's own decision to follow the real precedent, not `docs/RUNTIME.md`'s stale spec):

```rust
/// `list[int]`'s runtime object (D-104: v0.2 codegen only supports
/// `list[int]`; other element types stay a `pycc_types` diagnostic,
/// T0034, never reaching codegen). Header shape follows `PyStrObj`'s
/// real precedent (`rc: Cell<u32>` plus payload) rather than
/// `docs/RUNTIME.md`'s stale 16-byte generic-header spec (corrected in
/// this plan's own Task 12) -- `PyStrObj` is this runtime's only other
/// heap object and it never had a `type_id`/`flags` field either.
#[repr(C)]
pub struct PyIntListObj {
    rc: Cell<u32>,
    len: Cell<usize>,
    cap: Cell<usize>,
    data: Cell<*mut i64>,
}

const INITIAL_CAPACITY: usize = 4;

#[no_mangle]
pub extern "C" fn pycc_rt_int_list_new() -> *mut PyIntListObj {
    let data = unsafe {
        std::alloc::alloc(
            std::alloc::Layout::array::<i64>(INITIAL_CAPACITY).expect("layout overflow"),
        )
    } as *mut i64;
    if data.is_null() {
        panic!("pycc_rt: allocation failure constructing an int list");
    }
    Box::into_raw(Box::new(PyIntListObj {
        rc: Cell::new(1),
        len: Cell::new(0),
        cap: Cell::new(INITIAL_CAPACITY),
        data: Cell::new(data),
    }))
}

#[no_mangle]
pub extern "C" fn pycc_rt_int_list_append(list: *mut PyIntListObj, value: i64) {
    let list = unsafe { &*list };
    let len = list.len.get();
    let cap = list.cap.get();
    if len == cap {
        let new_cap = cap * 2;
        let new_data = unsafe {
            std::alloc::realloc(
                list.data.get() as *mut u8,
                std::alloc::Layout::array::<i64>(cap).expect("layout overflow"),
                new_cap * std::mem::size_of::<i64>(),
            )
        } as *mut i64;
        if new_data.is_null() {
            panic!("pycc_rt: allocation failure growing an int list");
        }
        list.data.set(new_data);
        list.cap.set(new_cap);
    }
    unsafe {
        *list.data.get().add(len) = value;
    }
    list.len.set(len + 1);
}

#[no_mangle]
pub extern "C" fn pycc_rt_int_list_get(list: *mut PyIntListObj, index: i64) -> i64 {
    let list = unsafe { &*list };
    let len = list.len.get();
    if index < 0 || index as usize >= len {
        panic!("pycc_rt: list index out of range");
    }
    unsafe { *list.data.get().add(index as usize) }
}

#[no_mangle]
pub extern "C" fn pycc_rt_int_list_len(list: *mut PyIntListObj) -> i64 {
    unsafe { &*list }.len.get() as i64
}

#[no_mangle]
pub extern "C" fn pycc_rt_int_list_incref(list: *mut PyIntListObj) {
    let list = unsafe { &*list };
    list.rc.set(list.rc.get() + 1);
}

#[no_mangle]
pub extern "C" fn pycc_rt_int_list_decref(list: *mut PyIntListObj) {
    let rc = unsafe { &*list }.rc.get();
    if rc == 1 {
        let list_box = unsafe { Box::from_raw(list) };
        unsafe {
            std::alloc::dealloc(
                list_box.data.get() as *mut u8,
                std::alloc::Layout::array::<i64>(list_box.cap.get()).expect("layout overflow"),
            );
        }
        // list_box drops here, freeing the header allocation too.
    } else {
        unsafe { &*list }.rc.set(rc - 1);
    }
}
```

(Match this crate's own existing allocation/dealloc idioms exactly — e.g. check whether `PyStrObj`'s existing heap payload uses `std::alloc` directly the same way, or goes through a different allocator wrapper already established in this file, and follow that established pattern rather than the raw `std::alloc` calls shown above if the file already has its own convention.)

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p pycc_rt int_list_new_starts_empty int_list_append_then_get_round_trips int_list_grows_past_its_initial_capacity int_list_get_out_of_range_panics_honestly int_list_incref_decref_round_trip_does_not_free_early
```

- [ ] **Step 5: Commit**

```bash
git add crates/pycc_rt/src/lib.rs
git commit -m "list[int] part 4: pycc_rt PyIntListObj runtime object (D-104)"
```

---

## Task 11: `list[int]` codegen

**Files:**
- Modify: `crates/pycc_codegen/src/lib.rs`
- Test: `tests/slice1_codegen_depth.rs` (or wherever this project's existing end-to-end compile+run tests live — confirm the exact file by checking PR-9's own Task 5 precedent, which added its `list`-adjacent tests to `tests/slice1_codegen_depth.rs`)

**Interfaces:**
- Consumes: `MirExpr::ListLiteral`/`Subscript`/`ListAppend`, `MirStmt::ForList` (Task 9); `pycc_rt`'s `PyIntListObj` extern "C" functions (Task 10); Task 5's already-updated `ty_to_basic_type` (`Ty::List(_) => ptr type`).
- Produces: a real, compiled, runnable `list[int]` — literal construction, `.append()`, indexed read, `len()`, and `for`-iteration, all lowering to calls into Task 10's runtime functions.

- [ ] **Step 1: Write the failing end-to-end test**

Add to `tests/slice1_codegen_depth.rs`, following this file's own existing compile-and-run test pattern (build the program, run the produced binary, assert its stdout):

```rust
#[test]
fn list_int_literal_append_index_len_and_iteration_all_work() {
    let source = r#"
def _run():
    x = [10, 20, 30]
    x.append(40)
    print(len(x))
    print(x[0])
    print(x[3])
    for v in x:
        print(v)

_run()
"#;
    let output = compile_and_run(source);
    assert_eq!(output, "4\n10\n40\n10\n20\n30\n40\n");
}
```

(Match `compile_and_run`'s exact existing signature/helper from this file — this is the same end-to-end pattern PR-9's Task 5 used for its `pep_0526_var_annotations_smoke.py`-adjacent tests.)

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test --test slice1_codegen_depth list_int_literal_append_index_len_and_iteration_all_work
```

Expected: FAIL — `HirExpr::Call { callee: "len", .. }` isn't recognized as a builtin yet, and `MirExpr::ListLiteral`/`Subscript`/`ListAppend`/`MirStmt::ForList` have no codegen arm yet (this will currently panic with the honest "not yet supported" messages Task 5 added, or simply fail to compile if codegen's match is exhaustive over `MirExpr`/`MirStmt` — confirm which by running it once before writing any new code).

- [ ] **Step 3: Add `"len"` as a recognized builtin call, alongside the existing `"print"` handling**

`pycc_codegen`'s existing call-dispatch (`rg -n '"print"' crates/pycc_codegen/src/lib.rs` to find the exact spot, e.g. line ~805) already special-cases `callee == "print"`. Add a parallel `callee == "len"` branch there: given one `list[int]`-typed argument, emit a call to `pycc_rt_int_list_len` and return its `i64` result (declare the extern function the same way this file already declares `pycc_rt_int_list_append`/etc. — follow the exact existing pattern used for declaring any other `pycc_rt_*` extern function in this file, e.g. `pycc_rt_str_concat`).

(`pycc_types`' own `"len"` type-checking arm — arity/`Ty::List(_)` validation, generic over element type, reusing `T0033` for both failure shapes — already landed in Task 8, alongside its commit implementing `T0032`/`T0033`/`T0034`. Don't go looking for it here or assume it still needs adding to `pycc_types`; this step is codegen-only.)

- [ ] **Step 4: Add codegen for `MirExpr::ListLiteral`**

Wherever `MirExpr` is matched for codegen (the function handling `MirExpr::BinOp`/`MirExpr::IntLiteral`/etc.), add:

```rust
        MirExpr::ListLiteral(elements) => {
            let list_ptr = self.build_call(self.pycc_rt_int_list_new_fn, &[], "list_new");
            for element in elements {
                let value = self.emit_expr(element, /* ...existing params... */);
                self.build_call(
                    self.pycc_rt_int_list_append_fn,
                    &[list_ptr.into(), value.into()],
                    "list_append",
                );
            }
            list_ptr
        }
```

- [ ] **Step 5: Add codegen for `MirExpr::Subscript`**

```rust
        MirExpr::Subscript { base, index } => {
            let base_ptr = self.emit_expr(base, /* ... */);
            let index_val = self.emit_expr(index, /* ... */);
            self.build_call(
                self.pycc_rt_int_list_get_fn,
                &[base_ptr.into(), index_val.into()],
                "list_get",
            )
        }
```

- [ ] **Step 6: Add codegen for `MirExpr::ListAppend`**

```rust
        MirExpr::ListAppend { list, value } => {
            let list_ptr = self.lookup_local(list, /* ...existing local-lookup mechanism... */);
            let value_val = self.emit_expr(value, /* ... */);
            self.build_call(
                self.pycc_rt_int_list_append_fn,
                &[list_ptr.into(), value_val.into()],
                "list_append",
            );
            /* .append() returns None -- return whatever this codegen's existing
               None-representation value is, matching how other None-returning
               calls (e.g. print) already produce their return value */
        }
```

- [ ] **Step 7: Add codegen for `MirStmt::ForList`, mirroring `MirStmt::ForRange`'s existing loop construction as its own parallel inline copy**

Locate `MirStmt::ForRange`'s codegen (`crates/pycc_codegen/src/lib.rs` around line 2055) and read its comments before touching anything: this codebase's own `ForRange` arm is a **deliberate inline copy** of `emit_body_then_branch`'s basic-block-building logic (see the comments around `crates/pycc_codegen/src/lib.rs` lines ~1226 and ~2135), not a call into a shared, reusable loop-building helper. **Do not factor this into a shared helper as part of this task.** Refactoring a 372-call-site file's existing, intentionally-duplicated control-flow logic is out of scope for a feature task and is exactly the kind of drive-by restructuring that draws review pushback on an already-large diff — add `ForList` as its own inline copy that follows the same structure `ForRange` actually uses (loop-header/loop-body/loop-exit basic blocks, `br`/`phi` wiring, etc., copied verbatim from `ForRange`'s real arm), parametrized by two differences: the loop bound comes from a runtime call to `pycc_rt_int_list_len` instead of a static/computed integer bound, and each iteration prepends one indexed read (`pycc_rt_int_list_get(list_ptr, i)`, storing the result into `var`'s local slot) before emitting the user's own loop body. Read `ForRange`'s exact current code directly — the sketch below shows the shape of what's needed, not verbatim code to paste, since the real basic-block-building calls must come from what `ForRange`'s arm actually does today:

```rust
        MirStmt::ForList { var, list, body } => {
            let list_ptr = self.lookup_local(list, /* ...existing local-lookup mechanism, matching ForRange's own... */);
            let len = self.build_call(self.pycc_rt_int_list_len_fn, &[list_ptr.into()], "list_len");
            // From here down, copy ForRange's own basic-block/loop-building
            // code verbatim (loop-header/loop-body/loop-exit blocks, phi
            // node, increment, branch-back), substituting `len` as the
            // runtime-computed upper bound in place of ForRange's static
            // one, and inserting the indexed read below as the first thing
            // in the loop body.
            let element = self.build_call(
                self.pycc_rt_int_list_get_fn,
                &[list_ptr.into(), /* current index value, from the copied loop induction variable */.into()],
                "list_get",
            );
            self.store_local(var, element);
            for stmt in body {
                self.emit_stmt(stmt, /* ... */);
            }
            // ... loop increment/branch-back/exit, copied from ForRange's own arm ...
        }
```

Add a one-line comment at the top of the new `ForList` arm noting it is an intentional inline duplicate of `ForRange`'s loop-building logic, matching that arm's own existing comment convention — and record the duplication between the two arms as a follow-up in this plan's closing "Follow-ups intentionally out of this plan's scope" list (a shared loop-building helper is a reasonable future refactor once a third consumer needs the same shape, not a change to make inside this task).

- [ ] **Step 8: Run the test to verify it passes**

```bash
cargo test --test slice1_codegen_depth list_int_literal_append_index_len_and_iteration_all_work
cargo build --workspace
cargo test --workspace
```

- [ ] **Step 9: Commit**

```bash
git add crates/pycc_codegen/src/lib.rs tests/slice1_codegen_depth.rs
git commit -m "list[int] part 5: pycc_codegen for literal/append/index/len/for-list, real end-to-end run (D-104)"
```

---

## Task 12: PEP-585 conformance fixture + docs sweep

**Files:**
- Create: `tests/fixtures/pep_0585_builtin_generics.py`
- Modify: `tests/conformance.rs` (one new dual-profile test)
- Modify: `docs/PYTHON_STANDARDS.md` (PEP 585 row: flip status, correct scope wording)
- Modify: `docs/ROADMAP.md` (conformance-count note)
- Modify: `docs/RUNTIME.md` (correct the header-shape inconsistency per D-104)
- Modify: `docs/TYPE_SYSTEM.md` (note v0.2's narrower-than-full-PEP-585 scope, if the existing "Generics" section reads as already-complete)
- Modify: `docs/DELIVERY_PLAN.md` (PR-10 row status)

**Interfaces:**
- Consumes: Task 11's working `list[int]` end-to-end.

- [ ] **Step 1: Write the fixture**

Create `tests/fixtures/pep_0585_builtin_generics.py`, keeping every `list[int]` value inside a private helper (D-104's own annotation-scope cut — no public function may have a `list[T]`-annotated parameter or return type in v0.2):

```python
def _demo():
    x = [10, 20, 30]
    x.append(40)
    print(len(x))
    print(x[0])
    print(x[3])
    for v in x:
        print(v)

_demo()
```

- [ ] **Step 2: Verify the fixture runs correctly against CPython first**

```bash
python3.14 tests/fixtures/pep_0585_builtin_generics.py
```

Expected output: `4`, `10`, `40`, `10`, `20`, `30`, `40` (one per line) — confirm this matches before wiring the pycc side, per this project's own "verify empirically" convention.

- [ ] **Step 3: Write the failing conformance test**

Add to `tests/conformance.rs`, copying the exact pattern of `pep_0526_var_annotations_matches_cpython_3_14_6_byte_for_byte` (PR-9's own precedent, the dual-profile pattern):

```rust
#[test]
#[ignore = "requires a pinned python3.14 (CPython 3.14.6) oracle on PATH"]
fn pep_0585_builtin_generics_matches_cpython_3_14_6_byte_for_byte() {
    let py_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/pep_0585_builtin_generics.py");
    let (pycc_debug, cpython) = run_conformance_fixture_with_profile("pep_0585", &py_path, false);
    assert_eq!(pycc_debug, cpython);
    let (pycc_release, cpython) = run_conformance_fixture_with_profile("pep_0585", &py_path, true);
    assert_eq!(pycc_release, cpython);
}
```

- [ ] **Step 4: Run the test to verify it fails, then passes**

```bash
cargo test --test conformance pep_0585_builtin_generics --include-ignored
```

Expected: FAIL before this task's own fixture/test exist, PASS once written (this test doesn't depend on any code from Tasks 2-11 that isn't already committed by this point in the plan).

- [ ] **Step 5: Update `docs/PYTHON_STANDARDS.md`'s PEP 585 row**

Current row (line ~155): `| [585](https://peps.python.org/pep-0585/) | Builtin generics \`list[int]\` | typing | \`py39/pep_0585_builtin_generics.py\` | ☐ |`

Change to (fixing the stale `py39/` path per D-102, and keeping the Feature-column wording already scoped to `list[int]` specifically — it was never mis-scoped like PEP 526's row was, since it already says `list[int]`, not "builtin generics" unqualified):

```markdown
| [585](https://peps.python.org/pep-0585/) | Builtin generics `list[int]` (v0.2 scope: `list[int]` only — `dict`/`set`/`tuple` generics are PR-11's own PEP-585 coverage, D-104) | typing | `pep_0585_builtin_generics.py` | ✅ |
```

- [ ] **Step 6: Update `docs/ROADMAP.md`'s conformance-count note**

Update the D-088 acceptance-bullet annotation (the same one PR-9's Task 3 updated) to record: 10 of the required ≥15 rows are now green (up from 9 after PR-9), still zero margin per the design doc's own "zero-margin" warning — restate it.

- [ ] **Step 7: Fix `docs/RUNTIME.md`'s header-shape inconsistency**

Find the line describing a generic 16-byte heap-object header (`rc`/`type_id`/`flags`) and the `list[T]` line. Correct both to describe what's actually implemented: no project-wide generic header exists; each heap object (`PyStrObj`, now `PyIntListObj`) defines its own header inline, and both existing ones use just `rc: Cell<u32>` plus their own payload fields — no `type_id`/`flags` anywhere in the actual runtime. Cross-reference D-104 for the decision record.

- [ ] **Step 8: Check `docs/TYPE_SYSTEM.md`'s "Generics" section for scope drift**

Read the "Generics" section (line ~63-68). If its wording ("Monomorphization (Rust model): `list[int]` and `list[str]` are distinct compiled types") could be read as already-true for every element type, add a one-line note that v0.2 has only landed `list[int]`'s own compiled path (D-104); the full monomorphization model described there remains the v1.0 target.

- [ ] **Step 9: Update `docs/DELIVERY_PLAN.md`'s PR-10 row**

Mark PR-10 delivered, following the exact same per-PR status-marking convention PR-6/PR-7/PR-8/PR-9's own rows already use.

- [ ] **Step 10: Explicitly verify `docs/TESTING.md` and `docs/SPEC.md` need no changes**

Per AGENTS.md's "if a code change genuinely has no documentation impact, explicitly verify that conclusion rather than skipping the docs review by default": read `docs/TESTING.md`'s conformance-harness section and confirm it still accurately describes `tests/conformance.rs`'s shape after this PR (it should — this PR adds one more dual-profile fixture test through the exact same `run_conformance_fixture_with_profile` helper D-102 already documented there, no new harness shape). Read `docs/SPEC.md`'s index and confirm no new crate, no renamed/added/removed spec document, and no changed cross-reference resulted from this PR (it didn't — `pycc_rt` and `pycc_codegen` both already existed and are already listed). If either file's description has actually drifted, fix it now rather than leaving this step's own conclusion undocumented; if not, no commit is needed for this step, but do not skip actually reading both files first.

- [ ] **Step 11: Commit**

```bash
git add tests/fixtures/pep_0585_builtin_generics.py tests/conformance.rs docs/PYTHON_STANDARDS.md docs/ROADMAP.md docs/RUNTIME.md docs/TYPE_SYSTEM.md docs/DELIVERY_PLAN.md
git commit -m "PEP-585 (list[int]) conformance fixture + docs sweep (D-104)"
```

---

## Task 13: Open the PR, wait for CI, run the pinned reviewer, merge

**Files:** none new; process only.

- [ ] **Step 1: Push and open the PR**

```bash
git push -u origin feat/v0-2-pr10-ty-representation-migration
gh pr create --title "v0.2 PR-10: Ty representation migration + monomorphization foundation + list[int] thin slice" --body "..."
```

Wait for the full CI matrix to go green on all 5 Tier-1 targets (`build-test-coverage`, all four `native-build-test` legs, both `cross-compile-*` jobs, `frontend-perf-measure`/`frontend-perf-gate`, `ci-gate`, `audit`) before proceeding. If `frontend-perf-gate` fails, check the actual reported delta and whether any Rust source changed since the last commit that passed it cleanly before assuming noise (per this project's own established methodology, D-095/D-096/D-101 and this same session's own PR-9 precedent) — trigger a full `gh run rerun` for an independent measurement rather than dismissing a first failure outright.

- [ ] **Step 2: Run the pinned local reviewer**

Per D-068/`docs/AGENT_TOOLING.md`: dispatch the pinned `ievo:deep-reviewer` against the full `merge-base(origin/main)..HEAD` diff (not a two-dot diff against `main`'s current tip — refresh `origin/main` first and compute the actual merge-base). Address every actionable finding before merge, re-reviewing scoped fixes as needed. Pay particular attention to whether it finds any additional non-exhaustive `Ty`/`HirExpr`/`MirExpr` dispatch site Task 5's own audit missed — that crate's catch-all-match risk is this plan's own single highest-risk area.

- [ ] **Step 3: Merge once required checks are green and review is clean**

Follow this project's own established merge gate and squash-merge convention (matching PR-6 through PR-9's own precedent).

---

## Follow-ups intentionally out of this plan's scope (record if not already tracked)

- `dict[K,V]`/`set[T]`/`tuple[...]` codegen and their own conformance fixtures — PR-11's scope per `docs/DELIVERY_PLAN.md`.
- `list[str]`/`list[float]`/`list[bool]`/nested `list[list[T]]` codegen — deliberately deferred by D-104; each is its own later PR's scope once `list[int]`'s pattern is proven.
- `list[T]` annotation syntax (`x: list[int] = [...]`, typed public function parameters/returns) — deliberately deferred by D-104.
- Subscript-assignment (`x[0] = value`) — deliberately deferred by D-104.
- General attribute/method-call dispatch beyond `.append()` — deliberately deferred by D-104.
- A real iterator protocol (`__iter__`/`__next__`) beyond `ForList`'s index-counted desugaring — deliberately deferred by D-104.
- The generic function type-parameter `Ty` placeholder — deliberately deferred to PR-13 by D-103.
- A shared loop-building helper factoring out the basic-block construction now duplicated between `MirStmt::ForRange` and `MirStmt::ForList` (Task 11 Step 7) — worth doing once a third consumer needs the same counted-loop shape, deliberately not attempted inside this plan to avoid an unrelated refactor of `pycc_codegen`'s existing, intentionally-inlined `ForRange` logic during a feature task.
