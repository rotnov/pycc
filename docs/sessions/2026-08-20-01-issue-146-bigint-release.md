# 2026-08-20-01 — Issue #624 (Part 1 of #146): refcount heap bigints

## Overall status

Implementation, tests, and documentation for
[#624](https://github.com/rotnov/pycc/issues/624) — "Part 1 of
[#146](https://github.com/rotnov/pycc/issues/146): Refcount heap bigints and
release them at named-storage and loop-induction sites" — are complete and
committed on the task branch `claude/issue-146-bigint-release`.

The work was written against `883312d9f61e994d81cc84e7b2b40ced4b4b14d2`. It was
then rebased without conflict onto
`aa93dec8` — the tip of `origin/main` after
[#621](https://github.com/rotnov/pycc/pull/621) (a one-line `AGENTS.md`
citation fix) merged. #621 touches no file this change touches, so the rebase
was mechanical and the gate results recorded below still describe this content.

The branch is pushed and its pull request is open; #624, #146, and #625 were all
still open when this entry was last re-resolved against the remote.

## What the change does

[D-058](../decisions/D-058-int-overflow-to-bigint-d-001-is-a-minimal-hand.md)
accepted leaking every heap bigint on the grounds that promotion is an
overflow-only path.
[D-179](../decisions/D-179-range-loops-drive-bigint-bounds-steps-and.md) (#147)
removed the premise: a `range()` loop in the bigint domain now allocates one
`BigIntObj` per iteration, making the leak linear in trip count.

The new [D-180](../decisions/D-180-refcount-heap-bigints-and-release-them-at-named.md)
**narrows** D-058 (it does not supersede it — the representation and the
"once promoted, stays promoted" rule are untouched):

- `BigIntObj` gains a non-atomic `rc: Cell<u32>`, matching `PyStrObj`'s shape.
- `pycc_rt_bigint_retain` / `pycc_rt_bigint_release` take D-141 *encoded words*,
  no-op on smallints, the two bool markers, and the empty-slot word `0`.
- `pycc_codegen` emits an inline `(w & 0b11) == 0 && w != 0` guard before every
  call, so the D-084/D-140 nbody throughput floor is not exposed to a call per
  smallint assignment.
- Release is centralized *inside* `emit_assign`, gated on the storage slot's
  declared `Ty::Int` — not on the assigned value's type, and not per call site.
- Retains are applied only where the value's new home takes ownership:
  assignment target, `return`, instance attribute, call argument — a strictly
  smaller set than `str`'s `incref_if_str_duplicate`.
- `ForRange` and the three comprehension emitters own their induction values
  explicitly, so `for i in range(b, b, b)` (one object, three names) neither
  leaks nor double-frees.

## Files touched

- `crates/pycc_rt/src/int_encoding.rs` — `rc` field, `BigIntObj::new`,
  `bigint_retain`/`bigint_release`, test-only drop counter.
- `crates/pycc_rt/src/lib.rs` — the two `extern "C"` wrappers plus unit tests.
- `crates/pycc_codegen/src/lib.rs` — `RtFns` entries, `Ty::Int` slot
  zero-initialization, `emit_bigint_refcount_call`,
  `release_int_slot_before_store`, `release_int_attr_slot_before_store`,
  `retain_if_int_duplicate`, and the `ForRange`/comprehension wiring.
- `crates/pycc_codegen/src/int_const.rs` — stale leak comment repointed at
  D-180 and Part 2.
- `tests/issue_146_bigint_release.rs` (new) — 16 tests: aliasing, named
  storage, comprehensions, and two `#[cfg(unix)]` peak-RSS ratio assertions.
- `docs/decisions/D-180-*.md` (new), `docs/decisions/README.md` (regenerated),
  `docs/RUNTIME.md`, `docs/ROADMAP.md`.

`decref_str_slot_before_store` and its `#[should_panic]` test are deliberately
**unmodified**: generalizing that helper to `Ty::Int` would have orphaned the
panic arm and made it uncoverable under the D-014 region gate. A separate int
helper inside `emit_assign` avoids that and gives the stronger property
("released before every int store" is a property of the function, not of a
13-site audit).

## Gate results at the committed tree

| Gate | Exit |
| --- | --- |
| `cargo test --workspace` | 0 |
| `cargo test --test issue_146_bigint_release` | 0 (16 passed) |
| `cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100` | 0 (100.00% regions / functions / lines) |
| `cargo clippy --workspace --all-targets -- -D warnings` | 0 |
| `cargo doc --workspace --no-deps` | 0 |
| `RUBYOPT=-EUTF-8 ruby scripts/check_roadmap_evidence.rb` | 0 |
| `ruby scripts/check_ci_permissions.rb` | 0 |
| `python3 scripts/generate_decisions_index.py docs/decisions docs/decisions/README.md` | 0 |
| iEvo `deep-reviewer` on the staged diff | 0 findings |

The red-first discipline was honored: both peak-RSS ratio tests were confirmed
FAILING on the unfixed tree (ratios 1.9241 and 1.9258 against a 1.35 bound) via
`git stash push crates/`, then passing after `git stash pop`.

## Known follow-ups

- **`cargo test --test nbody_bench --release -- --ignored` cannot pass on this
  machine.** `oracle_python_bin()` asserts exactly `Python 3.14.7`; the two
  local interpreters are 3.14.6 and 3.14.4, so the gate exits 101 identically
  before and after the change. A direct wall-clock substitute (the nbody
  fixture at 20x iterations, median of five release runs) gave 1.34s before and
  1.23s after — within measurement noise, so it establishes "no visible
  regression" and cannot resolve a small one. CI has the correct oracle.
- **Part 2 is [#625](https://github.com/rotnov/pycc/issues/625)** — discarded
  arithmetic temporaries, including the per-evaluation bigint *literal* that
  `int_const::emit_int_constant` materializes. That is the largest residual
  leak class and is explicitly out of Part 1's scope. The peak-RSS repro here
  deliberately uses `x = x + 1` rather than a literal for exactly that reason.
- The remaining residual leaks are enumerated as consequences in D-180. Module
  globals are deliberately not released at module exit; that omission is
  recorded there as a decision, not an oversight.
- D-180 does **not** discharge the standing D-107/D-124 container-refcounting
  follow-ups. It records the egress-retain precondition list that whoever
  widens the D-141 container boundary must satisfy.

## Where a fresh session should resume

Read `docs/decisions/D-180-*.md` first — it is the contract for this change and
names every deliberate omission. Then `tests/issue_146_bigint_release.rs` for
the observable behavior. `git log claude/issue-146-bigint-release` has the
commits; they are unpushed, so a resuming session must confirm the branch still
exists locally before assuming this work is available.
