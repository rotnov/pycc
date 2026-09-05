//! Base-class resolution and C3 method-resolution-order computation
//! (`validate_bases`, `resolve_mro`, and the `compute_c3_mro` algorithm they
//! sit on).
//!
//! Extracted verbatim from `crates/pycc_hir/src/class.rs` per AGENTS.md's
//! file-decomposition rule and D-185's per-file tracking issue (#548): this
//! is one cohesion-driven seam of that 5,332-line file, not a rewrite. Every
//! diagnostic message, every check, and the C3 algorithm itself are
//! unchanged -- the only edits are the ones the module boundary forces
//! (`lower_class`'s two inline blocks became two named functions taking the
//! class-header `range` explicitly, and the circular-inheritance check spells
//! its `Vec<String>` membership test as `iter().any(...)` now that
//! `class_name` arrives as a `&str` rather than an owned `String`).
//!
//! The seam is "which bases are legal, and what order do they linearize
//! into". Everything upstream of it stays in `class.rs`: parsing a class
//! header's base expressions into names (including the duplicate-base
//! rejection), the `Enum`/`Protocol`/`ABC` marker-base consumption, and the
//! generic-class-with-bases gate -- that last one is a PEP 695 type-parameter
//! restriction that happens to read `bases.is_empty()`, not a statement about
//! base resolution, so it deliberately stays with the other `type_param`
//! checks.

use crate::{HirClassDef, unsupported};
use pycc_diag::Diagnostic;

/// Validates every direct base of the class being lowered against the
/// classes already defined earlier in the same module (#432).
///
/// Rejects, in order, an unknown base (a name not defined earlier in the
/// module), an enum base class (#941), a PEP 695 generic base class, and
/// circular inheritance (the base already lists this class in its own MRO).
/// Each rejection is a `C0001` capability diagnostic spanning the class
/// header's `range`.
///
/// The enum-base rejection keys on `HirClassDef::is_enum` (the #921
/// provenance flag), so a `StrEnum`-derived base and a member-less
/// docstring-only enum (#744) are caught alike, and a class whose base was
/// itself rejected for extending an enum never gets far enough to be
/// checked. Without it, D-225's `ensure_init` synthesizes an empty
/// constructor for the subclass and instantiating it aborts the compiled
/// program at run time (`value`/`name` slots never filled). The wording
/// follows `docs/DIAGNOSTICS.md`'s C0001 convention: a base *with* members
/// names the `TypeError` CPython itself raises for the same program, while
/// a member-less base -- which CPython does allow extending -- is reported
/// as not supported yet.
///
/// `defined_classes` maps each already-defined class name to its own
/// `HirClassDef`, in source order.
pub(super) fn validate_bases(
    class_name: &str,
    bases: &[String],
    defined_classes: &[(String, HirClassDef)],
    range: std::ops::Range<u32>,
) -> Result<(), Diagnostic> {
    // #432: validate each base class against the already-defined classes.
    for base_name in bases {
        let Some(base_def) = defined_classes.iter().find(|(name, _)| name == base_name) else {
            // The message is built in `module` so #867's cascade classifier
            // can parse it back (D-219).
            return Err(unsupported(
                crate::module::unknown_base_message(class_name, base_name),
                range.clone(),
            ));
        };
        // #941: an enum class is not a real base. CPython refuses to extend
        // an enum that has members; a member-less enum is extensible in
        // CPython but `lower_enum_class` only fires on a direct
        // `Enum`/`StrEnum` marker base, so that shape is unsupported here.
        if base_def.1.is_enum {
            let message = if base_def.1.enum_members.is_empty() {
                format!(
                    "class `{class_name}` cannot inherit from member-less enum class \
                     `{base_name}` -- extending an enum class that has no members is not \
                     supported yet"
                )
            } else {
                format!(
                    "class `{class_name}` cannot inherit from enum class `{base_name}` -- \
                     CPython raises `TypeError: <enum '{class_name}'> cannot extend <enum \
                     '{base_name}'>` because an enum class with members cannot be extended"
                )
            };
            return Err(unsupported(message, range.clone()));
        }
        // Generic classes (from #387) with a type_param cannot be used as
        // base classes yet.
        if base_def.1.type_param.is_some() {
            return Err(unsupported(
                format!(
                    "class `{class_name}` cannot inherit from generic class `{base_name}` -- \
                     generic classes as bases are not supported yet"
                ),
                range.clone(),
            ));
        }
        // Reject circular inheritance: if the base class already lists this
        // class in its own MRO, then inheriting from it would create a
        // cycle.
        if base_def.1.mro.iter().any(|name| name == class_name) {
            return Err(unsupported(
                format!(
                    "class `{class_name}` cannot inherit from `{base_name}` -- circular \
                     inheritance is not supported"
                ),
                range.clone(),
            ));
        }
    }
    Ok(())
}

/// Computes the class's C3 linearization, mapping an impossible merge onto
/// the `C0001` "inconsistent method resolution order" diagnostic (#432).
///
/// A `None` from `compute_c3_mro` means the inheritance order is
/// inconsistent (a C3 conflict), which is rejected as a circular/inconsistent
/// inheritance error.
pub(super) fn resolve_mro(
    class_name: &str,
    bases: &[String],
    defined_classes: &[(String, HirClassDef)],
    range: std::ops::Range<u32>,
) -> Result<Vec<String>, Diagnostic> {
    compute_c3_mro(class_name, bases, defined_classes).ok_or_else(|| {
        unsupported(
            format!(
                "class `{class_name}` has an inconsistent method resolution order (MRO) -- \
                 the C3 linearization of its base classes is impossible"
            ),
            range,
        )
    })
}

/// Computes the C3 linearization (MRO) for a class with the given name and
/// direct bases, using the already-defined classes' own MROs. This is the
/// standard C3 algorithm from <https://en.wikipedia.org/wiki/C3_linearization>:
///
/// `L[C] = C + merge(L[B1], L[B2], ..., [B1, B2, ...])`
///
/// where `merge` repeatedly takes the head of the first non-empty list that
/// does not appear in the tail of any other list, and appends it to the
/// result. If no such element exists, the linearization is impossible (a
/// conflicting inheritance order) and `None` is returned.
///
/// `defined_classes` maps each already-defined class name to its own
/// `HirClassDef` (which carries its own `mro`). Every base in `bases` must
/// already be present in `defined_classes` -- `lower_class` validates this
/// before calling this function.
fn compute_c3_mro(
    class_name: &str,
    bases: &[String],
    defined_classes: &[(String, HirClassDef)],
) -> Option<Vec<String>> {
    if bases.is_empty() {
        return Some(vec![class_name.to_string()]);
    }
    // Collect each base's own MRO (already computed, since bases must be
    // defined before the derived class).
    let mut sequences: Vec<Vec<String>> = Vec::with_capacity(bases.len() + 1);
    for base_name in bases {
        let base_def = defined_classes
            .iter()
            .find(|(name, _)| name == base_name)
            .map(|(_, def)| def)
            .expect("base class must be defined before the derived class");
        sequences.push(base_def.mro.clone());
    }
    // The last sequence is the list of base names themselves.
    sequences.push(bases.to_vec());
    let mut result = vec![class_name.to_string()];
    loop {
        // Remove empty sequences.
        sequences.retain(|s| !s.is_empty());
        if sequences.is_empty() {
            return Some(result);
        }
        // Find the first head that does not appear in the tail of any
        // other sequence.
        let mut chosen: Option<String> = None;
        for seq in &sequences {
            let candidate = &seq[0];
            let in_tail = sequences
                .iter()
                .any(|s| s.iter().skip(1).any(|elem| elem == candidate));
            if !in_tail {
                chosen = Some(candidate.clone());
                break;
            }
        }
        let Some(candidate) = chosen else {
            // C3 linearization is impossible -- a conflicting inheritance
            // order. This is a "circular or inconsistent MRO" error.
            return None;
        };
        result.push(candidate.clone());
        // Remove the chosen element from the head of every sequence that
        // starts with it.
        for seq in &mut sequences {
            if seq[0] == candidate {
                seq.remove(0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::HirClassDef;
    use crate::class::tests::lower_ok;
    use crate::lower_checked;

    // -- #432: inheritance, C3 MRO, @override, inherited __init__ -----------

    #[test]
    fn single_inheritance_produces_correct_mro() {
        let hir = lower_ok(
            "class A:\n    def __init__(self) -> None:\n        return\n    def f(self) -> int:\n        return 1\nclass B(A):\n    def __init__(self) -> None:\n        return\n",
        );
        let (_, b_def) = &hir.class_defs[1];
        assert_eq!(b_def.bases, vec!["A".to_string()]);
        assert_eq!(b_def.mro, vec!["B".to_string(), "A".to_string()]);
    }

    #[test]
    fn multiple_inheritance_produces_c3_mro() {
        let hir = lower_ok(
            "class A:\n    def __init__(self) -> None:\n        return\nclass B:\n    def __init__(self) -> None:\n        return\nclass C(A, B):\n    def __init__(self) -> None:\n        return\n",
        );
        let (_, c_def) = &hir.class_defs[2];
        assert_eq!(c_def.bases, vec!["A".to_string(), "B".to_string()]);
        assert_eq!(
            c_def.mro,
            vec!["C".to_string(), "A".to_string(), "B".to_string()]
        );
    }

    #[test]
    fn diamond_inheritance_produces_correct_c3_mro() {
        let hir = lower_ok(
            "class A:\n    def __init__(self) -> None:\n        return\nclass B(A):\n    def __init__(self) -> None:\n        return\nclass C(A):\n    def __init__(self) -> None:\n        return\nclass D(B, C):\n    def __init__(self) -> None:\n        return\n",
        );
        let (_, d_def) = &hir.class_defs[3];
        // C3: D, B, C, A
        assert_eq!(
            d_def.mro,
            vec![
                "D".to_string(),
                "B".to_string(),
                "C".to_string(),
                "A".to_string()
            ]
        );
    }

    #[test]
    fn circular_inheritance_is_rejected() {
        let module = crate::pycc_parser_test_helper::parse(
            "class A(B):\n    def __init__(self) -> None:\n        return\nclass B(A):\n    def __init__(self) -> None:\n        return\n",
        );
        // The first class `A(B)` is rejected because `B` is not yet defined.
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic
                .message
                .contains("inherits from unknown class `B`"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn inheriting_from_a_generic_class_is_rejected() {
        let module = crate::pycc_parser_test_helper::parse(
            "class A[T]:\n    def __init__(self, x: T) -> None:\n        self.x = x\nclass B(A):\n    def __init__(self) -> None:\n        return\n",
        );
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic
                .message
                .contains("cannot inherit from generic class `A`"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn a_class_with_an_unknown_base_is_unsupported() {
        // #432: `class C(Base):` where `Base` is not defined earlier in the
        // module is rejected with C0001 (unknown base class).
        let module = crate::pycc_parser_test_helper::parse(
            "class C(Base):\n    def __init__(self) -> None:\n        return\n",
        );
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic
                .message
                .contains("inherits from unknown class `Base`"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn an_inconsistent_c3_mro_is_rejected() {
        // #432: a classic C3 linearization conflict. `C(A, B)` gives MRO
        // [C, A, B] and `D(B, A)` gives MRO [D, B, A]. `E(C, D)` then has
        // no valid C3 merge: after choosing C and D, the remaining
        // sequences [A, B] and [B, A] have no head that does not appear in
        // the other's tail, so `compute_c3_mro` returns `None`.
        let module = crate::pycc_parser_test_helper::parse(
            "class A:\n    def __init__(self) -> None:\n        return\nclass B:\n    def __init__(self) -> None:\n        return\nclass C(A, B):\n    def __init__(self) -> None:\n        return\nclass D(B, A):\n    def __init__(self) -> None:\n        return\nclass E(C, D):\n    def __init__(self) -> None:\n        return\n",
        );
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic
                .message
                .contains("inconsistent method resolution order (MRO)"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn circular_inheritance_in_mro_is_rejected() {
        // #432: circular inheritance (A's MRO contains B, and B inherits
        // from A) is impossible through normal source-order processing --
        // a base class must be defined before the derived class that
        // inherits from it, so A's MRO can never contain B before B is
        // even defined. This test bypasses `module::lower_all` (which processes
        // classes in source order) and calls `lower_class` directly with a
        // hand-built `defined_classes` whose "A" entry already lists "B"
        // in its MRO, exercising the defensive circular-inheritance check
        // that is otherwise unreachable from any real source program.
        //
        // The `find_map` includes a leading non-class statement so its
        // `_ => None` arm is exercised (not just the `ClassDef` arm),
        // matching this file's own established coverage-gate convention
        // (see e.g. `a_subclass_clone_body_is_substituted` above).
        let module = crate::pycc_parser_test_helper::parse(
            "def _dummy() -> None:\n    return\nclass B(A):\n    def __init__(self) -> None:\n        return\n",
        );
        let def = module
            .body
            .iter()
            .find_map(|stmt| match stmt {
                pycc_ast::Stmt::ClassDef(def) => Some(def),
                _ => None,
            })
            .expect("test fixture must contain a class definition");
        let fake_a = HirClassDef {
            class_attrs: Vec::new(),
            exception_type_tag: None,
            name: "A".to_string(),
            bases: Vec::new(),
            mro: vec!["A".to_string(), "B".to_string()],
            attrs: vec![],
            methods: vec![("__init__".to_string(), "A.__init__".to_string())],
            type_param: None,
            properties: Vec::new(),
            static_methods: Vec::new(),
            class_methods: Vec::new(),
            is_enum: false,
            enum_members: Vec::new(),
            is_dataclass: false,
            dataclass_fields: Vec::new(),
            is_protocol: false,
            runtime_checkable: false,
            protocol_members: Vec::new(),
            abstract_methods: Vec::new(),
            is_abstract: false,
        };
        let defined_classes = vec![("A".to_string(), fake_a)];
        let diagnostic =
            crate::class::lower_class(def, &[], &defined_classes, &[], &[]).unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic
                .message
                .contains("circular inheritance is not supported"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    // -- #941: an enum class cannot be used as a base --------------------------

    /// Asserts the `C0001` `validate_bases` reports for `source` carries
    /// `expected` and spans the offending class header (`header`).
    fn assert_enum_base_rejected(source: &str, header: &str, expected: &str) {
        let module = crate::pycc_parser_test_helper::parse(source);
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic.message.contains(expected),
            "unexpected message: {}",
            diagnostic.message
        );
        let start = source
            .find(header)
            .expect("the fixture contains the class header");
        let span = diagnostic
            .span
            .expect("`validate_bases` spans the class header");
        assert_eq!(
            span.start as usize, start,
            "the diagnostic must start at the class header, got: {span:?}"
        );
    }

    #[test]
    fn inheriting_from_an_enum_with_members_names_cpythons_type_error() {
        // The issue's own shape: `Foo(Color)` where `Color` has members.
        // CPython 3.14 raises `TypeError: <enum 'Foo'> cannot extend
        // <enum 'Color'>` for the same class statement.
        assert_enum_base_rejected(
            "class Color(Enum):\n    RED = 1\nclass Foo(Color):\n    pass\n",
            "class Foo(Color)",
            "class `Foo` cannot inherit from enum class `Color` -- CPython raises `TypeError: \
             <enum 'Foo'> cannot extend <enum 'Color'>`",
        );
    }

    #[test]
    fn inheriting_from_a_member_less_enum_is_not_supported_yet() {
        // `class Base(Enum): pass` is already rejected by `lower_enum_class`
        // (an enum body must contain only member assignments), so the only
        // reachable member-less enum is the docstring-only one (#744).
        // CPython allows extending it, which is why the wording is "not
        // supported yet" rather than CPython's own error.
        assert_enum_base_rejected(
            "class Base(Enum):\n    \"A base.\"\nclass Color(Base):\n    RED = 1\n",
            "class Color(Base)",
            "class `Color` cannot inherit from member-less enum class `Base` -- extending an \
             enum class that has no members is not supported yet",
        );
    }

    #[test]
    fn inheriting_from_a_str_enum_is_rejected_too() {
        // `StrEnum` shares `lower_enum_class` (#892) and therefore the
        // `is_enum` marker the check keys on.
        assert_enum_base_rejected(
            "class Kind(StrEnum):\n    AXIAL = \"axial\"\nclass Sub(Kind):\n    pass\n",
            "class Sub(Kind)",
            "class `Sub` cannot inherit from enum class `Kind`",
        );
    }

    #[test]
    fn a_grand_subclass_of_an_enum_is_rejected_at_the_first_subclass() {
        // `Bar(Foo)` never gets checked: lowering stops at `Foo(Color)`,
        // the first class that names the enum as a base.
        assert_enum_base_rejected(
            "class Color(Enum):\n    RED = 1\nclass Foo(Color):\n    pass\nclass Bar(Foo):\n    pass\n",
            "class Foo(Color)",
            "class `Foo` cannot inherit from enum class `Color`",
        );
    }

    #[test]
    fn an_enum_base_listed_after_an_ordinary_base_is_rejected() {
        // The check runs per base, so the enum need not be the first one.
        assert_enum_base_rejected(
            "class A:\n    def __init__(self) -> None:\n        return\nclass Color(Enum):\n    RED = 1\nclass Foo(A, Color):\n    pass\n",
            "class Foo(A, Color)",
            "class `Foo` cannot inherit from enum class `Color`",
        );
    }

    #[test]
    fn a_non_enum_base_beside_an_enum_class_still_lowers() {
        // The negative: an ordinary base is unaffected by the new check
        // even when an enum class is defined in the same module.
        let hir = lower_ok(
            "class Color(Enum):\n    RED = 1\nclass A:\n    def __init__(self) -> None:\n        return\nclass B(A):\n    pass\n",
        );
        let (_, b_def) = &hir.class_defs[2];
        assert_eq!(b_def.bases, vec!["A".to_string()]);
        assert!(!b_def.is_enum);
    }
}
