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

/// Any use of owned storage beyond the admitted operations is refused in
/// the *owned* wording, not the buffer-parameter one. The probe is an alias
/// `z = a`, not `return a`: Part 2b of #1142 (#1164) admits the return of an
/// owned name, so the statement that used to carry this assertion now type
/// checks. Aliasing is still refused, and it is refused through the same
/// `crate::expr::reject_memoryview_read` owned arm.
#[test]
fn any_other_use_of_owned_storage_names_the_artifact_as_its_owner() {
    let hir = func(
        vec![],
        Ty::None,
        vec![
            alloc_four("ndarray"),
            HirStmt::Assign {
                target: "z".to_string(),
                value: HirExpr::Name("a".to_string()),
            },
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

/// ...and the one use Part 2b of #1142 (#1164) *does* admit: returning the
/// owned name from a function whose declared return type is the buffer type.
/// Two-directional, so a future change that reinstates the refusal cannot
/// pass by merely changing its wording.
#[test]
fn returning_owned_storage_is_admitted_by_egress() {
    for callee in ["ndarray", "NDArray"] {
        let hir = func(
            vec![],
            Ty::MemoryView,
            vec![
                alloc_four(callee),
                HirStmt::Return(Some(HirExpr::Name("a".to_string()))),
            ],
        );
        assert!(check(&hir).is_ok(), "{callee}");
    }
}

/// Egress admits the *owned* name only. A buffer parameter returned from the
/// same signature keeps its own pre-existing refusal, because handing back a
/// view the host lent for one call is a use-after-free rather than a missing
/// capability.
#[test]
fn returning_a_buffer_parameter_is_still_refused_by_its_own_arm() {
    let hir = func(
        vec![("b".to_string(), Ty::MemoryView)],
        Ty::MemoryView,
        vec![HirStmt::Return(Some(HirExpr::Name("b".to_string())))],
    );
    let err = check(&hir).unwrap_err();
    assert_eq!(err.code, "C0001");
    assert!(err.message.contains("buffer parameter"), "{}", err.message);
    assert!(
        !err.message
            .contains("this `pycc build --ext` artifact allocated"),
        "{}",
        err.message
    );
}

/// Egress keys on the declared return type, not on the operand alone: an
/// owned name returned from a function declared to return something else is
/// still the ordinary return-type mismatch, not a silent admission.
#[test]
fn returning_owned_storage_from_a_non_buffer_signature_is_a_type_error() {
    let hir = func(
        vec![],
        Ty::Float,
        vec![
            alloc_four("ndarray"),
            HirStmt::Return(Some(HirExpr::Name("a".to_string()))),
        ],
    );
    let err = check(&hir).unwrap_err();
    assert_ne!(err.code, "C0003", "{}", err.message);
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
/// The post-join probe is an alias rather than `return a`, which #1164 now
/// admits; `owned_storage_returned_after_a_branch_join_is_admitted` covers
/// the joining shape on the admitted side.
#[test]
fn owned_provenance_survives_a_branch_join() {
    let hir = func(
        vec![("c".to_string(), Ty::Bool)],
        Ty::None,
        vec![
            HirStmt::If {
                test: HirExpr::Name("c".to_string()),
                body: vec![alloc_four("ndarray")],
                orelse: vec![alloc_four("NDArray")],
            },
            HirStmt::Assign {
                target: "z".to_string(),
                value: HirExpr::Name("a".to_string()),
            },
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
        Ty::None,
        vec![
            alloc_four("ndarray"),
            HirStmt::While {
                test: HirExpr::Name("c".to_string()),
                body: vec![alloc_four("NDArray")],
            },
            HirStmt::Assign {
                target: "z".to_string(),
                value: HirExpr::Name("a".to_string()),
            },
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

/// The joining shapes on the admitted side: a name bound to owned storage on
/// both arms of a branch, and one reallocated inside a loop body, are each
/// still returnable after the join. Egress reads the same `owned_buffers`
/// set the refusal above reads, so the two tests move together.
#[test]
fn owned_storage_returned_after_a_branch_join_is_admitted() {
    let branch = func(
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
    assert!(check(&branch).is_ok());

    let loop_join = func(
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
    assert!(check(&loop_join).is_ok());
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

/// The text of `buffer::buffer_parameter_rebinding`, for the second half of
/// the two-directional assertion below. Clearing provenance while leaving
/// the stale `Ty::MemoryView` binding in place would swap the owned refusal
/// for this one, so a test that pinned only [`OWNED_BUFFER_REFUSAL`]'s
/// absence would pass on that non-fix.
const PARAMETER_BUFFER_REFUSAL: &str = "a buffer parameter of a `pycc build --ext` export";

/// `a = <value>`, the reviewer's own shape.
fn rebind_a(value: HirExpr) -> HirStmt {
    HirStmt::Assign {
        target: "a".to_string(),
        value,
    }
}

/// `range(0, 3, 1)` as a comprehension iterable.
fn comp_range() -> CompIter {
    CompIter::Range {
        start: HirExpr::IntLiteral(0),
        stop: HirExpr::IntLiteral(3),
        step: HirExpr::IntLiteral(1),
    }
}

/// Every statement shape that rebinds the name `a`, each preceded by
/// `a = ndarray(4)` in [`a_rebinding_of_an_owned_buffer_name_drops_its_
/// stale_provenance`].
///
/// A table rather than prose because it *is* the enumeration the round-11
/// finding asked for: the reviewer's counter-example was the plain `Assign`
/// arm, and the condition it belongs to is "any binder that is not the
/// producer seam". Every member below was confirmed to mask the check
/// phase's own diagnostic before the fix.
///
/// `ForRange` is in the table as a regression pin rather than as a member
/// that was broken: its arm already unifies the existing term against
/// `Ty::Int` and so already reported `T0023`. The `while`/`if` entries are
/// the two join helpers, which is where a per-site invalidation alone is
/// undone by the provenance union.
fn owned_buffer_rebinding_shapes() -> Vec<(&'static str, Vec<HirStmt>)> {
    vec![
        ("assign", vec![rebind_a(HirExpr::IntLiteral(1))]),
        (
            "annassign",
            vec![HirStmt::AnnAssign {
                target: "a".to_string(),
                annotation: Ty::Int,
                value: Some(HirExpr::IntLiteral(1)),
                is_final: false,
            }],
        ),
        (
            "walrus",
            vec![HirStmt::ExprStmt(HirExpr::NamedExpr {
                name: "a".to_string(),
                value: Box::new(HirExpr::IntLiteral(1)),
            })],
        ),
        (
            "for-range",
            vec![HirStmt::ForRange {
                var: "a".to_string(),
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
                body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(0)))],
            }],
        ),
        (
            "for-list",
            vec![
                HirStmt::Assign {
                    target: "lst".to_string(),
                    value: HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1)]),
                },
                HirStmt::ForList {
                    var: "a".to_string(),
                    list: "lst".to_string(),
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(0)))],
                },
            ],
        ),
        (
            "for-object",
            vec![HirStmt::ForObject {
                var: "a".to_string(),
                iter: Box::new(HirExpr::AttrGet {
                    base: Box::new(HirExpr::Name("n".to_string())),
                    attr: "rows".to_string(),
                }),
                body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(0)))],
            }],
        ),
        (
            "list-comprehension",
            vec![HirStmt::ListCompAssign {
                target: "a".to_string(),
                var: "q".to_string(),
                iter: comp_range(),
                cond: None,
                elt: Box::new(HirExpr::Name("q".to_string())),
            }],
        ),
        (
            "set-comprehension",
            vec![HirStmt::SetCompAssign {
                target: "a".to_string(),
                var: "q".to_string(),
                iter: comp_range(),
                cond: None,
                elt: Box::new(HirExpr::Name("q".to_string())),
            }],
        ),
        (
            "dict-comprehension",
            vec![HirStmt::DictCompAssign {
                target: "a".to_string(),
                var: "q".to_string(),
                iter: comp_range(),
                cond: None,
                key: Box::new(HirExpr::Name("q".to_string())),
                value: Box::new(HirExpr::Name("q".to_string())),
            }],
        ),
        (
            "match-capture",
            vec![HirStmt::Match {
                subject: HirExpr::IntLiteral(1),
                cases: vec![pycc_hir::HirMatchCase {
                    pattern: pycc_hir::HirPattern::Capture("a".to_string()),
                    guard: None,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(0)))],
                }],
            }],
        ),
        (
            "except-as",
            vec![HirStmt::Try {
                body: vec![HirStmt::Assign {
                    target: "z".to_string(),
                    value: HirExpr::IntLiteral(1),
                }],
                handlers: vec![pycc_hir::HirExceptHandler {
                    exc_type: Some(vec!["ValueError".to_string()]),
                    name: Some("a".to_string()),
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(0)))],
                }],
                orelse: Vec::new(),
                finalbody: Vec::new(),
            }],
        ),
        (
            "except-star-as",
            vec![HirStmt::TryStar {
                body: vec![HirStmt::Assign {
                    target: "z".to_string(),
                    value: HirExpr::IntLiteral(1),
                }],
                handlers: vec![pycc_hir::HirExceptHandler {
                    exc_type: Some(vec!["ValueError".to_string()]),
                    name: Some("a".to_string()),
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(0)))],
                }],
                orelse: Vec::new(),
                finalbody: Vec::new(),
            }],
        ),
        (
            "loop-body",
            vec![HirStmt::While {
                test: HirExpr::BoolLiteral(false),
                body: vec![rebind_a(HirExpr::IntLiteral(1))],
            }],
        ),
        (
            "one-branch",
            vec![HirStmt::If {
                test: HirExpr::BoolLiteral(true),
                body: vec![rebind_a(HirExpr::IntLiteral(1))],
                orelse: Vec::new(),
            }],
        ),
    ]
}

/// Round-11 review finding 2: `a = ndarray(4)` then `a = 1` then `b = a` in
/// an unannotated private helper reported the artifact-owned `C0001` for the
/// read instead of the check phase's `T0023` for the reassignment. The
/// solver kept `a`'s original `Ok(Ty::MemoryView)` term (ordinary
/// assignments are first-term-wins) *and* its `owned_buffers` marker, so the
/// read was refused before `module::merge_solver_first` could prefer the
/// correct answer.
///
/// Every shape in [`owned_buffer_rebinding_shapes`] is driven, and the
/// expected diagnostic is taken from the check phase's own answer to the
/// identical program without the read rather than written out here -- the
/// technique `the_solver_refuses_a_producer_under_an_annotation_that_rejects
/// _it` established, and what keeps this table from pinning a message that
/// could drift from `check_assignment`'s.
#[test]
fn a_rebinding_of_an_owned_buffer_name_drops_its_stale_provenance() {
    // Every mismatch is collected rather than asserted in place: the table
    // is the enumeration, so a failure must name *which* shapes regressed,
    // not only the first one.
    let mut wrong: Vec<String> = Vec::new();
    for (label, rebinding) in owned_buffer_rebinding_shapes() {
        let mut body = vec![alloc_four("ndarray")];
        body.extend(rebinding);
        let mut without_read = body.clone();
        without_read.push(HirStmt::Return(Some(HirExpr::IntLiteral(0))));
        body.push(read_whole_buffer());
        body.push(HirStmt::Return(Some(HirExpr::IntLiteral(0))));

        let expected = check(&unannotated_helper_module(without_read, Ty::Int)).unwrap_err();
        let actual = check(&unannotated_helper_module(body, Ty::Int)).unwrap_err();
        if actual.code != expected.code
            || actual.message != expected.message
            || actual.message.contains(OWNED_BUFFER_REFUSAL)
            || actual.message.contains(PARAMETER_BUFFER_REFUSAL)
        {
            wrong.push(format!(
                "{label}: got {} `{}`, expected {} `{}`",
                actual.code, actual.message, expected.code, expected.message
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// The reviewer's own program, pinned code-for-code rather than against the
/// check phase's answer, in both the unannotated-helper spelling that the
/// solver infers and the fully annotated spelling that it does not. The
/// annotated spelling masked identically before the fix -- this solver walks
/// every function body, not only the ones whose signature it has to infer --
/// so a fix verified on the helper alone would have left half the class
/// open.
#[test]
fn reassigning_an_owned_buffer_to_an_int_reports_the_reassignment() {
    let body = vec![
        alloc_four("ndarray"),
        rebind_a(HirExpr::IntLiteral(1)),
        read_whole_buffer(),
        HirStmt::Return(Some(HirExpr::IntLiteral(0))),
    ];
    for hir in [
        unannotated_helper_module(body.clone(), Ty::Int),
        func(vec![], Ty::Int, body.clone()),
    ] {
        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0023");
        assert_eq!(
            err.message,
            "cannot assign `int` to `a`, previously inferred as `memoryview`"
        );
        assert!(!err.message.contains(OWNED_BUFFER_REFUSAL));
        assert!(!err.message.contains(PARAMETER_BUFFER_REFUSAL));
    }
}

/// The keep path the invalidation must not swallow: reassigning an owned
/// name to *another* buffer keeps it owned, because codegen frees the
/// previous allocation before the store (D-074) and `check_assignment`
/// admits the rebinding for exactly that reason. Driven through both
/// producer arms.
#[test]
fn reallocating_an_owned_buffer_keeps_its_provenance() {
    for realloc in [
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
                alloc_four("ndarray"),
                realloc.clone(),
                read_whole_buffer(),
                HirStmt::Return(Some(HirExpr::IntLiteral(0))),
            ],
            Ty::Int,
        );
        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "C0001", "{realloc:?}");
        assert!(
            err.message.contains(OWNED_BUFFER_REFUSAL),
            "{}",
            err.message
        );
    }
}

/// The join's own keep path, and why the invalidation is keyed on "some
/// branch binds this name to something else" rather than on intersecting the
/// two owned sets: a name each branch allocates is still owned after the
/// join, and a name only one branch allocates must not be dropped merely
/// because the other branch never mentions it.
#[test]
fn a_buffer_allocated_in_every_branch_is_still_owned_after_the_join() {
    let hir = unannotated_helper_module(
        vec![
            HirStmt::If {
                test: HirExpr::BoolLiteral(true),
                body: vec![alloc_four("ndarray")],
                orelse: vec![alloc_four("NDArray")],
            },
            HirStmt::Return(Some(element())),
        ],
        Ty::Float,
    );
    assert!(check(&hir).is_ok());
}

/// The contested case the join helper is for: one branch allocates, the
/// other binds the same name to an `int`. Neither per-site invalidation nor
/// the provenance union can answer this alone -- the branch that allocates
/// re-adds the marker and its `Ty::MemoryView` term wins the first-term-wins
/// binding merge -- so the read reported the owned-buffer `C0001` over the
/// check phase's `T0023` for the conflicting join.
#[test]
fn a_buffer_allocated_in_only_one_branch_is_contested_by_the_other() {
    let hir = unannotated_helper_module(
        vec![
            HirStmt::If {
                test: HirExpr::BoolLiteral(true),
                body: vec![alloc_four("ndarray")],
                orelse: vec![rebind_a(HirExpr::IntLiteral(1))],
            },
            read_whole_buffer(),
            HirStmt::Return(Some(HirExpr::IntLiteral(0))),
        ],
        Ty::Int,
    );
    let err = check(&hir).unwrap_err();
    assert_eq!(err.code, "T0023");
    assert!(!err.message.contains(OWNED_BUFFER_REFUSAL));
    assert!(!err.message.contains(PARAMETER_BUFFER_REFUSAL));
}

/// The solver half of Part 2b of #1142 (#1164)'s egress interception. The
/// module carries an unannotated helper so the constraint solver -- not the
/// concrete fast path -- is what walks `g`'s `return a`, and the admission
/// must hold there too: the solver runs first, so without its own
/// interception the owned-buffer `C0001` would displace the admission before
/// the check phase ever looked.
#[test]
fn the_solver_admits_an_owned_buffer_return() {
    let hir = HirModule {
        seeded_builtin_exception_classes: false,
        items: vec![
            HirItem::Function {
                name: "_h".to_string(),
                params: vec![("n".to_string(), Ty::Infer)],
                return_ty: Ty::Infer,
                body: vec![HirStmt::Return(Some(HirExpr::Name("n".to_string())))],
            },
            HirItem::Function {
                name: "g".to_string(),
                params: vec![],
                return_ty: Ty::MemoryView,
                body: vec![
                    alloc_four("ndarray"),
                    HirStmt::Return(Some(HirExpr::Name("a".to_string()))),
                ],
            },
            HirItem::Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(call(
                    "_h",
                    vec![HirExpr::IntLiteral(4)],
                )))],
            },
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    assert!(check(&hir).is_ok());
}

/// An *inferred* return type declines the egress admission: only a written
/// `-> memoryview` annotation admits one, so an unannotated helper that
/// returns its own owned storage keeps the owned-buffer refusal.
#[test]
fn an_inferred_return_type_does_not_admit_an_owned_buffer_return() {
    let hir = unannotated_helper_module(
        vec![
            alloc_four("ndarray"),
            HirStmt::Return(Some(HirExpr::Name("a".to_string()))),
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

/// An intra-artifact call to a buffer-returning function is refused by the
/// check phase, naming the callee. This is what keeps
/// `crates/pycc_codegen/src/call_result.rs`'s `Ty::MemoryView` panic
/// unreachable from source (#1164's third completion criterion).
#[test]
fn calling_a_buffer_returning_function_is_refused() {
    let hir = HirModule {
        seeded_builtin_exception_classes: false,
        items: vec![
            HirItem::Function {
                name: "make".to_string(),
                params: vec![],
                return_ty: Ty::MemoryView,
                body: vec![
                    alloc_four("ndarray"),
                    HirStmt::Return(Some(HirExpr::Name("a".to_string()))),
                ],
            },
            HirItem::Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: Ty::MemoryView,
                body: vec![HirStmt::Return(Some(call("make", vec![])))],
            },
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let err = check(&hir).unwrap_err();
    assert_eq!(err.code, "C0001");
    assert!(
        err.message
            .contains("calling `make`, whose return type is a buffer"),
        "{}",
        err.message
    );
    assert!(
        !err.message.contains(OWNED_BUFFER_REFUSAL),
        "{}",
        err.message
    );
}

/// ...and the solver refuses the same call with the same text, so the two
/// walkers cannot drift. The unannotated helper is what routes the call
/// through the solver rather than the concrete fast path.
#[test]
fn the_solver_refuses_calling_a_buffer_returning_function() {
    let hir = HirModule {
        seeded_builtin_exception_classes: false,
        items: vec![
            HirItem::Function {
                name: "make".to_string(),
                params: vec![],
                return_ty: Ty::MemoryView,
                body: vec![
                    alloc_four("ndarray"),
                    HirStmt::Return(Some(HirExpr::Name("a".to_string()))),
                ],
            },
            HirItem::Function {
                name: "_h".to_string(),
                params: vec![("n".to_string(), Ty::Infer)],
                return_ty: Ty::Infer,
                body: vec![HirStmt::Return(Some(call("make", vec![])))],
            },
            HirItem::Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(call(
                    "_h",
                    vec![HirExpr::IntLiteral(4)],
                )))],
            },
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let err = check(&hir).unwrap_err();
    assert_eq!(err.code, "C0001");
    assert!(
        err.message
            .contains("calling `make`, whose return type is a buffer"),
        "{}",
        err.message
    );
}

/// The egress admission in `check_stmt_in_function` skips the ordinary
/// assignability check whenever the returned name is in `owned_buffers`,
/// trusting that membership implies a `Ty::MemoryView` term. That invariant
/// is maintained by the contested-join invalidation rule, not by the
/// admission itself, so this pins the one shape where a regression in that
/// rule would turn into a *silent* admission rather than a diagnostic: a
/// contested name returned from a function that really is annotated
/// `-> memoryview`. Every other contested read reports through a position
/// the admission never reaches.
#[test]
fn a_contested_buffer_is_not_silently_admitted_by_the_egress_return() {
    let hir = HirModule {
        seeded_builtin_exception_classes: false,
        items: vec![HirItem::Function {
            name: "make".to_string(),
            params: vec![],
            return_ty: Ty::MemoryView,
            body: vec![
                HirStmt::If {
                    test: HirExpr::BoolLiteral(true),
                    body: vec![alloc_four("ndarray")],
                    orelse: vec![rebind_a(HirExpr::IntLiteral(1))],
                },
                HirStmt::Return(Some(HirExpr::Name("a".to_string()))),
            ],
        }],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let err = check(&hir).unwrap_err();
    assert_eq!(err.code, "T0023", "{}", err.message);
    assert!(
        !err.message.contains(OWNED_BUFFER_REFUSAL),
        "{}",
        err.message
    );
}

/// `return a` from a function declared `-> memoryview`, after `body`.
fn egress_after(body: Vec<HirStmt>) -> HirModule {
    let mut stmts = body;
    stmts.push(HirStmt::Return(Some(HirExpr::Name("a".to_string()))));
    func(vec![("c".to_string(), Ty::Bool)], Ty::MemoryView, stmts)
}

/// `match c: case True: <cases.0>  case _: <cases.1>`, the one `match` shape
/// this file needs: a `bool` subject two singleton patterns cover.
fn match_bool(first: Vec<HirStmt>, wildcard: Vec<HirStmt>) -> HirStmt {
    HirStmt::Match {
        subject: HirExpr::Name("c".to_string()),
        cases: vec![
            pycc_hir::HirMatchCase {
                pattern: pycc_hir::HirPattern::Singleton(true),
                guard: None,
                body: first,
            },
            pycc_hir::HirMatchCase {
                pattern: pycc_hir::HirPattern::Wildcard,
                guard: None,
                body: wildcard,
            },
        ],
    }
}

/// `try: <body> except ValueError: <handler>`.
fn try_except(body: Vec<HirStmt>, handler: Vec<HirStmt>) -> HirStmt {
    HirStmt::Try {
        body,
        handlers: vec![pycc_hir::HirExceptHandler {
            exc_type: Some(vec!["ValueError".to_string()]),
            name: None,
            body: handler,
        }],
        orelse: Vec::new(),
        finalbody: Vec::new(),
    }
}

/// The definite-assignment half of the egress admission (review round 3 of
/// #1164), refusing direction, over **every join form that can leave a name
/// in `owned_buffers` while its binding joins back as `Maybe`**.
///
/// `owned_buffers` joins as a *union* and the binding joins on the
/// `Definitely`/`Maybe`/unbound lattice, so an admission that consults only
/// the former accepts a buffer that exists on one path and not another.
/// `pycc check` reported success for `if c: a = ndarray(4)` / `return a`, and
/// codegen then loaded the null-initialized slot and handed the host an
/// internal `SystemError` where the definite-assignment contract in
/// `docs/TYPE_SYSTEM.md` owes a `T0041`.
///
/// The table is the enumeration, not an example: `if` without `else`, an
/// `if`/`else` that binds on one arm only, a `while` body, a `ForRange` body,
/// a `match` case, and a `try` body are every construct whose join can
/// produce that disagreement. `ForList` is the same join helper as
/// `ForRange` (both route through `join_loop_body`) and is covered at the
/// public-CLI level in `tests/issue_1164_memoryview_egress.rs`. A `for`
/// **loop variable** is provably not a member: binding it runs the
/// rebind-over-owned-buffer rule, which drops the name from `owned_buffers`
/// before the loop join is taken, so the name can never be both owned and
/// `Maybe` -- `a = ndarray(4)` followed by `for a in range(n)` is the
/// ordinary `T0023` for the conflicting rebinding, asserted below.
///
/// The diagnostic asserted is `T0041` specifically, not merely "an error":
/// falling through to the ordinary path must reach the possibly-unbound read,
/// not a type mismatch and not the owned-buffer `C0001`.
#[test]
fn a_possibly_unbound_owned_buffer_is_refused_at_the_egress_return() {
    let alloc = || vec![alloc_four("ndarray")];
    let forms: Vec<(&str, HirStmt)> = vec![
        (
            "if without else",
            HirStmt::If {
                test: HirExpr::Name("c".to_string()),
                body: alloc(),
                orelse: Vec::new(),
            },
        ),
        (
            "if/else binding one arm",
            HirStmt::If {
                test: HirExpr::Name("c".to_string()),
                body: alloc(),
                orelse: vec![HirStmt::Assign {
                    target: "other".to_string(),
                    value: HirExpr::IntLiteral(1),
                }],
            },
        ),
        (
            "while body",
            HirStmt::While {
                test: HirExpr::Name("c".to_string()),
                body: alloc(),
            },
        ),
        (
            "for-range body",
            HirStmt::ForRange {
                var: "i".to_string(),
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(2),
                step: HirExpr::IntLiteral(1),
                body: alloc(),
            },
        ),
        ("match case", match_bool(alloc(), Vec::new())),
        ("try body", try_except(alloc(), Vec::new())),
    ];
    for (label, form) in forms {
        let err = check(&egress_after(vec![form])).unwrap_err();
        assert_eq!(err.code, "T0041", "{label}: {}", err.message);
        assert!(
            err.message.contains("may not be bound on every path"),
            "{label}: {}",
            err.message
        );
    }

    // The provably-impossible member, pinned rather than argued: a `for`
    // target that displaces an owned name is not owned after the rebinding,
    // so no join can hand the egress an owned-and-`Maybe` name this way.
    let rebound = egress_after(vec![
        alloc_four("ndarray"),
        HirStmt::ForRange {
            var: "a".to_string(),
            start: HirExpr::IntLiteral(0),
            stop: HirExpr::IntLiteral(2),
            step: HirExpr::IntLiteral(1),
            body: Vec::new(),
        },
    ]);
    let err = check(&rebound).unwrap_err();
    assert_eq!(err.code, "T0023", "{}", err.message);
}

/// The regression direction of the test above: on every one of those join
/// forms, a buffer that *is* definitely assigned is still returned.
///
/// The narrowing must cost the admission nothing it had. Each arm binds the
/// owned name on every path the join can take -- both `if` arms, both `match`
/// cases, and, for the two loop forms, before the loop as well as inside it,
/// which is what leaves the pre-existing `Definitely` binding in place across
/// the join.
///
/// A `try` body has no admitted twin, and that is a property of the language
/// rather than of this admission: the `try` join reports every body binding
/// back as `Maybe` whatever the handlers do, because a raise can interrupt
/// the body at any point. The last arm pins that the buffer program and the
/// equivalent `int` program get the same answer, so the refusal above is the
/// definite-assignment contract and not a buffer-specific one.
#[test]
fn a_definitely_assigned_owned_buffer_is_still_admitted_on_every_join_form() {
    let alloc = || vec![alloc_four("ndarray")];
    let admitted: Vec<(&str, Vec<HirStmt>)> = vec![
        (
            "if/else binding both arms",
            vec![HirStmt::If {
                test: HirExpr::Name("c".to_string()),
                body: alloc(),
                orelse: vec![alloc_four("NDArray")],
            }],
        ),
        (
            "while body over a pre-bound name",
            vec![
                alloc_four("ndarray"),
                HirStmt::While {
                    test: HirExpr::Name("c".to_string()),
                    body: alloc(),
                },
            ],
        ),
        (
            "for-range body over a pre-bound name",
            vec![
                alloc_four("ndarray"),
                HirStmt::ForRange {
                    var: "i".to_string(),
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(2),
                    step: HirExpr::IntLiteral(1),
                    body: alloc(),
                },
            ],
        ),
        (
            "match binding every case",
            vec![match_bool(alloc(), vec![alloc_four("NDArray")])],
        ),
        (
            "unconditional rebinding after a one-armed join",
            vec![
                HirStmt::If {
                    test: HirExpr::Name("c".to_string()),
                    body: alloc(),
                    orelse: Vec::new(),
                },
                alloc_four("NDArray"),
            ],
        ),
    ];
    for (label, body) in admitted {
        assert!(
            check(&egress_after(body)).is_ok(),
            "{label} should still be admitted"
        );
    }

    // The `try` body's absence from that list is the language's answer, not
    // this admission's: the same program with an `int` is refused too.
    let scalar = func(
        vec![("c".to_string(), Ty::Bool)],
        Ty::Int,
        vec![
            try_except(
                vec![HirStmt::Assign {
                    target: "a".to_string(),
                    value: HirExpr::IntLiteral(1),
                }],
                vec![HirStmt::Assign {
                    target: "a".to_string(),
                    value: HirExpr::IntLiteral(2),
                }],
            ),
            HirStmt::Return(Some(HirExpr::Name("a".to_string()))),
        ],
    );
    assert_eq!(check(&scalar).unwrap_err().code, "T0041");
}

/// The negative twin of `the_solver_admits_an_owned_buffer_return`: with the
/// unannotated helper in the module that routes it through the *constraint
/// solver*, a possibly-unbound owned buffer is still refused with `T0041`.
///
/// This is a walker-attribution pin rather than a discriminating one, and the
/// distinction is worth stating because the module is deliberately the solver
/// module. The solver's own `!maybe_bindings` conjunct is **not**
/// independently observable: `collect_expr_constraints`'s `Name` arm tests
/// `maybe_bindings` before it reaches `bindings`, so a maybe-bound operand
/// answers `Ok(None)` and the fall-through unifies nothing either way. What
/// this test pins is that routing a program through the solver cannot lose
/// the check phase's `T0041` -- the failure mode the egress admission has now
/// produced twice, where a walker's early return swallows a diagnostic the
/// other walker owns.
#[test]
fn the_solver_declines_a_possibly_unbound_owned_buffer_return() {
    let hir = HirModule {
        seeded_builtin_exception_classes: false,
        items: vec![
            HirItem::Function {
                name: "_h".to_string(),
                params: vec![("n".to_string(), Ty::Infer)],
                return_ty: Ty::Infer,
                body: vec![HirStmt::Return(Some(HirExpr::Name("n".to_string())))],
            },
            HirItem::Function {
                name: "g".to_string(),
                params: vec![],
                return_ty: Ty::MemoryView,
                body: vec![
                    HirStmt::If {
                        test: HirExpr::BoolLiteral(true),
                        body: vec![alloc_four("ndarray")],
                        orelse: Vec::new(),
                    },
                    HirStmt::Return(Some(HirExpr::Name("a".to_string()))),
                ],
            },
            HirItem::Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(call(
                    "_h",
                    vec![HirExpr::IntLiteral(4)],
                )))],
            },
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let err = check(&hir).unwrap_err();
    assert_eq!(err.code, "T0041", "{}", err.message);
}
