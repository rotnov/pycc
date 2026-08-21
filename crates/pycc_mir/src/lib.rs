pub use pycc_hir::HirClassDef;
mod exception;
#[cfg(test)]
use exception::lower_exception_value;
pub use exception::{MirExceptHandler, MirExceptionValue};
use exception::{lower_raise, resolve_exception_tag};
use pycc_hir::{
    CompIter, FStringPart, HirExpr, HirItem, HirMatchCase, HirModule, HirPattern, HirStmt,
    UnaryOpKind, eval_isinstance_single, eval_issubclass_single, extract_class_names,
    is_builtin_type_name,
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
    /// A continuation that the validated HIR proves cannot be reached. The
    /// match lowerer uses this for the no-case fallback of an exhaustive
    /// `match`: preserving the proof in MIR keeps later fallthrough analysis
    /// aligned with the type checker instead of inventing a synthetic `NoOp`
    /// path after all exhaustive alternatives failed.
    Unreachable,
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
    /// `raise <exception>` (PEP 3110, #382).
    Raise {
        exception: MirExceptionValue,
    },
    /// `raise <exception> from <cause>` (PEP 409, #382).
    RaiseFrom {
        exception: MirExceptionValue,
        cause: MirExceptionValue,
    },
    /// Bare `raise` (re-raise, #382). Only valid inside an except handler.
    Reraise,
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
        HirStmt::Try {
            body,
            handlers,
            orelse,
            finalbody,
        } => {
            let body = body
                .iter()
                .map(|s| lower_stmt(s, scopes, classes, current_class))
                .collect();
            let handlers = handlers
                .iter()
                .map(|h| {
                    let exc_type_tag = h.exc_type.as_deref().map(|name| {
                        resolve_exception_tag(name)
                            .expect("pycc_types rejects unknown exception handler types before MIR")
                    });
                    if let (Some(exc_type), Some(name)) = (&h.exc_type, &h.name) {
                        // The type checker binds `except T as name` only in
                        // the handler's cloned environment. MIR maintains
                        // its own type scopes, so record the same binding
                        // before lowering expressions in the handler body.
                        // A bare handler cannot have an `as` name in Python.
                        bind(
                            scopes,
                            name.clone(),
                            Ty::Instance(Box::new(exc_type.clone())),
                        );
                    }
                    let handler_body = h
                        .body
                        .iter()
                        .map(|s| lower_stmt(s, scopes, classes, current_class))
                        .collect();
                    MirExceptHandler {
                        exc_type_tag,
                        binding_name: h.name.clone(),
                        binding_ty: h
                            .name
                            .as_ref()
                            .zip(h.exc_type.as_ref())
                            .map(|_| Ty::Instance(Box::new(h.exc_type.clone().unwrap()))),
                        body: handler_body,
                    }
                })
                .collect();
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
        HirStmt::Raise { exc, cause } => lower_raise(exc, cause, scopes, classes, current_class),
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
    let chain = lower_match_chain(&subj_var, &subj_ty, cases, scopes, classes, current_class);
    MirStmt::Seq(vec![assign, chain])
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
        // `pycc_types::check` rejects every non-exhaustive match before MIR
        // lowering (T0030), including guarded-only coverage. Reaching the
        // end of the validated case chain is therefore statically
        // impossible and must remain terminal in MIR.
        return MirStmt::Unreachable;
    }
    let case = &cases[0];
    let rest = &cases[1..];
    let subj_ref = MirExpr::Name {
        name: subj_var.to_string(),
        ty: subj_ty.clone(),
    };
    let (alternatives, bindings) =
        lower_pattern_conds(&subj_ref, &case.pattern, scopes, classes, current_class);
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
fn nest_match_conds(conds: &[MirExpr], inner_body: Vec<MirStmt>, else_chain: MirStmt) -> MirStmt {
    if conds.is_empty() {
        return MirStmt::Seq(inner_body);
    }
    let (first, rest) = conds.split_first().unwrap();
    MirStmt::If {
        test: first.clone(),
        body: vec![nest_match_conds(
            rest,
            inner_body.clone(),
            else_chain.clone(),
        )],
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
        HirPattern::Capture(name) => (vec![vec![]], vec![(name.clone(), subj.clone())]),
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
        HirPattern::SequenceStar(sub_pats, rest) => lower_sequence_conds(
            subj,
            sub_pats,
            rest.as_ref(),
            scopes,
            classes,
            current_class,
        ),
        HirPattern::Mapping(pairs, rest) => {
            lower_mapping_conds(subj, pairs, rest.as_ref(), scopes, classes, current_class)
        }
        HirPattern::Class {
            class_name,
            positional,
            keyword,
        } => lower_class_conds(
            subj,
            class_name,
            positional,
            keyword,
            scopes,
            classes,
            current_class,
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
            let (alts, bindings) = lower_pattern_conds(subj, inner, scopes, classes, current_class);
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
        op: if rest.is_some() {
            CmpOpKind::GtE
        } else {
            CmpOpKind::Eq
        },
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
        // #603 (Part 2 of #573): `-x`/`+x` over a non-literal operand is
        // rewritten into the equivalent binary expression rather than
        // getting a `MirExpr` variant of its own, the same way `!=` between
        // dataclass instances is lowered as a negated `__eq__` call below
        // -- pycc's MIR deliberately has no unary node.
        //
        // The rewrite is representation-sensitive, so it is not a single
        // uniform shape:
        //
        // * `int`/`bool` become `0 - x` / `0 + x`. `pycc_rt`'s `int_sub`
        //   and `int_add` both handle an already-promoted bigint operand
        //   and the smallint boundary, so negation inherits arbitrary
        //   precision for free. `x * -1` would *not*: `int_mul` calls
        //   `require_inline_int` and aborts on a bigint.
        // * `float` becomes `x * -1.0` / `x * 1.0`, a plain LLVM `fmul`.
        //   `0.0 - x` would be wrong for `-0.0` (it yields `+0.0`), and
        //   multiplication is exact for infinities and NaN too.
        //
        // `UAdd` is not folded away, because it is not the identity: on a
        // `bool` operand it crosses into `int` (`+True` is `1`), which the
        // `0 + x` / `x * 1.0` shapes deliver through the same typing rules
        // as `USub`. Any non-numeric operand is already rejected by
        // `pycc_types::unop::unary_result_type` and never reaches here.
        HirExpr::UnaryOp { op, operand } => {
            let operand = lower_expr(operand, scopes, classes, current_class);
            let (op, left, right) = if operand.ty() == Ty::Float {
                let factor = if matches!(op, UnaryOpKind::USub) {
                    -1.0
                } else {
                    1.0
                };
                (BinOpKind::Mul, operand, MirExpr::FloatLiteral(factor))
            } else {
                let op = if matches!(op, UnaryOpKind::USub) {
                    BinOpKind::Sub
                } else {
                    BinOpKind::Add
                };
                (op, MirExpr::IntLiteral(0), operand)
            };
            let ty = binop_result_ty(op, left.ty(), right.ty());
            MirExpr::BinOp {
                op,
                left: Box::new(left),
                right: Box::new(right),
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
            // PEP 560 (#610): `C[x]` on a bare class name is
            // `C.__class_getitem__(x)`. `pycc_types`' own `Subscript` arm
            // has already resolved the hook through the MRO and rejected a
            // class that does not define it, so reaching this point with a
            // class-name base means the call is valid. The guard is the same
            // one that arm applies -- a name bound as a value shadows the
            // class, and `C[0]` then reads that value's element.
            //
            // Lowering is delegated to this function's own `MethodCall` arm
            // by way of a synthetic node rather than duplicated here: that
            // arm already handles both the `@staticmethod` and the
            // `@classmethod` spelling of the hook (the latter needs a
            // `NullInstance` receiver prepended), and re-deriving either
            // shape here would be a second copy of logic that must not
            // drift. `lower_expr` on a bare class name would panic (class
            // names are not in scope), which is why the interception happens
            // before the base is lowered.
            if let HirExpr::Name(class_name) = base.as_ref()
                && !scopes.iter().any(|scope| scope.contains_key(class_name))
                && classes.contains_key(class_name.as_str())
            {
                return lower_expr(
                    &HirExpr::MethodCall {
                        base: base.clone(),
                        method: "__class_getitem__".to_string(),
                        args: vec![(**index).clone()],
                    },
                    scopes,
                    classes,
                    current_class,
                );
            }
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
            // current function's first parameter) as the instance.
            // #587: only a `@property` resolves this way. A `super` object
            // proxies class-level attributes and descriptors, not the
            // instance's own attributes, so `super().<instance attr>` no
            // longer lowers to a slot read against `self` — the type
            // checker rejects it with `T0047` first.
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
                // #587: nothing else resolves through `super()`. A `super`
                // object proxies class-level attributes and descriptors
                // along the MRO, not the instance's own attributes, so an
                // `AttrGet` naming an instance attribute (or a name declared
                // nowhere in the MRO at all) is rejected by
                // `pycc_types::class::resolve_super_attr_get` with `T0047`
                // or `T0044` before any HIR reaches this crate. Properties
                // are the only class-level member pycc models today, so the
                // loop above is exhaustive for well-typed input.
                panic!(
                    "pycc_mir: internal error: `super().{attr}` is not a property on any class \
                     after `{current}` in its MRO -- pycc_types::check should have rejected this \
                     HIR with T0047 or T0044 before it reached pycc_mir"
                );
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
                    classes.get(mro_class.as_str()).is_some_and(|mro_def| {
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
    // #574 (Part 1 of #123): string repetition. `pycc_types`'
    // `numeric_result_type` types `str * int` and `int * str` -- with
    // `bool` accepted as the count, since `bool <: int` -- as `str`, so
    // this function has to say the same, for exactly the reason the
    // `Div` paragraph below gives: `pycc_types` already accepted the
    // program on that promise, and answering `Ty::Int` here would make
    // MIR's `ty` lie about what codegen must produce. Codegen does not
    // lower repetition yet; it stops at its own explicit, named D-072
    // boundary (exit 101, see `docs/CLI_SPEC.md`). Native repetition is
    // #575.
    if op == BinOpKind::Mul
        && ((left == Ty::Str && matches!(right, Ty::Int | Ty::Bool))
            || (right == Ty::Str && matches!(left, Ty::Int | Ty::Bool)))
    {
        return Ty::Str;
    }
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
mod tests;
