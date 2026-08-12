use pycc_ast::{Expr, ModModule, Stmt};
use pycc_diag::{Diagnostic, Span};

mod class;
mod expr;
mod stmt;

pub use class::{HirClassDef, PropertyDef};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ty {
    Int,
    Float,
    Bool,
    Str,
    None,
    Infer,
    /// A generic function's own type parameter (PEP 695, D-133), e.g. the `T`
    /// in `def f[T](x: T) -> T`. Distinct from `Ty::Infer`: this is resolved by
    /// call-site substitution (D-134), not by unification, and must never
    /// reach `pycc_mir` unsubstituted (same invariant `Ty::Infer` already
    /// holds, see the assertion near this enum's other pre-MIR checks).
    /// Boxed as `Box<String>`, not `Box<str>`: `str` is unsized, so
    /// `Box<str>` is itself a fat (16-byte, data-ptr + length) pointer,
    /// which measured `size_of::<Ty>() == 24` here -- it broke the
    /// niche-filling layout uniformity the other thin-pointer variants
    /// rely on (see `Tuple`'s doc comment above for the same phenomenon
    /// with `Box<[Ty]>` vs `Box<Vec<Ty>>`). `Box<String>` is a single
    /// (8-byte) thin pointer to a further heap-indirected `String` --
    /// confirmed by measurement to restore `size_of::<Ty>() == 16`,
    /// matching `Tuple`'s `Box<Vec<Ty>>` shape. (D-133's ADR text says
    /// `Box<str>`; this is a deliberate, measurement-driven deviation --
    /// `Box<str>` does not in fact keep `Ty` at the D-109 ceiling.)
    Param(Box<String>),
    /// `list[T]`. Type-checking is planned to accept any scalar `T`; only
    /// `T = Ty::Int` gets real codegen in v0.2 (D-105). Codegen rejecting
    /// every other element type before it becomes an unhandled codegen
    /// case is planned via a `pycc_types` diagnostic (`T0034`, per D-105 --
    /// not yet implemented as of this commit; this variant only defines
    /// the type representation).
    List(Box<Ty>),
    /// `dict[K, V]`. No v0.2 code path constructs this yet (PR-11's own
    /// scope per `docs/DELIVERY_PLAN.md`) -- the variant exists now only
    /// because D-089 decided `Ty`'s full recursive shape up front, so
    /// every later PR's match arms are additive, not migratory again.
    /// The key/value pair is boxed together as a single pointer (D-109):
    /// this variant was not itself the dominant contributor to `Ty`'s
    /// pre-fix 24-byte size (`Tuple(Vec<Ty>)` was), but once `Tuple`
    /// shrinks to a single 8-byte pointer too, `Dict`'s original
    /// two-separate-`Box<Ty>` shape (16 bytes) would become the new
    /// ceiling -- boxing it avoids that, not because `dict[K,V]` needs
    /// its own codegen yet.
    Dict(Box<(Ty, Ty)>),
    /// `set[T]`. Same status as `Dict` above -- PR-11's own scope.
    Set(Box<Ty>),
    /// `tuple[A, B, ...]`. Same status as `Dict` above -- PR-11's own
    /// scope. Boxed (D-109) as `Box<Vec<Ty>>` -- a second indirection: a
    /// thin (8-byte) pointer to a heap-allocated `Vec<Ty>` -- not as
    /// `Box<[Ty]>` (a 16-byte fat pointer: data ptr + length), which was
    /// tried first and measured `size_of::<Ty>() == 24`, no reduction at
    /// all from the pre-fix size (confirmed independently in-crate and
    /// via a standalone `rustc` reproduction). `Box<Vec<Ty>>` measured
    /// `size_of::<Ty>() == 16` instead (`align_of::<Ty>()` stayed `8` in
    /// every configuration measured). The most plausible explanation is
    /// that rustc's niche-filling enum-layout optimization (the trick
    /// behind `size_of::<Option<Box<T>>>() == size_of::<Box<T>>()`) needs
    /// every dataful variant to share a uniform pointer shape to collapse
    /// the discriminant for free -- `Box<[Ty]>`'s fat pointer broke that
    /// uniformity against `List`/`Dict`/`Set`'s thin ones, `Box<Vec<Ty>>`
    /// restores it -- but this project has not independently re-derived
    /// rustc's exact layout algorithm against every configuration (in
    /// particular, the pre-fix shape already had non-uniform dataful
    /// variant sizes yet still measured 24, not the 32 bytes a naive
    /// "tag plus largest payload" rule would predict). Treat the measured
    /// numbers above as the authoritative facts and this paragraph's
    /// mechanism as a plausible, not proven, explanation.
    Tuple(Box<Vec<Ty>>),
    /// A single-class instance (no inheritance in this PR -- see D-154,
    /// "class instance layout"). Carries only the originating class's name,
    /// not its shape: attribute-name -> slot-index mapping and method
    /// signatures live in a module-level side table
    /// (`HirModule::class_defs`, `pycc_hir::class::HirClassDef`), the same
    /// shape `type_aliases`/`imports` already use for compile-time-only
    /// side information that isn't part of a `HirItem`/`HirStmt` value
    /// itself. Boxed as `Box<String>`, not `Box<str>`, mirroring
    /// `Ty::Param`'s own documented reasoning exactly: `Box<str>` is a fat
    /// (16-byte) pointer because `str` is unsized, which would break the
    /// niche-filling layout uniformity every other dataful `Ty` variant
    /// relies on to stay at a single thin (8-byte) pointer; `Box<String>`
    /// is a single thin pointer to a further heap-indirected `String`, so
    /// this variant does not move `size_of::<Ty>()` past the D-109 16-byte
    /// ceiling (see `ty_size_stays_within_d109_ceiling` below).
    Instance(Box<String>),
}

impl Ty {
    pub fn name(&self) -> String {
        match self {
            Ty::Int => "int".to_string(),
            Ty::Float => "float".to_string(),
            Ty::Bool => "bool".to_string(),
            Ty::Str => "str".to_string(),
            Ty::None => "None".to_string(),
            Ty::Infer => "<inferred>".to_string(),
            Ty::Param(name) => name.to_string(),
            Ty::List(elem) => format!("list[{}]", elem.name()),
            Ty::Dict(kv) => format!("dict[{}, {}]", kv.0.name(), kv.1.name()),
            Ty::Set(elem) => format!("set[{}]", elem.name()),
            Ty::Tuple(elems) => format!(
                "tuple[{}]",
                elems.iter().map(Ty::name).collect::<Vec<_>>().join(", ")
            ),
            Ty::Instance(class_name) => class_name.to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOpKind {
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv,
    Mod,
    Pow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOpKind {
    Eq,
    NotEq,
    Lt,
    LtE,
    Gt,
    GtE,
}

#[derive(Debug, Clone, PartialEq)]
pub enum HirExpr {
    IntLiteral(i64),
    FloatLiteral(f64),
    BoolLiteral(bool),
    StringLiteral(String),
    Name(String),
    Call {
        callee: String,
        args: Vec<HirExpr>,
    },
    BinOp {
        op: BinOpKind,
        left: Box<HirExpr>,
        right: Box<HirExpr>,
    },
    Compare {
        op: CmpOpKind,
        left: Box<HirExpr>,
        right: Box<HirExpr>,
    },
    FString(Vec<FStringPart>),
    /// `[e1, e2, ...]`. Element homogeneity is `pycc_types`' job, not this
    /// lowering step's -- HIR only records the syntactic shape (D-105).
    ListLiteral(Vec<HirExpr>),
    /// `base[index]`, a read (Load position). `Stmt::Assign`'s own target
    /// handling below special-cases an `Expr::Subscript` target on a bare
    /// name into a dedicated `HirStmt::DictSet` node instead of ever
    /// constructing this variant for it (PR-11 Task 3, D-123 -- `list[int]`
    /// itself is still read-only-indexed, D-105, but that is now
    /// `pycc_types`' judgment on `HirStmt::DictSet`'s base type, not a
    /// structural HIR-shape restriction), and every other assignment/for
    /// target (`Stmt::AnnAssign`, `Stmt::For`) still rejects a non-bare-name
    /// target before ever calling `lower_expr` on it. So a `Subscript` node
    /// still reaches this arm only in a value (Load) position; no separate
    /// `ExprContext` check is needed to enforce that here.
    Subscript {
        base: Box<HirExpr>,
        index: Box<HirExpr>,
    },
    /// `base[start:stop:step]` (PR-12, D-118). Each bound is independently
    /// optional, matching Python's own slice grammar (`xs[:3]`, `xs[2:]`,
    /// `xs[:]`, `xs[::2]` all parse). Unlike `Subscript`'s `index` (a plain
    /// `HirExpr`), each bound here is an `Option<Box<HirExpr>>` -- an
    /// omitted bound has no source expression to lower at all, and
    /// defaulting it to a literal `0`/some sentinel here would be
    /// incorrect: `stop`'s default is `len(base)`, which needs `base`'s own
    /// already-lowered value to compute, not a value knowable at this
    /// lowering step in isolation. `pycc_types`/`pycc_mir`/`pycc_codegen`
    /// each apply the actual default at the point they have enough context
    /// to do so correctly.
    ///
    /// Like `Subscript` above, this variant is only ever constructed in
    /// Load (value) position. A slice **assignment target**
    /// (`xs[1:3] = value`) never reaches it: `Stmt::Assign`'s own
    /// `Expr::Subscript` target arm (see that arm's own handling below)
    /// calls `lower_expr(&sub.slice)` directly on the bare `Expr::Slice`
    /// node, and `lower_expr`'s outer match has no top-level arm for
    /// `Expr::Slice` (only the nested one inside this file's own
    /// `Expr::Subscript` value-position arm) -- so it falls through to the
    /// generic "expression kind not supported yet" `C0001` catch-all,
    /// unchanged by this task. This is intentional, not an oversight: D-118
    /// scopes this PR's slicing support to reads only.
    Slice {
        base: Box<HirExpr>,
        start: Option<Box<HirExpr>>,
        stop: Option<Box<HirExpr>>,
        step: Option<Box<HirExpr>>,
    },
    /// `list.append(value)`, recognized as a single dedicated node rather
    /// than through any general method-call mechanism (D-105). Unlike
    /// `Subscript` above, this arm is *not* structurally restricted to any
    /// particular position -- because `ListAppend` is an `HirExpr` (not a
    /// statement-only form), it currently lowers successfully anywhere an
    /// expression is accepted, e.g. `y = x.append(2)` or
    /// `print(x.append(1))`, even though real Python's `list.append()`
    /// always returns `None` there and a value-producing use is
    /// meaningless. This lowering step deliberately does not judge that
    /// (see `list_append_used_as_a_value_lowers_successfully_today` below,
    /// which locks in today's actual behavior). `pycc_types` doesn't reject
    /// it either -- it type-checks `y = x.append(2)` as binding `y: None`,
    /// same as any other `None`-typed value. `print(x.append(1))` runs and
    /// prints `None`, matching CPython. D-131 also gives
    /// `y = x.append(2)` ordinary canonical `None` assignment storage, so
    /// both the side effect and the stored unit value are preserved. D-072
    /// remains narrower: it rejects using `print()` itself as a nested
    /// expression, not materializable `None` results such as `.append()`.
    ListAppend {
        list: String,
        value: Box<HirExpr>,
    },
    /// `{k1: v1, k2: v2, ...}`. Key/value homogeneity and the `dict[str,
    /// int]`-only codegen gate are `pycc_types`' job, not this lowering
    /// step's -- HIR only records the syntactic shape (mirrors
    /// `ListLiteral`, PR-11 Task 3). A dict-unpacking entry (`{**other}`,
    /// `DictItem.key == None` in the upstream grammar) is rejected as
    /// unsupported at lowering time (see `lower_expr`'s `Expr::Dict` arm)
    /// rather than represented here, since this variant has no shape for
    /// it.
    DictLiteral(Vec<(HirExpr, HirExpr)>),
    /// `{e1, e2, ...}`. Element homogeneity and the `set[int]`-only codegen
    /// gate are `pycc_types`' job, not this lowering step's -- HIR only
    /// records the syntactic shape (mirrors `ListLiteral`/`DictLiteral`
    /// exactly, PR-11 Task 7). Unlike `Expr::Dict`'s `DictItem`, upstream's
    /// `ExprSet` has no unpacking hole in its `elts: Vec<Expr>` shape, so
    /// there is no analogous rejection arm to write. Python also has no
    /// empty-set literal syntax at all (`{}` always parses as `Expr::Dict`,
    /// the grammar's own ambiguity resolution) -- so an empty
    /// `SetLiteral` can only ever be constructed by hand-built HIR (e.g. a
    /// `pycc_types` unit test), never by `lower_expr` itself.
    SetLiteral(Vec<HirExpr>),
    /// `(e1, e2, ...)`. Element *heterogeneity* is deliberately allowed at
    /// this HIR layer -- unlike `ListLiteral`/`SetLiteral`, a tuple's whole
    /// point is mixing element types (D-116). `pycc_types` still gates which
    /// element *types* are accepted (int/bool/float only, T0039) and which
    /// index forms are readable (literal in-range only, T0040); this
    /// variant only records the syntactic shape.
    ///
    /// Only a parenthesized/bare tuple literal (`(1, 2)`, `1, 2`) lowers to
    /// this form. Tuple-unpacking assignment (`a, b = t`) is a distinct,
    /// deferred capability (D-116) with no HIR shape of its own yet.
    TupleLiteral(Vec<HirExpr>),
    /// `list.pop()` (PR-12, D-119): a hand-recognized special form, mirroring
    /// `ListAppend`'s own shape exactly (no general method-call dispatch).
    /// No-argument form only -- removes and returns the list's last element,
    /// panicking at runtime if the list is empty (honest-panic convention,
    /// D-119). Unlike `ListAppend`, whose value is always `None`, this
    /// variant's own value is the list's element type -- `y = xs.pop()` is
    /// its primary intended use, not merely a today's-actual-behavior
    /// curiosity. A bare `xs.pop()` `ExprStmt` discarding the popped value is
    /// also fine and matches CPython. `pycc_types` gates the base value's
    /// type (`T0033`); MIR lowering and real runtime behavior are a later
    /// task's job (D-119 point 2 of the delivery split).
    ///
    /// One pre-existing, tracked caveat this variant newly reaches (not
    /// introduced or fixed by this task): `pycc_types`' constraint solver
    /// gives this expression no unification term (`Ok(None)`, mirroring
    /// every container-literal expression, per D-116's own correction), so
    /// once any function in a module is unannotated, assigning `y =
    /// xs.pop()` inside that solver-checked function's body never registers
    /// a binding for `y` -- a later read of `y` in the same function then
    /// fails with a misleading "not bound before this use" instead of the
    /// expected type. This is not a novel gap: `HirExpr::Subscript`'s own
    /// `collect_expr_constraints` arm (predates D-116/D-119, see commit
    /// `0930903`) already returns `Ok(None)` the same way, so a scalar-typed
    /// expression (`xs[0]`) already reached this exact gap long before this
    /// task. `.pop()` is simply another instance of the same pre-existing
    /// class, not a new or different failure mode.
    ListPop {
        list: String,
    },
    /// `dict.get(key, default)` (PR-12, D-119): exactly two arguments --
    /// returns `default` if `key` is absent from the dict, else the stored
    /// value. Mirrors `ListAppend`'s hand-recognized shape structurally, but
    /// -- like `ListPop` above -- is value-producing (the dict's value
    /// type), not `None`-producing. The zero/one-argument form CPython also
    /// supports (returning `None` on a missing key) is deliberately not
    /// shipped: this compiler has no `Optional[int]`/`None`-union
    /// representation for a `dict[str, int]`'s value type, so requiring the
    /// caller to always supply a same-typed default sidesteps that gap
    /// entirely rather than half-solving it. Shares `ListPop`'s own
    /// pre-existing D-116 solver-binding caveat above verbatim (also
    /// `Ok(None)` in the solver, also scalar-valued).
    DictGetOrDefault {
        dict: String,
        key: Box<HirExpr>,
        default: Box<HirExpr>,
    },
    /// `set.add(value)` (PR-12, D-119): mirrors `ListAppend`'s shape and
    /// `None`-producing behavior exactly. D-131 gives `y = s.add(1)` the
    /// same canonical `None` assignment storage as `.append()`; D-072's
    /// remaining nested-expression rejection is specific to `print()`'s
    /// own result. Insertion deduplicates exactly like set-literal
    /// construction already does (`pycc_rt_int_set_add`, already shipped by
    /// PR-11a), from a second, user-facing call site added by a later task.
    SetAdd {
        set: String,
        value: Box<HirExpr>,
    },
    /// `base.attr` (D-154, Part 1 of #375): a read of an instance attribute
    /// (or, structurally, any attribute-shaped expression -- `base` is
    /// lowered generically, since `lower_expr` has no type information to
    /// narrow it at this stage). Distinct from the `Expr::Attribute` arm's
    /// other lowering target, `HirExpr::Name("module.symbol")`: that form
    /// only fires when the receiver is a bare name that resolves against
    /// `pycc_std`'s stdlib module registry (a compile-time textual match,
    /// D-136/D-137); every other attribute-access shape -- including `self`
    /// and any other class-instance-typed receiver -- falls through to this
    /// variant instead. `pycc_types` rejects it when `base`'s inferred type
    /// is not `Ty::Instance` or `attr` does not name one of that class's
    /// declared attribute slots; `pycc_mir`/`pycc_codegen` resolve `attr` to
    /// a compile-time slot index via the class's `HirClassDef` (never a
    /// runtime string-keyed lookup, per the class-instance-layout ADR).
    AttrGet {
        base: Box<HirExpr>,
        attr: String,
    },
    /// `base.method(args)` (D-154, Part 1 of #375): a call shaped like an
    /// attribute access followed by a call, structurally recognized for any
    /// receiver that isn't one of the hand-recognized container methods
    /// (`.append()`/`.pop()`/`.get()`/`.add()`, tried first, see
    /// `lower_expr`'s `Expr::Call` arm) or a resolved stdlib module symbol
    /// call (`math.sqrt(x)`, also tried first). `self.foo()` and
    /// `some_instance.foo()` both lower to this same shape -- there is
    /// nothing at HIR-lowering time to distinguish them, since both are
    /// just "call a method named `foo` on whatever `base` evaluates to".
    /// `pycc_types` resolves `base`'s inferred type to a `Ty::Instance`,
    /// looks `method` up in that class's method table, and checks the call
    /// against the method's real signature; `pycc_mir`/`pycc_codegen`
    /// resolve the call to the method's compile-time-known mangled function
    /// symbol (`<ClassName>.<method_name>`, D-006's static-dispatch framing
    /// for a non-inherited class -- there is no vtable or runtime dispatch
    /// in this PR).
    MethodCall {
        base: Box<HirExpr>,
        method: String,
        args: Vec<HirExpr>,
    },
    /// `C[type_arg](args)` (PEP 695, #387): instantiation of a generic class
    /// with an explicit type argument. `class` is the generic class's name,
    /// `type_arg` is the resolved concrete type (a scalar `Ty` — int/float/
    /// bool/str — resolved at HIR-lowering time from the subscript's slice
    /// expression), and `args` are the constructor arguments. Reuses PR-13's
    /// `Ty::Param` call-site-substitution mechanism (D-133/D-134): at
    /// monomorphization time, the class's methods are specialized with `T`
    /// substituted by `type_arg`, and this expression is rewritten into an
    /// ordinary `HirExpr::Call` to the specialized class's mangled name.
    GenericClassInstantiate {
        class: String,
        type_arg: Ty,
        args: Vec<HirExpr>,
    },
    /// Zero-arg `super()` (PEP 3135, #433): represents the implicit
    /// `super(__class__, self)` reference available inside a method body.
    /// Only ever appears as the `base` of a `HirExpr::MethodCall` or
    /// `HirExpr::AttrGet` — a bare `super()` used as a standalone value is
    /// rejected at HIR-lowering time (it has no useful static-dispatch
    /// lowering on its own). The enclosing class name is NOT carried here:
    /// it is recovered downstream from the method's own mangled
    /// `<ClassName>.<method>` name (type checker) or from the MIR lowering
    /// context, the same way `self`'s own `Ty::Instance(class_name)` is
    /// already recovered. `super().method(args)` resolves at compile time to
    /// the next class after the current one in the MRO (D-006's static-
    /// dispatch framing — no vtable, no runtime dispatch, per the #433 ADR).
    Super,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FStringPart {
    Literal(String),
    Interpolation(Box<HirExpr>),
}

/// A comprehension's iterable source (PR-12, D-117): reuses
/// `HirStmt::ForList`'s own iterable polymorphism verbatim (a bare name is
/// resolved to `Ty::List`/`Ty::Dict`/`Ty::Set` downstream by
/// `pycc_types`/`pycc_mir`, exactly like a plain `for` loop) rather than
/// inventing a narrower, comprehension-specific iterable gate.
#[derive(Debug, Clone, PartialEq)]
pub enum CompIter {
    Range {
        start: HirExpr,
        stop: HirExpr,
        step: HirExpr,
    },
    Name(String),
}

// ---------------------------------------------------------------------------
// Issue #435: compile-time `isinstance`/`issubclass` evaluation helpers.
//
// pycc uses static dispatch (D-006) — `is_assignable` does not allow
// `Ty::Instance("D")` to be assigned to `Ty::Instance("B")`, so every
// variable's runtime type is exactly its declared static type. Therefore
// `isinstance` and `issubclass` can always be evaluated at compile time,
// emitting constant boolean values. No runtime type tags or RTTI are needed.
//
// These helpers are shared by `pycc_types` (type checker) and `pycc_mir`
// (MIR lowering) so both compute identical results from the same inputs.
// ---------------------------------------------------------------------------

/// Returns `true` if `name` is one of the builtin scalar type names pycc
/// recognizes as a valid class argument to `isinstance`/`issubclass`:
/// `int`, `str`, `float`, `bool`.
pub fn is_builtin_type_name(name: &str) -> bool {
    matches!(name, "int" | "str" | "float" | "bool")
}

/// Returns `true` if `name` is the builtin `Enum` base name recognized by
/// `lower_class` as a marker that a class is a PEP 435 enum (#379, PR-19).
/// `Enum` is not a user-defined class in `class_defs` -- it is a builtin
/// base name consumed as a marker, not recorded in the class's `bases`/`mro`.
/// `pycc_std` registers `enum.Enum` as an `EnumMarker` symbol so
/// `from enum import Enum` resolves (the import is a no-op binding — `Enum`
/// is never a first-class value, only a base class marker). The bare name
/// `Enum` (without any import) is also accepted, matching pycc's existing
/// textual-resolution precedent for `math.sqrt`.
pub fn is_enum_base_name(name: &str) -> bool {
    name == "Enum"
}

/// Computes the compile-time result of `isinstance(obj, target_class)`.
///
/// `obj_ty` is the inferred static type of the object expression.
/// `target_class` is the class name from the second argument (already
/// validated as either a user-defined class or a builtin type name).
/// `obj_mro` is the MRO of the object's class (if `obj_ty` is
/// `Ty::Instance`); for non-instance types it is unused.
///
/// Builtin subtype rules: `bool` is a subtype of `int` (matching CPython's
/// own type hierarchy where `bool` inherits from `int`).
pub fn eval_isinstance_single(obj_ty: &Ty, target_class: &str, obj_mro: &[String]) -> bool {
    match obj_ty {
        Ty::Instance(_) => obj_mro.iter().any(|c| c == target_class),
        Ty::Int => target_class == "int",
        Ty::Bool => target_class == "bool" || target_class == "int",
        Ty::Str => target_class == "str",
        Ty::Float => target_class == "float",
        _ => false,
    }
}

/// Computes the compile-time result of `issubclass(cls, target_class)`.
///
/// `cls` is the source class name from the first argument (already
/// validated as either a user-defined class or a builtin type name).
/// `target_class` is the class name from the second argument.
/// `cls_mro` is the MRO of the source class (if it is a user-defined
/// class); for builtin types it is unused.
///
/// Builtin subtype rules: `issubclass(bool, int)` is `true` (matching
/// CPython's own type hierarchy). Same-builtin comparisons (`issubclass(int,
/// int)`) are `true`.
pub fn eval_issubclass_single(cls: &str, target_class: &str, cls_mro: &[String]) -> bool {
    if is_builtin_type_name(cls) {
        if cls == "bool" && target_class == "int" {
            return true;
        }
        return cls == target_class;
    }
    // User class: check if target is in the source class's MRO.
    // The MRO includes the class itself, so `issubclass(D, D)` is true.
    if is_builtin_type_name(target_class) {
        // A user class is not a subclass of a builtin type (pycc's MRO
        // does not include `object` or any builtin).
        return false;
    }
    cls_mro.iter().any(|c| c == target_class)
}

/// Extracts class names from an `isinstance`/`issubclass` class argument
/// expression. The argument must be either:
/// - `HirExpr::Name(name)` — a single class name
/// - `HirExpr::TupleLiteral(elements)` — a tuple of class names, where each
///   element is `HirExpr::Name(name)`
///
/// Returns `Ok(names)` if the expression matches one of these shapes,
/// or `Err(ExtractClassNamesError)` if it doesn't (the caller produces the
/// appropriate diagnostic). An empty tuple is rejected (at least one class
/// is required).
pub fn extract_class_names(arg: &HirExpr) -> Result<Vec<String>, ExtractClassNamesError> {
    match arg {
        HirExpr::Name(name) => Ok(vec![name.clone()]),
        HirExpr::TupleLiteral(elements) => {
            if elements.is_empty() {
                return Err(ExtractClassNamesError);
            }
            let mut names = Vec::with_capacity(elements.len());
            for elem in elements {
                match elem {
                    HirExpr::Name(name) => names.push(name.clone()),
                    _ => return Err(ExtractClassNamesError),
                }
            }
            Ok(names)
        }
        _ => Err(ExtractClassNamesError),
    }
}

/// Error returned by [`extract_class_names`] when the argument is not a
/// valid class name or tuple of class names. The caller is responsible for
/// producing the appropriate diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtractClassNamesError;

#[derive(Debug, Clone, PartialEq)]
pub enum HirStmt {
    ExprStmt(HirExpr),
    Assign {
        target: String,
        value: HirExpr,
    },
    AnnAssign {
        target: String,
        annotation: Ty,
        value: Option<HirExpr>,
    },
    If {
        test: HirExpr,
        body: Vec<HirStmt>,
        orelse: Vec<HirStmt>,
    },
    While {
        test: HirExpr,
        body: Vec<HirStmt>,
    },
    ForRange {
        var: String,
        start: HirExpr,
        stop: HirExpr,
        step: HirExpr,
        body: Vec<HirStmt>,
    },
    /// `for var in list:`, parallel to the existing `ForRange` -- desugars
    /// to an index-counted loop starting in a later PR-10 task (D-105),
    /// not here. Also reused, unconditionally, for `for k in <bare-name
    /// dict>:` (PR-11 Task 3, D-123: iterates keys in insertion order) --
    /// this lowering step has no type information to distinguish a
    /// `list`-typed iterable from a `dict`-typed one, so `pycc_types`'
    /// own `ForList` arms resolve `list`'s real type to `Ty::List`,
    /// `Ty::Dict` (binding `var` as the key type), or reject it, not here.
    ForList {
        var: String,
        list: String,
        body: Vec<HirStmt>,
    },
    /// `<bare name>[key] = value`, PR-11 Task 3 (D-123 supersedes D-105's
    /// "no subscript assignment target anywhere in this file" consequence
    /// for `list[int]`; see `Stmt::Assign`'s own lowering arm below). `dict`
    /// is carried as a plain variable name, exactly like `ForList`'s `list`
    /// field and `ListAppend`'s `list` field -- this lowering step has no
    /// type information, so a `list[int]` target also lowers to this node
    /// today (`pycc_types` rejects it with `T0033`, mirroring how
    /// `ForList`'s own `list` field is resolved to `Ty::List`, `Ty::Dict`,
    /// or rejected downstream, not here).
    DictSet {
        dict: String,
        key: HirExpr,
        value: HirExpr,
    },
    /// `target = [elt for var in iter [if cond]]` (PR-12, D-117). Scoped to
    /// exactly one `for` clause and at most one `if` filter; only lowered
    /// when the comprehension is the direct RHS of a bare-name
    /// `Stmt::Assign` (see that arm's own handling below) -- anywhere else
    /// a comprehension expression appears, `lower_expr` has no arm for it
    /// and it falls through to that function's existing generic
    /// "expression kind not supported yet" catch-all. `var` is already the
    /// D-117 synthesized internal name, not the source spelling -- every
    /// occurrence of the source name inside `cond`/`elt` has already been
    /// rewritten by `rename_name_in_expr` before this node is constructed,
    /// so downstream crates never see the user's own loop-variable
    /// spelling at all.
    ListCompAssign {
        target: String,
        var: String,
        iter: CompIter,
        cond: Option<Box<HirExpr>>,
        elt: Box<HirExpr>,
    },
    /// `target = {key: value for var in iter [if cond]}` (PR-12, D-117).
    /// Mirrors `ListCompAssign` exactly except for the key/value split
    /// (Python's dict-comprehension grammar has no direct list-comp analog
    /// of a single `elt`).
    DictCompAssign {
        target: String,
        var: String,
        iter: CompIter,
        cond: Option<Box<HirExpr>>,
        key: Box<HirExpr>,
        value: Box<HirExpr>,
    },
    /// `target = {elt for var in iter [if cond]}` (PR-12, D-117). Mirrors
    /// `ListCompAssign` exactly -- a set comprehension's own shape is
    /// identical to a list comprehension's, differing only in which
    /// runtime constructor/insert pair `pycc_codegen` ends up calling.
    SetCompAssign {
        target: String,
        var: String,
        iter: CompIter,
        cond: Option<Box<HirExpr>>,
        elt: Box<HirExpr>,
    },
    Return(Option<HirExpr>),
    /// `base.attr = value` (D-154, Part 1 of #375). Structurally recognized
    /// for any assignment target shaped like an attribute access -- `base`
    /// is lowered generically, exactly like `HirExpr::AttrGet`'s own `base`,
    /// and for the same reason (no type information is available at this
    /// lowering step to narrow it to only `self` or only an instance-typed
    /// receiver). No slot `Ty` is carried here: `pycc_types` resolves
    /// `attr`'s declared type from the class's `HirModule::class_defs` entry
    /// (populated by `class::lower_class`'s own `__init__` pre-scan) and
    /// checks `value` against it; `pycc_mir`/`pycc_codegen` resolve `attr`
    /// to a compile-time slot index the same way. This single shape covers
    /// both an attribute's first (slot-establishing) assignment inside
    /// `__init__` and every later reassignment inside any method body --
    /// the two are distinguished only by `class::lower_class`'s own
    /// pre-scan, not by this node's own shape.
    AttrSet {
        base: HirExpr,
        attr: String,
        value: HirExpr,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum HirItem {
    Function {
        name: String,
        params: Vec<(String, Ty)>,
        return_ty: Ty,
        body: Vec<HirStmt>,
    },
    TopLevelStmt(HirStmt),
}

/// A compile-time-only import binding recorded by a module-level
/// `import`/`from ... import ...` statement resolved against `pycc_std`'s
/// registry (D-136/D-137). Mirrors `type_aliases`' side-table shape: an
/// import has zero runtime footprint of its own (no `HirStmt`/`HirItem` is
/// produced for it), it only makes a later name/attribute lookup resolve to
/// a stdlib registry entry.
#[derive(Debug, Clone, PartialEq)]
pub enum ImportBinding {
    /// `import math` -- binds `math` (or, for a dotted-but-single-segment
    /// name, whatever `local_name` ends up being; D-137 rejects every
    /// import shape other than a single bare recognized module name, so in
    /// practice `local_name` always equals the resolved module's source
    /// spelling) as a module namespace marker. `math` itself never carries
    /// a `Ty` -- only `math.<attr>` attribute access on this bound name
    /// resolves further, via `pycc_std::resolve_symbol`.
    Module {
        local_name: String,
        module: pycc_std::StdModule,
    },
    /// `from math import sqrt` -- binds `sqrt` directly to the resolved
    /// registry symbol, as if it were a fixed, non-inferred `Ty`/signature
    /// (the alias table from PR-13 is the closest existing precedent).
    Symbol {
        local_name: String,
        module: pycc_std::StdModule,
        symbol: pycc_std::StdSymbol,
    },
}

/// The bound local name of an import, regardless of which `ImportBinding`
/// variant it is -- used by `lower_checked`'s class-name-collision check
/// (D-068 review finding on #385) so it does not need to duplicate the
/// match on both variants at its own call site.
fn import_local_name(binding: &ImportBinding) -> &str {
    match binding {
        ImportBinding::Module { local_name, .. } | ImportBinding::Symbol { local_name, .. } => {
            local_name
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct HirModule {
    pub items: Vec<HirItem>,
    /// Compile-time-only name-to-`Ty` bindings from a `type X = <expr>`
    /// statement or a legacy `X: TypeAlias = <expr>` annotated assignment
    /// (D-135). Populated in source order by `lower_checked` as it walks the
    /// module body. Neither form lowers to any `HirItem`/`HirStmt` of its
    /// own -- the alias has zero HIR/MIR/codegen/runtime footprint, and this
    /// field exists purely so a later annotation naming the alias resolves
    /// to the same `Ty` (see `annotation_to_ty`'s alias-table lookup).
    pub type_aliases: Vec<(String, Ty)>,
    /// Compile-time-only stdlib import bindings (D-136/D-137), populated in
    /// source order by `lower_checked` exactly like `type_aliases`. Only a
    /// module-level `import`/`from ... import ...` statement is recognized
    /// here -- one nested inside a function body or any other block still
    /// reaches plain `lower_stmt`, which has no arm for `Stmt::Import`/
    /// `Stmt::ImportFrom` and falls through to the generic `C0001`
    /// catch-all, exactly like every other statement kind this compiler
    /// does not support inside a nested block.
    pub imports: Vec<ImportBinding>,
    /// Class name -> declared shape (attribute slots in first-`__init__`-
    /// assignment order, method table) (D-154, Part 1 of #375). Populated by
    /// `class::lower_class` as `lower_checked` walks the module body, in
    /// source order, mirroring `type_aliases`/`imports`'s own shape: a class
    /// definition has no `HirItem`/`HirStmt` footprint of its own (unlike a
    /// top-level function) -- only its individual methods do, each lowered
    /// into `items` as an ordinary mangled `HirItem::Function` (see
    /// `class::lower_class`'s own doc comment for the mangling scheme and
    /// the reasoning for not adding a dedicated `HirItem::ClassDef` variant).
    pub class_defs: Vec<(String, HirClassDef)>,
}

/// Lowers a parsed module into the HIR subset implemented by this pycc
/// version. Syntactically valid Python outside that subset returns `C0001`
/// with the unsupported node's source span instead of panicking.
///
/// Type aliases (D-135) are resolved in a single left-to-right pass: a
/// `type X = <expr>` or legacy `X: TypeAlias = <expr>` statement is
/// evaluated and recorded into `aliases` as soon as it is reached, so it is
/// visible to every later statement's annotations (including a later
/// function's parameter/return annotations and later top-level
/// `AnnAssign`s) but not to any earlier one -- matching this compiler's
/// existing single-pass, source-order lowering model instead of
/// introducing hoisting.
pub fn lower_checked(module: &ModModule) -> Result<HirModule, Diagnostic> {
    let mut aliases: Vec<(String, Ty)> = Vec::new();
    let mut imports: Vec<ImportBinding> = Vec::new();
    let mut class_defs: Vec<(String, HirClassDef)> = Vec::new();
    let mut items = Vec::with_capacity(module.body.len());
    for stmt in &module.body {
        if let Some((name, ty)) = lower_type_alias_stmt(stmt, &aliases)? {
            // D-068 review finding on #385, second round: the class-vs-alias
            // check below (at the `Stmt::ClassDef` arm) only ever catches a
            // class defined *after* a same-named alias -- without this
            // check, `class Foo: ...` followed by `type Foo = int` would
            // silently establish a second, alias-shaped `Foo` binding with
            // no diagnostic, the exact failure mode this finding exists to
            // close, just in the untreated direction.
            if class_defs.iter().any(|(class_name, _)| *class_name == name) {
                return Err(unsupported(
                    format!(
                        "type alias `{name}` collides with a class of the same name \
                         already defined in this module"
                    ),
                    pycc_ast::stmt_range(stmt),
                ));
            }
            aliases.push((name, ty));
            continue;
        }
        if let Some((name, ty)) = lower_legacy_type_alias_ann_assign(stmt, &aliases)? {
            // Same reverse-direction check as the `type X = ...` arm above,
            // for the legacy `X: TypeAlias = <expr>` spelling.
            if class_defs.iter().any(|(class_name, _)| *class_name == name) {
                return Err(unsupported(
                    format!(
                        "type alias `{name}` collides with a class of the same name \
                         already defined in this module"
                    ),
                    pycc_ast::stmt_range(stmt),
                ));
            }
            aliases.push((name, ty));
            continue;
        }
        if let Some(mut bound) = lower_import_stmt(stmt)? {
            // Same reverse-direction check as the two type-alias arms above,
            // for `import ...`/`from ... import ...` (a single statement can
            // bind more than one local name, e.g. `from math import sqrt,
            // pi`, so every bound name is checked, not just the first).
            if let Some(colliding) = bound.iter().map(import_local_name).find(|local_name| {
                class_defs
                    .iter()
                    .any(|(class_name, _)| class_name == local_name)
            }) {
                return Err(unsupported(
                    format!(
                        "import `{colliding}` collides with a class of the same name \
                         already defined in this module"
                    ),
                    pycc_ast::stmt_range(stmt),
                ));
            }
            imports.append(&mut bound);
            continue;
        }
        if let Stmt::ClassDef(def) = stmt {
            let (class_def, mut method_items) = class::lower_class(def, &aliases, &class_defs)?;
            // D-154 Part 1's own post-merge review finding: two module-level
            // classes sharing a name would each lower their own `__init__`
            // (and any other same-named method) to the identical mangled
            // `<Name>.<method>` function name, silently colliding in
            // `HirModule::items`/`class_defs`'s `HashMap`-collected class
            // table downstream (`pycc_types::Environment::classes`,
            // `pycc_mir`'s own `classes` map) rather than producing a clean
            // diagnostic -- reject it here, at the same point `lower_class`'s
            // own duplicate-method check (`crates/pycc_hir/src/class.rs`)
            // fires for the identical shape one level down.
            if class_defs.iter().any(|(name, _)| name == &class_def.name) {
                return Err(unsupported(
                    format!(
                        "class `{}` is defined more than once in this module",
                        class_def.name
                    ),
                    def.range,
                ));
            }
            // D-068 review finding on #385: a class name colliding with an
            // already-defined top-level function, type alias, or import
            // name produced no diagnostic and silently, permanently
            // shadowed the earlier binding -- `pycc_types::Environment`
            // checks `env.lookup_class(callee)` before the ordinary
            // function lookup at every call site (`crates/pycc_types/src/
            // lib.rs`), on the (until now unenforced) assumption that a
            // class name can never collide with a real function name in
            // this compiler's flat, single-namespace model. Enforce that
            // assumption here, at the same point the class-vs-class check
            // above already fires, rather than leaving it merely asserted
            // in a comment one crate over. Only a top-level function name
            // is checked against `items` (a method's own mangled
            // `<ClassName>.<method>` name can never collide with a bare
            // class name -- a real Python `NAME` token can never contain a
            // `.`, `pycc_hir::class`'s own doc comment).
            if items.iter().any(
                |item| matches!(item, HirItem::Function { name, .. } if *name == class_def.name),
            ) {
                return Err(unsupported(
                    format!(
                        "class `{}` collides with a function of the same name already \
                         defined in this module",
                        class_def.name
                    ),
                    def.range,
                ));
            }
            if aliases.iter().any(|(name, _)| name == &class_def.name) {
                return Err(unsupported(
                    format!(
                        "class `{}` collides with a type alias of the same name already \
                         defined in this module",
                        class_def.name
                    ),
                    def.range,
                ));
            }
            if imports
                .iter()
                .any(|binding| import_local_name(binding) == class_def.name)
            {
                return Err(unsupported(
                    format!(
                        "class `{}` collides with an import of the same name already \
                         defined in this module",
                        class_def.name
                    ),
                    def.range,
                ));
            }
            class_defs.push((class_def.name.clone(), class_def));
            items.append(&mut method_items);
            continue;
        }
        if let Stmt::FunctionDef(def) = stmt
            && class_defs.iter().any(|(name, _)| name == def.name.as_str())
        {
            // The reverse direction of the check above: a top-level
            // function defined *after* a same-named class must be rejected
            // too, not only a class defined after a same-named function.
            return Err(unsupported(
                format!(
                    "function `{}` collides with a class of the same name already \
                     defined in this module",
                    def.name
                ),
                def.range,
            ));
        }
        let item = match stmt {
            Stmt::FunctionDef(def) => lower_function(def, &aliases)?,
            other => {
                HirItem::TopLevelStmt(stmt::lower_stmt(other, &aliases, false, false, None, None)?)
            }
        };
        items.push(item);
    }
    Ok(HirModule {
        items,
        type_aliases: aliases,
        imports,
        class_defs,
    })
}

/// Recognizes a module-level `Stmt::Import`/`Stmt::ImportFrom` and resolves
/// it against `pycc_std`'s registry (D-136/D-137). Returns `Ok(None)` for
/// any other statement kind, leaving it to the caller's own dispatch --
/// mirroring `lower_type_alias_stmt`'s shape exactly.
///
/// D-137 is fail-closed: every recognized-but-out-of-scope shape (multiple
/// names in one `import` statement, an `as` alias, a relative import, an
/// unresolvable module) is `C0001`, the same generic "statement kind not
/// supported yet" diagnostic this file already uses for every other
/// unimplemented statement kind -- matching the plan's explicit instruction
/// to reuse `C0001` rather than add a new code for "we recognize this is an
/// import but don't support this particular shape." A recognized module
/// with one unresolvable symbol inside an otherwise-valid `from math import
/// ...` list is instead `C0002` (D-136's own decision text), distinguishing
/// "we don't support this import shape at all" from "we support `math`,
/// just not `math.<this-symbol>`" -- and it fails the whole statement, not
/// a partial bind of the names that did resolve.
fn lower_import_stmt(stmt: &Stmt) -> Result<Option<Vec<ImportBinding>>, Diagnostic> {
    match stmt {
        Stmt::Import(import) => {
            let [alias] = import.names.as_slice() else {
                return Err(unsupported(
                    "only a single module per `import` statement is supported so far",
                    import.range,
                ));
            };
            if alias.asname.is_some() {
                return Err(unsupported(
                    "`import ... as ...` aliasing is not supported yet",
                    import.range,
                ));
            }
            let module_name = alias.name.as_str();
            let Some(module) = pycc_std::resolve_module(module_name) else {
                return Err(unsupported(
                    format!("import of module `{module_name}` is not supported yet"),
                    import.range,
                ));
            };
            Ok(Some(vec![ImportBinding::Module {
                local_name: module_name.to_string(),
                module,
            }]))
        }
        Stmt::ImportFrom(import) => {
            if import.level != 0 {
                return Err(unsupported(
                    "a relative import (`from . import ...`) is not supported yet",
                    import.range,
                ));
            }
            // A `level == 0` `Stmt::ImportFrom` always carries a module name
            // -- the only way to reach `module: None` is a relative import
            // (`from . import x`, `from .. import x`, ...), which always
            // has `level >= 1` and is already rejected above. Verified
            // directly against the vendored parser: `from import x` (no
            // dots, no module name) is a parse error (`L0001`, "Expected a
            // module name"), so `lower_checked` never sees this shape at
            // all, matching this file's existing precedent of verifying an
            // "impossible" shape against the real parser rather than
            // assuming it.
            let module_name = import
                .module
                .as_ref()
                .expect("a non-relative `from ... import ...` always names a module")
                .as_str();
            let Some(module) = pycc_std::resolve_module(module_name) else {
                return Err(unsupported(
                    format!("import of module `{module_name}` is not supported yet"),
                    import.range,
                ));
            };
            if import.names.is_empty()
                || import.names.iter().any(|alias| alias.name.as_str() == "*")
            {
                return Err(unsupported(
                    "`from ... import *` (wildcard import) is not supported yet",
                    import.range,
                ));
            }
            let mut bound = Vec::with_capacity(import.names.len());
            for alias in &import.names {
                if alias.asname.is_some() {
                    return Err(unsupported(
                        "`from ... import x as y` aliasing is not supported yet",
                        import.range,
                    ));
                }
                let symbol_name = alias.name.as_str();
                let Some(symbol) = pycc_std::resolve_symbol(module, symbol_name) else {
                    return Err(unresolved_symbol(
                        format!(
                            "module `{module_name}` has no importable symbol named `{symbol_name}`"
                        ),
                        import.range,
                    ));
                };
                bound.push(ImportBinding::Symbol {
                    local_name: symbol_name.to_string(),
                    module,
                    symbol,
                });
            }
            Ok(Some(bound))
        }
        _ => Ok(None),
    }
}

/// Recognizes a PEP 695 `type X = <expr>` statement and evaluates its RHS as
/// a type expression, reusing `annotation_to_ty` (D-135) -- the same
/// resolver used for parameter/return/variable annotations, since a type
/// alias's RHS is syntactically just another type expression. Returns
/// `Ok(None)` for any other statement kind, leaving it to the caller's own
/// dispatch.
///
/// A generic alias (`type X[T] = ...`) is rejected with `T0042`, not the
/// generic `unsupported`/`C0001` catch-all: D-134/D-135 explicitly scope a
/// generic alias out of this PR, but -- unlike, say, `async def`, which is
/// simply unrecognized syntax -- this shape *is* recognized and type-checked
/// far enough to name precisely why it is rejected, the same reasoning
/// `check_generic_function`'s own `T0042` diagnostics already use for a
/// generic function's out-of-scope shapes.
fn lower_type_alias_stmt(
    stmt: &Stmt,
    aliases: &[(String, Ty)],
) -> Result<Option<(String, Ty)>, Diagnostic> {
    let Stmt::TypeAlias(type_alias) = stmt else {
        return Ok(None);
    };
    // `type_alias.type_params` being `Some(_)` at all is enough to reject:
    // `ruff_python_parser`'s own `parse_type_params` reports a parse error
    // (`EmptyTypeParams`, surfaced by this crate's own `pycc_parser::parse`
    // as `L0001` before this function ever runs) for an empty `[]`, so a
    // `Some(type_params)` reaching this point always has at least one entry
    // -- there is no valid parsed input where an extra `.type_params.is_empty()`
    // check here would ever be reached with a `false` result to skip on
    // (confirmed against the pinned `ruff_python_parser = "0.0.6"` registry
    // source, the same way this function's own name-target extraction below
    // documents its own unreachable shape).
    if type_alias.type_params.is_some() {
        let range = std::ops::Range::<u32>::from(type_alias.range);
        return Err(Diagnostic::error(
            "T0042",
            "a generic type alias (`type X[T] = ...`) is not supported yet".to_string(),
            Span::new(range.start, range.end),
        ));
    }
    // Unlike the legacy `AnnAssign` form's target (which can be an
    // `Attribute`/`Subscript`, see `lower_legacy_type_alias_ann_assign`
    // below), `ruff_python_parser`'s own `parse_type_alias_statement`
    // unconditionally builds this field as `Expr::Name(self.parse_name(...))`
    // -- there is no valid source text that parses a `type` statement with a
    // non-name target, so there is no `unsupported`/unreachable fallback
    // branch to write or cover here (confirmed against the pinned
    // `ruff_python_parser = "0.0.6"` registry source). `.expect(...)`, not a
    // hand-rolled panic arm, per this crate's own documented coverage
    // convention (`pycc_ast::re_exported_grammar_types_resolve_and_have_the_expected_shape`'s
    // comment): the panic path lives in libcore, invisible to instrumented
    // regions, the same way `.unwrap()`'s does.
    let name = type_alias
        .name
        .as_name_expr()
        .expect("ruff always parses a `type` statement's name as Expr::Name");
    let ty = annotation_to_ty(&type_alias.value, None, None, aliases)?;
    Ok(Some((name.id.to_string(), ty)))
}

/// Recognizes the legacy `X: TypeAlias = <expr>` annotated-assignment form
/// of a type alias (PEP 613). Real Python requires `from typing import
/// TypeAlias` before this annotation is meaningful, but requiring that
/// import here is not merely inconsistent with existing precedent -- it is
/// currently infeasible: `pycc_hir` has no `Stmt::Import`/`Stmt::ImportFrom`
/// handling anywhere in this crate, so `from typing import TypeAlias` would
/// itself be unconditionally rejected with the generic `C0001` ("statement
/// kind not supported yet") diagnostic if pycc tried to require it first.
/// There is no accepted-bare-typing-name precedent to lean on either --
/// `Any` is the only other typing-shaped bare name `annotation_to_ty`
/// currently recognizes, and it is rejected with `T0002`, not accepted. So
/// this function accepts the bare annotation name `TypeAlias`
/// unconditionally, not by analogy to an existing precedent, but because
/// real import verification cannot be expressed with this crate's current
/// statement coverage (plan-deviation note, since the design doc leaves
/// this specific question open; import support is PR-14's).
///
/// Returns `Ok(None)` for any statement that is not this exact shape --
/// including an ordinary `X: TypeAlias` with no value, which is invalid as a
/// type alias and instead falls through to the ordinary `AnnAssign` lowering
/// path, where `annotation_to_ty` rejects the bare name `TypeAlias` with the
/// same `C0001` catch-all as any other unrecognized annotation name.
fn lower_legacy_type_alias_ann_assign(
    stmt: &Stmt,
    aliases: &[(String, Ty)],
) -> Result<Option<(String, Ty)>, Diagnostic> {
    let Stmt::AnnAssign(ann) = stmt else {
        return Ok(None);
    };
    let Expr::Name(annotation_name) = ann.annotation.as_ref() else {
        return Ok(None);
    };
    if annotation_name.id.as_str() != "TypeAlias" {
        return Ok(None);
    }
    let Some(value) = ann.value.as_deref() else {
        return Ok(None);
    };
    let Expr::Name(target) = ann.target.as_ref() else {
        return Ok(None);
    };
    let ty = annotation_to_ty(value, None, None, aliases)?;
    Ok(Some((target.id.to_string(), ty)))
}

fn lower_function(
    def: &pycc_ast::StmtFunctionDef,
    aliases: &[(String, Ty)],
) -> Result<HirItem, Diagnostic> {
    if def.is_async {
        return Err(unsupported(
            "async functions are not supported yet",
            def.range,
        ));
    }
    if !def.decorator_list.is_empty() {
        return Err(unsupported(
            "function decorators are not supported yet",
            def.range,
        ));
    }
    let type_param: Option<Box<str>> = match def.type_params.as_deref() {
        None => None,
        Some(type_params) => match type_params.type_params.as_slice() {
            [single] => Some(type_param_name(single, def.range)?.into()),
            _ => {
                return Err(unsupported(
                    "generic functions with more than one type parameter are not supported yet",
                    def.range,
                ));
            }
        },
    };
    let is_public = !def.name.as_str().starts_with('_'); // D-038
    let params = lower_params(
        &def.parameters,
        is_public,
        def.name.as_str(),
        type_param.as_deref(),
        aliases,
    )?;
    let return_ty = lower_return_annotation(
        def.returns.as_deref(),
        is_public,
        def.name.as_str(),
        type_param.as_deref(),
        None,
        aliases,
    )?;
    let body = stmt::lower_body(&def.body, aliases, false, true, None, type_param.as_deref())?;
    Ok(HirItem::Function {
        name: def.name.to_string(),
        params,
        return_ty,
        body,
    })
}

/// Extracts a PEP 695 `TypeVar`'s identifier -- e.g. the `T` in `def
/// f[T](...)`. `Ty::Param` (D-133) is resolved by call-site substitution
/// (D-134) into one concrete scalar type per call, which is only a coherent
/// model for a plain `TypeVar`: `TypeVarTuple` (`def f[*Ts](...)`) stands for
/// a variable-length sequence of types, and `ParamSpec` (`def f[**P](...)`)
/// stands for a parameter list shape, neither of which `Ty::Param` can
/// represent. `def_range` is the enclosing function's range, reused for the
/// diagnostic span since `TypeParam`'s own range would require reaching past
/// the `pycc_ast` facade for the `Ranged` trait for no benefit here (the
/// arity-gate rejection just above already reports the same function-level
/// span for the analogous "too many type parameters" case).
pub(crate) fn type_param_name<R>(
    type_param: &pycc_ast::TypeParam,
    def_range: R,
) -> Result<&str, Diagnostic>
where
    std::ops::Range<u32>: From<R>,
{
    match type_param {
        pycc_ast::TypeParam::TypeVar(tv) => Ok(tv.name.as_str()),
        pycc_ast::TypeParam::TypeVarTuple(_) => Err(unsupported(
            "a `TypeVarTuple` type parameter (`*Ts`) is not supported yet",
            def_range,
        )),
        pycc_ast::TypeParam::ParamSpec(_) => Err(unsupported(
            "a `ParamSpec` type parameter (`**P`) is not supported yet",
            def_range,
        )),
    }
}

fn lower_params(
    parameters: &pycc_ast::Parameters,
    is_public: bool,
    fn_name: &str,
    type_param: Option<&str>,
    aliases: &[(String, Ty)],
) -> Result<Vec<(String, Ty)>, Diagnostic> {
    // Every parameter kind and default value below is silently absent from
    // `parameters.args`/`ParameterWithDefault::default` -- an earlier version
    // of this function only ever iterated `.args` and never checked for any
    // of these, so a function using them got a wrong signature built from
    // whatever plain positional args happened to exist, instead of the
    // explicit capability diagnostic every other out-of-scope construct in
    // this file produces (self-review finding, pre-merge).
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
    lower_arg_list(
        &parameters.args,
        is_public,
        fn_name,
        type_param,
        None,
        aliases,
    )
}

/// Lowers a plain positional-parameter list (no `/`/`*`/`**`/keyword-only
/// markers -- callers are responsible for rejecting those first, since their
/// diagnostics differ by caller: `lower_params` reports them against a
/// top-level function's own `parameters`, `class::lower_method` (D-154, Part
/// 1 of #375) reports the identical checks against a method's `parameters`,
/// which also includes the leading `self` parameter that helper strips
/// before delegating here). Factored out of `lower_params` (which still owns
/// every top-level function's own shape validation, unchanged) so both
/// callers share this one per-parameter annotation-resolution rule instead
/// of duplicating it.
pub(crate) fn lower_arg_list(
    args: &[pycc_ast::ParameterWithDefault],
    is_public: bool,
    fn_name: &str,
    type_param: Option<&str>,
    class_name: Option<&str>,
    aliases: &[(String, Ty)],
) -> Result<Vec<(String, Ty)>, Diagnostic> {
    args.iter()
        .map(|param| {
            if param.default.is_some() {
                return Err(unsupported(
                    "default parameter values are not supported yet",
                    param.range,
                ));
            }
            let name = param.parameter.name.as_str();
            match &param.parameter.annotation {
                Some(ann) => Ok((
                    name.to_string(),
                    annotation_to_ty(ann, type_param, class_name, aliases)?,
                )),
                None if is_public => Err(Diagnostic::error(
                    "T0001",
                    format!(
                        "parameter `{name}` of public function `{fn_name}` needs a type annotation"
                    ),
                    Span::new(0, 0),
                )
                .with_help(format!("add a type annotation to parameter `{name}`"))),
                None => Ok((name.to_string(), Ty::Infer)),
            }
        })
        .collect()
}

fn lower_return_annotation(
    returns: Option<&Expr>,
    is_public: bool,
    fn_name: &str,
    type_param: Option<&str>,
    class_name: Option<&str>,
    aliases: &[(String, Ty)],
) -> Result<Ty, Diagnostic> {
    match returns {
        Some(ann) => annotation_to_ty(ann, type_param, class_name, aliases),
        None if is_public => Err(Diagnostic::error(
            "T0001",
            format!("public function `{fn_name}` needs a return type annotation"),
            Span::new(0, 0),
        )
        .with_help(format!("add a return type annotation to `{fn_name}`"))),
        None => Ok(Ty::Infer),
    }
}

/// Resolves an annotation expression to a `Ty`. `aliases` is the D-135 type
/// alias table (`(name, Ty)` pairs recorded by `lower_checked` for every
/// `type X = ...`/legacy `X: TypeAlias = ...` statement reached so far, in
/// source order): checked as the last resort for a bare name before falling
/// through to the `C0001` "not supported yet" catch-all, so an alias name
/// resolves exactly like any other recognized bare-name annotation.
///
/// `class_name` is the enclosing class's name when lowering a method's
/// annotations (PEP 673 `Self` and PEP 649/749 self-referential deferred
/// annotations, #387): `Some(name)` makes both `"Self"` and the class's own
/// name resolve to `Ty::Instance(Box::new(name))` — the same type `self`
/// has. `None` for top-level functions and all other annotation contexts
/// (module-level `AnnAssign`, type aliases), where `"Self"` and a bare class
/// name remain unrecognized (C0001), matching CPython's own scope rule that
/// `Self` is only valid inside a class body.
fn annotation_to_ty(
    annotation: &Expr,
    type_param: Option<&str>,
    class_name: Option<&str>,
    aliases: &[(String, Ty)],
) -> Result<Ty, Diagnostic> {
    match annotation {
        Expr::NoneLiteral(_) => Ok(Ty::None),
        Expr::Name(name) if Some(name.id.as_str()) == type_param => {
            Ok(Ty::Param(Box::new(name.id.to_string())))
        }
        // PEP 673 (#387): `Self` inside a class method's annotation resolves
        // to the enclosing class's instance type — the same type `self` has.
        // Outside a class (`class_name` is `None`), `"Self"` falls through to
        // the alias/C0001 path below, matching CPython's own scoping rule.
        Expr::Name(name) if name.id.as_str() == "Self" && class_name.is_some() => {
            Ok(Ty::Instance(Box::new(class_name.unwrap().to_string())))
        }
        // PEP 649/749 (#387): a method's return-type annotation may reference
        // the enclosing class's own name (self-referential deferred
        // annotation, e.g. `class Node: def next(self) -> Node: ...`). Inside
        // a class body (`class_name` is `Some`), the class's own name resolves
        // to `Ty::Instance(class_name)`. This is specifically for the
        // self-referential case — cross-class references are not in scope.
        Expr::Name(name) if Some(name.id.as_str()) == class_name => {
            Ok(Ty::Instance(Box::new(name.id.to_string())))
        }
        Expr::Name(name) => match name.id.as_str() {
            "int" => Ok(Ty::Int),
            "float" => Ok(Ty::Float),
            "bool" => Ok(Ty::Bool),
            "str" => Ok(Ty::Str),
            "Any" => Err(Diagnostic::error(
                "T0002",
                "`Any` is not permitted in pycc code outside a declared interop boundary"
                    .to_string(),
                Span::new(0, 0),
            )),
            other => aliases
                .iter()
                .rev()
                .find(|(alias_name, _)| alias_name == other)
                .map(|(_, ty)| ty.clone())
                .ok_or_else(|| {
                    unsupported(
                        format!("type annotation `{other}` is not supported yet"),
                        pycc_ast::expr_range(annotation),
                    )
                }),
        },
        // Issue #435 (Part D, __class_getitem__): `ClassName[type_arg]` as a
        // type annotation (PEP 560). A class that defines `__class_getitem__`
        // allows subscript syntax in annotations. In pycc's static type
        // system, this resolves to `Ty::Instance(ClassName)` — the class
        // itself, ignoring the type argument for now (consistent with how
        // generic classes are handled by PEP 695's `GenericClassInstantiate`
        // for actual instantiation, not annotation). The base must be a bare
        // name (a class name); any other subscript shape is rejected.
        Expr::Subscript(sub) => {
            let Expr::Name(base_name) = sub.value.as_ref() else {
                return Err(unsupported(
                    "a subscripted type annotation's base must be a bare class name",
                    pycc_ast::expr_range(&sub.value),
                ));
            };
            // The class name resolves the same way a bare-name annotation
            // does — through the alias table or as a known class name. We
            // reuse `annotation_to_ty` on the bare name so self-referential
            // class names, aliases, and builtin types all resolve identically.
            annotation_to_ty(
                &Expr::Name(base_name.clone()),
                type_param,
                class_name,
                aliases,
            )
        }
        other => Err(unsupported(
            format!("only a bare name type annotation is supported so far: {other:?}"),
            pycc_ast::expr_range(other),
        )),
    }
}

fn unsupported<R>(message: impl Into<String>, range: R) -> Diagnostic
where
    std::ops::Range<u32>: From<R>,
{
    let range = std::ops::Range::<u32>::from(range);
    Diagnostic::error("C0001", message, Span::new(range.start, range.end))
}

/// `C0002`: "stdlib symbol not supported yet" (D-136) -- distinct from
/// `C0001`. Used only when the *module* of a `from ... import ...`
/// statement is recognized but a specific imported name inside it is not
/// registered (e.g. `from math import isnan`), as opposed to `C0001`'s
/// "we don't recognize this import shape/module at all."
fn unresolved_symbol<R>(message: impl Into<String>, range: R) -> Diagnostic
where
    std::ops::Range<u32>: From<R>,
{
    let range = std::ops::Range::<u32>::from(range);
    Diagnostic::error("C0002", message, Span::new(range.start, range.end))
}

/// `L0001`: reused (not a new code, D-148) for a post-parse statement- or
/// expression-context violation caught during HIR lowering -- a construct
/// that is syntactically well-formed but only valid Python in a context the
/// enclosing statement or expression isn't in (`break`/`continue` outside a
/// loop, `async for` outside an async function -- D-148; `yield`/`yield
/// from` outside a function -- D-149). CPython itself raises `SyntaxError`
/// for all of these, the same failure class `L0001` already covers for the
/// parser's own grammar violations; unlike a genuine parser `L0001`, one
/// emitted from here carries no "expected set" in its message.
fn context_invalid<R>(message: impl Into<String>, range: R) -> Diagnostic
where
    std::ops::Range<u32>: From<R>,
{
    let range = std::ops::Range::<u32>::from(range);
    Diagnostic::error("L0001", message, Span::new(range.start, range.end))
}

#[cfg(test)]
mod tests {
    use super::*;
    // `lower_comprehension_header`/`rename_name_in_expr` moved to `expr.rs`
    // (issue #361, D-149) but the two tests below call them directly,
    // bypassing the public `lower_checked` entry point -- unlike every other
    // test in this module, so a `super::*` glob import alone does not reach
    // them (it only reaches items defined directly in `lib.rs`, not items
    // re-exported from a sibling module). See `expr.rs`'s own module doc
    // comment for why these two stayed here instead of moving.
    use crate::expr::{lower_comprehension_header, rename_name_in_expr};

    fn assert_capability_error(source: &str, expected_message: &str, expected_span: Span) {
        let module = pycc_parser_test_helper::parse(source);
        let diagnostic = lower_checked(&module).unwrap_err();

        assert_eq!(diagnostic.code, "C0001");
        assert!(diagnostic.message.contains(expected_message));
        assert_eq!(diagnostic.span, Some(expected_span));
    }

    fn assert_capability_error_message(source: &str, expected_message: &str) {
        let module = pycc_parser_test_helper::parse(source);
        let diagnostic = lower_checked(&module).unwrap_err();

        assert_eq!(diagnostic.code, "C0001");
        assert!(diagnostic.message.contains(expected_message));
        assert!(diagnostic.span.is_some());
    }

    /// Sibling of `assert_capability_error_message` for the new `L0001`
    /// context-invalidity diagnostic (issue #141, D-148) -- kept separate
    /// rather than parameterizing the existing helper so every other
    /// existing `C0001` call site stays untouched.
    fn assert_context_invalid_error_message(source: &str, expected_message: &str) {
        let module = pycc_parser_test_helper::parse(source);
        let diagnostic = lower_checked(&module).unwrap_err();

        assert_eq!(diagnostic.code, "L0001");
        assert!(diagnostic.message.contains(expected_message));
        assert!(diagnostic.span.is_some());
    }

    #[test]
    fn ty_name_returns_the_python_spelling_of_every_scalar_variant() {
        // The four recursive container variants (List/Dict/Set/Tuple) are
        // covered separately by `ty_name_describes_nested_container_types`
        // below, since their expected `.name()` output depends on a nested
        // `Ty` argument rather than being a fixed string per variant.
        assert_eq!(Ty::Int.name(), "int");
        assert_eq!(Ty::Float.name(), "float");
        assert_eq!(Ty::Bool.name(), "bool");
        assert_eq!(Ty::Str.name(), "str");
        assert_eq!(Ty::None.name(), "None");
        assert_eq!(Ty::Infer.name(), "<inferred>");
        assert_eq!(Ty::Param(Box::new("T".to_string())).name(), "T");
        assert_eq!(
            Ty::Instance(Box::new("MyClass".to_string())).name(),
            "MyClass"
        );
    }

    #[test]
    fn ty_list_variant_is_structurally_comparable_and_not_copy() {
        let a = Ty::List(Box::new(Ty::Int));
        let b = Ty::List(Box::new(Ty::Int));
        let c = Ty::List(Box::new(Ty::Str));
        assert_eq!(a, b);
        assert_ne!(a, c);
        // `Ty` is `Clone` but deliberately not `Copy` (a `Box`/`Vec`-holding
        // enum can't implement `Copy`), so producing a second owned value
        // from `a` requires this explicit `.clone()` -- unlike the old flat
        // scalar `Ty`, where an implicit copy would have made `.clone()`
        // merely redundant rather than required. (`.clone()` alone can't
        // prove Copy's absence at compile time -- it also compiles, just
        // redundantly, for a `Copy` type -- so this comment documents the
        // property rather than the assertions below enforcing it.)
        let d = a.clone();
        assert_eq!(a, d);
    }

    #[test]
    fn ty_shrinks_after_boxing_dict_and_tuple_d109() {
        // D-109: before this task, size_of::<Ty>() measured 24 bytes (Vec<Ty>'s
        // ptr+len+cap dominates). This is a real regression guard, not a vibe --
        // it must stay strictly smaller than 24 forever, catching any future
        // change that re-inflates Ty back to its pre-fix size. This test's
        // own numeric assertion documents PR-11's variant set specifically
        // (PR-11 itself added no new Ty variants, only the Dict/Tuple boxing
        // fix) -- every later PR that adds a new dataful variant (D-133's
        // `Ty::Param`, D-154's `Ty::Instance`) is covered instead by the
        // more general `ty_size_stays_within_d109_ceiling` test below, which
        // tracks the ceiling for the *current* variant set rather than
        // pinning it to one historical PR's shape.
        assert_eq!(
            std::mem::size_of::<Ty>(),
            16,
            "size_of::<Ty>() must stay 16 bytes (PR-10 Task 14, D-109) -- if it \
             moves, something accidentally widened Ty's boxing, not the containers \
             themselves",
        );
    }

    #[test]
    fn ty_size_stays_within_d109_ceiling() {
        // D-133 added `Ty::Param(Box<String>)`; D-154 added
        // `Ty::Instance(Box<String>)`. Both are a single (thin, 8-byte)
        // pointer -- unlike `Box<str>`, which measured 24 bytes here because
        // `str` is unsized (see each variant's own doc comment) -- so
        // neither pushes `size_of::<Ty>()` back past the 16-byte ceiling
        // D-109 established.
        assert!(
            std::mem::size_of::<Ty>() <= 16,
            "size_of::<Ty>() must stay within the D-109 16-byte ceiling; adding \
             Ty::Param(Box<String>) (D-133) and Ty::Instance(Box<String>) (D-154) \
             must not regress it, got {}",
            std::mem::size_of::<Ty>(),
        );
    }

    #[test]
    fn ty_name_describes_nested_container_types() {
        assert_eq!(Ty::Int.name(), "int");
        assert_eq!(Ty::List(Box::new(Ty::Int)).name(), "list[int]");
        assert_eq!(
            Ty::Dict(Box::new((Ty::Str, Ty::Float))).name(),
            "dict[str, float]"
        );
        assert_eq!(Ty::Set(Box::new(Ty::Bool)).name(), "set[bool]");
        assert_eq!(
            Ty::Tuple(Box::new(vec![Ty::Int, Ty::Str])).name(),
            "tuple[int, str]"
        );
    }

    #[test]
    fn ty_instance_name_is_the_bare_class_name() {
        // Unlike every other dataful variant, `Ty::Instance`'s `.name()`
        // is not wrapped in a `<kind>[...]` shape -- a class instance's
        // type is spelled exactly like the class itself in real Python
        // (`Point`, not `Instance[Point]`).
        assert_eq!(Ty::Instance(Box::new("Point".to_string())).name(), "Point");
    }

    #[test]
    fn unsupported_statement_and_expression_return_spanned_capability_diagnostics() {
        // #435: `pass` is now supported, so use `with` — a valid Python
        // statement that is still unsupported — to exercise the C0001
        // capability error path for statements.
        assert_capability_error(
            "with open(\"x\") as f:\n    pass\n",
            "statement kind not supported yet",
            Span::new(0, 29),
        );
        assert_capability_error(
            "x = lambda: 1\n",
            "expression kind not supported yet",
            Span::new(4, 13),
        );
    }

    #[test]
    fn capability_errors_propagate_through_every_supported_container() {
        // `(1, 2)` was this table's "genuinely unhandled at every level"
        // poison fixture -- a list literal used to fill this role (see
        // `a_tuple_literal_expression_lowers_successfully`'s own comment)
        // until Task 7 (D-105) added list-literal lowering, and a tuple
        // literal took over in turn until this task (PR-11b Task 2, D-116)
        // added tuple-literal lowering too. `lambda: 1` (parenthesized
        // throughout, purely to dodge grammar ambiguity with the
        // surrounding syntax -- not because any position here actually
        // requires it) takes over now, since `lower_expr` still has no
        // `Expr::Lambda` arm.
        let cases = [
            // #435: `pass` is now supported (filtered as a no-op in
            // `lower_body`), so use `with open("x") as f: pass` — a valid
            // Python statement that is still unsupported — to exercise the
            // C0001 capability error path in statement positions.
            (
                "function body",
                "def _f():\n    with open(\"x\") as f:\n        pass\n",
            ),
            ("if test", "if (lambda: 1):\n    print(1)\n"),
            (
                "if else body",
                "if True:\n    print(1)\nelse:\n    with open(\"x\") as f:\n        pass\n",
            ),
            ("while test", "while (lambda: 1):\n    print(1)\n"),
            (
                "while body",
                "while True:\n    with open(\"x\") as f:\n        pass\n",
            ),
            (
                "one-argument range stop",
                "for i in range((lambda: 1)):\n    print(i)\n",
            ),
            (
                "two-argument range start",
                "for i in range((lambda: 1), 1):\n    print(i)\n",
            ),
            (
                "two-argument range stop",
                "for i in range(0, (lambda: 1)):\n    print(i)\n",
            ),
            (
                "three-argument range start",
                "for i in range((lambda: 1), 1, 1):\n    print(i)\n",
            ),
            (
                "three-argument range stop",
                "for i in range(0, (lambda: 1), 1):\n    print(i)\n",
            ),
            (
                "three-argument range step",
                "for i in range(0, 1, (lambda: 1)):\n    print(i)\n",
            ),
            (
                "for body",
                "for i in range(1):\n    with open(\"x\") as f:\n        pass\n",
            ),
            ("return value", "def _f():\n    return (lambda: 1)\n"),
            (
                "elif test",
                "if True:\n    print(1)\nelif (lambda: 1):\n    print(2)\n",
            ),
            (
                "elif body",
                "if True:\n    print(1)\nelif True:\n    with open(\"x\") as f:\n        pass\n",
            ),
            (
                "nested else body",
                "if True:\n    print(1)\nelif True:\n    print(2)\nelse:\n    with open(\"x\") as f:\n        pass\n",
            ),
            ("binary left operand", "x = (lambda: 1) + 1\n"),
            ("binary right operand", "x = 1 + (lambda: 1)\n"),
            ("f-string interpolation", "x = f\"{(lambda: 1)}\"\n"),
            ("comparison left operand", "x = (lambda: 1) == 1\n"),
            ("comparison right operand", "x = 1 == (lambda: 1)\n"),
        ];

        for (container, source) in cases {
            let module = pycc_parser_test_helper::parse(source);
            let diagnostic = lower_checked(&module).unwrap_err();
            assert_eq!(diagnostic.code, "C0001", "wrong diagnostic for {container}");
            assert!(
                diagnostic
                    .message
                    .contains("expression kind not supported yet")
                    || diagnostic
                        .message
                        .contains("statement kind not supported yet"),
                "wrong message for {container}: {}",
                diagnostic.message
            );
            assert!(diagnostic.span.is_some(), "missing span for {container}");
        }
    }

    #[test]
    fn lowers_a_function_definition_without_calling_it() {
        // Defining `main` alone has no observable effect -- matches
        // CPython exactly (confirmed empirically: `python3.14 hello.py`
        // on this exact source prints nothing). Only an explicit call
        // (see the next test) makes it run.
        let module = pycc_parser_test_helper::parse("def main() -> None:\n    print(42)\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::Function {
                name: "main".to_string(),
                params: vec![],
                return_ty: Ty::None,
                body: vec![HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::IntLiteral(42)],
                })],
            }]
        );
    }

    #[test]
    fn lowers_a_call_to_a_user_defined_function() {
        let module =
            pycc_parser_test_helper::parse("def main() -> None:\n    print(42)\n\nmain()\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![
                HirItem::Function {
                    name: "main".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![HirStmt::ExprStmt(HirExpr::Call {
                        callee: "print".to_string(),
                        args: vec![HirExpr::IntLiteral(42)],
                    })],
                },
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "main".to_string(),
                    args: vec![],
                })),
            ]
        );
    }

    #[test]
    fn lowers_top_level_print_with_no_main() {
        let module = pycc_parser_test_helper::parse("print(42)\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::IntLiteral(42)],
            }))]
        );
    }

    #[test]
    fn lowers_a_name_reference_used_as_a_call_argument() {
        // Exercises HirExpr::Name specifically -- every other test so far
        // only ever passes an IntLiteral or zero args to a call, never a
        // bare name reference used as a *value* (as opposed to an
        // assignment target, which Task 6 handles separately).
        let module = pycc_parser_test_helper::parse("def f() -> None:\n    print(x)\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: Ty::None,
                body: vec![HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::Name("x".to_string())],
                })],
            }]
        );
    }

    #[test]
    fn a_bare_boolean_literal_expression_is_now_supported() {
        let module = pycc_parser_test_helper::parse("True\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(
                HirExpr::BoolLiteral(true)
            ))]
        );
    }

    #[test]
    fn lowers_an_assignment_and_a_later_reference_to_it() {
        let module = pycc_parser_test_helper::parse("x = 1\nprint(x)\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(1),
                }),
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::Name("x".to_string())],
                })),
            ]
        );
    }

    #[test]
    fn lowers_a_binary_addition() {
        let module = pycc_parser_test_helper::parse("x = 1 + 2\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(HirExpr::IntLiteral(1)),
                    right: Box::new(HirExpr::IntLiteral(2)),
                },
            })]
        );
    }

    #[test]
    fn lowers_every_arithmetic_operator() {
        let cases = [
            ("x = 1 - 2\n", BinOpKind::Sub),
            ("x = 1 * 2\n", BinOpKind::Mul),
            ("x = 1 / 2\n", BinOpKind::Div),
            ("x = 1 // 2\n", BinOpKind::FloorDiv),
            ("x = 1 % 2\n", BinOpKind::Mod),
            ("x = 1 ** 2\n", BinOpKind::Pow),
        ];
        for (source, expected_op) in cases {
            let module = pycc_parser_test_helper::parse(source);
            let hir = lower_checked(&module).unwrap();
            assert_eq!(
                hir.items,
                vec![HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::BinOp {
                        op: expected_op,
                        left: Box::new(HirExpr::IntLiteral(1)),
                        right: Box::new(HirExpr::IntLiteral(2)),
                    },
                })],
                "wrong lowering for {source:?}"
            );
        }
    }

    #[test]
    fn lowers_a_float_literal() {
        let module = pycc_parser_test_helper::parse("x = 1.5\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::FloatLiteral(1.5),
            })]
        );
    }

    #[test]
    fn a_multi_target_assignment_is_unsupported() {
        assert_capability_error_message(
            "x = y = 1\n",
            "only a single assignment target is supported so far",
        );
    }

    #[test]
    fn assigning_to_an_attribute_target_lowers_to_attr_set() {
        // D-154 (Part 1 of #375) supersedes this test's own former
        // "`x.attr = 1` is unsupported" invariant: attribute-assignment
        // targets are now structurally recognized (`HirStmt::AttrSet`) for
        // every class method's own `self.<attr> = ...` writes, and -- since
        // this lowering step has no type information, mirroring
        // `HirStmt::DictSet`'s own bare-name-base precedent -- for any other
        // base expression too. `pycc_types` rejects a base that isn't
        // actually a class instance, or an attribute name the base's class
        // never declares.
        let module = pycc_parser_test_helper::parse("x.attr = 1\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::AttrSet {
                base: HirExpr::Name("x".to_string()),
                attr: "attr".to_string(),
                value: HirExpr::IntLiteral(1),
            })]
        );
    }

    #[test]
    fn assigning_to_an_attribute_target_propagates_an_unsupported_base_expression() {
        // Exercises the `?` inside `Stmt::Assign`'s own `Expr::Attribute`
        // arm's `base` lowering (D-154), mirroring
        // `method_call_propagates_an_unsupported_base_expression` above.
        let module = pycc_parser_test_helper::parse("(1j).attr = 1\n");
        let diagnostic = lower_checked(&module).unwrap_err();

        assert_eq!(diagnostic.code, "C0001");
    }

    #[test]
    fn assigning_to_a_tuple_unpacking_target_is_unsupported() {
        // The remaining assignment-target shape this file still rejects
        // after `Expr::Name`/`Expr::Subscript`/`Expr::Attribute` are all
        // now recognized: multi-target unpacking (`a, b = ...`) has no HIR
        // shape at all yet.
        assert_capability_error_message(
            "a, b = 1, 2\n",
            "only assigning to a bare name is supported so far",
        );
    }

    #[test]
    fn subscript_assignment_to_a_bare_name_base_lowers_to_dict_set() {
        // PR-11 Task 3 (D-123) supersedes D-105's "no subscript assignment
        // target anywhere in this file" invariant this test used to lock in
        // (`list[int]` alone stayed read-only-indexed; `dict[str, int]`
        // ships `d[k] = v`). This lowering step has no type information (the
        // same reason `ForList`'s own bare-name iterable isn't type-checked
        // here either), so `x[0] = 1` lowers to `HirStmt::DictSet`
        // regardless of whether `x` actually turns out to be a `list` or a
        // `dict` -- `pycc_types` now owns rejecting a `list`-typed base with
        // `T0033` (see that crate's own test module), relocating rather than
        // removing the read-only-list invariant.
        let module = pycc_parser_test_helper::parse("x[0] = 1\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::DictSet {
                dict: "x".to_string(),
                key: HirExpr::IntLiteral(0),
                value: HirExpr::IntLiteral(1),
            })]
        );
    }

    #[test]
    fn subscript_assignment_to_a_non_bare_name_base_is_unsupported() {
        // `f()[0] = 1` has no plain variable name to record as `DictSet`'s
        // own `dict` field -- rejected explicitly rather than guessed at.
        assert_capability_error_message(
            "f()[0] = 1\n",
            "only assigning to a bare-name subscript target (`name[key] = value`) is supported so far",
        );
    }

    #[test]
    fn a_dict_set_target_with_an_unsupported_key_propagates_the_key_error() {
        // (1, 2) no longer fails to lower (this task) -- lambda is still
        // unsupported and exercises the identical propagation path.
        assert_capability_error_message("x[lambda: 1] = 1\n", "expression kind not supported yet");
    }

    #[test]
    fn a_dict_set_target_with_an_unsupported_value_propagates_the_value_error() {
        // (1, 2) no longer fails to lower (this task) -- lambda is still
        // unsupported and exercises the identical propagation path.
        assert_capability_error_message("x[0] = lambda: 1\n", "expression kind not supported yet");
    }

    #[test]
    fn matrix_multiplication_is_unsupported() {
        assert_capability_error_message("x = a @ b\n", "binary operator not supported yet");
    }

    #[test]
    fn a_with_statement_is_unsupported() {
        // `with` is valid Python but not implemented — it exercises the
        // same catch-all that
        // `unsupported_statement_and_expression_return_spanned_capability_diagnostics`
        // does for expressions. (#435: `pass` was previously used here but
        // is now supported as a no-op for PEP 487 hook bodies.)
        assert_capability_error_message(
            "with open(\"x\") as f:\n    pass\n",
            "statement kind not supported yet",
        );
    }

    #[test]
    fn a_bare_literal_expression_statement_is_now_supported() {
        // `42` alone at module level is legal (if pointless) Python -- an
        // expression statement whose value is simply discarded. The old
        // HIR shape only ever represented a *call* expression statement,
        // so this used to panic; HirExpr::IntLiteral now represents any
        // expression, not just call arguments.
        let module = pycc_parser_test_helper::parse("42\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(
                HirExpr::IntLiteral(42)
            ))]
        );
    }

    #[test]
    fn non_name_callee_is_unsupported() {
        // `foo.bar()` no longer reaches this message (Task 7, D-105): a
        // `.` callee is now checked for the `.append()` shape first, and
        // rejected with its own dedicated message when it isn't `.append`
        // (see `calling_a_non_append_method_is_unsupported`). This fixture
        // instead calls the *result* of a call -- neither a bare name nor
        // an attribute access -- to keep exercising the fallback rejection.
        assert_capability_error_message("foo()()\n", "only calling a bare name");
    }

    #[test]
    fn calling_a_zero_arg_function_other_than_print_is_supported() {
        let module = pycc_parser_test_helper::parse("foo()\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                callee: "foo".to_string(),
                args: vec![],
            }))]
        );
    }

    #[test]
    fn calling_a_non_print_function_with_arguments_is_now_supported() {
        // This used to panic in the pre-Task-5 HIR shape (only zero-arg
        // user-function calls were representable at all) -- HirExpr::Call
        // now carries arbitrary args for every callee, print included;
        // real type-checking of a call's arguments against a declared
        // signature is Task 9's job, not this lowering step's.
        let module = pycc_parser_test_helper::parse("foo(42)\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                callee: "foo".to_string(),
                args: vec![HirExpr::IntLiteral(42)],
            }))]
        );
    }

    #[test]
    fn print_with_more_than_one_argument_is_now_supported_at_the_hir_level() {
        // Same rationale as above -- HirExpr::Call no longer special-cases
        // `print`'s arity at lowering time.
        let module = pycc_parser_test_helper::parse("print(1, 2)\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
            }))]
        );
    }

    #[test]
    fn print_with_a_float_argument_is_now_supported_at_the_hir_level() {
        // MIR/codegen still only understands an integer-literal argument to
        // `print` (see pycc_mir::lower_instr); this is HIR-only.
        let module = pycc_parser_test_helper::parse("print(2.5)\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::FloatLiteral(2.5)],
            }))]
        );
    }

    #[test]
    fn print_with_an_integer_too_large_for_i64_is_unsupported() {
        assert_capability_error_message(
            "print(99999999999999999999999999999999)\n",
            "does not fit in i64",
        );
    }

    #[test]
    fn a_complex_number_literal_is_unsupported() {
        // Complex isn't in v0.1's type-representation table (int/float/bool/str/None
        // per TYPE_SYSTEM.md) -- unlike float/bool, this isn't deferred to a later
        // PR-4 task, it's simply out of scope for pycc entirely.
        assert_capability_error_message("x = 3j\n", "numeric literal kind not supported yet");
    }

    #[test]
    fn lowers_a_boolean_literal() {
        let module = pycc_parser_test_helper::parse("x = True\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::BoolLiteral(true),
            })]
        );
    }

    #[test]
    fn lowers_a_single_comparison() {
        let module = pycc_parser_test_helper::parse("x = 1 < 2\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::Compare {
                    op: CmpOpKind::Lt,
                    left: Box::new(HirExpr::IntLiteral(1)),
                    right: Box::new(HirExpr::IntLiteral(2)),
                },
            })]
        );
    }

    #[test]
    fn lowers_every_comparison_operator() {
        let cases = [
            ("x = 1 == 2\n", CmpOpKind::Eq),
            ("x = 1 != 2\n", CmpOpKind::NotEq),
            ("x = 1 < 2\n", CmpOpKind::Lt),
            ("x = 1 <= 2\n", CmpOpKind::LtE),
            ("x = 1 > 2\n", CmpOpKind::Gt),
            ("x = 1 >= 2\n", CmpOpKind::GtE),
        ];
        for (source, expected_op) in cases {
            let module = pycc_parser_test_helper::parse(source);
            let hir = lower_checked(&module).unwrap();
            assert_eq!(
                hir.items,
                vec![HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::Compare {
                        op: expected_op,
                        left: Box::new(HirExpr::IntLiteral(1)),
                        right: Box::new(HirExpr::IntLiteral(2)),
                    },
                })],
                "wrong lowering for {source:?}"
            );
        }
    }

    #[test]
    fn a_chained_comparison_is_not_supported_yet() {
        assert_capability_error_message("x = 1 < 2 < 3\n", "chained comparisons");
    }

    #[test]
    fn an_is_comparison_is_not_supported_yet() {
        assert_capability_error_message("x = 1 is 2\n", "comparison operator not supported yet");
    }

    #[test]
    fn a_tuple_literal_expression_lowers_successfully() {
        // Tuple literals were this file's own "genuinely unhandled at every
        // level" fixture before this task (list/dict/set literals filled
        // that role earlier and became supported in turn). Now that
        // `Expr::Tuple` has a real arm, this asserts the actual shape
        // rather than a lowering failure -- `pycc_types` (not this crate)
        // now owns which element types/index forms are valid (D-116).
        let module = pycc_parser_test_helper::parse("x = (1, 2)\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items[0],
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::TupleLiteral(vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)]),
            })
        );
    }

    #[test]
    fn lowers_an_if_with_no_else() {
        let module = pycc_parser_test_helper::parse("if True:\n    print(1)\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::If {
                test: HirExpr::BoolLiteral(true),
                body: vec![HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::IntLiteral(1)],
                })],
                orelse: vec![],
            })]
        );
    }

    #[test]
    fn lowers_an_if_with_an_else() {
        let module =
            pycc_parser_test_helper::parse("if True:\n    print(1)\nelse:\n    print(2)\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::If {
                test: HirExpr::BoolLiteral(true),
                body: vec![HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::IntLiteral(1)],
                })],
                orelse: vec![HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::IntLiteral(2)],
                })],
            })]
        );
    }

    #[test]
    fn lowers_an_elif_as_a_nested_if_in_orelse() {
        let module = pycc_parser_test_helper::parse(
            "if False:\n    print(1)\nelif True:\n    print(2)\nelse:\n    print(3)\n",
        );
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::If {
                test: HirExpr::BoolLiteral(false),
                body: vec![HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::IntLiteral(1)],
                })],
                orelse: vec![HirStmt::If {
                    test: HirExpr::BoolLiteral(true),
                    body: vec![HirStmt::ExprStmt(HirExpr::Call {
                        callee: "print".to_string(),
                        args: vec![HirExpr::IntLiteral(2)],
                    })],
                    orelse: vec![HirStmt::ExprStmt(HirExpr::Call {
                        callee: "print".to_string(),
                        args: vec![HirExpr::IntLiteral(3)],
                    })],
                }],
            })]
        );
    }

    #[test]
    fn lowers_a_while_loop() {
        let module = pycc_parser_test_helper::parse("while True:\n    print(1)\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::While {
                test: HirExpr::BoolLiteral(true),
                body: vec![HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::IntLiteral(1)],
                })],
            })]
        );
    }

    #[test]
    fn a_while_else_is_not_supported_yet() {
        assert_capability_error_message(
            "while True:\n    print(1)\nelse:\n    print(2)\n",
            "while/else is not supported yet",
        );
    }

    #[test]
    fn lowers_a_for_range_loop_with_one_argument() {
        let module = pycc_parser_test_helper::parse("for i in range(3):\n    print(i)\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::ForRange {
                var: "i".to_string(),
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
                body: vec![HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::Name("i".to_string())],
                })],
            })]
        );
    }

    #[test]
    fn lowers_a_for_range_loop_with_start_and_stop() {
        let module = pycc_parser_test_helper::parse("for i in range(1, 3):\n    print(i)\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::ForRange {
                var: "i".to_string(),
                start: HirExpr::IntLiteral(1),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
                body: vec![HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::Name("i".to_string())],
                })],
            })]
        );
    }

    #[test]
    fn lowers_a_for_range_loop_with_start_stop_and_step() {
        let module = pycc_parser_test_helper::parse("for i in range(0, 6, 2):\n    print(i)\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::ForRange {
                var: "i".to_string(),
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(6),
                step: HirExpr::IntLiteral(2),
                body: vec![HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::Name("i".to_string())],
                })],
            })]
        );
    }

    #[test]
    fn iterating_a_non_call_expression_is_not_supported_yet() {
        // A list *literal* iterable is still rejected (Task 7/D-105 only
        // added support for a bare-name iterable, i.e. `for v in some_list:`
        // -- see `lowers_for_over_a_list_name_to_for_list`); the rejection
        // message itself changed to mention that new bare-name-list form.
        assert_capability_error_message(
            "for i in [1, 2, 3]:\n    print(i)\n",
            "only `for x in range(...)` or `for x in <list>` is supported so far",
        );
    }

    #[test]
    fn calling_something_other_than_range_in_a_for_is_not_supported_yet() {
        assert_capability_error_message(
            "for i in items(3):\n    print(i)\n",
            "only iterating over `range(...)` is supported so far",
        );
    }

    #[test]
    fn calling_via_an_attribute_in_a_for_is_not_supported_yet() {
        assert_capability_error_message(
            "for i in a.b(3):\n    print(i)\n",
            "only `for x in range(...)` is supported so far",
        );
    }

    #[test]
    fn range_with_too_many_arguments_is_not_supported() {
        assert_capability_error_message(
            "for i in range(1, 2, 3, 4):\n    print(i)\n",
            "range() with 4 arguments is not supported",
        );
    }

    #[test]
    fn a_tuple_for_target_is_not_supported_yet() {
        assert_capability_error_message(
            "for i, j in range(3):\n    print(i)\n",
            "only a bare name for-target is supported so far",
        );
    }

    #[test]
    fn a_for_else_is_not_supported_yet() {
        assert_capability_error_message(
            "for i in range(3):\n    print(i)\nelse:\n    print(0)\n",
            "for/else is not supported yet",
        );
    }

    #[test]
    fn an_async_for_inside_an_async_function_is_not_supported_yet() {
        // `async for` is only valid Python syntax inside an `async def` body,
        // so this now hits the (newer, more general) async-function rejection
        // in lower_function before lower_stmt's own `for_stmt.is_async` check
        // is ever reached -- the fixture still exercises real, valid Python
        // that must be rejected, just via the outer boundary now.
        assert_capability_error_message(
            "async def f() -> None:\n    async for i in range(3):\n        print(i)\n",
            "async functions are not supported yet",
        );
    }

    #[test]
    fn a_top_level_async_for_is_context_invalid() {
        // Issue #141 / D-148: `async for` outside an async function is
        // syntactically well-formed but CPython rejects it as a
        // `SyntaxError`, so this is now `L0001`, not `C0001` -- there is no
        // reachable "valid, just unimplemented" case here (see Correction 2
        // in the published plan): `lower_function` already rejects any
        // `async def` before this arm could ever be reached from inside a
        // real async function.
        assert_context_invalid_error_message(
            "async for i in range(3):\n    print(i)\n",
            "'async for' outside async function",
        );
    }

    #[test]
    fn an_async_for_inside_a_synchronous_function_is_context_invalid() {
        // The issue's own "other invalid async context" example beyond top
        // level: a synchronous `def` body reaches `lower_function`'s body
        // lowering (which resets `in_loop` but not any async-context state),
        // then this `for_stmt.is_async` arm -- unconditionally `L0001`,
        // exactly like the top-level case, since a synchronous function
        // provides no more of an "async function" context than module scope
        // does.
        assert_context_invalid_error_message(
            "def f() -> None:\n    async for i in range(3):\n        print(i)\n",
            "'async for' outside async function",
        );
    }

    #[test]
    fn a_top_level_break_is_context_invalid() {
        assert_context_invalid_error_message("break\n", "'break' outside loop");
    }

    #[test]
    fn a_top_level_continue_is_context_invalid() {
        assert_context_invalid_error_message("continue\n", "'continue' not properly in loop");
    }

    #[test]
    fn a_break_inside_a_synchronous_function_with_no_loop_is_context_invalid() {
        // Regression guard for `lower_function`'s own `false` call site
        // (entering a function body resets `in_loop`): a `break` directly in
        // a function body, with no enclosing loop, must still be
        // context-invalid, not silently inherit `true` from some outer
        // caller state.
        assert_context_invalid_error_message(
            "def f() -> None:\n    break\n",
            "'break' outside loop",
        );
    }

    #[test]
    fn a_break_inside_a_for_loop_is_still_unsupported() {
        // Regression guard: a real enclosing loop keeps break/continue on
        // the existing valid-but-unimplemented `C0001` path -- this issue is
        // scoped to classification, not to implementing loop control flow.
        assert_capability_error_message(
            "for i in range(3):\n    break\n",
            "statement kind not supported yet",
        );
    }

    #[test]
    fn a_continue_inside_a_while_loop_is_still_unsupported() {
        assert_capability_error_message(
            "while True:\n    continue\n",
            "statement kind not supported yet",
        );
    }

    #[test]
    fn a_break_inside_an_if_inside_a_for_loop_is_still_unsupported() {
        // Guards the `If` arm's and `lower_elif_else_clauses`' pass-through
        // of the caller's `in_loop` value: an `if` nested inside a loop body
        // must not reset loop context to `false`.
        assert_capability_error_message(
            "for i in range(3):\n    if i:\n        break\n",
            "statement kind not supported yet",
        );
    }

    #[test]
    fn a_top_level_yield_is_context_invalid() {
        // Issue #361 / D-149, this crate's expression-lowering sequel to
        // #141/D-148: `yield` outside any function is syntactically
        // well-formed but CPython rejects it as a `SyntaxError`, so this is
        // now `L0001`, not `C0001`.
        assert_context_invalid_error_message("yield 1\n", "'yield' outside function");
    }

    #[test]
    fn a_top_level_yield_from_is_context_invalid() {
        assert_context_invalid_error_message(
            "yield from [1, 2]\n",
            "'yield from' outside function",
        );
    }

    #[test]
    fn a_yield_nested_inside_a_top_level_if_is_still_context_invalid() {
        // Pins that `in_function` correctly stays `false` through `If`/
        // `lower_elif_else_clauses` recursion at module scope -- mirrors the
        // equivalent existing coverage for `break`/`continue` (D-148).
        assert_context_invalid_error_message("if True:\n    yield 1\n", "'yield' outside function");
    }

    #[test]
    fn a_yield_inside_a_real_function_is_still_unsupported() {
        // Regression guard: a real enclosing function keeps `yield` on the
        // existing valid-but-unimplemented `C0001` path -- generator codegen
        // remains out of scope for this issue.
        assert_capability_error_message(
            "def f() -> None:\n    yield 1\n",
            "expression kind not supported yet",
        );
    }

    #[test]
    fn a_yield_from_inside_a_real_function_is_still_unsupported() {
        assert_capability_error_message(
            "def f() -> None:\n    yield from [1, 2]\n",
            "expression kind not supported yet",
        );
    }

    #[test]
    fn a_yield_inside_a_comprehension_if_filter_stays_unsupported_at_module_scope() {
        // Regression-pinning test (D-149 correction 5): a `yield` reached
        // through a comprehension's `if`-filter is governed by a third,
        // scope-independent CPython rule (`'yield' inside list
        // comprehension`) this issue does not implement. The comprehension
        // helper cluster hardcodes a literal `true` at its own internal
        // `lower_expr` call sites instead of forwarding the real ambient
        // `in_function` value, so this must stay `C0001` at module scope,
        // unchanged by this fix -- guarding against a future edit that
        // "helpfully" threads the real value through and starts emitting the
        // wrong-per-CPython `L0001: 'yield' outside function` message here.
        assert_capability_error_message(
            "y = [x for x in range(3) if (yield x)]\n",
            "expression kind not supported yet",
        );
    }

    #[test]
    fn lowers_a_fully_annotated_public_function_with_params_and_return() {
        let module =
            pycc_parser_test_helper::parse("def add(a: int, b: int) -> int:\n    return a + b\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::Function {
                name: "add".to_string(),
                params: vec![("a".to_string(), Ty::Int), ("b".to_string(), Ty::Int)],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(HirExpr::Name("a".to_string())),
                    right: Box::new(HirExpr::Name("b".to_string())),
                }))],
            }]
        );
    }

    #[test]
    fn lowers_a_return_with_no_value() {
        let module = pycc_parser_test_helper::parse("def f() -> None:\n    return\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: Ty::None,
                body: vec![HirStmt::Return(None)],
            }]
        );
    }

    #[test]
    fn an_unannotated_public_function_missing_param_annotations_produces_t0001() {
        let module = pycc_parser_test_helper::parse("def add(a, b) -> int:\n    return a + b\n");
        let diag = lower_checked(&module).unwrap_err();
        assert_eq!(diag.code, "T0001");
        assert!(diag.message.contains('a'));
    }

    #[test]
    fn an_unannotated_public_function_missing_return_annotation_produces_t0001() {
        let module = pycc_parser_test_helper::parse("def add(a: int, b: int):\n    return a + b\n");
        let diag = lower_checked(&module).unwrap_err();
        assert_eq!(diag.code, "T0001");
        assert!(diag.message.contains("add"));
    }

    #[test]
    fn an_unannotated_private_function_is_allowed() {
        let module = pycc_parser_test_helper::parse("def _add(a, b):\n    return a + b\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::Function {
                name: "_add".to_string(),
                params: vec![("a".to_string(), Ty::Infer), ("b".to_string(), Ty::Infer)],
                return_ty: Ty::Infer,
                body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(HirExpr::Name("a".to_string())),
                    right: Box::new(HirExpr::Name("b".to_string())),
                }))],
            }]
        );
    }

    #[test]
    fn every_supported_annotation_type_lowers_correctly() {
        let cases = [
            ("def f(x: int) -> int:\n    return x\n", Ty::Int),
            ("def f(x: float) -> float:\n    return x\n", Ty::Float),
            ("def f(x: bool) -> bool:\n    return x\n", Ty::Bool),
            ("def f(x: str) -> str:\n    return x\n", Ty::Str),
            ("def f(x: None) -> None:\n    return x\n", Ty::None),
        ];
        for (source, expected_ty) in cases {
            let module = pycc_parser_test_helper::parse(source);
            let hir = lower_checked(&module).unwrap();
            assert_eq!(
                hir.items,
                vec![HirItem::Function {
                    name: "f".to_string(),
                    params: vec![("x".to_string(), expected_ty.clone())],
                    return_ty: expected_ty,
                    body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
                }],
                "wrong lowering for {source:?}"
            );
        }
    }

    #[test]
    fn an_unsupported_annotation_type_returns_a_capability_error() {
        assert_capability_error_message(
            "def f(x: list) -> None:\n    return\n",
            "type annotation `list` is not supported yet",
        );
    }

    #[test]
    fn a_non_bare_name_annotation_returns_a_capability_error() {
        assert_capability_error_message(
            "def f(x: a.b) -> None:\n    return\n",
            "only a bare name type annotation is supported so far",
        );
    }

    #[test]
    fn a_default_parameter_value_returns_a_capability_error() {
        // Regression test (self-review finding, pre-merge): lower_params
        // used to only read `.parameter`, silently ignoring `.default` --
        // producing a wrong signature (as if `b` had no default at all)
        // instead of an explicit capability diagnostic.
        assert_capability_error_message(
            "def f(a: int, b: int = 2) -> int:\n    return a + b\n",
            "default parameter values are not supported yet",
        );
    }

    #[test]
    fn a_positional_only_parameter_returns_a_capability_error() {
        assert_capability_error_message(
            "def f(a: int, /, b: int) -> int:\n    return a + b\n",
            "positional-only parameters",
        );
    }

    #[test]
    fn a_keyword_only_parameter_returns_a_capability_error() {
        assert_capability_error_message(
            "def f(a: int, *, b: int) -> int:\n    return a + b\n",
            "keyword-only parameters",
        );
    }

    #[test]
    fn a_vararg_parameter_returns_a_capability_error() {
        assert_capability_error_message(
            "def f(*args: int) -> None:\n    return\n",
            "*args` is not supported yet",
        );
    }

    #[test]
    fn a_kwarg_parameter_returns_a_capability_error() {
        assert_capability_error_message(
            "def f(**kwargs: int) -> None:\n    return\n",
            "**kwargs` is not supported yet",
        );
    }

    #[test]
    fn an_async_function_is_rejected_without_losing_async_semantics() {
        assert_capability_error_message(
            "async def f() -> None:\n    return\n",
            "async functions are not supported yet",
        );
    }

    #[test]
    fn a_decorated_function_is_rejected_without_losing_the_decorator() {
        assert_capability_error_message(
            "@decorator\ndef f() -> None:\n    return\n",
            "function decorators are not supported yet",
        );
    }

    #[test]
    fn a_generic_function_with_two_type_parameters_is_rejected() {
        // D-133: exactly one type parameter is accepted (see
        // `a_single_type_parameter_is_lowered_to_ty_param` below); two or
        // more still hit the frontend arity gate, since the underlying
        // representation and call-site substitution (D-134) are scoped to
        // the single-type-parameter case only.
        assert_capability_error_message(
            "def f[T, U](x: T) -> U:\n    return x\n",
            "generic functions with more than one type parameter are not supported yet",
        );
    }

    #[test]
    fn a_single_type_parameter_is_lowered_to_ty_param() {
        // D-133: `Ty::Param` is resolved by call-site substitution (D-134),
        // not by unification -- this test only asserts the frontend lowers
        // `T` consistently to `Ty::Param("T")` everywhere it appears in the
        // signature, not that substitution happens yet.
        let module = pycc_parser_test_helper::parse("def f[T](x: T) -> T:\n    return x\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::Function {
                name: "f".to_string(),
                params: vec![("x".to_string(), Ty::Param(Box::new("T".to_string())))],
                return_ty: Ty::Param(Box::new("T".to_string())),
                body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
            }]
        );
    }

    #[test]
    fn a_type_var_tuple_type_parameter_is_rejected() {
        // D-133: `Ty::Param` models one `TypeVar` resolved to one concrete
        // scalar via call-site substitution (D-134) -- `*Ts` stands for a
        // variable-length sequence of types instead, which `Ty::Param`
        // cannot represent, so this must be an explicit capability
        // rejection rather than silently treated like a plain `TypeVar`.
        assert_capability_error_message(
            "def f[*Ts](x: int) -> None:\n    return\n",
            "a `TypeVarTuple` type parameter (`*Ts`) is not supported yet",
        );
    }

    #[test]
    fn a_param_spec_type_parameter_is_rejected() {
        // D-133: same reasoning as the `TypeVarTuple` case above -- `**P`
        // stands for a parameter-list shape, not a single scalar type.
        assert_capability_error_message(
            "def f[**P](x: int) -> None:\n    return\n",
            "a `ParamSpec` type parameter (`**P`) is not supported yet",
        );
    }

    #[test]
    fn a_keyword_call_argument_is_rejected_instead_of_being_erased() {
        assert_capability_error_message(
            "def f() -> None:\n    return\n\nf(extra=undefined)\n",
            "keyword call arguments are not supported yet",
        );
    }

    #[test]
    fn a_keyword_range_argument_is_rejected_instead_of_being_erased() {
        assert_capability_error_message(
            "for i in range(stop=3):\n    i\n",
            "keyword arguments to range() are not supported yet",
        );
    }

    #[test]
    fn lowers_a_plain_string_literal() {
        let module = pycc_parser_test_helper::parse("x = \"hi\"\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::StringLiteral("hi".to_string()),
            })]
        );
    }

    #[test]
    fn an_any_annotation_produces_t0002() {
        let module = pycc_parser_test_helper::parse("def f(x: Any) -> None:\n    pass\n");
        let diag = lower_checked(&module).unwrap_err();
        assert_eq!(diag.code, "T0002");
    }

    #[test]
    fn an_any_return_annotation_produces_t0002() {
        let module = pycc_parser_test_helper::parse("def f() -> Any:\n    pass\n");
        let diag = lower_checked(&module).unwrap_err();
        assert_eq!(diag.code, "T0002");
    }

    #[test]
    fn lowers_a_basic_f_string_with_one_interpolation() {
        let module = pycc_parser_test_helper::parse("x = 1\ny = f\"value: {x}\"\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items[1],
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "y".to_string(),
                value: HirExpr::FString(vec![
                    FStringPart::Literal("value: ".to_string()),
                    FStringPart::Interpolation(Box::new(HirExpr::Name("x".to_string()))),
                ]),
            })
        );
    }

    #[test]
    fn lowers_an_f_string_with_only_literal_parts() {
        let module = pycc_parser_test_helper::parse("y = f\"no interpolation\"\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::Assign {
                target: "y".to_string(),
                value: HirExpr::FString(vec![FStringPart::Literal("no interpolation".to_string())]),
            })]
        );
    }

    #[test]
    fn an_f_string_with_a_format_spec_is_not_supported_yet() {
        assert_capability_error_message("x = 1.5\ny = f\"{x:.2f}\"\n", "format spec");
    }

    #[test]
    fn an_f_string_with_a_conversion_flag_is_not_supported_yet() {
        assert_capability_error_message("x = 1\ny = f\"{x!r}\"\n", "conversion");
    }

    #[test]
    fn lowers_an_annotated_assignment_with_a_value() {
        let module = pycc_parser_test_helper::parse("x: int = 1\nprint(x)\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![
                HirItem::TopLevelStmt(HirStmt::AnnAssign {
                    target: "x".to_string(),
                    annotation: Ty::Int,
                    value: Some(HirExpr::IntLiteral(1)),
                }),
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::Name("x".to_string())],
                })),
            ]
        );
    }

    #[test]
    fn lowers_an_annotated_assignment_with_no_value() {
        let module = pycc_parser_test_helper::parse("x: int\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::AnnAssign {
                target: "x".to_string(),
                annotation: Ty::Int,
                value: None,
            })]
        );
    }

    #[test]
    fn rejects_an_annotated_assignment_to_a_non_name_target() {
        // Unlike `Stmt::Assign` (which now accepts an `Expr::Attribute`
        // target -- `obj.attr = 1` -- as of D-154 Part 1 of #375, see
        // `stmt.rs`'s own comment on that arm), `Stmt::AnnAssign` still only
        // accepts a bare-name target: `obj.attr: int = 1` has no
        // attribute-annotated-assignment support anywhere in the compiler.
        assert_capability_error_message(
            "obj.attr: int = 1\n",
            "only assigning to a bare name is supported so far",
        );
    }

    #[test]
    fn rejects_a_parenthesized_annotated_assignment_target_instead_of_erasing_it() {
        // Regression test (advisor-review finding, pre-merge): `(x): int = 1`
        // still lowers `ann.target` to `Expr::Name("x")` -- identical to the
        // unparenthesized `x: int = 1` -- but upstream's own parser sets
        // `simple = false` for it (verified against the pinned
        // ruff_python_parser = "0.0.6" registry source), a real CPython
        // semantic difference this compiler doesn't model. An earlier draft
        // of this arm only matched on `Expr::Name` and ignored `ann.simple`
        // entirely, silently treating the two forms as identical instead of
        // producing the explicit capability diagnostic this file uses for
        // every other unmodeled AST field (see `lower_params`'s own
        // documented self-review finding above).
        assert_capability_error_message(
            "(x): int = 1\n",
            "a parenthesized annotated-assignment target is not supported yet",
        );
    }

    #[test]
    fn an_annotated_assignment_with_an_unsupported_annotation_returns_a_capability_error() {
        // Exercises the `annotation_to_ty(...)?` early-return branch inside
        // the new AnnAssign arm specifically (as opposed to the already
        // covered function-parameter/return-annotation call sites).
        assert_capability_error_message("x: list\n", "type annotation `list` is not supported yet");
    }

    #[test]
    fn an_annotated_assignment_with_an_unsupported_value_returns_a_capability_error() {
        // Exercises the `lower_expr(...)?` early-return branch for
        // AnnAssign's value expression specifically.
        // (1, 2) no longer fails to lower (this task) -- lambda is still
        // unsupported and exercises the identical propagation path.
        assert_capability_error_message(
            "x: int = lambda: 1\n",
            "expression kind not supported yet",
        );
    }

    // -- Task 7 (D-105): list[int] frontend HIR forms --------------------

    #[test]
    fn lowers_a_list_literal() {
        // Not a `let PATTERN = ... else { panic!(...) }` destructure -- this
        // file's own coverage-gate lesson (see pycc_ast's documented
        // `re_exported_grammar_types_resolve_and_have_the_expected_shape`
        // finding) is that a hand-written panic arm is never taken on the
        // happy path and shows up as a permanently uncovered region under
        // D-014's 100%-regions gate. A direct `assert_eq!` against the whole
        // expected `HirItem` avoids that without weakening the assertion.
        let module = pycc_parser_test_helper::parse("x = [1, 2, 3]\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items[0],
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::ListLiteral(vec![
                    HirExpr::IntLiteral(1),
                    HirExpr::IntLiteral(2),
                    HirExpr::IntLiteral(3),
                ]),
            })
        );
    }

    #[test]
    fn lowers_a_read_subscript() {
        let module = pycc_parser_test_helper::parse("x = [1, 2, 3]\ny = x[0]\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items[1],
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "y".to_string(),
                value: HirExpr::Subscript {
                    base: Box::new(HirExpr::Name("x".to_string())),
                    index: Box::new(HirExpr::IntLiteral(0)),
                },
            })
        );
    }

    #[test]
    fn lowers_append_as_a_dedicated_hir_node_not_a_generic_call() {
        let module = pycc_parser_test_helper::parse("x = [1]\nx.append(2)\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items[1],
            HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::ListAppend {
                list: "x".to_string(),
                value: Box::new(HirExpr::IntLiteral(2)),
            }))
        );
    }

    #[test]
    fn list_append_used_as_a_value_lowers_successfully_today() {
        // Real Python's `list.append()` always returns `None`, so using its
        // result as a value (as opposed to a bare `ExprStmt`) is meaningless
        // -- but rejecting that is a type judgment, which is `pycc_types`'
        // job (see `HirExpr::ListAppend`'s own doc comment), not this
        // lowering step's. This test locks in today's actual behavior --
        // `y = x.append(2)` lowers successfully -- so a future change to
        // this arm doesn't silently start rejecting (or accepting some
        // different shape of) value-position `.append()` without its own
        // deliberate decision.
        let module = pycc_parser_test_helper::parse("x = [1]\ny = x.append(2)\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items[1],
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "y".to_string(),
                value: HirExpr::ListAppend {
                    list: "x".to_string(),
                    value: Box::new(HirExpr::IntLiteral(2)),
                },
            })
        );
    }

    #[test]
    fn lowers_for_over_a_list_name_to_for_list() {
        let module = pycc_parser_test_helper::parse("x = [1, 2, 3]\nfor v in x:\n    print(v)\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items[1],
            HirItem::TopLevelStmt(HirStmt::ForList {
                var: "v".to_string(),
                list: "x".to_string(),
                body: vec![HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::Name("v".to_string())],
                })],
            })
        );
    }

    // -- PR-11 Task 3 (D-123): dict[str, int] frontend HIR forms ---------

    #[test]
    fn lowers_a_dict_literal() {
        let module = pycc_parser_test_helper::parse("x = {\"a\": 1, \"b\": 2}\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items[0],
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::DictLiteral(vec![
                    (
                        HirExpr::StringLiteral("a".to_string()),
                        HirExpr::IntLiteral(1),
                    ),
                    (
                        HirExpr::StringLiteral("b".to_string()),
                        HirExpr::IntLiteral(2),
                    ),
                ]),
            })
        );
    }

    #[test]
    fn lowers_an_empty_dict_literal() {
        // `{}` is an empty *dict* literal in Python grammar (an empty set has
        // no literal spelling -- `set()` is a call) -- `pycc_types` rejects
        // it (its element types can't be inferred), but lowering itself
        // succeeds, mirroring `HirExpr::ListLiteral(vec![])`'s own split of
        // responsibility.
        let module = pycc_parser_test_helper::parse("x = {}\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::DictLiteral(vec![]),
            })]
        );
    }

    #[test]
    fn dict_unpacking_inside_a_literal_is_unsupported() {
        assert_capability_error_message(
            "x = {**y}\n",
            "dict-unpacking (`**expr`) inside a dict literal is not supported yet",
        );
    }

    #[test]
    fn a_dict_literal_with_an_unsupported_key_propagates_the_key_error() {
        // (1, 2) no longer fails to lower (this task) -- lambda is still
        // unsupported and exercises the identical propagation path.
        assert_capability_error_message(
            "x = {(lambda: 1): 1}\n",
            "expression kind not supported yet",
        );
    }

    #[test]
    fn a_dict_literal_with_an_unsupported_value_propagates_the_value_error() {
        // (1, 2) no longer fails to lower (this task) -- lambda is still
        // unsupported and exercises the identical propagation path.
        assert_capability_error_message(
            "x = {\"a\": (lambda: 1)}\n",
            "expression kind not supported yet",
        );
    }

    // -- PR-11 Task 7 (D-123): set[int] frontend HIR forms ---------------

    #[test]
    fn lowers_a_set_literal() {
        let module = pycc_parser_test_helper::parse("x = {1, 2, 3}\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items[0],
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::SetLiteral(vec![
                    HirExpr::IntLiteral(1),
                    HirExpr::IntLiteral(2),
                    HirExpr::IntLiteral(3),
                ]),
            })
        );
    }

    #[test]
    fn a_set_literal_with_an_unsupported_element_propagates_the_element_error() {
        // (1, 2) no longer fails to lower (this task) -- lambda is still
        // unsupported and exercises the identical propagation path.
        assert_capability_error_message("x = {(lambda: 1)}\n", "expression kind not supported yet");
    }

    #[test]
    fn subscripted_type_annotation_with_unknown_base_is_rejected() {
        // #435 (Part D): subscripted type annotations (`ClassName[type_arg]`)
        // are now supported for known class names (PEP 560
        // `__class_getitem__`). `list[int]` is still rejected because `list`
        // itself is not a recognized type annotation in pycc (only
        // int/float/bool/str and user-defined class names are), not because
        // subscript syntax is universally rejected.
        assert_capability_error_message(
            "x: list[int] = []\n",
            "type annotation `list` is not supported yet",
        );
    }

    #[test]
    fn subscripted_type_annotation_with_non_name_base_is_rejected() {
        // #435 (Part D): a subscripted annotation whose base is not a bare
        // name (e.g. `a.b[int]`) is rejected — only a bare class name is
        // supported as the base of a subscripted type annotation.
        assert_capability_error_message(
            "x: a.b[int] = 1\n",
            "a subscripted type annotation's base must be a bare class name",
        );
    }

    #[test]
    fn an_empty_list_literal_lowers_to_an_empty_vec() {
        let module = pycc_parser_test_helper::parse("x = []\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::ListLiteral(vec![]),
            })]
        );
    }

    #[test]
    fn appending_to_a_non_bare_name_base_is_unsupported() {
        // `.append()` recognition is restricted to a bare-name list (D-105);
        // `a.b.append(1)` has a non-name `attr.value` (itself an attribute
        // access), so it must be rejected rather than silently accepted.
        assert_capability_error_message(
            "a.b.append(1)\n",
            "`.append()` is only supported on a bare-name list so far",
        );
    }

    #[test]
    fn append_with_zero_arguments_is_unsupported() {
        assert_capability_error_message(
            "x.append()\n",
            "list.append() takes exactly one argument, got 0",
        );
    }

    #[test]
    fn append_with_two_arguments_is_unsupported() {
        assert_capability_error_message(
            "x.append(1, 2)\n",
            "list.append() takes exactly one argument, got 2",
        );
    }

    #[test]
    fn calling_an_unrecognized_method_lowers_to_a_generic_method_call() {
        // Before D-154 (Part 1 of #375), any `.method()` call other than
        // `.append()`/`.pop()`/`.get()`/`.add()` was rejected right here at
        // HIR-lowering time (D-105, widened by D-119) -- this project had no
        // general method-dispatch shape at all yet. D-154 adds one
        // (`HirExpr::MethodCall`, for instance methods), and -- since this
        // lowering step has no type information to distinguish "receiver is
        // a class instance" from "receiver is anything else" -- every
        // `.method()` call not claimed by a hand-recognized container
        // method or a resolved stdlib symbol call now lowers successfully
        // into that generic shape. `foo`/`x` are never assigned in either
        // snippet below, so real rejection now happens downstream, at
        // `pycc_types` (an unbound-name or non-instance-receiver
        // diagnostic), not here. `.remove()` is kept as the same
        // deliberately-chosen example as before (a real `list` method this
        // compiler doesn't special-case, D-119) to show it now takes the
        // identical generic path as an arbitrary, entirely unrecognized
        // name like `.bar()`.
        let module = pycc_parser_test_helper::parse("foo.bar()\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(
                HirExpr::MethodCall {
                    base: Box::new(HirExpr::Name("foo".to_string())),
                    method: "bar".to_string(),
                    args: vec![],
                }
            ))]
        );

        let module = pycc_parser_test_helper::parse("x.remove(1)\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(
                HirExpr::MethodCall {
                    base: Box::new(HirExpr::Name("x".to_string())),
                    method: "remove".to_string(),
                    args: vec![HirExpr::IntLiteral(1)],
                }
            ))]
        );
    }

    // The five tests below exercise each new arm's own `?`-propagation path
    // specifically (an inner element/base/index/argument/body expression
    // that itself fails to lower), as opposed to every test above, which
    // only ever supplies inner expressions that lower successfully.

    #[test]
    fn a_list_literal_with_an_unsupported_element_propagates_the_element_error() {
        // (1, 2) no longer fails to lower (this task) -- lambda is still
        // unsupported and exercises the identical propagation path.
        assert_capability_error_message("x = [(lambda: 1)]\n", "expression kind not supported yet");
    }

    #[test]
    fn a_subscript_with_an_unsupported_base_propagates_the_base_error() {
        // (1, 2) no longer fails to lower (this task) -- lambda is still
        // unsupported and exercises the identical propagation path.
        assert_capability_error_message(
            "y = (lambda: 1)[0]\n",
            "expression kind not supported yet",
        );
    }

    #[test]
    fn a_subscript_with_an_unsupported_index_propagates_the_index_error() {
        // (1, 2) no longer fails to lower (this task) -- lambda is still
        // unsupported and exercises the identical propagation path.
        assert_capability_error_message("y = x[lambda: 1]\n", "expression kind not supported yet");
    }

    #[test]
    fn an_append_with_an_unsupported_argument_propagates_the_argument_error() {
        // (1, 2) no longer fails to lower (this task) -- lambda is still
        // unsupported and exercises the identical propagation path.
        assert_capability_error_message(
            "x.append(lambda: 1)\n",
            "expression kind not supported yet",
        );
    }

    // -- PR-12 Task 10 (D-119): remaining container methods depth --------
    // `list.pop()`, `dict.get(key, default)`, `set.add(value)` -- each
    // mirrors `.append()`'s own hand-recognized-special-form shape and test
    // coverage exactly (bare-name-base gate, arity gate, value-position
    // lowering, argument-propagation).

    #[test]
    fn lowers_pop_as_a_dedicated_hir_node_not_a_generic_call() {
        let module = pycc_parser_test_helper::parse("x = [1]\nx.pop()\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items[1],
            HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::ListPop {
                list: "x".to_string(),
            }))
        );
    }

    #[test]
    fn list_pop_used_as_a_value_lowers_successfully() {
        // Unlike `ListAppend`, `.pop()`'s value is the list's element type,
        // not `None` -- `y = x.pop()` is the primary intended use, not a
        // curiosity being merely tolerated.
        let module = pycc_parser_test_helper::parse("x = [1]\ny = x.pop()\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items[1],
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "y".to_string(),
                value: HirExpr::ListPop {
                    list: "x".to_string(),
                },
            })
        );
    }

    #[test]
    fn popping_from_a_non_bare_name_base_is_unsupported() {
        assert_capability_error_message(
            "a.b.pop()\n",
            "`.pop()` is only supported on a bare-name list so far",
        );
    }

    #[test]
    fn pop_with_one_argument_is_unsupported() {
        assert_capability_error_message("x.pop(0)\n", "list.pop() takes no arguments, got 1");
    }

    #[test]
    fn lowers_get_as_a_dedicated_hir_node_not_a_generic_call() {
        let module = pycc_parser_test_helper::parse("d = {\"a\": 1}\nd.get(\"a\", 0)\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items[1],
            HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::DictGetOrDefault {
                dict: "d".to_string(),
                key: Box::new(HirExpr::StringLiteral("a".to_string())),
                default: Box::new(HirExpr::IntLiteral(0)),
            }))
        );
    }

    #[test]
    fn dict_get_used_as_a_value_lowers_successfully() {
        let module = pycc_parser_test_helper::parse("d = {\"a\": 1}\ny = d.get(\"a\", 0)\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items[1],
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "y".to_string(),
                value: HirExpr::DictGetOrDefault {
                    dict: "d".to_string(),
                    key: Box::new(HirExpr::StringLiteral("a".to_string())),
                    default: Box::new(HirExpr::IntLiteral(0)),
                },
            })
        );
    }

    #[test]
    fn getting_from_a_non_bare_name_base_is_unsupported() {
        assert_capability_error_message(
            "a.b.get(\"a\", 0)\n",
            "`.get()` is only supported on a bare-name dict so far",
        );
    }

    #[test]
    fn get_with_one_argument_is_unsupported() {
        assert_capability_error_message(
            "d.get(\"a\")\n",
            "dict.get() takes exactly two arguments (key, default), got 1",
        );
    }

    #[test]
    fn get_with_three_arguments_is_unsupported() {
        assert_capability_error_message(
            "d.get(\"a\", 0, 1)\n",
            "dict.get() takes exactly two arguments (key, default), got 3",
        );
    }

    #[test]
    fn a_get_call_with_an_unsupported_key_propagates_the_key_error() {
        assert_capability_error_message(
            "d.get(lambda: 1, 0)\n",
            "expression kind not supported yet",
        );
    }

    #[test]
    fn a_get_call_with_an_unsupported_default_propagates_the_default_error() {
        assert_capability_error_message(
            "d.get(\"a\", lambda: 1)\n",
            "expression kind not supported yet",
        );
    }

    #[test]
    fn lowers_add_as_a_dedicated_hir_node_not_a_generic_call() {
        let module = pycc_parser_test_helper::parse("s = {1}\ns.add(2)\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items[1],
            HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::SetAdd {
                set: "s".to_string(),
                value: Box::new(HirExpr::IntLiteral(2)),
            }))
        );
    }

    #[test]
    fn set_add_used_as_a_value_lowers_successfully_today() {
        // Mirrors `ListAppend`'s own "today's actual behavior" test exactly
        // -- `.add()`'s value is always `None`, and D-131 lets an assignment
        // preserve that materialized unit value in ordinary storage.
        let module = pycc_parser_test_helper::parse("s = {1}\ny = s.add(2)\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items[1],
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "y".to_string(),
                value: HirExpr::SetAdd {
                    set: "s".to_string(),
                    value: Box::new(HirExpr::IntLiteral(2)),
                },
            })
        );
    }

    #[test]
    fn adding_to_a_non_bare_name_base_is_unsupported() {
        assert_capability_error_message(
            "a.b.add(1)\n",
            "`.add()` is only supported on a bare-name set so far",
        );
    }

    #[test]
    fn add_with_zero_arguments_is_unsupported() {
        assert_capability_error_message("s.add()\n", "set.add() takes exactly one argument, got 0");
    }

    #[test]
    fn add_with_two_arguments_is_unsupported() {
        assert_capability_error_message(
            "s.add(1, 2)\n",
            "set.add() takes exactly one argument, got 2",
        );
    }

    #[test]
    fn an_add_with_an_unsupported_argument_propagates_the_argument_error() {
        assert_capability_error_message("s.add(lambda: 1)\n", "expression kind not supported yet");
    }

    #[test]
    fn a_for_list_body_with_an_unsupported_statement_propagates_the_body_error() {
        // (1, 2) no longer fails to lower (this task) -- lambda is still
        // unsupported and exercises the identical propagation path.
        assert_capability_error_message(
            "x = [1, 2, 3]\nfor v in x:\n    lambda: 1\n",
            "expression kind not supported yet",
        );
    }

    // -- PR-11b Task 2 (D-116): tuple[...] frontend HIR forms -------------

    #[test]
    fn a_bare_unparenthesized_tuple_expression_lowers_the_same_as_parenthesized() {
        // Python's tuple literal syntax does not require parentheses
        // (`1, 2` and `(1, 2)` parse to the same `Expr::Tuple` node); this
        // locks in that this crate's own lowering treats both identically,
        // since `lower_expr` never inspects `ExprTuple::parenthesized`.
        let module = pycc_parser_test_helper::parse("x = 1, 2\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items[0],
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::TupleLiteral(vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)]),
            })
        );
    }

    #[test]
    fn a_heterogeneous_tuple_literal_lowers_with_mixed_element_kinds() {
        // Unlike `ListLiteral`/`SetLiteral`, this crate does not reject
        // mixed element kinds at the HIR layer -- D-116 makes heterogeneity
        // tuple's own defining feature, judged (for element *type*, not
        // syntactic kind) entirely by `pycc_types`, not here.
        let module = pycc_parser_test_helper::parse("x = (1, True, 2.5)\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items[0],
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::TupleLiteral(vec![
                    HirExpr::IntLiteral(1),
                    HirExpr::BoolLiteral(true),
                    HirExpr::FloatLiteral(2.5),
                ]),
            })
        );
    }

    #[test]
    fn a_tuple_element_that_fails_to_lower_propagates_its_own_error() {
        assert_capability_error_message(
            "x = (1, lambda: 1)\n",
            "expression kind not supported yet",
        );
    }

    // -- PR-12 Task 2 (D-117): comprehension frontend HIR forms ----------

    #[test]
    fn lowers_a_list_comprehension_over_range_to_list_comp_assign() {
        let module = pycc_parser_test_helper::parse("y = [i for i in range(3)]\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::ListCompAssign {
                target: "y".to_string(),
                var: "0comp_11_i".to_string(),
                iter: CompIter::Range {
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                },
                cond: None,
                elt: Box::new(HirExpr::Name("0comp_11_i".to_string())),
            })]
        );
    }

    #[test]
    fn lowers_a_list_comprehension_with_an_if_filter() {
        let module = pycc_parser_test_helper::parse("y = [i for i in range(5) if i]\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::ListCompAssign {
                target: "y".to_string(),
                var: "0comp_11_i".to_string(),
                iter: CompIter::Range {
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(5),
                    step: HirExpr::IntLiteral(1),
                },
                cond: Some(Box::new(HirExpr::Name("0comp_11_i".to_string()))),
                elt: Box::new(HirExpr::Name("0comp_11_i".to_string())),
            })]
        );
    }

    #[test]
    fn lowers_a_dict_comprehension_with_an_f_string_key_renaming_the_interpolation() {
        // Confirms `FString`'s own `rename_name_in_expr` arm actually
        // rewrites an interpolated loop-variable reference, not just a bare
        // `Name` expression.
        let module = pycc_parser_test_helper::parse("y = {f\"n{i}\": i for i in range(3)}\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::DictCompAssign {
                target: "y".to_string(),
                var: "0comp_20_i".to_string(),
                iter: CompIter::Range {
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                },
                cond: None,
                key: Box::new(HirExpr::FString(vec![
                    FStringPart::Literal("n".to_string()),
                    FStringPart::Interpolation(Box::new(HirExpr::Name("0comp_20_i".to_string()))),
                ])),
                value: Box::new(HirExpr::Name("0comp_20_i".to_string())),
            })]
        );
    }

    #[test]
    fn lowers_a_set_comprehension_to_set_comp_assign() {
        let module = pycc_parser_test_helper::parse("y = {i for i in range(3)}\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::SetCompAssign {
                target: "y".to_string(),
                var: "0comp_11_i".to_string(),
                iter: CompIter::Range {
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                },
                cond: None,
                elt: Box::new(HirExpr::Name("0comp_11_i".to_string())),
            })]
        );
    }

    #[test]
    fn lowers_a_set_comprehension_with_an_if_filter() {
        // `SetCompAssign`'s own `cond` field needs a dedicated if-filter
        // test distinct from the plain set-comprehension test above: the
        // `cond.map(|c| rename_name_in_expr(...))` closure inside
        // `lower_set_comp_assign` is only reached when `cond.is_some()`.
        let module = pycc_parser_test_helper::parse("y = {i for i in range(5) if i}\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::SetCompAssign {
                target: "y".to_string(),
                var: "0comp_11_i".to_string(),
                iter: CompIter::Range {
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(5),
                    step: HirExpr::IntLiteral(1),
                },
                cond: Some(Box::new(HirExpr::Name("0comp_11_i".to_string()))),
                elt: Box::new(HirExpr::Name("0comp_11_i".to_string())),
            })]
        );
    }

    #[test]
    fn lowers_a_dict_comprehension_with_an_if_filter() {
        // Same reasoning as `lowers_a_set_comprehension_with_an_if_filter`
        // above, for `lower_dict_comp_assign`'s own `cond.map(...)` closure.
        let module = pycc_parser_test_helper::parse("y = {i: i for i in range(5) if i}\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::DictCompAssign {
                target: "y".to_string(),
                var: "0comp_14_i".to_string(),
                iter: CompIter::Range {
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(5),
                    step: HirExpr::IntLiteral(1),
                },
                cond: Some(Box::new(HirExpr::Name("0comp_14_i".to_string()))),
                key: Box::new(HirExpr::Name("0comp_14_i".to_string())),
                value: Box::new(HirExpr::Name("0comp_14_i".to_string())),
            })]
        );
    }

    #[test]
    fn a_comprehension_with_two_for_clauses_is_unsupported() {
        assert_capability_error_message(
            "y = [i for i in range(3) for j in range(3)]\n",
            "a comprehension with more than one `for` clause is not supported yet",
        );
    }

    #[test]
    fn a_comprehension_with_two_if_filters_is_unsupported() {
        assert_capability_error_message(
            "y = [i for i in range(5) if i if i]\n",
            "a comprehension with more than one `if` filter is not supported yet",
        );
    }

    #[test]
    fn a_comprehension_with_a_non_bare_name_target_is_unsupported() {
        assert_capability_error_message(
            "y = [a for (a, b) in xs]\n",
            "only a bare name comprehension target is supported so far",
        );
    }

    #[test]
    fn an_async_for_comprehension_is_unsupported() {
        assert_capability_error_message(
            "y = [i async for i in xs]\n",
            "async comprehensions are not supported yet",
        );
    }

    #[test]
    fn a_comprehension_used_as_a_call_argument_is_not_specially_recognized() {
        // Pins the "only `Stmt::Assign`-RHS position" restriction (D-117): a
        // comprehension anywhere else still falls through to `lower_expr`'s
        // existing generic catch-all, not a new comprehension-specific
        // error path.
        assert_capability_error_message(
            "print([i for i in range(3)])\n",
            "expression kind not supported yet",
        );
    }

    #[test]
    fn a_comprehension_iterating_a_bare_name_produces_comp_iter_name() {
        let module = pycc_parser_test_helper::parse("xs = [1, 2, 3]\ny = [i for i in xs]\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items[1],
            HirItem::TopLevelStmt(HirStmt::ListCompAssign {
                target: "y".to_string(),
                var: "0comp_26_i".to_string(),
                iter: CompIter::Name("xs".to_string()),
                cond: None,
                elt: Box::new(HirExpr::Name("0comp_26_i".to_string())),
            })
        );
    }

    #[test]
    fn a_comprehension_range_iterable_referencing_the_loop_variables_own_source_name_is_not_renamed()
     {
        // Pins `lower_comprehension_header`'s own documented asymmetry
        // (D-117): `iter` is deliberately never passed through
        // `rename_name_in_expr`, unlike `elt`/`cond`/`key`/`value` -- this
        // is correct CPython scoping, since a comprehension's iterable
        // expression evaluates in the *enclosing* scope, before the
        // comprehension's own loop variable is ever bound.
        // `[i for i in range(i)]`'s `range(i)` must read the *outer* `i`
        // (here, the module-level `i = 5`), not the comprehension's own
        // synthesized loop variable. Without this test, a future change
        // that "fixed" this asymmetry by renaming `iter` too would silently
        // break correct scoping with every other existing test (and 100%
        // coverage) still green, since no other test uses an iterable
        // expression that shares the loop variable's own source name.
        let module = pycc_parser_test_helper::parse("i = 5\ny = [i for i in range(i)]\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items[1],
            HirItem::TopLevelStmt(HirStmt::ListCompAssign {
                target: "y".to_string(),
                var: "0comp_17_i".to_string(),
                iter: CompIter::Range {
                    start: HirExpr::IntLiteral(0),
                    // The un-renamed enclosing-scope `i`, not
                    // `HirExpr::Name("0comp_17_i")`.
                    stop: HirExpr::Name("i".to_string()),
                    step: HirExpr::IntLiteral(1),
                },
                cond: None,
                elt: Box::new(HirExpr::Name("0comp_17_i".to_string())),
            })
        );
    }

    #[test]
    fn a_comprehension_bare_name_iterable_sharing_the_loop_variables_own_source_name_is_not_renamed()
     {
        // Same reasoning as the `range(i)` test above, for `CompIter::Name`
        // instead of `CompIter::Range`: `[xs for xs in xs]`'s iterable `xs`
        // must resolve to the un-renamed enclosing-scope name, not the
        // comprehension's own synthesized loop variable, even though both
        // are spelled identically in the source.
        let module = pycc_parser_test_helper::parse("xs = [1, 2, 3]\ny = [xs for xs in xs]\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items[1],
            HirItem::TopLevelStmt(HirStmt::ListCompAssign {
                target: "y".to_string(),
                var: "0comp_27_xs".to_string(),
                // The un-renamed enclosing-scope `xs`, not
                // `CompIter::Name("0comp_27_xs".to_string())`.
                iter: CompIter::Name("xs".to_string()),
                cond: None,
                elt: Box::new(HirExpr::Name("0comp_27_xs".to_string())),
            })
        );
    }

    #[test]
    fn a_comprehension_iterating_a_list_literal_is_unsupported() {
        // Neither a bare name nor a call -- exercises
        // `lower_comprehension_iter`'s own "not Name, not Call" branch,
        // which is not shared with `Stmt::For`'s separate (textually
        // similar but distinct) iterable-shape checks.
        assert_capability_error_message(
            "y = [i for i in [1, 2]]\n",
            "only `range(...)` or a bare-name iterable is supported so far in a comprehension",
        );
    }

    #[test]
    fn a_comprehension_iterating_a_non_name_callee_call_is_unsupported() {
        assert_capability_error_message(
            "y = [i for i in f()()]\n",
            "only calling `range(...)` is supported so far in a comprehension",
        );
    }

    #[test]
    fn a_comprehension_iterating_a_non_range_call_is_unsupported() {
        assert_capability_error_message(
            "y = [i for i in foo(3)]\n",
            "only iterating over `range(...)` is supported so far in a comprehension, got `foo`",
        );
    }

    #[test]
    fn a_comprehension_range_call_with_keyword_arguments_is_unsupported() {
        assert_capability_error_message(
            "y = [i for i in range(stop=3)]\n",
            "keyword arguments to range() are not supported yet",
        );
    }

    // The eight tests below each exercise one `?`-propagation region on its
    // own `?` operator's specific call site (mirroring this file's existing
    // "the five tests below exercise each new arm's own `?`-propagation
    // path specifically" precedent above): `lower_set_comp_assign` and
    // `lower_dict_comp_assign` are structurally near-identical to
    // `lower_list_comp_assign`, but each function's own `?` is a distinct
    // coverage region, so an error test against one function's call site
    // does not also cover its sibling's.

    #[test]
    fn a_set_comprehension_with_an_unsupported_header_propagates_the_header_error() {
        // Exercises both `Stmt::Assign`'s own `Expr::SetComp(comp) =>
        // lower_set_comp_assign(...)?` call site and
        // `lower_set_comp_assign`'s own internal
        // `lower_comprehension_header(&comp.generators)?` call site in one
        // test, since the header error propagates through both in the same
        // nested call.
        assert_capability_error_message(
            "y = {i for i in range(3) for j in range(3)}\n",
            "a comprehension with more than one `for` clause is not supported yet",
        );
    }

    #[test]
    fn a_comprehension_iter_range_call_with_too_many_arguments_is_unsupported() {
        // Exercises `lower_comprehension_iter`'s own
        // `lower_range_call(call)?` call site specifically -- distinct from
        // `Stmt::For`'s own separate call site to the same shared helper
        // (already covered by `range_with_too_many_arguments_is_not_supported`
        // above).
        assert_capability_error_message(
            "y = [i for i in range(1, 2, 3, 4)]\n",
            "range() with 4 arguments is not supported",
        );
    }

    #[test]
    fn a_comprehension_if_filter_with_an_unsupported_expression_propagates_the_filter_error() {
        assert_capability_error_message(
            "y = [i for i in range(3) if (lambda: 1)]\n",
            "expression kind not supported yet",
        );
    }

    #[test]
    fn a_list_comprehension_element_that_fails_to_lower_propagates_the_element_error() {
        assert_capability_error_message(
            "y = [(lambda: 1) for i in range(3)]\n",
            "expression kind not supported yet",
        );
    }

    #[test]
    fn a_set_comprehension_element_that_fails_to_lower_propagates_the_element_error() {
        assert_capability_error_message(
            "y = {(lambda: 1) for i in range(3)}\n",
            "expression kind not supported yet",
        );
    }

    #[test]
    fn a_dict_comprehension_with_an_unsupported_header_propagates_the_header_error() {
        assert_capability_error_message(
            "y = {i: i for i in range(3) for j in range(3)}\n",
            "a comprehension with more than one `for` clause is not supported yet",
        );
    }

    #[test]
    fn a_dict_comprehension_key_that_fails_to_lower_propagates_the_key_error() {
        assert_capability_error_message(
            "y = {(lambda: 1): i for i in range(3)}\n",
            "expression kind not supported yet",
        );
    }

    #[test]
    fn a_dict_comprehension_value_that_fails_to_lower_propagates_the_value_error() {
        assert_capability_error_message(
            "y = {i: (lambda: 1) for i in range(3)}\n",
            "expression kind not supported yet",
        );
    }

    #[test]
    fn dict_comp_key_unpacking_parses_successfully_and_is_rejected_at_lowering() {
        // The task-2 brief this test suite followed assumed
        // `{**x for k in y}`-shaped source could never actually parse (so
        // `comp.key == None` would be unreachable, modeled with an internal
        // panic). Verified false directly against the vendored
        // `ruff_python_parser`: it parses this successfully as
        // `ExprDictComp { key: None, value: Name("x"), .. }`, silently
        // dropping the `**` rather than erroring -- so `pycc_parser::parse`
        // itself succeeds here, and `lower_dict_comp_assign` is the one
        // that must reject it, with an ordinary `C0001` capability
        // diagnostic instead of a panic.
        assert!(pycc_parser::parse("y = {**x for k in z}\n").is_ok());
        assert_capability_error_message(
            "y = {**x for k in z}\n",
            "dict-unpacking (`**expr`) inside a dict comprehension is not supported yet",
        );
    }

    #[test]
    fn lower_comprehension_header_rejects_an_empty_generators_slice() {
        // Real parsed source can never produce a comprehension with zero
        // generators -- this is the only way to reach the `[generator] =
        // generators else { ... }` arm's failure branch and its own
        // `generators.first().map(|g| g.range).unwrap_or_default()`
        // span-fallback expression at all (D-014's region coverage gate
        // would otherwise flag that fallback as an uncoverable dead
        // branch).
        let err = lower_comprehension_header(&[], None).unwrap_err();
        assert_eq!(err.code, "C0001");
        assert!(
            err.message
                .contains("a comprehension with more than one `for` clause is not supported yet")
        );
        assert_eq!(err.span, Some(Span::new(0, 0)));
    }

    // -- Task 6 (D-118): list[int] slicing frontend HIR forms ------------

    #[test]
    fn lowers_a_slice_expression_with_both_bounds_present() {
        let module = pycc_parser_test_helper::parse("xs = [1, 2, 3]\ny = xs[1:3]\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items[1],
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "y".to_string(),
                value: HirExpr::Slice {
                    base: Box::new(HirExpr::Name("xs".to_string())),
                    start: Some(Box::new(HirExpr::IntLiteral(1))),
                    stop: Some(Box::new(HirExpr::IntLiteral(3))),
                    step: None,
                },
            })
        );
    }

    #[test]
    fn lowers_a_slice_expression_with_only_the_stop_bound() {
        let module = pycc_parser_test_helper::parse("xs = [1, 2, 3]\ny = xs[:3]\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items[1],
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "y".to_string(),
                value: HirExpr::Slice {
                    base: Box::new(HirExpr::Name("xs".to_string())),
                    start: None,
                    stop: Some(Box::new(HirExpr::IntLiteral(3))),
                    step: None,
                },
            })
        );
    }

    #[test]
    fn lowers_a_slice_expression_with_only_the_start_bound() {
        let module = pycc_parser_test_helper::parse("xs = [1, 2, 3]\ny = xs[2:]\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items[1],
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "y".to_string(),
                value: HirExpr::Slice {
                    base: Box::new(HirExpr::Name("xs".to_string())),
                    start: Some(Box::new(HirExpr::IntLiteral(2))),
                    stop: None,
                    step: None,
                },
            })
        );
    }

    #[test]
    fn lowers_a_slice_expression_with_all_bounds_omitted() {
        let module = pycc_parser_test_helper::parse("xs = [1, 2, 3]\ny = xs[:]\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items[1],
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "y".to_string(),
                value: HirExpr::Slice {
                    base: Box::new(HirExpr::Name("xs".to_string())),
                    start: None,
                    stop: None,
                    step: None,
                },
            })
        );
    }

    #[test]
    fn lowers_a_slice_expression_with_a_step() {
        let module = pycc_parser_test_helper::parse("xs = [1, 2, 3]\ny = xs[::2]\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items[1],
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "y".to_string(),
                value: HirExpr::Slice {
                    base: Box::new(HirExpr::Name("xs".to_string())),
                    start: None,
                    stop: None,
                    step: Some(Box::new(HirExpr::IntLiteral(2))),
                },
            })
        );
    }

    #[test]
    fn an_ordinary_single_expression_subscript_still_lowers_to_subscript_not_slice() {
        // Regression pin for Step 2's new `match sub.slice.as_ref() { ... }`
        // dispatch (D-118): a colon-free subscript must keep taking the `_`
        // arm and producing the pre-existing `HirExpr::Subscript` shape,
        // unaffected by the new `Expr::Slice` arm added alongside it.
        // (Already exercised incidentally by `lowers_a_read_subscript`
        // above; pinned again here, by name, as the dedicated regression
        // test for this specific change.)
        let module = pycc_parser_test_helper::parse("xs = [1, 2, 3]\ny = xs[0]\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items[1],
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "y".to_string(),
                value: HirExpr::Subscript {
                    base: Box::new(HirExpr::Name("xs".to_string())),
                    index: Box::new(HirExpr::IntLiteral(0)),
                },
            })
        );
    }

    #[test]
    fn a_slice_expressions_base_and_bounds_are_recursively_lowered() {
        // `f()`/`g()` stand in for "some already-lowerable non-literal
        // shape" -- confirms `base`/`start`/`stop` are each passed through
        // the real `lower_expr` recursively, not merely accepted as raw
        // literals.
        let module = pycc_parser_test_helper::parse("xs = [1, 2, 3]\ny = xs[f():g()]\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items[1],
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "y".to_string(),
                value: HirExpr::Slice {
                    base: Box::new(HirExpr::Name("xs".to_string())),
                    start: Some(Box::new(HirExpr::Call {
                        callee: "f".to_string(),
                        args: vec![],
                    })),
                    stop: Some(Box::new(HirExpr::Call {
                        callee: "g".to_string(),
                        args: vec![],
                    })),
                    step: None,
                },
            })
        );
    }

    #[test]
    fn a_slice_with_an_unsupported_base_propagates_the_base_error() {
        // Exercises the `Expr::Slice(slice) => ...` arm's own
        // `lower_expr(&sub.value)?` call site specifically -- a distinct
        // region from the `_` arm's identically-worded call site, already
        // covered by `a_subscript_with_an_unsupported_base_propagates_the_base_error`.
        assert_capability_error_message(
            "y = (lambda: 1)[0:1]\n",
            "expression kind not supported yet",
        );
    }

    #[test]
    fn a_slice_with_an_unsupported_start_bound_propagates_the_start_error() {
        assert_capability_error_message(
            "xs = [1, 2, 3]\ny = xs[(lambda: 1):3]\n",
            "expression kind not supported yet",
        );
    }

    #[test]
    fn a_slice_with_an_unsupported_stop_bound_propagates_the_stop_error() {
        assert_capability_error_message(
            "xs = [1, 2, 3]\ny = xs[1:(lambda: 1)]\n",
            "expression kind not supported yet",
        );
    }

    #[test]
    fn a_slice_with_an_unsupported_step_propagates_the_step_error() {
        assert_capability_error_message(
            "xs = [1, 2, 3]\ny = xs[1:3:(lambda: 1)]\n",
            "expression kind not supported yet",
        );
    }

    #[test]
    fn a_slice_assignment_target_is_not_specially_recognized() {
        // Pins `HirExpr::Slice`'s own documented "Load position only"
        // restriction (D-118): unlike a plain-index assignment target
        // (`xs[0] = 1`, which reaches `HirStmt::DictSet` via `Stmt::Assign`'s
        // own `Expr::Subscript` target arm), a slice assignment target
        // (`xs[1:3] = value`) calls `lower_expr` directly on the bare
        // `Expr::Slice` node -- which has no top-level arm -- and falls
        // through to the same generic catch-all as any other unsupported
        // expression kind. This is intentional (slicing ships read-only in
        // this PR), not a regression to fix later without revisiting D-118.
        assert_capability_error_message(
            "xs = [1, 2, 3]\nxs[1:3] = [4, 5]\n",
            "expression kind not supported yet",
        );
    }

    #[test]
    fn rename_name_in_expr_rewrites_every_hir_expr_variant() {
        // Exhaustiveness-pinning test for `rename_name_in_expr`'s own
        // "let the compiler enumerate every site" discipline (D-117,
        // mirroring D-107's `Scalar::List` precedent) -- every arm must be
        // hit by at least one case, and every conditional inside an arm
        // (name matches `from` vs. doesn't) must be hit on both sides.

        // Name: renamed when it matches `from`, left alone otherwise.
        assert_eq!(
            rename_name_in_expr(HirExpr::Name("old".to_string()), "old", "new"),
            HirExpr::Name("new".to_string())
        );
        assert_eq!(
            rename_name_in_expr(HirExpr::Name("other".to_string()), "old", "new"),
            HirExpr::Name("other".to_string())
        );

        // The four grouped literal variants are returned unchanged.
        assert_eq!(
            rename_name_in_expr(HirExpr::IntLiteral(1), "old", "new"),
            HirExpr::IntLiteral(1)
        );
        assert_eq!(
            rename_name_in_expr(HirExpr::FloatLiteral(1.5), "old", "new"),
            HirExpr::FloatLiteral(1.5)
        );
        assert_eq!(
            rename_name_in_expr(HirExpr::BoolLiteral(true), "old", "new"),
            HirExpr::BoolLiteral(true)
        );
        assert_eq!(
            rename_name_in_expr(HirExpr::StringLiteral("s".to_string()), "old", "new"),
            HirExpr::StringLiteral("s".to_string())
        );

        // Call: renames every argument.
        assert_eq!(
            rename_name_in_expr(
                HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::Name("old".to_string())],
                },
                "old",
                "new",
            ),
            HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::Name("new".to_string())],
            }
        );

        // BinOp: renames both sides.
        assert_eq!(
            rename_name_in_expr(
                HirExpr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(HirExpr::Name("old".to_string())),
                    right: Box::new(HirExpr::Name("old".to_string())),
                },
                "old",
                "new",
            ),
            HirExpr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(HirExpr::Name("new".to_string())),
                right: Box::new(HirExpr::Name("new".to_string())),
            }
        );

        // Compare: renames both sides.
        assert_eq!(
            rename_name_in_expr(
                HirExpr::Compare {
                    op: CmpOpKind::Eq,
                    left: Box::new(HirExpr::Name("old".to_string())),
                    right: Box::new(HirExpr::Name("old".to_string())),
                },
                "old",
                "new",
            ),
            HirExpr::Compare {
                op: CmpOpKind::Eq,
                left: Box::new(HirExpr::Name("new".to_string())),
                right: Box::new(HirExpr::Name("new".to_string())),
            }
        );

        // FString: covers both a `Literal` part (passed through unchanged)
        // and an `Interpolation` part (recursed into) in the same tree.
        assert_eq!(
            rename_name_in_expr(
                HirExpr::FString(vec![
                    FStringPart::Literal("text".to_string()),
                    FStringPart::Interpolation(Box::new(HirExpr::Name("old".to_string()))),
                ]),
                "old",
                "new",
            ),
            HirExpr::FString(vec![
                FStringPart::Literal("text".to_string()),
                FStringPart::Interpolation(Box::new(HirExpr::Name("new".to_string()))),
            ])
        );

        // ListLiteral: renames every element.
        assert_eq!(
            rename_name_in_expr(
                HirExpr::ListLiteral(vec![HirExpr::Name("old".to_string())]),
                "old",
                "new",
            ),
            HirExpr::ListLiteral(vec![HirExpr::Name("new".to_string())])
        );

        // Subscript: renames both base and index.
        assert_eq!(
            rename_name_in_expr(
                HirExpr::Subscript {
                    base: Box::new(HirExpr::Name("old".to_string())),
                    index: Box::new(HirExpr::Name("old".to_string())),
                },
                "old",
                "new",
            ),
            HirExpr::Subscript {
                base: Box::new(HirExpr::Name("new".to_string())),
                index: Box::new(HirExpr::Name("new".to_string())),
            }
        );

        // Slice: renames `base` and every present bound. Only the `Some`
        // side of each `Option::map` is exercised here -- the `None` side
        // has no closure body of its own to cover (it is just the same
        // `start`/`stop`/`step` value flowing through unchanged), and is
        // separately pinned by `lowers_a_slice_expression_with_all_bounds_omitted`
        // above at the `lower_expr` level.
        assert_eq!(
            rename_name_in_expr(
                HirExpr::Slice {
                    base: Box::new(HirExpr::Name("old".to_string())),
                    start: Some(Box::new(HirExpr::Name("old".to_string()))),
                    stop: Some(Box::new(HirExpr::Name("old".to_string()))),
                    step: Some(Box::new(HirExpr::Name("old".to_string()))),
                },
                "old",
                "new",
            ),
            HirExpr::Slice {
                base: Box::new(HirExpr::Name("new".to_string())),
                start: Some(Box::new(HirExpr::Name("new".to_string()))),
                stop: Some(Box::new(HirExpr::Name("new".to_string()))),
                step: Some(Box::new(HirExpr::Name("new".to_string()))),
            }
        );

        // ListAppend: covers both the `list` field matching `from` and not
        // matching it, plus renaming `value` in both cases.
        assert_eq!(
            rename_name_in_expr(
                HirExpr::ListAppend {
                    list: "old".to_string(),
                    value: Box::new(HirExpr::Name("old".to_string())),
                },
                "old",
                "new",
            ),
            HirExpr::ListAppend {
                list: "new".to_string(),
                value: Box::new(HirExpr::Name("new".to_string())),
            }
        );
        assert_eq!(
            rename_name_in_expr(
                HirExpr::ListAppend {
                    list: "other".to_string(),
                    value: Box::new(HirExpr::Name("old".to_string())),
                },
                "old",
                "new",
            ),
            HirExpr::ListAppend {
                list: "other".to_string(),
                value: Box::new(HirExpr::Name("new".to_string())),
            }
        );

        // DictLiteral: renames both key and value of every pair.
        assert_eq!(
            rename_name_in_expr(
                HirExpr::DictLiteral(vec![(
                    HirExpr::Name("old".to_string()),
                    HirExpr::Name("old".to_string()),
                )]),
                "old",
                "new",
            ),
            HirExpr::DictLiteral(vec![(
                HirExpr::Name("new".to_string()),
                HirExpr::Name("new".to_string()),
            )])
        );

        // SetLiteral: renames every element.
        assert_eq!(
            rename_name_in_expr(
                HirExpr::SetLiteral(vec![HirExpr::Name("old".to_string())]),
                "old",
                "new",
            ),
            HirExpr::SetLiteral(vec![HirExpr::Name("new".to_string())])
        );

        // TupleLiteral: renames every element.
        assert_eq!(
            rename_name_in_expr(
                HirExpr::TupleLiteral(vec![HirExpr::Name("old".to_string())]),
                "old",
                "new",
            ),
            HirExpr::TupleLiteral(vec![HirExpr::Name("new".to_string())])
        );

        // ListPop: covers both the `list` field matching `from` and not
        // matching it (PR-12 Task 10, D-119; mirrors `ListAppend` above).
        assert_eq!(
            rename_name_in_expr(
                HirExpr::ListPop {
                    list: "old".to_string(),
                },
                "old",
                "new",
            ),
            HirExpr::ListPop {
                list: "new".to_string(),
            }
        );
        assert_eq!(
            rename_name_in_expr(
                HirExpr::ListPop {
                    list: "other".to_string(),
                },
                "old",
                "new",
            ),
            HirExpr::ListPop {
                list: "other".to_string(),
            }
        );

        // DictGetOrDefault: covers both the `dict` field matching `from` and
        // not matching it, plus renaming `key`/`default` in both cases.
        assert_eq!(
            rename_name_in_expr(
                HirExpr::DictGetOrDefault {
                    dict: "old".to_string(),
                    key: Box::new(HirExpr::Name("old".to_string())),
                    default: Box::new(HirExpr::Name("old".to_string())),
                },
                "old",
                "new",
            ),
            HirExpr::DictGetOrDefault {
                dict: "new".to_string(),
                key: Box::new(HirExpr::Name("new".to_string())),
                default: Box::new(HirExpr::Name("new".to_string())),
            }
        );
        assert_eq!(
            rename_name_in_expr(
                HirExpr::DictGetOrDefault {
                    dict: "other".to_string(),
                    key: Box::new(HirExpr::Name("old".to_string())),
                    default: Box::new(HirExpr::Name("old".to_string())),
                },
                "old",
                "new",
            ),
            HirExpr::DictGetOrDefault {
                dict: "other".to_string(),
                key: Box::new(HirExpr::Name("new".to_string())),
                default: Box::new(HirExpr::Name("new".to_string())),
            }
        );

        // SetAdd: covers both the `set` field matching `from` and not
        // matching it, plus renaming `value` in both cases.
        assert_eq!(
            rename_name_in_expr(
                HirExpr::SetAdd {
                    set: "old".to_string(),
                    value: Box::new(HirExpr::Name("old".to_string())),
                },
                "old",
                "new",
            ),
            HirExpr::SetAdd {
                set: "new".to_string(),
                value: Box::new(HirExpr::Name("new".to_string())),
            }
        );
        assert_eq!(
            rename_name_in_expr(
                HirExpr::SetAdd {
                    set: "other".to_string(),
                    value: Box::new(HirExpr::Name("old".to_string())),
                },
                "old",
                "new",
            ),
            HirExpr::SetAdd {
                set: "other".to_string(),
                value: Box::new(HirExpr::Name("new".to_string())),
            }
        );

        // AttrGet (D-154): renames `base`, `attr` is untouched (it names a
        // field, never a local variable this rename could shadow).
        assert_eq!(
            rename_name_in_expr(
                HirExpr::AttrGet {
                    base: Box::new(HirExpr::Name("old".to_string())),
                    attr: "x".to_string(),
                },
                "old",
                "new",
            ),
            HirExpr::AttrGet {
                base: Box::new(HirExpr::Name("new".to_string())),
                attr: "x".to_string(),
            }
        );

        // MethodCall (D-154): renames `base` and every argument; `method`
        // is untouched for the same reason `attr` is above.
        assert_eq!(
            rename_name_in_expr(
                HirExpr::MethodCall {
                    base: Box::new(HirExpr::Name("old".to_string())),
                    method: "bump".to_string(),
                    args: vec![HirExpr::Name("old".to_string())],
                },
                "old",
                "new",
            ),
            HirExpr::MethodCall {
                base: Box::new(HirExpr::Name("new".to_string())),
                method: "bump".to_string(),
                args: vec![HirExpr::Name("new".to_string())],
            }
        );

        // Super (#433): carries no names to rename — returned unchanged.
        assert_eq!(
            rename_name_in_expr(HirExpr::Super, "old", "new"),
            HirExpr::Super
        );
    }

    // -- D-135: `type` statement and legacy `TypeAlias` -------------------

    #[test]
    fn a_type_statement_resolves_the_alias_in_a_later_parameter_annotation() {
        let module = pycc_parser_test_helper::parse(
            "type IntAlias = int\n\
             def f(x: IntAlias) -> int:\n\
             \x20   return x\n",
        );
        let hir = lower_checked(&module).unwrap();

        // Zero HIR footprint: the `type` statement itself contributes no
        // `HirItem` -- the only item present is the function it fed, and its
        // `x` parameter resolved to `Ty::Int` through the alias.
        assert_eq!(
            hir.items,
            vec![HirItem::Function {
                name: "f".to_string(),
                params: vec![("x".to_string(), Ty::Int)],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
            }]
        );
        assert_eq!(hir.type_aliases, vec![("IntAlias".to_string(), Ty::Int)]);
    }

    #[test]
    fn a_legacy_type_alias_annotated_assignment_resolves_the_alias_the_same_way() {
        let module = pycc_parser_test_helper::parse(
            "IntAlias: TypeAlias = int\n\
             def f(x: IntAlias) -> int:\n\
             \x20   return x\n",
        );
        let hir = lower_checked(&module).unwrap();

        // Same zero-HIR-footprint contract as the `type` statement form:
        // the legacy annotated assignment contributes no `HirItem` either.
        assert_eq!(
            hir.items,
            vec![HirItem::Function {
                name: "f".to_string(),
                params: vec![("x".to_string(), Ty::Int)],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
            }]
        );
        assert_eq!(hir.type_aliases, vec![("IntAlias".to_string(), Ty::Int)]);
    }

    #[test]
    fn a_generic_type_alias_is_rejected_with_t0042() {
        let module = pycc_parser_test_helper::parse("type Alias[T] = list[T]\n");
        let diagnostic = lower_checked(&module).unwrap_err();

        assert_eq!(diagnostic.code, "T0042");
    }

    #[test]
    fn a_generic_type_alias_t0042_span_points_at_the_type_statement_not_byte_zero() {
        // The `type` statement is deliberately not the first line, so a
        // regression back to a hardcoded `Span::new(0, 0)` would be caught:
        // byte 0 falls inside the preceding `def f() -> int:` line, not the
        // `type Alias[T] = int` statement this diagnostic is actually about.
        let source = "def f() -> int:\n    return 1\ntype Alias[T] = int\n";
        let type_stmt_start = source.find("type Alias").unwrap() as u32;
        let type_stmt_end = source.rfind('\n').unwrap() as u32;

        let module = pycc_parser_test_helper::parse(source);
        let diagnostic = lower_checked(&module).unwrap_err();

        assert_eq!(diagnostic.code, "T0042");
        assert_ne!(diagnostic.span, Some(Span::new(0, 0)));
        assert_eq!(
            diagnostic.span,
            Some(Span::new(type_stmt_start, type_stmt_end))
        );
    }

    #[test]
    fn an_unresolvable_type_alias_rhs_falls_through_to_the_existing_c0001_diagnostic() {
        let module = pycc_parser_test_helper::parse("type Bad = NotARealType\n");
        let diagnostic = lower_checked(&module).unwrap_err();

        assert_eq!(diagnostic.code, "C0001");
    }

    #[test]
    fn an_unresolvable_legacy_type_alias_rhs_also_falls_through_to_c0001() {
        let module = pycc_parser_test_helper::parse("Bad: TypeAlias = NotARealType\n");
        let diagnostic = lower_checked(&module).unwrap_err();

        assert_eq!(diagnostic.code, "C0001");
    }

    #[test]
    fn a_bare_type_alias_annotation_with_no_value_is_not_treated_as_an_alias() {
        // `X: TypeAlias` with no assigned value can't define an alias (there
        // is no RHS to resolve) -- it falls through to the ordinary
        // `AnnAssign` lowering path, where `annotation_to_ty` rejects the
        // bare name `TypeAlias` itself with the same `C0001` catch-all as
        // any other unrecognized annotation name, and the alias table stays
        // empty.
        let module = pycc_parser_test_helper::parse("X: TypeAlias\n");
        let diagnostic = lower_checked(&module).unwrap_err();

        assert_eq!(diagnostic.code, "C0001");
    }

    #[test]
    fn a_legacy_type_alias_annotation_on_a_non_name_target_is_not_treated_as_an_alias() {
        // Unlike a `type` statement's own target (always a bare name, see
        // `lower_type_alias_stmt`'s doc comment), a legacy `AnnAssign`
        // target can be an `Attribute`/`Subscript`, e.g. `obj.x: TypeAlias =
        // int`. `lower_legacy_type_alias_ann_assign` recognizes the `X:
        // TypeAlias = ...` shape only for a bare-name target and otherwise
        // falls through to the ordinary `AnnAssign` lowering path, which
        // rejects a non-name annotated-assignment target with the same
        // `C0001` catch-all it already uses for every other non-name
        // target -- the alias table stays empty either way.
        let module = pycc_parser_test_helper::parse("obj.x: TypeAlias = int\n");
        let diagnostic = lower_checked(&module).unwrap_err();

        assert_eq!(diagnostic.code, "C0001");
    }

    #[test]
    fn import_math_binds_the_module_namespace() {
        let module = pycc_parser_test_helper::parse("import math\n");
        let hir = lower_checked(&module).expect("recognized stdlib import must lower");

        assert_eq!(
            hir.imports,
            vec![ImportBinding::Module {
                local_name: "math".to_string(),
                module: pycc_std::StdModule::Math,
            }]
        );
        assert!(hir.items.is_empty(), "a bare `import math` has no HirItem");
    }

    #[test]
    fn import_cgi_is_c0001() {
        let module = pycc_parser_test_helper::parse("import cgi\n");
        let diagnostic = lower_checked(&module).unwrap_err();

        assert_eq!(diagnostic.code, "C0001");
    }

    #[test]
    fn from_math_import_sqrt_and_pi_binds_both_names() {
        let module = pycc_parser_test_helper::parse("from math import sqrt, pi\n");
        let hir = lower_checked(&module).expect("both registered symbols must resolve");

        let sqrt_symbol = pycc_std::resolve_symbol(pycc_std::StdModule::Math, "sqrt")
            .expect("math.sqrt is registered");
        let pi_symbol = pycc_std::resolve_symbol(pycc_std::StdModule::Math, "pi")
            .expect("math.pi is registered");
        assert_eq!(
            hir.imports,
            vec![
                ImportBinding::Symbol {
                    local_name: "sqrt".to_string(),
                    module: pycc_std::StdModule::Math,
                    symbol: sqrt_symbol,
                },
                ImportBinding::Symbol {
                    local_name: "pi".to_string(),
                    module: pycc_std::StdModule::Math,
                    symbol: pi_symbol,
                },
            ]
        );
    }

    #[test]
    fn from_math_import_sqrt_and_unregistered_tan_is_c0002_not_a_partial_bind() {
        let module = pycc_parser_test_helper::parse("from math import sqrt, tan\n");
        let diagnostic = lower_checked(&module).unwrap_err();

        // Whole statement fails closed -- `sqrt` is not partially bound
        // even though it is itself registered.
        assert_eq!(diagnostic.code, "C0002");
    }

    #[test]
    fn from_enum_import_enum_binds_enum_marker() {
        let module = pycc_parser_test_helper::parse("from enum import Enum\n");
        let hir = lower_checked(&module).expect("enum.Enum must resolve");

        let enum_symbol = pycc_std::resolve_symbol(pycc_std::StdModule::Enum, "Enum")
            .expect("enum.Enum is registered");
        assert_eq!(
            hir.imports,
            vec![ImportBinding::Symbol {
                local_name: "Enum".to_string(),
                module: pycc_std::StdModule::Enum,
                symbol: enum_symbol,
            }]
        );
    }

    #[test]
    fn import_math_as_m_is_c0001() {
        let module = pycc_parser_test_helper::parse("import math as m\n");
        let diagnostic = lower_checked(&module).unwrap_err();

        assert_eq!(diagnostic.code, "C0001");
    }

    #[test]
    fn from_math_import_sqrt_as_s_is_c0001() {
        let module = pycc_parser_test_helper::parse("from math import sqrt as s\n");
        let diagnostic = lower_checked(&module).unwrap_err();

        assert_eq!(diagnostic.code, "C0001");
    }

    #[test]
    fn from_unregistered_module_import_is_c0001() {
        let module = pycc_parser_test_helper::parse("from os import path\n");
        let diagnostic = lower_checked(&module).unwrap_err();

        assert_eq!(diagnostic.code, "C0001");
    }

    #[test]
    fn from_dot_import_x_is_c0001() {
        let module = pycc_parser_test_helper::parse("from . import x\n");
        let diagnostic = lower_checked(&module).unwrap_err();

        assert_eq!(diagnostic.code, "C0001");
    }

    #[test]
    fn from_math_import_star_is_c0001() {
        let module = pycc_parser_test_helper::parse("from math import *\n");
        let diagnostic = lower_checked(&module).unwrap_err();

        assert_eq!(diagnostic.code, "C0001");
    }

    #[test]
    fn import_two_modules_in_one_statement_is_c0001() {
        let module = pycc_parser_test_helper::parse("import math, os\n");
        let diagnostic = lower_checked(&module).unwrap_err();

        assert_eq!(diagnostic.code, "C0001");
    }

    #[test]
    fn math_sqrt_call_lowers_to_a_qualified_callee() {
        let module = pycc_parser_test_helper::parse("import math\nprint(math.sqrt(2.0))\n");
        let hir = lower_checked(&module).expect("math.sqrt(...) must lower");

        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::Call {
                    callee: "math.sqrt".to_string(),
                    args: vec![HirExpr::FloatLiteral(2.0)],
                }],
            }))]
        );
    }

    #[test]
    fn math_pi_bare_reference_lowers_to_a_qualified_name() {
        let module = pycc_parser_test_helper::parse("import math\nprint(math.pi)\n");
        let hir = lower_checked(&module).expect("math.pi must lower");

        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::Name("math.pi".to_string())],
            }))]
        );
    }

    #[test]
    fn math_tan_call_is_unsupported_since_it_is_not_registered() {
        let module = pycc_parser_test_helper::parse("import math\nmath.tan(1.0)\n");
        let diagnostic = lower_checked(&module).unwrap_err();

        assert_eq!(diagnostic.code, "C0001");
    }

    #[test]
    fn math_tan_bare_reference_is_unsupported_since_it_is_not_registered() {
        let module = pycc_parser_test_helper::parse("import math\nprint(math.tan)\n");
        let diagnostic = lower_checked(&module).unwrap_err();

        assert_eq!(diagnostic.code, "C0001");
    }

    #[test]
    fn os_path_bare_attribute_access_lowers_to_a_generic_attr_get() {
        // No `import os` here on purpose -- `os` isn't a registered
        // `pycc_std` module at all, so this exercises the "receiver name
        // does not resolve to a stdlib module" branch directly, distinct
        // from `math_tan_bare_reference_is_unsupported_since_it_is_not_registered`
        // above (recognized module, unregistered attribute -- which stays
        // `C0001`, see that test and `math_tan_call_is_unsupported_since_it_is_not_registered`'s
        // own updated comment). Before D-154 (Part 1 of #375), a receiver
        // that didn't resolve to a stdlib module made *any* attribute
        // access `C0001` unconditionally; D-154 adds a generic
        // instance-attribute-read shape (`HirExpr::AttrGet`) that this now
        // falls through to instead, deferring real rejection (`os` is
        // never assigned, so it isn't a class instance either) to
        // `pycc_types`.
        let module = pycc_parser_test_helper::parse("print(os.path)\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::AttrGet {
                    base: Box::new(HirExpr::Name("os".to_string())),
                    attr: "path".to_string(),
                }],
            }))]
        );
    }

    #[test]
    fn attribute_access_on_a_non_name_receiver_lowers_to_a_generic_attr_get() {
        // Same D-154 widening as the `os.path` test above, exercised on a
        // receiver that isn't even a bare name (a list literal) -- `base`
        // is lowered generically regardless of its own shape.
        let module = pycc_parser_test_helper::parse("print([1, 2].sqrt)\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::AttrGet {
                    base: Box::new(HirExpr::ListLiteral(vec![
                        HirExpr::IntLiteral(1),
                        HirExpr::IntLiteral(2)
                    ])),
                    attr: "sqrt".to_string(),
                }],
            }))]
        );
    }

    #[test]
    fn math_sqrt_call_propagates_an_unsupported_argument_expression() {
        // Exercises the `?` inside the stdlib-call arm's own argument
        // lowering (`call.arguments.args.iter().map(lower_expr).collect()`)
        // taking its error path, as opposed to every other stdlib-call test
        // above, which only exercises the success path.
        let module = pycc_parser_test_helper::parse("import math\nmath.sqrt(1j)\n");
        let diagnostic = lower_checked(&module).unwrap_err();

        assert_eq!(diagnostic.code, "C0001");
    }

    #[test]
    fn method_call_propagates_an_unsupported_base_expression() {
        // Exercises the `?` inside `MethodCall`'s own `base` lowering
        // (D-154), as opposed to `method_call_on_a_non_name_receiver_lowers_to_a_generic_method_call`
        // below, which only exercises the success path for a non-name base.
        let module = pycc_parser_test_helper::parse("(1j).bump()\n");
        let diagnostic = lower_checked(&module).unwrap_err();

        assert_eq!(diagnostic.code, "C0001");
    }

    #[test]
    fn method_call_propagates_an_unsupported_argument_expression() {
        // Exercises the `?` inside `MethodCall`'s own argument lowering
        // (D-154), mirroring `math_sqrt_call_propagates_an_unsupported_argument_expression`
        // above.
        let module = pycc_parser_test_helper::parse("p.bump(1j)\n");
        let diagnostic = lower_checked(&module).unwrap_err();

        assert_eq!(diagnostic.code, "C0001");
    }

    #[test]
    fn attr_get_propagates_an_unsupported_base_expression() {
        // Exercises the `?` inside `AttrGet`'s own `base` lowering (D-154).
        let module = pycc_parser_test_helper::parse("(1j).x\n");
        let diagnostic = lower_checked(&module).unwrap_err();

        assert_eq!(diagnostic.code, "C0001");
    }

    #[test]
    fn method_call_on_a_non_name_receiver_lowers_to_a_generic_method_call() {
        // Exercises the call-position stdlib-intrinsic branch's own
        // `Expr::Name(receiver)` guard failing (as opposed to the bare
        // attribute-access arm's analogous guard above) -- before D-154
        // (Part 1 of #375) this was unconditionally `C0001`; now it falls
        // through to the generic `HirExpr::MethodCall` shape instead, same
        // as `os_path_bare_attribute_access_lowers_to_a_generic_attr_get`
        // above.
        let module = pycc_parser_test_helper::parse("[1, 2].sqrt()\n");
        let hir = lower_checked(&module).unwrap();
        assert_eq!(
            hir.items,
            vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(
                HirExpr::MethodCall {
                    base: Box::new(HirExpr::ListLiteral(vec![
                        HirExpr::IntLiteral(1),
                        HirExpr::IntLiteral(2)
                    ])),
                    method: "sqrt".to_string(),
                    args: vec![],
                }
            ))]
        );
    }

    #[test]
    fn import_inside_a_function_body_is_c0001() {
        // The module-level side-table is populated only by `lower_checked`'s
        // top-level loop (mirroring `type_aliases`); a nested import still
        // reaches plain `lower_stmt`, which has no arm for `Stmt::Import`.
        let module =
            pycc_parser_test_helper::parse("def f() -> None:\n    import math\n    return None\n");
        let diagnostic = lower_checked(&module).unwrap_err();

        assert_eq!(diagnostic.code, "C0001");
    }

    // -- PEP 695 generic class instantiation (#387) -----------------------

    /// Helper: lowers source that defines a generic class `C[T]` and then
    /// uses `C[<type_arg>](<args>)` at module scope, returning the lowered
    /// HIR so the test can inspect the `GenericClassInstantiate` expression.
    fn lower_generic_class_instantiation(source: &str) -> crate::HirModule {
        let module = pycc_parser_test_helper::parse(source);
        lower_checked(&module).expect("test fixture should lower successfully")
    }

    #[test]
    fn generic_class_instantiation_lowers_with_int_type_arg() {
        let hir = lower_generic_class_instantiation(
            "class C[T]:\n    def __init__(self, x: T) -> None:\n        self.x = x\nC[int](1)\n",
        );
        // The last item should be a top-level ExprStmt wrapping a
        // GenericClassInstantiate with class "C", type_arg Int, and one arg.
        let last = hir.items.last().expect("should have items");
        assert!(matches!(
            last,
            HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::GenericClassInstantiate {
                class,
                type_arg,
                args,
            })) if class == "C" && *type_arg == Ty::Int && args.len() == 1 && args[0] == HirExpr::IntLiteral(1)
        ));
    }

    #[test]
    fn generic_class_instantiation_lowers_with_float_bool_and_str_type_args() {
        for (source, expected_ty) in [
            (
                "class C[T]:\n    def __init__(self, x: T) -> None:\n        self.x = x\nC[float](1)\n",
                Ty::Float,
            ),
            (
                "class C[T]:\n    def __init__(self, x: T) -> None:\n        self.x = x\nC[bool](1)\n",
                Ty::Bool,
            ),
            (
                "class C[T]:\n    def __init__(self, x: T) -> None:\n        self.x = x\nC[str](1)\n",
                Ty::Str,
            ),
        ] {
            let hir = lower_generic_class_instantiation(source);
            let last = hir.items.last().expect("should have items");
            assert!(matches!(
                last,
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::GenericClassInstantiate {
                    type_arg, ..
                })) if *type_arg == expected_ty
            ));
        }
    }

    #[test]
    fn generic_class_instantiation_rejects_a_non_name_type_arg() {
        // `C[1](args)` — the slice is a number literal, not a bare name.
        assert_capability_error_message(
            "class C[T]:\n    def __init__(self, x: T) -> None:\n        self.x = x\nC[1](1)\n",
            "a generic class type argument must be a bare type name",
        );
    }

    #[test]
    fn generic_class_instantiation_rejects_an_unrecognized_type_arg_name() {
        // `C[unknown](args)` — the name is not one of int/float/bool/str.
        assert_capability_error_message(
            "class C[T]:\n    def __init__(self, x: T) -> None:\n        self.x = x\nC[unknown](1)\n",
            "a generic class type argument `unknown` is not supported yet",
        );
    }

    #[test]
    fn generic_class_instantiation_rejects_a_non_name_subscript_base() {
        // `(1 + 2)[int](args)` — the subscript base is a BinOp, not a bare
        // name, so the "calling a subscript expression" rejection fires.
        assert_capability_error_message(
            "(1 + 2)[int](1)\n",
            "calling a subscript expression is not supported yet",
        );
    }

    #[test]
    fn generic_class_instantiation_propagates_an_arg_lowering_error() {
        // `C[int](lambda: 1)` — the arg `lambda: 1` is an unsupported
        // expression that `lower_expr` rejects. This exercises the `?` on
        // the `.collect::<Result<Vec<_>, _>>()?` at expr.rs line 352,
        // propagating the error from the arg's `lower_expr` call.
        assert_capability_error_message(
            "class C[T]:\n    def __init__(self, x: T) -> None:\n        self.x = x\nC[int](lambda: 1)\n",
            "expression kind not supported yet",
        );
    }

    #[test]
    fn rename_name_in_expr_handles_generic_class_instantiate_in_comprehension() {
        // A list comprehension whose `elt` is a `GenericClassInstantiate`
        // expression exercises `rename_name_in_expr`'s
        // `GenericClassInstantiate` arm: the loop variable `x` inside the
        // instantiation's args is renamed to the comprehension's synthesized
        // variable. The expression must lower successfully and produce a
        // `ListCompAssign` whose `elt` is a `GenericClassInstantiate`.
        let hir = lower_generic_class_instantiation(
            "class C[T]:\n    def __init__(self, x: T) -> None:\n        self.x = x\nxs = [C[int](x) for x in range(3)]\n",
        );
        // Find the ListCompAssign statement.
        let comp = hir.items.iter().find_map(|item| match item {
            HirItem::TopLevelStmt(HirStmt::ListCompAssign { elt, .. }) => Some(elt.clone()),
            _ => None,
        });
        let elt = comp.expect("should find a ListCompAssign");
        // The elt should be a GenericClassInstantiate — proving
        // rename_name_in_expr's GenericClassInstantiate arm was traversed.
        assert!(
            matches!(elt.as_ref(), HirExpr::GenericClassInstantiate { class, type_arg, .. }
                if class == "C" && *type_arg == Ty::Int),
            "expected GenericClassInstantiate elt, got {elt:?}",
        );
    }

    // #433: super() lowering tests.

    #[test]
    fn super_init_lowers_to_method_call_with_super_base() {
        // `super().__init__()` inside a method body lowers to
        // `HirExpr::MethodCall { base: Super, method: "__init__", args: [] }`.
        let module = pycc_parser_test_helper::parse(
            "class A:\n    def __init__(self) -> None:\n        return\nclass B(A):\n    def __init__(self) -> None:\n        super().__init__()\n",
        );
        let hir = lower_checked(&module).unwrap();
        // Find B.__init__'s body.
        let init = hir.items.iter().find_map(|item| match item {
            HirItem::Function { name, body, .. } if name == "B.__init__" => {
                Some(body.first().cloned())
            }
            _ => None,
        });
        let stmt = init
            .flatten()
            .expect("should find B.__init__ with a non-empty body");
        assert_eq!(
            stmt,
            HirStmt::ExprStmt(HirExpr::MethodCall {
                base: Box::new(HirExpr::Super),
                method: "__init__".to_string(),
                args: vec![],
            })
        );
    }

    #[test]
    fn super_method_lowers_to_method_call_with_super_base() {
        // `super().greet()` inside a method body lowers to
        // `HirExpr::MethodCall { base: Super, method: "greet", args: [] }`.
        let module = pycc_parser_test_helper::parse(
            "class A:\n    def __init__(self) -> None:\n        return\n    def greet(self) -> int:\n        return 1\nclass B(A):\n    def __init__(self) -> None:\n        return\n    def greet(self) -> int:\n        return super().greet()\n",
        );
        let hir = lower_checked(&module).unwrap();
        let greet = hir.items.iter().find_map(|item| match item {
            HirItem::Function { name, body, .. } if name == "B.greet" => body.first().cloned(),
            _ => None,
        });
        let stmt = greet.expect("should find B.greet with a non-empty body");
        assert_eq!(
            stmt,
            HirStmt::Return(Some(HirExpr::MethodCall {
                base: Box::new(HirExpr::Super),
                method: "greet".to_string(),
                args: vec![],
            }))
        );
    }

    #[test]
    fn super_attr_lowers_to_attr_get_with_super_base() {
        // `super().x` inside a method body lowers to
        // `HirExpr::AttrGet { base: Super, attr: "x" }`.
        let module = pycc_parser_test_helper::parse(
            "class A:\n    def __init__(self) -> None:\n        self.x = 1\nclass B(A):\n    def __init__(self) -> None:\n        super().__init__()\n    def get_x(self) -> int:\n        return super().x\n",
        );
        let hir = lower_checked(&module).unwrap();
        let get_x = hir.items.iter().find_map(|item| match item {
            HirItem::Function { name, body, .. } if name == "B.get_x" => body.first().cloned(),
            _ => None,
        });
        let stmt = get_x.expect("should find B.get_x with a non-empty body");
        assert_eq!(
            stmt,
            HirStmt::Return(Some(HirExpr::AttrGet {
                base: Box::new(HirExpr::Super),
                attr: "x".to_string(),
            }))
        );
    }

    #[test]
    fn bare_super_outside_method_is_c0001() {
        // A bare `super()` at top level is rejected with C0001.
        let module = pycc_parser_test_helper::parse("x = super()\n");
        let err = lower_checked(&module).unwrap_err();
        assert_eq!(err.code, "C0001");
        assert!(
            err.message.contains("bare `super()`"),
            "should mention bare super(), got: {}",
            err.message
        );
    }

    #[test]
    fn super_with_arguments_is_not_zero_arg_super() {
        // `super(A, self)` (two-arg super) is not a zero-arg super() call,
        // so `is_zero_arg_super_call` returns false — it falls through to
        // the ordinary `Expr::Call` path, which lowers `super(A, self)` as
        // a regular call to the `super` builtin. The type checker then
        // rejects it with C0001 ("call to builtin `super` is valid Python
        // but not implemented yet"). This test verifies the HIR lowering
        // succeeds (the rejection happens later, at type-check time).
        let module = pycc_parser_test_helper::parse(
            "class A:\n    def __init__(self) -> None:\n        super(A, self).__init__()\n",
        );
        let hir = lower_checked(&module).unwrap();
        // The base of the MethodCall should be a Call to "super", not a Super.
        assert_eq!(
            hir.items,
            vec![HirItem::Function {
                name: "A.__init__".to_string(),
                params: vec![("self".to_string(), Ty::Instance(Box::new("A".to_string())))],
                return_ty: Ty::None,
                body: vec![HirStmt::ExprStmt(HirExpr::MethodCall {
                    base: Box::new(HirExpr::Call {
                        callee: "super".to_string(),
                        args: vec![
                            HirExpr::Name("A".to_string()),
                            HirExpr::Name("self".to_string()),
                        ],
                    }),
                    method: "__init__".to_string(),
                    args: vec![],
                })],
            }]
        );
    }

    #[test]
    fn super_method_outside_method_is_c0001() {
        // `super().foo()` at top level (outside a method body) is rejected
        // with C0001 at HIR-lowering time.
        let module = pycc_parser_test_helper::parse("x = super().foo()\n");
        let err = lower_checked(&module).unwrap_err();
        assert_eq!(err.code, "C0001");
    }

    #[test]
    fn super_attr_outside_method_is_c0001() {
        // `super().foo` at top level (outside a method body) is rejected
        // with C0001 at HIR-lowering time.
        let module = pycc_parser_test_helper::parse("x = super().foo\n");
        let err = lower_checked(&module).unwrap_err();
        assert_eq!(err.code, "C0001");
    }

    #[test]
    fn super_method_with_unsupported_arg_is_c0001() {
        // `super().foo(x if True else y)` — the ternary argument is an
        // unsupported expression kind, so `lower_expr` on the argument
        // returns Err, which the `?` in the super().method() lowering path
        // propagates as C0001.
        let module = pycc_parser_test_helper::parse(
            "class A:\n    def __init__(self) -> None:\n        super().foo(x if True else y)\n",
        );
        let err = lower_checked(&module).unwrap_err();
        assert_eq!(err.code, "C0001");
    }

    #[test]
    fn super_attr_assignment_is_c0001() {
        // #448: `super().attr = value` is rejected with a dedicated C0001
        // diagnostic that names super() attribute assignment, not the
        // confusing bare-super() message.
        let module = pycc_parser_test_helper::parse(
            "class A:\n    def __init__(self) -> None:\n        super().x = 1\n",
        );
        let err = lower_checked(&module).unwrap_err();
        assert_eq!(err.code, "C0001");
        assert!(
            err.message.contains("super().attr = value"),
            "should mention super().attr = value, got: {}",
            err.message
        );
        assert!(
            !err.message.contains("bare `super()`"),
            "should not use the bare-super() message, got: {}",
            err.message
        );
    }

    // -----------------------------------------------------------------------
    // #435: compile-time isinstance/issubclass helper unit tests
    // -----------------------------------------------------------------------

    #[test]
    fn eval_isinstance_single_covers_all_builtin_types() {
        // `float` target against `float` object — covers the `Ty::Float` arm.
        assert!(eval_isinstance_single(&Ty::Float, "float", &[]));
        assert!(!eval_isinstance_single(&Ty::Float, "int", &[]));
        // `None` object against any target — covers the `_ => false` arm.
        assert!(!eval_isinstance_single(&Ty::None, "int", &[]));
        // `Ty::Int` arm — true for "int", false for others.
        assert!(eval_isinstance_single(&Ty::Int, "int", &[]));
        assert!(!eval_isinstance_single(&Ty::Int, "str", &[]));
        // `Ty::Bool` arm — true for "bool" and "int" (subtype).
        assert!(eval_isinstance_single(&Ty::Bool, "bool", &[]));
        assert!(eval_isinstance_single(&Ty::Bool, "int", &[]));
        assert!(!eval_isinstance_single(&Ty::Bool, "str", &[]));
        // `Ty::Str` arm — true for "str", false for others.
        assert!(eval_isinstance_single(&Ty::Str, "str", &[]));
        assert!(!eval_isinstance_single(&Ty::Str, "int", &[]));
        // `Ty::Instance` arm — checks MRO membership.
        let mro = vec!["D".to_string(), "B".to_string(), "A".to_string()];
        assert!(eval_isinstance_single(
            &Ty::Instance(Box::new("D".to_string())),
            "D",
            &mro
        ));
        assert!(eval_isinstance_single(
            &Ty::Instance(Box::new("D".to_string())),
            "B",
            &mro
        ));
        assert!(eval_isinstance_single(
            &Ty::Instance(Box::new("D".to_string())),
            "A",
            &mro
        ));
        assert!(!eval_isinstance_single(
            &Ty::Instance(Box::new("D".to_string())),
            "C",
            &mro
        ));
    }

    #[test]
    fn eval_issubclass_single_covers_builtin_same_type_and_user_vs_builtin() {
        // Same builtin type — covers the `return cls == target_class` line.
        assert!(eval_issubclass_single("int", "int", &[]));
        assert!(eval_issubclass_single("str", "str", &[]));
        assert!(!eval_issubclass_single("int", "str", &[]));
        // User class vs builtin target — covers the `return false` line.
        assert!(!eval_issubclass_single("D", "int", &["D".to_string()]));
        // `bool` is a subtype of `int` — covers the `bool`/`int` special case.
        assert!(eval_issubclass_single("bool", "int", &[]));
        assert!(eval_issubclass_single("bool", "bool", &[]));
        assert!(!eval_issubclass_single("bool", "str", &[]));
        // User class MRO check — covers the `cls_mro.iter().any` line.
        let mro = vec!["D".to_string(), "B".to_string(), "A".to_string()];
        assert!(eval_issubclass_single("D", "D", &mro));
        assert!(eval_issubclass_single("D", "B", &mro));
        assert!(eval_issubclass_single("D", "A", &mro));
        assert!(!eval_issubclass_single("D", "C", &mro));
    }

    #[test]
    fn extract_class_names_rejects_empty_tuple_and_non_name_elements() {
        // Empty tuple — covers the `elements.is_empty()` error path.
        let result = extract_class_names(&HirExpr::TupleLiteral(vec![]));
        assert!(result.is_err());

        // Tuple with a non-name element — covers the `_ => return Err` path.
        let result = extract_class_names(&HirExpr::TupleLiteral(vec![HirExpr::IntLiteral(42)]));
        assert!(result.is_err());

        // Non-name, non-tuple expression — covers the top-level `_ => Err` path.
        let result = extract_class_names(&HirExpr::IntLiteral(99));
        assert!(result.is_err());

        // Valid single name.
        let result = extract_class_names(&HirExpr::Name("D".to_string()));
        assert_eq!(result.unwrap(), vec!["D".to_string()]);

        // Valid tuple of names.
        let result = extract_class_names(&HirExpr::TupleLiteral(vec![
            HirExpr::Name("B".to_string()),
            HirExpr::Name("C".to_string()),
        ]));
        assert_eq!(result.unwrap(), vec!["B".to_string(), "C".to_string()]);
    }

    #[test]
    fn is_builtin_type_name_recognizes_four_scalar_types() {
        assert!(is_builtin_type_name("int"));
        assert!(is_builtin_type_name("str"));
        assert!(is_builtin_type_name("float"));
        assert!(is_builtin_type_name("bool"));
        assert!(!is_builtin_type_name("D"));
        assert!(!is_builtin_type_name("object"));
    }
}

#[cfg(test)]
mod pycc_parser_test_helper {
    pub fn parse(source: &str) -> pycc_ast::ModModule {
        pycc_parser::parse(source).expect("test fixture must parse")
    }
}
