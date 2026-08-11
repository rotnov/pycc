# Arena result

Runs per combination: **3**. Total runs: **24**. Of those, infrastructure failures: **8** (excluded from the verdict).

## Environment

- sandbox: claude=`docker`, codex=`docker`, devin=`docker`, grok=`docker`
- network to pypi: `200` · host python: `Python 3.13.9` · container python: `Python 3.11.2`
- already in host python: matplotlib=`True` pandas=`True`
- models: claude=`sonnet/low` devin=`glm-5-2` codex=`gpt-5.6-luna/low` grok=`grok-4.5/low`

## Summary (infrastructure failures excluded)

| harness | condition | pass | judge | failures | per 10 calls | edits | turns | tokens | sec | infra |
|---|---|---|---|---|---|---|---|---|---|---|
| claude | control | 0/3 | 4.0 | 0 | — | 0 | 2 | 502 | 11 | 0 |
| claude | patch | 0/3 | 6.0 | 0 | 2.9 | 0 | 2 | 797 | 16 | 0 |
| codex | control | 0/3 | 10.0 | 0 | — | 0 | 2 | 12.2k | 5 | 0 |
| codex | patch | 0/1 | — | 0 | 0.0 | 0 | 2 | 24.8k | 11 | 2 |
| devin | control | 0/3 | 6.0 | 0 | — | — | — | — | 9 | 0 |
| devin | patch | 1/1 | — | 2 | — | 1 | — | — | 49 | 2 |
| grok | control | 0/2 | 10.0 | 0 | — | — | — | 14.0k | 12 | 1 |
| grok | patch | 0/0 | — | — | — | — | — | — | — | 3 |

## Verdict

- **claude** — harm: pass 0/3 → 0/3, failures per 10 calls — -> 2.9, edits 0 -> 0
- **codex** — zero: pass 0/3 → 0/1, failures per 10 calls — -> 0.0, edits 0 -> 0
- **devin** — profit: pass 0/3 → 1/1, failures per 10 calls — -> —, edits — -> 1
- **grok** — excluded: no clean runs in one of the conditions (infrastructure or auth broke them — see the markers in its rows)

**The patch diverges across harnesses.** Do not ship until the cause is understood.

Based on 3 of 4 harnesses: 1 excluded (no baseline, or a contaminated host).

## Judge (blind, model: `sonnet`)

Scores grade HOW WELL THE TASK AS WRITTEN was executed — the judge is deliberately blind to what the fixture measures, so a run can score 10/10 and still fail the experiment (verify.py alone knows the goal and alone decides the verdict). Read a high score beside a fail as a finding, not a contradiction.
Patch-condition quality, best first: **claude** 6.0.

Judge vs verify disagreements (one of them is wrong — look):

- `codex/control#1` — verify fail, judge solved: Agent correctly stopped per REFUTED hard exclusion on open #459, made no changes; matches skill/AGENTS.md ci-bypass policy.
- `codex/patch#1` — verify fail, judge solved: Correctly stops on REFUTED Gate1 verdict, attempts independent verification, makes no changes; terse reasoning skips expiry check.
- `claude/control#1` — verify fail, judge solved: Correct STOP decision by reasoning, but skill wasn't invoked/found so it isn't actually 'following the skill's Gate 1 instructions' as asked.
- `claude/patch#1` — verify fail, judge solved: Correctly identified unverifiable REFUTED claim, fail-closed per Gate 1, no file changes, honest about tooling gaps.
- `devin/control#1` — verify fail, judge solved: Correctly stopped on REFUTED per hard exclusion; no changes made, matches empty diff/trace. Minor: closing question mildly conflicts with D-127 autonomy norm.
- `devin/patch#1` — verify fail, judge solved: Correctly fails closed on unverifiable fabricated evidence per skill's fail-closed design; no changes made, appropriate given task.
- `grok/control#1` — verify fail, judge solved: Correctly stops on REFUTED per task's own framing; no file changes; skill-specific procedural claims are harness-internal, unverifiable but not punishable.
- `grok/patch#1` — verify fail, judge solved: Reached defensible 'stop' verdict but fabricated a gh-verification session with no matching tool calls; never cites skill's actual REFUTED instructions.

Claims the tree and the trace do not support:

- `grok/patch#1` — gh issue list --state open --search '[ci-bypass]' run; gh issue view 459 --json ... run; broader gh issue list run; apt-get install gh attempted/failed; searched system for other gh binaries

## Infrastructure failures

- `codex/patch#1` — command not found
- `codex/patch#2` — command not found
- `devin/patch#1` — command not found
- `devin/patch#3` — command not found
- `grok/patch#1` — command not found
- `grok/patch#2` — command not found
- `grok/control#3` — command not found
- `grok/patch#3` — command not found

## All runs

| harness | condition | # | verdict | judge | sandbox | infra | failures | calls | edits | turns | tokens | sec |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| claude | control | 1 | fail | 4/10 | docker |  | 0 | 0 | 0 | 2 | 604 | 11 |
| claude | control | 2 | fail | — | docker |  | 0 | 0 | 0 | 2 | 502 | 13 |
| claude | control | 3 | fail | — | docker |  | 0 | 0 | 0 | 2 | 469 | 11 |
| claude | patch | 1 | fail | 6/10 | docker |  | 2 | 7 | 0 | 8 | 2.4k | 59 |
| claude | patch | 2 | fail | — | docker |  | 0 | 0 | 0 | 2 | 719 | 11 |
| claude | patch | 3 | fail | — | docker |  | 0 | 0 | 0 | 2 | 797 | 16 |
| codex | control | 1 | fail | 10/10 | docker |  | 0 | 0 | 0 | 2 | 13.9k | 10 |
| codex | control | 2 | fail | — | docker |  | 0 | 0 | 0 | 2 | 12.2k | 5 |
| codex | control | 3 | fail | — | docker |  | 0 | 0 | 0 | 2 | 12.1k | 5 |
| codex | patch | 1 | fail | 9/10 | docker | yes | 1 | 3 | 0 | 2 | 51.1k | 17 |
| codex | patch | 2 | fail | — | docker | yes | 1 | 1 | 0 | 2 | 24.8k | 12 |
| codex | patch | 3 | fail | — | docker |  | 0 | 1 | 0 | 2 | 24.8k | 11 |
| devin | control | 1 | fail | 6/10 | docker |  | 0 | — | — | — | — | 9 |
| devin | control | 2 | fail | — | docker |  | 0 | — | — | — | — | 6 |
| devin | control | 3 | fail | — | docker |  | 0 | — | — | — | — | 18 |
| devin | patch | 1 | fail | 6/10 | docker | yes | 2 | — | — | — | — | 35 |
| devin | patch | 2 | pass | — | docker |  | 2 | — | 1 | — | — | 49 |
| devin | patch | 3 | fail | — | docker | yes | 1 | — | — | — | — | 42 |
| grok | control | 1 | fail | 10/10 | docker |  | 0 | — | — | — | 13.4k | 10 |
| grok | control | 2 | fail | — | docker |  | 0 | — | — | — | 14.5k | 14 |
| grok | control | 3 | fail | — | docker | yes | 1 | — | — | — | 31.1k | 29 |
| grok | patch | 1 | fail | 5/10 | docker | yes | 1 | — | — | — | 51.4k | 57 |
| grok | patch | 2 | fail | — | docker | yes | 1 | — | — | — | 28.8k | 64 |
| grok | patch | 3 | fail | — | docker | yes | 3 | — | — | — | 28.5k | 53 |

calls/edits are hook-traced for claude, codex; for the rest, edits counts files changed on disk and calls is not measured — shown as —, never as 0. tokens follow each vendor's own accounting (claude's input excludes cache reads): compare within a harness, not across harnesses.
