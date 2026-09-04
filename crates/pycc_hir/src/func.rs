//! Function-definition lowering: parameters, return annotations, and the
//! annotation-to-`Ty` conversion every annotated binding in the crate goes
//! through.
//!
//! Extracted from `lib.rs` per AGENTS.md's file-decomposition rule (issue
//! #547, Part 2). This is not a low-fan-in leaf the way `stmt`/`expr` were:
//! `lower_function` and `lower_params` are each called once (from
//! `module::lower_top_level_item` and from `lower_function` respectively), but the three
//! `pub(crate)` helpers are reached from across the crate --
//! `lower_return_annotation` from 3 call sites in `class.rs`,
//! `lower_arg_list` from 8, `type_param_name` from 1, and `annotation_to_ty`
//! from `class.rs`, `stmt.rs`, `tests.rs`, and the sibling `import` module's
//! type-alias lowering. `lib.rs` therefore re-exports all five items
//! `pub(crate)`, so every existing `crate::`-qualified call site keeps
//! resolving unchanged.

use crate::class::ClassAnnotationInfo;
use crate::{HirItem, Ty, stmt, unsupported};
use pycc_ast::{Expr, Operator};
use pycc_diag::{Diagnostic, Span};

pub(crate) fn lower_function(
    def: &pycc_ast::StmtFunctionDef,
    aliases: &[(String, Ty)],
    class_defs: &[ClassAnnotationInfo],
) -> Result<HirItem, Diagnostic> {
    if def.is_async {
        return Err(unsupported(
            "async functions are not supported yet",
            def.range,
        ));
    }
    if !def.decorator_list.is_empty() {
        return Err(unsupported(
            "function decorators are not supported yet",
            def.range,
        ));
    }
    let type_param: Option<Box<str>> = match def.type_params.as_deref() {
        None => None,
        Some(type_params) => match type_params.type_params.as_slice() {
            [single] => Some(type_param_name(single, def.range)?.into()),
            _ => {
                return Err(unsupported(
                    "generic functions with more than one type parameter are not supported yet",
                    def.range,
                ));
            }
        },
    };
    let is_public = !def.name.as_str().starts_with('_'); // D-038
    let params = lower_params(
        &def.parameters,
        is_public,
        def.name.as_str(),
        type_param.as_deref(),
        aliases,
        class_defs,
    )?;
    let return_ty = lower_return_annotation(
        def.returns.as_deref(),
        is_public,
        def.name.as_str(),
        type_param.as_deref(),
        None,
        aliases,
        class_defs,
    )?;
    let body = stmt::lower_body(
        &def.body,
        aliases,
        false,
        true,
        false,
        // #795 (PEP 654): a nested function body always starts
        // `Outside` any enclosing `except*` clause -- CPython accepts a
        // `return` in a `def` nested inside an `except*` body. This is a
        // constant, not a conditional on the enclosing context, exactly like
        // the three `false`s above it.
        stmt::ExceptStarCtx::Outside,
        None,
        type_param.as_deref(),
        class_defs,
    )?;
    Ok(HirItem::Function {
        name: def.name.to_string(),
        params,
        return_ty,
        body,
    })
}

/// Extracts a PEP 695 `TypeVar`'s identifier -- e.g. the `T` in `def
/// f[T](...)`. `Ty::Param` (D-133) is resolved by call-site substitution
/// (D-134) into one concrete scalar type per call, which is only a coherent
/// model for a plain `TypeVar`: `TypeVarTuple` (`def f[*Ts](...)`) stands for
/// a variable-length sequence of types, and `ParamSpec` (`def f[**P](...)`)
/// stands for a parameter list shape, neither of which `Ty::Param` can
/// represent. `def_range` is the enclosing function's range, reused for the
/// diagnostic span since `TypeParam`'s own range would require reaching past
/// the `pycc_ast` facade for the `Ranged` trait for no benefit here (the
/// arity-gate rejection just above already reports the same function-level
/// span for the analogous "too many type parameters" case).
pub(crate) fn type_param_name<R>(
    type_param: &pycc_ast::TypeParam,
    def_range: R,
) -> Result<&str, Diagnostic>
where
    std::ops::Range<u32>: From<R>,
{
    match type_param {
        pycc_ast::TypeParam::TypeVar(tv) => Ok(tv.name.as_str()),
        pycc_ast::TypeParam::TypeVarTuple(_) => Err(unsupported(
            "a `TypeVarTuple` type parameter (`*Ts`) is not supported yet",
            def_range,
        )),
        pycc_ast::TypeParam::ParamSpec(_) => Err(unsupported(
            "a `ParamSpec` type parameter (`**P`) is not supported yet",
            def_range,
        )),
    }
}

pub(crate) fn lower_params(
    parameters: &pycc_ast::Parameters,
    is_public: bool,
    fn_name: &str,
    type_param: Option<&str>,
    aliases: &[(String, Ty)],
    class_defs: &[ClassAnnotationInfo],
) -> Result<Vec<(String, Ty)>, Diagnostic> {
    // Every parameter kind and default value below is silently absent from
    // `parameters.args`/`ParameterWithDefault::default` -- an earlier version
    // of this function only ever iterated `.args` and never checked for any
    // of these, so a function using them got a wrong signature built from
    // whatever plain positional args happened to exist, instead of the
    // explicit capability diagnostic every other out-of-scope construct in
    // this file produces (self-review finding, pre-merge).
    //
    // PEP 570 (#383): positional-only parameters (`posonlyargs`, before the
    // `/` marker) are now lowered via the same `lower_arg_list` path as
    // ordinary `args`, prepended before `args` in the parameter list. Since
    // keyword call arguments are already globally unsupported (rejected in
    // `expr.rs`/`stmt.rs`), every parameter is already effectively
    // positional-only — accepting `posonlyargs` changes nothing about
    // call-site checking.
    if parameters.vararg.is_some() {
        return Err(unsupported(
            "`*args` is not supported yet",
            parameters.range,
        ));
    }
    if !parameters.kwonlyargs.is_empty() {
        return Err(unsupported(
            "keyword-only parameters are not supported yet",
            parameters.range,
        ));
    }
    if parameters.kwarg.is_some() {
        return Err(unsupported(
            "`**kwargs` is not supported yet",
            parameters.range,
        ));
    }
    // PEP 570 (#383): lower `posonlyargs` (before `/`) via the same
    // `lower_arg_list` path as ordinary `args`, prepending them. The full
    // parameter list is `posonlyargs ++ args`.
    let mut params = lower_arg_list(
        &parameters.posonlyargs,
        is_public,
        fn_name,
        type_param,
        None,
        aliases,
        class_defs,
    )?;
    params.extend(lower_arg_list(
        &parameters.args,
        is_public,
        fn_name,
        type_param,
        None,
        aliases,
        class_defs,
    )?);
    Ok(params)
}

/// Lowers a plain positional-parameter list (no `/`/`*`/`**`/keyword-only
/// markers -- callers are responsible for rejecting those first, since their
/// diagnostics differ by caller: `lower_params` reports them against a
/// top-level function's own `parameters`, `class::lower_method` (D-154, Part
/// 1 of #375) reports the identical checks against a method's `parameters`,
/// which also includes the leading `self` parameter that helper strips
/// before delegating here). Factored out of `lower_params` (which still owns
/// every top-level function's own shape validation, unchanged) so both
/// callers share this one per-parameter annotation-resolution rule instead
/// of duplicating it.
pub(crate) fn lower_arg_list(
    args: &[pycc_ast::ParameterWithDefault],
    is_public: bool,
    fn_name: &str,
    type_param: Option<&str>,
    class_name: Option<&str>,
    aliases: &[(String, Ty)],
    class_defs: &[ClassAnnotationInfo],
) -> Result<Vec<(String, Ty)>, Diagnostic> {
    args.iter()
        .map(|param| {
            if param.default.is_some() {
                return Err(unsupported(
                    "default parameter values are not supported yet",
                    param.range,
                ));
            }
            let name = param.parameter.name.as_str();
            match &param.parameter.annotation {
                Some(ann) => Ok((
                    name.to_string(),
                    annotation_to_ty(ann, type_param, class_name, aliases, class_defs)?,
                )),
                None if is_public => Err(Diagnostic::error(
                    "T0001",
                    format!(
                        "parameter `{name}` of public function `{fn_name}` needs a type annotation"
                    ),
                    Span::new(0, 0),
                )
                .with_help(format!("add a type annotation to parameter `{name}`"))),
                None => Ok((name.to_string(), Ty::Infer)),
            }
        })
        .collect()
}

pub(crate) fn lower_return_annotation(
    returns: Option<&Expr>,
    is_public: bool,
    fn_name: &str,
    type_param: Option<&str>,
    class_name: Option<&str>,
    aliases: &[(String, Ty)],
    class_defs: &[ClassAnnotationInfo],
) -> Result<Ty, Diagnostic> {
    match returns {
        Some(ann) => {
            let ty = annotation_to_ty(ann, type_param, class_name, aliases, class_defs)?;
            // D-227 (issue #918) lowers container annotations in parameter,
            // local-variable, type-alias and protocol-member positions only.
            // Return position is deliberately excluded and tracked separately
            // as issue #925: a container-typed *call result* already reaches
            // an unhandled codegen case today (issue #926, via D-146's
            // private-helper return-type solver), so accepting the annotation
            // here would widen a known panic's reachability rather than add a
            // working feature. Rejecting it keeps Part 1 diagnostic-complete.
            if matches!(ty, Ty::List(_) | Ty::Dict(_) | Ty::Set(_) | Ty::Tuple(_)) {
                return Err(unsupported(
                    format!(
                        "a container return type annotation (`{}`) is not supported yet -- \
                         container annotations are currently supported in parameter, \
                         local-variable, type-alias and protocol-member positions only",
                        ty.name()
                    ),
                    pycc_ast::expr_range(ann),
                ));
            }
            Ok(ty)
        }
        None if is_public => Err(Diagnostic::error(
            "T0001",
            format!("public function `{fn_name}` needs a return type annotation"),
            Span::new(0, 0),
        )
        .with_help(format!("add a return type annotation to `{fn_name}`"))),
        None => Ok(Ty::Infer),
    }
}

/// The four builtin container types this version lowers from a parameterized
/// annotation (D-227, issue #918). `frozenset[T]` and `type[T]` are absent on
/// purpose: neither has a `Ty` variant, and adding one would have to clear
/// D-109's 16-byte `size_of::<Ty>()` ceiling first.
const CONTAINER_ANNOTATION_NAMES: [&str; 4] = ["list", "set", "dict", "tuple"];

/// A worked parameterized example for a bare container annotation's `C0001`,
/// or `None` for a name that is not one of the four. `tuple` gets its own
/// two-argument example: `tuple[int]` is legal but atypical, and a
/// single-element example would read as if `tuple` were homogeneous.
fn bare_container_example(name: &str) -> Option<&'static str> {
    match name {
        "list" => Some("list[int]"),
        "set" => Some("set[int]"),
        "dict" => Some("dict[str, int]"),
        "tuple" => Some("tuple[int, int]"),
        _ => None,
    }
}

/// Lowers a parameterized builtin container annotation -- `list[T]`,
/// `set[T]`, `dict[K, V]` or `tuple[A, B, ...]` -- to its `Ty` (D-227,
/// issue #918).
///
/// Three checks run in a fixed order, so the reported diagnostic always
/// describes the outermost thing that is wrong:
///
/// 1. an `...` type argument (the homogeneous-variadic `tuple[int, ...]`),
///    rejected with `T0053` because a runtime-length tuple has no fixed-arity
///    `Ty::Tuple` representation;
/// 2. arity -- `list`/`set` take exactly one argument, `dict` exactly two,
///    `tuple` at least one (so the empty `tuple[()]`, which reaches here as a
///    zero-element `Expr::Tuple`, is rejected here rather than silently
///    lowering to a zero-field tuple);
/// 3. each argument's own type, recursively through [`annotation_to_ty`],
///    then the shared element-type capability gate
///    ([`crate::container::check_container_ty`]) that container *literals*
///    also run.
///
/// Between 3's recursion and the capability gate, a `Ty::Param` element is
/// rejected with `T0042` -- the same code and wording `pycc_types`' own
/// signature scan uses, but carrying the annotation's real span instead of
/// that scan's `Span::new(0, 0)`. Catching it here rather than relying on the
/// downstream scan is not just a nicer caret: `substitute_ty` is not
/// recursive, so a `Ty::Param` buried inside a container would not be
/// substituted at a call site even where the scan did let it through.
fn container_annotation_to_ty(
    family: &str,
    slice: &Expr,
    annotation: &Expr,
    type_param: Option<&str>,
    class_name: Option<&str>,
    aliases: &[(String, Ty)],
    class_defs: &[ClassAnnotationInfo],
) -> Result<Ty, Diagnostic> {
    let span = {
        let range = pycc_ast::expr_range(annotation);
        Span::new(range.start, range.end)
    };
    // A single type argument arrives as the bare expression; two or more (and
    // the empty `tuple[()]`) arrive as an `Expr::Tuple`.
    let args: Vec<&Expr> = match slice {
        Expr::Tuple(tuple) => tuple.elts.iter().collect(),
        other => vec![other],
    };
    if args
        .iter()
        .any(|arg| matches!(arg, Expr::EllipsisLiteral(_)))
    {
        return Err(Diagnostic::error(
            "T0053",
            format!(
                "the `...` type argument in `{family}[...]` is not supported yet -- a homogeneous-variadic container has no compile-time length, so write an explicit fixed-arity annotation such as `tuple[int, int]` instead"
            ),
            span,
        ));
    }
    let exact_arity = match family {
        "list" | "set" => Some(1usize),
        "dict" => Some(2usize),
        // `tuple` is variadic in arity: any count of one or more.
        _ => None,
    };
    match exact_arity {
        Some(expected) if args.len() != expected => {
            return Err(Diagnostic::error(
                "T0053",
                format!(
                    "container type annotation `{family}[...]` takes exactly {expected} type argument{}, got {}",
                    if expected == 1 { "" } else { "s" },
                    args.len()
                ),
                span,
            ));
        }
        None if args.is_empty() => {
            return Err(Diagnostic::error(
                "T0053",
                "container type annotation `tuple[...]` takes at least 1 type argument -- the empty tuple `tuple[()]` is not supported yet".to_string(),
                span,
            ));
        }
        _ => {}
    }
    let mut elements = Vec::with_capacity(args.len());
    for arg in &args {
        let element = annotation_to_ty(arg, type_param, class_name, aliases, class_defs)?;
        if let Ty::Param(name) = &element {
            return Err(Diagnostic::error(
                "T0042",
                format!(
                    "type parameter `{name}` used inside a container position is not supported yet -- v0.2 only instantiates a bare type-parameter position, matching D-105's own fixed-container-element-type restriction"
                ),
                span,
            ));
        }
        elements.push(element);
    }
    let mut elements = elements.into_iter();
    let ty = match family {
        "list" => Ty::List(Box::new(elements.next().expect("arity checked above"))),
        "set" => Ty::Set(Box::new(elements.next().expect("arity checked above"))),
        "dict" => {
            let key = elements.next().expect("arity checked above");
            let value = elements.next().expect("arity checked above");
            Ty::Dict(Box::new((key, value)))
        }
        _ => Ty::Tuple(Box::new(elements.collect())),
    };
    crate::container::check_container_ty(&ty, span)?;
    Ok(ty)
}

/// Resolves an annotation expression to a `Ty`. `aliases` is the D-135 type
/// alias table (`(name, Ty)` pairs recorded by `module::lower_all` for every
/// `type X = ...`/legacy `X: TypeAlias = ...` statement reached so far, in
/// source order): checked as the last resort for a bare name before falling
/// through to the `C0001` "not supported yet" catch-all, so an alias name
/// resolves exactly like any other recognized bare-name annotation.
///
/// `class_name` is the enclosing class's name when lowering a method's
/// annotations (PEP 673 `Self` and PEP 649/749 self-referential deferred
/// annotations, #387): `Some(name)` makes both `"Self"` and the class's own
/// name resolve to `Ty::Instance(Box::new(name))` — the same type `self`
/// has. `None` for top-level functions and all other annotation contexts
/// (module-level `AnnAssign`, type aliases), where `"Self"` and a bare class
/// name remain unrecognized (C0001), matching CPython's own scope rule that
/// `Self` is only valid inside a class body.
///
/// `class_defs` is the projected slice of already-defined classes
/// (#380, PR-20): a bare name matching a known class resolves to
/// `Ty::Instance` (or `Ty::Protocol` if the class is a protocol). This
/// fixes the pre-existing gap where cross-class annotations
/// (`def f(a: A) -> int` where `A` is a user-defined class) produced `C0001`.
/// Checked after the builtin/`Self`/self-referential arms and before the
/// alias table, so a class name takes priority over an alias of the same
/// name (matching Python's own scope rule where class definitions bind the
/// class name in the enclosing namespace).
pub(crate) fn annotation_to_ty(
    annotation: &Expr,
    type_param: Option<&str>,
    class_name: Option<&str>,
    aliases: &[(String, Ty)],
    class_defs: &[ClassAnnotationInfo],
) -> Result<Ty, Diagnostic> {
    match annotation {
        Expr::NoneLiteral(_) => Ok(Ty::None),
        Expr::Name(name) if Some(name.id.as_str()) == type_param => {
            Ok(Ty::Param(Box::new(name.id.to_string())))
        }
        // PEP 673 (#387): `Self` inside a class method's annotation resolves
        // to the enclosing class's instance type — the same type `self` has.
        // Outside a class (`class_name` is `None`), `"Self"` falls through to
        // the alias/C0001 path below, matching CPython's own scoping rule.
        // #911 (Part 1 of #885): `ClassVar` is a *class-body-only* annotation
        // wrapper -- PEP 526 defines it as "this name is a class variable,
        // not an instance one", which is meaningless on a parameter, a
        // return type, or a local `AnnAssign`. It is deliberately **not**
        // unwrapped here the way `Final`/`Annotated` are: silently accepting
        // `def f(x: ClassVar[int])` would make the spelling look supported
        // when it carries no meaning at all. `pycc_hir::class::body` strips
        // the wrapper before calling this function, so the class-body
        // position never reaches this arm.
        Expr::Name(name) if name.id.as_str() == "ClassVar" => Err(unsupported(
            "`ClassVar` is only valid on a class-body attribute declaration \
             (`X: ClassVar[int] = 1` inside a `class` body), and takes exactly \
             one type argument",
            pycc_ast::expr_range(annotation),
        )),
        Expr::Name(name) if name.id.as_str() == "Self" && class_name.is_some() => {
            Ok(Ty::Instance(Box::new(class_name.unwrap().to_string())))
        }
        // PEP 649/749 (#387): a method's return-type annotation may reference
        // the enclosing class's own name (self-referential deferred
        // annotation, e.g. `class Node: def next(self) -> Node: ...`). Inside
        // a class body (`class_name` is `Some`), the class's own name resolves
        // to `Ty::Instance(class_name)`. This is specifically for the
        // self-referential case — cross-class references are not in scope.
        Expr::Name(name) if Some(name.id.as_str()) == class_name => {
            Ok(Ty::Instance(Box::new(name.id.to_string())))
        }
        Expr::Name(name) => match name.id.as_str() {
            "int" => Ok(Ty::Int),
            "float" => Ok(Ty::Float),
            "bool" => Ok(Ty::Bool),
            "str" => Ok(Ty::Str),
            "Any" => Err(Diagnostic::error(
                "T0002",
                "`Any` is not permitted in pycc code outside a declared interop boundary"
                    .to_string(),
                Span::new(0, 0),
            )),
            other => {
                // #380 (PR-20): resolve a bare name matching a known class
                // to `Ty::Instance` (or `Ty::Protocol` if the class is a
                // protocol). This fixes the pre-existing gap where
                // cross-class annotations (`def f(a: A) -> int` where `A`
                // is a user-defined class) produced `C0001`. Checked before
                // the alias table so a class name takes priority over an
                // alias of the same name.
                if let Some(info) = class_defs.iter().find(|info| info.name == other) {
                    if info.is_protocol {
                        return Ok(Ty::Protocol(Box::new(other.to_string())));
                    }
                    return Ok(Ty::Instance(Box::new(other.to_string())));
                }
                aliases
                    .iter()
                    .rev()
                    .find(|(alias_name, _)| alias_name == other)
                    .map(|(_, ty)| ty.clone())
                    .ok_or_else(|| {
                        // D-227 (issue #918): a *bare* builtin container name
                        // gets its own message naming the parameterized form
                        // that now works. Reached only here, after a
                        // user-defined class and the alias table have both
                        // had their chance, so `class list:` and
                        // `type list = ...` still win.
                        if let Some(example) = bare_container_example(other) {
                            return unsupported(
                                crate::module::bare_container_annotation_message(other, example),
                                pycc_ast::expr_range(annotation),
                            );
                        }
                        // The message is built in `module` so #867's cascade
                        // classifier can parse it back (D-219).
                        unsupported(
                            crate::module::unknown_annotation_name_message(other),
                            pycc_ast::expr_range(annotation),
                        )
                    })
            }
        },
        // Issue #435 (Part D, __class_getitem__): `ClassName[type_arg]` as a
        // type annotation (PEP 560). A class that defines `__class_getitem__`
        // allows subscript syntax in annotations. In pycc's static type
        // system, this resolves to `Ty::Instance(ClassName)` — the class
        // itself, ignoring the type argument for now (consistent with how
        // generic classes are handled by PEP 695's `GenericClassInstantiate`
        // for actual instantiation, not annotation). The base must be a bare
        // name (a class name); any other subscript shape is rejected.
        //
        // PEP 593 (#383): `Annotated[X, ...]` is recognized as a bare name
        // (no `from typing import Annotated` required, matching the existing
        // `TypeAlias`/`Any` precedent) and unwrapped to `X`, discarding all
        // metadata arguments. Per PEP 593's own spec, a static type checker
        // that does not understand a piece of metadata must treat
        // `Annotated[X, ...]` as `X` — this is correct, not a shortcut. The
        // first subscript argument is `X`; for the tuple form
        // `Annotated[X, meta1, meta2, ...]` the first element is `X`.
        // PEP 593 requires at least two arguments (the type and at least one
        // metadata element); `Annotated[X]` without metadata is rejected,
        // matching CPython's own `TypeError`.
        Expr::Subscript(sub) => {
            let Expr::Name(base_name) = sub.value.as_ref() else {
                return Err(unsupported(
                    "a subscripted type annotation's base must be a bare class name",
                    pycc_ast::expr_range(&sub.value),
                ));
            };
            match base_name.id.as_str() {
                // #911: see the bare-`ClassVar` arm above -- `ClassVar[T]`
                // is legal only on a class-body attribute declaration, where
                // `pycc_hir::class::body::strip_class_var` removes it before
                // this function ever sees it.
                "ClassVar" => Err(unsupported(
                    "`ClassVar` is only valid on a class-body attribute declaration \
                     (`X: ClassVar[int] = 1` inside a `class` body)",
                    pycc_ast::expr_range(annotation),
                )),
                "Annotated" => {
                    let Expr::Tuple(tuple) = sub.slice.as_ref() else {
                        return Err(unsupported(
                            "Annotated requires at least two arguments: the type and at least one metadata element",
                            pycc_ast::expr_range(&sub.slice),
                        ));
                    };
                    if tuple.elts.len() < 2 {
                        return Err(unsupported(
                            "Annotated requires at least two arguments: the type and at least one metadata element",
                            pycc_ast::expr_range(&sub.slice),
                        ));
                    }
                    annotation_to_ty(&tuple.elts[0], type_param, class_name, aliases, class_defs)
                }
                // PEP 591 (#383): `Final[X]` unwraps to `X`. `Final` is a
                // binding-level property (this name may not be reassigned),
                // not a type-level property — the type is just `X`. The
                // non-reassignability is tracked separately by the type
                // checker's `Environment.finals` set, populated from
                // `HirStmt::AnnAssign`'s `is_final` flag (set at lowering
                // time in `stmt.rs`). `Final` takes exactly one type
                // argument; `Final[X, Y]` is rejected.
                "Final" => {
                    let x = match sub.slice.as_ref() {
                        Expr::Tuple(tuple) if tuple.elts.len() != 1 => {
                            return Err(unsupported(
                                "Final takes exactly one type argument",
                                pycc_ast::expr_range(&sub.slice),
                            ));
                        }
                        Expr::Tuple(tuple) => &tuple.elts[0],
                        other => other,
                    };
                    annotation_to_ty(x, type_param, class_name, aliases, class_defs)
                }
                // The class name resolves the same way a bare-name annotation
                // does — through the alias table or as a known class name. We
                // reuse `annotation_to_ty` on the bare name so self-referential
                // class names, aliases, and builtin types all resolve
                // identically.
                //
                // PEP 560 (#611): before that, reject a subscript on a known
                // class that is not subscriptable. CPython raises
                // `TypeError: type 'C' is not subscriptable` for the same
                // program, and pycc's own value-position path (#610) already
                // reports it as `T0044` through `t0044_unknown_member`, so
                // this arm reuses that code rather than the surrounding
                // `C0001`. A name that is *not* a known class — a builtin
                // like `list`, an alias, or an undefined name — is left to
                // the recursion below, which reports it exactly as it does
                // today.
                _ => {
                    let known_class = class_defs
                        .iter()
                        .find(|info| info.name == base_name.id.as_str());
                    if let Some(info) = known_class {
                        if !info.subscriptable {
                            return Err(Diagnostic::error(
                                "T0044",
                                format!(
                                    "class `{}` does not define `__class_getitem__`, so \
                                     `{}[...]` is not a valid type annotation",
                                    info.name, info.name
                                ),
                                {
                                    let range = pycc_ast::expr_range(annotation);
                                    Span::new(range.start, range.end)
                                },
                            ));
                        }
                        // Issue #693 (PEP 560): when the class's `__class_getitem__`
                        // hook has a resolvable declared return type, the
                        // annotation resolves to *that* type -- matching
                        // `pycc_types::resolve_static_or_class_method_call`'s
                        // identical use of the hook's declared return type for
                        // value-position `C[x]` (#610) -- rather than to
                        // `Ty::Instance(ClassName)`. `class_getitem_return` is
                        // `None` when subscriptability comes only from a PEP 695
                        // type parameter with no explicit hook (that case is
                        // handled by `GenericClassInstantiate`, not here), from
                        // the self-referential entry `lower_class` pushes for the
                        // class it is currently lowering, or when the hook has no
                        // explicit return annotation -- `class_getitem_return_ty`
                        // deliberately treats a raw, pre-inference `Ty::Infer` as
                        // unresolved rather than propagating it here, since this
                        // crate never runs its own inference pass (see
                        // `lower_method`'s doc comment). In every such case the
                        // fallback below is unchanged from before this issue.
                        if let Some(return_ty) = &info.class_getitem_return {
                            return Ok(return_ty.clone());
                        }
                    }
                    // D-227 (issue #918): the four builtin container types are
                    // lowered here, from a *parameterized* annotation only --
                    // the bare `list`/`dict`/`set`/`tuple` spelling still falls
                    // through the `Expr::Name` arm's `other =>` branch and gets
                    // its own `C0001`.
                    //
                    // Deliberately checked *after* the known-class lookup above
                    // and gated on the alias table: `class list:` and
                    // `type list = ...` both legally shadow the builtin in
                    // Python, and silently retyping `x: list[int]` as a builtin
                    // list when the user defined their own `list` would be a
                    // miscompile, not merely a worse diagnostic. (The plan for
                    // #918 proposed a dedicated match arm ahead of this one,
                    // which would have shadowed both; the ordering here costs
                    // nothing and keeps the user's own definition winning.)
                    if known_class.is_none()
                        && CONTAINER_ANNOTATION_NAMES.contains(&base_name.id.as_str())
                        && !aliases.iter().any(|(name, _)| name == base_name.id.as_str())
                    {
                        return container_annotation_to_ty(
                            base_name.id.as_str(),
                            sub.slice.as_ref(),
                            annotation,
                            type_param,
                            class_name,
                            aliases,
                            class_defs,
                        );
                    }
                    annotation_to_ty(
                        &Expr::Name(base_name.clone()),
                        type_param,
                        class_name,
                        aliases,
                        class_defs,
                    )
                }
            }
        }
        // `T | None` / `None | T` (PEP 604, D-197, #763, Part 1 of #747):
        // accept exactly the 2-operand shape where one side is
        // `Expr::NoneLiteral`, recursing into the other side so
        // `list[int] | None`, `SomeClass | None`, and `T | None` (a generic
        // param) all parse for free through their own existing arms above.
        // A 2-operand union where *neither* side is `None` (`int | str`), or
        // any chain (`ops.len() != 1`, e.g. `A | B | None`), is a general
        // union: explicitly out of scope for this PR (Part 2+), rejected
        // with `T0048` rather than silently misparsed or falling through to
        // the generic catch-all below.
        Expr::BinOp(bin_op) if bin_op.op == Operator::BitOr => {
            let other_side = match (bin_op.left.as_ref(), bin_op.right.as_ref()) {
                (Expr::NoneLiteral(_), other) => other,
                (other, Expr::NoneLiteral(_)) => other,
                _ => {
                    let range = std::ops::Range::<u32>::from(bin_op.range);
                    return Err(Diagnostic::error(
                        "T0048",
                        "general union annotations (`X | Y` where neither side is `None`) are not supported yet -- only `T | None` (PEP 604 Optional) is",
                        Span::new(range.start, range.end),
                    ));
                }
            };
            let inner = annotation_to_ty(other_side, type_param, class_name, aliases, class_defs)?;
            // `Optional[T]` is only supported for `T` in `{int, float, bool}`
            // (D-197, #763, Part 1 of #747; widened to `float`/`bool` by
            // #809, Part 2): codegen's `{ inner, i8 }` representation
            // (`crates/pycc_codegen/src/lib.rs`'s `ty_to_basic_type`) and
            // every downstream `Scalar`/emit site are exercised and tested
            // for these three scalar inner types only. Refcounted/pointer
            // inner types (`str` and friends) and general `A | B` unions
            // stay out of scope, mirroring `list[int]`'s own `T0034` scope
            // cut (D-105/D-122). Gated here, pre-lowering, at the one place
            // a `Ty::Optional` is ever constructed from source, so nothing
            // else in the pipeline needs to re-derive this check: a
            // `Ty::Optional` reaching `pycc_types`/`pycc_mir`/`pycc_codegen`
            // always wraps one of `Ty::Int`, `Ty::Float`, or `Ty::Bool`
            // starting from this return.
            if !matches!(inner, Ty::Int | Ty::Float | Ty::Bool) {
                let range = std::ops::Range::<u32>::from(bin_op.range);
                return Err(Diagnostic::error(
                    "T0049",
                    format!(
                        "`Optional[{}]` is not supported yet -- only `Optional[int]`, `Optional[float]`, and `Optional[bool]` (`int | None`, `float | None`, `bool | None`) are",
                        inner.name()
                    ),
                    Span::new(range.start, range.end),
                ));
            }
            Ok(Ty::Optional(Box::new(inner)))
        }
        other => Err(unsupported(
            format!(
                "only a bare name type annotation is supported so far, got {}",
                pycc_ast::expr_kind_name(other)
            ),
            pycc_ast::expr_range(other),
        )),
    }
}
