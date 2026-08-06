---
id: D-021
title: "Agent task preflight and documentation refresh"
status: accepted
---

## D-021: Agent task preflight and documentation refresh

- Status: accepted
- Context: this AI-first repository treats specifications and generated Rust API documentation as implementation inputs. Starting from a stale remote ref, stale rustdoc, or an unidentified dirty tree can make an otherwise-correct agent optimize against the wrong contract or overwrite user work.
- Decision: before a new task mutates the repository, the agent records status and HEAD, fetches/prunes without changing checked-out files, resolves the remote default branch, starts from that exact commit in a clean task branch/worktree, generates `cargo doc --workspace --no-deps`, and reads the specification/rustdoc that owns the affected area. Continuing dirty work is allowed only in place and only after comparing it with refreshed remote state; no implicit pull, merge, rebase, reset, or branch switch is authorized.
- Authority and scope: the preflight authorizes read-only Git/network discovery, creation of an isolated task branch/worktree, and local generated rustdoc. It does not authorize integrating upstream changes into a dirty tree, publishing a branch, or changing external systems.
- Privacy and failure behavior: preflight output stays in the task transcript and must not publish local paths or repository content. A failed fetch or documentation build is recorded and understood; older generated documentation must not be represented as current.
- Rollback: the isolated worktree/branch can be removed after preserving any intended commits. Changing this mandatory sequence requires a superseding decision because every later agent task depends on it.

