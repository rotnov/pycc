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
//! `pass`, a class-level attribute declaration, or any other statement kind
//! is `C0001`); redefining a non-`__init__` method name within one class
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

use crate::{HirItem, Ty, lower_arg_list, unsupported};
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
pub(crate) fn lower_class(
    def: &pycc_ast::StmtClassDef,
    aliases: &[(String, Ty)],
    defined_classes: &[(String, HirClassDef)],
) -> Result<(HirClassDef, Vec<HirItem>), Diagnostic> {
    if !def.decorator_list.is_empty() {
        return Err(unsupported(
            "class decorators are not supported yet",
            def.range,
        ));
    }
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
    // #432: validate each base class against the already-defined classes.
    for base_name in &bases {
        let Some(base_def) = defined_classes.iter().find(|(name, _)| name == base_name) else {
            return Err(unsupported(
                format!(
                    "class `{class_name}` inherits from unknown class `{base_name}` -- base \
                     classes must be defined earlier in the same module"
                ),
                def.range,
            ));
        };
        // Generic classes (from #387) with a type_param cannot be used as
        // base classes yet.
        if base_def.1.type_param.is_some() {
            return Err(unsupported(
                format!(
                    "class `{class_name}` cannot inherit from generic class `{base_name}` -- \
                     generic classes as bases are not supported yet"
                ),
                def.range,
            ));
        }
        // Reject circular inheritance: if the base class already lists this
        // class in its own MRO, then inheriting from it would create a
        // cycle.
        if base_def.1.mro.contains(&class_name) {
            return Err(unsupported(
                format!(
                    "class `{class_name}` cannot inherit from `{base_name}` -- circular \
                     inheritance is not supported"
                ),
                def.range,
            ));
        }
    }
    // #432: compute the C3 linearization (MRO). A `None` return means the
    // inheritance order is inconsistent (a C3 conflict), which is also
    // rejected as a circular/inconsistent inheritance error.
    let mro = compute_c3_mro(&class_name, &bases, defined_classes).ok_or_else(|| {
        unsupported(
            format!(
                "class `{class_name}` has an inconsistent method resolution order (MRO) -- \
                 the C3 linearization of its base classes is impossible"
            ),
            def.range,
        )
    })?;
    let mut methods: Vec<(String, String)> = Vec::new();
    let mut items: Vec<HirItem> = Vec::new();
    let mut attrs: Vec<(String, Ty)> = Vec::new();
    let mut properties: Vec<PropertyDef> = Vec::new();
    let mut static_methods: Vec<(String, String)> = Vec::new();
    let mut class_methods: Vec<(String, String)> = Vec::new();
    let mut enum_members: Vec<(String, i64)> = Vec::new();
    let mut init_seen = false;
    // #379 (PR-19): an enum class body is assignments only (`RED = 1`),
    // not method definitions. Handle this in a separate loop before the
    // regular class-body loop below, which rejects non-`def` statements.
    if is_enum {
        // An enum class's attrs are the two reserved member attributes
        // (`value`, `name`), so `member.value`/`member.name` resolve via
        // the existing `resolve_attr_get`/MIR slot resolution unchanged.
        attrs = vec![
            ("value".to_string(), Ty::Int),
            ("name".to_string(), Ty::Str),
        ];
        for stmt in &def.body {
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
        return Ok((
            HirClassDef {
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
            },
            items,
        ));
    }
    for stmt in &def.body {
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
        if method_name == "__init__"
            && matches!(kind, MethodKind::StaticMethod | MethodKind::ClassMethod)
        {
            return Err(unsupported(
                "`@staticmethod` and `@classmethod` cannot decorate `__init__` -- \
                 the constructor must be a regular instance method",
                method_def.range,
            ));
        }
        let (item, params) = lower_method(
            method_def,
            &class_name,
            type_param.as_deref(),
            aliases,
            &kind,
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
        }
        items.push(item);
    }
    if !methods.iter().any(|(name, _)| name == "__init__") {
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
                        validate_init_subclass_body(&fd.body, fd.range)?;
                        break;
                    }
                    _ => {}
                }
            }
        }
    }
    Ok((
        HirClassDef {
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
    if !parameters.posonlyargs.is_empty() {
        return Err(unsupported(
            "positional-only parameters (`/`) are not supported yet",
            parameters.range,
        ));
    }
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
        MethodKind::StaticMethod => lower_arg_list(
            &parameters.args,
            params_is_public,
            method_name,
            type_param,
            Some(class_name),
            aliases,
        )?,
        MethodKind::ClassMethod => {
            let [cls_param, rest @ ..] = parameters.args.as_slice() else {
                return Err(unsupported(
                    "a `@classmethod` must take `cls` as its first parameter",
                    def.range,
                ));
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
            p.extend(lower_arg_list(
                rest,
                params_is_public,
                method_name,
                type_param,
                Some(class_name),
                aliases,
            )?);
            p
        }
        _ => {
            let [self_param, rest @ ..] = parameters.args.as_slice() else {
                return Err(unsupported(
                    "a method must take `self` as its first parameter",
                    def.range,
                ));
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
            match kind {
                MethodKind::PropertyGetter { .. } if !rest.is_empty() => {
                    return Err(unsupported(
                        "a `@property` getter must take only `self` (no additional parameters)",
                        parameters.range,
                    ));
                }
                MethodKind::PropertySetter { .. } if rest.len() != 1 => {
                    return Err(unsupported(
                        "a `@<name>.setter` setter must take exactly one parameter besides `self`",
                        parameters.range,
                    ));
                }
                _ => {}
            }
            let self_ty = Ty::Instance(Box::new(class_name.to_string()));
            let mut p = vec![("self".to_string(), self_ty)];
            p.extend(lower_arg_list(
                rest,
                params_is_public,
                method_name,
                type_param,
                Some(class_name),
                aliases,
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
    )?;
    let body = crate::stmt::lower_body(
        &def.body,
        aliases,
        false,
        true,
        Some(class_name),
        type_param,
    )?;
    // #377/#436: compute the mangled name based on the method kind. A
    // regular method uses `<Class>.<name>`. A property getter uses the
    // same `<Class>.<name>`. A property setter uses
    // `<Class>.<name>.setter`. A static method uses
    // `<Class>.<name>.static`. A class method uses
    // `<Class>.<name>.classmethod`. The `.static`/`.classmethod` suffixes
    // prevent collision with a regular method of the same name, since a
    // real Python identifier can never contain a `.`.
    let mangled_name = match kind {
        MethodKind::Regular { .. } | MethodKind::PropertyGetter { .. } => {
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

/// #435 (Part B): Validates that an `__init_subclass__` method body is
/// statically evaluable — only a `pass` body, an empty body, or a body
/// consisting solely of a docstring (a bare string-literal expression
/// statement) is accepted. Any other statement (a `print` call, an
/// assignment, a return, etc.) is rejected with `C0001`, since pycc's
/// compile-time class-creation model has no mechanism to run side-effecting
/// statements at class-definition time.
fn validate_init_subclass_body<R>(body: &[Stmt], range: R) -> Result<(), Diagnostic>
where
    std::ops::Range<u32>: From<R>,
{
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
                return Err(unsupported(
                    "`__init_subclass__` must be statically evaluable (only `pass` \
                     or a docstring is supported in this version) — \
                     side-effecting statements are not supported yet",
                    range,
                ));
            }
            _ => {
                return Err(unsupported(
                    "`__init_subclass__` must be statically evaluable (only `pass` \
                     or a docstring is supported in this version) — \
                     side-effecting statements are not supported yet",
                    range,
                ));
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

    fn lower_ok(source: &str) -> crate::HirModule {
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
        // independent functions in `lib.rs`, each needing its own check and
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
    fn a_positional_only_method_parameter_is_unsupported() {
        assert_c0001("class C:\n    def __init__(self, x: int, /) -> None:\n        return\n");
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
            "class A:\n    def __init__(self) -> None:\n        return\nclass B(A):\n    pass\n",
        );
        // `pass` in a class body is C0001 (not a def), so this is rejected
        // before the __init__ check even runs.
        let diagnostic = lower_checked(&module).unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
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
        // even defined. This test bypasses `lower_checked` (which processes
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
            name: "A".to_string(),
            bases: Vec::new(),
            mro: vec!["A".to_string(), "B".to_string()],
            attrs: vec![],
            methods: vec![("__init__".to_string(), "A.__init__".to_string())],
            type_param: None,
            properties: Vec::new(),
            static_methods: Vec::new(),
            class_methods: Vec::new(),
            enum_members: Vec::new(),
        };
        let defined_classes = vec![("A".to_string(), fake_a)];
        let diagnostic = super::lower_class(def, &[], &defined_classes).unwrap_err();
        assert_eq!(diagnostic.code, "C0001");
        assert!(
            diagnostic
                .message
                .contains("circular inheritance is not supported"),
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
    fn subclass_without_own_init_subclass_inherits_from_base_without_validation() {
        // A base class B defines `__init_subclass__`, and a subclass D
        // does NOT define its own `__init_subclass__`. The outer `if` on
        // line 831 is false (D's methods don't include `__init_subclass__`),
        // so the validation block is skipped entirely.
        let module = crate::pycc_parser_test_helper::parse(
            "class B:\n    def __init__(self) -> None:\n        return\n    def __init_subclass__(self) -> None:\n        pass\nclass D(B):\n    def __init__(self) -> None:\n        super().__init__()\n",
        );
        let hir = lower_checked(&module);
        assert!(
            hir.is_ok(),
            "subclass without own __init_subclass should be accepted"
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
}
