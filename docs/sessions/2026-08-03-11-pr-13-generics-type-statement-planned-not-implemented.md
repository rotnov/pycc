# 2026-08-03 checkpoint 11: PR-13 (generics + `type` statement) planned, no code written

## Status

Planning-only checkpoint. **No code was written, no tests were added, no PR was
opened.** This session produced the required judgment-call ADRs and the
file-level implementation plan for PR-13, committed on a fresh branch, so the
next session can start implementation directly instead of re-deriving scope.

## What was done

- D-021 preflight: fetched `origin/main` (tip `a2527a6`, no drift observed
  during this session — same commit at start and end), created a fresh
  worktree/branch `feat/v0-2-pr13-generics-type-stmt` from it (this
  repository's dispatching worktree, `.claude/worktrees/project-overview-53ef3d`,
  had unrelated pre-existing state per the task instructions and was
  deliberately not reused), ran `cargo doc --workspace --no-deps` (succeeded,
  confirms the tree builds and docs are current).
- Verified the open questions PR-13 needed to resolve, against real source
  rather than the design doc's own framing, before writing anything:
  - `Ty` (defined in `crates/pycc_hir/src/lib.rs`) has no type-parameter
    variant yet; PR-10 did not add one despite the design doc flagging it as
    "PR-10 or PR-13, whichever gets there first" (D-089/design doc §4).
  - `def f[T](...)` hits an explicit, already-implemented `unsupported`
    rejection in `lower_function` (`def.type_params.is_some()`), the same
    shape as every other deliberately-unimplemented-syntax guard in this
    compiler.
  - `Stmt::TypeAlias` is already recognized by `pycc_ast`'s `stmt_range` (the
    vendored parser produces the node) but has no `pycc_hir::lower_stmt` arm,
    so it falls into the generic `C0001` catch-all today, the same fate PR-11
    recorded for `CmpOp::In` and PR-12 for `Expr::UnaryOp`.
  - Confirmed current highest ADR is D-132 and highest diagnostic code is
    `T0041` (re-verified against the freshly fetched `origin/main`, not
    trusted from any older doc, per this repo's own repeated-collision
    history).
- Wrote three new ADRs in `docs/DECISIONS.md`, status `proposed` pending
  implementation-time re-verification of one assumption each ADR states
  explicitly:
  - **D-133**: generic-function type-parameter representation is a new
    `Ty::Param(Box<str>)` variant, resolved by call-site substitution before
    `pycc_mir` lowering, distinct from `Ty::Infer`.
  - **D-134**: PR-13's thin slice — exactly one type parameter, any number of
    call sites, each independently monomorphized to one of the four scalar
    `Ty`s the existing solver already reaches; everything else (multiple type
    parameters, a type parameter in a container position, an inconsistent or
    non-scalar call site) rejected pre-codegen with new diagnostic `T0042`.
  - **D-135**: `type` statement and legacy `TypeAlias` are a compile-time-only
    name-to-`Ty` binding table inside `pycc_hir`/`pycc_types`, with zero HIR,
    MIR, codegen, or runtime footprint; a generic alias is out of scope and
    shares `T0042` with D-134's rejections.
- Wrote the file-level implementation plan:
  `docs/superpowers/plans/2026-08-03-v0-2-pr-13-generics-type-statement.md`
  — seven tasks (Ty variant + frontend arity gate; symbolic body check +
  call-site substitution in `pycc_types`; monomorphized MIR lowering;
  `type`/`TypeAlias` lowering; two conformance fixtures; the six-document
  documentation sweep AGENTS.md requires; review-and-merge). The plan
  explicitly flags several details that need re-verification against the
  actual `pycc_mir`/`pycc_types` monomorphization helper names at
  implementation time rather than guessing them now (e.g. the exact existing
  `list[int]` mangling/dispatch convention Task 3 must mirror) — this is a
  known gap in the plan's own "zero placeholder" bar, recorded here rather
  than silently papered over.

## What was explicitly NOT done

- No implementation of any of the plan's seven tasks.
- No test written, no fixture written, no diagnostic code registered in
  `docs/DIAGNOSTICS.md` yet (only planned).
- `subagent-driven-development`'s per-task implement/review/fix loop was not
  started.
- No pull request opened.
- The three ADRs are `proposed`, not `accepted` — they record this session's
  design decision but have not been validated against working code yet;
  flip to `accepted` once Task 1-4's actual implementation confirms the
  representation holds up (in particular, `Ty::Param`'s exact byte layout
  against the D-109 16-byte ceiling, and whether the frontend-perf-gate is
  affected at all by this variant, since a symbolic pre-instantiation type
  check runs on every generic function body regardless of whether it's ever
  called).

## Where a fresh session should resume

1. Re-run the D-021 preflight from scratch (fetch `origin/main` again; do not
   assume this session's `a2527a6`/D-132/T0041 baseline still holds).
2. Read `docs/superpowers/plans/2026-08-03-v0-2-pr-13-generics-type-statement.md`
   in full, then D-133/D-134/D-135 in `docs/DECISIONS.md`.
3. Start Task 1 (`Ty::Param` variant + `lower_function`'s single-type-parameter
   acceptance) using `superpowers:subagent-driven-development`'s per-task
   implement/review/fix-loop pattern, exactly as the task instructions
   describe for the whole PR.
4. Nothing here blocks starting immediately — this was a resource/turn-budget
   stopping point for this session, not an external blocker (no CI gate, no
   manifest-protection sequence, no concurrent-session conflict was hit).
