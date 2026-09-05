//! Spanned rejection of a call to an enum class (#921).
//!
//! An enum class is constructor-less by design (`lower_enum_class`
//! early-returns before `class::init::ensure_init`, D-225): its members are
//! compile-time singletons, and CPython's `EnumType.__call__` value lookup
//! (`Color(1)`) is not implemented. Before this module, `Color()` and
//! `Color(1)` reached `pycc_types`/`pycc_mir` with no `__init__` in the MRO
//! and panicked. HIR expressions carry no spans, so the one place a
//! diagnostic can still point at the call expression is an AST-level scan;
//! `lower_module` runs it once per top-level item, right after that item is
//! lowered, so the collection order stays the loop order D-217 rule 3 pins.
//! `pycc_types::class::resolve_instantiation` keeps a span-less guard on
//! `HirClassDef::is_enum` behind this scan (defense in depth), so the two
//! "no `__init__` in the MRO" panics describe an invariant that holds.

use pycc_ast::visitor::{self, Visitor};
use pycc_ast::{Expr, Stmt, StmtClassDef};
use pycc_diag::Diagnostic;

/// The one `C0001` message for every positional-argument call shape on an
/// enum class (`Color()`, `Color(1)`, `Color(2)`, `Color(1, 2)`). Shared
/// with `pycc_types::class::resolve_instantiation` so the spanned and the
/// span-less rejection render identically. `<MEMBER>` is literal: a
/// member-less enum (`class E(Enum): "doc"`, #744) has no member to name.
/// The zero-argument form is deliberately not `T0021`: pycc models no enum
/// constructor to report an arity error against -- the whole value-lookup
/// call form is the unimplemented construct.
pub fn enum_class_call_message(class_name: &str) -> String {
    format!(
        "calling an enum class (`{class_name}(...)`) is not supported yet -- refer to a \
         member by name (`{class_name}.<MEMBER>`) instead"
    )
}

/// The names of every top-level class in `body` whose header is exactly one
/// bare-name enum marker base (`class Color(Enum):`, `class S(StrEnum):`),
/// the same predicate `lower_class` uses to route a class to
/// `lower_enum_class`. Computed syntactically *before* the top-level loop so
/// a call inside a `def` that precedes the class definition in source is
/// still found. A class whose base is not a bare name
/// (`class Color(enum.Enum):`) is not an enum class to `lower_class` either
/// and is left to its own `C0001`.
pub(crate) fn syntactic_enum_class_names(body: &[Stmt]) -> Vec<String> {
    body.iter()
        .filter_map(|stmt| match stmt {
            Stmt::ClassDef(def) if has_single_enum_marker_base(def) => Some(def.name.to_string()),
            _ => None,
        })
        .collect()
}

fn has_single_enum_marker_base(def: &StmtClassDef) -> bool {
    let Some(arguments) = def.arguments.as_deref() else {
        return false;
    };
    let [base] = &*arguments.args else {
        return false;
    };
    let Expr::Name(name) = base else {
        return false;
    };
    arguments.keywords.is_empty() && crate::is_enum_base_name(name.id.as_str())
}

/// Every call in `stmt` (at any depth -- a nested `print(Color(1))`, a call
/// inside a `def` body or a comprehension) whose callee is a bare name in
/// `enum_class_names` and whose argument list carries no keyword, as one
/// `C0001` per call at the call expression's own span, in walk order.
///
/// The walk is [`pycc_ast::visitor::Visitor`] with only `visit_expr`
/// overridden, for the reason `exception::module_references_builtin_exception_name`
/// gives: a hand-rolled match misses positions silently.
///
/// Two deliberate limits, both stated here rather than in a comment:
///
/// - The scan keys on the bare callee *name*, not on scope resolution. A
///   parameter or local that shadows an enum class name and is then called
///   (`def f(Color: int) -> None: Color()`) is reported as this `C0001`
///   rather than as a call on an `int`; such a program is invalid either
///   way, and a top-level function, alias, or import cannot share a class
///   name (`lower_top_level_item`'s collision `C0001`).
/// - A call with a keyword argument (`Color(value=1)`) is skipped: `lower_expr`
///   already reports exactly one `C0001 keyword call arguments are not
///   supported yet` at that call, and the scan runs on failed items too, so
///   without the skip that program would carry two `C0001`s at one span.
///
/// `enum_class_names` is assembled by `lower_module` from the syntactic
/// pre-collection plus every `HirClassDef` with `is_enum` known at scan
/// time (an enum pulled in by a project import), minus the names currently
/// poisoned (D-219). Both extra sets are order-dependent like the poison
/// check itself: an enum imported by a statement *after* a `def` that
/// calls it is unknown when that `def` is scanned, and a call inside a
/// `def` that precedes a failing `class E(Enum): pass` is scanned before
/// `E` is poisoned. In both cases the report is still a true diagnostic --
/// the first falls through to `pycc_types`' span-less guard, the second is
/// reported here.
pub(crate) fn reject_enum_class_calls(stmt: &Stmt, enum_class_names: &[String]) -> Vec<Diagnostic> {
    struct CallScan<'n> {
        enum_class_names: &'n [String],
        diagnostics: Vec<Diagnostic>,
    }
    impl<'a> Visitor<'a> for CallScan<'_> {
        fn visit_expr(&mut self, expr: &'a Expr) {
            if let Expr::Call(call) = expr
                && let Expr::Name(callee) = call.func.as_ref()
                && call.arguments.keywords.is_empty()
                && self
                    .enum_class_names
                    .iter()
                    .any(|n| n == callee.id.as_str())
            {
                self.diagnostics.push(crate::unsupported(
                    enum_class_call_message(callee.id.as_str()),
                    call.range,
                ));
            }
            // Keep walking: an argument may nest another call.
            visitor::walk_expr(self, expr);
        }
    }
    let mut scan = CallScan {
        enum_class_names,
        diagnostics: Vec::new(),
    };
    scan.visit_stmt(stmt);
    scan.diagnostics
}

#[cfg(test)]
mod tests {
    use super::enum_class_call_message;
    use crate::lower_all;
    use pycc_diag::{Diagnostic, Span};

    fn lower_err(source: &str) -> Vec<Diagnostic> {
        let module = crate::pycc_parser_test_helper::parse(source);
        lower_all(&module).expect_err("test fixture should be rejected")
    }

    fn assert_enum_call(diagnostic: &Diagnostic, class_name: &str, call_text: &str, source: &str) {
        assert_eq!(diagnostic.code, "C0001");
        assert_eq!(diagnostic.message, enum_class_call_message(class_name));
        let start = source.find(call_text).unwrap() as u32;
        assert_eq!(
            diagnostic.span,
            Some(Span::new(start, start + call_text.len() as u32))
        );
    }

    const COLOR: &str = "class Color(Enum):\n    RED = 1\n    GREEN = 2\n";

    #[test]
    fn a_module_level_zero_argument_call_is_rejected_at_the_call() {
        let source = format!("{COLOR}c = Color()\n");
        let diagnostics = lower_err(&source);
        assert_eq!(diagnostics.len(), 1);
        assert_enum_call(&diagnostics[0], "Color", "Color()", &source);
    }

    #[test]
    fn a_value_lookup_call_inside_a_function_body_is_rejected() {
        let source = format!("{COLOR}def f() -> None:\n    c = Color(1)\n    print(c.value)\n");
        let diagnostics = lower_err(&source);
        assert_eq!(diagnostics.len(), 1);
        assert_enum_call(&diagnostics[0], "Color", "Color(1)", &source);
    }

    #[test]
    fn calls_before_and_after_the_class_are_both_found_in_loop_order() {
        // The `def` precedes the class in source, so only the syntactic
        // pre-collection can know `Color` when the `def` is scanned.
        let source = format!("def f() -> None:\n    Color(1)\n{COLOR}Color(2)\n");
        let diagnostics = lower_err(&source);
        assert_eq!(diagnostics.len(), 2);
        assert_enum_call(&diagnostics[0], "Color", "Color(1)", &source);
        assert_enum_call(&diagnostics[1], "Color", "Color(2)", &source);
    }

    #[test]
    fn a_nested_call_is_found() {
        let source = format!("{COLOR}print(Color(1))\n");
        let diagnostics = lower_err(&source);
        assert_eq!(diagnostics.len(), 1);
        assert_enum_call(&diagnostics[0], "Color", "Color(1)", &source);
    }

    #[test]
    fn a_non_enum_class_call_and_a_member_access_are_untouched() {
        let source = format!(
            "class P:\n    def __init__(self, x: int) -> None:\n        self.x = x\n{COLOR}p = P(1)\nc = Color.RED\nprint(c.value)\n"
        );
        let module = crate::pycc_parser_test_helper::parse(&source);
        lower_all(&module).expect("no enum class is called");
    }

    #[test]
    fn a_str_enum_class_call_is_rejected_the_same_way() {
        let source = "class S(StrEnum):\n    A = \"a\"\ns = S(\"a\")\n";
        let diagnostics = lower_err(source);
        assert_eq!(diagnostics.len(), 1);
        assert_enum_call(&diagnostics[0], "S", "S(\"a\")", source);
    }

    #[test]
    fn a_docstring_only_enum_class_call_is_rejected() {
        // #744 accepts a member-less enum; `enum_members` is empty, so only
        // the provenance marker (`is_enum`/the syntactic set) can catch it.
        let source = "class E(Enum):\n    \"doc\"\ne = E()\n";
        let diagnostics = lower_err(source);
        assert_eq!(diagnostics.len(), 1);
        assert_enum_call(&diagnostics[0], "E", "E()", source);
    }

    #[test]
    fn a_call_to_a_poisoned_enum_class_is_a_suppressed_cascade() {
        // `class E(Enum): pass` fails to lower and poisons `E` (D-219); the
        // later `E()` is a consequence of that skip, not a second gap.
        let source = "class E(Enum):\n    pass\ne = E()\n";
        let diagnostics = lower_err(source);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert_eq!(diagnostics[0].code, "C0001");
        assert_ne!(diagnostics[0].message, enum_class_call_message("E"));
    }

    #[test]
    fn a_keyword_call_keeps_exactly_the_keyword_diagnostic() {
        let source = format!("{COLOR}c = Color(value=1)\n");
        let diagnostics = lower_err(&source);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert_eq!(diagnostics[0].code, "C0001");
        let message = &diagnostics[0].message;
        assert!(message.contains("keyword call arguments"), "{message}");
    }

    #[test]
    fn a_class_whose_base_is_not_a_bare_name_is_not_an_enum_class() {
        // `class Color(enum.Enum):` takes the pre-collection's non-`Name`
        // arm and is rejected by `lower_class`'s own base-shape `C0001`;
        // the later `Color()` is then a poisoned-name cascade.
        let source = "class Color(enum.Enum):\n    RED = 1\nc = Color()\n";
        let diagnostics = lower_err(source);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        let message = &diagnostics[0].message;
        assert!(
            message.contains("a base class must be a bare name"),
            "{message}"
        );
    }
}
