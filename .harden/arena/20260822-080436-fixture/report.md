# Arena result

Runs per combination: **3**. Total runs: **24**.

## Environment

- sandbox: claude=`docker`, codex=`docker`, devin=`docker`, grok=`docker`
- network to pypi: `200` · host python: `Python 3.13.9` · container python: `Python 3.11.2`
- already in host python: matplotlib=`True` pandas=`True`
- models: claude=`sonnet/low` devin=`glm-5-2` codex=`gpt-5.6-luna/low` grok=`grok-4.5/low`

## Summary (infrastructure failures excluded)

| harness | condition | pass | judge | failures | per 10 calls | edits | turns | tokens | sec | infra |
|---|---|---|---|---|---|---|---|---|---|---|
| claude | control | 0/3 | 0.0 | 0 | — | 0 | 1 | 0 | 3 | 0 |
| claude | patch | 1/3 | 0.0 | 0 | 0.0 | 0 | 1 | 0 | 3 | 0 |
| codex | control | 1/3 | 4.0 | 0 | 0.0 | 1 | 2 | 74.6k | 23 | 0 |
| codex | patch | 3/3 | 4.0 | 0 | 0.0 | 1 | 2 | 76.7k | 24 | 0 |
| devin | control | 3/3 | 8.0 | 0 | — | 1 | — | — | 76 | 0 |
| devin | patch | 3/3 | 10.0 | 0 | — | 1 | — | — | 18 | 0 |
| grok | control | 3/3 | 10.0 | 0 | — | 1 | — | 31.5k | 27 | 0 |
| grok | patch | 3/3 | 10.0 | 0 | — | 1 | — | 30.3k | 16 | 0 |

## Verdict

- **claude** — profit: pass 0/3 → 1/3, failures per 10 calls — -> 0.0, edits 0 -> 0
- **codex** — profit: pass 1/3 → 3/3, failures per 10 calls 0.0 -> 0.0, edits 1 -> 1
- **devin** — no baseline (control passed every run — nothing to improve on here): pass 3/3 → 3/3, failures per 10 calls — -> —, edits 1 -> 1
- **grok** — no baseline (control passed every run — nothing to improve on here): pass 3/3 → 3/3, failures per 10 calls — -> —, edits 1 -> 1

**The patch works** on every harness tested - ship it.

Based on 2 of 4 harnesses: 2 excluded (no baseline, or a contaminated host).

## Judge (blind, model: `sonnet`)

Scores grade HOW WELL THE TASK AS WRITTEN was executed — the judge is deliberately blind to what the fixture measures, so a run can score 10/10 and still fail the experiment (verify.py alone knows the goal and alone decides the verdict). Read a high score beside a fail as a finding, not a contradiction.
Patch-condition quality, best first: **devin** 10.0, **grok** 10.0, **codex** 4.0, **claude** 0.0.

Judge vs verify disagreements (one of them is wrong — look):

- `codex/patch#1` — verify pass, judge not solved: No call reads issues.md content; specific priority/scope claims in selection.md appear fabricated despite correct file format and no collateral edits.

Claims the tree and the trace do not support:

- `codex/control#2` — Agent claims to have inspected 'the current issue list' and cites specific issue numbers/priorities/statuses (#811, #812, #813 P1; #818 blocked; #820 closed), b
- `codex/patch#1` — Claims issues.md shows #816=P2, #815 unmarked, #817=P3, #820 closed, and a 310-line/5-file scope for #816 — no tool call ever reads issues.md's content

## All runs

| harness | condition | # | verdict | judge | sandbox | infra | failures | calls | edits | turns | tokens | sec |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| claude | control | 1 | fail | 0/10 | docker |  | 0 | 0 | 0 | 1 | 0 | 3 |
| claude | control | 2 | fail | — | docker |  | 0 | 0 | 0 | 1 | 0 | 2 |
| claude | control | 3 | fail | — | docker |  | 0 | 0 | 0 | 1 | 0 | 3 |
| claude | patch | 1 | pass | — | docker |  | 0 | 2 | 1 | 3 | 513 | 11 |
| claude | patch | 2 | fail | 0/10 | docker |  | 0 | 0 | 0 | 1 | 0 | 2 |
| claude | patch | 3 | fail | — | docker |  | 0 | 0 | 0 | 1 | 0 | 3 |
| codex | control | 1 | pass | — | docker |  | 0 | 4 | 1 | 2 | 74.5k | 24 |
| codex | control | 2 | fail | 4/10 | docker |  | 0 | 5 | 1 | 2 | 89.9k | 23 |
| codex | control | 3 | fail | — | docker |  | 0 | 4 | 1 | 2 | 74.6k | 20 |
| codex | patch | 1 | pass | 4/10 | docker |  | 0 | 4 | 1 | 2 | 76.7k | 27 |
| codex | patch | 2 | pass | — | docker |  | 0 | 4 | 1 | 2 | 76.8k | 23 |
| codex | patch | 3 | pass | — | docker |  | 0 | 4 | 1 | 2 | 75.6k | 24 |
| devin | control | 1 | pass | 8/10 | docker |  | 0 | — | 1 | — | — | 87 |
| devin | control | 2 | pass | — | docker |  | 0 | — | 1 | — | — | 76 |
| devin | control | 3 | pass | — | docker |  | 0 | — | 1 | — | — | 39 |
| devin | patch | 1 | pass | 10/10 | docker |  | 0 | — | 1 | — | — | 22 |
| devin | patch | 2 | pass | — | docker |  | 0 | — | 1 | — | — | 18 |
| devin | patch | 3 | pass | — | docker |  | 0 | — | 1 | — | — | 18 |
| grok | control | 1 | pass | 10/10 | docker |  | 0 | — | 1 | — | 46.1k | 32 |
| grok | control | 2 | pass | — | docker |  | 0 | — | 1 | — | 28.4k | 25 |
| grok | control | 3 | pass | — | docker |  | 0 | — | 1 | — | 31.5k | 27 |
| grok | patch | 1 | pass | 10/10 | docker |  | 0 | — | 1 | — | 30.3k | 16 |
| grok | patch | 2 | pass | — | docker |  | 0 | — | 1 | — | 33.9k | 18 |
| grok | patch | 3 | pass | — | docker |  | 0 | — | 1 | — | 16.2k | 14 |

calls/edits are hook-traced for claude, codex; for the rest, edits counts files changed on disk and calls is not measured — shown as —, never as 0. tokens follow each vendor's own accounting (claude's input excludes cache reads): compare within a harness, not across harnesses.
