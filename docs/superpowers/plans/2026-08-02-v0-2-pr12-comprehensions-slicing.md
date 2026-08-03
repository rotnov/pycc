# PR-12: Comprehensions, Slicing, Container Methods Depth, PEP-709 Fixture — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close `docs/DELIVERY_PLAN.md` row 12: list/dict/set comprehensions, `list[int]` slicing (`xs[a:b:c]`), one new depth-method per mutable container (`list.pop()`, `dict.get(key, default)`, `set.add(value)`), and the PEP-709 conformance fixture — on top of PR-10's `list[int]` thin slice and PR-11a/PR-11b's `dict[str, int]`/`set[int]`/`tuple[...]`.

**Architecture:** Every new capability in this PR operates on the four `Ty` container variants `Ty::List`/`Ty::Dict`/`Ty::Set`/`Ty::Tuple` that already exist (D-089); this PR mints **zero new `Ty` variants and zero new `pycc_codegen::Scalar` variants** — comprehensions and slicing both *produce* an already-representable container type (`Ty::List(Int)`, `Ty::Dict(Str,Int)`, `Ty::Set(Int)`), so `Scalar::List`/`Dict`/`Set` are reused as-is. Two real architectural findings drive the two riskiest design forks below:

1. **`pycc_codegen::emit_expr` never builds a basic block; only `emit_stmt` does** (confirmed: every `append_basic_block`/`build_conditional_branch` call site in `crates/pycc_codegen/src/lib.rs` is inside `emit_stmt`'s `If`/`While`/`ForRange`/`ForList`/`ForDict`/`ForSet` arms — there is no `BoolOp`/short-circuit `and`/`or` anywhere in this compiler, which would be the other place expression-position control flow could already exist). A comprehension needs a loop to run before it produces a value, so it cannot be a general `HirExpr`/`MirExpr` usable in arbitrary nested positions without inventing expression-position control flow this codebase has never needed. This PR instead makes each comprehension kind a **statement**, restricted to being the direct value of a bare-name `Stmt::Assign` (`xs = [elt for var in iter]`), exactly mirroring how `for`/`while`/`if` are already statements, not expressions, in this compiler.
2. **This compiler's storage-slot model is a single flat, name-keyed map per scope** (`collect_module_bindings`/`collect_stmt_bindings` in `pycc_codegen`, `Environment` in `pycc_types`) with no lexical shadowing anywhere — confirmed by `MirStmt::ForRange`'s loop variable sharing the *same* slot as any pre-existing binding of that name (this is why an ordinary `for i in range(3):` after `i = 100` already leaks/overwrites `i` in both CPython and today's pycc, correctly). A comprehension's own loop variable must **not** leak into an enclosing binding of the same name — that is precisely the behavior PEP 709 depends on and precisely what this PR's own conformance fixture (design doc §2, PEP 709 row) needs to demonstrate. Rather than inventing real lexical scoping, this plan synthesizes a **collision-proof internal name** for each comprehension's loop variable (a name beginning with a digit, which can never be produced by lowering real Python source, since no valid Python identifier starts with one) and rewrites the comprehension's own `elt`/`cond`/`key`/`value` sub-expressions to reference that synthesized name instead of the source spelling. This needs no new scoping machinery — it is just another ordinary entry in the existing flat namespace, guaranteed distinct from every real binding by construction.

**Tech Stack:** Same workspace as PR-10/PR-11 — `pycc_ast`/`pycc_hir`/`pycc_types`/`pycc_mir`/`pycc_codegen`/`pycc_rt`, `inkwell = "0.9.0"` (LLVM 22.1), `ruff_python_ast = "0.0.6"`/`ruff_python_parser = "0.0.6"`. `ruff_python_ast`'s `ExprListComp`/`ExprSetComp`/`ExprDictComp`/`Comprehension` (verified directly against the installed `ruff_python_ast-0.0.6` registry source, `src/generated.rs:9707-9744` and `src/nodes.rs:2788`) are already produced by the parser today — `pycc_ast`'s own `expr_range` helper (`crates/pycc_ast/src/lib.rs:54-56`) already matches all three comprehension expression kinds (for span purposes only; no lowering exists yet), confirming these currently fall through to `pycc_hir`'s generic "expression kind not supported yet" `C0001` catch-all.

## Global Constraints

- **Branch:** this plan executes on `feat/v0-2-pr12-comprehensions-slicing`, a new branch/worktree taken from the tip of `feat/v0-2-pr11-dict-set-tuple` (last commit `29e9653`, "Update SESSION_LOG: PR-11b... complete") per D-021 — PR-11 is content-complete but still unmerged to `main` (blocked on the unrelated frontend-perf-gate/issue #109 governance question, D-109's own correction), so this plan continues stacking rather than waiting. Re-run `git status --short --branch` and `git log --oneline -5` before starting to confirm this is still the correct base; do not rebase, merge, or switch branches without the task authorizing it.
- **D-014 coverage gate:** 100% line + region coverage (`cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`) is a hard merge invariant. Every new branch — including every honest-panic arm this plan adds to `pycc_rt`/`pycc_codegen` — needs a test that actually executes it.
- **No new `Ty` variant, no new `Scalar` variant (state this explicitly, it is a deliberate, notable outcome of this plan, not an oversight):** every container this PR touches already has a `Ty`/`Scalar` representation from D-089/D-105/D-121/D-115. Comprehensions produce `Ty::List(Int)`/`Ty::Dict(Str,Int)`/`Ty::Set(Int)` (the same three combinations D-105/D-122 already ship codegen for); slicing a `list[int]` produces another `list[int]`; the three new methods each operate on and (for `.pop()`/`.get()`) return already-representable scalar/container types.
- **Verify the live highest D-number and T-code before minting either (do not trust this plan's own numbers blindly):**
  ```bash
  grep -oE '^## D-[0-9]+' docs/DECISIONS.md | sed 's/## D-//' | sort -n | tail -3
  grep -oE '"T00[0-9]+"' crates/pycc_types/src/lib.rs | sort -u | tail -3
  ```
  Expected at plan-writing time: `116` is the highest `D-` number, `"T0040"` is the highest code. This plan mints `D-117`–`D-120` and **zero new `T0xxx` codes** (see the diagnostics-reuse rationale in Task 1's D-119). If the grep output differs, stop and use the actually-next-free `D-` numbers instead of `117`–`120`.
- **Diagnostics reuse, not proliferation:** every new rejection path in this plan reuses an existing code — `T0033` (base type doesn't support the operation: slicing, `.pop()`, `.get()`, `.add()`, arity mismatches — extend its `docs/DIAGNOSTICS.md` description, Task 12), `T0021` (operand/argument type mismatch — slice bounds, `.get()`'s default value, `.add()`'s value), and `T0034`/`T0036`/`T0038` (a comprehension's own produced element/key/value type gate — identical rule to a literal's, just reached via a new code path). Every comprehension **shape** restriction (more than one `for` clause, more than one `if` filter, a non-bare-name target, an unsupported iterable shape, `async for`) is a structural HIR-lowering restriction and reuses the existing generic `C0001` (`unsupported(...)`) path, exactly like `Stmt::For`'s own existing restrictions — not a new diagnostic family.
- **New `pycc_rt` functions needed: exactly three.** `pycc_rt_int_list_slice` (slicing), `pycc_rt_int_list_pop` (`.pop()`), `pycc_rt_dict_get_or_default` (`.get(key, default)`). `set.add()` needs **zero** new `pycc_rt` code: `pycc_rt_int_set_add` already exists (`crates/pycc_rt/src/lib.rs:1464`, built for `SetLiteral`'s own dedup-on-construction codegen) and this plan only adds a new HIR/MIR/codegen path that calls it from a second, user-facing call site. Comprehensions likewise need **zero** new `pycc_rt` code: they desugar to the existing `pycc_rt_int_list_new`+`_append` / `pycc_rt_dict_new`+`_set` / `pycc_rt_int_set_new`+`_add` pairs PR-10/PR-11a already ship and test.
- **Comprehension scope, precisely:** exactly one `for` clause (`generators.len() == 1`), at most one `if` filter (`generators[0].ifs.len() <= 1`), a bare-name loop target (mirrors `Stmt::For`'s own existing restriction), no `async for`, and an iterable that is `range(...)` **or** a bare name resolved downstream to `Ty::List`/`Ty::Dict`/`Ty::Set` — this is not a new, narrower restriction invented for comprehensions: it is `HirStmt::ForList`'s own existing iterable-shape acceptance, reused verbatim (Task 2 factors out and reuses `Stmt::For`'s own range-argument-parsing code rather than duplicating it). A comprehension is only lowered when it is the **direct RHS of a bare-name `Stmt::Assign`** (`xs = [...]`); anywhere else a comprehension expression appears (a function argument, a nested expression, `return [...]`, `Stmt::AnnAssign`'s RHS since no container annotation syntax exists per D-105 cut 1 anyway), it falls through unchanged to `lower_expr`'s existing generic-expression `C0001` catch-all — a real, deliberate scope cut, not an oversight (recorded in D-117, Task 1).
- **Slicing scope, precisely:** `list[int]` only (`dict`/`set` are not slice-subscriptable in real Python either — `d[1:2]` and `s[1:2]` both raise `TypeError` in CPython, so rejecting them is correct behavior, not a scope cut; `tuple[...]` slicing is a genuine, real deferral, tracked as a `docs/ROADMAP.md` follow-up, Task 13). `start`/`stop` are each optional, arbitrary runtime `int`-typed expressions (not literal-only, unlike D-116's tuple-index rule — a slice's *result type* never depends on the bound values, unlike a tuple subscript's element type, so there is no static-soundness reason to require a literal here); defaulting to `0`/`len(list)` when omitted, and **clamped** into `[0, len]` after a non-negative check — clamping is required, not a nicety, or the accepted subset (which must support common omitted-bound forms like `xs[:3]`/`xs[2:]`) cannot match CPython's own out-of-range-slice-bound behavior byte-for-byte. `step` is optional (defaults to `1`) and must be a positive runtime `int`; a negative `start`/`stop`/non-positive `step` is an honest `pycc_rt` panic, extending D-108's own uniform "index sign is a runtime check, not a compile-time literal check" precedent to slicing rather than adding a redundant literal-vs-runtime split.
- **Container-methods scope, precisely:** `list.pop()` (no-argument form only — removes and returns the last element, panics if empty), `dict.get(key, default)` (exactly two arguments — no zero/one-argument form, which would need an `Optional[int]`/`None`-union return type this compiler has no representation for), `set.add(value)` (mirrors `.append()`'s hand-recognized shape, dedups exactly like literal construction already does). `tuple[...]` gets **no** new method in this plan (real Python tuples have no mutating methods; `.count()`/`.index()` aren't requested and are deferred without comment). **Explicitly deferred, re-affirming D-123's own reasoning rather than silently reopening it:** `set[int]`'s membership test (`in`) — still blocked on the general `in`/comparison-operator project-wide feature (`pycc_hir` routes every `CmpOp::In`/`NotIn` through the same generic `C0001` used for `is`/`is not`; adding real `in` support is a language-wide comparison-operator feature affecting `str`/`list` membership too, not a "container method," and bundling it here would break this PR's own thin-slice scope the same way D-123 already reasoned when first deferring it). Also deferred, not attempted: `list.extend()`/`.insert()`/`.remove()`/`.sort()`, `dict.keys()`/`.values()`/`.items()` (`.keys()` is redundant with the already-shipped `for k in d:`; `.values()`/`.items()` need new iteration forms), `set` union/intersection/etc., and real (non-leak-only) refcounting for any container (D-107/D-124's own already-tracked follow-up, unaffected by this PR).
- **`container.to_str()`/`truthy()` stay out of scope, explicitly reaffirmed, not implied by any fixture in this plan.** `print(a_list)` / `f"{a_dict}"` / `if a_set:` all still reach the honest `panic!(...)` arms D-107/D-124/D-116 already ship (`crates/pycc_codegen/src/lib.rs`'s `to_str`/`truthy` functions) — real CPython `repr()`-formatting (`[0, 2, 4]`'s exact spacing, `{'a': 1}`'s exact quoting) is not built by this plan. Every new fixture in this plan (Task 13) prints container contents **element-wise** (`for v in xs: print(v)`), never a container value directly — verified against real fixtures already shipped: neither `dict_order.py` nor `pep_0585_set_int.py` nor `tuple_heterogeneous.py` ever calls `print()` on a container value either.
- **Zero `.github/workflows/ci.yml` changes.**

---

## Task 1: Record D-117–D-120 ADRs

**Files:**
- Modify: `docs/DECISIONS.md`

- [ ] **Step 1: Verify the live highest D-number and T-code**

  Run the two `grep` commands from the Global Constraints section above. Stop and adjust the numbers below if the output differs from `116`/`T0040`.

- [ ] **Step 2: Add the four summary-table rows**

  Immediately after the existing `| D-116 | ... |` row, before the blank line and `## Template`:

  ```markdown
  | D-117 | List/dict/set comprehensions are statement-level (`Stmt::Assign`-RHS-only), reusing `ForList`'s existing iterable polymorphism; the loop variable is rewritten to a synthesized, collision-proof internal name so it cannot leak into an enclosing same-named binding (PEP 709's own observable guarantee) | accepted |
  | D-118 | `list[int]` slicing (`xs[a:b:c]`) ships with non-negative runtime bounds, CPython-matching clamping, and a positive step; negative bounds/non-positive step are honest runtime panics, extending D-108's precedent; `dict`/`set`/`tuple` slicing stay unsupported (the first two correctly, per real Python's own `TypeError`; `tuple` as a genuine deferral) | accepted |
  | D-119 | `list.pop()`, `dict.get(key, default)`, `set.add(value)` ship as v0.2's "remaining container methods depth"; `tuple` gets none; `in`-based set membership stays deferred (re-affirms D-123); every new rejection path reuses an existing `T0021`/`T0033`/`T0034`/`T0036`/`T0038` diagnostic — zero new `T0xxx` codes | accepted |
  | D-120 | The PEP-709 conformance fixture demonstrates the one CPython-observable, statically-testable guarantee PEP 709 depends on (a comprehension's own loop variable does not leak into an enclosing same-named binding) rather than literal bytecode-inlining, which has no analog in pycc's architecture; container `to_str`/`truthy` stay explicitly out of this PR's scope, so the fixture prints element-wise | accepted |
  ```

- [ ] **Step 3: Add the full D-117 section**

  Insert after D-116's own full section (after its final "...deferred to whichever future PR widens the element-type gate to include str." — locate via `grep -n "## D-116" docs/DECISIONS.md` then read to the next `## D-` heading to find the exact insertion point, since D-116 has accumulated two "Correction" appendices since it was first written):

  ```markdown
  ## D-117: List/dict/set comprehensions are statement-level, `Stmt::Assign`-RHS-only, with a synthesized loop-variable name

  - Status: accepted
  - Context: PR-12 (`docs/DELIVERY_PLAN.md` row 12) must add list/dict/set comprehensions. Two architectural facts, verified directly against the live tree rather than assumed, constrain the design: (1) `crates/pycc_codegen/src/lib.rs`'s `emit_expr` never calls `append_basic_block`/`build_conditional_branch` anywhere -- every such call in this crate is inside `emit_stmt`'s `If`/`While`/`ForRange`/`ForList`/`ForDict`/`ForSet` arms, and this compiler has no `BoolOp`/short-circuit `and`/`or` (which would be the other place expression-position control flow could already exist) -- so a comprehension, which needs a loop to run before it produces a value, cannot be a general `HirExpr`/`MirExpr` without inventing expression-position control flow this codebase has never had. (2) This compiler's storage model is a single flat, name-keyed slot map per scope (`collect_module_bindings`/`collect_stmt_bindings` in `pycc_codegen`, `Environment` in `pycc_types`), confirmed by `MirStmt::ForRange`'s loop variable sharing the *same* slot as any pre-existing same-named binding (`crates/pycc_codegen/src/lib.rs`'s `collect_stmt_bindings`, `ForRange`/`ForList` arms both call `bindings.entry(var.clone()).or_insert(...)` against the one flat map) -- this is why an ordinary `for i in range(3):` after `i = 100` already leaks/overwrites `i` in both CPython and today's pycc, correctly. A comprehension's own loop variable must *not* leak (real Python has scoped comprehensions since their introduction; PEP 709's own inlining depends on that scoping being preserved), which this flat model cannot express without either real lexical shadowing (a mechanism this compiler has never needed) or some other trick.
  - Decision: two-part.
    1. **Statement-level, not expression-level.** Each comprehension kind gets its own `HirStmt`/`MirStmt` variant (`ListCompAssign`/`DictCompAssign`/`SetCompAssign`), lowered only when a comprehension expression (`Expr::ListComp`/`SetComp`/`DictComp`) is the direct RHS of a bare-name `Stmt::Assign` (`xs = [elt for var in iter [if cond]]`). Anywhere else a comprehension expression appears syntactically (a function argument, a nested sub-expression, `return [...]`, an `AnnAssign` RHS), `pycc_hir`'s `lower_expr` has no arm for it and it falls through to that function's existing generic "expression kind not supported yet" `C0001` catch-all -- the same fate as every other expression kind this compiler doesn't yet lower, not a new failure mode invented for this decision. Scoped, per generator clause: exactly one `for` (`generators.len() == 1`), at most one `if` filter, a bare-name loop target, no `async for` -- and the iterable is `range(...)` or a bare name, reusing `HirStmt::ForList`'s own existing iterable-shape acceptance (resolved to `Ty::List`/`Ty::Dict`/`Ty::Set` downstream, exactly like a plain `for` loop) rather than inventing a narrower, comprehension-specific gate. `pycc_codegen`'s `emit_stmt` builds the actual loop for each new `MirStmt` variant by reusing the identical basic-block-building shape `ForRange`/`ForList`/`ForDict`/`ForSet` already use (test/body/after blocks), parametrized internally on which container/iterable combination applies -- mirroring the precedent PR-11b's own `MirExpr::Subscript` codegen already established (one MIR node, one codegen arm, branching internally on the resolved `base.ty()` rather than a combinatorial explosion of MIR variants per base-type combination).
    2. **Synthesized, collision-proof loop-variable name.** Before embedding the comprehension's `elt`/`cond`/`key`/`value` sub-expressions in the returned `HirStmt`, every occurrence of the source-level loop-variable name is rewritten to `format!("0comp_{}_{source_name}", target_start_offset)` -- a name beginning with a digit, which can never begin a valid Python identifier (confirmed against `ruff_python_parser`'s own tokenizer: a `NAME` token cannot start with a decimal digit), so this string can never be produced by lowering real Python source, regardless of what the user names their own variables. `target_start_offset` is the comprehension's own `for`-target `Expr::Name`'s byte-offset (`TextRange::start()`), not a mutable counter -- two syntactically distinct comprehensions in one file can never share a target's start offset, so this needs no threaded lowering state, no global/thread-local counter, and stays fully deterministic across repeated compiles of the same source. The synthesized name is then just another ordinary entry in the existing flat namespace (bound via the same `bind_variable`/`check_assignment`/`collect_stmt_bindings` machinery every other loop variable already uses) -- it stays "bound" after the comprehension completes, exactly like every other loop variable already does today (`for i in range(3): pass` leaves `i` bound afterward too), which is harmless here specifically because no real Python source can ever name it to observe that fact.
  - Alternatives: build expression-position control flow in `emit_expr` so a comprehension can appear in any expression position (rejected for v0.2 -- this is a materially larger, riskier change to this crate's core codegen architecture for a capability this PR's own row in `docs/DELIVERY_PLAN.md` doesn't ask for; tracked as a `docs/ROADMAP.md` follow-up, Task 13). Reject any comprehension whose loop-variable name collides with an existing enclosing binding, sidestepping the scoping question entirely (rejected -- this is exactly the one case PEP 709's own fixture needs to demonstrate compiling correctly; rejecting it would make the PEP-709 fixture (D-120) impossible to write meaningfully). Build real lexical shadowing (a scope stack keyed by more than name alone) (rejected as strictly more machinery than this PR's own narrow need justifies -- the synthesized-name trick fully solves the one case that matters, iteration variables, with zero changes to the existing flat-namespace model; real lexical scoping is a larger investment better justified once a feature that actually needs nested nameable nested scopes -- e.g. v0.3's classes, or nested function defs -- arrives).
  - Consequences: `pycc_hir` gains a small, exhaustive `rename_name_in_expr` rewrite helper (Task 2) that every future `HirExpr` variant must add an arm to (compiler-enforced, matching this project's own "let the compiler enumerate every site" precedent, e.g. D-107's `Scalar::List` variant argument). A future PR building real nested lexical scopes (e.g. for v0.3 classes or nested `def`s) supersedes this decision's synthesized-name trick for its own new scoping needs, but does not need to revisit comprehensions' own use of it, since the synthesized name remains collision-proof regardless of what other scoping mechanism coexists with it later. **A second, independent consequence, found during this plan's own review and not obvious from the synthesized-name mechanism alone:** a `Dict`-sourced comprehension's per-iteration loop-variable binding (`var: Ty::Str`) must reproduce `MirStmt::ForDict`'s own existing `pycc_rt_str_incref` call on the read key before storing it (`crates/pycc_codegen/src/lib.rs:4179-4181`), and must not decref it on the next iteration's rebind, exactly like `ForDict` already does not -- this is a real `str`-refcounting requirement (D-060), independent of whether the comprehension's own container refcounting is leak-only (D-107/D-124), and independent of D-117's own synthesized-name mechanism; getting it wrong risks corrupting the *source* dict's own key storage, not merely a cosmetic gap. `DictCompAssign` additionally needs its own `key` field passed through `incref_if_str_duplicate` before `pycc_rt_dict_set`, exactly mirroring `MirStmt::DictSet`'s own existing call (`crates/pycc_codegen/src/lib.rs:4072`) -- both fixes are Task 5b's own scope, not Task 5a's, since only a `Dict`-producing comprehension can ever have a `Ty::Str`-typed field that gets stored persistently into a new container in this PR's own accepted-type scope (D-122 makes `dict[str, int]` the only shipped combination whose *key* is `str`; no shipped `list`/`set` element type is ever `str`).

  ## D-118: `list[int]` slicing ships with non-negative clamped bounds and a positive step; `dict`/`set`/`tuple` slicing stay unsupported

  - Status: accepted
  - Context: `docs/DELIVERY_PLAN.md` row 12 and the design doc's own PR-12 breakdown name `xs[a:b:c]` slicing without specifying its exact supported subset. D-108 already established that `list[int]`'s plain index read (`x[i]`) rejects any negative index with a runtime panic rather than CPython's own last-element addressing, uniformly for both literal and runtime-computed indices ("`pycc_types` has no way to reject `lst[-1]` at compile time... so this panics on *any* negative index"). Real Python slicing additionally requires supporting *omitted* bounds (`xs[:3]`, `xs[2:]`, `xs[:]`) and out-of-range bounds being silently clamped rather than raising (`[1,2,3][0:100] == [1,2,3]` in real CPython) -- both are common, ordinary uses this PR's own fixtures need to exercise, so slicing cannot simply inherit indexing's "panic on anything unusual" behavior wholesale without becoming useless for the common case.
  - Decision: `list[int]` is the only container this PR ships slicing for. `dict`/`set` are not slice-subscriptable in real Python either (`d[1:2]`/`s[1:2]` both raise `TypeError: unhashable type: 'slice'` in CPython, since a `slice` object is used as a dict/set key/lookup, not a range operation) -- rejecting them with the existing `T0033` ("does not support ... subscript") is the *correct* behavior, not a scope cut. `tuple[...]` slicing is a genuine deferral (real Python does support it, returning a new, differently-shaped tuple whose arity depends on the slice bounds -- D-115's fixed-arity-struct representation would need compile-time-known bounds to produce the right struct type, extending D-116's own literal-index precedent; tracked as a `docs/ROADMAP.md` follow-up, Task 13, not attempted here). For the shipped `list[int]` case: `start`/`stop` are each optional, arbitrary runtime `Ty::Int` expressions (not literal-only, unlike D-116's tuple-index rule -- a list slice's *result type* is always `list[int]` regardless of the bound values, so there is no static-soundness reason, unlike a heterogeneous tuple's per-position element type, to require a compile-time-known literal here), defaulting to `0`/`len(list)` when omitted. After a non-negative check (panicking on any negative bound, extending D-108's own uniform runtime-check precedent to slicing rather than adding a redundant separate literal-vs-runtime split), `start`/`stop` are clamped into `[0, len]` -- required, not optional, for the accepted subset to match CPython byte-for-byte on ordinary omitted/out-of-range bounds. `step` is optional (default `1`) and must be a positive runtime `Ty::Int`; a non-positive step is an honest panic (`"pycc_rt: slice step must be positive"`), a real, deliberate divergence from CPython's own negative-step-reverses/zero-step-`ValueError` semantics.
  - Alternatives: require a compile-time-literal `start`/`stop`/`step`, mirroring D-116's tuple-index rule (rejected -- unlike a tuple subscript, a list slice's result type never depends on the bound values, so there is no soundness argument for the restriction, and it would needlessly reject the overwhelmingly common runtime-variable-bound case, e.g. `xs[i:i+3]`). Support CPython's full negative-index/negative-step semantics (rejected -- extends far past this PR's own "thin slice" scope for a capability no fixture in this plan's own Task 13 needs, and would reopen D-108's already-settled "no CPython last-element addressing" scope cut for indexing too, for consistency, which is a strictly bigger, separate change). Reject out-of-range bounds with a panic instead of clamping (rejected outright -- this would make the accepted subset diverge from CPython on the ordinary, common case of an omitted or over-long bound, defeating the entire purpose of shipping the feature at all).
  - Consequences: `pycc_rt_int_list_slice` (Task 9) needs a real, executing test per branch for D-014's coverage gate: the ordinary in-range path, clamp-start-high, clamp-stop-high, an empty result (`start >= stop` after clamping), a negative-start panic, a negative-stop panic, and a non-positive-step panic -- seven distinct paths, none of which the conformance fixture (Task 13) exercises on its own (fixtures only prove the happy path matches CPython; the panic/clamp branches need their own in-crate tests). A future PR adding `tuple[...]` slicing revisits this decision's "tuple slicing stays unsupported" clause specifically, not its `list[int]` rules.

  ## D-119: `list.pop()`, `dict.get(key, default)`, `set.add(value)` are v0.2's "remaining container methods depth"; zero new diagnostic codes

  - Status: accepted
  - Context: `docs/DELIVERY_PLAN.md` row 12 asks for "remaining container methods depth across all four types" without naming which methods. D-105 shipped `list.append()`, D-123 shipped `dict`'s `d[k] = v` and explicitly deferred `set.add()`/`in`, D-116 shipped no tuple methods at all (real Python tuples have none worth adding here). `pycc_rt_int_set_add` (`crates/pycc_rt/src/lib.rs:1464`) already exists, built for `SetLiteral`'s own dedup-on-construction codegen, but has no user-facing (`HirExpr`/`MirExpr`) call site yet.
  - Decision: exactly one new growable/lookup operation per mutable container type. `list.pop()` -- no-argument form only, removes and returns the list's last element, panics if empty (honest-panic convention, matching CPython's own `IndexError`-on-empty-pop in spirit, though pycc has no catchable exceptions). `dict.get(key, default)` -- exactly two arguments; a zero- or one-argument `.get()` (returning `None` on a missing key) is not shipped, since this compiler has no `Optional[int]`/`None`-union representation for a `dict[str, int]`'s value type -- requiring the caller to always supply a same-typed default sidesteps that gap entirely rather than half-solving it. `set.add(value)` -- mirrors `.append()`'s existing hand-recognized shape exactly, calling the already-existing `pycc_rt_int_set_add` from a second call site. `tuple[...]` gets no new method (out of scope, not an oversight). `set[int]`'s membership test (`in`) stays deferred, re-affirming D-123's own reasoning rather than silently reopening it: `in` is a general comparison-operator feature (also relevant to `str`-in-`str`, future `list`/`dict` membership), not a "container method," and this compiler's `pycc_hir` still routes every `CmpOp::In`/`NotIn` through the generic `C0001` used for `is`/`is not` -- adding real support is a separate, differently-shaped PR. Every new rejection path in this PR reuses an existing diagnostic: `T0033` for a wrong base type or wrong argument count (mirroring `.append()`/`len()`'s own existing precedent exactly -- `docs/DIAGNOSTICS.md`'s `T0033` row is widened, Task 12, to list `.pop()`/`.get()`/`.add()`/slicing alongside `.append()`/`len()`), `T0021` for a wrong argument *type* (mirroring `.append()`'s own value-type-mismatch precedent). Zero new `T0xxx` codes are minted by this entire plan.
  - Alternatives: ship `.get(key)` (one-argument, panicking on a missing key, mirroring `d[k]`'s own existing semantics) instead of or alongside the two-argument form (rejected as redundant -- a one-argument panicking `.get()` behaves identically to `d[k]` already, so it adds a second spelling for the same operation without adding real capability; the two-argument default-returning form is the one CPython behavior `d[k]` cannot already express). Ship `set.add()`/`in` together, since D-123 named them as a pair (rejected -- `in` remains a separate, materially larger, general-language feature; bundling it here would break this PR's own "thin slice" framing the same way D-123 already reasoned once). Mint a dedicated new diagnostic per new method (e.g. a `T0041` for `.pop()`-on-empty at compile time) (rejected -- an empty-list-at-compile-time-known-statically case doesn't generally exist for a runtime-populated list, so this would be dead code; the real "wrong base type"/"wrong arg count" cases are structurally identical to `.append()`/`len()`'s own already-covered `T0033` case).
  - Consequences: `docs/DIAGNOSTICS.md`'s `T0033` row description is the one diagnostics-registry line this whole plan touches (Task 12); no other row changes. A future PR shipping `in` support project-wide should revisit `set[int]`'s own scope to add membership testing, per D-123's own already-recorded follow-up (unchanged by this decision).

  ## D-120: The PEP-709 fixture demonstrates loop-variable non-leakage, not bytecode inlining; container `to_str`/`truthy` stay explicitly out of scope

  - Status: accepted
  - Context: the design doc's own PEP table (§2) assigns PEP 709 ("Comprehension inlining semantics") to this PR without specifying what a pycc conformance fixture for it should actually assert -- CPython 3.12's real PEP 709 change is an *implementation* detail (comprehensions stop running in an implicit nested function/frame, for a performance win) that pycc, an AOT compiler with no bytecode, no frames, and no `locals()`, has no analog for; the design doc's own PEP-649/749 row already established the precedent that "not empirically reachable given this architecture" is a real, first-class finding to record rather than force a fixture that doesn't actually test anything. The one CPython-observable, PEP-709-relevant behavior that *is* both real and staticaly testable in pycc's current scope is that a comprehension's own loop variable does not leak into or clobber an enclosing binding of the same name -- true both before and after PEP 709 (comprehensions have had their own scope since their introduction; PEP 709 preserves this while changing how it's implemented), and, per D-117's synthesized-name mechanism, now genuinely exercised by this compiler for the first time (an ordinary `for` loop, by contrast, *does* leak/overwrite, matching CPython's own bare-`for` behavior, confirmed already true of `ForRange`/`ForList` before this PR). Separately: printing a container value directly (`print(a_list)`) still panics in `pycc_codegen`'s `to_str` (D-107/D-124/D-116's own already-shipped honest panics), so a fixture asserting comprehension output must print element-wise, never the container itself.
  - Decision: `tests/fixtures/pep_0709_comp_inline.py` asserts loop-variable non-leakage, not literal bytecode-inlining behavior:
    ```python
    i = 100
    xs = [i * 2 for i in range(3)]
    print(i)
    for v in xs:
        print(v)
    ```
    Real CPython prints `100` (the outer `i` survives untouched) followed by `0`, `2`, `4` (the comprehension's own result) -- if this compiler's comprehension lowering used the *source* loop-variable name directly instead of D-117's synthesized name, the shared flat-namespace slot model would make the outer `i` end up overwritten (to `2`, the loop's final value) exactly like a bare `for i in range(3):` already does, and this fixture would fail to match CPython. This is a real, non-trivial, CPython-verified assertion this compiler's own architecture makes genuinely easy to get wrong, not a token gesture at the PEP's name.
  - Alternatives: assert on `locals()`/frame-introspection differences (rejected outright -- neither exists in this compiler at all, and CPython's own PEP 709 change is precisely about *not* creating a frame, which pycc never did for comprehensions to begin with, since it never runs comprehensions as nested functions in the first place). Use the walrus operator (`:=`) inside the comprehension, since its interaction with enclosing scope is the single most distinctive real PEP-709-adjacent CPython behavior (rejected -- pycc has no assignment-expression support at all, in or out of a comprehension, so this is not reachable at any scope this compiler currently has). Print the resulting container directly instead of element-wise (rejected -- `to_str` on a container panics today by design (D-107/D-124/D-116); a fixture that reached that panic would fail to compile at all, not merely fail to match CPython).
  - Consequences: this decision also reaffirms, in one place, that container `to_str`/`truthy` remain unimplemented after this PR -- not a silent gap this plan's own fixtures could be mistaken for having closed. `docs/ROADMAP.md`'s v0.2 corpus acceptance bullet (PR-14's own scope) needs the same element-wise-printing discipline for the same reason, flagged there as a follow-up (Task 13).
  ```

- [ ] **Step 4: Confirm the table and full sections are both present and in the same relative order as every other `D-xxx` entry (table row first in file order, full section later, matching the existing convention)** by re-running the Step 1 `grep` and confirming it now reports `120`.

---

## Task 2: `pycc_hir` — Comprehension frontend (`HirStmt::ListCompAssign`/`DictCompAssign`/`SetCompAssign`, synthesized loop-variable rename)

**Files:**
- Modify: `crates/pycc_ast/src/lib.rs`, `crates/pycc_hir/src/lib.rs`

**Interfaces:**
- Produces: `HirStmt::ListCompAssign`/`DictCompAssign`/`SetCompAssign`, `pycc_hir::CompIter` (public — `pycc_mir` matches on it), `rename_name_in_expr` (crate-visible; may become a documented internal API if a later PR needs it).
- Consumes: `ruff_python_ast::{ExprListComp, ExprSetComp, ExprDictComp, Comprehension}`, re-exported through `pycc_ast` by this task (not already present there — see Step 0).

- [ ] **Step 0: Widen `pycc_ast`'s curated re-export list**

  `crates/pycc_ast/src/lib.rs`'s `pub use ruff_python_ast::{ ... };` block (line 1) is a **curated** facade, not a blanket re-export — confirmed directly: it currently lists `Arguments, CmpOp, ConversionFlag, ElifElseClause, Expr, ExprBinOp, ExprBooleanLiteral, ExprCall, ExprCompare, ExprContext, ExprFString, ExprName, ExprNumberLiteral, ExprStringLiteral, ExprUnaryOp, Identifier, InterpolatedElement, InterpolatedStringElement, InterpolatedStringLiteralElement, ModModule, Number, Operator, Parameters, Stmt, StmtAnnAssign, StmtAssign, StmtExpr, StmtFor, StmtFunctionDef, StmtIf, StmtReturn, StmtWhile, UnaryOp` — notably, `ExprList`/`ExprDict`/`ExprSet`/`ExprSubscript`/`ExprAttribute` are all *absent* too, yet `lower_expr` already matches `Expr::List(list)`/`Expr::Dict(dict)`/etc. without issue, because binding a variant's inner value via pattern matching needs no import of that inner struct's *name* — only a function signature that spells the type out explicitly (e.g. `fn f(x: &ExprListComp)`) does. This task's own helper functions (`lower_comprehension_header`, `lower_list_comp_assign`, `lower_set_comp_assign`, `lower_dict_comp_assign`) all take a `Comprehension`/`ExprListComp`/`ExprSetComp`/`ExprDictComp` by name as a parameter type, and `crates/pycc_hir/Cargo.toml` depends only on `pycc_ast`, never `ruff_python_ast` directly (confirmed) — so these four types must be added to `pycc_ast`'s re-export list first, or `pycc_hir` cannot name them at all. Add `Comprehension, ExprDictComp, ExprListComp, ExprSetComp` to the existing `pub use ruff_python_ast::{ ... };` list (alphabetized, matching the existing list's own ordering convention).

- [ ] **Step 1: Re-verify the current shape of `lower_stmt`'s `Stmt::Assign` arm and `Stmt::For`'s range-parsing block**

  ```bash
  grep -n "Stmt::Assign(assign)" crates/pycc_hir/src/lib.rs
  grep -n "range() with" crates/pycc_hir/src/lib.rs
  ```

  Confirm the `Expr::Name(name) => HirStmt::Assign { target: ..., value: lower_expr(&assign.value)? }` arm (around line 440) and the range-argument-parsing `match &*call.arguments.args { [stop] => ..., [start, stop] => ..., [start, stop, step] => ..., other => ... }` block (around lines 590-608) still match this plan's description (Task background reading, above). Stop and adjust subsequent steps if either has moved or changed shape.

- [ ] **Step 2: Add `CompIter` and the three new `HirStmt` variants**

  In the `HirStmt` enum, immediately after the existing `DictSet { ... }` variant:

  ```rust
      /// A comprehension's iterable source (PR-12, D-117): reuses
      /// `HirStmt::ForList`'s own iterable polymorphism verbatim (a bare
      /// name is resolved to `Ty::List`/`Ty::Dict`/`Ty::Set` downstream by
      /// `pycc_types`/`pycc_mir`, exactly like a plain `for` loop) rather
      /// than inventing a narrower, comprehension-specific iterable gate.
      #[derive(Debug, Clone, PartialEq)]
      pub enum CompIter {
          Range {
              start: HirExpr,
              stop: HirExpr,
              step: HirExpr,
          },
          Name(String),
      }

      /// `target = [elt for var in iter [if cond]]` (PR-12, D-117). Scoped to
      /// exactly one `for` clause and at most one `if` filter; only lowered
      /// when the comprehension is the direct RHS of a bare-name
      /// `Stmt::Assign` (see that arm's own handling below) -- anywhere
      /// else a comprehension expression appears, `lower_expr` has no arm
      /// for it and it falls through to that function's existing generic
      /// "expression kind not supported yet" catch-all. `var` is already
      /// the D-117 synthesized internal name, not the source spelling --
      /// every occurrence of the source name inside `cond`/`elt` has
      /// already been rewritten by `rename_name_in_expr` before this node
      /// is constructed, so downstream crates never see the user's own
      /// loop-variable spelling at all.
      ListCompAssign {
          target: String,
          var: String,
          iter: CompIter,
          cond: Option<Box<HirExpr>>,
          elt: Box<HirExpr>,
      },
      /// `target = {key: value for var in iter [if cond]}` (PR-12, D-117).
      /// Mirrors `ListCompAssign` exactly except for the key/value split
      /// (Python's dict-comprehension grammar has no direct list-comp
      /// analog of a single `elt`).
      DictCompAssign {
          target: String,
          var: String,
          iter: CompIter,
          cond: Option<Box<HirExpr>>,
          key: Box<HirExpr>,
          value: Box<HirExpr>,
      },
      /// `target = {elt for var in iter [if cond]}` (PR-12, D-117). Mirrors
      /// `ListCompAssign` exactly -- a set comprehension's own shape is
      /// identical to a list comprehension's, differing only in which
      /// runtime constructor/insert pair `pycc_codegen` ends up calling.
      SetCompAssign {
          target: String,
          var: String,
          iter: CompIter,
          cond: Option<Box<HirExpr>>,
          elt: Box<HirExpr>,
      },
  ```

  (Actual placement: `CompIter` is a standalone top-level `pub enum`, not nested inside `HirStmt` — the indentation above is illustrative of doc-comment intent only; declare `CompIter` immediately before the `HirStmt` enum definition itself, and add the three new variants as ordinary members of the existing `pub enum HirStmt { ... }` block, in the same style as `DictSet` immediately above them.)

- [ ] **Step 3: Add `rename_name_in_expr`, exhaustive over `HirExpr`**

  ```rust
  /// Rewrites every occurrence of the bare name `from` inside `expr` to
  /// `to` (PR-12, D-117) -- used to give a comprehension's own loop
  /// variable a synthesized, collision-proof internal name (see
  /// `synthesize_comp_var_name` below) without inventing real lexical
  /// scoping. Exhaustive over `HirExpr` on purpose: a future variant added
  /// to this enum must add its own arm here too, the same "let the
  /// compiler enumerate every site" discipline this project's own
  /// `Scalar::List` precedent (D-107) already established for
  /// `pycc_codegen`. Safe to apply blindly (no risk of renaming an
  /// unrelated same-named binding from some other nested scope) because
  /// v0.2's comprehension grammar has no nested comprehensions, no lambda,
  /// and no nested function defs inside a comprehension's own
  /// `elt`/`cond`/`key`/`value` -- none of those are expressible here at
  /// all yet.
  fn rename_name_in_expr(expr: HirExpr, from: &str, to: &str) -> HirExpr {
      let recurse = |e: HirExpr| rename_name_in_expr(e, from, to);
      match expr {
          HirExpr::Name(n) => HirExpr::Name(if n == from { to.to_string() } else { n }),
          HirExpr::IntLiteral(_)
          | HirExpr::FloatLiteral(_)
          | HirExpr::BoolLiteral(_)
          | HirExpr::StringLiteral(_) => expr,
          HirExpr::Call { callee, args } => HirExpr::Call {
              callee,
              args: args.into_iter().map(recurse).collect(),
          },
          HirExpr::BinOp { op, left, right } => HirExpr::BinOp {
              op,
              left: Box::new(recurse(*left)),
              right: Box::new(recurse(*right)),
          },
          HirExpr::Compare { op, left, right } => HirExpr::Compare {
              op,
              left: Box::new(recurse(*left)),
              right: Box::new(recurse(*right)),
          },
          HirExpr::FString(parts) => HirExpr::FString(
              parts
                  .into_iter()
                  .map(|part| match part {
                      FStringPart::Literal(s) => FStringPart::Literal(s),
                      FStringPart::Interpolation(e) => {
                          FStringPart::Interpolation(Box::new(recurse(*e)))
                      }
                  })
                  .collect(),
          ),
          HirExpr::ListLiteral(es) => HirExpr::ListLiteral(es.into_iter().map(recurse).collect()),
          HirExpr::Subscript { base, index } => HirExpr::Subscript {
              base: Box::new(recurse(*base)),
              index: Box::new(recurse(*index)),
          },
          HirExpr::ListAppend { list, value } => HirExpr::ListAppend {
              list: if list == from { to.to_string() } else { list },
              value: Box::new(recurse(*value)),
          },
          HirExpr::DictLiteral(pairs) => HirExpr::DictLiteral(
              pairs
                  .into_iter()
                  .map(|(k, v)| (recurse(k), recurse(v)))
                  .collect(),
          ),
          HirExpr::SetLiteral(es) => HirExpr::SetLiteral(es.into_iter().map(recurse).collect()),
          HirExpr::TupleLiteral(es) => HirExpr::TupleLiteral(es.into_iter().map(recurse).collect()),
      }
  }
  ```

  **Note:** this match must be exhaustive against `HirExpr`'s *current* shape at implementation time — re-run `cargo build -p pycc_hir` after writing it and add any arm the compiler flags as missing (e.g. if Task 6/10 land their own new `HirExpr` variants — `Slice`/`ListPop`/`DictGetOrDefault`/`SetAdd` — before this task in whatever order the implementer chooses, this function needs their arms too; if this task runs first, revisit this function once those variants exist, in the *same* task that adds them, not as an afterthought).

- [ ] **Step 4: Add `synthesize_comp_var_name`**

  ```rust
  /// Synthesizes a collision-proof internal name for a comprehension's
  /// loop variable (D-117): a leading digit can never begin a valid Python
  /// identifier (confirmed against the vendored `ruff_python_parser`'s own
  /// tokenizer -- a `NAME` token cannot start with a decimal digit), so
  /// this string can never be produced by lowering real Python source, no
  /// matter what the user names their own variables -- no new
  /// lexical-scoping machinery is needed; this is just another ordinary
  /// entry in the existing flat, name-keyed slot model. Seeded by the
  /// loop target's own byte offset, not a mutable counter: two distinct
  /// comprehensions in one file can never share a target's start offset,
  /// so this needs no threaded lowering state and stays fully
  /// deterministic across repeated compiles of the same source.
  fn synthesize_comp_var_name(target_start: ruff_text_size::TextSize, source_name: &str) -> String {
      format!("0comp_{}_{source_name}", u32::from(target_start))
  }
  ```

- [ ] **Step 5: Factor `Stmt::For`'s range-argument parsing into a reusable helper**

  Extract the existing inline block (the `callee.id.as_str() != "range"` check through the `match &*call.arguments.args { ... }` returning `(start, stop, step)`) into:

  ```rust
  /// Parses `range(...)`'s argument list into `(start, stop, step)`
  /// `HirExpr`s, defaulting `start`/`step` per Python's own `range()`
  /// overloads. Shared by `Stmt::For`'s own lowering and
  /// `lower_comprehension_iter` below (PR-12) -- factored out rather than
  /// duplicated a second time.
  fn lower_range_call(call: &pycc_ast::ExprCall) -> Result<(HirExpr, HirExpr, HirExpr), Diagnostic> {
      // ... exact body of the existing inline block, unchanged ...
  }
  ```

  Update `Stmt::For`'s own lowering to call `lower_range_call(call)?` in place of the inline block it previously had (behavior-preserving refactor — run the existing `pycc_hir` test suite after this step alone, before adding any new functionality, to confirm zero regressions).

- [ ] **Step 6: Add `lower_comprehension_iter`, reusing `Stmt::For`'s own iterable-shape acceptance**

  ```rust
  /// Resolves a comprehension's `for var in <iter>` clause into a
  /// `CompIter`, reusing `Stmt::For`'s own iterable-shape acceptance
  /// verbatim (D-117): `range(...)` or a bare name (resolved to
  /// `Ty::List`/`Ty::Dict`/`Ty::Set` downstream by `pycc_types`/
  /// `pycc_mir`, exactly like a plain `for` loop). Any other shape is
  /// rejected with the existing generic `C0001` path, mirroring
  /// `Stmt::For`'s own "only `for x in range(...)` or `for x in <list>` is
  /// supported so far" message.
  fn lower_comprehension_iter(iter_expr: &Expr) -> Result<CompIter, Diagnostic> {
      if let Expr::Name(name) = iter_expr {
          return Ok(CompIter::Name(name.id.as_str().to_string()));
      }
      let Expr::Call(call) = iter_expr else {
          return Err(unsupported(
              format!(
                  "only `range(...)` or a bare-name iterable is supported so far in a comprehension: {iter_expr:?}"
              ),
              pycc_ast::expr_range(iter_expr),
          ));
      };
      let Expr::Name(callee) = call.func.as_ref() else {
          return Err(unsupported(
              "only calling `range(...)` is supported so far in a comprehension",
              pycc_ast::expr_range(&call.func),
          ));
      };
      if callee.id.as_str() != "range" {
          return Err(unsupported(
              format!(
                  "only iterating over `range(...)` is supported so far in a comprehension, got `{}`",
                  callee.id
              ),
              call.range,
          ));
      }
      if !call.arguments.keywords.is_empty() {
          return Err(unsupported(
              "keyword arguments to range() are not supported yet",
              call.range,
          ));
      }
      let (start, stop, step) = lower_range_call(call)?;
      Ok(CompIter::Range { start, stop, step })
  }
  ```

- [ ] **Step 7: Add the shared comprehension-header helper**

  ```rust
  /// Validates and lowers a comprehension's shared shape (D-117): exactly
  /// one generator clause, no `async for`, a bare-name loop target, at
  /// most one `if` filter. Returns the loop target's *source* name, its
  /// synthesized internal replacement, the resolved `CompIter`, and the
  /// (not-yet-renamed) lowered `if`-filter expression, if present --
  /// renaming is the caller's job (Step 8/9/10 below), since `elt`/`key`/
  /// `value` also need the identical rename and this helper has no
  /// visibility into which of those the caller is building.
  fn lower_comprehension_header(
      generators: &[pycc_ast::Comprehension],
  ) -> Result<(String, String, CompIter, Option<HirExpr>), Diagnostic> {
      let [gen] = generators else {
          return Err(unsupported(
              "a comprehension with more than one `for` clause is not supported yet",
              generators
                  .first()
                  .map(|g| g.range)
                  .unwrap_or_default(),
          ));
      };
      if gen.is_async {
          return Err(unsupported(
              "async comprehensions are not supported yet",
              gen.range,
          ));
      }
      let Expr::Name(var) = &gen.target else {
          return Err(unsupported(
              "only a bare name comprehension target is supported so far",
              pycc_ast::expr_range(&gen.target),
          ));
      };
      let cond = match gen.ifs.as_slice() {
          [] => None,
          [single] => Some(lower_expr(single)?),
          _ => {
              return Err(unsupported(
                  "a comprehension with more than one `if` filter is not supported yet",
                  gen.range,
              ));
          }
      };
      let iter = lower_comprehension_iter(&gen.iter)?;
      let source_name = var.id.as_str().to_string();
      let synth_var = synthesize_comp_var_name(var.range.start(), &source_name);
      Ok((source_name, synth_var, iter, cond))
  }
  ```

  `pycc_ast::Comprehension`'s exact field names (`target`/`iter`/`ifs`/`is_async`/`range`) match `ruff_python_ast::Comprehension` (`nodes.rs:2788`, confirmed during this plan's own research) exactly, since Step 0 makes `pycc_ast::Comprehension` a direct re-export (a type alias, not a wrapper) — no field-name translation needed.

- [ ] **Step 8: Add `lower_list_comp_assign`/`lower_set_comp_assign`/`lower_dict_comp_assign`**

  ```rust
  fn lower_list_comp_assign(target: &str, comp: &pycc_ast::ExprListComp) -> Result<HirStmt, Diagnostic> {
      let (source_name, synth_var, iter, cond) = lower_comprehension_header(&comp.generators)?;
      let elt = rename_name_in_expr(lower_expr(&comp.elt)?, &source_name, &synth_var);
      let cond = cond.map(|c| rename_name_in_expr(c, &source_name, &synth_var));
      Ok(HirStmt::ListCompAssign {
          target: target.to_string(),
          var: synth_var,
          iter,
          cond: cond.map(Box::new),
          elt: Box::new(elt),
      })
  }

  fn lower_set_comp_assign(target: &str, comp: &pycc_ast::ExprSetComp) -> Result<HirStmt, Diagnostic> {
      let (source_name, synth_var, iter, cond) = lower_comprehension_header(&comp.generators)?;
      let elt = rename_name_in_expr(lower_expr(&comp.elt)?, &source_name, &synth_var);
      let cond = cond.map(|c| rename_name_in_expr(c, &source_name, &synth_var));
      Ok(HirStmt::SetCompAssign {
          target: target.to_string(),
          var: synth_var,
          iter,
          cond: cond.map(Box::new),
          elt: Box::new(elt),
      })
  }

  fn lower_dict_comp_assign(target: &str, comp: &pycc_ast::ExprDictComp) -> Result<HirStmt, Diagnostic> {
      let Some(key_expr) = comp.key.as_deref() else {
          // Real Python's dict-comprehension grammar (`{k: v for ...}`)
          // has no `**`-unpacking form the way a plain `Expr::Dict` literal
          // does -- verify this is genuinely unreachable from the parser
          // (e.g. write a quick throwaway `pycc_parser::parse` probe on
          // `{**x for k in y}`-shaped source, which should fail to *parse*
          // at all, not merely produce `key: None`) before relying on this
          // panic never firing; if it turns out reachable, replace this
          // with a real `unsupported(...)` diagnostic instead.
          unreachable!(
              "pycc_hir: internal error: a dict comprehension with no key expression should be unparseable"
          );
      };
      let (source_name, synth_var, iter, cond) = lower_comprehension_header(&comp.generators)?;
      let key = rename_name_in_expr(lower_expr(key_expr)?, &source_name, &synth_var);
      let value = rename_name_in_expr(lower_expr(&comp.value)?, &source_name, &synth_var);
      let cond = cond.map(|c| rename_name_in_expr(c, &source_name, &synth_var));
      Ok(HirStmt::DictCompAssign {
          target: target.to_string(),
          var: synth_var,
          iter,
          cond: cond.map(Box::new),
          key: Box::new(key),
          value: Box::new(value),
      })
  }
  ```

- [ ] **Step 9: Wire the three helpers into `Stmt::Assign`'s existing `Expr::Name` arm**

  Replace:
  ```rust
  Expr::Name(name) => HirStmt::Assign {
      target: name.id.as_str().to_string(),
      value: lower_expr(&assign.value)?,
  },
  ```
  with:
  ```rust
  Expr::Name(name) => match assign.value.as_ref() {
      Expr::ListComp(comp) => lower_list_comp_assign(name.id.as_str(), comp)?,
      Expr::SetComp(comp) => lower_set_comp_assign(name.id.as_str(), comp)?,
      Expr::DictComp(comp) => lower_dict_comp_assign(name.id.as_str(), comp)?,
      _ => HirStmt::Assign {
          target: name.id.as_str().to_string(),
          value: lower_expr(&assign.value)?,
      },
  },
  ```

  This is the **only** place a comprehension expression is ever specially recognized — every other position (function args, nested expressions, `return`, `Expr::Subscript` assignment targets) still routes through plain `lower_expr`, which has no arm for `Expr::ListComp`/`SetComp`/`DictComp`/`GeneratorExp` and falls through to its existing catch-all. Confirm via `grep -n "expression kind not supported yet\|expr kind not supported" crates/pycc_hir/src/lib.rs` that this catch-all exists and covers these AST node kinds (it must, or `lower_expr` isn't currently exhaustive and this step's assumption needs revisiting).

- [ ] **Step 10: Tests**

  Add unit tests directly against `lower_stmt`/`lower_checked` (mirroring this file's existing test style) covering: (a) a plain list comprehension over `range(...)` lowers to `HirStmt::ListCompAssign` with the expected synthesized `var` name and renamed `elt`; (b) a list comprehension with an `if` filter; (c) a dict comprehension with an f-string key (`{f"n{i}": i for i in range(3)}`) — confirms `FString`'s own `rename_name_in_expr` arm actually rewrites an interpolated loop-variable reference; (d) a set comprehension; (e) more than one `for` clause rejected with `C0001`; (f) more than one `if` filter rejected with `C0001`; (g) a non-bare-name comprehension target (`[x for (a, b) in xs]`-shaped, if constructible) rejected with `C0001`; (h) `async for` rejected with `C0001`; (i) a comprehension used as a function-call argument (`print([x for x in range(3)])`) still reaches the *existing* generic catch-all, not a new error path — pins the "only `Stmt::Assign`-RHS position" restriction; (j) iterating a bare-name variable (not `range(...)`) produces `CompIter::Name`; (k) `rename_name_in_expr` itself, directly, against a hand-built `HirExpr` tree covering every variant (this is the exhaustiveness-pinning test D-014's coverage gate needs — every arm must be hit by at least one case); (l) `lower_comprehension_header(&[])` (an empty generators slice), called **directly**, not through the parser — real parsed source can never produce a comprehension with zero generators, so this is the only way to reach the `[gen] = generators else { ... }` arm's failure branch and its own `generators.first().map(|g| g.range).unwrap_or_default()` span-fallback expression at all; without this direct-call test, D-014's region gate flags that fallback as an uncoverable dead branch. Run `cargo llvm-cov -p pycc_hir` and confirm 100%.

---

## Task 3: `pycc_types` — Comprehension type-checking (all match sites)

**Files:**
- Modify: `crates/pycc_types/src/lib.rs`

**Interfaces:**
- Consumes: `HirStmt::ListCompAssign`/`DictCompAssign`/`SetCompAssign`, `pycc_hir::CompIter` (Task 2).
- Produces: type-checking for the three new statement kinds across every site that currently matches on `HirStmt`.

- [ ] **Step 1: Re-locate every site needing a new arm**

  ```bash
  grep -n "HirStmt::ForList\|HirStmt::DictSet\|fn collect_local_names\|fn collect_expr_constraints\|fn check_stmt\b\|fn check_stmt_in_function\|fn block_always_returns\|fn contains_return" crates/pycc_types/src/lib.rs
  ```

  Confirm all five real (non-test) match sites this plan's own research found still exist and are shaped as described: (1) `collect_local_names` (~line 106, gathers which names are function-locally bound, for the unbound-local-vs-undefined diagnostic distinction — has an explicit catch-all `HirStmt::ExprStmt(_) | HirStmt::Return(_) | HirStmt::DictSet { .. } => {}`); (2) `collect_expr_constraints`'s own statement walk (~line 489, the private-helper signature-inference solver) plus its sibling `contains_return`-style helper (~line 668); (3) `check_stmt` (~line 1523, module/top-level scope); (4) `check_function_in`'s `block_always_returns` (~line 1774, an exhaustive false-returning match with no catch-all — every `HirStmt` variant is named explicitly); (5) `check_stmt_in_function` (~line 1852, function-body scope). Stop and adjust if any site's shape or line has materially changed.

- [ ] **Step 2: Add a shared `resolve_comp_iter` helper**

  ```rust
  /// Resolves a comprehension's iterable (`pycc_hir::CompIter`) to the
  /// loop variable's type, without binding it -- mirrors `HirStmt::ForList`'s
  /// own resolution exactly (`check_stmt`/`check_stmt_in_function`'s
  /// existing `ForList` arms), reused rather than duplicated a third time
  /// (PR-12, D-117). Range/list/dict/set element-type resolution is
  /// identical to `ForList`'s; a comprehension adds nothing new here.
  fn resolve_comp_iter(
      env: &Environment,
      local_names: &[&str],
      iter: &pycc_hir::CompIter,
  ) -> Result<Ty, Diagnostic> {
      match iter {
          pycc_hir::CompIter::Range { start, stop, step } => {
              check_range_operand_in(env, local_names, "start", start)?;
              check_range_operand_in(env, local_names, "stop", stop)?;
              check_range_operand_in(env, local_names, "step", step)?;
              Ok(Ty::Int)
          }
          pycc_hir::CompIter::Name(name) => {
              let base_ty = lookup_bound_name(env, local_names, name)?;
              match base_ty {
                  Ty::List(elem_ty) => Ok(*elem_ty),
                  Ty::Dict(kv) => Ok(kv.0),
                  Ty::Set(elem_ty) => Ok(*elem_ty),
                  other => Err(Diagnostic::error(
                      "T0033",
                      format!(
                          "`{}` cannot be iterated with `for ... in ...` (only list[T]/dict[K, V]/set[T] supports this)",
                          other.name()
                      ),
                      Span::new(0, 0),
                  )),
              }
          }
      }
  }
  ```

  (Use `check_range_operand`/`lookup_bound_name` with an empty `local_names` slice at module scope, exactly mirroring how `ForList`'s own module-scope `check_stmt` arm already does — see Step 1's grep output for the exact existing pattern to copy.)

- [ ] **Step 3: Add the `check_stmt`/`check_stmt_in_function` arms**

  Module scope (`check_stmt`), immediately after the existing `HirStmt::ForList { .. }` arm:

  ```rust
  HirStmt::ListCompAssign { target, var, iter, cond, elt } => {
      let var_ty = resolve_comp_iter(env, &[], iter)?;
      check_assignment(env, var, var_ty)?;
      if let Some(cond) = cond {
          infer_expr(env, cond)?;
      }
      let elt_ty = infer_expr(env, elt)?;
      if elt_ty != Ty::Int {
          return Err(Diagnostic::error(
              "T0034",
              format!(
                  "list codegen only supports `list[int]` in v0.2, got a comprehension producing `list[{}]`",
                  elt_ty.name()
              ),
              Span::new(0, 0),
          ));
      }
      check_assignment(env, target, Ty::List(Box::new(Ty::Int)))
  }
  HirStmt::SetCompAssign { target, var, iter, cond, elt } => {
      let var_ty = resolve_comp_iter(env, &[], iter)?;
      check_assignment(env, var, var_ty)?;
      if let Some(cond) = cond {
          infer_expr(env, cond)?;
      }
      let elt_ty = infer_expr(env, elt)?;
      if elt_ty != Ty::Int {
          return Err(Diagnostic::error(
              "T0038",
              format!(
                  "set codegen only supports `set[int]` in v0.2, got a comprehension producing `set[{}]`",
                  elt_ty.name()
              ),
              Span::new(0, 0),
          ));
      }
      check_assignment(env, target, Ty::Set(Box::new(Ty::Int)))
  }
  HirStmt::DictCompAssign { target, var, iter, cond, key, value } => {
      let var_ty = resolve_comp_iter(env, &[], iter)?;
      check_assignment(env, var, var_ty)?;
      if let Some(cond) = cond {
          infer_expr(env, cond)?;
      }
      let key_ty = infer_expr(env, key)?;
      let value_ty = infer_expr(env, value)?;
      if key_ty != Ty::Str || value_ty != Ty::Int {
          return Err(Diagnostic::error(
              "T0036",
              format!(
                  "dict codegen only supports `dict[str, int]` in v0.2, got a comprehension producing `dict[{}, {}]`",
                  key_ty.name(),
                  value_ty.name()
              ),
              Span::new(0, 0),
          ));
      }
      check_assignment(env, target, Ty::Dict(Box::new((Ty::Str, Ty::Int))))
  }
  ```

  Function scope (`check_stmt_in_function`): identical three arms, replacing `env`/`infer_expr`/`&[]` with `local_names`-aware equivalents (`infer_expr_in(env, local_names, ...)`, `resolve_comp_iter(env, local_names, iter)`), mirroring exactly how `ForList`'s own two arms (module vs. function scope) already differ only in that respect.

- [ ] **Step 4: Add the `collect_local_names` arm**

  ```rust
  HirStmt::ListCompAssign { target, var, .. }
  | HirStmt::SetCompAssign { target, var, .. }
  | HirStmt::DictCompAssign { target, var, .. } => {
      if !is_local(names, target) {
          names.push(target);
      }
      if !is_local(names, var) {
          names.push(var);
      }
  }
  ```

  (Both `target` and the synthesized `var` are new local names — mirrors `ForList`'s own single `var`-push, doubled since a comprehension introduces two names where a `for` loop introduces one.)

- [ ] **Step 5: Add the `collect_expr_constraints`/solver arm — a deliberate no-op, matching container-literal precedent**

  Per D-116's own correction note ("a container-literal assignment's target never receives a solver binding at all... confirmed empirically for all four container types"), this plan's own comprehension statements get the **identical** treatment: no unification term is registered for `target`. Add:

  ```rust
  HirStmt::ListCompAssign { .. } | HirStmt::SetCompAssign { .. } | HirStmt::DictCompAssign { .. } => {
      // Mirrors `ListLiteral`/`DictLiteral`/`SetLiteral`'s own existing
      // solver gap (D-116's correction note): this solver only unifies
      // scalar `Ty::Infer` parameters/returns, so a comprehension's own
      // target -- exactly like any other container-literal assignment --
      // receives no term here. An entirely unannotated private helper
      // containing a comprehension therefore has the identical
      // pre-existing limitation `list`/`dict`/`set`/`tuple` literals
      // already have, not a new gap this statement introduces.
  }
  ```

  Add the same three variants to the sibling `contains_return`-style helper (~line 668) in its `=> false` catch-all group (mirroring how `HirStmt::Assign`/`DictSet` are already grouped there).

- [ ] **Step 6: Add the `block_always_returns` arm**

  This match has **no catch-all** (every variant named explicitly) — add:

  ```rust
  | HirStmt::ListCompAssign { .. }
  | HirStmt::SetCompAssign { .. }
  | HirStmt::DictCompAssign { .. } => false,
  ```

  to the existing `false`-returning group alongside `Assign`/`AnnAssign`/`While`/`ForRange`/`ForList`/`DictSet`.

- [ ] **Step 7: Tests**

  Cover, at both module and function scope: (a) a valid `list[int]`-producing comprehension type-checks and binds `target: Ty::List(Int)`; (b) a comprehension producing `list[str]`/`list[float]`/`list[bool]` is rejected with `T0034` (mirrors `ListLiteral`'s own existing genericity tests); (c) the dict/set equivalents with `T0036`/`T0038`; (d) an `if` filter of non-bool-convertible type still type-checks (any type accepted as truthy, mirroring `If`/`While`); (e) an iterable resolving to a non-list/dict/set base is rejected with `T0033` (reuses `ForList`'s own existing message); (f) the synthesized loop-variable name is genuinely bound and usable inside `elt`/`cond` (i.e., type-checking a comprehension whose `elt` references the loop variable succeeds — pins that `check_assignment(env, var, var_ty)` really runs before `elt`/`cond` are checked); (g) an entirely unannotated private helper containing a comprehension still fails with the *existing* "cannot infer" `T0021` class of error (pins Step 5's deliberate no-op, matching the pre-existing container-literal gap rather than a new one). Run `cargo llvm-cov -p pycc_types` and confirm 100%.

---

## Task 4: `pycc_mir` — Comprehension lowering (`MirStmt::ListCompAssign`/`DictCompAssign`/`SetCompAssign`, `CompSource`)

**Files:**
- Modify: `crates/pycc_mir/src/lib.rs`

**Interfaces:**
- Consumes: `HirStmt::ListCompAssign`/`DictCompAssign`/`SetCompAssign` (Task 2), the already-checked/resolved HIR (Task 3's `check_and_resolve` output).
- Produces: `MirStmt::ListCompAssign`/`DictCompAssign`/`SetCompAssign`, `pycc_mir::CompSource`.

- [ ] **Step 1: Re-verify `lower_stmt`'s signature and the existing `HirStmt::ForList` split**

  ```bash
  grep -n "fn lower_stmt\|fn lookup\b\|fn bind_variable\b" crates/pycc_mir/src/lib.rs
  ```

  Confirm `lower_stmt(stmt: &HirStmt, scopes: &mut Vec<HashMap<String, Ty>>) -> MirStmt` and the `HirStmt::ForList` arm's `match lookup(scopes, list) { Ty::List(elem_ty) => ..., Ty::Dict(kv) => ..., Ty::Set(elem_ty) => ..., other => panic!(...) }` shape (this plan's own research, `crates/pycc_mir/src/lib.rs:492-557`) still hold. This task's own comprehension arms are a direct structural mirror of this exact block.

- [ ] **Step 2: Add `CompSource` and the three new `MirStmt` variants**

  ```rust
  /// A comprehension's already-resolved iterable source (PR-12, D-117) --
  /// the MIR-level counterpart to `pycc_hir::CompIter`, but with a
  /// bare-name iterable already split into its concrete container kind,
  /// mirroring `HirStmt::ForList`'s own split into `MirStmt::ForList`/
  /// `ForDict`/`ForSet` at this exact lowering stage. Kept as a field on
  /// each `*CompAssign` variant (not exploded into a full cross-product of
  /// top-level `MirStmt` variants per comprehension-kind × source-kind
  /// combination) -- mirrors the precedent `MirExpr::Subscript` already
  /// established (one node, internal branching on the resolved type in
  /// `pycc_codegen`), avoiding a 3×4 combinatorial explosion for no
  /// benefit.
  #[derive(Debug, PartialEq)]
  pub enum CompSource {
      Range {
          start: MirExpr,
          stop: MirExpr,
          step: MirExpr,
      },
      List(String),
      Dict(String),
      Set(String),
  }
  ```

  In `MirStmt`, immediately after the existing `ForSet { .. }` variant. Each variant carries its own `var_ty: Ty` explicitly — **not** derived later in `pycc_codegen` by inspecting `elt`/`key`/`value`/`cond` for a matching `Name` (a fragile, redundant re-derivation of a fact `resolve_comp_source` already computes once, below, and stores nowhere for a later consumer to read cheaply and unambiguously):

  ```rust
  /// `target = [elt for var in <source> [if cond]]`, already fully
  /// resolved (mirrors `HirStmt::ListCompAssign`, PR-12, D-117). `var_ty`
  /// is the loop variable's own resolved type (`Ty::Int` for a `Range`
  /// source; the iterated container's element/key type for a `List`/
  /// `Dict`/`Set` source) -- carried explicitly, rather than requiring
  /// `pycc_codegen` to re-derive it from `elt`, because `elt` need not
  /// contain any `Name` reference to `var` at all in general (though in
  /// practice it usually does) and re-deriving it by walking `elt` would
  /// be a second, independent computation of a fact `resolve_comp_source`
  /// (Step 3, below) already has in hand once.
  ListCompAssign {
      target: String,
      var: String,
      var_ty: Ty,
      source: CompSource,
      cond: Option<Box<MirExpr>>,
      elt: Box<MirExpr>,
  },
  DictCompAssign {
      target: String,
      var: String,
      var_ty: Ty,
      source: CompSource,
      cond: Option<Box<MirExpr>>,
      key: Box<MirExpr>,
      value: Box<MirExpr>,
  },
  SetCompAssign {
      target: String,
      var: String,
      var_ty: Ty,
      source: CompSource,
      cond: Option<Box<MirExpr>>,
      elt: Box<MirExpr>,
  },
  ```

- [ ] **Step 3: Add a shared `resolve_comp_source` helper and the three `lower_stmt` arms**

  ```rust
  /// Resolves a `pycc_hir::CompIter` into a fully-typed `CompSource`,
  /// lowering any range sub-expressions and binding `var`'s type into
  /// `scopes` -- mirrors `HirStmt::ForList`'s own resolution exactly
  /// (`lower_stmt`'s existing `ForList` arm), reused via this shared
  /// helper rather than duplicated three times (once per comprehension
  /// kind).
  fn resolve_comp_source(
      iter: &pycc_hir::CompIter,
      var: &str,
      scopes: &mut Vec<HashMap<String, Ty>>,
  ) -> (CompSource, Ty) {
      match iter {
          pycc_hir::CompIter::Range { start, stop, step } => {
              let start = lower_expr(start, scopes);
              let stop = lower_expr(stop, scopes);
              let step = lower_expr(step, scopes);
              bind_variable(scopes, var.to_string(), Ty::Int);
              (CompSource::Range { start, stop, step }, Ty::Int)
          }
          pycc_hir::CompIter::Name(name) => match lookup(scopes, name) {
              Ty::List(elem_ty) => {
                  bind_variable(scopes, var.to_string(), (*elem_ty).clone());
                  (CompSource::List(name.clone()), *elem_ty)
              }
              Ty::Dict(kv) => {
                  bind_variable(scopes, var.to_string(), kv.0.clone());
                  (CompSource::Dict(name.clone()), kv.0)
              }
              Ty::Set(elem_ty) => {
                  bind_variable(scopes, var.to_string(), (*elem_ty).clone());
                  (CompSource::Set(name.clone()), *elem_ty)
              }
              other => panic!(
                  "pycc_mir: internal error: `{name}` is neither a list, dict, nor set (found `{}`) -- pycc_types::check should have rejected this HIR before it reached pycc_mir",
                  other.name()
              ),
          },
      }
  }
  ```

  Add to `lower_stmt`, immediately after the existing `HirStmt::ForList { .. }` arm:

  ```rust
  HirStmt::ListCompAssign { target, var, iter, cond, elt } => {
      let (source, var_ty) = resolve_comp_source(iter, var, scopes);
      let cond = cond.as_deref().map(|c| lower_expr(c, scopes));
      let elt = lower_expr(elt, scopes);
      bind_variable(scopes, target.clone(), Ty::List(Box::new(elt.ty())));
      MirStmt::ListCompAssign {
          target: target.clone(),
          var: var.clone(),
          var_ty,
          source,
          cond: cond.map(Box::new),
          elt: Box::new(elt),
      }
  }
  HirStmt::SetCompAssign { target, var, iter, cond, elt } => {
      let (source, var_ty) = resolve_comp_source(iter, var, scopes);
      let cond = cond.as_deref().map(|c| lower_expr(c, scopes));
      let elt = lower_expr(elt, scopes);
      bind_variable(scopes, target.clone(), Ty::Set(Box::new(elt.ty())));
      MirStmt::SetCompAssign {
          target: target.clone(),
          var: var.clone(),
          var_ty,
          source,
          cond: cond.map(Box::new),
          elt: Box::new(elt),
      }
  }
  HirStmt::DictCompAssign { target, var, iter, cond, key, value } => {
      let (source, var_ty) = resolve_comp_source(iter, var, scopes);
      let cond = cond.as_deref().map(|c| lower_expr(c, scopes));
      let key = lower_expr(key, scopes);
      let value = lower_expr(value, scopes);
      bind_variable(
          scopes,
          target.clone(),
          Ty::Dict(Box::new((key.ty(), value.ty()))),
      );
      MirStmt::DictCompAssign {
          target: target.clone(),
          var: var.clone(),
          var_ty,
          source,
          cond: cond.map(Box::new),
          key: Box::new(key),
          value: Box::new(value),
      }
  }
  ```

- [ ] **Step 4: Tests**

  Mirror `ForList`/`ForDict`/`ForSet`'s own existing lowering tests: (a) a range-sourced list comprehension lowers to `CompSource::Range` with `var_ty: Ty::Int`; (b) a bare-name-list-sourced one resolves `CompSource::List` with `var_ty` equal to the list's element type; (c) the dict/set equivalents resolve `CompSource::Dict`/`Set` correctly, including the dict case's `var_ty` being the *key* type, not the value type (mirrors `ForList`'s own `kv.0` choice); (d) the internal-error panic fires for a name that resolves to neither list/dict/set (construct this directly via hand-built HIR + `scopes`, bypassing `pycc_types`, exactly like this crate's own existing `ForList` panic test does). Run `cargo llvm-cov -p pycc_mir` and confirm 100%.

---

## Task 5a: `pycc_codegen` — List-comprehension codegen (establishes the shared loop-building pattern)

**Files:**
- Modify: `crates/pycc_codegen/src/lib.rs`

**Interfaces:**
- Consumes: `MirStmt::ListCompAssign`, `pycc_mir::CompSource` (Task 4).
- Produces: real codegen; **zero new `Scalar` variants** (reuses `Scalar::List`).

Split out from a single combined "Task 5" per an advisor review: three comprehension kinds × four source kinds × with/without an `if` filter is a 24-path combinatorial surface, each needing an executing test for D-014's region-coverage gate — proportionally the largest single task in this plan. `ListCompAssign` across all reachable sources proves the whole loop-building pattern once; Task 5b then reuses it for `DictCompAssign`/`SetCompAssign` rather than re-deriving it from scratch, mirroring PR-11's own a/b split precedent (dict+set vs. tuple).

- [ ] **Step 1: Re-locate `collect_stmt_bindings` and each `emit_stmt` For-family arm**

  ```bash
  grep -n "fn collect_stmt_bindings\|MirStmt::ForRange {\|MirStmt::ForList {\|MirStmt::ForDict {\|MirStmt::ForSet {\|fn emit_stmt\b\|fn emit_assign\b\|fn build_int_list_append\b" crates/pycc_codegen/src/lib.rs
  ```

  Read each `emit_stmt` For-family arm in full before writing this task's own code — this plan's own research (`ForRange` ~3747-3958, `ForList` ~3909-3958, `ForDict` ~4128-4230) confirms each already builds its own `test`/`body`/`after` basic-block trio, with `ForList`/`ForDict`/`ForSet` each an "intentional inline duplicate" of `ForRange`'s own shape (per that code's own comments). `ListCompAssign`'s own arm is a **fourth** inline duplicate, differing only in (a) allocating and storing the target's empty container before the loop starts, (b) which per-iteration `_get`/`_len` FFI pair backs the loop test/body depending on `source`, and (c) a conditional `.append()` call inside the loop body instead of arbitrary user statements. Also confirm `emit_assign`'s exact signature (`crates/pycc_codegen/src/lib.rs:2551`, `fn emit_assign<'ctx>(context, builder, locals: &mut HashMap<String, StorageSlot<'ctx>>, target: &str, value: Scalar<'ctx>)`) — its own existing `Scalar::List` arm (line ~2580) already stores a list pointer into a named slot with **no** decref/incref traffic (D-107's leak-only rule), confirming this is the exact, already-correct function to call for storing the comprehension's own freshly built container into `target`'s slot; no separate `store_named_value`-style helper needs inventing.

- [ ] **Step 2: Add `collect_stmt_bindings`'s `ListCompAssign` arm — two slots**

  ```rust
  // A comprehension introduces *two* new bindings, not one: `target` (the
  // produced container) and `var` (the synthesized loop variable, D-117).
  // `var_ty` is carried explicitly on the MIR node itself (Task 4) --
  // no re-derivation from `elt` is needed or attempted here.
  MirStmt::ListCompAssign { target, var, var_ty, elt, .. } => {
      bindings.entry(var.clone()).or_insert_with(|| var_ty.clone());
      bindings.entry(target.clone()).or_insert(pycc_mir::Ty::List(Box::new(elt.ty())));
  }
  ```

- [ ] **Step 3: Add the `emit_stmt` arm for `MirStmt::ListCompAssign`**

  ```rust
  MirStmt::ListCompAssign { target, var, var_ty, source, cond, elt } => {
      // 1. Allocate the target's empty backing list and store it in its
      //    already-declared slot (Step 2's `collect_stmt_bindings` arm
      //    guarantees this slot exists before `emit_stmt` ever runs, the
      //    same invariant every other container-typed target already
      //    relies on) -- mirrors `MirExpr::ListLiteral`'s own
      //    `rt.int_list_new` call, then `emit_assign` (Step 1's own note
      //    above: its existing `Scalar::List` arm needs no decref/incref).
      let new_list = builder
          .build_call(rt.int_list_new, &[], "comp_list_new")
          .expect("build_call should not fail for a well-formed list allocation")
          .try_as_basic_value()
          .expect_basic("pycc_rt_int_list_new returns a non-void pointer")
          .into_pointer_value();
      emit_assign(context, builder, locals, target, Scalar::List(new_list));

      // 2. Build the loop -- test/body/after basic blocks. When `source`
      //    is `Range`, mirror `MirStmt::ForRange`'s own shape exactly
      //    (three sub-expressions, a phi-based induction variable). When
      //    `source` is `List`/`Dict`/`Set`, mirror `ForList`/`ForDict`/
      //    `ForSet`'s own shape (an indexed `_get`+`_len` loop against the
      //    resolved base container), including each one's own
      //    per-iteration loop-variable binding exactly -- in particular,
      //    a `Dict` source must reproduce `ForDict`'s own `rt.str_incref`
      //    call on the read key (`crates/pycc_codegen/src/lib.rs:4179-4181`)
      //    before binding `var` via `emit_assign`, and must NOT call
      //    `decref_str_slot_before_store` for that per-iteration bind
      //    (mirroring `ForDict`'s own comment: "this specific write never
      //    itself calls `decref_str_slot_before_store`") -- this keeps
      //    `var`'s own reference safely alive across the iteration
      //    without corrupting the source dict's own key. This is
      //    necessary regardless of what `elt` does with `var` afterward,
      //    exactly like `ForDict`'s own unconditional treatment; get this
      //    right here even though no *reachable* `list[int]`-producing
      //    comprehension in this PR's own scope can actually be sourced
      //    from a dict (T0034 requires `elt: Ty::Int`, and this compiler
      //    has no `str`-to-`int` builtin of any kind yet -- not even
      //    `len(str)`, confirmed via `crates/pycc_types/src/lib.rs`'s own
      //    `len` handling, which accepts only `List`/`Dict`/`Set` -- so
      //    `CompSource::Dict` reaching this specific arm is, for now, an
      //    internal-error panic path a type-checked program can never
      //    trigger; Task 5b's own `DictCompAssign` arm is where a
      //    `Dict`-sourced comprehension is actually reachable and tested).

      // 3. Inside the loop body: if `cond` is `Some`, evaluate it, branch
      //    on truthiness into a small `if_taken`/`if_skip` pair of blocks
      //    (mirroring `MirStmt::If`'s own two-block shape), and only
      //    inside `if_taken` do step 3b below; if `cond` is `None`, do
      //    step 3b unconditionally.
      // 3b. Evaluate `elt`, run it through the identical tag/untag
      //    sequence `MirExpr::ListAppend`'s own arm already uses
      //    (`to_tagged_int` then `build_untag_checked`, `crates/
      //    pycc_codegen/src/lib.rs:1976-1982`), and call
      //    `build_int_list_append(builder, rt, new_list, raw)`.
  }
  ```

  **This step needs the implementer to read `ForList`/`ForRange`/`ForDict`'s full `emit_stmt` arms directly from the live source before writing code** — this plan's own research confirmed the shape (three basic blocks, an indexed `_get`+`_len` loop, `ForDict`'s own key-incref requirement) but did not transcribe every `inkwell` builder call; do not guess at exact call sequences from this sketch alone.

- [ ] **Step 4: Tests**

  Cover every reachable source × filter combination for `ListCompAssign`: (a) `xs = [i * 2 for i in range(5)]` (Range source, no filter); (b) `ys = [x for x in xs if x > 4]` (List source, sourced from a prior comprehension's own output, with a filter); (c) `zs = [x for x in some_set]` (Set source — reachable and type-safe, since `set[int]`'s element type is `Ty::Int`, satisfying `list[int]`'s own T0034 gate trivially); (d) an empty-result comprehension (`[i for i in range(0)]` or an always-false filter) produces a genuinely empty, still-valid list, not a null/dangling pointer; (e) a `CompSource::Dict` value constructed directly via hand-built MIR (bypassing `pycc_types`, exactly like this crate's own existing internal-error-panic tests for other unreachable-from-real-source shapes) confirms the dict-sourced per-iteration binding code path (Step 3's own point 2) does not crash even though no real program can reach it today — a real, if currently unreachable-from-source, coverage-gate-satisfying test, not dead code. Run `cargo llvm-cov -p pycc_codegen` and confirm every line/region this arm adds is hit.

---

## Task 5b: `pycc_codegen` — Dict/set-comprehension codegen (reuses Task 5a's shape; adds the dict-key refcounting fix)

**Files:**
- Modify: `crates/pycc_codegen/src/lib.rs`

**Interfaces:**
- Consumes: `MirStmt::DictCompAssign`/`SetCompAssign`, `pycc_mir::CompSource` (Task 4).
- Produces: real codegen; **zero new `Scalar` variants** (reuses `Scalar::Dict`/`Set`).

**The one genuinely new correctness question this task must resolve (found by review, not present in Task 5a):** `{k: 1 for k in d}` — a `Dict`-sourced `DictCompAssign`, unlike `Dict`-sourced `List`/`SetCompAssign`, **is** reachable and type-checks (`var`/`k` is `Ty::Str`, satisfying `dict[str, int]`'s own key-type gate, T0036, directly). Its `key` field is, in this common shape, exactly `MirExpr::Name { name: var, ty: Ty::Str }` — a bare reference to the *same* `PyStrObj` pointer the source dict `d` itself still owns at that index. `MirStmt::DictSet`'s own existing codegen (`crates/pycc_codegen/src/lib.rs:4060-4084`) calls `incref_if_str_duplicate(builder, rt, key, key_scalar)` on exactly this situation before `build_dict_set`, because "`pycc_rt_dict_set` adopts whatever key pointer it is given as `d`'s own permanent reference without incref'ing it itself (D-124)" — without that incref, the *new* dict this comprehension builds would hold a pointer whose only other owner (the source dict `d`) could, in principle, see it decref'd by a later dict-key rebinding (although, per Task 5a's own point 2, `var`'s own per-iteration bind never itself decrefs — the risk this call actually guards against is a *different*, more general one: `incref_if_str_duplicate`'s own job is to make the stored key genuinely independently owned, not merely alive for the remainder of this one iteration, so a future refcounting change to `var`'s own per-iteration handling, or to the source dict `d` itself, cannot retroactively invalidate a key already captured into a separate, independent dict). This task's own `DictCompAssign` arm must call `incref_if_str_duplicate` on `key` before `build_dict_set`, unconditionally, exactly mirroring `MirStmt::DictSet`'s own call — this is a no-op when `key` isn't a bare `Name` of an existing binding (e.g. an f-string-constructed key, which is fresh, `rc: 1`, and already safely owned by nothing else), so no source-kind-specific branching is needed for this half of the fix.

- [ ] **Step 1: Add `collect_stmt_bindings`'s `DictCompAssign`/`SetCompAssign` arms**

  ```rust
  MirStmt::DictCompAssign { target, var, var_ty, key, value, .. } => {
      bindings.entry(var.clone()).or_insert_with(|| var_ty.clone());
      bindings
          .entry(target.clone())
          .or_insert(pycc_mir::Ty::Dict(Box::new((key.ty(), value.ty()))));
  }
  MirStmt::SetCompAssign { target, var, var_ty, elt, .. } => {
      bindings.entry(var.clone()).or_insert_with(|| var_ty.clone());
      bindings.entry(target.clone()).or_insert(pycc_mir::Ty::Set(Box::new(elt.ty())));
  }
  ```

- [ ] **Step 2: Add the `emit_stmt` arms, reusing Task 5a's loop-building shape**

  `SetCompAssign` is structurally identical to `ListCompAssign` (Task 5a) — same allocate-then-loop-then-conditionally-insert shape, substituting `rt.int_set_new`/`build_int_set_add` for `rt.int_list_new`/`build_int_list_append`. `DictCompAssign` differs by evaluating **two** expressions (`key`/`value`) per taken iteration instead of one (`elt`), and by this task's own required fix:

  ```rust
  MirStmt::DictCompAssign { target, var, var_ty, source, cond, key, value } => {
      let new_dict = builder
          .build_call(rt.dict_new, &[], "comp_dict_new")
          .expect("build_call should not fail for a well-formed dict allocation")
          .try_as_basic_value()
          .expect_basic("pycc_rt_dict_new returns a non-void pointer")
          .into_pointer_value();
      emit_assign(context, builder, locals, target, Scalar::Dict(new_dict));

      // Loop-building: identical shape to Task 5a's `ListCompAssign` arm,
      // including the identical `Dict`-source `rt.str_incref`-on-read
      // requirement for `var`'s own per-iteration binding -- unlike
      // `ListCompAssign`, this combination (`Dict` source, `Dict`-producing
      // comprehension) IS reachable from real source (`{k: 1 for k in d}`),
      // so this path has a real end-to-end test below, not only an
      // internal-consistency one.

      // Inside the loop body (after the optional `cond` branch, identical
      // shape to Task 5a's point 3):
      //   let key_scalar = emit_expr(..., key);
      //   let key_scalar = incref_if_str_duplicate(builder, rt, key, key_scalar); // THE FIX -- see this task's own header note; mirrors MirStmt::DictSet's own call exactly, unconditionally, regardless of `source`
      //   let Scalar::Str(key_ptr) = key_scalar else { panic!("pycc_codegen: internal error: dict comprehension key did not evaluate to str -- pycc_types::check (T0036) should have rejected this before codegen") };
      //   let value_scalar = emit_expr(..., value);
      //   let tagged = to_tagged_int(context, builder, value_scalar);
      //   let raw = build_untag_checked(builder, rt, tagged, "dict_comp_untag_value");
      //   build_dict_set(builder, rt, new_dict, key_ptr, raw);
  }
  ```

- [ ] **Step 3: Tests**

  `SetCompAssign`: (a) `evens = {x for x in range(10) if x % 2 == 0}` (Range source, filter); (b) a `List`/`Set`-sourced set comprehension without a filter. `DictCompAssign`: (c) `named = {f"n{i}": i for i in range(3)}` (Range source, f-string key — the common case, no aliasing hazard); (d) **`d2 = {k: 1 for k in d}`** where `d` is a pre-existing `dict[str, int]` (`Dict` source) — this is this task's own load-bearing safety test: after building `d2`, read `d` again (e.g. `print(d.get("a", -1))` for a key known to be in `d`) and confirm `d`'s own contents are unaffected/not use-after-freed, and separately confirm `d2`'s own contents are correct and independently readable. Without Step 2's `incref_if_str_duplicate` fix, this test is the one this whole task exists to make pass safely (under a sanitizer or `cargo miri`, if available in this project's own test tooling — check `docs/TESTING.md` for whether Miri/ASan is already wired into any existing test target and reuse that harness here if so, since a plain `cargo test` run alone may not reliably surface a use-after-free on every platform). Run `cargo llvm-cov -p pycc_codegen` and confirm 100% (combined with Task 5a's own coverage on this same crate) — the combined Task 5a+5b surface is very likely the largest test-writing surface in this whole plan.

---

## Task 6: `pycc_hir` — Slicing frontend (`HirExpr::Slice`)

**Files:**
- Modify: `crates/pycc_hir/src/lib.rs`; modify `crates/pycc_ast/src/lib.rs` only if Task 2's own Step 0 has not already run first in whatever order the implementer chooses (this task additionally needs `ExprSlice` added to the same curated re-export list — check via `grep -n "ExprSlice" crates/pycc_ast/src/lib.rs` before duplicating Step 0's edit).

**Interfaces:**
- Produces: `HirExpr::Slice { base, start, stop, step }`.
- Consumes: `ruff_python_ast::ExprSlice` (`generated.rs:9945` of the vendored `ruff_python_ast-0.0.6`, already confirmed present; reached via `Expr::Subscript`'s existing `slice` field, which is `Expr::Slice(...)` for a colon-containing subscript, distinct from a plain single-expression `slice` field for ordinary indexing), re-exported through `pycc_ast` exactly like Task 2's own `Comprehension`/`ExprListComp`/`ExprSetComp`/`ExprDictComp` fix (same underlying gap: `pycc_ast`'s facade is curated, not blanket, and `pycc_hir` depends only on `pycc_ast`, never `ruff_python_ast` directly). This task's own code below only needs `ExprSlice`'s *fields* (`lower`/`upper`/`step`) via pattern-matching (`Expr::Slice(slice) => ...`, then `slice.lower`/`.upper`/`.step`), which needs no import at all — but if any later step in this task ends up needing to spell `ExprSlice` out as an explicit parameter type (unlikely given the sketch below), add it to the re-export list the same way.

- [ ] **Step 1: Add the `HirExpr::Slice` variant**

  Immediately after the existing `Subscript { .. }` variant:

  ```rust
  /// `base[start:stop:step]` (PR-12, D-118). Each bound is independently
  /// optional, matching Python's own slice grammar (`xs[:3]`, `xs[2:]`,
  /// `xs[:]`, `xs[::2]` all parse). Unlike `Subscript`'s `index` (a plain
  /// `HirExpr`), each bound here is an `Option<Box<HirExpr>>` -- an
  /// omitted bound has no source expression to lower at all, and
  /// defaulting it to a literal `0`/some sentinel here would be
  /// incorrect: `stop`'s default is `len(base)`, which needs `base`'s own
  /// already-lowered value to compute, not a value knowable at this
  /// lowering step in isolation. `pycc_types`/`pycc_mir`/`pycc_codegen`
  /// each apply the actual default at the point they have enough context
  /// to do so correctly.
  Slice {
      base: Box<HirExpr>,
      start: Option<Box<HirExpr>>,
      stop: Option<Box<HirExpr>>,
      step: Option<Box<HirExpr>>,
  },
  ```

- [ ] **Step 2: Wire `Expr::Subscript`'s existing arm to distinguish a slice from a plain index**

  Replace the existing:
  ```rust
  Expr::Subscript(sub) => HirExpr::Subscript {
      base: Box::new(lower_expr(&sub.value)?),
      index: Box::new(lower_expr(&sub.slice)?),
  },
  ```
  with:
  ```rust
  Expr::Subscript(sub) => match sub.slice.as_ref() {
      Expr::Slice(slice) => HirExpr::Slice {
          base: Box::new(lower_expr(&sub.value)?),
          start: slice.lower.as_deref().map(lower_expr).transpose()?.map(Box::new),
          stop: slice.upper.as_deref().map(lower_expr).transpose()?.map(Box::new),
          step: slice.step.as_deref().map(lower_expr).transpose()?.map(Box::new),
      },
      _ => HirExpr::Subscript {
          base: Box::new(lower_expr(&sub.value)?),
          index: Box::new(lower_expr(&sub.slice)?),
      },
  },
  ```

  (`ExprSlice`'s field names — `lower`/`upper`/`step`, all `Option<Box<Expr>>` — confirmed against `ruff_python_ast-0.0.6`'s `generated.rs:9945-9951` during this plan's own research: `pub struct ExprSlice { node_index, range, lower: Option<Box<Expr>>, upper: Option<Box<Expr>>, step: Option<Box<Expr>> }` — note the field is named `lower`/`upper`, not `start`/`stop`, in the upstream AST; this plan's own `HirExpr::Slice` uses `start`/`stop` for readability, so the mapping is `slice.lower → start`, `slice.upper → stop`.)

- [ ] **Step 3: Add `rename_name_in_expr`'s `Slice` arm**

  (Only needed if Task 6 runs after Task 2 in whatever order the implementer chooses; if it runs before, Task 2's Step 3 must add this arm instead. Either way, exactly one of the two tasks adds it — do not duplicate.)

  ```rust
  HirExpr::Slice { base, start, stop, step } => HirExpr::Slice {
      base: Box::new(recurse(*base)),
      start: start.map(|s| Box::new(recurse(*s))),
      stop: stop.map(|s| Box::new(recurse(*s))),
      step: step.map(|s| Box::new(recurse(*s))),
  },
  ```

- [ ] **Step 4: Tests**

  Cover: (a) `xs[1:3]` lowers to `HirExpr::Slice` with both bounds `Some`, `step: None`; (b) `xs[:3]`/`xs[2:]`/`xs[:]` each lower with the expected `None` bounds; (c) `xs[::2]` lowers with `step: Some`; (d) an ordinary single-expression subscript (`xs[0]`) still lowers to the existing `HirExpr::Subscript`, unaffected by this change (regression pin); (e) a slice expression's `base`/bounds are recursively lowered correctly (e.g. `xs[f():g()]` for some already-lowerable `f()`/`g()` shape). Run `cargo llvm-cov -p pycc_hir` and confirm 100% (combined with Task 2's own coverage — both tasks touch this same crate, so run the full crate's coverage after both are complete, not each task in isolation).

---

## Task 7: `pycc_types` — Slicing type-checking

**Files:**
- Modify: `crates/pycc_types/src/lib.rs`

**Interfaces:**
- Consumes: `HirExpr::Slice` (Task 6).
- Produces: type-checking reusing `T0033`/`T0021`; result type `Ty::List(Int)`.

- [ ] **Step 1: Re-locate `infer_expr_in`'s existing `Subscript` arm**

  ```bash
  grep -n "HirExpr::Subscript" crates/pycc_types/src/lib.rs
  ```

  Read its existing base-type-gate shape (list-only, `T0033` on a non-list base) to mirror exactly.

- [ ] **Step 2: Add the `HirExpr::Slice` arm**

  ```rust
  HirExpr::Slice { base, start, stop, step } => {
      let base_ty = infer_expr_in(env, local_names, base)?;
      let Ty::List(elem_ty) = &base_ty else {
          return Err(Diagnostic::error(
              "T0033",
              format!("`{}` does not support slicing (only list[int] does)", base_ty.name()),
              Span::new(0, 0),
          ));
      };
      if **elem_ty != Ty::Int {
          return Err(Diagnostic::error(
              "T0034",
              format!(
                  "list codegen only supports `list[int]` in v0.2, cannot slice `list[{}]`",
                  elem_ty.name()
              ),
              Span::new(0, 0),
          ));
      }
      for (label, bound) in [("start", start), ("stop", stop), ("step", step)] {
          if let Some(bound) = bound {
              let bound_ty = infer_expr_in(env, local_names, bound)?;
              if !is_assignable(bound_ty.clone(), Ty::Int) {
                  return Err(Diagnostic::error(
                      "T0021",
                      format!("slice {label} must be `int`, got `{}`", bound_ty.name()),
                      Span::new(0, 0),
                  ));
              }
          }
      }
      Ok(base_ty.clone())
  }
  ```

  **Note the diagnostic-order decision here, worth pinning with its own test:** the base-type gate (`T0033`) is checked before the element-type gate (`T0034`), which is checked before any bound's type (`T0021`) — mirroring this file's existing "callee/base-type errors before argument errors" convention (D-110's own callee-first precedent, applied here to base-type-before-bound-type). Add this as its own arm to **both** `infer_expr` (module scope, delegating to `infer_expr_in` with an empty `local_names`, matching every other expression arm's existing split) and `infer_expr_in` (function scope) — confirm via `grep -n "fn infer_expr\b" crates/pycc_types/src/lib.rs` whether `HirExpr` variants are matched once in a shared function both callers use, or duplicated; this plan's own reading suggests `infer_expr` is a thin wrapper delegating straight into `infer_expr_in`, in which case only **one** arm is needed, not two — verify before assuming duplication.

- [ ] **Step 3: Add the solver (`collect_expr_constraints`) arm**

  Following the same "container operations get no unification term, mirroring existing precedent" pattern Task 3 Step 5 established for comprehensions — but note that unlike a comprehension *statement* (which produces a container the solver simply can't type at all), a slice *expression*'s `base` might itself be a normal, already-inferable sub-expression the solver needs to keep walking into (e.g. `some_param[1:3]` inside a private helper, where `some_param`'s own type is exactly what the solver is trying to pin down). Recurse into `base`/`start`/`stop`/`step` for constraint-collection purposes (so any nested inferable sub-expression is still visited), but do not attempt to produce a term for the `Slice` expression's own overall type — mirror whatever the existing `Subscript` arm in this same solver function already does verbatim (check via `grep -n "HirExpr::Subscript" crates/pycc_types/src/lib.rs` which of the several matches is inside `collect_expr_constraints` specifically), since a slice's own constraint-collection needs are structurally identical to a subscript's.

- [ ] **Step 4: Tests**

  Cover: (a) `xs[1:3]` on a `list[int]` type-checks, result `Ty::List(Int)`; (b) each of the three bounds omitted individually and all three together (`xs[:]`); (c) a non-list base (`Ty::Dict`/`Ty::Set`/`Ty::Tuple`/scalar) rejected with `T0033`; (d) a `list[str]`/`list[float]`/`list[bool]` base rejected with `T0034`; (e) a non-`int` bound (e.g. a `float` or `str` expression) rejected with `T0021`, for each of `start`/`stop`/`step` independently; (f) the base-before-element-before-bound diagnostic-order pin from Step 2's own note; (g) a `bool` bound is accepted (mirrors `is_assignable`'s existing `bool`-widens-to-`int` rule, exercised the same way `.append()`'s own value-type check already is). Run `cargo llvm-cov -p pycc_types` and confirm 100% (combined with Task 3's own coverage on this same crate).

---

## Task 8: `pycc_mir` — Slicing lowering (`MirExpr::Slice`)

**Files:**
- Modify: `crates/pycc_mir/src/lib.rs`

**Interfaces:**
- Consumes: `HirExpr::Slice` (Task 6, already type-checked by Task 7).
- Produces: `MirExpr::Slice { base, start, stop, step }`, `ty()` deriving `base.ty()` unchanged (a `list[int]` slice is still `list[int]`).

- [ ] **Step 1: Add the `MirExpr::Slice` variant**

  ```rust
  /// `base[start:stop:step]` (mirrors `HirExpr::Slice`, PR-12, D-118).
  /// `ty()` below returns `base.ty()` unchanged -- slicing a `list[int]`
  /// always produces another `list[int]`, unlike `Subscript`'s own
  /// element-type-narrowing `ty()` derivation.
  Slice {
      base: Box<MirExpr>,
      start: Option<Box<MirExpr>>,
      stop: Option<Box<MirExpr>>,
      step: Option<Box<MirExpr>>,
  },
  ```

  Add the corresponding `ty()` arm: `MirExpr::Slice { base, .. } => base.ty(),`.

- [ ] **Step 2: Add the `lower_expr` arm**

  ```rust
  HirExpr::Slice { base, start, stop, step } => MirExpr::Slice {
      base: Box::new(lower_expr(base, scopes)),
      start: start.as_deref().map(|e| Box::new(lower_expr(e, scopes))),
      stop: stop.as_deref().map(|e| Box::new(lower_expr(e, scopes))),
      step: step.as_deref().map(|e| Box::new(lower_expr(e, scopes))),
  },
  ```

- [ ] **Step 3: Tests**

  Cover: (a) `xs[1:3]` lowers with both bounds `Some`; (b) each omitted-bound combination lowers with the corresponding `None`; (c) `ty()` returns `Ty::List(Int)` for a `list[int]`-based slice, unaffected by which bounds are present. Run `cargo llvm-cov -p pycc_mir` and confirm 100% (combined with Task 4's own coverage).

---

## Task 9: `pycc_codegen` + `pycc_rt` — Slicing codegen (`pycc_rt_int_list_slice`, reusing `Scalar::List`)

**Files:**
- Modify: `crates/pycc_rt/src/lib.rs`, `crates/pycc_codegen/src/lib.rs`

**Interfaces:**
- Produces: `pycc_rt_int_list_slice` (new `pycc_rt` FFI function); real `emit_expr` codegen for `MirExpr::Slice`.

- [ ] **Step 1: Add `pycc_rt_int_list_slice`, private-logic-plus-public-wrapper split**

  Mirrors `int_list_get`/`pycc_rt_int_list_get`'s own existing split (a panicking private fn, a thin `extern "C"` public wrapper) — add immediately after `pycc_rt_int_list_len`:

  ```rust
  fn int_list_slice(list: &PyIntListObj, start: i64, stop: i64, step: i64) -> *mut PyIntListObj {
      if start < 0 {
          panic!("pycc_rt: slice start must be non-negative");
      }
      if stop < 0 {
          panic!("pycc_rt: slice stop must be non-negative");
      }
      if step <= 0 {
          panic!("pycc_rt: slice step must be positive");
      }
      let items = list.items.take();
      let len = items.len() as i64;
      let clamped_start = start.min(len);
      let clamped_stop = stop.min(len);
      let result = pycc_rt_int_list_new();
      let mut i = clamped_start;
      while i < clamped_stop {
          unsafe { pycc_rt_int_list_append(result, items[i as usize]) };
          i += step;
      }
      list.items.set(items);
      result
  }

  /// Returns a **new** list containing the clamped, strided sub-range
  /// `[start, stop)` of `list`'s elements, stepping by `step` (Python's
  /// `list[start:stop:step]`, D-118's v0.2 `list[int]` slice). Panics on
  /// a negative `start`/`stop` or a non-positive `step` -- v0.2 ships no
  /// CPython-style negative-index/negative-step semantics, extending
  /// D-108's own uniform "no negative addressing" scope cut to slicing.
  /// `start`/`stop` are clamped into `[0, len]` after the sign check,
  /// matching CPython's own out-of-range-slice-bound clamping -- required
  /// for the accepted subset (omitted/over-long bounds) to match CPython
  /// byte-for-byte, not merely a nicety.
  ///
  /// # Element representation
  /// `start`/`stop`/`step` are raw, untagged `i64` offsets/strides, not
  /// D-061-tagged `Ty::Int` values -- a caller with a tagged operand must
  /// `pycc_rt_int_untag_checked` each one first, exactly like
  /// `pycc_rt_int_list_get`'s own `index` parameter. The returned list's
  /// own elements are copied through unchanged (already raw, untagged
  /// `i64`s per `PyIntListObj`'s own representation, D-106) -- no
  /// per-element tag/untag conversion is needed here at all, unlike a
  /// single-element read.
  ///
  /// # Safety
  /// `list` must be a live `PyIntListObj` pointer.
  #[unsafe(no_mangle)]
  pub unsafe extern "C" fn pycc_rt_int_list_slice(
      list: *const PyIntListObj,
      start: i64,
      stop: i64,
      step: i64,
  ) -> *mut PyIntListObj {
      int_list_slice(unsafe { &*list }, start, stop, step)
  }
  ```

- [ ] **Step 2: `pycc_rt` tests — one per D-118 branch**

  ```rust
  #[test]
  fn pycc_rt_int_list_slice_returns_the_in_range_sub_range() { /* [10,20,30,40,50][1:4:1] == [20,30,40] */ }
  #[test]
  fn pycc_rt_int_list_slice_clamps_a_too_high_stop() { /* [1,2,3][0:100:1] == [1,2,3] */ }
  #[test]
  fn pycc_rt_int_list_slice_clamps_a_too_high_start() { /* [1,2,3][100:200:1] == [] (start clamped to len, still < stop after clamp only if stop also clamps -- verify: both clamp to len, so start==stop, empty result) */ }
  #[test]
  fn pycc_rt_int_list_slice_with_start_at_or_past_stop_is_empty() { /* [1,2,3][2:1:1] == [] */ }
  #[test]
  #[should_panic(expected = "pycc_rt: slice start must be non-negative")]
  fn pycc_rt_int_list_slice_rejects_negative_start() { /* call the private int_list_slice directly, per this file's established panic-testing convention */ }
  #[test]
  #[should_panic(expected = "pycc_rt: slice stop must be non-negative")]
  fn pycc_rt_int_list_slice_rejects_negative_stop() { }
  #[test]
  #[should_panic(expected = "pycc_rt: slice step must be positive")]
  fn pycc_rt_int_list_slice_rejects_zero_step() { }
  #[test]
  #[should_panic(expected = "pycc_rt: slice step must be positive")]
  fn pycc_rt_int_list_slice_rejects_negative_step() { }
  #[test]
  fn pycc_rt_int_list_slice_with_a_step_greater_than_one_skips_elements() { /* [0,1,2,3,4,5][0:6:2] == [0,2,4] */ }
  ```

  (Test bodies elided — construct via `pycc_rt_int_list_new`/`_append` exactly like this file's own existing `list`-family tests already do, e.g. the pattern at `crates/pycc_rt/src/lib.rs:2356-2368`.) Run `cargo llvm-cov -p pycc_rt` and confirm every branch inside `int_list_slice` is hit.

- [ ] **Step 3: `pycc_codegen`'s `emit_expr` arm for `MirExpr::Slice`**

  ```rust
  MirExpr::Slice { base, start, stop, step } => {
      let base_scalar = emit_expr(context, builder, module, rt, user_functions, locals, base);
      let base_ptr = expect_list_pointer(base_scalar, "the sliced value");
      let len_i64 = build_int_list_len(builder, rt, base_ptr); // reuses len()'s own existing call site
      let start_i64 = match start {
          Some(e) => build_untag_checked(
              builder,
              rt,
              to_tagged_int(context, builder, emit_expr(context, builder, module, rt, user_functions, locals, e)),
              "slice_untag_start",
          ),
          None => context.i64_type().const_int(0, false),
      };
      let stop_i64 = match stop {
          Some(e) => build_untag_checked(
              builder,
              rt,
              to_tagged_int(context, builder, emit_expr(context, builder, module, rt, user_functions, locals, e)),
              "slice_untag_stop",
          ),
          None => len_i64,
      };
      let step_i64 = match step {
          Some(e) => build_untag_checked(
              builder,
              rt,
              to_tagged_int(context, builder, emit_expr(context, builder, module, rt, user_functions, locals, e)),
              "slice_untag_step",
          ),
          None => context.i64_type().const_int(1, false),
      };
      let result_ptr = build_int_list_slice(builder, rt, base_ptr, start_i64, stop_i64, step_i64);
      Scalar::List(result_ptr)
  }
  ```

  `build_int_list_len`/`build_untag_checked`/`expect_list_pointer` all already exist under these exact names and signatures (`crates/pycc_codegen/src/lib.rs:701`/`659`/`640`, confirmed during this plan's own research) — only `build_call_pycc_rt_int_list_slice` is new, added by Step 4 below following the same `RtFns`-field-plus-thin-wrapper shape Task 11's own Step 3 note already establishes for `pycc_rt_int_list_pop`/`pycc_rt_dict_get_or_default` (a new `int_list_slice: FunctionValue<'ctx>` field on `RtFns<'ctx>`, `crates/pycc_codegen/src/lib.rs:171`, declared alongside `int_list_new`'s own `declare(...)` call at line 359, plus a `build_int_list_slice` wrapper mirroring `build_int_list_get`'s own one-`build_call`-plus-`.expect(...)` shape — rename the sketch's illustrative `build_call_pycc_rt_int_list_slice` to `build_int_list_slice` to match this crate's own existing `build_int_list_*` naming convention exactly, not the `pycc_rt_`-prefixed name, which belongs to the `pycc_rt` side only).

- [ ] **Step 4: Add `int_list_slice` to `RtFns` and declare it**

  Add `int_list_slice: FunctionValue<'ctx>` to the `RtFns<'ctx>` struct (`crates/pycc_codegen/src/lib.rs:171`) and its own `declare("pycc_rt_int_list_slice", ...)` call at `RtFns`'s construction site, mirroring `int_list_new`'s exact pattern (line 359) — its LLVM signature is 4 `i64` parameters (the `list` pointer plus `start`/`stop`/`step`) returning the same opaque pointer type every other `PyIntListObj`-returning function already declares (confirm the exact pointer type used there, e.g. `ptr_type` per `inkwell`'s opaque-pointer LLVM 22 configuration, matching `int_list_new`'s own `ptr_type.fn_type(&[], false)`-style declaration one line away). Add the `build_int_list_slice` wrapper function itself (Step 3's own note above).

- [ ] **Step 5: Tests**

  Cover, at the `pycc_codegen` level (real end-to-end compiled-and-run tests, mirroring this crate's own existing list/dict/set test style): (a) `xs[1:3]` on a `[10,20,30,40,50]`-literal list produces `[20,30]`; (b) each omitted-bound form; (c) a step of `2`; (d) the resulting slice is a genuinely independent list (mutating the original via `.append()` afterward does not affect the slice's own contents, or vice versa) — pins that `pycc_rt_int_list_slice` really allocates a new backing object rather than aliasing. Run `cargo llvm-cov -p pycc_codegen` and confirm 100% (combined with Task 5a/5b's own coverage on this same crate).

---

## Task 10: Container methods — frontend + type-checking (`list.pop()`, `dict.get(key, default)`, `set.add(value)`)

**Files:**
- Modify: `crates/pycc_hir/src/lib.rs`, `crates/pycc_types/src/lib.rs`

**Interfaces:**
- Produces: `HirExpr::ListPop { list }`, `HirExpr::DictGetOrDefault { dict, key, default }`, `HirExpr::SetAdd { set, value }`; type-checking for all three, reusing `T0033`/`T0021`.

- [ ] **Step 1: Re-verify `lower_expr`'s existing `.append()`-recognition block**

  ```bash
  grep -n "attr.attr.as_str() == \"append\"" crates/pycc_hir/src/lib.rs
  ```

  Confirm the `Expr::Call`'s `if let Expr::Attribute(attr) = call.func.as_ref() { if attr.attr.as_str() == "append" { ... } return Err(unsupported(format!("only the \`.append()\` method is supported so far, got \`.{}(...)\`", attr.attr), call.range)); }` shape (this plan's own research, `crates/pycc_hir/src/lib.rs:717-746`) still holds — this task's three new methods are added as sibling `if` branches inside the same `Expr::Attribute` check, before its final catch-all `Err(unsupported(...))`.

- [ ] **Step 2: Add the three new `HirExpr` variants**

  ```rust
  /// `list.pop()` (PR-12, D-119): a hand-recognized special form, mirroring
  /// `ListAppend`'s own shape exactly (no general method-call dispatch).
  /// No-argument form only -- removes and returns the list's last element.
  ListPop {
      list: String,
  },
  /// `dict.get(key, default)` (PR-12, D-119): exactly two arguments --
  /// returns `default` if `key` is absent, else the stored value. Mirrors
  /// `ListAppend`'s hand-recognized shape.
  DictGetOrDefault {
      dict: String,
      key: Box<HirExpr>,
      default: Box<HirExpr>,
  },
  /// `set.add(value)` (PR-12, D-119): mirrors `ListAppend`'s shape
  /// exactly -- dedups on insert, exactly like set-literal construction
  /// already does (`pycc_rt_int_set_add`, already shipped by PR-11a).
  SetAdd {
      set: String,
      value: Box<HirExpr>,
  },
  ```

- [ ] **Step 3: Wire the three methods into `lower_expr`'s `Expr::Call`/`Expr::Attribute` handling**

  ```rust
  if let Expr::Attribute(attr) = call.func.as_ref() {
      let Expr::Name(base_name) = attr.value.as_ref() else {
          return Err(unsupported(
              format!("`.{}()` is only supported on a bare-name value so far", attr.attr),
              pycc_ast::expr_range(&attr.value),
          ));
      };
      match attr.attr.as_str() {
          "append" => {
              // ... existing body, unchanged ...
          }
          "pop" => {
              let [] = &*call.arguments.args else {
                  return Err(unsupported(
                      format!(
                          "list.pop() takes no arguments, got {}",
                          call.arguments.args.len()
                      ),
                      call.range,
                  ));
              };
              return Ok(HirExpr::ListPop {
                  list: base_name.id.as_str().to_string(),
              });
          }
          "get" => {
              let [key, default] = &*call.arguments.args else {
                  return Err(unsupported(
                      format!(
                          "dict.get() takes exactly two arguments (key, default), got {}",
                          call.arguments.args.len()
                      ),
                      call.range,
                  ));
              };
              return Ok(HirExpr::DictGetOrDefault {
                  dict: base_name.id.as_str().to_string(),
                  key: Box::new(lower_expr(key)?),
                  default: Box::new(lower_expr(default)?),
              });
          }
          "add" => {
              let [value] = &*call.arguments.args else {
                  return Err(unsupported(
                      format!(
                          "set.add() takes exactly one argument, got {}",
                          call.arguments.args.len()
                      ),
                      call.range,
                  ));
              };
              return Ok(HirExpr::SetAdd {
                  set: base_name.id.as_str().to_string(),
                  value: Box::new(lower_expr(value)?),
              });
          }
          other => {
              return Err(unsupported(
                  format!("only the `.append()`/`.pop()`/`.get()`/`.add()` methods are supported so far, got `.{other}(...)`"),
                  call.range,
              ));
          }
      }
  }
  ```

  **Note:** the existing code's `Expr::Name(list_name) = attr.value.as_ref() else { ... }` bare-name check was previously duplicated inline only for `.append()`; this refactor hoists it once for all four methods (Step 3's own `let Expr::Name(base_name) = ...` at the top) — confirm the existing `.append()` arm's own error message wording (`"\`.append()\` is only supported on a bare-name list so far"`) and decide whether to keep per-method wording or unify it as shown above; either is defensible, but pick one and update any existing test asserting the old `.append()`-specific wording if it changes.

- [ ] **Step 4: Add `rename_name_in_expr` arms for the three new variants**

  ```rust
  HirExpr::ListPop { list } => HirExpr::ListPop { list },
  HirExpr::DictGetOrDefault { dict, key, default } => HirExpr::DictGetOrDefault {
      dict,
      key: Box::new(recurse(*key)),
      default: Box::new(recurse(*default)),
  },
  HirExpr::SetAdd { set, value } => HirExpr::SetAdd {
      set,
      value: Box::new(recurse(*value)),
  },
  ```

  (`list`/`dict`/`set` base-name fields are plain `String`s, not renamed unless they equal `from` — mirroring `ListAppend`'s own existing arm; a comprehension's `elt` referencing e.g. `d.get(k, 0)` where `d` is some *other*, non-loop-variable dict is the common case and must not be touched.)

- [ ] **Step 5: `pycc_types` — add the three type-checking arms**

  Mirror `ListAppend`'s exact existing shape (`T0033` on wrong base type, `T0021` on wrong value type) — add to both `infer_expr`/`infer_expr_in` (or their single shared function, per Task 7 Step 2's own note about confirming whether these are one function or two):

  ```rust
  HirExpr::ListPop { list } => {
      let list_ty = lookup_bound_name(env, local_names, list)?;
      let Ty::List(elem_ty) = &list_ty else {
          return Err(Diagnostic::error(
              "T0033",
              format!("`{}` does not support `.pop()`", list_ty.name()),
              Span::new(0, 0),
          ));
      };
      Ok((**elem_ty).clone())
  }
  HirExpr::DictGetOrDefault { dict, key, default } => {
      let dict_ty = lookup_bound_name(env, local_names, dict)?;
      let Ty::Dict(kv) = &dict_ty else {
          return Err(Diagnostic::error(
              "T0033",
              format!("`{}` does not support `.get()`", dict_ty.name()),
              Span::new(0, 0),
          ));
      };
      let key_ty = infer_expr_in(env, local_names, key)?;
      if !is_assignable(key_ty.clone(), kv.0.clone()) {
          return Err(Diagnostic::error(
              "T0021",
              format!("cannot look up a `{}` key in a dict of `{}` keys", key_ty.name(), kv.0.name()),
              Span::new(0, 0),
          ));
      }
      let default_ty = infer_expr_in(env, local_names, default)?;
      if !is_assignable(default_ty.clone(), kv.1.clone()) {
          return Err(Diagnostic::error(
              "T0021",
              format!("cannot use a `{}` default for a dict of `{}` values", default_ty.name(), kv.1.name()),
              Span::new(0, 0),
          ));
      }
      Ok(kv.1.clone())
  }
  HirExpr::SetAdd { set, value } => {
      let set_ty = lookup_bound_name(env, local_names, set)?;
      let Ty::Set(elem_ty) = &set_ty else {
          return Err(Diagnostic::error(
              "T0033",
              format!("`{}` does not support `.add()`", set_ty.name()),
              Span::new(0, 0),
          ));
      };
      let value_ty = infer_expr_in(env, local_names, value)?;
      if !is_assignable(value_ty.clone(), (**elem_ty).clone()) {
          return Err(Diagnostic::error(
              "T0021",
              format!("cannot add `{}` to a set of `{}`", value_ty.name(), elem_ty.name()),
              Span::new(0, 0),
          ));
      }
      Ok(Ty::None)
  }
  ```

  Also add the same three arms to the solver's `collect_expr_constraints` (as no-op-producing recursion, mirroring `ListAppend`'s own existing solver treatment — check via `grep -n "HirExpr::ListAppend" crates/pycc_types/src/lib.rs` which arm is inside `collect_expr_constraints` specifically and copy its exact shape).

- [ ] **Step 6: Tests**

  `pycc_hir`: (a) `xs.pop()` lowers to `ListPop`; (b) `xs.pop(0)` rejected (wrong arity); (c) `d.get("a", 0)` lowers to `DictGetOrDefault`; (d) `d.get("a")` rejected (wrong arity); (e) `s.add(1)` lowers to `SetAdd`; (f) an unrecognized method name (e.g. `.remove()`) still rejected with the widened catch-all message. `pycc_types`: (g) `.pop()` on a `list[int]` infers `Ty::Int`; (h) `.pop()` on a non-list rejected `T0033`; (i) `.get()` on `dict[str,int]` with a `str` key and `int` default infers `Ty::Int`; (j) `.get()` with a wrong-typed key or default each rejected `T0021`; (k) `.get()` on a non-dict rejected `T0033`; (l) `.add()` on `set[int]` with an `int`/`bool` value infers `Ty::None`; (m) `.add()` on a non-set rejected `T0033`; (n) `.add()` with a wrong-typed value rejected `T0021`. Run `cargo llvm-cov -p pycc_hir -p pycc_types` and confirm 100% (combined with prior tasks' own coverage on these two crates).

---

## Task 11: Container methods — MIR + codegen + `pycc_rt`

**Files:**
- Modify: `crates/pycc_mir/src/lib.rs`, `crates/pycc_codegen/src/lib.rs`, `crates/pycc_rt/src/lib.rs`

**Interfaces:**
- Consumes: `HirExpr::ListPop`/`DictGetOrDefault`/`SetAdd` (Task 10, already type-checked).
- Produces: `MirExpr::ListPop`/`DictGetOrDefault`/`SetAdd`; real codegen; one new `pycc_rt` function (`pycc_rt_int_list_pop`) plus one new codegen call site for the already-existing `pycc_rt_int_set_add`; one new `pycc_rt` function (`pycc_rt_dict_get_or_default`).

- [ ] **Step 1: `pycc_mir` — add the three `MirExpr` variants + `lower_expr`/`ty()` arms**

  ```rust
  /// `.pop()` (mirrors `HirExpr::ListPop`, PR-12, D-119). `ty()` returns
  /// the list's own element type -- empirically always `Ty::Int` (T0034
  /// rejects every other `list[T]` before codegen), derived rather than
  /// hardcoded for the identical reason `ListLiteral`'s own `ty()` derives
  /// (D-105's own precedent).
  ListPop {
      list: String,
      ty: Ty,
  },
  DictGetOrDefault {
      dict: String,
      key: Box<MirExpr>,
      default: Box<MirExpr>,
      ty: Ty,
  },
  /// `.add()` always returns `None`, exactly like `ListAppend` -- a true
  /// invariant, not narrowed by any gate, hardcoded on purpose (mirrors
  /// `ListAppend`'s own `ty()` arm).
  SetAdd {
      set: String,
      value: Box<MirExpr>,
  },
  ```

  `lower_expr` arms: `HirStmt`'s own environment (`scopes`) must supply the list/dict/set's element/key/value type via `lookup(scopes, list)` (mirroring `HirExpr::Subscript`'s own existing `lower_expr` arm, which already does this to derive its own `ty()` input — confirm via `grep -n "HirExpr::Subscript" crates/pycc_mir/src/lib.rs` for the exact lookup pattern to copy):

  ```rust
  HirExpr::ListPop { list } => {
      let Ty::List(elem_ty) = lookup(scopes, list) else {
          panic!("pycc_mir: internal error: `{list}` is not list-typed -- pycc_types::check should have rejected this HIR before it reached pycc_mir")
      };
      MirExpr::ListPop { list: list.clone(), ty: *elem_ty }
  }
  HirExpr::DictGetOrDefault { dict, key, default } => {
      let Ty::Dict(kv) = lookup(scopes, dict) else {
          panic!("pycc_mir: internal error: `{dict}` is not dict-typed -- pycc_types::check should have rejected this HIR before it reached pycc_mir")
      };
      MirExpr::DictGetOrDefault {
          dict: dict.clone(),
          key: Box::new(lower_expr(key, scopes)),
          default: Box::new(lower_expr(default, scopes)),
          ty: kv.1,
      }
  }
  HirExpr::SetAdd { set, value } => MirExpr::SetAdd {
      set: set.clone(),
      value: Box::new(lower_expr(value, scopes)),
  },
  ```

  `ty()` arms: `MirExpr::ListPop { ty, .. } => ty.clone()`, `MirExpr::DictGetOrDefault { ty, .. } => ty.clone()`, `MirExpr::SetAdd { .. } => Ty::None`.

- [ ] **Step 2: `pycc_rt` — add `pycc_rt_int_list_pop` and `pycc_rt_dict_get_or_default`**

  ```rust
  fn int_list_pop(list: &PyIntListObj) -> i64 {
      let mut items = list.items.take();
      let Some(value) = items.pop() else {
          list.items.set(items);
          panic!("pycc_rt: pop from empty list");
      };
      list.items.set(items);
      value
  }

  /// Removes and returns the last element (Python's `list.pop()`, D-119).
  /// Panics if `list` is empty, matching this file's "honest panic over
  /// silently wrong data" convention (CPython raises a catchable
  /// `IndexError` here; this compiler has no exception model, so this is
  /// an unrecoverable panic instead).
  ///
  /// # Element representation
  /// The returned value is a raw, untagged `i64` read straight out of the
  /// backing store -- a caller must `raw_i64_to_tagged_int` it before
  /// treating it as an ordinary `Ty::Int`, exactly like
  /// `pycc_rt_int_list_get`'s own return value.
  ///
  /// # Safety
  /// `list` must be a live `PyIntListObj` pointer.
  #[unsafe(no_mangle)]
  pub unsafe extern "C" fn pycc_rt_int_list_pop(list: *mut PyIntListObj) -> i64 {
      int_list_pop(unsafe { &*list })
  }
  ```

  ```rust
  fn dict_get_or_default(dict: &PyDictObj, key: *mut PyStrObj, default: i64) -> i64 {
      let entries = dict.entries.take();
      let found = entries
          .iter()
          .find(|(k, _)| unsafe { pycc_rt_str_cmp(*k, key) } == 0)
          .map(|(_, v)| *v);
      dict.entries.set(entries);
      found.unwrap_or(default)
  }

  /// Returns the value stored for `key`, or `default` if `key` is absent
  /// (Python's `dict.get(key, default)`, D-119) -- unlike
  /// `pycc_rt_dict_get`, this **never panics** on a missing key; that is
  /// the entire point of the two-argument form.
  ///
  /// # Safety
  /// Same as `pycc_rt_dict_get`.
  #[unsafe(no_mangle)]
  pub unsafe extern "C" fn pycc_rt_dict_get_or_default(
      dict: *mut PyDictObj,
      key: *mut PyStrObj,
      default: i64,
  ) -> i64 {
      dict_get_or_default(unsafe { &*dict }, key, default)
  }
  ```

  Tests: (a) `.pop()` on a non-empty list returns the last element and shrinks `len()` by one; (b) `.pop()` on an empty list panics with the exact message; (c) `.get()`-or-default on a present key returns the stored value; (d) `.get()`-or-default on a missing key returns `default`, never panicking. Run `cargo llvm-cov -p pycc_rt` and confirm 100%.

- [ ] **Step 3: `pycc_codegen` — declare the two new externs, add the three `emit_expr` arms**

  Three existing, confirmed helpers do the name-based base lookup this task needs: `emit_list_name_read`/`emit_dict_name_read`/`emit_set_name_read` (`crates/pycc_codegen/src/lib.rs:2195`/`2239`/`2275`), the exact functions `MirExpr::ListAppend`/`MirStmt::DictSet`/`MirStmt::ForSet` already use for their own plain-`String` base fields. `MirExpr::ListAppend`'s own arm (`crates/pycc_codegen/src/lib.rs:1976-1984`) is the exact template for `.pop()`'s tag/untag handling and its `Ty::None`-result shape — confirmed it returns `Scalar::Bool(context.i8_type().const_int(0, false))` for `None` (D-075's canonical carrier), **not** a `Scalar::None` variant, which does not exist. `MirExpr::DictGet`'s own arm (`crates/pycc_codegen/src/lib.rs:2062-2068`) is the exact template for `.get()`'s key handling: it extracts `Scalar::Str(key_ptr)` directly with **no** `incref_if_str_duplicate` call, because a read-only lookup never stores the key pointer anywhere persistent (unlike `MirStmt::DictSet`'s own key, which *is* incref'd, since `pycc_rt_dict_set` adopts it as a permanent reference, D-124) — `.get()`'s key must follow `DictGet`'s no-incref pattern, not `DictSet`'s, and this distinction is easy to get backwards by copying the wrong precedent.

  ```rust
  MirExpr::ListPop { list, .. } => {
      let list_ptr =
          emit_list_name_read(context, builder, module, rt, user_functions, locals, list);
      let raw = build_int_list_pop(builder, rt, list_ptr); // new: mirrors build_int_list_get's own declare-and-call shape
      Scalar::Int(raw_i64_to_tagged_int(context, builder, raw))
  }
  MirExpr::DictGetOrDefault { dict, key, default, .. } => {
      let dict_ptr =
          emit_dict_name_read(context, builder, module, rt, user_functions, locals, dict);
      let key_scalar = emit_expr(context, builder, module, rt, user_functions, locals, key);
      let Scalar::Str(key_ptr) = key_scalar else {
          panic!(
              "pycc_codegen: internal error: dict.get() key did not evaluate to str -- \
               pycc_types::check (T0021) should have rejected this before codegen"
          )
      };
      let default_scalar = emit_expr(context, builder, module, rt, user_functions, locals, default);
      let tagged_default = to_tagged_int(context, builder, default_scalar);
      let raw_default = build_untag_checked(builder, rt, tagged_default, "dict_get_untag_default");
      let raw = build_dict_get_or_default(builder, rt, dict_ptr, key_ptr, raw_default); // new
      Scalar::Int(raw_i64_to_tagged_int(context, builder, raw))
  }
  MirExpr::SetAdd { set, value } => {
      let set_ptr =
          emit_set_name_read(context, builder, module, rt, user_functions, locals, set);
      let value_scalar = emit_expr(context, builder, module, rt, user_functions, locals, value);
      let tagged = to_tagged_int(context, builder, value_scalar);
      let raw = build_untag_checked(builder, rt, tagged, "set_untag_added");
      build_int_set_add(builder, rt, set_ptr, raw); // ALREADY EXISTS (crates/pycc_codegen/src/lib.rs:823), built for SetLiteral's own per-element construction codegen -- this call site is its second consumer; zero new codegen wrapper or extern declaration needed for set.add() at all
      Scalar::Bool(context.i8_type().const_int(0, false)) // Ty::None's canonical carrier, mirroring ListAppend's own identical result exactly -- NOT `Scalar::None`, which does not exist
  }
  ```

  `build_int_list_pop`/`build_dict_get_or_default` are new thin wrapper functions this task adds, mirroring `build_int_list_get`/`build_dict_len`'s own existing one-`build_call`-plus-`.expect(...)` shape exactly (`crates/pycc_codegen/src/lib.rs:679`/`783`) — each needs its own new field on `RtFns<'ctx>` (`crates/pycc_codegen/src/lib.rs:171`, e.g. `int_list_pop: FunctionValue<'ctx>`, `dict_get_or_default: FunctionValue<'ctx>`) plus a `declare(...)` call at `RtFns`'s own construction site, mirroring `int_list_new`'s exact pattern (`crates/pycc_codegen/src/lib.rs:359`). `set.add()` needs neither: `int_set_add: FunctionValue<'ctx>` (line 235) and `build_int_set_add` (line 823) already exist end-to-end from `SetLiteral`'s own construction codegen — this task adds only a second call site, no new declaration.

- [ ] **Step 4: Tests**

  End-to-end compiled-and-run tests (mirroring `ListAppend`'s own existing test style): (a) `xs = [1,2,3]; y = xs.pop(); print(y); print(len(xs))` produces `3` then `2`; (b) `d = {"a": 1}; print(d.get("a", -1))` produces `1`; (c) `print(d.get("z", -1))` produces `-1`; (d) `s = {1,2}; s.add(3); print(len(s))` produces `3`; (e) `s.add(1)` (already present) does not grow `len(s)` (dedup). Run `cargo llvm-cov -p pycc_mir -p pycc_codegen` and confirm 100%.

---

## Task 12: Diagnostics registry + `tests/diagnostics/`

**Files:**
- Modify: `docs/DIAGNOSTICS.md`
- Add: new files under `tests/diagnostics/` (exact naming convention: confirm via `ls tests/diagnostics/` and follow whatever numbering/naming scheme is already established there — this plan's own research did not read that directory's contents in detail).

- [ ] **Step 1: Widen `T0033`'s row**

  In `docs/DIAGNOSTICS.md`, change:
  ```markdown
  | `T0033` | error | value does not support list/dict/set operations (subscript, item assignment, for, `.append()`, `len()`), or `len()` called with the wrong number of arguments |
  ```
  to:
  ```markdown
  | `T0033` | error | value does not support list/dict/set operations (subscript, slicing, item assignment, `for`, `.append()`, `.pop()`, `.get()`, `.add()`, `len()`), or a wrong argument count for any of `len()`/`.pop()`/`.get()`/`.add()` |
  ```

  No other row changes — `T0034`/`T0036`/`T0038` already describe the underlying rule generically enough to cover a comprehension-produced container without wording changes (verify their exact current wording via `grep -n "T0034\|T0036\|T0038" docs/DIAGNOSTICS.md` first, and only add a clarifying phrase if the current wording actually implies "literal only," which this plan's own research did not find it does).

- [ ] **Step 2: Add `tests/diagnostics/` cases**

  One case per genuinely new rejection path this plan adds: (a) slicing a non-list (`T0033`); (b) slicing a `list[str]` (`T0034`); (c) a non-`int` slice bound (`T0021`); (d) `.pop()` on a non-list (`T0033`); (e) `.pop()` with an argument (`T0033`, arity); (f) `.get()` on a non-dict (`T0033`); (g) `.get()` with a wrong-typed key/default (`T0021`, two cases); (h) `.get()` with the wrong argument count (`T0033`); (i) `.add()` on a non-set (`T0033`); (j) `.add()` with a wrong-typed value (`T0021`); (k) a comprehension with more than one `for` clause (`C0001`); (l) a comprehension with more than one `if` filter (`C0001`); (m) a comprehension used in a non-assignment position, e.g. as a `print(...)` argument (`C0001`, pins the "falls through to the existing generic catch-all" design decision from D-117); (n) a comprehension producing a non-`list[int]`/`dict[str,int]`/`set[int]` combination (`T0034`/`T0036`/`T0038`, three cases, one per container kind). Follow the existing `tests/diagnostics/` file format exactly (read at least two existing cases, e.g. whichever ones cover `T0034`/`T0036`/`T0038` from PR-10/PR-11, before writing new ones).

- [ ] **Step 3: Run the full diagnostics test suite** (`cargo test --test diagnostics` or whatever the existing invocation is — confirm via `grep -rn "diagnostics" .github/workflows/ci.yml` or `docs/TESTING.md` if unsure) and confirm every new case passes.

---

## Task 13: Conformance fixtures + docs sweep

**Files:**
- Add: `tests/fixtures/pep_0709_comp_inline.py`, `tests/fixtures/container_methods_slicing.py`
- Modify: `tests/conformance.rs`, `docs/PYTHON_STANDARDS.md`, `docs/ROADMAP.md`, `docs/RUNTIME.md`, `docs/DELIVERY_PLAN.md`

- [ ] **Step 1: Add `tests/fixtures/pep_0709_comp_inline.py`**

  ```python
  i = 100
  xs = [i * 2 for i in range(3)]
  print(i)
  for v in xs:
      print(v)
  ```

  Run the pinned `python3.14` oracle locally and record its exact stdout (expected, per this plan's own reasoning in D-120: `100`, then `0`, `2`, `4`, each on its own line) — **do not assume this plan's prediction is correct without actually running it**; adjust the fixture or this plan's own expectation if the oracle disagrees.

- [ ] **Step 2: Add `tests/fixtures/container_methods_slicing.py`**

  A single breadth fixture exercising slicing + all three new methods + both comprehension source-container combinations not already covered by the PEP-709 fixture, entirely via element-wise printing (per this plan's own container-`to_str` scope cut):

  ```python
  xs = [1, 2, 3, 4, 5]
  ys = xs[1:4]
  for v in ys:
      print(v)
  print(len(ys))

  xs.append(6)
  last = xs.pop()
  print(last)
  print(len(xs))

  named = {f"n{i}": i * i for i in range(4)}
  print(named.get("n2", -1))
  print(named.get("missing", -1))

  evens = {x for x in range(10) if x % 2 == 0}
  evens.add(100)
  print(len(evens))
  ```

  Run the pinned oracle and record its exact output before wiring the conformance test — **verify, do not guess**, especially the `evens` count (`{0,2,4,6,8}` plus `100` added = 6 elements; confirm this arithmetic against the real oracle output, not just this plan's own reasoning).

- [ ] **Step 3: Wire both fixtures into `tests/conformance.rs`**

  Mirror the exact existing pattern (`pep_0585_builtin_generics_matches_cpython_3_14_6_byte_for_byte`'s own shape, `tests/conformance.rs:299-307`) — both `--debug` and `--release` profiles, `#[ignore = "requires a pinned python3.14 (CPython 3.14.6) oracle on PATH"]`:

  ```rust
  #[test]
  #[ignore = "requires a pinned python3.14 (CPython 3.14.6) oracle on PATH"]
  fn pep_0709_comp_inline_matches_cpython_3_14_6_byte_for_byte() {
      let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pep_0709_comp_inline.py");
      let (debug_pycc, debug_cpython) =
          run_conformance_fixture_with_profile("pep_0709_comp_inline_debug", &fixture, false);
      assert_eq!(debug_pycc, debug_cpython, "pycc (--debug) and CPython 3.14.6 disagree on tests/fixtures/pep_0709_comp_inline.py");
      let (release_pycc, release_cpython) =
          run_conformance_fixture_with_profile("pep_0709_comp_inline_release", &fixture, true);
      assert_eq!(release_pycc, release_cpython, "pycc (--release) and CPython 3.14.6 disagree on tests/fixtures/pep_0709_comp_inline.py");
  }

  #[test]
  #[ignore = "requires a pinned python3.14 (CPython 3.14.6) oracle on PATH"]
  fn container_methods_slicing_matches_cpython_3_14_6_byte_for_byte() {
      let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/container_methods_slicing.py");
      let (debug_pycc, debug_cpython) =
          run_conformance_fixture_with_profile("container_methods_slicing_debug", &fixture, false);
      assert_eq!(debug_pycc, debug_cpython, "pycc (--debug) and CPython 3.14.6 disagree on tests/fixtures/container_methods_slicing.py");
      let (release_pycc, release_cpython) =
          run_conformance_fixture_with_profile("container_methods_slicing_release", &fixture, true);
      assert_eq!(release_pycc, release_cpython, "pycc (--release) and CPython 3.14.6 disagree on tests/fixtures/container_methods_slicing.py");
  }
  ```

  Run both locally (both `--debug` and `--release`) against the pinned oracle and confirm byte-for-byte agreement before committing, exactly per D-102's own established discipline — do **not** mark either as CI-observed in `docs/PYTHON_STANDARDS.md` (Step 4 below) until an actual CI run on all 5 Tier-1 targets confirms it, mirroring PR-11a/PR-11b's own identical "local-only for now" discipline for `dict_order.py`/`pep_0585_set_int.py`/`tuple_heterogeneous.py`.

- [ ] **Step 4: `docs/PYTHON_STANDARDS.md` — flip PEP 709's row**

  Change:
  ```markdown
  | [709](https://peps.python.org/pep-0709/) | Comprehension inlining semantics | sem | `py312/pep_0709_comp_inline.py` | ☐ |
  ```
  to (matching the flat `tests/fixtures/` naming convention this repository actually uses, not the design doc's illustrative nested-path table):
  ```markdown
  | [709](https://peps.python.org/pep-0709/) | Comprehension inlining semantics -- pycc has no bytecode/frame model to "inline" the way CPython's own PEP 709 change does; this row instead verifies the one CPython-observable, statically-testable guarantee PEP 709 depends on: a comprehension's own loop variable does not leak into an enclosing same-named binding (D-117/D-120) | sem | `pep_0709_comp_inline.py` | ☐ |
  ```

  Leave the `☐` in place until CI-observed on all 5 Tier-1 targets, per D-102's manual-flip policy (do not flip on local-only verification, matching every prior PR's own identical discipline).

- [ ] **Step 5: `docs/ROADMAP.md` — add PR-12's own entry**

  Following PR-11b's own entry's exact style (a `**PR-12 (<date>) ...**` paragraph plus explicit "Follow-up" paragraphs), add, after the existing PR-11b paragraph and before the closing `---` or next section boundary:

  - Summary of what shipped (comprehensions, slicing, the three new methods, the PEP-709 fixture), citing D-117–D-120.
  - The conformance-matrix count: PEP 709 is the **only** new distinct PEP row this PR adds (comprehensions/slicing/methods depth for already-shipped containers don't add a new PEP row of their own) — update the running numerator/denominator exactly as PR-10/PR-11's own entries did, citing the same `grep -cE "^\| \[.*✅.*\|$" docs/PYTHON_STANDARDS.md`-style verification command, and reconfirm the "zero margin" warning from D-088's Update note still applies (PR-13/PR-14 still need every one of their own remaining rows).
  - Explicit follow-ups, mirroring the existing style: (a) tuple slicing deferred (D-118); (b) `set[int]`'s membership test (`in`) still deferred, still blocked on the same general `in`-operator gap D-123 already named, unaffected by this PR; (c) comprehensions restricted to `Stmt::Assign`-RHS position only — a comprehension used as a function argument, return value, or nested sub-expression is not supported (D-117), tracked as a future PR's own scope if ever prioritized; (d) container `to_str`/`truthy` remain unimplemented (D-120 reaffirms D-107/D-124/D-116 — not a new gap, just re-flagged since this PR's own fixtures could otherwise be mistaken for having exercised it); (e) real (non-leak-only) refcounting for any container remains unaddressed (D-107/D-124's own already-tracked item, unaffected by this PR); (f) PR-14's own hand-authored corpus (D-088's OSS-package replacement) needs the same element-wise-printing discipline this PR's own fixtures established, for the identical reason.

- [ ] **Step 6: `docs/RUNTIME.md` — extend the `list[T]`/`dict[K,V]`/`set[T]` "Current state" lines**

  Add a `**Current state (through PR-12, D-117/D-118/D-119):**` clause to each of the three relevant lines (mirroring the existing `**Current state (through PR-11a, D-121):**`-style clauses already there), noting: `list[int]` now supports slicing (non-negative clamped bounds, positive step, D-118) and `.pop()`; `dict[str,int]` now supports `.get(key, default)`; `set[int]` now supports `.add()`; all three now support comprehension construction (list/dict/set comprehensions desugar to the same construction primitives their literal forms already use, D-117).

- [ ] **Step 7: `docs/DELIVERY_PLAN.md` — rewrite row 12's own content cell**

  Mirror exactly how row 10/row 11's own cells were rewritten in place once each PR shipped (bold **"Implemented"**/**"Delivered"**-style prefix, a summary of what shipped citing the new D-numbers, and an explicit list of what was *not* delivered, mirroring row 11's own "Not delivered by either sub-PR, tracked as..." closing paragraph).

- [ ] **Step 8: Final documentation cross-check**

  Re-read `docs/SPEC.md`'s own `DECISIONS.md` row (the long parenthetical listing every ADR topic covered) and append a phrase for D-117–D-120's own topics, mirroring how each prior PR's own decisions were appended to that same running list.

---

## Task 14: Final whole-branch review, then PR

**Files:** none (process task)

- [ ] **Step 1: Full workspace build + test**

  ```bash
  cargo build --workspace
  cargo test --workspace
  cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100
  ```

  Fix any gap before proceeding — do not proceed to review with a red coverage gate.

- [ ] **Step 2: `cargo doc --workspace --no-deps`**

  Confirm it succeeds and that every new public item (`pycc_hir::CompIter`, every new `HirExpr`/`HirStmt`/`MirExpr`/`MirStmt` variant, every new `pycc_rt` `extern "C"` function) has a real doc comment, not a placeholder.

- [ ] **Step 3: Re-read every touched spec doc's own affected section** (per AGENTS.md's own Completion Check): `docs/RUNTIME.md`, `docs/TYPE_SYSTEM.md` (confirm no section describes slicing/comprehensions/these three methods in a way this PR's actual scope now contradicts — widen or correct if so), `docs/DIAGNOSTICS.md`, `docs/PYTHON_STANDARDS.md`, `docs/ROADMAP.md`, `docs/DELIVERY_PLAN.md`, `docs/DECISIONS.md`.

- [ ] **Step 4: Local pinned reviewer** (D-068): dispatch the pinned `ievo` `deep-reviewer` against the full committed range from this branch's merge base (PR-11's own tip) through `HEAD`, per `docs/AGENT_TOOLING.md`'s own process. Address every actionable P0/P1 finding; re-run if a fix round changes a previously-reviewed diff materially.

- [ ] **Step 5: Open the PR**

  Per `docs/DELIVERY_PLAN.md`'s own delivery mechanics — PR-12 stacks on the still-unmerged PR-11 branch, so its own PR (if opened before PR-11 merges) should say so explicitly in its description, mirroring how PR-11b's own PR (if any) would have needed to disclose stacking on PR-11a. Confirm current PR-11/PR-10 merge status via a fresh `gh pr list`/`gh pr view` before deciding whether to target `main` directly or continue stacking — do not assume PR-11's D-109/#109 governance block has resolved without checking.

- [ ] **Step 6: Update `docs/SESSION_LOG.md`**

  Per D-066, a fresh entry (newest first) recording this PR's own completion status, grounded in the exact commit/CI state actually observed at write time (re-fetch and re-resolve any referenced remote state immediately before committing, per AGENTS.md's own explicit rule for this file).

---

## Self-review checklist (per the `writing-plans` skill's own requirement)

- [ ] Every task ends in an independently testable deliverable (each task's own "Tests" step names concrete, checkable assertions — no task defers testing to a later task).
- [ ] No placeholder code — `var_ty` is carried explicitly on every `*CompAssign` MIR variant (Task 4) specifically so `pycc_codegen` never needs a `todo!()`-marked re-derivation; the one place this plan uses abbreviated, commented pseudocode instead of fully spelled-out Rust (Task 5b Step 2's `DictCompAssign` sketch, whose loop body is shown as a sequence of `//`-prefixed lines rather than duplicating Task 5a's fully-written statements) must be expanded into real, uncommented code during implementation, mirroring Task 5a's own arm exactly in form — not left as shorthand.
- [ ] Every new `D-` number and `T0xxx` code is verified against the live tree before use, not hardcoded from this plan's own snapshot (Task 1 Step 1, Task 12's own diagnostics-reuse rationale).
- [ ] Every new HIR/MIR/`Scalar` exhaustive match this plan touches (`rename_name_in_expr`, `collect_stmt_bindings`, `block_always_returns`, every `check_stmt`/`check_stmt_in_function`/`collect_local_names`/`collect_expr_constraints` site) is called out by name with its exact current line-location grep command, not assumed stable from this plan's own one-time reading.
- [ ] The two riskiest design forks (statement-level comprehension desugaring; synthesized loop-variable naming) are each backed by a specific, checked architectural fact (no `BoolOp`/expression-position control flow anywhere in `pycc_codegen`; the flat name-keyed slot model), not asserted by fiat, and each is recorded as its own ADR (D-117) rather than only living in this plan's own text.
- [ ] Diagnostics reuse is exhaustively justified per new rejection path (Task 1's D-119, Task 12) rather than assumed — this plan mints zero new `T0xxx` codes, a deliberate, stated outcome.
- [ ] The PEP-709 fixture (D-120) is verified against this plan's own architectural finding (container `to_str` panics) before being written, not designed first and only later checked against that constraint.
- [ ] Every doc this PR's own behavior touches (`docs/RUNTIME.md`, `docs/DIAGNOSTICS.md`, `docs/PYTHON_STANDARDS.md`, `docs/ROADMAP.md`, `docs/DELIVERY_PLAN.md`, `docs/SPEC.md`, `docs/DECISIONS.md`) has its own explicit update step (Task 13), not left to an ambient "docs sweep" with no concrete substeps.
