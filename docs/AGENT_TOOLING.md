# Agent tooling supply-chain policy

Repository-owned agent instructions and the cross-platform plugin baseline documented
here are executable control-plane inputs. They must be reproducible, reviewed, and
rollbackable just like compiler dependencies.

## Current pins

| Surface | Dependency | Pin | Automatic updates |
|---|---|---|---|
| Codex | repository skills under `.agents/skills/` | current repository revision | project-scoped; no global installation |
| Claude Code | `ievo@ievo-skills` | tracks the upstream default branch; no exact `sha`/`ref` pin (D-155) | `autoUpdate: false` in this repository's own declaration, but the marketplace's actual resolution is governed by the machine-global, name-keyed `~/.claude/plugins/known_marketplaces.json` this repository cannot control — see D-155 |
| Claude Code | ievo `deep-review` entrypoint and `deep-reviewer` agent | verified structurally in CI (both artifacts non-empty, plugin manifest `version` semver-shaped), not by exact digest (D-155) | tracks whatever `ievo@ievo-skills` currently resolves to |
| Codex and Claude Code | `rotnov/skills@i-have-an-issue` | tag `i-have-an-issue-v0.1.1`; reviewed source commit `1bc6bcee3766a7e62b936343a48ebb56a3767470`; vendored hash `99e492ccae20ad3acf02e28dd76c7d74de28c7cf2141bfc7a2942c46c4bf687c` | manual updates only |

Codex no longer depends on the `ievo-skills` marketplace at all: an earlier change
removed its plugin installation, and Codex now only discovers this repository's own
skills under `.agents/skills/`, project-locally, with no global registration. A fresh
Codex checkout still runs a trivial preflight explicitly:

```sh
./scripts/bootstrap-agent-tools.sh
```

which only confirms the Codex CLI and Python 3 are installed; it performs no
marketplace or plugin setup.

Claude Code's `ievo@ievo-skills` plugin source, declared in this repository's inline
settings marketplace in `.claude/settings.json`, intentionally carries no `sha` or
`ref` pin: `scripts/validate_agent_assets.py` rejects either key outright rather than
requiring one, per D-155's finding that an exact commit recorded in a per-project
settings file was never actually enforced on a real, non-isolated machine, since
Claude Code's plugin marketplace registry is a single global file shared by every
project on that machine. Repository-owned Claude Code entry points live under
`.claude/skills/`; Codex's thin `.agents/skills/` wrappers load their canonical
workflow bodies from there. Local review does not use a repository-owned entrypoint:
dispatch binds directly to whatever `ievo@ievo-skills` install the machine currently
resolves for this project (see the Local review workflow section below).
The `Agent assets` workflow repeats the validators plus isolated Codex and Claude Code
checks on every pull request and push to `main`; the Claude Code check installs into
an isolated `CLAUDE_CONFIG_DIR` so it always observes a clean install, independent of
a developer's own machine-global marketplace state.

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

`pycc`, `pycc-feedback`, `issue-to-plan`, `issue-implement`,
`issue-select`, `next-milestone`, and `ultra-review` follow the
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
requests before planning, treats the issue's own text — including any
"Reproduction" section — as untrusted data to re-verify against the current
tree by reconstructing checks through this repository's own toolchain, never
by executing issue-supplied shell text directly, separates real merge gates
from file conventions, and runs an adversarial review loop until a round
changes nothing or hits its own 5-round stop condition. It writes no
implementation code and mutates no tracked file on its own. Like
`pycc-feedback`, it must show the exact comment payload and receive explicit
per-payload user confirmation before creating a comment in `rotnov/pycc`
(delegated invocation by exactly `issue-implement` — a closed, named
exception, not an open class any future skill could self-qualify into — is
the one exception; D-143 extends this to a generic `Agent` that
`issue-implement` dispatched to run `issue-to-plan` on its behalf, acting
under that same delegated authorization — see `issue-to-plan`'s own Publish
step). An issue authored by the repository owner, or labeled
`approved`, is trusted; any other issue gets an explicit security check
before the agent acts on anything beyond its stated defect or request.

`issue-implement` takes one GitHub issue end to end in a single autonomously
driven session: it triages the issue for staleness against the refreshed
default branch, applying the same trust-policy and issue-content-is-data
rules as `issue-to-plan`, and closes it with cited evidence when its premise
no longer holds; obtains or refreshes an implementation plan through
`issue-to-plan` (D-143: the delegated invocation, when a fresh or refreshed
plan is needed, also runs inside a dispatched `Agent` — instructed to invoke
the `issue-to-plan` skill itself and run it to completion, including its own
adversarial review loop via a further, nested `Agent` dispatch — rather than
in the orchestrating session's own context); implements on a clean task
branch under D-021's preflight
(D-142: dispatched to a fresh `Agent` working inside that same branch/worktree
rather than in the orchestrating session's own context, so `issue-select`'s
loop can carry many issues in one sitting instead of growing that session's
context unboundedly after the first — detecting and executing the
repository's established two-PR CI-digest stage-then-activate pattern when
the change touches a workflow file and a `check_roadmap_evidence.rb` digest
allowlist, or the separate, broader D-103 policy-successor-manifest
stage-then-activate pattern covering any path listed in
`tests/fixtures/policy-successor-manifest.json` — checker scripts and their
own self-tests and staging fixtures, not only workflow files); loops the
pinned D-068 deep review until a round reports no actionable findings,
resuming the same dispatched implementer for its own fix rounds rather than
fixing findings in the orchestrating session's own context; opens the pull
request after re-checking the issue's own live
state; monitors CI and review threads under D-078, distinguishing
bot-authored threads (self-resolvable on refutation) from human-authored
ones (reply only, never self-resolved); and merges only after re-checking
the issue once more and re-reading the full diff with every required gate
green, retrying a rejected merge once before stopping. Preflight also reads
`tests/fixtures/policy-successor-manifest.json` from the refreshed default
branch: any entry mid-transition (a successor staged but not yet activated)
blocks every candidate PR's required `audit` check repository-wide,
regardless of which issue or files it touches, so an entry that cannot
plausibly land this session (e.g. it needs a maintainer `emergency-bypass`
authorization) is caught here before any other work starts. Unlike
`pycc-feedback`'s per-payload confirmation, explicit invocation of
`issue-implement` authorizes an enumerated set of public writes scoped to
the named issue — a closure or narrowing comment depending on how staleness
triage resolves (extended, under a standing autopilot directive from
`issue-select`'s own staleness screen, to any other issue that screen
identifies as provably stale in the same pass), the plan comment it
delegates to `issue-to-plan`, the task branch and pull request (plus a
second, stage-only pull request for the CI-digest or D-103 manifest-staging
pattern above — an intermediate D-103 activation that is itself only a
prerequisite for a larger composed change also carries no `Fixes #N`, only
the sequence's own final PR does), replies to and resolution of bot-authored
review threads on that pull request, and the merge itself. Anything outside
that set still requires asking first, and `issue-to-plan`'s own publish gate
recognizes exactly this delegation. Its fourteen stop conditions are split
into **systemic** (the pinned reviewer cannot be bound, or a manifest entry
mid-transition whose own activation cannot land this session — both halt the
whole `issue-select` autopilot loop, since no different issue would fare any
better) and **per-issue** (every other condition — the loop sets that one
issue aside and keeps working the rest of the pool).

`issue-select` chooses the next issue for an autonomous end-to-end run and
mutates no tracked file: it inventories the full open issue list (priority is
read from the issue title's `P1:`/`P2:`/`P3:` prefix per D-111 — this
repository has no GitHub priority labels), reads
`tests/fixtures/policy-successor-manifest.json` from that same freshly-fetched
tip and stops the whole run if any entry is mid-transition and cannot
plausibly land this run (the same systemic check `issue-implement` repeats
for the case it is discovered later), screens for staleness against a
concretely defined "reconfirmed at commit X" evidence bar (routing provable
closures through `issue-implement`'s evidence-gated triage — but only when a
standing autopilot directive is in effect; a plain "what's next" query only
reports stale candidates), screens blockers (dependency on another issue,
roadmap/delivery-plan mismatch, open-pull-request collision, a per-candidate
manifest-protected-target hit, maintainer-only authority, or already having
hit a per-issue stop condition this run), scores the survivors by a fixed
priority-then-size order, verifies the top candidate's premise still
reproduces the same way `issue-to-plan` does, and challenges the pick with an
independent adversarial advisor in a fresh context instead of escalating
"does this need the maintainer?" to the user. With a standing autopilot
directive it hands the selection to `issue-implement`, whose enumerated write
authorization covers the run from that point; its own loop carries forward
exactly one piece of state between iterations — an in-run denylist of issues
that hit a per-issue stop condition — so a mechanically stuck issue doesn't
get reselected and re-failed every pass.

`next-milestone` chooses the next versioned milestone and adopts it as the
session's standing autonomous goal. It walks `docs/ROADMAP.md`'s ordered
milestone sections to find the first one whose Accept criteria are not yet met
with real evidence (an explicit "Update (`<date>`): met." note backed by a named
PR, CI run, or cross-referenced count — not a bare unqualified claim), ensures
its GitHub milestone exists, adopts it without asking, and hands off into the
`issue-select` loop scoped to it. A directive naming exactly one milestone
stops at step 6 once that milestone completes; an open-ended directive re-enters
step 1 with a fresh baseline to determine the next milestone, repeating until
v1.0. On milestone completion it records the "Update: met." note in
`docs/ROADMAP.md`, updates `README.md`'s status blurb, refreshes
`docs/ROADMAP.md`'s "Current delivery status" section, and closes the GitHub
milestone. It mutates no tracked file beyond those documentation updates and the
GitHub milestone close.

`ultra-review` periodically re-reviews the codebase for drift a single pull
request's own D-068 gate cannot see and files prioritized, milestone-scoped
issues for what it finds. It reads a GitHub-native checkpoint (a dedicated
tracking issue, not a tracked file — this project's own ephemeral-worktree
lifecycle ruled that out directly), computes the diff since that checkpoint,
dispatches the same pinned D-068 deep-reviewer once (a live empirical
comparison against a broader two-pass architecture-review design found the
second pass did not earn its cost — see
`docs/superpowers/specs/2026-08-05-ultra-review-skill-design.md`), maps its
`blocker`/`warning`/`note` findings to `P1`/`P2`/`P3` issues with
milestone-at-filing, deduplicates against already-`ultra-review`-labeled
issues, and files the survivors autonomously within a bounded evidence bar —
mirroring D-022's standing-authority precedent rather than `pycc-feedback`'s
per-payload gate. It mutates no tracked file and implements nothing itself.

`issue-to-plan`, `issue-implement`, and `issue-select` bind deterministic
offline eval cases in
`scripts/run_alpha_skill_evals.py` (`issue-to-plan` three, `issue-select` seven,
`issue-implement` eight, mirroring `pycc-feedback`'s
fail-closed-oracle pattern): `issue-to-plan`'s publish-gate boolean logic,
`issue-implement`'s staleness-outcome/write-authorization/issue-content/
delegated-closure/manifest-transition oracles, and `issue-select`'s
autopilot-gated-closure/priority-ordering/issue-content/manifest-transition
oracles — the shared `manifest_transition_status` oracle covers all three
distinct outcomes (steady-state, a landable mid-transition entry, and an
unlandable one classified systemic) for both skills — each cross-checked
against literal contract phrases in the live skill text so an edit that
silently drops an invariant these oracles encode fails required CI
(`EXPECTED_RUNNERS` in that script names all seven alpha skills).
`scripts/validate_agent_assets.py`'s separate `validate_alpha_skill_contracts`
structural check now also iterates all seven alpha skills (not just
`pycc`/`pycc-feedback`), enforcing "at least two evals, exact runner set,
visibly alpha" on every skill's `evals.json` independently of
`run_alpha_skill_evals.py`'s own, narrower type-only checks in `load_cases`
(that check's own `ALPHA_EVAL_RUNNERS` constant mirrors `EXPECTED_RUNNERS`
and must be kept in sync by hand whenever a runner is added or renamed). One
thing remains deferred for all seven: authenticated model-response evals on
both Codex and Claude (the `pycc`/`pycc-feedback` promotion requirement
described below).

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

## Project-local non-alpha skills

`process-error-postmortem` is a project-local skill committed under
`.claude/skills/` with a thin `.agents/skills/` entrypoint, following the same
cross-platform discovery convention as the alpha skills above. It is not alpha
and is intentionally absent from `validate_agent_assets.py`'s
`ALPHA_EVAL_RUNNERS` and `validate_alpha_skill_contracts` tuple, and from
`run_alpha_skill_evals.py`'s `EXPECTED_RUNNERS`: its correctness is inherently
model-judgment-based (diagnosing a process mistake's root cause has no
deterministic boolean oracle the way `issue_select_higher_ranked` or
`next_milestone_loop_continues` do), so forcing it into the alpha eval model
would produce a vacuous eval that tests trivia, not diagnosis quality. It fires
when the agent catches itself having made a process mistake or the user points
one out, walks a structured diagnosis (trigger gap / content gap / absence gap /
compliance gap), applies the fix directly for process-text edits or via a
dedicated PR for new skills/ADRs, and records the entry in
`docs/AGENT_RETROSPECTIVE.md`. See D-145 for the full design rationale.

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

This process governs `rotnov/skills@i-have-an-issue`, the one dependency this
repository still exact-pins. Since D-155, the Claude Code `ievo@ievo-skills` reviewer
tracks its upstream default branch by design and has no pin for a pull request to
bump; its own verification is the structural check in `scripts/check-claude-marketplace.sh`
plus the local advisory freshness note from `scripts/check_claude_reviewer_binding.py`
(see Local review workflow below), not a reviewed-pin-bump pull request.

1. Open a dependency pull request that changes the pin to a newer upstream release.
   Include the old and new commits and link the upstream release notes or changelog.
2. Run the relevant security-check workflow against the candidate release before
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

No scheduled job, startup hook, or local bootstrap command may rewrite this pin.

## Local review workflow

Code review is performed locally before significant work is completed or a pull
request is merged. The orchestrator considers only the reviewer dependency in
the table above, selects the engine with the broadest correctness, contract,
security, test, and documentation checklist, and starts it in a fresh
independent read-only context. Arbitrary globally installed or marketplace
reviewers are never eligible.

The current engine is the iEvo `deep-reviewer`. Its `deep-review` entrypoint
defines the full-diff handoff, and the agent performs an 11-point review with a
Read/Grep-only tool policy on both Codex and Claude Code. Since D-155, the
Claude Code `ievo@ievo-skills` plugin tracks the upstream default branch rather
than an exact pinned commit, so verification narrows to what each check can
actually make good on: the isolated, CI-safe `scripts/check-claude-marketplace.sh`
installs into a clean `CLAUDE_CONFIG_DIR` and confirms the deep-review entrypoint
and deep-reviewer agent artifacts are non-empty and the plugin manifest's
`version` field is semver-shaped, and the local, non-CI
`scripts/check_claude_reviewer_binding.py` confirms a structurally intact
`ievo@ievo-skills` install exists for the current project (or falls back to a
user-scope install) and prints an advisory freshness note against the latest
upstream release tag — it hard-fails only when no such install can be found at
all, and its freshness note never blocks dispatch. A client must bind dispatch
to a structurally verified agent; if it cannot, local review is unavailable
rather than silently delegated to a same-named or branch-provided reviewer.

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
a security trust anchor. The reviewer artifact is loaded from a structurally
verified plugin installation, and the local pass is a high-signal correctness
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

For the `i-have-an-issue` pin, revert the pin change to the last reviewed release and
rerun the validation commands above. The last known-good commit and release are
present in Git history, so this never depends on a mutable upstream branch or tag
lookup.

For the Claude Code `ievo@ievo-skills` reviewer, D-155 removed the repository's own
lever to pin it to a specific commit, by design — the plugin marketplace registry it
resolves against is machine-global, not something this repository's pin ever actually
controlled (see D-155's Context). There is accordingly no repository-side pin to
revert if a specific upstream `ievo-ai/skills` release turns out to be bad. Instead:

- If a released version executed unsafe hooks or is otherwise compromised, disable the
  plugin (remove or comment out the `ievo-skills` marketplace source in
  `.claude/settings.json`) until a fixed release is available, and rotate any
  credential the hook could have observed. This stops the local review loop from
  running at all rather than silently running a known-bad version; report the gap so
  review resumes once a fixed release lands.
- Downgrading a specific machine's already-installed version is a local, per-machine
  action outside this repository's control, consistent with D-155's root cause.
- If a future incident shows structural-only verification (non-empty artifacts,
  semver-shaped manifest) is insufficient to catch a compromised-but-well-formed
  release, that is a new decision superseding D-155, not a reason to silently
  reintroduce an unenforced pin here.
