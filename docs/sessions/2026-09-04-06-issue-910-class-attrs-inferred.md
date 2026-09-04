# 2026-09-04-06 — #910: un-annotated class-body assignments with a literal RHS

## Previous iteration's merge outcome (2026-09-04-05 / #912)

[PR #922](https://github.com/rotnov/pycc/pull/922) — "Synthesize an implicit
zero-argument constructor for a class with no `__init__` (#912)" — **merged**
as `440114ffd1e7a5b6888d9dc011444ab48bc65d4e`, which is `origin/main`'s tip and
this branch's base. [#912](https://github.com/rotnov/pycc/issues/912) is CLOSED.
The decision it recorded is
[D-225](../decisions/D-225-synthesize-an-implicit-zero-argument-constructor.md).
The D-068 pinned reviewer returned **no P0/P1**; its one warning (a test that
claimed to exercise `inherits_init`'s MRO arm but ran an empty `mro.iter().skip(1)`)
and one of its two notes (a stale "every panic message is unchanged" claim in
`crates/pycc_types/src/class/binding.rs`'s module header) were both fixed before
merge. The remaining note — the `class MyError(Exception, Base)` base-order
permutation — was recorded as a coverage observation, not a defect, and is
unchanged.

### One correction to that session's file

`docs/sessions/2026-09-04-05-issue-912-no-init.md` states that the `nbody` 20x
speedup gate failing at 18.58x was "the first occurrence of that gate failing in
the last 40 `ci.yml` runs" and that a recurrence "deserves its own issue". Both
claims are wrong, and D-130 forbids editing that file, so the correction is
recorded here.

[#641](https://github.com/rotnov/pycc/issues/641) has been open since well before
that run and tracks exactly this flake — "the nbody 20x speedup gate measures
below threshold then passes on re-run" — with a series of prior sub-threshold
measurements, 17.32x on macOS and 17.96x on `ubuntu-latest x86_64` in its own
title plus further observations in its comments. The 18.58x measurement from PR
#922's `native-build-test (ubuntu-latest, x86_64-unknown-linux-gnu)` run
[33884131474](https://github.com/rotnov/pycc/actions/runs/33884131474) has been
added there as one more data point, together with the attribution evidence that
#922's diff cannot have caused it. No new issue was filed, and none should be.

The "first occurrence in the last 40 runs" reading came from
`gh run list --workflow ci.yml --limit 40 --json conclusion`. That query
structurally cannot see this failure class: a re-run overwrites the *run's*
`conclusion`, so every flake that was re-run to green — which is every instance
of #641 — reads as `success`. Counting a specific gate's failures needs per-job
attempt history, or the tracking issue's own accumulated record. The lesson is
logged in `docs/AGENT_RETROSPECTIVE.md`.

## Status: implemented and pushed, pull request NOT yet open

`origin/main` is at `440114ff` (re-fetched immediately before this file was
committed) and there are **zero** open pull requests in the repository. Issue
[#910](https://github.com/rotnov/pycc/issues/910) is OPEN, milestone v0.4. The
work below lives on `autopilot/iter-2026-09-04-06`, pushed but with no pull
request; opening it, running the D-068 reviewer, and merging are the
orchestrating session's steps, not this one's.

## What changed

An un-annotated class-body assignment with a literal right-hand side is now the
same compile-time constant [#911](https://github.com/rotnov/pycc/issues/911)
already made of `X: int = 1`. The design is
[D-226](../decisions/D-226-infer-an-un-annotated-class-attribute-s-type.md):
infer the type from the literal, then reuse Part 1's path unchanged.

Four commits, in dependency order:

1. **D-185 decomposition.** Class-attribute lowering and the collision check
   moved out of `crates/pycc_hir/src/class/body.rs` into a new
   `crates/pycc_hir/src/class/attrs.rs` — a pure move, no behavior change. This
   follows `class/body.rs` (#917) and `class/init.rs` (#922) as the third split
   of `class.rs`'s neighborhood; `class.rs` itself is still oversized and
   [#548](https://github.com/rotnov/pycc/issues/548) stays open.
2. **The collision hole, fixed rather than deferred.**
   `reject_class_attr_collisions` compared a class attribute only against the
   instance-attribute and `@property` tables and silently skipped methods. It
   now also covers `methods`, `static_methods`, and `class_methods`, for the
   class itself **and for every base in its MRO**, with the diagnostic naming
   which table collided and which base it came from. This was measured against
   CPython 3.14 at the base commit before the fix: `f = 2` beside `def f(self)`
   printed `2` where CPython prints a bound method, and `class B(A): f: int = 2`
   over `class A: def f(self)` let `b.f()` dispatch to `A.f` and print `1` where
   CPython raises `TypeError: 'int' object is not callable`. Both divergences are
   recorded in D-226's Consequences.
3. **Fixture re-vehicling.** `x = 1` in a class body was the "still unsupported"
   vehicle for eleven tests across three binaries. All of them move to the
   single-line in-class form `async def m(self) -> None: pass` (and `n` where a
   test needs two distinct names) — including the `span_of` needles in
   `crates/pycc_hir/src/module/tests.rs`, whose spans are derived from the needle
   text — and the four `tests/diagnostics/c0001_*` expectation files were
   regenerated from the binary rather than hand-edited.
   `tests/diagnostics/c0001_protocol_body_assign.py` is deliberately untouched:
   a `Protocol` body early-returns through `lower_protocol_class` and never
   reaches the walk.
4. **The feature itself.** `infer_class_attr_ty` maps the literal — after
   unwrapping a unary `+`/`-` on a number — to `Ty::Int`/`Float`/`Bool`/`Str`,
   `lower_unannotated_class_attr` delegates to an untouched `class_attr_value`,
   and `walk_class_body` intercepts `Stmt::Assign` outside a `@dataclass` body.
   `__slots__` is rejected in both spellings, checked before the right-hand side
   is examined so the two report identically. The class-body catch-all is
   reworded and split on `is_dataclass`.

The **#585/D-224 scalar-only invariant** — the doc comment on
`fn lower_class_attr`, cited by name — is not relaxed. Inference can only
produce a scalar `Ty`, so the invariant holds for the new spelling by
construction; there is deliberately no `Ty::Param` branch, which #911 already
rejects and which would be dead code under the D-014 region gate.

Deliberately out of scope, unchanged: `self.v: int = 0`
([#891](https://github.com/rotnov/pycc/issues/891)), the `Enum`-subclass
instantiation panic ([#921](https://github.com/rotnov/pycc/issues/921)),
`ClassVar` in a `@dataclass` body
([#913](https://github.com/rotnov/pycc/issues/913)), and `Final[...]` on a class
attribute ([#916](https://github.com/rotnov/pycc/issues/916)).

## Documentation

`docs/TYPE_SYSTEM.md` is the owning document and carries six edits: both
spellings in the opening sentence, the inference-holds-the-invariant note on the
scalar-slot bullet, the rejected right-hand-side shapes, the full collision list
with its CPython divergence, a new `__slots__` bullet, and a rewritten
known-limitations paragraph. `docs/ROADMAP.md` gains a #910 paragraph.
`docs/decisions/` gains D-226 and its regenerated index.

Two documents were checked and deliberately **not** changed, at the convention
level rather than by default:

- `docs/PYTHON_STANDARDS.md`'s PEP 526 row, and the matching
  `tests/fixtures/conformance-breadth-manifest.json` block (`matrix_line: 295`),
  describe *variable annotations*. #910 adds an **un**-annotated spelling, which
  is not PEP 526 material, so neither the matrix prose nor the manifest's
  `proven`/`not_proven` lists gain or lose an entry. The `matrix_line` pin is
  enforced by `scripts/check_conformance_breadth.py`, and the checker passes
  unchanged.
- No new `roadmap-evidence` identifier is due: #910 adds no roadmap acceptance
  item, only prose.

One documentation constraint was hit and is now tracked. The first `docs/ROADMAP.md`
paragraph written for #910 (~1.8 KB) pushed `sh scripts/check-site.sh` over the
272 KiB llms.txt non-optional aggregate budget by 851 bytes. The paragraph was
rewritten to ~0.9 KB to fit, which is documentation trimmed for a website-artifact
reason rather than an editorial one. After this change the expansion stands at
278,490 of 278,528 bytes — **38 bytes of headroom**, with `docs/ROADMAP.md` alone
at 67% of the total and growing on essentially every pull request. That is filed as
[#923](https://github.com/rotnov/pycc/issues/923) (v0.4), which lays out the three
options: a third ceiling raise after D-200 and D-218, splitting the roadmap's
per-issue narrative into an Optional-classified document, or a per-resource budget
so the pressure surfaces on the right document. The next iteration that adds roadmap
prose of any size will fail `check-site.sh` before it can commit.

`docs/LANGUAGE_SUBSET.md` does not exist in this tree; the class-attribute
subset rules live in `docs/TYPE_SYSTEM.md`.

## Known follow-ups

- **[#548](https://github.com/rotnov/pycc/issues/548)** — `class.rs` D-185
  decomposition. Still open; `class/` now holds `attrs.rs`, `body.rs`,
  `enum_class.rs`, `init.rs`, `mro.rs`, `protocol.rs`.
- **[#641](https://github.com/rotnov/pycc/issues/641)** — the `nbody` 20x gate
  flake. Still open, now with the 18.58x data point. Expect it to fire on this
  branch's CI too; check for a re-run-to-green before treating it as new.
- **[#923](https://github.com/rotnov/pycc/issues/923)** — the llms.txt aggregate
  budget, 38 bytes of headroom. **Blocks the next roadmap paragraph**; resolve it
  before or as part of the next iteration's documentation step.
- **[#921](https://github.com/rotnov/pycc/issues/921)**,
  **[#913](https://github.com/rotnov/pycc/issues/913)**,
  **[#916](https://github.com/rotnov/pycc/issues/916)**,
  **[#891](https://github.com/rotnov/pycc/issues/891)** — the four class-body
  gaps this change names but does not close.
- **Peer-session boundary** (unchanged from 2026-09-04-05): a parallel session
  owns the #881–#895 import series
  ([#899](https://github.com/rotnov/pycc/issues/899)). Stay out of
  `crates/pycc_hir/src/import.rs`, `module.rs`, `program.rs`, and any
  `src/modules.rs` / `src/frontend.rs`. This change edits
  `crates/pycc_hir/src/module/tests.rs` — the tests submodule, not the owned
  file — and only its test bodies and fixture needles; no behavior there changed.

## Where to resume

Open the pull request for `autopilot/iter-2026-09-04-06` against `main`, run the
D-068 pinned reviewer over the full merge-base..HEAD range, address findings,
wait for `audit` and `ci-gate`, and merge. Then record the merge outcome in a new
`docs/sessions/` file — never by editing this one.

## Merge outcome

_To be recorded by the next iteration's session file._
