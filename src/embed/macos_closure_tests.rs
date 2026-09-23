//! The macOS relocation of locked-closure images (the pycc.lock decision
//! entry, rule 8; #1242) against real Mach-O images the test builds with
//! `cc`: each dependency arm on one build, and the two refusals. The
//! distribution is installed and locked by the test itself, so no
//! python3.14 runs.

use super::*;
use crate::embed::fake_layout::{cc, dylib, macho_library};

fn deps(image: &Path) -> Vec<String> {
    macho::parse_otool_l(
        &bundle::run_tool("otool", &[OsString::from("-L"), image.into()]).expect("otool"),
    )
}

/// Builds `<build>/<name>` (a `-dynamiclib` or `-bundle`), optionally with
/// an install name, linked against `links` and carrying `rpaths`, and
/// returns its bytes for the distribution's payload.
fn image(
    build: &Path,
    name: &str,
    kind: &str,
    id: Option<&str>,
    links: &[&Path],
    rpaths: &[&str],
) -> Vec<u8> {
    std::fs::create_dir_all(build).unwrap();
    let source = build.join(format!("{name}.c"));
    std::fs::write(
        &source,
        format!(
            "int pycc_fake_{}(void) {{ return 3; }}\n",
            name.replace('.', "_")
        ),
    )
    .unwrap();
    let out = build.join(name);
    let mut args: Vec<String> = vec![kind.into(), "-o".into(), out.display().to_string()];
    if let Some(id) = id {
        args.extend(["-install_name".to_string(), id.to_string()]);
    }
    args.push(source.display().to_string());
    args.extend(links.iter().map(|link| link.display().to_string()));
    args.extend(rpaths.iter().map(|rpath| format!("-Wl,-rpath,{rpath}")));
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    cc(build, &args);
    std::fs::read(out).unwrap()
}

fn macos_env(tag: &str) -> (Env, PathBuf) {
    let env = Env::bare(tag, "import tinynat\n");
    macho_library(&env.layout);
    let build = env.root.join("build");
    (env, build)
}

#[test]
fn every_closure_dependency_arm_relocates_on_one_build() {
    let (env, build) = macos_env("embed_macos_closure");
    let vendor = env.layout.prefix.join("lib").join("libvendor.dylib");
    dylib(&vendor, "pycc_fake_vendor", &[]);
    // Arm 0: each dylib keeps its own id.
    let helper = image(
        &build,
        "libhelper.dylib",
        "-dynamiclib",
        Some("@rpath/libhelper.dylib"),
        &[],
        &[],
    );
    let sib = image(
        &build,
        "libsib.dylib",
        "-dynamiclib",
        Some("@loader_path/.dylibs/libsib.dylib"),
        &[],
        &[],
    );
    // Arms 1, 2, 4 and 5: libSystem kept, the source libpython rewritten,
    // `@rpath` kept through its `@loader_path` rpath, the prefix library
    // vendored.
    let ext = image(
        &build,
        "ext_so",
        "-bundle",
        None,
        &[
            &build.join("libhelper.dylib"),
            &env.layout.library(),
            &vendor,
        ],
        &["@loader_path/.dylibs"],
    );
    // Arm 3: an `@loader_path` sibling in the image's own payload.
    let sib_user = image(
        &build,
        "sib_so",
        "-bundle",
        None,
        &[&build.join("libsib.dylib")],
        &[],
    );
    let class = [0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 0x34];
    write_dist(
        &env.plat,
        "tinynat",
        "1.0",
        &[
            ("tinynat/__init__.py", b"X = 1\n"),
            ("tinynat/.dylibs/libhelper.dylib", &helper),
            ("tinynat/.dylibs/libsib.dylib", &sib),
            ("tinynat/_ext.so", &ext),
            ("tinynat/_sib.so", &sib_user),
            ("tinynat/Main.class", &class),
            ("tinynat/tool", b"#!/bin/sh\n"),
        ],
        &[],
    );
    {
        use std::os::unix::fs::PermissionsExt;
        let tool = env.plat.join("tinynat/tool");
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    env.lock();
    env.embed_on(&env.toolchain(), EmbedPlatform::MacOs)
        .expect("embedded");

    let sidecar = env.sidecar();
    let closure = sidecar.join("closure").join("tinynat");
    let ext_deps = deps(&closure.join("_ext.so"));
    for expected in [
        "@rpath/libhelper.dylib",
        "@rpath/libpython3.14.dylib",
        "@loader_path/../../lib/libvendor.dylib",
        "/usr/lib/libSystem.B.dylib",
    ] {
        assert!(ext_deps.contains(&expected.to_string()), "{ext_deps:?}");
    }
    assert_eq!(
        deps(&sidecar.join("lib").join("libvendor.dylib"))[0],
        "@rpath/libvendor.dylib"
    );
    assert!(
        deps(&closure.join("_sib.so")).contains(&"@loader_path/.dylibs/libsib.dylib".to_string())
    );
    assert_eq!(
        deps(&closure.join(".dylibs/libhelper.dylib"))[0],
        "@rpath/libhelper.dylib"
    );
    assert_eq!(
        deps(&closure.join(".dylibs/libsib.dylib"))[0],
        "@loader_path/.dylibs/libsib.dylib"
    );
    bundle::run_tool(
        "codesign",
        &[OsString::from("-v"), closure.join("_ext.so").into()],
    )
    .expect("the rewritten image is signed again");
    // Neither a `.class` file nor a script is a Mach-O image; both are
    // copied byte for byte, the script still executable.
    assert_eq!(std::fs::read(closure.join("Main.class")).unwrap(), class);
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(closure.join("tool"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o755);
    }
}

/// An `@rpath` whose first rpath is absolute (it may resolve outside the
/// bundle on the machine that runs the program), and a library outside
/// the interpreter and the distribution, are both refused naming the
/// image, its distribution and #1243, with no sidecar left behind.
#[test]
fn an_unbundleable_closure_dependency_is_refused() {
    for case in ["rpath", "outside"] {
        let (env, build) = macos_env(&format!("embed_macos_closure_{case}"));
        let (dep, ext) = if case == "rpath" {
            let helper = image(
                &build,
                "libhelper.dylib",
                "-dynamiclib",
                Some("@rpath/libhelper.dylib"),
                &[],
                &[],
            );
            let ext = image(
                &build,
                "ext_so",
                "-bundle",
                None,
                &[&build.join("libhelper.dylib")],
                &["/pycc-test-nowhere/lib", "@loader_path/.dylibs"],
            );
            write_dist(
                &env.plat,
                "tinyhelp",
                "1.0",
                &[("tinynat/.dylibs/libhelper.dylib", &helper)],
                &[],
            );
            ("@rpath/libhelper.dylib".to_string(), ext)
        } else {
            let outside = build.join("outside").join("libout.dylib");
            dylib(&outside, "pycc_fake_out", &[]);
            let ext = image(&build, "ext_so", "-bundle", None, &[&outside], &[]);
            (outside.display().to_string(), ext)
        };
        write_dist(
            &env.plat,
            "tinynat",
            "1.0",
            &[
                ("tinynat/__init__.py", b"X = 1\n"),
                ("tinynat/_ext.so", &ext),
            ],
            &[],
        );
        env.lock();
        let err = env
            .embed_on(&env.toolchain(), EmbedPlatform::MacOs)
            .expect_err("not relocatable");
        assert!(
            err.contains("`closure/tinynat/_ext.so` (distribution `tinynat`)"),
            "{err}"
        );
        assert!(err.contains(&format!("depends on `{dep}`")), "{err}");
        assert!(err.contains("#1243"), "{err}");
        assert!(!env.sidecar().exists());
    }
}
