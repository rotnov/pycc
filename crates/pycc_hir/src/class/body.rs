//! The class-body statement walk extracted from [`super::lower_class`]
//! (D-185: decompose the part a change touches).
//!
//! `lower_class` handles a class's *shape* -- decorators, bases, MRO
//! resolution, dataclass validation, and the synthesized `__init__`/`__eq__`/
//! `__repr__` of a `@dataclass`. This module owns the middle phase: the
//! single pass over `def.body` that classifies each statement, lowers every
//! method into a mangled `HirItem::Function`, and accumulates the tables
//! (`methods`, `attrs`, `properties`, `static_methods`, `class_methods`,
//! `dataclass_fields`, `abstract_methods`) that `lower_class` then folds into
//! the finished [`HirClassDef`].
//!
//! Enum and protocol class bodies never reach this walk -- `lower_class`
//! returns through `lower_enum_class`/`lower_protocol_class` before it.

use super::{
    CONTAINER_METHOD_NAMES, ClassAnnotationInfo, HirClassDef, MethodKind, PropertyDef,
    classify_decorator, collect_init_attrs, is_declaration_body, is_scalar_slot_type, lower_method,
};
use crate::{HirItem, Ty, unsupported};
use pycc_ast::{Expr, Stmt};
use pycc_diag::{Diagnostic, Span};

/// The read-only inputs the class-body walk needs from [`super::lower_class`].
///
/// Grouped into a struct rather than passed positionally: the walk takes
/// eight distinct read-only inputs, well past the point where positional
/// arguments stop being readable (and past `clippy::too_many_arguments`).
pub(super) struct ClassBodyInput<'a> {
    /// The class body's statements (`def.body`).
    pub(super) body: &'a [Stmt],
    /// The class's own name, used for method mangling and diagnostics.
    pub(super) class_name: &'a str,
    /// Whether the class carries `@dataclass` (#378).
    pub(super) is_dataclass: bool,
    /// The PEP 695 type parameter name, if the class is generic.
    pub(super) type_param: Option<&'a str>,
    /// Module-level type aliases, for annotation resolution.
    pub(super) aliases: &'a [(String, Ty)],
    /// Class annotation info for every class visible here, including a
    /// self-entry for the class being lowered (PEP 560, #611).
    pub(super) class_name_defs: &'a [ClassAnnotationInfo],
    /// The class's resolved C3 MRO, most-derived first.
    pub(super) mro: &'a [String],
    /// Every class lowered before this one.
    pub(super) defined_classes: &'a [(String, HirClassDef)],
}

/// The tables the class-body walk accumulates, handed back to
/// [`super::lower_class`] for the final [`HirClassDef`].
pub(super) struct ClassBodyOutput {
    /// `(source name, mangled name)` for every regular method.
    pub(super) methods: Vec<(String, String)>,
    /// The lowered `HirItem::Function` for every method in the body.
    pub(super) items: Vec<HirItem>,
    /// Instance attribute slots derived from `__init__` (D-154).
    pub(super) attrs: Vec<(String, Ty)>,
    /// `@property` getters (and their setters).
    pub(super) properties: Vec<PropertyDef>,
    /// `(source name, mangled name)` for every `@staticmethod`.
    pub(super) static_methods: Vec<(String, String)>,
    /// `(source name, mangled name)` for every `@classmethod`.
    pub(super) class_methods: Vec<(String, String)>,
    /// `@dataclass` fields declared in this body, in source order (#378).
    pub(super) dataclass_fields: Vec<(String, Ty)>,
    /// Names of `@abstractmethod`s declared in this body (#380).
    pub(super) abstract_methods: Vec<String>,
}

/// Walks a non-enum, non-protocol class body once, lowering every method and
/// accumulating the class's tables.
///
/// Returns the first `Diagnostic` any statement produces; a class body
/// statement that is not a `def`, a bare `pass`, or a docstring is `C0001`
/// (with the `@dataclass` `AnnAssign` carve-out, #378).
pub(super) fn walk_class_body(input: &ClassBodyInput<'_>) -> Result<ClassBodyOutput, Diagnostic> {
    let &ClassBodyInput {
        body,
        class_name,
        is_dataclass,
        type_param,
        aliases,
        class_name_defs,
        mro,
        defined_classes,
    } = input;
    let mut methods: Vec<(String, String)> = Vec::new();
    let mut items: Vec<HirItem> = Vec::new();
    let mut attrs: Vec<(String, Ty)> = Vec::new();
    let mut properties: Vec<PropertyDef> = Vec::new();
    let mut static_methods: Vec<(String, String)> = Vec::new();
    let mut class_methods: Vec<(String, String)> = Vec::new();
    let mut dataclass_fields: Vec<(String, Ty)> = Vec::new();
    let mut abstract_methods: Vec<String> = Vec::new();
    let mut init_seen = false;
    for stmt in body {
        // #378 (PR-18): a `@dataclass` class body accepts `AnnAssign`
        // (`x: int` or `x: int = default`) alongside method definitions.
        // An annotated field contributes to `dataclass_fields`. A
        // non-dataclass class still rejects `AnnAssign` (class-level
        // attribute declarations are a separate feature, out of scope for
        // this PR).
        if let Stmt::Pass(_) = stmt {
            // `pass` is a no-op in any class body (dataclass or not). A
            // zero-field dataclass (`@dataclass\nclass Empty:\n    pass`)
            // relies on this to have a valid body with no fields and no
            // methods.
            continue;
        }
        // #744: a docstring (a bare string-literal expression statement) is
        // a no-op, matching `validate_init_subclass_body`'s existing
        // precedent for the same construct.
        if let Stmt::Expr(expr_stmt) = stmt
            && matches!(*expr_stmt.value, Expr::StringLiteral(_))
        {
            continue;
        }
        if let Stmt::AnnAssign(ann) = stmt {
            if !is_dataclass {
                return Err(unsupported(
                    "a class body statement must be a method definition (`def ...`) -- no \
                     other statement kind is supported yet",
                    pycc_ast::stmt_range(stmt),
                ));
            }
            // The target must be a single bare name.
            let Expr::Name(target_name) = ann.target.as_ref() else {
                return Err(unsupported(
                    "a dataclass field annotation must target a bare name (`x: int`), not an \
                     attribute access, subscript, or other expression",
                    pycc_ast::expr_range(&ann.target),
                ));
            };
            let field_name = target_name.id.to_string();
            // Reject duplicate field names.
            if dataclass_fields.iter().any(|(name, _)| name == &field_name) {
                return Err(unsupported(
                    format!(
                        "dataclass field `{field_name}` is already defined in class \
                         `{class_name}` -- duplicate field names are not allowed"
                    ),
                    ann.range,
                ));
            }
            let field_ty = crate::annotation_to_ty(
                &ann.annotation,
                type_param,
                Some(class_name),
                aliases,
                class_name_defs,
            )?;
            // #378 (PR-18): a dataclass field's type must be a scalar slot
            // type (int/float/bool/str, or a generic type parameter `T`
            // that is substituted with a scalar at monomorphization time).
            // The instance attribute-slot storage is a single `i64` word
            // per slot (D-154), which has no representation for a heap-
            // object-typed attribute (`list[T]`, `dict[K, V]`, `set[T]`),
            // a by-value `tuple[...]`, `None`, or a class instance
            // (`Ty::Instance`, including a self-referential field like
            // `next: Node` or `next: Self`, which `annotation_to_ty`
            // resolves to `Ty::Instance` -- see its self-referential class
            // name and `Self` arms). Rejecting here, structurally, keeps
            // every field type this PR's own `pycc_codegen`/`pycc_rt` slices
            // actually implement -- matching `slot_ty_from_init_rhs`'s own
            // scalar-only restriction for hand-written `__init__` bodies.
            if !is_scalar_slot_type(&field_ty) {
                return Err(unsupported(
                    format!(
                        "dataclass field `{field_name}` has type `{}`, which is not a scalar \
                         slot type -- only `int`, `float`, `bool`, `str`, or a generic type \
                         parameter is supported as a dataclass field in this version (the \
                         instance attribute-slot storage is a single word per slot, with no \
                         representation for a heap object, tuple, `None`, or class instance)",
                        field_ty.name()
                    ),
                    ann.range,
                ));
            }
            // A field with a default value (`x: int = field(default=...)` or
            // `x: int = 42`) is recognized but rejected with C0001 -- field
            // defaults are deferred to a follow-up issue (the compiler has no
            // optional-parameter mechanism yet). A bare `field()` call with
            // no arguments is also rejected (a field with `field()` and no
            // default is meaningless).
            if let Some(value) = &ann.value {
                // Recognize `field(...)` call shapes specifically, for a
                // clearer diagnostic message.
                if let Expr::Call(call) = value.as_ref()
                    && let Expr::Name(name) = call.func.as_ref()
                    && name.id.as_str() == "field"
                {
                    return Err(unsupported(
                        "dataclass field defaults are not supported yet -- only required \
                         fields are supported in this version (`field(default=...)` and \
                         `field(default_factory=...)` are deferred to a follow-up issue)",
                        ann.range,
                    ));
                }
                return Err(unsupported(
                    "dataclass field defaults are not supported yet -- only required fields \
                     (no default value) are supported in this version",
                    ann.range,
                ));
            }
            dataclass_fields.push((field_name, field_ty));
            continue;
        }
        let Stmt::FunctionDef(method_def) = stmt else {
            return Err(unsupported(
                "a class body statement must be a method definition (`def ...`) -- no \
                 other statement kind is supported yet",
                pycc_ast::stmt_range(stmt),
            ));
        };
        let method_name = method_def.name.as_str().to_string();
        if CONTAINER_METHOD_NAMES.contains(&method_name.as_str()) {
            return Err(unsupported(
                format!(
                    "method name `{method_name}` collides with the compiler's built-in \
                     container-method syntax, not supported yet"
                ),
                method_def.range,
            ));
        }
        // #386: `__init__` redefinition stays C0001 -- the compile-time
        // attribute-slot pre-scan (`collect_init_attrs`) derives slot types
        // from the first `__init__` body's assignments and cannot reconcile
        // a second, different `__init__` body. A non-`__init__` method
        // redefinition is a rebind, not an error (see below).
        if method_name == "__init__" && init_seen {
            return Err(unsupported(
                "redefining `__init__` in the same class body is not supported yet \
                 -- the attribute-slot pre-scan cannot reconcile two different \
                 `__init__` bodies",
                method_def.range,
            ));
        }
        // #378 (PR-18): a `@dataclass` class auto-generates `__init__`,
        // `__eq__`, and `__repr__` -- an explicit definition of any of these
        // is rejected with C0001 (the synthesized method replaces it).
        if is_dataclass && matches!(method_name.as_str(), "__init__" | "__eq__" | "__repr__") {
            return Err(unsupported(
                format!(
                    "a `@dataclass` class auto-generates `{method_name}`; an explicit \
                     `{method_name}` is not allowed in a `@dataclass` body"
                ),
                method_def.range,
            ));
        }
        // #377: classify the method's decorator list to determine whether
        // it is a regular method, a `@property` getter, or a
        // `@<name>.setter` setter. `lower_method` uses this to compute the
        // correct mangled name (a setter uses a `.setter` suffix to avoid
        // colliding with the getter's mangled name, since both share the
        // same source method name).
        let kind = classify_decorator(
            &method_def.decorator_list,
            &method_name,
            method_def.range.into(),
        )?;
        // #436: `@staticmethod` and `@classmethod` on `__init__` are
        // rejected -- a constructor must be a regular instance method.
        // #380 (PR-20): `@abstractmethod` on `__init__` is also rejected
        // -- an abstract `__init__` would prevent instantiation of any
        // subclass, which is not a meaningful pattern in pycc's
        // compile-time-only ABC model.
        if method_name == "__init__"
            && matches!(
                kind,
                MethodKind::StaticMethod | MethodKind::ClassMethod | MethodKind::AbstractMethod
            )
        {
            return Err(unsupported(
                "`@staticmethod`, `@classmethod`, and `@abstractmethod` cannot decorate \
                 `__init__` -- the constructor must be a regular instance method",
                method_def.range,
            ));
        }
        let (item, params) = lower_method(
            method_def,
            class_name,
            type_param,
            aliases,
            &kind,
            class_name_defs,
        )?;
        if method_name == "__init__" {
            init_seen = true;
            attrs = collect_init_attrs(&method_def.body, &params)?;
        }
        match &kind {
            MethodKind::Regular { is_override } => {
                // #432: if `@override` is present, verify the method name
                // exists in at least one base class's methods or
                // properties (walking the MRO, excluding the current class
                // itself). If no matching base method exists, emit T0031.
                if *is_override {
                    let found_in_base = mro.iter().skip(1).any(|mro_class| {
                        // Every class in the MRO (except the first, which is
                        // `class_name` itself and is skipped) was placed there
                        // by `compute_c3_mro`, which only references classes
                        // from `defined_classes` -- so this lookup always
                        // succeeds. Using `.expect()` (whose panic path lives
                        // in libcore, outside this crate's instrumented
                        // regions) instead of a `let .. else { return false
                        // }` avoids a permanently-uncovered else branch under
                        // D-014's 100%-region coverage gate.
                        let (_, base_def) = defined_classes
                            .iter()
                            .find(|(name, _)| name == mro_class)
                            .expect("every class in the MRO must be in defined_classes");
                        base_def
                            .methods
                            .iter()
                            .any(|(name, _)| name == &method_name)
                            || base_def.properties.iter().any(|p| p.name == method_name)
                    });
                    if !found_in_base {
                        return Err(Diagnostic::error(
                            "T0031",
                            format!(
                                "`@override` on method `{class_name}.{method_name}` does not \
                                 override any method or property of the same name in a base \
                                 class"
                            ),
                            Span::new(
                                u32::from(method_def.range.start()),
                                u32::from(method_def.range.end()),
                            ),
                        ));
                    }
                }
                let mangled = format!("{class_name}.{method_name}");
                // #377: reject a regular method whose name collides with an
                // existing property. Both would share the same `<Class>.<name>`
                // mangled symbol, and the stale method table entry would let
                // `obj.name()` (method-call syntax) resolve to the property
                // getter function — silently accepting a call shape that
                // CPython rejects after the property shadows the method.
                if properties.iter().any(|p| p.name == method_name) {
                    return Err(unsupported(
                        format!(
                            "a `@property` named `{method_name}` is already defined in this \
                             class -- a method cannot shadow a property of the same name"
                        ),
                        method_def.range,
                    ));
                }
                // #436: reject a regular method whose name collides with an
                // existing static or class method. Although the mangled
                // names differ (`.static`/`.classmethod` suffix), allowing
                // both would be confusing — the method-call syntax
                // `obj.name()` would resolve to the regular method while
                // `ClassName.name()` would resolve to the static/class
                // method, with no clear indication to the user that these
                // are different functions.
                if static_methods.iter().any(|(name, _)| name == &method_name) {
                    return Err(unsupported(
                        format!(
                            "a `@staticmethod` named `{method_name}` is already defined in \
                             this class -- a regular method cannot share a name with a \
                             `@staticmethod`"
                        ),
                        method_def.range,
                    ));
                }
                if class_methods.iter().any(|(name, _)| name == &method_name) {
                    return Err(unsupported(
                        format!(
                            "a `@classmethod` named `{method_name}` is already defined in \
                             this class -- a regular method cannot share a name with a \
                             `@classmethod`"
                        ),
                        method_def.range,
                    ));
                }
                // #386: rebind semantics for non-`__init__` method
                // redefinition. Both definitions share the same mangled
                // `<ClassName>.<method>` name, so PR #358's function-
                // pointer slot infrastructure already handles the actual
                // rebind at the codegen level (the second `def`'s source-
                // order execution stores the new function's address into
                // the slot). Here, replacing the method table entry on
                // redefinition rather than appending a duplicate keeps the
                // table clean -- the mangled name is the same either way,
                // so `resolve_method_call` and MIR lowering's
                // `.methods.iter().find(..)` resolve identically.
                if let Some(entry) = methods.iter_mut().find(|(name, _)| name == &method_name) {
                    *entry = (method_name.clone(), mangled.clone());
                } else {
                    methods.push((method_name.clone(), mangled));
                }
            }
            // #377: a `@property` getter. The getter's mangled name is
            // `<Class>.<name>` (the same scheme a regular method uses),
            // but it is NOT entered into `methods` -- it is accessed via
            // attribute syntax (`obj.x`), not method-call syntax
            // (`obj.x()`). A duplicate getter for the same property name
            // is rejected (a property is defined once, not rebound).
            MethodKind::PropertyGetter { prop_name } => {
                // #377: reject a property getter whose name collides with an
                // existing method. Both would share the same `<Class>.<name>`
                // mangled symbol, and the method table entry would let
                // `obj.name()` (method-call syntax) resolve to the property
                // getter function — silently accepting a call shape that
                // CPython rejects after the property shadows the method.
                if methods.iter().any(|(name, _)| name == prop_name) {
                    return Err(unsupported(
                        format!(
                            "a method named `{prop_name}` is already defined in this class -- \
                             a `@property` getter cannot shadow a method of the same name"
                        ),
                        method_def.range,
                    ));
                }
                if properties.iter().any(|p| &p.name == prop_name) {
                    return Err(unsupported(
                        format!(
                            "a `@property` getter for `{prop_name}` is already defined in \
                             this class -- redefining a property getter is not supported yet"
                        ),
                        method_def.range,
                    ));
                }
                properties.push(PropertyDef {
                    name: prop_name.clone(),
                    getter: format!("{class_name}.{prop_name}"),
                    setter: None,
                });
            }
            // #377: a `@<name>.setter` setter. The setter's mangled name
            // is `<Class>.<name>.setter` (the `.setter` suffix ensures it
            // cannot collide with the getter's `<Class>.<name>` mangled
            // name, since a real Python identifier can never contain a
            // `.`). The property's getter must already be defined (a
            // setter without a preceding getter is `C0001`), and a
            // duplicate setter for the same property is rejected.
            MethodKind::PropertySetter { prop_name } => {
                let Some(prop) = properties.iter_mut().find(|p| &p.name == prop_name) else {
                    return Err(unsupported(
                        format!(
                            "a `@{prop_name}.setter` decorator requires a preceding \
                             `@property` getter for `{prop_name}` in the same class"
                        ),
                        method_def.range,
                    ));
                };
                if prop.setter.is_some() {
                    return Err(unsupported(
                        format!(
                            "a setter for property `{prop_name}` is already defined in \
                             this class -- redefining a property setter is not supported yet"
                        ),
                        method_def.range,
                    ));
                }
                prop.setter = Some(format!("{class_name}.{prop_name}.setter"));
            }
            // #436: a `@staticmethod`. Registered in `static_methods`
            // (not `methods`) with a `.static` suffix mangled name. A
            // duplicate static method name is a rebind, matching regular
            // method rebind semantics (#386). A static method name must
            // not collide with a regular method, property, or class
            // method of the same name in the same class.
            MethodKind::StaticMethod => {
                if methods.iter().any(|(name, _)| name == &method_name) {
                    return Err(unsupported(
                        format!(
                            "a method named `{method_name}` is already defined in this class \
                             -- a `@staticmethod` cannot share a name with a regular method"
                        ),
                        method_def.range,
                    ));
                }
                if properties.iter().any(|p| p.name == method_name) {
                    return Err(unsupported(
                        format!(
                            "a `@property` named `{method_name}` is already defined in this \
                             class -- a `@staticmethod` cannot share a name with a property"
                        ),
                        method_def.range,
                    ));
                }
                if class_methods.iter().any(|(name, _)| name == &method_name) {
                    return Err(unsupported(
                        format!(
                            "a `@classmethod` named `{method_name}` is already defined in \
                             this class -- a `@staticmethod` cannot share a name with a \
                             `@classmethod`"
                        ),
                        method_def.range,
                    ));
                }
                let mangled = format!("{class_name}.{method_name}.static");
                if let Some(entry) = static_methods
                    .iter_mut()
                    .find(|(name, _)| name == &method_name)
                {
                    *entry = (method_name.clone(), mangled.clone());
                } else {
                    static_methods.push((method_name.clone(), mangled));
                }
            }
            // #436: a `@classmethod`. Registered in `class_methods`
            // (not `methods`) with a `.classmethod` suffix mangled name.
            // A duplicate class method name is a rebind, matching regular
            // method rebind semantics (#386). A class method name must
            // not collide with a regular method, property, or static
            // method of the same name in the same class.
            MethodKind::ClassMethod => {
                if methods.iter().any(|(name, _)| name == &method_name) {
                    return Err(unsupported(
                        format!(
                            "a method named `{method_name}` is already defined in this class \
                             -- a `@classmethod` cannot share a name with a regular method"
                        ),
                        method_def.range,
                    ));
                }
                if properties.iter().any(|p| p.name == method_name) {
                    return Err(unsupported(
                        format!(
                            "a `@property` named `{method_name}` is already defined in this \
                             class -- a `@classmethod` cannot share a name with a property"
                        ),
                        method_def.range,
                    ));
                }
                if static_methods.iter().any(|(name, _)| name == &method_name) {
                    return Err(unsupported(
                        format!(
                            "a `@staticmethod` named `{method_name}` is already defined in \
                             this class -- a `@classmethod` cannot share a name with a \
                             `@staticmethod`"
                        ),
                        method_def.range,
                    ));
                }
                let mangled = format!("{class_name}.{method_name}.classmethod");
                if let Some(entry) = class_methods
                    .iter_mut()
                    .find(|(name, _)| name == &method_name)
                {
                    *entry = (method_name.clone(), mangled.clone());
                } else {
                    class_methods.push((method_name.clone(), mangled));
                }
            }
            // #380 (PR-20, PEP 3119): an `@abstractmethod`. Registered
            // in `methods` (it is still a regular method for dispatch
            // purposes — a subclass overrides it with a regular method of
            // the same name) AND in `abstract_methods` (so `lower_class`
            // can verify concrete subclasses override every inherited
            // abstract method). The method body must be declaration-style
            // (`...` or `pass`).
            MethodKind::AbstractMethod => {
                // Verify the method body is declaration-style (`...` or
                // `pass`). A non-declaration body is rejected with C0001
                // — an abstract method with an implementation is a
                // contradiction in pycc's compile-time-only ABC model.
                if !is_declaration_body(&method_def.body) {
                    return Err(unsupported(
                        format!(
                            "an `@abstractmethod` `{class_name}.{method_name}` must have a \
                             declaration-style body (`...` or `pass`), not an implementation"
                        ),
                        method_def.range,
                    ));
                }
                let mangled = format!("{class_name}.{method_name}");
                if let Some(entry) = methods.iter_mut().find(|(name, _)| name == &method_name) {
                    *entry = (method_name.clone(), mangled.clone());
                } else {
                    methods.push((method_name.clone(), mangled));
                }
                abstract_methods.push(method_name.clone());
            }
        }
        items.push(item);
    }
    Ok(ClassBodyOutput {
        methods,
        items,
        attrs,
        properties,
        static_methods,
        class_methods,
        dataclass_fields,
        abstract_methods,
    })
}

#[cfg(test)]
mod tests {
    use crate::class::tests::{assert_c0001, lower_ok};
    use crate::{HirItem, lower_checked};

    #[test]
    fn a_non_def_class_body_statement_is_unsupported() {
        assert_c0001("class C:\n    x: int\n");
    }

    #[test]
    fn redefining_init_in_one_class_body_is_unsupported() {
        // #386: `__init__` redefinition stays C0001 -- the compile-time
        // attribute-slot pre-scan (`collect_init_attrs`) cannot reconcile
        // two different `__init__` bodies.
        assert_c0001(
            "class C:\n    def __init__(self) -> None:\n        return\n    def __init__(self) -> None:\n        return\n",
        );
    }

    #[test]
    fn redefining_a_non_init_method_rebinds_to_the_latest_definition() {
        // #386: a non-`__init__` method redefinition is a rebind, not an
        // error. Both definitions lower into separate `HirItem::Function`s
        // with the same mangled name (`C.foo`), and the method table entry
        // is replaced (not duplicated) -- so `methods` has exactly one
        // `foo` entry, while `items` has two `C.foo` function items.
        let hir = lower_ok(
            "class C:\n    def __init__(self) -> None:\n        return\n    def foo(self) -> None:\n        return\n    def foo(self) -> None:\n        return\n",
        );
        assert_eq!(hir.class_defs.len(), 1);
        let (_, class_def) = &hir.class_defs[0];
        // The method table has exactly one `foo` entry (replaced, not
        // duplicated), plus the `__init__` entry.
        assert_eq!(
            class_def.methods,
            vec![
                ("__init__".to_string(), "C.__init__".to_string()),
                ("foo".to_string(), "C.foo".to_string()),
            ]
        );
        // Both definitions are lowered as separate `HirItem::Function`s
        // with the same mangled name -- PR #358's function-pointer slot
        // handles the rebind at the codegen level. Using `matches!` rather
        // than an `if let .. { true } else { false }` keeps the closure
        // branch-free under D-014's 100%-region coverage gate (every item
        // in this fixture is a `HirItem::Function`, so an `else { false }`
        // arm would be a permanently uncovered region).
        let foo_items: Vec<&HirItem> = hir
            .items
            .iter()
            .filter(|item| matches!(item, HirItem::Function { name, .. } if name == "C.foo"))
            .collect();
        assert_eq!(foo_items.len(), 2, "both foo definitions should be lowered");
    }

    #[test]
    fn an_ordinary_class_with_a_docstring_lowers_successfully() {
        // #744: a class docstring (a bare string-literal expression
        // statement) is a no-op in an ordinary (non-dataclass) class body.
        let hir = lower_ok(
            "class C:\n    \"A class.\"\n    def __init__(self) -> None:\n        return\n",
        );
        assert_eq!(hir.class_defs.len(), 1);
    }

    #[test]
    fn an_ordinary_class_with_a_non_leading_docstring_lowers_successfully() {
        // #744's guard has no position check: a bare string-literal
        // expression statement is a no-op anywhere in the body, not only
        // when it appears first. Place it after `__init__` to exercise
        // that non-leading position directly, rather than only inferring
        // it from the loop structure.
        let hir = lower_ok(
            "class C:\n    def __init__(self) -> None:\n        return\n    \"A class.\"\n",
        );
        assert_eq!(hir.class_defs.len(), 1);
    }

    #[test]
    fn a_non_string_expression_statement_in_a_class_body_is_still_rejected() {
        // #744's docstring exemption covers only a bare string-literal
        // expression statement: a bare non-string expression statement in a
        // class body remains C0001, distinguishing it from the docstring
        // no-op added alongside it.
        assert_c0001("class C:\n    42\n    def __init__(self) -> None:\n        return\n");
    }

    #[test]
    fn a_method_named_get_collides_with_the_container_method_syntax() {
        // D-068 review finding on #385: without `CONTAINER_METHOD_NAMES`'s
        // own rejection, `buf.get(5)` below would hit `expr.rs`'s
        // hand-recognized dict-`.get()` fast path first (no type
        // information is available at that lowering step to know `buf` is
        // actually a `Buf` instance) and fail with the confusing "`.get()`
        // is only supported as `dict.get(key, default)` with exactly two
        // arguments so far, got 1" message instead of ever reaching `Buf`'s
        // own `get` method. Asserting the
        // *exact* message, not just the `C0001` code, is what actually
        // distinguishes "rejected with the new, clear diagnostic" from
        // "rejected with the old, confusing one" -- both are `C0001`.
        let module = crate::pycc_parser_test_helper::parse(
            "class Buf:\n    def __init__(self) -> None:\n        return\n    def get(self, k: int) -> int:\n        return k\n\nbuf = Buf()\nbuf.get(5)\n",
        );
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic.message.contains(
                "method name `get` collides with the compiler's built-in container-method syntax"
            ),
            "unexpected message: {}",
            diagnostic.message
        );
        assert!(
            !diagnostic.message.contains("exactly two arguments"),
            "the confusing container-method message must not resurface: {}",
            diagnostic.message
        );
    }

    #[test]
    fn a_method_named_append_pop_or_add_is_also_rejected() {
        // Same collision as `a_method_named_get_collides_with_the_container_method_syntax`
        // above, exercised for the remaining three names `CONTAINER_METHOD_NAMES`
        // guards against -- each with its own deliberately-mismatched-arity
        // call site (mirroring the `get` test's own `buf.get(5)`), so the
        // old, confusing container-method message (asserted absent below)
        // is actually reachable pre-fix, not merely untriggered.
        let cases = [
            (
                "append",
                "c.append()",
                "list.append() takes exactly one argument, got 0",
            ),
            ("pop", "c.pop(1)", "list.pop() takes no arguments, got 1"),
            (
                "add",
                "c.add()",
                "set.add() takes exactly one argument, got 0",
            ),
        ];
        for (name, call, old_message) in cases {
            let source = format!(
                "class C:\n    def __init__(self) -> None:\n        return\n    def {name}(self) -> None:\n        return\n\nc = C()\n{call}\n"
            );
            let module = crate::pycc_parser_test_helper::parse(&source);
            let diagnostic = lower_checked(&module).unwrap_err();
            assert_eq!(diagnostic.code, "C0001", "name: {name}");
            assert!(
                diagnostic.message.contains(&format!(
                    "method name `{name}` collides with the compiler's built-in container-method syntax"
                )),
                "name: {name}, message: {}",
                diagnostic.message
            );
            assert!(
                !diagnostic.message.contains(old_message),
                "the confusing container-method message must not resurface, name: {name}, message: {}",
                diagnostic.message
            );
        }
    }

    #[test]
    fn an_async_method_is_unsupported() {
        assert_c0001("class C:\n    async def __init__(self) -> None:\n        return\n");
    }

    #[test]
    fn a_decorated_method_is_unsupported() {
        assert_c0001(
            "class C:\n    @staticmethod\n    def __init__(self) -> None:\n        return\n",
        );
    }

    #[test]
    fn a_generic_method_is_unsupported() {
        assert_c0001("class C:\n    def __init__[T](self) -> None:\n        return\n");
    }

    #[test]
    fn a_property_getter_shadowing_a_method_is_rejected() {
        let module = crate::pycc_parser_test_helper::parse(
            "class C:\n    def __init__(self) -> None:\n        return\n    def x(self) -> int:\n        return 1\n    @property\n    def x(self) -> int:\n        return 2\n",
        );
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic.message.contains("cannot shadow a method"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn a_method_shadowing_a_property_getter_is_rejected() {
        let module = crate::pycc_parser_test_helper::parse(
            "class C:\n    def __init__(self) -> None:\n        return\n    @property\n    def x(self) -> int:\n        return 2\n    def x(self) -> int:\n        return 1\n",
        );
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic.message.contains("cannot shadow a property"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn a_method_shadowing_a_static_method_is_rejected() {
        let module = crate::pycc_parser_test_helper::parse(
            "class C:\n    def __init__(self) -> None:\n        return\n    @staticmethod\n    def foo(x: int) -> int:\n        return x\n    def foo(self, x: int) -> int:\n        return x + 1\n",
        );
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic
                .message
                .contains("cannot share a name with a `@staticmethod`"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn a_method_shadowing_a_class_method_is_rejected() {
        let module = crate::pycc_parser_test_helper::parse(
            "class C:\n    def __init__(self) -> None:\n        return\n    @classmethod\n    def foo(cls, x: int) -> int:\n        return x\n    def foo(self, x: int) -> int:\n        return x + 1\n",
        );
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic
                .message
                .contains("cannot share a name with a `@classmethod`"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn a_static_method_shadowing_a_method_is_rejected() {
        let module = crate::pycc_parser_test_helper::parse(
            "class C:\n    def __init__(self) -> None:\n        return\n    def foo(self, x: int) -> int:\n        return x\n    @staticmethod\n    def foo(x: int) -> int:\n        return x + 1\n",
        );
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic
                .message
                .contains("cannot share a name with a regular method"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn a_static_method_shadowing_a_property_is_rejected() {
        let module = crate::pycc_parser_test_helper::parse(
            "class C:\n    def __init__(self) -> None:\n        return\n    @property\n    def foo(self) -> int:\n        return 1\n    @staticmethod\n    def foo(x: int) -> int:\n        return x + 1\n",
        );
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic
                .message
                .contains("cannot share a name with a property"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn a_static_method_shadowing_a_class_method_is_rejected() {
        let module = crate::pycc_parser_test_helper::parse(
            "class C:\n    def __init__(self) -> None:\n        return\n    @classmethod\n    def foo(cls, x: int) -> int:\n        return x\n    @staticmethod\n    def foo(x: int) -> int:\n        return x + 1\n",
        );
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic
                .message
                .contains("cannot share a name with a `@classmethod`"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn a_class_method_shadowing_a_method_is_rejected() {
        let module = crate::pycc_parser_test_helper::parse(
            "class C:\n    def __init__(self) -> None:\n        return\n    def foo(self, x: int) -> int:\n        return x\n    @classmethod\n    def foo(cls, x: int) -> int:\n        return x + 1\n",
        );
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic
                .message
                .contains("cannot share a name with a regular method"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn a_class_method_shadowing_a_property_is_rejected() {
        let module = crate::pycc_parser_test_helper::parse(
            "class C:\n    def __init__(self) -> None:\n        return\n    @property\n    def foo(self) -> int:\n        return 1\n    @classmethod\n    def foo(cls, x: int) -> int:\n        return x + 1\n",
        );
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic
                .message
                .contains("cannot share a name with a property"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn a_class_method_shadowing_a_static_method_is_rejected() {
        let module = crate::pycc_parser_test_helper::parse(
            "class C:\n    def __init__(self) -> None:\n        return\n    @staticmethod\n    def foo(x: int) -> int:\n        return x\n    @classmethod\n    def foo(cls, x: int) -> int:\n        return x + 1\n",
        );
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic
                .message
                .contains("cannot share a name with a `@staticmethod`"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn staticmethod_on_init_is_rejected() {
        assert_c0001(
            "class C:\n    @staticmethod\n    def __init__(x: int) -> None:\n        return\n",
        );
    }
}
