//! `plan_embed` consuming `pycc.lock` (#1242): the locked closure lands in
//! `OUT.pycc/closure/` with the launcher's `PYCC_EMBED_CLOSURE` define, and
//! every refusal leaves a previous sidecar untouched. Each lock is written
//! by `pycc lock` itself against a fake interpreter layout and fake site
//! directories, and the Linux arm runs no Mach-O tool, so no python3.14 and
//! no `otool` run.

use super::super::fake_layout::{FakeLayout, fake_layout};
use super::super::*;
use super::relative_files;
use crate::interop_policy::InteropCli;
use crate::lock::fixture::write_dist;
use crate::lock::probe::{LockProbe, parse_lock_probe, tests::probe_lines};
use pycc_scratch::ScratchDir;

const HOST: (&str, &str) = ("aarch64", "macos");

struct Env {
    root: PathBuf,
    _dir: ScratchDir,
    layout: FakeLayout,
    pure: PathBuf,
    /// Read only by the macOS closure-image tests in `macos_closure_tests`.
    #[cfg_attr(not(target_os = "macos"), expect(dead_code))]
    plat: PathBuf,
    lock_probe: LockProbe,
    entry: PathBuf,
}

impl Env {
    /// `tinypkg` requiring `tinydep`, both in purelib.
    fn new(tag: &str, body: &str) -> Self {
        let env = Self::bare(tag, body);
        write_dist(
            &env.pure,
            "tinypkg",
            "1.0",
            &[("tinypkg/__init__.py", b"from tinydep import X\n")],
            &["tinydep"],
        );
        write_dist(
            &env.pure,
            "tinydep",
            "2.0",
            &[("tinydep/__init__.py", b"X = 1\n")],
            &[],
        );
        env
    }

    /// Empty site directories.
    fn bare(tag: &str, body: &str) -> Self {
        let dir = ScratchDir::new(tag).unwrap();
        let root = std::fs::canonicalize(&*dir).unwrap();
        let layout_root = root.join("layout");
        std::fs::create_dir_all(&layout_root).unwrap();
        let layout = fake_layout(&layout_root);
        let pure = root.join("pure");
        let plat = root.join("plat");
        std::fs::create_dir_all(&pure).unwrap();
        std::fs::create_dir_all(&plat).unwrap();
        let lock_probe = parse_lock_probe(&probe_lines(&pure, &plat)).unwrap();
        let project = root.join("project");
        std::fs::create_dir_all(&project).unwrap();
        let entry = project.join("m.py");
        std::fs::write(&entry, body).unwrap();
        Self {
            root,
            _dir: dir,
            layout,
            pure,
            plat,
            lock_probe,
            entry,
        }
    }

    fn toolchain(&self) -> EmbedToolchain {
        EmbedToolchain::with_probes("pyfake", self.layout.probe.clone(), self.lock_probe.clone())
    }

    fn lock(&self) {
        crate::lock::run_lock_on(
            &self.entry,
            false,
            InteropCli::default(),
            &self.toolchain(),
            HOST,
        )
        .unwrap_or_else(|_| panic!("`pycc lock` must succeed"));
    }

    fn out(&self) -> PathBuf {
        self.root.join("app")
    }

    fn sidecar(&self) -> PathBuf {
        self.root.join("app.pycc")
    }

    fn obj(&self) -> PathBuf {
        self.root.join("main.o")
    }

    fn embed_with(&self, toolchain: &EmbedToolchain) -> Result<EmbedPlan, String> {
        self.embed_on(toolchain, EmbedPlatform::Linux)
    }

    fn embed_on(
        &self,
        toolchain: &EmbedToolchain,
        platform: EmbedPlatform,
    ) -> Result<EmbedPlan, String> {
        let hir = crate::frontend::lock_frontend(&self.entry, InteropCli::default())
            .unwrap_or_else(|_| panic!("the fixture must type-check"));
        plan_embed(
            &self.out(),
            &self.entry,
            &hir,
            toolchain,
            platform,
            HOST,
            &self.obj(),
        )
    }

    fn embed(&self) -> Result<EmbedPlan, String> {
        self.embed_with(&self.toolchain())
    }

    fn config(&self) -> String {
        std::fs::read_to_string(self.obj().with_file_name(EMBED_CONFIG_INC_NAME)).unwrap()
    }

    /// A marked sidecar from an earlier build, for the refusals to leave
    /// untouched.
    fn previous_sidecar(&self) {
        std::fs::create_dir(self.sidecar()).unwrap();
        std::fs::write(self.sidecar().join(layout::MARKER_NAME), "pycc-bundle 1\n").unwrap();
        std::fs::write(self.sidecar().join("previous.txt"), "kept").unwrap();
    }

    fn assert_previous_sidecar_intact(&self) {
        assert_eq!(
            relative_files(&self.sidecar()),
            [layout::MARKER_NAME, "previous.txt"]
        );
    }
}

#[test]
fn a_locked_closure_is_copied_into_the_sidecar_with_the_closure_define() {
    let env = Env::new("embed_lock_closure", "import tinypkg\n");
    env.lock();
    env.embed().expect("embedded");
    let files = relative_files(&env.sidecar());
    let closure: Vec<&str> = files
        .iter()
        .map(String::as_str)
        .filter(|rel| rel.starts_with("closure/"))
        .collect();
    assert!(
        closure.contains(&"closure/tinypkg/__init__.py"),
        "{files:?}"
    );
    assert!(
        closure.contains(&"closure/tinydep/__init__.py"),
        "{files:?}"
    );
    assert!(
        closure.contains(&"closure/tinypkg-1.0.dist-info/METADATA"),
        "{files:?}"
    );
    assert_eq!(
        std::fs::read(env.sidecar().join("closure/tinypkg/__init__.py")).unwrap(),
        b"from tinydep import X\n"
    );
    assert!(
        env.config().contains("#define PYCC_EMBED_CLOSURE"),
        "{}",
        env.config()
    );
}

#[test]
fn a_stdlib_only_program_gets_no_closure_directory_and_no_define() {
    // With a lock: its section is consumed (the library digest is checked)
    // but the closure is empty.
    let env = Env::new("embed_lock_stdlib", "import json\n");
    env.lock();
    env.embed().expect("embedded");
    assert!(!env.sidecar().join("closure").exists());
    assert!(
        !env.config().contains("PYCC_EMBED_CLOSURE"),
        "{}",
        env.config()
    );
    // Without one.
    std::fs::remove_file(env.entry.with_file_name("pycc.lock")).unwrap();
    env.embed().expect("embedded");
    assert!(!env.sidecar().join("closure").exists());
}

#[test]
fn a_missing_lock_is_refused_before_the_interpreter_is_probed() {
    let env = Env::new("embed_lock_missing", "import tinypkg\n");
    env.previous_sidecar();
    let toolchain = EmbedToolchain::with_interpreter("/nonexistent/pycc-test-python");
    let err = env.embed_with(&toolchain).unwrap_err();
    assert!(err.contains("which does not exist"), "{err}");
    assert!(err.contains("run `pycc lock"), "{err}");
    env.assert_previous_sidecar_intact();
}

#[test]
fn a_payload_file_edited_after_the_lock_is_refused_and_the_old_sidecar_kept() {
    let env = Env::new("embed_lock_edited", "import tinypkg\n");
    env.lock();
    env.previous_sidecar();
    std::fs::write(env.pure.join("tinydep/__init__.py"), "X = 2\n").unwrap();
    let err = env.embed().unwrap_err();
    assert!(err.contains("does not match its RECORD hash"), "{err}");
    assert!(err.contains("run `pycc lock"), "{err}");
    env.assert_previous_sidecar_intact();
}

#[test]
fn a_different_interpreter_library_is_refused_and_the_old_sidecar_kept() {
    for (tag, body) in [
        ("embed_lock_lib", "import tinypkg\n"),
        ("embed_lock_lib_stdlib", "import json\n"),
    ] {
        let env = Env::new(tag, body);
        env.lock();
        env.previous_sidecar();
        std::fs::write(env.layout.library(), "another library").unwrap();
        let err = env.embed().unwrap_err();
        assert!(err.contains("libpython-sha256"), "{err}");
        assert!(err.contains("run `pycc lock"), "{err}");
        env.assert_previous_sidecar_intact();
    }
}

#[test]
fn an_interpreter_field_mismatch_is_refused_after_the_probe() {
    let env = Env::new("embed_lock_interp", "import tinypkg\n");
    env.lock();
    let mut probe = env.layout.probe.clone();
    probe.version = (3, 14, 9);
    let toolchain = EmbedToolchain::with_probes("pyfake", probe, env.lock_probe.clone());
    let err = env.embed_with(&toolchain).unwrap_err();
    assert!(err.contains("python"), "{err}");
    assert!(err.contains("3.14.9"), "{err}");
}

#[cfg(target_os = "macos")]
#[path = "macos_closure_tests.rs"]
mod macos_closure_tests;
