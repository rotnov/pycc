//! Per-hero manifest facts for the Architecture pipeline trace (D-243, Part 2
//! of #566).
//!
//! These live in their own integration test rather than in
//! `tests/site_evidence.rs`, which the D-230 language and diagnostics records
//! pin byte-for-byte to a preserved source blob: adding a case there fails the
//! site gate with "language artifact differs from preserved source blob". The
//! implementation plan asked for the case in that file; this file is the
//! recorded deviation.
//!
//! Nothing here re-runs the pipeline. `tests/architecture_trace.rs` owns
//! re-derivation and `scripts/site_pipeline_evidence.py` owns the record
//! shape; this file proves only the small set of manifest facts a Rust reader
//! depends on.

use serde_json::json;
use std::path::Path;

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// The record exists in the inventory under the expected kind and state,
/// points at the page and the test that re-derives it, and carries only
/// immutable links.
#[test]
fn architecture_hero_record_states_a_partial_pipeline_trace() {
    let manifest = std::fs::read_to_string(repo_root().join("site/evidence-heroes.json"))
        .expect("the evidence manifest must be readable");
    let document: serde_json::Value =
        serde_json::from_str(&manifest).expect("the evidence manifest must be valid JSON");
    assert_eq!(document["schema_version"], json!("2.2.0"));
    let hero = document["heroes"]
        .as_array()
        .expect("heroes must be an array")
        .iter()
        .find(|hero| hero["page_id"] == json!("architecture"))
        .expect("the inventory must carry an architecture record");

    assert_eq!(hero["kind"], json!("compiler-pipeline-trace"));
    assert_eq!(hero["state"], json!("partial"));
    assert_eq!(hero["page_path"], json!("site/architecture/index.html"));
    assert_eq!(hero["test"]["path"], json!("tests/architecture_trace.rs"));
    assert_eq!(
        hero["fixture"]["path"],
        json!("tests/fixtures/quick_start.py")
    );

    for field in [
        "evidence_id",
        "command",
        "snapshot",
        "repository",
        "attestation",
        "environment",
        "limitations",
        "stable_links",
    ] {
        assert!(
            !hero[field].is_null(),
            "architecture {field} must be present"
        );
    }

    let stages = hero["snapshot"]["stages"]
        .as_array()
        .expect("the trace must carry an ordered stage list");
    let ids: Vec<&str> = stages
        .iter()
        .map(|stage| stage["id"].as_str().expect("every stage has an id"))
        .collect();
    assert_eq!(
        ids,
        [
            "source",
            "parser",
            "hir",
            "type-check",
            "mir",
            "llvm-ir",
            "native",
            "stdout",
        ]
    );
    // `partial` is exactly the claim that one stage carries no evidence.
    let unevidenced: Vec<&str> = stages
        .iter()
        .filter(|stage| stage["evidence"] == json!("none"))
        .map(|stage| stage["id"].as_str().expect("every stage has an id"))
        .collect();
    assert_eq!(unevidenced, ["llvm-ir"]);

    let links = hero["stable_links"]
        .as_object()
        .expect("stable links must be an object");
    assert!(!links.is_empty());
    for (name, link) in links {
        let url = link.as_str().expect("every stable link is a string");
        assert!(
            url.starts_with("https://github.com/rotnov/pycc/"),
            "architecture {name} must stay inside the repository"
        );
        assert!(
            !url.contains("/blob/main/") && !url.contains("/tree/main"),
            "architecture {name} must not point at a moving ref"
        );
    }
}
