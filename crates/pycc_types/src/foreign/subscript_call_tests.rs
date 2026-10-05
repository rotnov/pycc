//! Part 2a of #1371: checking a call of a subscript result
//! (`HirExpr::ExprCall`, `table[k](args)`), admitted only on a CPython
//! object.
//!
//! Kept apart from `foreign/tests.rs` (see #1314); the fixture helpers are
//! copied from `call_tests.rs` because a sibling module cannot reach them.

/// Lowers `source` with every `import` request answered `Foreign`.
fn lower_all_foreign(source: &str) -> pycc_hir::HirModule {
    let module = pycc_parser::parse(source).expect("test source must parse");
    let mut resolved = pycc_hir::ResolvedImports::default();
    for request in pycc_hir::project_import_requests(&module) {
        resolved.insert(request.span, pycc_hir::ResolvedImport::Foreign);
    }
    pycc_hir::lower_module(&module, &resolved, None)
        .unwrap_or_else(|diagnostics| panic!("{source:?} must lower: {diagnostics:#?}"))
        .hir
}

fn check(source: &str) -> Result<(), Vec<pycc_diag::Diagnostic>> {
    crate::check_all(&lower_all_foreign(source)).map(|_| ())
}

const IMPORT: &str = "from itertools import product\n";

fn admitted(snippet: &str) {
    let source = format!("{IMPORT}{snippet}");
    check(&source).unwrap_or_else(|diagnostics| panic!("{source:?}: {diagnostics:#?}"));
}

/// The single diagnostic `snippet` is refused with, by code and phrase.
fn refused(snippet: &str, code: &str, phrase: &str) {
    let source = format!("{IMPORT}{snippet}");
    let diagnostics = check(&source).expect_err(&source);
    assert_eq!(diagnostics.len(), 1, "{source:?}: {diagnostics:#?}");
    assert_eq!(diagnostics[0].code, code, "{source:?}: {diagnostics:#?}");
    assert!(
        diagnostics[0].message.contains(phrase),
        "{source:?}: {diagnostics:#?}"
    );
}

/// Scalar and object keys and arguments, in a module body, an annotated
/// function and an unannotated private helper (the constraint solver's
/// path), with the `object` result feeding a consumer that takes one.
#[test]
fn a_call_of_an_object_subscript_is_admitted_in_every_body() {
    admitted("product['a'](1, 2.5, True, 'ab')\n");
    admitted("product[product](product)\n");
    admitted("s = str(product[1]('x'))\n");
    admitted("product['a']()\n");
    admitted("def f(k: str) -> None:\n    print(str(product[k](product)))\n");
    admitted("def _h(k):\n    return product[k](1)\n\n\nx = _h('a')\nprint(x)\n");
    admitted("x = product['a']['b'](1)\n");
}

/// A callee that is not a CPython object keeps the `C0001` HIR reported for
/// this shape before Part 2a.
#[test]
fn a_subscript_callee_that_is_not_an_object_is_refused() {
    refused(
        "t = {'a': 1}\nt['a'](1)\n",
        "C0001",
        "calling a subscript expression is not supported yet",
    );
    refused(
        "def f(t: list[int]) -> None:\n    t[0](1)\n",
        "C0001",
        "or a call of a CPython object is",
    );
}

/// An argument outside the packable operands is refused with the shared
/// object-call rule's `I0404`.
#[test]
fn a_non_packable_argument_is_refused() {
    refused(
        "product['a']([1])\n",
        "I0404",
        "passing a `list[int]` argument to a CPython object's call",
    );
    refused(
        "product['a'](None)\n",
        "I0404",
        "argument to a CPython object's call",
    );
}

/// The callee is checked before the arguments (CPython's evaluation order):
/// a bad key reports its own diagnostic even when an argument is also bad.
#[test]
fn the_callee_is_checked_before_the_arguments() {
    refused(
        "product[[1]]([1])\n",
        "I0404",
        "indexing a CPython object with a `list[int]` key",
    );
}

/// A walrus in the key or an argument binds its name, in a module body and
/// in a function (`collect_named_expr_bindings` and the solver's
/// `bind_named_expr_targets` both walk the new node).
#[test]
fn a_walrus_in_the_callee_or_an_argument_binds() {
    admitted("product[(k := 1)]((y := 2))\nprint(k)\nprint(y)\n");
    admitted("def f() -> None:\n    product[(k := 1)]((y := 2))\n    print(k)\n    print(y)\n");
    admitted("def _h():\n    product[(k := 1)]((y := 2))\n    return y\n\n\nz = _h()\n");
}

/// A generic function call inside the callee or an argument is
/// monomorphized through the new node, a generic body's generic call there
/// is still found by the recursive-instantiation check, and a generic class
/// instantiation inside an argument is collected (and then refused as an
/// argument, having no boundary form).
#[test]
fn generic_calls_inside_the_callee_and_arguments_are_rewritten() {
    admitted("def ident[T](x: T) -> T:\n    return x\n\n\nproduct[ident('a')](ident(1))\n");
    refused(
        "def ident[T](x: T) -> T:\n    return x\n\n\ndef g[T](x: T) -> T:\n    \
         product[ident('a')](1)\n    return x\n\n\ng(1)\n",
        "T0042",
        "generic function `g` calls generic function `ident`",
    );
    refused(
        "class C[T]:\n    def __init__(self, x: T) -> None:\n        self.x = x\n\n\n\
         product['a'](C[int](1))\n",
        "I0404",
        "argument to a CPython object's call",
    );
}

/// A call of a subscript result inside a method of a class that another
/// class inherits from is walked by the inherited-copies facts pass.
#[test]
fn a_call_in_an_inherited_method_is_admitted() {
    admitted(
        "class A:\n    def m(self, k: str) -> None:\n        print(str(product[k](1)))\n\n\n\
         class B(A):\n    pass\n\n\nB().m('a')\n",
    );
}
