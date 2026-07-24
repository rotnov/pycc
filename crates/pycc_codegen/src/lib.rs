use inkwell::OptimizationLevel;
use inkwell::context::Context;
use inkwell::module::Linkage;
use inkwell::targets::{CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine};
use inkwell::types::IntType;
use inkwell::values::FunctionValue;
use pycc_mir::{MirInstr, MirItem, MirModule};
use std::collections::HashMap;
use std::path::Path;

pub fn compile_to_object(mir: &MirModule, output_path: &Path) -> Result<(), String> {
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
    // See the module-level comment block below for why these five
    // .expect()s (this one included) are deliberate rather than
    // Result-threaded: each covers an operation that is infallible given
    // how this function always calls it. write_to_file, at the very end,
    // is the one genuine, externally-triggerable failure mode and stays a
    // real Result the caller must handle.
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
        compile_to_object(&mir, &obj_path).expect("codegen should succeed");
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
        compile_to_object(&mir, &obj_path).expect("codegen should succeed");
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
        let err = compile_to_object(&mir, &obj_path).expect_err("should be rejected");
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
        let err = compile_to_object(&mir, &obj_path).expect_err("should be rejected");
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
        compile_to_object(&mir, &obj_path).expect("defining a function under any name should succeed");
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
