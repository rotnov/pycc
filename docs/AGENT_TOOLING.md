# Agent tooling supply-chain policy

Repository-scoped agent instructions and plugins are executable control-plane inputs.
They must be reproducible, reviewed, and rollbackable just like compiler dependencies.

## Current pins

| Surface | Dependency | Pin | Automatic updates |
|---|---|---|---|
| Codex | `ievo@ievo-skills` | commit `7d5f3e12d0556cb6c5df2974e2babe0433674186` (`v0.58.1`) | disabled by immutable source |
| Codex | repository skills under `.agents/skills/` | current repository revision | project-scoped; no global installation |
| Claude Code | `ievo@ievo-skills` | commit `7d5f3e12d0556cb6c5df2974e2babe0433674186` (`v0.58.1`) | `autoUpdate: false` |

The Codex pin lives in `.agents/plugins/marketplace.json`. The Claude Code pin is the
`sha` of the `git-subdir` plugin source inside the inline settings marketplace in
`.claude/settings.json`. Marketplace `ref` values only support branches and tags, so
the exact commit belongs on the plugin source instead. Both surfaces resolve the same
immutable commit rather than trusting a movable release tag. A fresh Codex checkout
is bootstrapped explicitly:

```sh
./scripts/bootstrap-agent-tools.sh
```

Codex does not implicitly register a repository-local marketplace. The bootstrap
script replaces a same-named registration from another checkout, registers this
repository as the marketplace, and installs only the pinned iEvo plugin.
Repository-owned Codex entry points live under `.agents/skills/`, so Codex discovers
them only while this checkout is active; they are never installed globally or made
available to unrelated repositories. The entry points load the canonical workflow
bodies from this checkout's `.claude/skills/`, and CI requires the two skill sets and
their discovery metadata to stay in lockstep.
Claude Code reads the project-scoped marketplace declaration after the repository is
trusted and enables the configured plugin without enabling automatic updates.
The `Agent assets` workflow repeats the validators plus isolated Codex and Claude Code
checks on every pull request and push to `main`. Claude Code validates the project
settings and the extracted inline marketplace with its strict manifest validator, so
a developer's global plugin installation cannot mask a broken repository pin or
marketplace declaration.

## Reviewed update process

1. Open a dependency pull request that changes both pins to the same upstream release.
   Include the old and new commits and link the upstream release notes or changelog.
2. Run the iEvo security-check workflow against the candidate release before accepting
   it. Record its verdict and any reviewed exceptions in the pull request.
3. Review the plugin and skill diff, paying particular attention to instructions,
   hooks, external writes, shell commands, network access, and newly introduced
   dependencies.
4. Run:

   ```sh
   python3 scripts/validate_agent_assets.py
   ./scripts/check-codex-marketplace.sh
   ./scripts/check-claude-marketplace.sh
   ```

5. Merge only through the normal reviewed pull-request and required-CI path.

No scheduled job, startup hook, or local bootstrap command may rewrite these pins.

## Local review workflow

Code review is performed locally before significant work is completed or a pull
request is merged. The repository-owned `review-local-changes` skill provides
the model-invocable Codex and Claude Code entrypoints. It searches only
repository-owned reviewers and dependencies listed in the pin table above,
then selects the most comprehensive eligible engine by checklist coverage,
full-diff support, enforced read-only tools, independent reviewer context, and
support on both clients. Arbitrary globally installed or marketplace skills
are never eligible.

The current engine is the pinned iEvo `deep-reviewer` agent, whose tool
allowlist is read-only and whose explicit 11-point review runs in a separate
context on both supported clients. The upstream `deep-review` skill is
explicit-invocation-only, so the repository wrapper performs its deterministic
scope selection before autonomously dispatching the pinned reviewer agent. The
shared `prepare_review.py` helper validates that agent's tool policy and emits
separate committed, staged, and working-tree scopes, including untracked
non-ignored files. A future engine becomes eligible only after it passes the
reviewed update process and is added to the repository-owned wrapper and pin
table.

The helper binds preparation to iEvo commit
`7d5f3e12d0556cb6c5df2974e2babe0433674186` and reviewer SHA-256
`b5e11469ba8144686d07eccc3d0759662b9c1bc4c3a6f3d79961dc82f5e53ab2`.
Both values change together during the reviewed update process. Scope
preparation fails closed when the remote default branch, merge base, pin, or
artifact cannot be verified. Every committed, staged, tracked-working, and
untracked path is classified before dispatch. Only regular non-symlink files
enter the reviewer's file-read list; symlinks, symlinked path components,
gitlinks, and deleted paths are described as inert metadata without following
them. The fixed repository pin is traversed and opened descriptor-relative
with no-follow semantics for every component. Preparation fails closed on a
platform that cannot provide that race-safe primitive.

For uncommitted work, the selected skill reviews the staged or working-tree
diff. For a clean pull-request branch, it reviews the committed range from the
merge base with the refreshed remote default branch through `HEAD`. Using the
merge base avoids treating default-branch-only commits as reversed changes when
the task branch is behind.

The review is read-only. Actionable correctness, contract, security, test, or
documentation findings are fixed and the local review is rerun when those fixes
materially change the diff. GitHub comments such as `@codex review` are not a
required gate: the asynchronous service can be delayed or unavailable, while
the pinned local workflow remains reproducible. An external GitHub review may
still be requested explicitly by the user.

Marketplace popularity alone never authorizes installing a review skill.
Installing a new third-party reviewer requires user authorization and the same
security-check and pinning process as any other agent dependency.
The isolated Codex and Claude marketplace checks install the exact pinned
plugin, verify the reviewer artifact digest, exercise every shared scope, and
exercise fail-closed artifact and base-resolution paths. They validate
discovery and the deterministic contract around dispatch. Model-backed
dispatch is not executed in unauthenticated CI. At runtime, clients with named
plugin-agent support bind the reviewer to the verified artifact and its native
read-only policy. Other clients use the repository-owned fallback: a fresh
subagent receives the same verified instructions, no mutation or network
authority, and the workflow rejects the run if a before/after Git status
snapshot changes.

## Rollback

Revert both pin changes to the last reviewed release, rerun the two validation
commands, and rerun `./scripts/bootstrap-agent-tools.sh` to refresh the local Codex
installation. If the candidate release executed unsafe hooks, disable the plugin until
the rollback is complete and rotate any credential that the hook could have observed.

The last known-good commit and release are present in Git history, so rollback never
depends on a mutable upstream branch or tag lookup.
