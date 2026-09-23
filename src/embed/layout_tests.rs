use super::*;

fn probe() -> EmbedProbe {
    EmbedProbe {
        version: (3, 14, 7),
        executable: PathBuf::from("/opt/py/bin/python3.14"),
        include: PathBuf::from("/opt/py/include/python3.14"),
        stdlib: PathBuf::from("/opt/py/lib/python3.14"),
        base_prefix: PathBuf::from("/opt/py"),
        enable_shared: true,
        framework: String::new(),
        ldlibrary: "libpython3.14.so".to_string(),
        libdir: PathBuf::from("/nonexistent-pycc/lib"),
        instsoname: "libpython3.14.so.1.0".to_string(),
        gil_disabled: false,
    }
}

#[test]
fn the_host_platform_matches_the_build_host() {
    let expected = if cfg!(target_os = "macos") {
        EmbedPlatform::MacOs
    } else {
        EmbedPlatform::Linux
    };
    assert_eq!(EmbedPlatform::HOST, expected);
}

#[test]
fn the_sidecar_is_named_after_the_output_file() {
    assert_eq!(
        sidecar_name(Path::new("dist/app")),
        Ok("app.pycc".to_string())
    );
    assert_eq!(
        sidecar_name(Path::new("my app.bin")),
        Ok("my app.bin.pycc".to_string())
    );
}

#[test]
fn a_name_the_loader_cannot_carry_is_refused() {
    for bad in ["a$b", "a:b", "$ORIGIN"] {
        let message = sidecar_name(Path::new(bad)).expect_err("refused");
        assert!(message.contains("contains `$` or `:`"), "{message}");
    }
    let message = sidecar_name(Path::new("/")).expect_err("no file name");
    assert!(message.contains("names no file"), "{message}");
}

#[cfg(unix)]
#[test]
fn a_non_utf8_name_is_refused() {
    use std::os::unix::ffi::OsStrExt;
    let name = std::ffi::OsStr::from_bytes(b"app\xff");
    let message = sidecar_name(Path::new(name)).expect_err("not UTF-8");
    assert!(message.contains("is not UTF-8"), "{message}");
}

#[test]
fn the_sidecar_parent_of_a_bare_name_is_the_current_directory() {
    assert_eq!(sidecar_parent(Path::new("app")), Path::new("."));
    assert_eq!(sidecar_parent(Path::new("dist/app")), Path::new("dist"));
}

#[test]
fn the_rpath_is_relative_to_the_executable_on_each_platform() {
    assert_eq!(
        rpath_args(EmbedPlatform::MacOs, "app.pycc"),
        [
            "-Xlinker",
            "-rpath",
            "-Xlinker",
            "@executable_path/app.pycc/lib"
        ]
    );
    assert_eq!(
        rpath_args(EmbedPlatform::Linux, "app.pycc"),
        ["-Xlinker", "-rpath", "-Xlinker", "$ORIGIN/app.pycc/lib"]
    );
}

#[test]
fn the_source_library_follows_the_framework_or_the_plain_layout() {
    let plain = probe();
    assert_eq!(
        source_library(&plain),
        PathBuf::from("/nonexistent-pycc/lib/libpython3.14.so")
    );
    let framework = EmbedProbe {
        framework: "Python".to_string(),
        libdir: PathBuf::from("/F/Python.framework/Versions/3.14/lib"),
        ..probe()
    };
    assert_eq!(
        source_library(&framework),
        PathBuf::from("/F/Python.framework/Versions/3.14/Python")
    );
}

#[test]
fn the_bundled_library_name_depends_on_the_platform() {
    assert_eq!(
        bundled_library_name(EmbedPlatform::MacOs, &probe()),
        "libpython3.14.dylib"
    );
    assert_eq!(
        bundled_library_name(EmbedPlatform::Linux, &probe()),
        "libpython3.14.so.1.0"
    );
    let no_soname = EmbedProbe {
        instsoname: String::new(),
        ..probe()
    };
    assert_eq!(
        bundled_library_name(EmbedPlatform::Linux, &no_soname),
        "libpython3.14.so"
    );
    assert_eq!(stdlib_dir_name(&probe()), "python3.14");
}

#[test]
fn the_marker_records_the_version_the_interpreter_and_the_digest() {
    let text = marker_text(&probe(), "abc123");
    assert_eq!(
        text,
        "pycc-bundle 1\npython 3.14.7\nexecutable /opt/py/bin/python3.14\nlibpython-sha256 abc123\n"
    );
    assert!(marker_is_current(&text));
    assert!(!marker_is_current("pycc-bundle 2\n"));
    assert!(!marker_is_current(""));
}

#[test]
fn the_stdlib_copy_skips_exactly_the_filtered_paths() {
    for skipped in [
        "site-packages",
        "site-packages/numpy/__init__.py",
        "__pycache__",
        "json/__pycache__/decoder.cpython-314.pyc",
        "test",
        "test/test_os.py",
        "tkinter",
        "tkinter/ttk.py",
        "turtle.py",
        "idlelib",
        "turtledemo/__main__.py",
        "lib-dynload/_tkinter.cpython-314-darwin.so",
    ] {
        assert!(
            skip_in_stdlib_copy(Path::new(skipped)),
            "{skipped} is skipped"
        );
    }
    for kept in [
        "os.py",
        "json",
        "json/decoder.py",
        "lib-dynload",
        "lib-dynload/_json.cpython-314-darwin.so",
        "unittest/test/__init__.py",
        "turtledemo_notes.txt",
        "",
    ] {
        assert!(!skip_in_stdlib_copy(Path::new(kept)), "{kept} is kept");
    }
}

#[test]
fn a_c_string_literal_escapes_everything_outside_printable_ascii() {
    assert_eq!(c_string_literal("app.pycc"), "\"app.pycc\"");
    assert_eq!(c_string_literal("my app"), "\"my app\"");
    assert_eq!(c_string_literal("a\"b\\c"), "\"a\\042b\\134c\"");
    assert_eq!(c_string_literal("é1"), "\"\\303\\2511\"");
    assert_eq!(c_string_literal("a\nb"), "\"a\\012b\"");
    assert!(embed_config_inc("app.pycc").contains("#define PYCC_EMBED_SIDECAR \"app.pycc\"\n"));
}

#[test]
fn a_loader_relative_reference_climbs_to_the_library_directory() {
    assert_eq!(
        loader_relative(Path::new("libvendor.dylib"), "libinner.dylib"),
        "@loader_path/libinner.dylib"
    );
    assert_eq!(
        loader_relative(
            Path::new("python3.14/lib-dynload/_ssl.so"),
            "libssl.3.dylib"
        ),
        "@loader_path/../../libssl.3.dylib"
    );
}
