# `docs/DECISIONS.md` decomposition — design

**Goal:** Replace the single, ever-growing `docs/DECISIONS.md` (1498 lines, 125
long-form entries, 139 index rows as of this writing) with one file per decision under
`docs/decisions/`, plus a *generated* index — so no future decision can silently omit
its own index row, and reading, diffing, or blaming one decision no longer touches the
other 124.

**Architecture:** Mechanical split of the current file on its real `## D-XXX: ...`
entry boundaries into `docs/decisions/D-XXX-<slug>.md` files, each carrying a small
frontmatter block (`id`, `title`, `status`) ahead of its unchanged prose body. A
checked-in generator script rebuilds `docs/decisions/README.md`'s index table from
that frontmatter; a companion checker (mirroring this repository's existing `scripts/
check_*` convention) fails if the committed index and the generator's fresh output
disagree. Every inbound `docs/DECISIONS.md#d-xxx-...` reference across the repository
is rewritten to point at the new per-decision file directly (no anchor needed — each
file holds exactly one decision).

**Tech stack:** No new dependencies. Python or Ruby for the generator/checker script
pair (matching whichever sibling script this repository's own convention favors for a
new `scripts/check_*.{py,rb}` — see `scripts/check_roadmap_evidence.rb` and
`scripts/validate_agent_assets.py` as the two closest existing shapes), plain Markdown
+ YAML frontmatter for the decision files themselves.

## Global Constraints

- Precedent this design mirrors: [D-130](../../DECISIONS.md#d-130-decompose-the-session-handoff-log-into-per-session-files),
  which replaced `docs/SESSION_LOG.md` with `docs/sessions/` (one file per checkpoint).
  Reuse its migration rigor (mechanical split on real boundaries, byte-for-byte
  round-trip verification, relative-link rebasing for moved sibling references, the
  retired file fully removed rather than stubbed) — but not its no-index conclusion;
  see "Why not just copy D-130" below.
- `docs/DECISIONS.md`'s own rule, unchanged by this migration: "Changing an accepted
  decision requires a new entry, not an edit." A decomposition that moves an entry's
  file location is not an edit to its content — the prose body must survive the split
  byte-for-byte.
- Every normative documentation claim should be enforceable by a test or CI check
  (`docs/SPEC.md`'s own doc-lifecycle rule) — the generated-index property is exactly
  this: the checker is the enforcement.
- `AGENTS.md`'s "Keep documentation current": every cross-reference this migration
  touches gets updated in the same change, not left pointing at a retired anchor.

---

## Current state (measured, not estimated)

- `docs/DECISIONS.md`: 1498 lines, 125 `## D-` long-form entries, 139 index-table rows
  (some early entries are `proposed`/index-only, not yet promoted to long-form — the
  gap between 125 and 139 is real and this design does not change that).
- Status vocabulary in actual use: 123 `accepted`, 12 `proposed`, 4 `superseded`
  (confirmed by grep; `rejected`/`deprecated` appear in the format line's own stated
  vocabulary but are not currently used by any entry).
- IDs are already zero-padded in text (`D-001` … `D-148`), so lexical filename sort
  already matches numeric decision order with no extra padding logic needed.
- 40 inbound `DECISIONS.md#d-xxx-<slug>` references across 12 files. Three prefix shapes
  are in use, corrected here after the implementation's own reference-rewriter was
  caught by review matching only the first: a literal `docs/` segment, optionally
  preceded by one or more `../` (`docs/DECISIONS.md#...`; `../../../docs/DECISIONS.md#...`
  from three-levels-deep `.claude/skills/*/SKILL.md` files); a same-directory `./` with
  no `docs/` segment at all, from files already inside `docs/` itself
  (`./DECISIONS.md#...`, e.g. `docs/PYTHON_STANDARDS.md`, `docs/DELIVERY_PLAN.md`); and a
  bare `../` or `../../` with no `docs/` segment, from files inside a `docs/`
  subdirectory (`../DECISIONS.md#...` from `docs/sessions/`; `../../DECISIONS.md#...`
  from `docs/superpowers/specs/`). A rewriter matching only the first shape would
  silently leave 9 of the 40 real references unrewritten, becoming dead links the moment
  `docs/DECISIONS.md` is deleted.

  | File | References |
  |---|---|
  | `AGENTS.md` | 18 |
  | `docs/superpowers/plans/2026-08-05-ultra-review-skill.md` | 3 |
  | `.claude/skills/next-milestone/SKILL.md` | 3 |
  | `docs/superpowers/specs/2026-07-28-v0-2-collections-generics-design.md` | 2 |
  | `docs/sessions/README.md` | 2 |
  | `docs/PYTHON_STANDARDS.md` | 2 |
  | `docs/DELIVERY_PLAN.md` | 2 |
  | `.claude/skills/ultra-review/SKILL.md` | 2 |
  | `.claude/skills/issue-select/SKILL.md` | 2 |
  | `.claude/skills/issue-implement/SKILL.md` | 2 |
  | `docs/sessions/2026-07-31-01-...md` (one historical entry) | 1 |
  | `docs/ROADMAP.md` | 1 |

  A dated `docs/sessions/` entry is historical record, not live prose (per this
  project's own carve-out for append-only dated files) — its one reference is rewritten
  too, since a dead link is a defect regardless of the surrounding file's mutability,
  but its *narrative* is not otherwise touched.

- `docs/SPEC.md:18`'s own DECISIONS.md row: a single giant parenthetical listing the
  topic of every decision ever made, grown alongside the file itself into the most
  visibly "noise over info" artifact of the current design — this is fixed as part of
  the same change, not filed as a separate follow-up, since it is a direct, obvious
  symptom of the exact problem this migration solves.

## File structure

`docs/decisions/D-XXX-<slug>.md`, one per decision, `XXX` the existing zero-padded ID
verbatim. `<slug>` is a short (≈5-8 word) kebab-case rendering of the entry's own
heading title, truncated for filesystem friendliness — not GitHub's full computed
anchor slug, which for several entries in this file already runs past 80 characters.
Each file:

```markdown
---
id: D-XXX
title: <the heading's own title text, unabbreviated>
status: accepted
---

# D-XXX: <title>

- Status: accepted
- Context: ...
- Decision: ...
- Alternatives: ...
- Consequences: ...
```

The `- Status: ...` line inside the body is preserved unchanged (some entries' Status
lines already carry narrowing references, e.g. D-066's after D-130 — that convention is
untouched); the frontmatter `status` is a machine-readable mirror of the same fact,
sourced from the *current* index table row for that ID during migration, not
re-derived from the prose (the prose is occasionally more nuanced, e.g. "accepted
(closes issue #242; D-130 is reserved by concurrent PR #313)" — frontmatter takes the
first word only, `accepted`, which is all the generated index needs).

## Why the index is generated, not dropped (the deliberate deviation from D-130)

D-130 replaced `SESSION_LOG.md`'s single file with `docs/sessions/` and *no* index —
sessions resume by `ls`-ing the directory and reading the newest few files, because
that file was read chronologically and a hand-maintained index would just be the same
shared-mutable-tail problem restated one level up (D-130's own Alternatives section
makes this argument explicitly).

`docs/DECISIONS.md` is read differently: the 40 measured inbound references above all
look up a decision **by ID or topic**, not by recency — the table is how a reader (or
an agent resolving the next free `D-` number, a task this session has done many times)
finds the right entry without opening 125 files. Dropping it in favor of `ls` would
regress that workflow. But hand-maintaining it reintroduces exactly the conflict class
D-130 closed: two sessions each adding a new decision file, then both hand-editing the
same table's tail, is the identical shared-mutable-tail collision D-130's own Context
section describes hitting during its own migration.

The resolution: make the index a **generated artifact**, never hand-edited.
`scripts/generate_decisions_index.py` reads every `docs/decisions/D-*.md` file's
frontmatter, sorts by numeric ID, and writes `docs/decisions/README.md`'s table
deterministically. `scripts/check_decisions_index.py` (or a `--check` mode of the same
script) re-generates in memory and fails with a diff if the committed file disagrees.
Two concurrent sessions adding different decision files now produce a merge conflict,
if any, only in the generated table — resolved by re-running the generator, never by
hand-merging prose. This is the direct fix for the exact class of gap this session's
own `ultra-review` dogfood run just found live (rotnov/pycc#363: a new decision's index
row simply forgotten) — it becomes structurally impossible to forget, rather than
merely less likely.

## Migration process

Mirrors D-130's own rigor:

1. Split `docs/DECISIONS.md` on its real entry boundaries (`^## D-\d+:` heading lines,
   confirmed the reliable delimiter the same way D-130 confirmed it for `SESSION_LOG
   .md`'s `^## YYYY-MM-DD` lines) into 125 bodies.
2. For each, look up its `status` from the *current* index table's matching `| D-XXX |
   ... | <status> |` row (only 125 of the 139 rows have a matching long-form body —
   see "Out of scope" below for the other 14).
3. Write each `docs/decisions/D-XXX-<slug>.md` with the frontmatter block prepended and
   the body otherwise byte-identical to the original section.
4. Round-trip check: reassemble every generated file's body (stripping the added
   frontmatter) in ID order and diff against the original file's long-form-entries
   section. Zero-diff required before anything is deleted.
5. Generate `docs/decisions/README.md` via the new generator script.
6. Rewrite all 40 inbound references (both link forms) across the 12 files identified
   above, from `[...](<rel-prefix>docs/DECISIONS.md#d-xxx-<old-slug>)` to
   `[...](<rel-prefix>docs/decisions/D-xxx-<new-slug>.md)`, preserving each reference's
   existing relative-path depth.
7. Delete `docs/DECISIONS.md` outright (no stub — matching D-130's own precedent for
   `SESSION_LOG.md`).
8. Fix `docs/SPEC.md:18`'s row: replace the giant enumerated parenthetical with a short
   pointer to `docs/decisions/README.md`, and repoint the row's own link.
9. This migration is itself a decision: it gets its own new entry (next free `D-`
   number, re-resolved at PR-open time per this project's existing convention) — filed
   directly as `docs/decisions/D-1NN-<slug>.md` in the new format, the first entry the
   new structure carries.

## Alternatives considered

- **Copy D-130 exactly (no index, `ls`-only discovery)** — rejected per "Why the index
  is generated" above: the two files have different real read patterns, and dropping
  the index would regress an actively-used topic/ID lookup workflow.
- **Range-grouped files** (`D001-050.md`, `D051-100.md`, ...) — rejected (user's own
  call): fewer files, but arbitrary boundaries that track nothing real about the
  decisions themselves, and the file keeps growing without bound inside each range
  bucket the same way the single file does today, just three times slower.
- **Topic-grouped files** (`ci-governance.md`, `compiler-runtime.md`, ...) — rejected:
  most decisions in this log span more than one area (a compiler change with its own
  CI-gate implication, for instance), so grouping would need a subjective call per
  entry and would not obviously reduce cross-reference churn versus one-file-per-ID.
- **Hand-maintained index, no generator** — rejected: reintroduces the shared-tail
  conflict this migration exists to remove, and does not close the #363-class gap the
  way a generated-and-checked index does.
- **Leave a redirect stub at `docs/DECISIONS.md`** — rejected, matching D-130's own
  choice not to stub `SESSION_LOG.md`: this repository has no external consumers of
  these paths, only internal cross-references, all of which this migration updates in
  the same change.

## Out of scope

- The 14 index-table rows with no matching long-form entry (`proposed`/index-only
  decisions never promoted to a full section) are migrated as index-only stub files
  (frontmatter + a one-line body noting "index-only, no long-form entry recorded") —
  not silently dropped, but also not expanded into invented long-form content this
  design has no authority to write.
- Re-litigating any accepted decision's own content — this is a storage-location
  migration only, per `docs/DECISIONS.md`'s own "a new decision supersedes, it does not
  silently edit, an accepted one" rule, applied here to *location* the same way D-130
  applied it to D-066's own entry (narrowed, not rewritten).
- Automating discovery of *future* stale cross-reference patterns beyond this one
  file's own migration — `ultra-review`'s own dispatch prompt already gained a general
  reminder for the related-but-distinct "prose describing another PR's transient
  state" pattern (rotnov/pycc#364) as a separate, already-applied fix; that reminder is
  unrelated to this migration's own scope and not extended further here.
