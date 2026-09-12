//! Unit tests for the `--ext` build seam.
//!
//! Every test here is non-`#[ignore]`d and needs no CPython installation:
//! `.github/workflows/ci.yml`'s coverage job runs `llvm-cov` without
//! `--include-ignored`, so an ignored test earns zero coverage while its
//! lines stay in `scripts/check_diff_coverage.py`'s denominator. The
//! CPython-driven behaviour of a *built* artifact is verified separately by
//! the ignored oracle tests under `tests/`.
//!
//! No test here asserts a rendered `Path`: `Display`/`to_string_lossy`
//! carries the building host's separator, so `dist/m` renders as `dist\m` on
//! Windows. Paths are compared as `PathBuf`s built with `Path::join`.

use super::*;
use pycc_hir::Ty;

fn func(name: &str, params: &[(&str, Ty)], return_ty: Ty) -> HirItem {
    HirItem::Function {
        name: name.to_string(),
        params: params
            .iter()
            .map(|(n, ty)| ((*n).to_string(), ty.clone()))
            .collect(),
        return_ty,
        body: Vec::new(),
    }
}

fn module(items: Vec<HirItem>) -> HirModule {
    HirModule {
        items,
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
        seeded_builtin_exception_classes: false,
    }
}

fn probe(version: (u32, u32), include: &Path) -> ExtProbe {
    ExtProbe {
        version,
        include: include.to_path_buf(),
        libs: include.join("libs"),
    }
}

// ---------------------------------------------------------------- version

#[test]
fn a_major_minor_line_parses_and_extra_components_are_ignored() {
    assert_eq!(parse_version("3.13"), Some((3, 13)));
    assert_eq!(parse_version(" 3.14.7 \r"), Some((3, 14)));
    assert_eq!(parse_version("4.0"), Some((4, 0)));
}

#[test]
fn a_version_line_without_two_numeric_components_is_rejected() {
    assert_eq!(parse_version("3"), None);
    assert_eq!(parse_version("x.13"), None);
    assert_eq!(parse_version("3.x"), None);
    assert_eq!(parse_version(""), None);
}

#[test]
fn the_floor_admits_the_stable_abi_version_and_everything_newer() {
    assert!(check_floor(MIN_PYTHON).is_ok());
    assert!(check_floor((3, 14)).is_ok());
    assert!(check_floor((4, 0)).is_ok());
}

#[test]
fn the_floor_rejects_an_interpreter_older_than_the_declared_limited_api() {
    let message = check_floor((3, 12)).expect_err("3.12 is below the 3.13 floor");
    assert!(message.contains("3.13"), "{message}");
    assert!(message.contains("3.12"), "{message}");
    assert!(message.contains("Py_LIMITED_API"), "{message}");
    assert!(check_floor((2, 7)).is_err());
}

// ------------------------------------------------------------------ probe

#[test]
fn a_three_line_probe_parses_into_its_three_fields() {
    let parsed = parse_probe_output("3.13\n/usr/include/python3.13\n/opt/py/libs\n")
        .expect("a well-formed probe parses");
    assert_eq!(parsed.version, (3, 13));
    assert_eq!(parsed.include, Path::new("/usr/include/python3.13"));
    assert_eq!(parsed.libs, Path::new("/opt/py/libs"));
}

#[test]
fn a_truncated_or_empty_probe_is_rejected_rather_than_half_believed() {
    assert_eq!(parse_probe_output(""), None);
    assert_eq!(parse_probe_output("3.13\n"), None);
    assert_eq!(parse_probe_output("3.13\n/usr/include\n"), None);
    assert_eq!(parse_probe_output("nope\n/usr/include\n/libs\n"), None);
    // A blank include line parses as three lines but names no directory.
    assert_eq!(parse_probe_output("3.13\n\n/libs\n"), None);
}

#[test]
fn an_override_answers_without_running_any_interpreter() {
    let dir = pycc_scratch::ScratchDir::new("ext-probe").expect("scratch");
    let toolchain =
        ExtToolchain::with_probe("definitely-not-an-interpreter-1036", probe((3, 14), &dir));
    let resolved = toolchain.probe().expect("an override skips the spawn");
    assert_eq!(resolved.version, (3, 14));
    assert_eq!(resolved.include, *dir);
    assert_eq!(
        toolchain.interpreter(),
        std::ffi::OsStr::new("definitely-not-an-interpreter-1036")
    );
}

#[test]
fn an_override_is_still_subject_to_the_version_floor() {
    let dir = pycc_scratch::ScratchDir::new("ext-probe-floor").expect("scratch");
    let toolchain = ExtToolchain::with_probe("python3", probe((3, 12), &dir));
    let message = toolchain.probe().expect_err("3.12 is below the floor");
    assert!(message.contains("3.12"), "{message}");
}

#[test]
fn a_header_directory_that_does_not_exist_is_reported_before_the_compiler_sees_it() {
    let dir = pycc_scratch::ScratchDir::new("ext-probe-missing").expect("scratch");
    let missing = dir.join("no-such-include-dir");
    let toolchain = ExtToolchain::with_probe("python3", probe((3, 13), &missing));
    let message = toolchain
        .probe()
        .expect_err("a missing include dir is an error");
    assert!(message.contains("PYCC_PYTHON_INCLUDE"), "{message}");
}

#[test]
fn an_interpreter_that_cannot_be_spawned_is_an_environment_failure() {
    let toolchain = ExtToolchain::with_interpreter("pycc-no-such-interpreter-1036");
    let message = toolchain
        .probe()
        .expect_err("a missing interpreter is an error");
    assert!(
        message.contains("pycc-no-such-interpreter-1036"),
        "{message}"
    );
    assert!(message.contains("PYCC_PYTHON"), "{message}");
}

/// `/bin/sh` exists and spawns, but rejects the probe script, so this is the
/// one arm that needs a real child process to reach. Unix-only because
/// `/bin/sh` is: the coverage job runs on macOS, so the line is covered.
#[cfg(unix)]
#[test]
fn an_interpreter_that_rejects_the_probe_script_is_reported_with_its_exit_status() {
    let toolchain = ExtToolchain::with_interpreter("/bin/sh");
    let message = toolchain
        .probe()
        .expect_err("sh cannot run the probe script");
    assert!(
        message.contains("failed to report its own configuration"),
        "{message}"
    );
}

/// `/bin/echo` spawns, exits 0, and prints something that is not a probe
/// result -- the unparseable-output arm, distinct from the non-zero one.
#[cfg(unix)]
#[test]
fn an_interpreter_that_prints_nonsense_is_reported_as_unparseable() {
    let toolchain = ExtToolchain::with_interpreter("/bin/echo");
    let message = toolchain.probe().expect_err("echo prints no probe result");
    assert!(
        message.contains("could not\n                 parse") || message.contains("not parse"),
        "{message}"
    );
}

#[test]
fn the_environment_constructor_defaults_to_python3_and_no_override() {
    // Reads the ambient environment deliberately: this is the production
    // constructor, and the assertion holds for either state of the two
    // variables -- an override is honoured when set, and the default
    // interpreter name is `python3` when `PYCC_PYTHON` is not.
    let toolchain = ExtToolchain::from_env();
    match std::env::var_os("PYCC_PYTHON") {
        Some(value) => assert_eq!(toolchain.interpreter(), value),
        None => assert_eq!(toolchain.interpreter(), std::ffi::OsStr::new("python3")),
    }
}

// --------------------------------------------------------------- platform

#[test]
fn each_triple_family_selects_its_own_link_platform() {
    assert_eq!(
        ExtLinkPlatform::from_target_triple("x86_64-pc-windows-msvc"),
        ExtLinkPlatform::Windows
    );
    assert_eq!(
        ExtLinkPlatform::from_target_triple("aarch64-apple-darwin"),
        ExtLinkPlatform::MacOs
    );
    assert_eq!(
        ExtLinkPlatform::from_target_triple("x86_64-unknown-linux-gnu"),
        ExtLinkPlatform::Linux
    );
    // An unrecognized triple falls to the ELF default rather than panicking.
    assert_eq!(
        ExtLinkPlatform::from_target_triple("riscv64-unknown-none"),
        ExtLinkPlatform::Linux
    );
}

#[test]
fn no_target_resolves_to_the_host_and_a_target_overrides_it() {
    assert_eq!(ExtLinkPlatform::resolve(None), ExtLinkPlatform::HOST);
    assert_eq!(
        ExtLinkPlatform::resolve(Some("x86_64-pc-windows-msvc")),
        ExtLinkPlatform::Windows
    );
    // Whatever this host is, it is one of the three.
    assert!(matches!(
        ExtLinkPlatform::HOST,
        ExtLinkPlatform::MacOs | ExtLinkPlatform::Linux | ExtLinkPlatform::Windows
    ));
}

#[test]
fn the_three_link_platforms_collapse_onto_the_two_suffix_families() {
    assert!(matches!(
        ExtLinkPlatform::Windows.suffix_platform(),
        ExtPlatform::Windows
    ));
    assert!(matches!(
        ExtLinkPlatform::MacOs.suffix_platform(),
        ExtPlatform::Unix
    ));
    assert!(matches!(
        ExtLinkPlatform::Linux.suffix_platform(),
        ExtPlatform::Unix
    ));
}

#[test]
fn macos_links_a_bundle_with_deferred_symbol_lookup_and_no_libpython() {
    let args = ext_link_args(ExtLinkPlatform::MacOs, Path::new("/unused"));
    assert_eq!(
        args,
        vec![
            OsString::from("-bundle"),
            OsString::from("-undefined"),
            OsString::from("dynamic_lookup"),
        ]
    );
    assert!(
        !args
            .iter()
            .any(|arg| arg.to_string_lossy().contains("python"))
    );
}

#[test]
fn linux_links_a_plain_shared_object_with_no_libpython() {
    let args = ext_link_args(ExtLinkPlatform::Linux, Path::new("/unused"));
    assert_eq!(args, vec![OsString::from("-shared")]);
}

#[test]
fn windows_links_against_the_stable_abi_import_library_in_the_probed_directory() {
    let libs = Path::new("C:").join("Python313").join("libs");
    let args = ext_link_args(ExtLinkPlatform::Windows, &libs);
    // Compared as an `OsString` built from `Path::join`, never a rendered
    // path: the separator is the host's, not the target's.
    assert_eq!(
        args,
        vec![
            OsString::from("-shared"),
            OsString::from("-L"),
            libs.as_os_str().to_os_string(),
            OsString::from("-lpython3"),
        ]
    );
}

#[test]
fn the_elf_compile_args_name_the_include_directory_the_shim_and_fpic() {
    let include = Path::new("inc").join("python3.13");
    let shim = Path::new("scratch").join(SHIM_C_NAME);
    let args = ext_compile_args(ExtLinkPlatform::Linux, &include, &shim);
    assert_eq!(
        args,
        vec![
            OsString::from("-I"),
            include.as_os_str().to_os_string(),
            OsString::from("-fPIC"),
            shim.as_os_str().to_os_string(),
        ]
    );
}

/// The Mach-O arm states `-fPIC` too. clang already defaults to it there,
/// so this pins the flag as a property of the artifact rather than one
/// inherited from whichever host happened to build it.
#[test]
fn the_mach_o_compile_args_state_fpic_rather_than_inherit_it() {
    let include = Path::new("inc").join("python3.13");
    let shim = Path::new("scratch").join(SHIM_C_NAME);
    let args = ext_compile_args(ExtLinkPlatform::MacOs, &include, &shim);
    assert_eq!(
        args,
        vec![
            OsString::from("-I"),
            include.as_os_str().to_os_string(),
            OsString::from("-fPIC"),
            shim.as_os_str().to_os_string(),
        ]
    );
}

/// PE/COFF is position independent by construction and its drivers warn
/// that `-fPIC` is ignored, so the Windows arm must not emit it.
#[test]
fn the_windows_compile_args_omit_fpic() {
    let include = Path::new("inc").join("python3.13");
    let shim = Path::new("scratch").join(SHIM_C_NAME);
    let args = ext_compile_args(ExtLinkPlatform::Windows, &include, &shim);
    assert_eq!(
        args,
        vec![
            OsString::from("-I"),
            include.as_os_str().to_os_string(),
            shim.as_os_str().to_os_string(),
        ]
    );
}

// ---------------------------------------------------------------- exports

#[test]
fn every_public_int_only_module_level_function_is_exported_in_source_order() {
    let hir = module(vec![
        func("first", &[("x", Ty::Int)], Ty::Int),
        func("second", &[], Ty::Int),
        func("third", &[("a", Ty::Int), ("b", Ty::Int)], Ty::Int),
    ]);
    assert_eq!(
        collect_exports(&hir).expect("an int-only program exports cleanly"),
        vec![
            ExtExport {
                name: "first".to_string(),
                arity: 1
            },
            ExtExport {
                name: "second".to_string(),
                arity: 0
            },
            ExtExport {
                name: "third".to_string(),
                arity: 2
            },
        ]
    );
}

#[test]
fn a_private_name_a_method_and_a_monomorphized_specialization_are_not_exports() {
    let hir = module(vec![
        // D-038: a leading underscore is private, and `--ext` uses the same
        // predicate as every other visibility decision in this compiler.
        func("_helper", &[("x", Ty::Str)], Ty::Str),
        // A method reaches `HirItem::Function` under a dotted name; it is
        // not a module-level function, so it is neither exported nor a gap.
        func("Point.norm", &[("self", Ty::Float)], Ty::Float),
        // A monomorphized specialization has no `fnptr_` global to call
        // through -- codegen dispatches it directly.
        func("0gen_identity_int", &[("x", Ty::Str)], Ty::Str),
        // A top-level statement is not a function at all.
        HirItem::TopLevelStmt(pycc_hir::HirStmt::ExprStmt(pycc_hir::HirExpr::IntLiteral(
            0,
        ))),
        func("kept", &[], Ty::Int),
    ]);
    assert_eq!(
        collect_exports(&hir).expect("no public gap remains"),
        vec![ExtExport {
            name: "kept".to_string(),
            arity: 0
        }]
    );
}

#[test]
fn a_program_with_no_public_function_exports_nothing_rather_than_failing() {
    let hir = module(vec![func("_only", &[], Ty::Int)]);
    assert_eq!(collect_exports(&hir).expect("not an error"), Vec::new());
}

#[test]
fn a_non_int_parameter_is_a_capability_gap_naming_the_parameter() {
    let hir = module(vec![func("scale", &[("factor", Ty::Float)], Ty::Int)]);
    let gaps = collect_exports(&hir).expect_err("float is not bridged in Part 1");
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0].code, EXT_CAPABILITY_CODE);
    assert_eq!(gaps[0].span, None);
    assert!(
        gaps[0].message.contains("`factor: float`"),
        "{}",
        gaps[0].message
    );
    assert!(gaps[0].message.contains("`_scale`"), "{}", gaps[0].message);
}

#[test]
fn a_non_int_return_type_is_a_capability_gap_naming_the_return_type() {
    let hir = module(vec![func("name_of", &[("x", Ty::Int)], Ty::Str)]);
    let gaps = collect_exports(&hir).expect_err("str is not bridged in Part 1");
    assert_eq!(gaps.len(), 1);
    assert!(gaps[0].message.contains("`-> str`"), "{}", gaps[0].message);
}

#[test]
fn a_bool_signature_is_a_gap_because_the_boundary_is_int_only_in_part_one() {
    // `bool` is a conforming *argument* type at the boundary (D-141 gives it
    // a dedicated encoding), but a declared `bool` parameter is a different
    // thing: the compiled function's own ABI slot is `i1`, not the encoded
    // word the shim produces. Part 1 declares `int` only.
    let hir = module(vec![func("flag", &[("b", Ty::Bool)], Ty::Int)]);
    let gaps = collect_exports(&hir).expect_err("bool is not an int slot");
    assert!(gaps[0].message.contains("`b: bool`"), "{}", gaps[0].message);
}

#[test]
fn every_gap_in_a_program_is_collected_before_the_build_gives_up() {
    let hir = module(vec![
        func("a", &[("x", Ty::Float)], Ty::Int),
        func("b", &[], Ty::None),
        func("c", &[("x", Ty::Int)], Ty::Int),
        func("d", &[("xs", Ty::List(Box::new(Ty::Int)))], Ty::Int),
    ]);
    let gaps = collect_exports(&hir).expect_err("three of the four are gaps");
    assert_eq!(gaps.len(), 3);
    assert!(gaps[1].message.contains("`-> None`"), "{}", gaps[1].message);
    assert!(
        gaps[2].message.contains("`xs: list`"),
        "{}",
        gaps[2].message
    );
}

#[test]
fn every_ty_the_gap_message_can_name_renders_a_python_spelling() {
    let cases = [
        (Ty::Float, "float"),
        (Ty::Bool, "bool"),
        (Ty::Str, "str"),
        (Ty::None, "None"),
        (Ty::List(Box::new(Ty::Int)), "list"),
        (Ty::Dict(Box::new((Ty::Str, Ty::Int))), "dict"),
        (Ty::Set(Box::new(Ty::Int)), "set"),
        (Ty::Tuple(Box::new(vec![Ty::Int])), "tuple"),
        (Ty::Param(Box::new("T".to_string())), "that type"),
    ];
    for (ty, spelling) in cases {
        assert_eq!(render_ty(&ty), spelling);
        assert_eq!(render_ty(&Ty::Int), "int");
    }
}

// --------------------------------------------------------- generated C

#[test]
fn the_generated_companion_defines_the_module_name_in_both_spellings() {
    let inc = generate_exports_inc("fastmath", &[]);
    assert!(
        inc.contains("#define PYCC_EXT_MODULE_NAME fastmath\n"),
        "{inc}"
    );
    assert!(
        inc.contains("#define PYCC_EXT_MODULE_NAME_STR \"fastmath\"\n"),
        "{inc}"
    );
    // An export-free module still emits a terminated method table, which is
    // what `PyModuleDef` requires.
    assert!(
        inc.contains("static PyMethodDef pycc_ext_methods[] = {"),
        "{inc}"
    );
    assert!(inc.contains("    {NULL, NULL, 0, NULL},\n};\n"), "{inc}");
}

#[test]
fn a_nullary_export_declares_a_void_parameter_list_and_checks_its_arity() {
    let inc = generate_exports_inc(
        "m",
        &[ExtExport {
            name: "answer".to_string(),
            arity: 0,
        }],
    );
    assert!(inc.contains("extern void *fnptr_answer;"), "{inc}");
    assert!(
        inc.contains("result = ((long long (*)(void))fnptr_answer)();"),
        "{inc}"
    );
    assert!(
        inc.contains("takes exactly 0 arguments (%zd given)"),
        "{inc}"
    );
    assert!(
        inc.contains("(PyCFunction)(void (*)(void))pycc_ext_wrap_answer, METH_FASTCALL"),
        "{inc}"
    );
}

#[test]
fn a_unary_export_uses_the_singular_arity_message_and_unpacks_one_argument() {
    let inc = generate_exports_inc(
        "m",
        &[ExtExport {
            name: "square".to_string(),
            arity: 1,
        }],
    );
    assert!(
        inc.contains("takes exactly 1 argument (%zd given)"),
        "{inc}"
    );
    assert!(
        inc.contains("pycc_ext_unpack_int(args[0], \"square\", 0, &a0)"),
        "{inc}"
    );
    assert!(
        inc.contains("result = ((long long (*)(long long))fnptr_square)(a0);"),
        "{inc}"
    );
    assert!(
        inc.contains("return pycc_ext_pack_int(\"square\", result);"),
        "{inc}"
    );
}

#[test]
fn a_binary_export_unpacks_each_argument_at_its_own_index() {
    let inc = generate_exports_inc(
        "m",
        &[ExtExport {
            name: "add".to_string(),
            arity: 2,
        }],
    );
    assert!(
        inc.contains("takes exactly 2 arguments (%zd given)"),
        "{inc}"
    );
    assert!(
        inc.contains("pycc_ext_unpack_int(args[0], \"add\", 0, &a0)"),
        "{inc}"
    );
    assert!(
        inc.contains("pycc_ext_unpack_int(args[1], \"add\", 1, &a1)"),
        "{inc}"
    );
    assert!(
        inc.contains("result = ((long long (*)(long long, long long))fnptr_add)(a0, a1);"),
        "{inc}"
    );
}

#[test]
fn every_wrapper_checks_the_runtime_exception_flag_before_packing_a_result() {
    // A compiled function that raised returns a neutral carrier and leaves
    // the thread-local flag set, so packing it first would hand Python a
    // fabricated value instead of the exception.
    let inc = generate_exports_inc(
        "m",
        &[ExtExport {
            name: "risky".to_string(),
            arity: 1,
        }],
    );
    let check = inc
        .find("pycc_rt_ext_pending_type() >= 0")
        .expect("the wrapper checks the pending flag");
    let pack = inc
        .find("pycc_ext_pack_int(\"risky\"")
        .expect("the wrapper packs a result");
    assert!(
        check < pack,
        "the pending check must precede the pack:\n{inc}"
    );
}

#[test]
fn the_embedded_shim_is_the_tracked_c_file_and_declares_the_limited_api_floor() {
    assert!(
        SHIM_C.contains("#define Py_LIMITED_API 0x030D0000"),
        "{SHIM_C}"
    );
    // The multi-phase init contract: `PyInit_` only returns the def.
    assert!(SHIM_C.contains("PyModuleDef_Init(&pycc_ext_moduledef)"));
    // The free-threaded guard, and the detector that actually works:
    // the version string, not `sys._is_gil_enabled()`.
    assert!(SHIM_C.contains("strstr(Py_GetVersion(), \"free-threading build\")"));
    // It includes the generated companion under the exact name this module
    // writes it as, so the two really do land side by side.
    assert!(SHIM_C.contains(&format!("#include \"{EXPORTS_INC_NAME}\"")));
    assert_eq!(SHIM_C_NAME, "pycc_ext_module.c");
    // The module body runs in the exec slot, never in `PyInit_`.
    assert!(SHIM_C.contains("{Py_mod_exec, (void *)pycc_ext_exec_module}"));
    assert!(SHIM_C.contains("Py_MOD_MULTIPLE_INTERPRETERS_NOT_SUPPORTED"));
    // The symbol codegen emits for the module body under `--ext`.
    assert!(SHIM_C.contains(pycc_codegen::EXT_MODULE_EXEC_SYMBOL));
}

#[test]
fn the_shims_bigint_egress_releases_the_reference_before_it_raises() {
    // D-180 rule 6 hands the wrapper a retained return value, so the arm
    // the D-244 amendment turns into an `OverflowError` has to drop it --
    // otherwise every refused bigint result leaks a `BigIntObj`.
    assert!(SHIM_C.contains("extern void pycc_rt_bigint_release(long long word);"));
    let body = &SHIM_C[SHIM_C
        .find("static PyObject *pycc_ext_pack_int(")
        .expect("the pack helper")..];
    let bigint_arm = body.find("case PYCC_EXT_INT_BIGINT:").expect("bigint arm");
    let release = body
        .find("pycc_rt_bigint_release(encoded);")
        .expect("the release");
    let raise = body.find("PyExc_OverflowError").expect("the OverflowError");
    assert!(
        bigint_arm < release && release < raise,
        "the reference must be dropped on the bigint arm, before the raise"
    );
    // The unclassifiable-word arm must not release: it is not a pointer
    // this code knows to be live.
    let fallback = &body[body.find("    default:").expect("the default arm")..];
    assert!(!fallback.contains("pycc_rt_bigint_release"), "{fallback}");
}

#[test]
fn the_shims_unpack_order_puts_bool_before_int_before_the_type_error() {
    // Searched inside the function body, past its own doc comment, so the
    // comment's prose ordering cannot stand in for the code's.
    let body = &SHIM_C[SHIM_C
        .find("static int pycc_ext_unpack_int(PyObject *obj")
        .expect("the unpack helper")..];
    let bool_check = body.find("PyBool_Check(obj)").expect("bool arm");
    let long_check = body.find("!PyLong_Check(obj)").expect("int arm");
    let convert = body
        .find("raw = PyLong_AsLongLongAndOverflow")
        .expect("conversion");
    assert!(
        bool_check < long_check && long_check < convert,
        "D-141's bool markers must be read before the int conversion flattens them"
    );
    // The inline-integer range, never `i64`: the gate is the runtime's own
    // encoder, called after CPython's overflow check (#1040).
    assert!(SHIM_C.contains("pycc_rt_ext_int_encode(raw, out) != 0"));
    assert!(SHIM_C.contains("2**62"));
}
