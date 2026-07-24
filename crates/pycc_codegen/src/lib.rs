use inkwell::AddressSpace;
use inkwell::OptimizationLevel;
use inkwell::context::Context;
use inkwell::module::Linkage;
use inkwell::targets::{
    CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine,
};
use inkwell::types::{FunctionType, IntType, PointerType};
use inkwell::values::{FunctionValue, GlobalValue};
use pycc_mir::{MirInstr, MirItem, MirModule};
use std::collections::{BTreeSet, HashMap};
use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

/// Return the exact target triple used by native object emission.
///
/// The copied `String` intentionally outlives the LLVM-owned message. The
/// wrapper itself is not dropped because LLVM 22.1.1's prebuilt Windows
/// libraries have been observed to crash while disposing this message.
pub fn native_target_triple() -> String {
    let triple = std::mem::ManuallyDrop::new(TargetMachine::get_default_triple());
    triple.as_str().to_string_lossy().into_owned()
}

#[derive(Debug, PartialEq)]
struct LinkCommand {
    program: OsString,
    args: Vec<OsString>,
}

/// Link one generated native object and the v0.1 C runtime with a compiler
/// driver that is explicitly pinned to the same LLVM target.
///
/// `PYCC_CLANG_<NORMALIZED_TARGET>` overrides `PYCC_CLANG`, which in turn
/// defaults to `clang`. Overrides must name a Clang-compatible driver.
pub fn link_object_with_runtime(
    object_path: &Path,
    runtime_source_path: &Path,
    output_path: &Path,
) -> Result<(), String> {
    let target = native_target_triple();
    let program = select_clang(&target, environment_var);
    run_link_command(&link_command(
        &target,
        object_path,
        runtime_source_path,
        output_path,
        program,
    ))
}

fn normalized_target(target: &str) -> String {
    target
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

fn environment_var(name: &str) -> Option<OsString> {
    std::env::var_os(name)
}

fn select_clang(target: &str, environment: fn(&str) -> Option<OsString>) -> OsString {
    let target_key = format!("PYCC_CLANG_{}", normalized_target(target));
    if let Some(program) = environment(&target_key) {
        return program;
    }
    if let Some(program) = environment("PYCC_CLANG") {
        return program;
    }
    OsString::from("clang")
}

fn link_command(
    target: &str,
    object_path: &Path,
    runtime_source_path: &Path,
    output_path: &Path,
    program: OsString,
) -> LinkCommand {
    let mut args = vec![OsString::from(format!("--target={target}"))];
    if target.ends_with("-windows-msvc") {
        args.push(OsString::from("-fuse-ld=lld"));
    }
    args.extend([
        object_path.as_os_str().to_owned(),
        runtime_source_path.as_os_str().to_owned(),
        OsString::from("-o"),
        output_path.as_os_str().to_owned(),
    ]);
    LinkCommand { program, args }
}

fn run_link_command(command: &LinkCommand) -> Result<(), String> {
    let status = Command::new(&command.program)
        .args(&command.args)
        .status()
        .map_err(|error| {
            format!(
                "could not start target-aware compiler driver `{}`: {error}",
                command.program.to_string_lossy()
            )
        })?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "target-aware compiler driver `{}` exited with {status}",
            command.program.to_string_lossy()
        ))
    }
}

pub fn compile_to_object(mir: &MirModule, output_path: &Path) -> Result<(), String> {
    let context = Context::create();
    let module = context.create_module("pycc_module");
    let builder = context.create_builder();

    let i64_type = context.i64_type();
    let void_type = context.void_type();

    let print_fn_type = void_type.fn_type(&[i64_type.into()], false);
    let print_fn = module.add_function("pycc_rt_print_i64", print_fn_type, Some(Linkage::External));
    let pointer_type = context.ptr_type(AddressSpace::default());
    let name_error_fn_type = void_type.fn_type(&[pointer_type.into()], false);
    let name_error_fn = module.add_function(
        "pycc_rt_name_error",
        name_error_fn_type,
        Some(Linkage::External),
    );

    // Every Python name has a runtime binding initialized to null. Executing
    // a `def` stores that particular definition's function pointer into the
    // binding. Calls load the binding when they execute, preserving both
    // call-before-def NameError behavior and later redefinitions.
    let mut binding_names = BTreeSet::new();
    for item in &mir.items {
        match item {
            MirItem::Function { name, body } => {
                binding_names.insert(name.as_str());
                for instr in body {
                    record_called_name(&mut binding_names, instr);
                }
            }
            MirItem::TopLevelStmt(instr) => record_called_name(&mut binding_names, instr),
        }
    }
    let mut bindings = HashMap::new();
    for name in binding_names {
        let binding = module.add_global(pointer_type, None, &format!("pybind_{name}"));
        binding.set_initializer(&pointer_type.const_null());
        bindings.insert(name.to_string(), binding);
    }

    // LLVM declarations still come first so bodies can be emitted in any
    // order. Unlike the old name-keyed map, this vector retains every
    // repeated definition as a distinct function.
    let no_arg_void_fn_type = void_type.fn_type(&[], false);
    let mut function_versions = vec![None; mir.items.len()];
    for (index, item) in mir.items.iter().enumerate() {
        if let MirItem::Function { name, .. } = item {
            let mangled = format!("pyfn_{name}_def_{index}");
            function_versions[index] =
                Some(module.add_function(&mangled, no_arg_void_fn_type, None));
        }
    }

    let entry_fn_type = i64_type.fn_type(&[], false);
    let entry_fn = module.add_function("main", entry_fn_type, None);
    let entry_block = context.append_basic_block(entry_fn, "entry");
    let emitter = Emitter {
        context: &context,
        builder: &builder,
        print_fn,
        name_error_fn,
        bindings: &bindings,
        no_arg_void_fn_type,
        pointer_type,
        i64_type,
    };
    builder.position_at_end(entry_block);
    for (index, item) in mir.items.iter().enumerate() {
        match item {
            MirItem::Function { name, .. } => {
                let function = function_versions[index]
                    .expect("every function item received an LLVM declaration");
                builder
                    .build_store(
                        bindings[name.as_str()].as_pointer_value(),
                        function.as_global_value().as_pointer_value(),
                    )
                    .expect("storing a function binding should not fail");
            }
            MirItem::TopLevelStmt(instr) => emitter.emit_instr(entry_fn, instr),
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
        .expect(
            "build_return should not fail: builder is always freshly positioned before this call",
        );

    // Second pass: fill in each user function's body, now that every
    // function (including ones a body might call) is already declared.
    for (index, item) in mir.items.iter().enumerate() {
        if let MirItem::Function { body, .. } = item {
            let f =
                function_versions[index].expect("every function item received an LLVM declaration");
            let block = context.append_basic_block(f, "entry");
            builder.position_at_end(block);
            for instr in body {
                emitter.emit_instr(f, instr);
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

    Target::initialize_native(&InitializationConfig::default()).expect(
        "initializing codegen for the native host should never fail on a supported Tier-1 target",
    );
    let triple = std::mem::ManuallyDrop::new(TargetMachine::get_default_triple());
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

fn record_called_name<'a>(names: &mut BTreeSet<&'a str>, instr: &'a MirInstr) {
    if let MirInstr::CallUserFunction { name } = instr {
        names.insert(name.as_str());
    }
}

struct Emitter<'ctx, 'a> {
    context: &'ctx Context,
    builder: &'a inkwell::builder::Builder<'ctx>,
    print_fn: FunctionValue<'ctx>,
    name_error_fn: FunctionValue<'ctx>,
    bindings: &'a HashMap<String, GlobalValue<'ctx>>,
    no_arg_void_fn_type: FunctionType<'ctx>,
    pointer_type: PointerType<'ctx>,
    i64_type: IntType<'ctx>,
}

impl Emitter<'_, '_> {
    fn emit_instr(&self, current_fn: FunctionValue<'_>, instr: &MirInstr) {
        match instr {
            MirInstr::CallPrint { arg } => {
                let arg_value = self.i64_type.const_int(*arg as u64, true);
                self.builder
                    .build_call(self.print_fn, &[arg_value.into()], "call_print")
                    .expect("build_call should not fail for a well-formed print call");
            }
            MirInstr::CallUserFunction { name } => {
                let function_pointer = self
                    .builder
                    .build_load(
                        self.pointer_type,
                        self.bindings[name.as_str()].as_pointer_value(),
                        "load_python_binding",
                    )
                    .expect("loading a function binding should not fail")
                    .into_pointer_value();
                let is_missing = self
                    .builder
                    .build_is_null(function_pointer, "binding_is_missing")
                    .expect("checking a function binding should not fail");
                let missing_block = self.context.append_basic_block(current_fn, "name_error");
                let call_block = self
                    .context
                    .append_basic_block(current_fn, "call_bound_function");
                let continuation = self
                    .context
                    .append_basic_block(current_fn, "after_bound_call");
                self.builder
                    .build_conditional_branch(is_missing, missing_block, call_block)
                    .expect("branching on a function binding should not fail");

                self.builder.position_at_end(missing_block);
                let name_value = self
                    .builder
                    .build_global_string_ptr(name, "missing_python_name")
                    .expect("building a static function-name string should not fail");
                self.builder
                    .build_call(
                        self.name_error_fn,
                        &[name_value.as_pointer_value().into()],
                        "raise_name_error",
                    )
                    .expect("calling the runtime NameError helper should not fail");
                self.builder
                    .build_unreachable()
                    .expect("terminating the NameError block should not fail");

                self.builder.position_at_end(call_block);
                self.builder
                    .build_indirect_call(
                        self.no_arg_void_fn_type,
                        function_pointer,
                        &[],
                        "call_user_function",
                    )
                    .expect("calling a bound zero-argument function should not fail");
                self.builder
                    .build_unconditional_branch(continuation)
                    .expect("continuing after a bound function call should not fail");
                self.builder.position_at_end(continuation);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pycc_mir::{MirInstr, MirItem, MirModule};
    use std::process::Command;

    fn target_clang(name: &str) -> Option<OsString> {
        (name == "PYCC_CLANG_X86_64_PC_WINDOWS_MSVC").then(|| OsString::from("target-clang"))
    }

    fn global_clang(name: &str) -> Option<OsString> {
        (name == "PYCC_CLANG").then(|| OsString::from("global-clang"))
    }

    fn no_clang(_name: &str) -> Option<OsString> {
        None
    }

    #[test]
    fn target_specific_clang_override_wins() {
        let selected = select_clang("x86_64-pc-windows-msvc", target_clang);
        assert_eq!(selected, OsString::from("target-clang"));
    }

    #[test]
    fn global_clang_override_and_default_are_supported() {
        let selected = select_clang("aarch64-apple-darwin", global_clang);
        assert_eq!(selected, OsString::from("global-clang"));
        assert_eq!(
            select_clang("aarch64-apple-darwin", no_clang),
            OsString::from("clang")
        );
    }

    #[test]
    fn msvc_link_command_uses_the_exact_target_and_lld() {
        let command = link_command(
            "x86_64-pc-windows-msvc",
            Path::new("program.o"),
            Path::new("runtime.c"),
            Path::new("program.exe"),
            OsString::from("clang"),
        );
        assert_eq!(
            command.args,
            [
                "--target=x86_64-pc-windows-msvc",
                "-fuse-ld=lld",
                "program.o",
                "runtime.c",
                "-o",
                "program.exe",
            ]
            .map(OsString::from)
        );
    }

    #[test]
    fn non_msvc_link_command_does_not_force_the_windows_linker() {
        let command = link_command(
            "aarch64-apple-darwin",
            Path::new("program.o"),
            Path::new("runtime.c"),
            Path::new("program"),
            OsString::from("clang"),
        );
        assert!(!command.args.contains(&OsString::from("-fuse-ld=lld")));
    }

    #[test]
    fn missing_compiler_driver_is_a_clean_error() {
        let command = LinkCommand {
            program: OsString::from("__pycc_missing_compiler_driver__"),
            args: vec![],
        };
        let error = run_link_command(&command).expect_err("missing driver must fail");
        assert!(error.contains("could not start target-aware compiler driver"));
    }

    #[test]
    fn target_aware_linker_accepts_an_object_from_the_same_clang_target() {
        let dir = tempfile_dir("target_aware_link");
        let main_source = dir.join("main.c");
        let object_path = dir.join("main.o");
        let runtime_source = dir.join("runtime.c");
        let binary_path = dir.join(format!("target_aware_link{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(
            &main_source,
            "extern void pycc_rt_print_i64(long long);\n\
             int main(void) { pycc_rt_print_i64(42); return 0; }\n",
        )
        .unwrap();
        pycc_rt::write_c_runtime(&runtime_source).unwrap();

        let target = native_target_triple();
        let program = select_clang(&target, environment_var);
        let compile_status = Command::new(&program)
            .arg(format!("--target={target}"))
            .args(["-c"])
            .arg(&main_source)
            .arg("-o")
            .arg(&object_path)
            .status()
            .expect("configured Clang driver should start");
        assert!(compile_status.success(), "C fixture compilation failed");

        super::link_object_with_runtime(&object_path, &runtime_source, &binary_path)
            .expect("target-aware linking should succeed");
        let output = Command::new(binary_path)
            .output()
            .expect("linked fixture should run");
        assert!(output.status.success());
        let stdout = String::from_utf8(output.stdout).expect("runtime output must be UTF-8");
        assert_eq!(stdout.replace("\r\n", "\n"), "42\n");
    }

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
                MirItem::TopLevelStmt(MirInstr::CallUserFunction {
                    name: "main".to_string(),
                }),
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
                MirItem::TopLevelStmt(MirInstr::CallUserFunction {
                    name: "main".to_string(),
                }),
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
    fn calling_a_function_before_its_definition_raises_name_error_at_runtime() {
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirInstr::CallUserFunction {
                    name: "foo".to_string(),
                }),
                MirItem::Function {
                    name: "foo".to_string(),
                    body: vec![MirInstr::CallPrint { arg: 42 }],
                },
            ],
        };
        let dir = tempfile_dir("slice0_call_before_def");
        let obj_path = dir.join("slice0_call_before_def.o");
        compile_to_object(&mir, &obj_path).expect("codegen should succeed");
        let bin_path = dir.join("slice0_call_before_def");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.status.code(), Some(101));
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("name 'foo' is not defined"));
    }

    #[test]
    fn an_undefined_name_in_an_uncalled_function_body_has_no_effect() {
        let mir = MirModule {
            items: vec![MirItem::Function {
                name: "main".to_string(),
                body: vec![MirInstr::CallUserFunction {
                    name: "also_does_not_exist".to_string(),
                }],
            }],
        };
        let dir = tempfile_dir("slice0_undefined_fn_nested");
        let obj_path = dir.join("slice0_undefined_fn_nested.o");
        compile_to_object(&mir, &obj_path).expect("codegen should succeed");
        let bin_path = dir.join("slice0_undefined_fn_nested");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
    }

    #[test]
    fn repeated_definitions_rebind_the_name_in_execution_order() {
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "foo".to_string(),
                    body: vec![MirInstr::CallPrint { arg: 1 }],
                },
                MirItem::TopLevelStmt(MirInstr::CallUserFunction {
                    name: "foo".to_string(),
                }),
                MirItem::Function {
                    name: "foo".to_string(),
                    body: vec![MirInstr::CallPrint { arg: 2 }],
                },
                MirItem::TopLevelStmt(MirInstr::CallUserFunction {
                    name: "foo".to_string(),
                }),
            ],
        };
        let dir = tempfile_dir("slice0_redefinition");
        let obj_path = dir.join("slice0_redefinition.o");
        compile_to_object(&mir, &obj_path).expect("codegen should succeed");
        let bin_path = dir.join("slice0_redefinition");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert!(output.status.success());
        assert_eq!(output.stdout, b"1\n2\n");
    }

    #[test]
    fn calls_from_function_bodies_read_the_binding_at_call_time() {
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "call_foo".to_string(),
                    body: vec![MirInstr::CallUserFunction {
                        name: "foo".to_string(),
                    }],
                },
                MirItem::Function {
                    name: "foo".to_string(),
                    body: vec![MirInstr::CallPrint { arg: 1 }],
                },
                MirItem::TopLevelStmt(MirInstr::CallUserFunction {
                    name: "call_foo".to_string(),
                }),
                MirItem::Function {
                    name: "foo".to_string(),
                    body: vec![MirInstr::CallPrint { arg: 2 }],
                },
                MirItem::TopLevelStmt(MirInstr::CallUserFunction {
                    name: "call_foo".to_string(),
                }),
            ],
        };
        let dir = tempfile_dir("slice0_body_binding");
        let obj_path = dir.join("slice0_body_binding.o");
        compile_to_object(&mir, &obj_path).expect("codegen should succeed");
        let bin_path = dir.join("slice0_body_binding");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert!(output.status.success());
        assert_eq!(output.stdout, b"1\n2\n");
    }

    #[test]
    fn recursive_function_body_resolves_its_runtime_binding() {
        // The slice has no conditional or return instruction with which to
        // build a terminating recursive fixture. Compiling, verifying, and
        // linking a self-call still proves that a recursive body resolves
        // through the runtime binding created for its own definition rather
        // than depending on source-order predeclaration in the old name map.
        let mir = MirModule {
            items: vec![MirItem::Function {
                name: "recurse".to_string(),
                body: vec![MirInstr::CallUserFunction {
                    name: "recurse".to_string(),
                }],
            }],
        };
        let dir = tempfile_dir("slice0_recursive_binding");
        let obj_path = dir.join("slice0_recursive_binding.o");
        compile_to_object(&mir, &obj_path).expect("recursive codegen should succeed");
        let bin_path = dir.join("slice0_recursive_binding");
        link_object_with_runtime(&obj_path, &bin_path);
    }

    #[test]
    fn a_function_can_be_defined_under_any_name_without_being_called() {
        // There is no longer a "must be named main" restriction: any
        // function name is legal to *define*; only calling one runs it.
        let mir = MirModule {
            items: vec![MirItem::Function {
                name: "helper".to_string(),
                body: vec![],
            }],
        };
        let dir = tempfile_dir("slice0_any_fn_name");
        let obj_path = dir.join("slice0_any_fn_name.o");
        compile_to_object(&mir, &obj_path)
            .expect("defining a function under any name should succeed");
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
            .join(format!(
                "pycc_codegen_test_nonexistent_dir_{}",
                std::process::id()
            ))
            .join("does_not_exist")
            .join("out.o");
        let err =
            compile_to_object(&mir, &bad_path).expect_err("should fail: parent dir doesn't exist");
        assert!(!err.is_empty());
    }

    fn tempfile_dir(label: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("pycc_codegen_test_{label}_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn link_object_with_runtime(obj_path: &std::path::Path, bin_path: &std::path::Path) {
        let rt_source_path = bin_path.with_extension("pycc_rt.c");
        pycc_rt::write_c_runtime(&rt_source_path).expect("embedded runtime should be writable");
        super::link_object_with_runtime(obj_path, &rt_source_path, bin_path)
            .expect("target-aware linking should succeed");
    }
}
