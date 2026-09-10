# 2026-09-10 (02) — issue #80: vendored-skill lock defined over the tracked payload

## Status

Delivered by the pull request that carries this file. Base
`41412c10` (`origin/main` at branch time); branch
`fix/issue-80-vendored-skill-payload-guard`, four commits:

- `9f9f358c` — `scripts/validate_agent_assets.py` enumerates the vendored
  skill's payload from `git ls-files --stage -z --full-name -- <prefix>`,
  rejects fail-closed by class (non-blob modes incl. gitlinks and symlinks,
  non-UTF-8 paths, unmerged stages, `.pyc`/`.pyo`, `__pycache__`/
  `__pypackages__`/`.git`/`node_modules` components, missing or non-regular
  working-tree entries, empty payload), and computes the upstream-identical
  skills-CLI 1.5.20 path-plus-content SHA-256 over the accepted files only.
  Mutation tests per class plus temp-repo end-to-end tests; one
  `docs/AGENT_TOOLING.md` paragraph.
- `6c5b98d2` — review round 1: enumeration anchored at the repository root
  (a nested `.git` inside the skill root can no longer redirect it);
  directory-component rejection case-folds like the suffix rule.
- `765861c7` — review round 2: the test fixture copies only tracked entries
  enumerated through the production helper; the docs paragraph names every
  rejection class; uppercase-suffix mutation test.
- harden batch commit: `.harden/findings/issue-80.jsonl` and three incident
  counters.

`skills-lock.json` (`computedHash` `99e492cc…`) is byte-identical to `main`.

## Gates at HEAD

`python3 -B -m unittest discover -s scripts` — 1028 tests OK, 6 skipped;
`validate_agent_assets.py`, `validate_agent_policies.py`,
`check_ci_permissions.rb` — exit 0. D-068 review: three rounds
(`ievo:deep-reviewer`), round 3 clean.

## Corrections to the issue and plan

- The issue's "the upstream CLI hash excludes `__pycache__`/`.pyc`" premise
  was not upstream behaviour — the exclusion was local; this change removes
  it and instead rejects those classes before hashing (the issue's second
  option).
- The plan specified the enumerator anchored on the skill directory and a
  `copytree` fixture; both contradicted the plan's own "tracked index, never
  a working-tree walk" constraint and were corrected in review (recorded as
  `.harden/incidents/plan-contradicts-its-own-constraint/`).

## Follow-ups

None filed. The harden counters name a mechanical candidate (asserting each
rejection-reason constant is documented) for the next occurrence.

## Where to resume

`docs/AGENT_TOOLING.md` (vendored-skill lock paragraph) and
`scripts/validate_agent_assets.py` (`skill_payload_entries`,
`skill_payload_rejection`, `validate_skill_payload`, `validate_skill_lock`).
