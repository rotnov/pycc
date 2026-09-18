//! Binding keyword call arguments to parameters by name (Part 1 of #884,
//! issue #1125).
//!
//! `HirExpr::Call` is purely positional: it carries `args: Vec<HirExpr>` and
//! nothing that could name a parameter. Rather than widen that node — and
//! every consumer of it in `pycc_types`, `pycc_mir` and `pycc_codegen` — this
//! module resolves `f(b=2, a=1)` to the positional vector `f(1, 2)` *during
//! lowering*, where the call site's source spans still exist. Everything
//! downstream keeps seeing a complete positional argument list and needs no
//! change at all.
//!
//! Two pieces make that possible:
//!
//! * [`SignatureTable`], collected once per module from its top-level `def`s
//!   before any item is lowered, so a call written above its own `def` binds
//!   just as well as one written below it; and
//! * [`bind_keyword_arguments`], which implements the subset of CPython's
//!   call-binding rules this part supports.
//!
//! The supported subset is deliberately narrow, and a call outside it is
//! rejected exactly as before this change (`C0001`, "keyword call arguments
//! are not supported yet") rather than being bound wrongly: only a call whose
//! callee is a bare name naming a module-level `def` whose parameters are all
//! required and positional is bindable. A method call, a `super().m()` call, a
//! container or stdlib-intrinsic call, a class instantiation and a `**kwargs`
//! unpacking all keep the old rejection — see [`is_bindable_call`].
//!
//! The four ways a bindable call can still be wrong are errors CPython itself
//! raises as `TypeError`, so they are `T0021`, not `C0001` (`docs/DIAGNOSTICS.md`).
//! `pycc_hir` already emits `T0021` with a real source span at `import.rs`
//! (D-222); `pycc_types`' positional-arity twin cannot, because HIR carries no
//! spans by the time it runs.

use std::collections::HashMap;

use pycc_ast::{Expr, ExprCall, Parameters, Stmt};
use pycc_diag::{Diagnostic, Span};

use crate::HirExpr;

/// One module-level `def`'s keyword-bindable signature.
struct Signature {
    /// Parameter names in exactly the order `func::lower_params` builds
    /// them: `posonlyargs` first, then `args`. Index `i` here is index `i`
    /// of the `HirItem::Function::params` vector, and therefore index `i`
    /// of the positional `HirExpr::Call::args` vector this module produces.
    names: Vec<String>,
    /// How many leading [`Signature::names`] entries came from
    /// `posonlyargs`. They fill positionally like any other parameter but
    /// can never be named by a keyword (PEP 570).
    posonly_count: usize,
}

/// Every top-level `def` in one module whose parameters this part can bind
/// keyword arguments against.
///
/// Absence from the table is not an error: it means "keep the pre-#1125
/// behaviour for this callee", i.e. reject a keyword call against it with
/// the unchanged `C0001` capability message.
#[derive(Default)]
pub(crate) struct SignatureTable {
    by_name: HashMap<String, Signature>,
}

impl SignatureTable {
    /// Collects the table from a module's top-level statements.
    ///
    /// A later `def` of the same name replaces an earlier one, matching
    /// Python's own rebinding of the module-level name.
    pub(crate) fn collect(body: &[Stmt]) -> Self {
        let mut by_name = HashMap::new();
        for stmt in body {
            if let Stmt::FunctionDef(def) = stmt
                && let Some(signature) = signature_of(&def.parameters)
            {
                by_name.insert(def.name.as_str().to_string(), signature);
            }
        }
        Self { by_name }
    }

    fn get(&self, callee: &str) -> Option<&Signature> {
        self.by_name.get(callee)
    }
}

/// Reads one `def`'s parameter list, or `None` when it uses a parameter kind
/// this part does not bind against.
///
/// `*args`, keyword-only parameters, `**kwargs` and default values are all
/// rejected outright by `func::lower_params`, so a module containing such a
/// `def` never compiles either way; leaving those names out of the table
/// keeps the call site's own diagnostic the pre-existing capability message
/// instead of a `T0021` derived from a signature pycc cannot represent.
fn signature_of(parameters: &Parameters) -> Option<Signature> {
    if parameters.vararg.is_some()
        || !parameters.kwonlyargs.is_empty()
        || parameters.kwarg.is_some()
    {
        return None;
    }
    let declared = parameters.posonlyargs.iter().chain(parameters.args.iter());
    if declared.clone().any(|param| param.default.is_some()) {
        return None;
    }
    Some(Signature {
        names: declared
            .map(|param| param.parameter.name.as_str().to_string())
            .collect(),
        posonly_count: parameters.posonlyargs.len(),
    })
}

/// Whether `call`'s keyword arguments can be bound by name.
///
/// This is the single predicate that narrows `expr.rs`'s blanket keyword
/// rejection. It is deliberately evaluated *before* the call-shape arms that
/// follow it there, and it answers `false` for every shape those arms handle,
/// so none of them can silently erase a keyword it never inspects.
pub(crate) fn is_bindable_call(signatures: &SignatureTable, call: &ExprCall) -> bool {
    let Expr::Name(callee) = call.func.as_ref() else {
        return false;
    };
    // `arg: None` is a `**kwargs` unpacking, which has no parameter name to
    // bind and stays rejected.
    call.arguments.keywords.iter().all(|kw| kw.arg.is_some())
        && signatures.get(callee.id.as_str()).is_some()
}

/// Binds `positional` and `keywords` to `callee`'s parameters, returning the
/// complete positional argument vector `HirExpr::Call` needs.
///
/// `keywords` carries each keyword's name, its own source range, and its
/// already-lowered value. The caller has established through
/// [`is_bindable_call`] that `callee` is in `signatures`.
pub(crate) fn bind_keyword_arguments(
    signatures: &SignatureTable,
    callee: &str,
    positional: Vec<HirExpr>,
    keywords: Vec<(String, std::ops::Range<u32>, HirExpr)>,
    call_range: std::ops::Range<u32>,
) -> Result<Vec<HirExpr>, Diagnostic> {
    let signature = signatures
        .get(callee)
        .expect("caller checked `is_bindable_call`");
    let arity = signature.names.len();
    if positional.len() > arity {
        return Err(Diagnostic::error(
            "T0021",
            format!(
                "`{callee}` expects {arity} argument(s), got {}",
                positional.len() + keywords.len()
            ),
            span_of(call_range),
        )
        .with_help(format!("pass exactly {arity} argument(s)")));
    }
    let mut slots: Vec<Option<HirExpr>> = Vec::with_capacity(arity);
    slots.extend(positional.into_iter().map(Some));
    slots.resize_with(arity, || None);
    for (name, range, value) in keywords {
        let Some(index) = signature.names.iter().position(|param| *param == name) else {
            return Err(Diagnostic::error(
                "T0021",
                format!("`{callee}` got an unexpected keyword argument `{name}`"),
                span_of(range),
            )
            .with_help(format!(
                "`{callee}` accepts {}",
                describe_bindable(signature)
            )));
        };
        if index < signature.posonly_count {
            return Err(Diagnostic::error(
                "T0021",
                format!(
                    "`{callee}` got positional-only parameter `{name}` passed as a keyword argument"
                ),
                span_of(range),
            )
            .with_help(format!("pass `{name}` positionally")));
        }
        if slots[index].is_some() {
            return Err(Diagnostic::error(
                "T0021",
                format!("`{callee}` got multiple values for argument `{name}`"),
                span_of(range),
            )
            .with_help(format!(
                "`{name}` is already supplied positionally at this call"
            )));
        }
        slots[index] = Some(value);
    }
    let missing: Vec<String> = signature
        .names
        .iter()
        .zip(slots.iter())
        .filter(|(_, slot)| slot.is_none())
        .map(|(name, _)| format!("`{name}`"))
        .collect();
    if !missing.is_empty() {
        return Err(Diagnostic::error(
            "T0021",
            format!(
                "`{callee}` is missing required argument(s): {}",
                missing.join(", ")
            ),
            span_of(call_range),
        )
        .with_help(format!("pass exactly {arity} argument(s)")));
    }
    Ok(slots
        .into_iter()
        .map(|slot| slot.expect("every slot was filled or reported missing"))
        .collect())
}

/// The human-readable list of parameter names a keyword may name, for the
/// unexpected-keyword help text.
fn describe_bindable(signature: &Signature) -> String {
    let bindable: Vec<String> = signature.names[signature.posonly_count..]
        .iter()
        .map(|name| format!("`{name}`"))
        .collect();
    if bindable.is_empty() {
        return "no keyword arguments".to_string();
    }
    bindable.join(", ")
}

fn span_of(range: std::ops::Range<u32>) -> Span {
    Span::new(range.start, range.end)
}

#[cfg(test)]
mod tests {
    use crate::{HirExpr, HirItem, HirStmt};

    /// The lowered HIR of `source`, which must lower cleanly.
    fn lower(source: &str) -> crate::HirModule {
        let module = pycc_parser::parse(source).expect("test fixture must parse");
        crate::lower_checked(&module).expect("fixture must lower")
    }

    fn lower_err(source: &str) -> pycc_diag::Diagnostic {
        let module = pycc_parser::parse(source).expect("test fixture must parse");
        crate::lower_checked(&module).expect_err("fixture must be rejected")
    }

    /// The argument vector of the first top-level `HirExpr::Call` statement.
    fn first_call_args(source: &str) -> Vec<HirExpr> {
        lower(source)
            .items
            .iter()
            .find_map(|item| match item {
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call { args, .. })) => {
                    Some(args.clone())
                }
                _ => None,
            })
            .expect("fixture has no top-level call statement")
    }

    /// Every lowered function's name and body, in module order.
    fn function_bodies(hir: &crate::HirModule) -> Vec<(&str, &Vec<HirStmt>)> {
        hir.items
            .iter()
            .filter_map(|item| match item {
                HirItem::Function { name, body, .. } => Some((name.as_str(), body)),
                HirItem::TopLevelStmt(_) => None,
            })
            .collect()
    }

    const DEF_AB: &str = "def f(a: int, b: int) -> None:\n    print(a)\n    print(b)\n\n";

    #[test]
    fn a_keyword_call_binds_to_the_same_arguments_as_its_positional_twin() {
        let keyword = first_call_args(&format!("{DEF_AB}f(b=2, a=1)\n"));
        let positional = first_call_args(&format!("{DEF_AB}f(1, 2)\n"));
        assert_eq!(keyword, positional);
        assert_eq!(
            keyword,
            vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)]
        );
    }

    #[test]
    fn a_mixed_positional_and_keyword_call_binds_the_keyword_to_the_later_parameter() {
        assert_eq!(
            first_call_args(&format!("{DEF_AB}f(1, b=2)\n")),
            vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)]
        );
    }

    #[test]
    fn a_keyword_call_written_before_its_own_def_still_binds() {
        // The table is collected from the whole module body before any item
        // is lowered, so forward references bind like any other call.
        assert_eq!(
            first_call_args(&format!(
                "def g() -> None:\n    f(b=2, a=1)\n\n{DEF_AB}g()\n"
            )),
            vec![]
        );
        let hir = lower(&format!(
            "def g() -> None:\n    f(b=2, a=1)\n\n{DEF_AB}g()\n"
        ));
        let body = function_bodies(&hir)
            .into_iter()
            .find_map(|(name, body)| (name == "g").then_some(body))
            .expect("the module must hold `g`");
        assert_eq!(
            body[0],
            HirStmt::ExprStmt(HirExpr::Call {
                callee: "f".to_string(),
                args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
            })
        );
    }

    #[test]
    fn a_keyword_call_inside_a_method_body_binds_too() {
        // `ClassBodyInput` forwards the table to every method body. The
        // compiler cannot catch a missing forward there -- an empty table
        // would merely make this call fall back to `C0001` -- so this test
        // is the only guard on that seam.
        let hir = lower(&format!(
            "{DEF_AB}class C:\n    def m(self) -> None:\n        f(b=2, a=1)\n"
        ));
        let call = function_bodies(&hir)
            .into_iter()
            .find_map(|(_, body)| {
                body.iter().find_map(|stmt| match stmt {
                    HirStmt::ExprStmt(call @ HirExpr::Call { callee, .. }) if callee == "f" => {
                        Some(call.clone())
                    }
                    _ => None,
                })
            })
            .expect("the method body must hold the bound call");
        assert_eq!(
            call,
            HirExpr::Call {
                callee: "f".to_string(),
                args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
            }
        );
    }

    #[test]
    fn a_keyword_naming_a_parameter_after_the_slash_marker_binds() {
        assert_eq!(
            first_call_args(
                "def f(a: int, /, b: int) -> None:\n    print(a)\n    print(b)\n\nf(1, b=2)\n"
            ),
            vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)]
        );
    }

    #[test]
    fn an_unexpected_keyword_name_is_a_type_error_at_the_keyword_span() {
        let source = format!("{DEF_AB}f(1, 2, c=3)\n");
        let diagnostic = lower_err(&source);
        assert_eq!(diagnostic.code, "T0021");
        assert_eq!(
            diagnostic.message,
            "`f` got an unexpected keyword argument `c`"
        );
        let start = u32::try_from(source.find("c=3").expect("fixture holds `c=3`"))
            .expect("fixture is short");
        assert_eq!(
            diagnostic.span,
            Some(pycc_diag::Span::new(start, start + 3))
        );
        assert_eq!(diagnostic.help.as_deref(), Some("`f` accepts `a`, `b`"));
    }

    #[test]
    fn a_stray_keyword_on_a_zero_parameter_callee_reports_no_bindable_names() {
        let diagnostic = lower_err("def f() -> None:\n    return\n\nf(extra=1)\n");
        assert_eq!(diagnostic.code, "T0021");
        assert_eq!(
            diagnostic.message,
            "`f` got an unexpected keyword argument `extra`"
        );
        assert_eq!(
            diagnostic.help.as_deref(),
            Some("`f` accepts no keyword arguments")
        );
    }

    #[test]
    fn a_positional_only_parameter_passed_as_a_keyword_is_a_type_error() {
        let diagnostic = lower_err(
            "def f(a: int, /, b: int) -> None:\n    print(a)\n    print(b)\n\nf(a=1, b=2)\n",
        );
        assert_eq!(diagnostic.code, "T0021");
        assert_eq!(
            diagnostic.message,
            "`f` got positional-only parameter `a` passed as a keyword argument"
        );
        assert_eq!(diagnostic.help.as_deref(), Some("pass `a` positionally"));
    }

    #[test]
    fn a_parameter_supplied_twice_is_a_type_error() {
        let diagnostic = lower_err(&format!("{DEF_AB}f(1, a=2, b=3)\n"));
        assert_eq!(diagnostic.code, "T0021");
        assert_eq!(
            diagnostic.message,
            "`f` got multiple values for argument `a`"
        );
        assert_eq!(
            diagnostic.help.as_deref(),
            Some("`a` is already supplied positionally at this call")
        );
    }

    #[test]
    fn a_parameter_left_unsupplied_is_a_type_error_at_the_call_span() {
        let diagnostic = lower_err(&format!("{DEF_AB}f(a=1)\n"));
        assert_eq!(diagnostic.code, "T0021");
        assert_eq!(
            diagnostic.message,
            "`f` is missing required argument(s): `b`"
        );
        assert_eq!(
            diagnostic.help.as_deref(),
            Some("pass exactly 2 argument(s)")
        );
    }

    #[test]
    fn more_positional_arguments_than_parameters_is_a_type_error() {
        let diagnostic = lower_err("def f(a: int) -> None:\n    print(a)\n\nf(1, 2, a=3)\n");
        assert_eq!(diagnostic.code, "T0021");
        assert_eq!(diagnostic.message, "`f` expects 1 argument(s), got 3");
        assert_eq!(
            diagnostic.help.as_deref(),
            Some("pass exactly 1 argument(s)")
        );
    }

    #[test]
    fn a_later_def_of_the_same_name_replaces_an_earlier_one_in_the_table() {
        let diagnostic = lower_err(
            "def f(a: int) -> None:\n    print(a)\n\ndef f(b: int) -> None:\n    print(b)\n\nf(a=1)\n",
        );
        // Whatever the module-level rebinding rule decides about the two
        // `def`s themselves, the table never answers with the first one's
        // parameter list.
        assert_ne!(
            diagnostic.message,
            "`f` is missing required argument(s): `a`"
        );
    }

    #[test]
    fn a_kwargs_unpacking_keeps_the_unchanged_capability_rejection() {
        let diagnostic = lower_err(&format!("{DEF_AB}d = {{}}\nf(**d)\n"));
        assert_eq!(diagnostic.code, "C0001");
        assert_eq!(
            diagnostic.message,
            "keyword call arguments are not supported yet"
        );
    }

    #[test]
    fn a_def_using_a_parameter_kind_the_table_excludes_is_never_bindable() {
        // Each of these `def`s is rejected by `func::lower_params` on its own
        // terms; the point here is that `signature_of` returns `None` for it
        // rather than recording a signature pycc cannot represent.
        for (source, message) in [
            (
                "def f(*args) -> None:\n    return\n",
                "`*args` is not supported yet",
            ),
            (
                "def f(*, a: int) -> None:\n    print(a)\n",
                "keyword-only parameters are not supported yet",
            ),
            (
                "def f(**kwargs) -> None:\n    return\n",
                "`**kwargs` is not supported yet",
            ),
            (
                "def f(a: int = 1) -> None:\n    print(a)\n",
                "default parameter values are not supported yet",
            ),
        ] {
            let diagnostic = lower_err(&format!("{source}\nf(a=1)\n"));
            assert_eq!(diagnostic.code, "C0001", "source: {source}");
            assert_eq!(diagnostic.message, message, "source: {source}");
        }
    }

    #[test]
    fn every_call_shape_outside_the_bindable_one_keeps_the_capability_rejection() {
        for source in [
            // An undefined callee is not in the table.
            "undefined_callee(extra=1)\n",
            // A method call.
            "class C:\n    def m(self, a: int) -> None:\n        print(a)\n\nc = C()\nc.m(a=1)\n",
            // A `super()` method call.
            "class B:\n    def m(self, a: int) -> None:\n        print(a)\n\nclass C(B):\n    def m(self, a: int) -> None:\n        super().m(a=1)\n",
            // A bare `super()` call.
            "class C:\n    def m(self) -> None:\n        super(x=1)\n",
            // A class instantiation.
            "class C:\n    def __init__(self, a: int) -> None:\n        self.a = a\n\nc = C(a=1)\n",
            // A builtin.
            "print(sep=1)\n",
            // A container method.
            "xs = [1]\nxs.append(value=2)\n",
            // A stdlib intrinsic through an aliased receiver.
            "import math as m\n\nx = m.sqrt(x=1.0)\n",
            // A generic class instantiation through a subscript callee.
            "class C[T]:\n    def __init__(self, a: int) -> None:\n        self.a = a\n\nc = C[int](a=1)\n",
        ] {
            let diagnostic = lower_err(source);
            assert_eq!(diagnostic.code, "C0001", "source: {source}");
            assert_eq!(
                diagnostic.message, "keyword call arguments are not supported yet",
                "source: {source}"
            );
        }
    }

    #[test]
    fn a_keyword_range_argument_stays_rejected_in_both_positions() {
        for source in [
            "for i in range(stop=3):\n    print(i)\n",
            "xs = [i for i in range(stop=3)]\n",
        ] {
            let diagnostic = lower_err(source);
            assert_eq!(diagnostic.code, "C0001", "source: {source}");
        }
    }
}
