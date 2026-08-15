# PR-21: `match` statement with exhaustiveness checking (PEP 634-636)

**Issue:** [#381](https://github.com/rotnov/pycc/issues/381)
**Milestone:** v0.3 — classes & pattern matching
**Design spec:** `docs/superpowers/specs/2026-08-06-v0-3-classes-pattern-matching-design.md` §4
**Delivery plan:** `docs/DELIVERY_PLAN.md` PR-21 row

## Goal

Implement PEP 634-636 structural pattern matching: `match subject: case pattern [if guard]: body` with all pattern kinds (literal, capture, wildcard, sequence, mapping, or-patterns, class patterns), exhaustiveness checking (diagnostic `T0030`), and full HIR → type checker → MIR → codegen pipeline support.

## Design decision: exhaustiveness algorithm

**Choice: simpler per-arm coverage check** (not decision-tree compilation).

Rationale: pycc's static type system gives every matched expression a known compile-time type. The exhaustiveness check verifies that the patterns cover every value of that type:

1. **Irrefutable pattern present** (wildcard `_`, bare-name capture, or an or-pattern containing an irrefutable sub-pattern) **without a guard** → exhaustive. A guarded wildcard (`case _ if cond:`) is refutable (the guard may fail).
2. **`bool` subject**: exhaustive if both `True` and `False` singleton patterns are covered (across all arms, including or-patterns).
3. **Enum subject** (`Ty::Instance` where `HirClassDef.enum_members` is non-empty): exhaustive if every enum member's value pattern is covered.
4. **All other types** (`int`, `str`, `float`, `list`, `dict`, `set`, `tuple`, non-enum `Instance`): only an irrefutable pattern makes them exhaustive (infinite or open domain).
5. If none of the above → `T0030` with a message listing the uncovered cases.

This is sufficient for v0.3's scope (the design doc's §4 open question explicitly leaves this as PR-21's own design work, and T0030's reserved text does not commit to an algorithm).

## Architecture

### Layer 1: HIR (`crates/pycc_hir`)

New types in `crates/pycc_hir/src/lib.rs`:

```rust
pub enum HirPattern {
    Wildcard,                           // case _:
    Capture(String),                    // case x:
    Literal(HirExpr),                   // case 42: / case "hello": / case 3.14:
    Singleton(bool),                    // case True: / case False:
    NoneSingleton,                      // case None:
    Sequence(Vec<HirPattern>),          // case [a, b, c]:
    SequenceStar(Vec<HirPattern>, Option<String>), // case [a, *rest]:
    Mapping(Vec<(HirExpr, HirPattern)>, Option<String>), // case {"k": v}:
    Class {
        class_name: String,
        positional: Vec<HirPattern>,   // case Point(0, 0):
        keyword: Vec<(String, HirPattern)>, // case Point(x=0, y=0):
    },
    Or(Vec<HirPattern>),                // case 1 | 2 | 3:
    As(Box<HirPattern>, String),        // case [a, b] as pair:
}

pub struct HirMatchCase {
    pub pattern: HirPattern,
    pub guard: Option<HirExpr>,
    pub body: Vec<HirStmt>,
}

// New HirStmt variant:
HirStmt::Match {
    subject: HirExpr,
    cases: Vec<HirMatchCase>,
}
```

Lowering in `crates/pycc_hir/src/stmt.rs`:
- Add `Stmt::Match(ruff_python_ast::StmtMatch { subject, cases, .. })` arm to `lower_stmt`.
- Lower the subject expression via `lower_expr`.
- For each `MatchCase`, lower the pattern via a new `lower_pattern` function, the guard via `lower_expr`, and the body via `lower_body`.
- `lower_pattern` maps `ruff_python_ast::Pattern` → `HirPattern`:
  - `MatchValue` → `Literal(lower_expr(value))` (int/float/str/bool literals; other value kinds → `C0001`)
  - `MatchSingleton` → `Singleton(true/false)` or `NoneSingleton`
  - `MatchSequence` → `Sequence` or `SequenceStar` (if any `MatchStar` is present)
  - `MatchMapping` → `Mapping` (keys lowered as expressions, patterns lowered recursively; `rest` → the optional rest name)
  - `MatchClass` → `Class { class_name, positional, keyword }` (cls must be `Expr::Name`; otherwise `C0001`)
  - `MatchStar` → only valid inside a sequence; handled by `MatchSequence` arm
  - `MatchAs` → if `pattern` is `None`: `Capture(name)` or `Wildcard` (if name is also `None`); if `pattern` is `Some`: `As(Box(lower_pattern(pattern)), name)`
  - `MatchOr` → `Or(patterns.iter().map(lower_pattern).collect())`
- Unsupported pattern sub-shapes → `C0001` (e.g., complex expressions in `MatchValue` that aren't literals).

### Layer 2: Type checker (`crates/pycc_types`)

New code in `crates/pycc_types/src/lib.rs`:

**Pattern checking** (`check_pattern`):
- Takes `(env, pattern, subject_ty) -> Result<Vec<(String, Ty)>, Diagnostic>` — returns the list of captured variable bindings.
- `Wildcard`: no bindings, always matches.
- `Capture(name)`: binds `name: subject_ty`. Always matches.
- `Literal(expr)`: infer `expr`'s type, check it's compatible with `subject_ty` (same type or assignable). No bindings.
- `Singleton(b)`: check `subject_ty` is `Ty::Bool`. No bindings.
- `NoneSingleton`: check `subject_ty` is `Ty::None`. No bindings.
- `Sequence(patterns)`: check `subject_ty` is `Ty::List(elt_ty)`, check `patterns.len()` matches (for fixed-length), recursively check each sub-pattern against `elt_ty`. Bind all sub-captures.
- `SequenceStar(patterns, rest)`: similar, but `rest` captures a `Ty::List(elt_ty)` of the remaining elements.
- `Mapping(pairs, rest)`: check `subject_ty` is `Ty::Dict(Box((key_ty, val_ty)))`, check each key expression is `key_ty`, recursively check each value pattern against `val_ty`. `rest` captures `Ty::Dict(Box((key_ty, val_ty)))`.
- `Class { class_name, positional, keyword }`: look up the class in `env.classes`. Check `subject_ty` is `Ty::Instance(class_name)` (or a subclass via MRO). For positional patterns, match against the class's `__init__` parameter types (after `self`). For keyword patterns, match against the named attribute/parameter types. Bind all sub-captures.
- `Or(patterns)`: check each sub-pattern against `subject_ty`. All sub-patterns must bind the same set of capture names (PEP 634 requirement). Return the shared bindings.
- `As(inner, name)`: check `inner` against `subject_ty`, collect its bindings, add `name: subject_ty`.

**Match statement checking** (`check_stmt` arm for `HirStmt::Match`):
1. Infer `subject` type.
2. For each case:
   a. Clone `env` (each case is an independent branch, like `if` arms).
   b. `check_pattern(&mut case_env, &pattern, subject_ty)` → bindings.
   c. Bind all captured variables in `case_env`.
   d. If guard exists, infer its type and check it's `Ty::Bool`.
   e. Check the body in `case_env`.
   f. Collect the case environment for joining.
3. **Exhaustiveness check**: if not exhaustive, emit `T0030`.
4. **Join**: join all case environments (like `join_if_branches` but for N branches). If exhaustive, all bindings from all arms join; if not exhaustive (but still accepted — Python allows non-exhaustive match), bindings are `Maybe`.

**Exhaustiveness check** (`check_exhaustive`):
- Returns `true` if the patterns cover all values of `subject_ty`.
- Algorithm per the design decision above.

### Layer 3: MIR lowering (`crates/pycc_mir`)

Desugar `HirStmt::Match` into a chain of `MirStmt::If`:

```
match subject:
    case pat1 [if guard1]:
        body1
    case pat2 [if guard2]:
        body2
    ...
```

becomes (conceptually):

```
__match_subj = subject
if <pat1 matches __match_subj> [and guard1]:
    <bind captures from pat1>
    body1
elif <pat2 matches __match_subj> [and guard2]:
    <bind captures from pat2>
    body2
else:
    <if non-exhaustive: runtime no-op or panic>
```

**Pattern match condition generation** (`lower_pattern_match`):
- Returns `(MirExpr /* match condition */, Vec<MirStmt> /* binding assignments */)`.
- `Wildcard`/`Capture`: condition is `True`; binding is `target = __match_subj`.
- `Literal(expr)`: condition is `__match_subj == expr`.
- `Singleton(b)`: condition is `__match_subj == True/False`.
- `NoneSingleton`: condition is `__match_subj == None` (if None is supported).
- `Sequence(patterns)`: condition is `isinstance(__match_subj, list) and len(__match_subj) == len(patterns) and <each element matches>`. Bindings are element-wise subscripts.
- `SequenceStar(patterns, rest)`: condition is `isinstance(__match_subj, list) and len(__match_subj) >= len(patterns) and <each leading element matches>`. `rest` binds to a slice of the remaining elements.
- `Mapping(pairs, rest)`: condition is `isinstance(__match_subj, dict) and <each key in dict> and <each value matches>`. Bindings are `__match_subj[key]` for each value pattern.
- `Class { class_name, positional, keyword }`: condition is `isinstance(__match_subj, class_name) and <each positional arg matches __match_subj.attr> and <each keyword arg matches __match_subj.attr>`. Bindings are attribute accesses.
- `Or(patterns)`: condition is `pat1_cond or pat2_cond or ...`. Bindings from the first matching pattern (all must bind the same names).
- `As(inner, name)`: condition is `inner_cond`; binding is `name = __match_subj` plus inner bindings.

**Note on `isinstance` in MIR**: pycc already has compile-time `isinstance` evaluation (PR-16/#435). For pattern matching, the `isinstance` check in a pattern context is also compile-time when the subject's type is known. Since pycc's static dispatch model means the subject's type is always known at compile time, `isinstance(__match_subj, Class)` is a compile-time `true`/`false` — if the subject is already typed as `Class`, the check is `true`; if not, it's `false`. This means class patterns on a subject of the right type have no runtime type-check overhead, and class patterns on a subject of the wrong type are statically rejected by the type checker.

For sequence/mapping patterns, the type checker already verified the subject is a `list`/`dict`, so the `isinstance` check in the MIR condition is always `true` and can be omitted — the condition reduces to just the length check and element-wise comparisons.

**Subject evaluation**: the subject expression is evaluated once and stored in a synthesized temp variable `__match_subj_N` (collision-proof, mirroring the comprehension variable renaming pattern).

### Layer 4: Codegen (`crates/pycc_codegen`)

**No changes needed.** The MIR desugaring produces only `MirStmt::If`, `MirStmt::Assign`, and existing `MirExpr` variants, all of which codegen already handles.

### Layer 5: Diagnostics (`crates/pycc_diag`)

Update `T0030` in `crates/pycc_diag/src/explain.rs`:
- Change the explanation from "not currently emitted" to the real semantics.
- Summary: "non-exhaustive `match` (missing cases listed)"
- Explanation: describes what exhaustiveness means and lists the uncovered cases.

Update `docs/DIAGNOSTICS.md`:
- `T0030` row: update from "reserved" to active.

### Layer 6: Tests

**Integration tests** (`tests/issue_381_match.rs`):
- Wildcard pattern: `match x: case _: ...`
- Capture pattern: `match x: case y: print(y)`
- Literal patterns: `match x: case 1: ... case 2: ... case _: ...`
- Singleton patterns: `match b: case True: ... case False: ...`
- Sequence patterns: `match xs: case [a, b]: ... case [a, *rest]: ... case _: ...`
- Mapping patterns: `match d: case {"k": v}: ... case _: ...`
- Or-patterns: `match x: case 1 | 2 | 3: ... case _: ...`
- Class patterns: `match p: case Point(x=0, y=0): ... case _: ...`
- As-patterns: `match xs: case [a, b] as pair: ...`
- Guard: `match x: case n if n > 0: ... case _: ...`
- Exhaustiveness: `match b: case True: ... case False: ...` (exhaustive, no T0030)
- Non-exhaustive: `match x: case 1: ... case 2: ...` → T0030
- Enum exhaustiveness: `match c: case Color.RED: ... case Color.GREEN: ... case Color.BLUE: ...` (exhaustive)
- Enum non-exhaustive: missing one member → T0030
- Binding in case body: captured variable is readable in the case body
- Definite-assignment join: variable bound in all arms is definitely bound after match
- Type error: pattern type mismatch → appropriate T0xxx diagnostic

**Conformance fixture** (`tests/fixtures/pep_0634_match.py`):
- A comprehensive fixture exercising all pattern kinds, verified byte-for-byte against CPython 3.14.6.

**Unit tests** in each crate:
- HIR: pattern lowering tests for each pattern kind.
- Type checker: pattern checking, binding, exhaustiveness tests.
- MIR: desugaring tests for each pattern kind.

### Layer 7: Documentation

- `docs/ROADMAP.md`: add PR-21 paragraph to v0.3 section.
- `docs/DIAGNOSTICS.md`: update T0030 row.
- `docs/PYTHON_STANDARDS.md`: PEP 634-636 row stays `☐` until CI-observed green on all 5 Tier-1 targets (per D-102).
- New ADR: `docs/decisions/D-169-match-exhaustiveness-per-arm-coverage-check.md` recording the exhaustiveness algorithm choice.
- `crates/pycc_diag/src/explain.rs`: update T0030 explanation.

## Implementation order

1. HIR: `HirPattern`, `HirMatchCase`, `HirStmt::Match`, `lower_pattern`, `lower_stmt` Match arm.
2. Type checker: `check_pattern`, `check_stmt` Match arm, `check_exhaustive`, join logic.
3. MIR: `lower_pattern_match`, `lower_stmt` Match desugaring.
4. Diagnostics: update T0030.
5. Tests: integration tests, conformance fixture, unit tests.
6. Documentation: ROADMAP, DIAGNOSTICS, ADR, PYTHON_STANDARDS.
7. Verify: `cargo build --workspace`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`, `cargo doc --workspace --no-deps`.

## Scope boundaries

- **No `None` singleton in v0.3** unless trivially supported: `None` is `Ty::None`, and pycc already has `Ty::None`. The `case None:` pattern is included if it's a simple equality check; otherwise deferred.
- **No nested match**: a `match` inside a `case` body is supported (it's just another statement).
- **No `case` outside `match`**: already handled by D-148's `in_loop`-style context flag — `case` outside `match` is `L0001`.
- **Sequence patterns on `tuple`**: supported if the subject is `Ty::Tuple` (fixed-length, no star). Sequence patterns on `list` support star.
- **Mapping patterns on `dict`**: supported with string keys only (matching pycc's `dict[str, int]` gate).
- **Class patterns**: positional arguments match against `__init__` parameters (after `self`); keyword arguments match against attribute names. Only classes with a known `__init__` signature are supported for positional patterns; keyword patterns work for any class with declared attributes.
- **Guards**: the guard is evaluated after the pattern matches, in the environment extended with pattern bindings. A failing guard does not make the arm taken.
- **Irrefutable pattern warning**: CPython warns when an irrefutable pattern appears before other patterns. pycc does not implement warnings — an irrefutable pattern in a non-final position is accepted (later arms are unreachable, but pycc does not diagnose unreachable arms in v0.3).
