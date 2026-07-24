use inkwell::OptimizationLevel;
use inkwell::context::Context;
use inkwell::module::Linkage;
use inkwell::targets::{CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine};
use pycc_mir::{MirInstr, MirItem, MirModule};
use std::path::Path;

pub fn compile_to_object(mir: &MirModule, output_path: &Path) -> Result<(), String> {
    let context = Context::create();
    let module = context.create_module("pycc_module");
    let builder = context.create_builder();

    let i64_type = context.i64_type();
    let void_type = context.void_type();

    let print_fn_type = void_type.fn_type(&[i64_type.into()], false);
    let print_fn = module.add_function("pycc_rt_print_i64", print_fn_type, Some(Linkage::External));

    let main_fn_type = i64_type.fn_type(&[], false);
    let main_fn = module.add_function("main", main_fn_type, None);
    let entry_block = context.append_basic_block(main_fn, "entry");
    builder.position_at_end(entry_block);

    let mut user_main_body: Option<&[MirInstr]> = None;
    for item in &mir.items {
        match item {
            MirItem::TopLevelStmt(instr) => emit_instr(&builder, print_fn, i64_type, instr),
            MirItem::Function { name, body } if name == "main" => user_main_body = Some(body),
            MirItem::Function { name, .. } => {
                return Err(format!(
                    "pycc_codegen v0.1: only a function named `main` is supported so far, got `{name}`"
                ));
            }
        }
    }
    if let Some(body) = user_main_body {
        for instr in body {
            emit_instr(&builder, print_fn, i64_type, instr);
        }
    }
    // The five .expect()s below are deliberate, not sloppy error handling:
    // each covers an operation that is infallible given how this function
    // always calls it (a freshly positioned builder, IR this function
    // always generates validly by construction, the native host's own
    // default target). None of them can be triggered by any input this
    // function accepts, so returning a Result callers would have to
    // handle -- for an error condition no caller-supplied MIR could ever
    // cause -- would be misleading, not more defensive. Compare
    // write_to_file below: a real filesystem-dependent failure mode,
    // which stays a genuine Result the caller must handle.
    builder
        .build_return(Some(&i64_type.const_int(0, false)))
        .expect("build_return should not fail: builder is always freshly positioned before this call");

    module.verify().expect(
        "generated IR should always be well-formed for this fixed instruction shape; \
         a verify() failure here means a bug in pycc_codegen itself, not bad user input",
    );

    Target::initialize_native(&InitializationConfig::default())
        .expect("initializing codegen for the native host should never fail on a supported Tier-1 target");
    let triple = TargetMachine::get_default_triple();
    let target = Target::from_triple(&triple)
        .expect("the host's own default target triple should always be a valid target");
    let target_machine = target
        .create_target_machine(
            &triple,
            "generic",
            "",
            OptimizationLevel::None,
            RelocMode::Default,
            CodeModel::Default,
        )
        .expect("creating a target machine for the native host with generic CPU/features should never fail");
    target_machine
        .write_to_file(&module, FileType::Object, output_path)
        .map_err(|e| e.to_string())
}

fn emit_instr<'ctx>(
    builder: &inkwell::builder::Builder<'ctx>,
    print_fn: inkwell::values::FunctionValue<'ctx>,
    i64_type: inkwell::types::IntType<'ctx>,
    instr: &MirInstr,
) {
    let MirInstr::CallPrint { arg } = instr;
    let arg_value = i64_type.const_int(*arg as u64, true);
    builder
        .build_call(print_fn, &[arg_value.into()], "call_print")
        .expect("build_call should not fail for a well-formed print call");
}

#[cfg(test)]
mod tests {
    use super::*;
    use pycc_mir::{MirInstr, MirItem, MirModule};
    use std::process::Command;

    #[test]
    fn compiles_main_calling_print_to_a_running_binary() {
        let mir = MirModule {
            items: vec![MirItem::Function {
                name: "main".to_string(),
                body: vec![MirInstr::CallPrint { arg: 42 }],
            }],
        };
        let dir = tempfile_dir("slice0");
        let obj_path = dir.join("slice0.o");
        compile_to_object(&mir, &obj_path).expect("codegen should succeed");
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
        compile_to_object(&mir, &obj_path).expect("codegen should succeed");
        let bin_path = dir.join("slice0_toplevel");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"42\n");
    }

    #[test]
    fn top_level_statements_run_before_a_combined_user_main() {
        // Not one of PR-2's two named fixtures, but RUNTIME.md's ordering
        // guarantee ("top-level code ... runs once ... at process start")
        // has to hold even when a module happens to define both -- so it
        // gets a test now rather than being an unverified assumption.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirInstr::CallPrint { arg: 1 }),
                MirItem::Function {
                    name: "main".to_string(),
                    body: vec![MirInstr::CallPrint { arg: 2 }],
                },
            ],
        };
        let dir = tempfile_dir("slice0_combined");
        let obj_path = dir.join("slice0_combined.o");
        compile_to_object(&mir, &obj_path).expect("codegen should succeed");
        let bin_path = dir.join("slice0_combined");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"1\n2\n");
    }

    #[test]
    fn a_function_named_anything_other_than_main_is_rejected() {
        let mir = MirModule {
            items: vec![MirItem::Function { name: "helper".to_string(), body: vec![] }],
        };
        let dir = tempfile_dir("slice0_bad_fn_name");
        let obj_path = dir.join("slice0_bad_fn_name.o");
        let err = compile_to_object(&mir, &obj_path).expect_err("should be rejected");
        assert!(err.contains("helper"), "error should name the offending function: {err}");
    }

    #[test]
    fn write_to_file_failure_is_reported_as_an_error() {
        // A real, reachable failure mode (unlike the five internal
        // invariants asserted via .expect() in compile_to_object): the
        // output path's parent directory doesn't exist.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirInstr::CallPrint { arg: 42 })],
        };
        let bad_path = std::env::temp_dir()
            .join(format!("pycc_codegen_test_nonexistent_dir_{}", std::process::id()))
            .join("does_not_exist")
            .join("out.o");
        let err = compile_to_object(&mir, &bad_path).expect_err("should fail: parent dir doesn't exist");
        assert!(!err.is_empty());
    }

    fn tempfile_dir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("pycc_codegen_test_{label}_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Test-only linking helper. `pycc`'s real CLI (Task 8) does this via
    /// `cc`; duplicated minimally here so pycc_codegen's own tests can prove
    /// the object file it produces actually links and runs, without
    /// depending on the `pycc` binary crate (that would be a dependency
    /// cycle: pycc depends on pycc_codegen, not the other way around).
    fn link_object_with_runtime(obj_path: &std::path::Path, bin_path: &std::path::Path) {
        let rt_lib_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug");
        let status = Command::new("cc")
            .arg(obj_path)
            .arg("-L").arg(&rt_lib_dir)
            .arg("-lpycc_rt")
            .arg("-o").arg(bin_path)
            .status()
            .expect("cc should run");
        assert!(status.success(), "linking failed");
    }
}
