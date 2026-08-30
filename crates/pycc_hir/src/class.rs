//! Class-definition lowering (D-154, Part 1 of #375): `lower_class`, its
//! per-method helper `lower_method`, and the `__init__`-body attribute-slot
//! pre-scan (`collect_init_attrs`/`slot_ty_from_init_rhs`).
//!
//! A single, non-generic, non-inheriting class is represented with **no**
//! `HirItem` footprint of its own -- unlike a top-level `def`, `class Foo:
//! ...` produces no `HirItem::ClassDef` node. Instead:
//! - Each method (including `__init__`) lowers into an ordinary
//!   `HirItem::Function` under a mangled `<ClassName>.<method_name>` name
//!   (the `.` separator, not `__`, follows this crate's own existing
//!   `mangle_generic_instantiation`-adjacent precedent documented on
//!   `pycc_types::Environment`'s doc comment: a real Python `NAME` token can
//!   never contain a `.`, so this spelling can never collide with a real
//!   top-level `def`), with `self` as an implicit first parameter typed
//!   `Ty::Instance(Box::new(class_name))`. This means `pycc_types`'
//!   `functions` map, `pycc_mir::lower_item`, and `pycc_codegen`'s
//!   per-function LLVM symbol emission need no new function-shaped case at
//!   all to handle a method body -- only the existing one, plus the
//!   `self`/`Ty::Instance` parameter they already know how to type.
//! - The class's own shape (attribute slots, method table) is recorded in
//!   `HirModule::class_defs`, a module-level side table mirroring
//!   `type_aliases`/`imports`'s existing shape exactly: compile-time-only
//!   information with no `HirItem`/`HirStmt` footprint of its own.
//!
//! The rejected alternative -- a dedicated `HirItem::ClassDef` variant, with
//! method bodies held only inside `HirClassDef`'s own table -- was
//! considered and dropped: `pycc_mir::build`'s own two-pass dispatch
//! (`matches!(item, HirItem::TopLevelStmt(_))` then
//! `matches!(item, HirItem::Function { .. })`, each followed by an
//! `.expect("every HIR item is either a function or a top-level
//! statement")`) would need a third pass or predicate change, plus a new
//! `MirItem` variant and a `pycc_codegen` no-op arm for it -- all to carry
//! information the side-table shape already carries with zero additional
//! `HirItem`/`MirItem` surface and zero new coverage regions in either of
//! those two crates' own item-dispatch code.
//!
//! **Class-body statement execution** follows PR #358's redefinition-is-
//! rebind pattern, extended to class methods via mangled-name namespacing
//! (#386): a class body statement must be a `def` (a nested class, a bare
//! `pass`, a bare string-literal expression statement -- a docstring, #744,
//! accepted anywhere in the body, matching `validate_init_subclass_body`'s
//! own precedent -- a class-level attribute declaration, or any other
//! statement kind is `C0001`); redefining a non-`__init__` method name within
//! one class
//! body **rebinds** -- the second `def` replaces the method table entry, and
//! the latest definition is the one dispatched to at runtime (both
//! definitions share the same mangled `<ClassName>.<method>` name, so PR
//! #358's function-pointer slot infrastructure already handles the rebind:
//! the second `def`'s source-order execution stores the new function's
//! address into the slot, and later calls dispatch to it); redefining
//! `__init__` is still `C0001` (the compile-time attribute-slot pre-scan
//! `collect_init_attrs` cannot reconcile two different `__init__` bodies); a
//! class must declare `__init__` (a class with no `__init__` is `C0001` --
//! this PR ships no default no-op constructor). The attribute-slot pre-scan
//! below only looks at `__init__`'s own top-level body statements (no
//! recursion into a nested `if`/`while`/`for`), matching this same minimal,
//! single-pass scope.

mod mro;

use crate::{HirExpr, HirItem, HirStmt, Ty, lower_arg_list, unsupported};
use mro::{resolve_mro, validate_bases};
use pycc_ast::{Decorator, Expr, Number, Stmt};
use pycc_diag::{Diagnostic, Span};

/// Method names that collide with `crates/pycc_hir/src/expr.rs`'s own
/// hand-recognized container-method call syntax (`Expr::Call` over
/// `Expr::Attribute`'s fast path for `.append()`/`.pop()`/`.get()`/
/// `.add()`, checked *before* the generic instance-method-call fallback --
/// see that file's own comment on that ordering). That fast path runs with
/// no type information available -- it cannot tell a real `list`/`dict`/
/// `set` receiver from a class instance whose own method just happens to
/// share one of these four names -- so `some_instance.get(5)` would
/// silently misroute into the dict-`get` fast path and fail with a
/// confusing "dict.get() takes exactly two arguments" diagnostic instead of
/// ever reaching the user's own method (D-068 review finding on #385).
/// Rejecting the name here, at class-definition time, turns that confusing
/// failure into a clear, immediate one.
const CONTAINER_METHOD_NAMES: [&str; 4] = ["append", "pop", "get", "add"];

/// A single class's declared shape (D-154): its attribute slots, in
/// first-`__init__`-assignment source order, and its method table (method
/// name -> the mangled `HirItem::Function` name in `HirModule::items` its
/// body was lowered to). See this module's own doc comment for why neither
/// field duplicates a method's body -- `methods` carries only the mangled
/// name, never a second copy.
#[derive(Debug, Clone, PartialEq)]
pub struct HirClassDef {
    pub name: String,
    /// Direct base class names, in source order from the class header
    /// (`class C(A, B):` → `["A", "B"]`). Empty for a class with no bases.
    /// Part 1 of #432: inheritance machinery.
    pub bases: Vec<String>,
    /// The computed C3 linearization (MRO) for this class: the class's own
    /// name first, followed by its bases' MROs in C3 order. For a class with
    /// no bases, this is `[self_name]`. Computed at HIR-lowering time by
    /// `compute_c3_mro` and stored here so every downstream consumer
    /// (`pycc_types`, `pycc_mir`) resolves methods/attributes/properties
    /// through the same MRO without re-deriving it. Part 1 of #432.
    pub mro: Vec<String>,
    pub attrs: Vec<(String, Ty)>,
    pub methods: Vec<(String, String)>,
    /// `@property` definitions (#377): each entry records a property's
    /// attribute name, the mangled getter method name, and (if present) the
    /// mangled setter method name. Property methods are lowered into
    /// ordinary mangled `HirItem::Function`s (just like regular methods) but
    /// are NOT entered into `methods` -- they are accessed via attribute
    /// syntax (`obj.x`), not method-call syntax (`obj.x()`). The type
    /// checker resolves `obj.x` reads/writes against this table, and MIR
    /// lowering rewrites property-shaped `AttrGet`/`AttrSet` into ordinary
    /// `MirExpr::Call`s to the getter/setter's mangled name, reusing the
    /// existing method-call infrastructure with no new MIR/codegen variant.
    pub properties: Vec<PropertyDef>,
    /// `@staticmethod` definitions (#436): each entry is
    /// `(method_name, mangled_name)`, where the mangled name uses a
    /// `.static` suffix (e.g. `C.create.static`). Static methods are NOT
    /// entered into `methods` -- they have their own table, and the
    /// `.static` suffix prevents collision with a regular method of the
    /// same name. Static methods can be called on both the class
    /// (`C.create(args)`) and an instance (`instance.create(args)`).
    pub static_methods: Vec<(String, String)>,
    /// `@classmethod` definitions (#436): each entry is
    /// `(method_name, mangled_name)`, where the mangled name uses a
    /// `.classmethod` suffix (e.g. `C.create.classmethod`). Class methods
    /// are NOT entered into `methods` -- they have their own table, and
    /// the `.classmethod` suffix prevents collision with a regular method
    /// of the same name. Class methods take an implicit `cls` parameter
    /// (typed `Ty::Instance(class_name)`) as their first parameter, and
    /// can be called on both the class and an instance.
    pub class_methods: Vec<(String, String)>,
    /// PEP 695 (#387): the class's single type parameter name, if it is a
    /// generic class (`class C[T]:`). `None` for a non-generic class. At
    /// instantiation site (`C[int](args)`), this parameter is substituted
    /// with the concrete type, reusing PR-13's `Ty::Param` call-site-
    /// substitution mechanism (D-133/D-134).
    pub type_param: Option<String>,
    /// PEP 435 (#379, PR-19): the enum members of an enum class
    /// (`class Color(Enum): RED = 1; GREEN = 2`), in source order. Each
    /// entry is `(member_name, value)` where `value` is the integer
    /// literal assigned to the member. Empty for a non-enum class; a
    /// non-empty vec marks this class as an enum class. An enum class has
    /// `bases = []` and `mro = [self_name]` (the `Enum` base is consumed
    /// as a marker, not a real base), no `__init__` requirement, and
    /// `attrs = [("value", Ty::Int), ("name", Ty::Str)]` so existing
    /// `resolve_attr_get`/MIR slot resolution handle `member.value`/
    /// `member.name` unchanged. Each member is a compile-time singleton
    /// instance allocated once at module-init time (see `pycc_codegen`'s
    /// per-member init sequence).
    pub enum_members: Vec<(String, i64)>,
    /// PEP 557/681 (#378, PR-18): `true` when this class is decorated with
    /// `@dataclass` or `@dataclass_transform(...)`. A dataclass class
    /// auto-generates `__init__`, `__eq__`, and `__repr__` from its
    /// `dataclass_fields` at HIR-lowering time (see `lower_class`'s own
    /// synthesis logic). `false` for an ordinary class and for an enum
    /// class.
    pub is_dataclass: bool,
    /// PEP 557 (#378, PR-18): the dataclass's annotated fields, in
    /// declaration order. Each entry is `(field_name, field_type)`. Empty
    /// for a non-dataclass class and for a dataclass with no fields (a
    /// zero-field dataclass is valid per PEP 557). Populated from
    /// `Stmt::AnnAssign` nodes in the class body; a field with a default
    /// value (`x: int = field(default=...)`) is recognized but rejected
    /// with `C0001` (field defaults are deferred to a follow-up issue --
    /// the compiler has no optional-parameter mechanism yet).
    pub dataclass_fields: Vec<(String, Ty)>,
    /// PEP 544 (#380, PR-20): `true` when this class is a protocol class
    /// (`class P(Protocol):`). A protocol class is a compile-time-only
    /// interface description — it is never instantiated and has no runtime
    /// representation. Its `protocol_members` field lists the required
    /// methods and attributes. A protocol class has `bases = []` and
    /// `mro = [self_name]` (the `Protocol` base is consumed as a marker,
    /// like `Enum`), unless it inherits from another user-defined protocol
    /// (`class Q(P):` where `P` is a protocol), in which case `P` is a
    /// real base and `Q` inherits `P`'s `protocol_members`.
    pub is_protocol: bool,
    /// PEP 544 (#380, PR-20): `true` when this protocol class is decorated
    /// with `@runtime_checkable`. A `@runtime_checkable` protocol can be
    /// used in `isinstance` checks (evaluated at compile time as a
    /// structural conformance check). A non-`@runtime_checkable` protocol
    /// used in `isinstance` is rejected with `T0021`. Always `false` for a
    /// non-protocol class.
    pub runtime_checkable: bool,
    /// PEP 544 (#380, PR-20): the required methods and attributes of a
    /// protocol class. Each entry is either a method requirement (name +
    /// parameter types + return type) or an attribute requirement (name +
    /// type). For a protocol inheriting from another protocol, this
    /// includes inherited members. Empty for a non-protocol class.
    pub protocol_members: Vec<ProtocolMember>,
    /// PEP 3119 (#380, PR-20): method names decorated with
    /// `@abstractmethod` in this class or any base class. A concrete
    /// subclass that does not override every inherited abstract method is
    /// rejected with `C0001` at class-definition time. Empty for a class
    /// with no abstract methods.
    pub abstract_methods: Vec<String>,
    /// PEP 3119 (#380, PR-20): `true` when this class inherits from `ABC`
    /// (`class C(ABC):`). An abstract class cannot be instantiated
    /// (rejected with `C0001`). The `ABC` base is consumed as a marker
    /// (like `Enum`/`Protocol`), not recorded as a real base.
    pub is_abstract: bool,
    /// Part 2 of #541 (D-189), widened by Part 2 of #543 (#739, D-194): the
    /// runtime exception type tag this class is raised and caught under, or
    /// `None` when the class is **not raisable**.
    ///
    /// `None` never means "synthetic". D-188 makes
    /// `HirModule::seeded_builtin_exception_classes` the sole provenance
    /// signal for syntheticness, and this field carries no provenance
    /// information whatsoever. There are two families of synthetic builtin
    /// exception classes, and only one of them carries `None` here: the
    /// original flat seven (`Exception`, `ValueError`, `TypeError`,
    /// `KeyError`, `IndexError`, `ZeroDivisionError`, `RuntimeError`) carry
    /// `None` even though they are among the most raisable classes in the
    /// language, because their tags are fixed constants resolved by name
    /// (`pycc_mir::exception::resolve_exception_tag`) rather than assigned
    /// per module. The 16-member PEP 3151 `OSError` family added by D-194
    /// has no such name-based fallback and instead carries a fixed `Some`
    /// tag directly on this field, assigned by array index in
    /// `pycc_hir::exception::builtin_exception_class_defs`. Reading `None`
    /// as "user-defined" or `Some` as "user-defined" is equally wrong.
    ///
    /// A tag is assigned by `lower_checked` to every user-declared class whose
    /// MRO reaches a builtin exception class, in a deterministic order, from
    /// the range `25..=255` — `0..=24` are reserved for the 25-member builtin
    /// hierarchy (the flat seven, the `OSError` family, and `ExceptionGroup`/
    /// `BaseExceptionGroup` per Part 3 of #382 (#542, PEP 654, D-202)). A
    /// module declaring more than 231 such classes is rejected with `C0001`.
    /// Every other class — including a user class that never touches the
    /// exception hierarchy — keeps `None`.
    pub exception_type_tag: Option<u8>,
}

/// PEP 544 (#380, PR-20): a single required member of a protocol class.
/// Either a method requirement (name + parameter types + return type) or
/// an attribute requirement (name + type).
#[derive(Debug, Clone, PartialEq)]
pub enum ProtocolMember {
    /// A required method: name, parameter types (excluding `self`), and
    /// return type.
    Method {
        name: String,
        param_tys: Vec<Ty>,
        return_ty: Ty,
    },
    /// A required attribute: name and type.
    Attribute { name: String, ty: Ty },
}

/// A single `@property` definition (#377): the attribute name exposed to
/// user code (e.g. `"x"` in `obj.x`), the mangled getter method name
/// (e.g. `"C.x"`, the same `<Class>.<name>` mangling a regular method
/// uses), and -- if a `@<name>.setter` method was also defined -- the
/// mangled setter method name (e.g. `"C.x.setter"`, using a `.setter`
/// suffix that a real Python identifier can never contain, so it cannot
/// collide with a regular method's mangled name).
#[derive(Debug, Clone, PartialEq)]
pub struct PropertyDef {
    pub name: String,
    pub getter: String,
    pub setter: Option<String>,
}

/// How a class-body `def` is classified by its decorator list (#377):
/// a regular method (no decorator or an unrecognized one -- the latter is
/// rejected), a `@property` getter, or a `@<name>.setter` setter for the
/// property named `<name>`. `lower_class` uses this to decide the method's
/// mangled name and which table (`methods` vs `properties`) it belongs to.
enum MethodKind {
    /// A regular method (no decorator, or `@override`). `is_override` is
    /// `true` when the method is decorated `@override` (PEP 698) -- the
    /// method is still lowered as an ordinary mangled `HirItem::Function`
    /// exactly like a non-override regular method, but `lower_class`
    /// additionally verifies the method name exists in at least one base
    /// class's methods or properties (emitting T0031 if not).
    Regular { is_override: bool },
    /// `@property` getter for the attribute named `prop_name` (which is
    /// also the method's own source name, e.g. `def x(self) -> int` with
    /// `@property` is a getter for attribute `"x"`).
    PropertyGetter { prop_name: String },
    /// `@<prop_name>.setter` setter for the property named `prop_name`.
    /// The method's own source name must match `prop_name` (this is
    /// validated in `classify_decorator`).
    PropertySetter { prop_name: String },
    /// `@staticmethod` (#436): a static method. Takes no implicit `self`
    /// or `cls` parameter -- the method's own parameter list is exactly
    /// what the user wrote. Mangled name uses a `.static` suffix to avoid
    /// colliding with a regular method of the same name.
    StaticMethod,
    /// `@classmethod` (#436): a class method. Takes an implicit `cls`
    /// parameter (typed `Ty::Instance(class_name)`, matching `self`'s own
    /// type in this compiler's static-dispatch model) as its first
    /// parameter. Mangled name uses a `.classmethod` suffix to avoid
    /// colliding with a regular method of the same name.
    ClassMethod,
    /// `@abstractmethod` (#380, PR-20, PEP 3119): an abstract method.
    /// The method is still lowered as an ordinary mangled
    /// `HirItem::Function`, but `lower_class` records its name in
    /// `HirClassDef::abstract_methods`. A concrete subclass that does not
    /// override every inherited abstract method is rejected with `C0001`.
    /// The method body must be a declaration-style body (`...` or `pass`).
    AbstractMethod,
}

/// Classifies a method's decorator list (#377, #432). Returns:
/// - `Regular { is_override: false }` if the list is empty.
/// - `Regular { is_override: true }` if the single decorator is `@override`
///   (a bare name `override`, PEP 698).
/// - `PropertyGetter` if the single decorator is `@property` (a bare name
///   `property`).
/// - `PropertySetter` if the single decorator is `@<name>.setter` (an
///   attribute access on a bare name, where the attribute is `"setter"`).
///   The method's own source name must match `<name>` -- a mismatch
///   (e.g. `@x.setter def y(self, v): ...`) is rejected with `C0001`,
///   matching the common Python idiom where the setter's method name
///   matches the property name.
/// - Any other decorator shape (multiple decorators, a call-shaped
///   decorator, a different attribute name) is rejected with `C0001`,
///   preserving the pre-#377 "method decorators are not supported yet"
///   diagnostic for everything outside `@property`/`@<name>.setter`/
///   `@override`.
fn classify_decorator(
    decorator_list: &[Decorator],
    method_name: &str,
    range: std::ops::Range<u32>,
) -> Result<MethodKind, Diagnostic> {
    if decorator_list.is_empty() {
        return Ok(MethodKind::Regular { is_override: false });
    }
    if decorator_list.len() > 1 {
        return Err(unsupported(
            "method decorators are not supported yet",
            range,
        ));
    }
    let decorator = &decorator_list[0];
    match &decorator.expression {
        // `@property` -- a bare name `property`.
        Expr::Name(name) if name.id.as_str() == "property" => Ok(MethodKind::PropertyGetter {
            prop_name: method_name.to_string(),
        }),
        // `@override` -- a bare name `override` (PEP 698). The method is
        // still a regular method; `lower_class` verifies the method name
        // exists in at least one base class.
        Expr::Name(name) if name.id.as_str() == "override" => {
            Ok(MethodKind::Regular { is_override: true })
        }
        // `@staticmethod` (#436) -- a bare name `staticmethod`.
        Expr::Name(name) if name.id.as_str() == "staticmethod" => Ok(MethodKind::StaticMethod),
        // `@classmethod` (#436) -- a bare name `classmethod`.
        Expr::Name(name) if name.id.as_str() == "classmethod" => Ok(MethodKind::ClassMethod),
        // `@abstractmethod` (#380, PR-20, PEP 3119) -- a bare name
        // `abstractmethod`. Recognized as a bare name without requiring
        // `from abc import abstractmethod`, matching the
        // `Final`/`Annotated`/`Enum` precedent.
        Expr::Name(name) if name.id.as_str() == "abstractmethod" => Ok(MethodKind::AbstractMethod),
        // `@<name>.setter` -- an attribute access on a bare name, where
        // the attribute is `"setter"`.
        Expr::Attribute(attr) => {
            let Expr::Name(base_name) = attr.value.as_ref() else {
                return Err(unsupported(
                    "method decorators are not supported yet",
                    range,
                ));
            };
            if attr.attr.as_str() != "setter" {
                return Err(unsupported(
                    "method decorators are not supported yet",
                    range,
                ));
            }
            let prop_name = base_name.id.as_str().to_string();
            // The decorator name must match the method's own source name
            // (e.g. `@x.setter def x(self, v): ...`, not `@x.setter def
            // y(self, v): ...`). This is the overwhelmingly common idiom;
            // the uncommon mismatch case is rejected rather than
            // silently supporting a shape this PR does not need.
            if prop_name != method_name {
                return Err(unsupported(
                    format!(
                        "a `@{prop_name}.setter` decorator must decorate a method named \
                         `{prop_name}`, not `{method_name}`"
                    ),
                    range,
                ));
            }
            Ok(MethodKind::PropertySetter { prop_name })
        }
        _ => Err(unsupported(
            "method decorators are not supported yet",
            range,
        )),
    }
}

/// PEP 3129/557/681/544 (#378/#380, PR-18/PR-20): the result of
/// classifying a class's decorator list. `is_dataclass` is `true` for
/// `@dataclass`/`@dataclass_transform(...)`. `runtime_checkable` is `true`
/// for `@runtime_checkable` (PEP 544). Both can be `false` (an ordinary
/// class with no decorators). `@dataclass` combined with
/// `@runtime_checkable` is rejected — a protocol class cannot be a
/// dataclass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ClassDecoratorInfo {
    is_dataclass: bool,
    runtime_checkable: bool,
}

/// PEP 3129/557/681/544 (#378/#380, PR-18/PR-20): classifies a class's
/// decorator list to determine whether the class is a `@dataclass` (PEP
/// 557), a `@dataclass_transform(...)` (PEP 681), or a
/// `@runtime_checkable` (PEP 544) class. Returns a `ClassDecoratorInfo`
/// struct.
///
/// Any other class decorator shape (a different bare name, an
/// attribute-access decorator, `@dataclass(frozen=True)` with options) is
/// rejected with `C0001`, preserving the pre-#378 "class decorators are
/// not supported yet" diagnostic for everything outside
/// `@dataclass`/`@dataclass_transform()`/`@runtime_checkable`.
fn classify_class_decorator(
    decorator_list: &[Decorator],
    range: std::ops::Range<u32>,
) -> Result<ClassDecoratorInfo, Diagnostic> {
    let mut info = ClassDecoratorInfo {
        is_dataclass: false,
        runtime_checkable: false,
    };
    if decorator_list.is_empty() {
        return Ok(info);
    }
    if decorator_list.len() > 2 {
        return Err(unsupported(
            "more than two class decorators are not supported yet",
            range,
        ));
    }
    for decorator in decorator_list {
        match &decorator.expression {
            // `@dataclass` -- a bare name `dataclass` (PEP 557).
            Expr::Name(name) if name.id.as_str() == "dataclass" => {
                info.is_dataclass = true;
            }
            // `@runtime_checkable` -- a bare name `runtime_checkable`
            // (PEP 544, #380, PR-20). Recognized as a bare name without
            // requiring `from typing import runtime_checkable`, matching
            // the `Final`/`Annotated`/`Enum` precedent.
            Expr::Name(name) if name.id.as_str() == "runtime_checkable" => {
                info.runtime_checkable = true;
            }
            // `@dataclass_transform(...)` -- a call whose callee is the
            // bare name `dataclass_transform` (PEP 681). The keyword
            // arguments are accepted but ignored -- pycc's dataclass
            // implementation always generates `__init__`/`__eq__`/`__repr__`,
            // matching the defaults.
            Expr::Call(call) => {
                let Expr::Name(name) = call.func.as_ref() else {
                    return Err(unsupported("class decorators are not supported yet", range));
                };
                if name.id.as_str() == "dataclass_transform" {
                    info.is_dataclass = true;
                } else if name.id.as_str() == "dataclass" {
                    // `@dataclass(frozen=True)` etc. -- rejected with C0001.
                    return Err(unsupported(
                        "`@dataclass` with options is not supported yet -- only a bare \
                         `@dataclass` is supported in this version",
                        range,
                    ));
                } else {
                    return Err(unsupported("class decorators are not supported yet", range));
                }
            }
            _ => {
                return Err(unsupported("class decorators are not supported yet", range));
            }
        }
    }
    // #380 (PR-20): `@dataclass` combined with `@runtime_checkable` is
    // rejected — a protocol class cannot be a dataclass (a protocol is a
    // compile-time-only interface description, not an instantiable class
    // with fields).
    if info.is_dataclass && info.runtime_checkable {
        return Err(unsupported(
            "`@dataclass` combined with `@runtime_checkable` is not supported -- a protocol \
             class cannot be a dataclass",
            range,
        ));
    }
    Ok(info)
}

/// Lowers a module-level `class Foo: ...` statement (D-154). Returns the
/// class's own declared shape (for `HirModule::class_defs`) alongside every
/// method it defines, already lowered into ordinary mangled
/// `HirItem::Function`s ready to append to `HirModule::items` -- see this
/// module's own doc comment for why a class has no `HirItem` of its own.
///
/// `defined_classes` is the list of already-lowered class definitions (name
/// → `HirClassDef`), in source order. Inheritance validation (unknown base,
/// circular inheritance, generic-class-as-base) uses this to check each base
/// class against the classes defined earlier in the module, matching Python's
/// own source-order class-body execution (a base class must be defined before
/// the derived class that inherits from it).
///
/// Every check below is a `C0001` capability diagnostic, not a design
/// question this PR resolves: a class decorator is out of scope entirely
/// (dataclasses/`dataclass_transform` are unrelated later PRs' own scope).
/// Generic classes (`class C[T]:`) with a single type parameter ARE now
/// supported by #387 (see the `type_params` handling below). Inheritance
/// (`class C(Base):`) is now supported by #432 (Part 1).
///
/// #379 (PR-19): Lower a PEP 435-style enum class. An enum class body is
/// assignments only (`RED = 1`), not method definitions. This function
/// validates the enum body and constructs the `HirClassDef` with
/// #380 (PR-20): Returns `true` if a method body is declaration-style
/// (`...` (Ellipsis) or `pass`), suitable for a protocol method or an
/// `@abstractmethod`. A body with any other content (a docstring, an
/// assignment, a return statement, etc.) is an implementation body.
fn is_declaration_body(body: &[Stmt]) -> bool {
    // `pass` is filtered out by `lower_body`, so a body of just `pass`
    // is an empty slice after filtering. But we check the raw AST here
    // before any filtering.
    body.iter().all(|stmt| match stmt {
        Stmt::Pass(_) => true,
        // `...` (Ellipsis) as an expression statement.
        Stmt::Expr(expr_stmt) => {
            matches!(expr_stmt.value.as_ref(), Expr::EllipsisLiteral(_))
        }
        _ => false,
    }) && !body.is_empty()
}

/// #380 (PR-20): Lowers a protocol class body. Protocol methods (with
/// `...` or `pass` bodies) are recorded as `ProtocolMember::Method` and
/// are NOT lowered to `HirItem::Function`s. Protocol annotated assignments
/// (`x: int`) are recorded as `ProtocolMember::Attribute`. Protocol
/// members from base protocols are inherited. A protocol method with an
/// implementation body is rejected with `C0001`. A protocol `__init__` is
/// rejected with `C0001`.
#[allow(clippy::too_many_arguments)]
fn lower_protocol_class(
    def: &pycc_ast::StmtClassDef,
    class_name: String,
    bases: Vec<String>,
    mro: Vec<String>,
    type_param: Option<String>,
    runtime_checkable: bool,
    defined_classes: &[(String, HirClassDef)],
    class_name_defs: &[ClassAnnotationInfo],
) -> Result<(HirClassDef, Vec<HirItem>), Diagnostic> {
    let mut protocol_members: Vec<ProtocolMember> = Vec::new();
    // Inherit protocol members from base protocols.
    for base_name in &bases {
        // This should have been validated already in `lower_class`.
        // Using `.expect()` (whose panic path lives in libcore, outside
        // this crate's instrumented regions) instead of a
        // `let .. else { panic!() }` avoids a permanently-uncovered
        // branch under D-014's 100%-region coverage gate.
        let base_def = &defined_classes
            .iter()
            .find(|(n, _)| n == base_name)
            .expect("pycc_hir: internal error: protocol base not found in defined_classes -- lower_class should have validated this")
            .1;
        for member in &base_def.protocol_members {
            // Only add inherited members that are not redeclared in this
            // class (redeclarations are added below from the class body).
            let name = match member {
                ProtocolMember::Method { name, .. } => name,
                ProtocolMember::Attribute { name, .. } => name,
            };
            if !protocol_members.iter().any(|m| match m {
                ProtocolMember::Method { name: n, .. } => n == name,
                ProtocolMember::Attribute { name: n, .. } => n == name,
            }) {
                protocol_members.push(member.clone());
            }
        }
    }
    for stmt in &def.body {
        match stmt {
            Stmt::FunctionDef(method_def) => {
                let method_name = method_def.name.to_string();
                // Reject `__init__` in a protocol class.
                if method_name == "__init__" {
                    return Err(unsupported(
                        format!(
                            "a protocol class `{class_name}` cannot define `__init__` -- \
                             protocols are not instantiated"
                        ),
                        method_def.range,
                    ));
                }
                // Reject decorators on protocol methods.
                if !method_def.decorator_list.is_empty() {
                    return Err(unsupported(
                        format!(
                            "decorators on protocol method `{class_name}.{method_name}` are \
                             not supported yet"
                        ),
                        method_def.range,
                    ));
                }
                // Reject generic protocol methods.
                if method_def.type_params.is_some() {
                    return Err(unsupported(
                        format!(
                            "a generic protocol method `{class_name}.{method_name}` is not \
                             supported yet"
                        ),
                        method_def.range,
                    ));
                }
                // The method body must be declaration-style (`...` or
                // `pass`).
                if !is_declaration_body(&method_def.body) {
                    return Err(unsupported(
                        format!(
                            "a protocol method `{class_name}.{method_name}` must have a \
                             declaration-style body (`...` or `pass`), not an implementation"
                        ),
                        method_def.range,
                    ));
                }
                // Lower the method's parameter and return types.
                // `self` is handled specially (assigned
                // `Ty::Instance(class_name)` directly, bypassing
                // `annotation_to_ty`), matching how `lower_method`
                // handles it for regular methods. The remaining
                // parameters go through `lower_arg_list`.
                let all_args = &method_def.parameters.args;
                let (params, return_ty) =
                    if !all_args.is_empty() && all_args[0].parameter.name.as_str() == "self" {
                        // Strip `self` and lower the rest.
                        let rest = &all_args[1..];
                        let method_is_public = !method_name.starts_with('_');
                        let p = crate::lower_arg_list(
                            rest,
                            method_is_public,
                            &method_name,
                            type_param.as_deref(),
                            Some(&class_name),
                            &[],
                            class_name_defs,
                        )?;
                        let r = crate::lower_return_annotation(
                            method_def.returns.as_deref(),
                            method_is_public,
                            &method_name,
                            type_param.as_deref(),
                            Some(&class_name),
                            &[],
                            class_name_defs,
                        )?;
                        (p, r)
                    } else {
                        // No `self` parameter — this is unusual for a
                        // protocol method but we handle it gracefully by
                        // lowering all parameters.
                        let method_is_public = !method_name.starts_with('_');
                        let p = crate::lower_arg_list(
                            all_args,
                            method_is_public,
                            &method_name,
                            type_param.as_deref(),
                            Some(&class_name),
                            &[],
                            class_name_defs,
                        )?;
                        let r = crate::lower_return_annotation(
                            method_def.returns.as_deref(),
                            method_is_public,
                            &method_name,
                            type_param.as_deref(),
                            Some(&class_name),
                            &[],
                            class_name_defs,
                        )?;
                        (p, r)
                    };
                // Protocol member signatures exclude `self` (already
                // stripped above).
                let param_tys: Vec<Ty> = params.iter().map(|(_, ty)| ty.clone()).collect();
                let member = ProtocolMember::Method {
                    name: method_name.clone(),
                    param_tys,
                    return_ty,
                };
                // Replace if redeclared, otherwise add.
                if let Some(existing) = protocol_members.iter().position(
                    |m| matches!(m, ProtocolMember::Method { name, .. } if name == &method_name),
                ) {
                    protocol_members[existing] = member;
                } else {
                    protocol_members.push(member);
                }
            }
            Stmt::AnnAssign(ann) => {
                let Expr::Name(target) = ann.target.as_ref() else {
                    return Err(unsupported(
                        "a protocol attribute annotation must target a bare name (`x: int`), \
                         not an attribute access, subscript, or other expression",
                        pycc_ast::expr_range(&ann.target),
                    ));
                };
                let attr_name = target.id.to_string();
                let attr_ty = crate::annotation_to_ty(
                    &ann.annotation,
                    type_param.as_deref(),
                    Some(&class_name),
                    &[],
                    class_name_defs,
                )?;
                // A protocol attribute cannot have a default value.
                if ann.value.is_some() {
                    return Err(unsupported(
                        format!(
                            "a protocol attribute `{class_name}.{attr_name}` cannot have a \
                             default value -- protocol attributes are requirements, not \
                             initializers"
                        ),
                        ann.range,
                    ));
                }
                let member = ProtocolMember::Attribute {
                    name: attr_name.clone(),
                    ty: attr_ty,
                };
                if let Some(existing) = protocol_members.iter().position(
                    |m| matches!(m, ProtocolMember::Attribute { name, .. } if name == &attr_name),
                ) {
                    protocol_members[existing] = member;
                } else {
                    protocol_members.push(member);
                }
            }
            Stmt::Pass(_) => {
                // `pass` is a no-op in a protocol body.
            }
            // #744: a docstring (a bare string-literal expression statement)
            // is a no-op, matching `validate_init_subclass_body`'s existing
            // precedent for the same construct.
            Stmt::Expr(expr_stmt) if matches!(*expr_stmt.value, Expr::StringLiteral(_)) => {}
            _ => {
                return Err(unsupported(
                    format!(
                        "a protocol class body must contain only method definitions (`def ...`) \
                         and annotated assignments (`x: int`) -- {:?} is not supported yet",
                        stmt
                    ),
                    pycc_ast::stmt_range(stmt),
                ));
            }
        }
    }
    Ok((
        HirClassDef {
            exception_type_tag: None,
            name: class_name,
            bases,
            mro,
            attrs: Vec::new(),
            methods: Vec::new(),
            properties: Vec::new(),
            static_methods: Vec::new(),
            class_methods: Vec::new(),
            type_param,
            enum_members: Vec::new(),
            is_dataclass: false,
            dataclass_fields: Vec::new(),
            is_protocol: true,
            runtime_checkable,
            protocol_members,
            abstract_methods: Vec::new(),
            is_abstract: false,
        },
        Vec::new(),
    ))
}

/// `enum_members` populated. Extracted from `lower_class` to isolate the
/// enum-specific code paths (see cargo-llvm-cov#276 for the coverage
/// instantiation issue that motivated the extraction).
fn lower_enum_class(
    def: &pycc_ast::StmtClassDef,
    class_name: String,
    bases: Vec<String>,
    mro: Vec<String>,
    type_param: Option<String>,
) -> Result<(HirClassDef, Vec<HirItem>), Diagnostic> {
    // An enum class's attrs are the two reserved member attributes
    // (`value`, `name`), so `member.value`/`member.name` resolve via
    // the existing `resolve_attr_get`/MIR slot resolution unchanged.
    let attrs = vec![
        ("value".to_string(), Ty::Int),
        ("name".to_string(), Ty::Str),
    ];
    let mut enum_members: Vec<(String, i64)> = Vec::new();
    for stmt in &def.body {
        // #744: a docstring (a bare string-literal expression statement) is
        // a no-op, matching `validate_init_subclass_body`'s existing
        // precedent for the same construct.
        if let Stmt::Expr(expr_stmt) = stmt
            && matches!(*expr_stmt.value, Expr::StringLiteral(_))
        {
            continue;
        }
        let Stmt::Assign(assign) = stmt else {
            return Err(unsupported(
                "an enum class body must contain only member assignments (`RED = 1`) -- \
                 no method definitions or other statements are supported yet",
                pycc_ast::stmt_range(stmt),
            ));
        };
        // The target must be a single bare name (not a tuple or
        // subscript).
        if assign.targets.len() != 1 {
            return Err(unsupported(
                "an enum member assignment must have a single target (`RED = 1`), not \
                 multiple targets",
                assign.range,
            ));
        }
        let Expr::Name(target_name) = &assign.targets[0] else {
            return Err(unsupported(
                "an enum member name must be a bare name (`RED = 1`), not an attribute \
                 access, subscript, or other expression",
                pycc_ast::expr_range(&assign.targets[0]),
            ));
        };
        let member_name = target_name.id.to_string();
        // Reject duplicate member names (matching CPython's
        // `TypeError: Attempted to reuse key`).
        if enum_members.iter().any(|(name, _)| name == &member_name) {
            return Err(unsupported(
                format!(
                    "enum member `{member_name}` is already defined in class \
                     `{class_name}` -- duplicate member names are not allowed"
                ),
                assign.range,
            ));
        }
        // The value must be an int literal (the only supported member
        // value type in v0.3, matching TYPE_SYSTEM.md's "integer
        // discriminant" representation). A bool literal is rejected
        // (it is a separate type in pycc, not an int subtype for
        // enum-value purposes). A non-literal value (e.g. `RED = f()`)
        // is also rejected -- enum values must be compile-time
        // literals in pycc's static model. The actual integer value is
        // extracted and carried in `enum_members` so codegen can
        // initialize each member's `value` slot with the correct
        // literal, not a position-derived guess.
        let member_value: i64 = match &*assign.value {
            Expr::NumberLiteral(number) => match &number.value {
                Number::Int(i) => {
                    let Some(value) = i.as_i64() else {
                        return Err(unsupported(
                            format!(
                                "enum member `{member_name}` has an integer value that does \
                                 not fit in i64 -- only i64-range values are supported"
                            ),
                            assign.range,
                        ));
                    };
                    value
                }
                _ => {
                    return Err(unsupported(
                        format!(
                            "enum member `{member_name}` has a non-integer value -- only \
                             `int` member values are supported in v0.3"
                        ),
                        assign.range,
                    ));
                }
            },
            _ => {
                return Err(unsupported(
                    format!(
                        "enum member `{member_name}` must be assigned an integer literal \
                         (`{member_name} = 1`), not an expression or non-integer value"
                    ),
                    assign.range,
                ));
            }
        };
        enum_members.push((member_name, member_value));
    }
    // An enum class has no methods, no __init__, and no items. Its
    // members are compile-time singletons allocated by codegen, not
    // runtime-instantiated objects.
    Ok((
        HirClassDef {
            exception_type_tag: None,
            name: class_name,
            bases,
            mro,
            attrs,
            methods: Vec::new(),
            properties: Vec::new(),
            static_methods: Vec::new(),
            class_methods: Vec::new(),
            type_param,
            enum_members,
            is_dataclass: false,
            dataclass_fields: Vec::new(),
            is_protocol: false,
            runtime_checkable: false,
            protocol_members: Vec::new(),
            abstract_methods: Vec::new(),
            is_abstract: false,
        },
        Vec::new(),
    ))
}

/// One already-defined class, projected down to exactly what
/// `annotation_to_ty` needs. Replaces the former `(String, bool)` pair
/// (#380, PR-20), which carried the class name and its protocol flag and had
/// nowhere to record #611's subscriptability answer.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ClassAnnotationInfo {
    pub(crate) name: String,
    pub(crate) is_protocol: bool,
    /// PEP 560 (#611): whether `ClassName[type_arg]` is legal in a type
    /// annotation. True when the class defines `__class_getitem__` somewhere
    /// in its MRO -- in either the `@staticmethod` or the `@classmethod`
    /// spelling -- or when it is a PEP 695 generic class (`class C[T]:`),
    /// which CPython makes implicitly subscriptable through `Generic`
    /// without any explicit hook of its own.
    pub(crate) subscriptable: bool,
    /// Issue #693 (PEP 560, extending #611): the declared return type of
    /// whichever `__class_getitem__` hook `pycc_types`'
    /// `resolve_static_or_class_method_call` would itself dispatch to for a
    /// value-position `ClassName[type_arg]` call on this MRO -- found by
    /// `class_getitem_return_ty`'s own two-pass MRO walk (every MRO entry's
    /// `static_methods` first, then, only if none declared the hook, every
    /// MRO entry's `class_methods`), which is deliberately the *same*
    /// two-pass order and not the single combined pass `subscriptable`'s own
    /// hook-existence search (`defines_class_getitem`) uses -- existence
    /// doesn't care which table wins, so that search can check both tables
    /// together at each MRO entry, but the winning declaration used to
    /// resolve a *return type* must be the exact same one value position
    /// would pick. `Some` only when an explicit hook exists somewhere in the
    /// MRO -- never set for subscriptability granted purely by a PEP 695
    /// type parameter with no hook of its own, since that case is
    /// deliberately still handled by `GenericClassInstantiate`, not by this
    /// field. `annotation_to_ty`'s `Subscript` arm routes
    /// `ClassName[type_arg]` through this return type instead of falling
    /// back to `Ty::Instance(ClassName)` when it is `Some`, and takes that
    /// same `Ty::Instance` fallback when it is `None` -- including the
    /// structurally-unreachable-in-practice case where `subscriptable` is
    /// true (the hook exists) but `class_getitem_return_ty`'s `items` lookup
    /// still comes back empty, since `subscriptable` is deliberately keyed
    /// on hook existence alone and never on this field's own resolution
    /// outcome (issue #693 deep-review, Finding 2). Always `None` for the
    /// self-referential entry `lower_class` pushes for the class it is
    /// currently lowering (see that call site's own comment) -- the hook's
    /// return type is not yet resolvable at that point, so a
    /// `__class_getitem__`-typed annotation used *inside* the defining
    /// class's own body still falls back to `Ty::Instance`, same as before
    /// this field existed.
    pub(crate) class_getitem_return: Option<Ty>,
}

/// Projects the full class table down to the [`ClassAnnotationInfo`] slice
/// `annotation_to_ty` consults. Both of this crate's own projection sites
/// (`lower_checked`'s per-statement rebuild and `lower_class`'s own
/// `defined_classes` view) go through here, so the two agree on every flag.
/// `items` is the module's `HirItem::Function` list accumulated so far (in
/// source order) -- it carries the resolved `return_ty` for every already-
/// lowered method, including any `__class_getitem__` hook, which `defs`
/// alone does not (`HirClassDef::static_methods`/`class_methods` store only
/// mangled names).
pub(crate) fn class_annotation_infos(
    defs: &[(String, HirClassDef)],
    items: &[HirItem],
) -> Vec<ClassAnnotationInfo> {
    defs.iter()
        .map(|(name, def)| {
            let class_getitem_return = class_getitem_return_ty(defs, &def.mro, items);
            ClassAnnotationInfo {
                name: name.clone(),
                is_protocol: def.is_protocol,
                // Subscriptability is gated purely on hook *existence*
                // (`defines_class_getitem`), never on `class_getitem_return`'s
                // own success at resolving a return type. This is
                // deliberately decoupled: `class_getitem_return_ty`'s
                // `items` lookup is documented as unreachable-in-practice
                // when the hook exists, but that is an invariant of the
                // current call graph, not something this type enforces. If
                // that invariant were ever violated, coupling
                // `subscriptable` to the resolution outcome would silently
                // flip a previously-accepted class to a T0044 rejection
                // instead of degrading to the pre-#693
                // `Ty::Instance(ClassName)` fallback that
                // `annotation_to_ty`'s `Subscript` arm already provides for
                // a `None` `class_getitem_return`.
                subscriptable: def.type_param.is_some()
                    || defines_class_getitem(defs, &def.mro),
                class_getitem_return,
            }
        })
        .collect()
}

/// Issue #693: resolves the declared return type of whichever
/// `__class_getitem__` hook `resolve_static_or_class_method_call` (in
/// `pycc_types`) would itself dispatch to for a value-position `C[x]` call on
/// this MRO, so that annotation-position `C[x]` and value-position `C[x]`
/// always resolve through the exact same MRO entry.
///
/// Deliberately replicates that function's two-pass order rather than a
/// single combined pass: first walk the full MRO (most-derived first)
/// checking only `static_methods`; only if no class in the MRO declares the
/// hook as a `@staticmethod` does a second full MRO walk check
/// `class_methods`. A single pass that checks both tables together at each
/// MRO entry would let a derived class's `@classmethod` override win over a
/// base class's `@staticmethod` declaration merely because the derived class
/// comes first in the MRO -- `resolve_static_or_class_method_call` instead
/// lets *any* static-method declaration anywhere in the MRO outrank *every*
/// class-method declaration, regardless of MRO position. Returns `None` when
/// no class in the MRO declares the hook in either form, or (structurally
/// unreachable in practice, since every `static_methods`/`class_methods`
/// entry is pushed alongside a matching `items.push(item)` in the same
/// `lower_class` call) when the mangled name has no corresponding
/// `HirItem::Function` yet.
fn class_getitem_return_ty(
    defs: &[(String, HirClassDef)],
    mro: &[String],
    items: &[HirItem],
) -> Option<Ty> {
    // `return_ty` here is read straight off the `HirItem::Function` this
    // crate's own lowering produced -- before `pycc_types::check_and_resolve`
    // ever runs (see `lower_method`'s doc comment on `Ty::Infer`). An
    // unannotated `__class_getitem__` hook therefore still carries the raw
    // `Ty::Infer` placeholder at this point, even though value-position
    // `C[x]` dispatch (`pycc_types::resolve_static_or_class_method_call`)
    // only ever observes the hook's *post-inference* signature via the
    // `Environment`. Treat `Ty::Infer` as unresolved here too, so
    // `func.rs`'s `annotation_to_ty` falls back to `Ty::Instance(ClassName)`
    // instead of propagating an internal placeholder into a resolved
    // annotation type (issue #693 review, codex finding).
    let resolve = |mangled: &str| -> Option<Ty> {
        items.iter().find_map(|item| match item {
            HirItem::Function {
                name, return_ty, ..
            } if name == mangled => match return_ty {
                Ty::Infer => None,
                other => Some(other.clone()),
            },
            _ => None,
        })
    };
    // Unlike most other MRO walks in this crate, a class here is not
    // guaranteed to be present in `defs`: `class_annotation_infos` also runs
    // (via `lower_class`'s own pre-method-loop projection) against a
    // hand-built `defined_classes` slice that
    // `class::mro::tests::circular_inheritance_in_mro_is_rejected`
    // deliberately leaves incomplete to exercise the *defensive*
    // circular-inheritance check downstream in `resolve_mro` -- that check,
    // not this function, is the one responsible for rejecting such an MRO.
    // Skip a class this function cannot resolve rather than panicking; each
    // loop naturally continues to the next ancestor, and no hook is found
    // through this leg either way.
    for mro_class in mro {
        let Some((_, mro_def)) = defs.iter().find(|(name, _)| name == mro_class) else {
            continue;
        };
        if let Some((_, mangled)) = mro_def
            .static_methods
            .iter()
            .find(|(method, _)| method == "__class_getitem__")
        {
            return resolve(mangled);
        }
    }
    for mro_class in mro {
        let Some((_, mro_def)) = defs.iter().find(|(name, _)| name == mro_class) else {
            continue;
        };
        if let Some((_, mangled)) = mro_def
            .class_methods
            .iter()
            .find(|(method, _)| method == "__class_getitem__")
        {
            return resolve(mangled);
        }
    }
    None
}

/// PEP 560 (#611): whether `def` declares `__class_getitem__` anywhere in
/// its MRO. Checks `static_methods` and `class_methods` -- the same two
/// tables, for the same reason, that `pycc_types`' own value-position
/// `resolve_static_or_class_method_call` walks when it dispatches `C[x]`.
/// The two crates must agree on which classes are subscriptable, so the
/// lookups are deliberately kept parallel. `class_annotation_infos` calls
/// this to compute `ClassAnnotationInfo::subscriptable` independently of
/// whether `class_getitem_return_ty`'s own resolution of the hook's return
/// type succeeds (issue #693 deep-review, Finding 2): a class can be
/// subscriptable purely by declaring the hook, even in the
/// structurally-unreachable-in-practice case where the return-type lookup
/// comes back empty.
///
/// Iterates the class table and tests MRO membership, rather than iterating
/// `mro` and looking each entry up in `defs` (the shape `class_getitem_return_ty`
/// uses, since it also needs the mangled name for a *specific* MRO entry
/// once found). The two shapes handle a `mro` entry absent from `defs` --
/// deliberately exercised by `class::mro::tests::circular_inheritance_in_mro_is_rejected`
/// via an incomplete `defined_classes` slice -- differently but equivalently
/// safely: `class_getitem_return_ty` walks `mro` and defensively `continue`s
/// past an entry it cannot look up, while this function walks `defs` and
/// filters by `mro.contains`, so an entry missing from `defs` is simply
/// never visited by the iteration at all. Neither shape panics or needs an
/// `Option`/`unwrap` for the missing case; this one just never constructs
/// the "look it up and get `None`" arm in the first place.
fn defines_class_getitem(defs: &[(String, HirClassDef)], mro: &[String]) -> bool {
    defs.iter().any(|(name, mro_def)| {
        mro.contains(name)
            && mro_def
                .static_methods
                .iter()
                .chain(&mro_def.class_methods)
                .any(|(method, _)| method == "__class_getitem__")
    })
}

/// `base_class_asts` (#585) pairs every already-lowered class name in this
/// module with its original `StmtClassDef`, so this call can re-inspect an
/// *earlier* class's own `__init_subclass__` body when the class being
/// lowered here inherits, rather than overrides, that hook -- `defined_classes`
/// alone only carries the already-lowered `HirClassDef`, which has no method
/// bodies left to re-validate. A base absent from this slice (a synthetic
/// builtin-exception class, or any base this crate cannot introspect) is
/// simply treated as not defining a validatable inherited hook, matching
/// this file's existing "unintrospectable -> unrestricted" posture elsewhere.
pub(crate) fn lower_class(
    def: &pycc_ast::StmtClassDef,
    aliases: &[(String, Ty)],
    defined_classes: &[(String, HirClassDef)],
    module_items: &[HirItem],
    base_class_asts: &[(String, &pycc_ast::StmtClassDef)],
) -> Result<(HirClassDef, Vec<HirItem>), Diagnostic> {
    // #380 (PR-20): build the projected class slice `annotation_to_ty` uses
    // to resolve cross-class annotations (including protocol-typed ones);
    // #611 (PEP 560) added the per-class subscriptability flag it carries;
    // #693 added `class_getitem_return`, resolved from `module_items` (every
    // `HirItem::Function` lowered by an earlier class or top-level `def` in
    // this module, in source order).
    let mut class_name_defs = class_annotation_infos(defined_classes, module_items);
    // PEP 3129/557/681/544 (#378/#380, PR-18/PR-20): classify the class's
    // decorator list. `@dataclass`/`@dataclass_transform(...)` and
    // `@runtime_checkable` are recognized; any other class decorator is
    // rejected with C0001.
    let decorator_info = classify_class_decorator(&def.decorator_list, def.range.into())?;
    let is_dataclass = decorator_info.is_dataclass;
    // PEP 695 (#387): a generic class (`class C[T]:`) with exactly one type
    // parameter is now supported, reusing PR-13's `Ty::Param` call-site-
    // substitution mechanism (D-133/D-134). More than one type parameter is
    // rejected, matching PR-13's own generic-function scoping.
    let type_param: Option<String> = match def.type_params.as_deref() {
        None => None,
        Some(type_params) => match type_params.type_params.as_slice() {
            [single] => Some(crate::type_param_name(single, def.range)?.to_string()),
            _ => {
                return Err(unsupported(
                    "generic classes with more than one type parameter are not supported yet",
                    def.range,
                ));
            }
        },
    };
    // #432: parse base class names from the class header's positional
    // arguments (`class C(A, B):` → `["A", "B"]`). Keyword arguments
    // (e.g. `metaclass=Meta`) are still rejected, and a non-`Expr::Name`
    // positional base (e.g. `class C(SomeMod.Base):`) is also rejected --
    // only a bare name is supported as a base class for now.
    let mut bases: Vec<String> = Vec::new();
    if let Some(arguments) = def.arguments.as_deref() {
        if !arguments.keywords.is_empty() {
            return Err(unsupported(
                "keyword arguments in a class header (e.g. `metaclass=`) are not supported yet",
                def.range,
            ));
        }
        for arg in arguments.args.iter() {
            let Expr::Name(name) = arg else {
                return Err(unsupported(
                    "a base class must be a bare name (e.g. `class C(Base):`), not an \
                     attribute access or other expression",
                    pycc_ast::expr_range(arg),
                ));
            };
            let base_name = name.id.to_string();
            // Reject duplicate bases in the same class header.
            if bases.contains(&base_name) {
                return Err(unsupported(
                    format!(
                        "class `{}` lists base `{base_name}` more than once -- duplicate bases \
                         are not supported",
                        def.name.as_str()
                    ),
                    def.range,
                ));
            }
            bases.push(base_name);
        }
    }
    let class_name = def.name.to_string();
    // #379 (PR-19): PEP 435 enum detection. A class whose single base is
    // the bare name `Enum` (`class Color(Enum):`) is an enum class. `Enum`
    // is a builtin base name (see `is_enum_base_name`), not a user-defined
    // class in `defined_classes` -- so it must be intercepted here, before
    // the unknown-base rejection below. An enum class has `bases = []` and
    // `mro = [self_name]` (the `Enum` base is consumed as a marker, not
    // recorded as a real base), no `__init__` requirement, and its body is
    // assignments only (`RED = 1`), not method definitions. Multiple bases
    // with `Enum` (e.g. `class C(Enum, Other):`) are rejected -- `Enum`
    // must be the sole base. A generic enum (`class C[T](Enum):`) is also
    // rejected -- enums are never generic.
    let is_enum = bases.len() == 1 && crate::is_enum_base_name(&bases[0]);
    if is_enum {
        if type_param.is_some() {
            return Err(unsupported(
                format!(
                    "generic enum class `{class_name}` is not supported -- enums cannot have \
                     type parameters"
                ),
                def.range,
            ));
        }
        // Consume the `Enum` base as a marker: clear `bases` so the class
        // has no real inheritance, and set `mro = [self_name]`.
        bases.clear();
    }
    // #380 (PR-20): PEP 544 protocol detection. A class whose sole base is
    // the bare name `Protocol` (`class P(Protocol):`) is a protocol class.
    // `Protocol` is a builtin base name (see `is_protocol_base_name`), not
    // a user-defined class — intercepted here, before the unknown-base
    // rejection below. A protocol class has `bases = []` and
    // `mro = [self_name]` (the `Protocol` base is consumed as a marker,
    // like `Enum`). Multiple bases with `Protocol` are rejected —
    // `Protocol` must be the sole base. A generic protocol is also
    // rejected. A protocol class with `@dataclass` is rejected (a protocol
    // is a compile-time-only interface, not an instantiable class with
    // fields).
    let is_protocol = bases.len() == 1 && crate::is_protocol_base_name(&bases[0]);
    if is_protocol {
        if type_param.is_some() {
            return Err(unsupported(
                format!(
                    "generic protocol class `{class_name}` is not supported -- protocols cannot \
                     have type parameters in this version"
                ),
                def.range,
            ));
        }
        if is_dataclass {
            return Err(unsupported(
                format!(
                    "protocol class `{class_name}` cannot be a `@dataclass` -- a protocol is a \
                     compile-time-only interface description, not an instantiable class with fields"
                ),
                def.range,
            ));
        }
        // Consume the `Protocol` base as a marker: clear `bases` so the
        // class has no real inheritance, and set `mro = [self_name]`.
        bases.clear();
    }
    // #380 (PR-20): PEP 3119 ABC detection. A class with `ABC` among its
    // bases (`class C(ABC):` or `class C(ABC, Base):`) is abstract. `ABC`
    // is a builtin base name (see `is_abc_base_name`), consumed as a
    // marker — removed from `bases` so it is not treated as a real base
    // class. Unlike `Enum`/`Protocol`, `ABC` can coexist with other bases
    // (`class C(ABC, Base):` is valid — `Base` is a real base, `ABC` is
    // just a marker).
    let is_abstract = bases.iter().any(|b| crate::is_abc_base_name(b));
    if is_abstract {
        // Remove `ABC` from bases — it is a marker, not a real base.
        bases.retain(|b| !crate::is_abc_base_name(b));
    }
    // #380 (PR-20): `@runtime_checkable` on a non-protocol class is
    // rejected — `runtime_checkable` is only valid on protocol classes.
    if decorator_info.runtime_checkable && !is_protocol {
        return Err(unsupported(
            format!(
                "`@runtime_checkable` on class `{class_name}` is not valid -- \
                 `@runtime_checkable` is only valid on protocol classes (`class P(Protocol):`)"
            ),
            def.range,
        ));
    }
    // #432: A generic class (from #387) with base classes is not supported
    // yet — `instantiate_generic_class_methods` creates a monomorphized
    // `HirClassDef` with empty `bases`/`mro`, silently dropping inheritance.
    // Reject this early with a clear error rather than letting it fail with
    // a confusing T0044 downstream.
    if type_param.is_some() && !bases.is_empty() {
        return Err(unsupported(
            format!(
                "generic class `{class_name}` with base classes is not supported yet -- \
                 generic classes cannot inherit from other classes in this version"
            ),
            def.range,
        ));
    }
    validate_bases(&class_name, &bases, defined_classes, def.range.into())?;
    // #380 (PR-20): protocol inheritance detection. A class that inherits
    // from a user-defined protocol (`class Q(P):` where `P` is a protocol)
    // is itself a protocol. This check runs after the `Protocol` marker
    // base consumption above (which handles `class P(Protocol):`) and
    // after the base validation loop (which ensures every base is a known
    // class). If any base is a protocol, this class is a protocol too —
    // it inherits the base's `protocol_members` and adds its own.
    let inherits_protocol = bases.iter().any(|base_name| {
        defined_classes
            .iter()
            .any(|(n, d)| n == base_name && d.is_protocol)
    });
    let is_protocol = is_protocol || inherits_protocol;
    if inherits_protocol {
        // A protocol inheriting from a protocol cannot also be a dataclass.
        if is_dataclass {
            return Err(unsupported(
                format!(
                    "protocol class `{class_name}` cannot be a `@dataclass` -- a protocol is a \
                     compile-time-only interface description, not an instantiable class with fields"
                ),
                def.range,
            ));
        }
        // `@runtime_checkable` is valid on a protocol inheriting from
        // another protocol — no extra check needed here.
    }
    let mro = resolve_mro(&class_name, &bases, defined_classes, def.range.into())?;
    // PEP 560 (#611): the class currently being lowered is not in
    // `defined_classes` yet, so without this entry a self-referential
    // `C[int]` annotation inside `C`'s own body would bypass the
    // subscriptability gate that every *other* class name goes through. Its
    // own `static_methods`/`class_methods` tables are still empty at this
    // point, so an own hook is detected by pre-scanning the class body's
    // `def`s; a hook inherited from a base is found by the same MRO walk
    // over `defined_classes` that `class_annotation_infos` uses. A
    // `classify_decorator` error is treated as "no hook" here -- the
    // class-body loop below reports it properly a few lines later, and
    // this pre-scan must not pre-empt that diagnostic.
    let declares_own_class_getitem = def.body.iter().any(|stmt| match stmt {
        Stmt::FunctionDef(method) if method.name.id.as_str() == "__class_getitem__" => matches!(
            classify_decorator(
                &method.decorator_list,
                "__class_getitem__",
                method.range.into()
            ),
            Ok(MethodKind::StaticMethod | MethodKind::ClassMethod)
        ),
        _ => false,
    });
    class_name_defs.push(ClassAnnotationInfo {
        name: class_name.clone(),
        is_protocol,
        subscriptable: type_param.is_some()
            || declares_own_class_getitem
            || defines_class_getitem(defined_classes, &mro),
        // #693: the hook's return type is not resolvable yet at this
        // point -- an own hook (`declares_own_class_getitem`) has not been
        // lowered into an `HirItem::Function` yet, and this self-referential
        // entry exists specifically to keep a same-body annotation from
        // hitting the "not subscriptable" rejection, not to type it
        // precisely. Such an annotation (rare: `ClassName[x]` referring to
        // the very class whose body it appears in) falls back to
        // `Ty::Instance`, exactly as it did before this field existed.
        class_getitem_return: None,
    });
    let mut methods: Vec<(String, String)> = Vec::new();
    let mut items: Vec<HirItem> = Vec::new();
    let mut attrs: Vec<(String, Ty)> = Vec::new();
    let mut properties: Vec<PropertyDef> = Vec::new();
    let mut static_methods: Vec<(String, String)> = Vec::new();
    let mut class_methods: Vec<(String, String)> = Vec::new();
    let enum_members: Vec<(String, i64)> = Vec::new();
    let mut dataclass_fields: Vec<(String, Ty)> = Vec::new();
    let mut abstract_methods: Vec<String> = Vec::new();
    let mut init_seen = false;
    // #379 (PR-19): an enum class body is assignments only (`RED = 1`),
    // not method definitions. Handle this in a separate function before the
    // regular class-body loop below, which rejects non-`def` statements.
    if is_enum {
        return lower_enum_class(def, class_name, bases, mro, type_param);
    }
    // #380 (PR-20): a protocol class body contains method definitions
    // (with `...` or `pass` bodies) and annotated assignments
    // (`x: int`). Methods are recorded as `ProtocolMember::Method` and
    // are NOT lowered to `HirItem::Function`s. Annotated assignments are
    // recorded as `ProtocolMember::Attribute`. A protocol method with an
    // implementation body (anything other than `...` or `pass`) is
    // rejected with `C0001`. A protocol `__init__` is rejected with
    // `C0001`. Protocol members from base protocols are inherited.
    if is_protocol {
        return lower_protocol_class(
            def,
            class_name,
            bases,
            mro,
            type_param,
            decorator_info.runtime_checkable,
            defined_classes,
            &class_name_defs,
        );
    }
    // #378 (PR-18): a `@dataclass` class inheriting from a non-dataclass
    // base is rejected -- the base must also be a dataclass for field
    // merging to work. A `@dataclass` with no bases is always valid.
    if is_dataclass {
        for base_name in &bases {
            let base_def = defined_classes
                .iter()
                .find(|(name, _)| name == base_name)
                .map(|(_, def)| def)
                .expect("base class must be defined before the derived class");
            if !base_def.is_dataclass {
                return Err(unsupported(
                    format!(
                        "a `@dataclass` class `{class_name}` cannot inherit from non-dataclass \
                         class `{base_name}` -- dataclass inheritance requires all bases to also \
                         be dataclasses in this version"
                    ),
                    def.range,
                ));
            }
        }
    }
    for stmt in &def.body {
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
                type_param.as_deref(),
                Some(&class_name),
                aliases,
                &class_name_defs,
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
            &class_name,
            type_param.as_deref(),
            aliases,
            &kind,
            &class_name_defs,
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
    // #378 (PR-18): synthesize `__init__`, `__eq__`, and `__repr__` for a
    // `@dataclass` class from its (merged) field list. The synthesized
    // methods flow through the existing method infrastructure as ordinary
    // mangled `HirItem::Function`s, exactly like hand-written methods.
    if is_dataclass {
        // Merge parent dataclass fields (via MRO, in reverse order so
        // parent fields come first) with own fields. The MRO is ordered
        // most-derived-first, so walking it in reverse gives
        // least-derived-first (parent fields before own fields), matching
        // PEP 557's field ordering for inheritance.
        let mut merged_fields: Vec<(String, Ty)> = Vec::new();
        for mro_class in mro.iter().skip(1).rev() {
            // Every class in the MRO (other than the class itself, which
            // `skip(1)` removes) was defined earlier in the module -- the
            // C3 MRO is built from already-lowered class definitions, and
            // dataclass inheritance requires all bases to be dataclasses.
            // Using `.expect` (whose panic path lives in libcore, outside
            // this crate's instrumented regions) avoids an `if let Some`
            // whose `else` branch is structurally unreachable and would
            // show up as a permanently uncovered line under D-014's 100%
            // line-coverage gate.
            let (_, base_def) = defined_classes
                .iter()
                .find(|(name, _)| name == mro_class)
                .expect("MRO class must be defined before the derived class");
            for (name, ty) in &base_def.dataclass_fields {
                match merged_fields.iter().find(|(n, _)| n == name) {
                    Some((_, existing_ty)) if existing_ty != ty => {
                        return Err(Diagnostic::error(
                            "T0052",
                            format!(
                                "attribute `{name}` is declared as `{}` and as `{}` by different dataclasses in the method resolution order of class `{class_name}`",
                                existing_ty.name(),
                                ty.name(),
                            ),
                            Span::new(
                                u32::from(def.range.start()),
                                u32::from(def.range.end()),
                            ),
                        ));
                    }
                    Some(_) => {}
                    None => merged_fields.push((name.clone(), ty.clone())),
                }
            }
        }
        for (name, ty) in &dataclass_fields {
            match merged_fields.iter().find(|(n, _)| n == name) {
                Some((_, existing_ty)) if existing_ty != ty => {
                    return Err(Diagnostic::error(
                        "T0052",
                        format!(
                            "attribute `{name}` is declared as `{}` in a base class and as `{}` in dataclass `{class_name}`, both in the same method resolution order",
                            existing_ty.name(),
                            ty.name(),
                        ),
                        Span::new(u32::from(def.range.start()), u32::from(def.range.end())),
                    ));
                }
                Some(_) => {}
                None => merged_fields.push((name.clone(), ty.clone())),
            }
        }
        // Populate `attrs` from the merged field list (the dataclass's
        // attribute slots are exactly its fields, in declaration order).
        attrs = merged_fields.clone();
        // Synthesize `__init__`: `def __init__(self, f1: T1, f2: T2, ...):
        // self.f1 = f1; self.f2 = f2; ...`
        let init_item = synthesize_dataclass_init(&class_name, &merged_fields);
        let init_mangled = format!("{class_name}.__init__");
        methods.push(("__init__".to_string(), init_mangled));
        items.push(init_item);
        // Synthesize `__eq__`: `def __eq__(self, other: ClassName) -> bool:
        // return self.f1 == other.f1 and self.f2 == other.f2 and ...`
        let eq_item = synthesize_dataclass_eq(&class_name, &merged_fields);
        let eq_mangled = format!("{class_name}.__eq__");
        methods.push(("__eq__".to_string(), eq_mangled));
        items.push(eq_item);
        // Synthesize `__repr__`: `def __repr__(self) -> str: return
        // "ClassName(f1=" + str(self.f1) + ", f2=" + str(self.f2) + ...)"`
        let repr_item = synthesize_dataclass_repr(&class_name, &merged_fields);
        let repr_mangled = format!("{class_name}.__repr__");
        methods.push(("__repr__".to_string(), repr_mangled));
        items.push(repr_item);
        // Store the merged field list for downstream consumers.
        dataclass_fields = merged_fields;
    }
    // #380 (PR-20, PEP 3119): collect inherited abstract methods from the
    // MRO. A concrete (non-abstract) class must override every inherited
    // abstract method — a concrete subclass missing an override is
    // rejected with `C0001`. An abstract class (`is_abstract`) can have
    // unimplemented abstract methods (they are inherited by subclasses).
    let mut all_abstract_methods: Vec<String> = abstract_methods.clone();
    // Iterate over MRO entries (beyond the class itself), finding each in
    // `defined_classes`.  Using `filter_map` avoids a conditional with an
    // unreachable `else` branch — every MRO class is always in
    // `defined_classes`, but `filter_map` skips any that aren't without
    // creating a permanently-uncovered region under D-014.
    for (_, base_def) in mro
        .iter()
        .skip(1)
        .filter_map(|mro_class| defined_classes.iter().find(|(n, _)| n == mro_class))
    {
        for am in &base_def.abstract_methods {
            if !all_abstract_methods.contains(am) {
                all_abstract_methods.push(am.clone());
            }
        }
    }
    // A concrete class must override every inherited abstract method.
    // "Override" means the method name appears in this class's own
    // `methods` (not just inherited). `is_abstract` classes are exempt.
    if !is_abstract {
        for am in &all_abstract_methods {
            // Check if this class overrides the abstract method with a
            // concrete (non-abstract) method.
            let overridden =
                methods.iter().any(|(name, _)| name == am) && !abstract_methods.contains(am);
            if !overridden {
                return Err(unsupported(
                    format!(
                        "concrete class `{class_name}` does not override abstract method \
                         `{am}` inherited from a base class -- all abstract methods must be \
                         overridden in a concrete class"
                    ),
                    def.range,
                ));
            }
        }
    }
    if !is_dataclass && !methods.iter().any(|(name, _)| name == "__init__") {
        // #432: a derived class without its own `__init__` inherits the
        // base class's `__init__` -- check the MRO for a class that has one.
        // The MRO is ordered most-derived-first, so the first `__init__`
        // found is the one that would be called (matching CPython's own
        // MRO-based constructor resolution).
        let has_inherited_init = mro.iter().skip(1).any(|mro_class| {
            defined_classes
                .iter()
                .find(|(name, _)| name == mro_class)
                .map(|(_, cd)| cd.methods.iter().any(|(mn, _)| mn == "__init__"))
                .unwrap_or(false)
        });
        if !has_inherited_init {
            return Err(unsupported(
                "a class without an `__init__` method is not supported yet \
                 (and no base class in its MRO provides one)",
                def.range,
            ));
        }
    }
    // #435 (Part B, __init_subclass__): PEP 487's `__init_subclass__` hook
    // is recognized as a valid method name. In CPython, `__init_subclass__`
    // is called automatically when a class is subclassed. In pycc's
    // compile-time model, class creation happens at HIR-lowering time, so
    // the hook has no runtime effect — it is accepted as a regular method
    // (it can be called explicitly by user code). However, if a base class
    // in the MRO defines `__init_subclass__`, the current class's own
    // `__init_subclass__` (if any) must be statically evaluable: only a
    // `pass` body (or a body consisting solely of a docstring expression
    // statement) is accepted, since any side-effecting statement would need
    // to run at class-creation time, which pycc does not support.
    if methods.iter().any(|(name, _)| name == "__init_subclass__") {
        // Check if any base class in the MRO (excluding the current class)
        // defines `__init_subclass__`.
        let base_has_init_subclass = mro.iter().skip(1).any(|mro_class| {
            defined_classes
                .iter()
                .find(|(name, _)| name == mro_class)
                .map(|(_, cd)| cd.methods.iter().any(|(mn, _)| mn == "__init_subclass__"))
                .unwrap_or(false)
        });
        if base_has_init_subclass {
            // The current class's `__init_subclass__` must be statically
            // evaluable. Find the last `__init_subclass__` def in the body
            // (rebind semantics: the last definition is the one in the
            // methods table) and validate its body.
            for stmt in def.body.iter().rev() {
                match stmt {
                    Stmt::FunctionDef(fd) if fd.name.as_str() == "__init_subclass__" => {
                        validate_init_subclass_body(&fd.body, fd.range, false)?;
                        break;
                    }
                    _ => {}
                }
            }
        }
    } else if let Some(base_ast) = mro.iter().skip(1).find_map(|mro_class| {
        // #585: the current class does not define its own
        // `__init_subclass__`, but a base in its MRO does. CPython invokes
        // that inherited hook automatically at this very subclass's
        // creation, so -- unlike a base class that is merely defined and
        // never subclassed, which must stay legal to compile -- this
        // subclass statement is the point where an unsupported, side-
        // effecting inherited hook must be rejected. Only a base this crate
        // can still see the original source of (an earlier class in this
        // same module, per `base_class_asts`) can be re-validated; a base
        // this crate cannot introspect (e.g. a synthetic builtin-exception
        // class, which is never in `base_class_asts`) is left unrestricted,
        // matching this file's existing posture elsewhere. Looking the base
        // up directly in `base_class_asts` (rather than via `defined_classes`
        // first) means a base this crate cannot introspect is filtered out
        // by the same `?` that filters out one lacking `__init_subclass__`,
        // instead of needing a second, separately-unreachable fallback path.
        let (_, base_ast) = base_class_asts.iter().find(|(name, _)| name == mro_class)?;
        base_ast
            .body
            .iter()
            .any(|s| matches!(s, Stmt::FunctionDef(fd) if fd.name.as_str() == "__init_subclass__"))
            .then_some(*base_ast)
    }) {
        for stmt in base_ast.body.iter().rev() {
            match stmt {
                Stmt::FunctionDef(fd) if fd.name.as_str() == "__init_subclass__" => {
                    validate_init_subclass_body(&fd.body, def.range, true)?;
                    break;
                }
                _ => {}
            }
        }
    }
    Ok((
        HirClassDef {
            exception_type_tag: None,
            name: class_name,
            bases,
            mro,
            attrs,
            methods,
            properties,
            static_methods,
            class_methods,
            type_param,
            enum_members,
            is_dataclass,
            dataclass_fields,
            is_protocol: false,
            runtime_checkable: false,
            protocol_members: Vec::new(),
            abstract_methods: all_abstract_methods,
            is_abstract,
        },
        items,
    ))
}

/// Lowers a single method definition into an ordinary `HirItem::Function`
/// under its mangled `<ClassName>.<method_name>` name, plus that method's
/// own full parameter list (including `self`) -- returned alongside so
/// `lower_class` can build the `__init__`-specific attribute-slot pre-scan's
/// parameter-name -> `Ty` lookup table without re-deriving it.
///
/// `self`'s type never goes through `annotation_to_ty` -- it is assigned
/// `Ty::Instance(Box::new(class_name))` directly (mirroring how the type
/// itself carries only the class's name, not its shape), bypassing the
/// class-typed-annotation restriction entirely. An explicit annotation on
/// `self` is rejected rather than silently ignored, so a user-written
/// (and unchecked) annotation there can never appear to be honored.
///
/// `__init__`'s own (non-`self`) parameters are *always* required to carry
/// an explicit type annotation, regardless of the ordinary "only a public
/// name requires one" rule (D-038) every other function/method follows --
/// a deliberate, narrower rule than D-038's, not an oversight: those
/// parameter types are the only source `collect_init_attrs` below has for
/// deriving an attribute slot's `Ty` structurally, at HIR-lowering time,
/// with no type-inference pass of its own (this crate never runs one --
/// see `Ty::Infer`'s own doc comment). An unannotated `__init__` parameter
/// referenced by a `self.<attr> = <param>` assignment would otherwise seed
/// the slot with `Ty::Infer`, which must never reach `pycc_mir` unresolved.
fn lower_method(
    def: &pycc_ast::StmtFunctionDef,
    class_name: &str,
    type_param: Option<&str>,
    aliases: &[(String, Ty)],
    kind: &MethodKind,
    class_defs: &[ClassAnnotationInfo],
) -> Result<(HirItem, Vec<(String, Ty)>), Diagnostic> {
    if def.is_async {
        return Err(unsupported(
            "an async method is not supported yet",
            def.range,
        ));
    }
    // #377: decorators are now classified by `classify_decorator` in
    // `lower_class` before this function is called -- the `kind` parameter
    // carries the result. No additional decorator check is needed here.
    if def.type_params.is_some() {
        return Err(unsupported(
            "a generic method is not supported yet",
            def.range,
        ));
    }
    let parameters = &def.parameters;
    // PEP 570 (#383): positional-only parameters (`posonlyargs`, before the
    // `/` marker) are now lowered. For `@staticmethod`, posonlyargs come
    // first (no implicit `self`/`cls`). For regular/classmethod methods,
    // `self`/`cls` is always the first parameter, so posonlyargs follow it.
    // Since keyword call arguments are already globally unsupported, every
    // parameter is already effectively positional-only — accepting
    // posonlyargs changes nothing about call-site checking.
    if parameters.vararg.is_some() {
        return Err(unsupported(
            "`*args` is not supported yet",
            parameters.range,
        ));
    }
    if !parameters.kwonlyargs.is_empty() {
        return Err(unsupported(
            "keyword-only parameters are not supported yet",
            parameters.range,
        ));
    }
    if parameters.kwarg.is_some() {
        return Err(unsupported(
            "`**kwargs` is not supported yet",
            parameters.range,
        ));
    }
    let method_name = def.name.as_str();
    let is_public = !method_name.starts_with('_'); // D-038
    let params_is_public = is_public || method_name == "__init__";
    // #436: a `@staticmethod` takes no implicit `self`/`cls` -- the
    // method's own parameter list is exactly what the user wrote. A
    // `@classmethod` takes an implicit `cls` (typed
    // `Ty::Instance(class_name)`, matching `self`'s own type in this
    // compiler's static-dispatch model) as its first parameter. A
    // regular/property method takes `self` as before.
    let params = match kind {
        MethodKind::StaticMethod => {
            // PEP 570 (#383): for `@staticmethod`, posonlyargs come first
            // (no implicit `self`/`cls`), then ordinary `args`.
            let mut p = lower_arg_list(
                &parameters.posonlyargs,
                params_is_public,
                method_name,
                type_param,
                Some(class_name),
                aliases,
                class_defs,
            )?;
            p.extend(lower_arg_list(
                &parameters.args,
                params_is_public,
                method_name,
                type_param,
                Some(class_name),
                aliases,
                class_defs,
            )?);
            p
        }
        MethodKind::ClassMethod => {
            // PEP 570 (#383): `cls` is the first parameter overall — it
            // may be in `posonlyargs` (if `/` follows it) or in `args`.
            // Extract it from the combined list, then lower the rest.
            if parameters.posonlyargs.is_empty() && parameters.args.is_empty() {
                return Err(unsupported(
                    "a `@classmethod` must take `cls` as its first parameter",
                    def.range,
                ));
            }
            let (cls_param, posonly_rest, args_rest) = if !parameters.posonlyargs.is_empty() {
                let (cls, rest_pos) = parameters.posonlyargs.split_first().unwrap();
                (cls, rest_pos, parameters.args.as_slice())
            } else {
                let (cls, rest_args) = parameters.args.split_first().unwrap();
                (cls, &[][..], rest_args)
            };
            if cls_param.parameter.name.as_str() != "cls" {
                return Err(unsupported(
                    "a `@classmethod`'s first parameter must be named `cls`",
                    parameters.range,
                ));
            }
            if cls_param.default.is_some() {
                return Err(unsupported(
                    "`cls` cannot have a default value",
                    parameters.range,
                ));
            }
            if cls_param.parameter.annotation.is_some() {
                return Err(unsupported(
                    "an explicit type annotation on `cls` is not supported yet",
                    parameters.range,
                ));
            }
            let cls_ty = Ty::Instance(Box::new(class_name.to_string()));
            let mut p = vec![("cls".to_string(), cls_ty)];
            // PEP 570 (#383): remaining posonlyargs follow `cls`, before
            // ordinary `args`.
            p.extend(lower_arg_list(
                posonly_rest,
                params_is_public,
                method_name,
                type_param,
                Some(class_name),
                aliases,
                class_defs,
            )?);
            p.extend(lower_arg_list(
                args_rest,
                params_is_public,
                method_name,
                type_param,
                Some(class_name),
                aliases,
                class_defs,
            )?);
            p
        }
        _ => {
            // PEP 570 (#383): `self` is the first parameter overall — it
            // may be in `posonlyargs` (if `/` follows it) or in `args`.
            if parameters.posonlyargs.is_empty() && parameters.args.is_empty() {
                return Err(unsupported(
                    "a method must take `self` as its first parameter",
                    def.range,
                ));
            }
            let (self_param, posonly_rest, args_rest) = if !parameters.posonlyargs.is_empty() {
                let (self_p, rest_pos) = parameters.posonlyargs.split_first().unwrap();
                (self_p, rest_pos, parameters.args.as_slice())
            } else {
                let (self_p, rest_args) = parameters.args.split_first().unwrap();
                (self_p, &[][..], rest_args)
            };
            if self_param.parameter.name.as_str() != "self" {
                return Err(unsupported(
                    "a method's first parameter must be named `self`",
                    parameters.range,
                ));
            }
            if self_param.default.is_some() {
                return Err(unsupported(
                    "`self` cannot have a default value",
                    parameters.range,
                ));
            }
            if self_param.parameter.annotation.is_some() {
                return Err(unsupported(
                    "an explicit type annotation on `self` is not supported yet",
                    parameters.range,
                ));
            }
            // #377: a `@property` getter takes only `self` (no additional
            // parameters); a `@<name>.setter` setter takes exactly one
            // additional parameter (the value to assign). A regular method
            // has no arity constraint beyond the structural checks above.
            let extra_count = posonly_rest.len() + args_rest.len();
            match kind {
                MethodKind::PropertyGetter { .. } if extra_count > 0 => {
                    return Err(unsupported(
                        "a `@property` getter must take only `self` (no additional parameters)",
                        parameters.range,
                    ));
                }
                MethodKind::PropertySetter { .. } if extra_count != 1 => {
                    return Err(unsupported(
                        "a `@<name>.setter` setter must take exactly one parameter besides `self`",
                        parameters.range,
                    ));
                }
                _ => {}
            }
            let self_ty = Ty::Instance(Box::new(class_name.to_string()));
            let mut p = vec![("self".to_string(), self_ty)];
            // PEP 570 (#383): remaining posonlyargs follow `self`, before
            // ordinary `args`.
            p.extend(lower_arg_list(
                posonly_rest,
                params_is_public,
                method_name,
                type_param,
                Some(class_name),
                aliases,
                class_defs,
            )?);
            p.extend(lower_arg_list(
                args_rest,
                params_is_public,
                method_name,
                type_param,
                Some(class_name),
                aliases,
                class_defs,
            )?);
            p
        }
    };
    let return_ty = crate::lower_return_annotation(
        def.returns.as_deref(),
        is_public,
        method_name,
        type_param,
        Some(class_name),
        aliases,
        class_defs,
    )?;
    let body = if matches!(kind, MethodKind::AbstractMethod) {
        // #380 (PR-20): an `@abstractmethod` has a declaration-style
        // body (`...` or `pass`) that is already validated in
        // `lower_class`. Skip body lowering — the abstract method is
        // registered as a function (for dispatch/mangling purposes)
        // but its body is never called. Use a `Return(None)` so the
        // function has a terminator for codegen; the type checker
        // skips the return-value check for abstract methods (see
        // `check_stmt_in_function`'s `Return(None)` arm).
        vec![crate::HirStmt::Return(None)]
    } else {
        crate::stmt::lower_body(
            &def.body,
            aliases,
            false,
            true,
            false,
            Some(class_name),
            type_param,
            class_defs,
        )?
    };
    // #377/#436: compute the mangled name based on the method kind. A
    // regular method uses `<Class>.<name>`. A property getter uses the
    // same `<Class>.<name>`. A property setter uses
    // `<Class>.<name>.setter`. A static method uses
    // `<Class>.<name>.static`. A class method uses
    // `<Class>.<name>.classmethod`. The `.static`/`.classmethod` suffixes
    // prevent collision with a regular method of the same name, since a
    // real Python identifier can never contain a `.`.
    let mangled_name = match kind {
        MethodKind::Regular { .. }
        | MethodKind::PropertyGetter { .. }
        | MethodKind::AbstractMethod => {
            format!("{class_name}.{method_name}")
        }
        MethodKind::PropertySetter { prop_name } => {
            format!("{class_name}.{prop_name}.setter")
        }
        MethodKind::StaticMethod => format!("{class_name}.{method_name}.static"),
        MethodKind::ClassMethod => format!("{class_name}.{method_name}.classmethod"),
    };
    Ok((
        HirItem::Function {
            name: mangled_name,
            params: params.clone(),
            return_ty,
            body,
        },
        params,
    ))
}

/// #378 (PR-18): Returns `true` if `ty` is a scalar slot type -- one that
/// fits in the single `i64` word per attribute slot that D-154's class-
/// instance layout uses. This is the same set `slot_ty_from_init_rhs`
/// accepts for hand-written `__init__` bodies (`int`/`float`/`bool`/`str`,
/// plus `Ty::Param` for PEP 695 generic classes, where the type parameter
/// is substituted with a concrete scalar at monomorphization time). A
/// dataclass field with a non-scalar type (`list[T]`, `dict[K, V]`,
/// `set[T]`, `tuple[...]`, `None`, or a class instance including a self-
/// referential `next: Node`/`next: Self`) is rejected at HIR-lowering
/// time with `C0001` before it can reach codegen and panic.
fn is_scalar_slot_type(ty: &Ty) -> bool {
    matches!(ty, Ty::Int | Ty::Float | Ty::Bool | Ty::Str | Ty::Param(_))
}

/// #378 (PR-18): Synthesizes a `__init__` method for a `@dataclass` class
/// from its (merged) field list. The synthesized method takes `self` plus
/// one parameter per field (in declaration order), and assigns each
/// parameter to the corresponding `self.<field>` attribute. All fields are
/// required (no defaults -- see the plan's §3.6 deferral).
fn synthesize_dataclass_init(class_name: &str, fields: &[(String, Ty)]) -> HirItem {
    let self_ty = Ty::Instance(Box::new(class_name.to_string()));
    let mut params: Vec<(String, Ty)> = vec![("self".to_string(), self_ty)];
    for (name, ty) in fields {
        params.push((name.clone(), ty.clone()));
    }
    let body: Vec<HirStmt> = fields
        .iter()
        .map(|(name, _)| HirStmt::AttrSet {
            base: HirExpr::Name("self".to_string()),
            attr: name.clone(),
            value: HirExpr::Name(name.clone()),
        })
        .collect();
    HirItem::Function {
        name: format!("{class_name}.__init__"),
        params,
        return_ty: Ty::None,
        body,
    }
}

/// #378 (PR-18): Synthesizes an `__eq__` method for a `@dataclass` class
/// from its (merged) field list. The synthesized method takes `self` and
/// `other` (both typed `Ty::Instance(class_name)`), and returns `bool` --
/// `True` if all fields are equal, `False` otherwise. The body uses a
/// series of `if self.<field> != other.<field>: return False` checks
/// followed by `return True`, since pycc's HIR has no `and`/`or` boolean
/// operator (short-circuit `and` is not lowered). A zero-field dataclass's
/// `__eq__` always returns `True` (two instances of a fieldless dataclass
/// are always equal, matching CPython's PEP 557).
fn synthesize_dataclass_eq(class_name: &str, fields: &[(String, Ty)]) -> HirItem {
    let self_ty = Ty::Instance(Box::new(class_name.to_string()));
    let params: Vec<(String, Ty)> = vec![
        ("self".to_string(), self_ty.clone()),
        ("other".to_string(), self_ty),
    ];
    let mut body: Vec<HirStmt> = Vec::new();
    for (name, _) in fields {
        // `if self.<field> != other.<field>: return False`
        body.push(HirStmt::If {
            test: HirExpr::Compare {
                op: crate::CmpOpKind::NotEq,
                left: Box::new(HirExpr::AttrGet {
                    base: Box::new(HirExpr::Name("self".to_string())),
                    attr: name.clone(),
                }),
                right: Box::new(HirExpr::AttrGet {
                    base: Box::new(HirExpr::Name("other".to_string())),
                    attr: name.clone(),
                }),
            },
            body: vec![HirStmt::Return(Some(HirExpr::BoolLiteral(false)))],
            orelse: Vec::new(),
        });
    }
    // `return True`
    body.push(HirStmt::Return(Some(HirExpr::BoolLiteral(true))));
    HirItem::Function {
        name: format!("{class_name}.__eq__"),
        params,
        return_ty: Ty::Bool,
        body,
    }
}

/// #378 (PR-18): Synthesizes a `__repr__` method for a `@dataclass` class
/// from its (merged) field list. The synthesized method takes `self` and
/// returns a `str` of the form `ClassName(field1=..., field2=..., ...)`.
/// Each field value is converted to a string via f-string interpolation
/// (which routes through the existing `to_str` codegen for scalars). The
/// string is built by concatenating literal and interpolated parts using
/// `pycc_rt_str_concat` at codegen time (the f-string codegen already
/// does this).
///
/// For a zero-field dataclass, `__repr__` returns `"ClassName()"`.
fn synthesize_dataclass_repr(class_name: &str, fields: &[(String, Ty)]) -> HirItem {
    let self_ty = Ty::Instance(Box::new(class_name.to_string()));
    let params: Vec<(String, Ty)> = vec![("self".to_string(), self_ty)];
    // Build the repr string as an f-string with literal and interpolation
    // parts. The codegen's f-string handling already converts each
    // interpolated value to a string via `to_str` and concatenates with
    // `pycc_rt_str_concat`.
    let body = if fields.is_empty() {
        vec![HirStmt::Return(Some(HirExpr::StringLiteral(format!(
            "{class_name}()"
        ))))]
    } else {
        let mut parts: Vec<crate::FStringPart> = Vec::new();
        parts.push(crate::FStringPart::Literal(format!("{class_name}(")));
        for (i, (name, _)) in fields.iter().enumerate() {
            if i > 0 {
                parts.push(crate::FStringPart::Literal(", ".to_string()));
            }
            parts.push(crate::FStringPart::Literal(format!("{name}=")));
            parts.push(crate::FStringPart::Interpolation(Box::new(
                HirExpr::AttrGet {
                    base: Box::new(HirExpr::Name("self".to_string())),
                    attr: name.clone(),
                },
            )));
        }
        parts.push(crate::FStringPart::Literal(")".to_string()));
        vec![HirStmt::Return(Some(HirExpr::FString(parts)))]
    };
    HirItem::Function {
        name: format!("{class_name}.__repr__"),
        params,
        return_ty: Ty::Str,
        body,
    }
}

/// Scans `__init__`'s own top-level body statements (no recursion into a
/// nested `if`/`while`/`for` -- see this module's own doc comment) for
/// `self.<attr> = <value>` assignments, building the attribute-slot list in
/// first-assignment source order. Only the *first* assignment to a given
/// attribute name establishes its slot and `Ty`; a later `self.<attr> =
/// ...` reassignment further down `__init__`'s own body is structurally
/// ignored here (it is still lowered normally by `stmt::lower_body` into an
/// ordinary `HirStmt::AttrSet`, and `pycc_types` checks its value against
/// the already-established slot type -- this pre-scan's only job is
/// deciding *which* attributes exist and their *first-assignment* type).
///
/// `params` is `lower_method`'s own full parameter list (including `self`
/// as its first entry) -- used to resolve a bare-parameter-name RHS's `Ty`.
fn collect_init_attrs(
    init_body: &[Stmt],
    params: &[(String, Ty)],
) -> Result<Vec<(String, Ty)>, Diagnostic> {
    let mut attrs: Vec<(String, Ty)> = Vec::new();
    for stmt in init_body {
        let Stmt::Assign(assign) = stmt else {
            continue;
        };
        // Not a `let [target] = .. else { continue }` guard: `init_body` is
        // only ever reached here once `stmt::lower_body` has already
        // lowered this exact body successfully (`lower_class` calls
        // `collect_init_attrs` after, never before,
        // `lower_method`'s own `stmt::lower_body(&def.body, ..)?` call --
        // see `lower_method`'s own doc comment) -- and that pass's own
        // `Stmt::Assign` handling (`crate::stmt::lower_stmt`) already
        // rejects a multi-target assignment (`self.x = self.y = 0`) with
        // `C0001` before this pre-scan ever runs. `.expect()`, not a
        // hand-rolled `continue`, per this crate's own established
        // coverage-gate convention for a provably-unreachable shape (see
        // `lower_type_alias_stmt`'s own `.expect(...)` precedent in
        // `lib.rs`): the panic path lives in libcore, outside this crate's
        // instrumented regions, unlike a `continue` here, which real
        // parsed source can never reach and which D-014's 100%-region gate
        // would otherwise demand a test for.
        let target = assign.targets.first().expect(
            "stmt::lower_body already rejected a multi-target assignment with C0001 \
             before this pre-scan runs",
        );
        let Expr::Attribute(attr) = target else {
            continue;
        };
        let Expr::Name(receiver) = attr.value.as_ref() else {
            continue;
        };
        if receiver.id.as_str() != "self" {
            continue;
        }
        let attr_name = attr.attr.to_string();
        if attrs.iter().any(|(name, _)| *name == attr_name) {
            continue;
        }
        let ty = slot_ty_from_init_rhs(&assign.value, params)?;
        attrs.push((attr_name, ty));
    }
    Ok(attrs)
}

/// #435 (Part B), extended by #585: Validates that an `__init_subclass__`
/// method body is statically evaluable — only a `pass` body, an empty body,
/// or a body consisting solely of a docstring (a bare string-literal
/// expression statement) is accepted. Any other statement (a `print` call,
/// an assignment, a return, etc.) is rejected with `C0001`, since pycc's
/// compile-time class-creation model has no mechanism to run side-effecting
/// statements at class-definition time.
///
/// `inherited` selects the diagnostic wording: `false` for a class's own
/// `__init_subclass__` (rejected at its own definition, per #435), `true`
/// for a hook inherited unchanged from a base class (rejected at the
/// *subclass's* creation site instead, per #585 — CPython invokes the
/// inherited hook there, so that is the point pycc's compile-time model
/// cannot honor).
fn validate_init_subclass_body<R>(body: &[Stmt], range: R, inherited: bool) -> Result<(), Diagnostic>
where
    std::ops::Range<u32>: From<R>,
{
    let message = if inherited {
        "a base class's `__init_subclass__`, inherited unchanged by this subclass, must be \
         statically evaluable (only `pass` or a docstring is supported in this version) — \
         CPython invokes the inherited hook when this subclass is created, and pycc cannot run \
         its side-effecting statements at that point yet"
    } else {
        "`__init_subclass__` must be statically evaluable (only `pass` \
         or a docstring is supported in this version) — \
         side-effecting statements are not supported yet"
    };
    for stmt in body {
        match stmt {
            Stmt::Pass(_) => continue,
            // A docstring: a bare string-literal expression statement. This
            // is the only expression statement accepted in
            // `__init_subclass__` — it has no side effects.
            Stmt::Expr(expr_stmt) => {
                if matches!(*expr_stmt.value, Expr::StringLiteral(_)) {
                    continue;
                }
                return Err(unsupported(message, range));
            }
            _ => {
                return Err(unsupported(message, range));
            }
        }
    }
    Ok(())
}

/// Resolves an instance attribute's slot `Ty` from its first-assignment RHS
/// inside `__init__`, structurally -- see this module's own doc comment and
/// `lower_method`'s doc comment for why this must not require a real
/// type-inference pass: only a bare reference to one of `__init__`'s own
/// (always-annotated) parameters, or a scalar literal, is accepted. Every
/// other RHS shape -- including an arithmetic expression, a call, or a
/// reference to `self` itself -- is `C0001`, matching the plan's own
/// explicit authorization ("any class-body statement kind other than a
/// `def` or a `self.<attr> = ...` inside `__init__` is `C0001` for this
/// PR").
fn slot_ty_from_init_rhs(value: &Expr, params: &[(String, Ty)]) -> Result<Ty, Diagnostic> {
    match value {
        // Two guarded arms of the same top-level `match`, deliberately
        // *asymmetric* rather than two structurally identical `matches!`
        // checks (`Number::Int(_)` / `Number::Float(_)`): every symmetric
        // shape tried for this Int/Float split -- two standalone `if let
        // ... && matches!(..)` chains, a nested `match &lit.value { .. }`
        // behind one outer `if let`, an `Option`-valued intermediate
        // `match`, a single outer `if let` wrapping two independent bare
        // `if matches!(..)` checks, and even two guarded arms of this same
        // trailing `match` when both guards called `matches!` against a
        // distinct `Number` variant -- reported the *second* of the two as
        // an uncovered region under `cargo llvm-cov`, regardless of which
        // variant it checked or what control-flow shape wrapped it, even
        // though it demonstrably executes (both
        // `an_init_attr_assigned_an_int_literal_establishes_an_int_slot`
        // and `an_init_attr_assigned_a_float_literal_establishes_a_float_slot`
        // below pass, each asserting the exact `Ty` this arm resolves to).
        // The common factor was always two source-adjacent regions with
        // byte-for-byte identical `matches!(lit.value, Number::<Variant>(_))`
        // shapes differing only in the variant name -- consistent with an
        // LLVM coverage-mapping counter getting deduplicated/shared across
        // two structurally-identical-looking regions, so only the first is
        // ever marked hit. Writing the second guard as a negation of the
        // first (`!matches!(.., Number::Int(_))`, rather than its own
        // positive `matches!(.., Number::Float(_))`) breaks that structural
        // symmetry and resolves it -- confirmed clean at 100% region
        // coverage for this file with this exact shape, after every
        // symmetric alternative above reproduced the identical artifact.
        Expr::NumberLiteral(lit) if matches!(lit.value, Number::Int(_)) => Ok(Ty::Int),
        Expr::NumberLiteral(lit) if !matches!(lit.value, Number::Int(_)) => Ok(Ty::Float),
        // The second arm's negation also correctly subsumes
        // `Number::Complex` (`1j`), which can never actually reach this
        // function: `expr::lower_expr`'s own `NumberLiteral` arm has no
        // case for `Number::Complex`, so `stmt::lower_body` (called before
        // this pre-scan ever runs, see `lower_class`) always rejects
        // `self.x = 1j` with `C0001` first, confirmed directly by running
        // this exact snippet through `lower_checked` rather than assumed.
        // A provably-unreachable `Number::Complex` value being classified
        // as `Ty::Float` by the negation above is therefore never
        // observable from any real parsed source.
        Expr::Name(name) => {
            let resolved = params
                .iter()
                .find(|(param_name, _)| param_name == name.id.as_str())
                .map(|(_, ty)| ty.clone());
            match resolved {
                // Only a scalar-typed parameter (int/float/bool/str) may
                // seed an attribute slot -- `pycc_rt::instance`'s slot
                // storage is a single `i64` word per slot (D-154's own
                // class-instance-layout ADR), which has no representation
                // for a heap-object-typed attribute (`list[T]`, `dict[K,
                // V]`, `set[T]`) or a by-value `tuple[...]` yet, and a
                // self-referential `Ty::Instance` attribute (`self.other =
                // some_other_instance_param`) is likewise out of this PR's
                // scope (no class currently has more than one instance
                // reachable this way to exercise it against). Rejecting
                // here, structurally, keeps every attribute type this PR's
                // own `pycc_codegen`/`pycc_rt` slices actually implement.
                //
                // PEP 695 (#387): `Ty::Param` is also accepted — a generic
                // class's `__init__` parameter typed `T` seeds a slot with
                // `Ty::Param("T")`, which is substituted with a concrete
                // scalar type at monomorphization time (reusing PR-13's
                // D-133/D-134 call-site-substitution mechanism). At runtime
                // the slot is still a single `i64` word, so the type
                // parameter is purely compile-time.
                Some(ty @ (Ty::Int | Ty::Float | Ty::Bool | Ty::Str | Ty::Param(_))) => Ok(ty),
                Some(other) => Err(unsupported(
                    format!(
                        "`self.<attr> = {}` cannot establish an attribute of type `{}` yet \
                         -- only a scalar (int/float/bool/str) parameter is supported",
                        name.id,
                        other.name()
                    ),
                    pycc_ast::expr_range(value),
                )),
                None => Err(unsupported(
                    format!(
                        "`self.<attr> = {}` must reference one of `__init__`'s own \
                         parameters to establish the attribute's type, or use a scalar \
                         literal",
                        name.id
                    ),
                    pycc_ast::expr_range(value),
                )),
            }
        }
        Expr::BooleanLiteral(_) => Ok(Ty::Bool),
        Expr::StringLiteral(_) => Ok(Ty::Str),
        other => Err(unsupported(
            "an instance attribute's first assignment inside `__init__` must be a bare \
             parameter name or a scalar literal (int/float/bool/str) so its type is known \
             at compile time",
            pycc_ast::expr_range(other),
        )),
    }
}

#[cfg(test)]
mod tests {
    use crate::{HirClassDef, HirExpr, HirItem, HirStmt, Ty, lower_checked};

    fn assert_c0001(source: &str) {
        let module = crate::pycc_parser_test_helper::parse(source);
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "C0001", "source: {source:?}");
    }

    pub(super) fn lower_ok(source: &str) -> crate::HirModule {
        // `.expect(...)`, not `.unwrap_or_else(|e| panic!(...))`: the
        // latter's closure body is its own hand-written region, never
        // executed on this helper's own happy path (every call site
        // expects success) -- this crate's own established coverage-gate
        // convention (see `slot_ty_from_init_rhs`'s own doc comment,
        // immediately above in this file) is `.expect()`, whose panic path
        // lives in libcore, outside this crate's instrumented regions.
        let module = crate::pycc_parser_test_helper::parse(source);
        lower_checked(&module).expect("test fixture should lower successfully")
    }

    // -- lower_class: class-level shape checks -----------------------------

    #[test]
    fn a_decorated_class_is_unsupported() {
        assert_c0001(
            "@some_decorator\nclass C:\n    def __init__(self) -> None:\n        return\n",
        );
    }

    #[test]
    fn a_generic_class_with_one_type_param_is_supported() {
        // PEP 695 (#387): `class C[T]:` with exactly one type parameter is
        // now supported. The type parameter `T` is recorded in
        // `HirClassDef::type_param` for later monomorphization.
        let hir =
            lower_ok("class C[T]:\n    def __init__(self, x: T) -> None:\n        self.x = x\n");
        assert_eq!(hir.class_defs.len(), 1);
        assert_eq!(hir.class_defs[0].1.type_param, Some("T".to_string()));
    }

    #[test]
    fn a_generic_class_with_two_type_params_is_unsupported() {
        assert_c0001("class C[T, U]:\n    def __init__(self) -> None:\n        return\n");
    }

    #[test]
    fn a_class_with_a_keyword_argument_is_unsupported() {
        assert_c0001("class C(metaclass=Meta):\n    def __init__(self) -> None:\n        return\n");
    }

    #[test]
    fn a_class_with_empty_parens_and_no_bases_is_supported() {
        // `class C():` is syntactically distinct from `class C:` (upstream
        // parses `arguments: Some(Arguments { args: [], keywords: [] })`)
        // but semantically equivalent -- no inheritance -- so it must not
        // be rejected merely because `arguments` is `Some(_)`.
        let hir = lower_ok("class C():\n    def __init__(self) -> None:\n        return\n");
        assert_eq!(hir.class_defs.len(), 1);
    }

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
    fn redefining_a_class_name_at_module_scope_is_unsupported() {
        // D-154 Part 1's own post-merge review finding: two module-level
        // classes sharing a name would each lower their own `__init__` to
        // the identical mangled `<Name>.__init__` function name, colliding
        // silently in `pycc_types`'/`pycc_mir`'s own `HashMap`-collected
        // class tables downstream rather than producing a clean diagnostic.
        // Mirrors `redefining_init_in_one_class_body_is_unsupported`
        // above, one level up (module scope rather than one class body).
        assert_c0001(
            "class C:\n    def __init__(self) -> None:\n        return\nclass C:\n    def __init__(self) -> None:\n        return\n",
        );
    }

    #[test]
    fn a_class_name_colliding_with_an_earlier_function_name_is_unsupported() {
        // D-068 review finding on #385: without this check, `class Foo`
        // below would silently, permanently shadow the earlier `def Foo()`
        // at every call site -- `pycc_types::Environment` checks
        // `env.lookup_class(callee)` before the ordinary function lookup
        // (`crates/pycc_types/src/lib.rs`), so `Foo()` would always resolve
        // to the class instantiation and the function would become
        // unreachable, with no diagnostic ever produced.
        let diagnostic = lower_checked(&crate::pycc_parser_test_helper::parse(
            "def Foo() -> None:\n    return\nclass Foo:\n    def __init__(self) -> None:\n        return\n",
        ))
        .unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic
                .message
                .contains("class `Foo` collides with a function of the same name"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn a_function_name_colliding_with_an_earlier_class_name_is_unsupported() {
        // The reverse order of
        // `a_class_name_colliding_with_an_earlier_function_name_is_unsupported`
        // above: the class comes first, the function second.
        let diagnostic = lower_checked(&crate::pycc_parser_test_helper::parse(
            "class Foo:\n    def __init__(self) -> None:\n        return\ndef Foo() -> None:\n    return\n",
        ))
        .unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic
                .message
                .contains("function `Foo` collides with a class of the same name"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn a_class_name_colliding_with_a_type_alias_is_unsupported() {
        let diagnostic = lower_checked(&crate::pycc_parser_test_helper::parse(
            "type Foo = int\nclass Foo:\n    def __init__(self) -> None:\n        return\n",
        ))
        .unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic
                .message
                .contains("class `Foo` collides with a type alias of the same name"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn a_class_name_colliding_with_a_module_import_is_unsupported() {
        let diagnostic = lower_checked(&crate::pycc_parser_test_helper::parse(
            "import math\nclass math:\n    def __init__(self) -> None:\n        return\n",
        ))
        .unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic
                .message
                .contains("class `math` collides with an import of the same name"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn a_class_name_colliding_with_a_symbol_import_is_unsupported() {
        // Exercises `import_local_name`'s other `ImportBinding` variant
        // (`Symbol`, from `from <module> import <name>`) -- the test above
        // only ever reaches the `Module` variant (`import math`), leaving
        // the `Symbol` arm of `import_local_name`'s own or-pattern
        // structurally unreachable under D-014's 100%-region coverage gate.
        let diagnostic = lower_checked(&crate::pycc_parser_test_helper::parse(
            "from math import sqrt\nclass sqrt:\n    def __init__(self) -> None:\n        return\n",
        ))
        .unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic
                .message
                .contains("class `sqrt` collides with an import of the same name"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn a_type_alias_colliding_with_an_earlier_class_name_is_unsupported() {
        // The reverse order of
        // `a_class_name_colliding_with_a_type_alias_is_unsupported` above:
        // the class comes first, the `type X = ...` alias second -- D-068
        // review finding on #385's second round: without this check, `type
        // Foo = int` below would silently establish a second, alias-shaped
        // `Foo` binding alongside the class, with no diagnostic.
        let diagnostic = lower_checked(&crate::pycc_parser_test_helper::parse(
            "class Foo:\n    def __init__(self) -> None:\n        return\ntype Foo = int\n",
        ))
        .unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic
                .message
                .contains("type alias `Foo` collides with a class of the same name"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn a_legacy_type_alias_colliding_with_an_earlier_class_name_is_unsupported() {
        // Same reverse-direction collision as
        // `a_type_alias_colliding_with_an_earlier_class_name_is_unsupported`
        // above, exercised through the legacy `X: TypeAlias = <expr>`
        // spelling (`lower_legacy_type_alias_ann_assign`) instead of `type X
        // = <expr>` (`lower_type_alias_stmt`) -- the two are lowered by
        // independent functions in `import.rs`, each needing its own check and
        // its own regression test.
        let diagnostic = lower_checked(&crate::pycc_parser_test_helper::parse(
            "class Foo:\n    def __init__(self) -> None:\n        return\nFoo: TypeAlias = int\n",
        ))
        .unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic
                .message
                .contains("type alias `Foo` collides with a class of the same name"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn a_module_import_colliding_with_an_earlier_class_name_is_unsupported() {
        // The reverse order of
        // `a_class_name_colliding_with_a_module_import_is_unsupported`
        // above: the class comes first, `import math` second.
        let diagnostic = lower_checked(&crate::pycc_parser_test_helper::parse(
            "class math:\n    def __init__(self) -> None:\n        return\nimport math\n",
        ))
        .unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic
                .message
                .contains("import `math` collides with a class of the same name"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn a_symbol_import_colliding_with_an_earlier_class_name_is_unsupported() {
        // The reverse order of
        // `a_class_name_colliding_with_a_symbol_import_is_unsupported`
        // above: the class comes first, `from math import sqrt` second.
        // Also exercises `bound.iter().map(import_local_name).find(..)`'s
        // own multi-binding search (`from math import pi, sqrt` binds two
        // names in one statement) finding the colliding name when it is not
        // the first one bound.
        let diagnostic = lower_checked(&crate::pycc_parser_test_helper::parse(
            "class sqrt:\n    def __init__(self) -> None:\n        return\nfrom math import pi, sqrt\n",
        ))
        .unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic
                .message
                .contains("import `sqrt` collides with a class of the same name"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn a_class_without_init_is_unsupported() {
        assert_c0001("class C:\n    def foo(self) -> None:\n        return\n");
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
        // actually a `Buf` instance) and fail with the confusing "dict.get()
        // takes exactly two arguments (key, default), got 1" message
        // instead of ever reaching `Buf`'s own `get` method. Asserting the
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
            !diagnostic
                .message
                .contains("dict.get() takes exactly two arguments"),
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

    // -- lower_method: method-shape checks ----------------------------------

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
    fn a_positional_only_method_parameter_lowers_successfully() {
        // PEP 570 (#383): positional-only parameters on methods are now
        // lowered. `self` remains the first parameter; posonlyargs follow
        // it, before ordinary `args`. The class also has a regular method
        // `foo` to exercise the `find_map` None branch (items before the
        // target function that don't match).
        let module = crate::pycc_parser_test_helper::parse(
            "class C:\n    def foo(self) -> None:\n        return\n    def __init__(self, x: int, /, y: int) -> None:\n        return\n",
        );
        let hir = lower_checked(&module).unwrap();
        // Find the __init__ function item and verify its params.
        let init = hir
            .items
            .iter()
            .filter_map(|item| match item {
                HirItem::Function { name, params, .. } if name == "C.__init__" => {
                    Some(params.clone())
                }
                _ => None,
            })
            .next()
            .expect("C.__init__ must be lowered");
        assert_eq!(
            init,
            vec![
                ("self".to_string(), Ty::Instance(Box::new("C".to_string()))),
                ("x".to_string(), Ty::Int),
                ("y".to_string(), Ty::Int),
            ]
        );
    }

    #[test]
    fn a_positional_only_classmethod_parameter_lowers_successfully() {
        // PEP 570 (#383): positional-only parameters on `@classmethod` are
        // now lowered. `cls` remains the first parameter; posonlyargs follow
        // it, before ordinary `args`. The mangled name uses a `.classmethod`
        // suffix (#436).
        let module = crate::pycc_parser_test_helper::parse(
            "class C:\n    def __init__(self) -> None:\n        return\n    @classmethod\n    def m(cls, x: int, /, y: int) -> None:\n        return\n",
        );
        let hir = lower_checked(&module).unwrap();
        let m = hir
            .items
            .iter()
            .filter_map(|item| match item {
                HirItem::Function { name, params, .. } if name == "C.m.classmethod" => {
                    Some(params.clone())
                }
                _ => None,
            })
            .next()
            .expect("C.m.classmethod must be lowered");
        assert_eq!(
            m,
            vec![
                ("cls".to_string(), Ty::Instance(Box::new("C".to_string()))),
                ("x".to_string(), Ty::Int),
                ("y".to_string(), Ty::Int),
            ]
        );
    }

    #[test]
    fn a_positional_only_staticmethod_parameter_lowers_successfully() {
        // PEP 570 (#383): positional-only parameters on `@staticmethod` are
        // now lowered. For static methods, posonlyargs come first (no
        // implicit `self`/`cls`), then ordinary `args`. The mangled name
        // uses a `.static` suffix (#436).
        let module = crate::pycc_parser_test_helper::parse(
            "class C:\n    def __init__(self) -> None:\n        return\n    @staticmethod\n    def m(x: int, /, y: int) -> None:\n        return\n",
        );
        let hir = lower_checked(&module).unwrap();
        let m = hir
            .items
            .iter()
            .filter_map(|item| match item {
                HirItem::Function { name, params, .. } if name == "C.m.static" => {
                    Some(params.clone())
                }
                _ => None,
            })
            .next()
            .expect("C.m.static must be lowered");
        assert_eq!(
            m,
            vec![("x".to_string(), Ty::Int), ("y".to_string(), Ty::Int),]
        );
    }

    #[test]
    fn a_positional_only_method_parameter_with_a_default_is_rejected() {
        // PEP 570 (#383): the `lower_arg_list` error path for posonlyargs
        // on methods (default values are unsupported) must fire.
        assert_c0001("class C:\n    def __init__(self, x: int = 0, /) -> None:\n        return\n");
    }

    #[test]
    fn a_positional_only_classmethod_parameter_with_a_default_is_rejected() {
        // PEP 570 (#383): the `lower_arg_list` error path for posonlyargs
        // on classmethods (default values are unsupported) must fire.
        assert_c0001(
            "class C:\n    def __init__(self) -> None:\n        return\n    @classmethod\n    def m(cls, x: int = 0, /) -> None:\n        return\n",
        );
    }

    #[test]
    fn a_positional_only_staticmethod_parameter_with_a_default_is_rejected() {
        // PEP 570 (#383): the `lower_arg_list` error path for posonlyargs
        // on static methods (default values are unsupported) must fire.
        assert_c0001(
            "class C:\n    def __init__(self) -> None:\n        return\n    @staticmethod\n    def m(x: int = 0, /) -> None:\n        return\n",
        );
    }

    #[test]
    fn a_vararg_method_parameter_is_unsupported() {
        assert_c0001("class C:\n    def __init__(self, *args) -> None:\n        return\n");
    }

    #[test]
    fn a_keyword_only_method_parameter_is_unsupported() {
        assert_c0001("class C:\n    def __init__(self, *, x: int) -> None:\n        return\n");
    }

    #[test]
    fn a_kwarg_method_parameter_is_unsupported() {
        assert_c0001("class C:\n    def __init__(self, **kwargs) -> None:\n        return\n");
    }

    #[test]
    fn a_method_with_no_parameters_at_all_is_unsupported() {
        assert_c0001("class C:\n    def __init__() -> None:\n        return\n");
    }

    #[test]
    fn a_method_whose_first_parameter_is_not_named_self_is_unsupported() {
        assert_c0001("class C:\n    def __init__(this) -> None:\n        return\n");
    }

    #[test]
    fn a_self_parameter_with_a_default_value_is_unsupported() {
        assert_c0001("class C:\n    def __init__(self=None) -> None:\n        return\n");
    }

    #[test]
    fn an_annotated_self_parameter_is_unsupported() {
        assert_c0001("class C:\n    def __init__(self: C) -> None:\n        return\n");
    }

    #[test]
    fn an_init_parameter_without_an_annotation_is_unsupported() {
        // Unlike an ordinary private (`_`-prefixed) top-level function or
        // method, `__init__`'s own parameters always require an
        // annotation, regardless of D-038's usual public-name-only rule --
        // see `lower_method`'s own doc comment for why. This is a `T0001`
        // missing-annotation diagnostic (the same code an ordinary public
        // function's own missing annotation produces), not `C0001`.
        let module = crate::pycc_parser_test_helper::parse(
            "class C:\n    def __init__(self, x) -> None:\n        return\n",
        );
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "T0001");
    }

    #[test]
    fn a_private_method_parameter_without_an_annotation_is_still_permitted() {
        // Contrast with the test above: an ordinary (non-`__init__`)
        // private method still follows the plain D-038 rule (an unannotated
        // parameter is only rejected for a *public* name).
        let hir = lower_ok(
            "class C:\n    def __init__(self, x: int) -> None:\n        self.x = x\n    def _helper(self, y) -> None:\n        return\n",
        );
        assert_eq!(hir.class_defs.len(), 1);
    }

    #[test]
    fn a_method_with_an_unsupported_return_annotation_propagates_the_error() {
        // Exercises `lower_method`'s own `?` on
        // `crate::lower_return_annotation`'s error path -- distinct from
        // every other method-shape test above, which only ever exercise a
        // parameter-side rejection.
        assert_c0001(
            "class C:\n    def __init__(self) -> None:\n        return\n    def bump(self) -> Frobnicate:\n        return\n",
        );
    }

    #[test]
    fn a_method_signature_and_self_ty_lower_correctly() {
        let hir = lower_ok(
            "class Point:\n    def __init__(self, x: int, y: int) -> None:\n        self.x = x\n        self.y = y\n",
        );
        assert_eq!(hir.class_defs.len(), 1);
        let (name, class_def) = &hir.class_defs[0];
        assert_eq!(name, "Point");
        assert_eq!(
            *class_def,
            HirClassDef {
                exception_type_tag: None,
                name: "Point".to_string(),
                bases: Vec::new(),
                mro: vec!["Point".to_string()],
                attrs: vec![("x".to_string(), Ty::Int), ("y".to_string(), Ty::Int),],
                methods: vec![("__init__".to_string(), "Point.__init__".to_string())],
                properties: Vec::new(),
                type_param: None,
                static_methods: Vec::new(),
                class_methods: Vec::new(),
                enum_members: Vec::new(),
                is_dataclass: false,
                dataclass_fields: Vec::new(),
                is_protocol: false,
                runtime_checkable: false,
                protocol_members: Vec::new(),
                abstract_methods: Vec::new(),
                is_abstract: false,
            }
        );
        // Direct value comparison, not a `let PATTERN = .. else { panic!(..) }`
        // destructure -- this crate's own established coverage-gate
        // convention (see `pycc_hir::lib.rs`'s
        // `re_exported_grammar_types_resolve_and_have_the_expected_shape`-
        // adjacent precedent): a hand-written panic arm never taken on the
        // happy path is a permanently uncovered region under D-014's
        // 100%-region gate.
        assert_eq!(
            hir.items[0],
            HirItem::Function {
                name: "Point.__init__".to_string(),
                params: vec![
                    (
                        "self".to_string(),
                        Ty::Instance(Box::new("Point".to_string()))
                    ),
                    ("x".to_string(), Ty::Int),
                    ("y".to_string(), Ty::Int),
                ],
                return_ty: Ty::None,
                body: vec![
                    HirStmt::AttrSet {
                        base: HirExpr::Name("self".to_string()),
                        attr: "x".to_string(),
                        value: HirExpr::Name("x".to_string()),
                    },
                    HirStmt::AttrSet {
                        base: HirExpr::Name("self".to_string()),
                        attr: "y".to_string(),
                        value: HirExpr::Name("y".to_string()),
                    },
                ],
            }
        );
    }

    // -- @property lowering (#377) ------------------------------------------

    #[test]
    fn a_property_getter_lowers_into_the_property_table() {
        let hir = lower_ok(
            "class C:\n    def __init__(self) -> None:\n        self._x = 0\n    @property\n    def x(self) -> int:\n        return self._x\n",
        );
        let (_, class_def) = &hir.class_defs[0];
        assert_eq!(class_def.properties.len(), 1);
        assert_eq!(
            class_def.properties[0],
            crate::PropertyDef {
                name: "x".to_string(),
                getter: "C.x".to_string(),
                setter: None,
            }
        );
        // The getter is NOT in the methods table (accessed via attribute
        // syntax, not method-call syntax).
        assert!(!class_def.methods.iter().any(|(name, _)| name == "x"));
    }

    #[test]
    fn a_property_getter_and_setter_lower_into_one_property_entry() {
        let hir = lower_ok(
            "class C:\n    def __init__(self) -> None:\n        self._x = 0\n    @property\n    def x(self) -> int:\n        return self._x\n    @x.setter\n    def x(self, v: int) -> None:\n        self._x = v\n",
        );
        let (_, class_def) = &hir.class_defs[0];
        assert_eq!(class_def.properties.len(), 1);
        assert_eq!(
            class_def.properties[0],
            crate::PropertyDef {
                name: "x".to_string(),
                getter: "C.x".to_string(),
                setter: Some("C.x.setter".to_string()),
            }
        );
    }

    #[test]
    fn a_property_getter_function_is_emitted_with_the_mangled_name() {
        let hir = lower_ok(
            "class C:\n    def __init__(self) -> None:\n        self._x = 0\n    @property\n    def x(self) -> int:\n        return self._x\n",
        );
        assert!(
            hir.items
                .iter()
                .any(|item| matches!(item, HirItem::Function { name, .. } if name == "C.x")),
            "getter function `C.x` should be in items"
        );
    }

    #[test]
    fn a_property_setter_function_is_emitted_with_the_setter_mangled_name() {
        let hir = lower_ok(
            "class C:\n    def __init__(self) -> None:\n        self._x = 0\n    @property\n    def x(self) -> int:\n        return self._x\n    @x.setter\n    def x(self, v: int) -> None:\n        self._x = v\n",
        );
        assert!(
            hir.items
                .iter()
                .any(|item| matches!(item, HirItem::Function { name, .. } if name == "C.x.setter")),
            "setter function `C.x.setter` should be in items"
        );
    }

    #[test]
    fn an_unrecognized_method_decorator_is_still_rejected() {
        assert_c0001(
            "class C:\n    def __init__(self) -> None:\n        return\n    @foo\n    def f(self) -> None:\n        return\n",
        );
    }

    #[test]
    fn multiple_decorators_on_a_method_are_rejected() {
        assert_c0001(
            "class C:\n    def __init__(self) -> None:\n        return\n    @property\n    @foo\n    def f(self) -> None:\n        return\n",
        );
    }

    #[test]
    fn a_property_getter_with_extra_parameters_is_rejected() {
        assert_c0001(
            "class C:\n    def __init__(self) -> None:\n        return\n    @property\n    def x(self, extra: int) -> int:\n        return 0\n",
        );
    }

    #[test]
    fn a_property_setter_with_no_value_parameter_is_rejected() {
        assert_c0001(
            "class C:\n    def __init__(self) -> None:\n        return\n    @property\n    def x(self) -> int:\n        return 0\n    @x.setter\n    def x(self) -> None:\n        return\n",
        );
    }

    #[test]
    fn a_property_setter_with_two_value_parameters_is_rejected() {
        assert_c0001(
            "class C:\n    def __init__(self) -> None:\n        return\n    @property\n    def x(self) -> int:\n        return 0\n    @x.setter\n    def x(self, a: int, b: int) -> None:\n        return\n",
        );
    }

    #[test]
    fn a_setter_without_a_preceding_getter_is_rejected() {
        let module = crate::pycc_parser_test_helper::parse(
            "class C:\n    def __init__(self) -> None:\n        return\n    @x.setter\n    def x(self, v: int) -> None:\n        return\n",
        );
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic
                .message
                .contains("requires a preceding `@property` getter"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn a_duplicate_property_getter_is_rejected() {
        let module = crate::pycc_parser_test_helper::parse(
            "class C:\n    def __init__(self) -> None:\n        return\n    @property\n    def x(self) -> int:\n        return 0\n    @property\n    def x(self) -> int:\n        return 1\n",
        );
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic.message.contains("already defined"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn a_duplicate_property_setter_is_rejected() {
        let module = crate::pycc_parser_test_helper::parse(
            "class C:\n    def __init__(self) -> None:\n        return\n    @property\n    def x(self) -> int:\n        return 0\n    @x.setter\n    def x(self, v: int) -> None:\n        return\n    @x.setter\n    def x(self, v: int) -> None:\n        return\n",
        );
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic.message.contains("already defined"),
            "unexpected message: {}",
            diagnostic.message
        );
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
    fn a_setter_decorator_name_not_matching_the_method_name_is_rejected() {
        let module = crate::pycc_parser_test_helper::parse(
            "class C:\n    def __init__(self) -> None:\n        return\n    @property\n    def x(self) -> int:\n        return 0\n    @x.setter\n    def y(self, v: int) -> None:\n        return\n",
        );
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic
                .message
                .contains("must decorate a method named `x`"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn a_non_setter_attribute_decorator_is_rejected() {
        assert_c0001(
            "class C:\n    def __init__(self) -> None:\n        return\n    @x.deleter\n    def x(self) -> None:\n        return\n",
        );
    }

    #[test]
    fn a_call_shaped_decorator_is_rejected() {
        assert_c0001(
            "class C:\n    def __init__(self) -> None:\n        return\n    @property()\n    def x(self) -> int:\n        return 0\n",
        );
    }

    #[test]
    fn a_setter_decorator_with_a_chained_attribute_base_is_rejected() {
        // `@a.b.setter` -- the decorator expression is an attribute access
        // (`(a.b).setter`) whose own base is *not* a bare `Expr::Name`
        // (it's `Expr::Attribute(Name("a"), "b")`). This exercises
        // `classify_decorator`'s `let Expr::Name(base_name) =
        // attr.value.as_ref() else { ... }` rejection arm -- distinct from
        // `a_non_setter_attribute_decorator_is_rejected` above, which
        // exercises the `attr.attr != "setter"` arm, and from
        // `a_setter_decorator_name_not_matching_the_method_name_is_rejected`,
        // which exercises the name-mismatch arm.
        assert_c0001(
            "class C:\n    def __init__(self) -> None:\n        return\n    @property\n    def x(self) -> int:\n        return 0\n    @a.b.setter\n    def x(self, v: int) -> None:\n        return\n",
        );
    }

    // -- #436: @staticmethod / @classmethod lowering ------------------------

    #[test]
    fn a_static_method_lowers_into_the_static_methods_table() {
        let hir = lower_ok(
            "class C:\n    def __init__(self) -> None:\n        return\n    @staticmethod\n    def create(x: int) -> int:\n        return x\n",
        );
        let (_, class_def) = &hir.class_defs[0];
        assert_eq!(
            class_def.static_methods,
            vec![("create".to_string(), "C.create.static".to_string())]
        );
        // The static method is NOT in the methods table.
        assert!(!class_def.methods.iter().any(|(name, _)| name == "create"));
    }

    #[test]
    fn a_class_method_lowers_into_the_class_methods_table() {
        let hir = lower_ok(
            "class C:\n    def __init__(self) -> None:\n        return\n    @classmethod\n    def greet(cls, x: int) -> int:\n        return x\n",
        );
        let (_, class_def) = &hir.class_defs[0];
        assert_eq!(
            class_def.class_methods,
            vec![("greet".to_string(), "C.greet.classmethod".to_string())]
        );
        // The class method is NOT in the methods table.
        assert!(!class_def.methods.iter().any(|(name, _)| name == "greet"));
    }

    #[test]
    fn a_static_method_function_is_emitted_with_the_static_mangled_name() {
        let hir = lower_ok(
            "class C:\n    def __init__(self) -> None:\n        return\n    @staticmethod\n    def create(x: int) -> int:\n        return x\n",
        );
        assert!(
            hir.items.iter().any(|item| matches!(
                item,
                HirItem::Function { name, .. } if name == "C.create.static"
            )),
            "static method function `C.create.static` should be in items"
        );
    }

    #[test]
    fn a_class_method_function_is_emitted_with_the_classmethod_mangled_name() {
        let hir = lower_ok(
            "class C:\n    def __init__(self) -> None:\n        return\n    @classmethod\n    def greet(cls, x: int) -> int:\n        return x\n",
        );
        assert!(
            hir.items.iter().any(|item| matches!(
                item,
                HirItem::Function { name, .. } if name == "C.greet.classmethod"
            )),
            "class method function `C.greet.classmethod` should be in items"
        );
    }

    #[test]
    fn a_static_method_has_no_implicit_self_parameter() {
        let hir = lower_ok(
            "class C:\n    def __init__(self) -> None:\n        return\n    @staticmethod\n    def create(x: int) -> int:\n        return x\n",
        );
        let fn_item = hir
            .items
            .iter()
            .find_map(|item| match item {
                HirItem::Function { name, params, .. } if name == "C.create.static" => Some(params),
                _ => None,
            })
            .expect("C.create.static should be in items");
        // No `self` — the parameter list is exactly what the user wrote.
        assert_eq!(*fn_item, vec![("x".to_string(), Ty::Int)]);
    }

    #[test]
    fn a_class_method_has_implicit_cls_typed_as_instance() {
        let hir = lower_ok(
            "class C:\n    def __init__(self) -> None:\n        return\n    @classmethod\n    def greet(cls, x: int) -> int:\n        return x\n",
        );
        let fn_item = hir
            .items
            .iter()
            .find_map(|item| match item {
                HirItem::Function { name, params, .. } if name == "C.greet.classmethod" => {
                    Some(params)
                }
                _ => None,
            })
            .expect("C.greet.classmethod should be in items");
        assert_eq!(
            *fn_item,
            vec![
                ("cls".to_string(), Ty::Instance(Box::new("C".to_string()))),
                ("x".to_string(), Ty::Int),
            ]
        );
    }

    #[test]
    fn staticmethod_on_init_is_rejected() {
        assert_c0001(
            "class C:\n    @staticmethod\n    def __init__(x: int) -> None:\n        return\n",
        );
    }

    #[test]
    fn classmethod_on_init_is_rejected() {
        assert_c0001(
            "class C:\n    @classmethod\n    def __init__(cls) -> None:\n        return\n",
        );
    }

    #[test]
    fn a_class_method_without_cls_is_rejected() {
        let module = crate::pycc_parser_test_helper::parse(
            "class C:\n    def __init__(self) -> None:\n        return\n    @classmethod\n    def f() -> int:\n        return 1\n",
        );
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic.message.contains("must take `cls`"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn a_class_method_with_a_non_cls_first_parameter_is_rejected() {
        let module = crate::pycc_parser_test_helper::parse(
            "class C:\n    def __init__(self) -> None:\n        return\n    @classmethod\n    def f(self, x: int) -> int:\n        return x\n",
        );
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic.message.contains("must be named `cls`"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn a_class_method_with_annotated_cls_is_rejected() {
        assert_c0001(
            "class C:\n    def __init__(self) -> None:\n        return\n    @classmethod\n    def f(cls: int, x: int) -> int:\n        return x\n",
        );
    }

    #[test]
    fn a_class_method_with_cls_default_is_rejected() {
        assert_c0001(
            "class C:\n    def __init__(self) -> None:\n        return\n    @classmethod\n    def f(cls = 1) -> int:\n        return 1\n",
        );
    }

    #[test]
    fn a_class_method_with_an_unsupported_parameter_annotation_is_rejected() {
        // Exercises the `?` error propagation on `lower_arg_list` for the
        // non-`cls` parameters of a `@classmethod` (distinct from the `cls`
        // validation checks above, which run before `lower_arg_list`).
        assert_c0001(
            "class C:\n    def __init__(self) -> None:\n        return\n    @classmethod\n    def f(cls, x: Frobnicate) -> int:\n        return 1\n",
        );
    }

    #[test]
    fn a_static_method_with_an_unsupported_parameter_annotation_is_rejected() {
        // Exercises the `?` error propagation on `lower_arg_list` for a
        // `@staticmethod`'s parameter list (which has no `self`/`cls`
        // preprocessing, so `lower_arg_list` is the only validation path).
        assert_c0001(
            "class C:\n    def __init__(self) -> None:\n        return\n    @staticmethod\n    def f(x: Frobnicate) -> int:\n        return 1\n",
        );
    }

    #[test]
    fn a_static_method_redefinition_rebinds() {
        let hir = lower_ok(
            "class C:\n    def __init__(self) -> None:\n        return\n    @staticmethod\n    def f(x: int) -> int:\n        return x\n    @staticmethod\n    def f(x: int) -> int:\n        return x + 1\n",
        );
        let (_, class_def) = &hir.class_defs[0];
        // Rebind: one entry, not two.
        assert_eq!(class_def.static_methods.len(), 1);
        assert_eq!(
            class_def.static_methods[0],
            ("f".to_string(), "C.f.static".to_string())
        );
    }

    #[test]
    fn a_class_method_redefinition_rebinds() {
        let hir = lower_ok(
            "class C:\n    def __init__(self) -> None:\n        return\n    @classmethod\n    def f(cls, x: int) -> int:\n        return x\n    @classmethod\n    def f(cls, x: int) -> int:\n        return x + 1\n",
        );
        let (_, class_def) = &hir.class_defs[0];
        assert_eq!(class_def.class_methods.len(), 1);
        assert_eq!(
            class_def.class_methods[0],
            ("f".to_string(), "C.f.classmethod".to_string())
        );
    }

    // -- collect_init_attrs / slot_ty_from_init_rhs -------------------------

    #[test]
    fn an_init_attr_assigned_from_an_unrelated_name_is_unsupported() {
        assert_c0001("class C:\n    def __init__(self, x: int) -> None:\n        self.y = z\n");
    }

    #[test]
    fn an_init_attr_assigned_from_self_is_unsupported() {
        // `pycc_rt::instance`'s slot storage (D-154's own class-instance-
        // layout ADR) is a single `i64` word per slot -- a heap-object-typed
        // attribute other than `str` (another class instance, or a
        // `list[T]`/`dict[K, V]`/`set[T]` value) has no representation this
        // PR's `pycc_codegen`/`pycc_rt` slices implement. `self` is the one
        // reachable way to produce a non-scalar-typed *parameter* under
        // `slot_ty_from_init_rhs`'s own lookup today: `annotation_to_ty` has
        // no arm for a subscripted annotation like `list[int]` at all (any
        // such parameter fails to lower with `C0001` before this pre-scan
        // ever runs -- confirmed directly, not assumed), so `self` (typed
        // `Ty::Instance` directly by `lower_method`, bypassing
        // `annotation_to_ty` entirely) is the only non-scalar entry
        // `params` can ever actually contain.
        assert_c0001("class C:\n    def __init__(self) -> None:\n        self.link = self\n");
    }

    #[test]
    fn an_init_attr_assigned_an_int_literal_establishes_an_int_slot() {
        let hir = lower_ok("class C:\n    def __init__(self) -> None:\n        self.x = 5\n");
        assert_eq!(hir.class_defs[0].1.attrs, vec![("x".to_string(), Ty::Int)]);
    }

    #[test]
    fn an_init_attr_assigned_a_float_literal_establishes_a_float_slot() {
        let hir = lower_ok("class C:\n    def __init__(self) -> None:\n        self.x = 1.5\n");
        assert_eq!(
            hir.class_defs[0].1.attrs,
            vec![("x".to_string(), Ty::Float)]
        );
    }

    #[test]
    fn an_init_attr_assigned_a_complex_literal_is_unsupported() {
        // `1j` fails to lower long before `collect_init_attrs`'s own
        // pre-scan ever runs -- see `slot_ty_from_init_rhs`'s own comment
        // on its guarded `NumberLiteral` arms for why.
        assert_c0001("class C:\n    def __init__(self) -> None:\n        self.x = 1j\n");
    }

    #[test]
    fn an_init_attr_assigned_a_bool_literal_establishes_a_bool_slot() {
        let hir = lower_ok("class C:\n    def __init__(self) -> None:\n        self.x = True\n");
        assert_eq!(hir.class_defs[0].1.attrs, vec![("x".to_string(), Ty::Bool)]);
    }

    #[test]
    fn an_init_attr_assigned_a_string_literal_establishes_a_str_slot() {
        let hir = lower_ok("class C:\n    def __init__(self) -> None:\n        self.x = \"hi\"\n");
        assert_eq!(hir.class_defs[0].1.attrs, vec![("x".to_string(), Ty::Str)]);
    }

    #[test]
    fn an_init_attr_assigned_an_arithmetic_expression_is_unsupported() {
        assert_c0001("class C:\n    def __init__(self, x: int) -> None:\n        self.y = x + 1\n");
    }

    #[test]
    fn a_second_assignment_to_the_same_init_attr_does_not_change_its_slot_type() {
        // The pre-scan only records the *first* assignment to a given
        // attribute name; a later `self.x = ...` inside `__init__` itself
        // is still lowered normally (as a second `HirStmt::AttrSet`), but
        // does not add a second slot or change the recorded type.
        let hir = lower_ok(
            "class C:\n    def __init__(self, x: int) -> None:\n        self.x = x\n        self.x = 0\n",
        );
        assert_eq!(hir.class_defs[0].1.attrs, vec![("x".to_string(), Ty::Int)]);
    }

    #[test]
    fn non_attribute_statements_inside_init_are_ignored_by_the_pre_scan() {
        // Exercises every early-`continue` guard in `collect_init_attrs`
        // that a non-`self.<attr> = <value>` statement can reach without
        // itself being rejected by the rest of the pipeline: a plain local
        // assignment (not an `Expr::Attribute` target) and an attribute
        // assignment on a receiver other than `self` are both simply
        // skipped by the pre-scan -- none of them contributes an attribute
        // slot, and none of them is rejected by this pass (later lowering
        // of the method body may still reject some of them for other
        // reasons; this pre-scan's own job is only to skip them, not judge
        // them).
        let hir = lower_ok(
            "class C:\n    def __init__(self, x: int) -> None:\n        y = 1\n        other.z = 1\n        self.x = x\n",
        );
        assert_eq!(hir.class_defs[0].1.attrs, vec![("x".to_string(), Ty::Int)]);
    }

    #[test]
    fn a_multi_target_assignment_inside_init_is_rejected_before_the_pre_scan_ever_runs() {
        // `self.x = self.y = 0` parses to a single `Stmt::Assign` with two
        // targets (`[Attribute(self.x), Attribute(self.y)]`).
        // `pycc_hir::stmt::lower_stmt` (D-154's own `Assign` handling is
        // unchanged there) rejects a multi-target assignment with `C0001`
        // ("only a single assignment target is supported so far") during
        // `stmt::lower_body`, which `lower_method` always calls -- and
        // requires to succeed -- before `collect_init_attrs`'s own
        // pre-scan ever runs (see that function's own doc comment for why
        // its `assign.targets.first().expect(...)` is therefore safe, not
        // a `continue`-guarded shape this pre-scan needs to skip itself).
        assert_c0001("class C:\n    def __init__(self) -> None:\n        self.x = self.y = 0\n");
    }

    #[test]
    fn an_attribute_assignment_on_a_nested_attribute_base_inside_init_is_ignored_by_the_pre_scan() {
        // `self.x.y = 0` -- the outer `Attribute`'s own `.value` is itself
        // an `Attribute` (`self.x`), not a bare `Expr::Name`, so the `let
        // Expr::Name(receiver) = attr.value.as_ref() else { continue }`
        // guard's own early-exit fires and this statement contributes no
        // attribute slot. Structurally this still lowers successfully at
        // the HIR level (attribute access/assignment is generic over any
        // base expression, D-154's own `HirExpr::AttrGet`/`HirStmt::AttrSet`
        // doc comments) -- `pycc_types` is what would reject `self.x.y = 0`
        // once `x` turns out not to be a declared attribute of any
        // instance type, which is out of this crate's own scope to assert
        // on here.
        let hir = lower_ok("class C:\n    def __init__(self) -> None:\n        self.x.y = 0\n");
        assert_eq!(hir.class_defs[0].1.attrs, Vec::<(String, Ty)>::new());
    }

    // PEP 673 (#387 Part 1): `Self` as a method return-type annotation
    // resolves to the class's own instance type at HIR-lowering time.
    #[test]
    fn self_return_annotation_resolves_to_class_instance() {
        let hir = lower_ok(
            "class C:\n    def __init__(self) -> None:\n        return\n    def clone(self) -> Self:\n        return self\n",
        );
        assert_eq!(hir.class_defs.len(), 1);
        // The `clone` method should be lowered as a function with return
        // type `Ty::Instance("C")`.
        let clone = hir.items.iter().find_map(|item| match item {
            HirItem::Function {
                name, return_ty, ..
            } if name == "C.clone" => Some(return_ty),
            _ => None,
        });
        assert_eq!(
            clone,
            Some(&Ty::Instance(Box::new("C".to_string()))),
            "Self return annotation should resolve to Ty::Instance(\"C\")"
        );
    }

    // PEP 673 (#387 Part 1): `Self` as a method parameter annotation also
    // resolves to the class's own instance type.
    #[test]
    fn self_param_annotation_resolves_to_class_instance() {
        let hir = lower_ok(
            "class C:\n    def __init__(self, value: int) -> None:\n        self.value = value\n    def merge(self, other: Self) -> None:\n        self.value = other.value\n",
        );
        // The `merge` method's `other` parameter should be typed
        // `Ty::Instance("C")`.
        let merge = hir.items.iter().find_map(|item| match item {
            HirItem::Function { name, params, .. } if name == "C.merge" => Some(params.clone()),
            _ => None,
        });
        let merge = merge.expect("C.merge should exist");
        // params[0] is `self`, params[1] is `other`
        assert_eq!(merge[1].1, Ty::Instance(Box::new("C".to_string())));
    }

    // PEP 649/749 (#387 Part 2): self-referential deferred annotations. A
    // class's method may use the class's own name as a parameter type
    // annotation, even though the class is not fully defined at that point
    // in source.
    #[test]
    fn class_name_in_own_method_annotation_resolves_to_instance() {
        let hir = lower_ok(
            "class Node:\n    def __init__(self, value: int) -> None:\n        self.value = value\n    def update(self, other: Node) -> None:\n        self.value = other.value\n",
        );
        let update = hir.items.iter().find_map(|item| match item {
            HirItem::Function { name, params, .. } if name == "Node.update" => Some(params.clone()),
            _ => None,
        });
        let update = update.expect("Node.update should exist");
        // params[1] is `other` with type `Ty::Instance("Node")`
        assert_eq!(update[1].1, Ty::Instance(Box::new("Node".to_string())));
    }

    // PEP 649/749 (#387 Part 2): the class name also works as a return
    // type annotation in its own methods.
    #[test]
    fn class_name_as_own_return_annotation_resolves_to_instance() {
        let hir = lower_ok(
            "class Builder:\n    def __init__(self) -> None:\n        self.count = 0\n    def clone(self) -> Builder:\n        return self\n",
        );
        let clone = hir.items.iter().find_map(|item| match item {
            HirItem::Function {
                name, return_ty, ..
            } if name == "Builder.clone" => Some(return_ty.clone()),
            _ => None,
        });
        assert_eq!(clone, Some(Ty::Instance(Box::new("Builder".to_string()))),);
    }

    // PEP 649/749 (#387 Part 2, Bug 4 fix): a local `AnnAssign` *inside* a
    // method body (e.g. `other: Node = self`) must also resolve the class
    // name to `Ty::Instance`. Before the fix, `lower_body` was called with
    // `class_name=None` from `lower_method`, so the class name was
    // unresolvable in statement-body annotations (C0001).
    #[test]
    fn class_name_in_method_body_local_annotation_resolves_to_instance() {
        let hir = lower_ok(
            "class Node:\n    def __init__(self) -> None:\n        self.x = 0\n    def next(self) -> Node:\n        other: Node = self\n        return other\n",
        );
        let next = hir.items.iter().find_map(|item| match item {
            HirItem::Function { name, body, .. } if name == "Node.next" => Some(body.clone()),
            _ => None,
        });
        let next = next.expect("Node.next should exist");
        // body[0] is the `other: Node = self` AnnAssign. Use `matches!` with
        // a guard (same pattern as `check_and_resolve_monomorphizes_a_generic_
        // class_with_self_typed_method` in pycc_types) to avoid an uncovered
        // `panic!`/`unreachable!` arm under the 100%-region coverage gate.
        assert!(matches!(
            &next[0],
            HirStmt::AnnAssign { annotation, .. }
            if annotation == &Ty::Instance(Box::new("Node".to_string()))
        ));
    }

    // PEP 695 (#387 Part 3): a generic class's __init__ parameter typed `T`
    // seeds an attribute slot with `Ty::Param("T")`, which is substituted
    // at monomorphization time.
    #[test]
    fn generic_class_init_param_seeds_param_typed_slot() {
        let hir =
            lower_ok("class C[T]:\n    def __init__(self, x: T) -> None:\n        self.x = x\n");
        assert_eq!(hir.class_defs[0].1.type_param, Some("T".to_string()));
        assert_eq!(
            hir.class_defs[0].1.attrs,
            vec![("x".to_string(), Ty::Param(Box::new("T".to_string())))]
        );
    }

    // PEP 695 (#387 Part 3): `TypeVarTuple` (`*Ts`) and `ParamSpec` (`**P`)
    // type parameters are not supported — `type_param_name`'s `?` error path
    // (the uncovered region at line 120) is exercised by both forms.
    #[test]
    fn a_generic_class_with_a_type_var_tuple_is_unsupported() {
        assert_c0001("class C[*Ts]:\n    def __init__(self) -> None:\n        return\n");
    }

    #[test]
    fn a_generic_class_with_a_param_spec_is_unsupported() {
        assert_c0001("class C[**P]:\n    def __init__(self) -> None:\n        return\n");
    }

    #[test]
    fn duplicate_bases_are_rejected() {
        let module = crate::pycc_parser_test_helper::parse(
            "class A:\n    def __init__(self) -> None:\n        return\nclass B(A, A):\n    def __init__(self) -> None:\n        return\n",
        );
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic.message.contains("lists base `A` more than once"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn a_generic_class_with_base_classes_is_rejected() {
        let module = crate::pycc_parser_test_helper::parse(
            "class Base:\n    def __init__(self) -> None:\n        return\nclass C[T](Base):\n    def __init__(self, x: T) -> None:\n        self.x = x\n",
        );
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic
                .message
                .contains("generic class `C` with base classes"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn override_on_a_valid_method_lowers_successfully() {
        let hir = lower_ok(
            "class A:\n    def __init__(self) -> None:\n        return\n    def f(self) -> int:\n        return 1\nclass B(A):\n    def __init__(self) -> None:\n        return\n    @override\n    def f(self) -> int:\n        return 2\n",
        );
        let (_, b_def) = &hir.class_defs[1];
        assert!(b_def.methods.iter().any(|(name, _)| name == "f"));
    }

    #[test]
    fn override_on_a_nonexistent_method_is_t0031() {
        let module = crate::pycc_parser_test_helper::parse(
            "class A:\n    def __init__(self) -> None:\n        return\n    def f(self) -> int:\n        return 1\nclass B(A):\n    def __init__(self) -> None:\n        return\n    @override\n    def g(self) -> int:\n        return 2\n",
        );
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "T0031");
        assert!(
            diagnostic.message.contains("does not override"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn override_on_a_property_is_valid() {
        let hir = lower_ok(
            "class A:\n    def __init__(self) -> None:\n        self._x = 0\n    @property\n    def x(self) -> int:\n        return self._x\nclass B(A):\n    def __init__(self) -> None:\n        return\n    @override\n    def x(self) -> int:\n        return 42\n",
        );
        let (_, b_def) = &hir.class_defs[1];
        // The @override-decorated method `x` is a regular method (not a
        // property), and it overrides the property `x` from `A`.
        assert!(b_def.methods.iter().any(|(name, _)| name == "x"));
    }

    #[test]
    fn a_derived_class_without_init_inherits_base_init() {
        let hir = lower_ok(
            "class A:\n    def __init__(self, x: int) -> None:\n        self.x = x\nclass B(A):\n    def f(self) -> int:\n        return self.x\n",
        );
        let (_, b_def) = &hir.class_defs[1];
        // B has no __init__ of its own, but inherits A's.
        assert!(!b_def.methods.iter().any(|(name, _)| name == "__init__"));
        assert_eq!(b_def.mro, vec!["B".to_string(), "A".to_string()]);
    }

    #[test]
    fn a_class_without_init_and_without_base_init_is_rejected() {
        let module = crate::pycc_parser_test_helper::parse(
            "class A:\n    def __init__(self) -> None:\n        return\nclass B(A):\n    def f(self) -> int:\n        return 1\n",
        );
        // `B` has no `__init__` and its base `A` has one, so this should
        // succeed (B inherits A's `__init__`). But `B`'s own `f` method
        // means it has a valid class body. The test name is historical --
        // it originally tested `pass` being rejected, but #378 made `pass`
        // valid in class bodies. This now tests that a derived class with
        // a method but no `__init__` inherits the base's `__init__`.
        let hir = lower_checked(&module).unwrap();
        let b_def = hir
            .class_defs
            .iter()
            .find(|(name, _)| name == "B")
            .map(|(_, def)| def)
            .unwrap();
        assert!(
            !b_def.methods.iter().any(|(mn, _)| mn == "__init__"),
            "B should not have its own __init__"
        );
    }

    #[test]
    fn a_class_with_no_bases_and_no_init_is_rejected() {
        let module = crate::pycc_parser_test_helper::parse(
            "class C:\n    def f(self) -> int:\n        return 1\n",
        );
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic
                .message
                .contains("no base class in its MRO provides one"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn a_non_name_base_class_expression_is_unsupported() {
        // #432: `class C(SomeMod.Base):` parses `SomeMod.Base` as an
        // `Expr::Attribute`, not an `Expr::Name` -- only a bare name is
        // supported as a base class for now.
        let module = crate::pycc_parser_test_helper::parse(
            "class C(SomeMod.Base):\n    def __init__(self) -> None:\n        return\n",
        );
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic
                .message
                .contains("a base class must be a bare name"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    // -----------------------------------------------------------------------
    // #435 (Part B): __init_subclass__ validation unit tests
    // -----------------------------------------------------------------------

    #[test]
    fn init_subclass_with_pass_body_in_subclass_of_base_with_init_subclass_is_accepted() {
        // A base class B defines `__init_subclass__` with `pass`, and a
        // subclass D also defines `__init_subclass__` with `pass`. This
        // exercises the `validate_init_subclass_body` path with a `pass`
        // body (the `Stmt::Pass` arm).
        let module = crate::pycc_parser_test_helper::parse(
            "class B:\n    def __init__(self) -> None:\n        return\n    def __init_subclass__(self) -> None:\n        pass\nclass D(B):\n    def __init__(self) -> None:\n        super().__init__()\n    def __init_subclass__(self) -> None:\n        pass\n",
        );
        let hir = lower_checked(&module);
        assert!(hir.is_ok(), "pass body should be accepted");
    }

    #[test]
    fn init_subclass_with_docstring_in_subclass_of_base_with_init_subclass_is_accepted() {
        // A subclass D defines `__init_subclass__` with a docstring body.
        // This exercises the `Stmt::Expr` + `StringLiteral` arm.
        let module = crate::pycc_parser_test_helper::parse(
            "class B:\n    def __init__(self) -> None:\n        return\n    def __init_subclass__(self) -> None:\n        pass\nclass D(B):\n    def __init__(self) -> None:\n        super().__init__()\n    def __init_subclass__(self) -> None:\n        \"docstring\"\n",
        );
        let hir = lower_checked(&module);
        assert!(hir.is_ok(), "docstring body should be accepted");
    }

    #[test]
    fn init_subclass_with_non_string_expr_in_subclass_of_base_with_init_subclass_is_rejected() {
        // A subclass D defines `__init_subclass__` with a non-string
        // expression statement (e.g. `42`). This exercises the
        // `Stmt::Expr` + non-`StringLiteral` error path.
        let module = crate::pycc_parser_test_helper::parse(
            "class B:\n    def __init__(self) -> None:\n        return\n    def __init_subclass__(self) -> None:\n        pass\nclass D(B):\n    def __init__(self) -> None:\n        super().__init__()\n    def __init_subclass__(self) -> None:\n        42\n",
        );
        let err = lower_checked(&module).unwrap_err();
        assert_eq!(err.code, "C0001");
        assert!(err.message.contains("__init_subclass__"));
    }

    #[test]
    fn init_subclass_with_return_in_subclass_of_base_with_init_subclass_is_rejected() {
        // A subclass D defines `__init_subclass__` with a `return`
        // statement. This exercises the `_ =>` catch-all error path.
        let module = crate::pycc_parser_test_helper::parse(
            "class B:\n    def __init__(self) -> None:\n        return\n    def __init_subclass__(self) -> None:\n        pass\nclass D(B):\n    def __init__(self) -> None:\n        super().__init__()\n    def __init_subclass__(self) -> None:\n        return\n",
        );
        let err = lower_checked(&module).unwrap_err();
        assert_eq!(err.code, "C0001");
        assert!(err.message.contains("__init_subclass__"));
    }

    #[test]
    fn init_subclass_with_empty_body_in_subclass_of_base_is_accepted() {
        // A subclass D defines `__init_subclass__` with an empty body
        // (just a docstring at the parser level, but the body after
        // docstring extraction is empty). This exercises the `Ok(())`
        // return from `validate_init_subclass_body`.
        // Actually, Python requires at least one statement in a body, so
        // we use `pass` which is filtered by `lower_body` but still
        // present in the AST for `validate_init_subclass_body`.
        // The `pass` body exercises both `Stmt::Pass` continue and the
        // final `Ok(())`.
        let module = crate::pycc_parser_test_helper::parse(
            "class B:\n    def __init__(self) -> None:\n        return\n    def __init_subclass__(self) -> None:\n        pass\nclass D(B):\n    def __init__(self) -> None:\n        super().__init__()\n    def __init_subclass__(self) -> None:\n        pass\n",
        );
        let hir = lower_checked(&module);
        assert!(hir.is_ok(), "pass body should be accepted");
    }

    #[test]
    fn init_subclass_without_base_having_init_subclass_is_not_validated() {
        // A class defines `__init_subclass__` but no base class has it.
        // The validation should NOT run (no base_has_init_subclass), so even
        // a non-trivial body is accepted (it's just a regular method).
        let module = crate::pycc_parser_test_helper::parse(
            "class D:\n    def __init__(self) -> None:\n        return\n    def __init_subclass__(self) -> None:\n        print(1)\n",
        );
        let hir = lower_checked(&module);
        assert!(
            hir.is_ok(),
            "non-trivial body without base init_subclass should be accepted"
        );
    }

    #[test]
    fn subclass_without_own_init_subclass_inherits_statically_evaluable_base_hook() {
        // A base class B defines `__init_subclass__` with a `pass` body,
        // and a subclass D does NOT define its own `__init_subclass__`.
        // D's methods don't include `__init_subclass__`, so this takes the
        // #585 inherited-hook path (re-validating B's body against D's
        // creation site) rather than the #435 own-hook path -- but since
        // B's body is statically evaluable, that re-validation still
        // succeeds.
        let module = crate::pycc_parser_test_helper::parse(
            "class B:\n    def __init__(self) -> None:\n        return\n    def __init_subclass__(self) -> None:\n        pass\nclass D(B):\n    def __init__(self) -> None:\n        super().__init__()\n",
        );
        let hir = lower_checked(&module);
        assert!(
            hir.is_ok(),
            "subclass inheriting a statically-evaluable __init_subclass__ should be accepted"
        );
    }

    #[test]
    fn subclass_without_own_init_subclass_rejects_side_effecting_inherited_body() {
        // #585: a base class B defines `__init_subclass__` with a
        // side-effecting body, and a subclass D inherits it unchanged
        // (does not override it). CPython would invoke B's hook when D is
        // created; pycc cannot run that side-effecting body at
        // class-creation time, so this must now be rejected at D's
        // creation site -- unlike `init_subclass_without_base_having_init_subclass_is_not_validated`,
        // where B is never subclassed and therefore stays legal.
        // B's `__init_subclass__` is deliberately followed by `__init__`
        // (not the last statement in the body) so the reverse walk over
        // `base_ast.body` also exercises its `_ => {}` catch-all arm before
        // finding the match, mirroring `init_subclass_before_init_in_body_validates_correctly`
        // below for the analogous walk over the subclass's own body.
        let module = crate::pycc_parser_test_helper::parse(
            "class B:\n    def __init_subclass__(self) -> None:\n        print(1)\n    def __init__(self) -> None:\n        return\nclass D(B):\n    def __init__(self) -> None:\n        super().__init__()\n",
        );
        let err = lower_checked(&module).unwrap_err();
        assert_eq!(err.code, "C0001");
        assert!(
            err.message.contains("inherited"),
            "unexpected message: {}",
            err.message
        );
    }

    #[test]
    fn base_alone_never_subclassed_with_side_effecting_init_subclass_stays_legal() {
        // #585's boundary case, restated explicitly: a base class defining
        // `__init_subclass__` with a side-effecting body must still compile
        // when it is never subclassed in this module -- CPython never
        // invokes the hook until a subclass actually exists, so rejecting
        // it here (at the base's own definition) would over-reject a
        // legal, standalone class. This is the same program shape as
        // `init_subclass_without_base_having_init_subclass_is_not_validated`
        // above; kept as its own #585-focused test for direct traceability
        // to the issue's own boundary requirement.
        let module = crate::pycc_parser_test_helper::parse(
            "class B:\n    def __init__(self) -> None:\n        return\n    def __init_subclass__(self) -> None:\n        print(1)\n",
        );
        let hir = lower_checked(&module);
        assert!(
            hir.is_ok(),
            "a base class's own side-effecting __init_subclass__ must stay legal when it is \
             never subclassed"
        );
    }

    #[test]
    fn init_subclass_before_init_in_body_validates_correctly() {
        // A subclass D defines `__init_subclass__` BEFORE `__init__` in
        // the body. The reverse iteration checks `__init__` first (not
        // `__init_subclass__`), hitting the `_ => {}` branch, then finds
        // `__init_subclass__` and validates it.
        let module = crate::pycc_parser_test_helper::parse(
            "class B:\n    def __init__(self) -> None:\n        return\n    def __init_subclass__(self) -> None:\n        pass\nclass D(B):\n    def __init_subclass__(self) -> None:\n        pass\n    def __init__(self) -> None:\n        super().__init__()\n",
        );
        let hir = lower_checked(&module);
        assert!(hir.is_ok(), "init_subclass before init should be accepted");
    }

    // -- #379 (PR-19): PEP 435 enum class lowering ------------------------

    #[test]
    fn generic_enum_class_is_rejected() {
        // `class C[T](Enum):` — a generic class whose single base is `Enum`.
        // The type parameter `T` triggers the generic-enum rejection at
        // line 448-455, distinct from the multiple-bases rejection.
        assert_c0001("class C[T](Enum):\n    RED = 1\n");
    }

    #[test]
    fn enum_member_with_multiple_targets_is_rejected() {
        // `RED = GREEN = 1` — a chain assignment with multiple targets,
        // which has `assign.targets.len() == 2`, triggering the rejection
        // at line 551-556. (Tuple unpacking `RED, GREEN = 1, 2` has a
        // single tuple target and hits a different path.)
        assert_c0001("class C(Enum):\n    RED = GREEN = 1\n");
    }

    #[test]
    fn enum_member_with_non_name_target_is_rejected() {
        assert_c0001("class C(Enum):\n    C.RED = 1\n");
    }

    #[test]
    fn enum_member_value_overflowing_i64_is_rejected() {
        assert_c0001("class C(Enum):\n    RED = 99999999999999999999999999\n");
    }

    #[test]
    fn enum_member_with_non_literal_value_is_rejected() {
        assert_c0001("x = 1\nclass C(Enum):\n    RED = x\n");
    }

    // -- #379: enum error paths covered via unit tests (not integration
    //    tests) to avoid cargo-llvm-cov issue #276 (instantiation merging) --

    #[test]
    fn enum_body_with_method_is_rejected_via_unit_test() {
        // Exercises the "enum class body must contain only member
        // assignments" error path (lines 828-832).
        assert_c0001(
            "class Color(Enum):\n    RED = 1\n    def f(self) -> int:\n        return 1\n",
        );
    }

    #[test]
    fn duplicate_enum_member_is_rejected_via_unit_test() {
        // Exercises the "duplicate enum member" error path (lines 854-860).
        assert_c0001("class Color(Enum):\n    RED = 1\n    RED = 2\n");
    }

    #[test]
    fn enum_member_float_value_is_rejected_via_unit_test() {
        // Exercises the "non-integer value" error path (lines 884-893).
        assert_c0001("class Color(Enum):\n    RED = 1.5\n");
    }

    #[test]
    fn enum_member_bool_value_is_rejected_via_unit_test() {
        // Exercises the "non-integer value" error path (lines 884-893)
        // with a `bool` literal, a distinct match arm from `float`.
        assert_c0001("class Color(Enum):\n    RED = True\n");
    }

    #[test]
    fn enum_class_with_docstring_is_accepted() {
        // #744: a class docstring (a bare string-literal expression
        // statement) is a no-op in an enum body, not a member assignment.
        let hir = lower_ok("class Color(Enum):\n    \"A color.\"\n    RED = 1\n    GREEN = 2\n");
        let (_, class_def) = &hir.class_defs[0];
        assert_eq!(class_def.enum_members.len(), 2);
        assert_eq!(class_def.enum_members[0].0, "RED");
    }

    #[test]
    fn an_enum_class_with_a_non_leading_docstring_is_accepted() {
        // #744's guard has no position check: place the docstring after a
        // member assignment to exercise the non-leading case directly.
        let hir = lower_ok("class Color(Enum):\n    RED = 1\n    \"A color.\"\n    GREEN = 2\n");
        let (_, class_def) = &hir.class_defs[0];
        assert_eq!(class_def.enum_members.len(), 2);
        assert_eq!(class_def.enum_members[0].0, "RED");
        assert_eq!(class_def.enum_members[1].0, "GREEN");
    }

    #[test]
    fn a_non_string_expression_statement_in_an_enum_body_is_still_rejected() {
        // #744's docstring exemption covers only a bare string-literal
        // expression statement: a bare non-string expression statement in
        // an enum body remains C0001, exercising the guard's false branch
        // distinctly from a non-`Stmt::Expr` statement (which already
        // short-circuits before the guard).
        assert_c0001("class Color(Enum):\n    42\n    RED = 1\n");
    }

    #[test]
    fn valid_enum_class_lowers_via_unit_test() {
        // Covers `lower_enum_class`'s `Ok` return path (lines 911-932)
        // inside this crate's own unit-test binary, working around
        // cargo-llvm-cov issue #276 (instantiation-merge gap between
        // the library and integration-test binaries).
        let hir = lower_ok("class Color(Enum):\n    RED = 1\n    GREEN = 2\n    BLUE = 3\n");
        assert_eq!(hir.class_defs.len(), 1);
        let (_, class_def) = &hir.class_defs[0];
        assert!(!class_def.is_protocol);
        assert_eq!(class_def.enum_members.len(), 3);
        assert_eq!(class_def.enum_members[0].0, "RED");
        assert_eq!(class_def.enum_members[0].1, 1);
    }

    #[test]
    fn single_member_enum_class_lowers_via_unit_test() {
        // Additional `lower_enum_class` `Ok` return coverage with a
        // minimal single-member enum.
        let hir = lower_ok("class Color(Enum):\n    RED = 0\n");
        let (_, class_def) = &hir.class_defs[0];
        assert_eq!(class_def.enum_members.len(), 1);
        assert_eq!(class_def.enum_members[0].0, "RED");
        assert_eq!(class_def.enum_members[0].1, 0);
    }

    #[test]
    fn runtime_checkable_on_non_protocol_is_rejected_via_unit_test() {
        // Covers the `@runtime_checkable` on a non-protocol class error
        // path (lines 1082-1089) inside this crate's own unit-test binary.
        assert_c0001("@runtime_checkable\nclass C:\n    pass\n");
    }

    // -- #378 (PR-18): dataclass (PEP 557) lowering -----------------------

    #[test]
    fn a_dataclass_with_two_fields_lowers_successfully() {
        let hir = lower_ok("@dataclass\nclass Point:\n    x: int\n    y: int\n");
        assert_eq!(hir.class_defs.len(), 1);
        let (_, class_def) = &hir.class_defs[0];
        assert!(class_def.is_dataclass);
        assert_eq!(
            class_def.dataclass_fields,
            vec![("x".to_string(), Ty::Int), ("y".to_string(), Ty::Int),]
        );
        // __init__, __eq__, and __repr__ should be auto-generated.
        assert!(class_def.methods.iter().any(|(mn, _)| mn == "__init__"));
        assert!(class_def.methods.iter().any(|(mn, _)| mn == "__eq__"));
        assert!(class_def.methods.iter().any(|(mn, _)| mn == "__repr__"));
    }

    #[test]
    fn a_dataclass_with_docstring_lowers_successfully() {
        // #744: a class docstring (a bare string-literal expression
        // statement) is a no-op in a dataclass body, not a field or method.
        let hir = lower_ok("@dataclass\nclass Point:\n    \"A point.\"\n    x: int\n    y: int\n");
        let (_, class_def) = &hir.class_defs[0];
        assert!(class_def.is_dataclass);
        assert_eq!(
            class_def.dataclass_fields,
            vec![("x".to_string(), Ty::Int), ("y".to_string(), Ty::Int),]
        );
    }

    #[test]
    fn a_dataclass_with_a_non_leading_docstring_lowers_successfully() {
        // #744's guard has no position check: place the docstring after a
        // field to exercise the non-leading case directly.
        let hir = lower_ok("@dataclass\nclass Point:\n    x: int\n    \"A point.\"\n    y: int\n");
        let (_, class_def) = &hir.class_defs[0];
        assert!(class_def.is_dataclass);
        assert_eq!(
            class_def.dataclass_fields,
            vec![("x".to_string(), Ty::Int), ("y".to_string(), Ty::Int),]
        );
    }

    #[test]
    fn a_dataclass_transform_decorator_lowers_successfully() {
        let hir = lower_ok("@dataclass_transform()\nclass Point:\n    x: int\n");
        assert_eq!(hir.class_defs.len(), 1);
        let (_, class_def) = &hir.class_defs[0];
        assert!(class_def.is_dataclass);
    }

    #[test]
    fn a_dataclass_with_inheritance_merges_parent_fields() {
        let hir = lower_ok(
            "@dataclass\nclass Base:\n    a: int\n@dataclass\nclass Derived(Base):\n    b: int\n",
        );
        assert_eq!(hir.class_defs.len(), 2);
        let (_, derived_def) = &hir.class_defs[1];
        assert!(derived_def.is_dataclass);
        // Parent field `a` comes before child field `b`.
        assert_eq!(
            derived_def.dataclass_fields,
            vec![("a".to_string(), Ty::Int), ("b".to_string(), Ty::Int),]
        );
        assert_eq!(
            derived_def.attrs,
            vec![("a".to_string(), Ty::Int), ("b".to_string(), Ty::Int),]
        );
    }

    #[test]
    fn a_dataclass_child_redeclaring_a_parent_field_deduplicates() {
        // When a child dataclass redeclares a field already present in a
        // parent with the *same* type, the merge keeps the parent's field
        // and skips the child's duplicate rather than treating it as a
        // conflict (see the differing-type case rejected as T0052 below).
        let hir = lower_ok(
            "@dataclass\nclass Base:\n    a: int\n@dataclass\nclass Derived(Base):\n    a: int\n",
        );
        assert_eq!(hir.class_defs.len(), 2);
        let (_, derived_def) = &hir.class_defs[1];
        assert!(derived_def.is_dataclass);
        // The field `a` appears only once (from the parent).
        assert_eq!(
            derived_def.dataclass_fields,
            vec![("a".to_string(), Ty::Int)]
        );
    }

    #[test]
    fn a_dataclass_grandchild_merges_parent_and_grandparent_fields_with_dedup() {
        // A 3-level inheritance chain where the middle class redeclares a
        // grandparent field with the *same* type. When processing the
        // grandchild, the MRO-based merge iterates over the grandparent's
        // fields first (adding `x`), then the middle class's fields. The
        // middle class's `x` is already in `merged_fields` with a matching
        // type, so it is deduplicated rather than rejected. This covers the
        // same-type "already present" branch in the MRO-based parent-fields
        // merge loop.
        let hir = lower_ok(
            "@dataclass\nclass A:\n    x: int\n@dataclass\nclass B(A):\n    x: int\n\
             @dataclass\nclass C(B):\n    y: int\n",
        );
        assert_eq!(hir.class_defs.len(), 3);
        let (_, c_def) = &hir.class_defs[2];
        assert!(c_def.is_dataclass);
        // `x` (from A, deduplicated against B's redeclaration) comes before
        // `y` (C's own field).
        assert_eq!(
            c_def.dataclass_fields,
            vec![("x".to_string(), Ty::Int), ("y".to_string(), Ty::Int),]
        );
    }

    #[test]
    fn a_dataclass_field_type_conflict_across_mro_is_rejected_via_unit_test() {
        // Mirrors `pycc_types::tests::dataclass_field_type_conflict_across_mro_is_rejected_during_hir_lowering`
        // inside this crate's own unit-test binary, working around
        // cargo-llvm-cov issue #276 (instantiation-merge gap between
        // the library and integration-test binaries): a dataclass field
        // redeclared with a *differing* type across the MRO must be
        // rejected with T0052, exercising the second merge loop's
        // conflicting-type arm.
        let diagnostic = lower_checked(&crate::pycc_parser_test_helper::parse(
            "@dataclass\nclass Base:\n    v: int\n@dataclass\nclass Derived(Base):\n    v: str\n",
        ))
        .unwrap_err();
        assert_eq!(diagnostic.code, "T0052");
        assert!(
            diagnostic.message.contains('v')
                && diagnostic.message.contains("int")
                && diagnostic.message.contains("str"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn a_dataclass_field_type_conflict_between_two_bases_is_rejected_via_unit_test() {
        // Mirrors `pycc_types::tests::dataclass_field_type_conflict_between_two_bases_is_rejected_during_hir_lowering`
        // inside this crate's own unit-test binary, for the same
        // cargo-llvm-cov instantiation-merge reason as the sibling test
        // above: two independent dataclass bases declaring the same
        // field with differing types must be rejected with T0052,
        // exercising the first merge loop's conflicting-type arm.
        let diagnostic = lower_checked(&crate::pycc_parser_test_helper::parse(
            "@dataclass\nclass A:\n    v: int\n@dataclass\nclass B:\n    v: str\n@dataclass\nclass Derived(A, B):\n    pass\n",
        ))
        .unwrap_err();
        assert_eq!(diagnostic.code, "T0052");
        assert!(
            diagnostic.message.contains('v')
                && diagnostic.message.contains("int")
                && diagnostic.message.contains("str"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn a_call_decorator_with_a_non_name_callee_is_rejected() {
        // `@some.attr()` -- a call whose callee is an attribute access, not
        // a bare name. `classify_class_decorator`'s `Expr::Call` arm hits
        // the `let Expr::Name(name) = call.func.as_ref() else { ... }`
        // rejection.
        assert_c0001("@some.attr()\nclass C:\n    x: int\n");
    }

    #[test]
    fn a_call_decorator_with_an_unknown_name_is_rejected() {
        // `@other()` -- a call whose callee is a bare name but not
        // `dataclass_transform` or `dataclass`.
        assert_c0001("@other()\nclass C:\n    x: int\n");
    }

    #[test]
    fn a_dataclass_with_duplicate_field_names_is_rejected() {
        assert_c0001("@dataclass\nclass C:\n    x: int\n    x: int\n");
    }

    #[test]
    fn a_dataclass_with_an_unsupported_annotation_is_rejected() {
        // `annotation_to_ty` returns an error for `undefined_type`, which
        // propagates through the `?` on the `annotation_to_ty(...)` call.
        assert_c0001("@dataclass\nclass C:\n    x: undefined_type\n");
    }

    #[test]
    fn a_dataclass_with_a_non_method_non_annassign_statement_is_rejected() {
        // An `Assign` statement (`y = 5`) in a dataclass body is neither an
        // `AnnAssign` (field) nor a `FunctionDef` (method) nor `Pass`.
        assert_c0001("@dataclass\nclass C:\n    x: int\n    y = 5\n");
    }

    #[test]
    fn a_zero_field_dataclass_lowers_successfully() {
        let hir = lower_ok("@dataclass\nclass Empty:\n    pass\n");
        assert_eq!(hir.class_defs.len(), 1);
        let (_, class_def) = &hir.class_defs[0];
        assert!(class_def.is_dataclass);
        assert!(class_def.dataclass_fields.is_empty());
        // __init__, __eq__, and __repr__ should still be auto-generated.
        assert!(class_def.methods.iter().any(|(mn, _)| mn == "__init__"));
        assert!(class_def.methods.iter().any(|(mn, _)| mn == "__eq__"));
        assert!(class_def.methods.iter().any(|(mn, _)| mn == "__repr__"));
    }

    #[test]
    fn a_bare_field_call_in_a_dataclass_is_rejected() {
        // `x: int = field()` -- a bare `field()` call with no arguments.
        assert_c0001("@dataclass\nclass C:\n    x: int = field()\n");
    }

    #[test]
    fn a_dataclass_inheriting_from_a_non_dataclass_is_rejected() {
        let diagnostic = lower_checked(&crate::pycc_parser_test_helper::parse(
            "class Base:\n    def __init__(self) -> None:\n        return\n@dataclass\nclass Derived(Base):\n    b: int\n",
        ))
        .unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic
                .message
                .contains("cannot inherit from non-dataclass class `Base`"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn an_explicit_init_in_a_dataclass_is_rejected() {
        let diagnostic = lower_checked(&crate::pycc_parser_test_helper::parse(
            "@dataclass\nclass C:\n    x: int\n    def __init__(self, x: int) -> None:\n        self.x = x\n",
        ))
        .unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic.message.contains("auto-generates `__init__`"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn an_explicit_eq_in_a_dataclass_is_rejected() {
        assert_c0001(
            "@dataclass\nclass C:\n    x: int\n    def __eq__(self, other: C) -> bool:\n        return self.x == other.x\n",
        );
    }

    #[test]
    fn an_explicit_repr_in_a_dataclass_is_rejected() {
        assert_c0001(
            "@dataclass\nclass C:\n    x: int\n    def __repr__(self) -> str:\n        return \"C\"\n",
        );
    }

    #[test]
    fn multiple_decorators_on_a_dataclass_are_rejected() {
        // #380 (PR-20): `@dataclass` combined with `@runtime_checkable`
        // is rejected — a protocol class cannot be a dataclass.
        assert_c0001("@dataclass\n@runtime_checkable\nclass C:\n    x: int\n");
    }

    #[test]
    fn a_dataclass_with_options_is_rejected() {
        assert_c0001("@dataclass(frozen=True)\nclass C:\n    x: int\n");
    }

    #[test]
    fn a_non_name_field_target_in_a_dataclass_is_rejected() {
        assert_c0001("@dataclass\nclass C:\n    self.x: int\n");
    }

    #[test]
    fn a_plain_default_in_a_dataclass_field_is_rejected() {
        assert_c0001("@dataclass\nclass C:\n    x: int = 42\n");
    }

    #[test]
    fn a_field_default_in_a_dataclass_is_rejected() {
        assert_c0001("@dataclass\nclass C:\n    x: int = field(default=0)\n");
    }

    #[test]
    fn a_field_default_factory_in_a_dataclass_is_rejected() {
        assert_c0001("@dataclass\nclass C:\n    x: int = field(default_factory=int)\n");
    }

    // -- #378 scalar-field restriction: non-scalar types are rejected -----

    #[test]
    fn a_self_referential_dataclass_field_is_rejected() {
        // `next: Node` resolves to `Ty::Instance("Node")` via
        // `annotation_to_ty`'s self-referential class-name arm. Without
        // the scalar-slot-type check, this panics in codegen (the
        // attribute-slot storage is a single word, with no representation
        // for a class instance). The check rejects it with C0001 instead.
        assert_c0001("@dataclass\nclass Node:\n    next: Node\n");
    }

    #[test]
    fn a_self_typed_dataclass_field_is_rejected() {
        // `next: Self` resolves to `Ty::Instance("Node")` via
        // `annotation_to_ty`'s `Self` arm (PEP 673). Same root cause as
        // the self-referential class-name case above.
        assert_c0001("@dataclass\nclass Node:\n    next: Self\n");
    }

    #[test]
    fn a_none_typed_dataclass_field_is_rejected() {
        // `x: None` resolves to `Ty::None` via `annotation_to_ty`'s
        // `NoneLiteral` arm. `None` is not a scalar slot type.
        assert_c0001("@dataclass\nclass C:\n    x: None\n");
    }

    #[test]
    fn a_cross_class_instance_dataclass_field_is_rejected() {
        // `x: Other` in class `C` is rejected by `annotation_to_ty` itself
        // (the self-referential class-name arm only matches the enclosing
        // class's own name, not other classes; `Other` is not a builtin
        // type or alias, so it falls through to "type annotation `Other`
        // is not supported yet"). This is still C0001, but reached before
        // the scalar-slot-type check -- included here to document that
        // cross-class instance fields are rejected regardless of which
        // guard fires first.
        assert_c0001("@dataclass\nclass Other:\n    pass\n@dataclass\nclass C:\n    x: Other\n");
    }

    #[test]
    fn a_dataclass_field_with_a_non_scalar_type_gives_a_clear_message() {
        // Verify the diagnostic message names the field and its type, so
        // the user can identify which field and what type caused the
        // rejection.
        let diagnostic = lower_checked(&crate::pycc_parser_test_helper::parse(
            "@dataclass\nclass Node:\n    next: Node\n",
        ))
        .unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic.message.contains("dataclass field `next`"),
            "unexpected message: {}",
            diagnostic.message
        );
        assert!(
            diagnostic.message.contains("not a scalar slot type"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    // -- PEP 544 (#380): Protocol class lowering ---------------------------

    #[test]
    fn a_protocol_class_with_a_pass_body_lowers_successfully() {
        let hir = lower_ok("from typing import Protocol\nclass P(Protocol):\n    pass\n");
        assert_eq!(hir.class_defs.len(), 1);
        assert!(hir.class_defs[0].1.is_protocol);
        assert!(hir.class_defs[0].1.protocol_members.is_empty());
    }

    #[test]
    fn a_protocol_class_with_a_docstring_lowers_successfully() {
        // #744: a class docstring (a bare string-literal expression
        // statement) is a no-op in a protocol body.
        let hir = lower_ok(
            "from typing import Protocol\nclass P(Protocol):\n    \"A protocol.\"\n    def foo(self) -> int: ...\n",
        );
        let def = &hir.class_defs[0].1;
        assert!(def.is_protocol);
        assert_eq!(def.protocol_members.len(), 1);
    }

    #[test]
    fn a_protocol_class_with_a_non_leading_docstring_lowers_successfully() {
        // #744's guard has no position check: place the docstring after a
        // method to exercise the non-leading case directly.
        let hir = lower_ok(
            "from typing import Protocol\nclass P(Protocol):\n    def foo(self) -> int: ...\n    \"A protocol.\"\n",
        );
        let def = &hir.class_defs[0].1;
        assert!(def.is_protocol);
        assert_eq!(def.protocol_members.len(), 1);
    }

    #[test]
    fn a_non_string_expression_statement_in_a_protocol_body_is_still_rejected() {
        // #744's docstring exemption covers only a bare string-literal
        // expression statement: a bare non-string expression statement in a
        // protocol body remains C0001, exercising the guard's false branch
        // distinctly from a non-`Stmt::Expr` statement
        // (`a_protocol_class_with_an_unsupported_statement_is_rejected`
        // above uses `Stmt::Assign`, which never reaches this guard at all).
        let module = crate::pycc_parser_test_helper::parse(
            "from typing import Protocol\nclass P(Protocol):\n    42\n    def foo(self) -> int: ...\n",
        );
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
    }

    #[test]
    fn a_protocol_class_with_a_method_and_attribute_lowers_successfully() {
        let hir = lower_ok(
            "from typing import Protocol\nclass P(Protocol):\n    def foo(self) -> int: ...\n    x: str\n",
        );
        let def = &hir.class_defs[0].1;
        assert!(def.is_protocol);
        assert_eq!(def.protocol_members.len(), 2);
    }

    #[test]
    fn a_protocol_method_with_a_parameter_lowers_successfully() {
        // Exercises the `params.iter().map(|(_, ty)| ty.clone())` closure
        // on line 715 — when a protocol method has parameters besides
        // `self`, the closure body is entered to collect parameter types
        // into `ProtocolMember::Method::param_tys`.
        let hir = lower_ok(
            "from typing import Protocol\nclass P(Protocol):\n    def foo(self, x: int) -> int: ...\n",
        );
        let def = &hir.class_defs[0].1;
        assert!(def.is_protocol);
        assert_eq!(def.protocol_members.len(), 1);
    }

    #[test]
    fn a_protocol_class_with_init_is_rejected() {
        let module = crate::pycc_parser_test_helper::parse(
            "from typing import Protocol\nclass P(Protocol):\n    def __init__(self) -> None: ...\n",
        );
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic.message.contains("cannot define `__init__`"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn a_protocol_method_with_a_decorator_is_rejected() {
        let module = crate::pycc_parser_test_helper::parse(
            "from typing import Protocol\nclass P(Protocol):\n    @staticmethod\n    def foo(self) -> int: ...\n",
        );
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic.message.contains("decorators on protocol method"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn a_generic_protocol_method_is_rejected() {
        let module = crate::pycc_parser_test_helper::parse(
            "from typing import Protocol\nclass P(Protocol):\n    def foo[T](self) -> int: ...\n",
        );
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic.message.contains("generic protocol method"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn a_protocol_method_with_an_implementation_body_is_rejected() {
        let module = crate::pycc_parser_test_helper::parse(
            "from typing import Protocol\nclass P(Protocol):\n    def foo(self) -> int:\n        return 1\n",
        );
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic.message.contains("declaration-style body"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn a_protocol_class_with_an_unsupported_statement_is_rejected() {
        let module = crate::pycc_parser_test_helper::parse(
            "from typing import Protocol\nclass P(Protocol):\n    x = 1\n",
        );
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
    }

    #[test]
    fn a_protocol_class_with_an_attribute_with_a_default_is_rejected() {
        let module = crate::pycc_parser_test_helper::parse(
            "from typing import Protocol\nclass P(Protocol):\n    x: int = 0\n",
        );
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic.message.contains("cannot have a default value"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn a_generic_protocol_class_is_rejected() {
        let module = crate::pycc_parser_test_helper::parse(
            "from typing import Protocol\nclass P[T](Protocol):\n    def foo(self) -> int: ...\n",
        );
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic.message.contains("generic protocol class"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn a_dataclass_protocol_class_is_rejected() {
        let module = crate::pycc_parser_test_helper::parse(
            "from typing import Protocol\n@dataclass\nclass P(Protocol):\n    def foo(self) -> int: ...\n",
        );
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic.message.contains("cannot be a `@dataclass`"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn a_protocol_class_inheriting_and_redeclaring_a_method_lowers() {
        let hir = lower_ok(
            "from typing import Protocol\nclass P(Protocol):\n    def foo(self) -> int: ...\nclass Q(P):\n    def foo(self) -> str: ...\n",
        );
        let q_def = hir.class_defs.iter().find(|(n, _)| n == "Q").unwrap();
        assert!(q_def.1.is_protocol);
        // Q inherits P's `foo` but redeclares it with a different return type.
        // The redeclared version should replace the inherited one.
        assert_eq!(q_def.1.protocol_members.len(), 1);
    }

    #[test]
    fn a_protocol_class_inheriting_an_attribute_lowers() {
        let hir = lower_ok(
            "from typing import Protocol\nclass P(Protocol):\n    x: int\nclass Q(P):\n    def foo(self) -> int: ...\n",
        );
        let q_def = hir.class_defs.iter().find(|(n, _)| n == "Q").unwrap();
        assert!(q_def.1.is_protocol);
        // Q inherits P's `x` attribute and adds its own `foo` method.
        assert_eq!(q_def.1.protocol_members.len(), 2);
    }

    // -- PEP 3119 (#380): ABC and @abstractmethod lowering ----------------

    #[test]
    fn an_abc_class_with_an_abstract_method_lowers_successfully() {
        let hir = lower_ok(
            "from abc import ABC, abstractmethod\nclass A(ABC):\n    @abstractmethod\n    def foo(self) -> int: ...\n    def __init__(self) -> None:\n        return\n",
        );
        let def = &hir.class_defs[0].1;
        assert!(def.is_abstract);
        assert!(def.abstract_methods.contains(&"foo".to_string()));
    }

    #[test]
    fn an_abstract_method_with_an_implementation_body_is_rejected() {
        let module = crate::pycc_parser_test_helper::parse(
            "from abc import ABC, abstractmethod\nclass A(ABC):\n    @abstractmethod\n    def foo(self) -> int:\n        return 1\n    def __init__(self) -> None:\n        return\n",
        );
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic.message.contains("declaration-style body"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn a_concrete_subclass_inheriting_an_unoverridden_abstract_method_is_rejected() {
        let module = crate::pycc_parser_test_helper::parse(
            "from abc import ABC, abstractmethod\nclass A(ABC):\n    @abstractmethod\n    def foo(self) -> int: ...\n    def __init__(self) -> None:\n        return\nclass B(A):\n    def __init__(self) -> None:\n        return\n",
        );
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic.message.contains("abstract"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn a_class_with_more_than_two_decorators_is_rejected() {
        let module = crate::pycc_parser_test_helper::parse(
            "from typing import Protocol, runtime_checkable\n@runtime_checkable\n@dataclass\n@runtime_checkable\nclass P(Protocol):\n    def foo(self) -> int: ...\n",
        );
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic
                .message
                .contains("more than two class decorators"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn a_protocol_method_with_a_pass_body_lowers_successfully() {
        let hir = lower_ok(
            "from typing import Protocol\nclass P(Protocol):\n    def foo(self) -> int:\n        pass\n",
        );
        let def = &hir.class_defs[0].1;
        assert!(def.is_protocol);
        assert_eq!(def.protocol_members.len(), 1);
    }

    #[test]
    fn a_protocol_method_without_self_lowers_successfully() {
        let hir = lower_ok(
            "from typing import Protocol\nclass P(Protocol):\n    def foo() -> int: ...\n",
        );
        let def = &hir.class_defs[0].1;
        assert!(def.is_protocol);
        assert_eq!(def.protocol_members.len(), 1);
    }

    #[test]
    fn a_protocol_attribute_with_a_non_name_target_is_rejected() {
        let module = crate::pycc_parser_test_helper::parse(
            "from typing import Protocol\nclass P(Protocol):\n    self.x: int\n",
        );
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic.message.contains("must target a bare name"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn a_protocol_class_redeclaring_an_inherited_attribute_lowers() {
        let hir = lower_ok(
            "from typing import Protocol\nclass P(Protocol):\n    x: int\nclass Q(P):\n    x: str\n",
        );
        let q_def = hir.class_defs.iter().find(|(n, _)| n == "Q").unwrap();
        assert!(q_def.1.is_protocol);
        // Q redeclares P's `x` with a different type. The redeclared
        // version should replace the inherited one.
        assert_eq!(q_def.1.protocol_members.len(), 1);
    }

    #[test]
    fn a_protocol_class_inheriting_a_method_with_own_attribute_lowers() {
        // This covers the Attribute arm in the inherited-member dedup
        // check (line 606 in lower_protocol_body).
        let hir = lower_ok(
            "from typing import Protocol\nclass P(Protocol):\n    def foo(self) -> int: ...\nclass Q(P):\n    x: str\n",
        );
        let q_def = hir.class_defs.iter().find(|(n, _)| n == "Q").unwrap();
        assert!(q_def.1.is_protocol);
        // Q inherits P's `foo` method and adds its own `x` attribute.
        assert_eq!(q_def.1.protocol_members.len(), 2);
    }

    #[test]
    fn a_dataclass_inheriting_from_a_protocol_is_rejected() {
        let module = crate::pycc_parser_test_helper::parse(
            "from typing import Protocol\nclass P(Protocol):\n    def foo(self) -> int: ...\n@dataclass\nclass Q(P):\n    x: int\n",
        );
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic.message.contains("cannot be a `@dataclass`"),
            "unexpected message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn an_abstract_method_redefined_in_the_same_class_is_accepted() {
        // Redefining an abstract method in the same class should update
        // the method binding (matching regular method redefinition
        // semantics) and keep it in abstract_methods.
        let hir = lower_ok(
            "from abc import ABC, abstractmethod\nclass A(ABC):\n    @abstractmethod\n    def foo(self) -> int: ...\n    @abstractmethod\n    def foo(self) -> str: ...\n    def __init__(self) -> None:\n        return\n",
        );
        let def = &hir.class_defs[0].1;
        assert!(def.is_abstract);
        assert!(def.abstract_methods.contains(&"foo".to_string()));
    }

    #[test]
    fn a_concrete_subclass_overriding_an_inherited_abstract_method_lowers() {
        let hir = lower_ok(
            "from abc import ABC, abstractmethod\nclass A(ABC):\n    @abstractmethod\n    def foo(self) -> int: ...\n    def __init__(self) -> None:\n        return\nclass B(A):\n    def foo(self) -> int:\n        return 1\n    def __init__(self) -> None:\n        return\n",
        );
        let b_def = hir.class_defs.iter().find(|(n, _)| n == "B").unwrap();
        // B is concrete (not abstract) because it overrides foo.
        assert!(!b_def.1.is_abstract);
        // B's abstract_methods includes inherited ones from A, but B
        // overrides foo with a concrete method.
        assert!(b_def.1.abstract_methods.contains(&"foo".to_string()));
    }

    #[test]
    fn a_protocol_inheriting_from_two_protocols_with_overlapping_methods_lowers() {
        // This exercises the dedup check in the inherited-member loop
        // (class.rs lines 604-606): when two base protocols declare the
        // same method, the second inheritance skips the duplicate.
        let hir = lower_ok(
            "from typing import Protocol\nclass P1(Protocol):\n    def foo(self) -> int: ...\nclass P2(Protocol):\n    def foo(self) -> int: ...\nclass Q(P1, P2):\n    def bar(self) -> int: ...\n",
        );
        let q_def = hir.class_defs.iter().find(|(n, _)| n == "Q").unwrap();
        assert!(q_def.1.is_protocol);
        // Q should have `foo` (from P1/P2, deduplicated) and `bar` (own).
        assert_eq!(q_def.1.protocol_members.len(), 2);
    }

    #[test]
    fn a_protocol_inheriting_from_two_protocols_with_overlapping_attributes_lowers() {
        // This exercises the Attribute arm of the dedup check in the
        // inherited-member loop (class.rs line 606): when two base
        // protocols declare the same attribute, the second inheritance
        // skips the duplicate.
        let hir = lower_ok(
            "from typing import Protocol\nclass P1(Protocol):\n    x: int\nclass P2(Protocol):\n    x: int\nclass Q(P1, P2):\n    y: str\n",
        );
        let q_def = hir.class_defs.iter().find(|(n, _)| n == "Q").unwrap();
        assert!(q_def.1.is_protocol);
        // Q should have `x` (from P1/P2, deduplicated) and `y` (own).
        assert_eq!(q_def.1.protocol_members.len(), 2);
    }

    #[test]
    fn a_protocol_method_with_self_and_unsupported_param_annotation_is_rejected() {
        // This exercises the `?` error path on lower_arg_list in the
        // `self` branch of protocol method lowering (line 677).
        assert_c0001(
            "from typing import Protocol\nclass P(Protocol):\n    def foo(self, x: Frobnicate) -> int: ...\n",
        );
    }

    #[test]
    fn a_protocol_method_with_self_and_unsupported_return_annotation_is_rejected() {
        // This exercises the `?` error path on lower_return_annotation
        // in the `self` branch of protocol method lowering (line 686).
        assert_c0001(
            "from typing import Protocol\nclass P(Protocol):\n    def foo(self) -> Frobnicate: ...\n",
        );
    }

    #[test]
    fn a_protocol_method_without_self_and_unsupported_param_annotation_is_rejected() {
        // This exercises the `?` error path on lower_arg_list in the
        // no-`self` branch of protocol method lowering (line 701).
        assert_c0001(
            "from typing import Protocol\nclass P(Protocol):\n    def foo(x: Frobnicate) -> int: ...\n",
        );
    }

    #[test]
    fn a_protocol_method_without_self_and_unsupported_return_annotation_is_rejected() {
        // This exercises the `?` error path on lower_return_annotation
        // in the no-`self` branch of protocol method lowering (line 710).
        assert_c0001(
            "from typing import Protocol\nclass P(Protocol):\n    def foo() -> Frobnicate: ...\n",
        );
    }

    #[test]
    fn a_protocol_attribute_with_unsupported_annotation_is_rejected() {
        // This exercises the `?` error path on annotation_to_ty in
        // protocol attribute lowering (line 745).
        assert_c0001("from typing import Protocol\nclass P(Protocol):\n    x: Frobnicate\n");
    }

    #[test]
    fn a_protocol_method_with_a_docstring_body_is_rejected() {
        // A docstring is a `Stmt::Expr` whose value is a string literal,
        // not an `EllipsisLiteral`.  This exercises the `false` branch of
        // the `matches!` inside `is_declaration_body` (line 560), which
        // causes the protocol-method body check to reject it.
        assert_c0001(
            "from typing import Protocol\nclass P(Protocol):\n    def foo(self) -> int:\n        \"docstring\"\n",
        );
    }

    #[test]
    fn an_abstract_method_with_a_docstring_body_is_rejected() {
        // Same `is_declaration_body` false branch, reached via the
        // `@abstractmethod` lowering path (line 1693).
        assert_c0001(
            "from abc import ABC, abstractmethod\nclass A(ABC):\n    @abstractmethod\n    def foo(self) -> int:\n        \"docstring\"\n",
        );
    }

    #[test]
    fn a_concrete_subclass_inheriting_abstract_method_from_grandparent_lowers() {
        // This exercises the abstract method inheritance loop (line 1786)
        // by having a three-level inheritance chain: A (abstract) -> B
        // (abstract, explicitly inherits ABC) -> C (concrete, overrides).
        // The loop iterates over MRO entries beyond the first, finding
        // inherited abstract methods from both A and B.
        let hir = lower_ok(
            "from abc import ABC, abstractmethod\nclass A(ABC):\n    @abstractmethod\n    def foo(self) -> int: ...\n    def __init__(self) -> None:\n        return\nclass B(A, ABC):\n    @abstractmethod\n    def bar(self) -> int: ...\n    def __init__(self) -> None:\n        return\nclass C(B):\n    def foo(self) -> int:\n        return 1\n    def bar(self) -> int:\n        return 2\n    def __init__(self) -> None:\n        return\n",
        );
        let c_def = hir.class_defs.iter().find(|(n, _)| n == "C").unwrap();
        assert!(!c_def.1.is_abstract);
        assert!(c_def.1.abstract_methods.contains(&"foo".to_string()));
        assert!(c_def.1.abstract_methods.contains(&"bar".to_string()));
    }
}
