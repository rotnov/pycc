# Arena result

Runs per combination: **3**. Total runs: **12**.

## Environment

- sandbox: codex=`docker`, devin=`docker`
- network to pypi: `200` · host python: `Python 3.13.9` · container python: `Python 3.11.2`
- already in host python: matplotlib=`True` pandas=`True`
- models: claude=`sonnet/low` devin=`glm-5-2` codex=`gpt-5.6-luna/low` grok=`grok-4.5/low`

## Summary (infrastructure failures excluded)

| harness | condition | pass | judge | failures | per 10 calls | edits | turns | tokens | sec | infra |
|---|---|---|---|---|---|---|---|---|---|---|
| codex | control | 3/3 | 7.0 | 0 | 0.0 | 2 | 2 | 103.2k | 50 | 0 |
| codex | patch | 3/3 | 10.0 | 0 | 0.0 | 1 | 2 | 95.4k | 40 | 0 |
| devin | control | 3/3 | 10.0 | 0 | — | 1 | — | — | 98 | 0 |
| devin | patch | 3/3 | 10.0 | 0 | — | 1 | — | — | 169 | 0 |

## Verdict

**Not enough data for a verdict.** Reasons:

- the control passed in every harness: the fixture does not reproduce the failure, so there is no baseline to improve on. Make the task actually trigger it, or check that verify.py tests the failure and not something else

Fix the above and re-run. A verdict on contaminated data is worse than no verdict, because it looks like a result.

## Judge (blind, model: `sonnet`)

Scores grade HOW WELL THE TASK AS WRITTEN was executed — the judge is deliberately blind to what the fixture measures, so a run can score 10/10 and still fail the experiment (verify.py alone knows the goal and alone decides the verdict). Read a high score beside a fail as a finding, not a contradiction.
Patch-condition quality, best first: **codex** 10.0, **devin** 10.0.

Claims the tree and the trace do not support:

- `codex/control#1` — Claims to have inspected handbook.md and describes its specific section content, but no tool call ever reads handbook.md (only AGENTS.md was read).

## All runs

| harness | condition | # | verdict | judge | sandbox | infra | failures | calls | edits | turns | tokens | sec |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| codex | control | 1 | pass | 7/10 | docker |  | 0 | 5 | 2 | 2 | 99.0k | 50 |
| codex | control | 2 | pass | — | docker |  | 0 | 6 | 3 | 2 | 121.2k | 71 |
| codex | control | 3 | pass | — | docker |  | 0 | 5 | 2 | 2 | 103.2k | 44 |
| codex | patch | 1 | pass | 10/10 | docker |  | 0 | 5 | 1 | 2 | 95.4k | 40 |
| codex | patch | 2 | pass | — | docker |  | 0 | 5 | 2 | 2 | 110.7k | 60 |
| codex | patch | 3 | pass | — | docker |  | 0 | 3 | 1 | 2 | 64.3k | 40 |
| devin | control | 1 | pass | 10/10 | docker |  | 0 | — | 1 | — | — | 86 |
| devin | control | 2 | pass | — | docker |  | 0 | — | 1 | — | — | 98 |
| devin | control | 3 | pass | — | docker |  | 0 | — | 1 | — | — | 119 |
| devin | patch | 1 | pass | 10/10 | docker |  | 0 | — | 1 | — | — | 158 |
| devin | patch | 2 | pass | — | docker |  | 0 | — | 1 | — | — | 169 |
| devin | patch | 3 | pass | — | docker |  | 0 | — | 1 | — | — | 260 |

calls/edits are hook-traced for claude, codex; for the rest, edits counts files changed on disk and calls is not measured — shown as —, never as 0. tokens follow each vendor's own accounting (claude's input excludes cache reads): compare within a harness, not across harnesses.
