use crate::class::ClassAnnotationInfo;
use pycc_ast::{Expr, ModModule, Stmt};
use pycc_diag::{Diagnostic, Span};

mod class;
mod exception;
mod expr;
mod stmt;

pub use class::{HirClassDef, PropertyDef, ProtocolMember};
pub use exception::{
    BUILTIN_EXCEPTION_CLASSES, HirExceptHandler, builtin_exception_parent,
    is_builtin_exception_class,
};

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
    /// A protocol-typed value (PEP 544, #380, PR-20). Carries the protocol
    /// class name. Distinct from `Ty::Instance` so that `is_assignable` and
    /// every match arm can distinguish "this value is an instance of a
    /// concrete class" from "this value is typed as a protocol." A
    /// protocol-typed value's concrete representation is determined by the
    /// first assignment (D-040 sticky-representation rule) or by
    /// monomorphization at each call site (D-006/D-134). Boxed as
    /// `Box<String>`, mirroring `Ty::Instance`'s own documented reasoning
    /// exactly — maintains the D-109 16-byte `size_of::<Ty>()` ceiling.
    Protocol(Box<String>),
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
            Ty::Protocol(name) => name.to_string(),
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

/// The unary operators this compiler supports (#603, Part 2 of #573).
///
/// `not` and `~` are deliberately absent rather than present-but-rejected:
/// they are #604 (Part 3), and leaving them out of this enum keeps every
/// exhaustive match over it honest about what is actually lowerable today
/// instead of carrying arms that only ever return an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOpKind {
    /// `-x`.
    USub,
    /// `+x`. Not a no-op: `+True` is the integer `1` in Python, so a
    /// `bool` operand crosses into `int` here (see
    /// `pycc_types::unop::unary_result_type`).
    UAdd,
}

impl UnaryOpKind {
    /// The operator's Python source spelling, for diagnostics.
    pub fn as_str(self) -> &'static str {
        match self {
            UnaryOpKind::USub => "-",
            UnaryOpKind::UAdd => "+",
        }
    }
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
    /// `-operand` / `+operand` where `operand` is *not* a numeric literal
    /// (#603, Part 2 of #573).
    ///
    /// A literal operand never reaches this variant: `expr.rs`'s
    /// `Expr::UnaryOp` arm folds `-1` straight into `HirExpr::IntLiteral(-1)`
    /// first (#602, Part 1), so this node exists exactly for the cases
    /// folding cannot reach -- `-x`, `-f(y)`, `-(a + b)`.
    UnaryOp {
        op: UnaryOpKind,
        operand: Box<HirExpr>,
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

/// Returns `true` if `name` is the builtin `Protocol` base name recognized
/// by `lower_class` as a marker that a class is a PEP 544 protocol (#380,
/// PR-20). `Protocol` is not a user-defined class in `class_defs` -- it is
/// a builtin base name consumed as a marker, not recorded in the class's
/// `bases`/`mro`. `pycc_std` registers `typing.Protocol` as a
/// `ProtocolMarker` symbol so `from typing import Protocol` resolves (the
/// import is a no-op binding — `Protocol` is never a first-class value,
/// only a base class marker). The bare name `Protocol` (without any
/// import) is also accepted, matching pycc's existing textual-resolution
/// precedent for `Enum`.
pub fn is_protocol_base_name(name: &str) -> bool {
    name == "Protocol"
}

/// Returns `true` if `name` is the builtin `ABC` base name recognized by
/// `lower_class` as a marker that a class is abstract (PEP 3119, #380,
/// PR-20). `ABC` is not a user-defined class in `class_defs` -- it is a
/// builtin base name consumed as a marker, not recorded in the class's
/// `bases`/`mro`. `pycc_std` registers `abc.ABC` as an `AbcMarker` symbol
/// so `from abc import ABC` resolves (the import is a no-op binding —
/// `ABC` is never a first-class value, only a base class marker). The bare
/// name `ABC` (without any import) is also accepted, matching pycc's
/// existing textual-resolution precedent for `Enum`/`Protocol`.
pub fn is_abc_base_name(name: &str) -> bool {
    name == "ABC"
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
        /// PEP 591 (#383): `true` when the original annotation was
        /// `Final[X]`, meaning this binding may not be reassigned after its
        /// initial assignment. Set at HIR-lowering time in `stmt.rs`'s
        /// `Stmt::AnnAssign` arm by inspecting the raw AST annotation
        /// before `annotation_to_ty` unwraps `Final[X]` to `X`. The type
        /// checker's `Environment.finals` set is populated from this flag,
        /// and `check_assignment` rejects a reassignment of a `Final` name
        /// with `T0045`.
        is_final: bool,
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
    /// PEP 634-636 (#381, PR-21): structural pattern matching. The subject is
    /// evaluated once and matched against each case in order; the first
    /// matching case's body executes. See `HirPattern`/`HirMatchCase` for
    /// the per-case shape.
    Match {
        subject: HirExpr,
        cases: Vec<HirMatchCase>,
    },
    /// `try`/`except`/`else`/`finally` (PEP 3110, #382, PR-22 Part 1).
    /// The body is executed; if an exception is raised, each handler is
    /// checked in order. The `orelse` runs only if no exception was raised.
    /// The `finalbody` always runs, whether the try completed normally, an
    /// exception was caught, or an exception is propagating.
    Try {
        body: Vec<HirStmt>,
        handlers: Vec<HirExceptHandler>,
        orelse: Vec<HirStmt>,
        finalbody: Vec<HirStmt>,
    },
    /// `raise` (PEP 3110, #382). `exc` is the exception expression to raise;
    /// `None` means a bare re-raise (only valid inside an except handler).
    /// `cause` is the optional `raise ... from cause` expression (PEP 409).
    Raise {
        exc: Option<HirExpr>,
        cause: Option<HirExpr>,
    },
}

/// PEP 634-636 (#381, PR-21): a single `match` case's pattern, lowered from
/// `ruff_python_ast::Pattern`. The type checker verifies each pattern against
/// the subject's known type and collects capture bindings; the MIR pass
/// desugars each pattern into a match condition plus binding assignments.
#[derive(Debug, Clone, PartialEq)]
pub enum HirPattern {
    /// `case _:` — matches anything, binds nothing.
    Wildcard,
    /// `case x:` — matches anything, binds `x` to the subject.
    Capture(String),
    /// `case 42:` / `case "hello":` / `case 3.14:` — a literal value pattern.
    Literal(HirExpr),
    /// `case True:` / `case False:` — a bool singleton pattern.
    Singleton(bool),
    /// `case None:` — the `None` singleton pattern.
    NoneSingleton,
    /// `case [a, b, c]:` — a fixed-length sequence pattern.
    Sequence(Vec<HirPattern>),
    /// `case [a, *rest]:` — a sequence pattern with a star-capture. The
    /// leading `Vec` are the fixed patterns before the star; `rest` captures
    /// the remaining elements as a list.
    SequenceStar(Vec<HirPattern>, Option<String>),
    /// `case {"k": v}:` — a mapping pattern. Each pair is `(key_expr,
    /// value_pattern)`; `rest` captures the remaining dict.
    Mapping(Vec<(HirExpr, HirPattern)>, Option<String>),
    /// `case Point(0, 0):` / `case Point(x=0, y=0):` — a class pattern.
    /// `positional` matches `__init__`'s parameters (after `self`);
    /// `keyword` matches named attributes.
    Class {
        class_name: String,
        positional: Vec<HirPattern>,
        keyword: Vec<(String, HirPattern)>,
    },
    /// `case 1 | 2 | 3:` — an or-pattern. All sub-patterns must bind the
    /// same set of capture names (PEP 634).
    Or(Vec<HirPattern>),
    /// `case [a, b] as pair:` — an as-pattern. The inner pattern must
    /// match, and `name` is bound to the subject.
    As(Box<HirPattern>, String),
}

/// PEP 634-636 (#381, PR-21): a single `case` clause in a `match`
/// statement.
#[derive(Debug, Clone, PartialEq)]
pub struct HirMatchCase {
    pub pattern: HirPattern,
    pub guard: Option<HirExpr>,
    pub body: Vec<HirStmt>,
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
        // #380 (PR-20): build the projected class slice `annotation_to_ty`
        // uses to resolve cross-class annotations; #611 (PEP 560) added the
        // per-class subscriptability flag it carries.
        let class_name_defs = class::class_annotation_infos(&class_defs);
        if let Some((name, ty)) = lower_type_alias_stmt(stmt, &aliases, &class_name_defs)? {
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
        if let Some((name, ty)) =
            lower_legacy_type_alias_ann_assign(stmt, &aliases, &class_name_defs)?
        {
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
            Stmt::FunctionDef(def) => lower_function(def, &aliases, &class_name_defs)?,
            other => HirItem::TopLevelStmt(stmt::lower_stmt(
                other,
                &aliases,
                false,
                false,
                None,
                None,
                &class_name_defs,
            )?),
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
    class_defs: &[ClassAnnotationInfo],
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
    let ty = annotation_to_ty(&type_alias.value, None, None, aliases, class_defs)?;
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
    class_defs: &[ClassAnnotationInfo],
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
    let ty = annotation_to_ty(value, None, None, aliases, class_defs)?;
    Ok(Some((target.id.to_string(), ty)))
}

fn lower_function(
    def: &pycc_ast::StmtFunctionDef,
    aliases: &[(String, Ty)],
    class_defs: &[ClassAnnotationInfo],
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
        class_defs,
    )?;
    let return_ty = lower_return_annotation(
        def.returns.as_deref(),
        is_public,
        def.name.as_str(),
        type_param.as_deref(),
        None,
        aliases,
        class_defs,
    )?;
    let body = stmt::lower_body(
        &def.body,
        aliases,
        false,
        true,
        None,
        type_param.as_deref(),
        class_defs,
    )?;
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
    class_defs: &[ClassAnnotationInfo],
) -> Result<Vec<(String, Ty)>, Diagnostic> {
    // Every parameter kind and default value below is silently absent from
    // `parameters.args`/`ParameterWithDefault::default` -- an earlier version
    // of this function only ever iterated `.args` and never checked for any
    // of these, so a function using them got a wrong signature built from
    // whatever plain positional args happened to exist, instead of the
    // explicit capability diagnostic every other out-of-scope construct in
    // this file produces (self-review finding, pre-merge).
    //
    // PEP 570 (#383): positional-only parameters (`posonlyargs`, before the
    // `/` marker) are now lowered via the same `lower_arg_list` path as
    // ordinary `args`, prepended before `args` in the parameter list. Since
    // keyword call arguments are already globally unsupported (rejected in
    // `expr.rs`/`stmt.rs`), every parameter is already effectively
    // positional-only — accepting `posonlyargs` changes nothing about
    // call-site checking.
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
    // PEP 570 (#383): lower `posonlyargs` (before `/`) via the same
    // `lower_arg_list` path as ordinary `args`, prepending them. The full
    // parameter list is `posonlyargs ++ args`.
    let mut params = lower_arg_list(
        &parameters.posonlyargs,
        is_public,
        fn_name,
        type_param,
        None,
        aliases,
        class_defs,
    )?;
    params.extend(lower_arg_list(
        &parameters.args,
        is_public,
        fn_name,
        type_param,
        None,
        aliases,
        class_defs,
    )?);
    Ok(params)
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
    class_defs: &[ClassAnnotationInfo],
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
                    annotation_to_ty(ann, type_param, class_name, aliases, class_defs)?,
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
    class_defs: &[ClassAnnotationInfo],
) -> Result<Ty, Diagnostic> {
    match returns {
        Some(ann) => annotation_to_ty(ann, type_param, class_name, aliases, class_defs),
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
///
/// `class_defs` is the projected slice of already-defined classes
/// (#380, PR-20): a bare name matching a known class resolves to
/// `Ty::Instance` (or `Ty::Protocol` if the class is a protocol). This
/// fixes the pre-existing gap where cross-class annotations
/// (`def f(a: A) -> int` where `A` is a user-defined class) produced `C0001`.
/// Checked after the builtin/`Self`/self-referential arms and before the
/// alias table, so a class name takes priority over an alias of the same
/// name (matching Python's own scope rule where class definitions bind the
/// class name in the enclosing namespace).
pub(crate) fn annotation_to_ty(
    annotation: &Expr,
    type_param: Option<&str>,
    class_name: Option<&str>,
    aliases: &[(String, Ty)],
    class_defs: &[ClassAnnotationInfo],
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
            other => {
                // #380 (PR-20): resolve a bare name matching a known class
                // to `Ty::Instance` (or `Ty::Protocol` if the class is a
                // protocol). This fixes the pre-existing gap where
                // cross-class annotations (`def f(a: A) -> int` where `A`
                // is a user-defined class) produced `C0001`. Checked before
                // the alias table so a class name takes priority over an
                // alias of the same name.
                if let Some(info) = class_defs.iter().find(|info| info.name == other) {
                    if info.is_protocol {
                        return Ok(Ty::Protocol(Box::new(other.to_string())));
                    }
                    return Ok(Ty::Instance(Box::new(other.to_string())));
                }
                aliases
                    .iter()
                    .rev()
                    .find(|(alias_name, _)| alias_name == other)
                    .map(|(_, ty)| ty.clone())
                    .ok_or_else(|| {
                        unsupported(
                            format!("type annotation `{other}` is not supported yet"),
                            pycc_ast::expr_range(annotation),
                        )
                    })
            }
        },
        // Issue #435 (Part D, __class_getitem__): `ClassName[type_arg]` as a
        // type annotation (PEP 560). A class that defines `__class_getitem__`
        // allows subscript syntax in annotations. In pycc's static type
        // system, this resolves to `Ty::Instance(ClassName)` — the class
        // itself, ignoring the type argument for now (consistent with how
        // generic classes are handled by PEP 695's `GenericClassInstantiate`
        // for actual instantiation, not annotation). The base must be a bare
        // name (a class name); any other subscript shape is rejected.
        //
        // PEP 593 (#383): `Annotated[X, ...]` is recognized as a bare name
        // (no `from typing import Annotated` required, matching the existing
        // `TypeAlias`/`Any` precedent) and unwrapped to `X`, discarding all
        // metadata arguments. Per PEP 593's own spec, a static type checker
        // that does not understand a piece of metadata must treat
        // `Annotated[X, ...]` as `X` — this is correct, not a shortcut. The
        // first subscript argument is `X`; for the tuple form
        // `Annotated[X, meta1, meta2, ...]` the first element is `X`.
        // PEP 593 requires at least two arguments (the type and at least one
        // metadata element); `Annotated[X]` without metadata is rejected,
        // matching CPython's own `TypeError`.
        Expr::Subscript(sub) => {
            let Expr::Name(base_name) = sub.value.as_ref() else {
                return Err(unsupported(
                    "a subscripted type annotation's base must be a bare class name",
                    pycc_ast::expr_range(&sub.value),
                ));
            };
            match base_name.id.as_str() {
                "Annotated" => {
                    let Expr::Tuple(tuple) = sub.slice.as_ref() else {
                        return Err(unsupported(
                            "Annotated requires at least two arguments: the type and at least one metadata element",
                            pycc_ast::expr_range(&sub.slice),
                        ));
                    };
                    if tuple.elts.len() < 2 {
                        return Err(unsupported(
                            "Annotated requires at least two arguments: the type and at least one metadata element",
                            pycc_ast::expr_range(&sub.slice),
                        ));
                    }
                    annotation_to_ty(&tuple.elts[0], type_param, class_name, aliases, class_defs)
                }
                // PEP 591 (#383): `Final[X]` unwraps to `X`. `Final` is a
                // binding-level property (this name may not be reassigned),
                // not a type-level property — the type is just `X`. The
                // non-reassignability is tracked separately by the type
                // checker's `Environment.finals` set, populated from
                // `HirStmt::AnnAssign`'s `is_final` flag (set at lowering
                // time in `stmt.rs`). `Final` takes exactly one type
                // argument; `Final[X, Y]` is rejected.
                "Final" => {
                    let x = match sub.slice.as_ref() {
                        Expr::Tuple(tuple) if tuple.elts.len() != 1 => {
                            return Err(unsupported(
                                "Final takes exactly one type argument",
                                pycc_ast::expr_range(&sub.slice),
                            ));
                        }
                        Expr::Tuple(tuple) => &tuple.elts[0],
                        other => other,
                    };
                    annotation_to_ty(x, type_param, class_name, aliases, class_defs)
                }
                // The class name resolves the same way a bare-name annotation
                // does — through the alias table or as a known class name. We
                // reuse `annotation_to_ty` on the bare name so self-referential
                // class names, aliases, and builtin types all resolve
                // identically.
                //
                // PEP 560 (#611): before that, reject a subscript on a known
                // class that is not subscriptable. CPython raises
                // `TypeError: type 'C' is not subscriptable` for the same
                // program, and pycc's own value-position path (#610) already
                // reports it as `T0044` through `t0044_unknown_member`, so
                // this arm reuses that code rather than the surrounding
                // `C0001`. A name that is *not* a known class — a builtin
                // like `list`, an alias, or an undefined name — is left to
                // the recursion below, which reports it exactly as it does
                // today.
                _ => {
                    if let Some(info) = class_defs
                        .iter()
                        .find(|info| info.name == base_name.id.as_str())
                        && !info.subscriptable
                    {
                        return Err(Diagnostic::error(
                            "T0044",
                            format!(
                                "class `{}` does not define `__class_getitem__`, so \
                                 `{}[...]` is not a valid type annotation",
                                info.name, info.name
                            ),
                            {
                                let range = pycc_ast::expr_range(annotation);
                                Span::new(range.start, range.end)
                            },
                        ));
                    }
                    annotation_to_ty(
                        &Expr::Name(base_name.clone()),
                        type_param,
                        class_name,
                        aliases,
                        class_defs,
                    )
                }
            }
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
mod tests;

#[cfg(test)]
mod pycc_parser_test_helper {
    pub fn parse(source: &str) -> pycc_ast::ModModule {
        pycc_parser::parse(source).expect("test fixture must parse")
    }
}
