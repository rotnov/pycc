//! Parity validator for #246.
//!
//! The Tier-1 target set/order is independently duplicated across four
//! places: `src/main.rs::TIER1_TARGETS` (the only runtime representation),
//! `docs/ARCHITECTURE.md`'s "Cross-platform (hard requirement)" table,
//! `tests/slice0.rs`'s exact-snapshot test of `pycc version --verbose`, and
//! `docs/CLI_SPEC.md`'s illustrative transcript. `tests/slice0.rs` already
//! pins the binary's actual output against its own literal, but nothing
//! ties that literal -- or the binary's real output -- back to either
//! documentation source. A mutation that changes the code and the
//! `slice0.rs` snapshot together, while leaving both docs untouched, passed
//! every existing test.
//!
//! This test closes that gap as a deterministic parity validator (the
//! issue's second suggested fix; see #246): it re-derives the Tier-1 list
//! from the binary's real `pycc version --verbose` output and from both
//! documentation sources, independently of `slice0.rs`'s hardcoded literal,
//! and asserts all three name the same five targets in the same order.

use std::path::PathBuf;
use std::process::Command;

fn pycc_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_pycc"))
}

/// Extracts the five backtick-quoted targets from ARCHITECTURE.md's
/// "Cross-platform (hard requirement)" table, in table order. Two of the
/// table's three rows each pair two targets in one cell
/// (`` `a` / `b` ``); the third row holds one target alone -- splitting each
/// row on backticks and keeping the odd-indexed segments recovers exactly
/// the quoted target names regardless of how many a given row holds.
fn architecture_tier1_targets() -> Vec<String> {
    // `include_str!` yields the file's raw checked-out bytes, and Windows
    // checkouts normalize LF to CRLF under the default `core.autocrlf` text
    // conversion (see `tests/diagnostics_test.rs`'s own note on this). This
    // parser only ever splits on `\n`/backticks, both CRLF-safe, but the
    // sibling `indented_list_after` below matches a literal trailing `\n`
    // marker, so both this text and that one are normalized identically for
    // consistency and defense-in-depth.
    let text = include_str!("../docs/ARCHITECTURE.md").replace("\r\n", "\n");
    let text = text.as_str();
    let table_start = text
        .find("| Target | Notes |")
        .expect("ARCHITECTURE.md must have the Cross-platform target table");
    let table = &text[table_start..];
    table
        .lines()
        .skip(2) // the header row itself, then the `|---|---|` separator row
        .take_while(|line| line.starts_with('|'))
        .flat_map(|line| {
            line.split('`')
                .enumerate()
                .filter(|(i, _)| i % 2 == 1)
                .map(|(_, part)| part.to_string())
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Extracts a two-space-indented list of lines immediately following the
/// given marker, stopping at the first non-indented line. Used both for
/// CLI_SPEC.md's illustrative transcript and for the binary's own real
/// `--verbose` stdout, since both render the target list the same way.
fn indented_list_after<'a>(text: &'a str, marker: &str) -> Vec<String> {
    let idx = text
        .find(marker)
        .unwrap_or_else(|| panic!("expected to find {marker:?}"));
    let after: &'a str = &text[idx + marker.len()..];
    after
        .lines()
        .take_while(|line| line.starts_with("  "))
        .map(|line| line.trim().to_string())
        .collect()
}

fn cli_spec_tier1_targets() -> Vec<String> {
    // See `architecture_tier1_targets`'s note: `indented_list_after` matches
    // a literal trailing `\n` marker, which a Windows CRLF checkout of this
    // file would turn into `\n`-less `\r\n`, making the marker search fail
    // and panic. Normalize before searching, exactly as
    // `tests/diagnostics_test.rs` does for its own fixture comparisons.
    let text = include_str!("../docs/CLI_SPEC.md").replace("\r\n", "\n");
    indented_list_after(&text, "tier-1 targets:\n")
}

fn binary_tier1_targets() -> Vec<String> {
    let output = Command::new(pycc_bin())
        .args(["version", "--verbose"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "pycc version --verbose failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    indented_list_after(&stdout, "tier-1 targets:\n")
}

#[test]
fn tier1_target_list_is_consistent_across_binary_and_docs() {
    let binary = binary_tier1_targets();
    let architecture = architecture_tier1_targets();
    let cli_spec = cli_spec_tier1_targets();

    assert_eq!(
        binary, architecture,
        "pycc version --verbose's actual target list diverged from \
         ARCHITECTURE.md's Cross-platform (hard requirement) table"
    );
    assert_eq!(
        binary, cli_spec,
        "pycc version --verbose's actual target list diverged from \
         CLI_SPEC.md's illustrative transcript"
    );
}
