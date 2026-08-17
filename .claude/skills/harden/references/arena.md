# Arena: proving an artefact works

Empirical check of a patch. The same task runs across every installed harness ×
two conditions (with and without the patch) — each run inside a Docker
container, in an isolated copy of the fixture. Output: a table of what passed,
tool failures, turns, tokens, time, a blind quality score per run, and whether
the agent got stuck.

Answers exactly one question: **does the patch help or not.**

## When to use

- A rule was added to `AGENTS.md` — does it actually work?
- A skill was edited — better, or broken?
- An incident from the log was fixed with an artefact — reproduce the incident
  and confirm the artefact holds.

Do not ship an artefact without this. A rule whose effect nobody measured
is a hope, not a mechanism.

## Fixture

A directory holding the task. Required files:

```
<fixture>/
  task.md            the prompt — exactly what the agent receives
  verify.py          objective check; exit 0 = task solved
  control.md         baseline
  patch.md           the patched version (what is being tested)
  setup.py           optional; seeds state that copying files cannot carry
```

Everything else the task needs — `skills/`, sources, data, `pyproject.toml` —
is optional and copied wholesale into every run.

`setup.py` exists for state a copied tree has no way to hold: a git repository,
a virtualenv, a seeded database. It runs once per run, from the fixture but with
the working copy as cwd, before the agent starts. It is deliberately **not**
copied in — an agent that reads the setup script learns what the fixture is
testing, which is the one thing it must not know. If it exits non-zero the whole
run aborts: a fixture that cannot seed itself breaks every condition identically,
so there is nothing to compare and no verdict to withhold.

**Paths in `setup.py` are `__file__`-relative, never host-absolute.** In docker
mode the project is mounted read-only at `/repo`, so a hardcoded
`/Users/...` does not exist inside the container. Measured on the first
containerised run: one fixture broke on exactly this before any agent started.

`verify.py` runs at the run's root after the agent finishes. It must check the
**result**, not the process: file created and correct, tests green, output
contains what it should. One exit code, no interpretation.

The contract is exactly that and nothing more: the script runs in the workdir
**with no arguments** — the arena passes nothing, and there is no response
file unless the agent happened to write one. Interrogate the workspace: disk,
git state, executed effects. The agent's prose sits in `_agent-output.txt`,
but grading prose is grading the story — measured: an agent reworded its
answer specifically to beat a phrase grep, and three field fixtures graded a
`response.md` no arena writes, so passing required the accident of authoring
that filename.

Make it catch **workarounds**, not just the happy path. A patch that the model
cannot satisfy tends to get bypassed by weakening the constraint — the example
fixture checks that `requires-python` was not silently lowered, which is exactly
how a real run cheated.

And prove the check can fail before believing any run: execute it by hand
against a deliberately broken tree — fabricated-success prose, no real work
done. A control that passes under a broken implementation is not a control
(harvested from a field project's journal, where a binary-size "did the
optimizer run" check passed under a stub that ignored the flag entirely).

## The control has to fail

The arena refuses a verdict for any harness whose control passed every run, and
refuses one entirely when nothing passed anywhere. Both refusals exist because a
comparison needs a baseline that shows the problem: if control already succeeds,
"the patch changes nothing" is a fact about the fixture, and if nothing succeeds
at all, the likeliest cause is a `verify.py` testing the wrong thing. One real
run failed all 18 while every agent had solved the task — the check tripped over
untracked files the arena itself had created.

So write the fixture backwards from the failure: make the task reproduce it under
`control`, confirm that it does, and only then ask whether `patch` prevents it.

**A rule that waits for a human cannot be measured this way.** Runs are
non-interactive, so a rule of the form "stop and ask" stalls both conditions
identically and the comparison measures nothing. Restate it as something with a
countable footprint — append a line to a log, write a file, name the step in the
output — and the same rule becomes observable: one line where it belongs, two
where it leaked.

## Running

```bash
uv run scripts/arena.py <fixture-dir> [--runs N] [--sandbox auto|docker|host]
```

- `--runs N` — repetitions per combination (default 1). Use 3+; behaviour is
  stochastic and a single run proves nothing.
- `--sandbox` — `auto` (default) uses docker when the daemon is up and falls
  back to host runs with a printed warning; `docker` refuses to run without it;
  `host` is the old behaviour, contamination included.
- `--only claude,codex` — a harness subset, for smoke runs.
- `--no-judge` — skip the blind quality judge.
- `--rebuild-image` — rebuild the container image (CLI updates land this way).
- `--docker-login <harness>` — one-time interactive login into that harness's
  container volume; needed for claude, whose macOS token lives in the Keychain
  and cannot be copied as a file.
- `--out DIR` — where results go (default `.harden/arena-runs/<timestamp>`).

Models are pinned on purpose — comparison only means something at fixed models:

| harness | model | why this one |
| --- | --- | --- |
| Claude Code | `sonnet` / effort `low` | mid-tier: a strong model solves the task correctly regardless, and the patch never gets tested |
| Devin | `glm-5-2` | free on promo — repeated runs cost nothing |
| Codex | `gpt-5.6-luna` / effort `low` | light, comparable to the others |
| Grok | `grok-4.5` / effort `low` | |

Override with `ARENA_CLAUDE_MODEL`, `ARENA_DEVIN_MODEL`, `ARENA_CODEX_MODEL`,
`ARENA_GROK_MODEL`, `ARENA_EFFORT`, `ARENA_JUDGE_MODEL`, `ARENA_IMAGE`.

A harness whose CLI is not installed is skipped with a printed line, not treated
as a failure — so the set is whatever the machine has, and a skipped harness must
never read as one that was covered.

## Docker is the environment, not an option

Host runs kept measuring the machine instead of the model, three separate times:
a package installed by one run poisoned the fixture for every later one; the
operator's settings changed the agents' language and effort; globally installed
skills gave some harnesses capabilities the fixture never granted. Each was
found by accident, after runs were already burned.

The container removes all three by construction:

- **One image** (`assets/arena/Dockerfile`): node + the harness CLIs + uv + a
  bare python. No pandas, no openpyxl — a fixture premised on an absent
  dependency stays premised.
- **One named volume per harness**, mounted at `/root`: auth and caches persist
  between runs, and nothing else exists — no user settings, no global skills.
  Runs never mount it directly: each run gets a throwaway **clone** of it, so
  runs of one harness execute concurrently (up to `ARENA_LANE_PARALLEL`, 6,
  under a global `ARENA_MAX_CONTAINERS` cap, 8) and the seed volume stays
  pristine. Clones die with their run; a startup sweep removes what a killed
  process left, recognizing live siblings by the PID in the clone's name.
- **Mounts per run**: the working copy at `/work` (rw), trace logs at `/trace`
  (rw), the project at `/repo` (**read-only** — no run can write back into the
  source tree, which a symlinked install once did).
- **setup.py, the agent and verify.py all execute inside the container** — the
  check runs in the same world as the work, or a host verify would pass on host
  packages the agent never had.

Auth: `codex` and `devin` credential files are copied from the host into the
volume automatically (credentials only — copying a config directory would
re-import the global skills the container exists to exclude). `claude` stores
its token in the macOS Keychain and `grok`'s auth file is device-bound, so each
needs its `--docker-login <harness>` once. A harness with no working
credentials in its volume falls back to a host run, and both the console and
the report say so. Presence of a file is not proof — a seeded grok auth.json
read as "authenticated" while every run died "Not signed in", and
`devin auth status` exits 0 either way, so where the CLI offers an oracle the
words of its answer decide, not the return code.

devin's containerisation was believed impossible ("Mach-O binary, no Linux
build") until the install script's platform matrix was actually read: it ships
aarch64/x86_64 linux bundles, and its seeded API key logs in cleanly. The
lesson stands in the Dockerfile comment; the report's placement line still
carries every exception per harness — a mixed run is stated, never silent.

### Host fallback

With `--sandbox host` (or no docker) the old per-harness confinement applies,
each measured rather than preferred:

| harness | how | note |
| --- | --- | --- |
| codex | its own `-s workspace-write` | wrapping it in Seatbelt as well makes it stop solving the task |
| grok | its own `--sandbox workspace` | a write to `$HOME` returns *Operation not permitted* |
| claude | macOS Seatbelt wrapper + `--settings` clean file | plugins stay visible: 124 skills on one measured machine |
| devin | **none** | its `--sandbox` forces a mode an org policy can block; named in every report |

Host runs inherit the host's python packages, and the report withholds the
verdict when the packages the task needs are already importable there.

## What gets measured

| metric | source | why |
| --- | --- | --- |
| `verdict` | exit code of `verify.py` | solved or not |
| `judge` | blind LLM judge, 0–10 | quality beyond pass/fail |
| `duration_sec` | wall clock | cost of the solution |
| `turns` | claude result envelope, codex event stream | how many model round-trips |
| `tokens` | claude `usage`, codex stream / `tokens used` trailer, grok envelope | what the run cost |
| `tool_calls` | `PreToolUse` hook | volume of work |
| `tool_failures` | `PostToolUseFailure` (CC), output parsing (others) | how often it stumbled |
| `fail_per_10_calls` | derived | failures normalised by work done |
| `edits` | Write/Edit calls (traced) or files changed on disk (untraced) | how much was actually built |
| `stuck` | normalised target, 3+ failures on one target | did it bang its head on a wall |
| `infra_failure` | markers in agent output | run was broken by the environment, not the patch |

**An unmeasured value prints as `—`, never as `0`.** Only claude and codex are
hook-traced; devin exposes no usage at all. A zero that means "no
instrumentation" reads as "the agent did nothing" — that misread happened on a
real report, where devin's 0-call rows looked like idle runs while the agent
had in fact solved the task.

`stuck` is counted per **target**, not per command string — otherwise 95% of the
phenomenon disappears (on real data: 2 hits by command string vs 59 by target).

`tool_failures` alone is meaningless: an agent that did nothing reports zero
failures and looks perfect. Always read it together with `verdict` and `edits`.

## The judge

"Which model handled it better" is not answerable from pass/fail alone: two
passing runs can differ by an order of magnitude in cleanliness. After all runs,
a judge model (`ARENA_JUDGE_MODEL`, default `sonnet`) grades runs 0–10 on the
host through the claude CLI. Each call takes 60–90 s, so the default scope is
**one representative run per harness × condition** — the first whose verify
verdict matches the cell's majority — four calls in parallel
(`ARENA_JUDGE_PARALLEL`). A 24-run report is judged in ~2 minutes for 8 calls.
`--judge-all` scores every run when the extra resolution is worth the spend;
judging all N runs of a cell mostly re-scores the same behaviour.

The judge is **blind**: it never sees the harness name, the condition, or
verify's result. It is also **advisory**: `verify.py` stays the ground truth,
and a judge/verify disagreement is listed in the report as a finding about one
of them, not resolved by averaging.

What it sees is the story **and the facts** (D-017): the task, the diff, the
transcript tail — plus the file tree of the working copy after the run and the
recorded tool calls, with model self-identification scrubbed throughout.
Project claims the tree and calls cannot support are reported as
`unsupported_claims` and cost the whole discipline score.

The scope is project claims only, and the boundary was measured from both
sides on one run. A "run skill exists in this project" claim first scored
10/10 (story-only judging saw no problem), then got flagged as fabrication
(tree-only fact-checking saw no such skill) — and both were wrong: the skill
is **built into the harness binary**, read from the agent's own skill list,
worded identically across four independent runs. Identical wording across
runs is the tell — hallucinations vary, injected context repeats. Built-in
capabilities are outside the judge's evidence, so it is told to neither
confirm nor punish claims about them.

The report ranks harnesses by mean patch-condition score and lists any
collateral the judge noticed. Known limit: the judge shares a vendor with one
contestant; blinding removes the label, not the stylistic kinship.

## Reading the result

A verdict is printed only when the data can carry one. Global causes block it
globally; per-harness causes exclude that harness by name and the rest carry
the verdict:

- fewer than 3 runs per combination — blocks;
- no network to pypi (dependencies cannot be installed) — blocks;
- every harness on a host that already has the packages — blocks; only *some*
  harnesses hosted (devin always is) — those are excluded, the docker-placed
  ones decide;
- a harness with no clean runs in a condition (dead auth, infra markers in
  every run) — excluded by name, never a veto. Measured: six 0-second
  "Not signed in" grok runs must not discard clean claude and codex data.

When blocked, the report says what to fix instead of guessing. This is
deliberate: a verdict on contaminated data is worse than no verdict, because
it looks like a result. That failure happened for real — a run was declared
"patch harms" when the actual cause was a disabled sandbox network.

Three outcomes worth distinguishing:

- **profit** — control fails and patch passes, or both pass but the patch shows
  fewer failures per 10 calls. The artefact works.
- **zero** — identical columns. Do not ship, and do not reword it either.
  The control reproduced the failure and the patch left it untouched: the model
  read the artefact and was unmoved, so the mechanism is too weak for this class
  rather than badly phrased. Escalate one rung instead — this is the same signal
  as a recurrence, bought early, since a recurrence *is* a `zero` discovered in
  production months later.
- **harm** — worse with the patch. A different failure from `zero`: here the
  model is not indifferent but resisting — a rule it cannot satisfy gets worked
  around, and the workaround is often destructive. A stronger version of the same
  artefact usually makes this worse.

`zero` and `harm` therefore point in different directions. Indifference means
climb the ladder; resistance means the type is wrong and a different kind of
guard is needed.

Also watch the **spread across harnesses**. A patch that helps one and harms
another is not ready.

## Limits worth knowing

- **A strong model masks the effect.** Opus solves the task correctly without any
  patch, so no rule will show a difference. Hence the mid-tier models above.
- **Codex needs hook trust.** The script passes
  `--dangerously-bypass-hook-trust`; acceptable only because the hooks are
  generated by the arena itself and live in a throwaway run directory.
- **Codex needs the sandbox network explicitly enabled.**
  `-c sandbox_workspace_write.network_access=true`. Without it dependency
  installation fails with `Could not resolve host` and the arena measures
  connectivity instead of the patch.
- **Devin runs with `--permission-mode dangerous`, deliberately.** The
  `Exec(...)` allowlist in `.devin/config.json` gives false control: Devin only
  checks the first binary of a chain, so one permitted `cd` opens everything
  after `&&` (verified: `cd … && uv venv …` is rejected while
  `head … && echo … && wc …` passes whole). An explicit flag beats an illusion
  of constraint. Safety rests on the run directory being throwaway — never run
  the arena inside a working repository.
- **Agents read `AGENTS.md` from outside the run directory.** One run reported
  seeing `/opt/homebrew/AGENTS.md` and chose to ignore it. Correct call, but
  luck rather than guarantee. Contamination hits control and patch equally, so
  the delta is partly protected — absolute numbers are not.
- **Devin and Codex metrics are poorer.** Codex hook payloads carry no exit code;
  Devin reports `success: true` for a failed command. Failures there are inferred
  from text and counted less precisely than on Claude Code.

## Output

- `<out>/report.md` — tables, judge section, verdict or the reasons it is withheld
- `<out>/results.jsonl` — raw per-run metrics, judge verdicts included
- `<out>/environment.json` — preflight: placement per harness, image id, network,
  host and container interpreters, versions
- `<out>/{control,patch}/<harness>-<n>/` — full working copies:
  `_agent-output.txt` (prose, whatever the CLI's native format), `_agent-raw.txt`
  (the native JSON when there was one), `_diff.patch` (what the agent changed),
  `_trace.tsv`, `_verify.txt`

Copies are kept on purpose: when a result is surprising, that is where to look.
