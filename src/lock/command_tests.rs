use super::*;
use crate::embed::fake_layout::fake_layout;
use crate::interop_policy::InteropCli;
use pycc_scratch::ScratchDir;
use schema::LockedNative;

const HOST: (&str, &str) = ("aarch64", "macos");

fn program(dir: &Path, body: &str) -> std::path::PathBuf {
    let path = dir.join("m.py");
    std::fs::write(&path, body).unwrap();
    path
}

fn layout_dir(dir: &Path) -> std::path::PathBuf {
    let layout = dir.join("layout");
    std::fs::create_dir_all(&layout).unwrap();
    layout
}

fn absent(dir: &Path) -> EmbedToolchain {
    EmbedToolchain::with_interpreter(dir.join("absent-python"))
}

fn env_message(result: Result<(), LockFailure>) -> String {
    match result {
        Err(LockFailure::Env(message)) => message,
        Err(LockFailure::Stale(message)) => panic!("stale, not env: {message}"),
        Err(LockFailure::Frontend(_)) => panic!("frontend, not env"),
        Ok(()) => panic!("succeeded"),
    }
}

fn section(entry: &str) -> LockTarget {
    LockTarget {
        entry: entry.to_string(),
        triple: "aarch64-apple-darwin".to_string(),
        python: "3.14.7".to_string(),
        cache_tag: "cpython-314".to_string(),
        platform: "macosx-11.0-arm64".to_string(),
        libpython_sha256: "aa".to_string(),
        roots: vec!["tinypkg".to_string()],
        package: vec![LockedPackage {
            name: "tinypkg".to_string(),
            version: "1.0".to_string(),
            site: "purelib".to_string(),
            files: 2,
            tree_sha256: "bb".to_string(),
            requires: Vec::new(),
        }],
        native: Vec::new(),
    }
}

#[test]
fn a_windows_or_non_tier_1_host_is_refused_before_the_program_is_read() {
    let dir = ScratchDir::new("lock_hosts").unwrap();
    let missing = dir.join("missing.py");
    let windows = run_lock_on(
        &missing,
        false,
        InteropCli::default(),
        &absent(&dir),
        ("x86_64", "windows"),
    );
    assert!(env_message(windows).contains("#1226"));
    let other = run_lock_on(
        &missing,
        false,
        InteropCli::default(),
        &absent(&dir),
        ("riscv64", "linux"),
    );
    assert!(env_message(other).contains("not one"));
}

#[test]
fn the_frontend_refusal_and_every_failure_class_map_to_their_exit_codes() {
    let dir = ScratchDir::new("lock_frontend").unwrap();
    let missing = dir.join("missing.py");
    // A fixed Tier-1 host: on Windows `run_lock` refuses before the frontend.
    let failure =
        run_lock_on(&missing, false, InteropCli::default(), &absent(&dir), HOST).unwrap_err();
    assert!(matches!(failure, LockFailure::Frontend(_)));
    assert_eq!(report_lock_failure(failure), 2);
    assert_eq!(report_lock_failure(LockFailure::Env("e".to_string())), 2);
    assert_eq!(report_lock_failure(LockFailure::Stale("s".to_string())), 1);
}

#[test]
fn an_entry_key_joins_components_with_slashes_and_refuses_non_utf8() {
    assert_eq!(entry_key(Path::new("app/main.py")).unwrap(), "app/main.py");
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let bad = Path::new(std::ffi::OsStr::from_bytes(b"app/\xff.py"));
        assert!(entry_key(bad).unwrap_err().contains("not valid UTF-8"));
    }
}

#[test]
fn an_unreadable_existing_lock_is_refused() {
    let dir = ScratchDir::new("lock_unreadable").unwrap();
    let path = program(&dir, "print(1)\n");
    std::fs::create_dir_all(dir.join("pycc.lock").join("inner")).unwrap();
    let result = run_lock_on(&path, false, InteropCli::default(), &absent(&dir), HOST);
    assert!(env_message(result).contains("cannot read"));
}

#[test]
fn a_failed_rename_removes_the_temporary_file() {
    let scratch = ScratchDir::new("lock_rename").unwrap();
    let dir = scratch.join("app");
    std::fs::create_dir_all(dir.join("pycc.lock").join("inner")).unwrap();
    let err = write_atomically(&dir, "pycc.lock", "text").unwrap_err();
    assert!(err.contains("cannot write"), "{err}");
    let names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["pycc.lock"]);
}

#[cfg(unix)]
#[test]
fn a_lock_that_cannot_be_removed_is_an_environment_failure() {
    use std::os::unix::fs::PermissionsExt;
    let dir = ScratchDir::new("lock_remove").unwrap();
    let app = dir.join("app");
    std::fs::create_dir_all(&app).unwrap();
    let path = program(&app, "print(1)\n");
    let lock = Lock {
        version: LOCK_VERSION,
        target: vec![section("m.py")],
    };
    std::fs::write(app.join("pycc.lock"), schema::render(&lock)).unwrap();
    std::fs::set_permissions(&app, std::fs::Permissions::from_mode(0o555)).unwrap();
    let result = run_lock_on(&path, false, InteropCli::default(), &absent(&dir), HOST);
    std::fs::set_permissions(&app, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(env_message(result).contains("cannot remove"));
}

#[test]
fn a_lock_in_another_form_fails_check() {
    let dir = ScratchDir::new("lock_form").unwrap();
    let path = program(&dir, "print(1)\n");
    let mut other = section("m.py");
    other.triple = "x86_64-pc-windows-msvc".to_string();
    let lock = Lock {
        version: LOCK_VERSION,
        target: vec![other],
    };
    let text = schema::render(&lock).replace("# Generated by `pycc lock`. Do not edit.\n", "");
    std::fs::write(dir.join("pycc.lock"), text).unwrap();
    match run_lock_on(&path, true, InteropCli::default(), &absent(&dir), HOST) {
        Err(LockFailure::Stale(message)) => {
            assert!(message.contains("not in the form"), "{message}")
        }
        _ => panic!("expected a stale lock"),
    }
}

#[test]
fn an_interpreter_that_fails_either_probe_is_refused() {
    let dir = ScratchDir::new("lock_probes").unwrap();
    let path = program(&dir, "import json\n");
    let result = run_lock_on(&path, false, InteropCli::default(), &absent(&dir), HOST);
    assert!(env_message(result).contains("could not run the embed interpreter"));
    let layout = fake_layout(&layout_dir(&dir));
    let toolchain = EmbedToolchain::with_probe(dir.join("absent-python"), layout.probe);
    let result = run_lock_on(&path, false, InteropCli::default(), &toolchain, HOST);
    assert!(env_message(result).contains("could not run the lock interpreter"));
}

#[cfg(unix)]
#[test]
fn an_unreadable_libpython_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let dir = ScratchDir::new("lock_libpython").unwrap();
    let path = program(&dir, "import json\n");
    let layout = fake_layout(&layout_dir(&dir));
    let lines = probe::tests::probe_lines(&dir, &dir);
    let script = probe::tests::fake_interpreter(&dir, &format!("cat <<'PYCC'\n{lines}PYCC\n"));
    std::fs::set_permissions(layout.library(), std::fs::Permissions::from_mode(0o000)).unwrap();
    let toolchain = EmbedToolchain::with_probe(script, layout.probe.clone());
    let result = run_lock_on(&path, false, InteropCli::default(), &toolchain, HOST);
    std::fs::set_permissions(layout.library(), std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(env_message(result).contains("libpython3.14.dylib"));
}

#[test]
fn a_stale_reason_names_what_changed() {
    let old = section("m.py");
    assert!(stale_reason(None, Some(&old), 0).contains("no section"));
    assert!(stale_reason(Some(&old), None, 0).contains("not needed"));
    assert!(stale_reason(Some(&old), Some(&old), 2).contains("2 section(s)"));
    assert!(stale_reason(None, None, 0).contains("not in the form"));
    let mut new = old.clone();
    new.platform = "linux-x86_64".to_string();
    assert_eq!(
        stale_reason(Some(&old), Some(&new), 0),
        "`platform` is `macosx-11.0-arm64` in the lock but `linux-x86_64` in the environment"
    );
}

#[test]
fn the_first_difference_covers_roots_packages_and_natives() {
    let old = section("m.py");
    let mut roots = old.clone();
    roots.roots.push("other".to_string());
    assert!(first_difference(&old, &roots).starts_with("`roots`"));
    let mut added = old.clone();
    added.package.push(LockedPackage {
        name: "tinydep".to_string(),
        ..old.package[0].clone()
    });
    assert!(first_difference(&old, &added).contains("`tinydep` is in the closure but not locked"));
    assert!(first_difference(&added, &old).contains("`tinydep` is locked but not in the closure"));
    let mut changed = old.clone();
    changed.package[0].tree_sha256 = "cc".to_string();
    assert!(first_difference(&old, &changed).contains("`tinypkg` differs"));
    let mut native = old.clone();
    native.native.push(LockedNative {
        name: "libfoo.dylib".to_string(),
        sha256: "dd".to_string(),
        required_by: vec!["tinypkg".to_string()],
    });
    assert!(first_difference(&native, &old).contains("[[target.native]]"));
}
