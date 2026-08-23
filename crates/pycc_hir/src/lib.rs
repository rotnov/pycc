use pycc_ast::{ModModule, Stmt};
use pycc_diag::{Diagnostic, Span};

mod class;
mod exception;
mod expr;
mod func;
mod import;
mod stmt;
mod typecheck;

pub use class::{HirClassDef, PropertyDef, ProtocolMember};
pub use exception::{
    BUILTIN_EXCEPTION_CLASSES, EXCEPTION_INIT_MANGLED_NAME, FIRST_USER_EXCEPTION_TYPE_TAG,
    HirExceptHandler, MAX_USER_EXCEPTION_CLASSES, builtin_exception_class_defs,
    builtin_exception_init_item, builtin_exception_parent, is_builtin_exception_class,
    is_flat_builtin_exception_class,
};
pub(crate) use func::{
    annotation_to_ty, lower_arg_list, lower_function, lower_return_annotation, type_param_name,
};
pub(crate) use import::{
    import_local_name, lower_import_stmt, lower_legacy_type_alias_ann_assign, lower_type_alias_stmt,
};
pub use typecheck::{
    ExtractClassNamesError, eval_isinstance_single, eval_issubclass_single, extract_class_names,
    is_abc_base_name, is_builtin_type_name, is_enum_base_name, is_protocol_base_name,
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
    /// Provenance for the builtin exception hierarchy (Part 1 of #541,
    /// D-188): `true` exactly when *this* lowering pass seeded the seven
    /// `BUILTIN_EXCEPTION_CLASSES` entries into `class_defs`, and `false`
    /// for every module whose classes are all user-authored.
    ///
    /// Seeding is all-or-nothing and its shadow gate guarantees no user
    /// top-level binding of any of the seven names survives alongside it,
    /// so this single flag plus `is_builtin_exception_class` identifies the
    /// synthetic entries exactly -- see `pycc_types`'s `bind_classes`.
    /// Provenance is recorded here rather than re-derived downstream
    /// because *no* property of a `HirClassDef`'s own shape is a sound
    /// proxy for who produced it: a user can author a class that is
    /// byte-for-byte identical to a synthetic one.
    pub seeded_builtin_exception_classes: bool,
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
    // Part 1 of #541 (extending D-173): give the builtin exception
    // hierarchy a real presence in the class table, seeded *before* any
    // user statement is lowered so a user class can inherit from one
    // (`class MyError(ValueError):`) exactly as it inherits from a user
    // base. Two gates, both of which must pass:
    //
    // * The module must actually *reference* one of the seven names. Every
    //   entry in `class_defs` costs the per-item work below (the projected
    //   class slice, the name-collision checks) and the per-function class
    //   binding in `pycc_types`, and a module that never names a builtin
    //   exception cannot observe the difference -- see
    //   `exception::module_references_builtin_exception_name`.
    // * The module's own top level must not *bind* any of the seven names.
    //   That gate is all-or-nothing, so every existing name-collision check
    //   below applies to the synthetic definitions with no exemption -- see
    //   `exception::module_shadows_builtin_exception_name`.
    let seeded_builtin_exception_classes =
        exception::module_references_builtin_exception_name(module)
            && !exception::module_shadows_builtin_exception_name(module);
    if seeded_builtin_exception_classes {
        class_defs.extend(builtin_exception_class_defs());
    }
    // Seeded at the *front* so every lookup below (base resolution,
    // annotation projection, the name-collision checks) sees them, then
    // rotated to the back once lowering finishes so `class_defs` still
    // opens with the module's own classes in source order.
    let synthetic_class_count = class_defs.len();
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
                false,
                None,
                None,
                &class_name_defs,
            )?),
        };
        items.push(item);
    }
    class_defs.rotate_left(synthetic_class_count);
    let user_class_count = class_defs.len() - synthetic_class_count;
    let mut any_user_exception_class = false;
    if synthetic_class_count > 0 {
        // Part 2 of #541 (D-189): assign each raisable user class its runtime
        // exception type tag here, in source order, so every downstream
        // consumer (`pycc_types`, `pycc_mir`, `pycc_codegen`) reads the same
        // number for the same class without re-deriving it. Source order is
        // the only ordering available that is stable across runs -- a hash
        // map's iteration order is not (risk R3 of this issue's plan).
        //
        // A class is raisable when its MRO reaches one of the seeded builtin
        // exception classes. `synthetic_class_count > 0` is exactly
        // `seeded_builtin_exception_classes`, so this branch never mistakes a
        // user class named `Exception` for the builtin one.
        let mut next_tag: u16 = u16::from(FIRST_USER_EXCEPTION_TYPE_TAG);
        for (_, def) in &mut class_defs[..user_class_count] {
            if !def
                .mro
                .iter()
                .any(|ancestor| is_builtin_exception_class(ancestor))
            {
                continue;
            }
            any_user_exception_class = true;
            if next_tag > u16::from(u8::MAX) {
                // The tag is a `u8` in `PyExceptionObj` and in every runtime
                // entry point that carries one, so the hierarchy cannot grow
                // past 256 types. No span is available here: `class_defs`
                // records no source range, and the diagnostic is about the
                // module's class count rather than any one declaration.
                return Err(Diagnostic::error(
                    "C0001",
                    format!(
                        "module declares more than {} exception classes; pycc \
                         supports at most {} user-defined exception classes \
                         per module",
                        MAX_USER_EXCEPTION_CLASSES, MAX_USER_EXCEPTION_CLASSES
                    ),
                    Span::new(0, 0),
                ));
            }
            def.exception_type_tag = Some(next_tag as u8);
            next_tag += 1;
        }
    }
    // The synthetic `Exception.__init__` body is emitted only when a user
    // class actually inherits it -- that is, when some user class's computed
    // MRO reaches one of the seeded builtin exception classes, which is
    // exactly the condition that assigned at least one tag above. The
    // class-table entries above are metadata every module needs for name and
    // base resolution; this is *code*, and emitting an uncallable constructor
    // into every compiled module would put a dead function in every object
    // file. The synthetic classes themselves can never call it: instantiating
    // one is rejected by the type checker
    // (`pycc_types::class::resolve_instantiation`).
    if any_user_exception_class {
        items.push(builtin_exception_init_item());
    }
    Ok(HirModule {
        items,
        type_aliases: aliases,
        imports,
        class_defs,
        seeded_builtin_exception_classes,
    })
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
