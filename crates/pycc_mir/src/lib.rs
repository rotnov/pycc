pub use pycc_hir::HirClassDef;
mod class;
#[cfg(test)]
use class::eval_isinstance_protocol;
use class::{class_def_of, mro_attrs};
mod exception;
pub use exception::{MirExceptHandler, MirExceptionValue};
use exception::{handler_type_tags, lower_raise};
mod expr;
use expr::lower_expr;
mod matching;
#[cfg(test)]
use matching::nest_match_alternatives;
use matching::try_lower_enum_member_attr;
mod stmt;
use pycc_hir::{CompIter, HirItem, HirModule};
use std::collections::HashMap;
use std::sync::atomic::AtomicUsize;
use stmt::lower_stmt;

// Re-exported (not just `use`d) because `pycc_codegen` doesn't depend on
// `pycc_hir` directly (see its Cargo.toml) -- `Ty`, `BinOpKind`, and
// `CmpOpKind` all reach this crate's public API through `MirExpr`'s fields
// (`Name`/`Call` carry a `Ty`; `BinOp` carries a `BinOpKind`; `Compare`
// carries a `CmpOpKind`), so each must be nameable as
// `pycc_mir::{Ty, BinOpKind, CmpOpKind}` from any downstream crate, exactly
// like `pycc_types` already re-exports `Ty` (`pycc_types::Ty`, its own line
// 4) for the same reason.
pub use pycc_hir::{BinOpKind, CmpOpKind, EXCEPTION_GROUP_TYPE_TAG, Ty};

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
    /// The bare literal `None` (D-197, #763, Part 1 of #747). Statically
    /// `Ty::None` on its own (mirroring `HirExpr::NoneLiteral`) -- when it
    /// (or a bare `inner`-typed value) flows into an `Optional[inner]`-typed
    /// slot, `pycc_codegen`'s `coerce_scalar_to_type` builds that slot's
    /// `{ inner, i8 }` present/absent representation, driven by the target
    /// type it is asked to coerce to. `OptionalWrap` below is the one place
    /// that target type is *not* simply "the slot's already-established
    /// type" -- see its own doc comment.
    NoneLiteral,
    /// Wraps a bare `inner`-typed value or a `NoneLiteral` so `.ty()`
    /// reports `Ty::Optional(inner)` regardless of the wrapped value's own
    /// static type (D-197, #763, Part 1 of #747). Exactly mirroring
    /// `IntBoundary`'s own "first assignment fixes a binding's
    /// representation" reason: `pycc_codegen::collect_stmt_bindings`
    /// derives a `MirStmt::Assign`'s slot type from `value.ty()` alone, with
    /// no memory of the HIR annotation that produced it, so an
    /// `AnnAssign`'s initializer under an `Optional[inner]` annotation must
    /// itself statically report `Ty::Optional(inner)` for the slot to be
    /// declared with the right representation at all -- unlike `IntBoundary`,
    /// the actual `{ inner, i8 }` struct-building work still happens at
    /// codegen (`pycc_codegen::coerce_scalar_to_type`, called with
    /// `Ty::Optional(inner)` as the target), not here; this node exists
    /// purely to fix `.ty()`. Only `pycc_mir::stmt::lower_stmt`'s
    /// `AnnAssign` arm introduces it, for the identical reason `IntBoundary`
    /// is introduced only there and not by a bare `Assign`: a later plain
    /// reassignment to an already-`Optional`-typed name (`x = None` after
    /// `x: int | None = 5`) needs no wrapper at all, because
    /// `pycc_mir::bind_variable`'s `or_insert` never overwrites the slot
    /// type the `AnnAssign` already fixed (D-074), and
    /// `coerce_scalar_to_type` is driven by that already-correct slot type
    /// directly at the assignment site either way.
    OptionalWrap(Box<MirExpr>, Box<Ty>),
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
    /// for "allocate, then call" -- unlike `MethodCall` (see this crate's
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
    /// Reads a caught exception's own message string (Part 3A of #541,
    /// #736): `str(e)` semantics -- the message alone, never
    /// `exception_print_and_exit`'s own uncaught-exception `"{type}:
    /// {message}"` format, which this node has nothing to do with. The
    /// boxed inner expression is the exception-typed value (typically a
    /// `MirExpr::Name` reference to an `except ... as e:` handler binding);
    /// `pycc_mir::class::rewrite_exception_to_message` is the sole
    /// constructor, applied at the same two lowering sites
    /// `rewrite_instance_to_repr` is (a `print` argument and an f-string
    /// interpolation) whenever the expression's static type names a
    /// registered exception class, so codegen's `to_str` always receives a
    /// `str` scalar for a caught exception rather than reaching its
    /// `Scalar::Instance` panic arm. `.ty()` below always reports
    /// `Ty::Str`, exactly like `rewrite_instance_to_repr`'s own
    /// `MirExpr::Call` rewrite.
    ExceptionMessage(Box<MirExpr>),
    /// PEP 572 (#774): `target := value`. Evaluates `value`, stores it into
    /// `name`'s already-predeclared storage slot (see
    /// `pycc_codegen::collect_expr_bindings`, this node's own slot-scanning
    /// counterpart to `collect_stmt_bindings`'s `MirStmt::Assign` handling),
    /// and yields that same value as the expression's own result -- the
    /// "evaluate, store, yield" shape the issue's own design calls for,
    /// distinct from every other `MirExpr` variant (all of which are pure
    /// reads/computations with no store side effect). `name` is restricted
    /// (at the `pycc_hir` boundary, before this node is ever constructed) to
    /// exactly three statement placements -- an `if`/`while` test condition
    /// or a bare expression statement, at any expression nesting depth
    /// within those -- so `pycc_types::collect_named_expr_bindings` and this
    /// crate's own `pycc_mir::stmt::collect_named_expr_bindings` only ever
    /// need to hoist-and-bind at those three statement sites. `ty` is
    /// `value`'s own type, exactly mirroring an assignment statement's RHS
    /// typing rule (see `pycc_types::expr::infer_expr_in`'s own `NamedExpr`
    /// arm).
    NamedExpr {
        name: String,
        value: Box<MirExpr>,
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
            MirExpr::NoneLiteral => Ty::None,
            MirExpr::OptionalWrap(_, inner) => Ty::Optional(inner.clone()),
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
            MirExpr::ExceptionMessage(_) => Ty::Str,
            MirExpr::NamedExpr { ty, .. } => ty.clone(),
        }
    }

    /// PEP 572 (#774): walks every `MirExpr::NamedExpr` reachable from
    /// `self`, at any nesting depth, appending `(name, ty)` for each one
    /// found (evaluation order, left to right/outer to inner as the fields
    /// are declared -- a walrus target is rebindable, so a later entry for
    /// the same `name` naturally overrides an earlier one when both are
    /// later applied via `bind_variable`'s "last write wins" semantics).
    /// `pycc_hir::stmt::lower_stmt`'s own `contains_named_expr` restriction
    /// guarantees a `NamedExpr` node only ever exists nested inside exactly
    /// three statement placements (an `if`/`while` test or a bare
    /// expression statement) -- this walker itself makes no such
    /// assumption and is exhaustive over every `MirExpr` variant so that
    /// adding a new variant without touching this function is a compile
    /// error, not a silently-skipped binding. Two independent call sites
    /// need this exact same walk: `pycc_mir::stmt::lower_stmt` (to
    /// `bind_variable` each name into `scopes` before lowering the
    /// statement's own body/next statement) and
    /// `pycc_codegen::collect_expr_bindings` (to predeclare each name's
    /// storage slot, mirroring `collect_stmt_bindings`'s own
    /// `MirStmt::Assign` handling) -- defined once, here, rather than
    /// duplicated in both crates.
    pub fn collect_named_expr_bindings(&self, out: &mut Vec<(String, Ty)>) {
        match self {
            MirExpr::IntLiteral(_)
            | MirExpr::FloatLiteral(_)
            | MirExpr::BoolLiteral(_)
            | MirExpr::StringLiteral(_)
            | MirExpr::NoneLiteral
            | MirExpr::Name { .. }
            | MirExpr::ListPop { .. }
            | MirExpr::NullInstance { .. } => {}
            MirExpr::IntBoundary(inner) => inner.collect_named_expr_bindings(out),
            MirExpr::OptionalWrap(inner, _) => inner.collect_named_expr_bindings(out),
            MirExpr::Call { args, .. } => {
                for arg in args {
                    arg.collect_named_expr_bindings(out);
                }
            }
            MirExpr::BinOp { left, right, .. } | MirExpr::Compare { left, right, .. } => {
                left.collect_named_expr_bindings(out);
                right.collect_named_expr_bindings(out);
            }
            MirExpr::FString(parts) => {
                for part in parts {
                    if let MirFStringPart::Interpolation(inner) = part {
                        inner.collect_named_expr_bindings(out);
                    }
                }
            }
            MirExpr::ListLiteral(elements)
            | MirExpr::SetLiteral(elements)
            | MirExpr::TupleLiteral(elements) => {
                for element in elements {
                    element.collect_named_expr_bindings(out);
                }
            }
            MirExpr::Subscript { base, index } => {
                base.collect_named_expr_bindings(out);
                index.collect_named_expr_bindings(out);
            }
            MirExpr::ListAppend { value, .. } | MirExpr::SetAdd { value, .. } => {
                value.collect_named_expr_bindings(out);
            }
            MirExpr::DictLiteral(pairs) => {
                for (key, value) in pairs {
                    key.collect_named_expr_bindings(out);
                    value.collect_named_expr_bindings(out);
                }
            }
            MirExpr::DictGet { dict, key } => {
                dict.collect_named_expr_bindings(out);
                key.collect_named_expr_bindings(out);
            }
            MirExpr::Slice {
                base,
                start,
                stop,
                step,
            } => {
                base.collect_named_expr_bindings(out);
                for bound in [start, stop, step].into_iter().flatten() {
                    bound.collect_named_expr_bindings(out);
                }
            }
            MirExpr::DictGetOrDefault { key, default, .. } => {
                key.collect_named_expr_bindings(out);
                default.collect_named_expr_bindings(out);
            }
            MirExpr::Instantiate(inst) => {
                for arg in &inst.args {
                    arg.collect_named_expr_bindings(out);
                }
            }
            MirExpr::AttrGet { base, .. } => base.collect_named_expr_bindings(out),
            MirExpr::ExceptionMessage(inner) => inner.collect_named_expr_bindings(out),
            MirExpr::NamedExpr { name, value, ty } => {
                value.collect_named_expr_bindings(out);
                out.push((name.clone(), ty.clone()));
            }
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
    /// Each handler's `exc_type_tag` is the sorted set of runtime exception
    /// type tags it accepts — the named class's own tag (matching `pycc_rt`'s
    /// `EXCEPTION_TYPE_*` constants for the seven builtins, or the tag HIR
    /// lowering assigned the class for a user-defined one) plus every
    /// raisable subclass's tag — or `None` for a bare `except:`.
    Try {
        body: Vec<MirStmt>,
        handlers: Vec<MirExceptHandler>,
        orelse: Vec<MirStmt>,
        finalbody: Vec<MirStmt>,
    },
    /// `try: ... except* T: ...` (PEP 654, Part 3 of #382, #542). Shares
    /// `Try`'s shape exactly -- `MirExceptHandler`'s `exc_type_tag` still
    /// carries the matched-type tag set (always `Some`, since `except*`
    /// requires a named type; see `pycc_hir::stmt`'s comment on the
    /// matching lowering site) -- but codegen dispatches each handler with
    /// a partition (`pycc_rt_exception_group_partition`) against the raised
    /// group rather than `Try`'s mutually-exclusive `type_matches` checks,
    /// and every matched subgroup's handler can run (PEP 654 semantics: more
    /// than one `except*` clause may fire for the same raised group), unlike
    /// `Try`'s first-match-wins dispatch.
    TryStar {
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
