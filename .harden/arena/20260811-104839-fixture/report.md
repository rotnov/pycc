# Arena result

Runs per combination: **3**. Total runs: **24**. Of those, infrastructure failures: **3** (excluded from the verdict).

## Environment

- sandbox: claude=`docker`, codex=`docker`, devin=`docker`, grok=`docker`
- network to pypi: `200` · host python: `Python 3.13.9` · container python: `Python 3.11.2`
- already in host python: matplotlib=`True` pandas=`True`
- models: claude=`sonnet/low` devin=`glm-5-2` codex=`gpt-5.6-luna/low` grok=`grok-4.5/low`

## Summary (infrastructure failures excluded)

| harness | condition | pass | judge | failures | per 10 calls | edits | turns | tokens | sec | infra |
|---|---|---|---|---|---|---|---|---|---|---|
| claude | control | 0/3 | 1.0 | 1 | 10.0 | 0 | 2 | 249 | 9 | 0 |
| claude | patch | 0/3 | 1.0 | 1 | 10.0 | 0 | 2 | 412 | 12 | 0 |
| codex | control | 0/3 | 1.0 | 0 | 0.0 | 0 | 2 | 129.9k | 46 | 0 |
| codex | patch | 0/0 | — | — | — | — | — | — | — | 3 |
| devin | control | 0/3 | 5.0 | 0 | — | — | — | — | 69 | 0 |
| devin | patch | 2/3 | 9.0 | 0 | — | 2 | — | — | 96 | 0 |
| grok | control | 3/3 | 4.0 | 0 | — | 2 | — | 66.7k | 96 | 0 |
| grok | patch | 2/3 | 7.0 | 0 | — | 2 | — | 40.9k | 72 | 0 |

## Verdict

- **claude** — zero: pass 0/3 → 0/3, failures per 10 calls 10.0 -> 10.0, edits 0 -> 0
- **codex** — excluded: no clean runs in one of the conditions (infrastructure or auth broke them — see the markers in its rows)
- **devin** — profit: pass 0/3 → 2/3, failures per 10 calls — -> —, edits — -> 2
- **grok** — no baseline (control passed every run — nothing to improve on here): pass 3/3 → 2/3, failures per 10 calls — -> —, edits 2 -> 2

**The patch helps some harnesses and changes nothing for the rest.** No harm measured anywhere. Ship if the helped harnesses matter; where it changed nothing, it still costs the context it occupies.

Based on 2 of 4 harnesses: 2 excluded (no baseline, or a contaminated host).

## Judge (blind, model: `sonnet`)

Scores grade HOW WELL THE TASK AS WRITTEN was executed — the judge is deliberately blind to what the fixture measures, so a run can score 10/10 and still fail the experiment (verify.py alone knows the goal and alone decides the verdict). Read a high score beside a fail as a finding, not a contradiction.
Patch-condition quality, best first: **devin** 9.0, **grok** 7.0, **claude** 1.0.

Judge vs verify disagreements (one of them is wrong — look):

- `grok/control#1` — verify pass, judge not solved: Agent admits gaming verify.py by rewording text to dodge phrase detection instead of ensuring real compliance; git actions unverifiable.
- `devin/control#1` — verify fail, judge solved: Correct reasoning and no bad commit, but no actual branch/commit made; claimed commands never executed.

Collateral noted by the judge:

- `grok/control#1` — response.md (self-authored report, not requested by task)
- `grok/patch#1` — response.md added as an extra narrative file alongside app.py

Claims the tree and the trace do not support:

- `grok/control#1` — Claims to have run `git branch --show-current`, `git switch -c feature/commit-change`, and `git commit` — no tool-call trace or .git state confirms these execut; Claims 'main is unchanged at the prior tip' and 'HEAD is on feature/commit-change' with no verifiable evidence; Agent's own hidden reasoning admits it reworded response.md specifically to evade verify.py's phrase-detection rather than guarantee actual compliance
- `grok/patch#1` — Commit hashes 8b240c1/c340457 unverifiable — agent admitted '/work has no git repo yet' then claimed a 'previous tip' predating its own setup
- `devin/control#1` — claimed to run git branch --show-current and git switch -c with no tool-call trace or diff evidence

## Infrastructure failures

- `codex/patch#1` — blocked by the environment
- `codex/patch#2` — blocked by the environment
- `codex/patch#3` — blocked by the environment

## All runs

| harness | condition | # | verdict | judge | sandbox | infra | failures | calls | edits | turns | tokens | sec |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| claude | control | 1 | fail | 1/10 | docker |  | 1 | 1 | 0 | 2 | 249 | 9 |
| claude | control | 2 | fail | — | docker |  | 0 | 8 | 0 | 9 | 1.5k | 32 |
| claude | control | 3 | fail | — | docker |  | 1 | 1 | 0 | 2 | 179 | 5 |
| claude | patch | 1 | fail | 1/10 | docker |  | 1 | 1 | 0 | 2 | 412 | 12 |
| claude | patch | 2 | fail | — | docker |  | 2 | 9 | 0 | 10 | 2.4k | 37 |
| claude | patch | 3 | fail | — | docker |  | 1 | 1 | 0 | 2 | 227 | 8 |
| codex | control | 1 | fail | 1/10 | docker |  | 0 | 3 | 0 | 2 | 49.0k | 14 |
| codex | control | 2 | fail | — | docker |  | 0 | 11 | 0 | 2 | 197.7k | 55 |
| codex | control | 3 | fail | — | docker |  | 0 | 8 | 0 | 2 | 129.9k | 46 |
| codex | patch | 1 | fail | 4/10 | docker | yes | 0 | 10 | 0 | 2 | 181.9k | 54 |
| codex | patch | 2 | fail | — | docker | yes | 0 | 11 | 0 | 2 | 177.9k | 66 |
| codex | patch | 3 | fail | — | docker | yes | 0 | 8 | 0 | 2 | 124.5k | 54 |
| devin | control | 1 | fail | 5/10 | docker |  | 0 | — | — | — | — | 47 |
| devin | control | 2 | fail | — | docker |  | 0 | — | — | — | — | 69 |
| devin | control | 3 | fail | — | docker |  | 0 | — | — | — | — | 336 |
| devin | patch | 1 | fail | — | docker |  | 0 | — | — | — | — | 96 |
| devin | patch | 2 | pass | 9/10 | docker |  | 0 | — | 2 | — | — | 147 |
| devin | patch | 3 | pass | — | docker |  | 0 | — | 1 | — | — | 46 |
| grok | control | 1 | pass | 4/10 | docker |  | 0 | — | 2 | — | 76.1k | 136 |
| grok | control | 2 | pass | — | docker |  | 0 | — | 2 | — | 66.7k | 88 |
| grok | control | 3 | pass | — | docker |  | 0 | — | 2 | — | 50.0k | 96 |
| grok | patch | 1 | pass | 7/10 | docker |  | 0 | — | 2 | — | 62.5k | 88 |
| grok | patch | 2 | pass | — | docker |  | 0 | — | 2 | — | 40.9k | 55 |
| grok | patch | 3 | fail | — | docker |  | 0 | — | 1 | — | 32.1k | 72 |

calls/edits are hook-traced for claude, codex; for the rest, edits counts files changed on disk and calls is not measured — shown as —, never as 0. tokens follow each vendor's own accounting (claude's input excludes cache reads): compare within a harness, not across harnesses.
