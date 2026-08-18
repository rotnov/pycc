---
id: D-175
title: "Scope the conformance-matrix fixture guard to green rows"
status: accepted
---

## D-175: Scope the conformance-matrix fixture guard to green rows

- Status: accepted
- Date: 2026-08-18
- Context:

  `docs/PYTHON_STANDARDS.md`'s matrix drifted from the fixture tree it describes
  and nothing caught it. Its **Conventions** block states the test path as
  `tests/conformance/pyXY/pep_NNNN_slug.py`, the eventual language-level-selecting
  harness, and then records that the fixtures PR-9 added "live flat at
  `tests/fixtures/pep_NNNN_slug.py` instead" and that "the `pyXY/` tree and its
  language-level selection do not exist yet". Both statements are true, and the
  matrix's fixture column carries both kinds of path at once. At the time this
  entry was written, 16 of the 25 `✅` rows cited a `pyNN/`-prefixed path that
  resolves to no file anywhere in the repository, while every one of those
  fixtures was present flat — the citations were simply stale.

  Issue [#578](https://github.com/rotnov/pycc/issues/578) asked for a guard
  asserting, "for every row of the matrix that names a fixture, that the named
  path resolves to a real file under `tests/fixtures/`". Taken literally that
  covers all 93 rows. But 68 of them are `☐`, and 65 of those cite a path for a
  fixture nobody has authored yet. Satisfying the literal wording would mean
  deleting 65 planning citations — the inventory the matrix exists to carry —
  in order to make a guard pass. That is the guard shaping the document rather
  than the document's own claims shaping the guard.

- Decision:

  `tests/conformance_matrix_guard.rs` asserts fixture existence and harness
  registration **only for rows marked `✅`**, and the matrix's fixture column is
  read as making two different kinds of statement depending on the status cell:

  - A `✅` row claims its fixture passes on all Tier-1 targets in both profiles.
    That claim is only meaningful if the file exists and actually runs, so the
    cited path must resolve relative to `tests/fixtures/` (which rejects a
    `pyNN/` prefix rather than silently resolving through the basename) and the
    fixture must be registered in `tests/conformance.rs`. "Registered" means the
    `tests/fixtures/<name>` form the harness's own `Path::join` calls use, not a
    bare file name — several fixtures are additionally named as bare backticked
    file names in that file's doc comments, and a bare-name search would count
    such a mention as a registration.
  - A `☐` row names the *planned* path a fixture will occupy under the harness
    the Conventions block describes. It asserts nothing about a file existing
    today, so the guard asserts nothing about it either.

  The guard additionally runs the inverse direction, which is status-independent
  because it starts from the tree rather than the document: every
  `tests/fixtures/pep_*.py` must be registered in `tests/conformance.rs` or carry
  an allowlist entry whose reason is recorded in the test source. The allowlist is
  itself guarded — an entry naming a fixture that no longer exists, or one that has
  since been registered, fails the test rather than lingering.

  When the real `pyXY/` harness is eventually built, this scoping is what has to
  change with it: the guard's path base moves, and at that point `☐` rows'
  planned paths become checkable too.

- Alternatives:

  - **Guard every row, and delete the 65 unresolvable `☐` citations.** Rejected:
    it destroys the matrix's planning inventory to satisfy a test, and the
    deleted names are exactly what Part 4 of #572 needs in order to know which
    fixtures to author.
  - **Guard every row, and author all 65 missing fixtures first.** Rejected as a
    scope inversion — that is the entire remaining conformance workload of v0.3
    and beyond, and it would make a bookkeeping guard block on it.
  - **Drop only the three citations #578 names (`pep_3135_super.py`,
    `pep_0487_init_subclass.py`, `pep_0560_class_getitem.py`).** Rejected: those
    three are `☐` rows citing not-yet-authored fixtures, structurally
    indistinguishable from 62 others in the same position. Singling them out
    makes the matrix less internally consistent, not more, and leaves the guard
    unable to state a rule that explains its own scope.
  - **Rewrite `☐` rows' paths to flat form without asserting existence.**
    Rejected: a flat path that resolves to nothing is not more honest than a
    `pyNN/` path that resolves to nothing, and it would erase the one signal
    distinguishing "planned under the future harness" from "shipped today".
  - **Enforce it in a `scripts/` checker instead of a Rust test.** Rejected: the
    inverse direction has to read `tests/conformance.rs`'s registrations, the
    check belongs with the harness it guards, and `cargo test --workspace --
    --include-ignored` already runs `tests/*.rs` files with no `ci.yml` edit —
    which keeps this change out of D-103's stage-then-activate process.

- Consequences:

  - A row cannot be flipped to `✅` while citing a path that does not resolve, so
    the D-102 by-hand flip policy gains a mechanical backstop it did not have.
    The `☐` → `✅` transition is now where the fixture-path claim gets checked.
  - `docs/TESTING.md`'s aspirational layer-2 and harness sections are annotated to
    distinguish the planned `pyXY/` layout from the flat reality, so the two no
    longer read as contradicting each other.
  - Adding a genuinely new non-conformance `pep_*.py` fixture now requires an
    allowlist entry with a stated reason. This is intended friction: the five
    existing entries each record a specific blocker, and four of them clear when
    [#579](https://github.com/rotnov/pycc/issues/579) lands.
  - The guard is a documentation-consistency check, not a conformance check. It
    cannot tell whether a fixture actually passes — only that the matrix's claims
    about which files exist and run are true. `tests/conformance.rs` remains the
    thing that proves passing.
