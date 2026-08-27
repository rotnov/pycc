# Incident: platform-wrapper-bypassed-by-new-code

**Date:** 2026-08-20
**Topic:** platform-wrapper-bypassed-by-new-code
**Verdict:** shipped (automated verify, see 2026-08-27 update)

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
strips comment lines, and checks the project's three recorded protections
for this hazard to three honestly-stated depths:

1. the owning conversion wrapper — fully checked: every call to the
   string-returning printer API must name the wrapper on the same line.
   The check is deliberately not keyed on the receiver's name, so a
   correctly wrapped call on some other value passes too;
2. the platform-guarded verification wrapper — fully checked: exactly one
   direct call to the verification API may exist, the wrapper's own;
3. suppressing the drop at the point the target triple is created — only
   tripwired. That wrapping is structural and spans several lines, so a
   line-oriented scan cannot confirm it; what the scan can do is pin the
   number of triple-producing call sites, so adding one fails the test and
   sends the author to the explanation above it.

The search needles are assembled at run time so the test's own body is not
counted as a violation of itself. The test's own comment states which
protection is checked and which is merely tripwired, rather than letting
the name imply uniform coverage — an independent review round caught an
earlier version of this comment claiming the crate had "exactly two safe
entry points" when its own ADR records three.

## Fixture

None. This is a static gate, not a behavioural change, so the arena has
nothing to measure — it exercises agents across harnesses and cannot run
a `cargo test` invariant. Proven instead by deliberate violators, both
directions, as `references/arena.md` prescribes for this artefact type.

## Verify

`verify: manual` at the time this section was first written -- superseded
by "## Update (2026-08-27): automated (#619)" below, which is now the
actual verification path. Kept here as the historical record of the
original hand-run proof.

Clean tree accepted, three deliberate violators rejected with a
non-zero exit:

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

# D: a new, unsuppressed target-triple call site
D_RC=101
assertion `left == right` failed: a TargetTriple owns an LLVMString and must be created
inside a ManuallyDrop (D-029); this count is a tripwire, so if you added a call site,
wrap it and raise the number -- if you removed one, lower it
  left: 3
 right: 2

# E: the negative control -- a correctly wrapped printer call on a receiver
#    other than the one the original version hardcoded. Must be accepted.
E_RC=0
```

## Sweep result

The crate is the workspace's only dependent of the FFI wrapper crate.
Its two sibling modules use that crate but touch none of the hazardous
APIs, so the guard's assertions hold across the whole existing surface
with no pre-existing violations to fix. The run-time directory scan keeps
that true for modules added later.

## Known limits

The guard is textual, not semantic: it recognizes each API by its call
spelling. A call reached through a re-export under a different name, or
split across lines, would evade it, and the third protection is pinned by
a count rather than proven. That is accepted — the failure this guards
against is a contributor reaching for the obvious raw API, not an
adversary — and the test's own comment says so rather than implying more.

## Update (2026-08-27): automated (#619)

The five cases above were proven by hand once, then pasted here as a
transcript with no fixture and no gate against the checking logic itself
regressing — so `Fixture: None` and `verify: manual` were both quietly
becoming false claims the moment anyone touched the guard again without
re-running the same manual proof. #619 closed that gap:

- The three checks inside
  `tests::every_inkwell_llvm_string_call_routes_through_a_d029_wrapper`
  were extracted into `tests::d029_guard::d029_violations(sources: &[(&str,
  &str)], expected_triple_call_sites: usize) -> Vec<String>`, a pure
  function with no filesystem access. The real test now calls it against
  the crate's actual sources and asserts the result is empty; the
  extraction preserves the run-time-assembled-needle trick that keeps the
  function's own body (and any fixture built the same way) from tripping
  the scan it performs on its own crate's `src/` directory. The guard and
  its own tests live in `crates/pycc_codegen/src/tests/d029_guard.rs`, a
  cohesion-driven submodule of `tests.rs` rather than more lines added
  directly to that already far-over-threshold file (AGENTS.md's
  decomposability rule).
- A new `tests::d029_guard::d029_violations_tests` module drives that
  same function against synthetic single- and multi-file sources, one
  test per case A-E above plus a positive compliant baseline and one case
  proving the tripwire's expected count is caller-supplied rather than
  hardcoded. `cargo test -p pycc_codegen d029` runs all of it, and it
  runs on every `cargo test` invocation like the guard itself, not only
  when a contributor remembers to reproduce this file's shell transcript.

**Fixture:** now exists — `tests::d029_guard::d029_violations_tests`.
**Verify:** now `verify: automated` — `cargo test -p pycc_codegen d029`.
The manual transcript above is kept as the historical record of the
original proof; it is no longer the actual verification path.
