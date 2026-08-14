# Taking a maintained skill instead of writing a guard

The rung for "someone has already solved this class". Writing your own version of
something already debugged by a thousand installs is the worst of the available
options — but installing one is running another party's instructions with your
permissions, so the steps below are not optional decoration.

Four steps, and each can end the process: **find → audit → prove → install.**

## 1. Find

```bash
uv run scripts/discover.py "<the failure class>" "<another phrasing>" "<a third>"
```

Several phrasings on purpose. A skill matching one of them shares a keyword; a
skill matching three covers the *shape* of the problem. The ranking puts match
count above install count for exactly that reason.

`npx skills find "<query>"` is the catalogue's own search and takes one phrasing
at a time. Either is fine as a source of candidates.

**Install counts are popularity, never safety.** They measure how many people
clicked, not what the content does. Nothing about a rank licenses skipping step 2.

Nothing found is a result: write the artefact yourself, one rung down.

## 2. Audit

Dispatch the auditor on the candidate before reading it into your own context:

```
Agent(subagent_type="general-purpose", prompt="Read
references/skill-audit.md in full as your role definition, then audit
<owner/repo@skill>. Observe and report only.")
```

A subagent rather than inline work, for a reason that applies here more than
anywhere: the candidate's contents are untrusted text, and reading them in the
main context *is* the exposure being audited.

**RED ends it.** YELLOW is a decision for the user, stated as what they would be
accepting. Do not soften a verdict because the source looks reputable — the
auditor's own rules say owner reputation is not evidence about content.

## 3. Prove it in the arena — install it there, not in the project

An audited skill is safe to run, not proven to help. Nothing third-party enters
the project before that is measured: **the arena gets it first, the project only
after a verdict.**

The fixture is the ordinary shape, with one difference that matters:

- `control.md` — the governance file without the skill;
- `patch.md` — identical, plus the routing line pointing at it;
- `verify.py` — the failure this class produces, checked for the workaround as
  well as the happy path;
- `setup.py` — installs the skill **into the patch copy only**.

That last point is not a detail. A skill is discovered by its description, so a
copy present in both conditions is reachable in both, and the control stops being
a baseline — the arena would then compare the skill against itself and report
`zero`. `setup.py` receives `ARENA_CONDITION`, which is what makes the split
possible:

```python
if os.environ.get("ARENA_CONDITION") == "patch":
    subprocess.run(["npx", "skills", "add", "<owner/repo@skill>",
                    "-a", "claude-code", "-y"], check=True)
```

`zero` means the skill is maintained, popular, safe **and useless against this
failure** — common enough to be worth the runs. Do not install on a `zero`: the
routing line would cost context in every session for nothing, and the skill's own
body would cost more.

## 4. Install into the project

```bash
npx skills add <owner/repo@skill> -a claude-code -a devin -a codex -a grok
```

The catalogue CLI produces the layout this project settled on independently
(D-013): one real copy in `.agents/skills/<name>/`, which devin, grok, codex and
opencode read directly, and a relative symlink for Claude Code, which does not
look in `.agents/` at all. Verified — it prints `symlinked: Claude Code`.

**Name every harness you use, even the ones you are not testing from.** With a
single `-a` it writes a plain copy into that harness's own directory instead; the
shared layout appears only when more than one is asked for.

It also writes `skills-lock.json`, so the version is recorded rather than implied.

Without `npx`, copy the skill into `.agents/skills/<name>/` and create the same
relative symlink by hand. The layout matters more than the tool that makes it.

Then the routing line, and only the routing line: name the skill and when to
reach for it. Its mechanics are its own documentation — copying them into a
governance file makes a second copy that drifts.

## Recording it

The incident's `artifact` is the skill's identifier **and version**, not just its
name — a catalogue entry changes under you. `verify: arena` with the run, since
step 3 produced one. The audit verdict belongs in the entry too: a future reader
deciding whether to update it needs to know what was accepted, not only that
something was.
