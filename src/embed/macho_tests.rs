use super::*;

/// Captured from uv's CPython 3.14.7 (`lib/libpython3.14.dylib`): a dylib
/// lists its own id first.
const UV_LIBPYTHON: &str = "/u/lib/libpython3.14.dylib:
\t/u/lib/libpython3.14.dylib (compatibility version 3.14.0, current version 3.14.0)
\t/usr/lib/libSystem.B.dylib (compatibility version 1.0.0, current version 1351.0.0)
\t/System/Library/Frameworks/CoreFoundation.framework/Versions/A/CoreFoundation (compatibility version 150.0.0, current version 3038.1.255)
";

/// Captured from Homebrew's CPython 3.14.6 (`lib-dynload/_ssl`): an
/// extension module has no id.
const HOMEBREW_SSL: &str = "/h/lib-dynload/_ssl.cpython-314-darwin.so:
\t/opt/homebrew/opt/openssl@3/lib/libssl.3.dylib (compatibility version 3.0.0, current version 3.0.0)
\t/opt/homebrew/opt/openssl@3/lib/libcrypto.3.dylib (compatibility version 3.0.0, current version 3.0.0)
\t/usr/lib/libSystem.B.dylib (compatibility version 1.0.0, current version 1356.0.0)
";

#[test]
fn otool_output_parses_into_install_names() {
    assert_eq!(
        parse_otool_l(UV_LIBPYTHON),
        [
            "/u/lib/libpython3.14.dylib",
            "/usr/lib/libSystem.B.dylib",
            "/System/Library/Frameworks/CoreFoundation.framework/Versions/A/CoreFoundation",
        ]
    );
    assert_eq!(
        parse_otool_l(HOMEBREW_SSL),
        [
            "/opt/homebrew/opt/openssl@3/lib/libssl.3.dylib",
            "/opt/homebrew/opt/openssl@3/lib/libcrypto.3.dylib",
            "/usr/lib/libSystem.B.dylib",
        ]
    );
    assert_eq!(
        parse_otool_l("/x.so:\n\n\t@rpath/libz.dylib\n"),
        ["@rpath/libz.dylib"]
    );
    assert!(parse_otool_l("").is_empty());
}

/// The shape of `otool -L` on a universal image: a header per slice, each
/// slice repeating the same names.
const UNIVERSAL_DYLIB: &str = "/u/lib/libpython3.14.dylib (architecture x86_64):
	/u/lib/libpython3.14.dylib (compatibility version 3.14.0, current version 3.14.0)
	/usr/lib/libSystem.B.dylib (compatibility version 1.0.0, current version 1351.0.0)
/u/lib/libpython3.14.dylib (architecture arm64):
	/u/lib/libpython3.14.dylib (compatibility version 3.14.0, current version 3.14.0)
	/usr/lib/libSystem.B.dylib (compatibility version 1.0.0, current version 1351.0.0)
";

#[test]
fn universal_otool_output_skips_slice_headers_and_repeats() {
    assert_eq!(
        parse_otool_l(UNIVERSAL_DYLIB),
        ["/u/lib/libpython3.14.dylib", "/usr/lib/libSystem.B.dylib"]
    );
}

fn classify(dep: &str, own_id: Option<&str>) -> MachoDep {
    let bundle_lib = [
        PathBuf::from("/prefix/Python.framework/Versions/3.14/Python"),
        PathBuf::from("/real/prefix/Python.framework/Versions/3.14/Python"),
    ];
    classify_macho_dep(
        dep,
        Path::new(dep),
        own_id,
        Path::new("/prefix"),
        &bundle_lib,
        "libpython3.14.dylib",
    )
}

#[test]
fn system_libraries_and_bundled_names_are_kept() {
    assert_eq!(classify("/usr/lib/libSystem.B.dylib", None), MachoDep::Keep);
    assert_eq!(
        classify(
            "/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation",
            None
        ),
        MachoDep::Keep
    );
    assert_eq!(classify("@rpath/libpython3.14.dylib", None), MachoDep::Keep);
    assert_eq!(
        classify("@rpath/libssl.3.dylib", Some("@rpath/libssl.3.dylib")),
        MachoDep::Keep
    );
}

#[test]
fn the_source_libpython_is_rewritten_never_vendored_though_it_is_under_the_prefix() {
    assert_eq!(
        classify("/prefix/Python.framework/Versions/3.14/Python", None),
        MachoDep::RewriteToBundled
    );
    let by_realpath = classify_macho_dep(
        "/link/Python",
        Path::new("/real/prefix/Python.framework/Versions/3.14/Python"),
        None,
        Path::new("/prefix"),
        &[PathBuf::from(
            "/real/prefix/Python.framework/Versions/3.14/Python",
        )],
        "libpython3.14.dylib",
    );
    assert_eq!(by_realpath, MachoDep::RewriteToBundled);
}

#[test]
fn a_prefix_internal_library_is_vendored_and_anything_else_refused() {
    assert_eq!(
        classify("/prefix/lib/libssl.3.dylib", None),
        MachoDep::Vendor(PathBuf::from("/prefix/lib/libssl.3.dylib"))
    );
    assert_eq!(
        classify("/opt/homebrew/opt/openssl@3/lib/libssl.3.dylib", None),
        MachoDep::Refuse
    );
    assert_eq!(classify("@rpath/libtcl9.0.dylib", None), MachoDep::Refuse);
    assert_eq!(
        classify("/prefixed-sibling/lib/x.dylib", None),
        MachoDep::Refuse
    );
}

#[test]
fn the_tool_argument_lists_are_exact() {
    let image = Path::new("/b/lib/libpython3.14.dylib");
    assert_eq!(
        set_id_args("@rpath/libpython3.14.dylib", image),
        [
            "-id",
            "@rpath/libpython3.14.dylib",
            "/b/lib/libpython3.14.dylib"
        ]
    );
    assert_eq!(
        change_args("/old", "@rpath/new", image),
        [
            "-change",
            "/old",
            "@rpath/new",
            "/b/lib/libpython3.14.dylib"
        ]
    );
    assert_eq!(
        codesign_args(image),
        ["-f", "-s", "-", "/b/lib/libpython3.14.dylib"]
    );
}
