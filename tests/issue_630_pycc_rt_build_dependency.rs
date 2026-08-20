//! `pycc_rt`'s archive must exist after an ordinary build of a clean
//! checkout, for both profiles, without anyone running
//! `cargo build -p pycc_rt` by hand.
//!
//! Each harness points a fresh, empty, absolute `CARGO_TARGET_DIR` at a
//! temporary directory and builds `pycc_codegen` there, then asserts that
//! *both* `debug/` and `release/` hold the archive. Building only the
//! profile under construction would still leave `pycc build --release`
//! broken after a plain `cargo build`, and only a clean root can tell the
//! difference: this workspace's own shared artifact directory already
//! contains both archives from earlier work, so an in-place assertion
//! would pass no matter what the build script did.
//!
//! Two harnesses and not four. A third case -- an *absolute*
//! `CARGO_TARGET_DIR` -- is what both of these already are, since a
//! temporary directory is absolute; there is no separate behavior to
//! cover. A fourth case -- a *relative* `CARGO_TARGET_DIR` -- is
//! deliberately not exercised as a process: its whole point is that Cargo
//! resolves the value against the invoking process's working directory,
//! which for a test binary is not something the test may change without
//! racing every other test in the same process. That path is covered as a
//! pure function instead, by
//! `pycc_artifact_layout::anchor_target_root_for_build_script`'s own unit
//! tests, which drive the contained and divergent cases directly.

use pycc_codegen::artifact_layout::{pycc_rt_lib_filename, should_scrub_for_nested_cargo};

/// A temporary directory removed when the test ends, however it ends.
struct TempTree(std::path::PathBuf);

impl Drop for TempTree {
    fn drop(&mut self) {
        // Branchless on purpose: a failed cleanup is not worth failing a
        // passing test over, and an `if` here would leave an arm that
        // never runs under D-014.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A fresh, empty, absolute directory nothing else is using.
fn fresh_target_root(label: &str) -> TempTree {
    let unique = format!(
        "pycc-issue-630-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_nanos())
            .unwrap_or_default()
    );
    let path = std::env::temp_dir().join(unique);
    std::fs::create_dir_all(&path).expect("cannot create the temporary build root");
    TempTree(path)
}

/// Builds `pycc_codegen` at `root` under the requested profile.
///
/// The ambient environment describes *this* build -- the one running the
/// test -- so it is scrubbed exactly the way the build script scrubs it
/// before its own nested invocation, and only then is the redirected
/// target directory set. Setting it without scrubbing first would leave
/// the outer `CARGO_TARGET_DIR` in place on the coverage job, where it is
/// already redirected, and the harness would silently assert against the
/// shared artifact directory instead of the clean one.
fn build_pycc_codegen_at(root: &std::path::Path, release: bool) {
    let mut command = std::process::Command::new(env!("CARGO"));
    command
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .arg("check")
        .arg("--locked")
        .arg("-p")
        .arg("pycc_codegen");
    if release {
        command.arg("--release");
    }
    for (name, _) in std::env::vars_os() {
        let name = name.to_string_lossy().into_owned();
        if should_scrub_for_nested_cargo(&name) {
            command.env_remove(&name);
        }
    }
    command.env("CARGO_TARGET_DIR", root);
    let output = command.output().expect("cannot spawn cargo");
    assert!(
        output.status.success(),
        "building pycc_codegen at {} failed ({})\n--- stderr ---\n{}",
        root.display(),
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Asserts both profile archives are present under `root`.
fn assert_both_profiles_installed(root: &std::path::Path) {
    let name = pycc_rt_lib_filename(None);
    for profile in ["debug", "release"] {
        let archive = root.join(profile).join(name);
        assert!(
            archive.exists(),
            "{} is missing after an ordinary build of a clean root",
            archive.display()
        );
    }
}

#[test]
fn a_clean_root_gets_both_pycc_rt_archives_from_a_dev_profile_build() {
    let tree = fresh_target_root("dev");
    build_pycc_codegen_at(&tree.0, false);
    assert_both_profiles_installed(&tree.0);
}

#[test]
fn a_clean_root_gets_both_pycc_rt_archives_from_a_release_profile_build() {
    let tree = fresh_target_root("release");
    build_pycc_codegen_at(&tree.0, true);
    assert_both_profiles_installed(&tree.0);
}

#[test]
fn pycc_rt_declares_no_dependencies() {
    // Three separate arguments rest on this being true, so it is asserted
    // rather than remembered:
    //
    // 1. the nested `cargo build -p pycc_rt` needs no registry access, so
    //    it never contends for `$CARGO_HOME/.package-cache` with the
    //    outer build that is already running;
    // 2. building it twice (both profiles) from a cold nested directory
    //    stays cheap enough to sit on every build;
    // 3. `--locked` cannot fail on a dependency the lockfile does not
    //    describe.
    //
    // Adding a dependency to `pycc_rt` invalidates all three and must be a
    // deliberate decision, not a silent one.
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("crates")
        .join("pycc_rt")
        .join("Cargo.toml");
    let text = std::fs::read_to_string(&manifest).expect("cannot read pycc_rt's manifest");
    let sections: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with('[') && line.contains("dependencies"))
        .collect();
    assert_eq!(
        sections,
        Vec::<&str>::new(),
        "pycc_rt gained a dependency section; see the comment in {}",
        file!()
    );
}
