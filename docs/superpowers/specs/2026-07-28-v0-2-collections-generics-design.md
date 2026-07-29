# v0.2 "collections & generics" — design

> Brainstormed in autopilot mode per the standing `/goal` directive, with the
> `advisor` tool as dialogue partner in place of interactive user Q&A (the user
> is unavailable until v0.2 ships and explicitly delegated design-fork
> decisions). Each approval gate the `brainstorming` skill normally asks the
> user for is instead recorded here with its rationale, the way this
> repository's own ADRs already work — this doc **is** the audit trail.

**Goal:** Ship v0.2 ("collections & generics") per `docs/ROADMAP.md`: `list`/`dict`/`set`/`tuple` with literals, comprehensions, slicing, and methods; PEP 585/695 generics via monomorphization; a `--release`/LTO profile; `pycc.toml`. Acceptance criteria as corrected by [D-088](../../DECISIONS.md#d-088-correct-v02s-acceptance-criteria-before-any-v02-pr-starts) (this file lives under `docs/superpowers/specs/`, two directories below `docs/` — not one).

**Architecture:** Same "thin slice first, then breadth" strategy v0.1 used (`docs/DELIVERY_PLAN.md`'s "v0.1 execution strategy"), reordered per two corrections D-088's verification surfaced: the performance-critical acceptance bullet and the conformance-count acceptance bullet both need new measurement infrastructure that doesn't exist yet, so that infrastructure lands before the features it will measure, not after.

**Tech stack:** No new external dependencies. `inkwell`'s existing LLVM bindings already expose optimization-level/LTO knobs (`--release` is CLI-driver + `pycc_codegen` wiring, not new tooling). Monomorphization is hand-rolled in `pycc_types`/`pycc_mir`, matching TYPE_SYSTEM.md's already-declared "Monomorphization (Rust model)" choice — no generics library is pulled in.

## Global Constraints

- D-014: 100% line/region coverage is a hard merge gate for every crate, every PR, no exceptions without a documented `docs/TESTING.md` exemption.
- D-021: every PR/task starts from a freshly fetched `origin/main`, in its own branch/worktree; never merge/rebase over uncommitted work.
- D-068: the pinned `ievo:deep-reviewer` reviews every significant diff before merge; actionable findings get fixed and re-reviewed.
- Every behavior change ships with its documentation update in the same commit (AGENTS.md's "Keep documentation current").
- `docs/ROADMAP.md`'s acceptance criteria are binary, not aspirational — a gate that can't be measured isn't a gate (this is exactly what D-088 fixed once already; don't reintroduce it).
- Spec-is-law autonomy policy (`docs/DELIVERY_PLAN.md`): where this doc is silent, the implementer picks the most conservative *actually available* option and records it as a new ADR — does not stop to ask.

---

## Context already verified (don't re-derive)

- **`docs/STDLIB_PLAN.md`** already schedules `math`/`sys` for v0.2 (Tier 1) and `--release`/`pycc.toml` are already fully specified in `docs/CLI_SPEC.md` (lines 54, 65-83) — v0.2's job for both is "build what's already spec'd," not fresh design.
- **Import support today is a clean, deliberate rejection** (`C0001: statement kind not supported yet`), not a partial implementation — verified by running `pycc check` on a file containing `import math`.
- **D-088** found and corrected three unreachable/unverified v0.2 acceptance bullets (corpus-Tier-1-compiles → hand-authored corpus; import scope → stdlib-intrinsic only; conformance count → 25 → 15) and scoped PEP 695 to generic functions + `type` statement (no generic classes; v0.3's class model doesn't exist).
- **D-089** found `pycc_hir::Ty` is a flat, `Copy`, six-variant scalar enum with no parameterized-container case, consumed 729 times across four crates, and decided it becomes a recursive `Box`/`Vec`-based enum (not an arena/interner) before any monomorphization work starts.

## Design decisions this doc adds

### 1. The nbody benchmark's measurement contract (previously unspecified)

`nbody` doesn't exist in this repository. The canonical reference is `pyperformance`'s `bm_nbody` (Computer Language Benchmarks Game N-body simulation, 5 solar-system bodies, `DEFAULT_ITERATIONS = 20000`) — verified by downloading `pyperformance` 1.14.0 and reading `benchmarks/bm_nbody/run_benchmark.py` directly. Its literal source uses nested tuple/list destructuring in `for` loops (`for (([x1,y1,z1], v1, m1), ([x2,y2,z2], v2, m2)) in pairs:`) that pycc's v0.1+v0.2 grammar doesn't confidently support (extended unpacking, PEP 3132, is not in v0.2's scope per D-088's PEP itemization) — so the fixture is a **hand-adapted rewrite**, not the literal upstream script, matching D-088's own "hand-authored corpus" precedent. (D-093 later raised the shipped fixture's iteration count past this `20000` to amortize the paired-comparison harness's own fixed per-process overhead; physics and constants are unaffected.)

- Same physics, same 5 bodies (sun/jupiter/saturn/uranus/neptune) and constants, same `advance` structure, rewritten using only pycc's implemented grammar. The shipped fixture (`tests/fixtures/nbody.py`) drops upstream's separate `report_energy` step and instead prints the sun's final position (`print(sun_x, sun_y, sun_z)`) after the `advance` loop — a differential comparison only needs one deterministic, easily-diffable output shared by both sides, and the energy computation contributes nothing PR-8's paired-comparison gate needs. **This fixture must not depend on containers**: PR-8 (which builds this benchmark) runs before PR-10/PR-11 give pycc any `list`/`tuple`/`dict`/`set` at all, so each body's 7 scalars (position × 3, velocity × 3, mass) are separate named `float` variables, and the 10 pairwise gravitational updates for 5 bodies are fully unrolled rather than iterated — verbose, but a real, valid Python program CPython also runs unchanged for the differential comparison. (An earlier draft of this contract deferred the data-representation choice to "whatever list/tuple PR-10/11 ship" — wrong, since that would make PR-8 depend on work sequenced two PRs later; corrected here.)
- **Method:** same-machine paired comparison (not the cross-runner D-051/D-053/D-056/D-062 regression-gate machinery — this compares two different programs on one machine at one time, not one program's speed across two commits). Both programs run `K = 5` times; take the median of each; ratio = median(CPython wall-clock) ÷ median(pycc `--release` binary wall-clock).
- **Profile:** pycc side built `--release` (LTO on); CPython side is the pinned oracle (`python3.14` → 3.14.6) with no special flags.
- **Gate:** ratio ≥ 20. This is PR-8's own benchmark harness to build (new script, e.g. `scripts/check_nbody_speedup.rb` alongside the existing `check_frontend_throughput.rb`, or a `tests/nbody_bench.rs` — PR-8's implementer picks the shape and records it as an ADR, the same way D-079 picked `tests/conformance.rs`'s shape for v0.1).

### 2. The 16 named PEPs backing the ≥15 conformance target

D-088 itemized the PEPs reachable from v0.1+v0.2's surface without added grammar scope, but didn't assign each to an owning PR or a fixture path. Doing that now — and correcting two rows a review caught: D-088's first draft claimed PEP 526 and PEP 594 were "already true" without empirically testing either; both were wrong (see the Fixture/Owning PR columns below for what's actually needed):

| PEP | Feature | Fixture | Owning PR | Why reachable there |
|---|---|---|---|---|
| 238 | True division | `py30/pep_0238_division.py` | PR-9 | Already true of v0.1 |
| 3105 | `print()` as function | `py30/pep_3105_print.py` | PR-9 | Already true of v0.1 |
| 3107 | Function annotations | `py30/pep_3107_annotations.py` | PR-9 | v0.1's whole typing model requires it |
| 3131 | Non-ASCII identifiers | `py30/pep_3131_unicode_ids.py` | PR-9 | Verified via a plain (non-annotated) assignment |
| 414 | `u''` literals | `py33/pep_0414_u_literal.py` | PR-9 | `str` literals already exist |
| 484 | Type hints | `py35/pep_0484_type_hints.py` | PR-9 | pycc's cornerstone, already true |
| 498 | f-strings | `py36/pep_0498_fstrings.py` | PR-9 | v0.1 already has basic f-strings |
| 515 | Numeric underscores | `py36/pep_0515_underscores.py` | PR-9 | Parser-level, no pycc work |
| 649/749 | Deferred annotations | `py314/pep_0649_deferred_ann.py` | PR-9 | Already true of v0.1's static-evaluation behavior |
| 526 | Variable annotations `x: int = 1` | `py36/pep_0526_var_annotations.py` | PR-9 | **Not free** — verified `pycc_hir` has no `Stmt::AnnAssign` case (only `Stmt::Assign`); `x: int = 1` hits `C0001` today. Small, bounded new frontend work, real PR-9 scope, not a free row |
| 585 | Builtin generics `list[int]` | `py39/pep_0585_builtin_generics.py` | PR-10 | v0.2's own headline feature |
| — | `dict` insertion order | `py37/dict_order.py` | PR-11 | Needs `dict[K,V]` to exist at all |
| 709 | Comprehension inlining semantics | `py312/pep_0709_comp_inline.py` | PR-12 | v0.2's own comprehensions work |
| 695 | `type` statement + generic functions | `py312/pep_0695_generics.py` | PR-13 | Scoped per D-088: functions + `type` stmt only, not generic classes |
| 613 | `TypeAlias` (legacy syntax) | `py310/pep_0613_typealias.py` | PR-13 | Same underlying alias mechanism as 695's `type` statement |
| 594 | Dead-battery removals | `py313/pep_0594_removals.py` | PR-14 | **Not testable earlier** — verified `import cgi` today produces the identical generic `C0001` every import produces (no resolution exists yet to distinguish "module removed" from "imports unsupported"). Only PR-14's stdlib-intrinsic import support makes this row meaningful |

16 fixtures ≥ 15 target, one item of margin. PR-9 owns 9 rows that need no new language feature (proving what v0.1 already does) plus PEP 526's own small bounded addition — it is not purely harness-only, but its new scope is narrow and well-defined, which is why it's still sequenced right after the perf harness and before any container work.

### 3. `set[T]`'s representation (flagged, not resolved)

`docs/RUNTIME.md`/`docs/TYPE_SYSTEM.md` specify `list[T]` (vec), `dict[K,V]` (insertion-ordered swiss table), and `tuple` (inline struct), but never state `set[T]`'s own layout. The natural default — a swiss table without a value slot, **matching CPython's own non-guaranteed set ordering** (unlike `dict`, which PEP 520/3.7 made insertion-ordered) — is PR-11's own decision to record via ADR when it's reached, not pre-decided here; flagging it now so PR-11's implementer doesn't have to rediscover the gap.

### 4. Generic function type-parameter representation (flagged, not resolved)

D-089 decided `Ty`'s container variants but explicitly left the generic-function type-parameter case (`def f[T](x: T) -> T`) to PR-10/PR-13's own implementer, against the real constraint-solving code in `collect_expr_constraints`/`infer_expr_in` — a placeholder/parameter marker distinct from the existing `Ty::Infer`, resolved to a concrete `Ty` at each call-site instantiation.

## PR breakdown

(Full rationale — including why `--release` and the conformance harness are resequenced ahead of the v0.1 pattern's "thin slice first" — lives in `docs/DELIVERY_PLAN.md`'s new v0.2 section, committed alongside this doc.)

1. **PR-8**: `--release`/LTO profile + `pycc.toml` + nbody benchmark harness (contract above — hand-unrolled, scalar-only fixture, no container dependency).
2. **PR-9**: Real per-PEP conformance harness (`tests/conformance/pyXY/`) seeded with the 9 no-new-work PEPs above, plus PEP 526's own small bounded `Stmt::AnnAssign` addition. Whether this warrants finally building the `pycc_testkit` crate (D-018/D-037/D-085 deferred it "until there's a PEP matrix to check against" — one now exists) is this PR's own ADR to record.
3. **PR-10**: `Ty` representation migration (D-089) + monomorphization foundation + `list[T]` end-to-end thin slice (literal, indexing, `len()`, iteration, `.append()`) + its own PEP-585 fixture.
4. **PR-11**: `dict[K,V]`, `set[T]` (representation decision above), `tuple[...]` + the dict-insertion-order fixture.
5. **PR-12**: Comprehensions + slicing + remaining container methods depth, across all four types + the PEP-709 fixture.
6. **PR-13**: PEP 695 generic functions + `type` statement (scope per D-088) + legacy `TypeAlias` + their fixtures.
7. **PR-14**: `pycc_std` crate (`math`, `sys`) + stdlib-intrinsic import binding (scope per D-088, which also makes the PEP-594 dead-battery-removal fixture meaningful for the first time) + the hand-authored container/generics corpus (D-088's OSS-package replacement) + buffer closing whatever's left on v0.2's corrected acceptance bullets.

## Testing

Every PR above follows this project's existing TDD convention (failing test first, D-014's 100% coverage, `docs/AGENT_TOOLING.md`'s pinned reviewer before merge) — no new testing philosophy introduced. PR-9's harness is the one structurally new piece of test infrastructure; every later PR adds its fixture(s) to it incrementally rather than the count being audited once at the end.

## Error handling

No new error-handling philosophy: unsupported valid Python still gets a spanned `C0001`, malformed input still gets `L0001`, and the type/binding diagnostics families (`T00xx`) grow exactly as many new codes as new rejection cases (e.g., "generic instantiation cannot resolve `T`," "container element type mismatch") — each with its own snapshot test, matching `docs/DIAGNOSTICS.md`'s existing quality bar.
