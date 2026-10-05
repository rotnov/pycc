//! The method part of #1140 at the `--ext` host boundary: a method's
//! default parameter values reach the generated `METH_FASTCALL` wrapper and
//! `Py_tp_init`, which then accept the shorter argument counts CPython
//! does. Pinned on the companion generated from real lowered source, so the
//! defaults are the ones `pycc_hir` really records.

use super::*;

/// Parses, lowers and type-checks `source`, returning the resolved module.
fn resolved(source: &str) -> HirModule {
    let module = pycc_parser::parse(source).expect("test fixture must parse");
    let hir = pycc_hir::lower_checked(&module).expect("test fixture must lower");
    pycc_types::check_and_resolve(&hir).expect("test fixture must check")
}

/// The generated companion for `source`'s exports and constructors.
fn companion(source: &str) -> String {
    let module = resolved(source);
    let exports = collect_exports(&module).expect("the module exports");
    let publications = collect_class_publications(&module, &exports);
    let ctors = collect_constructors(&module, &publications);
    generate_exports_inc("m", &exports, &[], &publications, &ctors)
}

const MODULE: &str = "class S:\n\
    \x20   def __init__(self, n: int, label: str = 'x') -> None:\n\
    \x20       self.n = n\n        self.label = label\n\
    \x20   def feed(self, token: int, is_end: bool = False) -> int:\n\
    \x20       return token\n\
    \x20   @staticmethod\n    def make(x: int = 3) -> int:\n        return x\n\
    \x20   @classmethod\n    def scaled(cls, f: float = -1.5) -> float:\n        return f\n\
    \x20   def plain(self, a: int) -> int:\n        return a\n";

#[test]
fn the_carried_defaults_drop_the_receiver_entry() {
    let module = resolved(MODULE);
    let exports = collect_exports(&module).expect("the module exports");
    let defaults = |name: &str| {
        exports
            .iter()
            .find(|export| export.name == name)
            .unwrap_or_else(|| panic!("`{name}` is exported"))
            .defaults
            .clone()
    };
    assert_eq!(
        defaults("S.feed"),
        vec![None, Some(pycc_hir::HirExpr::BoolLiteral(false))]
    );
    // A `@staticmethod` has no receiver to drop.
    assert_eq!(
        defaults("S.make.static"),
        vec![Some(pycc_hir::HirExpr::IntLiteral(3))]
    );
    assert_eq!(
        defaults("S.scaled.classmethod"),
        vec![Some(pycc_hir::HirExpr::FloatLiteral(-1.5))]
    );
    assert!(defaults("S.plain").is_empty());
    let ctors = collect_constructors(&module, &collect_class_publications(&module, &exports));
    assert_eq!(
        ctors[0].defaults,
        vec![
            None,
            Some(pycc_hir::HirExpr::StringLiteral("x".to_string()))
        ]
    );
}

#[test]
fn a_defaulted_method_wrapper_accepts_the_shorter_argument_counts() {
    let inc = companion(MODULE);
    assert!(
        inc.contains(
            "    if (nargs < 1 || nargs > 2) {\n        PyErr_Format(PyExc_TypeError, \
             \"S.feed() takes from 1 to 2 arguments (%zd given)\", nargs);\n"
        ),
        "{inc}"
    );
    assert!(
        inc.contains("    PyObject *v1 = nargs > 1 ? args[1] : Py_False;\n"),
        "{inc}"
    );
    // The defaulted argument goes through the parameter's own unchanged
    // unpack helper.
    assert!(inc.contains("pycc_ext_unpack_bool(v1, "), "{inc}");
    // A `@staticmethod` with every parameter defaulted accepts none.
    assert!(inc.contains("if (nargs < 0 || nargs > 1)"), "{inc}");
    assert!(
        inc.contains("static PyObject *pycc_ext_default_wrap_"),
        "{inc}"
    );
    assert!(inc.contains("PyLong_FromLongLong(3LL)"), "{inc}");
    assert!(inc.contains("PyFloat_FromDouble(-1.5e0)"), "{inc}");
}

#[test]
fn a_defaulted_constructor_accepts_the_shorter_argument_counts() {
    let inc = companion(MODULE);
    assert!(
        inc.contains(
            "    if (PyTuple_Size(args) < 1 || PyTuple_Size(args) > 2) {\n        \
             PyErr_Format(PyExc_TypeError, \"S.__init__() takes from 1 to 2 arguments \
             (%zd given)\", PyTuple_Size(args));\n        return -1;\n    }\n"
        ),
        "{inc}"
    );
    assert!(
        inc.contains(
            "    PyObject *v1 = PyTuple_Size(args) > 1 ? PyTuple_GetItem(args, 1) : \
             pycc_ext_default_init_S_1();\n    if (v1 == NULL) {\n        return -1;\n    }\n"
        ),
        "{inc}"
    );
    assert!(
        inc.contains("static PyObject *pycc_ext_default_init_S_1(void)"),
        "{inc}"
    );
    assert!(
        inc.contains("PyUnicode_FromStringAndSize(\"x\", 1)"),
        "{inc}"
    );
}

#[test]
fn a_method_without_defaults_keeps_the_exact_arity_check() {
    let inc = companion(MODULE);
    assert!(
        inc.contains("\"S.plain() takes exactly 1 argument (%zd given)\""),
        "{inc}"
    );
    let plain = companion(
        "class P:\n    def __init__(self, n: int) -> None:\n        self.n = n\n\
         \x20   def get(self) -> int:\n        return self.n\n",
    );
    assert!(
        plain.contains("\"P.__init__() takes exactly 1 argument (%zd given)\""),
        "{plain}"
    );
    assert!(!plain.contains("pycc_ext_default_"), "{plain}");
    assert!(!plain.contains("takes from"), "{plain}");
}

#[test]
fn a_receiver_exact_copy_carries_its_origin_s_defaults() {
    // `B.g` is compiled for `B` from `A.g`'s source (#1337, D-254), so a
    // host calling `B().g()` omits `k` exactly as on `A`.
    let module = resolved(
        "class A:\n    def __init__(self) -> None:\n        self.x = 0\n\
         \x20   def m(self) -> int:\n        return 1\n\
         \x20   def g(self, k: int = 4) -> int:\n        return self.m() + k\n\
         class B(A):\n    def m(self) -> int:\n        return 2\n",
    );
    let exports = collect_exports(&module).expect("the module exports");
    let copy = exports
        .iter()
        .find(|export| export.name == "B.g")
        .expect("the copy is exported");
    assert_eq!(copy.defaults, vec![Some(pycc_hir::HirExpr::IntLiteral(4))]);
}
