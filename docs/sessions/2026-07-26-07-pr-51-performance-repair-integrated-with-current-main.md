# 2026-07-26 — PR #51 performance repair integrated with current main

**Snapshot evidence:** the containing merge integrates local performance-repair
parent `a7f048d` with refreshed `main@128285fbfbcfaa29b1a6c8fa81da4d84bae8d67f`.
[PR #51](https://github.com/rotnov/pycc/pull/51) remained open and non-draft at
remote head `c1e855590a23307bcd8472979ff37f8bbfd0f8d9` before this local integration
was pushed. That remote head ran required CI as run `30206099702` from
active-D-062 `main@45545bb057f5cd9e8712610c6137f53ef56d3aae`.
Immediately before preparing the follow-up commit, a fetch confirmed
`origin/main` still at `128285fbfbcfaa29b1a6c8fa81da4d84bae8d67f`; GitHub
still reported the old remote head as open, non-draft, and dirty, with one
unresolved P1 review thread.

**Gate result:** trusted audit, agent checks, 100% coverage, Linux/macOS,
cross-compile, and the 5+5 measurement job passed. The isolated comparator
correctly blocked the changed-source candidate at `+10.7215%`: predecessor
aggregate median `7964.08 ns`, candidate `8817.95 ns`. This was not retried or
waived. The benchmark does not execute the changed root CLI sources, but it
exposed an existing redundant type-checker walk that could be removed without
changing the gate.

**Repair:** `pycc_types::check` now constructs already-concrete
function signatures directly and reserves constraint collection for modules
that contain real `Ty::Infer` signatures; a failed concrete validation falls
back to the historical solver-first order so diagnostic selection is stable.
The workspace coverage gate passes at 100% lines and regions, including
explicit fast-path, diagnostic-parity, solver-path, and collector edge cases;
workspace clippy, Rust documentation, roadmap evidence, and agent-asset checks
also pass. An initial local Criterion comparison improved from about `7.15 µs`
to `5.85 µs` (`−18.0%`); a later run after the diagnostic-order fallback
measured `6.99 µs` (about `−2.3%` from the same original observation). This
single-host evidence is noisy and is not selected as the gate result; the next
fixed 5+5 CI comparison remains authoritative.

**Pre-merge review repair:** the unresolved thread correctly found that valid
but unsupported Python could still panic during HIR lowering, aborting the
pre-commit batch with exit 101. The follow-up converts every user-reachable HIR
capability rejection to a spanned `C0001` diagnostic, keeps only an internal
parser-invariant assertion, and proves both exact CLI rendering and continued
multi-file checking after an unsupported construct. The workspace coverage
gate passes at 100% lines and regions; clippy, Rust documentation, roadmap
evidence, and agent-asset checks pass as well.

**Local review:** the exact pinned staged-diff reviewer found no implementation,
contract, security, test, or documentation defect in the repair; its only
finding was that the previous handoff text still listed that now-completed
review as pending. This paragraph replaces that stale instruction.
The subsequent full-range review found that adding a direct `ruff_text_size`
dependency would violate D-062's byte-identical manifest/lock precondition and
block CI before measurement. The fix keeps `Cargo.toml` and `Cargo.lock`
identical to the predecessor and exposes byte ranges through the existing
`pycc_ast` facade instead; an exhaustive facade test covers every upstream
statement and expression variant at 100% line and region coverage.

**Where to resume:** commit and push the verified repair, repeat exact-revision
`pre-commit try-repo`, and resolve the P1 thread only after the remote head
contains the verified fix. Treat the new CI run as new candidate evidence, not
a rerun of the failed head, and merge only if every required check is green
with no unresolved actionable review thread.
