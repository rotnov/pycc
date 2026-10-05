//! Part 8 of #1371: a call with keyword arguments on a CPython object
//! (`foreign::keyword_call`), and the `C0001` every other keyword call
//! keeps.
//!
//! The fixture helpers are copied from `call_tests.rs` because a sibling
//! module cannot reach them.

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

const HEAD: &str = "import builtins\nfrom itertools import product\n";

fn admitted(tail: &str) {
    let source = format!("{HEAD}{tail}");
    check(&source).unwrap_or_else(|diagnostics| panic!("{source:?}: {diagnostics:#?}"));
}

/// The single diagnostic `tail` is refused with, by code and a message
/// phrase; returns it so a caller can check its span.
fn refused(tail: &str, code: &str, phrase: &str) -> pycc_diag::Diagnostic {
    let source = format!("{HEAD}{tail}");
    let mut diagnostics = check(&source).expect_err(&source);
    assert_eq!(diagnostics.len(), 1, "{source:?}: {diagnostics:#?}");
    let diagnostic = diagnostics.remove(0);
    assert_eq!(diagnostic.code, code, "{source:?}: {diagnostic:#?}");
    assert!(
        diagnostic.message.contains(phrase),
        "{source:?}: {diagnostic:#?}"
    );
    diagnostic
}

const KEYWORD: &str = "keyword call arguments are not supported yet";

/// All three object-call shapes take keyword arguments of every admitted
/// argument type, with or without positional ones, in a module body and in
/// a function body, and the result is an `object`.
#[test]
fn every_object_call_shape_takes_keyword_arguments() {
    admitted("product(\"ab\", repeat=2)\n");
    admitted("x = builtins.sorted(\"ba\", reverse=True, key=None)\n");
    admitted("s = builtins.str(\"a,b\")\nt = s.split(\",\", maxsplit=1)\n");
    admitted("builtins.__dict__[\"int\"](\"11\", base=2)\n");
    admitted("product(a=1, b=2.5, c=True, d=\"x\", e=product, f=None)\n");
    admitted("s = str(product(repeat=1))\n");
    admitted(
        "def f(n: int) -> None:\n    o = builtins.str(\"a,b\")\n    \
         print(o.split(\",\", maxsplit=n))\n",
    );
    admitted("def g() -> None:\n    product(\"ab\", repeat=2)\n");
}

/// Since Part 8 a `None` argument is admitted, positional or keyword, in
/// every call shape.
#[test]
fn a_none_argument_is_admitted() {
    admitted("product(None)\n");
    admitted("builtins.max(1, None)\n");
    admitted("builtins.__dict__[\"print\"](None)\n");
    admitted("product(x=None)\n");
}

/// A keyword value outside the argument rule is refused with the same
/// I0404 a positional one gets, naming the shape.
#[test]
fn a_non_packable_keyword_value_is_refused() {
    refused(
        "product(repeat=[1])\n",
        "I0404",
        "passing a `list[int]` argument to a CPython object's call",
    );
    refused(
        "builtins.sorted(\"ab\", key=[1])\n",
        "I0404",
        "passing a `list[int]` argument to a CPython object's method",
    );
    refused(
        "class P:\n    def f(self) -> None:\n        product(state=self)\n",
        "I0404",
        "passing a `P` argument to a CPython object's call",
    );
}

/// An error in a keyword value, or in the positional half, is reported as
/// itself.
#[test]
fn an_ill_typed_operand_reports_its_own_error() {
    refused("product(repeat=missing)\n", "T0021", "missing");
    refused("product(missing, repeat=1)\n", "T0021", "missing");
}

/// A keyword call of anything that is not a CPython object keeps the
/// `C0001`, located at the call: a builtin, a pycc class, a pycc method, a
/// method of a pycc value, a subscript of a pycc container, a module `def`
/// rebound away from the binder, and a method of a receiver whose own
/// inference fails.
#[test]
fn a_keyword_call_of_a_non_object_keeps_the_c0001() {
    let print = refused("print(1, end=\"\")\n", "C0001", KEYWORD);
    let start = HEAD.len() as u32;
    let span = print.span.expect("the keyword refusal is located");
    assert_eq!((span.start, span.end), (start, start + 16), "{print:#?}");
    refused(
        "class C:\n    def __init__(self, a: int) -> None:\n        self.a = a\n\n\nC(a=1)\n",
        "C0001",
        KEYWORD,
    );
    refused(
        "class C:\n    def m(self, a: int) -> int:\n        return a\n\n\nC().m(a=1)\n",
        "C0001",
        KEYWORD,
    );
    refused("s = \"ab\"\ns.split(\",\", maxsplit=1)\n", "C0001", KEYWORD);
    refused("t = {\"a\": 1}\nt[\"a\"](k=1)\n", "C0001", KEYWORD);
    refused("missing.m(k=1)\n", "C0001", KEYWORD);
}
