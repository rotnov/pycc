# Agent tooling supply-chain policy

Repository-owned agent instructions and the cross-platform plugin baseline documented
here are executable control-plane inputs. They must be reproducible, reviewed, and
rollbackable just like compiler dependencies.

## Current pins

| Surface | Dependency | Pin | Automatic updates |
|---|---|---|---|
| Codex | `ievo@ievo-skills` | commit `7d5f3e12d0556cb6c5df2974e2babe0433674186` (`v0.58.1`) | disabled by immutable source |
| Codex | repository skills under `.agents/skills/` | current repository revision | project-scoped; no global installation |
| Claude Code | `ievo@ievo-skills` | commit `7d5f3e12d0556cb6c5df2974e2babe0433674186` (`v0.58.1`) | `autoUpdate: false` |
| Codex and Claude Code | `rotnov/skills@i-have-an-issue` | tag `i-have-an-issue-v0.1.1`; reviewed source commit `1bc6bcee3766a7e62b936343a48ebb56a3767470`; vendored hash `99e492ccae20ad3acf02e28dd76c7d74de28c7cf2141bfc7a2942c46c4bf687c` | manual updates only |

The Codex pin lives in `.agents/plugins/marketplace.json`. The Claude Code pin is the
`sha` of the `git-subdir` plugin source inside the inline settings marketplace in
`.claude/settings.json`. Marketplace `ref` values only support branches and tags, so
the exact commit belongs on the plugin source instead. Both surfaces resolve the same
immutable commit rather than trusting a movable release tag. A fresh Codex checkout
is bootstrapped explicitly:

```sh
./scripts/bootstrap-agent-tools.sh
```

The inline `ievo-skills` marketplace must contain exactly that one pinned `ievo`
plugin. Any sibling or malformed plugin entry invalidates the configuration; the
marketplace-wide source exemption is safe only because the complete entry set is
validated.

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

The `i-have-an-issue` skill is installed with:

```sh
npx skills@1.5.20 add rotnov/skills#i-have-an-issue-v0.1.1 \
  --skill i-have-an-issue -a claude-code --copy -y
```

The complete reviewed copy lives under `.claude/skills/i-have-an-issue/`; the
matching `.agents/skills/` entry is the repository's standard thin Codex
wrapper. `skills-lock.json` records the immutable upstream tag, exact reviewed
commit, and content hash. Updates are deliberate repository changes and must
preserve the canonical-copy/wrapper split.

The pre-install iEvo security review scanned all seven distributed files
(31,206 bytes). The content verdict is **YELLOW** because the skill necessarily
loads outsider-authored GitHub issue and pull-request text into agent context.
The skill mitigates that indirect prompt-injection exposure by treating those
artifacts as untrusted evidence and forbidding embedded instructions, secret
access, or unreviewed command execution. Skills.sh currently retains a Snyk
`E004` critical result from the previous upstream revision, where a hidden
iEvo overlay loader appeared in `SKILL.md`; the reviewed source commit removes
that loader and adds a repository validator that rejects its return. The same
audit reported `W011`
for the inherent third-party-content exposure. Gen Agent Trust Hub reported
Safe and Socket reported zero alerts. Recheck these external signals on the
next upstream scan; they are context, not a substitute for reviewing the
vendored bytes. The stale skills.sh index and rescan request are tracked in
[`vercel-labs/skills#1776`](https://github.com/vercel-labs/skills/issues/1776).

## Project-local alpha skills

`pycc` and `pycc-feedback` follow the
[Agent Skills specification](https://agentskills.io/specification) but remain
project-local alpha workflows. They are committed under `.claude/skills/` with
thin `.agents/skills/` entrypoints for equal Claude Code and Codex discovery.
They are intentionally absent from `skills-lock.json`, `rotnov/skills`, and
skills.sh until their trigger and output evals mature.

`pycc` distinguishes the implemented compiler slice from planned
specifications before running commands. `pycc-feedback` may reproduce,
minimize, sanitize, run sanitized duplicate searches, and prepare a public
GitHub draft without approval. Non-public search terms require a separate
exact-query preview and approval before transmission. The skill must also show
the exact write payload and receive explicit per-payload user confirmation
before creating an issue or comment in `rotnov/pycc`.

The required CI build runs `scripts/run_alpha_skill_evals.py` after resolving
both the Codex wrapper and the Claude Code canonical entrypoint. The primary
`pycc` offline eval creates a self-contained temporary source file, invokes the
freshly built compiler, executes the generated binary, and checks its exact
output. Its additional scenarios execute a successful explicit-file `check`,
the current `check --fix` rejection, and a self-created parser failure. The
`pycc-feedback` cases exercise
draft-only, private-source refusal, and context-free-consent invariants through
a fail-closed safety oracle that cannot perform a network write. All four
reviewed `i-have-an-issue` scenarios validate their distinct evidence criteria
and execute the vendored search helper's local help path without a network
request. The unit suite covers full scenario dispatch, build failure, wrong
output, entrypoint drift, incomplete runner registration, and every consent
predicate.

These deterministic checks do not invoke a language model and do not claim
that either client's generated response conforms to the prompts. Authenticated
model-response evals remain a promotion requirement before either alpha skill
can move into `skills-lock.json`, `rotnov/skills`, or skills.sh. The asset
validator enforces that fallback: `pycc` and `pycc-feedback` cannot enter the
locked skill set unless immutable HTTPS evidence exists for authenticated
model evals on both Codex and Claude. Until then, they remain project-local
alpha workflows. The separate `Agent assets` job still installs the real
pinned client CLIs and verifies discovery through both surfaces without model
credentials.

## Optional Claude Code plugins

`.claude/settings.json` also enables optional third-party capability plugins from the
`anthropics/claude-plugins-official` and `wshobson/agents` marketplaces. Those
marketplace sources are not pinned, are not installed by the Codex bootstrap, and are
not part of the reproducible cross-platform baseline above. The exemption is the
exact validated `ievo@ievo-skills` identity, not every plugin that happens to share
that marketplace name. Repository instructions, tests, and required workflows must
not depend on optional plugins. The agent-asset validator also requires `CLAUDE.md`
to contain exactly the `@AGENTS.md` import, keeping the shared Codex and Claude
instruction contract fail-closed instead of relying on documentation alone. It scans
the shared instructions (including scoped `AGENTS.md` and `CLAUDE.md` files), every tracked file
under the Codex, Claude, and iEvo evolution trees, tracked tests (including source
formats that can contain inline tests), required workflows, the repository `scripts/`
tree, local action manifests anywhere in the repository, every tracked file under the
conventional `.github/actions/` tree, interpreter-recognized script formats, and
tracked executables. It follows repository-relative script invocations from those
required assets through recognized interpreters, including recursively referenced
extensionless, non-executable scripts and scripts supplied on standard input with
shell `<` or `0<` redirection. Known interpreter options distinguish ordinary
values, loaded code files, and inline-code modes; ambiguous future options fail closed
by selecting every repository-relative operand. Windows `.exe` interpreter names and
backslash paths are normalized to the same Git/POSIX tracked paths without accepting
drive-qualified, UNC, or absolute targets. Parent components are resolved lexically
against the command's effective working directory when the result stays inside the
repository; paths that escape it fail closed. Discovery comes from
`git ls-files`; shell line continuations, escaped spaces, and Markdown or subshell
closing delimiters are handled without becoming part of a tracked path, and GitHub
Actions folded `run: >` scalars are scanned with their executed newline semantics,
including valid chomping and explicit indentation indicators such as `>-`, `>+`,
`>2`, and `>2-`; literal `|` forms receive the corresponding block treatment. Static
workflow-, job-, and step-level `working-directory` values, plus step-level values in
repository-wide composite action manifests, are applied to their effective `run`
steps. The directory remains in force through recursively invoked extensionless
helpers without leaking from one step or job into another. Quoted mapping keys and
block sequences with standalone `-` entries are supported. Dynamic or non-repository
working directories, YAML merge keys, aliases in structural positions, and flow-style
job/step structures fail validation when they cannot be resolved safely.
For non-composite actions, local JavaScript `main`, `pre`, and `post` entrypoints and
Docker `image` build files are resolved relative to the action manifest and scanned;
JavaScript entrypoints retain the repository workspace as their runtime working
directory for recursive helper discovery. External `docker://` images remain external
coordinates rather than repository paths.
Ignored caches and dependency checkouts therefore cannot make local validation
disagree with a clean CI checkout.
Required agent assets stored as Git symlinks are rejected rather than followed outside
the reviewed tree.
Within `.claude/settings.json`, `enabledPlugins` and `extraKnownMarketplaces` declare
optional capabilities and are excluded from the reference scan; behavioral fields
such as shared hooks remain required and scanned. The provenance frontmatter of a
vendored `.ievo/evolution` overlay is likewise metadata. A standard dated
`Vendored from <repo>` heading is also provenance only when its repository exactly
matches that frontmatter; every other part of the overlay body remains required and
scanned. Every selected asset is checked regardless of file extension,
using strict UTF-8 or BOM-tagged UTF-16 decoding; unknown encodings fail closed.
UTF-32 and NUL-bearing text are rejected explicitly so BOM-less UTF-16 cannot
masquerade as UTF-8. References to an optional plugin, its marketplace alias, or its
configured repository and URL source coordinates are rejected. URL schemes and hosts
are normalized case-insensitively, and explicit default ports for Git, HTTP, HTTPS,
and SSH are treated as their equivalent implicit forms. SCP-style and host/path forms
receive the same host normalization, including bracketed IPv6 literals, while
repository paths retain their case-sensitive identity on case-sensitive hosts.
GitHub owner/repository coordinates are matched case-insensitively for `github`
marketplace declarations and `github.com` URL or SCP sources, matching GitHub's
repository resolution without weakening path matching for other hosts. SCP-style
marketplace sources must use a fully qualified dotted host or a valid bracketed IPv6
literal; ambiguous single-label hosts are rejected instead of case-folding ordinary
`owner/repository` or `label:value` text.
For a pinned marketplace, only the exact validated baseline identity is exempt; an
unknown or disabled sibling is still rejected. The exact iEvo exemption applies only
while
`enabledPlugins["ievo@ievo-skills"]` is `true`.
Every non-baseline entry in `extraKnownMarketplaces` is optional from the moment it is
declared, even before any plugin from it is enabled.

Treat bytes resolved from those marketplaces as mutable and review them before use.
If an optional plugin becomes a repository dependency, pin a reviewed immutable
revision, provide the equivalent Codex capability or a safe documented fallback, and
extend the pinned-marketplace and parity checks in the same pull request before adding
the required reference.

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
