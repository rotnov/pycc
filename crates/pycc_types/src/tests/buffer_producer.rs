//! The artifact-owned buffer producer: what `crate::buffer` admits, and
//! every refusal that bounds it (Part 2a of #1142, issue #1165).
//!
//! Its own file rather than more lines in `tests.rs`, under AGENTS.md's
//! decomposability rule and the layout `constraints.rs` beside it
//! established. As a child module it sees the parent's private items
//! through `use super::*`, so nothing needed widened visibility.
//!
//! Every test here is non-`#[ignore]`d on purpose: CI's `llvm-cov` job runs
//! without `--include-ignored`, so an ignored test contributes nothing to
//! the changed-line denominator D-242 pins at 100%.

use super::*;

/// `def f(<params>) -> <ret>: <body>`, with no class table.
fn func(params: Vec<(String, Ty)>, return_ty: Ty, body: Vec<HirStmt>) -> HirModule {
    HirModule {
        seeded_builtin_exception_classes: false,
        items: vec![HirItem::Function {
            name: "f".to_string(),
            params,
            return_ty,
            body,
        }],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    }
}

/// `<callee>(<args>)`.
fn call(callee: &str, args: Vec<HirExpr>) -> HirExpr {
    HirExpr::Call {
        callee: callee.to_string(),
        args,
    }
}

/// `a = <callee>(4)`.
fn alloc_four(callee: &str) -> HirStmt {
    HirStmt::Assign {
        target: "a".to_string(),
        value: call(callee, vec![HirExpr::IntLiteral(4)]),
    }
}

/// `a[0]`.
fn element() -> HirExpr {
    HirExpr::Subscript {
        base: Box::new(HirExpr::Name("a".to_string())),
        index: Box::new(HirExpr::IntLiteral(0)),
    }
}

/// The admitted shape, in both spellings: allocate, store one element, read
/// one back. This is the whole of what Part 2a opens, so it is also the one
/// test that proves the producer is not refused outright.
#[test]
fn allocating_a_buffer_and_touching_one_element_type_checks() {
    for callee in ["ndarray", "NDArray"] {
        let hir = func(
            vec![],
            Ty::Float,
            vec![
                alloc_four(callee),
                HirStmt::DictSet {
                    dict: "a".to_string(),
                    key: HirExpr::IntLiteral(0),
                    value: HirExpr::FloatLiteral(1.5),
                },
                HirStmt::Return(Some(element())),
            ],
        );
        assert!(check(&hir).is_ok(), "{callee}");
    }
}

/// `len(a)` on artifact-owned storage, the sweep bound #1116 admitted for a
/// parameter: the length read keys on the *type*, so it reaches the owned
/// binding unchanged.
#[test]
fn reading_the_length_of_owned_storage_type_checks() {
    let hir = func(
        vec![],
        Ty::Int,
        vec![
            alloc_four("ndarray"),
            HirStmt::Return(Some(call("len", vec![HirExpr::Name("a".to_string())]))),
        ],
    );
    assert!(check(&hir).is_ok());
}

/// The annotated form of the same binding. `reject_memoryview_declaration`
/// refuses a bare `a: memoryview`, and Part 2a gates that refusal on the
/// initializer rather than changing its own message: with a producer on the
/// right-hand side the declaration is the admitted shape.
#[test]
fn an_annotated_buffer_binding_with_a_producer_is_admitted() {
    let hir = func(
        vec![],
        Ty::Float,
        vec![
            HirStmt::AnnAssign {
                target: "a".to_string(),
                annotation: Ty::MemoryView,
                value: Some(call("ndarray", vec![HirExpr::IntLiteral(4)])),
                is_final: false,
            },
            HirStmt::Return(Some(element())),
        ],
    );
    assert!(check(&hir).is_ok());
}

/// ...and the declaration with no initializer keeps the refusal it had.
#[test]
fn an_annotated_buffer_binding_without_a_producer_is_still_refused() {
    let hir = func(
        vec![],
        Ty::None,
        vec![HirStmt::AnnAssign {
            target: "a".to_string(),
            annotation: Ty::MemoryView,
            value: None,
            is_final: false,
        }],
    );
    assert_eq!(check(&hir).unwrap_err().code, "C0001");
}

/// Every position that is *not* an assignment's right-hand side gets the one
/// named producer-position refusal, rather than falling out incidentally as
/// a type mismatch somewhere downstream. The bare-statement row is the one
/// that matters for memory safety: without it the value is allocated,
/// never slotted, and leaked once per call.
#[test]
fn a_producer_outside_an_assignment_is_the_named_position_refusal() {
    for body in [
        vec![HirStmt::ExprStmt(call(
            "ndarray",
            vec![HirExpr::IntLiteral(4)],
        ))],
        vec![HirStmt::Assign {
            target: "n".to_string(),
            value: call("len", vec![call("ndarray", vec![HirExpr::IntLiteral(4)])]),
        }],
        vec![HirStmt::Assign {
            target: "a".to_string(),
            value: HirExpr::ListLiteral(vec![call("ndarray", vec![HirExpr::IntLiteral(4)])]),
        }],
        vec![HirStmt::Return(Some(call(
            "ndarray",
            vec![HirExpr::IntLiteral(4)],
        )))],
        vec![HirStmt::Assign {
            target: "a".to_string(),
            value: HirExpr::Subscript {
                base: Box::new(call("ndarray", vec![HirExpr::IntLiteral(4)])),
                index: Box::new(HirExpr::IntLiteral(0)),
            },
        }],
    ] {
        let hir = func(vec![], Ty::None, body.clone());
        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "C0001", "{body:?}");
        assert!(
            err.message
                .contains("whole right-hand side of an assignment"),
            "{}",
            err.message
        );
    }
}

/// A wrong arity is not a producer at all, so it reaches the same
/// position refusal -- whose message names the shape that *is* admitted.
#[test]
fn a_producer_with_the_wrong_arity_is_the_position_refusal() {
    let hir = func(
        vec![],
        Ty::None,
        vec![HirStmt::Assign {
            target: "a".to_string(),
            value: call(
                "ndarray",
                vec![HirExpr::IntLiteral(4), HirExpr::IntLiteral(5)],
            ),
        }],
    );
    let err = check(&hir).unwrap_err();
    assert_eq!(err.code, "C0001");
    assert!(
        err.message
            .contains("whole right-hand side of an assignment"),
        "{}",
        err.message
    );
}

/// Module scope gets its own refusal, because its ground is different: a
/// module-level frame gets no owned-slot epilogue, so free-at-exit has no
/// exit to run at. Part 2b lifts one of the two, not both.
#[test]
fn a_producer_at_module_scope_names_the_missing_frame() {
    for stmt in [
        HirStmt::Assign {
            target: "a".to_string(),
            value: call("ndarray", vec![HirExpr::IntLiteral(4)]),
        },
        HirStmt::AnnAssign {
            target: "a".to_string(),
            annotation: Ty::MemoryView,
            value: Some(call("ndarray", vec![HirExpr::IntLiteral(4)])),
            is_final: false,
        },
    ] {
        let hir = HirModule {
            seeded_builtin_exception_classes: false,
            items: vec![HirItem::TopLevelStmt(stmt)],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "C0001");
        assert!(err.message.contains("at module scope"), "{}", err.message);
    }
}

/// The length argument is an `int` element count, and a non-`int` says so in
/// the producer's own words rather than as a generic call mismatch.
#[test]
fn a_non_int_length_is_a_typed_refusal() {
    let hir = func(
        vec![],
        Ty::None,
        vec![HirStmt::Assign {
            target: "a".to_string(),
            value: call("ndarray", vec![HirExpr::StringLiteral("four".to_string())]),
        }],
    );
    let err = check(&hir).unwrap_err();
    assert_eq!(err.code, "T0033");
    assert!(err.message.contains("element count"), "{}", err.message);
}

/// A refusal raised while inferring the length argument is reported as
/// itself, not swallowed into the producer's own message.
#[test]
fn a_refusal_inside_the_length_argument_is_reported_unchanged() {
    let hir = func(
        vec![],
        Ty::None,
        vec![HirStmt::Assign {
            target: "a".to_string(),
            value: call("ndarray", vec![HirExpr::Name("missing".to_string())]),
        }],
    );
    let err = check(&hir).unwrap_err();
    assert!(err.message.contains("missing"), "{}", err.message);
}

/// Any use of owned storage beyond the three admitted operations is refused
/// in the *owned* wording, not the buffer-parameter one: `return a` here is
/// refused because egress does not exist yet, while the same statement on a
/// parameter is refused because it would hand back a view the host lent.
#[test]
fn any_other_use_of_owned_storage_names_the_artifact_as_its_owner() {
    let hir = func(
        vec![],
        Ty::MemoryView,
        vec![
            alloc_four("ndarray"),
            HirStmt::Return(Some(HirExpr::Name("a".to_string()))),
        ],
    );
    let err = check(&hir).unwrap_err();
    assert_eq!(err.code, "C0001");
    assert!(
        err.message
            .contains("this `pycc build --ext` artifact allocated"),
        "{}",
        err.message
    );
}

/// ...and a buffer *parameter* keeps its own message verbatim, so the two
/// provenances never report each other's reason.
#[test]
fn a_buffer_parameter_keeps_its_own_refusal() {
    let hir = func(
        vec![("b".to_string(), Ty::MemoryView)],
        Ty::MemoryView,
        vec![HirStmt::Return(Some(HirExpr::Name("b".to_string())))],
    );
    let err = check(&hir).unwrap_err();
    assert_eq!(err.code, "C0001");
    assert!(err.message.contains("buffer parameter"), "{}", err.message);
}

/// Rebinding a buffer parameter is refused, which is what keeps the flat
/// provenance model sound: no name is parameter-bound in one half of a
/// function and artifact-owned in the other.
#[test]
fn rebinding_a_buffer_parameter_is_refused() {
    let hir = func(
        vec![("b".to_string(), Ty::MemoryView)],
        Ty::None,
        vec![HirStmt::Assign {
            target: "b".to_string(),
            value: call("ndarray", vec![HirExpr::IntLiteral(4)]),
        }],
    );
    let err = check(&hir).unwrap_err();
    assert_eq!(err.code, "C0001");
    assert!(
        err.message
            .contains("a buffer parameter of a `pycc build --ext` export"),
        "{}",
        err.message
    );
}

/// D-244 #1129 statement (h) in call position: a program's own `def
/// ndarray` wins, and its result is that function's return type.
#[test]
fn a_program_that_defines_the_spelling_itself_keeps_its_own_meaning() {
    let hir = HirModule {
        seeded_builtin_exception_classes: false,
        items: vec![
            HirItem::Function {
                name: "ndarray".to_string(),
                params: vec![("n".to_string(), Ty::Int)],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::Name("n".to_string())))],
            },
            HirItem::Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: Ty::Int,
                body: vec![
                    alloc_four("ndarray"),
                    HirStmt::Return(Some(HirExpr::Name("a".to_string()))),
                ],
            },
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    assert!(check(&hir).is_ok());
}

/// The same statement through the class table rather than the function
/// table: `class NDArray` makes `NDArray(...)` an instantiation.
#[test]
fn a_program_that_defines_the_spelling_as_a_class_keeps_its_own_meaning() {
    let class_def = HirClassDef {
        class_attrs: Vec::new(),
        exception_type_tag: None,
        name: "NDArray".to_string(),
        bases: Vec::new(),
        mro: vec!["NDArray".to_string()],
        attrs: Vec::new(),
        methods: vec![("__init__".to_string(), "NDArray.__init__".to_string())],
        type_param: None,
        properties: Vec::new(),
        static_methods: Vec::new(),
        class_methods: Vec::new(),
        is_enum: false,
        implicit_object_init: false,
        enum_members: Vec::new(),
        is_dataclass: false,
        dataclass_fields: Vec::new(),
        is_protocol: false,
        runtime_checkable: false,
        protocol_members: Vec::new(),
        abstract_methods: Vec::new(),
        is_abstract: false,
    };
    let hir = HirModule {
        seeded_builtin_exception_classes: false,
        items: vec![
            HirItem::Function {
                name: "NDArray.__init__".to_string(),
                params: vec![
                    (
                        "self".to_string(),
                        Ty::Instance(Box::new("NDArray".to_string())),
                    ),
                    ("n".to_string(), Ty::Int),
                ],
                return_ty: Ty::None,
                body: vec![],
            },
            HirItem::Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: Ty::Instance(Box::new("NDArray".to_string())),
                body: vec![
                    alloc_four("NDArray"),
                    HirStmt::Return(Some(HirExpr::Name("a".to_string()))),
                ],
            },
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: vec![("NDArray".to_string(), class_def)],
    };
    assert!(check(&hir).is_ok());
}

/// A module-level binding of the spelling shadows it too -- and then the
/// call is refused as a call to a non-callable value, which is what CPython
/// would raise, rather than admitted as a producer.
#[test]
fn a_module_level_binding_of_the_spelling_shadows_the_producer() {
    let hir = HirModule {
        seeded_builtin_exception_classes: false,
        items: vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "ndarray".to_string(),
                value: HirExpr::IntLiteral(1),
            }),
            HirItem::Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: Ty::None,
                body: vec![alloc_four("ndarray")],
            },
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let err = check(&hir).unwrap_err();
    assert!(
        !err.message.contains("right-hand side of an assignment"),
        "{}",
        err.message
    );
}

/// Provenance survives a branch join in every joining shape: a name bound to
/// owned storage on one arm is still owned storage after the join, so the
/// use refusal that follows is the owned one rather than the parameter one.
#[test]
fn owned_provenance_survives_a_branch_join() {
    let hir = func(
        vec![("c".to_string(), Ty::Bool)],
        Ty::MemoryView,
        vec![
            HirStmt::If {
                test: HirExpr::Name("c".to_string()),
                body: vec![alloc_four("ndarray")],
                orelse: vec![alloc_four("NDArray")],
            },
            HirStmt::Return(Some(HirExpr::Name("a".to_string()))),
        ],
    );
    let err = check(&hir).unwrap_err();
    assert!(
        err.message
            .contains("this `pycc build --ext` artifact allocated"),
        "{}",
        err.message
    );
}

/// The same, through a loop body's join. The pre-loop allocation is not
/// decoration: a name bound *only* inside a loop body is `Maybe` after it
/// (D-147), so reading it is refused for that reason before provenance is
/// ever consulted -- the reachable shape is a reallocation inside the loop
/// of a name the frame already owns.
#[test]
fn owned_provenance_survives_a_loop_join() {
    let hir = func(
        vec![("c".to_string(), Ty::Bool)],
        Ty::MemoryView,
        vec![
            alloc_four("ndarray"),
            HirStmt::While {
                test: HirExpr::Name("c".to_string()),
                body: vec![alloc_four("NDArray")],
            },
            HirStmt::Return(Some(HirExpr::Name("a".to_string()))),
        ],
    );
    let err = check(&hir).unwrap_err();
    assert!(
        err.message
            .contains("this `pycc build --ext` artifact allocated"),
        "{}",
        err.message
    );
}

/// The public spelling predicate `src/memoryview_mode.rs`'s native-mode gate
/// consumes: exactly the two spellings, and `memoryview` deliberately not
/// among them.
#[test]
fn the_exported_spelling_predicate_names_exactly_the_two_producers() {
    assert!(crate::is_buffer_producer_spelling("ndarray"));
    assert!(crate::is_buffer_producer_spelling("NDArray"));
    assert!(!crate::is_buffer_producer_spelling("memoryview"));
}

/// The second export the native-mode gate consumes: the producer spellings a
/// function binds locally, filtered out of `function_local_names` so the gate
/// tests locality with the same computation
/// `buffer::producer_assignment_ty`'s statement-(h) block does. A parameter
/// and a body binding are both locals; a non-producer local is not returned,
/// and a function that binds neither returns nothing.
#[test]
fn the_exported_local_spellings_are_the_producers_a_function_binds() {
    let params = vec![("ndarray".to_string(), Ty::Int)];
    assert_eq!(
        crate::function_local_producer_spellings(&params, &[]),
        vec!["ndarray"]
    );

    let body = vec![
        HirStmt::Assign {
            target: "NDArray".to_string(),
            value: HirExpr::IntLiteral(1),
        },
        HirStmt::Assign {
            target: "other".to_string(),
            value: HirExpr::IntLiteral(2),
        },
    ];
    assert_eq!(
        crate::function_local_producer_spellings(&[], &body),
        vec!["NDArray"]
    );
    assert_eq!(
        crate::function_local_producer_spellings(&[], &body[1..]),
        Vec::<&str>::new()
    );
}

/// A module of two functions: an unannotated private helper `_h(n)` whose
/// body is `body`, and an annotated `f()` that calls it with `4`.
///
/// `Ty::Infer` on the helper is what routes the module through the
/// *constraint solver* rather than the concrete fast path, which is the only
/// way to reach `constraints.rs`'s own admitting seam for the producer. The
/// check phase's seam is reached by every other test in this file.
fn unannotated_helper_module(body: Vec<HirStmt>, f_return: Ty) -> HirModule {
    HirModule {
        seeded_builtin_exception_classes: false,
        items: vec![
            HirItem::Function {
                name: "_h".to_string(),
                params: vec![("n".to_string(), Ty::Infer)],
                return_ty: Ty::Infer,
                body,
            },
            HirItem::Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: f_return,
                body: vec![HirStmt::Return(Some(call(
                    "_h",
                    vec![HirExpr::IntLiteral(4)],
                )))],
            },
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    }
}

/// The solver's plain-`Assign` seam: an unannotated helper may allocate and
/// read back one element, exactly as an annotated function may.
#[test]
fn the_solver_admits_a_producer_in_an_unannotated_helper() {
    let hir = unannotated_helper_module(
        vec![
            HirStmt::Assign {
                target: "a".to_string(),
                value: call("ndarray", vec![HirExpr::Name("n".to_string())]),
            },
            HirStmt::Return(Some(element())),
        ],
        Ty::Float,
    );
    assert!(check(&hir).is_ok());
}

/// The solver's `AnnAssign` seam, which the plain-`Assign` seam above does
/// not reach: the annotated spelling of the same statement.
#[test]
fn the_solver_admits_an_annotated_producer_in_an_unannotated_helper() {
    let hir = unannotated_helper_module(
        vec![
            HirStmt::AnnAssign {
                target: "a".to_string(),
                annotation: Ty::MemoryView,
                value: Some(call("ndarray", vec![HirExpr::Name("n".to_string())])),
                is_final: false,
            },
            HirStmt::Return(Some(element())),
        ],
        Ty::Float,
    );
    assert!(check(&hir).is_ok());
}

/// A diagnostic raised while the solver collects the length argument's own
/// constraints propagates out of both seams rather than being swallowed by
/// the producer's admission.
#[test]
fn the_solver_propagates_a_refusal_from_the_length_argument() {
    for value in [
        // A nested producer: the *inner* call is in a length-argument
        // position, not an assignment's right-hand side, so collecting the
        // outer call's length argument is what raises the refusal.
        call(
            "ndarray",
            vec![call("ndarray", vec![HirExpr::IntLiteral(4)])],
        ),
        call("ndarray", vec![HirExpr::Name("missing".to_string())]),
    ] {
        for stmt in [
            HirStmt::Assign {
                target: "a".to_string(),
                value: value.clone(),
            },
            HirStmt::AnnAssign {
                target: "a".to_string(),
                annotation: Ty::MemoryView,
                value: Some(value.clone()),
                is_final: false,
            },
        ] {
            let hir = unannotated_helper_module(
                vec![stmt.clone(), HirStmt::Return(Some(element()))],
                Ty::Float,
            );
            assert!(check(&hir).is_err(), "{stmt:?}");
        }
    }
}

/// The solver's own parameter-rebinding guard, in both assignment
/// spellings. Without it the solver admits the rebinding, marks `b`
/// artifact-owned, and the later read of `b` raises the *owned* refusal --
/// which `module::merge_solver_first` then prefers over the check phase's
/// correct *parameter* refusal, making #1165's acceptance criterion 2 false
/// for this reachable shape.
///
/// `_h` takes a concrete `memoryview` parameter but leaves its return type
/// inferred, which is what routes it through the constraint solver rather
/// than the concrete fast path (see `unannotated_helper_module`).
#[test]
fn the_solver_refuses_rebinding_a_buffer_parameter() {
    for assignment in [
        HirStmt::Assign {
            target: "b".to_string(),
            value: call("ndarray", vec![HirExpr::IntLiteral(4)]),
        },
        HirStmt::AnnAssign {
            target: "b".to_string(),
            annotation: Ty::MemoryView,
            value: Some(call("ndarray", vec![HirExpr::IntLiteral(4)])),
            is_final: false,
        },
    ] {
        let hir = HirModule {
            seeded_builtin_exception_classes: false,
            items: vec![
                HirItem::Function {
                    name: "_h".to_string(),
                    params: vec![("b".to_string(), Ty::MemoryView)],
                    return_ty: Ty::Infer,
                    body: vec![
                        assignment.clone(),
                        HirStmt::Return(Some(HirExpr::Name("b".to_string()))),
                    ],
                },
                HirItem::Function {
                    name: "f".to_string(),
                    params: vec![("v".to_string(), Ty::MemoryView)],
                    return_ty: Ty::MemoryView,
                    body: vec![HirStmt::Return(Some(call(
                        "_h",
                        vec![HirExpr::Name("v".to_string())],
                    )))],
                },
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "C0001", "{assignment:?}");
        assert!(
            err.message
                .contains("a buffer parameter of a `pycc build --ext` export"),
            "{}",
            err.message
        );
    }
}

/// `b = a` -- a bare read of an owned name, which is what makes a masked
/// producer refusal *observable*: it is the use that raises the owned-buffer
/// `C0001` the solver's own answer would otherwise displace the correct
/// diagnostic with. A subscript read would not, since `a[0]` is admitted.
fn read_whole_buffer() -> HirStmt {
    HirStmt::Assign {
        target: "b".to_string(),
        value: HirExpr::Name("a".to_string()),
    }
}

/// The text of `buffer::owned_buffer_use_unsupported`, for the "and the
/// owned-buffer refusal is *absent*" half of every assertion below. Pinning
/// only the expected code passes just as well when both diagnostics are
/// produced and the wrong one wins, which is exactly the state these tests
/// exist to refuse.
const OWNED_BUFFER_REFUSAL: &str =
    "bound to buffer storage this `pycc build --ext` artifact allocated";

/// Codex review round 8: the solver collected the producer's length term and
/// threw it away, so `a = ndarray("x")` in an unannotated helper recorded an
/// owned buffer, the later read raised the owned-buffer `C0001`, and
/// `module::merge_solver_first` reported that instead of the check phase's
/// correct `T0033`.
#[test]
fn the_solver_refuses_a_non_int_producer_length() {
    for callee in ["ndarray", "NDArray"] {
        let producer = call(callee, vec![HirExpr::StringLiteral("x".to_string())]);
        for assignment in [
            HirStmt::Assign {
                target: "a".to_string(),
                value: producer.clone(),
            },
            HirStmt::AnnAssign {
                target: "a".to_string(),
                annotation: Ty::MemoryView,
                value: Some(producer.clone()),
                is_final: false,
            },
        ] {
            let hir = unannotated_helper_module(
                vec![
                    assignment.clone(),
                    read_whole_buffer(),
                    HirStmt::Return(Some(HirExpr::IntLiteral(0))),
                ],
                Ty::Int,
            );
            let err = check(&hir).unwrap_err();
            assert_eq!(err.code, "T0033", "{assignment:?}");
            assert_eq!(
                err.message,
                format!("`{callee}` expects an `int` element count, got `str`"),
            );
            assert!(
                !err.message.contains(OWNED_BUFFER_REFUSAL),
                "{}",
                err.message
            );
        }
    }
}

/// The same masking through the `AnnAssign` arm's *other* missing
/// condition: an annotation that does not admit a buffer. The solver bound
/// `a` as artifact-owned regardless of what `a` was declared as, so the
/// later read displaced the check phase's `T0025`.
///
/// The expected message is taken from the check phase's own answer to the
/// identical statement in an annotated function rather than written out
/// here, which is what pins the two phases to the one canonical `T0025`
/// (`crate::annotation_initializer_mismatch`).
#[test]
fn the_solver_refuses_a_producer_under_an_annotation_that_rejects_it() {
    for annotation in [Ty::Int, Ty::Str, Ty::List(Box::new(Ty::Int))] {
        let assignment = HirStmt::AnnAssign {
            target: "a".to_string(),
            annotation: annotation.clone(),
            value: Some(call("ndarray", vec![HirExpr::IntLiteral(4)])),
            is_final: false,
        };
        let solver = check(&unannotated_helper_module(
            vec![
                assignment.clone(),
                read_whole_buffer(),
                HirStmt::Return(Some(HirExpr::IntLiteral(0))),
            ],
            Ty::Int,
        ))
        .unwrap_err();
        let checker = check(&func(
            vec![],
            Ty::Int,
            vec![
                assignment.clone(),
                HirStmt::Return(Some(HirExpr::IntLiteral(0))),
            ],
        ))
        .unwrap_err();
        assert_eq!(solver.code, "T0025", "{annotation:?}");
        assert_eq!(solver.message, checker.message, "{annotation:?}");
        assert_eq!(solver.help, checker.help, "{annotation:?}");
        assert!(
            !solver.message.contains(OWNED_BUFFER_REFUSAL),
            "{}",
            solver.message
        );
    }
}

/// The keep path for both guards above: a *valid* producer in an
/// unannotated helper is still recognized, still marks the name
/// artifact-owned, and a later whole-buffer read is still the owned-buffer
/// `C0001`. Neither guard may over-suppress into admitting nothing.
#[test]
fn a_valid_producer_in_an_unannotated_helper_still_owns_its_buffer() {
    for annotation in [None, Some(Ty::MemoryView)] {
        let value = call("ndarray", vec![HirExpr::IntLiteral(4)]);
        let assignment = match annotation.clone() {
            None => HirStmt::Assign {
                target: "a".to_string(),
                value,
            },
            Some(annotation) => HirStmt::AnnAssign {
                target: "a".to_string(),
                annotation,
                value: Some(value),
                is_final: false,
            },
        };
        let hir = unannotated_helper_module(
            vec![
                assignment.clone(),
                read_whole_buffer(),
                HirStmt::Return(Some(HirExpr::IntLiteral(0))),
            ],
            Ty::Int,
        );
        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "C0001", "{assignment:?}");
        assert!(
            err.message.contains(OWNED_BUFFER_REFUSAL),
            "{}",
            err.message
        );
    }
}

/// The length guard admits a `bool`, exactly as the check phase's own
/// `Int | Bool` match does (`docs/TYPE_SYSTEM.md` rule 4/D-086), and admits
/// a length term that is still unresolved at the seam -- the helper's own
/// unannotated parameter, which is the admitted shape the guard must not
/// refuse.
#[test]
fn the_solver_admits_a_bool_length_and_an_unresolved_one() {
    for length in [HirExpr::BoolLiteral(true), HirExpr::Name("n".to_string())] {
        let hir = unannotated_helper_module(
            vec![
                HirStmt::Assign {
                    target: "a".to_string(),
                    value: call("ndarray", vec![length.clone()]),
                },
                HirStmt::Return(Some(element())),
            ],
            Ty::Float,
        );
        assert!(check(&hir).is_ok(), "{length:?}");
    }
}

/// `a: Final[<annotation>] = ndarray(4)`.
fn final_producer(annotation: Ty) -> HirStmt {
    HirStmt::AnnAssign {
        target: "a".to_string(),
        annotation,
        value: Some(call("ndarray", vec![HirExpr::IntLiteral(4)])),
        is_final: true,
    }
}

/// The fourth member of the same class, found by enumerating
/// `check_assignment` rather than reported: PEP 591's `T0045` is one of the
/// refusals that function applies to *every* assignment target, and the
/// solver's producer seam applied none of it but the parameter-rebinding
/// guard. A reassignment of a `Final` buffer name was therefore admitted by
/// the solver, recorded as artifact-owned, and the later read displaced the
/// check phase's `T0045` with the owned-buffer `C0001`.
///
/// Both producer arms are driven: the reassignment is the plain `Assign`
/// spelling in the first case and the annotated one in the second.
#[test]
fn the_solver_refuses_reassigning_a_final_buffer_name() {
    for reassignment in [
        HirStmt::Assign {
            target: "a".to_string(),
            value: call("ndarray", vec![HirExpr::IntLiteral(8)]),
        },
        HirStmt::AnnAssign {
            target: "a".to_string(),
            annotation: Ty::MemoryView,
            value: Some(call("ndarray", vec![HirExpr::IntLiteral(8)])),
            is_final: false,
        },
    ] {
        let hir = unannotated_helper_module(
            vec![
                final_producer(Ty::MemoryView),
                reassignment.clone(),
                read_whole_buffer(),
                HirStmt::Return(Some(HirExpr::IntLiteral(0))),
            ],
            Ty::Int,
        );
        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0045", "{reassignment:?}");
        assert_eq!(err.message, "cannot reassign `Final` name `a`");
        assert!(
            !err.message.contains(OWNED_BUFFER_REFUSAL),
            "{}",
            err.message
        );
    }
}

/// The keep path for the `Final` guard: the declaration's *own* first
/// assignment is not a reassignment, so a `Final` buffer binding is still
/// admitted and still artifact-owned. A guard recording finality before the
/// binding, rather than after it as the check phase does, would refuse this.
#[test]
fn a_final_buffer_declaration_is_still_admitted_and_still_owned() {
    let hir = unannotated_helper_module(
        vec![
            final_producer(Ty::MemoryView),
            read_whole_buffer(),
            HirStmt::Return(Some(HirExpr::IntLiteral(0))),
        ],
        Ty::Int,
    );
    let err = check(&hir).unwrap_err();
    assert_eq!(err.code, "C0001");
    assert!(
        err.message.contains(OWNED_BUFFER_REFUSAL),
        "{}",
        err.message
    );
}

/// A `Final` name declared inside one arm of an `if` is still `Final` after
/// the join, so a reassignment below it is refused: the solver collects each
/// branch in a clone of the environment, and the join must union the set the
/// way it already unions buffer provenance.
#[test]
fn final_names_survive_a_branch_and_a_loop_join() {
    for wrap in [
        |stmt: HirStmt| HirStmt::If {
            test: HirExpr::BoolLiteral(true),
            body: vec![stmt],
            orelse: Vec::new(),
        },
        |stmt: HirStmt| HirStmt::While {
            test: HirExpr::BoolLiteral(false),
            body: vec![stmt],
        },
    ] {
        let hir = unannotated_helper_module(
            vec![
                wrap(final_producer(Ty::MemoryView)),
                HirStmt::Assign {
                    target: "a".to_string(),
                    value: call("ndarray", vec![HirExpr::IntLiteral(8)]),
                },
                read_whole_buffer(),
                HirStmt::Return(Some(HirExpr::IntLiteral(0))),
            ],
            Ty::Int,
        );
        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0045");
    }
}
