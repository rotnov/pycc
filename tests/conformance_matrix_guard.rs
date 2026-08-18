//! Guards `docs/PYTHON_STANDARDS.md`'s conformance matrix against silent drift
//! from the fixture tree it describes.
//!
//! The matrix drifted from the tree once already and nothing caught it: green
//! rows cited `pyNN/`-prefixed fixture paths that resolve to no file anywhere in
//! this repository (see issue #572 and its Part 2, #578). This file is the check
//! that keeps the corrected state true, per `AGENTS.md`'s standing rule that
//! every normative documentation claim should be enforceable by a test where
//! practical.
//!
//! **Scope: `✅` rows only.** The matrix's own Conventions block defines the test
//! path as the eventual `tests/conformance/pyXY/pep_NNNN_slug.py` harness layout
//! and then records that today's fixtures live flat at `tests/fixtures/` and that
//! "the `pyXY/` tree and its language-level selection do not exist yet". A `☐`
//! row's path is therefore a *planned* path for a fixture nobody has authored,
//! and asserting that it resolves would be asserting something the matrix never
//! claimed. A `✅` row is different: it claims its fixture passes, which is only
//! meaningful if the file exists and actually runs. So a green row's path must
//! resolve, and its fixture must be registered in `tests/conformance.rs`.

use std::collections::BTreeSet;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(relative: &str) -> String {
    let path = repo_root().join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// One parsed matrix row: its source line number, its PEP cell, the fixture
/// paths its Test cell cites, and its status cell.
struct MatrixRow {
    line_number: usize,
    pep: String,
    fixtures: Vec<String>,
    status: String,
}

/// Extracts every ``` `...py` ``` span from a Test cell. A cell may cite more
/// than one fixture (PEP 594 cites two, PEP 585's row cites four), and it may
/// also carry prose and links around them, so the backtick spans are the signal
/// rather than whitespace splitting.
fn cited_fixtures(test_cell: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = test_cell;
    while let Some(open) = rest.find('`') {
        let after_open = &rest[open + 1..];
        let Some(close) = after_open.find('`') else {
            break;
        };
        let span = &after_open[..close];
        if span.ends_with(".py") {
            found.push(span.to_string());
        }
        rest = &after_open[close + 1..];
    }
    found
}

/// Parses the matrix's `| PEP | Feature | Cat | Test | St |` rows. Header and
/// separator rows are dropped by requiring a status cell that is one of the
/// documented status markers, which also skips every unrelated five-column table
/// elsewhere in the file.
fn parse_matrix(markdown: &str) -> Vec<MatrixRow> {
    let mut rows = Vec::new();
    for (index, line) in markdown.lines().enumerate() {
        if !line.starts_with('|') {
            continue;
        }
        let cells: Vec<&str> = line
            .trim()
            .trim_matches('|')
            .split('|')
            .map(str::trim)
            .collect();
        if cells.len() != 5 {
            continue;
        }
        let status = cells[4];
        if !matches!(status, "☐" | "⚙" | "✅") {
            continue;
        }
        rows.push(MatrixRow {
            line_number: index + 1,
            pep: cells[0].to_string(),
            fixtures: cited_fixtures(cells[3]),
            status: status.to_string(),
        });
    }
    rows
}

fn green_rows(markdown: &str) -> Vec<MatrixRow> {
    parse_matrix(markdown)
        .into_iter()
        .filter(|row| row.status == "✅")
        .collect()
}

/// `pep_*.py` fixtures that live under `tests/fixtures/` but are deliberately
/// not registered in `tests/conformance.rs`. Each entry records why, so that
/// removing one is a decision someone made rather than a line that rotted.
const UNREGISTERED_FIXTURE_ALLOWLIST: &[(&str, &str)] = &[
    (
        "pep_0526_var_annotations_smoke.py",
        "Documented throwaway smoke fixture \
         (docs/superpowers/plans/2026-07-30-v0-2-pr9-conformance-harness.md), consumed by \
         tests/slice1_codegen_depth.rs. It is not a conformance fixture and has no oracle \
         comparison.",
    ),
    (
        "pep_0681_dc_transform.py",
        "Blocked on the deliberate @dataclass_transform()-as-@dataclass divergence tracked by \
         issue #248: pycc synthesizes __init__/__eq__/__repr__ where CPython's own \
         typing.dataclass_transform synthesizes nothing, so this fixture cannot match the \
         oracle byte-for-byte at all. #579 registered the other three decorator fixtures; this \
         one is not merely awaiting the stdlib surface.",
    ),
];

fn is_allowlisted(fixture: &str) -> bool {
    UNREGISTERED_FIXTURE_ALLOWLIST
        .iter()
        .any(|(name, _reason)| *name == fixture)
}

/// True when `tests/conformance.rs` actually registers `fixture`.
///
/// Matches the `tests/fixtures/<name>` form the harness's own `Path::join`
/// calls use, not the bare file name. Several fixtures are also named in
/// `tests/conformance.rs`'s doc comments as bare backticked file names, and a
/// bare-name search would count such a mention as a registration.
fn is_registered(harness: &str, fixture: &str) -> bool {
    harness.contains(&format!("tests/fixtures/{fixture}"))
}

/// Every `pep_*.py` file directly under `tests/fixtures/`, sorted.
///
/// Scoped to the `pep_` prefix on purpose: `nbody.py`, `pr6_1000_loc_bench.py`,
/// `pre_commit_encoding.py`, `pre_commit_valid.py`, and `quick_start.py` are
/// benchmark, hook, and documentation fixtures that a naive "every `.py` under
/// `tests/fixtures/`" rule would wrongly demand conformance registration for.
fn pep_fixture_files() -> BTreeSet<String> {
    let dir = repo_root().join("tests").join("fixtures");
    let entries =
        std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("cannot list {}: {e}", dir.display()));
    let mut names = BTreeSet::new();
    for entry in entries {
        let entry = entry.expect("cannot read a tests/fixtures/ directory entry");
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("pep_") && name.ends_with(".py") {
            names.insert(name);
        }
    }
    assert!(
        !names.is_empty(),
        "no pep_*.py fixtures found under {} — the guard would pass vacuously",
        dir.display()
    );
    names
}

#[test]
fn every_green_matrix_row_cites_an_existing_flat_fixture() {
    let markdown = read("docs/PYTHON_STANDARDS.md");
    let rows = green_rows(&markdown);
    assert!(
        !rows.is_empty(),
        "no ✅ rows parsed out of docs/PYTHON_STANDARDS.md — the guard would pass vacuously"
    );

    let fixtures_dir = repo_root().join("tests").join("fixtures");
    let mut failures = Vec::new();
    for row in &rows {
        assert!(
            !row.fixtures.is_empty(),
            "line {}: PEP {} is marked ✅ but cites no fixture",
            row.line_number,
            row.pep
        );
        for fixture in &row.fixtures {
            // The cited path is interpreted relative to tests/fixtures/, so a
            // `pyNN/`-prefixed citation fails here rather than silently
            // resolving through its basename.
            if !fixtures_dir.join(fixture).is_file() {
                failures.push(format!(
                    "line {}: PEP {} is ✅ but `{}` does not resolve to a file under tests/fixtures/",
                    row.line_number, row.pep, fixture
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn every_green_matrix_fixture_is_registered_in_the_conformance_harness() {
    let markdown = read("docs/PYTHON_STANDARDS.md");
    let harness = read("tests/conformance.rs");

    let mut failures = Vec::new();
    for row in green_rows(&markdown) {
        for fixture in &row.fixtures {
            if !is_registered(&harness, fixture) {
                failures.push(format!(
                    "line {}: PEP {} is ✅ but `{}` is not registered in tests/conformance.rs",
                    row.line_number, row.pep, fixture
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn every_pep_fixture_is_registered_or_allowlisted() {
    let harness = read("tests/conformance.rs");

    let unregistered: Vec<String> = pep_fixture_files()
        .into_iter()
        .filter(|fixture| !is_registered(&harness, fixture))
        .filter(|fixture| !is_allowlisted(fixture))
        .collect();

    assert!(
        unregistered.is_empty(),
        "these tests/fixtures/pep_*.py files are neither registered in tests/conformance.rs nor \
         allowlisted in UNREGISTERED_FIXTURE_ALLOWLIST: {unregistered:?}"
    );
}

#[test]
fn the_allowlist_does_not_outlive_its_entries() {
    let harness = read("tests/conformance.rs");
    let present = pep_fixture_files();

    let mut failures = Vec::new();
    for (fixture, _reason) in UNREGISTERED_FIXTURE_ALLOWLIST {
        if !present.contains(*fixture) {
            failures.push(format!(
                "allowlist entry `{fixture}` names a fixture that no longer exists — drop the entry"
            ));
        } else if is_registered(&harness, fixture) {
            failures.push(format!(
                "allowlist entry `{fixture}` is now registered in tests/conformance.rs — drop the \
                 entry so the guard covers it"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn every_allowlist_entry_records_a_reason() {
    for (fixture, reason) in UNREGISTERED_FIXTURE_ALLOWLIST {
        assert!(
            reason.len() > 30,
            "allowlist entry `{fixture}` needs a real recorded reason, not `{reason}`"
        );
    }
}

#[test]
fn cited_fixtures_reads_every_backticked_path_in_a_cell() {
    // A single-fixture cell, the shape most rows have.
    assert_eq!(
        cited_fixtures("`pep_0435_enum.py`"),
        vec!["pep_0435_enum.py"]
    );
    // A multi-fixture cell carrying prose and a link between the paths, the
    // shape PEP 594's and PEP 585's rows have.
    assert_eq!(
        cited_fixtures("`a.py` + `b.py` (D-138; see [PR #328](https://example.invalid))"),
        vec!["a.py", "b.py"]
    );
    // A cell citing no fixture at all, and one whose backticked span is not a
    // path, must both yield nothing rather than a spurious entry.
    assert!(cited_fixtures("design doc only").is_empty());
    assert!(cited_fixtures("`--release`").is_empty());
    // An unterminated backtick must terminate the scan instead of looping.
    assert!(cited_fixtures("`unterminated.py").is_empty());
}

#[test]
fn parse_matrix_keeps_status_rows_and_drops_headers_and_separators() {
    let markdown = "\
| PEP | Feature | Cat | Test | St |
|---|---|---|---|---|
| [238](https://peps.python.org/pep-0238/) | True division | sem | `pep_0238_division.py` | ✅ |
| [3102](https://peps.python.org/pep-3102/) | Keyword-only args | syntax | `pep_3102_kwonly.py` | ☐ |
| [999](https://peps.python.org/pep-0999/) | In progress | sem | `pep_0999.py` | ⚙ |
| Track | Upstream checkpoint | pycc consequence |
";
    let rows = parse_matrix(markdown);
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].status, "✅");
    assert_eq!(rows[1].status, "☐");
    assert_eq!(rows[2].status, "⚙");
    assert_eq!(rows[0].line_number, 3);
    assert_eq!(rows[0].fixtures, vec!["pep_0238_division.py"]);

    // Only the ✅ row survives the green filter — a ☐ row's path is a planned
    // path and is deliberately not asserted to exist.
    let green = green_rows(markdown);
    assert_eq!(green.len(), 1);
    assert_eq!(green[0].fixtures, vec!["pep_0238_division.py"]);
}

#[test]
fn is_registered_requires_the_path_prefixed_form() {
    let harness = r#"
        /// See `pep_0594_dead_battery.py` for the negative control.
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pep_0435_enum.py");
    "#;
    assert!(is_registered(harness, "pep_0435_enum.py"));
    // Named only in a doc comment, never joined as a path — not a registration.
    assert!(!is_registered(harness, "pep_0594_dead_battery.py"));
    assert!(!is_registered(harness, "pep_0999_absent.py"));
}

#[test]
fn read_resolves_against_the_repository_root() {
    // Guards the path base itself: if CARGO_MANIFEST_DIR ever stops being the
    // repository root, every other test here would start reading the wrong
    // tree rather than failing loudly.
    assert!(repo_root().join("Cargo.toml").is_file());
    assert!(read("docs/PYTHON_STANDARDS.md").contains("Conformance Matrix"));
}
