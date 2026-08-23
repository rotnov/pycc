# Contributor handbook — `tinker`

`tinker` is a small command-line tool. This handbook is the whole process: it governs how a
merged change becomes a release, and it is the only document a release operator reads.

## Change classes

Every merged change carries exactly one class, declared in its pull-request title prefix:
`chore:`, `feature:`, or `breaking:`. The class is what the rest of this handbook branches on,
so a change whose class is unclear is never guessed at — see the stop conditions.

The prefix is not enforced by any check. It is a claim by the author, re-derived by the
operator from the diff in step 2 below.

## Authorized edits

Running the release workflow for a merged change authorizes exactly these writes, without
further confirmation:

1. an entry appended to `CHANGELOG.md` under the heading for the change's own class;
2. a bump of the version line in `tinker/__about__.py`, for a `feature` or a `breaking` change
   only — a `chore` never moves the version;
3. the release tag, for a `breaking` change only;
4. a line in `notes/internal.md`, for any class.

Anything outside this set still requires asking first. In particular, editing a previous
release's entry, force-pushing over a tag, or touching `docs/` is never authorized here.

## Repository layout

- `tinker/` — the package. `__about__.py` holds the single version literal.
- `CHANGELOG.md` — the user-facing record, one heading per section (`## Added`, `## Changed`,
  `## Fixed`, `## Removed`).
- `notes/internal.md` — the operator's own log. Never published, never read by users.
- `docs/` — the manual. Regenerated from the package's docstrings by a separate job; never
  edited by hand and never part of a release.
- `.github/workflows/release.yml` — publishes on a tag. Nothing in this handbook edits it.

## Workflow

### 1. Collect the merge

Read the merge commit and the pull-request body. Record the title prefix, the pull-request
number, and the author. If the merge is a revert, stop: reverts are released by re-running the
previous release, not by this workflow.

### 2. Classify, then decide whether an entry is owed

Re-derive the class from the diff rather than trusting the prefix — a mislabeled pull request
is common and the prefix is not enforced anywhere.

- **`chore`.** No entry is owed. Record the merge in `notes/internal.md` and stop.
- **`feature`.** An entry is owed. Continue.
- **`breaking`.** An entry is owed, and the migration rule below applies. Continue.

A merge that changes only tests, only CI configuration, or only files under `docs/` is a
`chore` regardless of its prefix.

### 3. Draft the entry text

One sentence, present tense, naming the user-visible effect rather than the mechanism. Do not
name internal identifiers; a reader of the changelog has never opened the source. Do not write
two sentences — if the effect needs two, the change needs two entries, and two entries means
the merge should have been two merges.

### 4. Format the entry by class

- **`feature`:** `- <sentence> (#<pr>)` under `## Added`.
- **`breaking`:** `- **BREAKING** <sentence> (#<pr>)` under `## Changed`, followed by a
  `Migration:` line that says what a user must do.

Entries are appended in merge order within their heading, never sorted.

### 5. Bump and tag

Apply whichever of the version bump and the tag the class authorizes above. The bump is a
minor bump for a `feature` and a major bump for a `breaking` change. A patch bump is never
produced by this workflow — patch releases are cut by the maintainer directly.

### 6. Verify the release artefacts

Count the entries added to `CHANGELOG.md` and assert the count matches what the class owes:
`1` for a `feature`, `2` for a `breaking` (the entry plus its `Migration:` line), and `0` for
a `chore`. A mismatch means the format step was applied for the wrong class.

Then re-read the diff of `CHANGELOG.md` and confirm no previously released entry moved.

### 7. Announce

Post the release note to the project's discussion board. The note quotes the changelog entry
verbatim and adds nothing.

## Reporting

At the end of each month the operator files a summary. The summary is a table with one row per
release and these columns:

| column | value |
|---|---|
| release | the tag, or `—` when the class authorized no tag |
| class | the change's class |
| entries | the count the verification step asserted |
| migration | `yes` for a `breaking` change, `no` for a `feature`, `—` for a `chore` |

The `migration` column is filled from the class alone, never by re-reading the entry: an
operator who reads the entry instead of the class will mark a feature that happens to mention
an upgrade step as needing migration, and the summary stops being comparable across months.

## Stop conditions

Stop and report, rather than proceeding on a guess, when:

- the diff supports two classes equally well and the prefix does not settle it;
- a `breaking` change's migration cannot be stated as an action the user takes;
- the version bump the class authorizes conflicts with a version already tagged;
- the changelog heading the class's format step targets does not exist yet.

## Appendix: worked examples

**A `chore`.** A merge retitles a test helper. No entry, no bump, no tag; one line in
`notes/internal.md`; the monthly row reads `— | chore | 0 | —`.

**A `feature`.** A merge adds a `--json` output mode. One entry under `## Added`, a minor
bump, no tag; the monthly row reads `— | feature | 1 | no`.

**A `breaking` change.** A merge drops a deprecated flag. Two lines under `## Changed`, a
major bump, a tag; the monthly row reads `v3.0.0 | breaking | 2 | yes`.
