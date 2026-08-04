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

## 2026-08-04 05:54 UTC — Run a project's own local validation scripts before pushing, not just after CI fails
**Trigger:** user-observed mistake while pushing the autopilot-async-monitoring skill

а ты локально не гоняешь его перед пушем?

поогоняй лоакально его перед пушем и вообще все тесты перед пушем, это быстрее чем ждать ci

Context: pushed a new skill (`.claude/skills/autopilot-async-monitoring/`)
without first running this repo's own CI-equivalent local checks
(`python3 -m unittest discover -s scripts`, `python3
scripts/validate_agent_assets.py`, `bash scripts/check-codex-marketplace.sh`).
CI caught two real, sequential problems that local runs would have caught
immediately: a missing `.agents/skills/` Codex wrapper (AGENTS.md's
cross-platform-discoverability rule), then a stale hardcoded `assert
len(wrappers) == 20` completeness count in
`scripts/check-codex-marketplace.sh` that needed bumping to 21 for the new
skill. Both were fixed in separate follow-up pushes, each waiting on a full
CI round-trip (multiple minutes) to surface, when running the same checks
locally first would have caught both in seconds and produced one clean
push instead of three.

**Rule:** before pushing any change in this repository, run the relevant
local test/validation suite(s) first, not just a narrowly-scoped script —
the user's own framing was "и вообще все тесты перед пушем" (and really
all the tests before pushing), not merely the one check most obviously
related to the diff. Concretely: grep `.github/workflows/ci.yml` for the
exact commands each relevant job runs (e.g. `python3 -m unittest discover
-s scripts -p 'test_*.py'`, `python3 scripts/validate_agent_assets.py`,
`bash scripts/check-codex-marketplace.sh`, and for Rust changes `cargo
test --workspace` / the relevant crate's tests) and run all of them locally
before pushing, rather than relying on CI to be the first place a
mechanical, locally-reproducible check runs. Local iteration is strictly
faster than a CI round-trip (multiple minutes per push) for anything that
CAN be run locally — this is the same "check real state before waiting"
discipline `autopilot-async-monitoring/SKILL.md` already covers for
PR/CI/agent status, applied one step earlier: verify locally before
creating the async wait in the first place, not just before consuming its
result.
