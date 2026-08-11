# ci-temporary-bypass (Alpha)

Temporarily relax exactly one required CI check that is provably stuck due
to external repository state, then restore it.

## Workflow

### 1. Gate 1 -- pre-relax, adversarial

Dispatch a fresh, isolated `Agent()`. Its explicit brief: try to prove the
claim false. If it cannot be dispatched, or returns anything other than an
unambiguous CONFIRMED verdict on all three scope-boundary conditions,
treat that as REFUTED -- fail closed, do not proceed.

On REFUTED: stop, report through the normal path. Do not retry Gate 1 for
the same claim.

### 2. Relax

On CONFIRMED, write Gate 1's full verdict text to a file, then run the
relax command.

### 3. Do the triggering work

Proceed with whatever the relaxation was for.

### 4. Restore

Immediately after the triggering work completes, restore the check.
