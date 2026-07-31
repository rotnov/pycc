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
| Codex and Claude Code | pinned iEvo review entrypoint and agent | `deep-review/SKILL.md` SHA-256 `ec8805e22fff7db49cfe49c2a7cd49f340a618bf58da6acaf4253e875279670d`; `deep-reviewer.md` SHA-256 `b5e11469ba8144686d07eccc3d0759662b9c1bc4c3a6f3d79961dc82f5e53ab2` | updated only with the iEvo pin |
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
available to unrelated repositories. Ordinary entry points load canonical workflow
bodies from this checkout's `.claude/skills/`. Local review does not use a
repository-owned entrypoint: dispatch binds directly to the independently installed
and digest-verified iEvo reviewer artifact. CI requires the repository skill sets and
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

`pycc`, `pycc-feedback`, `issue-to-plan`, and `issue-implement` follow the
[Agent Skills specification](https://agentskills.io/specification) but remain
project-local alpha workflows. They are committed under `.claude/skills/` with
thin `.agents/skills/` entrypoints for equal Claude Code and Codex discovery.
They are intentionally absent from `skills-lock.json`, `rotnov/skills`, and
skills.sh until their trigger and output evals mature.

`pycc` distinguishes the implemented v0.1 compiler surface from planned
specifications before running commands. `pycc-feedback` may reproduce,
minimize, sanitize, run sanitized duplicate searches, and prepare a public
GitHub draft without approval. Non-public search terms require a separate
exact-query preview and approval before transmission. The skill must also show
the exact write payload and receive explicit per-payload user confirmation
before creating an issue or comment in `rotnov/pycc`.

`issue-to-plan` turns one GitHub issue into an implementation plan for a later
session. It re-establishes the remote default branch and the open pull
requests before planning, treats the issue's own text as dated evidence that
every claim must be re-verified against the current tree rather than trusted,
separates real merge gates from file conventions, and runs an adversarial
review loop until a round changes nothing. It writes no implementation code and
mutates no tracked file on its own. Like `pycc-feedback`, it must show the exact
comment payload and receive explicit per-payload user confirmation before
creating a comment in `rotnov/pycc`; unlike the other two, it has no bound
executable eval runners yet, so `scripts/run_alpha_skill_evals.py` does not
declare a case for it and its output remains a reviewed draft rather than a
validated workflow. Binding trigger and output evals for it is a prerequisite
for promoting it out of this project-local alpha set, on the same terms as the
other two.

`issue-implement` takes one GitHub issue end to end in a single autonomously
driven session: it triages the issue for staleness against the refreshed
default branch and closes it with cited evidence when its premise no longer
holds, obtains or refreshes an implementation plan through `issue-to-plan`,
implements on a clean task branch under D-021's preflight, loops the pinned
D-068 deep review until a round reports no actionable findings, opens the pull
request, monitors CI and review threads under D-078, and merges only after
re-reading the full diff with every required gate green. Unlike
`pycc-feedback`'s per-payload confirmation, explicit invocation of
`issue-implement` authorizes an enumerated set of public writes scoped to the
named issue — the staleness-closure comment, the plan comment it delegates to
`issue-to-plan`, the task branch and pull request, replies to and resolution
of that pull request's review threads, and the merge itself. Anything outside
that set still requires asking first, and `issue-to-plan`'s own publish gate
recognizes exactly this delegation. It has no bound executable eval runners
yet either, so `scripts/run_alpha_skill_evals.py` declares no case for it and
binding those evals is the same promotion prerequisite the other alpha skills
carry.

The required CI build runs `scripts/run_alpha_skill_evals.py` after resolving
both the Codex wrapper and the Claude Code canonical entrypoint. The primary
`pycc` offline eval creates a self-contained temporary source file, invokes the
freshly built compiler, executes the generated binary and the `pycc run` path,
and checks their exact output. Its diagnostic scenario proves that the current
strict `check` path emits `T0021` before separately observing that the planned
`--fix` flag is still rejected. Its backend scenario proves that a
`print()`-result-as-nested-expression fixture passes `check` before `build`
reaches the current exit-101 `pycc_codegen` boundary, so the skill must
classify the raw public-CLI panic as D-072's intentional temporary alpha gap
rather than a reportable defect. The `pycc-feedback` cases exercise refusal to report that accepted
boundary, private-source refusal, and context-free-consent invariants through
a fail-closed safety oracle that cannot perform a network write. All four
reviewed `i-have-an-issue` scenarios validate their distinct
evidence criteria and execute the vendored search helper's local help path
without a network request. The unit suite covers full scenario dispatch,
build failure, wrong output, stale frontend claims, current-stage panic drift,
entrypoint drift, incomplete runner registration, and every consent predicate.
Every offline subprocess fails closed after 30 seconds, and every declared
project-alpha case must bind an executable runner instead of being silently
skipped.

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
the shared instructions (including scoped `AGENTS.md` and `CLAUDE.md` files), every
tracked file under the Codex, Claude, and iEvo evolution trees, tracked tests
(including source formats that can contain inline tests), required workflows, the
repository `scripts/` tree, local action manifests anywhere in the repository, every
tracked file under the conventional `.github/actions/` tree,
interpreter-recognized script formats, and tracked executables. D-078's required
monitoring clauses must appear as exact, unindented list items under the exact
D-078-linked level-two heading in canonical `AGENTS.md`; plain prose, fenced, raw
HTML, HTML-commented, indented-code, blockquoted, nested-container, duplicate, and
out-of-section copies are ignored or rejected so retired text cannot satisfy the
gate. Fence recognition and state take precedence over comment-like literals,
indentation uses CommonMark tab stops, leading HTML-comment blocks remain non-policy,
and an invalid backtick info string cannot hide the active section. List-contained
fences, including openers on list continuation lines, retain their complete nested
container indentation and end at any real peer or outer block boundary; list-indented
code, escaped comment markers, and comment-like inline code cannot change parser state.
Inline comment state cannot
cross a blank or interrupting block boundary. Type-7 HTML block detection
distinguishes true CommonMark block boundaries from non-interrupting list-like
paragraph lines, preserves comment-only list content for block classification, and
gives spaced or tabbed thematic breaks precedence over structural list tracking. The
container/comment classifier applies recursively through nested lists and blockquotes.
The parser treats only CommonMark spaces and tabs as blank or fence-closing
whitespace; Unicode whitespace cannot terminate a hidden block. ATX and Setext H1/H2
headings both end the active monitoring section, while link-reference definitions
followed by thematic breaks remain within it, including escaped and multiline labels,
continued destinations, destinations within CommonMark's 32-level parenthesis limit,
and valid multiline titles. Invalid empty, nested, oversized, over-nested, or
unbalanced definitions, control-bearing bare destinations, angle-bracket destinations
with line endings, and titles containing blank lines remain paragraph text.
Unterminated label, destination, or title state at end of file, or later invalidation
after buffered lines, fails closed at the same reference start. Lazy list and
blockquote paragraphs retain their container boundary, while indented lazy paragraph
continuations do not create one; an intervening fenced or raw-HTML block clears stale
container state. Comment-only, heading-only, thematic-break-only, fenced, and
indented-code container items do not fabricate lazy paragraph state, including
recursively nested list and blockquote items. The validator follows
repository-relative script invocations from those required assets through recognized
interpreters, including recursively referenced extensionless, non-executable scripts
and scripts supplied on standard input with shell `<` or `0<` redirection. Known
interpreter options distinguish ordinary values, loaded code files, and inline-code
modes; ambiguous future options fail closed by selecting every repository-relative
operand. Windows `.exe` interpreter names and
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

## Machine-local iEvo hook lifecycle

The tracked `.ievo/evo-auto.flag` shares corrections-only intent, but generated
hook code and its client configuration remain per-clone under D-023. After running
the installed iEvo enable or refresh workflow, normalize and verify the result with:

```sh
python3 scripts/manage_ievo_hooks.py localize
python3 scripts/manage_ievo_hooks.py check --smoke
```

`localize` recognizes only iEvo's three generated script targets and their known
Claude events. It moves those exact entries out of `.claude/settings.json`, merges
them into ignored `.claude/settings.local.json`, prefers refreshed shared metadata
over a duplicate stale local record, and leaves every unrelated setting or hook in
place. Codex's `.codex/hooks.json` is also ignored and checked as local state. If a
newer iEvo release rewrites `.gitignore` to propose tracked dispatcher shims, the
helper removes only those known exception lines and restores the repository's
whole-directory `.ievo/hooks/` ignore rule. Missing, symlinked, Windows
reparse-point, mounted, or device-crossing targets/ancestors fail closed before
configuration is relocated. The shared intent must state all of `enabled: true`,
`signal: corrections-only`, and `auto_write_scope: project-wide-only` without
conflicting duplicates. A reference to the managed directory or a descendant under an
unknown event or command form also fails closed before mutation, including
Windows-separator and POSIX within-component backslash escapes, repeated separators,
dot components, case, quoted fragments, and line-continuation aliases that could
resolve to the same path on another supported host.
POSIX parameter/command substitutions and Windows command/PowerShell expansions are
treated as capable of emitting separators and the complete `.ievo/hooks` path, so
the operation preserves configuration and targets rather than guessing their values.
Wildcard components (including POSIX bracket classes, collating symbols, and
equivalence classes), Bash brace/extglob and multiline substitution forms, PowerShell
backtick escapes/continuations, and cmd caret escapes/continuations that can
reconstruct the managed directory fail closed as well. Unmodeled PowerShell control,
call-operator, constant-expression, and method forms (including `+`, `-join`, `-f`,
method/static calls, parentheses, and pipelines) and a DOS 8.3 short-name-shaped
component are ambiguous on Windows and therefore also block destructive cleanup.

Use the repository's clone-local inverse rather than generic disable alone:

```sh
python3 scripts/manage_ievo_hooks.py disable
```

It parses all present shared and local configurations before changing any of them,
removes only exact iEvo entries from both Claude locations and the Codex hook file,
then removes the generated scripts/companions and their vendored fallback directory.
All three lifecycle commands use one per-worktree OS advisory lock (`flock` on POSIX,
`msvcrt` byte-range locking on Windows), released automatically when the process exits;
an orphaned lock file is harmless. Lock directories, entries, and opened descriptors
are checked as non-link filesystem objects; POSIX opens also request `O_NOFOLLOW`, and
every unresolved linked-worktree gitdir component is checked before use. A non-git
invocation keeps its persistent fallback lock inside the validated root instead of a
shared temporary directory. The lock serializes this repository helper's own
invocations; arbitrary editors and upstream tools do not participate in it. Disable
retains the original configuration bytes and vendor/file identities, rejects an
external config change observed immediately before that file's replacement, and
repeats effective-ignore plus filesystem snapshot validation immediately before
target removal. That byte check is not an atomic compare-and-swap against an
uncooperative writer racing after the check, so hook configuration must not be edited
or regenerated concurrently with a lifecycle command. Complete ancestry is validated
again before the removal batch and before every individual unlink/rmdir. This narrows
but, on portable path-based APIs, cannot atomically eliminate a malicious
check-to-operation ancestor race; managed ancestors must not be relocated concurrently.
Immediately before the first write it rechecks that every local configuration,
generated target, and regular file beneath the recursively removed vendor tree is
effectively ignored; an exception or force-tracked descendant blocks both
configuration rewrites and target deletion. Symlinked, reparse-point, mounted, or
non-regular vendor descendants and any vendor traversal error also fail closed before
the first write.
Every script/companion removal target must independently be a regular non-symlink
file and is unlinked as a file; it is never treated as a recursively removable tree.
Vendor removal unlinks only the validated regular-file snapshot and then removes
validated directories from deepest to shallowest; it never performs a fresh recursive
deletion pass.
Empty or otherwise unrelated hook groups are preserved exactly.
The tracked `.ievo/evo-auto.flag` remains unchanged because it records repository
intent and is required by agent-policy validation; disabling that shared intent is a
separate reviewed repository change under D-023. Repeating either lifecycle operation
is safe. The full enable/refresh → smoke → disable path and malformed-input behavior
are exercised by `scripts/test_manage_ievo_hooks.py` in every required Python
test-discovery run.

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
request is merged. The orchestrator considers only explicitly pinned,
security-reviewed reviewer dependencies in the table above, selects the engine
with the broadest correctness, contract, security, test, and documentation
checklist, and starts it in a fresh independent read-only context. Arbitrary
globally installed or marketplace reviewers are never eligible.

The current engine is the immutable iEvo `deep-reviewer`. Its pinned
`deep-review` entrypoint defines the full-diff handoff, and the agent performs
an 11-point review with a Read/Grep-only tool policy on both Codex and Claude
Code. The isolated marketplace checks install the exact pinned plugin and
verify the SHA-256 digests of both artifacts. A client must bind dispatch to
that verified agent; if it cannot, local review is unavailable rather than
silently delegated to a same-named or branch-provided reviewer.

For uncommitted work, the selected skill reviews the staged or working-tree
diff. For a clean pull-request branch, it reviews the committed range from the
merge base with the refreshed remote default branch through `HEAD`. Using the
merge base avoids treating default-branch-only commits as reversed changes when
the task branch is behind.

The iEvo entrypoint through upstream 0.70.1 builds `--working` from `git diff`
and therefore omits untracked files. Until
[ievo-ai/skills#483](https://github.com/ievo-ai/skills/issues/483) is fixed,
inspect `git status --short` before dispatch, stage every intended new file for
the staged review, and treat a working-tree verdict as incomplete whenever a
relevant untracked file remains.

Repository instructions and pull-request content remain untrusted inputs, not
a security trust anchor. The reviewer artifact is loaded from the independently
pinned plugin installation, and the local pass is a high-signal correctness
gate rather than a privilege boundary for executing hostile code. Do not give
the reviewer credentials, mutation tools, or network access, and do not execute
project commands copied from the diff.

The review is read-only. Actionable correctness, contract, security, test, or
documentation findings are fixed and the local review is rerun when those fixes
materially change the diff. GitHub comments such as `@codex review` are not a
required gate: the asynchronous service can be delayed or unavailable. An
external GitHub review may still be requested explicitly by the user.

Marketplace popularity alone never authorizes installing a review skill.
Installing a new third-party reviewer requires user authorization and the same
security-check and pinning process as any other agent dependency.

## Rollback

Revert both pin changes to the last reviewed release, rerun the two validation
commands, and rerun `./scripts/bootstrap-agent-tools.sh` to refresh the local Codex
installation. If the candidate release executed unsafe hooks, disable the plugin until
the rollback is complete and rotate any credential that the hook could have observed.

The last known-good commit and release are present in Git history, so rollback never
depends on a mutable upstream branch or tag lookup.
