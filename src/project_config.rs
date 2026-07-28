use serde::Deserialize;

#[derive(Debug, Deserialize, PartialEq)]
pub struct PyccToml {
    pub project: ProjectSection,
    #[serde(default)]
    pub build: BuildSection,
}

#[derive(Debug, Deserialize, PartialEq)]
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

/// `pycc init [NAME]`: scaffolds a starter `pycc.toml` + `src/main.py` in
/// `dir`. `name` defaults to `dir`'s own file-name component when omitted,
/// matching how `cargo init`/`npm init` derive a project name from the
/// target directory.
pub fn scaffold(name: Option<&str>, dir: &std::path::Path) -> std::io::Result<()> {
    let project_name = name
        .map(str::to_string)
        .or_else(|| dir.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "myapp".to_string());

    let toml_contents = format!(
        "[project]\nname = \"{project_name}\"\nentry = \"src/main.py\"\npython = \"3.14\"\n"
    );
    // Self-check: the scaffolded file must itself satisfy `parse`'s own
    // validation (including D-012's exact-\"3.14\" check). This is an
    // infallible invariant of this hardcoded template -- no caller input
    // can make it fail -- so `.expect(...)` documents a real assertion
    // about the template staying valid, rather than threading a `Result`
    // no actual input can produce (see docs/TESTING.md's
    // `.expect()`-for-genuine-invariants convention). It also gives
    // `parse` a real production call site: `pycc build`'s own use of a
    // neighboring `pycc.toml`'s `build.opt` as a default profile is a
    // separate, larger consumption point that lands together with the
    // `--release` flag it depends on (this PR's Task 3).
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
    fn scaffold_falls_back_to_myapp_and_propagates_the_write_error_when_the_directory_has_no_file_name_and_does_not_exist()
     {
        // A path ending in ".." has no `file_name()` component at all,
        // exercising `scaffold`'s final `unwrap_or_else("myapp")` fallback.
        // It also never exists on any platform, so the subsequent
        // `std::fs::write` fails with a portable `NotFound` -- unlike a
        // permission-based approach (e.g. a filesystem root), which is not
        // reliably unwritable across every Tier-1 target's CI runner --
        // exercising the `?` error-propagation branch in the same test
        // (verified empirically: writing here returns
        // `Os { code: 2, kind: NotFound, .. }`).
        let dir = std::path::Path::new("/pycc-nonexistent-parent-for-scaffold-test-xyz/..");
        assert!(dir.file_name().is_none());

        let err = scaffold(None, dir).expect_err("scaffold must propagate the underlying io error");
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
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
