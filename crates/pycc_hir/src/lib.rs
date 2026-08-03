use pycc_ast::{CmpOp, ElifElseClause, Expr, ModModule, Number, Operator, Stmt};
use pycc_diag::{Diagnostic, Span};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ty {
    Int,
    Float,
    Bool,
    Str,
    None,
    Infer,
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
            Ty::List(elem) => format!("list[{}]", elem.name()),
            Ty::Dict(kv) => format!("dict[{}, {}]", kv.0.name(), kv.1.name()),
            Ty::Set(elem) => format!("set[{}]", elem.name()),
            Ty::Tuple(elems) => format!(
                "tuple[{}]",
                elems.iter().map(Ty::name).collect::<Vec<_>>().join(", ")
            ),
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
    /// constructing this variant for it (PR-11 Task 3, D-113 -- `list[int]`
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
    /// prints `None`, matching CPython. Only `y = x.append(2)`'s
    /// assignment path currently surfaces a problem, and even that is not
    /// a rejection: `pycc_codegen`'s `collect_stmt_bindings` has no
    /// `Ty::None` arm, so it hits the same internal-error panic
    /// ("every assignment target must have a predeclared storage slot")
    /// already accepted for `y = f()` on a `None`-returning `f` (D-072) --
    /// a pre-existing gap this variant newly reaches, tracked as its own
    /// issue rather than fixed here.
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
    /// entirely rather than half-solving it.
    DictGetOrDefault {
        dict: String,
        key: Box<HirExpr>,
        default: Box<HirExpr>,
    },
    /// `set.add(value)` (PR-12, D-119): mirrors `ListAppend`'s shape and its
    /// `None`-producing/value-position quirk exactly (see `ListAppend`'s own
    /// doc comment above -- `y = s.add(1)` reaches the same
    /// `collect_stmt_bindings` `Ty::None`-binding gap `ListAppend` already
    /// does, not a new one) -- dedups on insert, exactly like set-literal
    /// construction already does (`pycc_rt_int_set_add`, already shipped by
    /// PR-11a), from a second, user-facing call site added by a later task.
    SetAdd {
        set: String,
        value: Box<HirExpr>,
    },
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
    /// dict>:` (PR-11 Task 3, D-113: iterates keys in insertion order) --
    /// this lowering step has no type information to distinguish a
    /// `list`-typed iterable from a `dict`-typed one, so `pycc_types`'
    /// own `ForList` arms resolve `list`'s real type to `Ty::List`,
    /// `Ty::Dict` (binding `var` as the key type), or reject it, not here.
    ForList {
        var: String,
        list: String,
        body: Vec<HirStmt>,
    },
    /// `<bare name>[key] = value`, PR-11 Task 3 (D-113 supersedes D-105's
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

#[derive(Debug, Clone, PartialEq)]
pub struct HirModule {
    pub items: Vec<HirItem>,
}

/// Lowers a parsed module into the HIR subset implemented by this pycc
/// version. Syntactically valid Python outside that subset returns `C0001`
/// with the unsupported node's source span instead of panicking.
pub fn lower_checked(module: &ModModule) -> Result<HirModule, Diagnostic> {
    let items = module
        .body
        .iter()
        .map(|stmt| match stmt {
            Stmt::FunctionDef(def) => lower_function(def),
            other => lower_stmt(other).map(HirItem::TopLevelStmt),
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(HirModule { items })
}

fn lower_function(def: &pycc_ast::StmtFunctionDef) -> Result<HirItem, Diagnostic> {
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
    if def.type_params.is_some() {
        return Err(unsupported(
            "generic function type parameters are not supported yet",
            def.range,
        ));
    }
    let is_public = !def.name.as_str().starts_with('_'); // D-038
    let params = lower_params(&def.parameters, is_public, def.name.as_str())?;
    let return_ty = lower_return_annotation(def.returns.as_deref(), is_public, def.name.as_str())?;
    let body = lower_body(&def.body)?;
    Ok(HirItem::Function {
        name: def.name.to_string(),
        params,
        return_ty,
        body,
    })
}

fn lower_params(
    parameters: &pycc_ast::Parameters,
    is_public: bool,
    fn_name: &str,
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
    parameters
        .args
        .iter()
        .map(|param| {
            if param.default.is_some() {
                return Err(unsupported(
                    "default parameter values are not supported yet",
                    param.range,
                ));
            }
            let name = param.parameter.name.as_str();
            match &param.parameter.annotation {
                Some(ann) => Ok((name.to_string(), annotation_to_ty(ann)?)),
                None if is_public => Err(Diagnostic::error(
                    "T0001",
                    format!(
                        "parameter `{name}` of public function `{fn_name}` needs a type annotation"
                    ),
                    Span::new(0, 0),
                )),
                None => Ok((name.to_string(), Ty::Infer)),
            }
        })
        .collect()
}

fn lower_return_annotation(
    returns: Option<&Expr>,
    is_public: bool,
    fn_name: &str,
) -> Result<Ty, Diagnostic> {
    match returns {
        Some(ann) => annotation_to_ty(ann),
        None if is_public => Err(Diagnostic::error(
            "T0001",
            format!("public function `{fn_name}` needs a return type annotation"),
            Span::new(0, 0),
        )),
        None => Ok(Ty::Infer),
    }
}

fn annotation_to_ty(annotation: &Expr) -> Result<Ty, Diagnostic> {
    match annotation {
        Expr::NoneLiteral(_) => Ok(Ty::None),
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
            other => Err(unsupported(
                format!("type annotation `{other}` is not supported yet"),
                pycc_ast::expr_range(annotation),
            )),
        },
        other => Err(unsupported(
            format!("only a bare name type annotation is supported so far: {other:?}"),
            pycc_ast::expr_range(other),
        )),
    }
}

fn lower_stmt(stmt: &Stmt) -> Result<HirStmt, Diagnostic> {
    let lowered = match stmt {
        Stmt::Expr(expr_stmt) => HirStmt::ExprStmt(lower_expr(&expr_stmt.value)?),
        Stmt::Assign(assign) => {
            let [target] = assign.targets.as_slice() else {
                return Err(unsupported(
                    format!(
                        "only a single assignment target is supported so far: {:?}",
                        assign.targets
                    ),
                    assign.range,
                ));
            };
            match target {
                Expr::Name(name) => match assign.value.as_ref() {
                    // Comprehension expressions are recognized only in this
                    // one position: the direct RHS of a bare-name
                    // `Stmt::Assign` (PR-12, D-117). Every other position
                    // (function args, nested expressions, `return`,
                    // `Expr::Subscript` assignment targets) still routes
                    // through plain `lower_expr`, which has no arm for
                    // `Expr::ListComp`/`SetComp`/`DictComp`/`GeneratorExp`
                    // and falls through to that function's existing
                    // generic "expression kind not supported yet"
                    // catch-all.
                    Expr::ListComp(comp) => lower_list_comp_assign(name.id.as_str(), comp)?,
                    Expr::SetComp(comp) => lower_set_comp_assign(name.id.as_str(), comp)?,
                    Expr::DictComp(comp) => lower_dict_comp_assign(name.id.as_str(), comp)?,
                    _ => HirStmt::Assign {
                        target: name.id.as_str().to_string(),
                        value: lower_expr(&assign.value)?,
                    },
                },
                // `<bare name>[key] = value`, PR-11 Task 3 (D-113): unlike
                // `list[int]`'s own read-only-indexing consequence (D-105),
                // `dict[str, int]` ships `d[k] = v`. This lowering step has
                // no type information (mirroring `ForList`'s own bare-name
                // iterable, which is resolved to `Ty::List`, `Ty::Dict`, or
                // rejected downstream), so a `list[int]` subscript-assignment target
                // also reaches `HirStmt::DictSet` here -- `pycc_types`
                // rejects it with `T0033` once the base's real type is
                // known, relocating (not removing) the invariant this file's
                // own `subscript_assignment_target_is_unsupported` test used
                // to enforce at the lowering level.
                Expr::Subscript(sub) => {
                    let Expr::Name(base_name) = sub.value.as_ref() else {
                        return Err(unsupported(
                            "only assigning to a bare-name subscript target (`name[key] = value`) is supported so far",
                            pycc_ast::expr_range(target),
                        ));
                    };
                    HirStmt::DictSet {
                        dict: base_name.id.as_str().to_string(),
                        key: lower_expr(&sub.slice)?,
                        value: lower_expr(&assign.value)?,
                    }
                }
                other => {
                    return Err(unsupported(
                        format!("only assigning to a bare name is supported so far: {other:?}"),
                        pycc_ast::expr_range(other),
                    ));
                }
            }
        }
        Stmt::AnnAssign(ann) => {
            let Expr::Name(name) = ann.target.as_ref() else {
                return Err(unsupported(
                    format!(
                        "only assigning to a bare name is supported so far: {:?}",
                        ann.target
                    ),
                    pycc_ast::expr_range(&ann.target),
                ));
            };
            // `ann.simple` is false either when the target isn't a bare name
            // (already rejected above) or when a bare name target is itself
            // parenthesized, e.g. `(x): int = 1` -- upstream's own parser
            // sets `simple = target.is_name_expr() && !target.is_parenthesized`
            // (verified against the pinned ruff_python_parser = "0.0.6"
            // registry source). CPython treats a parenthesized target as not
            // "simple" (it doesn't record a `__annotations__` entry the same
            // way), a real semantic difference this compiler doesn't model
            // yet -- reject explicitly instead of silently treating it the
            // same as the unparenthesized form.
            if !ann.simple {
                return Err(unsupported(
                    "a parenthesized annotated-assignment target is not supported yet",
                    pycc_ast::expr_range(&ann.target),
                ));
            }
            let annotation = annotation_to_ty(&ann.annotation)?;
            let value = ann.value.as_deref().map(lower_expr).transpose()?;
            HirStmt::AnnAssign {
                target: name.id.as_str().to_string(),
                annotation,
                value,
            }
        }
        Stmt::If(if_stmt) => HirStmt::If {
            test: lower_expr(&if_stmt.test)?,
            body: lower_body(&if_stmt.body)?,
            orelse: lower_elif_else_clauses(&if_stmt.elif_else_clauses)?,
        },
        Stmt::While(while_stmt) => {
            if !while_stmt.orelse.is_empty() {
                return Err(unsupported(
                    "while/else is not supported yet",
                    while_stmt.range,
                ));
            }
            HirStmt::While {
                test: lower_expr(&while_stmt.test)?,
                body: lower_body(&while_stmt.body)?,
            }
        }
        Stmt::For(for_stmt) => {
            if for_stmt.is_async {
                return Err(unsupported(
                    "async for is not supported yet",
                    for_stmt.range,
                ));
            }
            if !for_stmt.orelse.is_empty() {
                return Err(unsupported("for/else is not supported yet", for_stmt.range));
            }
            let Expr::Name(var) = for_stmt.target.as_ref() else {
                return Err(unsupported(
                    format!(
                        "only a bare name for-target is supported so far: {:?}",
                        for_stmt.target
                    ),
                    pycc_ast::expr_range(&for_stmt.target),
                ));
            };
            // A bare-name iterable is `for v in some_list:` (D-105) or
            // `for k in some_dict:` (PR-11 Task 3, D-113) -- resolved to
            // `Ty::List`, `Ty::Dict`, or rejected by pycc_types, not here;
            // HIR only records the syntactic shape.
            if let Expr::Name(list_name) = for_stmt.iter.as_ref() {
                return Ok(HirStmt::ForList {
                    var: var.id.to_string(),
                    list: list_name.id.as_str().to_string(),
                    body: lower_body(&for_stmt.body)?,
                });
            }
            let Expr::Call(call) = for_stmt.iter.as_ref() else {
                return Err(unsupported(
                    format!(
                        "only `for x in range(...)` or `for x in <list>` is supported so far: {:?}",
                        for_stmt.iter
                    ),
                    pycc_ast::expr_range(&for_stmt.iter),
                ));
            };
            let Expr::Name(callee) = call.func.as_ref() else {
                return Err(unsupported(
                    format!(
                        "only `for x in range(...)` is supported so far: {:?}",
                        call.func
                    ),
                    pycc_ast::expr_range(&call.func),
                ));
            };
            if callee.id.as_str() != "range" {
                return Err(unsupported(
                    format!(
                        "only iterating over `range(...)` is supported so far, got `{}`",
                        callee.id
                    ),
                    call.range,
                ));
            }
            if !call.arguments.keywords.is_empty() {
                return Err(unsupported(
                    "keyword arguments to range() are not supported yet",
                    call.range,
                ));
            }
            let (start, stop, step) = lower_range_call(call)?;
            HirStmt::ForRange {
                var: var.id.to_string(),
                start,
                stop,
                step,
                body: lower_body(&for_stmt.body)?,
            }
        }
        Stmt::Return(ret) => HirStmt::Return(ret.value.as_deref().map(lower_expr).transpose()?),
        other => {
            return Err(unsupported(
                "statement kind not supported yet",
                pycc_ast::stmt_range(other),
            ));
        }
    };
    Ok(lowered)
}

fn lower_body(body: &[Stmt]) -> Result<Vec<HirStmt>, Diagnostic> {
    body.iter().map(lower_stmt).collect()
}

fn lower_elif_else_clauses(clauses: &[ElifElseClause]) -> Result<Vec<HirStmt>, Diagnostic> {
    let Some((first, rest)) = clauses.split_first() else {
        return Ok(vec![]);
    };
    match &first.test {
        Some(test) => Ok(vec![HirStmt::If {
            test: lower_expr(test)?,
            body: lower_body(&first.body)?,
            orelse: lower_elif_else_clauses(rest)?,
        }]),
        None => {
            assert!(
                rest.is_empty(),
                "pycc_hir: an else clause must be the last elif_else_clause"
            );
            lower_body(&first.body)
        }
    }
}

fn lower_expr(expr: &Expr) -> Result<HirExpr, Diagnostic> {
    let lowered = match expr {
        Expr::NumberLiteral(lit) => match &lit.value {
            Number::Int(i) => {
                let Some(value) = i.as_i64() else {
                    return Err(unsupported(
                        format!("integer literal does not fit in i64: {i:?}"),
                        lit.range,
                    ));
                };
                HirExpr::IntLiteral(value)
            }
            Number::Float(f) => HirExpr::FloatLiteral(*f),
            other => {
                return Err(unsupported(
                    format!("numeric literal kind not supported yet: {other:?}"),
                    lit.range,
                ));
            }
        },
        Expr::Name(name) => HirExpr::Name(name.id.as_str().to_string()),
        Expr::List(list) => HirExpr::ListLiteral(
            list.elts
                .iter()
                .map(lower_expr)
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Expr::Dict(dict) => HirExpr::DictLiteral(
            dict.items
                .iter()
                .map(|item| {
                    let Some(key) = &item.key else {
                        return Err(unsupported(
                            "dict-unpacking (`**expr`) inside a dict literal is not supported yet",
                            pycc_ast::expr_range(&item.value),
                        ));
                    };
                    Ok((lower_expr(key)?, lower_expr(&item.value)?))
                })
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Expr::Set(set) => HirExpr::SetLiteral(
            set.elts
                .iter()
                .map(lower_expr)
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Expr::Tuple(tuple) => HirExpr::TupleLiteral(
            tuple
                .elts
                .iter()
                .map(lower_expr)
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Expr::Subscript(sub) => match sub.slice.as_ref() {
            // A colon-containing subscript (`xs[a:b:c]`) parses its `slice`
            // field as `Expr::Slice`, distinct from the plain single-
            // expression `slice` an ordinary index (`xs[0]`) produces
            // (PR-12, D-118). Each bound is independently optional in real
            // Python's own grammar, so each is lowered through
            // `Option::map`/`.transpose()` rather than assumed present.
            Expr::Slice(slice) => HirExpr::Slice {
                base: Box::new(lower_expr(&sub.value)?),
                start: slice
                    .lower
                    .as_deref()
                    .map(lower_expr)
                    .transpose()?
                    .map(Box::new),
                stop: slice
                    .upper
                    .as_deref()
                    .map(lower_expr)
                    .transpose()?
                    .map(Box::new),
                step: slice
                    .step
                    .as_deref()
                    .map(lower_expr)
                    .transpose()?
                    .map(Box::new),
            },
            _ => HirExpr::Subscript {
                base: Box::new(lower_expr(&sub.value)?),
                index: Box::new(lower_expr(&sub.slice)?),
            },
        },
        Expr::Call(call) => {
            if !call.arguments.keywords.is_empty() {
                return Err(unsupported(
                    "keyword call arguments are not supported yet",
                    call.range,
                ));
            }
            if let Expr::Attribute(attr) = call.func.as_ref() {
                if attr.attr.as_str() == "append" {
                    let Expr::Name(list_name) = attr.value.as_ref() else {
                        return Err(unsupported(
                            "`.append()` is only supported on a bare-name list so far",
                            pycc_ast::expr_range(&attr.value),
                        ));
                    };
                    let [value] = &*call.arguments.args else {
                        return Err(unsupported(
                            format!(
                                "list.append() takes exactly one argument, got {}",
                                call.arguments.args.len()
                            ),
                            call.range,
                        ));
                    };
                    return Ok(HirExpr::ListAppend {
                        list: list_name.id.as_str().to_string(),
                        value: Box::new(lower_expr(value)?),
                    });
                }
                if attr.attr.as_str() == "pop" {
                    let Expr::Name(list_name) = attr.value.as_ref() else {
                        return Err(unsupported(
                            "`.pop()` is only supported on a bare-name list so far",
                            pycc_ast::expr_range(&attr.value),
                        ));
                    };
                    let [] = &*call.arguments.args else {
                        return Err(unsupported(
                            format!(
                                "list.pop() takes no arguments, got {}",
                                call.arguments.args.len()
                            ),
                            call.range,
                        ));
                    };
                    return Ok(HirExpr::ListPop {
                        list: list_name.id.as_str().to_string(),
                    });
                }
                if attr.attr.as_str() == "get" {
                    let Expr::Name(dict_name) = attr.value.as_ref() else {
                        return Err(unsupported(
                            "`.get()` is only supported on a bare-name dict so far",
                            pycc_ast::expr_range(&attr.value),
                        ));
                    };
                    let [key, default] = &*call.arguments.args else {
                        return Err(unsupported(
                            format!(
                                "dict.get() takes exactly two arguments (key, default), got {}",
                                call.arguments.args.len()
                            ),
                            call.range,
                        ));
                    };
                    return Ok(HirExpr::DictGetOrDefault {
                        dict: dict_name.id.as_str().to_string(),
                        key: Box::new(lower_expr(key)?),
                        default: Box::new(lower_expr(default)?),
                    });
                }
                if attr.attr.as_str() == "add" {
                    let Expr::Name(set_name) = attr.value.as_ref() else {
                        return Err(unsupported(
                            "`.add()` is only supported on a bare-name set so far",
                            pycc_ast::expr_range(&attr.value),
                        ));
                    };
                    let [value] = &*call.arguments.args else {
                        return Err(unsupported(
                            format!(
                                "set.add() takes exactly one argument, got {}",
                                call.arguments.args.len()
                            ),
                            call.range,
                        ));
                    };
                    return Ok(HirExpr::SetAdd {
                        set: set_name.id.as_str().to_string(),
                        value: Box::new(lower_expr(value)?),
                    });
                }
                return Err(unsupported(
                    format!(
                        "only the `.append()`/`.pop()`/`.get()`/`.add()` methods are supported so far, got `.{}(...)`",
                        attr.attr
                    ),
                    call.range,
                ));
            }
            let Expr::Name(callee) = call.func.as_ref() else {
                return Err(unsupported(
                    format!(
                        "only calling a bare name is supported so far: {:?}",
                        call.func
                    ),
                    pycc_ast::expr_range(&call.func),
                ));
            };
            let args = call
                .arguments
                .args
                .iter()
                .map(lower_expr)
                .collect::<Result<Vec<_>, _>>()?;
            HirExpr::Call {
                callee: callee.id.as_str().to_string(),
                args,
            }
        }
        Expr::BinOp(bin_op) => {
            let op = match bin_op.op {
                Operator::Add => BinOpKind::Add,
                Operator::Sub => BinOpKind::Sub,
                Operator::Mult => BinOpKind::Mul,
                Operator::Div => BinOpKind::Div,
                Operator::FloorDiv => BinOpKind::FloorDiv,
                Operator::Mod => BinOpKind::Mod,
                Operator::Pow => BinOpKind::Pow,
                other => {
                    return Err(unsupported(
                        format!("binary operator not supported yet: {other:?}"),
                        bin_op.range,
                    ));
                }
            };
            HirExpr::BinOp {
                op,
                left: Box::new(lower_expr(&bin_op.left)?),
                right: Box::new(lower_expr(&bin_op.right)?),
            }
        }
        Expr::BooleanLiteral(lit) => HirExpr::BoolLiteral(lit.value),
        Expr::StringLiteral(lit) => HirExpr::StringLiteral(lit.value.to_str().to_string()),
        Expr::FString(fstring) => {
            let parts = fstring
                .value
                .elements()
                .map(|element| -> Result<FStringPart, Diagnostic> {
                    Ok(match element {
                        pycc_ast::InterpolatedStringElement::Literal(lit) => {
                            FStringPart::Literal(lit.value.to_string())
                        }
                        pycc_ast::InterpolatedStringElement::Interpolation(interp) => {
                            if interp.conversion != pycc_ast::ConversionFlag::None {
                                return Err(unsupported(
                                    "f-string conversion flags (!r/!s/!a) are not supported yet",
                                    interp.range,
                                ));
                            }
                            if interp.format_spec.is_some() {
                                return Err(unsupported(
                                    "f-string format spec ({x:...}) is not supported yet",
                                    interp.range,
                                ));
                            }
                            FStringPart::Interpolation(Box::new(lower_expr(&interp.expression)?))
                        }
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            HirExpr::FString(parts)
        }
        Expr::Compare(cmp) => {
            if cmp.ops.len() != 1 {
                return Err(unsupported(
                    format!("chained comparisons are not supported yet: {:?}", cmp.ops),
                    cmp.range,
                ));
            }
            let op = match cmp.ops[0] {
                CmpOp::Eq => CmpOpKind::Eq,
                CmpOp::NotEq => CmpOpKind::NotEq,
                CmpOp::Lt => CmpOpKind::Lt,
                CmpOp::LtE => CmpOpKind::LtE,
                CmpOp::Gt => CmpOpKind::Gt,
                CmpOp::GtE => CmpOpKind::GtE,
                other => {
                    return Err(unsupported(
                        format!("comparison operator not supported yet: {other:?}"),
                        cmp.range,
                    ));
                }
            };
            HirExpr::Compare {
                op,
                left: Box::new(lower_expr(&cmp.left)?),
                right: Box::new(lower_expr(&cmp.comparators[0])?),
            }
        }
        other => {
            return Err(unsupported(
                "expression kind not supported yet",
                pycc_ast::expr_range(other),
            ));
        }
    };
    Ok(lowered)
}

/// Rewrites every occurrence of the bare name `from` inside `expr` to `to`
/// (PR-12, D-117) -- used to give a comprehension's own loop variable a
/// synthesized, collision-proof internal name (see `synthesize_comp_var_name`
/// below) without inventing real lexical scoping. Exhaustive over `HirExpr`
/// on purpose: a future variant added to this enum must add its own arm here
/// too, the same "let the compiler enumerate every site" discipline this
/// project's own `Scalar::List` precedent (D-107) already established for
/// `pycc_codegen`. Safe to apply blindly (no risk of renaming an unrelated
/// same-named binding from some other nested scope) because v0.2's
/// comprehension grammar has no nested comprehensions, no lambda, and no
/// nested function defs inside a comprehension's own `elt`/`cond`/`key`/
/// `value` -- none of those are expressible here at all yet.
fn rename_name_in_expr(expr: HirExpr, from: &str, to: &str) -> HirExpr {
    let recurse = |e: HirExpr| rename_name_in_expr(e, from, to);
    match expr {
        HirExpr::Name(n) => HirExpr::Name(if n == from { to.to_string() } else { n }),
        HirExpr::IntLiteral(_)
        | HirExpr::FloatLiteral(_)
        | HirExpr::BoolLiteral(_)
        | HirExpr::StringLiteral(_) => expr,
        // `callee` (a bare `String`, never an `HirExpr::Name`) is
        // deliberately left untouched even if it equals `from`: this HIR
        // subset has no first-class functions, so `callee` always names a
        // module-level function definition, never a local variable this
        // rename could plausibly shadow -- unlike `args`, which are
        // recursed into normally.
        HirExpr::Call { callee, args } => HirExpr::Call {
            callee,
            args: args.into_iter().map(recurse).collect(),
        },
        HirExpr::BinOp { op, left, right } => HirExpr::BinOp {
            op,
            left: Box::new(recurse(*left)),
            right: Box::new(recurse(*right)),
        },
        HirExpr::Compare { op, left, right } => HirExpr::Compare {
            op,
            left: Box::new(recurse(*left)),
            right: Box::new(recurse(*right)),
        },
        HirExpr::FString(parts) => HirExpr::FString(
            parts
                .into_iter()
                .map(|part| match part {
                    FStringPart::Literal(s) => FStringPart::Literal(s),
                    FStringPart::Interpolation(e) => {
                        FStringPart::Interpolation(Box::new(recurse(*e)))
                    }
                })
                .collect(),
        ),
        HirExpr::ListLiteral(es) => HirExpr::ListLiteral(es.into_iter().map(recurse).collect()),
        HirExpr::Subscript { base, index } => HirExpr::Subscript {
            base: Box::new(recurse(*base)),
            index: Box::new(recurse(*index)),
        },
        HirExpr::Slice {
            base,
            start,
            stop,
            step,
        } => HirExpr::Slice {
            base: Box::new(recurse(*base)),
            start: start.map(|s| Box::new(recurse(*s))),
            stop: stop.map(|s| Box::new(recurse(*s))),
            step: step.map(|s| Box::new(recurse(*s))),
        },
        HirExpr::ListAppend { list, value } => HirExpr::ListAppend {
            list: if list == from { to.to_string() } else { list },
            value: Box::new(recurse(*value)),
        },
        HirExpr::DictLiteral(pairs) => HirExpr::DictLiteral(
            pairs
                .into_iter()
                .map(|(k, v)| (recurse(k), recurse(v)))
                .collect(),
        ),
        HirExpr::SetLiteral(es) => HirExpr::SetLiteral(es.into_iter().map(recurse).collect()),
        HirExpr::TupleLiteral(es) => HirExpr::TupleLiteral(es.into_iter().map(recurse).collect()),
        // `list`/`dict`/`set` base-name fields are plain `String`s, mirroring
        // `ListAppend`'s own arm exactly: renamed only when they equal
        // `from`, otherwise left untouched. This matters for a
        // comprehension's own `elt`/`cond` referencing e.g. `xs.pop()` where
        // `xs` is the loop variable being synthesized-renamed -- the common
        // case (some other, non-loop-variable base) must not be touched.
        HirExpr::ListPop { list } => HirExpr::ListPop {
            list: if list == from { to.to_string() } else { list },
        },
        HirExpr::DictGetOrDefault { dict, key, default } => HirExpr::DictGetOrDefault {
            dict: if dict == from { to.to_string() } else { dict },
            key: Box::new(recurse(*key)),
            default: Box::new(recurse(*default)),
        },
        HirExpr::SetAdd { set, value } => HirExpr::SetAdd {
            set: if set == from { to.to_string() } else { set },
            value: Box::new(recurse(*value)),
        },
    }
}

/// Synthesizes a collision-proof internal name for a comprehension's loop
/// variable (D-117): a leading digit can never begin a valid Python
/// identifier (confirmed against the vendored `ruff_python_parser`'s own
/// tokenizer -- a `NAME` token cannot start with a decimal digit), so this
/// string can never be produced by lowering real Python source, no matter
/// what the user names their own variables -- no new lexical-scoping
/// machinery is needed; this is just another ordinary entry in the existing
/// flat, name-keyed slot model. Seeded by the loop target's own byte offset,
/// not a mutable counter: two distinct comprehensions in one file can never
/// share a target's start offset, so this needs no threaded lowering state
/// and stays fully deterministic across repeated compiles of the same
/// source.
///
/// Takes a plain `u32` byte offset (from `pycc_ast::expr_range`) rather than
/// naming `ruff_text_size::TextSize` directly -- `pycc_hir` depends only on
/// `pycc_ast`, never on `ruff_text_size` (Step 0's own re-export widening is
/// this crate's one and only upstream-crate seam), and `pycc_ast`'s own
/// `expr_range`/`stmt_range` exist specifically to keep that boundary from
/// leaking (see their doc comments).
fn synthesize_comp_var_name(target_start: u32, source_name: &str) -> String {
    format!("0comp_{target_start}_{source_name}")
}

/// Parses `range(...)`'s argument list into `(start, stop, step)` `HirExpr`s,
/// defaulting `start`/`step` per Python's own `range()` overloads. Shared by
/// `Stmt::For`'s own lowering and `lower_comprehension_iter` below (PR-12) --
/// factored out rather than duplicated a second time. Callers are
/// responsible for checking the callee is actually `range` and carries no
/// keyword arguments first (their own diagnostics differ in wording between
/// a plain `for` loop and a comprehension's `for` clause), so this helper
/// only ever inspects `call.arguments.args`.
fn lower_range_call(call: &pycc_ast::ExprCall) -> Result<(HirExpr, HirExpr, HirExpr), Diagnostic> {
    match &*call.arguments.args {
        [stop] => Ok((
            HirExpr::IntLiteral(0),
            lower_expr(stop)?,
            HirExpr::IntLiteral(1),
        )),
        [start, stop] => Ok((
            lower_expr(start)?,
            lower_expr(stop)?,
            HirExpr::IntLiteral(1),
        )),
        [start, stop, step] => Ok((lower_expr(start)?, lower_expr(stop)?, lower_expr(step)?)),
        other => Err(unsupported(
            format!("range() with {} arguments is not supported", other.len()),
            call.range,
        )),
    }
}

/// Resolves a comprehension's `for var in <iter>` clause into a `CompIter`,
/// reusing `Stmt::For`'s own iterable-shape acceptance verbatim (D-117):
/// `range(...)` or a bare name (resolved to `Ty::List`/`Ty::Dict`/`Ty::Set`
/// downstream by `pycc_types`/`pycc_mir`, exactly like a plain `for` loop).
/// Any other shape is rejected with the existing generic `C0001` path,
/// mirroring `Stmt::For`'s own "only `for x in range(...)` or `for x in
/// <list>` is supported so far" message.
fn lower_comprehension_iter(iter_expr: &Expr) -> Result<CompIter, Diagnostic> {
    if let Expr::Name(name) = iter_expr {
        return Ok(CompIter::Name(name.id.as_str().to_string()));
    }
    let Expr::Call(call) = iter_expr else {
        return Err(unsupported(
            format!(
                "only `range(...)` or a bare-name iterable is supported so far in a comprehension: {iter_expr:?}"
            ),
            pycc_ast::expr_range(iter_expr),
        ));
    };
    let Expr::Name(callee) = call.func.as_ref() else {
        return Err(unsupported(
            "only calling `range(...)` is supported so far in a comprehension",
            pycc_ast::expr_range(&call.func),
        ));
    };
    if callee.id.as_str() != "range" {
        return Err(unsupported(
            format!(
                "only iterating over `range(...)` is supported so far in a comprehension, got `{}`",
                callee.id
            ),
            call.range,
        ));
    }
    if !call.arguments.keywords.is_empty() {
        return Err(unsupported(
            "keyword arguments to range() are not supported yet",
            call.range,
        ));
    }
    let (start, stop, step) = lower_range_call(call)?;
    Ok(CompIter::Range { start, stop, step })
}

/// Validates and lowers a comprehension's shared shape (D-117): exactly one
/// generator clause, no `async for`, a bare-name loop target, at most one
/// `if` filter. Returns the loop target's *source* name, its synthesized
/// internal replacement, the resolved `CompIter`, and the (not-yet-renamed)
/// lowered `if`-filter expression, if present -- renaming is the caller's
/// job (`lower_list_comp_assign`/`lower_set_comp_assign`/
/// `lower_dict_comp_assign` below), since `elt`/`key`/`value` also need the
/// identical rename and this helper has no visibility into which of those
/// the caller is building.
///
/// `iter` (the resolved `CompIter` returned above) is deliberately **never**
/// passed through `rename_name_in_expr` -- neither here nor by any caller --
/// unlike `cond`/`elt`/`key`/`value`, which all are. This is not an
/// oversight: it matches real CPython scoping. A comprehension's outermost
/// iterable expression evaluates in the *enclosing* scope, before the
/// comprehension's own scope exists at all -- `[i for i in range(i)]`'s
/// `range(i)` reads the *enclosing* `i`, not the comprehension's own loop
/// variable (confirmed directly against CPython). Renaming `iter`'s
/// occurrences of the source loop-variable name would therefore be actively
/// wrong, not merely redundant: it would make `range(i)` read the
/// comprehension's own (not-yet-bound) synthesized variable instead of
/// whatever `i` means in the enclosing scope. See
/// `a_comprehension_range_iterable_referencing_the_loop_variables_own_source_name_is_not_renamed`
/// and
/// `a_comprehension_bare_name_iterable_sharing_the_loop_variables_own_source_name_is_not_renamed`
/// below, which pin this behavior directly -- without them, a future change
/// that "fixed" this asymmetry by renaming `iter` too would silently break
/// correct scoping with every existing test still green.
fn lower_comprehension_header(
    generators: &[pycc_ast::Comprehension],
) -> Result<(String, String, CompIter, Option<HirExpr>), Diagnostic> {
    // Named `generator`, not `gen` -- `gen` is a reserved keyword as of the
    // 2024 edition (this workspace's own edition, reserved for a future
    // generator-block feature), so the brief's own `gen` binding does not
    // compile here.
    let [generator] = generators else {
        return Err(unsupported(
            "a comprehension with more than one `for` clause is not supported yet",
            generators.first().map(|g| g.range).unwrap_or_default(),
        ));
    };
    if generator.is_async {
        return Err(unsupported(
            "async comprehensions are not supported yet",
            generator.range,
        ));
    }
    let Expr::Name(var) = &generator.target else {
        return Err(unsupported(
            "only a bare name comprehension target is supported so far",
            pycc_ast::expr_range(&generator.target),
        ));
    };
    let cond = match generator.ifs.as_slice() {
        [] => None,
        [single] => Some(lower_expr(single)?),
        _ => {
            return Err(unsupported(
                "a comprehension with more than one `if` filter is not supported yet",
                generator.range,
            ));
        }
    };
    let iter = lower_comprehension_iter(&generator.iter)?;
    let source_name = var.id.as_str().to_string();
    let synth_var =
        synthesize_comp_var_name(pycc_ast::expr_range(&generator.target).start, &source_name);
    Ok((source_name, synth_var, iter, cond))
}

fn lower_list_comp_assign(
    target: &str,
    comp: &pycc_ast::ExprListComp,
) -> Result<HirStmt, Diagnostic> {
    let (source_name, synth_var, iter, cond) = lower_comprehension_header(&comp.generators)?;
    let elt = rename_name_in_expr(lower_expr(&comp.elt)?, &source_name, &synth_var);
    let cond = cond.map(|c| rename_name_in_expr(c, &source_name, &synth_var));
    Ok(HirStmt::ListCompAssign {
        target: target.to_string(),
        var: synth_var,
        iter,
        cond: cond.map(Box::new),
        elt: Box::new(elt),
    })
}

fn lower_set_comp_assign(
    target: &str,
    comp: &pycc_ast::ExprSetComp,
) -> Result<HirStmt, Diagnostic> {
    let (source_name, synth_var, iter, cond) = lower_comprehension_header(&comp.generators)?;
    let elt = rename_name_in_expr(lower_expr(&comp.elt)?, &source_name, &synth_var);
    let cond = cond.map(|c| rename_name_in_expr(c, &source_name, &synth_var));
    Ok(HirStmt::SetCompAssign {
        target: target.to_string(),
        var: synth_var,
        iter,
        cond: cond.map(Box::new),
        elt: Box::new(elt),
    })
}

fn lower_dict_comp_assign(
    target: &str,
    comp: &pycc_ast::ExprDictComp,
) -> Result<HirStmt, Diagnostic> {
    // Real Python's dict-comprehension grammar (`{k: v for ...}`) has no
    // `**`-unpacking form the way a plain `Expr::Dict` literal does -- but
    // unlike that literal case, the parser does *not* reject
    // `{**x for k in y}`-shaped source at parse time: confirmed directly
    // against the vendored `ruff_python_parser` (0.0.6), which parses it
    // successfully as `ExprDictComp { key: None, value: Name("x"), .. }`,
    // silently dropping the `**` token rather than erroring. The brief this
    // task followed assumed `key: None` was unreachable from real parsed
    // source and modeled it with an `unreachable!()`/`.expect()` internal
    // panic; that assumption is false, so this is a real (if unusual)
    // C0001 capability diagnostic, mirroring `Expr::Dict`'s own analogous
    // `**`-unpacking rejection, not an internal-error panic.
    let Some(key_expr) = comp.key.as_deref() else {
        return Err(unsupported(
            "dict-unpacking (`**expr`) inside a dict comprehension is not supported yet",
            pycc_ast::expr_range(&comp.value),
        ));
    };
    let (source_name, synth_var, iter, cond) = lower_comprehension_header(&comp.generators)?;
    let key = rename_name_in_expr(lower_expr(key_expr)?, &source_name, &synth_var);
    let value = rename_name_in_expr(lower_expr(&comp.value)?, &source_name, &synth_var);
    let cond = cond.map(|c| rename_name_in_expr(c, &source_name, &synth_var));
    Ok(HirStmt::DictCompAssign {
        target: target.to_string(),
        var: synth_var,
        iter,
        cond: cond.map(Box::new),
        key: Box::new(key),
        value: Box::new(value),
    })
}

fn unsupported<R>(message: impl Into<String>, range: R) -> Diagnostic
where
    std::ops::Range<u32>: From<R>,
{
    let range = std::ops::Range::<u32>::from(range);
    Diagnostic::error("C0001", message, Span::new(range.start, range.end))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // change that re-inflates Ty back to its pre-fix size.
        assert_eq!(
            std::mem::size_of::<Ty>(),
            16,
            "size_of::<Ty>() must stay 16 bytes (PR-10 Task 14, D-109) -- PR-11 adds \
             no new Ty variants, so this must not move; if it does, something in this \
             PR accidentally widened Ty's boxing, not the containers themselves",
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
    fn unsupported_statement_and_expression_return_spanned_capability_diagnostics() {
        assert_capability_error(
            "if True:\n    pass\n",
            "statement kind not supported yet",
            Span::new(13, 17),
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
            ("function body", "def _f():\n    pass\n"),
            ("if test", "if (lambda: 1):\n    print(1)\n"),
            ("if else body", "if True:\n    print(1)\nelse:\n    pass\n"),
            ("while test", "while (lambda: 1):\n    print(1)\n"),
            ("while body", "while True:\n    pass\n"),
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
            ("for body", "for i in range(1):\n    pass\n"),
            ("return value", "def _f():\n    return (lambda: 1)\n"),
            (
                "elif test",
                "if True:\n    print(1)\nelif (lambda: 1):\n    print(2)\n",
            ),
            (
                "elif body",
                "if True:\n    print(1)\nelif True:\n    pass\n",
            ),
            (
                "nested else body",
                "if True:\n    print(1)\nelif True:\n    print(2)\nelse:\n    pass\n",
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
    fn assigning_to_a_non_name_target_is_unsupported() {
        assert_capability_error_message(
            "x.attr = 1\n",
            "only assigning to a bare name is supported so far",
        );
    }

    #[test]
    fn subscript_assignment_to_a_bare_name_base_lowers_to_dict_set() {
        // PR-11 Task 3 (D-113) supersedes D-105's "no subscript assignment
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
    fn a_pass_statement_is_unsupported() {
        // `if` itself is supported (Task 8); `pass` inside it is not -- no
        // v0.1 grammar construct needs it (empty bodies aren't reachable
        // through anything pycc lowers) and it exercises the same catch-all
        // that `unsupported_statement_and_expression_return_spanned_capability_diagnostics`
        // does for expressions (a tuple literal filled that role there
        // until this task; a lambda does now, D-116).
        assert_capability_error_message("if True:\n    pass\n", "statement kind not supported yet");
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
    fn a_top_level_async_for_is_not_supported_yet() {
        assert_capability_error_message(
            "async for i in range(3):\n    print(i)\n",
            "async for is not supported yet",
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
    fn a_generic_function_is_rejected_without_losing_its_type_parameters() {
        assert_capability_error_message(
            "def f[T]() -> None:\n    return\n",
            "generic function type parameters are not supported yet",
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
        // Matches Stmt::Assign's own existing restriction (only a bare name
        // target is supported so far) -- e.g. `obj.attr: int = 1` has no
        // attribute-access support anywhere else in the compiler either.
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

    // -- PR-11 Task 3 (D-113): dict[str, int] frontend HIR forms ---------

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

    // -- PR-11 Task 7 (D-113): set[int] frontend HIR forms ---------------

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
    fn subscripted_type_annotation_is_still_rejected_on_purpose() {
        // D-105: v0.2 adds no annotation-syntax support for list[T]. This test
        // locks in that `x: list[int] = []` keeps failing today's existing
        // "only a bare name type annotation" capability error, so a future
        // change to `annotation_to_ty` doesn't silently start accepting this
        // without its own deliberate decision.
        assert_capability_error_message(
            "x: list[int] = []\n",
            "only a bare name type annotation is supported so far",
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
    fn calling_an_unrecognized_method_is_unsupported() {
        // Any other `.method()` call is rejected before ever falling through
        // to the bare-name-callee check below it -- this project only
        // special-cases `.append()`/`.pop()`/`.get()`/`.add()`, not general
        // method dispatch (D-105, widened by D-119). `.remove()` is a
        // deliberately chosen example: it is a real `list` method D-119
        // explicitly did not ship (`list.pop()` is the one growable/lookup
        // operation this PR ships for `list`), not a strawman.
        assert_capability_error_message(
            "foo.bar()\n",
            "only the `.append()`/`.pop()`/`.get()`/`.add()` methods are supported so far, got `.bar(...)`",
        );
        assert_capability_error_message(
            "x.remove(1)\n",
            "only the `.append()`/`.pop()`/`.get()`/`.add()` methods are supported so far, got `.remove(...)`",
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
        // -- `.add()`'s value is always `None`, same D-072 gap as `.append()`.
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
        let err = lower_comprehension_header(&[]).unwrap_err();
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
    }
}

#[cfg(test)]
mod pycc_parser_test_helper {
    pub fn parse(source: &str) -> pycc_ast::ModModule {
        pycc_parser::parse(source).expect("test fixture must parse")
    }
}
