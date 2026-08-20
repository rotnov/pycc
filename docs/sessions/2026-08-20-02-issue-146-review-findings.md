# 2026-08-20-02 — #624 review findings: codegen-depth oracle and a false coverage claim

## Overall status

Follow-up to `docs/sessions/2026-08-20-01-issue-146-bigint-release.md`. An
independent deep review of commit `61f05422` (the D-180 bigint refcounting
change for [#624](https://github.com/rotnov/pycc/issues/624)) raised two
findings; both were verified against the tree and both are fixed here, in a
new commit on the same local branch `claude/issue-146-bigint-release`.

The branch is based on `aa93dec8` (`origin/main` after
[#621](https://github.com/rotnov/pycc/pull/621) merged), re-fetched and
re-resolved immediately before committing. **Still nothing pushed and no pull
request** — the orchestrating session owns that.

## Finding 1 (blocker): the codegen-depth IR test was missing

#624's acceptance criteria call for a codegen-depth test alongside the
peak-RSS oracle and the `pycc_rt` unit tests, and none existed: every test in
`tests/issue_146_bigint_release.rs` is black-box (build a binary, assert its
stdout, measure its peak RSS). That leaves two properties with no oracle at
all, because neither is visible in a program's output *or* in its RSS:

1. Every `pycc_rt_bigint_retain`/`_release` call is reached only through
   `emit_bigint_refcount_call`'s inline `(word & 0b11) == 0 && word != 0`
   guard, **on the same word the call receives**. An unguarded call is
   correct-but-slow, so losing the guard regresses the D-084/D-140 nbody
   throughput floor silently rather than failing anything.
2. That guard splits the current basic block, so a caller recording a block
   for a phi incoming edge must re-read `get_insert_block()` afterwards. The
   original review closed this by hand, site by site — exactly the one-off
   audit a test is supposed to replace.

Two tests now cover both, one over a `Ty::Int` assignment site and one over a
`ForRange` induction site whose start, stop, and step are all the same name
(the aliasing shape). They parse the emitted LLVM IR and, for every refcount
call, walk backwards from the call to prove the whole guard chain — call block
is a `bigint_rc_call*` block, that block is the taken arm of a `br i1` whose
condition is `and i1` of `icmp eq (and i64 %w, 3), 0` and `icmp ne %w, 0`, and
`%w` is the call's own argument. They additionally run LLVM's module verifier,
which is what catches a phi whose incoming block stopped being a real
predecessor, and assert exact retain/release counts so that dropping any single
one fails loudly instead of drifting.

**Deviation from the review's suggested location.** The review pointed at
`tests/slice1_codegen_depth.rs`. That file (and
`tests/container_methods1_codegen_depth.rs`) drives hand-built MIR through the
*public* `compile_to_object`, which cannot see the emitted module at all —
`compile_to_object_with_observer` is private. IR-inspecting tests therefore
have to live in `crates/pycc_codegen`'s own test module, which is where this
repository's actual IR-inspection precedent already is
(`an_oversized_int_literal_materializes_a_runtime_bigint`), and which
`container_methods1_codegen_depth.rs`'s own header comment documents as the
intended home for tests that must count toward `cargo llvm-cov -p pycc_codegen`.

The oracle was confirmed to bite: mutating the guard mask from `0b11` to
`0b111` fails both tests. The mutation was reverted.

One non-obvious constraint shaped the analyzer's code: under D-014's region
gate, an error-reporting *closure* that never runs (`unwrap_or_else(|| panic!(..))`)
is an uncovered function, and a defensive `if` whose false arm never happens is
an uncovered region. The analyzer therefore reports every violation through
`assert*!`/`expect`, whose cold arms live in `core`, and carries no
never-false branch.

## Finding 2 (warning): a false coverage claim in a comment

The `MirStmt::AttrSet` arm claimed the bool-into-`int`-*attribute* gap was
"pinned by `tests/issue_146_bigint_release.rs`". It was not: the only related
fixture used a plain local, which goes through `release_int_slot_before_store`
(gated on the slot's declared `Ty`) rather than
`release_int_attr_slot_before_store` (gated on `value_ty`). Different helper,
different gate.

**Option taken: correct the comment, not add the fixture** — after first
attempting the fixture, which is what the review preferred and what the plan's
`AttrSet` option (b) called for. Writing it surfaced why it is impractical:

> `@dataclass class Box: v: int`; `b = Box(2**62)`; `b.v = True`; `print(b.v)`
> prints `0`, not `True`.

That is a separate, **pre-existing D-154 defect** and has nothing to do with
refcounting: `scalar_to_slot_word` stores a `Scalar::Bool` into an attribute
slot as a raw `zext` (word `1`) instead of a D-141 encoded word (`6`), so the
read-back decodes `1` as the smallint `0`. A local slot does not have this
problem because `emit_assign` routes the value through
`coerce_scalar_to_type` first, which is exactly why the sibling local-slot
fixture prints `True` correctly.

So the attribute shape is unobservable today: any fixture asserting the current
output would enshrine the wrong answer, and fixing the encoding is outside
#624's scope. The comment now says the gap is documented-but-unpinned, names
the reason, and points at D-180 Consequences item 6, which records the same
thing.

**This defect is not filed anywhere yet.** It is reported to the orchestrating
session rather than opened as an issue from here, since publishing to the
tracker is not this task's to do.

## Where a fresh session should resume

`git log claude/issue-146-bigint-release` — two commits, unpushed. D-180 is
still the contract; its Consequences section now also records where the two
structural properties are pinned and why those tests are in-crate.
