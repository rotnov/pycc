pub use pycc_hir::HirClassDef;
use pycc_hir::{
    CompIter, FStringPart, HirExpr, HirItem, HirMatchCase, HirModule, HirPattern, HirStmt,
    eval_isinstance_single, eval_issubclass_single, extract_class_names, is_builtin_type_name,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

// Re-exported (not just `use`d) because `pycc_codegen` doesn't depend on
// `pycc_hir` directly (see its Cargo.toml) -- `Ty`, `BinOpKind`, and
// `CmpOpKind` all reach this crate's public API through `MirExpr`'s fields
// (`Name`/`Call` carry a `Ty`; `BinOp` carries a `BinOpKind`; `Compare`
// carries a `CmpOpKind`), so each must be nameable as
// `pycc_mir::{Ty, BinOpKind, CmpOpKind}` from any downstream crate, exactly
// like `pycc_types` already re-exports `Ty` (`pycc_types::Ty`, its own line
// 4) for the same reason.
pub use pycc_hir::{BinOpKind, CmpOpKind, Ty};

/// Monotonic counter for synthesized match-subject temporaries. Each
/// `match` statement gets a unique `__match_subj_N` name, avoiding
/// collisions between multiple matches at the same scope level (which
/// would otherwise share the same `scopes.len()`-derived name and
/// cause a first-assignment-wins type drift in `bind_variable`).
static MATCH_SUBJECT_COUNTER: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, PartialEq)]
pub enum MirExpr {
    IntLiteral(i64),
    FloatLiteral(f64),
    BoolLiteral(bool),
    /// A `bool` crossing a statically-`int` boundary without becoming the
    /// arithmetic integer `0` or `1`.
    IntBoundary(Box<MirExpr>),
    StringLiteral(String),
    Name {
        name: String,
        ty: Ty,
    },
    Call {
        callee: String,
        args: Vec<MirExpr>,
        ty: Ty,
    },
    BinOp {
        op: BinOpKind,
        left: Box<MirExpr>,
        right: Box<MirExpr>,
        ty: Ty,
    },
    Compare {
        op: CmpOpKind,
        left: Box<MirExpr>,
        right: Box<MirExpr>,
        ty: Ty,
    },
    FString(Vec<MirFStringPart>),
    /// `[e1, e2, ...]`. No `ty` field: `ty()` below derives
    /// `Ty::List(Box::new(elements[0].ty()))` from the first element,
    /// exactly like `pycc_types::infer_expr_in`'s own `HirExpr::ListLiteral`
    /// arm derives the list's type from its elements rather than assuming
    /// `Ty::Int`. Empirically only `Ty::List(Box::new(Ty::Int))` ever
    /// reaches this crate today (`pycc_types`' T0034 gate rejects every
    /// other element type at construction time -- see that gate's own
    /// comment and its `a_for_list_loop_binds_its_variable_as_str_for_a_list_of_str`
    /// genericity test), but deriving here -- rather than hardcoding what
    /// today's upstream gate happens to allow -- keeps this lowering
    /// correct on its own terms, and correct automatically if that gate is
    /// ever relaxed, without requiring a matching `pycc_mir` change.
    ListLiteral(Vec<MirExpr>),
    /// `base[index]`, read-only (mirrors `HirExpr::Subscript`, D-105).
    /// `ty()` below derives its result from `base.ty()`'s element type
    /// (mirroring `pycc_types::infer_expr_in`'s own `Subscript` arm), for
    /// the same reason `ListLiteral` above derives rather than hardcodes.
    Subscript {
        base: Box<MirExpr>,
        index: Box<MirExpr>,
    },
    /// `list.append(value)` (mirrors `HirExpr::ListAppend`, D-105). `list` is
    /// carried as the plain variable name, exactly like `HirExpr::ListAppend`
    /// itself -- there is no sub-expression to recursively lower for it, only
    /// for `value`.
    ListAppend {
        list: String,
        value: Box<MirExpr>,
    },
    /// `{k1: v1, k2: v2, ...}` (mirrors `HirExpr::DictLiteral`, PR-11 Task 4).
    /// No `ty` field: `ty()` below derives `Ty::Dict(Box::new((pairs[0].0.ty(),
    /// pairs[0].1.ty())))` from the first pair, exactly like `ListLiteral`
    /// above derives its element type from its first element rather than
    /// hardcoding one. Empirically only `Ty::Dict(Box::new((Ty::Str,
    /// Ty::Int)))` ever reaches this crate today (`pycc_types`' T0036 gate
    /// rejects every other key/value combination at construction time), but
    /// deriving here keeps this lowering correct on its own terms rather
    /// than baking in an assumption this crate has no way to verify
    /// independently.
    DictLiteral(Vec<(MirExpr, MirExpr)>),
    /// `dict[key]`, read-only (mirrors `HirExpr::Subscript` on a
    /// dict-typed base -- see `lower_expr`'s own `HirExpr::Subscript` arm
    /// for why a dict-typed base is routed here instead of into
    /// `MirExpr::Subscript`). `ty()` below derives its result from
    /// `dict.ty()`'s value type, for the same reason `Subscript` derives
    /// from `base.ty()`'s element type rather than hardcoding one.
    DictGet {
        dict: Box<MirExpr>,
        key: Box<MirExpr>,
    },
    /// `{e1, e2, ...}` (mirrors `HirExpr::SetLiteral`, PR-11 Task 8). No
    /// `ty` field: `ty()` below derives `Ty::Set(Box::new(elements[0].ty()))`
    /// from the first element, exactly like `ListLiteral`/`DictLiteral`
    /// above derive their own element/key-value type from their first
    /// element/pair rather than hardcoding one. Empirically only
    /// `Ty::Set(Box::new(Ty::Int))` ever reaches this crate today
    /// (`pycc_types`' T0037/T0038 gates reject every other element type at
    /// construction time), but deriving here keeps this lowering correct on
    /// its own terms rather than baking in an assumption this crate has no
    /// way to verify independently. Unlike `ListLiteral`/`DictLiteral`, an
    /// *empty* `SetLiteral` is impossible from real Python source in an even
    /// stronger sense than "`pycc_types::check` would reject it": Python's
    /// own grammar has no empty-set-literal syntax at all (`{}` parses as an
    /// empty `dict`, not an empty `set` -- CPython's own grammar routes a
    /// bare `{}` to `ast.Dict`, never `ast.Set`), so `elements` is non-empty
    /// for every `SetLiteral` `pycc_hir::lower_expr`'s own `Expr::Set` arm
    /// could ever construct from a real parse.
    SetLiteral(Vec<MirExpr>),
    /// `(e1, e2, ...)` (mirrors `HirExpr::TupleLiteral`, PR-11b Task 4). No
    /// `ty` field: `ty()` below derives `Ty::Tuple(Box::new(elements.iter()
    /// .map(MirExpr::ty).collect()))` positionally from every element (not
    /// just the first, unlike `ListLiteral`/`DictLiteral`/`SetLiteral` --
    /// heterogeneity means every position can differ). Empirically only
    /// `int`/`bool`/`float` elements ever reach this crate today
    /// (`pycc_types`' T0039 gate rejects every other element type at
    /// construction time), but deriving here keeps this lowering correct on
    /// its own terms rather than baking in an assumption this crate has no
    /// way to verify independently.
    TupleLiteral(Vec<MirExpr>),
    /// `base[start:stop:step]` (mirrors `HirExpr::Slice`, PR-12 Task 8,
    /// D-118). Each bound is independently optional, exactly like
    /// `HirExpr::Slice` itself. No `ty` field: `ty()` below returns
    /// `base.ty()` unchanged -- slicing a `list[int]` always produces
    /// another `list[int]`, unlike `Subscript`'s own element-type-narrowing
    /// `ty()` derivation above. This lowering is purely structural (recurse
    /// into `base` and every present bound, same as every other MIR node in
    /// this enum); it does not implement the runtime clamping/panic
    /// behavior D-118 requires (non-negative check, clamp into `[0, len]`,
    /// positive-step check) -- that is `pycc_codegen`'s job (a later task),
    /// operating on this already-lowered shape.
    Slice {
        base: Box<MirExpr>,
        start: Option<Box<MirExpr>>,
        stop: Option<Box<MirExpr>>,
        step: Option<Box<MirExpr>>,
    },
    /// `list.pop()` (mirrors `HirExpr::ListPop`, PR-12, D-119). `ty()` below
    /// returns the list's own element type -- empirically always `Ty::Int`
    /// (T0034 rejects every other `list[T]` before codegen), derived rather
    /// than hardcoded for the identical reason `ListLiteral`'s own `ty()`
    /// derives (D-105's own precedent).
    ListPop {
        list: String,
        ty: Ty,
    },
    /// `dict.get(key, default)` (mirrors `HirExpr::DictGetOrDefault`, PR-12,
    /// D-119). `ty()` below returns the dict's own value type, derived from
    /// the `dict` name's binding rather than hardcoded, for the same reason
    /// `ListPop` above derives its element type.
    DictGetOrDefault {
        dict: String,
        key: Box<MirExpr>,
        default: Box<MirExpr>,
        ty: Ty,
    },
    /// `set.add(value)` (mirrors `HirExpr::SetAdd`, PR-12, D-119). `.add()`
    /// always returns `None`, exactly like `ListAppend` -- a true invariant,
    /// not narrowed by any gate, hardcoded on purpose (mirrors `ListAppend`'s
    /// own `ty()` arm).
    SetAdd {
        set: String,
        value: Box<MirExpr>,
    },
    /// `ClassName(args)` (D-154, Part 1 of #375): allocates a new instance
    /// with `attr_count` slots (`pycc_codegen`'s job, via the class
    /// instance layout ADR's `pycc_rt_instance_new`), then calls `ctor`
    /// (the mangled `<ClassName>.__init__`) with the fresh instance
    /// pointer as `self`, followed by `args`. `HirExpr::Call` has no shape
    /// for "allocate, then call" -- unlike `MethodCall` (see this file's
    /// own `lower_expr`, which lowers a method call directly into an
    /// ordinary `MirExpr::Call`, since a method call only ever *calls*, it
    /// never allocates), instantiation genuinely needs its own node.
    Instantiate(Box<InstantiateExpr>),
    /// `base.attr` (D-154, Part 1 of #375), resolved to a compile-time slot
    /// index by `lower_expr` against the class's `HirClassDef` -- never a
    /// runtime string-keyed lookup, per the class-instance-layout ADR.
    AttrGet {
        base: Box<MirExpr>,
        slot: usize,
        ty: Ty,
    },
    /// #436: A null instance pointer used as the `cls` argument when a
    /// `@classmethod` is called on a class name (`ClassName.method(args)`)
    /// rather than an instance. In this compiler's static-dispatch model,
    /// `cls` is typed as `Ty::Instance(class_name)` but is not meaningfully
    /// used in the method body -- the method was compiled for a specific
    /// class, so any `cls.method()` or `cls.attr` resolves at compile time
    /// to that class's methods/attributes. The null pointer is never
    /// dereferenced at runtime (the method body does not access `cls`'s
    /// slots in practice), so this is safe.
    NullInstance {
        ty: Ty,
    },
}

/// `MirExpr::Instantiate`'s payload, boxed (not inlined into that variant
/// directly) to keep `MirExpr`'s own size close to its other variants --
/// `ctor: String` + `attr_count: usize` + `args: Vec<MirExpr>` + `ty: Ty`
/// inlined directly measured large enough to trip clippy's
/// `large_enum_variant` lint (`-D warnings`) on `MirItem` (whose
/// `TopLevelStmt(MirStmt)` variant embeds `MirExpr` several layers deep,
/// e.g. via `MirStmt::AttrSet`'s two raw `MirExpr` fields and
/// `CompSource::Range`'s three), the same reasoning `Ty::Tuple(Box<Vec<Ty>>)`
/// and `Ty::Dict(Box<(Ty, Ty)>)` already apply one crate over.
#[derive(Debug, Clone, PartialEq)]
pub struct InstantiateExpr {
    pub ctor: String,
    pub attr_count: usize,
    pub args: Vec<MirExpr>,
    pub ty: Ty,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MirFStringPart {
    Literal(String),
    Interpolation(Box<MirExpr>),
}

impl MirExpr {
    pub fn ty(&self) -> Ty {
        match self {
            MirExpr::IntLiteral(_) | MirExpr::IntBoundary(_) => Ty::Int,
            MirExpr::FloatLiteral(_) => Ty::Float,
            MirExpr::BoolLiteral(_) => Ty::Bool,
            MirExpr::StringLiteral(_) | MirExpr::FString(_) => Ty::Str,
            MirExpr::Name { ty, .. }
            | MirExpr::Call { ty, .. }
            | MirExpr::BinOp { ty, .. }
            | MirExpr::Compare { ty, .. } => ty.clone(),
            MirExpr::ListLiteral(elements) => {
                let elem_ty = elements.first().map(|e| e.ty()).unwrap_or_else(|| {
                    panic!(
                        "pycc_mir: internal error: an empty list literal has no element type to derive -- pycc_types::check should have rejected this HIR before it reached pycc_mir"
                    )
                });
                Ty::List(Box::new(elem_ty))
            }
            // Unlike `ForList`'s own `Ty::List`/`Ty::Dict` dispatch just
            // below, this arm is *not* where a dict-typed base gets
            // resolved: `pycc_types::infer_expr_in`'s own `Subscript` arm
            // accepts a `Ty::Dict` base just as readily as `Ty::List` (it
            // returns the dict's value type directly), so a non-list base
            // reaching *this* HIR shape is not, on its own, something
            // `pycc_types::check` would have rejected. What actually keeps
            // a dict-typed base out of this arm is `lower_expr`'s own
            // `HirExpr::Subscript` arm: it inspects the lowered base's type
            // and constructs `MirExpr::DictGet` instead of
            // `MirExpr::Subscript` whenever that type is `Ty::Dict`, so no
            // `MirExpr::Subscript` node produced by `build()` ever carries a
            // dict base. A base that is neither list nor dict *is* still
            // impossible from a type-checked program (`pycc_types` rejects
            // every other subscript base with T0033 before HIR is even
            // constructed) -- this panic remains reachable only from a
            // hand-built `MirExpr` that bypasses both of those guarantees,
            // e.g. this file's own `a_subscript_over_a_non_list_bases_ty_panics_with_an_internal_error` test.
            MirExpr::Subscript { base, index } => match base.ty() {
                Ty::List(elem_ty) => *elem_ty,
                // PR-11b Task 4 (D-116): the literal-index extraction and
                // bounds check already happened in `pycc_types::infer_expr_in`
                // -- this re-derives the same positional element type from
                // the already-validated literal `index`, mirroring how
                // every other panic path in this file re-states "pycc_types
                // should have rejected this" rather than re-validating.
                Ty::Tuple(elems) => {
                    let MirExpr::IntLiteral(literal_index) = index.as_ref() else {
                        panic!(
                            "pycc_mir: internal error: tuple subscript index is not a literal int -- pycc_types::check should have rejected this HIR before it reached pycc_mir (T0040)"
                        )
                    };
                    let literal_index = usize::try_from(*literal_index).unwrap_or_else(|_| {
                        panic!(
                            "pycc_mir: internal error: tuple subscript index is negative -- pycc_types::check should have rejected this HIR before it reached pycc_mir (T0040)"
                        )
                    });
                    elems.get(literal_index).cloned().unwrap_or_else(|| {
                        panic!(
                            "pycc_mir: internal error: tuple subscript index out of range -- pycc_types::check should have rejected this HIR before it reached pycc_mir (T0040)"
                        )
                    })
                }
                other => panic!(
                    "pycc_mir: internal error: subscript base has non-list/tuple type `{}` -- pycc_types::check should have rejected this HIR before it reached pycc_mir, or lower_expr should have routed a dict base to MirExpr::DictGet instead",
                    other.name()
                ),
            },
            // `.append()` always returns `None` in Python, independent of
            // the list's element type -- a true invariant (like `Compare`'s
            // `Bool` and `ForRange`'s `Int` above), not narrowed by any
            // `pycc_types` gate, so this is hardcoded on purpose.
            MirExpr::ListAppend { .. } => Ty::None,
            MirExpr::DictLiteral(pairs) => {
                let (first_key, first_value) = pairs.first().unwrap_or_else(|| {
                    panic!(
                        "pycc_mir: internal error: an empty dict literal has no key/value type to derive -- pycc_types::check should have rejected this HIR before it reached pycc_mir"
                    )
                });
                Ty::Dict(Box::new((first_key.ty(), first_value.ty())))
            }
            MirExpr::DictGet { dict, .. } => match dict.ty() {
                Ty::Dict(kv) => kv.1,
                other => panic!(
                    "pycc_mir: internal error: dict subscript base has non-dict type `{}` -- pycc_types::check should have rejected this HIR before it reached pycc_mir",
                    other.name()
                ),
            },
            MirExpr::SetLiteral(elements) => {
                let first = elements.first().unwrap_or_else(|| {
                    panic!(
                        "pycc_mir: internal error: an empty set literal has no element type to derive -- pycc_types::check should have rejected this HIR before it reached pycc_mir"
                    )
                });
                Ty::Set(Box::new(first.ty()))
            }
            MirExpr::TupleLiteral(elements) => {
                if elements.is_empty() {
                    panic!(
                        "pycc_mir: internal error: an empty tuple literal has no element types to derive -- pycc_types::check should have rejected this HIR before it reached pycc_mir"
                    )
                }
                Ty::Tuple(Box::new(elements.iter().map(MirExpr::ty).collect()))
            }
            // A slice of a `list[int]` is still a `list[int]` -- `ty()`
            // passes `base.ty()` through unchanged, matching
            // `pycc_types::infer_expr_in`'s own `HirExpr::Slice` arm, which
            // returns `base_ty.clone()` after validating the base is
            // `Ty::List(Int)` (that validation already happened in
            // `pycc_types`; this crate only re-derives the resulting type).
            MirExpr::Slice { base, .. } => base.ty(),
            MirExpr::ListPop { ty, .. } => ty.clone(),
            MirExpr::DictGetOrDefault { ty, .. } => ty.clone(),
            // `.add()` always returns `None`, exactly like `ListAppend`
            // above -- a true invariant, not narrowed by any `pycc_types`
            // gate, so this is hardcoded on purpose.
            MirExpr::SetAdd { .. } => Ty::None,
            MirExpr::Instantiate(inst) => inst.ty.clone(),
            MirExpr::AttrGet { ty, .. } => ty.clone(),
            MirExpr::NullInstance { ty } => ty.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum MirStmt {
    ExprStmt(MirExpr),
    Assign {
        target: String,
        value: MirExpr,
    },
    /// A statement with zero runtime effect -- currently only produced by a
    /// value-less PEP 526 annotation (`x: int`), which CPython itself does
    /// nothing observable for either (confirmed empirically during PR-9
    /// planning: no store, no allocation, nothing an oracle diff could see).
    NoOp,
    If {
        test: MirExpr,
        body: Vec<MirStmt>,
        orelse: Vec<MirStmt>,
    },
    While {
        test: MirExpr,
        body: Vec<MirStmt>,
    },
    ForRange {
        var: String,
        start: MirExpr,
        stop: MirExpr,
        step: MirExpr,
        body: Vec<MirStmt>,
    },
    /// `for var in list:` (mirrors `HirStmt::ForList`, D-105). `list` is
    /// carried as the plain variable name, exactly like `HirStmt::ForList`
    /// itself; there is no start/stop/step to lower here, only `body`.
    ForList {
        var: String,
        list: String,
        body: Vec<MirStmt>,
    },
    /// `d[k] = v` (mirrors `HirStmt::DictSet`, PR-11 Task 4/D-123). `dict` is
    /// carried as the plain variable name, exactly like `ForList`'s `list`
    /// field and `ListAppend`'s `list` field -- there is no sub-expression to
    /// recursively lower for it, only for `key`/`value`.
    DictSet {
        dict: String,
        key: MirExpr,
        value: MirExpr,
    },
    /// `for var in dict:` (mirrors `HirStmt::ForList` on a dict-typed base --
    /// see `lower_stmt`'s own `HirStmt::ForList` arm for why a dict-typed
    /// base is lowered to this node instead of `MirStmt::ForList`). `dict` is
    /// carried as the plain variable name, exactly like `ForList`'s `list`
    /// field.
    ForDict {
        var: String,
        dict: String,
        body: Vec<MirStmt>,
    },
    /// `for var in set:` (mirrors `HirStmt::ForList` on a set-typed base --
    /// see `lower_stmt`'s own `HirStmt::ForList` arm for why a set-typed
    /// base is lowered to this node instead of `MirStmt::ForList`, PR-11
    /// Task 8, D-123). `set` is carried as the plain variable name, exactly
    /// like `ForList`'s `list` field and `ForDict`'s `dict` field.
    ForSet {
        var: String,
        set: String,
        body: Vec<MirStmt>,
    },
    /// `target = [elt for var in <source> [if cond]]`, already fully
    /// resolved (mirrors `HirStmt::ListCompAssign`, PR-12, D-117). `var_ty`
    /// is the loop variable's own resolved type (`Ty::Int` for a `Range`
    /// source; the iterated container's element/key type for a `List`/
    /// `Dict`/`Set` source) -- carried explicitly, rather than requiring
    /// `pycc_codegen` to re-derive it from `elt`, because `elt` need not
    /// contain any `Name` reference to `var` at all in general (though in
    /// practice it usually does) and re-deriving it by walking `elt` would
    /// be a second, independent computation of a fact `resolve_comp_source`
    /// (below) already has in hand once.
    ListCompAssign {
        target: String,
        var: String,
        var_ty: Ty,
        source: CompSource,
        cond: Option<Box<MirExpr>>,
        elt: Box<MirExpr>,
    },
    /// `target = {key: value for var in <source> [if cond]}`, already fully
    /// resolved (mirrors `HirStmt::DictCompAssign`, PR-12, D-117). Mirrors
    /// `ListCompAssign` exactly except for the key/value split.
    DictCompAssign {
        target: String,
        var: String,
        var_ty: Ty,
        source: CompSource,
        cond: Option<Box<MirExpr>>,
        key: Box<MirExpr>,
        value: Box<MirExpr>,
    },
    /// `target = {elt for var in <source> [if cond]}`, already fully
    /// resolved (mirrors `HirStmt::SetCompAssign`, PR-12, D-117). Mirrors
    /// `ListCompAssign` exactly -- a set comprehension's own shape is
    /// identical to a list comprehension's, differing only in which
    /// runtime constructor/insert pair `pycc_codegen` ends up calling.
    SetCompAssign {
        target: String,
        var: String,
        var_ty: Ty,
        source: CompSource,
        cond: Option<Box<MirExpr>>,
        elt: Box<MirExpr>,
    },
    Return(Option<MirExpr>),
    /// `base.attr = value` (D-154, Part 1 of #375), resolved to a
    /// compile-time slot index by `lower_stmt` against the class's
    /// `HirClassDef` -- mirrors `MirExpr::AttrGet`'s own resolution
    /// exactly.
    AttrSet {
        base: MirExpr,
        slot: usize,
        value: MirExpr,
    },
    /// PEP 634-636 (#381, PR-21): A sequence of statements executed in
    /// order — used by `match` lowering to pair the subject-temporary
    /// assignment with the nested `if` chain.
    Seq(Vec<MirStmt>),
    /// `try`/`except`/`else`/`finally` (PEP 3110, #382, PR-22 Part 1).
    /// Each handler's `exc_type_tag` is the resolved runtime exception
    /// type tag (matching `pycc_rt`'s `EXCEPTION_TYPE_*` constants), or
    /// `None` for a bare `except:`.
    Try {
        body: Vec<MirStmt>,
        handlers: Vec<MirExceptHandler>,
        orelse: Vec<MirStmt>,
        finalbody: Vec<MirStmt>,
    },
    /// `raise ExceptionType("msg")` (PEP 3110, #382). `exc_type_tag` is
    /// the resolved runtime exception type tag. `message` is the message
    /// expression.
    Raise {
        exc_type_tag: u8,
        message: MirExpr,
    },
    /// `raise ExceptionType("msg") from CauseType("cause")` (PEP 409, #382).
    /// Both the exception and its explicit cause are raised together.
    RaiseFrom {
        exc_type_tag: u8,
        message: MirExpr,
        cause_type_tag: u8,
        cause_message: MirExpr,
    },
    /// Bare `raise` (re-raise, #382). Only valid inside an except handler.
    Reraise,
}

/// A single `except` handler in a MIR `try` statement (PEP 3110, #382,
/// PR-22 Part 1). `exc_type_tag` is the resolved runtime exception type
/// tag, or `None` for a bare `except:` (catches all exceptions).
#[derive(Debug, Clone, PartialEq)]
pub struct MirExceptHandler {
    pub exc_type_tag: Option<u8>,
    pub body: Vec<MirStmt>,
}

/// A comprehension's already-resolved iterable source (PR-12, D-117) --
/// the MIR-level counterpart to `pycc_hir::CompIter`, but with a
/// bare-name iterable already split into its concrete container kind,
/// mirroring `HirStmt::ForList`'s own split into `MirStmt::ForList`/
/// `ForDict`/`ForSet` at this exact lowering stage. Kept as a field on
/// each `*CompAssign` variant (not exploded into a full cross-product of
/// top-level `MirStmt` variants per comprehension-kind x source-kind
/// combination) -- mirrors the precedent `MirExpr::Subscript` already
/// established (one node, internal branching on the resolved type in
/// `pycc_codegen`), avoiding a 3x4 combinatorial explosion for no
/// benefit.
#[derive(Debug, Clone, PartialEq)]
pub enum CompSource {
    Range {
        start: MirExpr,
        stop: MirExpr,
        step: MirExpr,
    },
    List(String),
    Dict(String),
    Set(String),
}

#[derive(Debug, PartialEq)]
pub enum MirItem {
    Function {
        name: String,
        params: Vec<(String, Ty)>,
        return_ty: Ty,
        body: Vec<MirStmt>,
    },
    TopLevelStmt(MirStmt),
}

pub struct MirModule {
    pub items: Vec<MirItem>,
    /// #379 (PR-19): the module's class definitions, carried through to
    /// codegen so it can emit per-enum-member singleton init sequences.
    /// Codegen needs the `enum_members` table (member names and value
    /// types) to allocate and initialize each member's singleton instance
    /// at module-init time. Non-enum class defs are also carried, though
    /// codegen only reads `enum_members` from them today.
    pub class_defs: Vec<(String, pycc_hir::HirClassDef)>,
}

pub fn build(hir: &HirModule) -> MirModule {
    let mut scopes: Vec<HashMap<String, Ty>> = vec![HashMap::new()];
    // D-154 (Part 1 of #375): a plain `HashMap` clone of `hir.class_defs`,
    // built once and threaded read-only through every lowering call below
    // (`lower_item`/`lower_stmt`/`lower_expr`/`resolve_comp_source`) --
    // mirrors `scopes`' own "compute once in `build`, thread through every
    // recursive call" shape, but never mutated the way `scopes` is (a
    // class's declared shape does not change while lowering a module).
    let classes: HashMap<String, HirClassDef> = hir.class_defs.iter().cloned().collect();
    // First pass: register every function's mangled `$fn:name` signature
    // before lowering any item body -- mirrors `pycc_types::check`'s own
    // two-pass fix (D-038/D-039) so a forward reference, a sibling call, or
    // a recursive self-call all resolve to the right return type regardless
    // of where the callee's `def` appears in the module.
    for item in &hir.items {
        if let HirItem::Function {
            name, return_ty, ..
        } = item
        {
            bind(&mut scopes, format!("$fn:{name}"), return_ty.clone());
        }
    }
    // Lower module statements first, in source order, so the module scope is
    // complete before any function body is lowered. This mirrors
    // `pycc_types::check_with_signatures`'s D-041 three-pass contract:
    // top-level forward reads stay invalid because these statements are still
    // visited sequentially, while a function may read a global assigned after
    // its `def` because function bodies are evaluated only when called.
    let mut lowered: Vec<Option<MirItem>> = hir.items.iter().map(|_| None).collect();
    for (index, item) in hir.items.iter().enumerate() {
        if matches!(item, HirItem::TopLevelStmt(_)) {
            lowered[index] = Some(lower_item(item, &mut scopes, &classes));
        }
    }
    for (index, item) in hir.items.iter().enumerate() {
        if matches!(item, HirItem::Function { .. }) {
            lowered[index] = Some(lower_item(item, &mut scopes, &classes));
        }
    }
    let items = lowered
        .into_iter()
        .map(|item| item.expect("every HIR item is either a function or a top-level statement"))
        .collect();
    MirModule {
        items,
        class_defs: hir.class_defs.clone(),
    }
}

fn lower_item(
    item: &HirItem,
    scopes: &mut Vec<HashMap<String, Ty>>,
    classes: &HashMap<String, HirClassDef>,
) -> MirItem {
    match item {
        HirItem::Function {
            name,
            params,
            return_ty,
            body,
        } => {
            // #433: extract the class name from a mangled
            // `<ClassName>.<method>` name so `lower_expr`'s `Super` arm
            // can resolve the next class in the MRO. A top-level function
            // name contains no `.`, so `current_class` is `None` for those.
            let current_class: Option<&str> =
                name.split('.').next().filter(|prefix| *prefix != name);
            scopes.push(params.iter().cloned().collect());
            let body = body
                .iter()
                .map(|s| lower_stmt(s, scopes, classes, current_class))
                .collect();
            scopes.pop();
            MirItem::Function {
                name: name.clone(),
                params: params.clone(),
                return_ty: return_ty.clone(),
                body,
            }
        }
        HirItem::TopLevelStmt(stmt) => {
            MirItem::TopLevelStmt(lower_stmt(stmt, scopes, classes, None))
        }
    }
}

fn bind(scopes: &mut [HashMap<String, Ty>], name: String, ty: Ty) {
    scopes
        .last_mut()
        .expect("at least one scope is always present")
        .insert(name, ty);
}

fn bind_variable(scopes: &mut [HashMap<String, Ty>], name: String, ty: Ty) {
    scopes
        .last_mut()
        .expect("at least one scope is always present")
        .entry(name)
        .or_insert(ty);
}

fn lookup(scopes: &[HashMap<String, Ty>], name: &str) -> Ty {
    scopes
        .iter()
        .rev()
        .find_map(|scope| scope.get(name).cloned())
        .unwrap_or_else(|| panic!("pycc_mir: internal error: `{name}` has no recorded type -- pycc_types::check should have rejected this HIR before it reached pycc_mir"))
}

/// #436/#432: Looks up a class in the `classes` table by its MRO entry
/// name, panicking with a consistent internal-error message if the class
/// is not registered. Centralizing this lookup ensures the defensive
/// panic is covered by a single test rather than duplicated across every
/// MRO walk (where later walks are unreachable after the first walk
/// panics on a ghost class).
fn mro_class_def<'a>(
    mro_class: &str,
    classes: &'a HashMap<String, HirClassDef>,
) -> &'a HirClassDef {
    classes.get(mro_class).unwrap_or_else(|| {
        panic!(
            "pycc_mir: internal error: class `{mro_class}` in MRO has no registered \
             HirClassDef -- pycc_types::check should have rejected this HIR before \
             it reached pycc_mir"
        )
    })
}

/// Resolves a `pycc_hir::CompIter` into a fully-typed `CompSource`,
/// lowering any range sub-expressions and binding `var`'s type into
/// `scopes` -- mirrors `HirStmt::ForList`'s own resolution exactly
/// (`lower_stmt`'s existing `ForList` arm), reused via this shared helper
/// rather than duplicated three times (once per comprehension kind, PR-12,
/// D-117). Takes `scopes` as `&mut [HashMap<String, Ty>]` (a slice), not
/// `&mut Vec<..>` like `lower_stmt`/`lower_item` -- unlike those two, this
/// helper never pushes/pops a scope itself, so `clippy::ptr_arg` genuinely
/// flags an owned `Vec` parameter here as unnecessary. `lower_item`'s
/// `&mut Vec` is a real need: its `Function` arm calls `scopes.push(..)`
/// before lowering the function body and `scopes.pop()` after, and
/// `&mut [_]` has no such methods. `lower_stmt` itself never calls a
/// `Vec`-only method on `scopes` either (every other callee it passes
/// `scopes` to -- `lower_expr`, `bind_variable`, `lookup`,
/// `resolve_comp_source` -- already takes a slice); it is self-recursive
/// (it calls itself for nested statement bodies, e.g. `If`/`While`), not
/// mutually recursive with `lower_item` (`lower_item` calls `lower_stmt`,
/// but `lower_stmt` never calls back into `lower_item`). `clippy::ptr_arg`
/// does not fire on a self-recursive function, so nothing forces or even
/// flags `lower_stmt`'s own `&mut Vec` as unnecessary; it keeps the owned
/// type by convention, matching its only caller, `lower_item`, rather than
/// out of its own requirement.
fn resolve_comp_source(
    iter: &CompIter,
    var: &str,
    scopes: &mut [HashMap<String, Ty>],
    classes: &HashMap<String, HirClassDef>,
    current_class: Option<&str>,
) -> (CompSource, Ty) {
    match iter {
        CompIter::Range { start, stop, step } => {
            let start = lower_expr(start, scopes, classes, current_class);
            let stop = lower_expr(stop, scopes, classes, current_class);
            let step = lower_expr(step, scopes, classes, current_class);
            bind_variable(scopes, var.to_string(), Ty::Int);
            (CompSource::Range { start, stop, step }, Ty::Int)
        }
        CompIter::Name(name) => match lookup(scopes, name) {
            Ty::List(elem_ty) => {
                bind_variable(scopes, var.to_string(), (*elem_ty).clone());
                (CompSource::List(name.clone()), *elem_ty)
            }
            Ty::Dict(kv) => {
                bind_variable(scopes, var.to_string(), kv.0.clone());
                (CompSource::Dict(name.clone()), kv.0)
            }
            Ty::Set(elem_ty) => {
                bind_variable(scopes, var.to_string(), (*elem_ty).clone());
                (CompSource::Set(name.clone()), *elem_ty)
            }
            other => panic!(
                "pycc_mir: internal error: `{name}` is neither a list, dict, nor set (found `{}`) -- pycc_types::check should have rejected this HIR before it reached pycc_mir",
                other.name()
            ),
        },
    }
}

fn lower_stmt(
    stmt: &HirStmt,
    scopes: &mut Vec<HashMap<String, Ty>>,
    classes: &HashMap<String, HirClassDef>,
    current_class: Option<&str>,
) -> MirStmt {
    match stmt {
        HirStmt::ExprStmt(expr) => {
            MirStmt::ExprStmt(lower_expr(expr, scopes, classes, current_class))
        }
        HirStmt::Assign { target, value } => {
            let value = lower_expr(value, scopes, classes, current_class);
            // The first assignment fixes a binding's representation.
            // In particular, assigning `bool` to an existing `int` is
            // accepted by the type checker but must not silently change the
            // later MIR name type from tagged i64 to i8.
            bind_variable(scopes, target.clone(), value.ty());
            MirStmt::Assign {
                target: target.clone(),
                value,
            }
        }
        HirStmt::AnnAssign {
            target,
            annotation,
            value: Some(value),
            is_final: _,
        } => {
            let value = lower_expr(value, scopes, classes, current_class);
            // `pycc_types::is_assignable` accepts an annotated initializer
            // in exactly two shapes: an exact type match, or a `bool`
            // initializer under an `int` annotation (`bool` is an `int`
            // subtype -- the only widening `is_assignable` allows). Unlike
            // plain `Assign` (whose bound type and lowered value type are
            // always the same, since both come from `value`), `pycc_types`
            // itself binds its checker `env` to the *annotation's* type for
            // `AnnAssign` (`check_assignment(env, target, *annotation)`,
            // not the initializer's inferred type) specifically so a later
            // annotated re-declaration is checked consistently -- see its
            // own comment citing this exact invariant. D-074's "first
            // assignment fixes a binding's representation" rule then
            // requires this lowering to agree, or a later plain
            // reassignment (`x: int = True; x = 5`) would silently widen
            // into a slot still permanently sized for `bool` (confirmed
            // empirically before this fix: the program above printed `11`,
            // the raw tagged-int bit pattern truncated through an `i8`
            // slot, instead of `5`). Keep the static `int` slot without
            // manufacturing arithmetic: `True + 0` is the integer `1`, but
            // an annotated boundary must retain the runtime identity
            // `True`.
            let value = if value.ty() == *annotation {
                value
            } else if value.ty() == Ty::Bool && *annotation == Ty::Int {
                // `bool` initializer under an `int` annotation — widen
                // to `int` via `IntBoundary` (D-141), preserving the
                // runtime `bool` identity while reporting `Ty::Int`.
                MirExpr::IntBoundary(Box::new(value))
            } else {
                value
            };
            // #380 (PR-20): when the annotation is a protocol type, bind
            // with the value's concrete type instead — the protocol type
            // is a compile-time-only interface, and the MIR needs the
            // concrete type for method/attribute resolution (static
            // dispatch). `pycc_types` already validated conformance and
            // binds the concrete type in its own environment; the MIR
            // must agree.
            let bind_ty = if matches!(annotation, Ty::Protocol(_)) {
                value.ty()
            } else {
                annotation.clone()
            };
            bind_variable(scopes, target.clone(), bind_ty);
            MirStmt::Assign {
                target: target.clone(),
                value,
            }
        }
        HirStmt::AnnAssign { value: None, .. } => MirStmt::NoOp,
        HirStmt::If { test, body, orelse } => MirStmt::If {
            test: lower_expr(test, scopes, classes, current_class),
            body: body
                .iter()
                .map(|s| lower_stmt(s, scopes, classes, current_class))
                .collect(),
            orelse: orelse
                .iter()
                .map(|s| lower_stmt(s, scopes, classes, current_class))
                .collect(),
        },
        HirStmt::While { test, body } => MirStmt::While {
            test: lower_expr(test, scopes, classes, current_class),
            body: body
                .iter()
                .map(|s| lower_stmt(s, scopes, classes, current_class))
                .collect(),
        },
        HirStmt::ForRange {
            var,
            start,
            stop,
            step,
            body,
        } => {
            let start = lower_expr(start, scopes, classes, current_class);
            let stop = lower_expr(stop, scopes, classes, current_class);
            let step = lower_expr(step, scopes, classes, current_class);
            bind_variable(scopes, var.clone(), Ty::Int);
            let body = body
                .iter()
                .map(|s| lower_stmt(s, scopes, classes, current_class))
                .collect();
            MirStmt::ForRange {
                var: var.clone(),
                start,
                stop,
                step,
                body,
            }
        }
        HirStmt::ForList { var, list, body } => {
            // The loop variable's type is `list`'s element (or, for a
            // dict-typed binding, key) type, derived via the same `lookup`
            // mechanism every other name reference in this file uses --
            // mirroring `pycc_types::check_stmt`'s own `ForList` arm
            // (`check_assignment(env, var, *elem_ty)` / `check_assignment(env,
            // var, kv.0)` / `check_assignment(env, var, *elem_ty)` for the
            // set case), not hardcoded to `Ty::Int`. Empirically only a
            // `Ty::List(Box::new(Ty::Int))`, `Ty::Dict(Box::new((Ty::Str,
            // Ty::Int)))`, or `Ty::Set(Box::new(Ty::Int))` binding ever
            // reaches this arm today (`pycc_types`' T0034/T0036/T0037/T0038
            // gates reject every other element/key-value combination before
            // HIR ever constructs one -- see those gates' own comments and
            // this crate's own genericity tests), but deriving here keeps
            // this lowering correct on its own terms rather than baking in
            // an assumption this crate has no way to verify independently.
            // `HirStmt::ForList` is reused unconditionally by `pycc_hir`'s
            // own lowering for any bare-name iterable, dict, list, or set
            // alike (it has no type information to pick a different node)
            // -- this is the point where the real type is resolved and
            // where a dict- or set-typed binding is routed into
            // `MirStmt::ForDict`/`MirStmt::ForSet` instead of
            // `MirStmt::ForList`, mirroring `lower_expr`'s own
            // `HirExpr::Subscript` arm doing the same list/dict routing for
            // reads (subscripting a set is rejected earlier, by
            // `pycc_types`' own T0033, so there is no set counterpart there).
            match lookup(scopes, list) {
                Ty::List(elem_ty) => {
                    bind_variable(scopes, var.clone(), *elem_ty);
                    let body = body
                        .iter()
                        .map(|s| lower_stmt(s, scopes, classes, current_class))
                        .collect();
                    MirStmt::ForList {
                        var: var.clone(),
                        list: list.clone(),
                        body,
                    }
                }
                Ty::Dict(kv) => {
                    bind_variable(scopes, var.clone(), kv.0);
                    let body = body
                        .iter()
                        .map(|s| lower_stmt(s, scopes, classes, current_class))
                        .collect();
                    MirStmt::ForDict {
                        var: var.clone(),
                        dict: list.clone(),
                        body,
                    }
                }
                // `for x in s:` (PR-11 Task 8, D-123): iterates a set's own
                // elements, binding the loop variable as the set's element
                // type -- mirrors the `Ty::Dict` arm immediately above,
                // which mirrors `pycc_types::check_stmt`'s own identical
                // `Ty::Set(elem_ty) => *elem_ty` arm (added in that crate's
                // Task 7 fix round).
                Ty::Set(elem_ty) => {
                    bind_variable(scopes, var.clone(), *elem_ty);
                    let body = body
                        .iter()
                        .map(|s| lower_stmt(s, scopes, classes, current_class))
                        .collect();
                    MirStmt::ForSet {
                        var: var.clone(),
                        set: list.clone(),
                        body,
                    }
                }
                other => panic!(
                    "pycc_mir: internal error: `{list}` is neither a list, dict, nor set (found `{}`) -- pycc_types::check should have rejected this HIR before it reached pycc_mir",
                    other.name()
                ),
            }
        }
        HirStmt::ListCompAssign {
            target,
            var,
            iter,
            cond,
            elt,
        } => {
            let (source, var_ty) = resolve_comp_source(iter, var, scopes, classes, current_class);
            let cond = cond
                .as_deref()
                .map(|c| lower_expr(c, scopes, classes, current_class));
            let elt = lower_expr(elt, scopes, classes, current_class);
            bind_variable(scopes, target.clone(), Ty::List(Box::new(elt.ty())));
            MirStmt::ListCompAssign {
                target: target.clone(),
                var: var.clone(),
                var_ty,
                source,
                cond: cond.map(Box::new),
                elt: Box::new(elt),
            }
        }
        HirStmt::SetCompAssign {
            target,
            var,
            iter,
            cond,
            elt,
        } => {
            let (source, var_ty) = resolve_comp_source(iter, var, scopes, classes, current_class);
            let cond = cond
                .as_deref()
                .map(|c| lower_expr(c, scopes, classes, current_class));
            let elt = lower_expr(elt, scopes, classes, current_class);
            bind_variable(scopes, target.clone(), Ty::Set(Box::new(elt.ty())));
            MirStmt::SetCompAssign {
                target: target.clone(),
                var: var.clone(),
                var_ty,
                source,
                cond: cond.map(Box::new),
                elt: Box::new(elt),
            }
        }
        HirStmt::DictCompAssign {
            target,
            var,
            iter,
            cond,
            key,
            value,
        } => {
            let (source, var_ty) = resolve_comp_source(iter, var, scopes, classes, current_class);
            let cond = cond
                .as_deref()
                .map(|c| lower_expr(c, scopes, classes, current_class));
            let key = lower_expr(key, scopes, classes, current_class);
            let value = lower_expr(value, scopes, classes, current_class);
            bind_variable(
                scopes,
                target.clone(),
                Ty::Dict(Box::new((key.ty(), value.ty()))),
            );
            MirStmt::DictCompAssign {
                target: target.clone(),
                var: var.clone(),
                var_ty,
                source,
                cond: cond.map(Box::new),
                key: Box::new(key),
                value: Box::new(value),
            }
        }
        HirStmt::Return(value) => MirStmt::Return(
            value
                .as_ref()
                .map(|v| lower_expr(v, scopes, classes, current_class)),
        ),
        HirStmt::DictSet { dict, key, value } => MirStmt::DictSet {
            dict: dict.clone(),
            key: lower_expr(key, scopes, classes, current_class),
            value: lower_expr(value, scopes, classes, current_class),
        },
        // D-154 (Part 1 of #375): `base.attr = value`, resolved to a
        // compile-time slot index exactly like `MirExpr::AttrGet` above.
        // #377: if `attr` is a `@property` with a setter, the assignment is
        // rewritten to an ordinary `MirStmt::ExprStmt(MirExpr::Call)` to
        // the setter's mangled name (with `base` as `self` and `value` as
        // the setter's parameter), reusing the existing method-call/codegen
        // infrastructure with no new MIR/codegen variant. A read-only
        // property (no setter) never reaches here -- `pycc_types::check`
        // rejects it with `T0044` before MIR lowering runs.
        HirStmt::AttrSet { base, attr, value } => {
            let base = lower_expr(base, scopes, classes, current_class);
            let value = lower_expr(value, scopes, classes, current_class);
            let class_def = class_def_of(&base, classes);
            // #432: walk the MRO for property lookup first (matching
            // `AttrGet`'s own MRO walk), then for regular attribute slots
            // using the flat MRO layout.
            for mro_class in &class_def.mro {
                let mro_def = mro_class_def(mro_class, classes);
                if let Some(prop) = mro_def.properties.iter().find(|p| p.name == *attr) {
                    let setter = prop.setter.as_ref().unwrap_or_else(|| {
                        panic!(
                            "pycc_mir: internal error: property `{attr}` on class `{mro_class}` \
                             has no setter -- pycc_types::check should have rejected this \
                             assignment before it reached pycc_mir"
                        )
                    });
                    let ty = lookup(scopes, &format!("$fn:{setter}"));
                    return MirStmt::ExprStmt(MirExpr::Call {
                        callee: setter.clone(),
                        args: vec![base, value],
                        ty,
                    });
                }
            }
            let flat_attrs = mro_attrs(class_def, classes);
            let (slot, _) = flat_attrs
                .iter()
                .enumerate()
                .find(|(_, (name, _))| name == attr)
                .unwrap_or_else(|| {
                    panic!(
                        "pycc_mir: internal error: attribute `{attr}` not declared on class `{}` \
                         or any base in its MRO -- pycc_types::check should have rejected this \
                         HIR before it reached pycc_mir",
                        class_def.name
                    )
                });
            MirStmt::AttrSet { base, slot, value }
        }
        HirStmt::Match { subject, cases } => {
            lower_match(subject, cases, scopes, classes, current_class)
        }
        HirStmt::Try { body, handlers, orelse, finalbody } => {
            let body = body
                .iter()
                .map(|s| lower_stmt(s, scopes, classes, current_class))
                .collect();
            let handlers = handlers.iter().map(|h| {
                let exc_type_tag = h.exc_type.as_deref().and_then(resolve_exception_tag);
                let handler_body = h.body
                    .iter()
                    .map(|s| lower_stmt(s, scopes, classes, current_class))
                    .collect();
                MirExceptHandler {
                    exc_type_tag,
                    body: handler_body,
                }
            }).collect();
            let orelse = orelse
                .iter()
                .map(|s| lower_stmt(s, scopes, classes, current_class))
                .collect();
            let finalbody = finalbody
                .iter()
                .map(|s| lower_stmt(s, scopes, classes, current_class))
                .collect();
            MirStmt::Try {
                body,
                handlers,
                orelse,
                finalbody,
            }
        }
        HirStmt::Raise { exc, cause } => {
            lower_raise(exc, cause, scopes, classes, current_class)
        }
    }
}

/// PEP 634-636 (#381, PR-21): Lowers a `match` statement into nested
/// `MirStmt::If` chains. The subject is evaluated once and stored in a
/// synthesized temporary (`__match_subj_N`); each case becomes an `if`
/// branch whose test is the pattern-match condition, whose body is the
/// case body (preceded by binding assignments), and whose `orelse` is
/// the next case (or `NoOp` for the final arm). Guards are handled via
/// a nested `if` inside the matched arm's body, since MIR has no `and`.
fn lower_match(
    subject: &HirExpr,
    cases: &[HirMatchCase],
    scopes: &mut Vec<HashMap<String, Ty>>,
    classes: &HashMap<String, HirClassDef>,
    current_class: Option<&str>,
) -> MirStmt {
    let subj_expr = lower_expr(subject, scopes, classes, current_class);
    let subj_ty = subj_expr.ty();
    let subj_var = format!(
        "__match_subj_{}",
        MATCH_SUBJECT_COUNTER.fetch_add(1, Ordering::SeqCst)
    );
    bind_variable(scopes, subj_var.clone(), subj_ty.clone());
    let assign = MirStmt::Assign {
        target: subj_var.clone(),
        value: subj_expr,
    };
    let chain = lower_match_chain(
        &subj_var,
        &subj_ty,
        cases,
        scopes,
        classes,
        current_class,
    );
    MirStmt::Seq(vec![assign, chain])
}

/// #382 (PR-22 Part 1): Resolves a builtin exception class name to its
/// runtime type tag (matching `pycc_rt`'s `EXCEPTION_TYPE_*` constants).
/// Returns `None` for unknown names (should not happen after type
/// checking, but handled gracefully).
fn resolve_exception_tag(name: &str) -> Option<u8> {
    match name {
        "Exception" => Some(0),
        "ValueError" => Some(1),
        "TypeError" => Some(2),
        "KeyError" => Some(3),
        "IndexError" => Some(4),
        "ZeroDivisionError" => Some(5),
        "RuntimeError" => Some(6),
        _ => None,
    }
}

/// #382 (PR-22 Part 1): Lowers a `raise` statement into MIR.
/// - `raise ExceptionType("msg")` → `MirStmt::Raise { exc_type_tag, message }`
/// - `raise ExceptionType("msg") from CauseType("cause")` → `MirStmt::RaiseFrom { ... }`
/// - bare `raise` → `MirStmt::Reraise`
fn lower_raise(
    exc: &Option<HirExpr>,
    cause: &Option<HirExpr>,
    scopes: &mut [HashMap<String, Ty>],
    classes: &HashMap<String, HirClassDef>,
    current_class: Option<&str>,
) -> MirStmt {
    // Bare `raise` (re-raise).
    if exc.is_none() {
        return MirStmt::Reraise;
    }
    let exc_expr = exc.as_ref().unwrap();
    // The exc expression should be `HirExpr::Call { callee: "ExceptionType", args: [msg] }`.
    // Extract the type name and message.
    let (exc_type_tag, message) = extract_exception_call(exc_expr, scopes, classes, current_class);
    // Check for `from cause`.
    if let Some(cause_expr) = cause {
        let (cause_type_tag, cause_message) =
            extract_exception_call(cause_expr, scopes, classes, current_class);
        return MirStmt::RaiseFrom {
            exc_type_tag,
            message,
            cause_type_tag,
            cause_message,
        };
    }
    MirStmt::Raise {
        exc_type_tag,
        message,
    }
}

/// #382 (PR-22 Part 1): Extracts the type tag and message expression from
/// a `HirExpr::Call { callee: "ExceptionType", args: [msg] }` expression.
/// Falls back to tag 0 (Exception) and a string literal "unknown" if the
/// shape doesn't match (should not happen after type checking).
fn extract_exception_call(
    expr: &HirExpr,
    scopes: &mut [HashMap<String, Ty>],
    classes: &HashMap<String, HirClassDef>,
    current_class: Option<&str>,
) -> (u8, MirExpr) {
    if let HirExpr::Call { callee, args } = expr {
        let tag = resolve_exception_tag(callee).unwrap_or(0);
        let message = if let Some(msg_expr) = args.first() {
            lower_expr(msg_expr, scopes, classes, current_class)
        } else {
            MirExpr::StringLiteral("unknown".to_string())
        };
        return (tag, message);
    }
    // Fallback: should not happen after type checking.
    (0, MirExpr::StringLiteral("unknown".to_string()))
}

/// Builds the nested `if` chain for match cases. Each case's pattern
/// produces a list of alternative condition-lists (any alternative's
/// conditions all being true is sufficient); these are nested as `if`
/// statements, with the innermost body containing the bindings and case
/// body (or a guard `if` if present).
fn lower_match_chain(
    subj_var: &str,
    subj_ty: &Ty,
    cases: &[HirMatchCase],
    scopes: &mut Vec<HashMap<String, Ty>>,
    classes: &HashMap<String, HirClassDef>,
    current_class: Option<&str>,
) -> MirStmt {
    if cases.is_empty() {
        return MirStmt::NoOp;
    }
    let case = &cases[0];
    let rest = &cases[1..];
    let subj_ref = MirExpr::Name {
        name: subj_var.to_string(),
        ty: subj_ty.clone(),
    };
    let (alternatives, bindings) = lower_pattern_conds(
        &subj_ref,
        &case.pattern,
        scopes,
        classes,
        current_class,
    );
    for (name, val) in &bindings {
        let ty = val.ty();
        bind_variable(scopes, name.clone(), ty);
    }
    let binding_stmts: Vec<MirStmt> = bindings
        .iter()
        .map(|(name, value)| MirStmt::Assign {
            target: name.clone(),
            value: value.clone(),
        })
        .collect();
    let case_body: Vec<MirStmt> = case
        .body
        .iter()
        .map(|s| lower_stmt(s, scopes, classes, current_class))
        .collect();
    let else_chain = lower_match_chain(subj_var, subj_ty, rest, scopes, classes, current_class);
    let inner_body = if let Some(guard) = &case.guard {
        let guard_cond = lower_expr(guard, scopes, classes, current_class);
        let mut body = binding_stmts;
        body.push(MirStmt::If {
            test: guard_cond,
            body: case_body,
            orelse: vec![else_chain.clone()],
        });
        body
    } else {
        let mut body = binding_stmts;
        body.extend(case_body);
        body
    };
    nest_match_alternatives(&alternatives, inner_body, else_chain)
}

/// Nests alternative condition-lists into a chain of `if` statements.
/// Each alternative is a conjunction (all conditions must be true); the
/// alternatives are combined as a disjunction (any one matching is
/// sufficient). The innermost `then` body is `inner_body`; each
/// alternative's `else` falls through to the next alternative, and the
/// last alternative's `else` falls through to `else_chain`.
fn nest_match_alternatives(
    alternatives: &[Vec<MirExpr>],
    inner_body: Vec<MirStmt>,
    else_chain: MirStmt,
) -> MirStmt {
    if alternatives.is_empty() {
        return MirStmt::Seq(inner_body);
    }
    let (first, rest) = alternatives.split_first().unwrap();
    let next_else = if rest.is_empty() {
        else_chain.clone()
    } else {
        nest_match_alternatives(rest, inner_body.clone(), else_chain.clone())
    };
    nest_match_conds(first, inner_body, next_else)
}

/// Nests a single alternative's conditions into a chain of `if`
/// statements. The innermost `then` body is `inner_body`; every `else`
/// falls through to `else_chain`.
fn nest_match_conds(
    conds: &[MirExpr],
    inner_body: Vec<MirStmt>,
    else_chain: MirStmt,
) -> MirStmt {
    if conds.is_empty() {
        return MirStmt::Seq(inner_body);
    }
    let (first, rest) = conds.split_first().unwrap();
    MirStmt::If {
        test: first.clone(),
        body: vec![nest_match_conds(rest, inner_body.clone(), else_chain.clone())],
        orelse: vec![else_chain],
    }
}

/// Lowers a pattern into a list of alternative condition-lists (each
/// inner list is a conjunction; the outer list is a disjunction) and a
/// list of binding assignments. For irrefutable patterns (wildcard,
/// capture), the alternatives list contains a single empty inner list
/// (always matches). For Or-patterns, each sub-pattern's alternatives
/// are flattened into the outer list.
fn lower_pattern_conds(
    subj: &MirExpr,
    pattern: &HirPattern,
    scopes: &[HashMap<String, Ty>],
    classes: &HashMap<String, HirClassDef>,
    current_class: Option<&str>,
) -> (Vec<Vec<MirExpr>>, Vec<(String, MirExpr)>) {
    match pattern {
        HirPattern::Wildcard => (vec![vec![]], vec![]),
        HirPattern::Capture(name) => {
            (vec![vec![]], vec![(name.clone(), subj.clone())])
        }
        HirPattern::Literal(lit) => {
            let lowered = lower_expr(lit, scopes, classes, current_class);
            (
                vec![vec![MirExpr::Compare {
                    op: CmpOpKind::Eq,
                    left: Box::new(subj.clone()),
                    right: Box::new(lowered),
                    ty: Ty::Bool,
                }]],
                vec![],
            )
        }
        HirPattern::Singleton(b) => (
            vec![vec![MirExpr::Compare {
                op: CmpOpKind::Eq,
                left: Box::new(subj.clone()),
                right: Box::new(MirExpr::BoolLiteral(*b)),
                ty: Ty::Bool,
            }]],
            vec![],
        ),
        HirPattern::NoneSingleton => (
            vec![vec![MirExpr::Compare {
                op: CmpOpKind::Eq,
                left: Box::new(subj.clone()),
                right: Box::new(MirExpr::Name {
                    name: "None".to_string(),
                    ty: Ty::None,
                }),
                ty: Ty::Bool,
            }]],
            vec![],
        ),
        HirPattern::Sequence(sub_pats) => {
            lower_sequence_conds(subj, sub_pats, None, scopes, classes, current_class)
        }
        HirPattern::SequenceStar(sub_pats, rest) => {
            lower_sequence_conds(subj, sub_pats, rest.as_ref(), scopes, classes, current_class)
        }
        HirPattern::Mapping(pairs, rest) => {
            lower_mapping_conds(subj, pairs, rest.as_ref(), scopes, classes, current_class)
        }
        HirPattern::Class {
            class_name,
            positional,
            keyword,
        } => lower_class_conds(
            subj, class_name, positional, keyword, scopes, classes, current_class,
        ),
        HirPattern::Or(subs) => {
            let mut all_alts = Vec::new();
            let mut all_bindings = Vec::new();
            for sub in subs {
                let (alts, b) = lower_pattern_conds(subj, sub, scopes, classes, current_class);
                all_alts.extend(alts);
                if all_bindings.is_empty() {
                    all_bindings = b;
                }
            }
            (all_alts, all_bindings)
        }
        HirPattern::As(inner, name) => {
            let (alts, bindings) =
                lower_pattern_conds(subj, inner, scopes, classes, current_class);
            let mut all = bindings;
            all.push((name.clone(), subj.clone()));
            (alts, all)
        }
    }
}

/// Lowers a sequence pattern into conditions: a length check plus
/// per-element sub-pattern conditions.
fn lower_sequence_conds(
    subj: &MirExpr,
    sub_pats: &[HirPattern],
    rest: Option<&String>,
    scopes: &[HashMap<String, Ty>],
    classes: &HashMap<String, HirClassDef>,
    current_class: Option<&str>,
) -> (Vec<Vec<MirExpr>>, Vec<(String, MirExpr)>) {
    let fixed = sub_pats.len();
    let len_cond = MirExpr::Compare {
        op: if rest.is_some() { CmpOpKind::GtE } else { CmpOpKind::Eq },
        left: Box::new(MirExpr::Call {
            callee: "len".to_string(),
            args: vec![subj.clone()],
            ty: Ty::Int,
        }),
        right: Box::new(MirExpr::IntLiteral(fixed as i64)),
        ty: Ty::Bool,
    };
    let mut conds = vec![len_cond];
    let mut bindings = Vec::new();
    for (i, sub_pat) in sub_pats.iter().enumerate() {
        let elem = MirExpr::Subscript {
            base: Box::new(subj.clone()),
            index: Box::new(MirExpr::IntLiteral(i as i64)),
        };
        let (alts, b) = lower_pattern_conds(&elem, sub_pat, scopes, classes, current_class);
        for alt in alts {
            conds.extend(alt);
        }
        bindings.extend(b);
    }
    if let Some(rest_name) = rest {
        bindings.push((rest_name.clone(), subj.clone()));
    }
    (vec![conds], bindings)
}

/// Lowers a mapping pattern into per-key-value check conditions.
fn lower_mapping_conds(
    subj: &MirExpr,
    pairs: &[(HirExpr, HirPattern)],
    rest: Option<&String>,
    scopes: &[HashMap<String, Ty>],
    classes: &HashMap<String, HirClassDef>,
    current_class: Option<&str>,
) -> (Vec<Vec<MirExpr>>, Vec<(String, MirExpr)>) {
    let mut conds = Vec::new();
    let mut bindings = Vec::new();
    for (key_expr, val_pat) in pairs {
        let key_lowered = lower_expr(key_expr, scopes, classes, current_class);
        let val = MirExpr::DictGet {
            dict: Box::new(subj.clone()),
            key: Box::new(key_lowered),
        };
        let (alts, b) = lower_pattern_conds(&val, val_pat, scopes, classes, current_class);
        for alt in alts {
            conds.extend(alt);
        }
        bindings.extend(b);
    }
    if let Some(rest_name) = rest {
        bindings.push((rest_name.clone(), subj.clone()));
    }
    (vec![conds], bindings)
}

/// Lowers a class pattern into per-attribute check conditions.
#[allow(clippy::expect_fun_call)]
fn lower_class_conds(
    subj: &MirExpr,
    class_name: &str,
    positional: &[HirPattern],
    keyword: &[(String, HirPattern)],
    scopes: &[HashMap<String, Ty>],
    classes: &HashMap<String, HirClassDef>,
    current_class: Option<&str>,
) -> (Vec<Vec<MirExpr>>, Vec<(String, MirExpr)>) {
    let class_def = classes.get(class_name).expect(&format!(
        "pycc_mir: internal error: class `{class_name}` has no registered HirClassDef -- \
         pycc_types::check should have rejected this HIR before it reached pycc_mir"
    ));
    let flat_attrs = mro_attrs(class_def, classes);
    let mut conds = Vec::new();
    let mut bindings = Vec::new();
    for (i, sub_pat) in positional.iter().enumerate() {
        let (_, ty) = flat_attrs.get(i).expect(&format!(
            "pycc_mir: internal error: class `{class_name}` has fewer attributes than \
             positional patterns -- pycc_types::check should have rejected this HIR \
             before it reached pycc_mir"
        ));
        let attr_val = MirExpr::AttrGet {
            base: Box::new(subj.clone()),
            slot: i,
            ty: ty.clone(),
        };
        let (alts, b) = lower_pattern_conds(&attr_val, sub_pat, scopes, classes, current_class);
        for alt in alts {
            conds.extend(alt);
        }
        bindings.extend(b);
    }
    for (attr_name, sub_pat) in keyword {
        let (slot, (_, ty)) = flat_attrs
            .iter()
            .enumerate()
            .find(|(_, (name, _))| name == attr_name)
            .expect(&format!(
                "pycc_mir: internal error: attribute `{attr_name}` not declared on class \
                 `{class_name}` or any base in its MRO -- pycc_types::check should have \
                 rejected this HIR before it reached pycc_mir"
            ));
        let attr_val = MirExpr::AttrGet {
            base: Box::new(subj.clone()),
            slot,
            ty: ty.clone(),
        };
        let (alts, b) = lower_pattern_conds(&attr_val, sub_pat, scopes, classes, current_class);
        for alt in alts {
            conds.extend(alt);
        }
        bindings.extend(b);
    }
    (vec![conds], bindings)
}
/// on the enum class) to `MirExpr::Name` reading the synthetic
/// `<Class>.<Member>.enum_member` global. Returns `None` if `base` is not
/// an enum class name or `attr` is not one of its members. Extracted from
/// `lower_expr` to isolate the enum-specific code paths (see
/// cargo-llvm-cov#276 for the coverage instantiation issue).
fn try_lower_enum_member_attr(
    base: &HirExpr,
    attr: &str,
    classes: &HashMap<String, HirClassDef>,
) -> Option<MirExpr> {
    if let HirExpr::Name(class_name) = base
        && let Some(class_def) = classes.get(class_name.as_str())
        && !class_def.enum_members.is_empty()
        && class_def.enum_members.iter().any(|(name, _)| name == attr)
    {
        return Some(MirExpr::Name {
            name: format!("{class_name}.{attr}.enum_member"),
            ty: Ty::Instance(Box::new(class_name.clone())),
        });
    }
    None
}

fn lower_expr(
    expr: &HirExpr,
    scopes: &[HashMap<String, Ty>],
    classes: &HashMap<String, HirClassDef>,
    current_class: Option<&str>,
) -> MirExpr {
    match expr {
        HirExpr::IntLiteral(n) => MirExpr::IntLiteral(*n),
        HirExpr::FloatLiteral(f) => MirExpr::FloatLiteral(*f),
        HirExpr::BoolLiteral(b) => MirExpr::BoolLiteral(*b),
        HirExpr::StringLiteral(s) => MirExpr::StringLiteral(s.clone()),
        // D-136: `math.pi` (a `pycc_hir`-qualified stdlib constant name --
        // real Python identifiers never contain `.`, see
        // `pycc_types::std_qualified_symbol`'s own doc comment for the
        // same invariant) is never bound in `scopes` the way an ordinary
        // assigned variable is, so it needs its own arm here rather than
        // falling into the ordinary `lookup` below, which would panic.
        HirExpr::Name(name) if name == "math.pi" => MirExpr::Name {
            name: name.clone(),
            ty: Ty::Float,
        },
        HirExpr::Name(name) => MirExpr::Name {
            name: name.clone(),
            ty: lookup(scopes, name),
        },
        HirExpr::Call { callee, args } => {
            // #435: `isinstance`/`issubclass` are compile-time-evaluated
            // builtins. They must be intercepted BEFORE the generic arg
            // lowering below, because the class arguments are class names
            // (not value expressions) and would fail to lower as ordinary
            // MIR expressions. The object argument (isinstance's args[0])
            // IS lowered normally to extract its type.
            // A user-defined function named `isinstance`/`issubclass` takes
            // priority over the builtin (same pattern as `float` — the type
            // checker's identical guard ensures a user-defined version is
            // never intercepted here, but the MIR guard is defense-in-depth
            // and mirrors the `float` builtin's own MIR-side check).
            let is_user_defined = scopes
                .iter()
                .any(|s| s.contains_key(&format!("$fn:{callee}")));
            if callee == "isinstance" && !is_user_defined {
                return lower_isinstance(args, scopes, classes, current_class);
            }
            if callee == "issubclass" && !is_user_defined {
                return lower_issubclass(args, classes);
            }
            let args: Vec<MirExpr> = args
                .iter()
                .map(|a| {
                    let lowered = lower_expr(a, scopes, classes, current_class);
                    // #378 (PR-18): for `print(instance)`, rewrite an
                    // instance-typed argument to a `__repr__` call so the
                    // codegen's `to_str` receives a `str` scalar. Only
                    // `print` feeds non-str values to `to_str` among call
                    // expressions (f-string interpolations are handled in
                    // the `FString` arm); other calls pass instance
                    // arguments directly as opaque pointers.
                    if callee == "print" {
                        rewrite_instance_to_repr(&lowered, classes)
                    } else {
                        lowered
                    }
                })
                .collect();
            // D-154 (Part 1 of #375): `ClassName(args)` (instantiation)
            // reuses `HirExpr::Call` -- there is no dedicated HIR shape for
            // it (`pycc_hir::class`'s own doc comment) -- so it is resolved
            // here, before the ordinary `$fn:` lookup below (which has no
            // entry for a bare class name; only its mangled methods are
            // registered). See `MirExpr::Instantiate`'s own doc comment for
            // why this needs a dedicated MIR node rather than folding into
            // `MirExpr::Call` the way `MethodCall` below does.
            if let Some(class_def) = classes.get(callee.as_str()) {
                // #432: resolve `__init__` via the MRO -- a derived class
                // without its own `__init__` inherits the base class's
                // constructor. The MRO is ordered most-derived-first, so
                // the first `__init__` found is the one to call.
                let ctor = class_def
                    .mro
                    .iter()
                    .find_map(|mro_class| {
                        let mro_def = classes.get(mro_class.as_str())?;
                        if mro_def.methods.iter().any(|(mn, _)| mn == "__init__") {
                            Some(format!("{mro_class}.__init__"))
                        } else {
                            None
                        }
                    })
                    .unwrap_or_else(|| {
                        panic!(
                            "pycc_mir: internal error: no `__init__` found in class `{callee}`'s \
                         MRO -- pycc_hir::lower_class should have rejected this before it \
                         reached pycc_mir"
                        )
                    });
                return MirExpr::Instantiate(Box::new(InstantiateExpr {
                    ctor,
                    // #432: allocate slots for all unique attributes across the
                    // MRO, not just this class's own declared attributes.
                    attr_count: mro_attr_count(class_def, classes),
                    args,
                    ty: Ty::Instance(Box::new(callee.clone())),
                }));
            }
            let ty = if callee == "print" {
                Ty::None
            } else if callee == "math.sqrt" {
                // D-136: `math.sqrt` is the other hand-recognized stdlib
                // intrinsic this PR lowers to real codegen -- mirrors
                // `pycc_types`'s own `std_qualified_symbol` dispatch
                // (always `Ty::Float`, this registry's only lowered
                // function's fixed return type; not a general per-registry
                // lookup since `pycc_mir` has no dependency on `pycc_std`
                // and does not need one for this PR's exactly-one-function
                // registry).
                Ty::Float
            } else if callee == "len" {
                // `len` is a hand-recognized builtin, same as `print` above,
                // not a user-declarable `$fn:` signature -- mirrors
                // `pycc_types::collect_expr_constraints`'s own `callee ==
                // "len"` arm (D-105 point 3). Without this branch, `len(lst)`
                // falls to the `lookup` fallback below, finds no registered
                // `$fn:len`, and panics even though `pycc_types` already
                // accepts `len(lst)` as valid, `Ty::Int`-typed.
                Ty::Int
            } else if callee == "float"
                && !scopes
                    .iter()
                    .any(|scope| scope.contains_key(&format!("$fn:{callee}")))
            {
                // Mirrors `pycc_types`' own `callee == "float"` arms (both the
                // public-body and private-helper paths), including their own
                // user-defined-function-takes-priority guard -- see
                // `pycc_types::infer_expr_in`'s comment for why `float` (unlike
                // `len`/`print`) needs this. Always `Ty::Float`.
                Ty::Float
            } else {
                lookup(scopes, &format!("$fn:{callee}"))
            };
            MirExpr::Call {
                callee: callee.clone(),
                args,
                ty,
            }
        }
        HirExpr::BinOp { op, left, right } => {
            let left = lower_expr(left, scopes, classes, current_class);
            let right = lower_expr(right, scopes, classes, current_class);
            let ty = binop_result_ty(*op, left.ty(), right.ty());
            MirExpr::BinOp {
                op: *op,
                left: Box::new(left),
                right: Box::new(right),
                ty,
            }
        }
        HirExpr::Compare { op, left, right } => {
            let left_lowered = lower_expr(left, scopes, classes, current_class);
            let right_lowered = lower_expr(right, scopes, classes, current_class);
            // #378 (PR-18): `==`/`!=` between same-class dataclass instances
            // is rewritten to a `MirExpr::Call` to the class's
            // compiler-synthesized `__eq__` method. `!=` is lowered as
            // `__eq__(left, right) != True` (the negation), since pycc's
            // MIR has no `UnaryOp::Not` node. This mirrors how `@property`
            // redirects attribute access to method calls -- a MIR-level
            // rewrite, not a new MIR node. Only `Eq` and `NotEq` are
            // rewritten, and only for dataclass classes (whose synthesized
            // `__eq__` has a known-correct signature); other comparison
            // operators (`<`, `<=`, `>`, `>=`) and non-dataclass classes
            // fall through to the default `MirExpr::Compare` (the type
            // checker rejects them with `T0021` before they reach MIR
            // lowering in normal compilation, but the MIR itself stays
            // semantically correct for defense-in-depth).
            if matches!(op, pycc_hir::CmpOpKind::Eq | pycc_hir::CmpOpKind::NotEq)
                && let (Ty::Instance(left_class), Ty::Instance(right_class)) =
                    (left_lowered.ty(), right_lowered.ty())
                && left_class == right_class
                && let Some(class_def) = classes.get(left_class.as_str())
                && class_def.is_dataclass
            {
                let eq_mangled = class_def.mro.iter().find_map(|mro_class| {
                    // Every class in the MRO was registered when the class
                    // was lowered; using `.expect` (whose panic path lives
                    // in libcore, outside this crate's instrumented regions)
                    // avoids a `?` whose `None` branch is structurally
                    // unreachable and would show up as a permanently
                    // uncovered region under D-014's 100% coverage gate.
                    let mro_def = classes
                        .get(mro_class.as_str())
                        .expect("MRO class must be registered");
                    mro_def
                        .methods
                        .iter()
                        .find(|(mn, _)| mn == "__eq__")
                        .map(|(_, mangled)| mangled.clone())
                });
                // A dataclass always has a synthesized `__eq__` in its
                // MRO (the `is_dataclass` guard above ensures we only
                // enter this block for dataclass classes). Using
                // `.expect` (whose panic path lives in libcore, outside
                // this crate's instrumented regions) avoids an `if let
                // Some` whose `None` branch is structurally unreachable
                // for a dataclass and would show up as a permanently
                // uncovered region under D-014's 100% coverage gate.
                let eq_mangled = eq_mangled.expect("dataclass must have __eq__");
                let eq_call = MirExpr::Call {
                    callee: eq_mangled,
                    args: vec![left_lowered, right_lowered],
                    ty: Ty::Bool,
                };
                // The `matches!` guard above limits entry to
                // `Eq`/`NotEq`, so an if/else (rather than a match
                // with a `_` arm) suffices and avoids an unreachable
                // arm that would be permanently uncovered under
                // D-014's 100% coverage gate.
                if *op == pycc_hir::CmpOpKind::Eq {
                    return eq_call;
                }
                return MirExpr::Compare {
                    op: pycc_hir::CmpOpKind::NotEq,
                    left: Box::new(eq_call),
                    right: Box::new(MirExpr::BoolLiteral(true)),
                    ty: Ty::Bool,
                };
            }
            MirExpr::Compare {
                op: *op,
                left: Box::new(left_lowered),
                right: Box::new(right_lowered),
                ty: Ty::Bool,
            }
        }
        HirExpr::FString(parts) => MirExpr::FString(
            parts
                .iter()
                .map(|p| match p {
                    FStringPart::Literal(s) => MirFStringPart::Literal(s.clone()),
                    FStringPart::Interpolation(e) => {
                        let lowered = lower_expr(e, scopes, classes, current_class);
                        // #378 (PR-18): if the interpolation is a class
                        // instance with `__repr__`, rewrite it to a call to
                        // `__repr__` so the codegen's `to_str` receives a
                        // `str` scalar, not an Instance scalar (which would
                        // panic). This mirrors how `==`/`!=` on instances is
                        // rewritten to `__eq__` calls in the Compare arm.
                        let rewritten = rewrite_instance_to_repr(&lowered, classes);
                        MirFStringPart::Interpolation(Box::new(rewritten))
                    }
                })
                .collect(),
        ),
        HirExpr::ListLiteral(elements) => MirExpr::ListLiteral(
            elements
                .iter()
                .map(|e| lower_expr(e, scopes, classes, current_class))
                .collect(),
        ),
        // `HirExpr::Subscript` is reused unconditionally by `pycc_hir`'s own
        // lowering for both a list read and a dict read (it has no type
        // information to pick a different node) -- `pycc_types::infer_expr_in`
        // accepts either base equally (see its own `Subscript` arm), so this
        // is the point where the real type is resolved and where a
        // dict-typed base is routed into `MirExpr::DictGet` instead of
        // `MirExpr::Subscript`, mirroring `lower_stmt`'s own `HirStmt::ForList`
        // arm doing the same list/dict routing for iteration.
        HirExpr::Subscript { base, index } => {
            let base = lower_expr(base, scopes, classes, current_class);
            let index = lower_expr(index, scopes, classes, current_class);
            match base.ty() {
                Ty::Dict(_) => MirExpr::DictGet {
                    dict: Box::new(base),
                    key: Box::new(index),
                },
                _ => MirExpr::Subscript {
                    base: Box::new(base),
                    index: Box::new(index),
                },
            }
        }
        HirExpr::ListAppend { list, value } => MirExpr::ListAppend {
            list: list.clone(),
            value: Box::new(lower_expr(value, scopes, classes, current_class)),
        },
        HirExpr::DictLiteral(pairs) => MirExpr::DictLiteral(
            pairs
                .iter()
                .map(|(k, v)| {
                    (
                        lower_expr(k, scopes, classes, current_class),
                        lower_expr(v, scopes, classes, current_class),
                    )
                })
                .collect(),
        ),
        HirExpr::SetLiteral(elements) => MirExpr::SetLiteral(
            elements
                .iter()
                .map(|e| lower_expr(e, scopes, classes, current_class))
                .collect(),
        ),
        HirExpr::TupleLiteral(elements) => MirExpr::TupleLiteral(
            elements
                .iter()
                .map(|e| lower_expr(e, scopes, classes, current_class))
                .collect(),
        ),
        // PR-12 Task 8 (D-118): purely structural -- recurse into `base` and
        // every present bound, same as every other MIR-lowering site in this
        // function. `pycc_types` already validated the base is
        // `Ty::List(Int)` and every present bound is `int`-assignable
        // (Task 7); this lowering does not re-validate or apply the runtime
        // clamping/panic behavior D-118 requires -- that is
        // `pycc_codegen`'s job, operating on this already-lowered shape.
        HirExpr::Slice {
            base,
            start,
            stop,
            step,
        } => MirExpr::Slice {
            base: Box::new(lower_expr(base, scopes, classes, current_class)),
            start: start
                .as_deref()
                .map(|e| Box::new(lower_expr(e, scopes, classes, current_class))),
            stop: stop
                .as_deref()
                .map(|e| Box::new(lower_expr(e, scopes, classes, current_class))),
            step: step
                .as_deref()
                .map(|e| Box::new(lower_expr(e, scopes, classes, current_class))),
        },
        // PR-12 Task 11 (D-119): `list`'s element type is resolved via the
        // same `lookup` mechanism every other name reference in this file
        // uses, mirroring `HirExpr::Subscript`'s own base-type lookup above.
        HirExpr::ListPop { list } => {
            let Ty::List(elem_ty) = lookup(scopes, list) else {
                panic!(
                    "pycc_mir: internal error: `{list}` is not list-typed -- pycc_types::check should have rejected this HIR before it reached pycc_mir"
                )
            };
            MirExpr::ListPop {
                list: list.clone(),
                ty: *elem_ty,
            }
        }
        HirExpr::DictGetOrDefault { dict, key, default } => {
            let Ty::Dict(kv) = lookup(scopes, dict) else {
                panic!(
                    "pycc_mir: internal error: `{dict}` is not dict-typed -- pycc_types::check should have rejected this HIR before it reached pycc_mir"
                )
            };
            MirExpr::DictGetOrDefault {
                dict: dict.clone(),
                key: Box::new(lower_expr(key, scopes, classes, current_class)),
                default: Box::new(lower_expr(default, scopes, classes, current_class)),
                ty: kv.1,
            }
        }
        HirExpr::SetAdd { set, value } => MirExpr::SetAdd {
            set: set.clone(),
            value: Box::new(lower_expr(value, scopes, classes, current_class)),
        },
        // D-154 (Part 1 of #375): `base.attr` -- resolved to a compile-time
        // slot index against the base's class's `HirClassDef`, per the
        // class-instance-layout ADR (never a runtime string-keyed lookup).
        // #377: if `attr` is a `@property`, the access is rewritten to an
        // ordinary `MirExpr::Call` to the getter's mangled name (with `base`
        // as `self`), reusing the existing method-call/codegen infrastructure
        // with no new MIR/codegen variant.
        // #432: property and attribute resolution walk the MRO, so an
        // attribute or property declared in a base class is found when
        // accessed on a derived class instance. The slot index is computed
        // from the MRO's flat attribute layout (`mro_attrs`).
        HirExpr::AttrGet { base, attr } => {
            // #433: `super().attr` — resolve the attribute starting from
            // the next class in the current class's MRO, using `self` (the
            // current function's first parameter) as the instance. The slot
            // index is still computed from the full MRO's flat layout, so
            // the same slot offset the base class's own methods would use
            // is reused here — `self` is the same object either way.
            if matches!(base.as_ref(), HirExpr::Super) {
                let current = match current_class {
                    Some(c) => c,
                    None => panic!(
                        "pycc_mir: internal error: `HirExpr::Super` reached lower_expr outside a \
                         method body -- pycc_hir::lower_expr should have rejected this with C0001"
                    ),
                };
                let self_expr = self_expr(scopes);
                let class_def = &classes[current];
                let current_pos = class_def
                    .mro
                    .iter()
                    .position(|c| c == current)
                    .expect("pycc_mir: internal error: class not found in its own MRO");
                let super_mro = &class_def.mro[current_pos + 1..];
                // Properties first (matching the non-super AttrGet arm).
                for mro_class in super_mro {
                    let mro_def = &classes[mro_class.as_str()];
                    if let Some(prop) = mro_def.properties.iter().find(|p| p.name == *attr) {
                        let ty = lookup(scopes, &format!("$fn:{}", prop.getter));
                        return MirExpr::Call {
                            callee: prop.getter.clone(),
                            args: vec![self_expr],
                            ty,
                        };
                    }
                }
                // Regular attribute slots — the slot index comes from the
                // full MRO's flat layout (since `self` is the same object),
                // but the *type* must come from the super_mro slice to match
                // the type checker's `resolve_super_attr_get`. A derived
                // class may redeclare an attribute with a different type;
                // using the full layout's type would pick the most-derived
                // declaration, but `super().attr` should use the base class's
                // declaration (#433 review fix).
                let flat_attrs = mro_attrs(class_def, classes);
                let slot = flat_attrs
                    .iter()
                    .enumerate()
                    .find(|(_, (name, _))| name == attr)
                    .map(|(slot, _)| slot)
                    .expect(
                        "pycc_mir: internal error: attribute not declared on class or any base in \
                         its MRO -- pycc_types::check should have rejected this HIR before it \
                         reached pycc_mir",
                    );
                let ty = super_mro
                    .iter()
                    .find_map(|mro_class| {
                        let mro_def = &classes[mro_class.as_str()];
                        mro_def.attrs.iter().find(|(name, _)| name == attr)
                    })
                    .map(|(_, ty)| ty.clone())
                    .expect(
                        "pycc_mir: internal error: attribute not found in super_mro -- \
                         pycc_types::check should have rejected this HIR before it reached pycc_mir",
                    );
                return MirExpr::AttrGet {
                    base: Box::new(self_expr),
                    slot,
                    ty,
                };
            }
            // #379 (PR-19): `Color.RED` — accessing an enum member by name
            // on the enum class. The base is `HirExpr::Name` referring to
            // an enum class (a class with non-empty `enum_members`), and
            // `attr` is a member name. Lower to `MirExpr::Name` reading a
            // synthetic global (`<Class>.<Member>.enum_member`) that
            // codegen initializes once at module-init time with the
            // member's singleton instance. The `.enum_member` suffix
            // ensures the name cannot collide with a real Python
            // identifier (which cannot contain `.`). The subsequent
            // `.value`/`.name` read on this result is a separate
            // `AttrGet` that resolves to a slot via the enum class's
            // `attrs = [("value", Int), ("name", Str)]` table.
            if let Some(enum_member_expr) =
                try_lower_enum_member_attr(base.as_ref(), attr.as_str(), classes)
            {
                return enum_member_expr;
            }
            let base = lower_expr(base, scopes, classes, current_class);
            let class_def = class_def_of(&base, classes);
            // #432: walk the MRO for property lookup first (matching
            // CPython's descriptor protocol precedence), then for regular
            // attribute slots using the flat MRO layout.
            for mro_class in &class_def.mro {
                let mro_def = mro_class_def(mro_class, classes);
                if let Some(prop) = mro_def.properties.iter().find(|p| p.name == *attr) {
                    let ty = lookup(scopes, &format!("$fn:{}", prop.getter));
                    return MirExpr::Call {
                        callee: prop.getter.clone(),
                        args: vec![base],
                        ty,
                    };
                }
            }
            let flat_attrs = mro_attrs(class_def, classes);
            let (slot, (_, ty)) = flat_attrs
                .iter()
                .enumerate()
                .find(|(_, (name, _))| name == attr)
                .unwrap_or_else(|| {
                    panic!(
                        "pycc_mir: internal error: attribute `{attr}` not declared on class `{}` \
                         or any base in its MRO -- pycc_types::check should have rejected this \
                         HIR before it reached pycc_mir",
                        class_def.name
                    )
                });
            MirExpr::AttrGet {
                base: Box::new(base),
                slot,
                ty: ty.clone(),
            }
        }
        // D-154 (Part 1 of #375): `base.method(args)` resolves to the
        // method's mangled, compile-time-known function symbol (D-006's
        // static-dispatch framing for a non-inherited class -- no vtable,
        // no runtime dispatch), then lowers into an ordinary `MirExpr::Call`
        // with `base` prepended as `self` -- unlike instantiation, a method
        // call never allocates, so it needs no dedicated MIR node of its
        // own (see `MirExpr::Instantiate`'s own doc comment for the
        // contrasting case).
        // #432: method resolution walks the MRO, so a method declared in a
        // base class is found when called on a derived class instance. A
        // subclass method shadows a base class method of the same name (the
        // subclass appears first in the MRO).
        HirExpr::MethodCall { base, method, args } => {
            // #433: `super().method(args)` — resolve the method starting
            // from the next class in the current class's MRO, using `self`
            // (the current function's first parameter) as the instance.
            // Lowers to a direct `MirExpr::Call` to the resolved method's
            // mangled name, with `self` prepended as the first argument —
            // no vtable, no runtime dispatch (D-006 static-dispatch framing,
            // per the #433 ADR).
            if matches!(base.as_ref(), HirExpr::Super) {
                let current = match current_class {
                    Some(c) => c,
                    None => panic!(
                        "pycc_mir: internal error: `HirExpr::Super` reached lower_expr outside a \
                         method body -- pycc_hir::lower_expr should have rejected this with C0001"
                    ),
                };
                let self_expr = self_expr(scopes);
                let class_def = &classes[current];
                let current_pos = class_def
                    .mro
                    .iter()
                    .position(|c| c == current)
                    .expect("pycc_mir: internal error: class not found in its own MRO");
                let super_mro = &class_def.mro[current_pos + 1..];
                let mangled = super_mro
                    .iter()
                    .find_map(|mro_class| {
                        let mro_def = &classes[mro_class.as_str()];
                        mro_def
                            .methods
                            .iter()
                            .find(|(name, _)| name == method)
                            .map(|(_, mangled)| mangled.clone())
                    })
                    .expect(
                        "pycc_mir: internal error: method not declared on class or any base in its \
                     MRO after the current class -- pycc_types::check should have rejected this \
                     HIR before it reached pycc_mir",
                    );
                let ty = lookup(scopes, &format!("$fn:{mangled}"));
                let mut call_args = Vec::with_capacity(args.len() + 1);
                call_args.push(self_expr);
                call_args.extend(
                    args.iter()
                        .map(|a| lower_expr(a, scopes, classes, current_class)),
                );
                return MirExpr::Call {
                    callee: mangled,
                    args: call_args,
                    ty,
                };
            }
            // #436: `ClassName.static_method(args)` or
            // `ClassName.class_method(args)` — a method call on a class
            // name (not an instance). The base is `HirExpr::Name` referring
            // to a registered class. `lower_expr` on a bare class name
            // would panic (class names are not in the scope), so intercept
            // here before lowering the base.
            if let HirExpr::Name(class_name) = base.as_ref()
                && classes.contains_key(class_name.as_str())
            {
                let class_def = &classes[class_name.as_str()];
                let static_mangled = class_def.mro.iter().find_map(|mro_class| {
                    let mro_def = mro_class_def(mro_class, classes);
                    mro_def
                        .static_methods
                        .iter()
                        .find(|(name, _)| name == method)
                        .map(|(_, mangled)| mangled.clone())
                });
                if let Some(mangled) = static_mangled {
                    let ty = lookup(scopes, &format!("$fn:{mangled}"));
                    let call_args: Vec<MirExpr> = args
                        .iter()
                        .map(|a| lower_expr(a, scopes, classes, current_class))
                        .collect();
                    return MirExpr::Call {
                        callee: mangled,
                        args: call_args,
                        ty,
                    };
                }
                let class_mangled = class_def.mro.iter().find_map(|mro_class| {
                    let mro_def = mro_class_def(mro_class, classes);
                    mro_def
                        .class_methods
                        .iter()
                        .find(|(name, _)| name == method)
                        .map(|(_, mangled)| mangled.clone())
                });
                if let Some(mangled) = class_mangled {
                    let ty = lookup(scopes, &format!("$fn:{mangled}"));
                    let mut call_args = Vec::with_capacity(args.len() + 1);
                    call_args.push(MirExpr::NullInstance {
                        ty: Ty::Instance(Box::new(class_name.clone())),
                    });
                    call_args.extend(
                        args.iter()
                            .map(|a| lower_expr(a, scopes, classes, current_class)),
                    );
                    return MirExpr::Call {
                        callee: mangled,
                        args: call_args,
                        ty,
                    };
                }
            }
            let base = lower_expr(base, scopes, classes, current_class);
            let class_def = class_def_of(&base, classes);
            // #436: check static_methods and class_methods before regular
            // method resolution. Static methods can be called on both
            // classes and instances; class methods can too. When called on
            // an instance, the instance is passed as `cls`/`self`.
            let static_mangled = class_def.mro.iter().find_map(|mro_class| {
                let mro_def = mro_class_def(mro_class, classes);
                mro_def
                    .static_methods
                    .iter()
                    .find(|(name, _)| name == method)
                    .map(|(_, mangled)| mangled.clone())
            });
            if let Some(mangled) = static_mangled {
                let ty = lookup(scopes, &format!("$fn:{mangled}"));
                let call_args: Vec<MirExpr> = args
                    .iter()
                    .map(|a| lower_expr(a, scopes, classes, current_class))
                    .collect();
                return MirExpr::Call {
                    callee: mangled,
                    args: call_args,
                    ty,
                };
            }
            let class_mangled = class_def.mro.iter().find_map(|mro_class| {
                let mro_def = mro_class_def(mro_class, classes);
                mro_def
                    .class_methods
                    .iter()
                    .find(|(name, _)| name == method)
                    .map(|(_, mangled)| mangled.clone())
            });
            if let Some(mangled) = class_mangled {
                let ty = lookup(scopes, &format!("$fn:{mangled}"));
                let mut call_args = Vec::with_capacity(args.len() + 1);
                call_args.push(base);
                call_args.extend(
                    args.iter()
                        .map(|a| lower_expr(a, scopes, classes, current_class)),
                );
                return MirExpr::Call {
                    callee: mangled,
                    args: call_args,
                    ty,
                };
            }
            // #432: walk the MRO to find the method's mangled name.
            let mangled = class_def
                .mro
                .iter()
                .find_map(|mro_class| {
                    let mro_def = mro_class_def(mro_class, classes);
                    mro_def
                        .methods
                        .iter()
                        .find(|(name, _)| name == method)
                        .map(|(_, mangled)| mangled.clone())
                })
                .unwrap_or_else(|| {
                    panic!(
                        "pycc_mir: internal error: method `{method}` not declared on class `{}` or \
                     any base in its MRO -- pycc_types::check should have rejected this HIR \
                     before it reached pycc_mir",
                        class_def.name
                    )
                });
            let ty = lookup(scopes, &format!("$fn:{mangled}"));
            let mut call_args = Vec::with_capacity(args.len() + 1);
            call_args.push(base);
            call_args.extend(
                args.iter()
                    .map(|a| lower_expr(a, scopes, classes, current_class)),
            );
            MirExpr::Call {
                callee: mangled,
                args: call_args,
                ty,
            }
        }
        // PEP 695 (#387): `GenericClassInstantiate` should never reach MIR
        // — `pycc_types::monomorphize` rewrites every
        // `GenericClassInstantiate` expression to an ordinary
        // `HirExpr::Call` to the mangled class name before MIR lowering
        // runs. If this arm is reached, it indicates a bug in the
        // monomorphization pass.
        HirExpr::GenericClassInstantiate { class, .. } => {
            panic!(
                "pycc_mir: internal error: `GenericClassInstantiate` for class `{class}` \
                 reached MIR lowering -- pycc_types::monomorphize should have rewritten it \
                 to an ordinary `HirExpr::Call` before this point"
            )
        }
        // #433: a bare `HirExpr::Super` should never reach MIR lowering —
        // HIR lowering rejects a standalone `super()` with C0001, and
        // `super().method()`/`super().attr` are handled by the special-case
        // blocks above before recursing into `lower_expr` for the base.
        HirExpr::Super => {
            panic!(
                "pycc_mir: internal error: a bare `HirExpr::Super` reached MIR lowering -- \
                 pycc_hir::lower_expr should have rejected this with C0001, or the \
                 `MethodCall`/`AttrGet` arms should have intercepted it before recursing"
            )
        }
    }
}

/// Resolves `expr`'s own `Ty::Instance` payload to its declared
/// `HirClassDef`, shared by `HirExpr::AttrGet`/`MethodCall`'s lowering above
/// and `HirStmt::AttrSet`'s below. Panics (never a `Result`, matching this
/// file's own established "pycc_types::check should have rejected this"
/// convention -- see `lookup`'s own doc comment) when `expr` isn't
/// instance-typed at all, or names a class this module's own `classes` table
/// doesn't have: both are impossible from a program `pycc_types::check` (or
/// `check_and_resolve`) already accepted, so only a hand-built `MirExpr`/
/// `HirModule` bypassing that validation (e.g. this file's own internal-error
/// tests) can reach either panic.
/// #378 (PR-18): If `expr` is a class-instance-typed expression whose
/// class has a `__repr__` method (found via the MRO), rewrites it to a
/// `MirExpr::Call` to that `__repr__` method, passing the original
/// expression as the `self` argument. The result type is `Ty::Str`. If
/// the expression is not an instance or the class has no `__repr__`,
/// returns the original expression unchanged.
///
/// This is used by `HirExpr::FString`'s interpolation lowering and
/// `HirExpr::Call`'s `print` argument lowering so the codegen's `to_str`
/// receives a `str` scalar (from the `__repr__` call's return value)
/// instead of an `Instance` scalar (which would panic in `to_str`).
fn rewrite_instance_to_repr(expr: &MirExpr, classes: &HashMap<String, HirClassDef>) -> MirExpr {
    let Ty::Instance(class_name) = expr.ty() else {
        return expr.clone();
    };
    let Some(class_def) = classes.get(class_name.as_str()) else {
        return expr.clone();
    };
    // #378 (PR-18): only rewrite for dataclass classes, whose
    // compiler-synthesized `__repr__` has a known-correct signature
    // `(self) -> str`. A user-defined `__repr__` on a non-dataclass class
    // may have a different arity or return type, which would cause a
    // codegen panic if rewritten to a call here. Non-dataclass instances
    // pass through unchanged (the type checker rejects `print(instance)` /
    // f-string interpolation of a non-dataclass instance with `T0021`
    // before codegen).
    if !class_def.is_dataclass {
        return expr.clone();
    }
    let repr_mangled = class_def.mro.iter().find_map(|mro_class| {
        // Every class in the MRO was registered when the class was lowered;
        // using `.expect` (whose panic path lives in libcore, outside this
        // crate's instrumented regions) avoids a `?` whose `None` branch is
        // structurally unreachable and would show up as a permanently
        // uncovered region under D-014's 100% coverage gate.
        let mro_def = classes
            .get(mro_class.as_str())
            .expect("MRO class must be registered");
        mro_def
            .methods
            .iter()
            .find(|(mn, _)| mn == "__repr__")
            .map(|(_, mangled)| mangled.clone())
    });
    // A dataclass always has a synthesized `__repr__` in its MRO (the
    // `is_dataclass` guard above ensures we only reach here for dataclass
    // classes). Using `.expect` (whose panic path lives in libcore,
    // outside this crate's instrumented regions) avoids a `match` whose
    // `None` arm is structurally unreachable for a dataclass and would
    // show up as a permanently uncovered region under D-014's 100%
    // coverage gate.
    let repr_mangled = repr_mangled.expect("dataclass must have __repr__");
    MirExpr::Call {
        callee: repr_mangled,
        args: vec![expr.clone()],
        ty: Ty::Str,
    }
}

fn class_def_of<'c>(expr: &MirExpr, classes: &'c HashMap<String, HirClassDef>) -> &'c HirClassDef {
    // #380 (PR-20): protocol-typed expressions resolve to the protocol's
    // own class def. This is used for method/attribute resolution on
    // protocol-typed variables that were not monomorphized (e.g. a
    // protocol-typed local variable inside a function body that doesn't
    // take a protocol parameter).
    let ty = expr.ty();
    let class_name = match &ty {
        Ty::Instance(name) => name.as_str(),
        Ty::Protocol(name) => name.as_str(),
        other => panic!(
            "pycc_mir: internal error: expected an instance- or protocol-typed expression, found `{}` -- pycc_types::check should have rejected this HIR before it reached pycc_mir",
            other.name()
        ),
    };
    classes.get(class_name).unwrap_or_else(|| {
        panic!(
            "pycc_mir: internal error: class `{class_name}` has no registered HirClassDef -- pycc_types::check should have rejected this HIR before it reached pycc_mir"
        )
    })
}

/// #433: Builds a `MirExpr::Name` for the current method's `self` parameter,
/// looked up from the innermost scope. Used by `super().method()` and
/// `super().attr` lowering to pass the most-derived instance as the implicit
/// first argument / attribute base. Panics if `self` is not bound in the
/// current scope (impossible for a method body that reached MIR lowering —
/// `pycc_hir` always includes `self` as the first parameter of a method).
fn self_expr(scopes: &[HashMap<String, Ty>]) -> MirExpr {
    let ty = lookup(scopes, "self");
    MirExpr::Name {
        name: "self".to_string(),
        ty,
    }
}

/// #432: Computes the flat attribute-slot layout for a class by walking its
/// MRO. Each class in the MRO (from most derived to most base) contributes
/// its own declared attributes that haven't already been seen by an earlier
/// (more derived) class. The result is a flat `(name, ty)` list whose indices
/// are the slot indices used at runtime -- the instance is allocated with
/// exactly this many slots (`mro_attr_count`), and every `AttrGet`/`AttrSet`
/// resolves its slot index against this flat layout, not the individual
/// class's own `attrs` list.
///
/// A derived class that re-declates an attribute of the same name as a base
/// class "wins" (its declaration appears first in the MRO, so its slot type
/// is the one used), matching CPython's own MRO-based attribute resolution.
fn mro_attrs(class_def: &HirClassDef, classes: &HashMap<String, HirClassDef>) -> Vec<(String, Ty)> {
    // #432: Walk the MRO most-base-first so that base class attributes
    // always occupy consistent low slot indices. This is critical for
    // inherited methods: when `Animal.speak` reads `self.name`, it
    // resolves the slot index from `Animal`'s `mro_attrs` (where `name`
    // is slot 0). If we walked most-derived-first, `Dog`'s `breed` would
    // get slot 0 and `name` would shift to slot 1 — but `Animal.speak`
    // would still read slot 0, getting `breed` instead of `name`.
    //
    // For re-declared attributes (a derived class re-declaring an attr
    // with the same name as a base), the most-derived declaration's type
    // wins — so we do a second pass over the MRO (most-derived-first) to
    // override types for attrs that were already assigned a slot.
    //
    // Collect the MRO defs once (verifying all classes exist) so both
    // passes share the same lookup and the panic path is only exercised
    // once.
    let mro_defs: Vec<&HirClassDef> = class_def
        .mro
        .iter()
        .map(|mro_class| mro_class_def(mro_class, classes))
        .collect();
    let mut result: Vec<(String, Ty)> = Vec::new();
    let mut slot_index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    // Pass 1: assign slots in most-base-first order (reverse MRO).
    for mro_def in mro_defs.iter().rev() {
        for (name, ty) in &mro_def.attrs {
            if !slot_index.contains_key(name) {
                slot_index.insert(name.clone(), result.len());
                result.push((name.clone(), ty.clone()));
            }
        }
    }
    // Pass 2: override types for re-declared attrs (most-derived wins).
    // Walk the MRO forward and, for each attr, take the type from the
    // first (most-derived) class that declares it. We track which attrs
    // have already been overridden to avoid a less-derived class
    // overwriting the most-derived type.
    let mut overridden: std::collections::HashSet<String> = std::collections::HashSet::new();
    for mro_def in &mro_defs {
        for (name, ty) in &mro_def.attrs {
            // `overridden.insert` returns true the first time we see this
            // attr in pass 2. Since pass 1 already assigned a slot for
            // every attr in the MRO, `slot_index[name]` always exists
            // here — direct indexing is safe.
            if overridden.insert(name.clone()) {
                let idx = slot_index[name];
                result[idx].1 = ty.clone();
            }
        }
    }
    result
}

/// #432: Returns the total number of attribute slots for a class, computed
/// from its MRO's flat attribute layout. Used by `Instantiate` to allocate
/// the correct number of slots.
fn mro_attr_count(class_def: &HirClassDef, classes: &HashMap<String, HirClassDef>) -> usize {
    mro_attrs(class_def, classes).len()
}

// ---------------------------------------------------------------------------
// Issue #435: compile-time `isinstance`/`issubclass` MIR lowering.
//
// Both builtins are evaluated at compile time (pycc's static dispatch model
// means every variable's runtime type is exactly its declared type), emitting
// `MirExpr::BoolLiteral(result)` constants. No runtime type tags or RTTI.
// ---------------------------------------------------------------------------

/// #435: Lowers `isinstance(obj, class_arg)` to a compile-time boolean
/// constant. The object expression is lowered to MIR (to extract its type
/// via `.ty()`), but the class argument is NOT lowered — it is a class
/// reference, not a value. The result is computed using
/// `eval_isinstance_single` with the object's class MRO.
fn lower_isinstance(
    args: &[HirExpr],
    scopes: &[HashMap<String, Ty>],
    classes: &HashMap<String, HirClassDef>,
    current_class: Option<&str>,
) -> MirExpr {
    // The type checker already validated arg count and class names. If we
    // reach here, args has exactly 2 elements and args[1] is a valid class
    // name or tuple of class names.
    let obj = lower_expr(&args[0], scopes, classes, current_class);
    let obj_ty = obj.ty();
    // Extract class names from the second argument.
    let class_names = extract_class_names(&args[1]).expect(
        "pycc_mir: internal error: isinstance's second argument was not validated by pycc_types",
    );
    // Compute the result: true if any target class matches.
    let obj_mro = match &obj_ty {
        Ty::Instance(class_name) => classes
            .get(class_name.as_str())
            .map(|cd| cd.mro.as_slice())
            .unwrap_or(&[]),
        _ => &[],
    };
    let result = class_names.iter().any(|target| {
        // #380 (PR-20, PEP 544): if the target is a protocol class, use
        // structural conformance checking instead of nominal MRO
        // membership. The type checker already validated that the
        // protocol is `@runtime_checkable`.
        if let Some(target_def) = classes.get(target.as_str())
            && target_def.is_protocol
        {
            return eval_isinstance_protocol(&obj_ty, target_def, classes);
        }
        eval_isinstance_single(&obj_ty, target, obj_mro)
    });
    MirExpr::BoolLiteral(result)
}

/// #380 (PR-20, PEP 544): Evaluates `isinstance(obj, Protocol)` at
/// compile time using structural conformance. Returns `true` if the
/// object's class has all the protocol's required members with
/// compatible types.
fn eval_isinstance_protocol(
    obj_ty: &Ty,
    proto_def: &HirClassDef,
    classes: &HashMap<String, HirClassDef>,
) -> bool {
    let Ty::Instance(class_name) = obj_ty else {
        return false;
    };
    let Some(class_def) = classes.get(class_name.as_str()) else {
        return false;
    };
    // Check each protocol member against the class's members (through
    // its MRO).
    use pycc_hir::ProtocolMember;
    for member in &proto_def.protocol_members {
        match member {
            ProtocolMember::Method {
                name: method_name,
                param_tys: proto_param_tys,
                return_ty: proto_return_ty,
            } => {
                // Look up the method through the MRO.
                let found = class_def.mro.iter().find_map(|mro_class| {
                    // An MRO entry may refer to a class not present in the
                    // `classes` map (e.g. a ghost base in unit tests).
                    // `filter_map` skips such entries, matching the
                    // `None`-is-not-found semantics of the attribute arm.
                    let mro_def = classes.get(mro_class.as_str())?;
                    let mangled = mro_def
                        .methods
                        .iter()
                        .find(|(n, _)| n == method_name)
                        .map(|(_, m)| m.as_str())?;
                    // The function signature is not available in MIR's
                    // `classes` table (it only has `HirClassDef`, not
                    // the `Environment`). For `@runtime_checkable`
                    // protocols, PEP 544 specifies that only the
                    // *presence* of attributes/methods is checked, not
                    // their types. This matches CPython's own
                    // `@runtime_checkable` behavior (it only checks
                    // for the presence of attributes, not their types).
                    Some(mangled)
                });
                if found.is_none() {
                    return false;
                }
                // For runtime_checkable, we only check presence, not
                // type compatibility (matching CPython's behavior).
                let _ = (proto_param_tys, proto_return_ty);
            }
            ProtocolMember::Attribute {
                name: attr_name, ..
            } => {
                // Look up the attribute through the MRO.
                let found = class_def.mro.iter().any(|mro_class| {
                    // An MRO entry may refer to a class not present in the
                    // `classes` map (e.g. a ghost base in unit tests).
                    // `is_some_and(…)` treats missing entries as not
                    // having the attribute, matching the method arm's
                    // `find_map` skip semantics.
                    classes
                        .get(mro_class.as_str())
                        .is_some_and(|mro_def| {
                            mro_def.attrs.iter().any(|(n, _)| n == attr_name)
                                || mro_def.properties.iter().any(|p| &p.name == attr_name)
                        })
                });
                if !found {
                    return false;
                }
            }
        }
    }
    true
}

/// #435: Lowers `issubclass(cls_arg, class_arg)` to a compile-time boolean
/// constant. Neither argument is lowered as a MIR expression — both are
/// class references. The result is computed using `eval_issubclass_single`
/// with the source class's MRO.
fn lower_issubclass(args: &[HirExpr], classes: &HashMap<String, HirClassDef>) -> MirExpr {
    // The type checker already validated arg count and class names. If we
    // reach here, args has exactly 2 elements, args[0] is a bare class name,
    // and args[1] is a valid class name or tuple of class names.
    let cls_name = match &args[0] {
        HirExpr::Name(name) => name.as_str(),
        _ => unreachable!(
            "pycc_mir: internal error: issubclass's first argument was not validated by pycc_types"
        ),
    };
    let target_names = extract_class_names(&args[1]).expect(
        "pycc_mir: internal error: issubclass's second argument was not validated by pycc_types",
    );
    // Get the source class's MRO (empty for builtin types).
    let cls_mro = if is_builtin_type_name(cls_name) {
        &[][..]
    } else {
        classes
            .get(cls_name)
            .map(|cd| cd.mro.as_slice())
            .unwrap_or(&[])
    };
    let result = target_names
        .iter()
        .any(|target| eval_issubclass_single(cls_name, target, cls_mro));
    MirExpr::BoolLiteral(result)
}

fn binop_result_ty(op: BinOpKind, left: Ty, right: Ty) -> Ty {
    if left == Ty::Str && right == Ty::Str && op == BinOpKind::Add {
        return Ty::Str;
    }
    // True division always produces `float`, even for two `int`/`bool`
    // operands -- this must match `pycc_types::numeric_result_type`'s own
    // rule (`(Some(_), Some(_)) if op == BinOpKind::Div => Ok(Ty::Float)`)
    // exactly, since `pycc_types` already accepted this program on that
    // promise; a mismatch here would make MIR's `ty` lie about what
    // codegen must produce (self-review correction: an earlier draft of
    // this function returned `Ty::Int` for `int / int`, which is simply
    // wrong -- `5 / 2` is `2.5`, not `2`).
    if op == BinOpKind::Div || left == Ty::Float || right == Ty::Float {
        return Ty::Float;
    }
    Ty::Int
}

#[cfg(test)]
mod tests {
    use super::*;
    use pycc_hir::{BinOpKind, CmpOpKind, FStringPart, HirClassDef, HirExpr, HirItem, HirMatchCase, HirModule, HirPattern, HirStmt, Ty};

    #[test]
    fn builds_an_assignment_and_a_later_name_reference() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(1),
                }),
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::Name("x".to_string())],
                })),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items,
            vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "x".to_string(),
                    value: MirExpr::IntLiteral(1),
                }),
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::Name {
                        name: "x".to_string(),
                        ty: Ty::Int
                    }],
                    ty: Ty::None,
                })),
            ]
        );
    }

    #[test]
    fn builds_a_function_with_typed_params_and_return() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "add".to_string(),
                params: vec![("a".to_string(), Ty::Int), ("b".to_string(), Ty::Int)],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(HirExpr::Name("a".to_string())),
                    right: Box::new(HirExpr::Name("b".to_string())),
                }))],
            }],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items,
            vec![MirItem::Function {
                name: "add".to_string(),
                params: vec![("a".to_string(), Ty::Int), ("b".to_string(), Ty::Int)],
                return_ty: Ty::Int,
                body: vec![MirStmt::Return(Some(MirExpr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(MirExpr::Name {
                        name: "a".to_string(),
                        ty: Ty::Int
                    }),
                    right: Box::new(MirExpr::Name {
                        name: "b".to_string(),
                        ty: Ty::Int
                    }),
                    ty: Ty::Int,
                }))],
            }]
        );
    }

    #[test]
    fn a_top_level_call_to_a_function_defined_later_resolves_via_two_pass_registration() {
        // Exercises `build`'s first pass directly: `helper`'s signature must
        // be registered before the top-level call to it is lowered, even
        // though `helper`'s `HirItem::Function` comes *after* the call in
        // `hir.items` -- exactly the forward-reference case D-038/D-039
        // already fixed on the `pycc_types::check` side.
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "helper".to_string(),
                    args: vec![],
                })),
                HirItem::Function {
                    name: "helper".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items[0],
            MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                callee: "helper".to_string(),
                args: vec![],
                ty: Ty::Int,
            }))
        );
    }

    #[test]
    fn a_function_can_call_itself_recursively_and_resolves_its_own_return_type() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "fact".to_string(),
                params: vec![("n".to_string(), Ty::Int)],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::Call {
                    callee: "fact".to_string(),
                    args: vec![HirExpr::Name("n".to_string())],
                }))],
            }],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items,
            vec![MirItem::Function {
                name: "fact".to_string(),
                params: vec![("n".to_string(), Ty::Int)],
                return_ty: Ty::Int,
                body: vec![MirStmt::Return(Some(MirExpr::Call {
                    callee: "fact".to_string(),
                    args: vec![MirExpr::Name {
                        name: "n".to_string(),
                        ty: Ty::Int
                    }],
                    ty: Ty::Int,
                }))],
            }]
        );
    }

    #[test]
    fn builds_an_if_statement_lowering_both_branches() {
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::If {
                test: HirExpr::BoolLiteral(true),
                body: vec![HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::IntLiteral(1)],
                })],
                orelse: vec![HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::IntLiteral(2)],
                })],
            })],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items,
            vec![MirItem::TopLevelStmt(MirStmt::If {
                test: MirExpr::BoolLiteral(true),
                body: vec![MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::IntLiteral(1)],
                    ty: Ty::None,
                })],
                orelse: vec![MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::IntLiteral(2)],
                    ty: Ty::None,
                })],
            })]
        );
    }

    #[test]
    fn builds_a_while_loop() {
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::While {
                test: HirExpr::BoolLiteral(true),
                body: vec![HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::IntLiteral(1)],
                })],
            })],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items,
            vec![MirItem::TopLevelStmt(MirStmt::While {
                test: MirExpr::BoolLiteral(true),
                body: vec![MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::IntLiteral(1)],
                    ty: Ty::None,
                })],
            })]
        );
    }

    #[test]
    fn builds_a_for_range_loop_binding_its_variable_as_int() {
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::ForRange {
                var: "i".to_string(),
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
                body: vec![HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::Name("i".to_string())],
                })],
            })],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items,
            vec![MirItem::TopLevelStmt(MirStmt::ForRange {
                var: "i".to_string(),
                start: MirExpr::IntLiteral(0),
                stop: MirExpr::IntLiteral(3),
                step: MirExpr::IntLiteral(1),
                body: vec![MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::Name {
                        name: "i".to_string(),
                        ty: Ty::Int
                    }],
                    ty: Ty::None,
                })],
            })]
        );
    }

    #[test]
    fn builds_a_return_with_no_value() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: Ty::None,
                body: vec![HirStmt::Return(None)],
            }],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items,
            vec![MirItem::Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: Ty::None,
                body: vec![MirStmt::Return(None)],
            }]
        );
    }

    #[test]
    fn an_annotated_assignment_whose_value_type_already_matches_the_annotation_lowers_unchanged() {
        // `x: int = 1` -- the initializer's own inferred type (`Ty::Int`)
        // already matches the annotation, so this is `lower_stmt`'s
        // "no widening needed" branch and `value` passes through
        // unchanged. This case cannot by itself distinguish binding the
        // annotation's type from binding the value's type (they're equal
        // here) -- the sibling test below, where they differ, is what
        // actually proves `lower_stmt` binds the annotation.
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::AnnAssign {
                    is_final: false,
                    target: "x".to_string(),
                    annotation: Ty::Int,
                    value: Some(HirExpr::IntLiteral(1)),
                }),
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Name("x".to_string()))),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items,
            vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "x".to_string(),
                    value: MirExpr::IntLiteral(1),
                }),
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Name {
                    name: "x".to_string(),
                    ty: Ty::Int,
                })),
            ]
        );
    }

    #[test]
    fn an_annotated_assignment_with_a_bool_value_under_an_int_annotation_widens_and_binds_int() {
        // `x: int = True` -- `pycc_types::is_assignable` accepts a `bool`
        // initializer under an `int` annotation as its one widening case,
        // and `pycc_types` itself binds its checker `env` to `Ty::Int`
        // (the annotation), not `Ty::Bool` (the initializer's own type) --
        // see its own comment citing this exact invariant. `lower_stmt`
        // must agree (D-074's "first assignment fixes a binding's
        // representation" rule): it wraps the lowered `BoolLiteral` in an
        // `IntBoundary` reporting `Ty::Int`, preserving D-141 runtime
        // identity without manufacturing arithmetic, and binds `x` to
        // `Ty::Int`, so a later
        // `Name` reference -- and any later plain reassignment -- agrees.
        // Before this fix, `lower_stmt` bound `Ty::Bool` here instead, and
        // the divergence from `pycc_types`' `Ty::Int` silently mis-sized
        // `x`'s eventual codegen slot (confirmed end to end:
        // `x: int = True; x = 5; return x` printed `11`, not `5`).
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::AnnAssign {
                    is_final: false,
                    target: "x".to_string(),
                    annotation: Ty::Int,
                    value: Some(HirExpr::BoolLiteral(true)),
                }),
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Name("x".to_string()))),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items,
            vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "x".to_string(),
                    value: MirExpr::IntBoundary(Box::new(MirExpr::BoolLiteral(true))),
                }),
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Name {
                    name: "x".to_string(),
                    ty: Ty::Int,
                })),
            ]
        );
    }

    #[test]
    fn an_annotated_assignment_with_a_bool_typed_compare_value_also_widens() {
        // The widening branch above is reachable for *any* `Ty::Bool`-typed
        // initializer under an `int` annotation, not merely a literal
        // `True`/`False` -- `pycc_types::is_assignable(Bool, Int)` accepts
        // a `Compare` result, a bool-typed name, or a bool-returning call
        // identically. This proves the same `IntBoundary` wrapping triggers
        // for a `Compare`-sourced `bool`, not only the literal
        // case the previous test exercises.
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::AnnAssign {
                is_final: false,
                target: "x".to_string(),
                annotation: Ty::Int,
                value: Some(HirExpr::Compare {
                    op: CmpOpKind::Lt,
                    left: Box::new(HirExpr::IntLiteral(1)),
                    right: Box::new(HirExpr::IntLiteral(2)),
                }),
            })],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items,
            vec![MirItem::TopLevelStmt(MirStmt::Assign {
                target: "x".to_string(),
                value: MirExpr::IntBoundary(Box::new(MirExpr::Compare {
                    op: CmpOpKind::Lt,
                    left: Box::new(MirExpr::IntLiteral(1)),
                    right: Box::new(MirExpr::IntLiteral(2)),
                    ty: Ty::Bool,
                })),
            })]
        );
    }

    #[test]
    fn a_value_less_annotated_assignment_lowers_to_a_no_op_and_binds_nothing() {
        // `y: int` alone has no runtime action -- CPython itself does
        // nothing observable for it either. `lower_stmt` must produce
        // `MirStmt::NoOp` and must NOT bind `y` in scope (matching
        // `pycc_types`' own Task 4 choice not to bind a value-less
        // declaration): a later read of `y` with no intervening assignment
        // still panics via `lookup`, proving no phantom binding leaked
        // through.
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::AnnAssign {
                is_final: false,
                target: "y".to_string(),
                annotation: Ty::Int,
                value: None,
            })],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(mir.items, vec![MirItem::TopLevelStmt(MirStmt::NoOp)]);
    }

    #[test]
    #[should_panic(expected = "has no recorded type")]
    fn a_value_less_annotated_assignment_does_not_bind_the_name() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::AnnAssign {
                    is_final: false,
                    target: "y".to_string(),
                    annotation: Ty::Int,
                    value: None,
                }),
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Name("y".to_string()))),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        build(&hir);
    }

    #[test]
    fn builds_a_compare_expression_with_bool_type() {
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::Compare {
                    op: CmpOpKind::Lt,
                    left: Box::new(HirExpr::IntLiteral(1)),
                    right: Box::new(HirExpr::IntLiteral(2)),
                },
            })],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items,
            vec![MirItem::TopLevelStmt(MirStmt::Assign {
                target: "x".to_string(),
                value: MirExpr::Compare {
                    op: CmpOpKind::Lt,
                    left: Box::new(MirExpr::IntLiteral(1)),
                    right: Box::new(MirExpr::IntLiteral(2)),
                    ty: Ty::Bool,
                },
            })]
        );
    }

    #[test]
    fn builds_an_f_string_with_a_literal_and_an_interpolation() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(1),
                }),
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "y".to_string(),
                    value: HirExpr::FString(vec![
                        FStringPart::Literal("n=".to_string()),
                        FStringPart::Interpolation(Box::new(HirExpr::Name("x".to_string()))),
                    ]),
                }),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items[1],
            MirItem::TopLevelStmt(MirStmt::Assign {
                target: "y".to_string(),
                value: MirExpr::FString(vec![
                    MirFStringPart::Literal("n=".to_string()),
                    MirFStringPart::Interpolation(Box::new(MirExpr::Name {
                        name: "x".to_string(),
                        ty: Ty::Int
                    })),
                ]),
            })
        );
    }

    #[test]
    fn string_concatenation_infers_str() {
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(HirExpr::StringLiteral("a".to_string())),
                    right: Box::new(HirExpr::StringLiteral("b".to_string())),
                },
            })],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items,
            vec![MirItem::TopLevelStmt(MirStmt::Assign {
                target: "x".to_string(),
                value: MirExpr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(MirExpr::StringLiteral("a".to_string())),
                    right: Box::new(MirExpr::StringLiteral("b".to_string())),
                    ty: Ty::Str,
                },
            })]
        );
    }

    #[test]
    fn true_division_of_two_ints_infers_float() {
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::BinOp {
                    op: BinOpKind::Div,
                    left: Box::new(HirExpr::IntLiteral(5)),
                    right: Box::new(HirExpr::IntLiteral(2)),
                },
            })],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items,
            vec![MirItem::TopLevelStmt(MirStmt::Assign {
                target: "x".to_string(),
                value: MirExpr::BinOp {
                    op: BinOpKind::Div,
                    left: Box::new(MirExpr::IntLiteral(5)),
                    right: Box::new(MirExpr::IntLiteral(2)),
                    ty: Ty::Float,
                },
            })]
        );
    }

    #[test]
    fn adding_a_float_left_operand_and_an_int_infers_float() {
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(HirExpr::FloatLiteral(1.5)),
                    right: Box::new(HirExpr::IntLiteral(2)),
                },
            })],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items,
            vec![MirItem::TopLevelStmt(MirStmt::Assign {
                target: "x".to_string(),
                value: MirExpr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(MirExpr::FloatLiteral(1.5)),
                    right: Box::new(MirExpr::IntLiteral(2)),
                    ty: Ty::Float,
                },
            })]
        );
    }

    #[test]
    fn adding_an_int_and_a_float_right_operand_infers_float() {
        // Distinct region from the left-operand case above: exercises
        // `right == Ty::Float` specifically (`left == Ty::Float` is false
        // here), not just `binop_result_ty`'s overall `Float` outcome.
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(HirExpr::IntLiteral(2)),
                    right: Box::new(HirExpr::FloatLiteral(1.5)),
                },
            })],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items,
            vec![MirItem::TopLevelStmt(MirStmt::Assign {
                target: "x".to_string(),
                value: MirExpr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(MirExpr::IntLiteral(2)),
                    right: Box::new(MirExpr::FloatLiteral(1.5)),
                    ty: Ty::Float,
                },
            })]
        );
    }

    #[test]
    fn a_function_resolves_a_module_global_assigned_after_its_definition() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "read_x".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
                },
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(5),
                }),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items[0],
            MirItem::Function {
                name: "read_x".to_string(),
                params: vec![],
                return_ty: Ty::Int,
                body: vec![MirStmt::Return(Some(MirExpr::Name {
                    name: "x".to_string(),
                    ty: Ty::Int,
                }))],
            }
        );
    }

    #[test]
    fn assigning_bool_to_an_existing_int_binding_preserves_its_mir_type() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(1),
                }),
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::BoolLiteral(true),
                }),
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Name("x".to_string()))),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items[2],
            MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Name {
                name: "x".to_string(),
                ty: Ty::Int,
            }))
        );
    }

    #[test]
    #[should_panic(expected = "has no recorded type")]
    fn a_top_level_read_still_cannot_resolve_a_later_assignment() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Name("x".to_string()))),
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(1),
                }),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        build(&hir);
    }

    #[test]
    #[should_panic(expected = "has no recorded type")]
    fn referencing_an_unbound_name_panics_with_an_internal_error() {
        // By construction (see this module's doc comment / D-057 discussion
        // in the task brief), every `Ty` reaching `pycc_mir` is already
        // concrete and every name already resolved by `pycc_types::check`
        // -- this HIR could never come from a real `check_and_resolve`
        // success, but the panic path itself still needs direct coverage.
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Name(
                "undefined".to_string(),
            )))],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        build(&hir);
    }

    #[test]
    fn a_function_local_shadowing_a_module_global_of_a_different_type_resolves_its_own_type() {
        // D-055: a function that assigns a name anywhere in its body
        // classifies that name as local for the *entire* body, independent
        // of any same-named module global -- Python scoping, not a
        // control-flow-sensitive fact. `x` is a module-level `str` here;
        // `f`'s own `x = 5; return x` must resolve to `f`'s own fresh
        // `Ty::Int`, never falling through to the module global's `Ty::Str`.
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::StringLiteral("hello".to_string()),
                }),
                HirItem::Function {
                    name: "f".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![
                        HirStmt::Assign {
                            target: "x".to_string(),
                            value: HirExpr::IntLiteral(5),
                        },
                        HirStmt::Return(Some(HirExpr::Name("x".to_string()))),
                    ],
                },
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items[1],
            MirItem::Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: Ty::Int,
                body: vec![
                    MirStmt::Assign {
                        target: "x".to_string(),
                        value: MirExpr::IntLiteral(5)
                    },
                    MirStmt::Return(Some(MirExpr::Name {
                        name: "x".to_string(),
                        ty: Ty::Int
                    })),
                ],
            }
        );
    }

    #[test]
    fn a_sibling_function_after_a_shadowing_function_still_reads_the_unshadowed_global() {
        // `lower_item` pushes and later pops an isolated function scope;
        // lowering one function's shadowing assignment must not mutate the
        // module scope seen by a later sibling function.
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::StringLiteral("hello".to_string()),
                }),
                HirItem::Function {
                    name: "shadows".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![
                        HirStmt::Assign {
                            target: "x".to_string(),
                            value: HirExpr::IntLiteral(5),
                        },
                        HirStmt::Return(Some(HirExpr::Name("x".to_string()))),
                    ],
                },
                HirItem::Function {
                    name: "reads_global".to_string(),
                    params: vec![],
                    return_ty: Ty::Str,
                    body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
                },
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items[2],
            MirItem::Function {
                name: "reads_global".to_string(),
                params: vec![],
                return_ty: Ty::Str,
                body: vec![MirStmt::Return(Some(MirExpr::Name {
                    name: "x".to_string(),
                    ty: Ty::Str
                }))],
            }
        );
    }

    #[test]
    fn a_function_parameter_shadowing_a_module_global_resolves_its_own_type() {
        // Parameters are part of D-055's lexical-local list too: a
        // parameter named the same as a module global must resolve to the
        // parameter's own type, never fall through to the global's.
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::StringLiteral("hello".to_string()),
                }),
                HirItem::Function {
                    name: "f".to_string(),
                    params: vec![("x".to_string(), Ty::Int)],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
                },
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items[1],
            MirItem::Function {
                name: "f".to_string(),
                params: vec![("x".to_string(), Ty::Int)],
                return_ty: Ty::Int,
                body: vec![MirStmt::Return(Some(MirExpr::Name {
                    name: "x".to_string(),
                    ty: Ty::Int
                }))],
            }
        );
    }

    #[test]
    fn a_for_range_variable_shadowing_a_module_global_resolves_its_own_type() {
        // `ForRange`'s loop variable is also part of D-055's lexical-local
        // list (it's a binding form, matching Python's own `for`-target
        // classification), so it must shadow a same-named module global too.
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "i".to_string(),
                    value: HirExpr::StringLiteral("hello".to_string()),
                }),
                HirItem::Function {
                    name: "f".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![
                        HirStmt::ForRange {
                            var: "i".to_string(),
                            start: HirExpr::IntLiteral(0),
                            stop: HirExpr::IntLiteral(3),
                            step: HirExpr::IntLiteral(1),
                            body: vec![],
                        },
                        HirStmt::Return(Some(HirExpr::Name("i".to_string()))),
                    ],
                },
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items[1],
            MirItem::Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: Ty::Int,
                body: vec![
                    MirStmt::ForRange {
                        var: "i".to_string(),
                        start: MirExpr::IntLiteral(0),
                        stop: MirExpr::IntLiteral(3),
                        step: MirExpr::IntLiteral(1),
                        body: vec![],
                    },
                    MirStmt::Return(Some(MirExpr::Name {
                        name: "i".to_string(),
                        ty: Ty::Int
                    })),
                ],
            }
        );
    }

    #[test]
    fn a_local_first_assigned_inside_nested_if_and_else_bodies_shadows_a_module_global() {
        // Exercises `lower_stmt` recursing into both `body` and `orelse` --
        // D-055 classifies a name as
        // function-local even when its only assignment is nested inside a
        // conditional, not just when it appears directly in the body.
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::StringLiteral("hello".to_string()),
                }),
                HirItem::Function {
                    name: "f".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![
                        HirStmt::If {
                            test: HirExpr::BoolLiteral(true),
                            body: vec![HirStmt::Assign {
                                target: "x".to_string(),
                                value: HirExpr::IntLiteral(1),
                            }],
                            orelse: vec![HirStmt::Assign {
                                target: "x".to_string(),
                                value: HirExpr::IntLiteral(2),
                            }],
                        },
                        HirStmt::Return(Some(HirExpr::Name("x".to_string()))),
                    ],
                },
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items[1],
            MirItem::Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: Ty::Int,
                body: vec![
                    MirStmt::If {
                        test: MirExpr::BoolLiteral(true),
                        body: vec![MirStmt::Assign {
                            target: "x".to_string(),
                            value: MirExpr::IntLiteral(1)
                        }],
                        orelse: vec![MirStmt::Assign {
                            target: "x".to_string(),
                            value: MirExpr::IntLiteral(2)
                        }],
                    },
                    MirStmt::Return(Some(MirExpr::Name {
                        name: "x".to_string(),
                        ty: Ty::Int
                    })),
                ],
            }
        );
    }

    #[test]
    fn a_local_first_assigned_inside_a_while_body_shadows_a_module_global() {
        // Exercises `lower_stmt` recursing into a `While` body.
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::StringLiteral("hello".to_string()),
                }),
                HirItem::Function {
                    name: "f".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![
                        HirStmt::While {
                            test: HirExpr::BoolLiteral(false),
                            body: vec![HirStmt::Assign {
                                target: "x".to_string(),
                                value: HirExpr::IntLiteral(1),
                            }],
                        },
                        HirStmt::Return(Some(HirExpr::Name("x".to_string()))),
                    ],
                },
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items[1],
            MirItem::Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: Ty::Int,
                body: vec![
                    MirStmt::While {
                        test: MirExpr::BoolLiteral(false),
                        body: vec![MirStmt::Assign {
                            target: "x".to_string(),
                            value: MirExpr::IntLiteral(1)
                        }],
                    },
                    MirStmt::Return(Some(MirExpr::Name {
                        name: "x".to_string(),
                        ty: Ty::Int
                    })),
                ],
            }
        );
    }

    #[test]
    fn a_local_first_assigned_inside_a_for_range_body_shadows_a_module_global() {
        // Exercises `lower_stmt` recursing into a `ForRange` body (distinct from the loop variable
        // itself, already covered by
        // `a_for_range_variable_shadowing_a_module_global_resolves_its_own_type`).
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::StringLiteral("hello".to_string()),
                }),
                HirItem::Function {
                    name: "f".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![
                        HirStmt::ForRange {
                            var: "loop_i".to_string(),
                            start: HirExpr::IntLiteral(0),
                            stop: HirExpr::IntLiteral(3),
                            step: HirExpr::IntLiteral(1),
                            body: vec![HirStmt::Assign {
                                target: "x".to_string(),
                                value: HirExpr::IntLiteral(1),
                            }],
                        },
                        HirStmt::Return(Some(HirExpr::Name("x".to_string()))),
                    ],
                },
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items[1],
            MirItem::Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: Ty::Int,
                body: vec![
                    MirStmt::ForRange {
                        var: "loop_i".to_string(),
                        start: MirExpr::IntLiteral(0),
                        stop: MirExpr::IntLiteral(3),
                        step: MirExpr::IntLiteral(1),
                        body: vec![MirStmt::Assign {
                            target: "x".to_string(),
                            value: MirExpr::IntLiteral(1)
                        }],
                    },
                    MirStmt::Return(Some(MirExpr::Name {
                        name: "x".to_string(),
                        ty: Ty::Int
                    })),
                ],
            }
        );
    }

    #[test]
    fn mir_expr_ty_covers_every_variant() {
        assert_eq!(MirExpr::IntLiteral(1).ty(), Ty::Int);
        assert_eq!(MirExpr::FloatLiteral(1.0).ty(), Ty::Float);
        assert_eq!(MirExpr::BoolLiteral(true).ty(), Ty::Bool);
        assert_eq!(MirExpr::StringLiteral("s".to_string()).ty(), Ty::Str);
        assert_eq!(MirExpr::FString(vec![]).ty(), Ty::Str);
        assert_eq!(
            MirExpr::Name {
                name: "x".to_string(),
                ty: Ty::Int
            }
            .ty(),
            Ty::Int
        );
        assert_eq!(
            MirExpr::Call {
                callee: "f".to_string(),
                args: vec![],
                ty: Ty::Bool
            }
            .ty(),
            Ty::Bool
        );
        assert_eq!(
            MirExpr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(MirExpr::IntLiteral(1)),
                right: Box::new(MirExpr::IntLiteral(2)),
                ty: Ty::Int,
            }
            .ty(),
            Ty::Int
        );
        assert_eq!(
            MirExpr::Compare {
                op: CmpOpKind::Eq,
                left: Box::new(MirExpr::IntLiteral(1)),
                right: Box::new(MirExpr::IntLiteral(2)),
                ty: Ty::Bool,
            }
            .ty(),
            Ty::Bool
        );
        assert_eq!(
            MirExpr::ListLiteral(vec![MirExpr::IntLiteral(1)]).ty(),
            Ty::List(Box::new(Ty::Int))
        );
        assert_eq!(
            MirExpr::Subscript {
                base: Box::new(MirExpr::Name {
                    name: "x".to_string(),
                    ty: Ty::List(Box::new(Ty::Int)),
                }),
                index: Box::new(MirExpr::IntLiteral(0)),
            }
            .ty(),
            Ty::Int
        );
        assert_eq!(
            MirExpr::ListAppend {
                list: "x".to_string(),
                value: Box::new(MirExpr::IntLiteral(1)),
            }
            .ty(),
            Ty::None
        );
        assert_eq!(
            MirExpr::DictLiteral(vec![(
                MirExpr::StringLiteral("a".to_string()),
                MirExpr::IntLiteral(1)
            )])
            .ty(),
            Ty::Dict(Box::new((Ty::Str, Ty::Int)))
        );
        assert_eq!(
            MirExpr::DictGet {
                dict: Box::new(MirExpr::Name {
                    name: "x".to_string(),
                    ty: Ty::Dict(Box::new((Ty::Str, Ty::Int))),
                }),
                key: Box::new(MirExpr::StringLiteral("a".to_string())),
            }
            .ty(),
            Ty::Int
        );
        assert_eq!(
            MirExpr::SetLiteral(vec![MirExpr::IntLiteral(1), MirExpr::IntLiteral(2)]).ty(),
            Ty::Set(Box::new(Ty::Int))
        );
        assert_eq!(
            MirExpr::TupleLiteral(vec![MirExpr::IntLiteral(1), MirExpr::BoolLiteral(true)]).ty(),
            Ty::Tuple(Box::new(vec![Ty::Int, Ty::Bool]))
        );
        assert_eq!(
            MirExpr::Slice {
                base: Box::new(MirExpr::Name {
                    name: "x".to_string(),
                    ty: Ty::List(Box::new(Ty::Int)),
                }),
                start: Some(Box::new(MirExpr::IntLiteral(1))),
                stop: None,
                step: None,
            }
            .ty(),
            Ty::List(Box::new(Ty::Int))
        );
        assert_eq!(
            MirExpr::ListPop {
                list: "x".to_string(),
                ty: Ty::Int,
            }
            .ty(),
            Ty::Int
        );
        assert_eq!(
            MirExpr::DictGetOrDefault {
                dict: "d".to_string(),
                key: Box::new(MirExpr::StringLiteral("a".to_string())),
                default: Box::new(MirExpr::IntLiteral(0)),
                ty: Ty::Int,
            }
            .ty(),
            Ty::Int
        );
        assert_eq!(
            MirExpr::SetAdd {
                set: "s".to_string(),
                value: Box::new(MirExpr::IntLiteral(1)),
            }
            .ty(),
            Ty::None
        );
        assert_eq!(
            MirExpr::Instantiate(Box::new(InstantiateExpr {
                ctor: "Point.__init__".to_string(),
                attr_count: 2,
                args: vec![],
                ty: Ty::Instance(Box::new("Point".to_string())),
            }))
            .ty(),
            Ty::Instance(Box::new("Point".to_string()))
        );
        assert_eq!(
            MirExpr::AttrGet {
                base: Box::new(MirExpr::Name {
                    name: "p".to_string(),
                    ty: Ty::Instance(Box::new("Point".to_string())),
                }),
                slot: 0,
                ty: Ty::Int,
            }
            .ty(),
            Ty::Int
        );
    }

    #[test]
    fn lowers_list_literal_to_mir() {
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)]),
            })],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        // Not a `let PATTERN = ... else { panic!(...) }` destructure -- this
        // file's own coverage-gate convention (see `pycc_hir`'s equivalent
        // `ListLiteral` test, commit 48f13e6) is that a hand-written panic
        // arm is never taken on the happy path and shows up as a
        // permanently uncovered region under D-014's 100%-regions gate. A
        // direct `assert_eq!` against the whole expected `MirItem` avoids
        // that without weakening the assertion.
        assert_eq!(
            mir.items[0],
            MirItem::TopLevelStmt(MirStmt::Assign {
                target: "x".to_string(),
                value: MirExpr::ListLiteral(vec![MirExpr::IntLiteral(1), MirExpr::IntLiteral(2)]),
            })
        );
    }

    #[test]
    fn lowers_for_list_to_mir_for_list() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::ListLiteral(vec![
                        HirExpr::IntLiteral(1),
                        HirExpr::IntLiteral(2),
                    ]),
                }),
                HirItem::TopLevelStmt(HirStmt::ForList {
                    var: "v".to_string(),
                    list: "x".to_string(),
                    body: vec![HirStmt::ExprStmt(HirExpr::Call {
                        callee: "print".to_string(),
                        args: vec![HirExpr::Name("v".to_string())],
                    })],
                }),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items[1],
            MirItem::TopLevelStmt(MirStmt::ForList {
                var: "v".to_string(),
                list: "x".to_string(),
                body: vec![MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::Name {
                        name: "v".to_string(),
                        ty: Ty::Int,
                    }],
                    ty: Ty::None,
                })],
            })
        );
    }

    #[test]
    fn lowers_subscript_to_mir_recursively() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::ListLiteral(vec![
                        HirExpr::IntLiteral(1),
                        HirExpr::IntLiteral(2),
                    ]),
                }),
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "y".to_string(),
                    value: HirExpr::Subscript {
                        base: Box::new(HirExpr::Name("x".to_string())),
                        index: Box::new(HirExpr::IntLiteral(0)),
                    },
                }),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items[1],
            MirItem::TopLevelStmt(MirStmt::Assign {
                target: "y".to_string(),
                value: MirExpr::Subscript {
                    base: Box::new(MirExpr::Name {
                        name: "x".to_string(),
                        ty: Ty::List(Box::new(Ty::Int)),
                    }),
                    index: Box::new(MirExpr::IntLiteral(0)),
                },
            })
        );
    }

    #[test]
    fn lowers_list_append_to_mir_recursively() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1)]),
                }),
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::ListAppend {
                    list: "x".to_string(),
                    value: Box::new(HirExpr::IntLiteral(2)),
                })),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items[1],
            MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::ListAppend {
                list: "x".to_string(),
                value: Box::new(MirExpr::IntLiteral(2)),
            }))
        );
    }

    #[test]
    fn lowers_list_pop_to_mir_deriving_its_element_type_from_the_list_binding() {
        // PR-12 Task 11 (D-119): `xs.pop()`'s `ty` is derived from `xs`'s
        // own `Ty::List` binding via `lookup`, mirroring
        // `HirExpr::Subscript`'s own base-type lookup.
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "xs".to_string(),
                    value: HirExpr::ListLiteral(vec![
                        HirExpr::IntLiteral(1),
                        HirExpr::IntLiteral(2),
                        HirExpr::IntLiteral(3),
                    ]),
                }),
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "y".to_string(),
                    value: HirExpr::ListPop {
                        list: "xs".to_string(),
                    },
                }),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items[1],
            MirItem::TopLevelStmt(MirStmt::Assign {
                target: "y".to_string(),
                value: MirExpr::ListPop {
                    list: "xs".to_string(),
                    ty: Ty::Int,
                },
            })
        );
    }

    #[test]
    #[should_panic(expected = "`xs` is not list-typed")]
    fn list_pop_over_a_non_list_binding_panics_with_an_internal_error() {
        // `pycc_types` already rejects `.pop()` on a non-list base (T0033)
        // before HIR reaches `pycc_mir`, but the defensive panic path in
        // `lower_expr`'s own `HirExpr::ListPop` arm still needs direct
        // coverage, mirroring `a_for_list_loop_over_a_non_list_non_dict_non_set_binding_panics_with_an_internal_error`
        // above.
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "xs".to_string(),
                    value: HirExpr::IntLiteral(5),
                }),
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::ListPop {
                    list: "xs".to_string(),
                })),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        build(&hir);
    }

    #[test]
    fn lowers_dict_get_or_default_to_mir_recursively_deriving_its_value_type() {
        // PR-12 Task 11 (D-119): `d.get(key, default)`'s `ty` is derived
        // from `d`'s own `Ty::Dict` binding's value type, and both `key`
        // and `default` are recursively lowered.
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "d".to_string(),
                    value: HirExpr::DictLiteral(vec![(
                        HirExpr::StringLiteral("a".to_string()),
                        HirExpr::IntLiteral(1),
                    )]),
                }),
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "y".to_string(),
                    value: HirExpr::DictGetOrDefault {
                        dict: "d".to_string(),
                        key: Box::new(HirExpr::StringLiteral("z".to_string())),
                        default: Box::new(HirExpr::IntLiteral(-1)),
                    },
                }),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items[1],
            MirItem::TopLevelStmt(MirStmt::Assign {
                target: "y".to_string(),
                value: MirExpr::DictGetOrDefault {
                    dict: "d".to_string(),
                    key: Box::new(MirExpr::StringLiteral("z".to_string())),
                    default: Box::new(MirExpr::IntLiteral(-1)),
                    ty: Ty::Int,
                },
            })
        );
    }

    #[test]
    #[should_panic(expected = "`d` is not dict-typed")]
    fn dict_get_or_default_over_a_non_dict_binding_panics_with_an_internal_error() {
        // Same reasoning as `list_pop_over_a_non_list_binding_panics_with_an_internal_error`
        // above, for `HirExpr::DictGetOrDefault`'s own defensive panic path.
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "d".to_string(),
                    value: HirExpr::IntLiteral(5),
                }),
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::DictGetOrDefault {
                    dict: "d".to_string(),
                    key: Box::new(HirExpr::StringLiteral("a".to_string())),
                    default: Box::new(HirExpr::IntLiteral(0)),
                })),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        build(&hir);
    }

    #[test]
    fn lowers_set_add_to_mir_recursively() {
        // PR-12 Task 11 (D-119): `s.add(value)` mirrors `ListAppend`'s own
        // shape exactly -- `set` is carried as a plain name, `value` is
        // recursively lowered.
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "s".to_string(),
                    value: HirExpr::SetLiteral(vec![HirExpr::IntLiteral(1)]),
                }),
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::SetAdd {
                    set: "s".to_string(),
                    value: Box::new(HirExpr::IntLiteral(2)),
                })),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items[1],
            MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::SetAdd {
                set: "s".to_string(),
                value: Box::new(MirExpr::IntLiteral(2)),
            }))
        );
    }

    #[test]
    fn lowers_math_sqrt_call_to_mir_with_float_type_without_panicking() {
        // D-136: without the dedicated `callee == "math.sqrt"` branch, this
        // would panic via `lookup`'s own "has no recorded type" message,
        // exactly like `len` above -- there is no `$fn:math.sqrt` signature
        // to find, even though `pycc_types` already accepts `math.sqrt(x)`
        // as valid, `Ty::Float`-typed.
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::Assign {
                target: "n".to_string(),
                value: HirExpr::Call {
                    callee: "math.sqrt".to_string(),
                    args: vec![HirExpr::FloatLiteral(2.0)],
                },
            })],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items[0],
            MirItem::TopLevelStmt(MirStmt::Assign {
                target: "n".to_string(),
                value: MirExpr::Call {
                    callee: "math.sqrt".to_string(),
                    args: vec![MirExpr::FloatLiteral(2.0)],
                    ty: Ty::Float,
                },
            })
        );
    }

    #[test]
    fn lowers_math_pi_name_to_mir_with_float_type_without_panicking() {
        // D-136: without the dedicated `name == "math.pi"` arm, this would
        // panic via `lookup`'s own "has no recorded type" message -- `pi`
        // is never bound in `scopes` the way an ordinary assigned variable
        // is.
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::Assign {
                target: "n".to_string(),
                value: HirExpr::Name("math.pi".to_string()),
            })],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items[0],
            MirItem::TopLevelStmt(MirStmt::Assign {
                target: "n".to_string(),
                value: MirExpr::Name {
                    name: "math.pi".to_string(),
                    ty: Ty::Float,
                },
            })
        );
    }

    #[test]
    fn lowers_len_call_to_mir_with_int_type_without_panicking() {
        // Required fix (beyond the brief): without a parallel `"len"` branch
        // in the `HirExpr::Call` lowering arm, this would panic via
        // `lookup`'s own "has no recorded type" message, since no `$fn:len`
        // signature is ever registered -- even though `pycc_types` already
        // accepts `len(lst)` as valid, `Ty::Int`-typed (D-105 point 3).
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::ListLiteral(vec![
                        HirExpr::IntLiteral(1),
                        HirExpr::IntLiteral(2),
                    ]),
                }),
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "n".to_string(),
                    value: HirExpr::Call {
                        callee: "len".to_string(),
                        args: vec![HirExpr::Name("x".to_string())],
                    },
                }),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items[1],
            MirItem::TopLevelStmt(MirStmt::Assign {
                target: "n".to_string(),
                value: MirExpr::Call {
                    callee: "len".to_string(),
                    args: vec![MirExpr::Name {
                        name: "x".to_string(),
                        ty: Ty::List(Box::new(Ty::Int)),
                    }],
                    ty: Ty::Int,
                },
            })
        );
    }

    #[test]
    fn lowers_float_call_to_mir_with_float_type_without_panicking() {
        // Mirrors `lowers_len_call_to_mir_with_int_type_without_panicking`
        // immediately above, for the same reason (#181): without a parallel
        // `"float"` branch in the `HirExpr::Call` lowering arm, this would
        // panic via `lookup`'s own "has no recorded type" message, since no
        // `$fn:float` signature is ever registered -- even though
        // `pycc_types` already accepts `float(x)` as valid, `Ty::Float`-typed.
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(3),
                }),
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "y".to_string(),
                    value: HirExpr::Call {
                        callee: "float".to_string(),
                        args: vec![HirExpr::Name("x".to_string())],
                    },
                }),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items[1],
            MirItem::TopLevelStmt(MirStmt::Assign {
                target: "y".to_string(),
                value: MirExpr::Call {
                    callee: "float".to_string(),
                    args: vec![MirExpr::Name {
                        name: "x".to_string(),
                        ty: Ty::Int,
                    }],
                    ty: Ty::Float,
                },
            })
        );
    }

    #[test]
    fn a_user_defined_float_function_is_lowered_as_a_real_call_not_the_builtin() {
        // Post-merge review finding: unlike `len`/`print`, `float` was
        // undefined until #181, so a program defining its own `float` was
        // valid on `main` immediately before this builtin landed --
        // reproduced directly, printing `6` on a pristine checkout. Without
        // this priority check, the builtin's hardcoded `Ty::Float` would
        // silently override the user function's own registered `Ty::Int`
        // return type.
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "float".to_string(),
                    params: vec![("x".to_string(), Ty::Int)],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                        op: BinOpKind::Add,
                        left: Box::new(HirExpr::Name("x".to_string())),
                        right: Box::new(HirExpr::IntLiteral(1)),
                    }))],
                },
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "y".to_string(),
                    value: HirExpr::Call {
                        callee: "float".to_string(),
                        args: vec![HirExpr::IntLiteral(5)],
                    },
                }),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items[1],
            MirItem::TopLevelStmt(MirStmt::Assign {
                target: "y".to_string(),
                value: MirExpr::Call {
                    callee: "float".to_string(),
                    args: vec![MirExpr::IntLiteral(5)],
                    ty: Ty::Int,
                },
            })
        );
    }

    #[test]
    fn list_literal_subscript_and_for_list_derive_their_type_from_actual_elements_not_hardcoded_int()
     {
        // Mirrors `pycc_types`'s own genericity tests for `ListLiteral`,
        // `Subscript`, and `ForList` (see e.g. its
        // `a_for_list_loop_binds_its_variable_as_str_for_a_list_of_str`):
        // this lowering must derive `ty()`/the loop variable's bound type
        // from the list's *actual* element type, not assume `Ty::Int`.
        // `pycc_types`'s T0034 gate means only `list[int]` ever reaches
        // this crate from a real compiled program, but this crate's own
        // lowering must not bake in that assumption independently of the
        // type it actually observes -- exactly the class of bug the
        // `AnnAssign` widening fix earlier in this file already guards
        // against (MIR's `ty` silently diverging from what codegen must
        // produce). Uses `str` elements specifically because they are
        // trivially distinguishable from the `Ty::Int` a hardcoded bug
        // would wrongly report.
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "xs".to_string(),
                    value: HirExpr::ListLiteral(vec![HirExpr::StringLiteral("a".to_string())]),
                }),
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "y".to_string(),
                    value: HirExpr::Subscript {
                        base: Box::new(HirExpr::Name("xs".to_string())),
                        index: Box::new(HirExpr::IntLiteral(0)),
                    },
                }),
                HirItem::TopLevelStmt(HirStmt::ForList {
                    var: "v".to_string(),
                    list: "xs".to_string(),
                    body: vec![HirStmt::ExprStmt(HirExpr::Name("v".to_string()))],
                }),
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Name("y".to_string()))),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        // `y = xs[0]` binds `y` as `Ty::Str`, derived from `xs`'s own
        // `Ty::List(Box::new(Ty::Str))` binding (itself derived from the
        // `StringLiteral` element), not `Ty::Int`.
        assert_eq!(
            mir.items[3],
            MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Name {
                name: "y".to_string(),
                ty: Ty::Str,
            }))
        );
        // `for v in xs:` binds `v` as `Ty::Str` too, derived from the same
        // list, not `Ty::Int`.
        assert_eq!(
            mir.items[2],
            MirItem::TopLevelStmt(MirStmt::ForList {
                var: "v".to_string(),
                list: "xs".to_string(),
                body: vec![MirStmt::ExprStmt(MirExpr::Name {
                    name: "v".to_string(),
                    ty: Ty::Str,
                })],
            })
        );
    }

    #[test]
    #[should_panic(expected = "an empty list literal has no element type")]
    fn an_empty_list_literals_ty_panics_with_an_internal_error() {
        // By construction (see this module's `lookup` panic doc comment /
        // D-057 discussion), `pycc_types::check` already rejects an empty
        // list literal (T0021) before any HIR reaches `pycc_mir` -- this
        // MIR node could never come from a real `check_and_resolve`
        // success, but the panic path itself still needs direct coverage.
        MirExpr::ListLiteral(vec![]).ty();
    }

    #[test]
    #[should_panic(expected = "subscript base has non-list/tuple type")]
    fn a_subscript_over_a_non_list_bases_ty_panics_with_an_internal_error() {
        // A non-list, non-dict, non-tuple subscript base (e.g. `Ty::Int`) is
        // rejected by `pycc_types` (T0033) before HIR reaches `pycc_mir` --
        // unlike a dict base (which `pycc_types` accepts, but `lower_expr`'s
        // own `HirExpr::Subscript` arm routes into `MirExpr::DictGet` instead
        // of ever constructing this node), so this defensive panic path in
        // `MirExpr::Subscript`'s own `ty()` arm still needs direct coverage
        // via a hand-built node that bypasses both guarantees.
        MirExpr::Subscript {
            base: Box::new(MirExpr::IntLiteral(1)),
            index: Box::new(MirExpr::IntLiteral(0)),
        }
        .ty();
    }

    #[test]
    #[should_panic(expected = "dict subscript base has non-dict type")]
    fn a_dict_get_over_a_non_dict_bases_ty_panics_with_an_internal_error() {
        // Same reasoning as the subscript panic above, for `MirExpr::DictGet`'s
        // own defensive `ty()` arm: no real lowering ever constructs this
        // node with a non-dict base (`lower_expr`'s own `HirExpr::Subscript`
        // arm only builds `MirExpr::DictGet` when the base's derived type is
        // `Ty::Dict`), but the panic path still needs direct coverage.
        MirExpr::DictGet {
            dict: Box::new(MirExpr::IntLiteral(1)),
            key: Box::new(MirExpr::StringLiteral("a".to_string())),
        }
        .ty();
    }

    #[test]
    #[should_panic(expected = "neither a list, dict, nor set")]
    fn a_for_list_loop_over_a_non_list_non_dict_non_set_binding_panics_with_an_internal_error() {
        // Same reasoning again: `pycc_types` already rejects `for v in x:`
        // when `x` is neither a list, dict, nor set (T0033), but the
        // defensive panic path in `lower_stmt`'s `ForList` arm still needs
        // direct coverage.
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(5),
                }),
                HirItem::TopLevelStmt(HirStmt::ForList {
                    var: "v".to_string(),
                    list: "x".to_string(),
                    body: vec![],
                }),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        build(&hir);
    }

    #[test]
    #[should_panic(expected = "an empty dict literal has no key/value type to derive")]
    fn an_empty_dict_literals_ty_panics_with_an_internal_error() {
        // By construction, `pycc_types::check` already rejects an empty
        // dict literal (T0021, mirroring the empty-list-literal case above)
        // before any HIR reaches `pycc_mir` -- this MIR node could never
        // come from a real `check_and_resolve` success, but the panic path
        // itself still needs direct coverage.
        MirExpr::DictLiteral(vec![]).ty();
    }

    #[test]
    fn dict_literal_lowers_to_mir_dict_literal_with_correct_ty() {
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::DictLiteral(vec![(
                    HirExpr::StringLiteral("a".to_string()),
                    HirExpr::IntLiteral(1),
                )]),
            })],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        let expected_value = MirExpr::DictLiteral(vec![(
            MirExpr::StringLiteral("a".to_string()),
            MirExpr::IntLiteral(1),
        )]);
        assert_eq!(expected_value.ty(), Ty::Dict(Box::new((Ty::Str, Ty::Int))));
        assert_eq!(
            mir.items[0],
            MirItem::TopLevelStmt(MirStmt::Assign {
                target: "x".to_string(),
                value: expected_value,
            })
        );
    }

    #[test]
    fn dict_get_ty_unwraps_the_value_type() {
        // `x["a"]` where `x: dict[str, int]` lowers `HirExpr::Subscript`
        // into `MirExpr::DictGet` (not `MirExpr::Subscript`), whose `ty()`
        // unwraps the dict's value type, mirroring `dict_get_ty_unwraps_the_value_type`
        // in the task brief.
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::DictLiteral(vec![(
                        HirExpr::StringLiteral("a".to_string()),
                        HirExpr::IntLiteral(1),
                    )]),
                }),
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "y".to_string(),
                    value: HirExpr::Subscript {
                        base: Box::new(HirExpr::Name("x".to_string())),
                        index: Box::new(HirExpr::StringLiteral("a".to_string())),
                    },
                }),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items[1],
            MirItem::TopLevelStmt(MirStmt::Assign {
                target: "y".to_string(),
                value: MirExpr::DictGet {
                    dict: Box::new(MirExpr::Name {
                        name: "x".to_string(),
                        ty: Ty::Dict(Box::new((Ty::Str, Ty::Int))),
                    }),
                    key: Box::new(MirExpr::StringLiteral("a".to_string())),
                },
            })
        );
        assert_eq!(
            MirExpr::DictGet {
                dict: Box::new(MirExpr::Name {
                    name: "x".to_string(),
                    ty: Ty::Dict(Box::new((Ty::Str, Ty::Int))),
                }),
                key: Box::new(MirExpr::StringLiteral("a".to_string())),
            }
            .ty(),
            Ty::Int
        );
    }

    #[test]
    fn a_list_subscript_still_lowers_to_mir_subscript_not_dict_get() {
        // Genericity check mirroring `list_literal_subscript_and_for_list_derive_their_type_from_actual_elements_not_hardcoded_int`
        // above: `lower_expr`'s `HirExpr::Subscript` arm must route based on
        // the base's *actual* derived type, not assume every subscript is a
        // dict read now that `MirExpr::DictGet` exists.
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "xs".to_string(),
                    value: HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1)]),
                }),
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "y".to_string(),
                    value: HirExpr::Subscript {
                        base: Box::new(HirExpr::Name("xs".to_string())),
                        index: Box::new(HirExpr::IntLiteral(0)),
                    },
                }),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items[1],
            MirItem::TopLevelStmt(MirStmt::Assign {
                target: "y".to_string(),
                value: MirExpr::Subscript {
                    base: Box::new(MirExpr::Name {
                        name: "xs".to_string(),
                        ty: Ty::List(Box::new(Ty::Int)),
                    }),
                    index: Box::new(MirExpr::IntLiteral(0)),
                },
            })
        );
    }

    #[test]
    fn dict_set_lowers_to_mir_dict_set_stmt() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::DictLiteral(vec![(
                        HirExpr::StringLiteral("a".to_string()),
                        HirExpr::IntLiteral(1),
                    )]),
                }),
                HirItem::TopLevelStmt(HirStmt::DictSet {
                    dict: "x".to_string(),
                    key: HirExpr::StringLiteral("b".to_string()),
                    value: HirExpr::IntLiteral(2),
                }),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items[1],
            MirItem::TopLevelStmt(MirStmt::DictSet {
                dict: "x".to_string(),
                key: MirExpr::StringLiteral("b".to_string()),
                value: MirExpr::IntLiteral(2),
            })
        );
    }

    #[test]
    fn for_k_in_dict_lowers_to_mir_for_dict() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::DictLiteral(vec![(
                        HirExpr::StringLiteral("a".to_string()),
                        HirExpr::IntLiteral(1),
                    )]),
                }),
                HirItem::TopLevelStmt(HirStmt::ForList {
                    var: "k".to_string(),
                    list: "x".to_string(),
                    body: vec![HirStmt::ExprStmt(HirExpr::Call {
                        callee: "print".to_string(),
                        args: vec![HirExpr::Name("k".to_string())],
                    })],
                }),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items[1],
            MirItem::TopLevelStmt(MirStmt::ForDict {
                var: "k".to_string(),
                dict: "x".to_string(),
                body: vec![MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::Name {
                        name: "k".to_string(),
                        ty: Ty::Str,
                    }],
                    ty: Ty::None,
                })],
            })
        );
    }

    #[test]
    #[should_panic(expected = "an empty set literal has no element type to derive")]
    fn an_empty_set_literals_ty_panics_with_an_internal_error() {
        // By construction, `pycc_types::check` already rejects an empty set
        // literal (mirroring the empty-list/empty-dict-literal cases above)
        // before any HIR reaches `pycc_mir` -- and, unlike those two, an
        // empty `SetLiteral` cannot even be *written* in real Python source
        // (`{}` always parses as an empty `dict`, never an empty `set`) --
        // but the panic path itself still needs direct coverage.
        MirExpr::SetLiteral(vec![]).ty();
    }

    #[test]
    fn tuple_literal_ty_derives_positionally_from_every_element() {
        let expr = MirExpr::TupleLiteral(vec![
            MirExpr::IntLiteral(1),
            MirExpr::BoolLiteral(true),
            MirExpr::FloatLiteral(2.5),
        ]);
        assert_eq!(
            expr.ty(),
            Ty::Tuple(Box::new(vec![Ty::Int, Ty::Bool, Ty::Float]))
        );
    }

    #[test]
    #[should_panic(expected = "an empty tuple literal has no element types to derive")]
    fn an_empty_tuple_literal_ty_panics_with_an_internal_error() {
        // By construction, `pycc_types::check` already rejects an empty
        // tuple literal (T0039, mirroring the empty-list/empty-dict-literal
        // cases above) before any HIR reaches `pycc_mir` -- but the panic
        // path itself still needs direct coverage.
        MirExpr::TupleLiteral(vec![]).ty();
    }

    #[test]
    fn tuple_subscript_ty_derives_the_positional_element_type() {
        let expr = MirExpr::Subscript {
            base: Box::new(MirExpr::TupleLiteral(vec![
                MirExpr::IntLiteral(1),
                MirExpr::BoolLiteral(true),
            ])),
            index: Box::new(MirExpr::IntLiteral(1)),
        };
        assert_eq!(expr.ty(), Ty::Bool);
    }

    #[test]
    #[should_panic(expected = "tuple subscript index is not a literal int")]
    fn a_tuple_subscript_with_a_non_literal_index_ty_panics_with_an_internal_error() {
        // By construction, `pycc_types::check` already rejects a
        // non-literal tuple subscript index (T0040) before any HIR reaches
        // `pycc_mir` -- but the panic path itself still needs direct
        // coverage via a hand-built node that bypasses that guarantee.
        let expr = MirExpr::Subscript {
            base: Box::new(MirExpr::TupleLiteral(vec![MirExpr::IntLiteral(1)])),
            index: Box::new(MirExpr::Name {
                name: "i".to_string(),
                ty: Ty::Int,
            }),
        };
        expr.ty();
    }

    #[test]
    #[should_panic(expected = "tuple subscript index is negative")]
    fn a_tuple_subscript_with_a_negative_index_ty_panics_with_an_internal_error() {
        // By construction, `pycc_types::check` already rejects a negative
        // literal tuple subscript index (T0040) before any HIR reaches
        // `pycc_mir` -- but the panic path itself still needs direct
        // coverage via a hand-built node that bypasses that guarantee.
        let expr = MirExpr::Subscript {
            base: Box::new(MirExpr::TupleLiteral(vec![MirExpr::IntLiteral(1)])),
            index: Box::new(MirExpr::IntLiteral(-1)),
        };
        expr.ty();
    }

    #[test]
    #[should_panic(expected = "tuple subscript index out of range")]
    fn a_tuple_subscript_out_of_range_ty_panics_with_an_internal_error() {
        // By construction, `pycc_types::check` already rejects an
        // out-of-range literal tuple subscript index (T0040) before any HIR
        // reaches `pycc_mir` -- but the panic path itself still needs
        // direct coverage via a hand-built node that bypasses that
        // guarantee.
        let expr = MirExpr::Subscript {
            base: Box::new(MirExpr::TupleLiteral(vec![MirExpr::IntLiteral(1)])),
            index: Box::new(MirExpr::IntLiteral(5)),
        };
        expr.ty();
    }

    #[test]
    fn tuple_literal_lowers_to_mir_tuple_literal_with_correct_ty() {
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::TupleLiteral(vec![
                    HirExpr::IntLiteral(1),
                    HirExpr::BoolLiteral(true),
                ]),
            })],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        let expected_value =
            MirExpr::TupleLiteral(vec![MirExpr::IntLiteral(1), MirExpr::BoolLiteral(true)]);
        assert_eq!(
            expected_value.ty(),
            Ty::Tuple(Box::new(vec![Ty::Int, Ty::Bool]))
        );
        assert_eq!(
            mir.items[0],
            MirItem::TopLevelStmt(MirStmt::Assign {
                target: "x".to_string(),
                value: expected_value,
            })
        );
    }

    #[test]
    fn set_literal_lowers_to_mir_set_literal_with_correct_ty() {
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::SetLiteral(vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)]),
            })],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        let expected_value =
            MirExpr::SetLiteral(vec![MirExpr::IntLiteral(1), MirExpr::IntLiteral(2)]);
        assert_eq!(expected_value.ty(), Ty::Set(Box::new(Ty::Int)));
        assert_eq!(
            mir.items[0],
            MirItem::TopLevelStmt(MirStmt::Assign {
                target: "x".to_string(),
                value: expected_value,
            })
        );
    }

    #[test]
    fn for_x_in_set_lowers_to_mir_for_set() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::SetLiteral(vec![
                        HirExpr::IntLiteral(1),
                        HirExpr::IntLiteral(2),
                    ]),
                }),
                HirItem::TopLevelStmt(HirStmt::ForList {
                    var: "v".to_string(),
                    list: "x".to_string(),
                    body: vec![HirStmt::ExprStmt(HirExpr::Call {
                        callee: "print".to_string(),
                        args: vec![HirExpr::Name("v".to_string())],
                    })],
                }),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items[1],
            MirItem::TopLevelStmt(MirStmt::ForSet {
                var: "v".to_string(),
                set: "x".to_string(),
                body: vec![MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::Name {
                        name: "v".to_string(),
                        ty: Ty::Int,
                    }],
                    ty: Ty::None,
                })],
            })
        );
    }

    // -- PR-12 Task 4 (D-117): comprehension lowering --

    #[test]
    fn a_range_sourced_list_comprehension_lowers_to_comp_source_range_with_var_ty_int_and_evaluates_its_filter()
     {
        // Exercises `resolve_comp_source`'s `CompIter::Range` branch and the
        // `ListCompAssign` arm's `cond: Some(..)` path (both closures on
        // that arm need at least one executing test for D-014's coverage
        // gate).
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::ListCompAssign {
                    target: "y".to_string(),
                    var: "i".to_string(),
                    iter: CompIter::Range {
                        start: HirExpr::IntLiteral(0),
                        stop: HirExpr::IntLiteral(3),
                        step: HirExpr::IntLiteral(1),
                    },
                    cond: Some(Box::new(HirExpr::BoolLiteral(true))),
                    elt: Box::new(HirExpr::Name("i".to_string())),
                }),
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "len".to_string(),
                    args: vec![HirExpr::Name("y".to_string())],
                })),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items[0],
            MirItem::TopLevelStmt(MirStmt::ListCompAssign {
                target: "y".to_string(),
                var: "i".to_string(),
                var_ty: Ty::Int,
                source: CompSource::Range {
                    start: MirExpr::IntLiteral(0),
                    stop: MirExpr::IntLiteral(3),
                    step: MirExpr::IntLiteral(1),
                },
                cond: Some(Box::new(MirExpr::BoolLiteral(true))),
                elt: Box::new(MirExpr::Name {
                    name: "i".to_string(),
                    ty: Ty::Int,
                }),
            })
        );
        // `target` is bound as `Ty::List(Ty::Int)`, derived from `elt`'s
        // type -- confirmed via the following statement's own lowered type.
        assert_eq!(
            mir.items[1],
            MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                callee: "len".to_string(),
                args: vec![MirExpr::Name {
                    name: "y".to_string(),
                    ty: Ty::List(Box::new(Ty::Int)),
                }],
                ty: Ty::Int,
            }))
        );
    }

    #[test]
    fn a_bare_name_list_sourced_list_comprehension_resolves_comp_source_list_with_the_lists_element_type()
     {
        // Exercises `resolve_comp_source`'s `CompIter::Name` branch resolving
        // to `Ty::List` -- uses `str` elements specifically (mirroring this
        // file's own `list_literal_subscript_and_for_list_derive_their_type_from_actual_elements_not_hardcoded_int`)
        // so `var_ty` is trivially distinguishable from the `Ty::Int` a
        // hardcoded bug would wrongly report.
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "xs".to_string(),
                    value: HirExpr::ListLiteral(vec![HirExpr::StringLiteral("a".to_string())]),
                }),
                HirItem::TopLevelStmt(HirStmt::ListCompAssign {
                    target: "y".to_string(),
                    var: "v".to_string(),
                    iter: CompIter::Name("xs".to_string()),
                    cond: None,
                    elt: Box::new(HirExpr::Name("v".to_string())),
                }),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items[1],
            MirItem::TopLevelStmt(MirStmt::ListCompAssign {
                target: "y".to_string(),
                var: "v".to_string(),
                var_ty: Ty::Str,
                source: CompSource::List("xs".to_string()),
                cond: None,
                elt: Box::new(MirExpr::Name {
                    name: "v".to_string(),
                    ty: Ty::Str,
                }),
            })
        );
    }

    #[test]
    fn a_range_sourced_set_comprehension_lowers_to_comp_source_range_with_var_ty_int_and_evaluates_its_filter()
     {
        // Exercises the `SetCompAssign` arm's own `cond: Some(..)` path
        // (distinct closures from `ListCompAssign`'s own, needing their own
        // executing test for D-014's coverage gate).
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::SetCompAssign {
                target: "y".to_string(),
                var: "i".to_string(),
                iter: CompIter::Range {
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                },
                cond: Some(Box::new(HirExpr::BoolLiteral(true))),
                elt: Box::new(HirExpr::Name("i".to_string())),
            })],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items[0],
            MirItem::TopLevelStmt(MirStmt::SetCompAssign {
                target: "y".to_string(),
                var: "i".to_string(),
                var_ty: Ty::Int,
                source: CompSource::Range {
                    start: MirExpr::IntLiteral(0),
                    stop: MirExpr::IntLiteral(3),
                    step: MirExpr::IntLiteral(1),
                },
                cond: Some(Box::new(MirExpr::BoolLiteral(true))),
                elt: Box::new(MirExpr::Name {
                    name: "i".to_string(),
                    ty: Ty::Int,
                }),
            })
        );
    }

    #[test]
    fn a_bare_name_set_sourced_set_comprehension_resolves_comp_source_set() {
        // Exercises `resolve_comp_source`'s `CompIter::Name` branch resolving
        // to `Ty::Set`.
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "s".to_string(),
                    value: HirExpr::SetLiteral(vec![
                        HirExpr::IntLiteral(1),
                        HirExpr::IntLiteral(2),
                    ]),
                }),
                HirItem::TopLevelStmt(HirStmt::SetCompAssign {
                    target: "y".to_string(),
                    var: "v".to_string(),
                    iter: CompIter::Name("s".to_string()),
                    cond: None,
                    elt: Box::new(HirExpr::Name("v".to_string())),
                }),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items[1],
            MirItem::TopLevelStmt(MirStmt::SetCompAssign {
                target: "y".to_string(),
                var: "v".to_string(),
                var_ty: Ty::Int,
                source: CompSource::Set("s".to_string()),
                cond: None,
                elt: Box::new(MirExpr::Name {
                    name: "v".to_string(),
                    ty: Ty::Int,
                }),
            })
        );
    }

    #[test]
    fn a_bare_name_dict_sourced_dict_comprehension_resolves_comp_source_dict_with_var_ty_as_the_key_type_not_the_value_type()
     {
        // Exercises `resolve_comp_source`'s `CompIter::Name` branch resolving
        // to `Ty::Dict`, and the `DictCompAssign` arm's own `cond: Some(..)`
        // path (distinct closures from `ListCompAssign`/`SetCompAssign`'s
        // own). Pins that `var_ty` is the dict's *key* type (`kv.0`), not its
        // value type (`kv.1`) -- mirrors `ForList`'s own identical
        // `Ty::Dict(kv) => kv.0` choice (`for_k_in_dict_lowers_to_mir_for_dict`
        // above binds a `dict[str, int]`'s loop variable as `Ty::Str`, the
        // key type, for the same reason).
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "d".to_string(),
                    value: HirExpr::DictLiteral(vec![(
                        HirExpr::StringLiteral("a".to_string()),
                        HirExpr::IntLiteral(1),
                    )]),
                }),
                HirItem::TopLevelStmt(HirStmt::DictCompAssign {
                    target: "y".to_string(),
                    var: "k".to_string(),
                    iter: CompIter::Name("d".to_string()),
                    cond: Some(Box::new(HirExpr::BoolLiteral(true))),
                    key: Box::new(HirExpr::Name("k".to_string())),
                    value: Box::new(HirExpr::IntLiteral(2)),
                }),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items[1],
            MirItem::TopLevelStmt(MirStmt::DictCompAssign {
                target: "y".to_string(),
                var: "k".to_string(),
                var_ty: Ty::Str,
                source: CompSource::Dict("d".to_string()),
                cond: Some(Box::new(MirExpr::BoolLiteral(true))),
                key: Box::new(MirExpr::Name {
                    name: "k".to_string(),
                    ty: Ty::Str,
                }),
                value: Box::new(MirExpr::IntLiteral(2)),
            })
        );
    }

    #[test]
    #[should_panic(expected = "neither a list, dict, nor set")]
    fn a_comprehension_over_a_non_list_non_dict_non_set_binding_panics_with_an_internal_error() {
        // Same reasoning as `a_for_list_loop_over_a_non_list_non_dict_non_set_binding_panics_with_an_internal_error`
        // above: `pycc_types` already rejects a comprehension whose bare-name
        // iterable is neither a list, dict, nor set (T0033), but
        // `resolve_comp_source`'s own defensive panic path still needs
        // direct coverage via a hand-built HIR that bypasses that guarantee.
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(5),
                }),
                HirItem::TopLevelStmt(HirStmt::ListCompAssign {
                    target: "y".to_string(),
                    var: "v".to_string(),
                    iter: CompIter::Name("x".to_string()),
                    cond: None,
                    elt: Box::new(HirExpr::Name("v".to_string())),
                }),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        build(&hir);
    }

    // -- Task 8 (D-118): `HirExpr::Slice` -> `MirExpr::Slice` lowering ----

    /// Builds `xs = [1, 2, 3]` followed by `y = <slice>` for some
    /// `HirExpr::Slice` reading `xs`, mirroring the fixture every Task 6
    /// (`pycc_hir`) slicing test starts from, so this lowering is exercised
    /// against the same shapes those frontend tests already pin.
    fn xs_list_then_slice(slice: HirExpr) -> HirModule {
        HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "xs".to_string(),
                    value: HirExpr::ListLiteral(vec![
                        HirExpr::IntLiteral(1),
                        HirExpr::IntLiteral(2),
                        HirExpr::IntLiteral(3),
                    ]),
                }),
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "y".to_string(),
                    value: slice,
                }),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        }
    }

    #[test]
    fn a_slice_expression_with_both_bounds_present_lowers_with_both_bounds_some() {
        // `xs[1:3]` (mirrors `pycc_hir`'s
        // `lowers_a_slice_expression_with_both_bounds_present`).
        let hir = xs_list_then_slice(HirExpr::Slice {
            base: Box::new(HirExpr::Name("xs".to_string())),
            start: Some(Box::new(HirExpr::IntLiteral(1))),
            stop: Some(Box::new(HirExpr::IntLiteral(3))),
            step: None,
        });
        let mir = build(&hir);
        assert_eq!(
            mir.items[1],
            MirItem::TopLevelStmt(MirStmt::Assign {
                target: "y".to_string(),
                value: MirExpr::Slice {
                    base: Box::new(MirExpr::Name {
                        name: "xs".to_string(),
                        ty: Ty::List(Box::new(Ty::Int)),
                    }),
                    start: Some(Box::new(MirExpr::IntLiteral(1))),
                    stop: Some(Box::new(MirExpr::IntLiteral(3))),
                    step: None,
                },
            })
        );
    }

    #[test]
    fn a_slice_expression_with_only_the_stop_bound_lowers_with_start_and_step_none() {
        // `xs[:3]` (mirrors `pycc_hir`'s
        // `lowers_a_slice_expression_with_only_the_stop_bound`).
        let hir = xs_list_then_slice(HirExpr::Slice {
            base: Box::new(HirExpr::Name("xs".to_string())),
            start: None,
            stop: Some(Box::new(HirExpr::IntLiteral(3))),
            step: None,
        });
        let mir = build(&hir);
        assert_eq!(
            mir.items[1],
            MirItem::TopLevelStmt(MirStmt::Assign {
                target: "y".to_string(),
                value: MirExpr::Slice {
                    base: Box::new(MirExpr::Name {
                        name: "xs".to_string(),
                        ty: Ty::List(Box::new(Ty::Int)),
                    }),
                    start: None,
                    stop: Some(Box::new(MirExpr::IntLiteral(3))),
                    step: None,
                },
            })
        );
    }

    #[test]
    fn a_slice_expression_with_only_the_start_bound_lowers_with_stop_and_step_none() {
        // `xs[2:]` (mirrors `pycc_hir`'s
        // `lowers_a_slice_expression_with_only_the_start_bound`).
        let hir = xs_list_then_slice(HirExpr::Slice {
            base: Box::new(HirExpr::Name("xs".to_string())),
            start: Some(Box::new(HirExpr::IntLiteral(2))),
            stop: None,
            step: None,
        });
        let mir = build(&hir);
        assert_eq!(
            mir.items[1],
            MirItem::TopLevelStmt(MirStmt::Assign {
                target: "y".to_string(),
                value: MirExpr::Slice {
                    base: Box::new(MirExpr::Name {
                        name: "xs".to_string(),
                        ty: Ty::List(Box::new(Ty::Int)),
                    }),
                    start: Some(Box::new(MirExpr::IntLiteral(2))),
                    stop: None,
                    step: None,
                },
            })
        );
    }

    #[test]
    fn a_slice_expression_with_all_bounds_omitted_lowers_with_every_bound_none() {
        // `xs[:]` (mirrors `pycc_hir`'s
        // `lowers_a_slice_expression_with_all_bounds_omitted`).
        let hir = xs_list_then_slice(HirExpr::Slice {
            base: Box::new(HirExpr::Name("xs".to_string())),
            start: None,
            stop: None,
            step: None,
        });
        let mir = build(&hir);
        assert_eq!(
            mir.items[1],
            MirItem::TopLevelStmt(MirStmt::Assign {
                target: "y".to_string(),
                value: MirExpr::Slice {
                    base: Box::new(MirExpr::Name {
                        name: "xs".to_string(),
                        ty: Ty::List(Box::new(Ty::Int)),
                    }),
                    start: None,
                    stop: None,
                    step: None,
                },
            })
        );
    }

    #[test]
    fn a_slice_expression_with_only_a_step_lowers_with_start_and_stop_none() {
        // `xs[::2]` (mirrors `pycc_hir`'s
        // `lowers_a_slice_expression_with_a_step`).
        let hir = xs_list_then_slice(HirExpr::Slice {
            base: Box::new(HirExpr::Name("xs".to_string())),
            start: None,
            stop: None,
            step: Some(Box::new(HirExpr::IntLiteral(2))),
        });
        let mir = build(&hir);
        assert_eq!(
            mir.items[1],
            MirItem::TopLevelStmt(MirStmt::Assign {
                target: "y".to_string(),
                value: MirExpr::Slice {
                    base: Box::new(MirExpr::Name {
                        name: "xs".to_string(),
                        ty: Ty::List(Box::new(Ty::Int)),
                    }),
                    start: None,
                    stop: None,
                    step: Some(Box::new(MirExpr::IntLiteral(2))),
                },
            })
        );
    }

    #[test]
    fn a_slice_expressions_base_and_every_present_bound_are_recursively_lowered() {
        // `f()`/`g()`/`h()` stand in for "some already-lowerable non-literal
        // shape" -- confirms `base`/`start`/`stop`/`step` are each passed
        // through the real `lower_expr` recursively (mirroring
        // `pycc_hir`'s own `a_slice_expressions_base_and_bounds_are_recursively_lowered`),
        // not merely accepted as raw literals or the base's bare `Name`.
        // Registers `f`/`g`/`h` as zero-arg functions returning `int` so
        // `lower_expr`'s `HirExpr::Call` arm resolves their `ty` via the
        // real `$fn:` lookup instead of panicking.
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "f".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(0)))],
                },
                HirItem::Function {
                    name: "g".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(0)))],
                },
                HirItem::Function {
                    name: "h".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(0)))],
                },
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "xs".to_string(),
                    value: HirExpr::ListLiteral(vec![
                        HirExpr::IntLiteral(1),
                        HirExpr::IntLiteral(2),
                        HirExpr::IntLiteral(3),
                    ]),
                }),
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
                        step: Some(Box::new(HirExpr::Call {
                            callee: "h".to_string(),
                            args: vec![],
                        })),
                    },
                }),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items[4],
            MirItem::TopLevelStmt(MirStmt::Assign {
                target: "y".to_string(),
                value: MirExpr::Slice {
                    base: Box::new(MirExpr::Name {
                        name: "xs".to_string(),
                        ty: Ty::List(Box::new(Ty::Int)),
                    }),
                    start: Some(Box::new(MirExpr::Call {
                        callee: "f".to_string(),
                        args: vec![],
                        ty: Ty::Int,
                    })),
                    stop: Some(Box::new(MirExpr::Call {
                        callee: "g".to_string(),
                        args: vec![],
                        ty: Ty::Int,
                    })),
                    step: Some(Box::new(MirExpr::Call {
                        callee: "h".to_string(),
                        args: vec![],
                        ty: Ty::Int,
                    })),
                },
            })
        );
    }

    #[test]
    fn a_slices_ty_derives_from_the_actual_base_type_not_hardcoded_list_of_int() {
        // Mirrors this file's own genericity test for `Subscript`/`ForList`
        // (`list_literal_subscript_and_for_list_derive_their_type_from_actual_elements_not_hardcoded_int`):
        // `MirExpr::Slice`'s `ty()` must derive its result from the actual
        // `base.ty()`, not assume `Ty::List(Box::new(Ty::Int))`.
        // `pycc_types`' T0034 gate means only `list[int]` ever reaches this
        // crate from a real compiled program (a `list[str]` slice is
        // rejected before `pycc_mir` ever sees it), but this crate's own
        // `ty()` must not bake in that assumption independently of the type
        // it actually observes -- so this test bypasses that gate with a
        // hand-built `MirExpr::Slice` over a `list[str]` base, exactly like
        // the `Subscript`/`DictGet` panic tests above bypass gates that
        // can't be reached from a real, type-checked program.
        let slice = MirExpr::Slice {
            base: Box::new(MirExpr::ListLiteral(vec![MirExpr::StringLiteral(
                "a".to_string(),
            )])),
            start: Some(Box::new(MirExpr::IntLiteral(0))),
            stop: None,
            step: None,
        };
        assert_eq!(slice.ty(), Ty::List(Box::new(Ty::Str)));

        // Presence/absence of bounds must not affect `ty()` either --
        // Task 8's brief requirement (c). Compare the all-bounds-omitted
        // shape against the same base.
        let slice_no_bounds = MirExpr::Slice {
            base: Box::new(MirExpr::ListLiteral(vec![MirExpr::StringLiteral(
                "a".to_string(),
            )])),
            start: None,
            stop: None,
            step: None,
        };
        assert_eq!(slice.ty(), slice_no_bounds.ty());
    }

    // -- D-154 (Part 1 of #375): class instantiation, attribute access,
    // method calls --------------------------------------------------------

    /// A minimal `Point` class module: `__init__(self, x: int, y: int)`
    /// sets both attributes from its own parameters; `bump(self) -> None`
    /// reads and mutates `self.x`. Mirrors `pycc_types::class::tests`'s own
    /// `point_module` fixture (same shape, this crate's own `HirModule`
    /// literal convention).
    fn point_module(extra_items: Vec<HirItem>) -> HirModule {
        let self_ty = Ty::Instance(Box::new("Point".to_string()));
        let init = HirItem::Function {
            name: "Point.__init__".to_string(),
            params: vec![
                ("self".to_string(), self_ty.clone()),
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
                HirStmt::Return(None),
            ],
        };
        let bump = HirItem::Function {
            name: "Point.bump".to_string(),
            params: vec![("self".to_string(), self_ty)],
            return_ty: Ty::None,
            body: vec![
                HirStmt::AttrSet {
                    base: HirExpr::Name("self".to_string()),
                    attr: "x".to_string(),
                    value: HirExpr::BinOp {
                        op: BinOpKind::Add,
                        left: Box::new(HirExpr::AttrGet {
                            base: Box::new(HirExpr::Name("self".to_string())),
                            attr: "x".to_string(),
                        }),
                        right: Box::new(HirExpr::IntLiteral(1)),
                    },
                },
                HirStmt::Return(None),
            ],
        };
        let mut items = vec![init, bump];
        items.extend(extra_items);
        HirModule {
            items,
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: vec![(
                "Point".to_string(),
                HirClassDef {
                    name: "Point".to_string(),
                    bases: Vec::new(),
                    mro: vec!["Point".to_string()],
                    attrs: vec![("x".to_string(), Ty::Int), ("y".to_string(), Ty::Int)],
                    methods: vec![
                        ("__init__".to_string(), "Point.__init__".to_string()),
                        ("bump".to_string(), "Point.bump".to_string()),
                    ],
                    type_param: None,
                    properties: Vec::new(),
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
                },
            )],
        }
    }

    #[test]
    fn instantiation_lowers_to_a_dedicated_mir_node() {
        let hir = point_module(vec![HirItem::TopLevelStmt(HirStmt::Assign {
            target: "p".to_string(),
            value: HirExpr::Call {
                callee: "Point".to_string(),
                args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
            },
        })]);
        let mir = build(&hir);
        assert_eq!(
            mir.items.last(),
            Some(&MirItem::TopLevelStmt(MirStmt::Assign {
                target: "p".to_string(),
                value: MirExpr::Instantiate(Box::new(InstantiateExpr {
                    ctor: "Point.__init__".to_string(),
                    attr_count: 2,
                    args: vec![MirExpr::IntLiteral(1), MirExpr::IntLiteral(2)],
                    ty: Ty::Instance(Box::new("Point".to_string())),
                })),
            }))
        );
    }

    #[test]
    fn a_method_call_lowers_to_an_ordinary_call_with_self_prepended() {
        let hir = point_module(vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "p".to_string(),
                value: HirExpr::Call {
                    callee: "Point".to_string(),
                    args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
                },
            }),
            HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::MethodCall {
                base: Box::new(HirExpr::Name("p".to_string())),
                method: "bump".to_string(),
                args: vec![],
            })),
        ]);
        let mir = build(&hir);
        assert_eq!(
            mir.items.last(),
            Some(&MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                callee: "Point.bump".to_string(),
                args: vec![MirExpr::Name {
                    name: "p".to_string(),
                    ty: Ty::Instance(Box::new("Point".to_string())),
                }],
                ty: Ty::None,
            })))
        );
    }

    #[test]
    fn a_method_call_with_arguments_lowers_each_argument_after_self() {
        // `point_module`'s own `bump` method takes no extra parameters, so
        // `a_method_call_lowers_to_an_ordinary_call_with_self_prepended`
        // above never exercises `MethodCall`'s own per-argument lowering
        // loop -- a standalone minimal `Counter.add(self, n: int) -> None`
        // fixture here does.
        let self_ty = Ty::Instance(Box::new("Counter".to_string()));
        let init = HirItem::Function {
            name: "Counter.__init__".to_string(),
            params: vec![("self".to_string(), self_ty.clone())],
            return_ty: Ty::None,
            body: vec![
                HirStmt::AttrSet {
                    base: HirExpr::Name("self".to_string()),
                    attr: "n".to_string(),
                    value: HirExpr::IntLiteral(0),
                },
                HirStmt::Return(None),
            ],
        };
        let add = HirItem::Function {
            name: "Counter.add".to_string(),
            params: vec![
                ("self".to_string(), self_ty.clone()),
                ("n".to_string(), Ty::Int),
            ],
            return_ty: Ty::None,
            body: vec![HirStmt::Return(None)],
        };
        let hir = HirModule {
            items: vec![
                init,
                add,
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "c".to_string(),
                    value: HirExpr::Call {
                        callee: "Counter".to_string(),
                        args: vec![],
                    },
                }),
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::MethodCall {
                    base: Box::new(HirExpr::Name("c".to_string())),
                    method: "add".to_string(),
                    args: vec![HirExpr::IntLiteral(5)],
                })),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: vec![(
                "Counter".to_string(),
                HirClassDef {
                    name: "Counter".to_string(),
                    bases: Vec::new(),
                    mro: vec!["Counter".to_string()],
                    attrs: vec![("n".to_string(), Ty::Int)],
                    methods: vec![
                        ("__init__".to_string(), "Counter.__init__".to_string()),
                        ("add".to_string(), "Counter.add".to_string()),
                    ],
                    type_param: None,
                    properties: Vec::new(),
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
                },
            )],
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items.last(),
            Some(&MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                callee: "Counter.add".to_string(),
                args: vec![
                    MirExpr::Name {
                        name: "c".to_string(),
                        ty: self_ty,
                    },
                    MirExpr::IntLiteral(5),
                ],
                ty: Ty::None,
            })))
        );
    }

    #[test]
    fn an_attribute_read_lowers_to_a_slot_index() {
        let hir = point_module(vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "p".to_string(),
                value: HirExpr::Call {
                    callee: "Point".to_string(),
                    args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
                },
            }),
            HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::AttrGet {
                    base: Box::new(HirExpr::Name("p".to_string())),
                    attr: "y".to_string(),
                }],
            })),
        ]);
        let mir = build(&hir);
        assert_eq!(
            mir.items.last(),
            Some(&MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                callee: "print".to_string(),
                args: vec![MirExpr::AttrGet {
                    base: Box::new(MirExpr::Name {
                        name: "p".to_string(),
                        ty: Ty::Instance(Box::new("Point".to_string())),
                    }),
                    slot: 1,
                    ty: Ty::Int,
                }],
                ty: Ty::None,
            })))
        );
    }

    #[test]
    fn an_attribute_write_lowers_to_a_slot_index() {
        let hir = point_module(vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "p".to_string(),
                value: HirExpr::Call {
                    callee: "Point".to_string(),
                    args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
                },
            }),
            HirItem::TopLevelStmt(HirStmt::AttrSet {
                base: HirExpr::Name("p".to_string()),
                attr: "x".to_string(),
                value: HirExpr::IntLiteral(9),
            }),
        ]);
        let mir = build(&hir);
        assert_eq!(
            mir.items.last(),
            Some(&MirItem::TopLevelStmt(MirStmt::AttrSet {
                base: MirExpr::Name {
                    name: "p".to_string(),
                    ty: Ty::Instance(Box::new("Point".to_string())),
                },
                slot: 0,
                value: MirExpr::IntLiteral(9),
            }))
        );
    }

    #[test]
    fn the_init_and_bump_methods_themselves_lower_with_self_typed_as_the_instance() {
        // Exercises `__init__`'s own body -- two `HirStmt::AttrSet`s whose
        // RHS is a bare parameter reference -- and `bump`'s own body -- an
        // `HirExpr::AttrGet` nested inside a `BinOp` -- end to end through
        // `build`, not just the module-scope exercises above. Direct value
        // comparisons against `mir.items[0]`/`[1]`, not a
        // `let PATTERN = .. else { panic!(..) }` destructure -- this file's
        // own established coverage-gate convention (see e.g.
        // `class_instantiation_...` tests elsewhere): a hand-written panic
        // arm never taken on the happy path is a permanently uncovered
        // region under D-014's 100%-region gate.
        let hir = point_module(vec![]);
        let mir = build(&hir);
        let self_ty = Ty::Instance(Box::new("Point".to_string()));
        assert_eq!(
            mir.items[0],
            MirItem::Function {
                name: "Point.__init__".to_string(),
                params: vec![
                    ("self".to_string(), self_ty.clone()),
                    ("x".to_string(), Ty::Int),
                    ("y".to_string(), Ty::Int),
                ],
                return_ty: Ty::None,
                body: vec![
                    MirStmt::AttrSet {
                        base: MirExpr::Name {
                            name: "self".to_string(),
                            ty: self_ty.clone(),
                        },
                        slot: 0,
                        value: MirExpr::Name {
                            name: "x".to_string(),
                            ty: Ty::Int,
                        },
                    },
                    MirStmt::AttrSet {
                        base: MirExpr::Name {
                            name: "self".to_string(),
                            ty: self_ty.clone(),
                        },
                        slot: 1,
                        value: MirExpr::Name {
                            name: "y".to_string(),
                            ty: Ty::Int,
                        },
                    },
                    MirStmt::Return(None),
                ],
            }
        );
        assert_eq!(
            mir.items[1],
            MirItem::Function {
                name: "Point.bump".to_string(),
                params: vec![("self".to_string(), self_ty.clone())],
                return_ty: Ty::None,
                body: vec![
                    MirStmt::AttrSet {
                        base: MirExpr::Name {
                            name: "self".to_string(),
                            ty: self_ty.clone(),
                        },
                        slot: 0,
                        value: MirExpr::BinOp {
                            op: BinOpKind::Add,
                            left: Box::new(MirExpr::AttrGet {
                                base: Box::new(MirExpr::Name {
                                    name: "self".to_string(),
                                    ty: self_ty.clone(),
                                }),
                                slot: 0,
                                ty: Ty::Int,
                            }),
                            right: Box::new(MirExpr::IntLiteral(1)),
                            ty: Ty::Int,
                        },
                    },
                    MirStmt::Return(None),
                ],
            }
        );
    }

    #[test]
    #[should_panic(expected = "expected an instance- or protocol-typed expression")]
    fn attr_get_on_a_non_instance_base_panics_with_an_internal_error() {
        // Bypasses `pycc_types::check` (which would reject this) with a
        // hand-built `HirExpr::AttrGet` over an `int`-typed base, matching
        // this file's own established internal-error-test convention (see
        // e.g. `referencing_an_unbound_name_panics_with_an_internal_error`
        // above).
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(1),
                }),
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::AttrGet {
                    base: Box::new(HirExpr::Name("x".to_string())),
                    attr: "y".to_string(),
                })),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let _ = build(&hir);
    }

    #[test]
    #[should_panic(expected = "class `Ghost` has no registered HirClassDef")]
    fn attr_get_over_an_instance_typed_parameter_from_an_unregistered_class_panics_with_an_internal_error()
     {
        // `class_def_of`'s *second* panic (as opposed to
        // `attr_get_on_a_non_instance_base_panics_with_an_internal_error`
        // above, which exercises its first): `x`'s own declared parameter
        // type names `Ghost`, but `hir.class_defs` never registers it --
        // parameter binding itself never consults `classes` at all, so
        // this reaches `class_def_of`'s own `classes.get(..)` lookup with a
        // genuinely instance-typed expression pointing at a class this
        // module's own side table has no entry for.
        let ghost_ty = Ty::Instance(Box::new("Ghost".to_string()));
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "f".to_string(),
                params: vec![("x".to_string(), ghost_ty)],
                return_ty: Ty::None,
                body: vec![
                    HirStmt::ExprStmt(HirExpr::AttrGet {
                        base: Box::new(HirExpr::Name("x".to_string())),
                        attr: "y".to_string(),
                    }),
                    HirStmt::Return(None),
                ],
            }],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let _ = build(&hir);
    }

    #[test]
    #[should_panic(expected = "attribute `z` not declared on class `Point`")]
    fn attr_get_of_an_undeclared_attribute_panics_with_an_internal_error() {
        let hir = point_module(vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "p".to_string(),
                value: HirExpr::Call {
                    callee: "Point".to_string(),
                    args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
                },
            }),
            HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::AttrGet {
                base: Box::new(HirExpr::Name("p".to_string())),
                attr: "z".to_string(),
            })),
        ]);
        let _ = build(&hir);
    }

    #[test]
    #[should_panic(expected = "method `fly` not declared on class `Point`")]
    fn method_call_of_an_undeclared_method_panics_with_an_internal_error() {
        let hir = point_module(vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "p".to_string(),
                value: HirExpr::Call {
                    callee: "Point".to_string(),
                    args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
                },
            }),
            HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::MethodCall {
                base: Box::new(HirExpr::Name("p".to_string())),
                method: "fly".to_string(),
                args: vec![],
            })),
        ]);
        let _ = build(&hir);
    }

    #[test]
    #[should_panic(expected = "attribute `z` not declared on class `Point`")]
    fn attr_set_of_an_undeclared_attribute_panics_with_an_internal_error() {
        let hir = point_module(vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "p".to_string(),
                value: HirExpr::Call {
                    callee: "Point".to_string(),
                    args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
                },
            }),
            HirItem::TopLevelStmt(HirStmt::AttrSet {
                base: HirExpr::Name("p".to_string()),
                attr: "z".to_string(),
                value: HirExpr::IntLiteral(1),
            }),
        ]);
        let _ = build(&hir);
    }

    // #377: a read-only `@property` (no setter) should never reach MIR
    // lowering -- `pycc_types::check` rejects the assignment with `T0044`
    // before MIR runs. This test bypasses the type checker with a hand-built
    // HIR to exercise the panic arm, matching this file's own established
    // internal-error-test convention (see e.g.
    // `attr_set_of_an_undeclared_attribute_panics_with_an_internal_error`
    // above).
    #[test]
    #[should_panic(
        expected = "pycc_mir: internal error: property `val` on class `Box` has no setter"
    )]
    fn attr_set_on_a_read_only_property_panics_with_an_internal_error() {
        use pycc_hir::{HirClassDef, PropertyDef};
        let self_ty = Ty::Instance(Box::new("Box".to_string()));
        let init = HirItem::Function {
            name: "Box.__init__".to_string(),
            params: vec![("self".to_string(), self_ty.clone())],
            return_ty: Ty::None,
            body: vec![
                HirStmt::AttrSet {
                    base: HirExpr::Name("self".to_string()),
                    attr: "_val".to_string(),
                    value: HirExpr::IntLiteral(0),
                },
                HirStmt::Return(None),
            ],
        };
        let getter = HirItem::Function {
            name: "Box.val".to_string(),
            params: vec![("self".to_string(), self_ty.clone())],
            return_ty: Ty::Int,
            body: vec![HirStmt::Return(Some(HirExpr::AttrGet {
                base: Box::new(HirExpr::Name("self".to_string())),
                attr: "_val".to_string(),
            }))],
        };
        let hir = HirModule {
            items: vec![
                init,
                getter,
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "b".to_string(),
                    value: HirExpr::Call {
                        callee: "Box".to_string(),
                        args: vec![],
                    },
                }),
                HirItem::TopLevelStmt(HirStmt::AttrSet {
                    base: HirExpr::Name("b".to_string()),
                    attr: "val".to_string(),
                    value: HirExpr::IntLiteral(42),
                }),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: vec![(
                "Box".to_string(),
                HirClassDef {
                    name: "Box".to_string(),
                    bases: Vec::new(),
                    mro: vec!["Box".to_string()],
                    attrs: vec![("_val".to_string(), Ty::Int)],
                    methods: vec![("__init__".to_string(), "Box.__init__".to_string())],
                    properties: vec![PropertyDef {
                        name: "val".to_string(),
                        getter: "Box.val".to_string(),
                        setter: None,
                    }],
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
                },
            )],
        };
        let _ = build(&hir);
    }

    // PEP 695 (#387): `GenericClassInstantiate` should never reach MIR —
    // `pycc_types::monomorphize` rewrites every such expression to an
    // ordinary `HirExpr::Call` before MIR lowering runs. This test bypasses
    // the type checker with a hand-built HIR to exercise the panic arm,
    // matching this file's own established internal-error-test convention.
    #[test]
    #[should_panic(
        expected = "pycc_mir: internal error: `GenericClassInstantiate` for class `C` reached MIR lowering"
    )]
    fn generic_class_instantiate_reaching_mir_panics_with_an_internal_error() {
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(
                HirExpr::GenericClassInstantiate {
                    class: "C".to_string(),
                    type_arg: Ty::Int,
                    args: vec![HirExpr::IntLiteral(1)],
                },
            ))],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let _ = build(&hir);
    }

    // #433: a bare `HirExpr::Super` should never reach MIR lowering —
    // HIR lowering rejects a standalone `super()` with C0001, and
    // `super().method()`/`super().attr` are handled by the special-case
    // blocks before recursing into `lower_expr` for the base. This test
    // bypasses the type checker with a hand-built HIR to exercise the
    // panic arm, matching this file's own established internal-error-test
    // convention.
    #[test]
    #[should_panic(
        expected = "pycc_mir: internal error: a bare `HirExpr::Super` reached MIR lowering"
    )]
    fn bare_super_reaching_mir_panics_with_an_internal_error() {
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Super))],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let _ = build(&hir);
    }

    // #449: `current_class.expect(...)` panic paths in the Super-base blocks
    // of `HirExpr::AttrGet` and `HirExpr::MethodCall`. A `HirExpr::Super` as
    // the base of an `AttrGet` or `MethodCall` at top level (outside a method
    // body, where `current_class` is `None`) should never reach MIR lowering
    // — HIR lowering rejects it with C0001. These tests bypass the type
    // checker with a hand-built HIR to exercise the panic arms, matching this
    // file's own established internal-error-test convention.

    #[test]
    #[should_panic(
        expected = "pycc_mir: internal error: `HirExpr::Super` reached lower_expr outside a method body"
    )]
    fn super_attr_get_outside_method_panics_with_an_internal_error() {
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::AttrGet {
                base: Box::new(HirExpr::Super),
                attr: "x".to_string(),
            }))],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let _ = build(&hir);
    }

    #[test]
    #[should_panic(
        expected = "pycc_mir: internal error: `HirExpr::Super` reached lower_expr outside a method body"
    )]
    fn super_method_call_outside_method_panics_with_an_internal_error() {
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(
                HirExpr::MethodCall {
                    base: Box::new(HirExpr::Super),
                    method: "f".to_string(),
                    args: vec![],
                },
            ))],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let _ = build(&hir);
    }

    // -- #432: MRO internal-error panic tests --------------------------------
    //
    // Every test below bypasses `pycc_types::check` (which would reject
    // this) with a hand-built HIR whose class's MRO references a class the
    // `classes` map has no entry for -- exactly the "MRO and classes side
    // table disagree" scenario each MRO-walk's own doc comment names as
    // unreachable from any real `check`-validated program, mirroring this
    // file's own established convention for internal-consistency panics.

    /// Builds a HIR module with a `Derived` class whose MRO lists
    /// `["Derived", "Ghost"]`, where `Ghost` is deliberately absent from
    /// `class_defs`. The `body_fn` closure receives the `self`-typed
    /// parameter name and returns the function body that triggers the
    /// specific MRO walk to test -- using a function parameter (not
    /// `Instantiate`) avoids the `mro_attr_count` → `mro_attrs` panic
    /// firing first, so the `AttrGet`/`AttrSet`/`MethodCall` MRO walk
    /// panic is the one actually reached.
    fn ghost_mro_param_module(body: Vec<HirStmt>) -> HirModule {
        use pycc_hir::HirClassDef;
        let self_ty = Ty::Instance(Box::new("Derived".to_string()));
        // The `__init__` body is deliberately empty (`return` only) -- an
        // `AttrSet` on `self` would itself walk the MRO and panic on the
        // absent `Ghost` entry before the `use_derived` function body (the
        // one that actually tests `AttrGet`/`MethodCall`) is ever lowered.
        let init = HirItem::Function {
            name: "Derived.__init__".to_string(),
            params: vec![("self".to_string(), self_ty.clone())],
            return_ty: Ty::None,
            body: vec![HirStmt::Return(None)],
        };
        // A function whose parameter is typed `Derived` -- the MRO walk
        // for `AttrGet`/`AttrSet`/`MethodCall` on this parameter triggers
        // the ghost-class panic without going through `Instantiate`.
        let user_fn = HirItem::Function {
            name: "use_derived".to_string(),
            params: vec![("d".to_string(), self_ty.clone())],
            return_ty: Ty::None,
            body,
        };
        HirModule {
            items: vec![init, user_fn],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: vec![(
                "Derived".to_string(),
                HirClassDef {
                    name: "Derived".to_string(),
                    bases: vec!["Ghost".to_string()],
                    mro: vec!["Derived".to_string(), "Ghost".to_string()],
                    attrs: vec![("x".to_string(), Ty::Int)],
                    methods: vec![("__init__".to_string(), "Derived.__init__".to_string())],
                    type_param: None,
                    properties: Vec::new(),
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
                },
            )],
        }
    }

    #[test]
    #[should_panic(
        expected = "pycc_mir: internal error: class `Ghost` in MRO has no registered HirClassDef"
    )]
    fn attr_set_with_a_ghost_class_in_the_mro_panics_with_an_internal_error() {
        let hir = ghost_mro_param_module(vec![
            HirStmt::AttrSet {
                base: HirExpr::Name("d".to_string()),
                attr: "x".to_string(),
                value: HirExpr::IntLiteral(42),
            },
            HirStmt::Return(None),
        ]);
        let _ = build(&hir);
    }

    #[test]
    #[should_panic(
        expected = "pycc_mir: internal error: class `Ghost` in MRO has no registered HirClassDef"
    )]
    fn attr_get_with_a_ghost_class_in_the_mro_panics_with_an_internal_error() {
        let hir = ghost_mro_param_module(vec![
            HirStmt::ExprStmt(HirExpr::AttrGet {
                base: Box::new(HirExpr::Name("d".to_string())),
                attr: "x".to_string(),
            }),
            HirStmt::Return(None),
        ]);
        let _ = build(&hir);
    }

    #[test]
    #[should_panic(
        expected = "pycc_mir: internal error: class `Ghost` in MRO has no registered HirClassDef"
    )]
    fn method_call_with_a_ghost_class_in_the_mro_panics_with_an_internal_error() {
        let hir = ghost_mro_param_module(vec![
            HirStmt::ExprStmt(HirExpr::MethodCall {
                base: Box::new(HirExpr::Name("d".to_string())),
                method: "f".to_string(),
                args: vec![],
            }),
            HirStmt::Return(None),
        ]);
        let _ = build(&hir);
    }

    #[test]
    #[should_panic(
        expected = "pycc_mir: internal error: class `Ghost` in MRO has no registered HirClassDef"
    )]
    fn mro_attrs_with_a_ghost_class_in_the_mro_panics_with_an_internal_error() {
        // `Instantiate` calls `mro_attr_count` → `mro_attrs`, which walks
        // the full MRO. The `__init__` MRO walk (using `?`, not panicking)
        // finds `Derived.__init__` first and returns early, so
        // `mro_attr_count` is reached -- and its `mro_attrs` call panics on
        // the absent `Ghost` entry.
        use pycc_hir::HirClassDef;
        let self_ty = Ty::Instance(Box::new("Derived".to_string()));
        let init = HirItem::Function {
            name: "Derived.__init__".to_string(),
            params: vec![("self".to_string(), self_ty.clone())],
            return_ty: Ty::None,
            body: vec![
                HirStmt::AttrSet {
                    base: HirExpr::Name("self".to_string()),
                    attr: "x".to_string(),
                    value: HirExpr::IntLiteral(1),
                },
                HirStmt::Return(None),
            ],
        };
        let hir = HirModule {
            items: vec![
                init,
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "d".to_string(),
                    value: HirExpr::Call {
                        callee: "Derived".to_string(),
                        args: vec![],
                    },
                }),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: vec![(
                "Derived".to_string(),
                HirClassDef {
                    name: "Derived".to_string(),
                    bases: vec!["Ghost".to_string()],
                    mro: vec!["Derived".to_string(), "Ghost".to_string()],
                    attrs: vec![("x".to_string(), Ty::Int)],
                    methods: vec![("__init__".to_string(), "Derived.__init__".to_string())],
                    type_param: None,
                    properties: Vec::new(),
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
                },
            )],
        };
        let _ = build(&hir);
    }

    #[test]
    #[should_panic(expected = "pycc_mir: internal error: no `__init__` found in class `C`'s MRO")]
    fn instantiate_with_no_init_in_the_mro_panics_with_an_internal_error() {
        // #432: `Instantiate`'s `__init__` MRO walk uses `?` (not
        // panicking) when a class is not in the `classes` map. Including
        // `Ghost` in the MRO (but not in `class_defs`) exercises that `?`
        // arm -- `C` is found but has no `__init__`, then `Ghost` is not
        // found, so `find_map` returns `None` and the `unwrap_or_else`
        // panic fires.
        use pycc_hir::HirClassDef;
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::Assign {
                target: "c".to_string(),
                value: HirExpr::Call {
                    callee: "C".to_string(),
                    args: vec![],
                },
            })],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: vec![(
                "C".to_string(),
                HirClassDef {
                    name: "C".to_string(),
                    bases: vec!["Ghost".to_string()],
                    mro: vec!["C".to_string(), "Ghost".to_string()],
                    attrs: vec![],
                    methods: vec![("f".to_string(), "C.f".to_string())],
                    type_param: None,
                    properties: Vec::new(),
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
                },
            )],
        };
        let _ = build(&hir);
    }

    #[test]
    fn mro_attrs_deduplicates_a_redeclared_attribute_across_the_mro() {
        // #432: a derived class that re-declares an attribute of the same
        // name as a base class "wins" (its declaration appears first in the
        // MRO). `mro_attrs`'s `seen` set skips the base class's duplicate
        // entry, so the flat layout has exactly one slot for `x`, not two.
        // This exercises the `if seen.insert(..)` false branch (the
        // already-seen skip), which is otherwise uncovered when no test
        // re-declares an attribute across the MRO.
        use pycc_hir::HirClassDef;
        let self_ty = Ty::Instance(Box::new("Derived".to_string()));
        let init = HirItem::Function {
            name: "Derived.__init__".to_string(),
            params: vec![("self".to_string(), self_ty.clone())],
            return_ty: Ty::None,
            body: vec![
                HirStmt::AttrSet {
                    base: HirExpr::Name("self".to_string()),
                    attr: "x".to_string(),
                    value: HirExpr::IntLiteral(1),
                },
                HirStmt::Return(None),
            ],
        };
        let base_init = HirItem::Function {
            name: "Base.__init__".to_string(),
            params: vec![(
                "self".to_string(),
                Ty::Instance(Box::new("Base".to_string())),
            )],
            return_ty: Ty::None,
            body: vec![
                HirStmt::AttrSet {
                    base: HirExpr::Name("self".to_string()),
                    attr: "x".to_string(),
                    value: HirExpr::IntLiteral(0),
                },
                HirStmt::Return(None),
            ],
        };
        let hir = HirModule {
            items: vec![
                base_init,
                init,
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "d".to_string(),
                    value: HirExpr::Call {
                        callee: "Derived".to_string(),
                        args: vec![],
                    },
                }),
                // AttrGet on `d.x` triggers `mro_attrs`, which walks the
                // MRO and deduplicates `x` (declared in both `Derived`
                // and `Base`).
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::AttrGet {
                    base: Box::new(HirExpr::Name("d".to_string())),
                    attr: "x".to_string(),
                })),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: vec![
                (
                    "Base".to_string(),
                    HirClassDef {
                        name: "Base".to_string(),
                        bases: Vec::new(),
                        mro: vec!["Base".to_string()],
                        attrs: vec![("x".to_string(), Ty::Int)],
                        methods: vec![("__init__".to_string(), "Base.__init__".to_string())],
                        type_param: None,
                        properties: Vec::new(),
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
                    },
                ),
                (
                    "Derived".to_string(),
                    HirClassDef {
                        name: "Derived".to_string(),
                        bases: vec!["Base".to_string()],
                        mro: vec!["Derived".to_string(), "Base".to_string()],
                        attrs: vec![("x".to_string(), Ty::Int)],
                        methods: vec![("__init__".to_string(), "Derived.__init__".to_string())],
                        type_param: None,
                        properties: Vec::new(),
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
                    },
                ),
            ],
        };
        let mir = build(&hir);
        // The Instantiate node should allocate exactly 1 slot (not 2),
        // because `x` is deduplicated across the MRO.
        let instantiate = mir
            .items
            .iter()
            .find_map(|item| match item {
                MirItem::TopLevelStmt(MirStmt::Assign {
                    value: MirExpr::Instantiate(inst),
                    ..
                }) => Some(inst),
                _ => None,
            })
            .expect("expected an Instantiate node");
        assert_eq!(instantiate.attr_count, 1);
    }

    #[test]
    fn mro_attrs_overrides_type_for_a_redeclared_attribute_with_a_different_type() {
        // #432: when a derived class re-declares an attribute with a
        // different type than the base, the most-derived declaration's
        // type wins (pass 2 of `mro_attrs`). This exercises the
        // `result[idx].1 = ty.clone()` line in the second pass.
        use pycc_hir::HirClassDef;
        let self_ty = Ty::Instance(Box::new("Derived".to_string()));
        let init = HirItem::Function {
            name: "Derived.__init__".to_string(),
            params: vec![("self".to_string(), self_ty.clone())],
            return_ty: Ty::None,
            body: vec![
                HirStmt::AttrSet {
                    base: HirExpr::Name("self".to_string()),
                    attr: "x".to_string(),
                    value: HirExpr::FloatLiteral(1.0),
                },
                HirStmt::Return(None),
            ],
        };
        let base_init = HirItem::Function {
            name: "Base.__init__".to_string(),
            params: vec![(
                "self".to_string(),
                Ty::Instance(Box::new("Base".to_string())),
            )],
            return_ty: Ty::None,
            body: vec![
                HirStmt::AttrSet {
                    base: HirExpr::Name("self".to_string()),
                    attr: "x".to_string(),
                    value: HirExpr::IntLiteral(0),
                },
                HirStmt::Return(None),
            ],
        };
        let hir = HirModule {
            items: vec![
                base_init,
                init,
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "d".to_string(),
                    value: HirExpr::Call {
                        callee: "Derived".to_string(),
                        args: vec![],
                    },
                }),
                // AttrGet on `d.x` triggers `mro_attrs`, which walks the
                // MRO and overrides `x`'s type from Int (Base) to Float
                // (Derived) in the second pass.
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::AttrGet {
                    base: Box::new(HirExpr::Name("d".to_string())),
                    attr: "x".to_string(),
                })),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: vec![
                (
                    "Base".to_string(),
                    HirClassDef {
                        name: "Base".to_string(),
                        bases: Vec::new(),
                        mro: vec!["Base".to_string()],
                        attrs: vec![("x".to_string(), Ty::Int)],
                        methods: vec![("__init__".to_string(), "Base.__init__".to_string())],
                        type_param: None,
                        properties: Vec::new(),
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
                    },
                ),
                (
                    "Derived".to_string(),
                    HirClassDef {
                        name: "Derived".to_string(),
                        bases: vec!["Base".to_string()],
                        mro: vec!["Derived".to_string(), "Base".to_string()],
                        attrs: vec![("x".to_string(), Ty::Float)],
                        methods: vec![("__init__".to_string(), "Derived.__init__".to_string())],
                        type_param: None,
                        properties: Vec::new(),
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
                    },
                ),
            ],
        };
        let mir = build(&hir);
        // The AttrGet node should have type Float (Derived's declaration
        // wins), not Int (Base's declaration).
        let attr_get = mir
            .items
            .iter()
            .find_map(|item| match item {
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::AttrGet { ty, .. })) => {
                    Some(ty.clone())
                }
                _ => None,
            })
            .expect("expected an AttrGet node");
        assert_eq!(
            attr_get,
            Ty::Float,
            "re-declared attribute should use the most-derived type (Float)"
        );
    }

    // #433: super() MIR lowering tests.

    /// Helper: builds a minimal two-class HIR module where `B.__init__`
    /// calls `super().__init__()` and `B.greet` calls `super().greet()`.
    fn super_module() -> HirModule {
        let self_a = Ty::Instance(Box::new("A".to_string()));
        let self_b = Ty::Instance(Box::new("B".to_string()));
        HirModule {
            items: vec![
                // A.__init__
                HirItem::Function {
                    name: "A.__init__".to_string(),
                    params: vec![("self".to_string(), self_a.clone())],
                    return_ty: Ty::None,
                    body: vec![HirStmt::AttrSet {
                        base: HirExpr::Name("self".to_string()),
                        attr: "x".to_string(),
                        value: HirExpr::IntLiteral(1),
                    }],
                },
                // A.greet
                HirItem::Function {
                    name: "A.greet".to_string(),
                    params: vec![("self".to_string(), self_a.clone())],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::AttrGet {
                        base: Box::new(HirExpr::Name("self".to_string())),
                        attr: "x".to_string(),
                    }))],
                },
                // B.__init__ — calls super().__init__()
                HirItem::Function {
                    name: "B.__init__".to_string(),
                    params: vec![("self".to_string(), self_b.clone())],
                    return_ty: Ty::None,
                    body: vec![HirStmt::ExprStmt(HirExpr::MethodCall {
                        base: Box::new(HirExpr::Super),
                        method: "__init__".to_string(),
                        args: vec![],
                    })],
                },
                // B.greet — calls super().greet()
                HirItem::Function {
                    name: "B.greet".to_string(),
                    params: vec![("self".to_string(), self_b)],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::MethodCall {
                        base: Box::new(HirExpr::Super),
                        method: "greet".to_string(),
                        args: vec![],
                    }))],
                },
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: vec![
                (
                    "A".to_string(),
                    HirClassDef {
                        name: "A".to_string(),
                        bases: vec![],
                        mro: vec!["A".to_string()],
                        attrs: vec![("x".to_string(), Ty::Int)],
                        methods: vec![
                            ("__init__".to_string(), "A.__init__".to_string()),
                            ("greet".to_string(), "A.greet".to_string()),
                        ],
                        type_param: None,
                        properties: Vec::new(),
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
                    },
                ),
                (
                    "B".to_string(),
                    HirClassDef {
                        name: "B".to_string(),
                        bases: vec!["A".to_string()],
                        mro: vec!["B".to_string(), "A".to_string()],
                        attrs: Vec::new(),
                        methods: vec![
                            ("__init__".to_string(), "B.__init__".to_string()),
                            ("greet".to_string(), "B.greet".to_string()),
                        ],
                        type_param: None,
                        properties: Vec::new(),
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
                    },
                ),
            ],
        }
    }

    #[test]
    fn super_init_lowers_to_direct_call_to_base_init() {
        let hir = super_module();
        let mir = build(&hir);
        let init = mir.items.iter().find_map(|item| match item {
            MirItem::Function { name, body, .. } if name == "B.__init__" => body.first(),
            _ => None,
        });
        assert_eq!(
            init,
            Some(&MirStmt::ExprStmt(MirExpr::Call {
                callee: "A.__init__".to_string(),
                args: vec![MirExpr::Name {
                    name: "self".to_string(),
                    ty: Ty::Instance(Box::new("B".to_string())),
                }],
                ty: Ty::None,
            }))
        );
    }

    #[test]
    fn super_method_lowers_to_direct_call_to_base_method() {
        let hir = super_module();
        let mir = build(&hir);
        let greet = mir.items.iter().find_map(|item| match item {
            MirItem::Function { name, body, .. } if name == "B.greet" => body.first(),
            _ => None,
        });
        assert_eq!(
            greet,
            Some(&MirStmt::Return(Some(MirExpr::Call {
                callee: "A.greet".to_string(),
                args: vec![MirExpr::Name {
                    name: "self".to_string(),
                    ty: Ty::Instance(Box::new("B".to_string())),
                }],
                ty: Ty::Int,
            })))
        );
    }

    #[test]
    fn super_attr_lowers_to_attr_get_with_self_base() {
        let self_a = Ty::Instance(Box::new("A".to_string()));
        let self_b = Ty::Instance(Box::new("B".to_string()));
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "A.__init__".to_string(),
                    params: vec![("self".to_string(), self_a)],
                    return_ty: Ty::None,
                    body: vec![HirStmt::AttrSet {
                        base: HirExpr::Name("self".to_string()),
                        attr: "x".to_string(),
                        value: HirExpr::IntLiteral(42),
                    }],
                },
                HirItem::Function {
                    name: "B.get_x".to_string(),
                    params: vec![("self".to_string(), self_b)],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::AttrGet {
                        base: Box::new(HirExpr::Super),
                        attr: "x".to_string(),
                    }))],
                },
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: vec![
                (
                    "A".to_string(),
                    HirClassDef {
                        name: "A".to_string(),
                        bases: vec![],
                        mro: vec!["A".to_string()],
                        attrs: vec![("x".to_string(), Ty::Int)],
                        methods: vec![("__init__".to_string(), "A.__init__".to_string())],
                        type_param: None,
                        properties: Vec::new(),
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
                    },
                ),
                (
                    "B".to_string(),
                    HirClassDef {
                        name: "B".to_string(),
                        bases: vec!["A".to_string()],
                        mro: vec!["B".to_string(), "A".to_string()],
                        attrs: Vec::new(),
                        methods: vec![("get_x".to_string(), "B.get_x".to_string())],
                        type_param: None,
                        properties: Vec::new(),
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
                    },
                ),
            ],
        };
        let mir = build(&hir);
        let get_x = mir.items.iter().find_map(|item| match item {
            MirItem::Function { name, body, .. } if name == "B.get_x" => body.first(),
            _ => None,
        });
        assert_eq!(
            get_x,
            Some(&MirStmt::Return(Some(MirExpr::AttrGet {
                base: Box::new(MirExpr::Name {
                    name: "self".to_string(),
                    ty: Ty::Instance(Box::new("B".to_string())),
                }),
                slot: 0,
                ty: Ty::Int,
            })))
        );
    }

    #[test]
    fn super_property_lowers_to_call_to_base_getter() {
        use pycc_hir::PropertyDef;
        let self_a = Ty::Instance(Box::new("A".to_string()));
        let self_b = Ty::Instance(Box::new("B".to_string()));
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "A.__init__".to_string(),
                    params: vec![("self".to_string(), self_a.clone())],
                    return_ty: Ty::None,
                    body: vec![
                        HirStmt::AttrSet {
                            base: HirExpr::Name("self".to_string()),
                            attr: "_val".to_string(),
                            value: HirExpr::IntLiteral(0),
                        },
                        HirStmt::Return(None),
                    ],
                },
                HirItem::Function {
                    name: "A.val".to_string(),
                    params: vec![("self".to_string(), self_a)],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::AttrGet {
                        base: Box::new(HirExpr::Name("self".to_string())),
                        attr: "_val".to_string(),
                    }))],
                },
                HirItem::Function {
                    name: "B.get_val".to_string(),
                    params: vec![("self".to_string(), self_b)],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::AttrGet {
                        base: Box::new(HirExpr::Super),
                        attr: "val".to_string(),
                    }))],
                },
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: vec![
                (
                    "A".to_string(),
                    HirClassDef {
                        name: "A".to_string(),
                        bases: vec![],
                        mro: vec!["A".to_string()],
                        attrs: vec![("_val".to_string(), Ty::Int)],
                        methods: vec![("__init__".to_string(), "A.__init__".to_string())],
                        type_param: None,
                        properties: vec![PropertyDef {
                            name: "val".to_string(),
                            getter: "A.val".to_string(),
                            setter: None,
                        }],
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
                    },
                ),
                (
                    "B".to_string(),
                    HirClassDef {
                        name: "B".to_string(),
                        bases: vec!["A".to_string()],
                        mro: vec!["B".to_string(), "A".to_string()],
                        attrs: Vec::new(),
                        methods: vec![("get_val".to_string(), "B.get_val".to_string())],
                        type_param: None,
                        properties: Vec::new(),
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
                    },
                ),
            ],
        };
        let mir = build(&hir);
        let get_val = mir.items.iter().find_map(|item| match item {
            MirItem::Function { name, body, .. } if name == "B.get_val" => body.first(),
            _ => None,
        });
        assert_eq!(
            get_val,
            Some(&MirStmt::Return(Some(MirExpr::Call {
                callee: "A.val".to_string(),
                args: vec![MirExpr::Name {
                    name: "self".to_string(),
                    ty: Ty::Instance(Box::new("B".to_string())),
                }],
                ty: Ty::Int,
            })))
        );
    }

    // -- #436: @staticmethod / @classmethod MIR lowering --------------------

    /// Builds a minimal module with a class `C` that has `__init__`, a
    /// static method `create(x: int) -> int`, and a class method
    /// `greet(cls, x: int) -> int`. Used by the #436 MIR tests below.
    fn static_class_hir(extra_items: Vec<HirItem>) -> HirModule {
        let self_ty = Ty::Instance(Box::new("C".to_string()));
        let init = HirItem::Function {
            name: "C.__init__".to_string(),
            params: vec![("self".to_string(), self_ty.clone())],
            return_ty: Ty::None,
            body: vec![HirStmt::Return(None)],
        };
        let static_fn = HirItem::Function {
            name: "C.create.static".to_string(),
            params: vec![("x".to_string(), Ty::Int)],
            return_ty: Ty::Int,
            body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
        };
        let class_fn = HirItem::Function {
            name: "C.greet.classmethod".to_string(),
            params: vec![
                ("cls".to_string(), self_ty.clone()),
                ("x".to_string(), Ty::Int),
            ],
            return_ty: Ty::Int,
            body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
        };
        let mut items = vec![init, static_fn, class_fn];
        items.extend(extra_items);
        HirModule {
            items,
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: vec![(
                "C".to_string(),
                pycc_hir::HirClassDef {
                    name: "C".to_string(),
                    bases: Vec::new(),
                    mro: vec!["C".to_string()],
                    attrs: Vec::new(),
                    methods: vec![("__init__".to_string(), "C.__init__".to_string())],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: vec![("create".to_string(), "C.create.static".to_string())],
                    class_methods: vec![("greet".to_string(), "C.greet.classmethod".to_string())],
                    enum_members: Vec::new(),
                    is_dataclass: false,
                    dataclass_fields: Vec::new(),
                    is_protocol: false,
                    runtime_checkable: false,
                    protocol_members: Vec::new(),
                    abstract_methods: Vec::new(),
                    is_abstract: false,
                },
            )],
        }
    }

    #[test]
    fn a_static_method_call_through_class_lowers_without_a_receiver() {
        let hir = static_class_hir(vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(
            HirExpr::MethodCall {
                base: Box::new(HirExpr::Name("C".to_string())),
                method: "create".to_string(),
                args: vec![HirExpr::IntLiteral(42)],
            },
        ))]);
        let mir = build(&hir);
        assert_eq!(
            mir.items.last(),
            Some(&MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                callee: "C.create.static".to_string(),
                args: vec![MirExpr::IntLiteral(42)],
                ty: Ty::Int,
            })))
        );
    }

    #[test]
    fn a_class_method_call_through_class_lowers_with_null_instance_receiver() {
        let hir = static_class_hir(vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(
            HirExpr::MethodCall {
                base: Box::new(HirExpr::Name("C".to_string())),
                method: "greet".to_string(),
                args: vec![HirExpr::IntLiteral(42)],
            },
        ))]);
        let mir = build(&hir);
        assert_eq!(
            mir.items.last(),
            Some(&MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                callee: "C.greet.classmethod".to_string(),
                args: vec![
                    MirExpr::NullInstance {
                        ty: Ty::Instance(Box::new("C".to_string())),
                    },
                    MirExpr::IntLiteral(42),
                ],
                ty: Ty::Int,
            })))
        );
    }

    #[test]
    fn a_static_method_call_through_instance_lowers_without_a_receiver() {
        let hir = static_class_hir(vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "c".to_string(),
                value: HirExpr::Call {
                    callee: "C".to_string(),
                    args: vec![],
                },
            }),
            HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::MethodCall {
                base: Box::new(HirExpr::Name("c".to_string())),
                method: "create".to_string(),
                args: vec![HirExpr::IntLiteral(42)],
            })),
        ]);
        let mir = build(&hir);
        assert_eq!(
            mir.items.last(),
            Some(&MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                callee: "C.create.static".to_string(),
                args: vec![MirExpr::IntLiteral(42)],
                ty: Ty::Int,
            })))
        );
    }

    #[test]
    fn a_class_method_call_through_instance_lowers_with_instance_as_cls() {
        let hir = static_class_hir(vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "c".to_string(),
                value: HirExpr::Call {
                    callee: "C".to_string(),
                    args: vec![],
                },
            }),
            HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::MethodCall {
                base: Box::new(HirExpr::Name("c".to_string())),
                method: "greet".to_string(),
                args: vec![HirExpr::IntLiteral(42)],
            })),
        ]);
        let mir = build(&hir);
        assert_eq!(
            mir.items.last(),
            Some(&MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                callee: "C.greet.classmethod".to_string(),
                args: vec![
                    MirExpr::Name {
                        name: "c".to_string(),
                        ty: Ty::Instance(Box::new("C".to_string())),
                    },
                    MirExpr::IntLiteral(42),
                ],
                ty: Ty::Int,
            })))
        );
    }

    #[test]
    fn null_instance_ty_returns_the_stored_type() {
        let expr = MirExpr::NullInstance {
            ty: Ty::Instance(Box::new("C".to_string())),
        };
        assert_eq!(expr.ty(), Ty::Instance(Box::new("C".to_string())));
    }

    #[test]
    #[should_panic(expected = "pycc_mir: internal error: `C` has no recorded type")]
    fn class_name_method_call_with_no_static_or_class_method_falls_through_to_instance_path() {
        // When a `MethodCall` on a class name does not find the method in
        // `static_methods` or `class_methods`, the code falls through to the
        // instance-receiver path, which calls `lower_expr` on the bare class
        // name. Since class names are not in the scope, `lookup` panics.
        // This covers the `None` branch of the class-name `class_mangled`
        // `if let` (the fallthrough past the class-name interception block).
        let hir = static_class_hir(vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(
            HirExpr::MethodCall {
                base: Box::new(HirExpr::Name("C".to_string())),
                method: "nonexistent".to_string(),
                args: vec![],
            },
        ))]);
        let _ = build(&hir);
    }

    #[test]
    #[should_panic(
        expected = "pycc_mir: internal error: class `Ghost` in MRO has no registered HirClassDef"
    )]
    fn static_method_call_through_class_name_with_ghost_mro_panics() {
        // A `MethodCall` on a class name whose MRO contains a ghost class
        // triggers the defensive panic in the static_methods MRO walk. The
        // method name `missing` is not in `C`'s own static_methods, so the
        // `find_map` continues to `Ghost` and panics.
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(
                HirExpr::MethodCall {
                    base: Box::new(HirExpr::Name("C".to_string())),
                    method: "missing".to_string(),
                    args: vec![],
                },
            ))],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: vec![(
                "C".to_string(),
                pycc_hir::HirClassDef {
                    name: "C".to_string(),
                    bases: vec!["Ghost".to_string()],
                    mro: vec!["C".to_string(), "Ghost".to_string()],
                    attrs: Vec::new(),
                    methods: vec![("__init__".to_string(), "C.__init__".to_string())],
                    type_param: None,
                    properties: Vec::new(),
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
                },
            )],
        };
        let _ = build(&hir);
    }

    #[test]
    #[should_panic(
        expected = "pycc_mir: internal error: class `Ghost` in MRO has no registered HirClassDef"
    )]
    fn class_method_call_through_class_name_with_ghost_mro_panics() {
        // Same as above but exercises the class_methods MRO walk. The
        // method name `missing` is not in `C`'s own static_methods or
        // class_methods, so both walks reach `Ghost` and the second one
        // (class_methods) panics.
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(
                HirExpr::MethodCall {
                    base: Box::new(HirExpr::Name("C".to_string())),
                    method: "missing".to_string(),
                    args: vec![],
                },
            ))],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: vec![(
                "C".to_string(),
                pycc_hir::HirClassDef {
                    name: "C".to_string(),
                    bases: vec!["Ghost".to_string()],
                    mro: vec!["C".to_string(), "Ghost".to_string()],
                    attrs: Vec::new(),
                    methods: vec![("__init__".to_string(), "C.__init__".to_string())],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: vec![("found".to_string(), "C.found.static".to_string())],
                    class_methods: Vec::new(),
                    enum_members: Vec::new(),
                    is_dataclass: false,
                    dataclass_fields: Vec::new(),
                    is_protocol: false,
                    runtime_checkable: false,
                    protocol_members: Vec::new(),
                    abstract_methods: Vec::new(),
                    is_abstract: false,
                },
            )],
        };
        let _ = build(&hir);
    }

    #[test]
    #[should_panic(
        expected = "pycc_mir: internal error: class `Ghost` in MRO has no registered HirClassDef"
    )]
    fn static_method_call_through_instance_with_ghost_mro_panics() {
        // A `MethodCall` on an instance whose class MRO contains a ghost
        // class triggers the defensive panic in the instance-receiver
        // static_methods MRO walk.
        let self_ty = Ty::Instance(Box::new("Derived".to_string()));
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "Derived.__init__".to_string(),
                    params: vec![("self".to_string(), self_ty.clone())],
                    return_ty: Ty::None,
                    body: vec![HirStmt::Return(None)],
                },
                HirItem::Function {
                    name: "use_derived".to_string(),
                    params: vec![("d".to_string(), self_ty.clone())],
                    return_ty: Ty::None,
                    body: vec![HirStmt::ExprStmt(HirExpr::MethodCall {
                        base: Box::new(HirExpr::Name("d".to_string())),
                        method: "missing".to_string(),
                        args: vec![],
                    })],
                },
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: vec![(
                "Derived".to_string(),
                pycc_hir::HirClassDef {
                    name: "Derived".to_string(),
                    bases: vec!["Ghost".to_string()],
                    mro: vec!["Derived".to_string(), "Ghost".to_string()],
                    attrs: Vec::new(),
                    methods: vec![("__init__".to_string(), "Derived.__init__".to_string())],
                    type_param: None,
                    properties: Vec::new(),
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
                },
            )],
        };
        let _ = build(&hir);
    }

    #[test]
    #[should_panic(
        expected = "pycc_mir: internal error: class `Ghost` in MRO has no registered HirClassDef"
    )]
    fn class_method_call_through_instance_with_ghost_mro_panics() {
        // Same as above but exercises the instance-receiver class_methods
        // MRO walk. `Derived` has a static method `found` so the
        // static_methods walk does not reach `Ghost`, but the class_methods
        // walk does.
        let self_ty = Ty::Instance(Box::new("Derived".to_string()));
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "Derived.__init__".to_string(),
                    params: vec![("self".to_string(), self_ty.clone())],
                    return_ty: Ty::None,
                    body: vec![HirStmt::Return(None)],
                },
                HirItem::Function {
                    name: "use_derived".to_string(),
                    params: vec![("d".to_string(), self_ty.clone())],
                    return_ty: Ty::None,
                    body: vec![HirStmt::ExprStmt(HirExpr::MethodCall {
                        base: Box::new(HirExpr::Name("d".to_string())),
                        method: "missing".to_string(),
                        args: vec![],
                    })],
                },
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: vec![(
                "Derived".to_string(),
                pycc_hir::HirClassDef {
                    name: "Derived".to_string(),
                    bases: vec!["Ghost".to_string()],
                    mro: vec!["Derived".to_string(), "Ghost".to_string()],
                    attrs: Vec::new(),
                    methods: vec![("__init__".to_string(), "Derived.__init__".to_string())],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: vec![("found".to_string(), "Derived.found.static".to_string())],
                    class_methods: Vec::new(),
                    enum_members: Vec::new(),
                    is_dataclass: false,
                    dataclass_fields: Vec::new(),
                    is_protocol: false,
                    runtime_checkable: false,
                    protocol_members: Vec::new(),
                    abstract_methods: Vec::new(),
                    is_abstract: false,
                },
            )],
        };
        let _ = build(&hir);
    }

    // -----------------------------------------------------------------------
    // #435: isinstance/issubclass MIR lowering unit tests
    // -----------------------------------------------------------------------

    #[test]
    fn isinstance_lowers_to_bool_literal_for_user_class() {
        // `isinstance(D(), D)` — D is in D's MRO, so the result is `true`.
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::Call {
                    callee: "isinstance".to_string(),
                    args: vec![
                        HirExpr::Call {
                            callee: "D".to_string(),
                            args: vec![],
                        },
                        HirExpr::Name("D".to_string()),
                    ],
                }],
            }))],
            type_aliases: vec![],
            imports: vec![],
            class_defs: vec![(
                "D".to_string(),
                HirClassDef {
                    name: "D".to_string(),
                    bases: vec![],
                    mro: vec!["D".to_string()],
                    methods: vec![("__init__".to_string(), "D.__init__".to_string())],
                    attrs: vec![],
                    static_methods: vec![],
                    class_methods: vec![],
                    properties: vec![],
                    type_param: None,
                    enum_members: Vec::new(),
                    is_dataclass: false,
                    dataclass_fields: Vec::new(),
                    is_protocol: false,
                    runtime_checkable: false,
                    protocol_members: Vec::new(),
                    abstract_methods: Vec::new(),
                    is_abstract: false,
                },
            )],
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items[0],
            MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                callee: "print".to_string(),
                args: vec![MirExpr::BoolLiteral(true)],
                ty: Ty::None,
            }))
        );
    }

    #[test]
    fn issubclass_lowers_to_bool_literal_for_same_class() {
        // `issubclass(D, D)` — D is in D's MRO, so the result is `true`.
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::Call {
                    callee: "issubclass".to_string(),
                    args: vec![
                        HirExpr::Name("D".to_string()),
                        HirExpr::Name("D".to_string()),
                    ],
                }],
            }))],
            type_aliases: vec![],
            imports: vec![],
            class_defs: vec![(
                "D".to_string(),
                HirClassDef {
                    name: "D".to_string(),
                    bases: vec![],
                    mro: vec!["D".to_string()],
                    methods: vec![("__init__".to_string(), "D.__init__".to_string())],
                    attrs: vec![],
                    static_methods: vec![],
                    class_methods: vec![],
                    properties: vec![],
                    type_param: None,
                    enum_members: Vec::new(),
                    is_dataclass: false,
                    dataclass_fields: Vec::new(),
                    is_protocol: false,
                    runtime_checkable: false,
                    protocol_members: Vec::new(),
                    abstract_methods: Vec::new(),
                    is_abstract: false,
                },
            )],
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items[0],
            MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                callee: "print".to_string(),
                args: vec![MirExpr::BoolLiteral(true)],
                ty: Ty::None,
            }))
        );
    }

    #[test]
    fn isinstance_with_float_lowers_to_bool_literal_true() {
        // `isinstance(1.5, float)` — covers the `Ty::Float` arm in
        // `eval_isinstance_single` at the MIR level.
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::Call {
                    callee: "isinstance".to_string(),
                    args: vec![
                        HirExpr::FloatLiteral(1.5),
                        HirExpr::Name("float".to_string()),
                    ],
                }],
            }))],
            type_aliases: vec![],
            imports: vec![],
            class_defs: vec![],
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items[0],
            MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                callee: "print".to_string(),
                args: vec![MirExpr::BoolLiteral(true)],
                ty: Ty::None,
            }))
        );
    }

    #[test]
    fn issubclass_with_int_int_lowers_to_bool_literal_true() {
        // `issubclass(int, int)` — covers the `return cls == target_class`
        // line in `eval_issubclass_single` at the MIR level.
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::Call {
                    callee: "issubclass".to_string(),
                    args: vec![
                        HirExpr::Name("int".to_string()),
                        HirExpr::Name("int".to_string()),
                    ],
                }],
            }))],
            type_aliases: vec![],
            imports: vec![],
            class_defs: vec![],
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items[0],
            MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                callee: "print".to_string(),
                args: vec![MirExpr::BoolLiteral(true)],
                ty: Ty::None,
            }))
        );
    }

    #[test]
    fn issubclass_with_user_class_vs_builtin_lowers_to_false() {
        // `issubclass(D, int)` — user class vs builtin target, covers the
        // `return false` line in `eval_issubclass_single`.
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::Call {
                    callee: "issubclass".to_string(),
                    args: vec![
                        HirExpr::Name("D".to_string()),
                        HirExpr::Name("int".to_string()),
                    ],
                }],
            }))],
            type_aliases: vec![],
            imports: vec![],
            class_defs: vec![(
                "D".to_string(),
                HirClassDef {
                    name: "D".to_string(),
                    bases: vec![],
                    mro: vec!["D".to_string()],
                    methods: vec![("__init__".to_string(), "D.__init__".to_string())],
                    attrs: vec![],
                    static_methods: vec![],
                    class_methods: vec![],
                    properties: vec![],
                    type_param: None,
                    enum_members: Vec::new(),
                    is_dataclass: false,
                    dataclass_fields: Vec::new(),
                    is_protocol: false,
                    runtime_checkable: false,
                    protocol_members: Vec::new(),
                    abstract_methods: Vec::new(),
                    is_abstract: false,
                },
            )],
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items[0],
            MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                callee: "print".to_string(),
                args: vec![MirExpr::BoolLiteral(false)],
                ty: Ty::None,
            }))
        );
    }

    #[test]
    #[should_panic(expected = "internal error: issubclass's first argument")]
    fn issubclass_with_non_name_first_arg_panics() {
        // This covers the `unreachable!` branch in `lower_issubclass`.
        // In practice, the type checker rejects this before MIR lowering,
        // but the MIR function must handle the case defensively.
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::Call {
                    callee: "issubclass".to_string(),
                    args: vec![HirExpr::IntLiteral(42), HirExpr::Name("int".to_string())],
                }],
            }))],
            type_aliases: vec![],
            imports: vec![],
            class_defs: vec![],
        };
        let _ = build(&hir);
    }

    #[test]
    fn enum_member_attr_get_lowers_to_synthetic_global() {
        // #379: `Color.RED` lowers to `MirExpr::Name` reading the
        // synthetic `<Class>.<Member>.enum_member` global.
        let class_def = pycc_hir::HirClassDef {
            name: "Color".to_string(),
            bases: vec![],
            mro: vec!["Color".to_string()],
            attrs: vec![
                ("value".to_string(), Ty::Int),
                ("name".to_string(), Ty::Str),
            ],
            methods: vec![],
            properties: vec![],
            static_methods: vec![],
            class_methods: vec![],
            type_param: None,
            enum_members: vec![("RED".to_string(), 1), ("GREEN".to_string(), 2)],
            is_dataclass: false,
            dataclass_fields: Vec::new(),
            is_protocol: false,
            runtime_checkable: false,
            protocol_members: Vec::new(),
            abstract_methods: Vec::new(),
            is_abstract: false,
        };
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::AttrGet {
                    base: Box::new(HirExpr::Name("Color".to_string())),
                    attr: "RED".to_string(),
                }],
            }))],
            type_aliases: vec![],
            imports: vec![],
            class_defs: vec![("Color".to_string(), class_def)],
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items[0],
            MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                callee: "print".to_string(),
                args: vec![MirExpr::Name {
                    name: "Color.RED.enum_member".to_string(),
                    ty: Ty::Instance(Box::new("Color".to_string())),
                }],
                ty: Ty::None,
            }))
        );
    }

    // -- #378 (PR-18): dataclass __eq__ / __repr__ MIR rewrites -----------

    /// A minimal dataclass-like `Point` module with `__init__`, `__eq__`,
    /// and `__repr__` methods registered in the class definition. Used by
    /// the dataclass MIR-rewrite tests below.
    fn dataclass_point_module(extra_items: Vec<HirItem>) -> HirModule {
        let self_ty = Ty::Instance(Box::new("Point".to_string()));
        let init = HirItem::Function {
            name: "Point.__init__".to_string(),
            params: vec![
                ("self".to_string(), self_ty.clone()),
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
                HirStmt::Return(None),
            ],
        };
        let eq = HirItem::Function {
            name: "Point.__eq__".to_string(),
            params: vec![
                ("self".to_string(), self_ty.clone()),
                ("other".to_string(), self_ty.clone()),
            ],
            return_ty: Ty::Bool,
            body: vec![HirStmt::Return(Some(HirExpr::BoolLiteral(true)))],
        };
        let repr = HirItem::Function {
            name: "Point.__repr__".to_string(),
            params: vec![("self".to_string(), self_ty.clone())],
            return_ty: Ty::Str,
            body: vec![HirStmt::Return(Some(HirExpr::StringLiteral(
                "Point()".to_string(),
            )))],
        };
        let mut items = vec![init, eq, repr];
        items.extend(extra_items);
        HirModule {
            items,
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: vec![(
                "Point".to_string(),
                HirClassDef {
                    name: "Point".to_string(),
                    bases: Vec::new(),
                    mro: vec!["Point".to_string()],
                    attrs: vec![("x".to_string(), Ty::Int), ("y".to_string(), Ty::Int)],
                    methods: vec![
                        ("__init__".to_string(), "Point.__init__".to_string()),
                        ("__eq__".to_string(), "Point.__eq__".to_string()),
                        ("__repr__".to_string(), "Point.__repr__".to_string()),
                    ],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    enum_members: Vec::new(),
                    is_dataclass: true,
                    dataclass_fields: vec![("x".to_string(), Ty::Int), ("y".to_string(), Ty::Int)],
                    is_protocol: false,
                    runtime_checkable: false,
                    protocol_members: Vec::new(),
                    abstract_methods: Vec::new(),
                    is_abstract: false,
                },
            )],
        }
    }

    #[test]
    fn an_eq_comparison_between_same_class_instances_lowers_to_eq_call() {
        let hir = dataclass_point_module(vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "p".to_string(),
                value: HirExpr::Call {
                    callee: "Point".to_string(),
                    args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
                },
            }),
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "q".to_string(),
                value: HirExpr::Call {
                    callee: "Point".to_string(),
                    args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
                },
            }),
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "r".to_string(),
                value: HirExpr::Compare {
                    op: CmpOpKind::Eq,
                    left: Box::new(HirExpr::Name("p".to_string())),
                    right: Box::new(HirExpr::Name("q".to_string())),
                },
            }),
        ]);
        let mir = build(&hir);
        // Items: [__init__, __eq__, __repr__, Assign p, Assign q, Assign r].
        // The comparison is at index 5. Using `matches!` with a guard
        // avoids a hand-written `other => panic!(...)` arm that would be
        // permanently uncovered under D-014's 100%-line-coverage gate
        // (the happy path always matches the expected pattern).
        assert!(matches!(
            &mir.items[5],
            MirItem::TopLevelStmt(MirStmt::Assign {
                value: MirExpr::Call { callee, .. },
                ..
            }) if callee == "Point.__eq__"
        ));
    }

    #[test]
    fn a_neq_comparison_between_same_class_instances_lowers_to_neq_of_eq_call() {
        let hir = dataclass_point_module(vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "p".to_string(),
                value: HirExpr::Call {
                    callee: "Point".to_string(),
                    args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
                },
            }),
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "q".to_string(),
                value: HirExpr::Call {
                    callee: "Point".to_string(),
                    args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
                },
            }),
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "r".to_string(),
                value: HirExpr::Compare {
                    op: CmpOpKind::NotEq,
                    left: Box::new(HirExpr::Name("p".to_string())),
                    right: Box::new(HirExpr::Name("q".to_string())),
                },
            }),
        ]);
        let mir = build(&hir);
        // The NotEq arm wraps the __eq__ call in a Compare with True.
        assert!(matches!(
            &mir.items[5],
            MirItem::TopLevelStmt(MirStmt::Assign {
                value: MirExpr::Compare {
                    op: CmpOpKind::NotEq,
                    left,
                    right,
                    ..
                },
                ..
            }) if matches!(left.as_ref(), MirExpr::Call { callee, .. } if callee == "Point.__eq__")
                && matches!(right.as_ref(), MirExpr::BoolLiteral(true))
        ));
    }

    #[test]
    fn a_non_eq_neq_comparison_between_instances_with_eq_falls_through_to_mir_compare() {
        // A non-Eq/NotEq comparison (`<`, `<=`, `>`, `>=`) between same-class
        // instances with `__eq__` falls through to the default
        // `MirExpr::Compare` rather than being rewritten to an `__eq__` call.
        // The `matches!(op, Eq | NotEq)` guard at the top of the rewrite
        // block prevents entry for other operators. The type checker would
        // reject `<` between class instances (T0021), but the MIR lowering
        // is tested directly here without going through the type checker.
        let hir = dataclass_point_module(vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "p".to_string(),
                value: HirExpr::Call {
                    callee: "Point".to_string(),
                    args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
                },
            }),
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "q".to_string(),
                value: HirExpr::Call {
                    callee: "Point".to_string(),
                    args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
                },
            }),
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "r".to_string(),
                value: HirExpr::Compare {
                    op: CmpOpKind::Lt,
                    left: Box::new(HirExpr::Name("p".to_string())),
                    right: Box::new(HirExpr::Name("q".to_string())),
                },
            }),
        ]);
        let mir = build(&hir);
        // The `Lt` comparison should NOT be rewritten to an `__eq__` call;
        // it should remain a `MirExpr::Compare` with the original operator.
        assert!(matches!(
            &mir.items[5],
            MirItem::TopLevelStmt(MirStmt::Assign {
                value: MirExpr::Compare { op, .. },
                ..
            }) if *op == CmpOpKind::Lt
        ));
    }

    #[test]
    fn an_eq_comparison_between_instances_without_eq_falls_through_to_mir_compare() {
        // When the class has no `__eq__` method in its MRO, the `if let
        // Some(eq_mangled)` block is not entered, and the comparison falls
        // through to the default `MirExpr::Compare`. This covers the `}`
        // (merge point) of the `if let Some(eq_mangled)` block.
        let self_ty = Ty::Instance(Box::new("Plain".to_string()));
        let init = HirItem::Function {
            name: "Plain.__init__".to_string(),
            params: vec![("self".to_string(), self_ty.clone())],
            return_ty: Ty::None,
            body: vec![HirStmt::Return(None)],
        };
        let hir = HirModule {
            items: vec![
                init,
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "p".to_string(),
                    value: HirExpr::Call {
                        callee: "Plain".to_string(),
                        args: vec![],
                    },
                }),
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "q".to_string(),
                    value: HirExpr::Call {
                        callee: "Plain".to_string(),
                        args: vec![],
                    },
                }),
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "r".to_string(),
                    value: HirExpr::Compare {
                        op: CmpOpKind::Eq,
                        left: Box::new(HirExpr::Name("p".to_string())),
                        right: Box::new(HirExpr::Name("q".to_string())),
                    },
                }),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: vec![(
                "Plain".to_string(),
                HirClassDef {
                    name: "Plain".to_string(),
                    bases: Vec::new(),
                    mro: vec!["Plain".to_string()],
                    attrs: Vec::new(),
                    methods: vec![("__init__".to_string(), "Plain.__init__".to_string())],
                    type_param: None,
                    properties: Vec::new(),
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
                },
            )],
        };
        let mir = build(&hir);
        // The last item should be a MirExpr::Compare (not a __eq__ call).
        assert!(matches!(
            &mir.items[3],
            MirItem::TopLevelStmt(MirStmt::Assign {
                value: MirExpr::Compare { op, .. },
                ..
            }) if *op == CmpOpKind::Eq
        ));
    }

    #[test]
    fn print_of_an_instance_with_an_unregistered_class_passes_through() {
        // `rewrite_instance_to_repr` returns `expr.clone()` when the
        // instance's class is not in the `classes` map. This test creates
        // a function with a parameter typed as `Ghost` (an unregistered
        // class), then prints it inside the function body. The MIR should
        // pass the instance through without rewriting (the codegen would
        // panic later, but we only test the MIR lowering here).
        let ghost_ty = Ty::Instance(Box::new("Ghost".to_string()));
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "test".to_string(),
                params: vec![("g".to_string(), ghost_ty.clone())],
                return_ty: Ty::None,
                body: vec![HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::Name("g".to_string())],
                })],
            }],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        // The function body's print argument should be a MirExpr::Name
        // with the Ghost instance type (not rewritten to a __repr__ call).
        // Using nested `matches!` avoids hand-written `other => panic!(...)`
        // arms that would be permanently uncovered under D-014's coverage gate.
        assert!(matches!(
            &mir.items[0],
            MirItem::Function { body, .. }
                if body.len() == 1
                    && matches!(
                        &body[0],
                        MirStmt::ExprStmt(MirExpr::Call { args, .. })
                            if args.len() == 1 && args[0].ty() == ghost_ty
                    )
        ));
    }

    #[test]
    fn print_of_an_instance_with_repr_lowers_to_repr_call() {
        let hir = dataclass_point_module(vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "p".to_string(),
                value: HirExpr::Call {
                    callee: "Point".to_string(),
                    args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
                },
            }),
            HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::Name("p".to_string())],
            })),
        ]);
        let mir = build(&hir);
        // Items: [__init__, __eq__, __repr__, Assign p, ExprStmt(print)].
        // The print is at index 4. The argument should be a MirExpr::Call
        // to Point.__repr__.
        assert!(matches!(
            &mir.items[4],
            MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                callee,
                args,
                ..
            })) if callee == "print"
                  && args.len() == 1
                  && matches!(
                      &args[0],
                      MirExpr::Call { callee: repr_callee, .. }
                          if repr_callee == "Point.__repr__"
                  )
        ));
    }

    // -- #380: eval_isinstance_protocol in compile-time isinstance -------

    #[test]
    fn isinstance_with_runtime_checkable_protocol_evaluates_to_true() {
        use pycc_hir::{HirClassDef, ProtocolMember};
        let proto_def = HirClassDef {
            name: "Drawable".to_string(),
            bases: Vec::new(),
            mro: vec!["Drawable".to_string()],
            attrs: Vec::new(),
            methods: Vec::new(),
            type_param: None,
            properties: Vec::new(),
            static_methods: Vec::new(),
            class_methods: Vec::new(),
            enum_members: Vec::new(),
            is_dataclass: false,
            dataclass_fields: Vec::new(),
            is_protocol: true,
            runtime_checkable: true,
            protocol_members: vec![ProtocolMember::Method {
                name: "draw".to_string(),
                param_tys: vec![],
                return_ty: Ty::None,
            }],
            abstract_methods: Vec::new(),
            is_abstract: false,
        };
        let class_def = HirClassDef {
            name: "Circle".to_string(),
            bases: Vec::new(),
            mro: vec!["Circle".to_string()],
            attrs: Vec::new(),
            methods: vec![("draw".to_string(), "Circle.draw".to_string())],
            type_param: None,
            properties: Vec::new(),
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
        };
        let classes: HashMap<String, HirClassDef> = [
            ("Drawable".to_string(), proto_def),
            ("Circle".to_string(), class_def),
        ]
        .into_iter()
        .collect();
        let obj_ty = Ty::Instance(Box::new("Circle".to_string()));
        let proto = classes.get("Drawable").unwrap();
        assert!(eval_isinstance_protocol(&obj_ty, proto, &classes));
    }

    #[test]
    fn isinstance_with_runtime_checkable_protocol_evaluates_to_false_for_missing_method() {
        use pycc_hir::{HirClassDef, ProtocolMember};
        let proto_def = HirClassDef {
            name: "Drawable".to_string(),
            bases: Vec::new(),
            mro: vec!["Drawable".to_string()],
            attrs: Vec::new(),
            methods: Vec::new(),
            type_param: None,
            properties: Vec::new(),
            static_methods: Vec::new(),
            class_methods: Vec::new(),
            enum_members: Vec::new(),
            is_dataclass: false,
            dataclass_fields: Vec::new(),
            is_protocol: true,
            runtime_checkable: true,
            protocol_members: vec![ProtocolMember::Method {
                name: "draw".to_string(),
                param_tys: vec![],
                return_ty: Ty::None,
            }],
            abstract_methods: Vec::new(),
            is_abstract: false,
        };
        let class_def = HirClassDef {
            name: "Circle".to_string(),
            bases: Vec::new(),
            mro: vec!["Circle".to_string()],
            attrs: Vec::new(),
            methods: vec![("foo".to_string(), "Circle.foo".to_string())],
            type_param: None,
            properties: Vec::new(),
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
        };
        let classes: HashMap<String, HirClassDef> = [
            ("Drawable".to_string(), proto_def),
            ("Circle".to_string(), class_def),
        ]
        .into_iter()
        .collect();
        let obj_ty = Ty::Instance(Box::new("Circle".to_string()));
        let proto = classes.get("Drawable").unwrap();
        assert!(!eval_isinstance_protocol(&obj_ty, proto, &classes));
    }

    #[test]
    fn isinstance_with_runtime_checkable_protocol_evaluates_to_false_for_non_instance() {
        use pycc_hir::{HirClassDef, ProtocolMember};
        let proto_def = HirClassDef {
            name: "Drawable".to_string(),
            bases: Vec::new(),
            mro: vec!["Drawable".to_string()],
            attrs: Vec::new(),
            methods: Vec::new(),
            type_param: None,
            properties: Vec::new(),
            static_methods: Vec::new(),
            class_methods: Vec::new(),
            enum_members: Vec::new(),
            is_dataclass: false,
            dataclass_fields: Vec::new(),
            is_protocol: true,
            runtime_checkable: true,
            protocol_members: vec![ProtocolMember::Method {
                name: "draw".to_string(),
                param_tys: vec![],
                return_ty: Ty::None,
            }],
            abstract_methods: Vec::new(),
            is_abstract: false,
        };
        let classes: HashMap<String, HirClassDef> =
            [("Drawable".to_string(), proto_def)].into_iter().collect();
        // Non-instance type should return false.
        let obj_ty = Ty::Int;
        let proto = classes.get("Drawable").unwrap();
        assert!(!eval_isinstance_protocol(&obj_ty, proto, &classes));
    }

    #[test]
    fn isinstance_with_runtime_checkable_protocol_evaluates_to_false_for_unknown_class() {
        use pycc_hir::{HirClassDef, ProtocolMember};
        let proto_def = HirClassDef {
            name: "Drawable".to_string(),
            bases: Vec::new(),
            mro: vec!["Drawable".to_string()],
            attrs: Vec::new(),
            methods: Vec::new(),
            type_param: None,
            properties: Vec::new(),
            static_methods: Vec::new(),
            class_methods: Vec::new(),
            enum_members: Vec::new(),
            is_dataclass: false,
            dataclass_fields: Vec::new(),
            is_protocol: true,
            runtime_checkable: true,
            protocol_members: vec![ProtocolMember::Method {
                name: "draw".to_string(),
                param_tys: vec![],
                return_ty: Ty::None,
            }],
            abstract_methods: Vec::new(),
            is_abstract: false,
        };
        let classes: HashMap<String, HirClassDef> =
            [("Drawable".to_string(), proto_def)].into_iter().collect();
        // Unknown class should return false.
        let obj_ty = Ty::Instance(Box::new("Unknown".to_string()));
        let proto = classes.get("Drawable").unwrap();
        assert!(!eval_isinstance_protocol(&obj_ty, proto, &classes));
    }

    #[test]
    fn isinstance_with_runtime_checkable_protocol_attribute_member() {
        use pycc_hir::{HirClassDef, ProtocolMember};
        let proto_def = HirClassDef {
            name: "HasX".to_string(),
            bases: Vec::new(),
            mro: vec!["HasX".to_string()],
            attrs: Vec::new(),
            methods: Vec::new(),
            type_param: None,
            properties: Vec::new(),
            static_methods: Vec::new(),
            class_methods: Vec::new(),
            enum_members: Vec::new(),
            is_dataclass: false,
            dataclass_fields: Vec::new(),
            is_protocol: true,
            runtime_checkable: true,
            protocol_members: vec![ProtocolMember::Attribute {
                name: "x".to_string(),
                ty: Ty::Int,
            }],
            abstract_methods: Vec::new(),
            is_abstract: false,
        };
        let class_def = HirClassDef {
            name: "Point".to_string(),
            bases: Vec::new(),
            mro: vec!["Point".to_string()],
            attrs: vec![("x".to_string(), Ty::Int)],
            methods: Vec::new(),
            type_param: None,
            properties: Vec::new(),
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
        };
        let classes: HashMap<String, HirClassDef> = [
            ("HasX".to_string(), proto_def),
            ("Point".to_string(), class_def),
        ]
        .into_iter()
        .collect();
        let obj_ty = Ty::Instance(Box::new("Point".to_string()));
        let proto = classes.get("HasX").unwrap();
        assert!(eval_isinstance_protocol(&obj_ty, proto, &classes));
    }

    #[test]
    fn isinstance_with_runtime_checkable_protocol_attribute_found_via_property() {
        // This exercises the `mro_def.properties.iter().any(...)` path
        // (line 2063) in the attribute presence check.
        use pycc_hir::{HirClassDef, PropertyDef, ProtocolMember};
        let proto_def = HirClassDef {
            name: "HasX".to_string(),
            bases: Vec::new(),
            mro: vec!["HasX".to_string()],
            attrs: Vec::new(),
            methods: Vec::new(),
            type_param: None,
            properties: Vec::new(),
            static_methods: Vec::new(),
            class_methods: Vec::new(),
            enum_members: Vec::new(),
            is_dataclass: false,
            dataclass_fields: Vec::new(),
            is_protocol: true,
            runtime_checkable: true,
            protocol_members: vec![ProtocolMember::Attribute {
                name: "x".to_string(),
                ty: Ty::Int,
            }],
            abstract_methods: Vec::new(),
            is_abstract: false,
        };
        let class_def = HirClassDef {
            name: "Point".to_string(),
            bases: Vec::new(),
            mro: vec!["Point".to_string()],
            attrs: Vec::new(),
            methods: Vec::new(),
            type_param: None,
            properties: vec![PropertyDef {
                name: "x".to_string(),
                getter: "get_x".to_string(),
                setter: None,
            }],
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
        };
        let classes: HashMap<String, HirClassDef> = [
            ("HasX".to_string(), proto_def),
            ("Point".to_string(), class_def),
        ]
        .into_iter()
        .collect();
        let obj_ty = Ty::Instance(Box::new("Point".to_string()));
        let proto = classes.get("HasX").unwrap();
        assert!(eval_isinstance_protocol(&obj_ty, proto, &classes));
    }

    #[test]
    fn isinstance_with_runtime_checkable_protocol_attribute_missing_returns_false() {
        // This exercises the `return false` path (line 2069) when the
        // attribute is not found in the class's MRO.
        use pycc_hir::{HirClassDef, ProtocolMember};
        let proto_def = HirClassDef {
            name: "HasX".to_string(),
            bases: Vec::new(),
            mro: vec!["HasX".to_string()],
            attrs: Vec::new(),
            methods: Vec::new(),
            type_param: None,
            properties: Vec::new(),
            static_methods: Vec::new(),
            class_methods: Vec::new(),
            enum_members: Vec::new(),
            is_dataclass: false,
            dataclass_fields: Vec::new(),
            is_protocol: true,
            runtime_checkable: true,
            protocol_members: vec![ProtocolMember::Attribute {
                name: "x".to_string(),
                ty: Ty::Int,
            }],
            abstract_methods: Vec::new(),
            is_abstract: false,
        };
        let class_def = HirClassDef {
            name: "NoX".to_string(),
            bases: Vec::new(),
            mro: vec!["NoX".to_string()],
            attrs: vec![("y".to_string(), Ty::Int)],
            methods: Vec::new(),
            type_param: None,
            properties: Vec::new(),
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
        };
        let classes: HashMap<String, HirClassDef> = [
            ("HasX".to_string(), proto_def),
            ("NoX".to_string(), class_def),
        ]
        .into_iter()
        .collect();
        let obj_ty = Ty::Instance(Box::new("NoX".to_string()));
        let proto = classes.get("HasX").unwrap();
        assert!(!eval_isinstance_protocol(&obj_ty, proto, &classes));
    }

    #[test]
    fn isinstance_with_runtime_checkable_protocol_attribute_missing_in_mro_returns_false() {
        // This exercises the `false` path (line 2065) when an MRO entry
        // is not found in the `classes` map.
        use pycc_hir::{HirClassDef, ProtocolMember};
        let proto_def = HirClassDef {
            name: "HasX".to_string(),
            bases: Vec::new(),
            mro: vec!["HasX".to_string()],
            attrs: Vec::new(),
            methods: Vec::new(),
            type_param: None,
            properties: Vec::new(),
            static_methods: Vec::new(),
            class_methods: Vec::new(),
            enum_members: Vec::new(),
            is_dataclass: false,
            dataclass_fields: Vec::new(),
            is_protocol: true,
            runtime_checkable: true,
            protocol_members: vec![ProtocolMember::Attribute {
                name: "x".to_string(),
                ty: Ty::Int,
            }],
            abstract_methods: Vec::new(),
            is_abstract: false,
        };
        // The class's MRO includes "Ghost" which is NOT in the classes
        // map, exercising the `else { false }` branch.
        let class_def = HirClassDef {
            name: "WithGhost".to_string(),
            bases: Vec::new(),
            mro: vec!["WithGhost".to_string(), "Ghost".to_string()],
            attrs: Vec::new(),
            methods: Vec::new(),
            type_param: None,
            properties: Vec::new(),
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
        };
        let classes: HashMap<String, HirClassDef> = [
            ("HasX".to_string(), proto_def),
            ("WithGhost".to_string(), class_def),
        ]
        .into_iter()
        .collect();
        let obj_ty = Ty::Instance(Box::new("WithGhost".to_string()));
        let proto = classes.get("HasX").unwrap();
        assert!(!eval_isinstance_protocol(&obj_ty, proto, &classes));
    }

    #[test]
    fn isinstance_with_runtime_checkable_protocol_method_missing_in_mro_returns_false() {
        // Exercises the `?`-None path in the method arm of
        // `eval_isinstance_protocol` when an MRO entry is not found in
        // the `classes` map. The attribute arm's equivalent path is
        // covered by `isinstance_with_runtime_checkable_protocol_attribute_missing_in_mro_returns_false`.
        use pycc_hir::{HirClassDef, ProtocolMember};
        let proto_def = HirClassDef {
            name: "HasDraw".to_string(),
            bases: Vec::new(),
            mro: vec!["HasDraw".to_string()],
            attrs: Vec::new(),
            methods: Vec::new(),
            type_param: None,
            properties: Vec::new(),
            static_methods: Vec::new(),
            class_methods: Vec::new(),
            enum_members: Vec::new(),
            is_dataclass: false,
            dataclass_fields: Vec::new(),
            is_protocol: true,
            runtime_checkable: true,
            protocol_members: vec![ProtocolMember::Method {
                name: "draw".to_string(),
                param_tys: vec![],
                return_ty: Ty::None,
            }],
            abstract_methods: Vec::new(),
            is_abstract: false,
        };
        // The class's MRO includes "Ghost" which is NOT in the classes
        // map, exercising the `?`-None path in the method arm's
        // `find_map` closure.
        let class_def = HirClassDef {
            name: "WithGhostMethod".to_string(),
            bases: Vec::new(),
            mro: vec!["WithGhostMethod".to_string(), "Ghost".to_string()],
            attrs: Vec::new(),
            methods: Vec::new(),
            type_param: None,
            properties: Vec::new(),
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
        };
        let classes: HashMap<String, HirClassDef> = [
            ("HasDraw".to_string(), proto_def),
            ("WithGhostMethod".to_string(), class_def),
        ]
        .into_iter()
        .collect();
        let obj_ty = Ty::Instance(Box::new("WithGhostMethod".to_string()));
        let proto = classes.get("HasDraw").unwrap();
        assert!(!eval_isinstance_protocol(&obj_ty, proto, &classes));
    }

    #[test]
    fn method_call_on_protocol_typed_parameter_resolves_via_class_def_of() {
        // This exercises the `Ty::Protocol(name) => name.as_str()` arm
        // in `class_def_of` (line 1855) by constructing a HIR module
        // with a protocol-typed parameter and a method call on it.
        let proto_ty = Ty::Protocol(Box::new("P".to_string()));
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "P.foo".to_string(),
                    params: vec![("self".to_string(), proto_ty.clone())],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
                HirItem::Function {
                    name: "f".to_string(),
                    params: vec![("x".to_string(), proto_ty)],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::MethodCall {
                        base: Box::new(HirExpr::Name("x".to_string())),
                        method: "foo".to_string(),
                        args: vec![],
                    }))],
                },
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: vec![(
                "P".to_string(),
                HirClassDef {
                    name: "P".to_string(),
                    bases: Vec::new(),
                    mro: vec!["P".to_string()],
                    attrs: Vec::new(),
                    methods: vec![("foo".to_string(), "P.foo".to_string())],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    enum_members: Vec::new(),
                    is_dataclass: false,
                    dataclass_fields: Vec::new(),
                    is_protocol: true,
                    runtime_checkable: false,
                    protocol_members: Vec::new(),
                    abstract_methods: Vec::new(),
                    is_abstract: false,
                },
            )],
        };
        // Just verify it doesn't panic — the MIR build succeeds when
        // `class_def_of` resolves the protocol-typed expression.
        let _ = build(&hir);
    }

    #[test]
    fn protocol_typed_annassign_binds_concrete_type() {
        // Covers the `else` branch (line 759) and the protocol-annotation
        // `bind_ty` path (line 769) in `lower_stmt`'s `AnnAssign` arm.
        // When the annotation is `Ty::Protocol("P")` and the value is
        // `Ty::Instance("C")`, the types don't match (so the `else`
        // branch is taken), and `bind_ty` uses the concrete type.
        let proto_ty = Ty::Protocol(Box::new("P".to_string()));
        let instance_ty = Ty::Instance(Box::new("C".to_string()));
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::AnnAssign {
                    target: "c".to_string(),
                    annotation: proto_ty,
                    value: Some(HirExpr::Call {
                        callee: "C".to_string(),
                        args: vec![],
                    }),
                    is_final: false,
                }),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: vec![
                (
                    "P".to_string(),
                    HirClassDef {
                        name: "P".to_string(),
                        bases: Vec::new(),
                        mro: vec!["P".to_string()],
                        attrs: Vec::new(),
                        methods: Vec::new(),
                        type_param: None,
                        properties: Vec::new(),
                        static_methods: Vec::new(),
                        class_methods: Vec::new(),
                        enum_members: Vec::new(),
                        is_dataclass: false,
                        dataclass_fields: Vec::new(),
                        is_protocol: true,
                        runtime_checkable: false,
                        protocol_members: Vec::new(),
                        abstract_methods: Vec::new(),
                        is_abstract: false,
                    },
                ),
                (
                    "C".to_string(),
                    HirClassDef {
                        name: "C".to_string(),
                        bases: Vec::new(),
                        mro: vec!["C".to_string()],
                        attrs: Vec::new(),
                        methods: vec![("__init__".to_string(), "C.__init__".to_string())],
                        type_param: None,
                        properties: Vec::new(),
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
                    },
                ),
            ],
        };
        let mir = build(&hir);
        // The MIR should bind `c` with the concrete `Instance("C")` type,
        // not the protocol type.
        assert!(mir.items.iter().any(|item| {
            matches!(item, MirItem::TopLevelStmt(MirStmt::Assign { target, value })
                if target == "c" && value.ty() == instance_ty)
        }));
    }

    #[test]
    fn isinstance_with_protocol_target_goes_through_lower_isinstance() {
        // Covers line 1999: the `eval_isinstance_protocol` call inside
        // `lower_isinstance` when the target class is a protocol.  The
        // existing `isinstance_with_runtime_checkable_protocol_*` tests
        // call `eval_isinstance_protocol` directly; this test goes
        // through the full `build` → `lower_expr` → `lower_isinstance`
        // path.
        use pycc_hir::ProtocolMember;
        let proto_def = HirClassDef {
            name: "Drawable".to_string(),
            bases: Vec::new(),
            mro: vec!["Drawable".to_string()],
            attrs: Vec::new(),
            methods: Vec::new(),
            type_param: None,
            properties: Vec::new(),
            static_methods: Vec::new(),
            class_methods: Vec::new(),
            enum_members: Vec::new(),
            is_dataclass: false,
            dataclass_fields: Vec::new(),
            is_protocol: true,
            runtime_checkable: true,
            protocol_members: vec![ProtocolMember::Method {
                name: "draw".to_string(),
                param_tys: vec![],
                return_ty: Ty::None,
            }],
            abstract_methods: Vec::new(),
            is_abstract: false,
        };
        let class_def = HirClassDef {
            name: "Circle".to_string(),
            bases: Vec::new(),
            mro: vec!["Circle".to_string()],
            attrs: Vec::new(),
            methods: vec![
                ("__init__".to_string(), "Circle.__init__".to_string()),
                ("draw".to_string(), "Circle.draw".to_string()),
            ],
            type_param: None,
            properties: Vec::new(),
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
        };
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "c".to_string(),
                    value: HirExpr::Call {
                        callee: "Circle".to_string(),
                        args: vec![],
                    },
                }),
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "isinstance".to_string(),
                    args: vec![
                        HirExpr::Name("c".to_string()),
                        HirExpr::Name("Drawable".to_string()),
                    ],
                })),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: vec![
                ("Drawable".to_string(), proto_def),
                ("Circle".to_string(), class_def),
            ],
        };
        let mir = build(&hir);
        // The isinstance call should be lowered to a BoolLiteral(true)
        // because Circle conforms to the Drawable protocol.
        assert!(mir
            .items
            .iter()
            .any(|item| matches!(item, MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::BoolLiteral(true))))));
    }

    // -- #381: match statement MIR lowering coverage -----------------------

    fn match_module(cases: Vec<HirMatchCase>) -> HirModule {
        HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(1),
                }),
                HirItem::TopLevelStmt(HirStmt::Match {
                    subject: HirExpr::Name("x".to_string()),
                    cases,
                }),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        }
    }

    fn match_module_list(cases: Vec<HirMatchCase>) -> HirModule {
        HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)]),
                }),
                HirItem::TopLevelStmt(HirStmt::Match {
                    subject: HirExpr::Name("x".to_string()),
                    cases,
                }),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        }
    }

    fn match_module_dict(cases: Vec<HirMatchCase>) -> HirModule {
        HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::DictLiteral(vec![(
                        HirExpr::StringLiteral("k".to_string()),
                        HirExpr::IntLiteral(1),
                    )]),
                }),
                HirItem::TopLevelStmt(HirStmt::Match {
                    subject: HirExpr::Name("x".to_string()),
                    cases,
                }),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        }
    }

    #[test]
    fn lowers_match_with_literal_pattern_to_mir() {
        let hir = match_module(vec![
            HirMatchCase {
                pattern: HirPattern::Literal(HirExpr::IntLiteral(1)),
                guard: None,
                body: vec![HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::IntLiteral(1)],
                })],
            },
            HirMatchCase {
                pattern: HirPattern::Wildcard,
                guard: None,
                body: vec![HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::IntLiteral(0)],
                })],
            },
        ]);
        let mir = build(&hir);
        assert!(!mir.items.is_empty());
    }

    #[test]
    fn lowers_match_with_singleton_pattern_to_mir() {
        let hir = match_module(vec![
            HirMatchCase {
                pattern: HirPattern::Singleton(true),
                guard: None,
                body: vec![],
            },
            HirMatchCase {
                pattern: HirPattern::Singleton(false),
                guard: None,
                body: vec![],
            },
        ]);
        let mir = build(&hir);
        assert!(!mir.items.is_empty());
    }

    #[test]
    fn lowers_match_with_none_singleton_pattern_to_mir() {
        let hir = match_module(vec![HirMatchCase {
            pattern: HirPattern::NoneSingleton,
            guard: None,
            body: vec![],
        }]);
        let mir = build(&hir);
        assert!(!mir.items.is_empty());
    }

    #[test]
    fn lowers_match_with_capture_pattern_to_mir() {
        let hir = match_module(vec![HirMatchCase {
            pattern: HirPattern::Capture("y".to_string()),
            guard: None,
            body: vec![HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::Name("y".to_string())],
            })],
        }]);
        let mir = build(&hir);
        assert!(!mir.items.is_empty());
    }

    #[test]
    fn lowers_match_with_sequence_pattern_to_mir() {
        let hir = match_module_list(vec![
            HirMatchCase {
                pattern: HirPattern::Sequence(vec![
                    HirPattern::Capture("a".to_string()),
                    HirPattern::Capture("b".to_string()),
                ]),
                guard: None,
                body: vec![],
            },
            HirMatchCase {
                pattern: HirPattern::Wildcard,
                guard: None,
                body: vec![],
            },
        ]);
        let mir = build(&hir);
        assert!(!mir.items.is_empty());
    }

    #[test]
    fn lowers_match_with_sequence_star_pattern_to_mir() {
        let hir = match_module_list(vec![
            HirMatchCase {
                pattern: HirPattern::SequenceStar(
                    vec![HirPattern::Capture("a".to_string())],
                    Some("rest".to_string()),
                ),
                guard: None,
                body: vec![],
            },
            HirMatchCase {
                pattern: HirPattern::Wildcard,
                guard: None,
                body: vec![],
            },
        ]);
        let mir = build(&hir);
        assert!(!mir.items.is_empty());
    }

    #[test]
    fn lowers_match_with_mapping_pattern_to_mir() {
        let hir = match_module_dict(vec![
            HirMatchCase {
                pattern: HirPattern::Mapping(
                    vec![(
                        HirExpr::StringLiteral("k".to_string()),
                        HirPattern::Capture("v".to_string()),
                    )],
                    None,
                ),
                guard: None,
                body: vec![],
            },
            HirMatchCase {
                pattern: HirPattern::Wildcard,
                guard: None,
                body: vec![],
            },
        ]);
        let mir = build(&hir);
        assert!(!mir.items.is_empty());
    }

    #[test]
    fn lowers_match_with_mapping_rest_pattern_to_mir() {
        let hir = match_module_dict(vec![
            HirMatchCase {
                pattern: HirPattern::Mapping(
                    vec![(
                        HirExpr::StringLiteral("k".to_string()),
                        HirPattern::Capture("v".to_string()),
                    )],
                    Some("rest".to_string()),
                ),
                guard: None,
                body: vec![],
            },
            HirMatchCase {
                pattern: HirPattern::Wildcard,
                guard: None,
                body: vec![],
            },
        ]);
        let mir = build(&hir);
        assert!(!mir.items.is_empty());
    }

    #[test]
    fn lowers_match_with_class_pattern_to_mir() {
        let class_def = HirClassDef {
            name: "P".to_string(),
            bases: Vec::new(),
            mro: vec!["P".to_string()],
            attrs: vec![("a".to_string(), Ty::Int)],
            methods: vec![("__init__".to_string(), "P.__init__".to_string())],
            properties: Vec::new(),
            static_methods: Vec::new(),
            class_methods: Vec::new(),
            type_param: None,
            enum_members: Vec::new(),
            is_dataclass: false,
            dataclass_fields: Vec::new(),
            is_protocol: false,
            runtime_checkable: false,
            protocol_members: Vec::new(),
            abstract_methods: Vec::new(),
            is_abstract: false,
        };
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(1),
                }),
                HirItem::TopLevelStmt(HirStmt::Match {
                    subject: HirExpr::Name("x".to_string()),
                    cases: vec![
                        HirMatchCase {
                            pattern: HirPattern::Class {
                                class_name: "P".to_string(),
                                positional: vec![HirPattern::Capture("a".to_string())],
                                keyword: vec![("a".to_string(), HirPattern::Capture("a2".to_string()))],
                            },
                            guard: None,
                            body: vec![],
                        },
                        HirMatchCase {
                            pattern: HirPattern::Wildcard,
                            guard: None,
                            body: vec![],
                        },
                    ],
                }),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: vec![("P".to_string(), class_def)],
        };
        let mir = build(&hir);
        assert!(!mir.items.is_empty());
    }

    #[test]
    fn lowers_match_with_or_pattern_to_mir() {
        let hir = match_module(vec![
            HirMatchCase {
                pattern: HirPattern::Or(vec![
                    HirPattern::Literal(HirExpr::IntLiteral(1)),
                    HirPattern::Literal(HirExpr::IntLiteral(2)),
                    HirPattern::Literal(HirExpr::IntLiteral(3)),
                ]),
                guard: None,
                body: vec![],
            },
            HirMatchCase {
                pattern: HirPattern::Wildcard,
                guard: None,
                body: vec![],
            },
        ]);
        let mir = build(&hir);
        assert!(!mir.items.is_empty());
    }

    #[test]
    fn lowers_match_with_as_pattern_to_mir() {
        let hir = match_module(vec![
            HirMatchCase {
                pattern: HirPattern::As(
                    Box::new(HirPattern::Literal(HirExpr::IntLiteral(1))),
                    "y".to_string(),
                ),
                guard: None,
                body: vec![],
            },
            HirMatchCase {
                pattern: HirPattern::Wildcard,
                guard: None,
                body: vec![],
            },
        ]);
        let mir = build(&hir);
        assert!(!mir.items.is_empty());
    }

    #[test]
    fn lowers_match_with_guard_to_mir() {
        let hir = match_module(vec![
            HirMatchCase {
                pattern: HirPattern::Capture("y".to_string()),
                guard: Some(HirExpr::Compare {
                    op: CmpOpKind::Gt,
                    left: Box::new(HirExpr::Name("y".to_string())),
                    right: Box::new(HirExpr::IntLiteral(3)),
                }),
                body: vec![HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::Name("y".to_string())],
                })],
            },
            HirMatchCase {
                pattern: HirPattern::Wildcard,
                guard: None,
                body: vec![],
            },
        ]);
        let mir = build(&hir);
        assert!(!mir.items.is_empty());
    }

    #[test]
    fn lowers_match_with_empty_cases_to_mir() {
        let hir = match_module(vec![]);
        let mir = build(&hir);
        assert!(!mir.items.is_empty());
    }

    #[test]
    fn lowers_match_with_wildcard_only_to_mir() {
        let hir = match_module(vec![HirMatchCase {
            pattern: HirPattern::Wildcard,
            guard: None,
            body: vec![HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::IntLiteral(0)],
            })],
        }]);
        let mir = build(&hir);
        assert!(!mir.items.is_empty());
    }

    #[test]
    fn nest_match_alternatives_with_empty_alternatives_returns_seq() {
        let inner_body = vec![MirStmt::ExprStmt(MirExpr::IntLiteral(0))];
        let else_chain = MirStmt::ExprStmt(MirExpr::IntLiteral(1));
        let result = nest_match_alternatives(&[], inner_body, else_chain);
        assert!(matches!(result, MirStmt::Seq(body) if body.len() == 1));
    }

    #[test]
    fn lowers_match_with_or_pattern_with_bindings_to_mir() {
        let hir = match_module(vec![HirMatchCase {
            pattern: HirPattern::Or(vec![
                HirPattern::Capture("a".to_string()),
                HirPattern::Capture("b".to_string()),
            ]),
            guard: None,
            body: vec![],
        }]);
        let mir = build(&hir);
        assert!(!mir.items.is_empty());
    }

    // #382 (PR-22 Part 1): MIR lowering tests for exception handling.

    fn try_module(
        body: Vec<HirStmt>,
        handlers: Vec<pycc_hir::HirExceptHandler>,
        orelse: Vec<HirStmt>,
        finalbody: Vec<HirStmt>,
    ) -> HirModule {
        HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::Try {
                body,
                handlers,
                orelse,
                finalbody,
            })],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        }
    }

    /// Test helper: extract a `Try` statement from a top-level MIR item,
    /// panicking if the item is not a `Try`.  The panic arm is covered by
    /// `expect_top_level_try_panics_on_non_try`.
    fn expect_top_level_try(
        item: &MirItem,
    ) -> (
        &[MirStmt],
        &[MirExceptHandler],
        &[MirStmt],
        &[MirStmt],
    ) {
        match item {
            MirItem::TopLevelStmt(MirStmt::Try { body, handlers, orelse, finalbody }) => {
                (body, handlers, orelse, finalbody)
            }
            _ => panic!("expected Try"),
        }
    }

    /// Test helper: extract a `Raise` statement from a top-level MIR item,
    /// panicking if the item is not a `Raise`.  The panic arm is covered by
    /// `expect_top_level_raise_panics_on_non_raise`.
    fn expect_top_level_raise(item: &MirItem) -> (&u8, &MirExpr) {
        match item {
            MirItem::TopLevelStmt(MirStmt::Raise { exc_type_tag, message }) => {
                (exc_type_tag, message)
            }
            _ => panic!("expected Raise"),
        }
    }

    /// Test helper: extract a `RaiseFrom` statement from a top-level MIR item,
    /// panicking if the item is not a `RaiseFrom`.  The panic arm is covered
    /// by `expect_top_level_raise_from_panics_on_non_raise_from`.
    fn expect_top_level_raise_from(item: &MirItem) -> (&u8, &u8, &MirExpr, &MirExpr) {
        match item {
            MirItem::TopLevelStmt(MirStmt::RaiseFrom { exc_type_tag, cause_type_tag, message, cause_message }) => {
                (exc_type_tag, cause_type_tag, message, cause_message)
            }
            _ => panic!("expected RaiseFrom"),
        }
    }

    /// Test helper: assert a top-level MIR item is a `Reraise`, panicking
    /// otherwise.  The panic arm is covered by
    /// `expect_top_level_reraise_panics_on_non_reraise`.
    fn expect_top_level_reraise(item: &MirItem) {
        match item {
            MirItem::TopLevelStmt(MirStmt::Reraise) => {}
            _ => panic!("expected Reraise"),
        }
    }

    #[test]
    #[should_panic(expected = "expected Try")]
    fn expect_top_level_try_panics_on_non_try() {
        expect_top_level_try(&MirItem::TopLevelStmt(MirStmt::NoOp));
    }

    #[test]
    #[should_panic(expected = "expected Raise")]
    fn expect_top_level_raise_panics_on_non_raise() {
        expect_top_level_raise(&MirItem::TopLevelStmt(MirStmt::NoOp));
    }

    #[test]
    #[should_panic(expected = "expected RaiseFrom")]
    fn expect_top_level_raise_from_panics_on_non_raise_from() {
        expect_top_level_raise_from(&MirItem::TopLevelStmt(MirStmt::NoOp));
    }

    #[test]
    #[should_panic(expected = "expected Reraise")]
    fn expect_top_level_reraise_panics_on_non_reraise() {
        expect_top_level_reraise(&MirItem::TopLevelStmt(MirStmt::NoOp));
    }

    /// Test helper: assert a `MirExpr` is a `StringLiteral`, returning
    /// the inner string.  Panicking otherwise.  The panic arm is covered
    /// by `expect_string_literal_panics_on_non_string`.
    fn expect_string_literal(expr: &MirExpr) -> &str {
        match expr {
            MirExpr::StringLiteral(s) => s,
            _ => panic!("expected StringLiteral"),
        }
    }

    #[test]
    #[should_panic(expected = "expected StringLiteral")]
    fn expect_string_literal_panics_on_non_string() {
        expect_string_literal(&MirExpr::IntLiteral(0));
    }

    #[test]
    fn lowers_try_with_value_error_handler_to_mir() {
        let hir = try_module(
            vec![HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::StringLiteral("hello".to_string())],
            })],
            vec![pycc_hir::HirExceptHandler {
                exc_type: Some("ValueError".to_string()),
                name: None,
                body: vec![HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::StringLiteral("caught".to_string())],
                })],
            }],
            Vec::new(),
            Vec::new(),
        );
        let mir = build(&hir);
        assert_eq!(mir.items.len(), 1);
        let (body, handlers, orelse, finalbody) = expect_top_level_try(&mir.items[0]);
        assert_eq!(body.len(), 1);
        assert_eq!(handlers.len(), 1);
        assert_eq!(handlers[0].exc_type_tag, Some(1)); // ValueError = 1
        assert!(orelse.is_empty());
        assert!(finalbody.is_empty());
    }

    #[test]
    fn lowers_try_with_bare_except_to_mir() {
        let hir = try_module(
            vec![HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::StringLiteral("body".to_string())],
            })],
            vec![pycc_hir::HirExceptHandler {
                exc_type: None,
                name: None,
                body: vec![HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::StringLiteral("caught".to_string())],
                })],
            }],
            Vec::new(),
            Vec::new(),
        );
        let mir = build(&hir);
        let (_, handlers, _, _) = expect_top_level_try(&mir.items[0]);
        assert_eq!(handlers[0].exc_type_tag, None); // bare except
    }

    #[test]
    fn lowers_try_with_else_and_finally_to_mir() {
        let hir = try_module(
            vec![HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::StringLiteral("body".to_string())],
            })],
            vec![pycc_hir::HirExceptHandler {
                exc_type: Some("Exception".to_string()),
                name: None,
                body: vec![HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::StringLiteral("handler".to_string())],
                })],
            }],
            vec![HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::StringLiteral("else".to_string())],
            })],
            vec![HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::StringLiteral("finally".to_string())],
            })],
        );
        let mir = build(&hir);
        let (body, handlers, orelse, finalbody) = expect_top_level_try(&mir.items[0]);
        assert_eq!(body.len(), 1);
        assert_eq!(handlers.len(), 1);
        assert_eq!(handlers[0].exc_type_tag, Some(0)); // Exception = 0
        assert_eq!(orelse.len(), 1);
        assert_eq!(finalbody.len(), 1);
    }

    #[test]
    fn lowers_raise_value_error_to_mir() {
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::Raise {
                exc: Some(HirExpr::Call {
                    callee: "ValueError".to_string(),
                    args: vec![HirExpr::StringLiteral("bad value".to_string())],
                }),
                cause: None,
            })],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        let (exc_type_tag, message) = expect_top_level_raise(&mir.items[0]);
        assert_eq!(*exc_type_tag, 1); // ValueError = 1
        expect_string_literal(message);
    }

    #[test]
    fn lowers_raise_from_to_mir() {
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::Raise {
                exc: Some(HirExpr::Call {
                    callee: "ValueError".to_string(),
                    args: vec![HirExpr::StringLiteral("bad".to_string())],
                }),
                cause: Some(HirExpr::Call {
                    callee: "TypeError".to_string(),
                    args: vec![HirExpr::StringLiteral("cause".to_string())],
                }),
            })],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        let (exc_type_tag, cause_type_tag, _, _) = expect_top_level_raise_from(&mir.items[0]);
        assert_eq!(*exc_type_tag, 1); // ValueError = 1
        assert_eq!(*cause_type_tag, 2); // TypeError = 2
    }

    #[test]
    fn lowers_bare_reraise_to_mir() {
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::Raise {
                exc: None,
                cause: None,
            })],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        expect_top_level_reraise(&mir.items[0]);
    }

    #[test]
    fn resolve_exception_tag_maps_all_builtin_types() {
        assert_eq!(resolve_exception_tag("Exception"), Some(0));
        assert_eq!(resolve_exception_tag("ValueError"), Some(1));
        assert_eq!(resolve_exception_tag("TypeError"), Some(2));
        assert_eq!(resolve_exception_tag("KeyError"), Some(3));
        assert_eq!(resolve_exception_tag("IndexError"), Some(4));
        assert_eq!(resolve_exception_tag("ZeroDivisionError"), Some(5));
        assert_eq!(resolve_exception_tag("RuntimeError"), Some(6));
        assert_eq!(resolve_exception_tag("UnknownError"), None);
    }

    #[test]
    fn lowers_raise_with_no_args_uses_fallback_message() {
        // `raise ValueError()` — no args, should use "unknown" fallback.
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::Raise {
                exc: Some(HirExpr::Call {
                    callee: "ValueError".to_string(),
                    args: vec![],
                }),
                cause: None,
            })],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        let (_, message) = expect_top_level_raise(&mir.items[0]);
        assert_eq!(expect_string_literal(message), "unknown");
    }

    #[test]
    fn lowers_raise_with_non_call_expr_uses_fallback() {
        // A non-Call expression as the raise exc — should hit the fallback
        // path in `extract_exception_call`.
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::Raise {
                exc: Some(HirExpr::Name("some_exc".to_string())),
                cause: None,
            })],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let mir = build(&hir);
        let (exc_type_tag, message) = expect_top_level_raise(&mir.items[0]);
        assert_eq!(*exc_type_tag, 0); // fallback tag
        expect_string_literal(message);
    }
}
