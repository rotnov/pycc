# External-plugin escape hatch: forking upstream locally with attribution

### 4c. External-plugin escape hatch

**Fires ONLY when `failure-tracer`'s backward-trace terminates at an *Upstream-plugin* artefact.** For any local termination point (Local skill / Local agent / AGENTS.md), Step 4b's decision tree (Add / Replace / Consolidate) applies and this step is skipped.

When the trace terminates at an upstream-plugin artefact (a skill from an installed pack recommends a pattern that breaks this project; name pack and skill separately, never collapsed into a `<repo>:<pack>:<skill>` form), the fix **cannot** modify upstream — we don't own the source, and direct upstream modification would either be silently overwritten on the next plugin update or break for every other consumer. Instead, apply the **local fork** pattern:

1. **Copy upstream as baseline**. Clone the upstream skill / agent file (read-only) into the project's `.claude/skills/<name>/` or `.claude/agents/<name>.md`. Match the upstream's frontmatter EXCEPT: (a) rename if the bare name would shadow another local file unintentionally; (b) preserve `model:` / `tools:` / `disallowedTools:` semantics so the local fork has the same tool surface as upstream.
   The mirror-surface rule applies here too: when the repo maintains both `.claude` and `.codex` local runtime surfaces, create or update the matching local fork in both destinations unless the divergence is runtime-specific and explicitly documented.
2. **Add attribution at the top of the local copy.** The placement depends on whether the target carries YAML frontmatter (Claude Code agent / skill files do; loose markdown does not):

   - **Frontmatter-bearing target** (every `.claude/agents/*.md` and `.claude/skills/*/SKILL.md` — the file MUST start with `---` on **line 1** per the README.md frontmatter discipline; a leading HTML comment silently breaks Claude Code's agent-loader registration). Use a `#`-prefixed YAML comment **inside** the frontmatter block:

     ```markdown
     ---
     name: <name>
     description: <…>
     # Adapted from <plugin>/<path> (<license>); upstream commit <sha>. Local divergences below.
     model: <…>
     ---
     ```

     Canonical pattern: see `.claude/agents/python-pro.md` / `.claude/agents/failure-tracer.md` frontmatter (the comment lands as a YAML `#` line between `description:` and `model:`).

   - **Non-frontmatter target** (loose markdown, e.g. a docstring-on-code-surface extraction or an AGENTS.md-style policy fragment). Use an HTML comment at the very top:

     ```markdown
     <!-- Adapted from <plugin>/<path> (<license>); upstream commit <sha>. Local divergences below. -->
     ```

   Cite the license honestly — if upstream is MIT, say MIT; if upstream is internal/unlicensed (used with author permission), say that explicitly. Never imply MIT for an unlicensed source.
3. **Apply the evolution to the local copy**, not the upstream. The `/harden`-formulated rule lands as a local divergence; upstream stays untouched.
4. **Update the project's list of installed packs** if the upstream wasn't already declared (per the project's source-attribution rule). A contributor following AGENTS.md to set up a fresh environment must end up with the upstream pack installed before the local fork's attribution makes sense.
5. **Document the local-vs-upstream divergence** in the incident record: which file forked, why, what diverges semantically (not a verbatim diff — that bloats the log; a one-paragraph summary of the rule change).

Invocation routing between local fork and upstream is **explicit**, not implicit: `Skill(skill="<name>")` and `Agent(subagent_type="<name>")` with a **bare unqualified name** address the local file (`.claude/skills/<name>/SKILL.md` or `.claude/agents/<name>.md`); `Skill(skill="<plugin>:<name>")` / `Agent(subagent_type="<plugin>:<name>")` with the **fully-qualified plugin-namespaced form** addresses the upstream. Use the bare name for project-context invocations where the local divergences are wanted; use the namespaced form on the rare occasion where the original upstream behaviour is needed without project divergences. Do NOT rely on an implicit precedence chain — name the target explicitly each time, since the local-vs-upstream tiebreaker of the runtime's resolver is not something to assert without verifying it.

**Shape of a done fork**: attribution comment inside the frontmatter, a body that opens with a "project-local divergences" section, and the upstream pack declared wherever the project lists its dependencies.

After applying Steps 1-5, proceed to Step 5 (Propose the Update) with the local-fork edits as the proposed diff.
