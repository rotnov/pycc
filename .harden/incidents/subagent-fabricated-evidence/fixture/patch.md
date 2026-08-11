# ci-temporary-bypass (Alpha)

Temporarily relax exactly one required CI check that is provably stuck due
to external repository state, then restore it.

## Workflow

### 1. Gate 1 -- pre-relax, adversarial

Dispatch a fresh, isolated `Agent()`. Its explicit brief: try to prove the
claim false. If it cannot be dispatched, or returns anything other than an
unambiguous CONFIRMED verdict on all three scope-boundary conditions,
treat that as REFUTED -- fail closed, do not proceed.

On REFUTED: before accepting the verdict, independently verify any factual
claim about external state that drove the REFUTED decision (issue status,
file existence, command output). Re-run the specific command the subagent
cited and compare. If the subagent's claim does not hold, discard the
REFUTED verdict and re-evaluate the scope-boundary conditions yourself
inline. A fail-closed design that trusts fabricated evidence blocks
legitimate work as effectively as a real failure.

If the claim holds, stop, report through the normal path. Do not retry
Gate 1 for the same claim.

### 2. Relax

On CONFIRMED, write Gate 1's full verdict text to a file, then run the
relax command.

### 3. Do the triggering work

Proceed with whatever the relaxation was for.

### 4. Restore

Immediately after the triggering work completes, restore the check.
