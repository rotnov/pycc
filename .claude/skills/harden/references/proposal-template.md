# Proposal: <the change, one line>

<!-- Copy this file, fill every section, then:
     gh issue create -R rotnov/harden --title "<the change>" --body-file <this file>
     The contract lives in references/upstream.md. Scrub first: nothing from
     the host project beyond what the failure itself requires. -->

## What happened

<!-- The incident, scrubbed of project specifics: what failed, where the trace
     terminated, and why it is generic — either the fix lands in the skill's
     own files, or the fixture below mentions nothing project-specific. -->

## The check

<!-- The complete arena fixture. A maintainer must be able to save these
     files into a directory and run `uv run scripts/arena.py <dir> --runs 3`
     with nothing else. Include setup.py only if state must be seeded;
     remember: paths in setup.py are __file__-relative, never absolute. -->

### task.md

```text
```

### control.md

```text
```

### patch.md

```text
```

### verify.py

```python
```

## The verdict

<!-- The field run's report: the summary table, the per-harness verdict
     lines, the judge section. Paste report.md from .harden/arena/<run>/. -->

## Environment

<!-- The report's environment block: placement per harness, models,
     container image — the header lines of report.md carry all of it. -->
