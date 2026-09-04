//! HIR-to-MIR expression lowering (#546): `lower_expr`, the per-`HirExpr`
//! dispatch that the crate root's lowering walk and every sibling lowering
//! module drive.

use super::class::{
    class_def_of, lower_isinstance, lower_issubclass, mro_attr_count, mro_attrs,
    rewrite_exception_to_message, rewrite_instance_to_repr, self_expr,
};
use super::{
    HirClassDef, InstantiateExpr, MirExpr, MirFStringPart, binop_result_ty, lookup, mro_class_def,
    try_lower_enum_member_attr,
};
use pycc_hir::{BinOpKind, ClassAttrValue, FStringPart, HirExpr, Ty, UnaryOpKind};
use std::collections::HashMap;

pub(super) fn lower_expr(
    expr: &HirExpr,
    scopes: &[HashMap<String, Ty>],
    classes: &HashMap<String, HirClassDef>,
    current_class: Option<&str>,
) -> MirExpr {
    match expr {
        HirExpr::IntLiteral(n) => MirExpr::IntLiteral(*n),
        HirExpr::FloatLiteral(f) => MirExpr::FloatLiteral(*f),
        HirExpr::BoolLiteral(b) => MirExpr::BoolLiteral(*b),
        HirExpr::StringLiteral(s) => MirExpr::StringLiteral(s.clone()),
        HirExpr::NoneLiteral => MirExpr::NoneLiteral,
        // D-136: `math.pi` (a `pycc_hir`-qualified stdlib constant name --
        // real Python identifiers never contain `.`, see
        // `pycc_types::std_qualified_symbol`'s own doc comment for the
        // same invariant) is never bound in `scopes` the way an ordinary
        // assigned variable is, so it needs its own arm here rather than
        // falling into the ordinary `lookup` below, which would panic.
        HirExpr::Name(name) if name == "math.pi" => MirExpr::Name {
            name: name.clone(),
            ty: Ty::Float,
        },
        // Issue #769 (Part 2 of #747): a name read inside a narrowing-
        // eligible branch (`super::narrowed_ty`'s `$narrowed:{name}`
        // sentinel, pushed by `stmt::lower_stmt`'s `HirStmt::If` arm) is
        // wrapped in `OptionalUnwrap` so `.ty()` reports the Optional's
        // inner type for this read alone, without touching the slot's own
        // still-`Optional` declared representation looked up via `lookup`
        // just below. Checked before the plain `Name` arm so a narrowed
        // read never falls through to it.
        HirExpr::Name(name) if super::narrowed_ty(scopes, name).is_some() => {
            let inner = super::narrowed_ty(scopes, name).expect("just matched Some above");
            MirExpr::OptionalUnwrap(
                Box::new(MirExpr::Name {
                    name: name.clone(),
                    ty: lookup(scopes, name),
                }),
                Box::new(inner),
            )
        }
        HirExpr::Name(name) => MirExpr::Name {
            name: name.clone(),
            ty: lookup(scopes, name),
        },
        HirExpr::Call { callee, args } => {
            // #435: `isinstance`/`issubclass` are compile-time-evaluated
            // builtins. They must be intercepted BEFORE the generic arg
            // lowering below, because the class arguments are class names
            // (not value expressions) and would fail to lower as ordinary
            // MIR expressions. The object argument (isinstance's args[0])
            // IS lowered normally to extract its type.
            // A user-defined function named `isinstance`/`issubclass` takes
            // priority over the builtin (same pattern as `float` — the type
            // checker's identical guard ensures a user-defined version is
            // never intercepted here, but the MIR guard is defense-in-depth
            // and mirrors the `float` builtin's own MIR-side check).
            let is_user_defined = scopes
                .iter()
                .any(|s| s.contains_key(&format!("$fn:{callee}")));
            if callee == "isinstance" && !is_user_defined {
                return lower_isinstance(args, scopes, classes, current_class);
            }
            if callee == "issubclass" && !is_user_defined {
                return lower_issubclass(args, classes);
            }
            // #767: `typing.cast(T, value)` is a runtime no-op — it only
            // changes a static checker's view of `value`'s type — so the
            // whole call expression lowers to `value` alone. Eliding it here
            // means no `MirExpr::Call` for `cast` ever reaches codegen, and
            // codegen needs no `cast` case at all. `args[0]` (the target
            // type) is a bare type name, not a value expression, and is
            // deliberately never lowered.
            //
            // Indexing `args[1]` is sound because `pycc_types::class::
            // check_cast` rejects every arity other than 2 with `T0021`, and
            // both `pycc_mir::build` call sites lower an already-type-checked
            // HIR module — the same "the type checker guarantees this shape"
            // invariant the `__init__`-in-MRO panic below relies on. A
            // user-defined `def cast(...)` takes priority over the special
            // case, exactly as for `isinstance`/`issubclass`.
            if callee == "cast" && !is_user_defined {
                return lower_expr(&args[1], scopes, classes, current_class);
            }
            let args: Vec<MirExpr> = args
                .iter()
                .map(|a| {
                    let lowered = lower_expr(a, scopes, classes, current_class);
                    // #378 (PR-18): for `print(instance)`, rewrite an
                    // instance-typed argument to a `__repr__` call so the
                    // codegen's `to_str` receives a `str` scalar. Only
                    // `print` feeds non-str values to `to_str` among call
                    // expressions (f-string interpolations are handled in
                    // the `FString` arm); other calls pass instance
                    // arguments directly as opaque pointers.
                    if callee == "print" {
                        // Part 3A of #541 (#736): try the exception-message
                        // rewrite before the dataclass `__repr__` one, so a
                        // caught exception binding is rendered as its
                        // message rather than falling through to
                        // `rewrite_instance_to_repr` (a no-op for it anyway,
                        // since an exception class is never a dataclass, but
                        // ordering this rewrite first keeps the exception
                        // path independent of that fact).
                        let lowered = rewrite_exception_to_message(&lowered, classes);
                        rewrite_instance_to_repr(&lowered, classes)
                    } else {
                        lowered
                    }
                })
                .collect();
            // D-154 (Part 1 of #375): `ClassName(args)` (instantiation)
            // reuses `HirExpr::Call` -- there is no dedicated HIR shape for
            // it (`pycc_hir::class`'s own doc comment) -- so it is resolved
            // here, before the ordinary `$fn:` lookup below (which has no
            // entry for a bare class name; only its mangled methods are
            // registered). See `MirExpr::Instantiate`'s own doc comment for
            // why this needs a dedicated MIR node rather than folding into
            // `MirExpr::Call` the way `MethodCall` below does.
            if let Some(class_def) = classes.get(callee.as_str()) {
                // #432: resolve `__init__` via the MRO -- a derived class
                // without its own `__init__` inherits the base class's
                // constructor. The MRO is ordered most-derived-first, so
                // the first `__init__` found is the one to call.
                let ctor = class_def
                    .mro
                    .iter()
                    .find_map(|mro_class| {
                        let mro_def = classes.get(mro_class.as_str())?;
                        if mro_def.methods.iter().any(|(mn, _)| mn == "__init__") {
                            Some(format!("{mro_class}.__init__"))
                        } else {
                            None
                        }
                    })
                    .unwrap_or_else(|| {
                        panic!(
                            "pycc_mir: internal error: no `__init__` found in class `{callee}`'s \
                         MRO -- pycc_hir::lower_class should have rejected this before it \
                         reached pycc_mir"
                        )
                    });
                return MirExpr::Instantiate(Box::new(InstantiateExpr {
                    ctor,
                    // #432: allocate slots for all unique attributes across the
                    // MRO, not just this class's own declared attributes.
                    attr_count: mro_attr_count(class_def, classes),
                    args,
                    ty: Ty::Instance(Box::new(callee.clone())),
                }));
            }
            let ty = if callee == "print" {
                Ty::None
            } else if callee == "math.sqrt" {
                // D-136: `math.sqrt` is the other hand-recognized stdlib
                // intrinsic this PR lowers to real codegen -- mirrors
                // `pycc_types`'s own `std_qualified_symbol` dispatch
                // (always `Ty::Float`, this registry's only lowered
                // function's fixed return type; not a general per-registry
                // lookup since `pycc_mir` has no dependency on `pycc_std`
                // and does not need one for this PR's exactly-one-function
                // registry).
                Ty::Float
            } else if callee == "len" {
                // `len` is a hand-recognized builtin, same as `print` above,
                // not a user-declarable `$fn:` signature -- mirrors
                // `pycc_types::collect_expr_constraints`'s own `callee ==
                // "len"` arm (D-105 point 3). Without this branch, `len(lst)`
                // falls to the `lookup` fallback below, finds no registered
                // `$fn:len`, and panics even though `pycc_types` already
                // accepts `len(lst)` as valid, `Ty::Int`-typed.
                Ty::Int
            } else if callee == "float"
                && !scopes
                    .iter()
                    .any(|scope| scope.contains_key(&format!("$fn:{callee}")))
            {
                // Mirrors `pycc_types`' own `callee == "float"` arms (both the
                // public-body and private-helper paths), including their own
                // user-defined-function-takes-priority guard -- see
                // `pycc_types::infer_expr_in`'s comment for why `float` (unlike
                // `len`/`print`) needs this. Always `Ty::Float`.
                Ty::Float
            } else {
                lookup(scopes, &format!("$fn:{callee}"))
            };
            MirExpr::Call {
                callee: callee.clone(),
                args,
                ty,
            }
        }
        // #603 (Part 2 of #573): `-x`/`+x` over a non-literal operand is
        // rewritten into the equivalent binary expression rather than
        // getting a `MirExpr` variant of its own -- pycc's MIR deliberately
        // has no `USub`/`UAdd`/`Invert` unary node (see the #604 comment
        // below for `not x`'s own dedicated `MirExpr::Not` exception to
        // that rule, which also supersedes the "negated `__eq__` call"
        // comparison this comment used to draw to the dataclass `!=`
        // lowering further below -- that lowering now itself uses
        // `MirExpr::Not` rather than illustrating "no unary node exists").
        //
        // The rewrite is representation-sensitive, so it is not a single
        // uniform shape:
        //
        // * `int`/`bool` become `0 - x` / `0 + x`. `pycc_rt`'s `int_sub`
        //   and `int_add` both handle an already-promoted bigint operand
        //   and the smallint boundary, so negation inherits arbitrary
        //   precision for free. `x * -1` would *not*: `int_mul` calls
        //   `require_inline_int` and aborts on a bigint.
        // * `float` becomes `x * -1.0` / `x * 1.0`, a plain LLVM `fmul`.
        //   `0.0 - x` would be wrong for `-0.0` (it yields `+0.0`), and
        //   multiplication is exact for infinities and NaN too.
        //
        // `UAdd` is not folded away, because it is not the identity: on a
        // `bool` operand it crosses into `int` (`+True` is `1`), which the
        // `0 + x` / `x * 1.0` shapes deliver through the same typing rules
        // as `USub`. Any non-numeric operand is already rejected by
        // `pycc_types::unop::unary_result_type` and never reaches here.
        //
        // #604 (Part 3 of #573) adds `not x` and `~x`. `~x` follows the
        // exact same "rewrite into a `BinOp`, no new MIR node" strategy as
        // `USub`/`UAdd`: `~x == -x - 1` for every `int`/`bool` operand (the
        // only operands `pycc_types::unop::unary_result_type` accepts for
        // `Invert`), so it reuses the identical `0 - x` shape and its
        // bigint-precision guarantee, then subtracts a further literal
        // `1` -- both legs going through `int_sub`. `not x` genuinely has
        // no binary-expression equivalent (its truthiness rule spans
        // types `BinOp`'s own typing has no notion of), so it is the one
        // case that *does* need a dedicated `MirExpr::Not` node, built
        // directly on `pycc_codegen`'s existing `truthy` helper (the same
        // one `if`/`while` conditions already call).
        HirExpr::UnaryOp { op, operand } => {
            let operand = lower_expr(operand, scopes, classes, current_class);
            match op {
                UnaryOpKind::USub | UnaryOpKind::UAdd => {
                    let (bin_op, left, right) = if operand.ty() == Ty::Float {
                        let factor = if matches!(op, UnaryOpKind::USub) {
                            -1.0
                        } else {
                            1.0
                        };
                        (BinOpKind::Mul, operand, MirExpr::FloatLiteral(factor))
                    } else {
                        let bin_op = if matches!(op, UnaryOpKind::USub) {
                            BinOpKind::Sub
                        } else {
                            BinOpKind::Add
                        };
                        (bin_op, MirExpr::IntLiteral(0), operand)
                    };
                    let ty = binop_result_ty(bin_op, left.ty(), right.ty());
                    MirExpr::BinOp {
                        op: bin_op,
                        left: Box::new(left),
                        right: Box::new(right),
                        ty,
                    }
                }
                UnaryOpKind::Invert => {
                    let negated_ty = binop_result_ty(BinOpKind::Sub, Ty::Int, operand.ty());
                    let negated = MirExpr::BinOp {
                        op: BinOpKind::Sub,
                        left: Box::new(MirExpr::IntLiteral(0)),
                        right: Box::new(operand),
                        ty: negated_ty.clone(),
                    };
                    let ty = binop_result_ty(BinOpKind::Sub, negated_ty, Ty::Int);
                    MirExpr::BinOp {
                        op: BinOpKind::Sub,
                        left: Box::new(negated),
                        right: Box::new(MirExpr::IntLiteral(1)),
                        ty,
                    }
                }
                UnaryOpKind::Not => MirExpr::Not(Box::new(operand)),
            }
        }
        HirExpr::BinOp { op, left, right } => {
            let left = lower_expr(left, scopes, classes, current_class);
            let right = lower_expr(right, scopes, classes, current_class);
            let ty = binop_result_ty(*op, left.ty(), right.ty());
            MirExpr::BinOp {
                op: *op,
                left: Box::new(left),
                right: Box::new(right),
                ty,
            }
        }
        HirExpr::Compare { op, left, right } => {
            let left_lowered = lower_expr(left, scopes, classes, current_class);
            let right_lowered = lower_expr(right, scopes, classes, current_class);
            // #378 (PR-18): `==`/`!=` between same-class dataclass instances
            // is rewritten to a `MirExpr::Call` to the class's
            // compiler-synthesized `__eq__` method. `!=` is lowered as
            // `__eq__(left, right) != True` (a `MirExpr::Compare` against a
            // `BoolLiteral`, predating #604's `MirExpr::Not`) rather than
            // negating the call result with `MirExpr::Not` directly --
            // `MirExpr::Not` computes truthiness (bool/int/float/str/
            // None/Optional) via `pycc_codegen`'s `truthy` helper, but this
            // call's result is already a `bool`, so an equality compare
            // against `true` is the simpler, equally-correct rewrite. This
            // mirrors how `@property` redirects attribute access to method
            // calls -- a MIR-level rewrite, not a new MIR node. Only `Eq`
            // and `NotEq` are rewritten, and only for dataclass classes
            // (whose synthesized `__eq__` has a known-correct signature);
            // other comparison operators (`<`, `<=`, `>`, `>=`) and
            // non-dataclass classes
            // fall through to the default `MirExpr::Compare` (the type
            // checker rejects them with `T0021` before they reach MIR
            // lowering in normal compilation, but the MIR itself stays
            // semantically correct for defense-in-depth).
            if matches!(op, pycc_hir::CmpOpKind::Eq | pycc_hir::CmpOpKind::NotEq)
                && let (Ty::Instance(left_class), Ty::Instance(right_class)) =
                    (left_lowered.ty(), right_lowered.ty())
                && left_class == right_class
                && let Some(class_def) = classes.get(left_class.as_str())
                && class_def.is_dataclass
            {
                let eq_mangled = class_def.mro.iter().find_map(|mro_class| {
                    // Every class in the MRO was registered when the class
                    // was lowered; using `.expect` (whose panic path lives
                    // in libcore, outside this crate's instrumented regions)
                    // avoids a `?` whose `None` branch is structurally
                    // unreachable and would show up as a permanently
                    // uncovered region under D-014's 100% coverage gate.
                    let mro_def = classes
                        .get(mro_class.as_str())
                        .expect("MRO class must be registered");
                    mro_def
                        .methods
                        .iter()
                        .find(|(mn, _)| mn == "__eq__")
                        .map(|(_, mangled)| mangled.clone())
                });
                // A dataclass always has a synthesized `__eq__` in its
                // MRO (the `is_dataclass` guard above ensures we only
                // enter this block for dataclass classes). Using
                // `.expect` (whose panic path lives in libcore, outside
                // this crate's instrumented regions) avoids an `if let
                // Some` whose `None` branch is structurally unreachable
                // for a dataclass and would show up as a permanently
                // uncovered region under D-014's 100% coverage gate.
                let eq_mangled = eq_mangled.expect("dataclass must have __eq__");
                let eq_call = MirExpr::Call {
                    callee: eq_mangled,
                    args: vec![left_lowered, right_lowered],
                    ty: Ty::Bool,
                };
                // The `matches!` guard above limits entry to
                // `Eq`/`NotEq`, so an if/else (rather than a match
                // with a `_` arm) suffices and avoids an unreachable
                // arm that would be permanently uncovered under
                // D-014's 100% coverage gate.
                if *op == pycc_hir::CmpOpKind::Eq {
                    return eq_call;
                }
                return MirExpr::Compare {
                    op: pycc_hir::CmpOpKind::NotEq,
                    left: Box::new(eq_call),
                    right: Box::new(MirExpr::BoolLiteral(true)),
                    ty: Ty::Bool,
                };
            }
            MirExpr::Compare {
                op: *op,
                left: Box::new(left_lowered),
                right: Box::new(right_lowered),
                ty: Ty::Bool,
            }
        }
        HirExpr::FString(parts) => MirExpr::FString(
            parts
                .iter()
                .map(|p| match p {
                    FStringPart::Literal(s) => MirFStringPart::Literal(s.clone()),
                    FStringPart::Interpolation(e) => {
                        let lowered = lower_expr(e, scopes, classes, current_class);
                        // #378 (PR-18): if the interpolation is a class
                        // instance with `__repr__`, rewrite it to a call to
                        // `__repr__` so the codegen's `to_str` receives a
                        // `str` scalar, not an Instance scalar (which would
                        // panic). This mirrors how `==`/`!=` on instances is
                        // rewritten to `__eq__` calls in the Compare arm.
                        // Part 3A of #541 (#736): try the exception-message
                        // rewrite first -- see the identical `print`-argument
                        // ordering comment above for why.
                        let lowered = rewrite_exception_to_message(&lowered, classes);
                        let rewritten = rewrite_instance_to_repr(&lowered, classes);
                        MirFStringPart::Interpolation(Box::new(rewritten))
                    }
                })
                .collect(),
        ),
        HirExpr::ListLiteral(elements) => MirExpr::ListLiteral(
            elements
                .iter()
                .map(|e| lower_expr(e, scopes, classes, current_class))
                .collect(),
        ),
        // `HirExpr::Subscript` is reused unconditionally by `pycc_hir`'s own
        // lowering for both a list read and a dict read (it has no type
        // information to pick a different node) -- `pycc_types::infer_expr_in`
        // accepts either base equally (see its own `Subscript` arm), so this
        // is the point where the real type is resolved and where a
        // dict-typed base is routed into `MirExpr::DictGet` instead of
        // `MirExpr::Subscript`, mirroring `lower_stmt`'s own `HirStmt::ForList`
        // arm doing the same list/dict routing for iteration.
        HirExpr::Subscript { base, index } => {
            // PEP 560 (#610): `C[x]` on a bare class name is
            // `C.__class_getitem__(x)`. `pycc_types`' own `Subscript` arm
            // has already resolved the hook through the MRO and rejected a
            // class that does not define it, so reaching this point with a
            // class-name base means the call is valid. The guard is the same
            // one that arm applies -- a name bound as a value shadows the
            // class, and `C[0]` then reads that value's element.
            //
            // Lowering is delegated to this function's own `MethodCall` arm
            // by way of a synthetic node rather than duplicated here: that
            // arm already handles both the `@staticmethod` and the
            // `@classmethod` spelling of the hook (the latter needs a
            // `NullInstance` receiver prepended), and re-deriving either
            // shape here would be a second copy of logic that must not
            // drift. `lower_expr` on a bare class name would panic (class
            // names are not in scope), which is why the interception happens
            // before the base is lowered.
            if let HirExpr::Name(class_name) = base.as_ref()
                && !scopes.iter().any(|scope| scope.contains_key(class_name))
                && classes.contains_key(class_name.as_str())
            {
                return lower_expr(
                    &HirExpr::MethodCall {
                        base: base.clone(),
                        method: "__class_getitem__".to_string(),
                        args: vec![(**index).clone()],
                    },
                    scopes,
                    classes,
                    current_class,
                );
            }
            let base = lower_expr(base, scopes, classes, current_class);
            let index = lower_expr(index, scopes, classes, current_class);
            match base.ty() {
                Ty::Dict(_) => MirExpr::DictGet {
                    dict: Box::new(base),
                    key: Box::new(index),
                },
                _ => MirExpr::Subscript {
                    base: Box::new(base),
                    index: Box::new(index),
                },
            }
        }
        HirExpr::ListAppend { list, value } => MirExpr::ListAppend {
            list: list.clone(),
            value: Box::new(lower_expr(value, scopes, classes, current_class)),
        },
        HirExpr::DictLiteral(pairs) => MirExpr::DictLiteral(
            pairs
                .iter()
                .map(|(k, v)| {
                    (
                        lower_expr(k, scopes, classes, current_class),
                        lower_expr(v, scopes, classes, current_class),
                    )
                })
                .collect(),
        ),
        HirExpr::SetLiteral(elements) => MirExpr::SetLiteral(
            elements
                .iter()
                .map(|e| lower_expr(e, scopes, classes, current_class))
                .collect(),
        ),
        HirExpr::TupleLiteral(elements) => MirExpr::TupleLiteral(
            elements
                .iter()
                .map(|e| lower_expr(e, scopes, classes, current_class))
                .collect(),
        ),
        // PR-12 Task 8 (D-118): purely structural -- recurse into `base` and
        // every present bound, same as every other MIR-lowering site in this
        // function. `pycc_types` already validated the base is
        // `Ty::List(Int)` and every present bound is `int`-assignable
        // (Task 7); this lowering does not re-validate or apply the runtime
        // clamping/panic behavior D-118 requires -- that is
        // `pycc_codegen`'s job, operating on this already-lowered shape.
        HirExpr::Slice {
            base,
            start,
            stop,
            step,
        } => MirExpr::Slice {
            base: Box::new(lower_expr(base, scopes, classes, current_class)),
            start: start
                .as_deref()
                .map(|e| Box::new(lower_expr(e, scopes, classes, current_class))),
            stop: stop
                .as_deref()
                .map(|e| Box::new(lower_expr(e, scopes, classes, current_class))),
            step: step
                .as_deref()
                .map(|e| Box::new(lower_expr(e, scopes, classes, current_class))),
        },
        // PR-12 Task 11 (D-119): `list`'s element type is resolved via the
        // same `lookup` mechanism every other name reference in this crate
        // uses, mirroring `HirExpr::Subscript`'s own base-type lookup above.
        HirExpr::ListPop { list } => {
            let Ty::List(elem_ty) = lookup(scopes, list) else {
                panic!(
                    "pycc_mir: internal error: `{list}` is not list-typed -- pycc_types::check should have rejected this HIR before it reached pycc_mir"
                )
            };
            MirExpr::ListPop {
                list: list.clone(),
                ty: *elem_ty,
            }
        }
        HirExpr::DictGetOrDefault { dict, key, default } => {
            let Ty::Dict(kv) = lookup(scopes, dict) else {
                panic!(
                    "pycc_mir: internal error: `{dict}` is not dict-typed -- pycc_types::check should have rejected this HIR before it reached pycc_mir"
                )
            };
            MirExpr::DictGetOrDefault {
                dict: dict.clone(),
                key: Box::new(lower_expr(key, scopes, classes, current_class)),
                default: Box::new(lower_expr(default, scopes, classes, current_class)),
                ty: kv.1,
            }
        }
        HirExpr::SetAdd { set, value } => MirExpr::SetAdd {
            set: set.clone(),
            value: Box::new(lower_expr(value, scopes, classes, current_class)),
        },
        // D-154 (Part 1 of #375): `base.attr` -- resolved to a compile-time
        // slot index against the base's class's `HirClassDef`, per the
        // class-instance-layout ADR (never a runtime string-keyed lookup).
        // #377: if `attr` is a `@property`, the access is rewritten to an
        // ordinary `MirExpr::Call` to the getter's mangled name (with `base`
        // as `self`), reusing the existing method-call/codegen infrastructure
        // with no new MIR/codegen variant.
        // #432: property and attribute resolution walk the MRO, so an
        // attribute or property declared in a base class is found when
        // accessed on a derived class instance. The slot index is computed
        // from the MRO's flat attribute layout (`mro_attrs`).
        HirExpr::AttrGet { base, attr } => {
            // #433: `super().attr` — resolve the attribute starting from
            // the next class in the current class's MRO, using `self` (the
            // current function's first parameter) as the instance.
            // #587: only a `@property` resolves this way. A `super` object
            // proxies class-level attributes and descriptors, not the
            // instance's own attributes, so `super().<instance attr>` no
            // longer lowers to a slot read against `self` — the type
            // checker rejects it with `T0047` first.
            if matches!(base.as_ref(), HirExpr::Super) {
                let current = match current_class {
                    Some(c) => c,
                    None => panic!(
                        "pycc_mir: internal error: `HirExpr::Super` reached lower_expr outside a \
                         method body -- pycc_hir::lower_expr should have rejected this with C0001"
                    ),
                };
                let self_expr = self_expr(scopes);
                let class_def = &classes[current];
                let current_pos = class_def
                    .mro
                    .iter()
                    .position(|c| c == current)
                    .expect("pycc_mir: internal error: class not found in its own MRO");
                let super_mro = &class_def.mro[current_pos + 1..];
                // Properties first (matching the non-super AttrGet arm).
                for mro_class in super_mro {
                    let mro_def = &classes[mro_class.as_str()];
                    if let Some(prop) = mro_def.properties.iter().find(|p| p.name == *attr) {
                        let ty = lookup(scopes, &format!("$fn:{}", prop.getter));
                        return MirExpr::Call {
                            callee: prop.getter.clone(),
                            args: vec![self_expr],
                            ty,
                        };
                    }
                }
                // #587: nothing else resolves through `super()`. A `super`
                // object proxies class-level attributes and descriptors
                // along the MRO, not the instance's own attributes, so an
                // `AttrGet` naming an instance attribute (or a name declared
                // nowhere in the MRO at all) is rejected by
                // `pycc_types::class::resolve_super_attr_get` with `T0047`
                // or `T0044` before any HIR reaches this crate. Properties
                // are the only class-level member pycc models today, so the
                // loop above is exhaustive for well-typed input.
                panic!(
                    "pycc_mir: internal error: `super().{attr}` is not a property on any class \
                     after `{current}` in its MRO -- pycc_types::check should have rejected this \
                     HIR with T0047 or T0044 before it reached pycc_mir"
                );
            }
            // #379 (PR-19): `Color.RED` — accessing an enum member by name
            // on the enum class. The base is `HirExpr::Name` referring to
            // an enum class (a class with non-empty `enum_members`), and
            // `attr` is a member name. Lower to `MirExpr::Name` reading a
            // synthetic global (`<Class>.<Member>.enum_member`) that
            // codegen initializes once at module-init time with the
            // member's singleton instance. The `.enum_member` suffix
            // ensures the name cannot collide with a real Python
            // identifier (which cannot contain `.`). The subsequent
            // `.value`/`.name` read on this result is a separate
            // `AttrGet` that resolves to a slot via the enum class's
            // `attrs = [("value", Int), ("name", Str)]` table.
            if let Some(enum_member_expr) =
                try_lower_enum_member_attr(base.as_ref(), attr.as_str(), classes)
            {
                return enum_member_expr;
            }
            // #911 (Part 1 of #885): `W.MIN_WIDTH` -- a class-level attribute
            // read through the class name. Intercepted before `lower_expr`
            // for the same reason the enum-member read above is: the base is
            // a class name, not a value binding, so lowering it as an
            // expression would fail.
            if let HirExpr::Name(class_name) = base.as_ref()
                && let Some(class_def) = classes.get(class_name.as_str())
                && let Some(folded) = fold_class_attr(class_def, attr)
            {
                return folded;
            }
            let base = lower_expr(base, scopes, classes, current_class);
            let class_def = class_def_of(&base, classes);
            // #432: walk the MRO for property lookup first (matching
            // CPython's descriptor protocol precedence), then for regular
            // attribute slots using the flat MRO layout.
            for mro_class in &class_def.mro {
                let mro_def = mro_class_def(mro_class, classes);
                if let Some(prop) = mro_def.properties.iter().find(|p| p.name == *attr) {
                    let ty = lookup(scopes, &format!("$fn:{}", prop.getter));
                    return MirExpr::Call {
                        callee: prop.getter.clone(),
                        args: vec![base],
                        ty,
                    };
                }
            }
            // #911 (Part 1 of #885): `w.MIN_WIDTH` / `self.MIN_WIDTH` -- a
            // class-level attribute read through an instance. Folded to its
            // constant *before* the `mro_attrs` lookup below, whose miss
            // panics: a class attribute deliberately never enters
            // `mro_attrs`/`mro_attr_count`, so it occupies no instance slot
            // and does not change any other attribute's slot index or the
            // allocation size of the class's instances.
            //
            // Discarding the already-lowered `base` here is sound because
            // `pycc_types` restricts a class-attribute read's base to a bare
            // name -- evaluating it has no observable effect.
            for mro_class in &class_def.mro {
                let mro_def = mro_class_def(mro_class, classes);
                if let Some(folded) = fold_class_attr(mro_def, attr) {
                    return folded;
                }
            }
            let flat_attrs = mro_attrs(class_def, classes);
            let (slot, (_, ty)) = flat_attrs
                .iter()
                .enumerate()
                .find(|(_, (name, _))| name == attr)
                .unwrap_or_else(|| {
                    panic!(
                        "pycc_mir: internal error: attribute `{attr}` not declared on class `{}` \
                         or any base in its MRO -- pycc_types::check should have rejected this \
                         HIR before it reached pycc_mir",
                        class_def.name
                    )
                });
            MirExpr::AttrGet {
                base: Box::new(base),
                slot,
                ty: ty.clone(),
            }
        }
        // D-154 (Part 1 of #375): `base.method(args)` resolves to the
        // method's mangled, compile-time-known function symbol (D-006's
        // static-dispatch framing for a non-inherited class -- no vtable,
        // no runtime dispatch), then lowers into an ordinary `MirExpr::Call`
        // with `base` prepended as `self` -- unlike instantiation, a method
        // call never allocates, so it needs no dedicated MIR node of its
        // own (see `MirExpr::Instantiate`'s own doc comment for the
        // contrasting case).
        // #432: method resolution walks the MRO, so a method declared in a
        // base class is found when called on a derived class instance. A
        // subclass method shadows a base class method of the same name (the
        // subclass appears first in the MRO).
        HirExpr::MethodCall { base, method, args } => {
            // #433: `super().method(args)` — resolve the method starting
            // from the next class in the current class's MRO, using `self`
            // (the current function's first parameter) as the instance.
            // Lowers to a direct `MirExpr::Call` to the resolved method's
            // mangled name, with `self` prepended as the first argument —
            // no vtable, no runtime dispatch (D-006 static-dispatch framing,
            // per the #433 ADR).
            if matches!(base.as_ref(), HirExpr::Super) {
                let current = match current_class {
                    Some(c) => c,
                    None => panic!(
                        "pycc_mir: internal error: `HirExpr::Super` reached lower_expr outside a \
                         method body -- pycc_hir::lower_expr should have rejected this with C0001"
                    ),
                };
                let self_expr = self_expr(scopes);
                let class_def = &classes[current];
                let current_pos = class_def
                    .mro
                    .iter()
                    .position(|c| c == current)
                    .expect("pycc_mir: internal error: class not found in its own MRO");
                let super_mro = &class_def.mro[current_pos + 1..];
                let mangled = super_mro
                    .iter()
                    .find_map(|mro_class| {
                        let mro_def = &classes[mro_class.as_str()];
                        mro_def
                            .methods
                            .iter()
                            .find(|(name, _)| name == method)
                            .map(|(_, mangled)| mangled.clone())
                    })
                    .expect(
                        "pycc_mir: internal error: method not declared on class or any base in its \
                     MRO after the current class -- pycc_types::check should have rejected this \
                     HIR before it reached pycc_mir",
                    );
                let ty = lookup(scopes, &format!("$fn:{mangled}"));
                let mut call_args = Vec::with_capacity(args.len() + 1);
                call_args.push(self_expr);
                call_args.extend(
                    args.iter()
                        .map(|a| lower_expr(a, scopes, classes, current_class)),
                );
                return MirExpr::Call {
                    callee: mangled,
                    args: call_args,
                    ty,
                };
            }
            // #436: `ClassName.static_method(args)` or
            // `ClassName.class_method(args)` — a method call on a class
            // name (not an instance). The base is `HirExpr::Name` referring
            // to a registered class. `lower_expr` on a bare class name
            // would panic (class names are not in the scope), so intercept
            // here before lowering the base.
            if let HirExpr::Name(class_name) = base.as_ref()
                && classes.contains_key(class_name.as_str())
            {
                let class_def = &classes[class_name.as_str()];
                let static_mangled = class_def.mro.iter().find_map(|mro_class| {
                    let mro_def = mro_class_def(mro_class, classes);
                    mro_def
                        .static_methods
                        .iter()
                        .find(|(name, _)| name == method)
                        .map(|(_, mangled)| mangled.clone())
                });
                if let Some(mangled) = static_mangled {
                    let ty = lookup(scopes, &format!("$fn:{mangled}"));
                    let call_args: Vec<MirExpr> = args
                        .iter()
                        .map(|a| lower_expr(a, scopes, classes, current_class))
                        .collect();
                    return MirExpr::Call {
                        callee: mangled,
                        args: call_args,
                        ty,
                    };
                }
                let class_mangled = class_def.mro.iter().find_map(|mro_class| {
                    let mro_def = mro_class_def(mro_class, classes);
                    mro_def
                        .class_methods
                        .iter()
                        .find(|(name, _)| name == method)
                        .map(|(_, mangled)| mangled.clone())
                });
                if let Some(mangled) = class_mangled {
                    let ty = lookup(scopes, &format!("$fn:{mangled}"));
                    let mut call_args = Vec::with_capacity(args.len() + 1);
                    call_args.push(MirExpr::NullInstance {
                        ty: Ty::Instance(Box::new(class_name.clone())),
                    });
                    call_args.extend(
                        args.iter()
                            .map(|a| lower_expr(a, scopes, classes, current_class)),
                    );
                    return MirExpr::Call {
                        callee: mangled,
                        args: call_args,
                        ty,
                    };
                }
            }
            let base = lower_expr(base, scopes, classes, current_class);
            let class_def = class_def_of(&base, classes);
            // #436: check static_methods and class_methods before regular
            // method resolution. Static methods can be called on both
            // classes and instances; class methods can too. When called on
            // an instance, the instance is passed as `cls`/`self`.
            let static_mangled = class_def.mro.iter().find_map(|mro_class| {
                let mro_def = mro_class_def(mro_class, classes);
                mro_def
                    .static_methods
                    .iter()
                    .find(|(name, _)| name == method)
                    .map(|(_, mangled)| mangled.clone())
            });
            if let Some(mangled) = static_mangled {
                let ty = lookup(scopes, &format!("$fn:{mangled}"));
                let call_args: Vec<MirExpr> = args
                    .iter()
                    .map(|a| lower_expr(a, scopes, classes, current_class))
                    .collect();
                return MirExpr::Call {
                    callee: mangled,
                    args: call_args,
                    ty,
                };
            }
            let class_mangled = class_def.mro.iter().find_map(|mro_class| {
                let mro_def = mro_class_def(mro_class, classes);
                mro_def
                    .class_methods
                    .iter()
                    .find(|(name, _)| name == method)
                    .map(|(_, mangled)| mangled.clone())
            });
            if let Some(mangled) = class_mangled {
                let ty = lookup(scopes, &format!("$fn:{mangled}"));
                let mut call_args = Vec::with_capacity(args.len() + 1);
                call_args.push(base);
                call_args.extend(
                    args.iter()
                        .map(|a| lower_expr(a, scopes, classes, current_class)),
                );
                return MirExpr::Call {
                    callee: mangled,
                    args: call_args,
                    ty,
                };
            }
            // #432: walk the MRO to find the method's mangled name.
            let mangled = class_def
                .mro
                .iter()
                .find_map(|mro_class| {
                    let mro_def = mro_class_def(mro_class, classes);
                    mro_def
                        .methods
                        .iter()
                        .find(|(name, _)| name == method)
                        .map(|(_, mangled)| mangled.clone())
                })
                .unwrap_or_else(|| {
                    panic!(
                        "pycc_mir: internal error: method `{method}` not declared on class `{}` or \
                     any base in its MRO -- pycc_types::check should have rejected this HIR \
                     before it reached pycc_mir",
                        class_def.name
                    )
                });
            let ty = lookup(scopes, &format!("$fn:{mangled}"));
            let mut call_args = Vec::with_capacity(args.len() + 1);
            call_args.push(base);
            call_args.extend(
                args.iter()
                    .map(|a| lower_expr(a, scopes, classes, current_class)),
            );
            MirExpr::Call {
                callee: mangled,
                args: call_args,
                ty,
            }
        }
        // PEP 695 (#387): `GenericClassInstantiate` should never reach MIR
        // — `pycc_types::monomorphize` rewrites every
        // `GenericClassInstantiate` expression to an ordinary
        // `HirExpr::Call` to the mangled class name before MIR lowering
        // runs. If this arm is reached, it indicates a bug in the
        // monomorphization pass.
        HirExpr::GenericClassInstantiate { class, .. } => {
            panic!(
                "pycc_mir: internal error: `GenericClassInstantiate` for class `{class}` \
                 reached MIR lowering -- pycc_types::monomorphize should have rewritten it \
                 to an ordinary `HirExpr::Call` before this point"
            )
        }
        // PEP 572 (#774): `target := value`. Lowers `value` first (bottom-up,
        // like every other composite arm here), then wraps it in
        // `MirExpr::NamedExpr` carrying the lowered value's own type -- this
        // node's own "evaluate, store, yield" codegen (see
        // `pycc_codegen::emit_expr_unchecked`'s `MirExpr::NamedExpr` arm)
        // does the actual store. This function only ever holds a read-only
        // `scopes: &[HashMap<String, Ty>]` slice, so it cannot itself
        // register `name`'s binding for a later statement to `lookup` --
        // that happens one level up, in `pycc_mir::stmt::lower_stmt`'s
        // `ExprStmt`/`If`/`While` arms, via
        // `pycc_mir::stmt::collect_named_expr_bindings`, which walks the
        // already-lowered `MirExpr` tree (so it can read `ty()` directly
        // rather than re-inferring it) and calls `bind_variable` for every
        // `NamedExpr` node found, before that statement's body/next
        // statement is lowered. `pycc_hir::stmt::lower_stmt`'s own
        // `contains_named_expr` restriction guarantees a `NamedExpr` node
        // only ever appears nested inside exactly those three statement
        // placements, so no other `lower_stmt` arm needs this treatment.
        // PEP 572 (#774): by the time this arm runs, `pre_bind_named_expr_targets`
        // (called by `pycc_mir::stmt::lower_stmt` before it lowers the whole
        // statement) has already bound `name` into `scopes` -- see that
        // function's own doc comment for why a pre-pass, not this ordinary
        // bottom-up lowering, has to be the one doing the actual `scopes`
        // mutation. This arm's own job is unchanged from a first glance:
        // lower `value` (again -- cheap, `value` has no side effects beyond
        // further nested walrus bindings the pre-pass already applied) and
        // wrap it in `MirExpr::NamedExpr` carrying its type.
        HirExpr::NamedExpr { name, value } => {
            let value = lower_expr(value, scopes, classes, current_class);
            let ty = value.ty();
            MirExpr::NamedExpr {
                name: name.clone(),
                value: Box::new(value),
                ty,
            }
        }
        // #433: a bare `HirExpr::Super` should never reach MIR lowering —
        // HIR lowering rejects a standalone `super()` with C0001, and
        // `super().method()`/`super().attr` are handled by the special-case
        // blocks above before recursing into `lower_expr` for the base.
        HirExpr::Super => {
            panic!(
                "pycc_mir: internal error: a bare `HirExpr::Super` reached MIR lowering -- \
                 pycc_hir::lower_expr should have rejected this with C0001, or the \
                 `MethodCall`/`AttrGet` arms should have intercepted it before recursing"
            )
        }
    }
}

/// PEP 572 (#774): walks `expr` for every `HirExpr::NamedExpr { name, value }`
/// node, in the expression's own left-to-right evaluation order, and binds
/// each `name` into `scopes` *before* `lower_stmt` calls `lower_expr` on the
/// whole enclosing statement.
///
/// This pre-pass exists because `lower_expr` itself only ever holds a
/// read-only `scopes: &[HashMap<String, Ty>]` slice -- by design, since it is
/// a pure bottom-up lowering with no other reason to mutate scope state. That
/// is fine for every ordinary case, where a `NamedExpr`'s binding only needs
/// to be visible to a *later statement* (handled by
/// `pycc_mir::stmt::collect_named_expr_bindings`, which walks the
/// already-lowered `MirExpr` tree after the fact). It breaks down for a
/// walrus value that itself references an *earlier walrus in the same
/// expression* -- `(a := 1) + (b := a + 1)` -- where `b`'s value needs `a`
/// already bound in `scopes` while `lower_expr` is still partway through
/// lowering the very same top-level expression that defines `a`. Without
/// this pre-pass, `lower_expr`'s `HirExpr::Name("a")` arm would call
/// `lookup(scopes, "a")`, which panics (`pycc_types::check` already proved
/// the program well-typed, including this exact ordering, by running its own
/// mirror-image pre-pass -- see `pycc_types::collect_named_expr_bindings`'s
/// doc comment for the type-checker side of this same fix).
///
/// Each `name`'s type is computed by actually lowering its `value` here
/// (recursing into `value` first, so a nested `NamedExpr` inside it is bound
/// before `value` itself is lowered) -- cheap and side-effect-free (beyond
/// the further nested bindings this same pre-pass already applies), and the
/// simplest way to get a `Ty` without re-deriving one structurally from the
/// unlowered `HirExpr`. `lower_stmt`'s own subsequent full-statement
/// `lower_expr` call re-lowers this same subtree, which is redundant but
/// harmless: `bind_variable`'s `.entry(..).or_insert(..)` semantics make a
/// repeated bind of the same name a no-op.
pub(super) fn pre_bind_named_expr_targets(
    expr: &HirExpr,
    scopes: &mut Vec<HashMap<String, Ty>>,
    classes: &HashMap<String, HirClassDef>,
    current_class: Option<&str>,
) {
    match expr {
        HirExpr::NamedExpr { name, value } => {
            pre_bind_named_expr_targets(value, scopes, classes, current_class);
            let lowered_value = lower_expr(value, scopes, classes, current_class);
            let ty = lowered_value.ty();
            // D-068 review of #780/#774's interaction (blocker finding 1): a
            // walrus target is a reassignment exactly like `Assign`'s own arm
            // in `stmt.rs` (see its paired `kill_narrowing`/`bind_variable`
            // calls), so it must clear any stale narrowing sentinel for
            // `name` the same way -- otherwise a subsequent read still
            // lowers to an unconditional `MirExpr::OptionalUnwrap` for a
            // value the walrus may have just overwritten with `None`.
            super::kill_narrowing(scopes, name);
            super::bind_variable(scopes, name.clone(), ty);
        }
        HirExpr::IntLiteral(_)
        | HirExpr::FloatLiteral(_)
        | HirExpr::BoolLiteral(_)
        | HirExpr::StringLiteral(_)
        | HirExpr::NoneLiteral
        | HirExpr::Name(_)
        | HirExpr::ListPop { .. }
        | HirExpr::Super => {}
        HirExpr::Call { args, .. } => {
            for arg in args {
                pre_bind_named_expr_targets(arg, scopes, classes, current_class);
            }
        }
        HirExpr::BinOp { left, right, .. } | HirExpr::Compare { left, right, .. } => {
            pre_bind_named_expr_targets(left, scopes, classes, current_class);
            pre_bind_named_expr_targets(right, scopes, classes, current_class);
        }
        HirExpr::UnaryOp { operand, .. } => {
            pre_bind_named_expr_targets(operand, scopes, classes, current_class)
        }
        HirExpr::FString(parts) => {
            for part in parts {
                if let FStringPart::Interpolation(inner) = part {
                    pre_bind_named_expr_targets(inner, scopes, classes, current_class);
                }
            }
        }
        HirExpr::ListLiteral(es) | HirExpr::SetLiteral(es) | HirExpr::TupleLiteral(es) => {
            for e in es {
                pre_bind_named_expr_targets(e, scopes, classes, current_class);
            }
        }
        HirExpr::Subscript { base, index } => {
            pre_bind_named_expr_targets(base, scopes, classes, current_class);
            pre_bind_named_expr_targets(index, scopes, classes, current_class);
        }
        HirExpr::Slice {
            base,
            start,
            stop,
            step,
        } => {
            pre_bind_named_expr_targets(base, scopes, classes, current_class);
            for bound in [start, stop, step].into_iter().flatten() {
                pre_bind_named_expr_targets(bound, scopes, classes, current_class);
            }
        }
        HirExpr::ListAppend { value, .. } | HirExpr::SetAdd { value, .. } => {
            pre_bind_named_expr_targets(value, scopes, classes, current_class);
        }
        HirExpr::DictLiteral(pairs) => {
            for (k, v) in pairs {
                pre_bind_named_expr_targets(k, scopes, classes, current_class);
                pre_bind_named_expr_targets(v, scopes, classes, current_class);
            }
        }
        HirExpr::DictGetOrDefault { key, default, .. } => {
            pre_bind_named_expr_targets(key, scopes, classes, current_class);
            pre_bind_named_expr_targets(default, scopes, classes, current_class);
        }
        HirExpr::AttrGet { base, .. } => {
            pre_bind_named_expr_targets(base, scopes, classes, current_class)
        }
        HirExpr::MethodCall { base, args, .. } => {
            pre_bind_named_expr_targets(base, scopes, classes, current_class);
            for arg in args {
                pre_bind_named_expr_targets(arg, scopes, classes, current_class);
            }
        }
        HirExpr::GenericClassInstantiate { args, .. } => {
            for arg in args {
                pre_bind_named_expr_targets(arg, scopes, classes, current_class);
            }
        }
    }
}

/// #911 (Part 1 of #885): folds a class-level attribute read into the
/// literal recorded in `HirClassDef::class_attrs`, or `None` when
/// `class_def` declares no attribute of that name.
///
/// A class attribute has no runtime storage at all -- no instance slot, no
/// module global (unlike an enum member's singleton), and no `pycc_codegen`
/// footprint -- so every read is resolved here, at MIR-lowering time.
fn fold_class_attr(class_def: &HirClassDef, attr: &str) -> Option<MirExpr> {
    let (_, _, value) = class_def
        .class_attrs
        .iter()
        .find(|(name, _, _)| name == attr)?;
    Some(match value {
        ClassAttrValue::Int(i) => MirExpr::IntLiteral(*i),
        ClassAttrValue::Float(f) => MirExpr::FloatLiteral(*f),
        ClassAttrValue::Bool(b) => MirExpr::BoolLiteral(*b),
        ClassAttrValue::Str(s) => MirExpr::StringLiteral(s.clone()),
    })
}
