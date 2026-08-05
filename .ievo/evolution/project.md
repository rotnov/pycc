# Project — Evolution Overlay

(project-wide rules accumulated here; loaded into context via marker block in AGENTS.md)

## 2026-07-24 22:54 UTC — Keep the project roadmap current
**Trigger:** user-defined convention

родамап нужно тоже держать обновленным

## 2026-07-26 12:58 UTC — Check PR state before waiting for CI
**Trigger:** user-observed mistake during PR monitoring

**Moved to** `.claude/skills/autopilot-async-monitoring/SKILL.md` (extracted 2026-08-04).

## 2026-07-26 21:37 UTC — Do not monitor historical pull requests
**Trigger:** user-observed mistake during PR monitoring

**Moved to** `.claude/skills/autopilot-async-monitoring/SKILL.md` (extracted 2026-08-04).

## 2026-07-30 17:50 UTC — Consider a background watcher for the autopilot loop
**Trigger:** user-defined convention

**Moved to** `.claude/skills/autopilot-async-monitoring/SKILL.md` (extracted 2026-08-04).

## 2026-08-04 05:19 UTC — Dispatched orchestrator agents must not stop-and-wait for their own sub-dispatches
**Trigger:** user-observed mistake during PR-13/PR-14 autonomous delivery

**Moved to** `.claude/skills/autopilot-async-monitoring/SKILL.md` (extracted 2026-08-04).

## 2026-08-04 05:48 UTC — Never run an unbounded `find` across the whole filesystem
**Trigger:** user-observed mistake during PR-14 autonomous delivery

find

Context: a dispatched PR-14 implementer agent ran `find / -path /pro...` (a
full-filesystem scan from root) and a separate `find ~/.cargo/registry/src
-maxdepth 1 -iname "*ruff_python_ast*"` while trying to locate something
(likely a crate or reference file). Both were caught, via a screenshot of
the client's "Background tasks" panel, still running after 5m55s and 3m54s
respectively with no sign of finishing — `find /` has no natural bound on a
real filesystem (mount points, permission-denied directories that still get
traversed slowly, network/cloud-synced folders) and can run for a very long
time or effectively hang. Because the command was already running inside a
dispatched sub-agent's own Bash tool call, it could not be killed directly
from the orchestrating session — a `SendMessage` to that agent only gets
delivered after its current tool call returns, so the fix had to wait for
the command to finish or time out on its own.

**Rule:** never invoke `find` with an unbounded or filesystem-root path
(bare `/`, `~`, or any directory not specifically known to be small) when
searching for a file, crate, or reference during agent work in this
project. Use a targeted approach instead: `cargo metadata --no-deps` or
`cargo tree` scoped to the current workspace for crate/dependency lookups,
the `Grep`/`Glob` tools scoped to `crates/` or another specific known
directory, or `find` with an explicit, narrow, bounded path and (for a
large directory) a `-maxdepth` limit. If genuinely uncertain where
something lives, ask or use a narrower repeated search rather than
defaulting to a whole-filesystem scan.
