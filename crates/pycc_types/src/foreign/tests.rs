//! Unit coverage for the foreign-binding helpers themselves; the
//! end-to-end refusals live in `tests/issue_1080_foreign_object.rs`.

use super::*;
use pycc_hir::ProjectBindingKind;

#[test]
fn lists_only_the_foreign_bindings() {
    let imports = vec![
        ImportBinding::Module {
            local_name: "math".to_string(),
            module: pycc_std::StdModule::Math,
        },
        ImportBinding::Symbol {
            local_name: "sqrt".to_string(),
            module: pycc_std::StdModule::Math,
            symbol: pycc_std::resolve_symbol(pycc_std::StdModule::Math, "sqrt")
                .expect("`math.sqrt` is a registered stdlib symbol"),
        },
        ImportBinding::Project {
            local_name: "helper".to_string(),
            module_path: "pkg.helper".to_string(),
            kind: ProjectBindingKind::Function,
        },
        ImportBinding::Foreign {
            local_name: "numpy".to_string(),
            module_path: "numpy".to_string(),
            item_index: 0,
            span: Span::new(0, 0),
        },
    ];
    assert_eq!(foreign_object_names(&imports), vec!["numpy"]);
}

#[test]
fn the_guard_admits_every_other_type_and_refuses_the_object() {
    assert!(reject_object_read("x", &Ty::Int).is_ok());
    let diagnostic = reject_object_read("numpy", &Ty::Object).expect_err("object is refused");
    assert_eq!(diagnostic.code, "I0404");
    assert!(diagnostic.message.contains("numpy"), "{diagnostic:?}");
}

/// Lowers a source snippet exactly as `crate::module`'s own tests do. No
/// resolver participates, so the snippet must not contain an `import`:
/// a foreign binding is classified by the driver (`src/modules.rs`) and is
/// attached to the lowered module by hand below.
fn lower(source: &str) -> pycc_hir::HirModule {
    let module = pycc_parser::parse(source).expect("test source must parse");
    pycc_hir::lower_checked(&module).expect("test source must lower")
}

fn with_foreign_import(mut hir: pycc_hir::HirModule) -> pycc_hir::HirModule {
    hir.imports.push(ImportBinding::Foreign {
        local_name: "numpy".to_string(),
        module_path: "numpy".to_string(),
        item_index: 0,
        span: Span::new(0, 0),
    });
    hir
}

/// Part 1 of #1026 gave `HirModule::imports` a reader downstream of the
/// type checker -- `pycc_mir::build` splices a `MirItem::ForeignImport` per
/// foreign binding, and `src/main.rs`'s `I0403` gate reads the same list --
/// where before it was fully discharged during HIR lowering.
/// `monomorphize` used to drop the field on the strength of having no such
/// reader, which silently produced an artifact that imported nothing.
///
/// Both of its exits are asserted: the early return taken by a module with
/// no generic function, and the rewriting path taken by one with.
#[test]
fn the_resolved_module_still_carries_its_foreign_imports() {
    for source in [
        "def f(x: int) -> int:\n    return x\n",
        "def f[T](x: T) -> T:\n    return x\n\n\ndef g() -> int:\n    return f(1)\n",
    ] {
        let resolved = crate::check_and_resolve_all_keyed(&with_foreign_import(lower(source)))
            .expect("the fixture type-checks");
        assert_eq!(
            foreign_object_names(&resolved.imports),
            vec!["numpy"],
            "{source}"
        );
    }
}

/// The same lowering helper, with the foreign binding recorded at an
/// arbitrary position rather than always at 0 -- which is what the
/// positional claims below are about.
fn with_foreign_import_at(mut hir: pycc_hir::HirModule, item_index: usize) -> pycc_hir::HirModule {
    hir.imports.push(ImportBinding::Foreign {
        local_name: "numpy".to_string(),
        module_path: "numpy".to_string(),
        item_index,
        span: Span::new(0, 0),
    });
    hir
}

/// The recorded position of the single foreign binding in `imports`.
fn foreign_position(imports: &[ImportBinding]) -> usize {
    imports
        .iter()
        .find_map(|binding| match binding {
            ImportBinding::Foreign { item_index, .. } => Some(*item_index),
            _ => None,
        })
        .expect("the fixture records exactly one foreign binding")
}

/// PR 1c of #1080 review finding 1, the panic arm. `monomorphize` drops
/// every original generic function, so an import recorded after two of
/// them used to survive as index 2 against an item list of length 0 --
/// `pycc_mir::splice_foreign_imports` then panicked in `Vec::insert`
/// ("insertion index (is 2) should be <= len (is 0)"). The position is
/// recomputed to 0, which is both in range and where the import belongs:
/// nothing that preceded it still exists.
#[test]
fn a_foreign_import_after_dropped_generics_is_repositioned_to_the_surviving_prefix() {
    let source = "def _a[T](x: T) -> T:\n    return x\n\n\ndef _b[T](x: T) -> T:\n    return x\n";
    let resolved = crate::check_and_resolve_all_keyed(&with_foreign_import_at(lower(source), 2))
        .expect("the fixture type-checks");
    assert!(resolved.items.is_empty(), "{:?}", resolved.items);
    assert_eq!(foreign_position(&resolved.imports), 0);
}

/// The same finding's silent arm, which is the one that reached an
/// artifact: one dropped generic followed by a surviving `def` left the
/// import at index 1 against a one-item list, so `insert(1, ..)` *appended*
/// it -- emitting the import after the function instead of before it, in
/// violation of D-244 rule 3. The recomputed position is 0, so the import
/// still precedes the item that followed it in the source.
#[test]
fn a_foreign_import_before_a_surviving_item_keeps_preceding_it() {
    let source = "def _a[T](x: T) -> T:\n    return x\n\n\ndef f() -> int:\n    return 1\n";
    let resolved = crate::check_and_resolve_all_keyed(&with_foreign_import_at(lower(source), 1))
        .expect("the fixture type-checks");
    assert_eq!(foreign_position(&resolved.imports), 0);
    let names: Vec<&str> = resolved
        .items
        .iter()
        .filter_map(|item| match item {
            pycc_hir::HirItem::Function { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(names, vec!["f"], "{:?}", resolved.items);
}

/// `monomorphize`'s other exit -- the early return a module with no
/// generic, no generic class and no protocol parameter takes -- returns
/// `hir.items` unchanged, so every recorded position must survive
/// untouched. Asserted rather than argued, because "this exit does not
/// rewrite the list" is exactly the kind of claim a later change breaks.
#[test]
fn the_non_generic_exit_leaves_a_recorded_position_alone() {
    let source = "x = 1\n\n\ndef f() -> int:\n    return 1\n";
    let resolved = crate::check_and_resolve_all_keyed(&with_foreign_import_at(lower(source), 1))
        .expect("the fixture type-checks");
    assert_eq!(foreign_position(&resolved.imports), 1);
    assert_eq!(resolved.items.len(), 2);
}

/// PR 1c of #1080 review finding 2. The pre-seed above cannot supersede a
/// binding the source-order pass makes *later*, so `def json()` followed by
/// `import json` left `json` marked def-rebound, the call gate skipped the
/// `I0404` refusal, and `json()` compiled into a call to the shadowed
/// function -- where CPython raises `TypeError: 'module' object is not
/// callable`. Applying the binding at its recorded position is what refuses
/// it.
#[test]
fn a_foreign_import_supersedes_an_earlier_def_of_the_same_name() {
    let source = "def json() -> int:\n    return 1\n\n\nx = json()\n";
    let hir = with_foreign_import_named(lower(source), "json", 1);
    let diagnostics = crate::check_all(&hir).expect_err("the call must be refused");
    assert!(
        diagnostics.iter().any(|d| d.code == "I0404"),
        "{diagnostics:?}"
    );
}

/// The opposite order is a program CPython *runs*: the `def` executes after
/// the import and rebinds the name to a function, so the call is an
/// ordinary call and must keep compiling. Refusing this one would be an
/// over-rejection introduced by the fix above, which is why both orders are
/// pinned.
#[test]
fn a_def_after_a_foreign_import_rebinds_the_name_and_still_compiles() {
    let source = "def json() -> int:\n    return 1\n\n\nx = json()\n";
    let hir = with_foreign_import_named(lower(source), "json", 0);
    crate::check_all(&hir).expect("the `def` below the import wins");
}

/// As [`with_foreign_import_at`], for a fixture that needs the binding to
/// collide with a name the source itself defines.
fn with_foreign_import_named(
    mut hir: pycc_hir::HirModule,
    local_name: &str,
    item_index: usize,
) -> pycc_hir::HirModule {
    hir.imports.push(ImportBinding::Foreign {
        local_name: local_name.to_string(),
        module_path: local_name.to_string(),
        item_index,
        span: Span::new(0, 0),
    });
    hir
}

/// `unroll_enum_loops` runs *after* `monomorphize` and is the one remaining
/// pass that changes how many items the list holds: a top-level
/// `for c in Color:` becomes one loop-variable assignment plus one body copy
/// per enum member. A position recorded after such a loop therefore has to
/// be recomputed as well, or `pycc_mir::splice_foreign_imports` emits the
/// import *inside* the unrolled sequence -- running it before top-level
/// statements that precede it in source. The non-foreign binding alongside
/// it pins that the remap carries the rest of the table through untouched.
#[test]
fn a_foreign_import_after_an_unrolled_enum_loop_is_repositioned_past_it() {
    let source =
        "class Color(Enum):\n    RED = 1\n    GREEN = 2\nfor c in Color:\n    print(c.value)\n";
    let mut hir = with_foreign_import_at(lower(source), 1);
    hir.imports.push(ImportBinding::Module {
        local_name: "math".to_string(),
        module: pycc_std::StdModule::Math,
    });
    let resolved = crate::check_and_resolve_all_keyed(&hir).expect("the fixture type-checks");
    assert_eq!(resolved.items.len(), 4, "{:?}", resolved.items);
    assert_eq!(foreign_position(&resolved.imports), 4);
    assert!(
        matches!(resolved.imports[1], ImportBinding::Module { .. }),
        "{:?}",
        resolved.imports
    );
}

// ---------------------------------------------------------------------------
// Part 2 of #1026 (#1081): the consumer-side refusals.
//
// Part 1 refused the *read* of a foreign binding, so one test per binding
// covered every derived operation. Part 2 admits the read and refuses each
// consuming site on its own (see this module's `super` doc), so the coverage
// obligation changes shape: every refused consumer is owed a test, and each
// is owed it in **both** producer shapes -- `numpy.pi` itself, and a call to
// an unannotated private helper whose solver-inferred return is `object`
// (`constraints.rs`'s `AttrGet` term). The second shape is what proves a
// refusal keys on the *type* rather than on the producing expression.
// ---------------------------------------------------------------------------

/// The label [`both_producer_shapes`] gives the helper-call shape.
const HELPER_SHAPE: &str = "a private helper returning `object`";

/// The `I0404` phrase a foreign read inside a function body now reports,
/// from `expr.rs`'s `HirExpr::Name` arm.
const FUNCTION_BODY_READ: &str = "using `numpy`, which is bound to a CPython object";

/// `snippet` in each of the two producer shapes: literally, and with every
/// `numpy.pi` replaced by a call to a private helper that returns one.
///
/// The helper carries no return annotation on purpose -- D-137's amendment
/// makes `object` unspellable in one, so a solver-inferred return is the
/// only way a call expression can have this type at all.
///
/// **PR 2a of #1081 narrowed what the second shape proves.** A foreign read
/// inside a function body is refused again (`expr.rs`'s `Name` arm), so the
/// helper's own body is now rejected before any consumer sees its result:
/// the shape pins the narrowing rather than a consumer's type-keyed
/// refusal. It stays in the table because that is exactly the regression a
/// silent re-admission of the read would show up as.
fn both_producer_shapes(snippet: &str) -> [(&'static str, String); 2] {
    [
        ("`numpy.pi`", snippet.to_string()),
        (
            HELPER_SHAPE,
            format!(
                "def _h():\n    return numpy.pi\n\n\n{}",
                snippet.replace("numpy.pi", "_h()")
            ),
        ),
    ]
}

/// The diagnostics `check_all` reports for `source` with `numpy` bound as a
/// foreign object, or `None` when it type-checks.
fn check_foreign(source: &str) -> Option<Vec<pycc_diag::Diagnostic>> {
    crate::check_all(&with_foreign_import(lower(source))).err()
}

/// Asserts that `source` is refused with exactly one diagnostic carrying
/// `code` and mentioning `phrase`. `shape` names the producer shape under
/// test, so a failure says which of the two broke.
fn assert_refused(shape: &str, source: &str, code: &str, phrase: &str) {
    let diagnostics =
        check_foreign(source).unwrap_or_else(|| panic!("must be refused ({shape}): {source}"));
    assert_eq!(
        diagnostics.len(),
        1,
        "{shape}: {diagnostics:?} for {source}"
    );
    assert_eq!(
        diagnostics[0].code, code,
        "{shape}: {diagnostics:?} for {source}"
    );
    assert!(
        diagnostics[0].message.contains(phrase),
        "{shape}: {diagnostics:?} for {source}"
    );
}

/// A discarded attribute load on a CPython object is the one operation
/// Part 2 adds, so at **module scope** it must not be refused.
///
/// This is the positive half of the migration: Part 1's
/// `reject_object_read` refused it unconditionally, and a regression that
/// reinstated it there would be invisible to every refusal test below.
#[test]
fn a_module_scope_attribute_load_on_a_cpython_object_is_admitted() {
    let source = "numpy.pi\n";
    assert!(check_foreign(source).is_none(), "{source}");
}

/// The same load inside a function body is refused (PR 2a of #1081).
///
/// Two things are pinned at once, because one rule answers both: a helper
/// body cannot read a foreign name, and therefore no call expression can
/// carry `object` either. The refusal is what keeps `pycc_codegen`'s
/// `foreign_attr::emit` reachable only from the module-exec entry, whose
/// failure edge is the only one a failed lookup can take, and what restores
/// the compile-time refusal of a helper called *above* its own `import`.
#[test]
fn a_function_body_may_not_read_a_foreign_object() {
    for source in [
        "def _h():\n    return numpy.pi\n\n\n_h()\n",
        "def _h():\n    return numpy.pi\n\n\nx = 1\n",
        "def f() -> None:\n    print(numpy)\n",
    ] {
        assert_refused(HELPER_SHAPE, source, "I0404", FUNCTION_BODY_READ);
    }
}

/// A module-body read placed *above* its own `import` is an unbound name.
///
/// PR 2a of #1081 removed Part 1's pre-seed in `crate::module` for exactly
/// this: with the read admitted, the seed made such a program type-check
/// and trap at run time. `T0021` is also the closer answer -- CPython
/// raises `NameError`.
#[test]
fn a_module_body_read_above_the_import_is_unbound() {
    let mut hir = lower("numpy.pi\n");
    hir.imports.push(ImportBinding::Foreign {
        local_name: "numpy".to_string(),
        module_path: "numpy".to_string(),
        item_index: 1,
        span: Span::new(0, 0),
    });
    let diagnostics = crate::check_all(&hir).expect_err("a read above the import is refused");
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0].code, "T0021", "{diagnostics:?}");
    assert!(
        diagnostics[0].message.contains("`numpy` is not defined"),
        "{diagnostics:?}"
    );
}

/// Every consuming site the refusal migration moved the `I0404` to, in
/// both producer shapes.
///
/// Every snippet here sits at **module scope**, which is the whole set of
/// positions a `Ty::Object` can still occupy after PR 2a of #1081 narrowed
/// the read inside a function body.
///
/// PR 3a of #1082 removed this table's five condition rows -- `if`,
/// `while` and the three comprehension guards -- along with the ten
/// `reject_object_condition` call sites they pinned. A truth test on a
/// CPython object is a supported operation now
/// (`crates/pycc_codegen/src/foreign_len.rs`), so there is no refusal left
/// to assert; `tests/issue_1082_foreign_len_and_truth.rs` asserts the
/// acceptance in its place. What remains here is the rendering, binding,
/// walrus and `match` group, which PR 3a does not touch. The in-function
/// shapes of all five condition sites still report the function-body read
/// refusal, which
/// [`the_in_function_condition_sites_report_the_function_body_read_refusal`]
/// pins unchanged.
#[test]
fn every_module_scope_consuming_site_refuses_a_cpython_object_in_both_producer_shapes() {
    for (phrase, snippet) in [
        // Rendering: `print` and f-string interpolation both route through
        // `reject_unrenderable`.
        ("printing or formatting", "print(numpy.pi)\n"),
        ("binding a CPython object to a name", "x = numpy.pi\n"),
        // PEP 572's walrus reaches the same `check_assignment` guard, so it
        // reports the binding refusal rather than the `T0050` the operand
        // would otherwise draw.
        (
            "binding a CPython object to a name",
            "if (y := numpy.pi):\n    print(1)\n",
        ),
        (
            "matching on a CPython object",
            "match numpy.pi:\n    case 1:\n        print(1)\n",
        ),
    ] {
        for (shape, source) in both_producer_shapes(snippet) {
            assert_refused(shape, &source, "I0404", phrase);
        }
    }
}

/// The five condition sites inside a function body, which is a separate
/// pass in `crate::lib`, all report the function-body read refusal -- one
/// rule, reached before any of them.
///
/// Unchanged by PR 3a of #1082. A truth test on a CPython object is now
/// supported *in a module body*; PR 2a's positional bound is what still
/// stops it here, and only the module-exec entry point has the failure
/// edge a raising `PyObject_IsTrue` takes.
#[test]
fn the_in_function_condition_sites_report_the_function_body_read_refusal() {
    for snippet in [
        "def f() -> None:\n    if numpy.pi:\n        print(1)\n",
        "def f() -> None:\n    while numpy.pi:\n        print(1)\n",
        "def f() -> None:\n    xs = [i for i in range(3) if numpy.pi]\n",
        "def f() -> None:\n    ys = {i for i in range(3) if numpy.pi}\n",
        "def f() -> None:\n    zs = {\"k\": i for i in range(3) if numpy.pi}\n",
    ] {
        assert_refused("`numpy.pi`", snippet, "I0404", FUNCTION_BODY_READ);
    }
}

/// `isinstance` is the one site whose two producer shapes give different
/// answers, so it is pinned separately rather than bent into the table.
///
/// The direct shape reaches `check_isinstance`'s new `I0404` guard. The
/// helper shape never gets there: `isinstance` is a compile-time predicate
/// in pycc and already refuses *any* call expression as its first argument
/// with `C0001`, because evaluating one would lose its side effects. That
/// pre-existing rule is a superset of the new one here, and Part 2 extends
/// rather than replaces it.
#[test]
fn isinstance_refuses_a_cpython_object_and_a_call_expression_for_different_reasons() {
    let source = "if isinstance(numpy.pi, int):\n    print(1)\n";
    assert_refused(
        "`numpy.pi`",
        source,
        "I0404",
        "testing a CPython object with `isinstance`",
    );
    assert_refused(
        "a private helper returning `object`",
        &both_producer_shapes(source)[1].1,
        "C0001",
        "side effects would be lost",
    );
}

/// The consumers Part 2 deliberately leaves to their pre-existing
/// diagnostics, pinned so a later refusal migration cannot silently
/// re-label them.
///
/// `docs/TYPE_SYSTEM.md`'s `object` row enumerates exactly this split: a
/// site that would otherwise *accept* the value needs the new `I0404`,
/// while a site whose existing rule already rejects `object` by name keeps
/// that rule -- a second guard there would be unreachable and untestable.
#[test]
fn the_operations_with_a_pre_existing_refusal_keep_it() {
    for (code, phrase, snippet) in [
        ("T0021", "operator Add is not defined", "x = numpy.pi + 1\n"),
        (
            "T0021",
            "cannot compare",
            "if numpy.pi < 1:\n    print(1)\n",
        ),
        (
            "T0021",
            "unary operator USub is not defined",
            "x = -numpy.pi\n",
        ),
        (
            "T0021",
            "unary operator Not is not defined",
            "if not numpy.pi:\n    print(1)\n",
        ),
        (
            "T0039",
            "tuple element type `object`",
            "t = (numpy.pi, 1)\n",
        ),
        // PR 3b of #1082 admits a subscript *load* but not a slice:
        // `HirExpr::Slice` is a different shape with its own arm, which
        // still reports `T0033`. Pinned here so a later part cannot admit
        // one by admitting the other.
        ("T0033", "does not support slicing", "numpy.pi[0:2]\n"),
    ] {
        assert_refused("`numpy.pi`", snippet, code, phrase);
    }
}

/// PR 3b of #1082: a discarded subscript *load* on a CPython object at
/// module scope is admitted, and so is a consumer that needs a real value
/// out of it.
///
/// This is the positive half of the migration, and the second snippet is
/// the one that matters: a bare `numpy.pi[0]` would still pass if the arm
/// merely stopped reporting `T0033`, while `len(numpy.pi[0])` only
/// type-checks if the arm actually answers `Ty::Object`.
#[test]
fn a_module_scope_subscript_of_a_cpython_object_is_admitted() {
    for source in [
        "numpy.pi[0]\n",
        "print(len(numpy.pi[0]))\n",
        "numpy.pi[1.5]\n",
        "numpy.pi[True]\n",
        "numpy.pi[\"k\"]\n",
    ] {
        assert!(check_foreign(source).is_none(), "{source}");
    }
}

/// Only the four scalars with a `pycc_ext_obj_pack_*` helper may be a key.
///
/// The second row is the one the plan singles out: a second `Ty::Object`
/// key looks like an ordinary foreign value and would reach codegen with no
/// packer at all, so it is refused here by the same rule a second
/// `Ty::Object` *argument* to a method call is.
#[test]
fn a_subscript_key_outside_the_packable_scalars_is_refused() {
    for snippet in [
        "numpy.pi[None]\n",
        "numpy.pi[numpy.e]\n",
        "numpy.pi[[1]]\n",
        "numpy.pi[(1, 2)]\n",
    ] {
        assert_refused(
            "`numpy.pi`",
            snippet,
            "I0404",
            "indexing a CPython object with a",
        );
    }
}

/// C6/K1 of the #1082 plan: binding the *result* of a subscript load to a
/// name is still refused, and the code it reports moved from `T0033`
/// ("`object` does not support indexing", which the load itself no longer
/// draws) to the `check_assignment` entry guard's own `I0404`.
///
/// This is a deliberate scope decision rather than an oversight: admitting
/// the binding needs the name-binding work the rest of #1026 carries, and
/// nothing in PR 3b changes `check_assignment`.
#[test]
fn binding_a_subscript_load_to_a_name_is_still_refused() {
    assert_refused(
        "`numpy.pi`",
        "x = numpy.pi[0]\n",
        "I0404",
        "binding a CPython object to a name",
    );
}

/// K7 of the #1082 plan: a subscript *store* is explicitly out of scope.
///
/// `o[k] = v` is a different HIR shape, refused by `pycc_hir` with `C0001`
/// before this crate ever sees it, so admitting the load cannot admit the
/// store by accident. Pinned so a later part has to change this assertion
/// deliberately.
#[test]
fn a_subscript_store_on_a_cpython_object_is_still_refused() {
    let source = "numpy.pi[0] = 1\n";
    let module = pycc_parser::parse(source).expect("test source must parse");
    let diagnostic = pycc_hir::lower_checked(&module).expect_err("a store target is refused");
    assert_eq!(diagnostic.code, "C0001", "{diagnostic:?}");
    assert!(
        diagnostic
            .message
            .contains("only assigning to a bare-name subscript target"),
        "{diagnostic:?}"
    );
}

/// The solver-side mirror (C4 of the #1082 plan): `constraints.rs`'s own
/// `Subscript` arm lifts a `Ty::Object` base to a `Ty::Object` term, so an
/// unannotated private helper returning one materializes its signature and
/// the user sees the real `I0404` for the in-function read.
///
/// Without the lift the helper's return variable stays unresolved and
/// signature materialization reports a `T0021` asking for an annotation
/// `object` cannot be spelled in (D-137) -- the exact dead end the
/// `AttrGet` arm's own comment describes. The assertion is therefore on
/// *which diagnostic* the user gets, not on the program being admitted:
/// PR 2a's positional bound still refuses the helper body.
#[test]
fn a_private_helper_returning_a_subscript_load_reports_the_read_refusal() {
    assert_refused(
        HELPER_SHAPE,
        "def _h():\n    return numpy.pi[0]\n\n\nx = 1\n",
        "I0404",
        FUNCTION_BODY_READ,
    );
}
