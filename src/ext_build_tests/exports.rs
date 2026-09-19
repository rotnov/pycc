//! Export-set and capability-gap tests for the `--ext` build seam: which
//! public module-level functions `collect_exports` admits, and the `C0003`
//! diagnostic it raises for every signature the boundary cannot carry.
//!
//! Split out of `ext_build_tests.rs` by #1049 under `AGENTS.md`'s
//! decomposability rule; the tests themselves are unchanged except where
//! #1049's own widening required re-pointing a fixture.

use super::*;

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
                class: None,
                method: None,
                receiver: false,
                params: vec![Ty::Int],
                return_ty: Ty::Int,
            },
            ExtExport {
                name: "second".to_string(),
                class: None,
                method: None,
                receiver: false,
                params: Vec::new(),
                return_ty: Ty::Int,
            },
            ExtExport {
                name: "third".to_string(),
                class: None,
                method: None,
                receiver: false,
                params: vec![Ty::Int, Ty::Int],
                return_ty: Ty::Int,
            },
            ExtExport {
                name: "scaled".to_string(),
                class: None,
                method: None,
                receiver: false,
                params: vec![Ty::Float],
                return_ty: Ty::Float,
            },
            ExtExport {
                name: "negated".to_string(),
                class: None,
                method: None,
                receiver: false,
                params: vec![Ty::Bool],
                return_ty: Ty::Bool,
            },
            ExtExport {
                name: "sink".to_string(),
                class: None,
                method: None,
                receiver: false,
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
        // A *bare* dotted name is a regular method, a property getter or an
        // abstract method -- this spelling cannot tell them apart -- and
        // #1143 admits none of the three. It is refused as representation,
        // so it is neither exported nor a gap. Only the `.static` and
        // `.classmethod` spellings reach the export set.
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
            class: None,
            method: None,
            receiver: false,
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
                class: None,
                method: None,
                receiver: false,
                params: Vec::new(),
                return_ty: Ty::Int,
            },
            // Definition order, last definition's signature: the entry keeps
            // the position the name first claimed.
            ExtExport {
                name: "two".to_string(),
                class: None,
                method: None,
                receiver: false,
                params: vec![Ty::Int, Ty::Int],
                return_ty: Ty::Int,
            },
            ExtExport {
                name: "three".to_string(),
                class: None,
                method: None,
                receiver: false,
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
    // `list[int]` rather than `str`: #1049 made `str` carriable, so the
    // original fixture would no longer produce a gap at all. `list` is the
    // nearest still-unadmitted parameter type now that #1050 admits
    // `tuple` -- and it stays unadmitted because it is a mutable object
    // with no by-value crossing, not merely an unimplemented one.
    let hir = module(vec![func(
        "greet",
        &[("who", Ty::List(Box::new(Ty::Int)))],
        Ty::Int,
    )]);
    let gaps = collect_exports(&hir).expect_err("list is not bridged");
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0].code, EXT_CAPABILITY_CODE);
    assert_eq!(gaps[0].span, None);
    let message = &gaps[0].message;
    assert!(message.contains("`who: list`"), "{message}");
    // Both ways out stay in the message: D-038's `_`-prefix opt-out, and
    // dropping `--ext` altogether.
    assert!(message.contains("`_greet`"), "{message}");
    assert!(message.contains("without --ext"), "{message}");
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
    // `list[int]` for the same reason as the parameter arm above: `str`
    // returns are carried after #1049.
    let hir = module(vec![func(
        "name_of",
        &[("x", Ty::Int)],
        Ty::List(Box::new(Ty::Int)),
    )]);
    let gaps = collect_exports(&hir).expect_err("list is not bridged");
    assert_eq!(gaps.len(), 1);
    assert!(gaps[0].message.contains("`-> list`"), "{}", gaps[0].message);
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
            class: None,
            method: None,
            receiver: false,
            params: vec![Ty::Bool],
            return_ty: Ty::Bool,
        }]
    );
}

#[test]
fn a_str_signature_is_carried_rather_than_gapped_in_either_position() {
    // Part 2 of #1037 (#1049): `str` is admissible at a parameter and as a
    // return type. It reaches the wrapper as an opaque `void *` in both
    // positions -- `ty_to_basic_type` gives `Ty::Str` a pointer -- so the
    // export set records it exactly like any other carriable type.
    let hir = module(vec![func("echo", &[("s", Ty::Str)], Ty::Str)]);
    let exports = collect_exports(&hir).expect("str is carried by #1049");
    assert_eq!(
        exports,
        vec![ExtExport {
            name: "echo".to_string(),
            class: None,
            method: None,
            receiver: false,
            params: vec![Ty::Str],
            return_ty: Ty::Str,
        }]
    );
}

#[test]
fn a_tuple_signature_is_carried_rather_than_gapped_in_either_position() {
    // Part 3 of #1037 (#1050): a `tuple` of carriable scalars is admissible
    // at a parameter and as a return type. The export entry records the
    // declared `Ty` unchanged -- the flattening into scalar slots is the
    // wrapper's and the thunk's business, and `collect_exports` stays a
    // statement about the Python signature.
    let hir = module(vec![func(
        "swap",
        &[("t", Ty::Tuple(Box::new(vec![Ty::Int, Ty::Float])))],
        Ty::Tuple(Box::new(vec![Ty::Float, Ty::Bool])),
    )]);
    let exports = collect_exports(&hir).expect("tuple is carried by #1050");
    assert_eq!(
        exports,
        vec![ExtExport {
            name: "swap".to_string(),
            class: None,
            method: None,
            receiver: false,
            params: vec![Ty::Tuple(Box::new(vec![Ty::Int, Ty::Float]))],
            return_ty: Ty::Tuple(Box::new(vec![Ty::Float, Ty::Bool])),
        }]
    );
}

#[test]
fn a_tuple_of_something_uncarriable_is_a_capability_gap_naming_the_tuple() {
    // The element type is what fails, but the message names the parameter's
    // own declared type: `render_ty` renders a `tuple` as the bare word, and
    // the reader's fix is to change the signature, not to reason about which
    // element the boundary refused. `tuple[list[int]]` is reachable today
    // only from hand-built HIR -- T0039 refuses it at the type checker --
    // so this is the defensive arm, kept because `boundary_carrier` recurses
    // and a recursion that cannot fail is a claim, not a fact.
    let hir = module(vec![func(
        "boxes",
        &[("t", Ty::Tuple(Box::new(vec![Ty::List(Box::new(Ty::Int))])))],
        Ty::Int,
    )]);
    let gaps = collect_exports(&hir).expect_err("a list element is not carriable");
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0].code, EXT_CAPABILITY_CODE);
    let message = &gaps[0].message;
    assert!(message.contains("`t: tuple`"), "{message}");
    // The remediation enumerates what the boundary *does* carry, and Part 3
    // of #1037 (#1050) put `tuple` into that list. A reader who reaches this
    // message through a `tuple` gap has to be told which tuples are carried,
    // not shown a scalar-only list that reads as "no tuple at all". #1129
    // put the buffer's second spelling into it for the same reason: a
    // reader who wrote `ndarray` and is shown a list naming only
    // `memoryview` reads it as "not that type at all". #1134 added the
    // third spelling `NDArray` on the same reasoning -- it is the one 18 of
    // the 19 array-parameter occurrences in the #1039 census use.
    assert!(
        message.contains(
            "a parameter must be `int`, `float`, `bool`, `str`, `memoryview` (or its \
             other spellings `ndarray` and `NDArray`) or a `tuple` of \
             `int`/`float`/`bool`, and a \
             return type must be one of those except the buffer, or `None`"
        ),
        "{message}"
    );
}

#[test]
fn a_str_element_is_a_capability_gap_even_though_str_itself_is_carried() {
    // The one element type that is a scalar at a top-level position and
    // still not carriable inside a tuple. #1049 gave `str` its own C slot
    // and `pycc_ext_unpack_str`, but the element shims are the `_at`
    // variants, which exist only for D-116's three element types. Were
    // this admitted, the wrapper would render a call to an undeclared
    // `pycc_ext_unpack_str_at` and the artifact would die in clang rather
    // than as a `C0003` gap -- so the narrowing lives in
    // `boundary_carrier`'s tuple arm and not in `into_scalar`, which
    // passes every scalar through. Reachable today only from hand-built
    // HIR: `T0039` refuses `tuple[int, str]` at the type checker.
    let hir = module(vec![func(
        "label",
        &[("t", Ty::Tuple(Box::new(vec![Ty::Str, Ty::Int])))],
        Ty::Int,
    )]);
    let gaps = collect_exports(&hir).expect_err("a str element is not carriable");
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0].code, EXT_CAPABILITY_CODE);
    let message = &gaps[0].message;
    assert!(message.contains("`t: tuple`"), "{message}");
}

#[test]
fn a_nested_tuple_return_is_a_capability_gap_rather_than_a_flattening() {
    // The one arm that would be silently wrong if it were admitted: a
    // nested tuple has no single scalar slot, and flattening it would make
    // `tuple[tuple[int, int], int]` and `tuple[int, int, int]` indis-
    // tinguishable at the seam -- the wrapper would then re-pack three
    // elements into a two-element Python tuple. D-116 gives a tuple type a
    // fixed arity of scalar elements, and the boundary carries exactly that.
    let hir = module(vec![func(
        "nest",
        &[("x", Ty::Int)],
        Ty::Tuple(Box::new(vec![Ty::Tuple(Box::new(vec![Ty::Int])), Ty::Int])),
    )]);
    let gaps = collect_exports(&hir).expect_err("a nested tuple is not carriable");
    assert_eq!(gaps.len(), 1);
    let message = &gaps[0].message;
    assert!(message.contains("`-> tuple`"), "{message}");
}

#[test]
fn a_private_name_carrying_a_tuple_is_neither_exported_nor_gapped() {
    // The D-038 name filter runs before the type admissibility question, so
    // an underscore-prefixed helper is invisible to the boundary whatever it
    // carries. This is the same set `pycc_codegen`'s `is_ext_exportable_name`
    // decides thunk emission from: a name excluded here gets no wrapper, so
    // a thunk for it would advertise a symbol nothing calls.
    let hir = module(vec![func(
        "_scratch",
        &[("t", Ty::Tuple(Box::new(vec![Ty::List(Box::new(Ty::Int))])))],
        Ty::Tuple(Box::new(vec![Ty::Int])),
    )]);
    assert_eq!(collect_exports(&hir).expect("not an error"), Vec::new());
}

#[test]
fn every_gap_in_a_program_is_collected_before_the_build_gives_up() {
    let hir = module(vec![
        // `dict`, not `list`: `d` below is already the `list` arm, and two
        // gaps rendering the same spelling would stop distinguishing which
        // index belongs to which function. The key type is `int` rather than
        // `str` so nothing here depends on a type #1049 made carriable.
        func(
            "a",
            &[("x", Ty::Dict(Box::new((Ty::Int, Ty::Int))))],
            Ty::Int,
        ),
        func("b", &[("x", Ty::None)], Ty::Int),
        // Carriable after #1048/#1049, so none of these joins the gap list.
        func("c", &[("x", Ty::Float)], Ty::None),
        func("d", &[("xs", Ty::List(Box::new(Ty::Int)))], Ty::Int),
        func("e", &[("x", Ty::Bool)], Ty::Int),
        func("f", &[("s", Ty::Str)], Ty::Str),
    ]);
    let gaps = collect_exports(&hir).expect_err("three of the six are gaps");
    assert_eq!(gaps.len(), 3);
    assert!(gaps[0].message.contains("`x: dict`"), "{}", gaps[0].message);
    assert!(gaps[1].message.contains("`x: None`"), "{}", gaps[1].message);
    let third = &gaps[2].message;
    assert!(third.contains("`xs: list`"), "{third}");
}

/// Part 1 of #1026 work item 12: D-244 rule 2 fixes the artifact's ABI to a
/// scalar set, and an opaque CPython object is not in it. The refusal is
/// `C0003` -- the same capability gap every other unadmitted type gets --
/// and it fires in `collect_exports`, before `boundary_carrier` is ever
/// consulted, which is what the `Ty::Object => false` arm in
/// `refusal_completeness.rs` pins from the other side.
///
/// Unreachable from Python source today: `object` is not spellable in an
/// annotation, and a parameter needs one. The HIR is built directly here
/// for exactly that reason -- the boundary's answer must be stated before
/// a later part of #1026 makes the shape reachable, not after.
#[test]
fn an_object_typed_parameter_is_refused_at_the_export_boundary() {
    let hir = module(vec![func("wrap", &[("x", Ty::Object)], Ty::Int)]);
    let gaps = collect_exports(&hir).expect_err("an opaque object is not carriable");
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0].code, EXT_CAPABILITY_CODE);
    let message = &gaps[0].message;
    assert!(message.contains("`x: object`"), "{message}");
}

/// The return half of the same refusal: `collect_exports` asks the two
/// positions against separate admissible sets, so a `-> object` that only
/// the parameter arm refused would still reach the wrapper.
#[test]
fn an_object_return_type_is_refused_at_the_export_boundary() {
    let hir = module(vec![func("fetch", &[("x", Ty::Int)], Ty::Object)]);
    let gaps = collect_exports(&hir).expect_err("an opaque object is not carriable");
    assert_eq!(gaps.len(), 1);
    let message = &gaps[0].message;
    assert!(message.contains("`-> object`"), "{message}");
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
        (Ty::Object, "object"),
        (Ty::MemoryView, "memoryview"),
        (Ty::Param(Box::new("T".to_string())), "that type"),
    ];
    for (ty, spelling) in cases {
        assert_eq!(render_ty(&ty), spelling);
        assert_eq!(render_ty(&Ty::Int), "int");
    }
}

// --- #1143: methods in the export set ----------------------------------

#[test]
fn a_public_static_and_class_method_of_a_public_class_are_exported() {
    // The mangled spellings `pycc_hir` actually emits: `.static` for a
    // `@staticmethod`, `.classmethod` for a `@classmethod`. The classmethod's
    // own parameter list leads with `cls`, which is the receiver and never
    // crosses the boundary, so `params` holds the receiver-free tail.
    let hir = module_with_classes(
        vec![
            func("Grid.scale.static", &[("n", Ty::Int)], Ty::Int),
            func(
                "Grid.make.classmethod",
                &[
                    ("cls", Ty::Instance(Box::new("Grid".to_string()))),
                    ("n", Ty::Int),
                ],
                Ty::Int,
            ),
        ],
        vec![("Grid".to_string(), class_def("Grid", None))],
    );
    assert_eq!(
        collect_exports(&hir).expect("both methods are carriable"),
        vec![
            ExtExport {
                name: "Grid.scale.static".to_string(),
                class: Some("Grid".to_string()),
                method: Some("scale".to_string()),
                receiver: false,
                params: vec![Ty::Int],
                return_ty: Ty::Int,
            },
            ExtExport {
                name: "Grid.make.classmethod".to_string(),
                class: Some("Grid".to_string()),
                method: Some("make".to_string()),
                receiver: true,
                params: vec![Ty::Int],
                return_ty: Ty::Int,
            },
        ]
    );
}

#[test]
fn a_method_of_a_user_exception_class_is_excluded_by_its_exception_type_tag() {
    // The exclusion selector is `HirClassDef::exception_type_tag.is_some()`,
    // deliberately and not `collect_user_exception_classes`' own selector:
    // the tag is set for exactly the classes the runtime treats as
    // exceptions, and the two selectors drifting apart must not be able to
    // widen the export set behind this rule. This test pins the tag.
    let hir = module_with_classes(
        vec![
            func("Failure.of.static", &[("n", Ty::Int)], Ty::Int),
            func("Grid.scale.static", &[("n", Ty::Int)], Ty::Int),
        ],
        vec![
            ("Failure".to_string(), class_def("Failure", Some(7))),
            ("Grid".to_string(), class_def("Grid", None)),
        ],
    );
    let exports = collect_exports(&hir).expect("no gap remains");
    assert_eq!(
        exports.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
        vec!["Grid.scale.static"]
    );
}

#[test]
fn a_private_class_or_a_private_method_leaves_the_export_set() {
    // D-038's `is_public_name` is applied to the class name and the method
    // name alike, never forked: either leading underscore removes the member.
    let hir = module_with_classes(
        vec![
            func("_Grid.scale.static", &[("n", Ty::Int)], Ty::Int),
            func("Grid._scale.static", &[("n", Ty::Int)], Ty::Int),
            func("Grid.scale.static", &[("n", Ty::Int)], Ty::Int),
        ],
        vec![
            ("_Grid".to_string(), class_def("_Grid", None)),
            ("Grid".to_string(), class_def("Grid", None)),
        ],
    );
    let exports = collect_exports(&hir).expect("no gap remains");
    assert_eq!(
        exports.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
        vec!["Grid.scale.static"]
    );
}

#[test]
fn a_bare_method_and_a_property_setter_are_refused_as_representation_not_as_a_gap() {
    // An instance method (`Grid.area`), a property getter (also spelled
    // `Grid.width`) and a property setter (`Grid.width.setter`) are excluded
    // *as representation*: they are never routed to the `C0003` gap
    // collector, so an unexportable signature on one of them is silence and
    // not a diagnostic. The signatures here are deliberately uncarriable --
    // a `list` parameter would be a `C0003` on any member that were in the
    // export set -- so the assertion distinguishes "excluded" from
    // "admitted and carriable".
    let hir = module_with_classes(
        vec![
            func(
                "Grid.area",
                &[("self", Ty::Instance(Box::new("Grid".to_string())))],
                Ty::List(Box::new(Ty::Int)),
            ),
            func(
                "Grid.width.setter",
                &[
                    ("self", Ty::Instance(Box::new("Grid".to_string()))),
                    ("value", Ty::List(Box::new(Ty::Int))),
                ],
                Ty::None,
            ),
        ],
        vec![("Grid".to_string(), class_def("Grid", None))],
    );
    assert_eq!(
        collect_exports(&hir).expect("neither member is a capability gap"),
        Vec::new()
    );
}

#[test]
fn an_unexportable_method_signature_is_a_capability_gap_naming_the_source_spelling() {
    // The subject renders `Grid.scale`, the source-level spelling -- never
    // `Grid.scale.static`, which is a compiler-internal mangling the user
    // never wrote. The remedy renames the *method*, so it renders
    // `Grid._scale` and not `_Grid.scale`.
    let hir = module_with_classes(
        vec![func(
            "Grid.scale.static",
            &[("who", Ty::List(Box::new(Ty::Int)))],
            Ty::Int,
        )],
        vec![("Grid".to_string(), class_def("Grid", None))],
    );
    let gaps = collect_exports(&hir).expect_err("list is not bridged");
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0].code, EXT_CAPABILITY_CODE);
    let message = &gaps[0].message;
    assert!(message.contains("method `Grid.scale`"), "{message}");
    assert!(!message.contains("Grid.scale.static"), "{message}");
    assert!(message.contains("`Grid._scale`"), "{message}");
    assert!(!message.contains("`_Grid.scale`"), "{message}");
}

#[test]
fn the_underscore_remedy_a_method_gap_prints_actually_removes_it_from_the_export_set() {
    // A remedy that does not work is worse than none: rename the method the
    // way the message above spells it and the member leaves the export set,
    // taking its gap with it.
    let hir = module_with_classes(
        vec![func(
            "Grid._scale.static",
            &[("who", Ty::List(Box::new(Ty::Int)))],
            Ty::Int,
        )],
        vec![("Grid".to_string(), class_def("Grid", None))],
    );
    assert_eq!(
        collect_exports(&hir).expect("the renamed method is no longer exported"),
        Vec::new()
    );
}

#[test]
fn two_static_methods_of_one_class_with_the_same_name_export_once() {
    // Python rebinds rather than redeclares inside a class body too, and the
    // dedup key is `(class, method)`: a bare name key would collapse two
    // different classes' `scale`, and a mangled-name key would let one class
    // publish `scale` twice -- once from `.static` and once from
    // `.classmethod` -- which the C compiler would reject as a duplicate
    // `ml_name` only at build time.
    let hir = module_with_classes(
        vec![
            func("Grid.scale.static", &[("n", Ty::Int)], Ty::Int),
            func(
                "Grid.scale.static",
                &[("a", Ty::Int), ("b", Ty::Int)],
                Ty::Int,
            ),
            func("Other.scale.static", &[("n", Ty::Int)], Ty::Int),
        ],
        vec![
            ("Grid".to_string(), class_def("Grid", None)),
            ("Other".to_string(), class_def("Other", None)),
        ],
    );
    let exports = collect_exports(&hir).expect("a rebind is not a capability gap");
    assert_eq!(
        exports
            .iter()
            .map(|e| (e.name.as_str(), e.params.len()))
            .collect::<Vec<_>>(),
        vec![("Grid.scale.static", 2), ("Other.scale.static", 1)]
    );
}

#[test]
fn the_driver_and_codegen_export_predicates_agree_on_every_shape() {
    // `src/ext_build.rs`'s `classify_export_name` and
    // `pycc_codegen::is_ext_exportable_name` duplicate one predicate across a
    // crate boundary, because `pycc_codegen` does not depend on `pycc_hir`
    // and so cannot call the one that owns `is_public_name`. Duplication
    // across a crate boundary is only safe while something proves the two
    // copies equal, which is this test. The codegen mirror is allowed to be
    // a *superset* on the class-table question alone -- it cannot see
    // `exception_type_tag` -- so the corpus here carries no class-table
    // dependence.
    for name in [
        "f",
        "_f",
        "",
        "Grid.scale.static",
        "Grid.make.classmethod",
        "_Grid.scale.static",
        "Grid._scale.static",
        "Grid.scale",
        "Grid.width.setter",
        "Grid.scale.other",
        "Grid.scale.static.extra",
        "0gen_identity_int",
        "0gen_f.scale.static",
        "Grid..static",
        ".static",
    ] {
        assert_eq!(
            classify_export_name(name).is_some(),
            pycc_codegen::is_ext_exportable_name(name),
            "predicates disagree on {name:?}"
        );
    }
}
