# Repository Instructions

## Project navigation

- Start with `docs/SPEC.md`. It is the index for the project's specifications and points to the document that owns each area.
- Treat normative specifications under `docs/` as part of the implementation contract, not as an after-the-fact description. The D-066 journals `docs/AGENT_RETROSPECTIVE.md` and `docs/sessions/` are the explicit informational exceptions: review them for factual accuracy, but do not treat their historical lessons or snapshots as implementation requirements. Promote any rule discovered there into `AGENTS.md`, `docs/decisions/`, or the owning specification before relying on it as policy.
- Before changing behavior, architecture, public APIs, the CLI, diagnostics, build or release processes, tests, or supported language semantics, read the relevant specification.

## Autonomous agent operation ([D-127](docs/decisions/D-127-autonomous-agent-operation-model.md))

- This is an experimental project whose explicit purpose is to be developed and advanced end-to-end by autonomous coding agents. Agents own the full delivery lifecycle for a task — planning, implementation, testing, documentation, review, and merge — without waiting on the repository owner.
- Do not pause a task to ask the repository owner a clarifying question, request approval, or ask them to pick between alternatives. When a task reaches a genuine fork in judgment (a design tradeoff, a policy interpretation, a risk call, whether a finding is real), resolve it yourself by consulting a stronger independent reviewer (this session's own advisor/second-opinion tool, or an independent review agent) rather than soliciting the repository owner. Record the reasoning and the resolution in the relevant ADR, plan, or session log exactly as you would any other decision.
- The repository owner does not answer prompts as part of normal operation. They may still intervene at any time on their own initiative — reading history, editing files directly, redirecting a task, overriding a decision, or answering if they choose to — and any such intervention always takes precedence over this section and over an agent's own judgment.
- This does not relax any other rule in this document or in an agent's own safety boundaries. Actions that require a human administrator elsewhere in this file (the [D-024](docs/decisions/D-024-protected-main-and-audited-emergency-bypass.md) emergency path, credential and account actions, and any other action a session's own safety rules reserve for the user) still require the repository owner to act, and an agent still asks before taking them — this section only removes the expectation of asking before ordinary engineering judgment calls.
- To conserve tokens across a long autonomous run, isolate a task's heavy work (plan authoring, implementation, iterative fix loops) inside dispatched `Agent` calls rather than doing it directly in this session's own context (see [D-142](docs/decisions/D-142-issue-implement-s-step-4-implementation-runs-in-a.md) for implementation/fix-loop dispatch and [D-143](docs/decisions/D-143-issue-implement-s-delegated-issue-to-plan.md) for its extension to `issue-to-plan`'s own plan-authoring work) — this is the mechanism actually available for bounding context growth at task checkpoints. An agent cannot trigger compaction on demand at a checkpoint of its own choosing; `autoCompactWindow` (`.claude/settings.json`, currently `250000`) only lowers the automatic threshold, it doesn't add an on-demand trigger. A genuinely fresh session per checkpoint would reset context fully, but starting one requires the repository owner or an external process to initiate it — an agent cannot restart itself into a clean session.

## Before starting a new task ([D-021](docs/decisions/D-021-agent-task-preflight-and-documentation-refresh.md))

1. Inspect `git status --short --branch` and record the current commit before any repository mutation. Preserve all existing user changes.
2. Fetch and prune remote refs without changing checked-out files, then resolve the remote's default branch dynamically.
3. Start every new task from the exact latest commit of that remote default branch in a clean task branch or isolated worktree.
4. When continuing an existing task branch, compare it with the refreshed default branch and integrate only when the task authorizes that mutation and existing work can be preserved. Prefer a fast-forward update; never merge, rebase, reset, switch branches, or pull over uncommitted or user-owned changes.
5. Confirm which commit is the actual task base, then run `cargo doc --workspace --no-deps` before making implementation changes. This refreshes the local Rust API documentation in `target/doc/` from that exact source revision.
6. Read `docs/SPEC.md`, the relevant linked specifications, and the generated documentation for the crates affected by the task.
7. If documentation generation fails, record and understand the failure before changing code. Do not present documentation generated from an older revision as current.
8. When the task starts or continues work on a versioned milestone (e.g. "implement v0.3"), also list the open issues assigned to that milestone (`gh issue list --repo <owner>/<repo> --milestone <name>`) and treat every one of them as in-scope alongside `docs/ROADMAP.md`'s own prose for that milestone. A milestone exists specifically to hold blockers discovered after the roadmap prose was last written; skipping this check silently drops them.
9. Unassigned issues are triaged into milestones dynamically, not only at this preflight: `.claude/skills/issue-select/SKILL.md` walks the complete open-issue list on every autopilot iteration and assigns milestone membership there as a matter of course (step 2 of that skill). The filer is the next-best point — judge and assign the milestone at issue-creation time unless the issue is clearly cross-cutting (CI governance, website/SEO, agent-tooling infra); in the rare case it genuinely is not `vX.Y`-scoped work, explicitly record "no milestone — cross-cutting" reasoning in the issue body so the omission is deliberate rather than accidental. Treat this preflight step as the backstop for work that reaches an issue outside either path: if an in-scope issue is still unassigned when a task starts, assign it now per [D-127](docs/decisions/D-127-autonomous-agent-operation-model.md) judgment rather than deferring; leave genuinely unclear cases unassigned rather than guessing.
10. Before writing implementation code for a multi-step or architecturally consequential task, confirm a written plan exists or produce one — this applies regardless of the task's source, not only issues opened by an external reporter. `.claude/skills/issue-to-plan/SKILL.md` is the one planning gate this project maintains, so route roadmap/milestone-decomposition work (a `docs/DELIVERY_PLAN.md` PR-N task, a `docs/decisions/` ADR follow-up, a CI-gate fix round) through it too instead of treating it as a separate, unplanned path: if no GitHub issue yet represents the item, open one first — in the item's milestone, body summarizing the `docs/DELIVERY_PLAN.md`/roadmap section it comes from and why — then run `issue-to-plan` on that issue exactly as for any other. `docs/DELIVERY_PLAN.md` itself stays at its existing milestone/PR-level granularity (one short paragraph per PR-N, as today) and is never rewritten to hold per-issue detail — it is only the source material issue-to-plan verifies against. Full implementation-level detail already has a home: a dated file under `docs/superpowers/plans/` (one per PR-N, the existing `superpowers:writing-plans` convention). When such a file already exists for the item, opening the issue and linking that file in its body satisfies this step without re-deriving the plan; when it doesn't yet exist and the item is large enough to need one, produce it there first via `superpowers:writing-plans`, then link it the same way — issue-to-plan's own comment either points at that file or, for a smaller item with no separate plan file, carries the plan directly. A small, single-file, mechanically-scoped fix (a docs correction, a one-line CI-permission fix) does not need this step — use judgment on "multi-step or architecturally consequential" rather than treating every change as requiring a formal plan. **Decompose large issues before implementing them.** An issue whose completion criteria span multiple independent code seams (e.g. one sub-task changes a core data structure, another applies the new structure to a specific control-flow construct, a third extends it to a separate pass) should be split into a dependency-ordered sequence of smaller issues, each independently mergeable and covering a subset of the original's completion criteria. Open the sub-issues in the same milestone, link them to the parent (e.g. "Part 1 of #N"), and implement them in dependency order — the parent issue stays open until all sub-issues close. The bar for "large enough to decompose" is not line count but architectural seam count: a 500-line change inside one function is one PR; a change that touches three independent subsystems with three independent test surfaces is three PRs even if each is small.

## Keep documentation current

- Write every durable artifact in English: documentation and specifications under `docs/`, decision entries, `AGENTS.md` and its imports, code comments and identifiers, tests, skills, commit messages, pull-request titles and bodies, and anything published to the issue tracker. Converse with the user in the user's own language, matching whatever language they write in; that choice never changes the language of anything written into the repository or published upstream. Translate the user's intent into English artifacts rather than mirroring their language into the tree.
- Documentation work is part of every implementation task. Update all affected documentation in the same change and commit as the code; a change is incomplete while its docs describe the old behavior.
- Keep descriptions honest about what exists now versus what is planned. Update examples, commands, status markers, acceptance criteria, and cross-references when their underlying behavior changes.
- Keep `docs/ROADMAP.md` current in the same pull request whenever behavior, platform support, milestone acceptance evidence, or delivery sequencing changes. Its current-status section describes the repository tree in the commit that contains it: count behavior and evidence added by that same commit, but never count work that exists only in another open pull request or unmerged branch.
- Mark a roadmap acceptance item `[x]` only with an inline
  `roadmap-evidence` identifier recognized by
  `scripts/check_roadmap_evidence.rb`. Add a failing public-CLI mutation test
  before teaching the checker a new identifier, and run both
  `ruby scripts/test_check_roadmap_evidence.rb` and
  `ruby scripts/check_roadmap_evidence.rb`.
- When milestone decomposition, dependencies, or execution order changes, update `docs/DELIVERY_PLAN.md` together with the roadmap.
- When adding, removing, renaming, or changing the purpose of a specification document under `docs/`, update `docs/SPEC.md` so it remains the reliable specification map.
- Record irreversible or project-wide design choices as a new file under `docs/decisions/` (see `docs/decisions/TEMPLATE.md`), then regenerate `docs/decisions/README.md` with `scripts/generate_decisions_index.py`. Do not silently rewrite an accepted decision; add a new decision that supersedes it.
- Every normative documentation claim should be enforceable where practical by a test, benchmark, or CI check, following the lifecycle rules in `docs/SPEC.md`.
- If a code change genuinely has no documentation impact, explicitly verify that conclusion rather than skipping the docs review by default.

## Generated documentation

- Markdown specifications and architectural explanations are human- and agent-authored source documents. Do not auto-generate or overwrite them from code.
- Rust API documentation is generated from source comments. After changing a public Rust API or its docs, run `cargo doc --workspace --no-deps` to verify it; do not commit `target/doc/`.
- If the repository later adds a checked-in documentation generator, keep its output deterministic, document one canonical regeneration command, and add a CI `--check`-style freshness gate.
- When a checked-in generated document exists, edit its source or generator, regenerate it, and commit the source and generated output together. Never patch generated output by hand.

## Support Codex and Claude Code

- Project development workflows target both OpenAI Codex and Claude Code. `AGENTS.md` is the shared canonical instruction source; keep `CLAUDE.md` as its import instead of duplicating the rules.
- Every new or changed repository-owned agent or skill must be discoverable and usable on both platforms. Add or update the Codex and Claude Code entrypoints, manifests, adapters, commands, and hooks together in the same pull request.
- Keep the behavior contract, safety gates, inputs, and outputs equivalent across the two platforms. Share the underlying implementation where practical; when platform APIs differ, keep the adapters thin and document the mapping.
- Test discovery and the primary success and failure paths on both platforms before merge. If a required capability is unavailable on one platform, provide a safe documented fallback rather than silently shipping a single-platform workflow.

## Report iEvo bugs upstream ([D-022](docs/decisions/D-022-autonomous-public-ievo-bug-reporting.md))

- Treat a reproducible iEvo malfunction, regression, broken hook, invalid command, or contradiction in an iEvo skill as an upstream bug.
- Report confirmed iEvo bugs autonomously to the public `ievo-ai/skills` GitHub repository without asking the user for additional permission.
- Search open and closed upstream issues first. Add useful evidence to an existing issue instead of creating a duplicate.
- Include the iEvo version, client surface, operating system, minimal reproduction, actual result, expected result, and relevant sanitized diagnostics.
- Public reports must never contain credentials, secrets, API keys, or tokens; redact those specifically. Otherwise, for iEvo-bug reports specifically (per [D-087](docs/decisions/D-087-relax-ievo-bug-report-privacy-scrubbing-to.md)), it is acceptable to include personal information, private repository data, proprietary source or documentation, raw conversation text, or identifying local paths as-is when doing so makes the report more actionable — this repository accepts that tradeoff for its own content given the project's experimental, single-maintainer status.
- Do not report expected behavior, ordinary project failures, or unverified suspicions. Gather enough evidence to make the report actionable and avoid automated issue spam.
- Link the upstream issue in the task summary and in the local PR when the reported bug affects the change being delivered.

## Keep machine-local hooks local ([D-023](docs/decisions/D-023-shared-auto-evolution-intent-with-local-hook.md), [D-025](docs/decisions/D-025-registered-contracts-for-shared-hook-targets.md), [D-077](docs/decisions/D-077-project-local-ievo-hook-lifecycle-is-symmetric.md), [D-081](docs/decisions/D-081-harden-the-project-local-ievo-lifecycle-boundary.md), [D-113](docs/decisions/D-113-reject-ancestor-symlinks-and-mount-points-in-the.md))

- Shared `.claude/settings.json` entries must not invoke scripts or other targets that are absent from a clean checkout. A hook whose target is gitignored is a clean-clone defect even when the hook failure is non-blocking.
- iEvo's generated hook scripts and vendored fallbacks under `.ievo/hooks/` are machine-local. Claude hook wiring belongs only in gitignored `.claude/settings.local.json`; Codex hook wiring belongs only in gitignored `.codex/hooks.json`. Never commit those entries or generated scripts.
- After enabling or refreshing iEvo, run `python3 scripts/manage_ievo_hooks.py localize` and then `python3 scripts/manage_ievo_hooks.py check --smoke`. The helper removes only the exact iEvo entries from shared Claude settings, preserves unrelated hooks, restores the repository's ignore policy if newer iEvo releases propose tracked shims, and validates both client surfaces.
- To disable auto-evolution in this clone, run `python3 scripts/manage_ievo_hooks.py disable`, not the generic iEvo disable workflow by itself. The project helper removes the exact iEvo entries from shared and local Claude settings plus Codex hooks before deleting their local targets, preserves the tracked project-wide intent flag, and is safe to repeat.
- Do not edit or regenerate hook configuration, or relocate `.claude`, `.codex`, `.ievo`, or their managed ancestors, while `localize`, `check`, or `disable` is running. The helper lock serializes its own invocations only; arbitrary editors and upstream tools do not participate, and portable path replacement/removal is not an atomic compare-and-swap.
- Shared hooks must not hide executable targets behind shell control separators, including literal line breaks, or in inline/stdin interpreter forms such as `sh -c`, `bash -s`, `python -c`, or `node --eval`.
- Interpreter options are fail-closed unless the validator explicitly models their operands; an option operand must never be mistaken for the executable hook target.
- Before changing shared hook configuration, test the tracked-file view of the repository. Every referenced filesystem target must be tracked and registered in `FAIL_SILENT_WRAPPER_CONTRACTS` with a tracked `scripts/test_*.py` contract that runs in required CI.
- A wrapper contract must simulate a clean clone without generated local hooks and prove that an absent local dependency produces a silent successful no-op. Adding a registered wrapper requires the D-025 security review; a merely tracked script is not sufficient.

## Protect main ([D-024](docs/decisions/D-024-protected-main-and-audited-emergency-bypass.md))

- `main` accepts changes only through pull requests. Branch protection requires the current CI check, resolved conversations, and an up-to-date branch.
- While the repository has only one maintainer, require zero approving reviews so the PR path remains usable; enable an independent approving review when a second human maintainer is available.
- Administrators and automation credentials do not bypass the rule for ordinary work. The emergency procedure, audit expectations, and recovery steps live in [REPOSITORY_GOVERNANCE.md](docs/REPOSITORY_GOVERNANCE.md).
- A failed `main-history-audit` run is a release-blocking governance incident. Open an issue, identify the bypass and actor, and restore protection before further merges.
- Every session's D-021 preflight also runs `python3 scripts/manage_ci_bypass.py status`.
  If branch protection differs from the documented baseline
  (`docs/REPOSITORY_GOVERNANCE.md`), search for an open `[ci-bypass]`-prefixed
  issue tracking it. If one exists and is not past its recorded expiry,
  no action is needed -- it is being actively worked. Otherwise (no
  tracking issue at all, or one that is open past its own recorded expiry
  with no restore recorded) this is a release-blocking governance
  incident: run `python3 scripts/manage_ci_bypass.py restore --to-baseline`
  (or `restore --incident <issue-number>` if a stale-but-identifiable
  incident exists and should be closed through its own recorded snapshot
  instead of the baseline) immediately -- or escalate if restore itself
  fails -- before any other work in this session.
- The push audit executes the pre-push `main` revision of `scripts/check_main_history.py`, with an immutable reviewed bootstrap fallback when that parent predates the checker; it never executes the revision being audited. Its workflow definition is still supplied by the pushed revision, so treat the job as defense-in-depth: the external repository monitor must verify the workflow content and expected run independently.

## Monitor only live repository events ([D-078](docs/decisions/D-078-external-repository-monitoring-is-checkpointed.md))

- Establish an explicit monitoring checkpoint from the refreshed remote default-branch commit. For every open pull request, record its number, state, draft status, and head commit; for every task-active pull request, also record mergeability, unresolved review threads, and required checks. Report the default-branch commit when monitoring starts or resumes.
- After the checkpoint, monitor only a newly observed default-branch commit; a newly opened or reopened pull request; a state, draft-status, or head change relative to the recorded baseline for an inventoried open pull request; or a mergeability, review-thread, or required-check change on a pull request already active in the current task. Ignore `updated_at` changes caused only by comments, reactions, labels, or other activity outside those fields.
- A pull request or issue cited only by documentation, an ADR, a retrospective, or a session log is historical evidence, not a live target. Do not poll a closed or merged pull request or a closed issue. Evaluate a task-active pull request's post-checkpoint close or merge once, then remove it from the live set; inspect an issue only when the active task explicitly names it.
- Before waiting on CI, query the pull request's current state, draft status, mergeability, head commit, and unresolved review threads. Stop waiting and re-evaluate when it closes, merges, becomes conflicting, or its head changes; never keep polling checks for a superseded head.
- When a new default-branch merge appears, inspect the introduced commit range and the exact post-merge workflows for that merge commit. Advance the checkpoint only after recording the new authoritative state, so the next cycle cannot rediscover the same event as new work.

## Testing and hard coverage gate

- One hundred percent line and region coverage is a hard merge invariant under D-014, not a target or guideline.
- CI must run `run_isolated "$TRUSTED_COV" llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100` on every pull request inside the checker-approved, sanitized `nobody` boundary and without an earlier head-controlled script. Do not merge while this gate is missing, skipped, cancelled, failing, or still in progress.
- Every behavior change must include tests for its success, failure, and relevant edge paths so the gate is satisfied by meaningful execution rather than incidental line hits.
- Never lower either threshold, remove either flag, disable the job, narrow the measured workspace, or exclude code merely to make a pull request pass.
- The only permitted exemption is a whole-file `--ignore-filename-regex` entry justified by an accepted design constraint and recorded in the exemption table in `docs/TESTING.md`. An undocumented exemption is a review-blocking defect.

## CI and deployment privilege boundaries

- Treat workflow definitions, scripts, and build inputs from every pull request as untrusted, including pull requests from branches in the base repository.
- A job that checks out or executes pull-request-controlled code, or consumes its artifacts, caches, or outputs, must use the minimum token permissions it needs: normally `contents: read`, or `permissions: {}` when repository access is unnecessary. Beyond that minimally scoped `GITHUB_TOKEN`, it must not receive write scopes, OIDC access, any secret or credential, or a protected environment.
- Grant any elevated capability only to the smallest isolated job that needs it. Every privileged job, including jobs in reusable workflows and workflows without a pull-request trigger, must use the exact `push` plus `refs/heads/main` guard enforced by `scripts/check_ci_permissions.rb`. It must establish its trusted commit source, validate the actor when actor identity is part of the trust decision, and must not execute untrusted code or consume untrusted state unless provenance and integrity are verified against that commit.
- Gate publish and deploy jobs to `refs/heads/main` and a protected environment. If the repository later adds a release-branch or tag deployment, extend the checker's explicit allowlist and record the corresponding ref-protection evidence in the same pull request before granting privilege. Never rely on a skipped step to contain credentials granted at workflow scope or to an earlier validation job.
- Regular CI must run `ruby scripts/check_ci_permissions.rb` for fast feedback. The read-only `Workflow policy` check (`audit`) is the trust anchor: it runs on every pull request from the base commit under `pull_request_target`, never checks out or executes pull-request code, and audits the head revision's workflow YAML plus roadmap acceptance evidence as data. Keep that check required before merging -- never permanently remove or downgrade it. The one narrow exception is D-125's session-driven temporary bypass, which may relax `audit` for exactly one publicly tracked, expiry-bound incident at a time and restores it immediately afterward; see `docs/REPOSITORY_GOVERNANCE.md`'s "Session-driven temporary bypass" section.
- Whenever a workflow adds a `pull_request`, `pull_request_target`, or chained trigger, begins executing a repository script, transfers state between jobs, or changes job-level `permissions`, review every job's effective permissions and all artifact, cache, output, and reusable-workflow boundaries. Add a focused negative-event check for privileged behavior where practical; otherwise record the unautomated trust assumptions and verification evidence in the owning specification or workflow.

## Keep source files decomposable

- A Rust source file over ~1,000 lines is a maintainability and agent-context risk (several `lib.rs` files here already reach 15-18k lines, past what a single `Read` call covers). When a task's own work touches such a file, decompose the part it touches into cohesion-driven submodules as part of that same change — not as a separate dedicated refactor task, and not by rewriting unrelated code.

## Code Review Rules

### Solo-maintainer branch protection

- This repository currently has one maintainer. Do not require an approving pull-request review in branch protection: GitHub does not count an author's approval of their own pull request, so that setting deadlocks solo-maintainer work.
- Keep required status checks, including the 100% coverage gate, and required conversation resolution enabled. Revisit the approving-review requirement when a second human maintainer is available.

### Local pinned review loop ([D-068](docs/decisions/D-068-use-a-pinned-local-reviewer-as-the-required.md))

- Before completing significant work or merging a pull request, inspect only
  the repository's explicitly pinned, security-reviewed reviewer dependencies
  documented in `docs/AGENT_TOOLING.md`. Select the eligible read-only reviewer
  with the broadest correctness, contract, security, test, and documentation
  checklist, then start it directly in a fresh independent local context. The
  current default is the pinned iEvo `deep-reviewer`.
- Load the reviewer only from the installed immutable plugin artifact whose
  digest is recorded in `docs/AGENT_TOOLING.md`; never substitute a similarly
  named global agent or a reviewer definition from the branch, index, or
  working tree. If the client cannot bind that exact reviewer, report the local
  review as unavailable instead of silently weakening the gate.
- Review staged or working-tree changes before commit. For an existing pull
  request with a clean tree, refresh the remote default branch and review the
  full committed range from its merge base through `HEAD`, not a two-dot diff
  against the default-branch tip.
- Before invoking the pinned reviewer, inspect `git status --short` and ensure
  every intended new file is part of the selected diff. iEvo `deep-review`
  through 0.70.1 omits untracked files from `--working`; stage new files before
  a staged review, and never treat a working-tree verdict as complete while
  relevant untracked files remain. Track the upstream fix in
  [ievo-ai/skills#483](https://github.com/ievo-ai/skills/issues/483).
- Do not select arbitrary globally installed or marketplace review skills.
  Add a new eligible reviewer only through the repository's agent-tool
  security-check and pinning process.
- Do not use a GitHub `@codex review` comment as a required gate. External
  GitHub reviews remain optional when the user explicitly requests one, but
  asynchronous review availability must not block the local review loop.
- Address every verified P0/P1 finding and every other actionable correctness
  or contract finding before merge. Keep fixes focused and rerun the selected
  review skill after fixes when its previous findings may no longer describe
  the current diff.
- Merge only when required checks, including the 100% coverage gate, are
  green and no unresolved actionable review finding or pull-request thread
  remains.
- Skill-based review is an additional high-signal pass, not a replacement for
  tests, specifications, branch protection, or independent human review.

### Review focus

- Check implementation against the relevant documents linked from `docs/SPEC.md`, especially public contracts, diagnostics, portability, error paths, ownership, and cross-crate boundaries.
- Flag concrete correctness, security, compatibility, test, and documentation defects. Leave formatting, lint, and other deterministic mechanical checks to CI.

## Keep a retrospective log and a session handoff log ([D-066](docs/decisions/D-066-maintain-an-agent-retrospective-log-and-a-session.md), [D-130](docs/decisions/D-130-decompose-the-session-handoff-log-into-per.md))

- `docs/AGENT_RETROSPECTIVE.md` is a process-mistake journal, not a code-bug tracker: log a mistake in *how the work was done* (wasted time, a wrong assumption, thrashing against a moving target, a convention violated before it was caught) when it cost meaningful time and the lesson is something a future session could actually act on. Do not log routine debugging, ordinary compiler errors, or first-try successes. Write date, what happened, root cause, what fixed it, and an actionable lesson — newest entry first.
- `docs/sessions/` holds one running handoff snapshot per checkpoint, not a shared transcript: at a meaningful checkpoint (a PR opened or merged, a milestone reached, before a long session ends or hands off), create a new file named `YYYY-MM-DD-NN-<slug>.md` (extending the `docs/superpowers/specs`/`docs/superpowers/plans` dated-slug convention with a two-digit same-day sequence number — `01` for the day's first entry, ascending for later ones, since several dates already carry many same-day checkpoints and the date prefix alone cannot order them) with overall status, what's currently in flight, known follow-ups, and where a fresh session should look to resume. Ground each snapshot in the exact commit and repository state actually inspected, and distinguish uncommitted or unmerged work from delivered work. Never edit a previous session's file to append a later checkpoint — a new checkpoint is always a new file, so two sessions active at once never race to edit the same one. `docs/sessions/README.md` is the sole exception: a static purpose statement, not an entry log, never appended to.
- To resume, list the dated entries under `docs/sessions/` (excluding `README.md`) sorted by filename — the date-then-`NN` prefix keeps lexical order chronological — and read the most recent few entries: there is no maintained index to keep current, only the directory itself.
- Immediately before committing a `docs/sessions/` entry that references remote state, fetch again and re-resolve every referenced default-branch commit, pull-request head/state, review thread, and CI result. If any referenced state changed while the snapshot was being drafted or reviewed, rewrite the entry from the authoritative git and GitHub state before committing; never preserve an already-completed step as current work merely because it was pending earlier in the session.
- Neither the retrospective journal nor `docs/sessions/` is a merge gate, CI-enforced, or machine-generated; both are reviewed like any other documentation change. Never write credentials, secrets, or personal information into either.
- These logs do not relax `docs/decisions/`'s own scope: an irreversible or project-wide design choice still belongs in its own file under `docs/decisions/` with its alternatives considered, not summarized here instead.

## Completion check

Before finishing a change:

1. Re-read the relevant entries linked from `docs/SPEC.md`.
2. Update the affected docs in the same patch as the implementation.
3. Check links, examples, commands, status statements, and references to renamed files.
4. Run the relevant tests and documentation generation or freshness checks.
5. Run the pinned local reviewer for significant changes and address its actionable findings.

<!-- ievo:start -->
**Before applying the instructions below**, read `.ievo/evolution/project.md` if it exists, and apply ALL rules from its sections IN ADDITION to the project's instructions.
<!-- ievo:end -->
