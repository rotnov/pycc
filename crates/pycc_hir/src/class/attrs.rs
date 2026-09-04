//! Class-body attribute lowering and collision checking, extracted from
//! [`super::body`] (D-185: decompose the part a change touches).
//!
//! `super::body` owns the statement walk itself -- classifying each statement
//! and lowering methods. This module owns everything specific to a class-level
//! *attribute*: stripping a `ClassVar[...]` wrapper, lowering an annotated
//! declaration into a `(name, type, constant value)` entry, extracting the
//! compile-time constant from its right-hand side, and rejecting a class
//! attribute whose name collides with something else the class exposes.

use super::{ClassAnnotationInfo, ClassAttrValue, HirClassDef, PropertyDef, is_scalar_slot_type};
use crate::{Ty, unsupported};
use pycc_ast::{Expr, Number};
use pycc_diag::Diagnostic;

/// #911: Strips a class-body-only `ClassVar[...]` wrapper from an annotation.
///
/// Returns the inner annotation and whether a wrapper was present. Unlike
/// `Final`/`Annotated`, `ClassVar` is **not** unwrapped by the shared
/// `pycc_hir::func::annotation_to_ty` -- it is valid only here, on a class
/// body attribute declaration, so `annotation_to_ty` rejects it outright and
/// this is the one caller that strips it first.
///
/// A bare `ClassVar` (no subscript) and a multi-argument `ClassVar[T, U]` are
/// both `C0001`, mirroring `Final`'s own "takes exactly one type argument".
pub(super) fn strip_class_var(annotation: &Expr) -> Result<(&Expr, bool), Diagnostic> {
    match annotation {
        Expr::Name(name) if name.id.as_str() == "ClassVar" => Err(unsupported(
            "a bare `ClassVar` is not a valid annotation -- write `ClassVar[<type>]` with \
             exactly one type argument",
            pycc_ast::expr_range(annotation),
        )),
        Expr::Subscript(sub) if matches!(sub.value.as_ref(), Expr::Name(n) if n.id.as_str() == "ClassVar") =>
        {
            if matches!(sub.slice.as_ref(), Expr::Tuple(_)) {
                return Err(unsupported(
                    "ClassVar takes exactly one type argument",
                    pycc_ast::expr_range(&sub.slice),
                ));
            }
            Ok((sub.slice.as_ref(), true))
        }
        other => Ok((other, false)),
    }
}

/// #911: Lowers one annotated class-body attribute declaration
/// (`MIN_WIDTH: int = -1024`, `LIMIT: ClassVar[int] = 8`) into a
/// `(name, type, constant value)` entry for [`HirClassDef::class_attrs`].
///
/// `annotation` is the `ClassVar`-stripped annotation; `already` is the
/// entries accumulated so far in this body, for duplicate detection.
///
/// **Named invariant -- class attributes are restricted to scalar slot
/// types.** Beyond D-154's single-word storage constraint, this is what
/// keeps `__set_name__` untriggerable per #585/D-213: rejecting a
/// non-scalar annotation rejects a descriptor-valued class attribute
/// (`x: SomeDescriptor = SomeDescriptor()`) along with it, so
/// `__set_name__`'s own precondition never arises. Relaxing this
/// restriction requires revisiting #585 in the same change.
pub(super) fn lower_class_attr(
    ann: &pycc_ast::StmtAnnAssign,
    annotation: &Expr,
    class_name: &str,
    type_param: Option<&str>,
    aliases: &[(String, Ty)],
    class_name_defs: &[ClassAnnotationInfo],
    already: &[(String, Ty, ClassAttrValue)],
) -> Result<(String, Ty, ClassAttrValue), Diagnostic> {
    let Expr::Name(target_name) = ann.target.as_ref() else {
        return Err(unsupported(
            "a class-level attribute annotation must target a bare name (`X: int = 1`), not an \
             attribute access, subscript, or other expression",
            pycc_ast::expr_range(&ann.target),
        ));
    };
    let attr_name = target_name.id.to_string();
    if already.iter().any(|(name, _, _)| name == &attr_name) {
        return Err(unsupported(
            format!(
                "class attribute `{attr_name}` is already defined in class `{class_name}` -- \
                 duplicate class attribute names are not allowed"
            ),
            ann.range,
        ));
    }
    // PEP 591 (#383): `Final[X]` unwraps to `X` inside `annotation_to_ty`,
    // carrying no finality with it -- a `Final` class attribute would
    // therefore be silently accepted as an ordinary rebindable one. Class
    // attributes are already write-rejected outright in Part 1, but
    // accepting the spelling would imply a finality guarantee this pass does
    // not model, so `Final[...]` on a class-body attribute stays out of
    // scope (see `docs/TYPE_SYSTEM.md`'s own `Final` scope statement).
    if matches!(annotation, Expr::Name(n) if n.id.as_str() == "Final")
        || matches!(annotation, Expr::Subscript(sub)
            if matches!(sub.value.as_ref(), Expr::Name(n) if n.id.as_str() == "Final"))
    {
        return Err(unsupported(
            format!(
                "`Final` on the class-level attribute `{attr_name}` is not supported yet -- \
                 write a plain scalar annotation (`{attr_name}: int = 1`) instead"
            ),
            ann.range,
        ));
    }
    let attr_ty = crate::annotation_to_ty(
        annotation,
        type_param,
        Some(class_name),
        aliases,
        class_name_defs,
    )?;
    // A type parameter has no compile-time constant value to fold, so a
    // generic class's own `T` is rejected here even though it *is* one of
    // `is_scalar_slot_type`'s accepted types.
    if matches!(attr_ty, Ty::Param(_)) {
        return Err(unsupported(
            format!(
                "class attribute `{attr_name}` is annotated with the type parameter `{}` -- a \
                 class attribute is a compile-time constant, and a type parameter has no \
                 constant value to fold",
                attr_ty.name()
            ),
            ann.range,
        ));
    }
    if !is_scalar_slot_type(&attr_ty) {
        return Err(unsupported(
            format!(
                "class attribute `{attr_name}` has type `{}`, which is not a scalar slot type \
                 -- only `int`, `float`, `bool`, and `str` are supported (a class attribute is \
                 a compile-time constant folded at every read; restricting it to scalars is \
                 also what keeps `__set_name__` untriggerable, see #585)",
                attr_ty.name()
            ),
            ann.range,
        ));
    }
    let Some(value) = &ann.value else {
        return Err(unsupported(
            format!(
                "class attribute `{attr_name}` has no value -- a class attribute is a \
                 compile-time constant and must be initialized with a literal \
                 (`{attr_name}: int = 1`)"
            ),
            ann.range,
        ));
    };
    let attr_value = class_attr_value(value, &attr_ty, &attr_name, ann.range.into())?;
    Ok((attr_name, attr_ty, attr_value))
}

/// #911: Extracts the compile-time constant value of a class attribute from
/// its right-hand side, checking it against the declared annotation.
///
/// Accepted shapes are deliberately wider than the enum-member extractor's
/// (`class/enum_class.rs`): a unary `+`/`-` applied to a numeric literal is
/// accepted too, because the motivating example in #885 is
/// `MIN_WIDTH: int = -1024`, which parses as `UnaryOp(USub,
/// NumberLiteral(1024))` and not as a literal at all.
fn class_attr_value(
    value: &Expr,
    attr_ty: &Ty,
    attr_name: &str,
    range: std::ops::Range<u32>,
) -> Result<ClassAttrValue, Diagnostic> {
    let bad_shape = || {
        unsupported(
            format!(
                "class attribute `{attr_name}` must be initialized with a literal -- only an \
                 `int`, `float`, `str`, or `bool` literal (optionally with a unary `+`/`-` on a \
                 number) is supported, because a class attribute is a compile-time constant"
            ),
            range.clone(),
        )
    };
    let mismatch = |found: &str| {
        unsupported(
            format!(
                "class attribute `{attr_name}` is annotated `{}` but is initialized with a \
                 `{found}` literal",
                attr_ty.name()
            ),
            range.clone(),
        )
    };
    // Unary `+`/`-` on a numeric literal, unwrapped to a signed number.
    let (negate, literal) = match value {
        Expr::UnaryOp(unary) => {
            let sign = match unary.op {
                pycc_ast::UnaryOp::USub => true,
                pycc_ast::UnaryOp::UAdd => false,
                _ => return Err(bad_shape()),
            };
            if !matches!(unary.operand.as_ref(), Expr::NumberLiteral(_)) {
                return Err(bad_shape());
            }
            (sign, unary.operand.as_ref())
        }
        other => (false, other),
    };
    match literal {
        Expr::NumberLiteral(number) => match &number.value {
            Number::Int(i) => {
                let Some(magnitude) = i.as_i64() else {
                    return Err(unsupported(
                        format!(
                            "class attribute `{attr_name}` has an integer value that does not \
                             fit in i64 -- only i64-range values are supported"
                        ),
                        range,
                    ));
                };
                let signed = if negate { -magnitude } else { magnitude };
                match attr_ty {
                    Ty::Int => Ok(ClassAttrValue::Int(signed)),
                    // An `int` literal under a `float` annotation widens,
                    // matching Python's own numeric tower (`x: float = 1`).
                    Ty::Float => Ok(ClassAttrValue::Float(signed as f64)),
                    _ => Err(mismatch("int")),
                }
            }
            Number::Float(f) => {
                let signed = if negate { -*f } else { *f };
                match attr_ty {
                    Ty::Float => Ok(ClassAttrValue::Float(signed)),
                    _ => Err(mismatch("float")),
                }
            }
            Number::Complex { .. } => Err(bad_shape()),
        },
        // `negate` is `true` only when the operand was a `NumberLiteral`
        // (checked above), so no sign can reach these two arms.
        Expr::BooleanLiteral(b) => match attr_ty {
            Ty::Bool => Ok(ClassAttrValue::Bool(b.value)),
            _ => Err(mismatch("bool")),
        },
        Expr::StringLiteral(s) => match attr_ty {
            Ty::Str => Ok(ClassAttrValue::Str(s.value.to_str().to_string())),
            _ => Err(mismatch("str")),
        },
        _ => Err(bad_shape()),
    }
}

/// The class-level tables [`reject_class_attr_collisions`] checks a class
/// attribute's name against.
///
/// Grouped into a struct rather than passed positionally: the check needs the
/// class's own attribute, property, and three method tables plus its MRO and
/// the module's class table, which is well past the point where positional
/// arguments stop being readable (and past `clippy::too_many_arguments`).
pub(super) struct ClassAttrCollisionInput<'a> {
    pub class_attrs: &'a [(String, Ty, ClassAttrValue)],
    pub attrs: &'a [(String, Ty)],
    pub properties: &'a [PropertyDef],
    pub methods: &'a [(String, String)],
    pub static_methods: &'a [(String, String)],
    pub class_methods: &'a [(String, String)],
    pub class_name: &'a str,
    pub mro: &'a [String],
    pub defined_classes: &'a [(String, HirClassDef)],
    pub range: std::ops::Range<u32>,
}

/// #911: Rejects a class attribute that collides with an instance attribute
/// slot, a `@property`, or a method of the same name, in either declaration
/// order.
///
/// This runs **after** the body walk rather than at the `AnnAssign` site:
/// `attrs` is populated by `collect_init_attrs` when the walk reaches
/// `__init__`, so a class attribute declared *before* `__init__` would see an
/// empty `attrs` and slip through a statement-site check.
///
/// The direction checked here is exactly one way round: every entry of *this*
/// class's `class_attrs` against this class's own tables and against every MRO
/// base's tables. The reverse direction -- this class's `attrs` (or
/// `properties`) shadowing an *ancestor's* `class_attrs` -- is deliberately
/// **not** checked here, because the write itself is what is ill-formed there,
/// and `pycc_types::class::check_attr_set` already rejects it with `T0044`
/// through `lookup_class_attr_through_mro`'s full-MRO walk, pointing at the
/// offending assignment rather than at the class as a whole.
///
/// A collision is `C0001`, not `T0052`: `T0052`'s existing condition fires
/// only when a redeclaration's `Ty` *differs*, and a same-typed class
/// attribute shadowing an instance slot is just as broken -- the read would
/// fold to a constant while the write targeted a slot.
///
/// #910 added the three method tables to this check. They were missing while
/// only the annotated spelling existed, and the gap is observable in both
/// directions:
///
/// * `class A: f: int = 2` alongside `def f(self)` printed `2` for `a.f`,
///   where CPython's later class-body binding wins and prints a bound method.
/// * `class B(A): f: int = 2` over an inherited `A.f` printed `1` for `b.f()`,
///   where CPython raises `TypeError: 'int' object is not callable`.
///
/// Neither divergence is modellable while a class attribute folds to a
/// constant at every read, so both spellings are rejected outright.
pub(super) fn reject_class_attr_collisions(
    input: &ClassAttrCollisionInput<'_>,
) -> Result<(), Diagnostic> {
    let &ClassAttrCollisionInput {
        class_attrs,
        attrs,
        properties,
        methods,
        static_methods,
        class_methods,
        class_name,
        mro,
        defined_classes,
        ref range,
    } = input;
    for (attr_name, _, _) in class_attrs {
        let own = [
            (
                attrs.iter().any(|(name, _)| name == attr_name),
                "an instance attribute",
            ),
            (
                properties.iter().any(|p| &p.name == attr_name),
                "an `@property`",
            ),
            (
                methods.iter().any(|(name, _)| name == attr_name),
                "a method",
            ),
            (
                static_methods.iter().any(|(name, _)| name == attr_name),
                "a `@staticmethod`",
            ),
            (
                class_methods.iter().any(|(name, _)| name == attr_name),
                "a `@classmethod`",
            ),
        ];
        for (hit, what) in own {
            if hit {
                return Err(class_attr_collision(
                    class_name,
                    attr_name,
                    what,
                    range.clone(),
                ));
            }
        }
        for base in mro.iter().skip(1) {
            // Every class in the MRO was placed there by `compute_c3_mro`,
            // which only references classes from `defined_classes` -- so
            // this lookup always succeeds. `.expect()`'s panic path lives in
            // libcore, outside this crate's instrumented regions (D-014).
            let (_, base_def) = defined_classes
                .iter()
                .find(|(name, _)| name == base)
                .expect("every class in the MRO must be in defined_classes");
            let inherited = [
                (
                    base_def.attrs.iter().any(|(name, _)| name == attr_name),
                    "an instance attribute",
                ),
                (
                    base_def.properties.iter().any(|p| &p.name == attr_name),
                    "an `@property`",
                ),
                (
                    base_def.methods.iter().any(|(name, _)| name == attr_name),
                    "a method",
                ),
                (
                    base_def
                        .static_methods
                        .iter()
                        .any(|(name, _)| name == attr_name),
                    "a `@staticmethod`",
                ),
                (
                    base_def
                        .class_methods
                        .iter()
                        .any(|(name, _)| name == attr_name),
                    "a `@classmethod`",
                ),
            ];
            for (hit, what) in inherited {
                if hit {
                    return Err(class_attr_collision(
                        class_name,
                        attr_name,
                        &format!("{what} inherited from `{base}`"),
                        range.clone(),
                    ));
                }
            }
        }
    }
    Ok(())
}

/// The single `C0001` a [`reject_class_attr_collisions`] collision produces.
fn class_attr_collision(
    class_name: &str,
    attr_name: &str,
    what: &str,
    range: std::ops::Range<u32>,
) -> Diagnostic {
    unsupported(
        format!(
            "class attribute `{class_name}.{attr_name}` collides with {what} of the same \
             name -- a class attribute is folded to a constant at every read, so it can never \
             share a name with a value that lives in an instance slot, behind a descriptor, or \
             in the class's method table"
        ),
        range,
    )
}

#[cfg(test)]
mod tests {
    use crate::lower_checked;

    /// Asserts `source` is rejected with a `C0001` whose message contains
    /// `needle`. `crate::class::tests::assert_c0001` pins only the code, and
    /// every case here needs the *specific* collision partner named.
    fn assert_collision(source: &str, needle: &str) {
        let module = crate::pycc_parser_test_helper::parse(source);
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "C0001", "source: {source:?}");
        assert!(
            diagnostic.message.contains(needle),
            "expected {needle:?} in {:?}",
            diagnostic.message
        );
    }

    // -- #910: a class attribute colliding with the class's own method -----

    #[test]
    fn a_class_attribute_colliding_with_a_method_is_rejected() {
        assert_collision(
            "class C:\n    f: int = 2\n\n    def f(self) -> int:\n        return 1\n",
            "collides with a method",
        );
    }

    #[test]
    fn a_class_attribute_colliding_with_a_static_method_is_rejected() {
        assert_collision(
            "class C:\n    f: int = 2\n\n    @staticmethod\n    def f() -> int:\n        return 1\n",
            "collides with a `@staticmethod`",
        );
    }

    #[test]
    fn a_class_attribute_colliding_with_a_class_method_is_rejected() {
        assert_collision(
            "class C:\n    f: int = 2\n\n    @classmethod\n    def f(cls) -> int:\n        return 1\n",
            "collides with a `@classmethod`",
        );
    }

    // -- #910: the same three collisions against an MRO base ---------------

    #[test]
    fn a_class_attribute_colliding_with_an_inherited_method_is_rejected() {
        assert_collision(
            "class A:\n    def f(self) -> int:\n        return 1\n\n\nclass B(A):\n    f: int = 2\n",
            "collides with a method inherited from `A`",
        );
    }

    #[test]
    fn a_class_attribute_colliding_with_an_inherited_static_method_is_rejected() {
        assert_collision(
            "class A:\n    @staticmethod\n    def f() -> int:\n        return 1\n\n\nclass B(A):\n    f: int = 2\n",
            "collides with a `@staticmethod` inherited from `A`",
        );
    }

    #[test]
    fn a_class_attribute_colliding_with_an_inherited_class_method_is_rejected() {
        assert_collision(
            "class A:\n    @classmethod\n    def f(cls) -> int:\n        return 1\n\n\nclass B(A):\n    f: int = 2\n",
            "collides with a `@classmethod` inherited from `A`",
        );
    }
}
