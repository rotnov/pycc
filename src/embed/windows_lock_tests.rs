//! `pycc lock` and `plan_embed` on the Windows-shaped fake layout (#1296):
//! a Windows lock section identifies the interpreter by its DLL, and a
//! Windows build bundles the locked pure-Python closure in
//! `OUT.pycc\closure\`, refusing one that holds a PE image until #1297.
//! Both run on every host, so the coverage host drives every Windows arm;
//! the real Windows build is `tests/issue_1296_windows_locked_closure.rs`.

use super::super::super::fake_layout::fake_windows_layout;
use super::*;

const WINDOWS: (&str, &str) = ("x86_64", "windows");

/// An [`Env`] whose interpreter is the Windows-shaped fake layout, with
/// `tinypkg` requiring `tinydep` in purelib, plus `extra` files in
/// `tinypkg`.
fn windows_env(tag: &str, body: &str, extra: &[(&str, &[u8])]) -> Env {
    let dir = ScratchDir::new(tag).unwrap();
    let root = std::fs::canonicalize(&*dir).unwrap();
    let layout_root = root.join("layout");
    std::fs::create_dir_all(&layout_root).unwrap();
    let layout = fake_windows_layout(&layout_root);
    let (pure, plat) = (root.join("pure"), root.join("plat"));
    std::fs::create_dir_all(&pure).unwrap();
    std::fs::create_dir_all(&plat).unwrap();
    let mut files: Vec<(&str, &[u8])> = vec![("tinypkg/__init__.py", b"from tinydep import X\n")];
    files.extend_from_slice(extra);
    write_dist(&pure, "tinypkg", "1.0", &files, &["tinydep"]);
    write_dist(
        &pure,
        "tinydep",
        "2.0",
        &[("tinydep/__init__.py", b"X = 1\n")],
        &[],
    );
    let lock_probe = parse_lock_probe(&probe_lines(&pure, &plat)).unwrap();
    let project = root.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let entry = project.join("m.py");
    std::fs::write(&entry, body).unwrap();
    Env {
        root,
        _dir: dir,
        layout,
        pure,
        plat,
        lock_probe,
        entry,
    }
}

impl Env {
    fn lock_windows(&self) {
        crate::lock::run_lock_on(
            &self.entry,
            false,
            InteropCli::default(),
            &self.toolchain(),
            WINDOWS,
        )
        .unwrap_or_else(|_| panic!("`pycc lock` must succeed on Windows"));
    }

    fn lock_text(&self) -> String {
        std::fs::read_to_string(self.entry.with_file_name("pycc.lock")).unwrap()
    }

    fn embed_windows(&self) -> Result<EmbedPlan, String> {
        let hir = crate::frontend::lock_frontend(&self.entry, InteropCli::default())
            .unwrap_or_else(|_| panic!("the fixture must type-check"));
        plan_embed(
            &self.out(),
            &self.entry,
            &hir,
            &self.toolchain(),
            EmbedPlatform::Windows,
            WINDOWS,
            &self.obj(),
        )
    }
}

/// A Windows lock section records the interpreter DLL's digest and no
/// native library; the build bundles the closure beside `Lib\` and
/// `DLLs\` and defines `PYCC_EMBED_CLOSURE` for the launcher.
#[test]
fn a_windows_build_bundles_the_locked_closure() {
    let env = windows_env("embed_windows_lock_closure", "import tinypkg\n", &[]);
    env.lock_windows();
    let text = env.lock_text();
    assert!(text.contains("x86_64-pc-windows-msvc"), "{text}");
    let digest = sha256::sha256_hex(b"python314");
    assert!(
        text.contains(&format!("libpython-sha256 = \"{digest}\"")),
        "{text}"
    );
    assert!(!text.contains("[[target.native]]"), "{text}");
    env.embed_windows().expect("embedded");
    let files = relative_files(&env.sidecar());
    for rel in [
        "closure/tinypkg/__init__.py",
        "closure/tinydep/__init__.py",
        "closure/tinypkg-1.0.dist-info/METADATA",
        "Lib/os.py",
        "DLLs/_ssl.pyd",
        "python314.dll",
    ] {
        assert!(files.iter().any(|file| file == rel), "{rel}: {files:?}");
    }
    assert_eq!(
        std::fs::read(env.sidecar().join("closure/tinypkg/__init__.py")).unwrap(),
        b"from tinydep import X\n"
    );
    assert!(env.config().contains("#define PYCC_EMBED_CLOSURE"));
}

/// A standard-library-only program's section locks no package: the build
/// copies no closure and defines no `PYCC_EMBED_CLOSURE`.
#[test]
fn a_windows_standard_library_program_bundles_no_closure() {
    let env = windows_env("embed_windows_lock_stdlib", "import json\n", &[]);
    env.lock_windows();
    env.embed_windows().expect("embedded");
    let files = relative_files(&env.sidecar());
    assert!(
        !files.iter().any(|file| file.starts_with("closure/")),
        "{files:?}"
    );
    assert!(!env.config().contains("PYCC_EMBED_CLOSURE"));
}

/// A closure holding a PE image still locks, but the Windows build
/// refuses it before staging, naming the file, its distribution, #1297
/// and `pycc build --ext`; an `.exe` launcher with the `MZ` magic is
/// bundled as data.
#[test]
fn a_windows_build_refuses_a_closure_holding_a_pe_image() {
    for (rel, bytes) in [
        ("tinypkg/fast.cp314-win_amd64.pyd", &b"MZ\x90\x00"[..]),
        ("tinypkg/FAST.PYD", b"not an image"),
        ("tinypkg/sub/helper.dll", b"MZ"),
        ("tinypkg/lib/blob.bin", b"MZ\x90\x00"),
    ] {
        let env = windows_env(
            "embed_windows_lock_image",
            "import tinypkg\n",
            &[(rel, bytes)],
        );
        env.lock_windows();
        env.previous_sidecar();
        let err = env.embed_windows().expect_err(rel);
        for part in [rel, "`tinypkg`", "#1297", "pycc build --ext"] {
            assert!(err.contains(part), "{rel}: {part}: {err}");
        }
        env.assert_previous_sidecar_intact();
    }
    let env = windows_env(
        "embed_windows_lock_exe",
        "import tinypkg\n",
        &[
            ("tinypkg/cli.exe", b"MZ\x90\x00"),
            ("tinypkg/CLI2.EXE", b"MZ"),
        ],
    );
    env.lock_windows();
    env.embed_windows().expect("an `.exe` is bundled as data");
    assert_eq!(
        std::fs::read(env.sidecar().join("closure/tinypkg/cli.exe")).unwrap(),
        b"MZ\x90\x00"
    );
}

/// A locked file that vanished before the build is refused by the
/// payload's own missing-file check, which runs ahead of the image screen
/// and leaves the previous sidecar untouched.
#[test]
fn a_windows_build_reports_a_vanished_closure_file() {
    let env = windows_env("embed_windows_lock_vanished", "import tinypkg\n", &[]);
    env.lock_windows();
    std::fs::remove_file(env.pure.join("tinydep/__init__.py")).unwrap();
    env.previous_sidecar();
    let err = env.embed_windows().expect_err("vanished");
    assert!(err.contains("tinydep"), "{err}");
    env.assert_previous_sidecar_intact();
}

/// A closure file changed after the lock is refused by its RECORD digest,
/// and an interpreter DLL changed after the lock by `libpython-sha256`,
/// which is the digest of the very file the sidecar bundles; both leave
/// the previous sidecar untouched.
#[test]
fn a_windows_build_refuses_a_stale_closure_or_interpreter() {
    let env = windows_env("embed_windows_lock_stale", "import tinypkg\n", &[]);
    env.lock_windows();
    std::fs::write(env.pure.join("tinypkg/__init__.py"), "changed\n").unwrap();
    env.previous_sidecar();
    let err = env.embed_windows().expect_err("stale file");
    assert!(err.contains("does not match its RECORD hash"), "{err}");
    env.assert_previous_sidecar_intact();

    let env = windows_env("embed_windows_lock_stale_dll", "import tinypkg\n", &[]);
    env.lock_windows();
    std::fs::write(env.layout.prefix.join("python314.dll"), "rebuilt").unwrap();
    env.previous_sidecar();
    let err = env.embed_windows().expect_err("stale DLL");
    assert!(err.contains("libpython-sha256"), "{err}");
    env.assert_previous_sidecar_intact();
}
