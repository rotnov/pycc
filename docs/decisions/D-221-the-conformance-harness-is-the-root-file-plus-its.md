---
id: D-221
title: "The conformance harness is `tests/conformance.rs` plus `tests/conformance/*.rs`; every harness text-reader audits the concatenation (#729)"
status: accepted
---

## D-221: The conformance harness is `tests/conformance.rs` plus `tests/conformance/*.rs`; every harness text-reader audits the concatenation (#729)

- Status: accepted
- Context: `AGENTS.md`'s "Keep source files decomposable" rule treats a Rust
  source file over ~1,000 lines as a maintainability and agent-context risk,
  and `tests/conformance.rs` had grown to 1455 lines and 58 `#[test]`s
  (#729). The obvious split — moving tests into submodules — is not a local
  change, because three independent checkers read that file *as text* and
  audit what it registers and asserts: `tests/conformance_matrix_guard.rs`
  (D-175: every evidence-backed matrix row's fixture must be registered in
  the harness, and every `tests/fixtures/pep_*.py` must be registered or
  allowlisted), `scripts/check_conformance_breadth.py` (D-177: the breadth
  manifest's fixtures must be registered), and `tests/conformance_oracle_guard.rs`
  (#224: every `*_matches_cpython_3_14_7_byte_for_byte` function must bind its
  `assert_eq!` to the dual-profile oracle helper, and every registered
  `pep_*.py` fixture must be run by a `#[test]` body). The issue named only
  the first two. A test moved out of the scanned file does not make the
  oracle guard *fail*; it makes that guard silently stop auditing the moved
  test while its `>= 40` differential-test floor still holds — measured: with
  seven tests moved and no reader changes, the matrix guard failed two tests
  and the breadth checker failed seven rows, but the oracle guard passed
  39/39. A decomposition that fixes only the readers that go red ships a
  green build with a weakened tautology guard.

  Two more facts about the tree shaped the shape of the split. First, rustc
  treats an integration-test file as a crate root, so `mod foo;` in
  `tests/conformance.rs` looks for `tests/foo.rs`, not
  `tests/conformance/foo.rs` (`error[E0583]`); the two working layouts are
  `#[path = "conformance/foo.rs"] mod foo;` or renaming the root to
  `tests/conformance/main.rs`. Second, `scripts/check_readme_testing_claims.rb`
  requires `tests/conformance.rs` to exist as README evidence and
  `scripts/test_classify_ci_changes.py` pins that path as compiler-selecting,
  so the root file must keep its name.
- Decision: the **conformance harness sources** are defined once, as a
  contract every text-reader of the harness must implement identically:
  `tests/conformance.rs` first, then every direct `tests/conformance/*.rs`
  file (extension exactly `rs`, non-recursive, sorted by file name), each
  preceded by a newline, with `\r\n` normalised to `\n` *after*
  concatenation; a missing module directory yields the root alone.
  `tests/harness_support/conformance_sources.rs` is the one Rust
  implementation, `#[path]`-included by both Rust guards (the directory name
  keeps Cargo's `tests/*.rs` auto-discovery from compiling it as a crate);
  `scripts/check_conformance_breadth.py`'s `read_harness` mirrors the same
  rule, as its `is_registered` already deliberately mirrors the matrix
  guard's. The root-first order is part of the contract because the oracle
  guard's mutation controls rewrite the *first* occurrence of a pattern in
  the concatenation, so every reader must see the same text in the same
  order, and the root keeps at least one dual-profile differential test.

  The harness root declares its cohort submodules through a compile-bound
  `harness_modules!` macro that both emits the `#[path]` declarations and
  records the declared paths, and a default-run test
  (`every_harness_module_on_disk_is_declared`) asserts set-equality between
  the declared paths and the `*.rs` files on disk. Together the two
  directions are closed: a declared file missing from disk fails to compile,
  and a file on disk that is not declared fails the test (it would otherwise
  be *read* by all three checkers — so its fixtures count as registered —
  while never being compiled or run). Cohorts are cohesion-driven
  (`classes.rs`, `exceptions.rs`, `numeric.rs` initially); a new fixture's
  test goes in the cohort that owns its semantics, not in the root. Fixtures
  themselves stay flat under `tests/fixtures/` per D-102: this decision
  concerns the harness's *Rust* sources only and does not supersede D-102,
  D-175, or D-177.
- Alternatives:
  - **Keep the registration literals in the root and move only test bodies
    out.** Rejected: the oracle guard's
    `every_registered_pep_fixture_is_exercised_by_a_test` requires each
    fixture literal to appear inside a `#[test]` body in the audited text,
    and `audit_differential` requires the `assert_eq!` in that same body, so
    the literals and the tests must move together; only the ~130 lines of
    helpers could move, which does not reach the threshold.
  - **Rename the crate root to `tests/conformance/main.rs`** so plain
    `mod foo;` works. Rejected: it breaks the README-claims checker's
    evidence path, the CI classifier's pinned path, all three readers'
    default paths, and dozens of documentation, ADR, and `ci.yml` comment
    references — every one fixable, but pure churn with no behaviour gain,
    and the `ci.yml` comment edits would drag the workflow into the change.
    `#[path]` had no precedent in this repository but is standard Rust,
    greppable, and leaves the root where every existing reader expects it.
  - **A declared manifest of harness files instead of a directory glob.**
    Rejected: one more registration that can drift — a submodule declared in
    Rust but forgotten in the list would silently escape the oracle guard,
    the exact hazard above. The glob plus the compile-bound set-equality
    test closes both directions without a second list.
  - **Duplicating the discovery code in each Rust guard**, as both already
    duplicate `repo_root`/`read`. Rejected: `tests/conformance_oracle_guard.rs`
    is itself 1020 lines, so `AGENTS.md`'s decomposition rule applies to the
    part this change touches there; a shared `#[path]`-included module both
    avoids growing that file and keeps the two Rust readers on one
    implementation. The Python checker keeps its own copy — two
    implementations of the rule, not three.
  - **A text-level "every on-disk module is declared" check inside the
    oracle guard.** Rejected: a substring check on the root text is
    satisfied by a commented-out declaration, and the oracle guard's
    `harness()` has no root-only view of the concatenation. The macro plus
    on-disk test in the `conformance` crate itself is sound by construction.
- Consequences: new conformance tests go in the owning cohort file; a
  further narrowing pull request may extract `containers.rs` or `typing.rs`
  if the root grows back toward the threshold. Any future reader of the
  harness text must apply this discovery rule, not read the root alone. The
  planned `tests/conformance/pyXY/` per-level fixture tree (D-102,
  `docs/TESTING.md`) is invisible to the non-recursive `*.rs` glob by
  design, so building it under that directory does not collide with the
  harness modules. The shared module's unit tests compile into both guard
  crates and therefore run twice; accepted, they are milliseconds. The
  contract is enforced by `tests/harness_support/conformance_sources.rs`'s
  own tests, `scripts/test_check_conformance_breadth.py`'s
  `ReadHarnessTests` and CLI round-trip (with its negative mirror), and
  `every_harness_module_on_disk_is_declared`.
