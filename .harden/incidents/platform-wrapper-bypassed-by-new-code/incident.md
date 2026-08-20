# Incident: platform-wrapper-bypassed-by-new-code

**Date:** 2026-08-20
**Topic:** platform-wrapper-bypassed-by-new-code
**Verdict:** shipped (manual verify)

## Symptom

A new codegen-depth IR test added on PR #626 (issue #624) called the
underlying FFI wrapper crate's module-verification API directly. Every
local gate passed on the development host; the change then crashed the
whole test binary in CI on the one platform that exercises the hazard,
with a process-level access violation rather than a test failure.

## Root cause

The crate already owns a platform-safe wrapper for that exact API — a
function that is compiled to a no-op on the affected platform precisely
because the underlying library's message-disposal path faults against the
prebuilt toolchain used there. The new test reached past the wrapper to
the raw third-party API. Nothing mechanical objected: the wrapper is a
convention, not a boundary, and the affected platform is the only place
the fault is reachable, so no gate on the development host could observe
it. Gap type: **absence** — no artefact of any kind guarded the wrapper.

## Termination point

`precommit`/`review-check` tier, realized as a workspace test:
`crates/pycc_codegen/src/lib.rs`, test module.

A checker script wired into `.github/workflows/ci.yml` was considered and
rejected: `ci.yml` is a steady-state entry in
`tests/fixtures/policy-successor-manifest.json`, so editing it (D-103)
forces a two-PR stage-then-activate cycle onto a single-PR task. A
workspace test needs no CI-configuration change, runs under the existing
`cargo test` gate on every platform, and binds every contributor and
agent equally. This tradeoff was resolved by this session per D-127 and
is recorded here and in the PR body in lieu of an approval pause.

## Artefact

**Type:** precommit-tier static gate (workspace test)
**File:** `crates/pycc_codegen/src/lib.rs`
**Change:** Added
`tests::every_inkwell_llvm_string_call_routes_through_a_d029_wrapper`.
It reads every `.rs` file in the crate's own `src/` directory at run time
(so a module added later is covered without anyone extending a list),
strips comment lines, and asserts two invariants: every call to the
string-returning printer API is an argument of the crate's own owning
wrapper, and exactly one direct call to the verification API exists — the
one inside the platform-guarded wrapper itself. The search needles are
assembled at run time so the test's own body is not counted as a
violation of itself.

## Fixture

None. This is a static gate, not a behavioural change, so the arena has
nothing to measure — it exercises agents across harnesses and cannot run
a `cargo test` invariant. Proven instead by deliberate violators, both
directions, as `references/arena.md` prescribes for this artefact type.

## Verify

`verify: manual`. Clean tree accepted, three deliberate violators
rejected with a non-zero exit:

```
$ cargo test -p pycc_codegen every_inkwell_llvm_string_call_routes_through_a_d029_wrapper
test tests::every_inkwell_llvm_string_call_routes_through_a_d029_wrapper ... ok    # RC=0

# A: a bare verification call added to a test helper
A_RC=101
assertion `left == right` failed: the only direct inkwell verify call may be the one
inside verify_module, which is skipped on Windows; everything else must go through
that wrapper (D-029)
  left: 2
 right: 1

# B: the printer API called without the owning wrapper
B_RC=101
assertion `left == right` failed: every inkwell print_to_string call must be an
argument of llvm_string_to_owned, or its LLVMString drops and faults on Windows (D-029)
  left: 3
 right: 2

# C: the same violation planted in a sibling module, proving the scan is
#    not limited to the file the test itself lives in
C_RC=101
assertion `left == right` failed: the only direct inkwell verify call may be the one
inside verify_module, which is skipped on Windows; everything else must go through
that wrapper (D-029)
  left: 2
 right: 1
```

## Sweep result

The crate is the workspace's only dependent of the FFI wrapper crate.
Its two sibling modules use that crate but touch none of the hazardous
APIs, so the guard's assertions hold across the whole existing surface
with no pre-existing violations to fix. The run-time directory scan keeps
that true for modules added later.

## Known limits

The guard is textual, not semantic: it recognizes the API by its call
spelling. A call reached through a re-export under a different name, or
split across lines, would evade it. That is accepted — the failure this
guards against is a contributor reaching for the obvious raw API, not an
adversary.
