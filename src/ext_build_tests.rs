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
    // The probe insists on the header itself, not just the directory.
    std::fs::write(dir.join("Python.h"), "").expect("the fixture header");
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

/// The directory exists and holds files, but not the one that matters. This
/// is what an interpreter installed without its development package reports,
/// and catching it here is what keeps it an environment failure (exit 2)
/// instead of a C compiler error the shared tail reports at exit 1.
#[test]
fn a_header_directory_without_python_h_is_an_environment_failure_not_a_compile_error() {
    let dir = pycc_scratch::ScratchDir::new("ext-probe-headerless").expect("scratch");
    std::fs::write(dir.join("pyconfig.h"), "").expect("a decoy header");
    let toolchain = ExtToolchain::with_probe("python3", probe((3, 13), &dir));
    let message = toolchain
        .probe()
        .expect_err("a directory without Python.h is an error");
    assert!(message.contains("Python.h"), "{message}");
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
    assert_eq!(
        args,
        vec![OsString::from("-shared"), OsString::from("-Wl,-Bsymbolic")]
    );
}

#[test]
fn only_the_elf_arm_asks_the_linker_to_bind_its_own_symbols_first() {
    // `-Bsymbolic` closes ELF's global-scope interposition, which is an ELF
    // problem alone: Mach-O records the defining library per reference and a
    // PE exports only what it declares. `ld64` and `link.exe` both reject
    // the flag, so leaking it onto either arm would break those two hosts
    // outright -- assert its absence rather than trusting the arms not to
    // drift. Reachable from any host because the platform is a parameter.
    for platform in [ExtLinkPlatform::MacOs, ExtLinkPlatform::Windows] {
        let args = ext_link_args(platform, Path::new("/unused"));
        assert!(
            !args
                .iter()
                .any(|arg| arg.to_string_lossy().contains("-Wl,")),
            "{args:?}"
        );
    }
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

/// The C shim maps a pending exception's type tag to a `PyExc_*` object
/// with a `switch` over literal tag values, because it cannot see
/// `pycc_hir::BUILTIN_EXCEPTION_CLASSES` from C. `pycc_rt`'s own
/// `exception_type_tags_match_the_c_shims_hardcoded_switch` pins the flat
/// seven against that crate's constants; this pins the *whole* array against
/// the switch itself, so adding, removing, or reordering a builtin class --
/// which renumbers every tag after it -- fails here instead of silently
/// raising the wrong CPython class out of a built artifact.
#[test]
fn every_exception_tag_the_c_shim_switches_on_still_names_that_class() {
    let mut pending: Option<usize> = None;
    let mut seen = Vec::new();
    for line in SHIM_C.lines().map(str::trim) {
        // A later `switch` in the same file dispatches on named constants;
        // only a decimal label belongs to the exception table.
        if let Some(tag) = line
            .strip_prefix("case ")
            .and_then(|r| r.strip_suffix(':'))
            .and_then(|r| r.parse().ok())
        {
            pending = Some(tag);
        } else if let Some(class) = line
            .strip_prefix("exc_type = PyExc_")
            .and_then(|r| r.strip_suffix(';'))
        {
            // The `default:` arm sets `exc_type` too, and carries no tag.
            if let Some(tag) = pending.take() {
                assert_eq!(
                    pycc_hir::BUILTIN_EXCEPTION_CLASSES.get(tag).copied(),
                    Some(class),
                    "src/ext/pycc_ext_module.c raises {class} for tag {tag}"
                );
                seen.push(tag);
            }
        }
    }
    // Tag 0 (`Exception`) reaches the same `PyExc_Exception` as the unnamed
    // tags through `default:`, and 23..=24 stay there deliberately -- the
    // shim's own comment carries why. Everything between is switched on.
    assert_eq!(seen, (1..=22).collect::<Vec<_>>());
}

// ---------------------------------------------------------------- exports

#[test]
fn every_public_carriable_module_level_function_is_exported_in_source_order() {
    let hir = module(vec![
        func("first", &[("x", Ty::Int)], Ty::Int),
        func("second", &[], Ty::Int),
        func("third", &[("a", Ty::Int), ("b", Ty::Int)], Ty::Int),
        // #1048 widened the boundary: each of these reaches the export set
        // rather than the gap list, and the signature travels with it.
        func("scaled", &[("factor", Ty::Float)], Ty::Float),
        func("negated", &[("flag", Ty::Bool)], Ty::Bool),
        func("sink", &[("x", Ty::Int)], Ty::None),
    ]);
    assert_eq!(
        collect_exports(&hir).expect("a carriable program exports cleanly"),
        vec![
            ExtExport {
                name: "first".to_string(),
                params: vec![Ty::Int],
                return_ty: Ty::Int,
            },
            ExtExport {
                name: "second".to_string(),
                params: Vec::new(),
                return_ty: Ty::Int,
            },
            ExtExport {
                name: "third".to_string(),
                params: vec![Ty::Int, Ty::Int],
                return_ty: Ty::Int,
            },
            ExtExport {
                name: "scaled".to_string(),
                params: vec![Ty::Float],
                return_ty: Ty::Float,
            },
            ExtExport {
                name: "negated".to_string(),
                params: vec![Ty::Bool],
                return_ty: Ty::Bool,
            },
            ExtExport {
                name: "sink".to_string(),
                params: vec![Ty::Int],
                return_ty: Ty::None,
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
            params: Vec::new(),
            return_ty: Ty::Int,
        }]
    );
}

#[test]
fn a_rebound_public_name_is_exported_once_with_the_last_definition_s_signature() {
    // Python rebinds rather than redeclares, and codegen follows it: one
    // `fnptr_two` global, bound to the second `def`. A second table entry
    // would emit `pycc_ext_wrap_two` twice and the C compiler would reject
    // the redefinition, so the export set collapses the rebind here.
    let hir = module(vec![
        func("one", &[], Ty::Int),
        func("two", &[("x", Ty::Int)], Ty::Int),
        func("three", &[], Ty::Int),
        func("two", &[("a", Ty::Int), ("b", Ty::Int)], Ty::Int),
    ]);
    assert_eq!(
        collect_exports(&hir).expect("a rebind is not a capability gap"),
        vec![
            ExtExport {
                name: "one".to_string(),
                params: Vec::new(),
                return_ty: Ty::Int,
            },
            // Definition order, last definition's signature: the entry keeps
            // the position the name first claimed.
            ExtExport {
                name: "two".to_string(),
                params: vec![Ty::Int, Ty::Int],
                return_ty: Ty::Int,
            },
            ExtExport {
                name: "three".to_string(),
                params: Vec::new(),
                return_ty: Ty::Int,
            },
        ]
    );
}

#[test]
fn a_program_with_no_public_function_exports_nothing_rather_than_failing() {
    let hir = module(vec![func("_only", &[], Ty::Int)]);
    assert_eq!(collect_exports(&hir).expect("not an error"), Vec::new());
}

#[test]
fn a_parameter_the_boundary_cannot_carry_is_a_capability_gap_naming_it() {
    let hir = module(vec![func("greet", &[("who", Ty::Str)], Ty::Int)]);
    let gaps = collect_exports(&hir).expect_err("str is not bridged by #1048");
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0].code, EXT_CAPABILITY_CODE);
    assert_eq!(gaps[0].span, None);
    assert!(
        gaps[0].message.contains("`who: str`"),
        "{}",
        gaps[0].message
    );
    // Both ways out stay in the message: D-038's `_`-prefix opt-out, and
    // dropping `--ext` altogether.
    assert!(gaps[0].message.contains("`_greet`"), "{}", gaps[0].message);
    assert!(
        gaps[0].message.contains("without --ext"),
        "{}",
        gaps[0].message
    );
}

#[test]
fn a_none_parameter_is_still_a_capability_gap_after_the_scalar_widening() {
    // `None` is admissible as a return type and not as a parameter, so the
    // two admissible sets are asked separately. This arm stays live until
    // #1047's call-argument ICE is fixed; widening it here would turn a
    // diagnostic into a codegen assertion failure.
    let hir = module(vec![func("sink", &[("x", Ty::None)], Ty::None)]);
    let gaps = collect_exports(&hir).expect_err("a None parameter is not carriable");
    assert_eq!(gaps.len(), 1);
    assert!(gaps[0].message.contains("`x: None`"), "{}", gaps[0].message);
}

#[test]
fn a_return_type_the_boundary_cannot_carry_is_a_capability_gap_naming_it() {
    let hir = module(vec![func("name_of", &[("x", Ty::Int)], Ty::Str)]);
    let gaps = collect_exports(&hir).expect_err("str is not bridged by #1048");
    assert_eq!(gaps.len(), 1);
    assert!(gaps[0].message.contains("`-> str`"), "{}", gaps[0].message);
}

#[test]
fn a_bool_signature_is_carried_rather_than_gapped_and_keeps_its_own_slot() {
    // A declared `bool` parameter is not the D-141 encoded word an `int`
    // slot carries: the compiled function's own ABI slot is a plain `i8`
    // holding 0/1 -- `i8` at the parameter position too, since parameters
    // and returns share one `ty_to_basic_type`. So it is carried by its own
    // helper and its own one-byte C type, never through the `int` path.
    let hir = module(vec![func("flag", &[("b", Ty::Bool)], Ty::Bool)]);
    let exports = collect_exports(&hir).expect("bool is carried by #1048");
    assert_eq!(
        exports,
        vec![ExtExport {
            name: "flag".to_string(),
            params: vec![Ty::Bool],
            return_ty: Ty::Bool,
        }]
    );
}

#[test]
fn every_gap_in_a_program_is_collected_before_the_build_gives_up() {
    let hir = module(vec![
        func("a", &[("x", Ty::Str)], Ty::Int),
        func("b", &[("x", Ty::None)], Ty::Int),
        // Carriable after #1048, so neither of these joins the gap list.
        func("c", &[("x", Ty::Float)], Ty::None),
        func("d", &[("xs", Ty::List(Box::new(Ty::Int)))], Ty::Int),
        func("e", &[("x", Ty::Bool)], Ty::Int),
    ]);
    let gaps = collect_exports(&hir).expect_err("three of the five are gaps");
    assert_eq!(gaps.len(), 3);
    assert!(gaps[0].message.contains("`x: str`"), "{}", gaps[0].message);
    assert!(gaps[1].message.contains("`x: None`"), "{}", gaps[1].message);
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
            params: Vec::new(),
            return_ty: Ty::Int,
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
            params: vec![Ty::Int],
            return_ty: Ty::Int,
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
            params: vec![Ty::Int, Ty::Int],
            return_ty: Ty::Int,
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
            params: vec![Ty::Int],
            return_ty: Ty::Int,
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
fn a_float_export_carries_a_double_through_every_slot_of_the_wrapper() {
    let inc = generate_exports_inc(
        "m",
        &[ExtExport {
            name: "scale".to_string(),
            params: vec![Ty::Float],
            return_ty: Ty::Float,
        }],
    );
    assert!(inc.contains("    double result;"), "{inc}");
    // The half-widened wrapper is the sharpest failure mode here: a
    // `long long result` left behind while the cast and the pack move to
    // `double` compiles clean, warns nothing, and returns 2 for 2.5.
    assert!(!inc.contains("long long result"), "{inc}");
    assert!(inc.contains("    double a0;"), "{inc}");
    assert!(
        inc.contains("pycc_ext_unpack_float(args[0], \"scale\", 0, &a0)"),
        "{inc}"
    );
    assert!(
        inc.contains("result = ((double (*)(double))fnptr_scale)(a0);"),
        "{inc}"
    );
    // The float packer cannot fail on a value, so it takes no function name.
    assert!(inc.contains("return pycc_ext_pack_float(result);"), "{inc}");
}

#[test]
fn a_bool_export_uses_a_one_byte_c_type_to_match_the_compiled_i8_slot() {
    let inc = generate_exports_inc(
        "m",
        &[ExtExport {
            name: "negate".to_string(),
            params: vec![Ty::Bool],
            return_ty: Ty::Bool,
        }],
    );
    assert!(inc.contains("    char result;"), "{inc}");
    assert!(inc.contains("    char a0;"), "{inc}");
    assert!(
        inc.contains("pycc_ext_unpack_bool(args[0], \"negate\", 0, &a0)"),
        "{inc}"
    );
    assert!(
        inc.contains("result = ((char (*)(char))fnptr_negate)(a0);"),
        "{inc}"
    );
    assert!(inc.contains("return pycc_ext_pack_bool(result);"), "{inc}");
    // `_Bool` is not `i1`-shaped here and `int` is four bytes: either would
    // disagree with the callee across an unchecked `void *` cast.
    assert!(!inc.contains("_Bool"), "{inc}");
    assert!(!inc.contains("(int (*)"), "{inc}");
}

#[test]
fn a_none_returning_export_casts_to_void_and_declares_no_result_at_all() {
    let inc = generate_exports_inc(
        "m",
        &[ExtExport {
            name: "sink".to_string(),
            params: vec![Ty::Int],
            return_ty: Ty::None,
        }],
    );
    // LLVM emits a `None` return as `void`, so there is nothing to hold and
    // nothing to assign -- both would be C type errors.
    assert!(!inc.contains("result"), "{inc}");
    assert!(
        inc.contains("((void (*)(long long))fnptr_sink)(a0);"),
        "{inc}"
    );
    assert!(inc.contains("    Py_RETURN_NONE;"), "{inc}");
    assert!(!inc.contains("pycc_ext_pack"), "{inc}");
}

#[test]
fn a_mixed_signature_gives_each_slot_its_own_c_type_and_unpack_helper() {
    let inc = generate_exports_inc(
        "m",
        &[ExtExport {
            name: "mix".to_string(),
            params: vec![Ty::Int, Ty::Float, Ty::Bool],
            return_ty: Ty::Float,
        }],
    );
    assert!(
        inc.contains("    long long a0;\n    double a1;\n    char a2;\n"),
        "{inc}"
    );
    assert!(
        inc.contains("pycc_ext_unpack_int(args[0], \"mix\", 0, &a0)"),
        "{inc}"
    );
    assert!(
        inc.contains("pycc_ext_unpack_float(args[1], \"mix\", 1, &a1)"),
        "{inc}"
    );
    assert!(
        inc.contains("pycc_ext_unpack_bool(args[2], \"mix\", 2, &a2)"),
        "{inc}"
    );
    assert!(
        inc.contains("result = ((double (*)(long long, double, char))fnptr_mix)(a0, a1, a2);"),
        "{inc}"
    );
}

#[test]
fn a_none_returning_wrapper_checks_the_exception_flag_before_returning_none() {
    // The arm with no result to inspect is exactly the one where skipping
    // the check would hand Python a fabricated `None` instead of the
    // exception the compiled function raised.
    let inc = generate_exports_inc(
        "m",
        &[ExtExport {
            name: "risky".to_string(),
            params: vec![Ty::Int],
            return_ty: Ty::None,
        }],
    );
    let check = inc
        .find("pycc_rt_ext_pending_type() >= 0")
        .expect("the wrapper checks the pending flag");
    let egress = inc
        .find("Py_RETURN_NONE;")
        .expect("the wrapper returns None");
    assert!(
        check < egress,
        "the pending check must precede the None egress:\n{inc}"
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
    // The helpers the generated wrappers call by name for each carried
    // scalar. A wrapper that names a helper the shim does not define fails
    // at C compile time, which no unit test here can reach.
    for helper in [
        "static int pycc_ext_unpack_int(PyObject *obj",
        "static int pycc_ext_unpack_float(PyObject *obj",
        "static int pycc_ext_unpack_bool(PyObject *obj",
        "static PyObject *pycc_ext_pack_int(",
        "static PyObject *pycc_ext_pack_float(double value)",
        "static PyObject *pycc_ext_pack_bool(char value)",
    ] {
        assert!(SHIM_C.contains(helper), "{helper}");
    }
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
