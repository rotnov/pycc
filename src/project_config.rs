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
    std::fs::write(dir.join("pycc.toml"), toml_contents)?;

    std::fs::create_dir_all(dir.join("src"))?;
    let main_py = "def main() -> None:\n    print(\"hello from pycc\")\n";
    std::fs::write(dir.join("src").join("main.py"), main_py)?;
    Ok(())
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
        // milestones -- a file using the full schema must still parse today.
        let toml = r#"
[project]
name = "myapp"
entry = "src/main.py"
python = "3.14"

[interop]
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
        // parent). This keeps the test independent of the more
        // OS-sensitive assumption used for the "the write itself fails"
        // test below.
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
    fn scaffold_propagates_the_pycc_toml_write_error_when_the_target_directory_does_not_exist() {
        // No parent-directory tricks here: `dir` simply never exists (and
        // is never created), so `std::fs::write(dir.join("pycc.toml"),
        // ..)` fails because its immediate parent doesn't exist -- a
        // plain `NotFound` on every Tier-1 platform, independent of any
        // `..`-resolution semantics. `name` is explicit so this test
        // exercises only the first `?` (the `pycc.toml` write), not the
        // name-resolution branches covered by the tests above.
        let dir = std::env::temp_dir().join(format!(
            "pycc_nonexistent_target_dir_{}",
            std::process::id()
        ));
        assert!(!dir.exists());

        let err =
            scaffold(Some("x"), &dir).expect_err("scaffold must propagate the underlying io error");
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
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
    fn scaffold_propagates_the_error_when_src_already_exists_as_a_plain_file() {
        // `dir/src` already existing as a regular file (not a directory)
        // makes `create_dir_all(dir.join("src"))` fail deterministically on
        // every Tier-1 target -- a directory/file entry-type conflict is a
        // universal filesystem property, unlike a permission-based
        // approach (verified empirically: `AlreadyExists`/"File exists").
        // `dir/pycc.toml` itself writes successfully first, so this
        // exercises the second `?` (the `create_dir_all` call), distinct
        // from the test above, which fails at the first `?`.
        let dir = std::env::temp_dir().join(format!("pycc_src_conflict_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("src"), "not a directory").unwrap();

        let err = scaffold(Some("x"), &dir).expect_err("scaffold must propagate the mkdir error");
        assert!(!err.to_string().is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scaffold_propagates_the_error_when_main_py_already_exists_as_a_directory() {
        // `dir/src/main.py` already existing as a directory makes the
        // final `std::fs::write` fail deterministically on every Tier-1
        // target (verified empirically: `IsADirectory`/"Is a directory"),
        // exercising the third `?` -- the earlier `pycc.toml` write and
        // `create_dir_all(dir/src)` (a no-op since it already exists) both
        // succeed first.
        let dir = std::env::temp_dir().join(format!("pycc_main_py_conflict_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src").join("main.py")).unwrap();

        let err = scaffold(Some("x"), &dir).expect_err("scaffold must propagate the write error");
        assert!(!err.to_string().is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }
}
