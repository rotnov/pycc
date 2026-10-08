//! The loader's answer to an absolute import of the entry module's own
//! top-level package under `pycc build --ext --foreign-relative-imports`
//! (#1382): [`Resolution::Foreign`] without a filesystem probe, so the
//! package's modules are never linked natively. Every other module, mode
//! and package keeps its answer.

use super::*;

/// Writes the package tree `top` (with `top.other` and the package `top.pkg`)
/// under `scratch`, plus the entry `top/pkg/m.py` holding `body`, and
/// returns the entry's path.
fn package_tree(scratch: &Path, body: &str) -> PathBuf {
    write(scratch, "top/__init__.py", "");
    write(scratch, "top/other.py", "y: int = 7\n");
    write(scratch, "top/pkg/__init__.py", "");
    write(scratch, "top/pkg/m.py", body)
}

/// The file names of every module `program` loaded, in load order.
fn file_names(program: &LoadedProgram) -> Vec<String> {
    program
        .modules
        .iter()
        .map(|module| {
            Path::new(&module.display_path)
                .file_name()
                .expect("a loaded module has a file name")
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

fn must_load_foreign(entry: &Path) -> LoadedProgram {
    load_foreign(entry).unwrap_or_else(|failure| panic!("must load: {}", describe(&failure)))
}

/// In the full tree, `from top.other import y` and `from top import other`
/// bind CPython's objects and load nothing: neither `top/other.py` nor
/// `top/__init__.py` is linked into the artifact.
#[test]
fn an_entry_import_of_its_own_package_is_foreign_in_the_full_tree() {
    let scratch = ScratchDir::new("modules_tests").expect("scratch");
    let entry = package_tree(
        &scratch,
        "from top.other import y\nfrom top import other\nfrom top.pkg.m2 import z\n",
    );
    let program = must_load_foreign(&entry);
    assert_eq!(file_names(&program), vec!["m.py".to_string()]);
    assert_eq!(
        entry_foreign_from_imports(&program),
        vec![
            ("top.other".to_string(), "y".to_string(), 0),
            ("top".to_string(), "other".to_string(), 0),
            ("top.pkg.m2".to_string(), "z".to_string(), 0),
        ]
    );
}

/// In the skeleton tree (only the `__init__.py` files), the dotted import
/// that used to keep its `C0001` (the probe missed past the first segment)
/// is foreign too: the answer never depends on what the package holds.
#[test]
fn an_entry_import_of_its_own_package_is_foreign_in_the_skeleton_tree() {
    let scratch = ScratchDir::new("modules_tests").expect("scratch");
    write(&scratch, "top/__init__.py", "");
    write(&scratch, "top/pkg/__init__.py", "");
    let entry = write(&scratch, "top/pkg/m.py", "from top.exceptions import Bad\n");
    let program = must_load_foreign(&entry);
    assert_eq!(
        entry_foreign_from_imports(&program),
        vec![("top.exceptions".to_string(), "Bad".to_string(), 0)]
    );
}

/// The plain forms bind foreign as well: `import top`, `import top.other`
/// and `import top.other as o` (#1381's shapes), each without loading a
/// file.
#[test]
fn a_plain_entry_import_of_its_own_package_is_foreign() {
    let scratch = ScratchDir::new("modules_tests").expect("scratch");
    let entry = package_tree(
        &scratch,
        "import top\nimport top.other\nimport top.other as o\n",
    );
    let program = must_load_foreign(&entry);
    assert_eq!(file_names(&program), vec!["m.py".to_string()]);
    let modules: Vec<String> = entry_foreign_plain_imports(&program)
        .into_iter()
        .map(|(_, module)| module)
        .collect();
    assert_eq!(
        modules,
        vec![
            "top".to_string(),
            "top.other".to_string(),
            "top.other".to_string()
        ]
    );
}

/// A from-import nested in a module-level `if`/`try` body (#1383) is
/// answered foreign the same way, where a nested project import would keep
/// its block-body `C0001`.
#[test]
fn a_nested_entry_import_of_its_own_package_is_foreign() {
    let scratch = ScratchDir::new("modules_tests").expect("scratch");
    let entry = package_tree(
        &scratch,
        "c = True\nif c:\n    from top.other import y\ntry:\n    \
         from top import other\nexcept ImportError:\n    pass\n",
    );
    let program = must_load_foreign(&entry);
    assert_eq!(file_names(&program), vec!["m.py".to_string()]);
    assert_eq!(
        entry_foreign_from_imports(&program),
        vec![
            ("top.other".to_string(), "y".to_string(), 0),
            ("top".to_string(), "other".to_string(), 0),
        ]
    );
}

/// Without the flag (every command but `pycc build --ext
/// --foreign-relative-imports`) D-222 is unchanged: the same import links
/// the package's `__init__.py` and the module natively.
#[test]
fn without_the_flag_an_own_package_import_stays_a_project_import() {
    let scratch = ScratchDir::new("modules_tests").expect("scratch");
    let entry = package_tree(&scratch, "from top.other import y\n");
    assert_eq!(
        loaded_paths(&entry),
        vec![
            "__init__.py".to_string(),
            "other.py".to_string(),
            "m.py".to_string()
        ]
    );
}

/// Only the entry's own top-level package is foreign. Another project
/// package beside it stays a project import, including one whose name merely
/// starts with the package's name: the root is compared segment by segment.
#[test]
fn another_project_package_stays_a_project_import() {
    let scratch = ScratchDir::new("modules_tests").expect("scratch");
    write(&scratch, "topx/__init__.py", "");
    write(&scratch, "topx/a.py", "b: int = 1\n");
    let entry = package_tree(&scratch, "from topx.a import b\n");
    let program = must_load_foreign(&entry);
    assert_eq!(
        file_names(&program),
        vec![
            "__init__.py".to_string(),
            "a.py".to_string(),
            "m.py".to_string()
        ]
    );
    assert!(entry_foreign_from_imports(&program).is_empty());
}

/// The opt-in is the entry module's alone: a dependency outside the package
/// that imports `top.other` still links it natively (D-222), as a
/// dependency's relative import does under #1366.
#[test]
fn a_dependency_import_of_the_entry_package_stays_a_project_import() {
    let scratch = ScratchDir::new("modules_tests").expect("scratch");
    write(
        &scratch,
        "helper.py",
        "from top.other import y\n\n\ndef twice() -> int:\n    return y * 2\n",
    );
    let entry = package_tree(&scratch, "from helper import twice\n");
    let program = must_load_foreign(&entry);
    assert_eq!(
        file_names(&program),
        vec![
            "__init__.py".to_string(),
            "other.py".to_string(),
            "helper.py".to_string(),
            "m.py".to_string()
        ]
    );
}

/// An entry whose directory is not a package has no own package, so an
/// absolute import of a project package beside it stays a project import
/// even under the flag.
#[test]
fn an_entry_outside_any_package_keeps_its_project_imports() {
    let scratch = ScratchDir::new("modules_tests").expect("scratch");
    write(&scratch, "top/__init__.py", "");
    write(&scratch, "top/other.py", "y: int = 7\n");
    let entry = write(&scratch, "main.py", "from top.other import y\n");
    let program = must_load_foreign(&entry);
    assert_eq!(
        file_names(&program),
        vec![
            "__init__.py".to_string(),
            "other.py".to_string(),
            "main.py".to_string()
        ]
    );
}

/// The own package is the *outermost* `__init__.py` directory, read from
/// the tree alone; a directory without one ends the climb.
#[test]
fn the_top_level_package_is_the_outermost_package_directory() {
    let scratch = ScratchDir::new("modules_tests").expect("scratch");
    let entry = package_tree(&scratch, "");
    assert_eq!(top_level_package(&entry), Some("top".to_string()));
    let top_init = scratch.join("top/__init__.py");
    assert_eq!(top_level_package(&top_init), Some("top".to_string()));
    let outside = write(&scratch, "main.py", "");
    assert_eq!(top_level_package(&outside), None);
}

/// An entry whose only non-`pycc_std` imports name its own package never
/// discovers a source root, so a malformed `pycc.toml` above it is neither
/// read nor reported (`docs/CLI_SPEC.md`). Without the flag the same import
/// is a project import, which runs discovery and reports the manifest.
#[test]
fn an_own_package_import_skips_source_root_discovery() {
    let scratch = ScratchDir::new("modules_tests").expect("scratch");
    write(&scratch, "pycc.toml", "this is not toml [\n");
    let entry = package_tree(&scratch, "from top.other import y\n");
    let program = must_load_foreign(&entry);
    assert!(program.manifest.is_none());
    let (path, _) = input_failure(&entry);
    assert!(path.ends_with("pycc.toml"), "{path}");
}
