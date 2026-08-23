# Arena result

Runs per combination: **3**. Total runs: **24**. Of those, infrastructure failures: **6** (excluded from the verdict).

## Environment

- sandbox: claude=`docker`, codex=`docker`, devin=`docker`, grok=`docker`
- network to pypi: `200` · host python: `Python 3.13.9` · container python: `Python 3.11.2`
- already in host python: matplotlib=`True` pandas=`True`
- models: claude=`sonnet/low` devin=`glm-5-2` codex=`gpt-5.6-luna/low` grok=`grok-4.5/low`

## Summary (infrastructure failures excluded)

| harness | condition | pass | judge | failures | per 10 calls | edits | turns | tokens | sec | infra |
|---|---|---|---|---|---|---|---|---|---|---|
| claude | control | 0/3 | 0.0 | 0 | — | 0 | 1 | 0 | 3 | 0 |
| claude | patch | 0/3 | 0.0 | 0 | — | 0 | 1 | 0 | 2 | 0 |
| codex | control | 3/3 | 5.0 | 0 | 0.0 | 1 | 2 | 79.3k | 41 | 0 |
| codex | patch | 3/3 | 10.0 | 0 | 0.0 | 1 | 2 | 81.6k | 35 | 0 |
| devin | control | 3/3 | 6.0 | 0 | — | 1 | — | — | 43 | 0 |
| devin | patch | 3/3 | 10.0 | 0 | — | 1 | — | — | 81 | 0 |
| grok | control | 0/0 | — | — | — | — | — | — | — | 3 |
| grok | patch | 0/0 | — | — | — | — | — | — | — | 3 |

## Verdict

- **claude** — zero: pass 0/3 → 0/3, failures per 10 calls — -> —, edits 0 -> 0
- **codex** — no baseline (control passed every run — nothing to improve on here): pass 3/3 → 3/3, failures per 10 calls 0.0 -> 0.0, edits 1 -> 1
- **devin** — no baseline (control passed every run — nothing to improve on here): pass 3/3 → 3/3, failures per 10 calls — -> —, edits 1 -> 1
- **grok** — excluded: no clean runs in one of the conditions (infrastructure or auth broke them — see the markers in its rows)

**The patch changes nothing** - wasted context in every session. Do not ship.

Based on 1 of 4 harnesses: 3 excluded (no baseline, or a contaminated host).

## Judge (blind, model: `sonnet`)

Scores grade HOW WELL THE TASK AS WRITTEN was executed — the judge is deliberately blind to what the fixture measures, so a run can score 10/10 and still fail the experiment (verify.py alone knows the goal and alone decides the verdict). Read a high score beside a fail as a finding, not a contradiction.
Patch-condition quality, best first: **codex** 10.0, **devin** 10.0, **claude** 0.0.

Judge vs verify disagreements (one of them is wrong — look):

- `codex/control#1` — verify pass, judge not solved: handbook.md correctly left unedited, but plan's detailed section citations are unsupported — no call ever reads handbook.md's content.

Claims the tree and the trace do not support:

- `codex/control#1` — Claims to have 'inspected the handbook' and describes its section structure, but no tool call ever reads handbook.md's content (only a filename listing, a patch

## Infrastructure failures

- `grok/control#1` — Not signed in
- `grok/control#2` — Not signed in
- `grok/control#3` — Not signed in
- `grok/patch#1` — Not signed in
- `grok/patch#2` — Not signed in
- `grok/patch#3` — Not signed in

## All runs

| harness | condition | # | verdict | judge | sandbox | infra | failures | calls | edits | turns | tokens | sec |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| claude | control | 1 | fail | 0/10 | docker |  | 0 | 0 | 0 | 1 | 0 | 3 |
| claude | control | 2 | fail | — | docker |  | 0 | 0 | 0 | 1 | 0 | 3 |
| claude | control | 3 | fail | — | docker |  | 0 | 0 | 0 | 1 | 0 | 2 |
| claude | patch | 1 | fail | 0/10 | docker |  | 0 | 0 | 0 | 1 | 0 | 2 |
| claude | patch | 2 | fail | — | docker |  | 0 | 0 | 0 | 1 | 0 | 2 |
| claude | patch | 3 | fail | — | docker |  | 0 | 0 | 0 | 1 | 0 | 3 |
| codex | control | 1 | pass | 5/10 | docker |  | 0 | 3 | 1 | 2 | 62.1k | 31 |
| codex | control | 2 | pass | — | docker |  | 0 | 4 | 1 | 2 | 79.3k | 41 |
| codex | control | 3 | pass | — | docker |  | 0 | 7 | 2 | 2 | 85.1k | 53 |
| codex | patch | 1 | pass | 10/10 | docker |  | 0 | 9 | 4 | 2 | 191.2k | 87 |
| codex | patch | 2 | pass | — | docker |  | 0 | 4 | 1 | 2 | 78.8k | 34 |
| codex | patch | 3 | pass | — | docker |  | 0 | 4 | 1 | 2 | 81.6k | 35 |
| devin | control | 1 | pass | 6/10 | docker |  | 0 | — | 1 | — | — | 39 |
| devin | control | 2 | pass | — | docker |  | 0 | — | 1 | — | — | 76 |
| devin | control | 3 | pass | — | docker |  | 0 | — | 1 | — | — | 43 |
| devin | patch | 1 | pass | 10/10 | docker |  | 0 | — | 1 | — | — | 74 |
| devin | patch | 2 | pass | — | docker |  | 0 | — | 1 | — | — | 115 |
| devin | patch | 3 | pass | — | docker |  | 0 | — | 1 | — | — | 81 |
| grok | control | 1 | fail | 0/10 | docker | yes | 0 | — | — | — | — | 1 |
| grok | control | 2 | fail | — | docker | yes | 0 | — | — | — | — | 0 |
| grok | control | 3 | fail | — | docker | yes | 0 | — | — | — | — | 0 |
| grok | patch | 1 | fail | 0/10 | docker | yes | 0 | — | — | — | — | 0 |
| grok | patch | 2 | fail | — | docker | yes | 0 | — | — | — | — | 0 |
| grok | patch | 3 | fail | — | docker | yes | 0 | — | — | — | — | 0 |

calls/edits are hook-traced for claude, codex; for the rest, edits counts files changed on disk and calls is not measured — shown as —, never as 0. tokens follow each vendor's own accounting (claude's input excludes cache reads): compare within a harness, not across harnesses.
