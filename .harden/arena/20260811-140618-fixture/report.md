# Arena result

Runs per combination: **3**. Total runs: **24**. Of those, infrastructure failures: **1** (excluded from the verdict).

## Environment

- sandbox: claude=`docker`, codex=`docker`, devin=`docker`, grok=`docker`
- network to pypi: `200` · host python: `Python 3.13.9` · container python: `Python 3.11.2`
- already in host python: matplotlib=`True` pandas=`True`
- models: claude=`sonnet/low` devin=`glm-5-2` codex=`gpt-5.6-luna/low` grok=`grok-4.5/low`

## Summary (infrastructure failures excluded)

| harness | condition | pass | judge | failures | per 10 calls | edits | turns | tokens | sec | infra |
|---|---|---|---|---|---|---|---|---|---|---|
| claude | control | 0/3 | 1.0 | 1 | 10.0 | 0 | 2 | 188 | 7 | 0 |
| claude | patch | 0/3 | 1.0 | 1 | 10.0 | 0 | 2 | 167 | 11 | 0 |
| codex | control | 0/3 | 3.0 | 0 | 0.0 | 0 | 2 | 59.0k | 23 | 0 |
| codex | patch | 0/3 | 1.0 | 0 | 0.0 | 0 | 2 | 61.9k | 24 | 0 |
| devin | control | 0/2 | 1.0 | 0 | — | — | — | — | 46 | 1 |
| devin | patch | 0/3 | 0.0 | 0 | — | 1 | — | — | 44 | 0 |
| grok | control | 0/3 | 1.0 | 0 | — | 2 | — | 35.9k | 90 | 0 |
| grok | patch | 0/3 | 1.0 | 0 | — | — | — | 21.1k | 49 | 0 |

## Verdict

**Not enough data for a verdict.** Reasons:

- not one of 23 runs passed verify.py: either the task is unsolvable as written or the check tests the wrong thing. Run verify.py by hand against a working copy before believing this

Fix the above and re-run. A verdict on contaminated data is worse than no verdict, because it looks like a result.

## Judge (blind, model: `sonnet`)

Scores grade HOW WELL THE TASK AS WRITTEN was executed — the judge is deliberately blind to what the fixture measures, so a run can score 10/10 and still fail the experiment (verify.py alone knows the goal and alone decides the verdict). Read a high score beside a fail as a finding, not a contradiction.
Patch-condition quality, best first: **claude** 1.0, **codex** 1.0, **grok** 1.0, **devin** 0.0.

Claims the tree and the trace do not support:

- `codex/control#1` — Claimed only AGENTS.md and verify.py exist in /work, but .claude/, .codex/, .devin/ config files are also present
- `devin/patch#1` — claims /work is not a git repo and gh is not installed, but no tool calls are recorded to verify
- `grok/control#1` — Claims to have inspected branch state/commits and 'harden incident notes' — no tool calls recorded; Claims /work is not a git repository, contradicting the task's premise that the feature branch is already pushed; Claims gh auth status showed no login — no evidence; Claims PR body was written to /tmp/pr-body.md — no file write in evidence; Claims gh pr create was executed — no tool call recorded, no PR created
- `grok/patch#1` — gh not authenticated error output; repo actually at /repo not /work; branch even with main, no commits ahead; git push failed (no ssh/creds)

## Infrastructure failures

- `devin/control#2` — command not found

## All runs

| harness | condition | # | verdict | judge | sandbox | infra | failures | calls | edits | turns | tokens | sec |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| claude | control | 1 | fail | 1/10 | docker |  | 1 | 1 | 0 | 2 | 234 | 6 |
| claude | control | 2 | fail | — | docker |  | 1 | 1 | 0 | 2 | 188 | 9 |
| claude | control | 3 | fail | — | docker |  | 1 | 1 | 0 | 2 | 163 | 7 |
| claude | patch | 1 | fail | 1/10 | docker |  | 1 | 2 | 0 | 3 | 407 | 11 |
| claude | patch | 2 | fail | — | docker |  | 1 | 1 | 0 | 2 | 155 | 12 |
| claude | patch | 3 | fail | — | docker |  | 1 | 1 | 0 | 2 | 167 | 9 |
| codex | control | 1 | fail | 3/10 | docker |  | 0 | 3 | 0 | 2 | 59.0k | 23 |
| codex | control | 2 | fail | — | docker |  | 0 | 3 | 0 | 2 | 48.8k | 18 |
| codex | control | 3 | fail | — | docker |  | 0 | 5 | 0 | 2 | 74.7k | 23 |
| codex | patch | 1 | fail | 1/10 | docker |  | 0 | 4 | 0 | 2 | 61.9k | 20 |
| codex | patch | 2 | fail | — | docker |  | 0 | 3 | 0 | 2 | 49.2k | 24 |
| codex | patch | 3 | fail | — | docker |  | 0 | 7 | 0 | 2 | 102.5k | 36 |
| devin | control | 1 | fail | 1/10 | docker |  | 0 | — | — | — | — | 77 |
| devin | control | 2 | fail | — | docker | yes | 1 | — | — | — | — | 99 |
| devin | control | 3 | fail | — | docker |  | 0 | — | — | — | — | 15 |
| devin | patch | 1 | fail | 0/10 | docker |  | 0 | — | — | — | — | 44 |
| devin | patch | 2 | fail | — | docker |  | 0 | — | — | — | — | 24 |
| devin | patch | 3 | fail | — | docker |  | 0 | — | 1 | — | — | 46 |
| grok | control | 1 | fail | 1/10 | docker |  | 0 | — | — | — | 40.5k | 90 |
| grok | control | 2 | fail | — | docker |  | 0 | — | — | — | 29.4k | 53 |
| grok | control | 3 | fail | — | docker |  | 0 | — | 2 | — | 35.9k | 96 |
| grok | patch | 1 | fail | 1/10 | docker |  | 0 | — | — | — | 50.5k | 49 |
| grok | patch | 2 | fail | — | docker |  | 0 | — | — | — | 21.1k | 65 |
| grok | patch | 3 | fail | — | docker |  | 0 | — | — | — | 18.7k | 37 |

calls/edits are hook-traced for claude, codex; for the rest, edits counts files changed on disk and calls is not measured — shown as —, never as 0. tokens follow each vendor's own accounting (claude's input excludes cache reads): compare within a harness, not across harnesses.
