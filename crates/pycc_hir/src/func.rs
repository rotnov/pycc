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

mod bare_container;
pub(crate) mod params;
#[cfg(test)]
mod params_tests;

use crate::class::ClassAnnotationInfo;
use crate::expr::keyword_bind::SignatureTable;
use crate::{HirItem, ImportBinding, Ty, stmt, unsupported};
use bare_container::{CONTAINER_ANNOTATION_NAMES, bare_container_example};
pub(crate) use bare_container::{with_bare_container_advice, with_bare_list_or_dict_advice};
use params::DefaultPolicy;
use pycc_ast::{Expr, Operator};
use pycc_diag::{Diagnostic, Span};

pub(crate) fn lower_function(
    def: &pycc_ast::StmtFunctionDef,
    aliases: &[(String, Ty)],
    class_defs: &[ClassAnnotationInfo],
    imports: &[ImportBinding],
    signatures: &SignatureTable,
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
    let is_public = crate::is_public_name(def.name.as_str()); // D-038
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
        imports,
        signatures,
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
    // Every parameter kind below is silently absent from
    // `parameters.args` -- an earlier version of this function only ever
    // iterated `.args` and never checked for any of these, so a function
    // using them got a wrong signature built from whatever plain positional
    // args happened to exist, instead of the explicit capability diagnostic
    // every other out-of-scope construct in this file produces (self-review
    // finding, pre-merge). The checks themselves now live in `params`,
    // shared with `class::lower_method`; a *default value* is no longer one
    // of them -- Part 2 of #884 (#1189) implements it for this path, and
    // `lower_arg_list`'s `DefaultPolicy` below is what keeps every other
    // path rejecting it.
    //
    // PEP 570 (#383): positional-only parameters (`posonlyargs`, before the
    // `/` marker) are lowered via the same `lower_arg_list` path as ordinary
    // `args`, prepended before `args` in the parameter list — so index `i`
    // of the returned vector is index `i` of `HirExpr::Call::args`.
    //
    // Part 1 of #884 (#1125) made that ordering load-bearing. Keyword call
    // arguments are no longer globally unsupported: a call to a module-level
    // `def` may now name a parameter, so the two parameter kinds are no
    // longer interchangeable at the call site. `expr::keyword_bind`
    // reproduces this exact concatenation order when it collects a
    // signature, and excludes the leading `posonlyargs` entries from the set
    // a keyword may name — naming one is a `T0021`, as in CPython. A
    // parameter after the `/` marker stays bindable by name.
    params::reject_unsupported_parameter_shapes(parameters)?;
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
        DefaultPolicy::Admit,
    )?;
    params.extend(lower_arg_list(
        &parameters.args,
        is_public,
        fn_name,
        type_param,
        None,
        aliases,
        class_defs,
        DefaultPolicy::Admit,
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
///
/// `policy` decides what a default value means here (Part 2 of #884, #1189).
/// Under [`DefaultPolicy::Reject`] -- every method and protocol-member
/// caller -- the pre-existing capability check runs unchanged and *before*
/// annotation resolution, so a method parameter whose annotation would
/// itself be rejected still reports the capability message first. Under
/// [`DefaultPolicy::Admit`] -- `lower_params`' two calls, and nothing else
/// -- the annotation is resolved first and `params::check_default` then
/// applies every default rule.
#[allow(clippy::too_many_arguments)]
pub(crate) fn lower_arg_list(
    args: &[pycc_ast::ParameterWithDefault],
    is_public: bool,
    fn_name: &str,
    type_param: Option<&str>,
    class_name: Option<&str>,
    aliases: &[(String, Ty)],
    class_defs: &[ClassAnnotationInfo],
    policy: DefaultPolicy,
) -> Result<Vec<(String, Ty)>, Diagnostic> {
    args.iter()
        .map(|param| {
            if policy == DefaultPolicy::Reject && param.default.is_some() {
                return Err(unsupported(
                    "default parameter values are not supported yet",
                    param.range,
                ));
            }
            let name = param.parameter.name.as_str();
            let ty = match &param.parameter.annotation {
                Some(ann) => annotation_to_ty(ann, type_param, class_name, aliases, class_defs)
                    .map_err(|error| with_bare_container_advice(error, ann))?,
                None if is_public => {
                    return Err(Diagnostic::error(
                        "T0001",
                        format!(
                            "parameter `{name}` of public function `{fn_name}` needs a type annotation"
                        ),
                        Span::new(0, 0),
                    )
                    .with_help(format!("add a type annotation to parameter `{name}`")));
                }
                None => Ty::Infer,
            };
            if let Some(default) = param.default.as_deref() {
                params::check_default(default, name, fn_name, &ty)?;
            }
            Ok((name.to_string(), ty))
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
            // Return position lowers a parameterized container annotation
            // exactly like every other position (#925, Part 2 of #918).
            // D-228 (Part 1) deliberately excluded it while a container-typed
            // call result still reached an unhandled codegen case; #925 added
            // the codegen arms that closed that gap, so that exclusion is
            // gone. Element-type and arity gates still fire on this
            // annotation's own span, because they run inside
            // `annotation_to_ty` below.
            let ty = annotation_to_ty(ann, type_param, class_name, aliases, class_defs)
                .map_err(|error| with_bare_container_advice(error, ann))?;
            // #934: the one return-position check that remains. A protocol
            // is a compile-time-only interface with no runtime
            // representation (D-166); a protocol-typed *parameter* or
            // *variable* is bound to the concrete class of the value it
            // receives (monomorphization, `pycc_mir`'s binding of the
            // inferred type), but a call to a `-> P` function has no
            // concrete type to bind, so every shape of such a function
            // used to type-check and then abort inside `pycc_mir` or
            // `pycc_codegen`. Rejecting the annotation here closes all of
            // them at once: this function is the single seam for a
            // module-level function, a method, and a protocol member
            // declaration. The check runs *after* `annotation_to_ty` so a
            // `-> list[P]` still reports D-105's `T0034` first, exactly as
            // D-228 pins for containers; `-> P | None` is `T0049` for the
            // same reason, so only a top-level `Ty::Protocol` reaches here.
            if let Ty::Protocol(protocol) = &ty {
                return Err(unsupported(
                    format!(
                        "a protocol class (`{protocol}`) as a return type annotation is not \
                         supported yet -- a protocol type is currently supported in parameter \
                         and variable positions only"
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

/// The six names [`annotation_to_ty`]'s `Expr::Name` arm answers *before* it
/// consults `class_defs` or the alias table **and that the `Expr::Subscript`
/// arm can reach**. Canonical statement of the precedence rule D-244
/// statements (h) and (k) describe; every site that dispatches on it derives
/// from here rather than restating the list. `ClassVar` is answered before
/// both tables too but is deliberately absent: the subscript arm intercepts
/// `ClassVar[...]` at the top of its own match, so it never reaches this
/// helper. The carrier's two ordinary-identifier spellings `ndarray` (#1129)
/// and `NDArray` (#1134) are deliberately absent too -- both are ordinary
/// identifiers resolved *after* both tables (D-244 statement (h)).
fn name_resolves_before_class_defs(base: &str) -> bool {
    matches!(
        base,
        "int" | "float" | "bool" | "str" | "Any" | "memoryview"
    )
}

/// The noun for [`annotation_to_ty`]'s non-class `T0044` (#931): what the
/// base of a subscripted annotation resolved to, when it is neither a class
/// nor an alias to one. The arms follow the **same precedence the
/// `Expr::Name` arm resolves a bare name in**, so the noun always agrees with
/// what the recursion that precedes the reject actually resolved: a type
/// parameter first, then `Self` inside a class, then the enclosing class's
/// own name, then the builtin scalars, and otherwise a `type` alias. Only the
/// *precedence* is mirrored, not the resolved `Ty`: since #948 a self-reference
/// inside a protocol body resolves to `Ty::Protocol` rather than
/// `Ty::Instance`, but the recursion's `Ok` value is discarded here and the
/// noun is keyed on the spelling alone, so the wording is unaffected.
///
/// The `class_name` arm is defensive: every current caller that passes
/// `class_name` also has that class in `class_defs` (`lower_class` pushes the
/// self-referential entry before lowering the body), so the known-class
/// ladder catches it first. The arm exists so a future caller cannot make
/// the noun say "type alias" for a class.
pub(crate) fn subscripted_base_description(
    base: &str,
    type_param: Option<&str>,
    class_name: Option<&str>,
) -> String {
    if Some(base) == type_param {
        format!("type parameter `{base}`")
    } else if base == "Self" && class_name.is_some() {
        "`Self`".to_string()
    } else if Some(base) == class_name {
        format!("class `{base}`")
    } else {
        match base {
            "int" | "float" | "bool" | "str" => format!("builtin type `{base}`"),
            _ => format!("type alias `{base}`"),
        }
    }
}

/// Lowers a parameterized builtin container annotation -- `list[T]`,
/// `set[T]`, `dict[K, V]` or `tuple[A, B, ...]` -- to its `Ty` (D-228,
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
        // The advice is per family. `tuple[X, ...]` is the one spelling that
        // means something in Python -- a homogeneous variadic tuple -- so it
        // gets the length explanation and a fixed-arity `tuple`. For
        // `list`/`set`/`dict`, `...` is simply not a type, and recommending a
        // `tuple` there would change the container the user asked for.
        //
        // The advice is split into the reason and the imperative fix, because
        // the fix is also published as structured `help` (D-152's "the message
        // already embeds the fix" family): the message keeps the whole
        // sentence, while `help` carries the imperative alone so a JSON or IDE
        // consumer reads an instruction rather than a restatement.
        let (advice, help) = if family == "tuple" {
            let help = "write an explicit fixed-arity annotation such as `tuple[int, int]`";
            (
                format!(
                    "a homogeneous-variadic container has no compile-time length, so {help} instead"
                ),
                help.to_string(),
            )
        } else {
            let example = bare_container_example(family)
                .expect("`family` is one of `CONTAINER_ANNOTATION_NAMES`");
            let help = format!("write the element type, e.g. `{example}`");
            (format!("`...` is not a type argument here; {help}"), help)
        };
        return Err(Diagnostic::error(
            "T0053",
            format!("the `...` type argument in `{family}[...]` is not supported yet -- {advice}"),
            span,
        )
        .with_help(help));
    }
    let exact_arity = match family {
        "list" | "set" | "frozenset" => Some(1usize),
        "dict" => Some(2usize),
        // `tuple` is variadic in arity: any count of one or more.
        _ => None,
    };
    match exact_arity {
        Some(expected) if args.len() != expected => {
            let example = bare_container_example(family)
                .expect("`family` is one of `CONTAINER_ANNOTATION_NAMES`");
            return Err(Diagnostic::error(
                "T0053",
                format!(
                    "container type annotation `{family}[...]` takes exactly {expected} type argument{}, got {}",
                    if expected == 1 { "" } else { "s" },
                    args.len()
                ),
                span,
            )
            .with_help(format!(
                "write exactly {expected} type argument{}, e.g. `{example}`",
                if expected == 1 { "" } else { "s" }
            )));
        }
        None if args.is_empty() => {
            return Err(Diagnostic::error(
                "T0053",
                "container type annotation `tuple[...]` takes at least 1 type argument -- the empty tuple `tuple[()]` is not supported yet".to_string(),
                span,
            )
            .with_help("write at least one element type, e.g. `tuple[int]`"));
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
        "frozenset" => Ty::FrozenSet(Box::new(elements.next().expect("arity checked above"))),
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

/// The type an annotation naming the enclosing class resolves to.
///
/// Both self-reference spellings -- PEP 673 `Self` and PEP 649/749's bare use
/// of the class's own name ([#387](https://github.com/rotnov/pycc/issues/387))
/// -- denote the enclosing class, so both go through this helper. A protocol
/// enclosing class yields `Ty::Protocol`, exactly as the cross-class
/// `class_defs` lookup in `annotation_to_ty`'s general `Expr::Name` arm does
/// for any other protocol name ([#948](https://github.com/rotnov/pycc/issues/948));
/// every other class yields `Ty::Instance`, the type `self` has.
fn enclosing_class_ty(class_name: &str, class_defs: &[ClassAnnotationInfo]) -> Ty {
    if class_defs
        .iter()
        .any(|info| info.name == class_name && info.is_protocol)
    {
        return Ty::Protocol(Box::new(class_name.to_string()));
    }
    Ty::Instance(Box::new(class_name.to_string()))
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
/// name resolve through `enclosing_class_ty` — `Ty::Instance(Box::new(name))`
/// for an ordinary class, the type `self` has, and `Ty::Protocol` when the
/// enclosing class is itself a protocol (#948). `None` for top-level functions
/// and all other annotation contexts
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
        // to the enclosing class's own type — `Ty::Instance`, the type `self`
        // has, for an ordinary class. Inside a *protocol* body it resolves to
        // `Ty::Protocol` instead (#948): `self` itself stays `Ty::Instance`
        // there, but the annotation denotes the protocol, so the two diverge
        // and a `-> Self` member return reaches #934's `C0001` gate exactly as
        // the bare `-> P` spelling does. Outside a class (`class_name` is
        // `None`), `"Self"` falls through to the alias/C0001 path below,
        // matching CPython's own scoping rule.
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
            Ok(enclosing_class_ty(class_name.unwrap(), class_defs))
        }
        // PEP 649/749 (#387): a method's return-type annotation may reference
        // the enclosing class's own name (self-referential deferred
        // annotation, e.g. `class Node: def next(self) -> Node: ...`). Inside
        // a class body (`class_name` is `Some`), the class's own name resolves
        // to `Ty::Instance(class_name)` — or to `Ty::Protocol(class_name)`
        // when the enclosing class is a protocol (#948), matching what the
        // general `Expr::Name` arm below resolves any *other* protocol name
        // to. This is specifically for the self-referential case —
        // cross-class references are not in scope.
        Expr::Name(name) if Some(name.id.as_str()) == class_name => {
            Ok(enclosing_class_ty(name.id.as_str(), class_defs))
        }
        Expr::Name(name) => match name.id.as_str() {
            "int" => Ok(Ty::Int),
            "float" => Ok(Ty::Float),
            "bool" => Ok(Ty::Bool),
            "str" => Ok(Ty::Str),
            // Part 1 of #1027: a one-dimensional `float` `memoryview`, the
            // carrier a `pycc build --ext` boundary admits so a host can
            // hand compiled code a NumPy-shaped array without copying it.
            // Lowered unconditionally here, in both parameter and return
            // position, because `pycc_hir` has no artifact-mode awareness
            // at all -- the two mode-dependent refusals live where the mode
            // is known: `src/ext_build.rs` refuses a `memoryview` *return*
            // with `C0003`, and `src/memoryview_mode.rs` refuses a
            // `memoryview` *signature* with `I0405` in a native build.
            //
            // This arm is also the parser of a bare `x: memoryview`
            // declaration, which neither mode-dependent refusal reaches --
            // both walk a `HirItem::Function`'s signature, not a statement
            // body. `crates/pycc_types`'s `reject_memoryview_declaration`
            // owns that position, in both modes.
            "memoryview" => Ok(Ty::MemoryView),
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
                if let Some(ty) = aliases
                    .iter()
                    .rev()
                    .find(|(alias_name, _)| alias_name == other)
                    .map(|(_, ty)| ty.clone())
                {
                    return Ok(ty);
                }
                // #1129/#1134: `ndarray` and `NDArray` are *further
                // spellings* of the same pycc type, not new ones. All three
                // mean "a one-dimensional, C-contiguous, format `'d'` buffer
                // exporter", which is exactly what
                // `pycc_ext_unpack_memoryview` enforces at the boundary, so
                // the three spellings have no run-time observable difference
                // and a distinct `Ty` variant would carry information no
                // consumer could read. Diagnostics therefore render the
                // canonical `memoryview` for any spelling, which is already
                // what the alias table does for `type Arr = memoryview`. `NDArray` (#1134) is the
                // capitalized `numpy.typing` spelling 18 of the 19
                // array-parameter occurrences in the #1039 census use; it
                // joins the set on exactly the terms `ndarray` did, and
                // registering it does not by itself make any of those
                // occurrences compile -- each also needs a name binding
                // that does not exist yet (`from numpy.typing import
                // NDArray` is refused by the foreign-import path, `import
                // numpy as np` is #883, and an attribute-form base
                // `np.ndarray` is #889).
                //
                // Recognized with **no import**, deliberately:
                // `annotation_to_ty` receives `type_param`, `class_name`,
                // `aliases` and `class_defs` and no import table at all, and
                // `import numpy` builds only from a `pycc.lock` closure
                // (#1242), so requiring one would be new machinery gating a
                // spelling on an installed numpy. `Any`, `Annotated`,
                // `TypeAlias` and `Self` are all recognized on those terms.
                //
                // Both are resolved *here* rather than beside `memoryview`
                // in the keyword list above, and that placement is the rule
                // rather than a detail: every name in that list is a Python
                // builtin or a `typing` name, while `ndarray` and `NDArray`
                // are ordinary identifiers a program may bind itself.
                // Reserving one ahead of `class_defs` and `aliases` would
                // make a module-level `class ndarray` or
                // `type NDArray = ...` mean something Python does not -- in
                // Python a local definition shadows an imported name, not
                // the other way round -- and measurably refused programs
                // that compiled before the spelling existed. The user's own
                // definition therefore wins, and the buffer carrier is what
                // a name nothing else binds falls back to. That is why
                // neither spelling appears in `name_resolves_before_class_defs`
                // nor, through it, in the subscript arm's own
                // `name_resolves_before_aliases` ladder.
                if matches!(other, "ndarray" | "NDArray") {
                    return Ok(Ty::MemoryView);
                }
                Err({
                    // The message is built in `module` so #867's cascade
                    // classifier can parse it back (D-219).
                    //
                    // A *bare* builtin container name (`list`, `dict`,
                    // ...) is deliberately not special-cased here.
                    // `annotation_to_ty` has no idea which annotation
                    // position it is lowering, and the parameterized
                    // form it would advise -- `list[int]` -- is rejected
                    // in half the positions that reach this function. The
                    // generic message is correct in all of them, so the
                    // advice is opted into by the callers that can
                    // honour it, through `with_bare_container_advice`.
                    unsupported(
                        crate::module::unknown_annotation_name_message(other),
                        pycc_ast::expr_range(annotation),
                    )
                })
            }
        },
        // Issue #435 (Part D, __class_getitem__): `ClassName[type_arg]` as a
        // type annotation (PEP 560). Since #1130 a subscript whose base
        // resolves to a type pycc can name nominally is accepted whether or
        // not the class defines `__class_getitem__`: in pycc's static type
        // system it resolves to the hook's declared return type when the
        // class has one (#693), and otherwise to `Ty::Instance(ClassName)` —
        // the class itself, ignoring the type argument (consistent with how
        // generic classes are handled by PEP 695's `GenericClassInstantiate`
        // for actual instantiation, not annotation). A `type A = C` alias is
        // transparent: `A[int]` behaves exactly as `C[int]`, and so is a
        // `type Arr = memoryview` alias to the buffer carrier. The base must
        // be a bare name; any other subscript shape is rejected, and (#931,
        // narrowed by #1130) a bare name that resolves to something that is
        // *not* a nameable type -- a type parameter, a builtin scalar,
        // `Self`, or an alias to a non-class, non-carrier `Ty` -- is
        // rejected with `T0044` rather than having its type argument
        // silently discarded (see the `_ =>` arm).
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
                // PEP 560 (#611, reversed in annotation position by #1130):
                // a subscript on a known class used to be rejected unless
                // the class defined `__class_getitem__`. It no longer is.
                // CPython 3.14 evaluates annotations lazily (PEP 649/749),
                // so `class C: pass` + `def f(a: C[int])` raises no
                // `TypeError` at definition time -- that error surfaces only
                // through `typing.get_type_hints`, and pycc builds no
                // runtime `__annotations__` object for any function, class
                // or module, so no pycc-compiled program can reach it.
                // Accepting the annotation therefore moves toward CPython
                // 3.14's observable behavior, not away from it. **Value
                // position is the opposite and is deliberately unchanged**:
                // `x = C[int]` is evaluated eagerly and really does raise
                // `TypeError: type 'C' is not subscriptable`, so
                // `pycc_types`' value-position `C[x]` path (#610) computes
                // its own subscriptability answer and keeps reporting
                // `T0044`. The resulting asymmetry is CPython's own, not an
                // invention of this change. The forfeiture #1130 accepts in
                // exchange: `C[int]` where `C` is a genuinely non-generic
                // user class is an ordinary typo that #611 caught and that
                // pycc now accepts silently.
                //
                // #931 rejects every resolvable base that is *not* a
                // nameable type: a PEP 695 type parameter (`T[int]`), a
                // builtin scalar (`int[str]`), `Self` inside a class, and a
                // `type` alias to a non-class, non-carrier type. Each of
                // those used to fall through to the bare-name recursion
                // below, which resolved the base and silently discarded the
                // type argument. CPython reports all of them with the same
                // `TypeError: ... is not subscriptable`, so they share the
                // `T0044` code and differ only in the noun
                // (`subscripted_base_description`). #1130 carves the buffer
                // carrier out of the alias half of that list -- see step 4.
                //
                // Two bases keep their pre-#931 diagnostic on purpose: an
                // undefined name still gets the exact `C0001` that
                // `module::cascade_name` parses back (D-219), and `Any`
                // still gets `T0002`. Both come out of the final recursion
                // on the bare base before the reject can fire.
                _ => {
                    let base = base_name.id.as_str();
                    let range = pycc_ast::expr_range(annotation);
                    // Step 1: resolve the class the base denotes -- directly,
                    // or through a `type A = C` alias, since PEP 695 aliases
                    // are transparent and `A[int]` must behave exactly as
                    // `C[int]`. This deliberately consults the alias *table*
                    // rather than recursing `annotation_to_ty` on the bare
                    // name: `Self` and the enclosing class's own name also
                    // resolve to `Ty::Instance` through the `Expr::Name` arm,
                    // and the alias path must not be how a class is reached
                    // for them (the self-referential `class_defs` entry
                    // `lower_class` pushes is).
                    //
                    // The alias table is consulted only when the `Expr::Name`
                    // arm would itself reach it. A type parameter, `Self`
                    // inside a class, the enclosing class's own name, the
                    // builtin scalar names and `Any` all resolve *before*
                    // `class_defs` and the alias table there, so an alias
                    // that happens to share such a name must not win here
                    // either (`type int = C` + bare `x: int` is `Int`; `type
                    // Any = C` + `Any[str]` is `T0002`; both stay that way).
                    //
                    // #1129: `memoryview` is on that list for the same reason
                    // -- it is a reserved keyword the `Expr::Name` arm answers
                    // before either table, so `type memoryview = C` never
                    // makes the name mean `C`, and `memoryview[...]` must not
                    // be reported against `C` either. Its sibling spellings
                    // `ndarray` (#1129) and `NDArray` (#1134) are deliberately
                    // absent: those are ordinary identifiers resolved *after*
                    // both tables, so an alias of either name genuinely does
                    // win here.
                    let name_resolves_before_aliases = Some(base) == type_param
                        || (base == "Self" && class_name.is_some())
                        || Some(base) == class_name
                        || name_resolves_before_class_defs(base);
                    let alias_target = if name_resolves_before_aliases {
                        None
                    } else {
                        aliases
                            .iter()
                            .rev()
                            .find(|(n, _)| n == base)
                            .map(|(_, ty)| ty)
                    };
                    // The *direct* class lookup is gated on the type
                    // parameter only: a type parameter shadows a same-named
                    // class in the bare-name arm, and before #931 the
                    // subscript arm disagreed with that -- `class G[U]:` +
                    // `def f[G](x: G[int])` passed the class ladder and then
                    // the ladder's final recursion resolved `G` to
                    // `Ty::Param("G")`, silently dropping `[int]`. It is NOT
                    // gated on `Self`/`class_name`: their class is the
                    // self-referential entry `lower_class` pushes, and
                    // `G[int]` inside `class G[T]`'s own body must stay
                    // accepted.
                    let known_class = if Some(base) == type_param
                        || (Some(base) != class_name && name_resolves_before_class_defs(base))
                    {
                        None
                    } else {
                        class_defs.iter().find(|info| info.name == base)
                    }
                    .or_else(|| {
                        alias_target.and_then(|ty| match ty {
                            Ty::Instance(n) | Ty::Protocol(n) => {
                                class_defs.iter().find(|info| info.name == n.as_str())
                            }
                            _ => None,
                        })
                    });
                    // Step 2: the known-class ladder (#611, #693). The first
                    // clause names the class; the trailing clause spells the
                    // *written* base, which differs from the class name when
                    // the base is an alias (`type A = C` / `x: A[int]`), so
                    // the text agrees with the caret.
                    if let Some(info) = known_class {
                        // Issue #693 (PEP 560): when the class's `__class_getitem__`
                        // hook has a resolvable declared return type, the
                        // annotation resolves to *that* type -- matching
                        // `pycc_types::resolve_static_or_class_method_call`'s
                        // identical use of the hook's declared return type for
                        // value-position `C[x]` (#610) -- rather than to
                        // `Ty::Instance(ClassName)`. `class_getitem_return` is
                        // `None` when the class is generic only through a PEP 695
                        // type parameter with no explicit hook (that case is
                        // handled by `GenericClassInstantiate`, not here), from
                        // the self-referential entry `lower_class` pushes for the
                        // class it is currently lowering, or when the hook has no
                        // explicit return annotation -- `class_getitem_return_ty`
                        // deliberately treats a raw, pre-inference `Ty::Infer` as
                        // unresolved rather than propagating it here, since this
                        // crate never runs its own inference pass (see
                        // `lower_method`'s doc comment). In every such case the
                        // annotation resolves to `Ty::Instance(ClassName)` (or
                        // `Ty::Protocol`) through the bare-name recursion, the
                        // class itself, ignoring the type argument -- the same
                        // way a class name, an alias to one, and the
                        // self-referential name all resolve as a bare
                        // annotation.
                        if let Some(return_ty) = &info.class_getitem_return {
                            return Ok(return_ty.clone());
                        }
                        return annotation_to_ty(
                            &Expr::Name(base_name.clone()),
                            type_param,
                            class_name,
                            aliases,
                            class_defs,
                        );
                    }
                    // Step 2b: an alias whose target is a class that is *not*
                    // in `class_defs` -- `from lib_a import A` where `lib_a`
                    // has `type A = G`: `import.rs`'s TypeAlias binding pushes
                    // the alias, but only the Class binding runs
                    // `copy_class_with_ancestors`, so `G` is absent here. That
                    // program was accepted before #931 and stays accepted
                    // exactly as before (fail-open on the class-like `Ty`,
                    // never a spurious reject). The missing copy is a
                    // pre-existing #881-area gap, out of scope here.
                    if matches!(alias_target, Some(Ty::Instance(_) | Ty::Protocol(_))) {
                        return annotation_to_ty(
                            &Expr::Name(base_name.clone()),
                            type_param,
                            class_name,
                            aliases,
                            class_defs,
                        );
                    }
                    // Step 3, D-228 (issue #918): the builtin container
                    // types (`frozenset` since Part 1 of #1319) are lowered
                    // here, from a *parameterized* annotation only -- the
                    // bare `list`/`dict`/`set`/`frozenset`/`tuple`
                    // spelling still falls through the `Expr::Name` arm's
                    // `other =>` branch and gets its own `C0001`.
                    //
                    // Deliberately checked *after* the known-class ladder
                    // above (which now also covers an alias to a class) and
                    // gated on the alias table: `class list:` and
                    // `type list = ...` both legally shadow the builtin in
                    // Python, and silently retyping `x: list[int]` as a builtin
                    // list when the user defined their own `list` would be a
                    // miscompile, not merely a worse diagnostic. (The plan for
                    // #918 proposed a dedicated match arm ahead of this one,
                    // which would have shadowed both; the ordering here costs
                    // nothing and keeps the user's own definition winning.)
                    //
                    // A PEP 695 type parameter shadows the builtin for the
                    // same reason a user-defined class does, and the
                    // `Expr::Name` arm above already gives `type_param` the
                    // first word: `def f[list](x: list[int])` declares `list`
                    // as a type variable, so the annotation subscripts that
                    // variable -- invalid Python, rejected below (#931) --
                    // rather than naming the builtin. Lowering it as
                    // `Ty::List(Int)` would silently drop the function's
                    // genericity.
                    if Some(base) != type_param
                        && CONTAINER_ANNOTATION_NAMES.contains(&base)
                        && !aliases.iter().any(|(name, _)| name == base)
                    {
                        return container_annotation_to_ty(
                            base,
                            sub.slice.as_ref(),
                            annotation,
                            type_param,
                            class_name,
                            aliases,
                            class_defs,
                        );
                    }
                    // Step 4 (#931, narrowed by #1130): resolve the bare base
                    // so an undefined name keeps its cascade-shaped `C0001`
                    // (D-219) and `Any` keeps `T0002`. A base that resolves
                    // here is either not a class at all -- a type parameter,
                    // `Self`, a builtin scalar, or an alias to a
                    // scalar/container/Optional/type parameter -- or it is one
                    // of the six names the `Expr::Name` arm answers before
                    // `class_defs` and a class of that name exists
                    // (`class int[T]: pass` + `def f(a: int[str])` reaches
                    // here with `int` in `class_defs`, because the gate on
                    // the direct lookup above skips it: the reserved name,
                    // not the shadow class, is what the bare form means).
                    // None of those accepts a type argument, so the subscript
                    // is rejected instead of silently discarding it -- except
                    // for the buffer carrier, which is a nameable type.
                    // #1130: the buffer carrier is a nameable type, so a
                    // subscript on it is erased exactly as a class's is --
                    // `memoryview[float]`, `ndarray[float]`,
                    // `NDArray[float]` and `type Arr = memoryview` +
                    // `Arr[float]` all lower to the carrier. Keyed on the
                    // resolved `Ty` rather than on the spelling, so a further
                    // carrier spelling is admitted by its registration alone
                    // -- which is exactly how #1134's `NDArray` reached this
                    // arm without an edit here, as #1129's `ndarray` did
                    // before it.
                    //
                    // Keying on the resolved type is safe only because the
                    // `Expr::Name` arm answers a PEP 695 type parameter
                    // (level 1) and the enclosing class's own name (level 3)
                    // *before* the `memoryview` keyword arm (level 4): a type
                    // parameter named `memoryview`, or a class named
                    // `memoryview` seen from inside its own body, resolves to
                    // `Ty::Param`/`Ty::Instance` here and the accept cannot
                    // swallow it. `Self[int]` likewise resolves to
                    // `Ty::Instance` through this same recursion and keeps
                    // #931's rejection, which is why the accept is keyed on
                    // `Ty::MemoryView` only and not on "any nominal type".
                    let resolved = annotation_to_ty(
                        &Expr::Name(base_name.clone()),
                        type_param,
                        class_name,
                        aliases,
                        class_defs,
                    )?;
                    if resolved == Ty::MemoryView {
                        return Ok(resolved);
                    }
                    Err(Diagnostic::error(
                        "T0044",
                        format!(
                            "{} is not subscriptable, so `{base}[...]` is not a valid type \
                             annotation",
                            subscripted_base_description(base, type_param, class_name)
                        ),
                        Span::new(range.start, range.end),
                    ))
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

#[cfg(test)]
mod return_annotation_tests;
