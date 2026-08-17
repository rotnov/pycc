# Installing a practice

Setting up something the project does not have yet — a decision log, a handoff
log — as opposed to repairing a failure. Same ladder, different starting point:
here the artefact type is chosen for a capability rather than against an incident.

The whole flow is yours to execute — the user says `/harden install adr` (or
asks in words) and the finished state is: files placed, rule injected, printed
after-steps performed, `--check` green.

```bash
uv run scripts/install-feature.py --list          # what is available, and why
uv run scripts/install-feature.py adr sessions    # install
uv run scripts/install-feature.py adr --check     # drift, missing rule block
uv run scripts/install-feature.py adr --uninstall # remove the rule, keep the files
```

## Step 0, unskippable: the practice may already be there

Before installing anything, establish that you would not be killing an
existing mechanism:

1. **Search the governance file** for the practice's vocabulary (decision,
   ADR, session, handoff, retrospective, postmortem, incident, …) — a
   hand-written rule means the project already runs its own edition, and the
   installer does not detect that. The list is examples, not a filter: search
   for the *domain* each practice occupies, not only its own preferred nouns —
   a project that journals process mistakes under another name is still a peer
   of the incident journal.
2. **Look at the target directories** (`docs/decisions/`, `docs/sessions/`) —
   existing content means existing practice, whatever the governance file says.
3. **Read the project's decisions log for contradictions** — a feature can
   collide with policy, not just with files. Measured: a project whose
   accepted autonomous-operation decision forbids asking the owner questions
   is a project where the `active-learning` feature is wrong BY THEIR LAW,
   not redundant.

Run the audit for **everything requested, before writing anything** — then act
per practice, not per invocation:

- a practice with no hit installs normally;
- a practice with a hit is **skipped whole** (partial installs of a practice —
  "just the generator", pointed at their log — are how mechanisms get killed),
  and the final report names each skip with its evidence: what exists, where,
  and which of their laws it rests on;
- their edition wins by default; replacing it is the user's explicit call,
  never a side effect. `--force` on the file layer (unpack, seeds) and the
  user's typed word on the flow layer ARE that call — and even then force
  replaces the *mechanism*, never their *data*: existing entries, logs and
  history stay where their consumers read them, migrated additively or not at
  all;
- a hit that is a *peer system* — a live mechanism of the same domain with its
  own writers and readers — deserves more than a skip line: map it and
  negotiate (docs/todo/T-032 tracks that protocol; until it lands, report the
  peer and stop).

The near-miss that wrote this step: a field project with 140+ decisions and
its own generator, one `install adr` away from a collision. The refinement
came from the same field: one colliding practice used to veto a whole
multi-practice install, which punished the parts that had no conflict.

Uninstall gets the mirror audit: confirm the fenced block you are removing is
the installer's (`<!-- harden:… -->` markers), never a hand-written rule that
merely resembles it — and read the dependents warning the installer prints
before deciding the removal is safe.

## What a feature is

Files, an optional gate, and **one routing line** in the governance file: where it
lives, what the format is, what regenerates it.

The line is deliberately not an explanation. Pasting mechanics into a file that
every session reloads is the move that grew one measured `AGENTS.md` to 158 KB,
and `rule` is the artefact type with the worst record in the corrections corpus.
The reference project shows both outcomes side by side: its ADR line is 298
characters and routes to a template, its session-log paragraph is 1101 characters
of pasted mechanics — and that copy has already diverged from the README it
duplicates.

So: **mechanics live in the feature's own README.** A feature that brings neither
files nor a dependency is documentation, and there is nothing to install — the
installer's job is not to generate paragraphs in a governance file.

A feature may legitimately have **no files of its own** if it `requires` another:
it then configures behaviour over that feature's structure and is unusable
without it, which the dependency states. `active-learning` is the worked example
— its rule routes into `docs/decisions/`, so it pulls in `adr`.

The limit is enforced rather than advised — a test fails if any bundled rule
exceeds 600 characters.

## What the installer will and will not touch

- **Injection is fenced** by `<!-- harden:<name> -->` markers, so a second run
  replaces instead of duplicating and `--uninstall` can find it again.
- **One governance file.** With both `AGENTS.md` and `CLAUDE.md` present it writes
  to `AGENTS.md` only — every harness reads it, and writing to both makes two
  copies of one rule that then drift apart.
- **Existing content is preserved**; the block is appended, never templated over.
- **Gates are printed, not wired in.** A pre-commit config belongs to the project;
  overwriting one is worse than leaving a gate uninstalled.
- **`--uninstall` removes the rule and keeps the files** — by then they hold real
  decisions and real snapshots.
- **`requires` pulls a dependency in first**, and says which it pulled. A rule
  routing into another feature's directory is broken without it: the line points
  at a path that does not exist, which is worse than no line, because it still
  reads as an instruction.
- **It does not notice a rule you wrote by hand.** Observed on the first real
  install: this repository already had the ADR rule written manually, and
  installing `adr` added a marked copy beside it — two versions of one rule, free
  to drift. Check the governance file after installing into a project that was
  already using the practice.

## Adding a feature

A bundle is `assets/features/<name>/` holding `feature.json`, `rule.md`, and the
files it installs:

```json
{
  "name": "adr",
  "summary": "one line for the picker",
  "why": "what breaks without it, in two or three sentences",
  "requires": ["another-feature"],
  "files": {},
  "init":  { "README.md": "docs/decisions/README.md" },
  "gate": "the command to add to pre-commit, or null",
  "after": ["a command to run once, to initialise"]
}
```

`files` may legitimately be empty when the practice's tooling rides inside its
own skill (the adr generator lives in `skills/adr/scripts/` and is referenced
by path) — one source, nothing copied, nothing to drift. `init` files seed
**only where nothing exists**: a file already present in the project survives
every install, the first one included — the installer prints "kept — the
project's own". (Stated this precisely because a field audit read the earlier
wording as "first install overwrites" and reported a destruction that the code
does not perform.)

**Everything in `files` is overwritten on reinstall**, so nothing there may hold
project data — a stale script must not survive an update.

That constraint decides where generated content goes: **not into a file the
bundle ships**. The decisions log keeps prose in `README.md`, which the bundle
owns and updates, and the generated table in `INDEX.md`, which the bundle never
touches. A generator writing into a shipped README would put the two in the same
file, and the next install would overwrite whichever the project had changed.

**The rule lives in `rule.md`, not in the JSON.** It used to be an escaped string
inside the manifest, which made the one part a human edits the one part hardest
to read. Prose belongs in a prose file; the 600-character limit still applies.

## The manifest of what is installed

Installing writes `.harden/installed.json` in the target project: per feature,
the digest of the rule as installed, the files it placed, and its dependencies.

It exists because `--check` cannot work without it. Comparing bytes against the
bundle cannot distinguish a feature that was never installed from one whose files
were deleted — both look identical from outside. With the manifest, the first
answers *not installed here*, the second *missing*, and a rule edited in the
bundle since installation is caught by the digest.

`--uninstall` drops the entry; the files stay, because by then they hold real
content.

## What uninstall does, and does not

It removes the fenced rule block from the governance file and the entry from the
manifest. It leaves the files, and it leaves dependencies alone — removing `adr`
because `active-learning` is going away would delete a routing line the project
may still be using.

It **warns when something still requires what is being removed**. Install
resolves dependencies forwards; removal has to look backwards, or a rule that is
still installed keeps pointing into a structure whose own rule has just been
taken out — silently. The warning does not block: you may know exactly what you
are doing.

Before adding one, answer the question the ladder asks of any artefact: **what
detects that this practice is being skipped?** A static command means the bundle
should carry a gate. Nothing mechanical means the honest bundle is files plus a
route, and the practice will depend on a carrier — a step inside a procedure that
runs anyway. If neither exists, the feature will be installed and ignored, which
is worse than not shipping it, because the routing line stays and is read
forever.

Ship the working implementation, not a copy of it: bundled scripts are this
repository's own, and a test fails when they drift.
