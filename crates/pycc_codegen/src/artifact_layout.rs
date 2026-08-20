//! Where `pycc` looks for build artifacts it did not produce itself.
//!
//! `pycc build` links the compiled object against `pycc_rt`'s static
//! library, which Cargo has already placed somewhere under the *Cargo
//! target directory*. That directory is not always `<workspace>/target`:
//! Cargo lets it be redirected, and this project's own coverage job does
//! exactly that (see `docs/TESTING.md`). Everything that needs to locate a
//! Cargo-produced artifact resolves it through this module rather than
//! joining `"target"` onto a manifest directory, so a redirected target
//! directory is honored uniformly by the compiler driver, by
//! `pycc_codegen`'s own link-and-run tests, and by the CLI integration
//! tests. `tests/target_dir_literals.rs` is the mechanical gate that keeps
//! it that way.

/// The directory name Cargo uses when the target directory is not
/// redirected. Named rather than inlined so that this module -- the single
/// authorized place to build that path -- does not itself trip
/// `tests/target_dir_literals.rs`, the gate that forbids the same join
/// everywhere else.
const DEFAULT_TARGET_DIR_NAME: &str = "target";

/// Resolves the Cargo target directory that holds this workspace's build
/// artifacts.
///
/// Three levels, in Cargo's own relative order (also stated in
/// `docs/CLI_SPEC.md`; all three must agree):
///
/// 1. `CARGO_TARGET_DIR`, when set to a non-empty value;
/// 2. otherwise `CARGO_BUILD_TARGET_DIR`, when set to a non-empty value --
///    this is Cargo's generic config-to-environment mapping of the
///    `build.target-dir` config key, and it is honored by Cargo whether or
///    not any `.cargo/config.toml` exists. Verified on this project's dev
///    host, not assumed: `CARGO_BUILD_TARGET_DIR=<dir> cargo build` puts
///    artifacts in `<dir>/debug` and creates no local `target/` at all,
///    and with both variables set `CARGO_TARGET_DIR` wins and the
///    `CARGO_BUILD_TARGET_DIR` path is never created;
/// 3. otherwise `<manifest_dir>/target`.
///
/// An empty value at either level is treated as unset. This is a
/// deliberate **divergence** from Cargo, which does not fall back but
/// rejects the build outright (`CARGO_TARGET_DIR= cargo build` exits 101
/// with "the target directory is set to an empty string in the
/// `CARGO_TARGET_DIR` environment variable"). A compiler driver looking
/// for an artifact someone else built has no equivalent of that abort:
/// honoring the empty string would resolve artifacts to a bare relative
/// `debug/`, which is worse than falling back, and an
/// exported-but-empty variable is a common shell accident rather than an
/// intent to redirect.
///
/// A relative value is passed through unchanged rather than being joined
/// onto `manifest_dir`. Cargo resolves a relative target directory against
/// the working directory of the process that invoked *it*; `pycc` is a
/// separate process, so passing the value through agrees with Cargo
/// exactly when `pycc` runs from that same directory, and re-anchoring it
/// on `manifest_dir` would agree with Cargo in no case at all.
///
/// Cargo's `--target-dir` **command-line flag** and a `build.target-dir`
/// key set in a `.cargo/config.toml` **config file** rank above both
/// variables in Cargo's precedence and are not consulted here. Reading
/// the config-file form means re-implementing Cargo's ancestor-walking
/// config discovery -- `$CARGO_HOME` merge, Cargo's own precedence and
/// path resolution -- a materially larger surface with its own
/// 100%-region cost under D-014. As for the flag: its resolved path does
/// reach an integration-test or bench binary, but only through the
/// *compile-time* `CARGO_TARGET_TMPDIR` macro, never the runtime
/// environment, and not at all to the `pycc` binary a user invokes
/// (measured: under `cargo test --target-dir <dir>`,
/// `env!("CARGO_TARGET_TMPDIR")` is `<dir>/tmp` while every runtime
/// lookup of it is `NotPresent`). Anchoring this one shared resolver on
/// it would give it two different resolution rules depending on which
/// binary it was compiled into. See `docs/decisions/D-183-*.md`.
///
/// `env_lookup` is a plain `fn` pointer, not `impl Fn(..)`, for the same
/// reason as [`find_pycc_rt_lib_dir_in`]'s `exists`: a generic parameter
/// would monomorphize one body per closure type, and each copy would only
/// ever exercise the arm *that* caller takes, which under `cargo llvm-cov`
/// reads as a real gap in D-014's region coverage.
pub fn resolve_cargo_target_root(
    manifest_dir: &std::path::Path,
    env_lookup: fn(&str) -> Option<String>,
) -> std::path::PathBuf {
    match env_lookup("CARGO_TARGET_DIR") {
        Some(value) if !value.is_empty() => std::path::PathBuf::from(value),
        _ => match env_lookup("CARGO_BUILD_TARGET_DIR") {
            Some(value) if !value.is_empty() => std::path::PathBuf::from(value),
            _ => manifest_dir.join(DEFAULT_TARGET_DIR_NAME),
        },
    }
}

/// Reads `CARGO_TARGET_DIR` from this process's real environment.
///
/// The production `env_lookup` for [`resolve_cargo_target_root`]; exists
/// as a named `fn` so every caller shares one function pointer instead of
/// each spelling out its own closure.
pub fn cargo_target_dir_from_env(key: &str) -> Option<String> {
    std::env::var(key).ok()
}

/// Rust's `staticlib` output naming is platform-specific: `lib<name>.a` on
/// Unix-like targets, but `<name>.lib` (no `lib` prefix, COFF format) on
/// `-msvc` targets -- verified empirically by cross-building `pycc_rt` for
/// `x86_64-pc-windows-msvc` and inspecting the resulting artifact
/// directory directly, not assumed from Unix convention. Keyed on the
/// *requested* `target` triple, not the host `#[cfg(windows)]` -- an
/// earlier version used a host-keyed constant, which silently checked for
/// the wrong filename whenever `--target` crossed OS families (e.g.
/// requesting an `-msvc` triple from a non-Windows host, or vice versa),
/// reporting a misleading "no build found" even when the correctly-named
/// file was right there (caught in PR review before merge).
pub fn pycc_rt_lib_filename(target: Option<&str>) -> &'static str {
    let targets_msvc = match target {
        Some(triple) => triple.contains("windows-msvc"),
        None => cfg!(windows),
    };
    if targets_msvc {
        "pycc_rt.lib"
    } else {
        "libpycc_rt.a"
    }
}

/// Locates the directory holding `pycc_rt`'s static library.
///
/// `target_root` is an already-resolved Cargo target directory -- callers
/// obtain it from [`resolve_cargo_target_root`] rather than joining
/// `"target"` themselves, so a redirected `CARGO_TARGET_DIR` is honored.
///
/// `target: None` (the common case) returns `<target_root>/debug/` or
/// `<target_root>/release/` -- always populated once `pycc_rt` has been
/// built for the requested profile (see that crate's own doc comment on
/// the build-order requirement).
///
/// `target: Some(triple)` looks under `<target_root>/<triple>/` -- where
/// `cargo build [--release] --target <triple> -p pycc_rt` (after `rustup
/// target add <triple>` if needed) puts it. Cross-compiling `pycc_rt`
/// itself for the requested target is not done automatically here: unlike
/// the ordinary case, there's no single always-correct way to trigger it
/// that doesn't either require duplicating `rustup`/`cargo`'s own
/// target-management logic or risk the same build-lock deadlock documented
/// on `pycc_rt`. Failing with a clear, actionable message is better than a
/// confusing linker error about a missing `-lpycc_rt`.
///
/// `release` selects which of `pycc_rt`'s own two builds to link against.
/// Before this parameter existed, the compiler driver unconditionally
/// linked the debug build regardless of `pycc build --release` -- a real,
/// unambiguous bug (not a design choice) caught while investigating why a
/// `--release` nbody benchmark's speedup ratio fell far short of
/// expectations (`tests/nbody_bench.rs`): `--release` was optimizing the
/// compiled module's own LLVM IR but every runtime call
/// (`pycc_rt_float_pow`, string helpers, etc.) still ran through an
/// unoptimized debug `pycc_rt`.
///
/// `exists` takes the filesystem-existence check as a parameter instead of
/// calling `Path::exists` directly, so tests can simulate "no build found"
/// without mutating this workspace's real, shared target directory (which
/// every other test also depends on -- deleting the real `libpycc_rt.a`,
/// even temporarily, would be flaky under parallel test execution).
///
/// A plain `fn` pointer, not `impl Fn(..)`: every caller (production and
/// all the tests) passes a non-capturing closure, so a concrete function
/// pointer covers all of them with a single compiled body. Genericity over
/// `impl Fn` would monomorphize a separate copy per closure type -- each
/// one only ever exercising the branches *that* caller takes, which under
/// `cargo llvm-cov` reads as a real gap in coverage even though every
/// branch collectively runs somewhere.
pub fn find_pycc_rt_lib_dir_in(
    target_root: &std::path::Path,
    target: Option<&str>,
    release: bool,
    exists: fn(&std::path::Path) -> bool,
) -> Result<std::path::PathBuf, String> {
    let profile = if release { "release" } else { "debug" };
    let release_flag = if release { " --release" } else { "" };
    let dir = match target {
        Some(triple) => target_root.join(triple).join(profile),
        None => target_root.join(profile),
    };
    if exists(&dir.join(pycc_rt_lib_filename(target))) {
        Ok(dir)
    } else if let Some(triple) = target {
        Err(format!(
            "no pycc_rt build found for target `{triple}` (expected {}). \
             Run `rustup target add {triple}` then `cargo build{release_flag} --target {triple} -p pycc_rt` first.",
            dir.display()
        ))
    } else {
        Err(format!(
            "no pycc_rt build found (expected {}). Run `cargo build{release_flag} -p pycc_rt` first.",
            dir.display()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace_target_root() -> std::path::PathBuf {
        std::path::PathBuf::from("/workspace/target")
    }

    #[test]
    fn an_unset_cargo_target_dir_falls_back_to_the_manifest_relative_directory() {
        let manifest = std::path::Path::new("/workspace");
        assert_eq!(
            resolve_cargo_target_root(manifest, |_| None),
            workspace_target_root()
        );
    }

    #[test]
    fn an_empty_cargo_target_dir_is_treated_as_unset() {
        let manifest = std::path::Path::new("/workspace");
        assert_eq!(
            resolve_cargo_target_root(manifest, |_| Some(String::new())),
            workspace_target_root()
        );
    }

    #[test]
    fn an_absolute_cargo_target_dir_replaces_the_manifest_relative_directory() {
        let manifest = std::path::Path::new("/workspace");
        assert_eq!(
            resolve_cargo_target_root(manifest, |_| Some("/elsewhere/build".to_string())),
            std::path::PathBuf::from("/elsewhere/build")
        );
    }

    #[test]
    fn a_relative_cargo_target_dir_is_passed_through_unchanged() {
        // Cargo resolves a relative CARGO_TARGET_DIR against the invoking
        // process's working directory, so re-anchoring it on `manifest`
        // here would disagree with where Cargo actually wrote artifacts.
        let manifest = std::path::Path::new("/workspace");
        assert_eq!(
            resolve_cargo_target_root(manifest, |_| Some("build-out".to_string())),
            std::path::PathBuf::from("build-out")
        );
    }

    /// Answers only for `CARGO_BUILD_TARGET_DIR`, so a test can prove the
    /// second precedence level is reached when the first is unset. A
    /// non-capturing closure, so it still coerces to the `fn` pointer.
    fn only_build_target_dir(key: &str) -> Option<String> {
        match key {
            "CARGO_BUILD_TARGET_DIR" => Some("/from-build-config".to_string()),
            _ => None,
        }
    }

    #[test]
    fn cargo_build_target_dir_is_honored_when_cargo_target_dir_is_unset() {
        // Cargo's generic config-to-env mapping of `build.target-dir`.
        // Measured: `CARGO_BUILD_TARGET_DIR=<dir> cargo build` writes to
        // `<dir>/debug` and creates no local `target/`.
        let manifest = std::path::Path::new("/workspace");
        assert_eq!(
            resolve_cargo_target_root(manifest, only_build_target_dir),
            std::path::PathBuf::from("/from-build-config")
        );
    }

    /// Answers for both variables, so a test can prove the first
    /// precedence level wins. A named `fn` rather than an inline closure
    /// on purpose: the resolver short-circuits on `CARGO_TARGET_DIR` and
    /// never asks for the second key, so the other arms are only ever
    /// reached by calling this directly -- which the test below does,
    /// keeping every arm covered under D-014 rather than leaving two
    /// unexecuted regions inside an anonymous closure.
    fn both_target_dirs(key: &str) -> Option<String> {
        match key {
            "CARGO_TARGET_DIR" => Some("/from-env".to_string()),
            "CARGO_BUILD_TARGET_DIR" => Some("/from-build-config".to_string()),
            _ => None,
        }
    }

    #[test]
    fn cargo_target_dir_outranks_cargo_build_target_dir_when_both_are_set() {
        // The fixture genuinely answers for both variables -- asserted
        // directly, since the resolver never asks for the second one.
        assert_eq!(
            both_target_dirs("CARGO_BUILD_TARGET_DIR"),
            Some("/from-build-config".to_string())
        );
        assert_eq!(both_target_dirs("SOMETHING_ELSE"), None);
        // Measured: with both set, artifacts land under CARGO_TARGET_DIR
        // and the CARGO_BUILD_TARGET_DIR path is never created.
        let manifest = std::path::Path::new("/workspace");
        assert_eq!(
            resolve_cargo_target_root(manifest, both_target_dirs),
            std::path::PathBuf::from("/from-env")
        );
    }

    #[test]
    fn an_empty_cargo_build_target_dir_is_also_treated_as_unset() {
        let manifest = std::path::Path::new("/workspace");
        assert_eq!(
            resolve_cargo_target_root(manifest, |key| match key {
                "CARGO_BUILD_TARGET_DIR" => Some(String::new()),
                _ => None,
            }),
            workspace_target_root()
        );
    }

    #[test]
    fn the_env_lookup_reads_the_named_variable_from_the_process() {
        // Three assertions rather than one, because agreeing with
        // `std::env::var("CARGO_TARGET_DIR")` alone is vacuous wherever
        // the variable is unset (`None == None` holds even for a function
        // that always returns `None`), which is the common local case.
        // A name no environment defines must come back `None`...
        assert_eq!(
            cargo_target_dir_from_env("PYCC_ARTIFACT_LAYOUT_UNSET_PROBE"),
            None
        );
        // ...and the lookup must use the key it is handed rather than a
        // hardcoded one, which a second, unrelated key demonstrates
        // wherever the environment defines it (and states truthfully, not
        // flakily, where it does not).
        assert_eq!(
            cargo_target_dir_from_env("PATH"),
            std::env::var("PATH").ok()
        );
        assert_eq!(
            cargo_target_dir_from_env("CARGO_TARGET_DIR"),
            std::env::var("CARGO_TARGET_DIR").ok()
        );
    }

    #[test]
    fn finds_the_native_lib_dir_when_it_exists() {
        let root = workspace_target_root();
        let result = find_pycc_rt_lib_dir_in(&root, None, false, |_| true);
        assert_eq!(result, Ok(root.join("debug")));
    }

    #[test]
    fn reports_a_clean_error_when_the_native_build_is_missing() {
        let root = workspace_target_root();
        let err = find_pycc_rt_lib_dir_in(&root, None, false, |_| false).unwrap_err();
        assert!(err.contains("cargo build -p pycc_rt"));
        // Must not suggest --release for a debug-profile lookup.
        assert!(!err.contains("--release"));
    }

    #[test]
    fn finds_the_target_specific_lib_dir_when_it_exists() {
        let root = workspace_target_root();
        let result =
            find_pycc_rt_lib_dir_in(&root, Some("x86_64-unknown-linux-gnu"), false, |_| true);
        assert_eq!(
            result,
            Ok(root.join("x86_64-unknown-linux-gnu").join("debug"))
        );
    }

    #[test]
    fn reports_a_clean_error_naming_the_target_when_its_build_is_missing() {
        let root = workspace_target_root();
        let err =
            find_pycc_rt_lib_dir_in(&root, Some("x86_64-unknown-linux-gnu"), false, |_| false)
                .unwrap_err();
        assert!(err.contains("x86_64-unknown-linux-gnu"));
        assert!(err.contains("rustup target add"));
    }

    #[test]
    fn finds_the_native_release_lib_dir_when_it_exists() {
        let root = workspace_target_root();
        let result = find_pycc_rt_lib_dir_in(&root, None, true, |_| true);
        assert_eq!(result, Ok(root.join("release")));
    }

    #[test]
    fn reports_a_clean_error_naming_release_when_the_native_release_build_is_missing() {
        let root = workspace_target_root();
        let err = find_pycc_rt_lib_dir_in(&root, None, true, |_| false).unwrap_err();
        assert!(err.contains("cargo build --release -p pycc_rt"));
        // Not a literal separator-bearing substring check: `dir.display()`
        // renders with the platform's native separator, so a hardcoded
        // forward-slash needle would fail on Windows -- a real bug caught
        // by the pinned reviewer in an earlier round. Joining a `PathBuf`
        // compares components instead of literal separator bytes.
        let expected_dir = root.join("release");
        assert!(err.contains(&expected_dir.display().to_string()));
    }

    #[test]
    fn finds_the_target_specific_release_lib_dir_when_it_exists() {
        let root = workspace_target_root();
        let result =
            find_pycc_rt_lib_dir_in(&root, Some("x86_64-unknown-linux-gnu"), true, |_| true);
        assert_eq!(
            result,
            Ok(root.join("x86_64-unknown-linux-gnu").join("release"))
        );
    }

    #[test]
    fn reports_a_clean_error_naming_the_target_and_release_when_its_build_is_missing() {
        let root = workspace_target_root();
        let err = find_pycc_rt_lib_dir_in(&root, Some("x86_64-unknown-linux-gnu"), true, |_| false)
            .unwrap_err();
        assert!(err.contains("x86_64-unknown-linux-gnu"));
        assert!(err.contains("rustup target add"));
        assert!(err.contains("cargo build --release --target x86_64-unknown-linux-gnu -p pycc_rt"));
    }

    #[test]
    fn a_redirected_target_root_is_honored_end_to_end() {
        // The #629 behavior in one assertion: with CARGO_TARGET_DIR set,
        // the lookup lands under the redirected root, not under
        // `<manifest>/target`.
        let root = resolve_cargo_target_root(std::path::Path::new("/workspace"), |_| {
            Some("/elsewhere/build".to_string())
        });
        let result = find_pycc_rt_lib_dir_in(&root, None, false, |_| true);
        assert_eq!(
            result,
            Ok(std::path::PathBuf::from("/elsewhere/build").join("debug"))
        );
    }

    #[test]
    fn an_msvc_target_uses_the_dot_lib_filename_regardless_of_host() {
        // The regression this guards: pycc_rt_lib_filename used to be a
        // host-#[cfg(windows)] constant, so requesting an -msvc target from
        // this (non-Windows) test runner silently checked for the wrong
        // filename (libpycc_rt.a instead of pycc_rt.lib).
        assert_eq!(
            pycc_rt_lib_filename(Some("x86_64-pc-windows-msvc")),
            "pycc_rt.lib"
        );
    }

    #[test]
    fn a_non_msvc_target_uses_the_lib_prefix_dot_a_filename() {
        assert_eq!(
            pycc_rt_lib_filename(Some("x86_64-unknown-linux-gnu")),
            "libpycc_rt.a"
        );
    }
}
