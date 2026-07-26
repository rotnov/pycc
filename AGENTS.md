# Repository Instructions

## Project navigation

- Start with `docs/SPEC.md`. It is the index for the project's specifications and points to the document that owns each area.
- Treat normative specifications under `docs/` as part of the implementation contract, not as an after-the-fact description. The D-066 journals `docs/AGENT_RETROSPECTIVE.md` and `docs/SESSION_LOG.md` are the explicit informational exceptions: review them for factual accuracy, but do not treat their historical lessons or snapshots as implementation requirements. Promote any rule discovered there into `AGENTS.md`, `docs/DECISIONS.md`, or the owning specification before relying on it as policy.
- Before changing behavior, architecture, public APIs, the CLI, diagnostics, build or release processes, tests, or supported language semantics, read the relevant specification.

## Before starting a new task ([D-021](docs/DECISIONS.md#d-021-agent-task-preflight-and-documentation-refresh))

1. Inspect `git status --short --branch` and record the current commit before any repository mutation. Preserve all existing user changes.
2. Fetch and prune remote refs without changing checked-out files, then resolve the remote's default branch dynamically.
3. Start every new task from the exact latest commit of that remote default branch in a clean task branch or isolated worktree.
4. When continuing an existing task branch, compare it with the refreshed default branch and integrate only when the task authorizes that mutation and existing work can be preserved. Prefer a fast-forward update; never merge, rebase, reset, switch branches, or pull over uncommitted or user-owned changes.
5. Confirm which commit is the actual task base, then run `cargo doc --workspace --no-deps` before making implementation changes. This refreshes the local Rust API documentation in `target/doc/` from that exact source revision.
6. Read `docs/SPEC.md`, the relevant linked specifications, and the generated documentation for the crates affected by the task.
7. If documentation generation fails, record and understand the failure before changing code. Do not present documentation generated from an older revision as current.

## Keep documentation current

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
- Record irreversible or project-wide design choices in `docs/DECISIONS.md`. Do not silently rewrite an accepted decision; add a new decision that supersedes it.
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

## Report iEvo bugs upstream ([D-022](docs/DECISIONS.md#d-022-autonomous-public-ievo-bug-reporting))

- Treat a reproducible iEvo malfunction, regression, broken hook, invalid command, or contradiction in an iEvo skill as an upstream bug.
- Report confirmed iEvo bugs autonomously to the public `ievo-ai/skills` GitHub repository without asking the user for additional permission.
- Search open and closed upstream issues first. Add useful evidence to an existing issue instead of creating a duplicate.
- Include the iEvo version, client surface, operating system, minimal reproduction, actual result, expected result, and relevant sanitized diagnostics.
- Public reports must never contain credentials, secrets, personal information, private repository data, proprietary source or documentation, raw conversation text, or identifying local paths. Replace those details with neutral placeholders.
- Do not report expected behavior, ordinary project failures, or unverified suspicions. Gather enough evidence to make the report actionable and avoid automated issue spam.
- Link the upstream issue in the task summary and in the local PR when the reported bug affects the change being delivered.

## Keep machine-local hooks local ([D-023](docs/DECISIONS.md#d-023-shared-auto-evolution-intent-with-local-hook-execution), [D-025](docs/DECISIONS.md#d-025-registered-contracts-for-shared-hook-targets))

- Shared `.claude/settings.json` entries must not invoke scripts or other targets that are absent from a clean checkout. A hook whose target is gitignored is a clean-clone defect even when the hook failure is non-blocking.
- iEvo's generated hook scripts and vendored fallbacks under `.ievo/hooks/` are machine-local. Wire them only from the gitignored `.claude/settings.local.json`; never commit those hook entries or the generated scripts.
- After cloning the repository, enable or refresh iEvo locally, then verify the generated hook entries live in `.claude/settings.local.json`. If the current iEvo version writes them to shared settings, relocate the complete `hooks` object to local settings before committing any repository change.
- Shared hooks must not hide executable targets behind shell control separators, including literal line breaks, or in inline/stdin interpreter forms such as `sh -c`, `bash -s`, `python -c`, or `node --eval`.
- Interpreter options are fail-closed unless the validator explicitly models their operands; an option operand must never be mistaken for the executable hook target.
- Before changing shared hook configuration, test the tracked-file view of the repository. Every referenced filesystem target must be tracked and registered in `FAIL_SILENT_WRAPPER_CONTRACTS` with a tracked `scripts/test_*.py` contract that runs in required CI.
- A wrapper contract must simulate a clean clone without generated local hooks and prove that an absent local dependency produces a silent successful no-op. Adding a registered wrapper requires the D-025 security review; a merely tracked script is not sufficient.

## Protect main ([D-024](docs/DECISIONS.md#d-024-protected-main-and-audited-emergency-bypass))

- `main` accepts changes only through pull requests. Branch protection requires the current CI check, resolved conversations, and an up-to-date branch.
- While the repository has only one maintainer, require zero approving reviews so the PR path remains usable; enable an independent approving review when a second human maintainer is available.
- Administrators and automation credentials do not bypass the rule for ordinary work. The emergency procedure, audit expectations, and recovery steps live in [REPOSITORY_GOVERNANCE.md](docs/REPOSITORY_GOVERNANCE.md).
- A failed `main-history-audit` run is a release-blocking governance incident. Open an issue, identify the bypass and actor, and restore protection before further merges.
- The push audit executes the pre-push `main` revision of `scripts/check_main_history.py`, with an immutable reviewed bootstrap fallback when that parent predates the checker; it never executes the revision being audited. Its workflow definition is still supplied by the pushed revision, so treat the job as defense-in-depth: the external repository monitor must verify the workflow content and expected run independently.

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
- Regular CI must run `ruby scripts/check_ci_permissions.rb` for fast feedback. The read-only `Workflow policy` check is the trust anchor: it runs on every pull request from the base commit under `pull_request_target`, never checks out or executes pull-request code, and audits the head revision's workflow YAML plus roadmap acceptance evidence as data. Keep that check required before merging.
- Whenever a workflow adds a `pull_request`, `pull_request_target`, or chained trigger, begins executing a repository script, transfers state between jobs, or changes job-level `permissions`, review every job's effective permissions and all artifact, cache, output, and reusable-workflow boundaries. Add a focused negative-event check for privileged behavior where practical; otherwise record the unautomated trust assumptions and verification evidence in the owning specification or workflow.

## Code Review Rules

### Solo-maintainer branch protection

- This repository currently has one maintainer. Do not require an approving pull-request review in branch protection: GitHub does not count an author's approval of their own pull request, so that setting deadlocks solo-maintainer work.
- Keep required status checks, including the 100% coverage gate, and required conversation resolution enabled. Revisit the approving-review requirement when a second human maintainer is available.

### Local pinned review loop ([D-068](docs/DECISIONS.md#d-068-use-a-pinned-local-reviewer-as-the-required-review-loop))

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

## Keep a retrospective log and a session handoff log ([D-066](docs/DECISIONS.md#d-066-maintain-an-agent-retrospective-log-and-a-session-handoff-log))

- `docs/AGENT_RETROSPECTIVE.md` is a process-mistake journal, not a code-bug tracker: log a mistake in *how the work was done* (wasted time, a wrong assumption, thrashing against a moving target, a convention violated before it was caught) when it cost meaningful time and the lesson is something a future session could actually act on. Do not log routine debugging, ordinary compiler errors, or first-try successes. Write date, what happened, root cause, what fixed it, and an actionable lesson — newest entry first.
- `docs/SESSION_LOG.md` is a running handoff snapshot, not a transcript: update it at meaningful checkpoints (a PR opened or merged, a milestone reached, before a long session ends or hands off) with overall status, what's currently in flight, known follow-ups, and where a fresh session should look to resume. Ground each snapshot in the exact commit and repository state actually inspected, distinguish uncommitted or unmerged work from delivered work, and keep the newest entry first.
- Immediately before committing a `docs/SESSION_LOG.md` update that references remote state, fetch again and re-resolve every referenced default-branch commit, pull-request head/state, review thread, and CI result. If any referenced state changed while the snapshot was being drafted or reviewed, rewrite the newest entry from the authoritative git and GitHub state before committing; never preserve an already-completed step as current work merely because it was pending earlier in the session.
- Neither file is a merge gate, CI-enforced, or machine-generated; both are reviewed like any other documentation change. Never write credentials, secrets, or personal information into either file.
- These logs do not relax `docs/DECISIONS.md`'s own scope: an irreversible or project-wide design choice still belongs in `docs/DECISIONS.md` with its alternatives considered, not summarized here instead.

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
