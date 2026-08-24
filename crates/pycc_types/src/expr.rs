//! Expression type inference: [`infer_expr`] and its `local_names`-aware
//! worker `infer_expr_in`.
//!
//! This is the crate's single largest cohesion boundary -- one recursive
//! walk over every [`HirExpr`] variant, producing the expression's [`Ty`]
//! or the diagnostic that rejects it. It was extracted from
//! [`lib.rs`](crate) per the repository's source-file decomposition rule
//! (AGENTS.md "Keep source files decomposable", tracked by issue #544),
//! as a pure relocation: no inference behavior changed with the move.
//!
//! Statement checking, assignability, and the class/exception/operator
//! helpers this walk calls into all stay where they were -- in
//! [`lib.rs`](crate) and its other submodules -- so this module is the
//! expression seam only.

use crate::binop::numeric_result_type;
use crate::class;
use crate::unop::unary_result_type;
use crate::{
    BindingState, Environment, enum_marker_is_not_a_value, enum_member_attr_type,
    instantiate_generic_call, is_assignable, is_known_callable_builtin, is_local, is_marker_kind,
    lookup_bound_name, marker_is_not_a_value, non_callable_binding, numeric_or_bool_compatible,
    possibly_unbound, std_constant_is_not_callable, std_function_used_as_a_value,
    std_qualified_symbol, std_receiver_name, std_receiver_shadowed, std_scalar_to_ty,
    unbound_local, unsupported_callable_builtin,
};

use pycc_diag::{Diagnostic, Span};
use pycc_hir::Ty;
use pycc_hir::{CmpOpKind as CmpOp, FStringPart, HirExpr};

pub fn infer_expr(env: &Environment, expr: &HirExpr) -> Result<Ty, Diagnostic> {
    infer_expr_in(env, &[], expr)
}

pub(crate) fn infer_expr_in(
    env: &Environment,
    local_names: &[&str],
    expr: &HirExpr,
) -> Result<Ty, Diagnostic> {
    match expr {
        HirExpr::IntLiteral(_) => Ok(Ty::Int),
        HirExpr::FloatLiteral(_) => Ok(Ty::Float),
        HirExpr::BoolLiteral(_) => Ok(Ty::Bool),
        HirExpr::StringLiteral(_) => Ok(Ty::Str),
        HirExpr::FString(parts) => {
            for part in parts {
                if let FStringPart::Interpolation(expr) = part {
                    infer_expr_in(env, local_names, expr)?; // any interpolatable type is allowed; Python str()-coerces at runtime
                }
            }
            Ok(Ty::Str)
        }
        HirExpr::Name(name) => {
            if let Some(symbol) = std_qualified_symbol(name) {
                // Post-review finding: see `std_receiver_shadowed`'s own
                // doc comment -- a real local/parameter named `math`
                // shadows the stdlib module.
                let receiver = std_receiver_name(name);
                if env.lookup(receiver).is_some() || is_local(local_names, receiver) {
                    return Err(std_receiver_shadowed(name));
                }
                return match symbol.kind {
                    pycc_std::StdSymbolKind::Constant { ty } => Ok(std_scalar_to_ty(ty)),
                    pycc_std::StdSymbolKind::Function { .. } => {
                        Err(std_function_used_as_a_value(name))
                    }
                    pycc_std::StdSymbolKind::EnumMarker => {
                        Err(enum_marker_is_not_a_value(name))
                    }
                    pycc_std::StdSymbolKind::ProtocolMarker
                    | pycc_std::StdSymbolKind::AbcMarker
                    | pycc_std::StdSymbolKind::DecoratorMarker
                    | pycc_std::StdSymbolKind::AnnotationMarker => {
                        Err(marker_is_not_a_value(name))
                    }
                };
            }
            // Issue #118 Part 1: three-way distinction -- definitely bound ->
            // ok, maybe bound -> T0041, unbound -> T0021 (local) or "not
            // defined" (global). The stdlib-qualified check above already
            // handled `math.sqrt`-style names.
            match env.binding_state(name) {
                Some(BindingState::Definitely(ty)) => Ok(ty.clone()),
                Some(BindingState::Maybe(_)) => Err(possibly_unbound(name)),
                None => {
                    if is_local(local_names, name) {
                        Err(unbound_local(name))
                    } else {
                        Err(Diagnostic::error(
                            "T0021",
                            format!("name `{name}` is not defined"),
                            Span::new(0, 0), // real span threading through HIR is out of scope for this task -- see Task 15's follow-up note
                        ))
                    }
                }
            }
        }
        HirExpr::UnaryOp { op, operand } => {
            let operand_ty = infer_expr_in(env, local_names, operand)?;
            unary_result_type(*op, operand_ty)
        }
        HirExpr::BinOp { op, left, right } => {
            let left_ty = infer_expr_in(env, local_names, left)?;
            let right_ty = infer_expr_in(env, local_names, right)?;
            numeric_result_type(*op, left_ty, right_ty)
        }
        HirExpr::Compare { op, left, right } => {
            let left_ty = infer_expr_in(env, local_names, left)?;
            let right_ty = infer_expr_in(env, local_names, right)?;
            // #378 (PR-18): `==`/`!=` between same-class dataclass instances
            // is accepted -- the compiler-synthesized `__eq__` method has a
            // known-correct signature `(self, other: SameClass) -> bool`.
            // This is restricted to dataclass classes (not any class with a
            // user-defined `__eq__`) because the MIR rewrite assumes the
            // synthesized signature; a user-defined `__eq__` with wrong
            // arity or return type would reach codegen and panic. Ordering
            // operators (`<`, `<=`, `>`, `>=`) between instances are always
            // rejected with T0021 -- pycc has no `__lt__`/`__le__`/`__gt__`/
            // `__ge__` dispatch. Different-class comparisons also stay T0021.
            if matches!(op, CmpOp::Eq | CmpOp::NotEq)
                && let (Ty::Instance(left_class), Ty::Instance(right_class)) =
                    (&left_ty, &right_ty)
                && left_class == right_class
                && let Some(class_def) = env.lookup_class(left_class)
                && class_def.is_dataclass
            {
                return Ok(Ty::Bool);
            }
            if numeric_or_bool_compatible(left_ty.clone(), right_ty.clone()) {
                Ok(Ty::Bool)
            } else {
                Err(Diagnostic::error(
                    "T0021",
                    format!(
                        "cannot compare `{}` and `{}`",
                        left_ty.name(),
                        right_ty.name()
                    ),
                    Span::new(0, 0),
                ))
            }
        }
        HirExpr::Call { callee, args } => {
            // D-110 (#133): a call target resolves through the active value
            // binding before any builtin or function-registry fallback -- a
            // module `helper = 1` shadows both a same-named `def` and a
            // builtin at every later call site, and every value binding in
            // the current subset is a primitive, so a shadowed target is
            // always non-callable. The gate is deliberately callee-first
            // (before argument inference), uniform with how the local gate
            // below always behaved. Local diagnostics are preserved exactly:
            // a value-bound local reported `non_callable_binding` before this
            // reordering too, and a local without a binding still falls
            // through to `unbound_local`. In pass 3 the environment is the
            // final module environment (D-041), so a body call is rejected
            // when the callee is value-bound anywhere at top level -- D-110
            // records that consequence (it rejects some later-rebind programs
            // CPython's dynamic order would run) as deliberate; source-order
            // visibility questions stay #22's scope.
            if env.lookup(callee).is_some() && !env.def_rebound.contains(callee) {
                return Err(non_callable_binding(callee));
            }
            // Issue #118 Part 1: a maybe-bound callee is not callable -- it
            // may not be bound on every path reaching this call. Reject with
            // T0041 before the `is_local` unbound check below, since a
            // maybe-bound local is "possibly unbound," not "never bound."
            if matches!(env.binding_state(callee), Some(BindingState::Maybe(_))) {
                return Err(possibly_unbound(callee));
            }
            if is_local(local_names, callee) {
                return Err(unbound_local(callee));
            }
            // #435: `isinstance`/`issubclass` are compile-time-evaluated
            // builtins. They must be intercepted BEFORE the generic arg
            // inference loop below, because the class argument (args[1] for
            // isinstance, both args for issubclass) is a class name or tuple
            // of class names — not a value expression. Inferring a bare class
            // name as a regular expression would fail with "name not defined"
            // (class names are registered in `env.classes`, not
            // `env.bindings`). The object argument (isinstance's args[0]) IS
            // inferred normally.
            // A user-defined function named `isinstance`/`issubclass` takes
            // priority over the builtin (same pattern as `float` — see the
            // `a_user_defined_float_function_takes_priority_over_the_builtin`
            // test and its identical guard in the constraint solver).
            if callee == "isinstance" && !env.lookup_function(callee).is_some() {
                return class::check_isinstance(env, local_names, args);
            }
            if callee == "issubclass" && !env.lookup_function(callee).is_some() {
                return class::check_issubclass(env, args);
            }
            // For a callee that survives D-110's binding gate above, preserve
            // the established diagnostic order by inferring every
            // argument before validating arity or compatibility. Most Python
            // calls are small, so keep up to four inferred types on the stack
            // and reserve a heap vector only for wider calls.
            const INLINE_ARG_TYPES: usize = 4;
            let mut inline_arg_tys = [const { Ty::Infer }; INLINE_ARG_TYPES];
            let heap_arg_tys;
            let arg_tys: &[Ty] = if args.len() <= INLINE_ARG_TYPES {
                for (slot, arg) in inline_arg_tys.iter_mut().zip(args) {
                    *slot = infer_expr_in(env, local_names, arg)?;
                }
                &inline_arg_tys[..args.len()]
            } else {
                heap_arg_tys = args
                    .iter()
                    .map(|arg| infer_expr_in(env, local_names, arg))
                    .collect::<Result<Vec<_>, _>>()?;
                &heap_arg_tys
            };
            if callee == "print" {
                return Ok(Ty::None); // print's own signature isn't user-declarable in v0.1
            }
            if callee == "len" {
                // D-105 point 3: `len(lst)` is a hand-recognized builtin
                // call, same as `print` above -- not a user-declarable
                // signature. Generic over any scalar element type (`T0034`
                // already gates non-`int` lists further upstream, at the
                // point a list literal is constructed), reusing T0033 for
                // both failure shapes (wrong arity, non-list argument) --
                // the same "value does not support list operations" shape
                // already established for `ForList`/`Subscript`/`ListAppend`.
                // PR-11 Task 3 (D-123): also accepts `Ty::Dict` (gated the
                // same way by `T0036`), so `len(d)` type-checks too. PR-11
                // Task 7 (D-123): also accepts `Ty::Set` (gated the same way
                // by `T0038`), so `len(s)` type-checks too.
                if arg_tys.len() != 1 {
                    return Err(Diagnostic::error(
                        "T0033",
                        format!("`len` expects exactly 1 argument, got {}", arg_tys.len()),
                        Span::new(0, 0),
                    ).with_help("pass exactly 1 argument"));
                }
                if !matches!(arg_tys[0], Ty::List(_) | Ty::Dict(_) | Ty::Set(_)) {
                    return Err(Diagnostic::error(
                        "T0033",
                        format!(
                            "`len` expects a `list[T]`, `dict[K, V]`, or `set[T]` argument, got `{}`",
                            arg_tys[0].name()
                        ),
                        Span::new(0, 0),
                    ).with_help("pass a `list[T]`, `dict[K, V]`, or `set[T]` value"));
                }
                return Ok(Ty::Int);
            }
            if let Some(symbol) = std_qualified_symbol(callee) {
                // Post-review finding: see `std_receiver_shadowed`'s own
                // doc comment -- a real local/parameter named `math`
                // shadows the stdlib module.
                let receiver = std_receiver_name(callee);
                if env.lookup(receiver).is_some() || is_local(local_names, receiver) {
                    return Err(std_receiver_shadowed(callee));
                }
                let pycc_std::StdSymbolKind::Function {
                    arg_tys: expected_arg_tys,
                    ret_ty,
                } = symbol.kind
                else {
                    return Err(if is_marker_kind(symbol.kind) {
                        if matches!(symbol.kind, pycc_std::StdSymbolKind::EnumMarker) {
                            enum_marker_is_not_a_value(callee)
                        } else {
                            marker_is_not_a_value(callee)
                        }
                    } else {
                        std_constant_is_not_callable(callee)
                    });
                };
                if arg_tys.len() != expected_arg_tys.len() {
                    return Err(Diagnostic::error(
                        "T0021",
                        format!(
                            "`{callee}` expects {} argument(s), got {}",
                            expected_arg_tys.len(),
                            arg_tys.len()
                        ),
                        Span::new(0, 0),
                    ).with_help(format!("pass exactly {} argument(s)", expected_arg_tys.len())));
                }
                for (arg_ty, expected) in arg_tys.iter().zip(expected_arg_tys) {
                    if *arg_ty != std_scalar_to_ty(*expected) {
                        return Err(Diagnostic::error(
                            "T0021",
                            format!(
                                "`{callee}` expects `{}`, got `{}`",
                                std_scalar_to_ty(*expected).name(),
                                arg_ty.name()
                            ),
                            Span::new(0, 0),
                        ).with_help(format!("pass a `{}` value", std_scalar_to_ty(*expected).name())));
                    }
                }
                return Ok(std_scalar_to_ty(ret_ty));
            }
            if callee == "float" && env.lookup_function(callee).is_none() {
                // D-086's own remedy for the int-to-float boundary: a hand-recognized
                // builtin, same as `len` above, not a user-declarable signature --
                // except, unlike `print`/`len`, a program can predate this builtin's
                // introduction with its own `def float(...)`. Reviewer finding
                // (post-merge review): unlike `print`/`len`, which have been
                // hand-recognized since before this compiler could compile
                // user-declared functions at all, `float` was undefined until
                // this issue, so a user-defined `float` was a valid, working
                // program on `main` immediately before this change landed --
                // silently reinterpreting it as this builtin would be a real
                // regression, not an inherited, already-accepted precedent.
                // `env.lookup_function` takes priority; only fall through to the
                // builtin when no such definition exists.
                // Always returns `Ty::Float` regardless of the argument's own type,
                // once that argument is numeric-like -- unlike `len`, there is no
                // homogeneity/element-type question to defer.
                if arg_tys.len() != 1 {
                    return Err(Diagnostic::error(
                        "T0021",
                        format!("`float` expects exactly 1 argument, got {}", arg_tys.len()),
                        Span::new(0, 0),
                    ).with_help("pass exactly 1 argument"));
                }
                if !matches!(arg_tys[0], Ty::Int | Ty::Float | Ty::Bool) {
                    return Err(Diagnostic::error(
                        "T0021",
                        format!(
                            "`float` expects an `int`, `float`, or `bool` argument, got `{}`",
                            arg_tys[0].name()
                        ),
                        Span::new(0, 0),
                    ).with_help("pass an `int`, `float`, or `bool` value"));
                }
                return Ok(Ty::Float);
            }
            // D-154 (Part 1 of #375): `ClassName(args)` (instantiation)
            // reuses this same generic `HirExpr::Call` node -- there is no
            // dedicated HIR shape for it (`pycc_hir::class`'s own doc
            // comment) -- so it is resolved here, checked before the
            // ordinary ("ClassName" as a plain function) lookup below, the
            // same precedence a generic-function call already gets just
            // above. `pycc_hir::lower_checked` enforces that a class name
            // can never collide with a top-level function, type-alias, or
            // import name in this compiler's flat, single-namespace model
            // -- both directions are rejected with `C0001` at HIR-lowering
            // time, before `env` is ever built (D-068 review finding on
            // #385; `crates/pycc_hir/src/lib.rs`'s `lower_checked`) -- so
            // trying the class table first here is unambiguous, not merely
            // an assumption.
            if env.lookup_class(callee).is_some() {
                return class::resolve_instantiation(env, callee, arg_tys);
            }
            // D-133/D-134: a call to a PEP 695 generic function is resolved
            // through call-site substitution, not through the ordinary
            // `functions` signature -- `env.lookup_function(callee)` would
            // otherwise see a signature still carrying `Ty::Param` and
            // reject every real (concrete) argument as an assignability
            // mismatch. Checked before the ordinary lookup below so this
            // takes precedence for every generic function, including one
            // reached recursively while inferring a nested call's own
            // arguments (e.g. `print(identity(1))`).
            if let Some(generic_func) = env.lookup_generic(callee) {
                return Ok(instantiate_generic_call(generic_func, arg_tys)?.return_ty);
            }
            let Some((param_tys, return_ty)) = env.lookup_function(callee) else {
                // Issue #142: before falling back to T0021 ("call to undefined
                // function"), check whether `callee` is a known Python 3.14
                // callable builtin that this compiler version does not implement
                // (e.g. `ValueError`, `Exception`, `int`, `range`). Such a call
                // is valid Python -- the builtin genuinely exists -- so it is a
                // capability gap (`C0001`), not a name-resolution failure
                // (`T0021`). This check is deliberately *after* the user-defined
                // function lookup, the `print`/`len`/`float` special cases, the
                // stdlib-qualified symbol lookup, the class-instantiation lookup,
                // and the generic-function lookup, so a user `def
                // ValueError(...)` always takes priority over this classification.
                if is_known_callable_builtin(callee) {
                    return Err(unsupported_callable_builtin(callee));
                }
                return Err(Diagnostic::error(
                    "T0021",
                    format!("call to undefined function `{callee}`"),
                    Span::new(0, 0),
                ));
            };
            // Issue #22: in top-level code, a call to a function whose
            // `def` has not been encountered yet in source order is a
            // static error -- CPython raises `NameError` at runtime for
            // the same case. Function bodies are exempt (all functions
            // are marked defined in `child_for_function`) because Python
            // evaluates a function body at call time, by which point all
            // module-level `def`s have typically executed.
            if !env.defined_functions.contains(callee.as_str()) {
                return Err(Diagnostic::error(
                    "T0021",
                    format!(
                        "cannot call function `{callee}` before its definition \
                         (NameError in CPython: name '{callee}' is not defined)"
                    ),
                    Span::new(0, 0),
                ));
            }
            if arg_tys.len() != param_tys.len() {
                return Err(Diagnostic::error(
                    "T0021",
                    format!(
                        "`{callee}` expects {} argument(s), got {}",
                        param_tys.len(),
                        arg_tys.len()
                    ),
                    Span::new(0, 0),
                ).with_help(format!("pass exactly {} argument(s)", param_tys.len())));
            }
            for (i, (arg_ty, param_ty)) in arg_tys.iter().zip(param_tys.iter()).enumerate() {
                if !class::is_assignable_env(env, arg_ty, param_ty) {
                    // #380 (PR-20): if the mismatch involves a protocol,
                    // produce a detailed T0046 conformance error.
                    let diag = if matches!(param_ty, Ty::Protocol(_)) || matches!(arg_ty, Ty::Protocol(_)) {
                        class::assignable_error(env, arg_ty, param_ty)
                    } else {
                        Diagnostic::error(
                            "T0021",
                            format!(
                                "argument {} of `{callee}` expects `{}`, got `{}`",
                                i + 1,
                                param_ty.name(),
                                arg_ty.name()
                            ),
                            Span::new(0, 0),
                        ).with_help(format!("pass a `{}` value", param_ty.name()))
                    };
                    return Err(diag);
                }
            }
            Ok(return_ty.clone())
        }
        HirExpr::ListLiteral(elements) => {
            // D-105: an empty list literal has no element to infer a type
            // from, and v0.2 has no `list[T]` annotation syntax to recover
            // it from instead -- reject plainly rather than letting
            // `Ty::Infer` leak into codegen. Reuses T0021 (an unconstrained
            // variable with no way to determine its type), not a new code
            // -- this is that same failure shape, not a distinct one.
            if elements.is_empty() {
                return Err(Diagnostic::error(
                    "T0021",
                    "an empty list literal's element type cannot be inferred without an annotation (list[T] annotations are not supported yet, D-105)".to_string(),
                    Span::new(0, 0),
                ));
            }
            let mut elem_ty: Option<Ty> = None;
            for element in elements {
                let this_ty = infer_expr_in(env, local_names, element)?;
                match &elem_ty {
                    None => elem_ty = Some(this_ty),
                    Some(expected) if *expected == this_ty => {}
                    Some(expected) => {
                        // Exact `Ty` equality, not `is_assignable`'s
                        // bool-is-an-int-subtype rule used elsewhere in this
                        // file -- D-105 requires every element to share the
                        // *exact same* `Ty`, so `[1, True]` is T0032 even
                        // though a bare `bool` is assignable to `int`.
                        return Err(Diagnostic::error(
                            "T0032",
                            format!(
                                "list element type mismatch: expected {} (from the first element), found {}",
                                expected.name(),
                                this_ty.name()
                            ),
                            Span::new(0, 0),
                        ).with_help(format!("use a `{}` value here", expected.name())));
                    }
                }
            }
            let elem_ty = elem_ty.expect("checked non-empty above");
            // This is the one place a list literal's element type becomes
            // known with a real source construct behind it (D-105's
            // Consequences) -- deliberately placed here rather than as a
            // separate pre-codegen pass. Everything above this gate (the
            // homogeneity check) is fully generic over any scalar `Ty`; a
            // future PR widening codegen to e.g. `list[str]` only has to
            // relax this one check.
            if elem_ty != Ty::Int {
                return Err(Diagnostic::error(
                    "T0034",
                    format!(
                        "list[{}] is not compiled yet (D-105) -- only list[int] is",
                        elem_ty.name()
                    ),
                    Span::new(0, 0),
                ));
            }
            Ok(Ty::List(Box::new(elem_ty)))
        }
        // PR-11 Task 3 (D-123): mirrors `ListLiteral`'s own homogeneity
        // check above, extended to a key/value pair, plus a `dict[str,
        // int]`-only gate mirroring `ListLiteral`'s own `T0034` gate
        // (D-122: "exactly one combination gets real codegen").
        HirExpr::DictLiteral(pairs) => {
            let Some((first_key, first_value)) = pairs.first() else {
                return Err(Diagnostic::error(
                    "T0021",
                    "an empty dict literal's key/value types cannot be inferred without an annotation (dict[K, V] annotations are not supported yet)".to_string(),
                    Span::new(0, 0),
                ));
            };
            let key_ty = infer_expr_in(env, local_names, first_key)?;
            let val_ty = infer_expr_in(env, local_names, first_value)?;
            for (key, value) in &pairs[1..] {
                let this_key_ty = infer_expr_in(env, local_names, key)?;
                let this_val_ty = infer_expr_in(env, local_names, value)?;
                // Exact `Ty` equality on both key and value, not
                // `is_assignable`'s bool-is-an-int-subtype rule -- same
                // reasoning as `ListLiteral`'s own homogeneity check.
                if this_key_ty != key_ty || this_val_ty != val_ty {
                    return Err(Diagnostic::error(
                        "T0035",
                        format!(
                            "dict entry type mismatch: expected {}: {} (from the first pair), found {}: {}",
                            key_ty.name(),
                            val_ty.name(),
                            this_key_ty.name(),
                            this_val_ty.name(),
                        ),
                        Span::new(0, 0),
                    ).with_help(format!("use a `{}` key and `{}` value here", key_ty.name(), val_ty.name())));
                }
            }
            let dict_ty = Ty::Dict(Box::new((key_ty, val_ty)));
            if dict_ty != Ty::Dict(Box::new((Ty::Str, Ty::Int))) {
                return Err(Diagnostic::error(
                    "T0036",
                    format!(
                        "{} is not compiled yet (D-122) -- only dict[str, int] is",
                        dict_ty.name()
                    ),
                    Span::new(0, 0),
                ));
            }
            Ok(dict_ty)
        }
        // PR-11 Task 7 (D-123): mirrors `ListLiteral`'s own homogeneity
        // check above, for a single-element-type container (no key/value
        // pair), plus a `set[int]`-only gate mirroring `ListLiteral`'s own
        // `T0034` gate (D-122: "exactly one combination gets real codegen").
        // Unlike `DictLiteral` above, the empty-literal branch below is
        // unreachable from any real Python source: `{}` always parses as an
        // empty *dict* (Python has no empty-set literal spelling at all --
        // `set()` is a call, not a literal), so this only fires for a
        // hand-built `HirExpr::SetLiteral(vec![])` (e.g. the crate root's
        // own unit tests, in `tests.rs`).
        HirExpr::SetLiteral(elements) => {
            let Some(first) = elements.first() else {
                return Err(Diagnostic::error(
                    "T0021",
                    "an empty set literal's element type cannot be inferred without an annotation (set[T] annotations are not supported yet)".to_string(),
                    Span::new(0, 0),
                ));
            };
            let elem_ty = infer_expr_in(env, local_names, first)?;
            for element in &elements[1..] {
                let this_ty = infer_expr_in(env, local_names, element)?;
                // Exact `Ty` equality, not `is_assignable`'s
                // bool-is-an-int-subtype rule -- same reasoning as
                // `ListLiteral`'s own homogeneity check.
                if this_ty != elem_ty {
                    return Err(Diagnostic::error(
                        "T0037",
                        format!(
                            "set element type mismatch: expected {} (from the first element), found {}",
                            elem_ty.name(),
                            this_ty.name(),
                        ),
                        Span::new(0, 0),
                    ).with_help(format!("use a `{}` value here", elem_ty.name())));
                }
            }
            let set_ty = Ty::Set(Box::new(elem_ty));
            if set_ty != Ty::Set(Box::new(Ty::Int)) {
                return Err(Diagnostic::error(
                    "T0038",
                    format!(
                        "{} is not compiled yet (D-122) -- only set[int] is",
                        set_ty.name()
                    ),
                    Span::new(0, 0),
                ));
            }
            Ok(set_ty)
        }
        // PR-11b Task 3 (D-116): unlike `ListLiteral`/`DictLiteral`/
        // `SetLiteral`'s homogeneity checks, this arm allows *any* mix of
        // accepted element types -- heterogeneity is tuple's own defining
        // feature. The gate is per-element type membership (int/bool/float
        // only), not agreement with a first element's type.
        HirExpr::TupleLiteral(elements) => {
            if elements.is_empty() {
                return Err(Diagnostic::error(
                    "T0021",
                    "an empty tuple literal's element types cannot be inferred without an annotation (tuple[...] annotations are not supported yet)".to_string(),
                    Span::new(0, 0),
                ));
            }
            let mut elem_tys = Vec::with_capacity(elements.len());
            for element in elements {
                let this_ty = infer_expr_in(env, local_names, element)?;
                if !matches!(this_ty, Ty::Int | Ty::Bool | Ty::Float) {
                    return Err(Diagnostic::error(
                        "T0039",
                        format!(
                            "tuple element type `{}` is not compiled yet (D-116) -- only int/bool/float elements are",
                            this_ty.name()
                        ),
                        Span::new(0, 0),
                    ));
                }
                elem_tys.push(this_ty);
            }
            Ok(Ty::Tuple(Box::new(elem_tys)))
        }
        HirExpr::Subscript { base, index } => {
            // PEP 560 (#610): `C[x]` where `C` is a bare class name is
            // `C.__class_getitem__(x)`, not a container index. The base is
            // `HirExpr::Name` referring to a registered class, exactly as in
            // the `MethodCall` arm's own `ClassName.static_method(args)`
            // interception -- and, like that arm, it must run before the
            // ordinary base inference, which has no type for a bare class
            // name and would reject it as an undefined name.
            //
            // Unlike that arm, this one also requires the name to be unbound
            // as a value: `class C: ...` followed by `C = [1, 2, 3]` is
            // accepted (only *type aliases* collide with a class name, see
            // `pycc_hir`'s own collision check), and `C[0]` must then read
            // the list element, not dispatch a class hook. `pycc_mir`'s own
            // `Subscript` arm applies the identical guard, so both crates
            // agree on which of the two `C[0]` means.
            //
            // A class that defines no `__class_getitem__` anywhere in its
            // MRO is rejected by `resolve_static_or_class_method_call`'s own
            // unknown-member diagnostic, which names the class and the
            // missing hook -- CPython raises `TypeError: type 'C' is not
            // subscriptable` for the same program.
            if let HirExpr::Name(class_name) = base.as_ref()
                && env.binding_state(class_name).is_none()
                && !is_local(local_names, class_name)
                && env.lookup_class(class_name).is_some()
            {
                let index_ty = infer_expr_in(env, local_names, index)?;
                return class::resolve_static_or_class_method_call(
                    env,
                    class_name,
                    "__class_getitem__",
                    &[index_ty],
                );
            }
            let base_ty = infer_expr_in(env, local_names, base)?;
            let index_ty = infer_expr_in(env, local_names, index)?;
            match base_ty {
                Ty::List(elem_ty) => {
                    // Reuses T0021 (an unconstrained/conflicting-constraint
                    // shape), not a new code -- a non-int-compatible index is
                    // that same "operand type mismatch" failure, not a
                    // distinct one.
                    //
                    // Uses `is_assignable`, not exact `Ty` equality: D-086
                    // already established that `bool` is accepted wherever
                    // `int` is expected at an operand boundary (mirroring
                    // `is_assignable`'s own existing param/assignment rule),
                    // and indexing is exactly that kind of boundary --
                    // `xs[True]` is ordinary, CPython-valid Python (`bool` is
                    // an `int` subtype, PEP 285), not a type error.
                    // `pycc_codegen`'s `to_numeric_encoded_int` already has a
                    // `Scalar::Bool` arm reached unconditionally by every
                    // subscript index, so this was a pure over-rejection in
                    // the type checker, not a missing codegen capability.
                    if !is_assignable(index_ty.clone(), Ty::Int) {
                        return Err(Diagnostic::error(
                            "T0021",
                            format!("list index must be `int`, found `{}`", index_ty.name()),
                            Span::new(0, 0),
                        ).with_help("use an `int` value"));
                    }
                    Ok(*elem_ty)
                }
                // PR-11 Task 3 (D-123): `d[k]` read. Uses exact `Ty`
                // equality on the key, not `is_assignable` -- every
                // `Ty::Dict` value that survives `DictLiteral`'s own
                // `T0036` gate above has key type exactly `Ty::Str` (no
                // other combination reaches codegen, and no other source
                // construct can produce a `Ty::Dict` value at all), so
                // there is no bool/int-style widening question here, unlike
                // the list-index case above.
                Ty::Dict(kv) => {
                    let (key_ty, val_ty) = *kv;
                    if index_ty != key_ty {
                        return Err(Diagnostic::error(
                            "T0021",
                            format!(
                                "dict key type mismatch: expected `{}`, found `{}`",
                                key_ty.name(),
                                index_ty.name()
                            ),
                            Span::new(0, 0),
                        ).with_help(format!("use a `{}` value here", key_ty.name())));
                    }
                    Ok(val_ty)
                }
                // PR-11b Task 3 (D-116): `t[k]` requires `k` to be a
                // literal, non-negative, in-bounds integer -- not merely
                // `int`-typed, unlike `List`'s case above. A heterogeneous
                // tuple's element type at position `k` is only knowable
                // when `k` is known at compile time, so every failure shape
                // (non-literal, negative, out-of-range) shares one code.
                Ty::Tuple(elems) => {
                    let HirExpr::IntLiteral(literal_index) = index.as_ref() else {
                        return Err(Diagnostic::error(
                            "T0040",
                            "tuple index must be a non-negative literal integer within range"
                                .to_string(),
                            Span::new(0, 0),
                        ).with_help("use a literal, non-negative integer index within range"));
                    };
                    let Ok(literal_index) = usize::try_from(*literal_index) else {
                        return Err(Diagnostic::error(
                            "T0040",
                            "tuple index must be a non-negative literal integer within range"
                                .to_string(),
                            Span::new(0, 0),
                        ).with_help("use a literal, non-negative integer index within range"));
                    };
                    let Some(elem_ty) = elems.get(literal_index) else {
                        return Err(Diagnostic::error(
                            "T0040",
                            "tuple index must be a non-negative literal integer within range"
                                .to_string(),
                            Span::new(0, 0),
                        ).with_help("use a literal, non-negative integer index within range"));
                    };
                    Ok(elem_ty.clone())
                }
                // PR-11 Task 7 (D-123): `Ty::Set` deliberately has no
                // explicit arm here and falls through to this rejection --
                // real Python sets are not subscriptable either (`s[0]`
                // raises `TypeError` in CPython too), so this is not a v0.2
                // scope cut needing its own arm, it is the semantically
                // correct behavior for every other base type as well.
                other => Err(Diagnostic::error(
                    "T0033",
                    format!("`{}` does not support indexing", other.name()),
                    Span::new(0, 0),
                )),
            }
        }
        // PR-12 Task 7 (D-118): `base[start:stop:step]` read. Only
        // `list[int]` ships slicing in v0.2 -- every other base (including
        // `Ty::Dict`/`Ty::Set`, which real CPython also rejects for `[i:j]`,
        // and `Ty::Tuple`, an explicit deferral rather than a "never
        // supported" case) reuses the same `T0033` code `Subscript`'s own
        // `other` fallthrough above already established for non-indexable
        // bases, not a new one. A `list[T]` with `T != Ty::Int` reuses
        // `ListLiteral`'s own `T0034` ("only list[int] is compiled") gate.
        // Diagnostic order is deliberately base-type (`T0033`) before
        // element-type (`T0034`) before any bound's own type (`T0021`),
        // mirroring this file's existing "callee/base-type errors before
        // argument errors" convention (D-110's callee-first precedent,
        // applied here to base-type-before-bound-type) -- pinned by
        // `slicing_reports_the_base_type_error_before_any_bound_error` and
        // `slicing_reports_the_element_type_error_before_any_bound_error`
        // in `tests.rs`.
        HirExpr::Slice {
            base,
            start,
            stop,
            step,
        } => {
            let base_ty = infer_expr_in(env, local_names, base)?;
            let Ty::List(elem_ty) = &base_ty else {
                return Err(Diagnostic::error(
                    "T0033",
                    format!(
                        "`{}` does not support slicing (only list[int] does)",
                        base_ty.name()
                    ),
                    Span::new(0, 0),
                ));
            };
            if **elem_ty != Ty::Int {
                return Err(Diagnostic::error(
                    "T0034",
                    format!(
                        "list codegen only supports `list[int]` in v0.2, cannot slice `list[{}]`",
                        elem_ty.name()
                    ),
                    Span::new(0, 0),
                ));
            }
            // Each bound is independently optional (`xs[:]`, `xs[1:]`,
            // `xs[:3]`, `xs[::2]` all parse) and, when present, an
            // arbitrary runtime `int`-typed expression -- not a literal-only
            // check -- so `is_assignable` (not exact `Ty` equality) applies
            // here, same as `Subscript`'s own index check above: `xs[True:]`
            // is ordinary, CPython-valid Python (`bool` is an `int`
            // subtype, PEP 285), not a type error. `step`'s runtime
            // positivity is deliberately not validated here -- it can't be,
            // for a non-literal runtime expression, at compile time; that
            // check is a later task's job (D-118).
            for (label, bound) in [("start", start), ("stop", stop), ("step", step)] {
                if let Some(bound) = bound {
                    let bound_ty = infer_expr_in(env, local_names, bound)?;
                    if !is_assignable(bound_ty.clone(), Ty::Int) {
                        return Err(Diagnostic::error(
                            "T0021",
                            format!("slice {label} must be `int`, got `{}`", bound_ty.name()),
                            Span::new(0, 0),
                        ).with_help("use an `int` value"));
                    }
                }
            }
            Ok(base_ty.clone())
        }
        HirExpr::ListAppend { list, value } => {
            let list_ty = lookup_bound_name(env, local_names, list)?;
            let Ty::List(elem_ty) = &list_ty else {
                return Err(Diagnostic::error(
                    "T0033",
                    format!("`{}` does not support `.append()`", list_ty.name()),
                    Span::new(0, 0),
                ));
            };
            let value_ty = infer_expr_in(env, local_names, value)?;
            // Uses `is_assignable`, not exact `Ty` equality -- matching the
            // `Subscript` index check above (D-086): `x = [1]; x.append(True)`
            // is ordinary, CPython-valid Python (`bool` is an `int` subtype),
            // and `pycc_codegen`'s `MirExpr::ListAppend` arm already routes
            // the appended value through `to_encoded_int`, preserving a
            // `Scalar::Bool` marker while validating the int-compatible
            // payload unconditionally, so there is no missing codegen
            // capability here either. This is NOT the same question as `ListLiteral`'s
            // own homogeneity check above (`[1, True]`'s element type is
            // genuinely ambiguous to infer -- there is no already-known
            // `elem_ty` to check against); `.append()` on an *already-typed*
            // `list[int]` has no such ambiguity, so the looser
            // `is_assignable` rule applies here, not there. Reuses T0021 (a
            // call-site/assignment constraint mismatch), not a new code.
            if !is_assignable(value_ty.clone(), (**elem_ty).clone()) {
                return Err(Diagnostic::error(
                    "T0021",
                    format!(
                        "cannot append `{}` to a list of `{}`",
                        value_ty.name(),
                        elem_ty.name()
                    ),
                    Span::new(0, 0),
                ).with_help(format!("change the value to `{}` (the expected/declared type), or the declaration/annotation to `{}` (the actual type)", elem_ty.name(), value_ty.name())));
            }
            Ok(Ty::None)
        }
        // PR-12 Task 10 (D-119): `list.pop()`. Unlike `ListAppend`, this
        // arm's result is the list's own element type, not `Ty::None` --
        // `.pop()` is meant to be used for its value.
        HirExpr::ListPop { list } => {
            let list_ty = lookup_bound_name(env, local_names, list)?;
            let Ty::List(elem_ty) = &list_ty else {
                return Err(Diagnostic::error(
                    "T0033",
                    format!("`{}` does not support `.pop()`", list_ty.name()),
                    Span::new(0, 0),
                ));
            };
            Ok((**elem_ty).clone())
        }
        // PR-12 Task 10 (D-119): `dict.get(key, default)`. The key check
        // uses exact `Ty` equality, not `is_assignable` -- same reasoning as
        // `Subscript`'s own `Ty::Dict` read arm and `check_dict_set` above
        // (every `Ty::Dict` value that survives `DictLiteral`'s own `T0036`
        // gate has key type exactly `Ty::Str`, so there is no bool/int-style
        // widening question for a dict *key*, unlike a list index or a
        // dict *value*). The `default` check does use `is_assignable`,
        // mirroring `ListAppend`'s/`check_dict_set`'s own value-position
        // leniency (D-086 `bool`-subtypes-`int`). The overall result is the
        // dict's *value* type, never `Ty::None`, since a missing key still
        // yields the (same-typed) default rather than `None`.
        HirExpr::DictGetOrDefault { dict, key, default } => {
            let dict_ty = lookup_bound_name(env, local_names, dict)?;
            let Ty::Dict(kv) = &dict_ty else {
                return Err(Diagnostic::error(
                    "T0033",
                    format!("`{}` does not support `.get()`", dict_ty.name()),
                    Span::new(0, 0),
                ));
            };
            let key_ty = infer_expr_in(env, local_names, key)?;
            if key_ty != kv.0 {
                return Err(Diagnostic::error(
                    "T0021",
                    format!(
                        "cannot look up a `{}` key in a dict of `{}` keys",
                        key_ty.name(),
                        kv.0.name()
                    ),
                    Span::new(0, 0),
                ).with_help(format!("change the value to `{}` (the expected/declared type), or the declaration/annotation to `{}` (the actual type)", kv.0.name(), key_ty.name())));
            }
            let default_ty = infer_expr_in(env, local_names, default)?;
            if !is_assignable(default_ty.clone(), kv.1.clone()) {
                return Err(Diagnostic::error(
                    "T0021",
                    format!(
                        "cannot use a `{}` default for a dict of `{}` values",
                        default_ty.name(),
                        kv.1.name()
                    ),
                    Span::new(0, 0),
                ).with_help(format!("change the value to `{}` (the expected/declared type), or the declaration/annotation to `{}` (the actual type)", kv.1.name(), default_ty.name())));
            }
            Ok(kv.1.clone())
        }
        // PR-12 Task 10 (D-119): `set.add(value)`. Mirrors `ListAppend`
        // exactly -- always `Ty::None`, with D-131's ordinary assignment
        // storage available when that result is bound to a name.
        HirExpr::SetAdd { set, value } => {
            let set_ty = lookup_bound_name(env, local_names, set)?;
            let Ty::Set(elem_ty) = &set_ty else {
                return Err(Diagnostic::error(
                    "T0033",
                    format!("`{}` does not support `.add()`", set_ty.name()),
                    Span::new(0, 0),
                ));
            };
            let value_ty = infer_expr_in(env, local_names, value)?;
            if !is_assignable(value_ty.clone(), (**elem_ty).clone()) {
                return Err(Diagnostic::error(
                    "T0021",
                    format!(
                        "cannot add `{}` to a set of `{}`",
                        value_ty.name(),
                        elem_ty.name()
                    ),
                    Span::new(0, 0),
                ).with_help(format!("change the value to `{}` (the expected/declared type), or the declaration/annotation to `{}` (the actual type)", elem_ty.name(), value_ty.name())));
            }
            Ok(Ty::None)
        }
        // D-154 (Part 1 of #375): an instance attribute read/method call --
        // see `class::resolve_attr_get`/`class::resolve_method_call` for the
        // actual resolution (both shared with `check_stmt`'s own
        // `HirStmt::AttrSet` arm, which needs the identical attribute-type
        // lookup for its own assigned-value check).
        HirExpr::AttrGet { base, attr } => {
            // #433: `super().attr` — resolve the attribute starting from
            // the next class in the current class's MRO, not from the
            // current class itself. The `self` instance retains its actual
            // (most-derived) type, so the slot index is still computed from
            // the full MRO's flat layout downstream.
            if matches!(base.as_ref(), HirExpr::Super) {
                return class::resolve_super_attr_get(env, attr);
            }
            // #436: `ClassName.attr` — accessing an attribute on a class
            // name (not an instance) is not supported. A static or class
            // method accessed without calling it (e.g. `C.create` instead
            // of `C.create()`) has no value representation in this
            // compiler's static-dispatch model. Reject with a clear error
            // rather than letting `infer_expr_in` on the class name
            // produce a confusing "name not defined" diagnostic.
            if let HirExpr::Name(class_name) = base.as_ref()
                && let Some(class_def) = env.lookup_class(class_name)
            {
                // #379 (PR-19): `Color.RED` — accessing an enum member by
                // name on the enum class.
                if let Some(ty) = enum_member_attr_type(class_def, class_name, attr) {
                    return Ok(ty);
                }
                return Err(Diagnostic::error(
                    "T0044",
                    format!(
                        "class `{class_name}` has no attribute named `{attr}` -- \
                         accessing a class attribute or method without an instance is \
                         not supported (use `instance.{attr}` or `{class_name}.{attr}()` \
                         for a static/class method)"
                    ),
                    Span::new(0, 0),
                ));
            }
            let base_ty = infer_expr_in(env, local_names, base)?;
            class::resolve_attr_get(env, &base_ty, attr)
        }
        HirExpr::MethodCall { base, method, args } => {
            // #433: `super().method(args)` — resolve the method starting
            // from the next class in the current class's MRO, with `self`
            // (the most-derived instance) as the implicit first argument.
            if matches!(base.as_ref(), HirExpr::Super) {
                let arg_tys = args
                    .iter()
                    .map(|arg| infer_expr_in(env, local_names, arg))
                    .collect::<Result<Vec<_>, _>>()?;
                return class::resolve_super_method_call(env, method, &arg_tys);
            }
            // #436: `ClassName.static_method(args)` or
            // `ClassName.class_method(args)` — a method call on a class
            // name (not an instance). The base is `HirExpr::Name` referring
            // to a registered class. Check the static_methods and
            // class_methods tables before the regular instance-method
            // resolution (which requires a `Ty::Instance` base and would
            // reject a bare class name).
            if let HirExpr::Name(class_name) = base.as_ref()
                && env.lookup_class(class_name).is_some()
                && class::has_static_or_class_method(env, class_name, method)
            {
                let arg_tys = args
                    .iter()
                    .map(|arg| infer_expr_in(env, local_names, arg))
                    .collect::<Result<Vec<_>, _>>()?;
                return class::resolve_static_or_class_method_call(
                    env, class_name, method, &arg_tys,
                );
            }
            let base_ty = infer_expr_in(env, local_names, base)?;
            let arg_tys = args
                .iter()
                .map(|arg| infer_expr_in(env, local_names, arg))
                .collect::<Result<Vec<_>, _>>()?;
            // #436: static and class methods can also be called on an
            // instance. Check the static/class method tables before the
            // regular instance-method resolution.
            if let Ty::Instance(ref class_name) = base_ty
                && class::has_static_or_class_method(env, class_name, method)
            {
                return class::resolve_static_or_class_method_call(
                    env, class_name, method, &arg_tys,
                );
            }
            class::resolve_method_call(env, &base_ty, method, &arg_tys)
        }
        // PEP 695 (#387): `C[type_arg](args)` — a generic class
        // instantiation. The result type is `Ty::Instance(class)` — the
        // type argument is a compile-time scalar substitution, not a
        // runtime value, so it does not affect the result's nominal type
        // (the class instance). The actual monomorphization (substituting
        // `T` with `type_arg` in the class's methods) happens later in
        // `pycc_types`' `instantiate_generic_call` / `monomorphize`
        // pipeline, reusing PR-13's generic-function infrastructure.
        HirExpr::GenericClassInstantiate { class, .. } => {
            // Verify the class exists. Genericity (the class has a type
            // parameter) is checked later during monomorphization's rewrite
            // pass (`rewrite_generic_calls_in_expr`), which rejects a
            // non-generic class used with `C[int](args)` with T0042.
            if !env.classes.contains_key(class) {
                return Err(Diagnostic::error(
                    "T0001",
                    format!("class `{class}` is not defined"),
                    Span::new(0, 0),
                ));
            }
            Ok(Ty::Instance(Box::new(class.to_string())))
        }
        // #433: a bare `HirExpr::Super` should never reach `infer_expr_in`
        // — HIR lowering rejects a standalone `super()` with C0001, and
        // `super().method()`/`super().attr` are handled by the `MethodCall`/
        // `AttrGet` arms below (which special-case a `Super` base before
        // recursing into `infer_expr_in` for it). This arm is a defense-in-
        // depth guard for a hand-built HIR that bypasses `lower_expr`'s own
        // rejection.
        HirExpr::Super => Err(Diagnostic::error(
            "C0001",
            "a bare `super()` expression is not supported — use `super().method()` or `super().attr`".to_string(),
            Span::new(0, 0),
        )),
    }
}
