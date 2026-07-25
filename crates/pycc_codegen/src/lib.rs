use inkwell::OptimizationLevel;
use inkwell::context::Context;
use inkwell::module::Linkage;
use inkwell::targets::{
    CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine, TargetTriple,
};
use inkwell::types::IntType;
use inkwell::values::FunctionValue;
use pycc_mir::{MirInstr, MirItem, MirModule};
use std::collections::HashMap;
use std::path::Path;

/// `target_triple`: `None` compiles for the host's own default target (the
/// common case). `Some(triple)` cross-compiles for a different Tier-1
/// target -- LLVM's codegen backend is inherently multi-target, so this
/// only requires `Target::initialize_all` (rather than
/// `Target::initialize_native`) plus the requested `TargetTriple` instead
/// of the host's default; producing an actual *linked binary* for a
/// foreign target is a separate concern the caller handles (see
/// `src/main.rs`'s `--target` handling and its doc comment on what's
/// actually achievable without bundling a full foreign sysroot).
pub fn compile_to_object(
    mir: &MirModule,
    output_path: &Path,
    target_triple: Option<&str>,
) -> Result<(), String> {
    let context = Context::create();
    let module = context.create_module("pycc_module");
    let builder = context.create_builder();

    let i64_type = context.i64_type();
    let void_type = context.void_type();

    let print_fn_type = void_type.fn_type(&[i64_type.into()], false);
    let print_fn = module.add_function("pycc_rt_print_i64", print_fn_type, Some(Linkage::External));

    // First pass: declare every user-defined function under a mangled name
    // (never the bare Python name) before emitting any body. Two reasons:
    // this is what lets a function call another function defined later in
    // the same module, or itself (recursion -- structurally supported by
    // this pass ordering, though nothing in v0.1's HIR/MIR can express a
    // recursive call *with* arguments or a return value yet); and mangling
    // is what stops a Python-level function actually named `main` from
    // colliding with the real C-ABI entry point below, which must be
    // literally named `main` for the OS loader to find it. A def alone has
    // no runtime effect in Python regardless of its name -- something has
    // to call it, which is exactly the bug this pass structure fixes (see
    // git history: an earlier version treated a function merely named
    // `main` as auto-invoked, which doesn't match CPython at all).
    let no_arg_void_fn_type = void_type.fn_type(&[], false);
    let mut user_functions: HashMap<&str, FunctionValue> = HashMap::new();
    for item in &mir.items {
        if let MirItem::Function { name, .. } = item {
            let mangled = format!("pyfn_{name}");
            let f = module.add_function(&mangled, no_arg_void_fn_type, None);
            user_functions.insert(name.as_str(), f);
        }
    }

    let entry_fn_type = i64_type.fn_type(&[], false);
    let entry_fn = module.add_function("main", entry_fn_type, None);
    let entry_block = context.append_basic_block(entry_fn, "entry");
    builder.position_at_end(entry_block);
    for item in &mir.items {
        if let MirItem::TopLevelStmt(instr) = item {
            emit_instr(&builder, print_fn, &user_functions, i64_type, instr)?;
        }
    }
    // See the module-level comment block below for why these .expect()s
    // (this one included) are deliberate rather than Result-threaded: each
    // covers an operation that stays infallible given how this function
    // always calls it. Two calls below are genuine, externally-triggerable
    // failure modes and stay real Results the caller must handle:
    // Target::from_triple (a user-supplied --target can legitimately name
    // a triple LLVM doesn't recognize) and write_to_file, at the very end.
    builder
        .build_return(Some(&i64_type.const_int(0, false)))
        .expect("build_return should not fail: builder is always freshly positioned before this call");

    // Second pass: fill in each user function's body, now that every
    // function (including ones a body might call) is already declared.
    for item in &mir.items {
        if let MirItem::Function { name, body } = item {
            let f = user_functions[name.as_str()];
            let block = context.append_basic_block(f, "entry");
            builder.position_at_end(block);
            for instr in body {
                emit_instr(&builder, print_fn, &user_functions, i64_type, instr)?;
            }
            builder
                .build_return(None)
                .expect("build_return should not fail: builder is always freshly positioned before this call");
        }
    }

    verify_module(&module);

    // initialize_all (not initialize_native): a requested target_triple may
    // not match the host's own architecture, and LLVM only has codegen
    // support for a target's backend if that backend was initialized.
    Target::initialize_all(&InitializationConfig::default());
    // ManuallyDrop, not a plain value: see D-029. TargetTriple wraps an
    // LLVMString (inkwell's own message wrapper around LLVMCreateMessage /
    // LLVMGetDefaultTargetTriple), whose Drop calls LLVMDisposeMessage --
    // this crashes on Windows against the official prebuilt LLVM 22.1.1
    // release. Suppressing the drop here, at the point of creation, covers
    // every exit path uniformly (the early `?` below included), not just
    // the success path a trailing forget would. Leaks one small string per
    // compile on every platform -- negligible in a short-lived CLI process,
    // and simpler than cfg-gating a type difference for a Windows-only leak.
    let triple = std::mem::ManuallyDrop::new(match target_triple {
        Some(t) => TargetTriple::create(t),
        None => TargetMachine::get_default_triple(),
    });
    let target = Target::from_triple(&triple).map_err(|e| {
        format!(
            "pycc_codegen: `{}` is not a target LLVM knows how to generate code for: {}",
            triple.as_str().to_string_lossy(),
            llvm_string_to_owned(e)
        )
    })?;
    let target_machine = target
        .create_target_machine(
            &triple,
            "generic",
            "",
            OptimizationLevel::None,
            RelocMode::Default,
            CodeModel::Default,
        )
        .expect(
            "creating a target machine with generic CPU/features should never fail for a \
             triple Target::from_triple has already accepted",
        );
    target_machine.write_to_file(&module, FileType::Object, output_path).map_err(llvm_string_to_owned)
}

/// See D-029: every message inkwell hands back as an `LLVMString` --
/// `Target::from_triple`'s and `TargetMachine::write_to_file`'s error
/// paths, on top of `TargetTriple` itself -- shares the same broken `Drop`
/// (`LLVMDisposeMessage`) on Windows against this LLVM release. Converts
/// to an owned `String` (a real copy, safe to keep and format past this
/// call) and forgets the original rather than letting it drop. General
/// fix at the one place `LLVMString` crosses into this crate's error
/// values, not a patch per call site -- so a future one isn't missed.
fn llvm_string_to_owned(message: inkwell::support::LLVMString) -> String {
    let owned = message.to_string();
    std::mem::forget(message);
    owned
}

/// Skipped on Windows: `module.verify()` crashes there with an access
/// violation when linked against the official prebuilt LLVM 22.1.1 release
/// -- isolated with stderr checkpoints bracketing every call from the end of
/// IR building through object emission (D-029): every other call completed,
/// consistently, across every test that reached this point; only this one
/// never returned. Root cause not further isolated -- no Windows debugger
/// available in this environment to get an exact crash address/frame -- so
/// this is a targeted skip of a *pure internal sanity check* (a failure
/// here would mean a pycc_codegen bug, never a rejection of legitimate user
/// code -- see the non-Windows body's own message), not a change to what
/// gets compiled. The identical IR-building code already runs verified on
/// macOS/Linux for every test in this suite, which narrows the residual,
/// Windows-only risk to "a bug that only produces malformed IR on a
/// Windows-specific code path" -- and no such path exists yet, since IR
/// building above has no platform-conditional logic at all.
#[cfg(windows)]
fn verify_module(_module: &inkwell::module::Module<'_>) {}

#[cfg(not(windows))]
fn verify_module(module: &inkwell::module::Module<'_>) {
    module.verify().expect(
        "generated IR should always be well-formed for this fixed instruction shape; \
         a verify() failure here means a bug in pycc_codegen itself, not bad user input",
    );
}

fn emit_instr<'ctx>(
    builder: &inkwell::builder::Builder<'ctx>,
    print_fn: FunctionValue<'ctx>,
    user_functions: &HashMap<&str, FunctionValue<'ctx>>,
    i64_type: IntType<'ctx>,
    instr: &MirInstr,
) -> Result<(), String> {
    match instr {
        MirInstr::CallPrint { arg } => {
            let arg_value = i64_type.const_int(*arg as u64, true);
            builder
                .build_call(print_fn, &[arg_value.into()], "call_print")
                .expect("build_call should not fail for a well-formed print call");
            Ok(())
        }
        MirInstr::CallUserFunction { name } => {
            let f = user_functions
                .get(name.as_str())
                .ok_or_else(|| format!("pycc_codegen v0.1: call to undefined function `{name}`"))?;
            builder
                .build_call(*f, &[], "call_user_fn")
                .expect("build_call should not fail for a well-formed zero-arg call");
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pycc_mir::{MirInstr, MirItem, MirModule};
    use std::process::Command;

    #[test]
    fn defining_main_without_calling_it_produces_no_output() {
        // The regression test for the bug this file's git history fixed:
        // a function definition alone must never run, regardless of its
        // name -- matches CPython exactly (confirmed empirically against
        // python3.14 on this exact source: zero bytes of stdout).
        let mir = MirModule {
            items: vec![MirItem::Function {
                name: "main".to_string(),
                body: vec![MirInstr::CallPrint { arg: 42 }],
            }],
        };
        let dir = tempfile_dir("slice0_uncalled_main");
        let obj_path = dir.join("slice0_uncalled_main.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("slice0_uncalled_main");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"");
    }

    #[test]
    fn compiles_an_explicit_call_to_main_to_a_running_binary() {
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "main".to_string(),
                    body: vec![MirInstr::CallPrint { arg: 42 }],
                },
                MirItem::TopLevelStmt(MirInstr::CallUserFunction { name: "main".to_string() }),
            ],
        };
        let dir = tempfile_dir("slice0");
        let obj_path = dir.join("slice0.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("slice0");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"42\n");
    }

    #[test]
    fn compiles_top_level_statement_with_no_main() {
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirInstr::CallPrint { arg: 42 })],
        };
        let dir = tempfile_dir("slice0_toplevel");
        let obj_path = dir.join("slice0_toplevel.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("slice0_toplevel");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"42\n");
    }

    #[test]
    fn top_level_statements_run_in_order_including_a_call_to_main() {
        // RUNTIME.md's ordering guarantee ("top-level code ... runs once
        // ... at process start") applies to top-level statements
        // themselves running in source order -- which now includes an
        // explicit call to a user function as just another top-level
        // statement, not a special auto-invoked case.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirInstr::CallPrint { arg: 1 }),
                MirItem::Function {
                    name: "main".to_string(),
                    body: vec![MirInstr::CallPrint { arg: 2 }],
                },
                MirItem::TopLevelStmt(MirInstr::CallUserFunction { name: "main".to_string() }),
            ],
        };
        let dir = tempfile_dir("slice0_combined");
        let obj_path = dir.join("slice0_combined.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("slice0_combined");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"1\n2\n");
    }

    #[test]
    fn calling_an_undefined_function_at_top_level_is_rejected() {
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirInstr::CallUserFunction {
                name: "does_not_exist".to_string(),
            })],
        };
        let dir = tempfile_dir("slice0_undefined_fn");
        let obj_path = dir.join("slice0_undefined_fn.o");
        let err = compile_to_object(&mir, &obj_path, None).expect_err("should be rejected");
        assert!(err.contains("does_not_exist"), "error should name the offending function: {err}");
    }

    #[test]
    fn calling_an_undefined_function_inside_a_function_body_is_rejected() {
        let mir = MirModule {
            items: vec![MirItem::Function {
                name: "main".to_string(),
                body: vec![MirInstr::CallUserFunction { name: "also_does_not_exist".to_string() }],
            }],
        };
        let dir = tempfile_dir("slice0_undefined_fn_nested");
        let obj_path = dir.join("slice0_undefined_fn_nested.o");
        let err = compile_to_object(&mir, &obj_path, None).expect_err("should be rejected");
        assert!(err.contains("also_does_not_exist"), "error should name the offending function: {err}");
    }

    #[test]
    fn a_function_can_be_defined_under_any_name_without_being_called() {
        // There is no longer a "must be named main" restriction: any
        // function name is legal to *define*; only calling one runs it.
        let mir = MirModule {
            items: vec![MirItem::Function { name: "helper".to_string(), body: vec![] }],
        };
        let dir = tempfile_dir("slice0_any_fn_name");
        let obj_path = dir.join("slice0_any_fn_name.o");
        compile_to_object(&mir, &obj_path, None).expect("defining a function under any name should succeed");
    }

    #[test]
    fn write_to_file_failure_is_reported_as_an_error() {
        // A real, reachable failure mode (unlike the internal invariants
        // asserted via .expect() in compile_to_object): the output path's
        // parent directory doesn't exist. an_unknown_target_triple_is_a_
        // clean_error below covers this function's other genuine failure
        // mode, Target::from_triple.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirInstr::CallPrint { arg: 42 })],
        };
        let bad_path = std::env::temp_dir()
            .join(format!("pycc_codegen_test_nonexistent_dir_{}", std::process::id()))
            .join("does_not_exist")
            .join("out.o");
        let err = compile_to_object(&mir, &bad_path, None).expect_err("should fail: parent dir doesn't exist");
        assert!(!err.is_empty());
    }

    #[test]
    fn cross_compiles_object_code_for_a_different_target_triple() {
        // This host is aarch64-apple-darwin; request the other macOS Tier-1
        // architecture. LLVM's codegen backend is inherently multi-target,
        // so this only needs Target::initialize_all (see compile_to_object)
        // plus the requested triple -- verified by checking the emitted
        // object file's actual architecture, not just that codegen didn't
        // error.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirInstr::CallPrint { arg: 42 })],
        };
        let dir = tempfile_dir("cross_x64");
        let obj_path = dir.join("cross_x64.o");
        compile_to_object(&mir, &obj_path, Some("x86_64-apple-darwin"))
            .expect("cross-compiling to a different Tier-1 target should succeed");

        assert!(
            object_file_cpu_type_is_x86_64(&obj_path),
            "expected a Mach-O object file with cputype x86_64"
        );
    }

    /// Reads the Mach-O header directly instead of shelling out to the
    /// `file` utility, which this test used to do: fragile on Windows,
    /// where `file` isn't a standard tool and only worked because Git's
    /// bundled `usr/bin/file.exe` happened to be on `PATH` there -- an
    /// environment coincidence, not a guarantee. This test only ever
    /// emits Mach-O (`--target x86_64-apple-darwin`), so a full
    /// multi-format parser isn't needed -- just enough of
    /// `mach_header_64`'s fixed layout (magic, then cputype, both
    /// little-endian on every Tier-1 target this project builds for) to
    /// assert the emitted object's architecture is genuinely x86_64, not
    /// a copy-paste no-op.
    fn object_file_cpu_type_is_x86_64(path: &std::path::Path) -> bool {
        const MH_MAGIC_64: [u8; 4] = 0xfeed_facf_u32.to_le_bytes();
        const CPU_TYPE_X86_64: [u8; 4] = 0x0100_0007_u32.to_le_bytes();
        let bytes = std::fs::read(path).expect("object file should be readable");
        bytes.len() >= 8 && bytes[0..4] == MH_MAGIC_64 && bytes[4..8] == CPU_TYPE_X86_64
    }

    #[test]
    fn an_unknown_target_triple_is_a_clean_error() {
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirInstr::CallPrint { arg: 42 })],
        };
        let dir = tempfile_dir("bad_triple");
        let obj_path = dir.join("bad_triple.o");
        let err = compile_to_object(&mir, &obj_path, Some("not-a-real-target-triple"))
            .expect_err("an unrecognized target triple should be rejected");
        assert!(!err.is_empty());
    }

    fn tempfile_dir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("pycc_codegen_test_{label}_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Test-only linking helper. `pycc`'s real CLI (Task 8) does this via
    /// `cc`/clang (see `src/main.rs`'s `linker_command`/`effective_link_target`/
    /// `add_windows_system_libs`); duplicated minimally here so
    /// pycc_codegen's own tests can prove the object file it produces
    /// actually links and runs, without depending on the `pycc` binary
    /// crate (that would be a dependency cycle: pycc depends on
    /// pycc_codegen, not the other way around). Needs the same Windows
    /// handling as `main.rs`, and for the same reasons: there's no default
    /// `cc` there (D-028) -- on this runner it silently resolved to
    /// MinGW's `gcc`, which cannot link the MSVC-ABI `pycc_rt.lib` (the
    /// exact "undefined reference to `__imp_...`"/`collect2` wall D-028
    /// already diagnosed for `main.rs`, reproduced here because this
    /// helper wasn't covered by that fix); clang's bare-invocation default
    /// target also proved unreliable (D-028), so `-target` must be
    /// explicit too.
    fn link_object_with_runtime(obj_path: &std::path::Path, bin_path: &std::path::Path) {
        let rt_lib_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug");

        #[cfg(windows)]
        let mut cmd = {
            let clang = std::path::Path::new(env!("LLVM_SYS_221_PREFIX")).join("bin").join("clang.exe");
            let mut cmd = Command::new(clang);
            cmd.arg("-target").arg("x86_64-pc-windows-msvc");
            cmd
        };
        #[cfg(not(windows))]
        let mut cmd = Command::new("cc");

        cmd.arg(obj_path).arg("-L").arg(&rt_lib_dir).arg("-lpycc_rt").arg("-o").arg(bin_path);

        #[cfg(windows)]
        for lib in [
            "ws2_32",
            "ntdll",
            "userenv",
            "advapi32",
            "shell32",
            "ole32",
            "uuid",
            "psapi",
            "dbghelp",
            "kernel32",
            "legacy_stdio_definitions",
        ] {
            cmd.arg(format!("-l{lib}"));
        }

        let status = cmd.status().expect("the linker driver should run");
        assert!(status.success(), "linking failed");
    }
}
