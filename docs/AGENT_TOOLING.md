# Agent tooling supply-chain policy

Repository-scoped agent instructions and plugins are executable control-plane inputs.
They must be reproducible, reviewed, and rollbackable just like compiler dependencies.

## Current pins

| Surface | Dependency | Pin | Automatic updates |
|---|---|---|---|
| Codex | `ievo@ievo-skills` | commit `7d5f3e12d0556cb6c5df2974e2babe0433674186` (`v0.58.1`) | disabled by immutable source |
| Codex | repository skills under `.agents/skills/` | current repository revision | project-scoped; no global installation |
| Claude Code | `ievo@ievo-skills` | commit `7d5f3e12d0556cb6c5df2974e2babe0433674186` (`v0.58.1`) | `autoUpdate: false` |
| Claude Code | six enabled `claude-plugins-official` plugins | commit `15ba5db4a9cfa1a1ec217c60c6fbb66f0f2dd66f` | `autoUpdate: false` |
| Claude Code | five enabled `claude-code-workflows` plugins | commit `c4b82b0ad771190355eb8e204b1329732a18449a` | `autoUpdate: false` |

The Codex pin lives in `.agents/plugins/marketplace.json`. Every enabled Claude Code
plugin is declared in an inline settings marketplace in `.claude/settings.json`, with
automatic updates disabled and a full commit `sha` on its `git-subdir` source.
Marketplace `ref` values support movable branches and tags, so they are prohibited for
these pins. Codex and Claude Code resolve iEvo from the same immutable commit rather
than trusting a movable release tag. A fresh Codex checkout is bootstrapped explicitly:

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
Claude Code reads the project-scoped marketplace declarations after the repository is
trusted and enables the configured plugins without enabling automatic updates.
The `Agent assets` workflow repeats the validators plus isolated Codex and Claude Code
checks on every pull request and push to `main`. Claude Code validates the project
settings and every extracted inline marketplace with its strict manifest validator,
then registers those extracted marketplaces and installs every enabled coordinate in a
fresh configuration. That install proves each remote commit and plugin subdirectory
actually resolves without help from global state. The repository validator also
requires every enabled plugin to resolve to exactly one approved HTTPS Git repository,
the matching `./plugins/<name>` subdirectory, and a full-SHA `git-subdir` source, so a
developer's global installation cannot mask a movable or broken repository declaration.

## Reviewed update process

1. Open a dependency pull request that changes every affected pin. Keep the Codex and
   Claude Code iEvo pins identical. Include the old and new commits and link the
   upstream release notes or changelog.
2. Run the iEvo security-check workflow against every candidate plugin release before
   accepting it. Record its verdict and any reviewed exceptions in the pull request.
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

## Rollback

Revert each affected pin to its last reviewed commit, rerun the validation commands,
and rerun `./scripts/bootstrap-agent-tools.sh` when the Codex iEvo pin changed. If the
candidate release executed unsafe hooks, disable the plugin until the rollback is
complete and rotate any credential that the hook could have observed.

The last known-good commit and release are present in Git history, so rollback never
depends on a mutable upstream branch or tag lookup.
