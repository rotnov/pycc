use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, PartialEq)]
pub struct PyccToml {
    pub project: ProjectSection,
    #[serde(default)]
    pub build: BuildSection,
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
pub struct ProjectSection {
    pub name: String,
    pub entry: String,
    pub python: String,
}

#[derive(Debug, Deserialize, PartialEq, Default)]
pub struct BuildSection {
    pub opt: Option<String>,
    pub targets: Option<Vec<String>>,
    #[serde(rename = "static")]
    pub static_: Option<bool>,
}

/// v1 accepts exactly Python 3.14 (D-012) -- a `pycc.toml` naming any other
/// version is a validation error, not a silent accept of an unsupported
/// language level.
pub fn parse(contents: &str) -> Result<PyccToml, String> {
    let config: PyccToml = toml::from_str(contents).map_err(|e| e.to_string())?;
    if config.project.python != "3.14" {
        return Err(format!(
            "pycc.toml: unsupported python version `{}` -- v1 accepts exactly \"3.14\" (D-012)",
            config.project.python
        ));
    }
    Ok(config)
}

/// Serialization-only wrapper matching `pycc.toml`'s `[project]` table
/// shape. `scaffold` writes only this section (never `[build]`, which
/// `PyccToml`'s own `BuildSection` -- all `Option` fields -- would need
/// `#[serde(skip_serializing_if = "Option::is_none")]` to serialize at
/// all, since the `toml` crate has no representation for TOML-absent
/// `None` values). Reusing `ProjectSection` directly (rather than
/// hand-formatting a string) is what makes `project_name` safe to
/// serialize regardless of its content -- see `scaffold`'s doc comment.
#[derive(Serialize)]
struct ScaffoldToml<'a> {
    project: &'a ProjectSection,
}

/// `pycc init [NAME]`: scaffolds a starter `pycc.toml` + `src/main.py` in
/// `dir`. `name` defaults to `dir`'s own file-name component when omitted,
/// matching how `cargo init`/`npm init` derive a project name from the
/// target directory.
///
/// Refusal contract (#237): before writing anything, every destination is
/// inspected, and the scaffold returns an `ErrorKind::AlreadyExists` error
/// -- "`pycc.toml` already exists", "`src` exists and is not a directory",
/// or "`src/main.py` already exists" -- leaving all existing paths
/// byte-for-byte unchanged. An existing `src` *directory* is not itself a
/// conflict (only its entry type and `main.py`'s presence are checked). A
/// missing `dir` is created. `pycc.toml` is written last, with
/// `create_new`, so a late failure never leaves it behind and a dangling
/// symlink at either file destination fails cleanly instead of writing
/// through to the symlink's target.
pub fn scaffold(name: Option<&str>, dir: &std::path::Path) -> std::io::Result<()> {
    let project_name = name
        .map(str::to_string)
        .or_else(|| dir.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "myapp".to_string());

    let project = ProjectSection {
        name: project_name,
        entry: "src/main.py".to_string(),
        python: "3.14".to_string(),
    };
    // Serializing through `toml::to_string` (rather than hand-formatting
    // `name = "{project_name}"` into a string) is what makes this safe
    // for *any* `project_name`, including one containing a `"` or `\`
    // (from an explicit `pycc init 'my"app'` or an oddly-named target
    // directory) -- the `toml` crate picks a literal (single-quoted)
    // string or escapes as needed, verified empirically (`name =
    // 'my"app\weird'` round-trips through `parse` unchanged). A hand-
    // formatted string would instead emit syntactically invalid TOML for
    // such a name, and the self-check below would panic instead of
    // returning a clean error.
    let toml_contents = toml::to_string(&ScaffoldToml { project: &project })
        .expect("ProjectSection must always serialize to valid TOML");
    // Self-check: the scaffolded file must itself satisfy `parse`'s own
    // validation (including D-012's exact-\"3.14\" check). This is a
    // genuine invariant of this function's own construction -- `entry`
    // and `python` are hardcoded literals, and `name` is safely escaped
    // above regardless of its content -- so `.expect()` documents a real
    // assertion rather than threading a `Result` no actual input can
    // produce (see docs/TESTING.md's `.expect()`-for-genuine-invariants
    // convention). It also gives `parse` a real production call site:
    // `pycc build`'s own use of a neighboring `pycc.toml`'s `build.opt`
    // as a default profile is a separate, larger consumption point that
    // lands together with the `--release` flag it depends on (this PR's
    // Task 3).
    parse(&toml_contents).expect("scaffold's own generated pycc.toml must parse and validate");

    // #237: inspect every destination before writing anything, so an
    // existing project is never silently replaced (the pre-check phase) and
    // a later failure can never leave a half-written scaffold over user
    // files (the write order below). Checks use follow-symlink semantics
    // deliberately: a symlink to a real file is user content and must
    // refuse, and a symlink to a directory is a usable `src`. A *dangling*
    // symlink passes these checks (its target does not exist), which is
    // why both file writes below use `create_new`: plain `fs::write` would
    // follow such a symlink and could silently create the file *outside*
    // the target directory when the symlink's target parent exists -- on
    // POSIX platforms `create_new` refuses the existing symlink entry
    // instead (O_EXCL never resolves symlinks; the Windows reparse-point
    // path shares W3's accepted cfg(unix) test gap). The checks
    // are a single-process guarantee, not an atomic one: nothing locks the
    // directory between check and write, the same non-atomicity
    // `cargo init`-family tools accept; `create_new` narrows that window
    // for the two files.
    let toml_path = dir.join("pycc.toml");
    let src_dir = dir.join("src");
    let main_py_path = src_dir.join("main.py");
    if toml_path.try_exists()? {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "`pycc.toml` already exists",
        ));
    }
    match std::fs::metadata(&src_dir) {
        Ok(meta) if !meta.is_dir() => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "`src` exists and is not a directory",
            ));
        }
        // An existing `src` *directory* is not by itself a conflict:
        // `create_dir_all` below tolerates it, and an empty `src/` next to
        // no `pycc.toml`/`main.py` carries no user content to destroy.
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    if main_py_path.try_exists()? {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "`src/main.py` already exists",
        ));
    }

    // Write order (#237): `pycc.toml` goes LAST, so any late failure in
    // the `src` steps leaves it uncreated -- the strongest form of the
    // issue's "a late write failure leaves `pycc.toml` unchanged"
    // regression. The worst a late failure can leave behind is an empty
    // `src/` directory, which contains no user data (the same residue
    // `cargo init`-family tools accept).
    std::fs::create_dir_all(&src_dir)?;
    let main_py = "def main() -> None:\n    print(\"hello from pycc\")\n";
    write_new(&main_py_path, main_py.as_bytes())?;
    write_new(&toml_path, toml_contents.as_bytes())?;
    Ok(())
}

/// `fs::write` with `create_new`: fails if the path already names any
/// entry, including a dangling symlink, instead of following it (#237).
/// When the write itself fails after `create_new` succeeded, the freshly
/// created file is removed (best-effort), so a quota or I/O failure cannot
/// leave behind an empty/partial `main.py` that makes the next `pycc init`
/// refuse our own residue, or a corrupt partial `pycc.toml` -- `create_new`
/// guarantees the entry is ours to delete (PR #253's review).
fn write_new(path: &std::path::Path, contents: &[u8]) -> std::io::Result<()> {
    write_new_in(path, contents, |file, contents| {
        use std::io::Write;
        file.write_all(contents)
    })
}

/// Testable core of [`write_new`], following the same dependency-injection
/// split as `find_pycc_rt_lib_dir`/`find_pycc_rt_lib_dir_in` in
/// `src/main.rs`: the injected writer lets a test exercise the
/// cleanup-on-write-failure path deterministically, which no portable
/// filesystem setup can trigger for a real `write_all`. A plain `fn`
/// pointer (not `impl FnOnce`) on purpose: a generic parameter would
/// monomorphize one copy per closure, and the production copy's
/// cleanup branch would count as uncovered under D-014's region gate even
/// though the test copy exercises the same source.
fn write_new_in(
    path: &std::path::Path,
    contents: &[u8],
    write: fn(&mut std::fs::File, &[u8]) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    let result = write(&mut file, contents);
    if result.is_err() {
        // Close the handle before removing -- Windows cannot delete an
        // open file. Best-effort: the write error stays the reported one.
        drop(file);
        let _ = std::fs::remove_file(path);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_minimal_valid_pycc_toml() {
        let toml = r#"
[project]
name = "myapp"
entry = "src/main.py"
python = "3.14"

[build]
opt = "release"
targets = ["x86_64-unknown-linux-gnu"]
static = true
"#;
        let config = parse(toml).expect("valid pycc.toml should parse");
        assert_eq!(config.project.name, "myapp");
        assert_eq!(config.project.entry, "src/main.py");
        assert_eq!(config.project.python, "3.14");
        assert_eq!(config.build.opt.as_deref(), Some("release"));
        assert_eq!(
            config.build.targets.as_deref(),
            Some(&["x86_64-unknown-linux-gnu".to_string()][..])
        );
        assert_eq!(config.build.static_, Some(true));
    }

    #[test]
    fn rejects_an_unsupported_python_version() {
        let toml = r#"
[project]
name = "myapp"
entry = "src/main.py"
python = "3.15"
"#;
        let err = parse(toml).expect_err("python != 3.14 must be rejected in v1");
        assert!(err.contains("3.14"), "error should mention the only supported version: {err}");
    }

    #[test]
    fn accepts_a_file_with_not_yet_implemented_sections() {
        // [interop] and [test] are documented in docs/CLI_SPEC.md for later
        // milestones -- a file using the planned D-114 policy schema must
        // still parse today even though neither section is consumed yet.
        let toml = r#"
[project]
name = "myapp"
entry = "src/main.py"
python = "3.14"

[interop]
policy = "allowlist"
allow = ["numpy"]

[test]
paths = ["tests/"]
"#;
        parse(toml).expect("documented-but-not-yet-consumed sections must not fail parsing");
    }

    #[test]
    fn rejects_malformed_toml_syntax() {
        let err = parse("this is not [valid toml").expect_err("malformed TOML must be rejected");
        assert!(!err.is_empty());
    }

    #[test]
    fn scaffold_writes_a_valid_pycc_toml_and_main_py() {
        let dir = std::env::temp_dir().join(format!("pycc_init_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        scaffold(Some("scaffoldtest"), &dir).expect("scaffold should succeed");

        let toml_contents = std::fs::read_to_string(dir.join("pycc.toml")).unwrap();
        let config = parse(&toml_contents).expect("scaffolded pycc.toml must itself parse");
        assert_eq!(config.project.name, "scaffoldtest");

        let main_py = std::fs::read_to_string(dir.join("src").join("main.py")).unwrap();
        assert!(main_py.contains("def main"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scaffold_derives_the_project_name_from_the_directory_when_none_is_given() {
        // `name: None` with a directory that does have a `file_name()`
        // component -- this exercises `scaffold`'s middle name-resolution
        // branch (derive from the target directory), distinct from both
        // the explicit-name test above and the no-file-name fallback test
        // below.
        let dir = std::env::temp_dir().join(format!("pycc_derived_name_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        scaffold(None, &dir).expect("scaffold should succeed");

        let toml_contents = std::fs::read_to_string(dir.join("pycc.toml")).unwrap();
        let config = parse(&toml_contents).expect("scaffolded pycc.toml must itself parse");
        let expected_name = dir.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(config.project.name, expected_name);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scaffold_falls_back_to_myapp_when_the_directory_has_no_file_name() {
        // `existing_subdir.join("..")` has no `file_name()` component at
        // all (it ends in ".."), exercising `scaffold`'s final
        // `unwrap_or_else("myapp")` fallback -- but unlike a bare
        // nonexistent-parent path, it resolves (on every Tier-1 platform)
        // to `existing_subdir`'s own real, already-existing, writable
        // parent directory, since both a lexical `..`-collapsing resolver
        // (Windows) and a component-by-component one (POSIX) agree on the
        // answer once every intermediate component genuinely exists
        // (verified empirically: writing there succeeds and lands in the
        // parent). This keeps the test independent of the filesystem-
        // mutation injection scenarios covered further below.
        let dir = std::env::temp_dir().join(format!("pycc_myapp_fallback_{}", std::process::id()));
        let existing_subdir = dir.join("existing_subdir");
        std::fs::create_dir_all(&existing_subdir).unwrap();
        let target = existing_subdir.join("..");
        assert!(target.file_name().is_none());

        scaffold(None, &target).expect("scaffold should succeed in the existing parent");

        let toml_contents = std::fs::read_to_string(dir.join("pycc.toml")).unwrap();
        let config = parse(&toml_contents).expect("scaffolded pycc.toml must itself parse");
        assert_eq!(config.project.name, "myapp");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scaffold_creates_a_missing_target_directory() {
        // Behavior change under #237's inverted write order: the first
        // mutation is now `create_dir_all(dir/src)`, which creates the
        // missing target directory and its parents, so scaffolding into a
        // nonexistent directory succeeds (matching `cargo init`-family
        // behavior). The public `pycc init` surface is unaffected -- it
        // always passes the existing current directory. This test used to
        // pin the old order's `NotFound` from writing `pycc.toml` first;
        // the Err arms are now covered by the injection trio below.
        let dir = std::env::temp_dir().join(format!(
            "pycc_nonexistent_target_dir_{}",
            std::process::id()
        ));
        std::fs::remove_dir_all(&dir).ok();
        assert!(!dir.exists());

        scaffold(Some("x"), &dir).expect("scaffold should create the missing directory");
        assert!(dir.join("pycc.toml").is_file());
        assert!(dir.join("src").join("main.py").is_file());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scaffold_handles_a_project_name_containing_toml_special_characters() {
        // A name with a `"` and a `\` would produce syntactically invalid
        // TOML if hand-formatted into a string literal -- this is exactly
        // the case the `toml::to_string`-based serialization in `scaffold`
        // (rather than `format!`) exists to handle safely: the round trip
        // through `parse` must recover the exact original name, and
        // `scaffold` must not panic.
        let dir = std::env::temp_dir().join(format!("pycc_special_name_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tricky_name = "my\"app\\weird";

        scaffold(Some(tricky_name), &dir).expect("scaffold must not panic on a special-char name");

        let toml_contents = std::fs::read_to_string(dir.join("pycc.toml")).unwrap();
        let config = parse(&toml_contents).expect("scaffolded pycc.toml must itself parse");
        assert_eq!(config.project.name, tricky_name);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scaffold_refuses_when_src_exists_as_a_plain_file() {
        // #237: `dir/src` existing as a regular file is caught by the
        // pre-check phase (it used to surface later, as `create_dir_all`'s
        // own error after `pycc.toml` had already been replaced). Nothing
        // is written on refusal.
        let dir = std::env::temp_dir().join(format!("pycc_src_conflict_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("src"), "not a directory").unwrap();

        let err = scaffold(Some("x"), &dir).expect_err("scaffold must refuse a non-directory src");
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(err.to_string(), "`src` exists and is not a directory");
        assert!(!dir.join("pycc.toml").exists());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scaffold_refuses_when_main_py_already_exists() {
        // #237: `dir/src/main.py` already existing (here as a directory,
        // any entry type counts) is caught by the pre-check phase, and
        // nothing is written -- it used to surface as the final write's
        // own error after `pycc.toml` had already been replaced.
        let dir = std::env::temp_dir().join(format!("pycc_main_py_conflict_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src").join("main.py")).unwrap();

        let err = scaffold(Some("x"), &dir).expect_err("scaffold must refuse an existing main.py");
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(err.to_string(), "`src/main.py` already exists");
        assert!(!dir.join("pycc.toml").exists());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scaffold_refuses_when_pycc_toml_already_exists() {
        // #237's headline defect: an existing `pycc.toml` was silently
        // replaced. Refusal must leave it byte-for-byte unchanged.
        let dir = std::env::temp_dir().join(format!("pycc_toml_conflict_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("pycc.toml"), "user content").unwrap();

        let err = scaffold(Some("x"), &dir).expect_err("scaffold must refuse an existing pycc.toml");
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(err.to_string(), "`pycc.toml` already exists");
        assert_eq!(
            std::fs::read_to_string(dir.join("pycc.toml")).unwrap(),
            "user content"
        );
        assert!(!dir.join("src").exists());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scaffold_accepts_a_preexisting_empty_src_directory() {
        // #237's deliberate non-conflict: an existing `src/` directory is
        // not itself a conflict -- only its entry type and `main.py`'s
        // presence are checked.
        let dir = std::env::temp_dir().join(format!("pycc_empty_src_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();

        scaffold(Some("x"), &dir).expect("an empty src directory is not a conflict");
        assert!(dir.join("pycc.toml").is_file());
        assert!(dir.join("src").join("main.py").is_file());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scaffold_accepts_a_populated_src_directory_without_main_py() {
        // The acceptance condition is deliberately "any directory without
        // a `main.py`", not emptiness: existing sibling files are left
        // untouched and scaffolded alongside.
        let dir = std::env::temp_dir().join(format!("pycc_populated_src_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src").join("lib.py"), "user lib").unwrap();

        scaffold(Some("x"), &dir).expect("a src directory without main.py is not a conflict");
        assert!(dir.join("pycc.toml").is_file());
        assert!(dir.join("src").join("main.py").is_file());
        assert_eq!(
            std::fs::read_to_string(dir.join("src").join("lib.py")).unwrap(),
            "user lib"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn write_new_removes_its_partial_file_when_the_write_fails() {
        // PR #253's review: a write failure after `create_new` succeeded
        // must not leave the freshly created entry behind. The injected
        // failing writer is the only portable way to trigger this path.
        let dir = std::env::temp_dir().join(format!("pycc_write_new_fail_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("partial.txt");

        let err = write_new_in(&path, b"contents", |_, _| {
            Err(std::io::Error::other("injected write failure"))
        })
        .expect_err("the injected write failure must propagate");
        assert_eq!(err.to_string(), "injected write failure");
        assert!(!path.exists(), "the partial file must be cleaned up");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn write_new_succeeds_and_keeps_the_file_on_a_clean_write() {
        let dir = std::env::temp_dir().join(format!("pycc_write_new_ok_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("whole.txt");

        write_new(&path, b"contents").expect("a clean write must succeed");
        assert_eq!(std::fs::read(&path).unwrap(), b"contents");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn scaffold_propagates_a_pycc_toml_existence_check_error() {
        // A self-referential `pycc.toml` symlink makes `try_exists` itself
        // fail (`ELOOP`), covering the first check's `?` propagation --
        // distinct from a dangling symlink, which resolves to Ok(false).
        let dir = std::env::temp_dir().join(format!("pycc_toml_eloop_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::os::unix::fs::symlink(dir.join("pycc.toml"), dir.join("pycc.toml")).unwrap();

        let err = scaffold(Some("x"), &dir).expect_err("the existence check must propagate ELOOP");
        assert!(!err.to_string().is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn scaffold_propagates_a_src_metadata_error_other_than_not_found() {
        // A self-referential `src` symlink makes `metadata` fail with
        // `ELOOP` -- neither a usable directory nor `NotFound` -- covering
        // the check's error-propagation arm.
        let dir = std::env::temp_dir().join(format!("pycc_src_eloop_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::os::unix::fs::symlink(dir.join("src"), dir.join("src")).unwrap();

        let err = scaffold(Some("x"), &dir).expect_err("the src metadata check must propagate");
        assert_ne!(err.kind(), std::io::ErrorKind::AlreadyExists);
        assert!(!dir.join("pycc.toml").exists());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn scaffold_propagates_a_main_py_existence_check_error() {
        // Same `ELOOP` shape for the third check: `src` is a real
        // directory, `src/main.py` is a self-referential symlink.
        let dir = std::env::temp_dir().join(format!("pycc_py_eloop_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::os::unix::fs::symlink(
            dir.join("src").join("main.py"),
            dir.join("src").join("main.py"),
        )
        .unwrap();

        let err = scaffold(Some("x"), &dir).expect_err("the main.py check must propagate ELOOP");
        assert!(!err.to_string().is_empty());
        assert!(!dir.join("pycc.toml").exists());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn scaffold_fails_cleanly_when_src_is_a_dangling_symlink() {
        // Injection 1 of #237's late-failure trio: a dangling `src`
        // symlink passes the follow-symlink pre-check (its target does not
        // exist), then `create_dir_all` fails on the symlink itself.
        // `pycc.toml` must not be created.
        let dir = std::env::temp_dir().join(format!("pycc_src_dangling_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::os::unix::fs::symlink(dir.join("nonexistent_target"), dir.join("src")).unwrap();

        let err = scaffold(Some("x"), &dir).expect_err("create_dir_all must fail on the symlink");
        assert!(!err.to_string().is_empty());
        assert!(!dir.join("pycc.toml").exists());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn scaffold_fails_cleanly_when_src_is_read_only() {
        // Injection 2: an existing empty `src/` passes the checks, the
        // directory-creation step is a no-op, and the `main.py` write
        // fails with a permission error. `pycc.toml` must not be created.
        // (Root ignores 0o555, so this would spuriously succeed under a
        // root test runner; hosted CI and the coverage sandbox's `nobody`
        // user are both non-root.)
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("pycc_src_readonly_{}", std::process::id()));
        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::set_permissions(&src, std::fs::Permissions::from_mode(0o555)).unwrap();

        let err = scaffold(Some("x"), &dir).expect_err("the main.py write must fail");
        assert!(!err.to_string().is_empty());
        assert!(!dir.join("pycc.toml").exists());

        std::fs::set_permissions(&src, std::fs::Permissions::from_mode(0o755)).ok();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn scaffold_leaves_pycc_toml_uncreated_when_its_own_write_fails_last() {
        // Injection 3, the issue's regression 4 at the last possible
        // moment: a dangling `pycc.toml` symlink passes the follow-symlink
        // pre-check (its target does not exist), both `src` steps succeed,
        // and the final `write_new` fails immediately with `EEXIST` on the
        // symlink entry itself -- `create_new` never follows it (POSIX
        // O_EXCL-on-symlink). The real `pycc.toml` path still resolves to
        // nothing afterwards.
        let dir = std::env::temp_dir().join(format!("pycc_toml_dangling_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::os::unix::fs::symlink(
            dir.join("missing_dir").join("pycc.toml"),
            dir.join("pycc.toml"),
        )
        .unwrap();

        let err = scaffold(Some("x"), &dir).expect_err("the final pycc.toml write must fail");
        assert!(!err.to_string().is_empty());
        // The write happened after both src steps...
        assert!(dir.join("src").join("main.py").is_file());
        // ...and pycc.toml still does not resolve to any file.
        assert!(!dir.join("pycc.toml").try_exists().unwrap());

        std::fs::remove_dir_all(&dir).ok();
    }
}
