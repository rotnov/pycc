# Incident: pr-body-heredoc-failure

**Date:** 2026-08-11
**Topic:** pr-body-heredoc-failure
**Verdict:** shipped (manual verify)

## Symptom

The command `gh pr create --title "title" --body "$(cat <<'EOF'...EOF)"`
failed with:
```
bash: -c: line 2: unexpected EOF while looking for matching `''
bash: -c: line 21: syntax error: unexpected end of file
```

The nested heredoc quoting inside `--body "$(cat <<'EOF'...EOF)"` is not
reliably parseable by the shell when executed through a command-call
interface. The PR body contained apostrophes and other characters that
conflicted with the heredoc delimiter quoting.

## Root cause

The system prompt's "Creating pull requests" section prescribes the
`--body "$(cat <<'EOF'...EOF)"` pattern. This pattern is fragile: any
content in the body that conflicts with the shell's quoting context
causes a parse error. The fix is to write the body to a temp file and
use `--body-file <path>`.

## Termination point

`Project rule`: `AGENTS.md`, new "Pull request creation" section.

## Artefact

**Type:** rule (project governance edit)
**File:** `AGENTS.md`
**Change:** Added a "Pull request creation" section instructing agents
to always use `--body-file <path>` and never inline a heredoc in the
`--body` argument.

## Fixture

`.harden/incidents/pr-body-heredoc-failure/fixture/`
- `task.md`: asks the agent to create a PR with gh pr create
- `control.md`: governance without the --body-file rule
- `patch.md`: governance with the --body-file rule
- `verify.py`: checks for --body-file usage and absence of heredoc

## Arena verdict

**Not enough data.** The sandbox lacks `gh` and `git`, so no agent could
actually create a PR. All 23 runs failed verify.py because the task is
unsolvable in the sandbox environment. The fixture tests command
execution, not a behavioural decision — the arena cannot exercise this
class.

## Verify

`verify: manual` — the rule is a command-syntax guideline, not a
behavioural change. The failure is a shell parse error, not an agent
judgment call. The rule's effectiveness is self-evident: `--body-file`
reads from a file and bypasses shell quoting entirely, eliminating the
class of failure. The arena cannot test this because the sandbox lacks
`gh`.
