---
id: D-190
title: "Map Claude's Agent tool to Codex's spawn_agent/wait_agent in dual-surface skills, with a mandatory unavailability fallback"
status: accepted
---

## D-190: Map Claude's Agent tool to Codex's spawn_agent/wait_agent in dual-surface skills, with a mandatory unavailability fallback
- Status: accepted
- Context: [D-019](D-019-codex-and-claude-code-are-equal-agent-surfaces.md) makes
  Codex and Claude Code equal agent surfaces, and `AGENTS.md` requires a platform
  gap to be closed with the equivalent capability or a safe documented fallback.
  Two repository-owned skills dispatch sub-agents on their *primary* path, not as
  an aside: `improve-codebase-architecture` step 1 calls for the Agent tool with
  `subagent_type=Explore`, and the `codebase-design` skill's design-it-twice step
  calls for three or more sub-agents in parallel. Their Codex adapters under
  `.agents/skills/` said nothing about either, which is worse than an omission —
  Codex's own operating guidance instructs an agent not to spawn sub-agents
  "unless the user or applicable AGENTS.md/skill instructions explicitly ask for
  sub-agents, delegation, or parallel agent work", and states that requests for
  depth, thoroughness, or detailed analysis do not themselves count as
  permission. A silent adapter therefore left the guardrail permanently engaged
  and the canonical step quietly unexecutable on one of the two surfaces. Part of
  [#53](https://github.com/rotnov/pycc/issues/53).
- Decision: treat Codex's `spawn_agent` (start a sub-agent) and `wait_agent` (join
  it) as the sanctioned mapping for Claude's `Agent` tool, and gate the
  requirement on an opt-in marker.
  1. A canonical skill that dispatches sub-agents on its primary path declares
     `requires-agent-dispatch: true` in its frontmatter. The marker sits in the
     file an author is already editing, unlike a module-level constant in the
     validator.
  2. `validate_skill_parity` in `scripts/validate_agent_assets.py` then requires
     the skill's Codex adapter to contain four exact literals: the phrase that
     grants Codex the permission its guardrail demands, `` `spawn_agent` ``,
     `` `wait_agent` ``, and the opening of the fallback paragraph. This mirrors
     the existing `disable-model-invocation` branch, and is keyed on the
     *canonical* skill's frontmatter, never the adapter's.
  3. The adapter must impose read-only discipline in the spawn brief's own text.
     Codex has no `subagent_type`, so Claude's read-only-by-tool-list `Explore`
     has no structural counterpart; the discipline can only be carried by the
     brief. This matters because `improve-codebase-architecture` deliberately
     keeps its report out of the repository, and a write-capable sub-agent would
     violate that.
  4. A named unavailability fallback is mandatory: the adapter must state what to
     do when the multi-agent feature is disabled, the agent depth limit is
     reached, or `spawn_agent` is not offered — run the same work inline and say
     so in the output — and must never silently skip the step.
- Alternatives:
  - *A module-level constant set in the validator* (the `ALPHA_EVAL_RUNNERS` /
    `PROJECT_ALPHA_SKILLS` pattern). Equally precedented and marginally simpler,
    but it lives far from the skill, so an author adding a new dispatch step never
    encounters it. The frontmatter marker is in the file being edited.
  - *An unscoped rule keyed on a regex such as `Agent tool|subagent_type`.*
    Rejected on measurement: 7 of the 30 canonical skills mention Agent-tool
    dispatch, and a regex cannot separate *required on the primary path* from
    *merely described* — `autopilot-async-monitoring` is about waiting on
    dispatches, and `issue-implement`/`issue-to-plan` cite dispatch as a
    D-142/D-143 token-conservation mechanism. The rule must be opt-in or it fires
    on five skills that need no mapping.
  - *A degradation-only fallback* ("Codex cannot do this; do it inline"). Rejected
    because the capability is real: the Codex CLI reports `multi_agent` as
    `stable` and enabled, and exposes `spawn_agent`/`wait_agent` among its tools.
    Documenting a gap that does not exist would leave Codex's own "do not spawn
    unless explicitly asked" guardrail engaged forever.
  - *Superseding D-019.* Not applicable — this entry is an interpretation of
    D-019's parity requirement for one capability class and leaves it standing.
- Consequences:
  - Both surfaces can execute the sub-agent steps of `improve-codebase-architecture`
    and `codebase-design`, and a future dual-surface skill has a named pattern to
    copy instead of re-deriving one.
  - The mapping is **derived, not observed**: it rests on the Codex CLI's
    advertised feature list and tool names, not on an authenticated Codex session
    that watched `spawn_agent` be offered inside the adapter. The behavioral smoke
    that would upgrade it to observed is out of reach of
    `scripts/run_alpha_skill_evals.py`, which is offline and deterministic and
    cannot launch a client or a language model; #53 stays open tracking exactly
    that. The mandatory fallback is what makes the adapters correct in the
    meantime, whichever way the observation lands.
  - The version coverage is narrower than the capability claim. `multi_agent` was
    read from Codex CLI **0.148.0**, the release installed locally, while required
    CI pins `CODEX_CLI_VERSION: "0.145.0"`. The surrounding surface is churning —
    `multi_agent_v2` is `stable` but off, `enable_fanout` is already `removed` — so
    the adapters name only the tools and never assume a concurrency limit, and the
    fallback carries any version where the feature is absent.
  - The validator enforces literal strings, so rewording an adapter's mapping prose
    breaks the gate. That is the intended tradeoff: the wording carries the
    permission Codex's guardrail checks for, so it is contract text, not prose.
  - Adding `requires-agent-dispatch` to a canonical skill without extending its
    adapter is now a hard `agent-assets` failure, which is the point — the marker
    is what keeps the canonical step and the adapter mapping from drifting apart.
