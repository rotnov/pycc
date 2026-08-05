use pycc_hir::{CompIter, FStringPart, HirExpr, HirItem, HirModule, HirStmt};
use std::collections::HashMap;

// Re-exported (not just `use`d) because `pycc_codegen` doesn't depend on
// `pycc_hir` directly (see its Cargo.toml) -- `Ty`, `BinOpKind`, and
// `CmpOpKind` all reach this crate's public API through `MirExpr`'s fields
// (`Name`/`Call` carry a `Ty`; `BinOp` carries a `BinOpKind`; `Compare`
// carries a `CmpOpKind`), so each must be nameable as
// `pycc_mir::{Ty, BinOpKind, CmpOpKind}` from any downstream crate, exactly
// like `pycc_types` already re-exports `Ty` (`pycc_types::Ty`, its own line
// 4) for the same reason.
pub use pycc_hir::{BinOpKind, CmpOpKind, Ty};

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
        }
    }
}

#[derive(Debug, PartialEq)]
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
#[derive(Debug, PartialEq)]
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
}

pub fn build(hir: &HirModule) -> MirModule {
    let mut scopes: Vec<HashMap<String, Ty>> = vec![HashMap::new()];
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
            lowered[index] = Some(lower_item(item, &mut scopes));
        }
    }
    for (index, item) in hir.items.iter().enumerate() {
        if matches!(item, HirItem::Function { .. }) {
            lowered[index] = Some(lower_item(item, &mut scopes));
        }
    }
    let items = lowered
        .into_iter()
        .map(|item| item.expect("every HIR item is either a function or a top-level statement"))
        .collect();
    MirModule { items }
}

fn lower_item(item: &HirItem, scopes: &mut Vec<HashMap<String, Ty>>) -> MirItem {
    match item {
        HirItem::Function {
            name,
            params,
            return_ty,
            body,
        } => {
            scopes.push(params.iter().cloned().collect());
            let body = body.iter().map(|s| lower_stmt(s, scopes)).collect();
            scopes.pop();
            MirItem::Function {
                name: name.clone(),
                params: params.clone(),
                return_ty: return_ty.clone(),
                body,
            }
        }
        HirItem::TopLevelStmt(stmt) => MirItem::TopLevelStmt(lower_stmt(stmt, scopes)),
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
) -> (CompSource, Ty) {
    match iter {
        CompIter::Range { start, stop, step } => {
            let start = lower_expr(start, scopes);
            let stop = lower_expr(stop, scopes);
            let step = lower_expr(step, scopes);
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

fn lower_stmt(stmt: &HirStmt, scopes: &mut Vec<HashMap<String, Ty>>) -> MirStmt {
    match stmt {
        HirStmt::ExprStmt(expr) => MirStmt::ExprStmt(lower_expr(expr, scopes)),
        HirStmt::Assign { target, value } => {
            let value = lower_expr(value, scopes);
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
        } => {
            let value = lower_expr(value, scopes);
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
            } else {
                MirExpr::IntBoundary(Box::new(value))
            };
            bind_variable(scopes, target.clone(), annotation.clone());
            MirStmt::Assign {
                target: target.clone(),
                value,
            }
        }
        HirStmt::AnnAssign { value: None, .. } => MirStmt::NoOp,
        HirStmt::If { test, body, orelse } => MirStmt::If {
            test: lower_expr(test, scopes),
            body: body.iter().map(|s| lower_stmt(s, scopes)).collect(),
            orelse: orelse.iter().map(|s| lower_stmt(s, scopes)).collect(),
        },
        HirStmt::While { test, body } => MirStmt::While {
            test: lower_expr(test, scopes),
            body: body.iter().map(|s| lower_stmt(s, scopes)).collect(),
        },
        HirStmt::ForRange {
            var,
            start,
            stop,
            step,
            body,
        } => {
            let start = lower_expr(start, scopes);
            let stop = lower_expr(stop, scopes);
            let step = lower_expr(step, scopes);
            bind_variable(scopes, var.clone(), Ty::Int);
            let body = body.iter().map(|s| lower_stmt(s, scopes)).collect();
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
                    let body = body.iter().map(|s| lower_stmt(s, scopes)).collect();
                    MirStmt::ForList {
                        var: var.clone(),
                        list: list.clone(),
                        body,
                    }
                }
                Ty::Dict(kv) => {
                    bind_variable(scopes, var.clone(), kv.0);
                    let body = body.iter().map(|s| lower_stmt(s, scopes)).collect();
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
                    let body = body.iter().map(|s| lower_stmt(s, scopes)).collect();
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
            let (source, var_ty) = resolve_comp_source(iter, var, scopes);
            let cond = cond.as_deref().map(|c| lower_expr(c, scopes));
            let elt = lower_expr(elt, scopes);
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
            let (source, var_ty) = resolve_comp_source(iter, var, scopes);
            let cond = cond.as_deref().map(|c| lower_expr(c, scopes));
            let elt = lower_expr(elt, scopes);
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
            let (source, var_ty) = resolve_comp_source(iter, var, scopes);
            let cond = cond.as_deref().map(|c| lower_expr(c, scopes));
            let key = lower_expr(key, scopes);
            let value = lower_expr(value, scopes);
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
        HirStmt::Return(value) => MirStmt::Return(value.as_ref().map(|v| lower_expr(v, scopes))),
        HirStmt::DictSet { dict, key, value } => MirStmt::DictSet {
            dict: dict.clone(),
            key: lower_expr(key, scopes),
            value: lower_expr(value, scopes),
        },
    }
}

fn lower_expr(expr: &HirExpr, scopes: &[HashMap<String, Ty>]) -> MirExpr {
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
            let args: Vec<MirExpr> = args.iter().map(|a| lower_expr(a, scopes)).collect();
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
            let left = lower_expr(left, scopes);
            let right = lower_expr(right, scopes);
            let ty = binop_result_ty(*op, left.ty(), right.ty());
            MirExpr::BinOp {
                op: *op,
                left: Box::new(left),
                right: Box::new(right),
                ty,
            }
        }
        HirExpr::Compare { op, left, right } => MirExpr::Compare {
            op: *op,
            left: Box::new(lower_expr(left, scopes)),
            right: Box::new(lower_expr(right, scopes)),
            ty: Ty::Bool,
        },
        HirExpr::FString(parts) => MirExpr::FString(
            parts
                .iter()
                .map(|p| match p {
                    FStringPart::Literal(s) => MirFStringPart::Literal(s.clone()),
                    FStringPart::Interpolation(e) => {
                        MirFStringPart::Interpolation(Box::new(lower_expr(e, scopes)))
                    }
                })
                .collect(),
        ),
        HirExpr::ListLiteral(elements) => {
            MirExpr::ListLiteral(elements.iter().map(|e| lower_expr(e, scopes)).collect())
        }
        // `HirExpr::Subscript` is reused unconditionally by `pycc_hir`'s own
        // lowering for both a list read and a dict read (it has no type
        // information to pick a different node) -- `pycc_types::infer_expr_in`
        // accepts either base equally (see its own `Subscript` arm), so this
        // is the point where the real type is resolved and where a
        // dict-typed base is routed into `MirExpr::DictGet` instead of
        // `MirExpr::Subscript`, mirroring `lower_stmt`'s own `HirStmt::ForList`
        // arm doing the same list/dict routing for iteration.
        HirExpr::Subscript { base, index } => {
            let base = lower_expr(base, scopes);
            let index = lower_expr(index, scopes);
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
            value: Box::new(lower_expr(value, scopes)),
        },
        HirExpr::DictLiteral(pairs) => MirExpr::DictLiteral(
            pairs
                .iter()
                .map(|(k, v)| (lower_expr(k, scopes), lower_expr(v, scopes)))
                .collect(),
        ),
        HirExpr::SetLiteral(elements) => {
            MirExpr::SetLiteral(elements.iter().map(|e| lower_expr(e, scopes)).collect())
        }
        HirExpr::TupleLiteral(elements) => {
            MirExpr::TupleLiteral(elements.iter().map(|e| lower_expr(e, scopes)).collect())
        }
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
            base: Box::new(lower_expr(base, scopes)),
            start: start.as_deref().map(|e| Box::new(lower_expr(e, scopes))),
            stop: stop.as_deref().map(|e| Box::new(lower_expr(e, scopes))),
            step: step.as_deref().map(|e| Box::new(lower_expr(e, scopes))),
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
                key: Box::new(lower_expr(key, scopes)),
                default: Box::new(lower_expr(default, scopes)),
                ty: kv.1,
            }
        }
        HirExpr::SetAdd { set, value } => MirExpr::SetAdd {
            set: set.clone(),
            value: Box::new(lower_expr(value, scopes)),
        },
    }
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
    use pycc_hir::{BinOpKind, CmpOpKind, FStringPart, HirExpr, HirItem, HirModule, HirStmt, Ty};

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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
                    target: "x".to_string(),
                    annotation: Ty::Int,
                    value: Some(HirExpr::IntLiteral(1)),
                }),
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Name("x".to_string()))),
            ],
         type_aliases: Vec::new(), imports: Vec::new(),};
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
                    target: "x".to_string(),
                    annotation: Ty::Int,
                    value: Some(HirExpr::BoolLiteral(true)),
                }),
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Name("x".to_string()))),
            ],
         type_aliases: Vec::new(), imports: Vec::new(),};
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
                target: "x".to_string(),
                annotation: Ty::Int,
                value: Some(HirExpr::Compare {
                    op: CmpOpKind::Lt,
                    left: Box::new(HirExpr::IntLiteral(1)),
                    right: Box::new(HirExpr::IntLiteral(2)),
                }),
            })],
         type_aliases: Vec::new(), imports: Vec::new(),};
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
                target: "y".to_string(),
                annotation: Ty::Int,
                value: None,
            })],
         type_aliases: Vec::new(), imports: Vec::new(),};
        let mir = build(&hir);
        assert_eq!(mir.items, vec![MirItem::TopLevelStmt(MirStmt::NoOp)]);
    }

    #[test]
    #[should_panic(expected = "has no recorded type")]
    fn a_value_less_annotated_assignment_does_not_bind_the_name() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::AnnAssign {
                    target: "y".to_string(),
                    annotation: Ty::Int,
                    value: None,
                }),
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Name("y".to_string()))),
            ],
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
    }

    #[test]
    fn lowers_list_literal_to_mir() {
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)]),
            })],
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
            type_aliases: Vec::new(), imports: Vec::new(),
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
         type_aliases: Vec::new(), imports: Vec::new(),};
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
}
