//! A buffer-carrier from-import (#1380, Part 2 of #1138, D-244): the import
//! binds a hidden name and runs in the host, its spelling stays an
//! annotation, and every other read or binding of the spelling is refused by
//! `carrier::reject_carrier_misuse` at the offending node's own span.

use super::from_foreign::lower_foreign;
use super::*;
use crate::FromImport;
use crate::import::reject_carrier_misuse;

/// Every module a fixture may import, answered as foreign.
const FOREIGN: &[&str] = &["numpy", "numpy.typing", "other", "ndarray"];

/// Both carrier imports, as lowering records them.
const IMPORTS: &str = "from numpy import ndarray\nfrom numpy.typing import NDArray\n";

fn carrier_imports() -> Vec<ImportBinding> {
    lower_foreign(IMPORTS, FOREIGN)
        .expect("the carrier imports lower")
        .hir
        .imports
}

/// The post-check alone over `IMPORTS` followed by `body`.
fn misuse(body: &str) -> Vec<Diagnostic> {
    let source = format!("{IMPORTS}{body}");
    reject_carrier_misuse(&parse(&source).body, &carrier_imports())
}

/// The span of `name` inside the first occurrence of `context` in `source`.
fn span_in(source: &str, context: &str, name: &str) -> Span {
    let start = source.find(context).expect("context occurs in the source")
        + context.find(name).expect("name occurs in its context");
    Span::new(start as u32, (start + name.len()) as u32)
}

/// `body` (after `IMPORTS`) is refused exactly once, at `name` inside
/// `context`, with the read or binding message for `name`.
fn assert_refused(body: &str, context: &str, name: &str, kind: &str) {
    let source = format!("{IMPORTS}{body}");
    let diagnostics = misuse(body);
    assert_eq!(diagnostics.len(), 1, "{body}: {diagnostics:#?}");
    let diagnostic = &diagnostics[0];
    assert_eq!(diagnostic.code, "C0001");
    let spelling = if name.starts_with("ndarray") {
        "ndarray"
    } else {
        "NDArray"
    };
    assert!(
        diagnostic
            .message
            .starts_with(&format!("{kind} `{spelling}`")),
        "{body}: {}",
        diagnostic.message
    );
    assert_eq!(
        diagnostic.span,
        Some(span_in(&source, context, name)),
        "{body}"
    );
}

fn assert_read_refused(body: &str, context: &str, name: &str) {
    assert_refused(body, context, name, "reading");
}

fn assert_binding_refused(body: &str, context: &str, name: &str) {
    assert_refused(body, context, name, "binding");
}

#[test]
fn each_carrier_pair_binds_a_hidden_name_and_keeps_the_real_one_in_from() {
    let source = "from numpy.typing import NDArray\nfrom numpy import ndarray, float64\n\
                  def total(a: NDArray, b: ndarray) -> float:\n    return a[0] + b[0]\n";
    let lowered = lower_foreign(source, FOREIGN).expect("must lower");
    let rows: Vec<(String, String, String, usize)> = lowered
        .hir
        .imports
        .iter()
        .map(|binding| match binding {
            ImportBinding::Foreign {
                local_name,
                module_path,
                from: Some(FromImport { name, index, .. }),
                ..
            } => (
                local_name.clone(),
                module_path.clone(),
                name.clone(),
                *index,
            ),
            other => panic!("unexpected binding {other:?}"),
        })
        .collect();
    let row = |local: &str, module: &str, name: &str, index| {
        (
            local.to_string(),
            module.to_string(),
            name.to_string(),
            index,
        )
    };
    assert_eq!(
        rows,
        vec![
            row("$carrier:NDArray", "numpy.typing", "NDArray", 0),
            row("$carrier:ndarray", "numpy", "ndarray", 0),
            row("float64", "numpy", "float64", 1),
        ]
    );
}

/// The identical import twice, and once more inside a `TYPE_CHECKING`
/// guard, is still one meaning of the name: nothing is refused.
#[test]
fn a_repeated_or_guarded_carrier_import_is_accepted() {
    let source = "from typing import TYPE_CHECKING\nfrom numpy import ndarray\n\
                  if TYPE_CHECKING:\n    from numpy import ndarray\n\
                  from numpy import ndarray\ndef f(a: ndarray) -> float:\n    return a[0]\n";
    lower_foreign(source, FOREIGN).expect("must lower");
}

#[test]
fn a_module_without_a_carrier_import_is_not_scanned() {
    let source = "ndarray = 1\nx = NDArray\n";
    assert!(reject_carrier_misuse(&parse(source).body, &[]).is_empty());
}

#[test]
fn every_annotation_position_is_accepted() {
    let diagnostics = misuse(
        "def f(a: NDArray, b: list[ndarray], *args: NDArray, c: ndarray = None, \
         **kw: NDArray) -> NDArray:\n    x: ndarray = a\n    return a\n\
         class C:\n    field: NDArray\n    other: \"ndarray\"\n\
         type A = NDArray\ntype G[T] = list[NDArray]\nB: TypeAlias = ndarray\n\
         y: list[NDArray] = []\n",
    );
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
}

#[test]
fn each_read_outside_an_annotation_is_refused() {
    for (body, context, name) in [
        ("x = NDArray\n", "x = NDArray", "NDArray"),
        ("A = NDArray\n", "A = NDArray", "NDArray"),
        (
            "def f(n: int) -> int:\n    a = ndarray(n)\n    return 0\n",
            "a = ndarray(n)",
            "ndarray",
        ),
        (
            "def f(a: object) -> bool:\n    return isinstance(a, ndarray)\n",
            "a, ndarray)",
            "ndarray",
        ),
        ("print(ndarray.__name__)\n", "print(ndarray", "ndarray"),
        (
            "def f(a: int = NDArray) -> int:\n    return 0\n",
            "= NDArray",
            "NDArray",
        ),
        (
            "@ndarray\ndef f() -> int:\n    return 0\n",
            "@ndarray",
            "ndarray",
        ),
        ("class C(ndarray):\n    pass\n", "C(ndarray", "ndarray"),
        (
            "def f[T: NDArray](a: T) -> T:\n    return a\n",
            "T: NDArray",
            "NDArray",
        ),
        (
            "class C:\n    x: NDArray = NDArray\n",
            "= NDArray",
            "NDArray",
        ),
        ("y: int = len(ndarray)\n", "len(ndarray", "ndarray"),
    ] {
        assert_read_refused(body, context, name);
    }
}

/// A use above the import is refused as well as one below it: the check
/// does not depend on where the import sits.
#[test]
fn a_producer_call_above_the_import_is_refused() {
    let source = "def f(n: int) -> float:\n    a = ndarray(n)\n    return 0.0\n\
                  from numpy import ndarray\n";
    let diagnostics = lower_foreign(source, FOREIGN).expect_err("refused");
    let expected = span_in(source, "a = ndarray", "ndarray");
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.span == Some(expected)
                && diagnostic
                    .message
                    .starts_with("reading `ndarray` outside a type annotation")),
        "{diagnostics:#?}"
    );
}

#[test]
fn each_definition_and_parameter_binding_is_refused() {
    for (body, context, name) in [
        (
            "def ndarray() -> int:\n    return 0\n",
            "def ndarray",
            "ndarray",
        ),
        ("class NDArray:\n    pass\n", "class NDArray", "NDArray"),
        (
            "def f(ndarray: int) -> int:\n    return 0\n",
            "(ndarray",
            "ndarray",
        ),
        (
            "def f(a: int, /, ndarray: int) -> int:\n    return 0\n",
            ", ndarray",
            "ndarray",
        ),
        (
            "def f(*ndarray: int) -> int:\n    return 0\n",
            "*ndarray",
            "ndarray",
        ),
        (
            "def f(*, ndarray: int) -> int:\n    return 0\n",
            ", ndarray",
            "ndarray",
        ),
        (
            "def f(**ndarray: int) -> int:\n    return 0\n",
            "**ndarray",
            "ndarray",
        ),
        ("g = lambda NDArray: 0\n", "lambda NDArray", "NDArray"),
        (
            "def f[NDArray](a: int) -> int:\n    return 0\n",
            "[NDArray",
            "NDArray",
        ),
        (
            "def f[*NDArray](a: int) -> int:\n    return 0\n",
            "[*NDArray",
            "NDArray",
        ),
        (
            "def f[**NDArray](a: int) -> int:\n    return 0\n",
            "[**NDArray",
            "NDArray",
        ),
    ] {
        assert_binding_refused(body, context, name);
    }
}

#[test]
fn each_statement_binding_is_refused() {
    for (body, context, name) in [
        (
            "try:\n    pass\nexcept ValueError as ndarray:\n    pass\n",
            "as ndarray",
            "ndarray",
        ),
        (
            "def f() -> None:\n    global ndarray\n",
            "global ndarray",
            "ndarray",
        ),
        (
            "def f() -> None:\n    x = 1\n    def g() -> None:\n        nonlocal NDArray\n",
            "nonlocal NDArray",
            "NDArray",
        ),
        ("ndarray = 1\n", "ndarray = 1", "ndarray"),
        ("NDArray += 1\n", "NDArray +=", "NDArray"),
        (
            "for ndarray in range(3):\n    pass\n",
            "for ndarray",
            "ndarray",
        ),
        (
            "with open('x') as NDArray:\n    pass\n",
            "as NDArray",
            "NDArray",
        ),
        (
            "y = [1 for ndarray in range(3)]\n",
            "for ndarray",
            "ndarray",
        ),
        ("y = (NDArray := 1)\n", "(NDArray", "NDArray"),
        (
            "def f() -> None:\n    del ndarray\n",
            "del ndarray",
            "ndarray",
        ),
        ("type NDArray = int\n", "type NDArray", "NDArray"),
        ("type G[NDArray] = int\n", "[NDArray", "NDArray"),
        (
            "ndarray: TypeAlias = int\n",
            "ndarray: TypeAlias",
            "ndarray",
        ),
    ] {
        assert_binding_refused(body, context, name);
    }
}

#[test]
fn each_second_import_binding_is_refused() {
    for (body, context, name) in [
        ("import ndarray\n", "\nimport ndarray", "ndarray"),
        (
            "import ndarray.sub\n",
            "\nimport ndarray.sub",
            "ndarray.sub",
        ),
        ("import json as NDArray\n", "as NDArray", "NDArray"),
        (
            "from other import ndarray\n",
            "from other import ndarray",
            "ndarray",
        ),
        (
            "from numpy import NDArray\n",
            "from numpy import NDArray",
            "NDArray",
        ),
    ] {
        assert_binding_refused(body, context, name);
    }
}

#[test]
fn each_match_capture_binding_is_refused() {
    for (body, context, name) in [
        (
            "match 1:\n    case ndarray:\n        pass\n",
            "case ndarray",
            "ndarray",
        ),
        (
            "match [1]:\n    case [*ndarray]:\n        pass\n",
            "*ndarray",
            "ndarray",
        ),
        (
            "match {}:\n    case {**ndarray}:\n        pass\n",
            "**ndarray",
            "ndarray",
        ),
        (
            "match 1:\n    case int() as NDArray:\n        pass\n",
            "as NDArray",
            "NDArray",
        ),
    ] {
        assert_binding_refused(body, context, name);
    }
}

/// #1485's fallback shape: the `except ImportError` rebinding is a second
/// binding of the spelling, refused at the assignment.
#[test]
fn a_try_except_fallback_rebinding_is_refused() {
    let source = "try:\n    from numpy import ndarray\nexcept ImportError:\n    ndarray = None\n";
    let diagnostics = lower_foreign(source, FOREIGN).expect_err("refused");
    let expected = span_in(source, "ndarray = None", "ndarray");
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.span == Some(expected)
                && diagnostic
                    .message
                    .starts_with("binding `ndarray` in a module that imports it")),
        "{diagnostics:#?}"
    );
}

/// Every offending site is reported, in source order.
#[test]
fn every_site_is_reported_in_source_order() {
    let diagnostics = misuse("x = NDArray\nndarray = 1\ny = ndarray\n");
    let messages: Vec<&str> = diagnostics
        .iter()
        .map(|diagnostic| &diagnostic.message[..9])
        .collect();
    assert_eq!(messages, ["reading `", "binding `", "reading `"]);
}

#[test]
fn the_messages_name_the_import_and_the_decision() {
    let read = &misuse("x = NDArray\n")[0];
    assert_eq!(
        read.message,
        "reading `NDArray` outside a type annotation is not supported yet: `from numpy.typing \
         import NDArray` keeps the name's buffer-annotation meaning in pycc (D-244), so it may \
         only annotate"
    );
    let binding = &misuse("ndarray = 1\n")[0];
    assert_eq!(
        binding.message,
        "binding `ndarray` in a module that imports it with `from numpy import ndarray` is not \
         supported yet: the import keeps the name's buffer-annotation meaning in pycc (D-244)"
    );
}
