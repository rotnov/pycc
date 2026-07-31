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
    /// The key/value pair is boxed together as a single pointer (D-109 --
    /// shrinks `Ty`'s own size, closing a real frontend-throughput
    /// regression the original two-separate-`Box<Ty>` shape caused), not
    /// because `dict[K,V]` needs its own codegen yet.
    Dict(Box<(Ty, Ty)>),
    /// `set[T]`. Same status as `Dict` above -- PR-11's own scope.
    Set(Box<Ty>),
    /// `tuple[A, B, ...]`. Same status as `Dict` above -- PR-11's own
    /// scope. Boxed (D-109) so every dataful variant of `Ty` is a uniform
    /// thin (8-byte) pointer: a first attempt boxed this as `Box<[Ty]>`
    /// (a 16-byte fat pointer -- data ptr + length), which measured
    /// `size_of::<Ty>() == 24` bytes, no reduction at all from the
    /// pre-fix size, because more than one variant here carries data of a
    /// different shape (`List`/`Dict`/`Set` are already thin `Box`
    /// pointers, `Tuple` was not), which defeats rustc's niche-filling
    /// enum-layout optimization (the trick that makes
    /// `size_of::<Option<Box<T>>>() == size_of::<Box<T>>()`): with no
    /// uniform niche across all dataful variants, rustc falls back to an
    /// explicit discriminant tag, adding a full pointer-aligned word on
    /// top of the *largest* variant's payload. `Box<Vec<Ty>>` (a second
    /// indirection: a thin pointer to a heap-allocated `Vec<Ty>`) closes
    /// that gap by making every dataful variant exactly 8 bytes, which
    /// measured `size_of::<Ty>() == 16` bytes -- a real reduction.
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
    /// `base[index]`, read-only (D-105 -- no subscript assignment target
    /// exists in v0.2). Every statement that extracts an assignment/for
    /// target rejects a non-bare-name target (see `Stmt::Assign`,
    /// `Stmt::AnnAssign`, and `Stmt::For`'s target handling in
    /// `lower_stmt` below) before ever calling `lower_expr` on it, so a
    /// `Subscript` node reaches this arm only in a value (Load) position;
    /// no separate `ExprContext` check is needed to enforce read-only-ness
    /// here.
    Subscript {
        base: Box<HirExpr>,
        index: Box<HirExpr>,
    },
    /// `list.append(value)`, recognized as a single dedicated node rather
    /// than through any general method-call mechanism (D-105). Unlike
    /// `Subscript` above, this arm is *not* structurally restricted to any
    /// particular position -- because `ListAppend` is an `HirExpr` (not a
    /// statement-only form), it currently lowers successfully anywhere an
    /// expression is accepted, e.g. `y = x.append(2)` or
    /// `print(x.append(1))`, even though real Python's `list.append()`
    /// always returns `None` there and a value-producing use is
    /// meaningless. This lowering step deliberately does not judge that --
    /// rejecting a value-position `.append()` (or any type-driven
    /// distinction at all) is `pycc_types`' job, not this one's (see
    /// `list_append_used_as_a_value_lowers_successfully_today` below, which
    /// locks in today's actual behavior).
    ListAppend {
        list: String,
        value: Box<HirExpr>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum FStringPart {
    Literal(String),
    Interpolation(Box<HirExpr>),
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
    /// not here.
    ForList {
        var: String,
        list: String,
        body: Vec<HirStmt>,
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
            let Expr::Name(name) = target else {
                return Err(unsupported(
                    format!("only assigning to a bare name is supported so far: {target:?}"),
                    pycc_ast::expr_range(target),
                ));
            };
            HirStmt::Assign {
                target: name.id.as_str().to_string(),
                value: lower_expr(&assign.value)?,
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
            // A bare-name iterable is `for v in some_list:` (D-105) --
            // resolved to `Ty::List` or rejected by pycc_types, not here;
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
            let (start, stop, step) = match &*call.arguments.args {
                [stop] => (
                    HirExpr::IntLiteral(0),
                    lower_expr(stop)?,
                    HirExpr::IntLiteral(1),
                ),
                [start, stop] => (
                    lower_expr(start)?,
                    lower_expr(stop)?,
                    HirExpr::IntLiteral(1),
                ),
                [start, stop, step] => (lower_expr(start)?, lower_expr(stop)?, lower_expr(step)?),
                other => {
                    return Err(unsupported(
                        format!("range() with {} arguments is not supported", other.len()),
                        call.range,
                    ));
                }
            };
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
        Expr::Subscript(sub) => HirExpr::Subscript {
            base: Box::new(lower_expr(&sub.value)?),
            index: Box::new(lower_expr(&sub.slice)?),
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
                return Err(unsupported(
                    format!(
                        "only the `.append()` method is supported so far, got `.{}(...)`",
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
        assert!(
            std::mem::size_of::<Ty>() < 24,
            "Ty::size_of() is {} bytes -- expected a real reduction from the pre-D-109 24 bytes",
            std::mem::size_of::<Ty>()
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
            "x = (1, 2)\n",
            "expression kind not supported yet",
            Span::new(4, 10),
        );
    }

    #[test]
    fn capability_errors_propagate_through_every_supported_container() {
        // Tuple literals (`(1, 2)`) are this table's "genuinely unhandled at
        // every level" poison fixture -- a list literal used to fill this
        // role (see `a_tuple_literal_expression_is_unsupported`'s own
        // comment) until Task 7 (D-105) added list-literal lowering.
        let cases = [
            ("function body", "def _f():\n    pass\n"),
            ("if test", "if (1, 2):\n    print(1)\n"),
            ("if else body", "if True:\n    print(1)\nelse:\n    pass\n"),
            ("while test", "while (1, 2):\n    print(1)\n"),
            ("while body", "while True:\n    pass\n"),
            (
                "one-argument range stop",
                "for i in range((1, 2)):\n    print(i)\n",
            ),
            (
                "two-argument range start",
                "for i in range((1, 2), 1):\n    print(i)\n",
            ),
            (
                "two-argument range stop",
                "for i in range(0, (1, 2)):\n    print(i)\n",
            ),
            (
                "three-argument range start",
                "for i in range((1, 2), 1, 1):\n    print(i)\n",
            ),
            (
                "three-argument range stop",
                "for i in range(0, (1, 2), 1):\n    print(i)\n",
            ),
            (
                "three-argument range step",
                "for i in range(0, 1, (1, 2)):\n    print(i)\n",
            ),
            ("for body", "for i in range(1):\n    pass\n"),
            ("return value", "def _f():\n    return (1, 2)\n"),
            (
                "elif test",
                "if True:\n    print(1)\nelif (1, 2):\n    print(2)\n",
            ),
            (
                "elif body",
                "if True:\n    print(1)\nelif True:\n    pass\n",
            ),
            (
                "nested else body",
                "if True:\n    print(1)\nelif True:\n    print(2)\nelse:\n    pass\n",
            ),
            ("binary left operand", "x = (1, 2) + 1\n"),
            ("binary right operand", "x = 1 + (1, 2)\n"),
            ("f-string interpolation", "x = f\"{(1, 2)}\"\n"),
            ("comparison left operand", "x = (1, 2) == 1\n"),
            ("comparison right operand", "x = 1 == (1, 2)\n"),
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
    fn subscript_assignment_target_is_unsupported() {
        // D-105: v0.2's `list[int]` slice is read-only -- there is no
        // subscript assignment target anywhere in this file (see
        // `HirExpr::Subscript`'s own doc comment). That invariant holds
        // today only as an incidental consequence of `Stmt::Assign`'s
        // existing bare-name-target check above (a `Subscript` node is not
        // an `Expr::Name`, so it's rejected there, the same as `x.attr = 1`)
        // -- not through any dedicated subscript-specific check. This test
        // names `x[0] = 1` explicitly so a future refactor of that target
        // extraction can't silently regress the read-only invariant without
        // a test calling it out by name, even though the message and code
        // path are shared with the `x.attr = 1` case above.
        assert_capability_error_message(
            "x[0] = 1\n",
            "only assigning to a bare name is supported so far",
        );
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
        // as `a_tuple_literal_expression_is_unsupported` does for expressions.
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
    fn a_tuple_literal_expression_is_unsupported() {
        // Before Task 7 (D-105), a list literal filled this role (every
        // other kind handled so far -- numbers, names, calls, binops,
        // bools, comparisons -- has its own dedicated arm) as the "genuinely
        // unhandled at every level" fixture that exercises the final catch-
        // all arm. List literals are supported now (see
        // `lowers_a_list_literal`), so a tuple literal -- dict/set/tuple
        // containers are out of this PR's scope -- takes over that role.
        assert_capability_error_message("x = (1, 2)\n", "expression kind not supported yet");
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
        assert_capability_error_message("x: int = (1, 2)\n", "expression kind not supported yet");
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
    fn calling_a_non_append_method_is_unsupported() {
        // Any other `.method()` call is rejected before ever falling through
        // to the bare-name-callee check below it -- this task only special-
        // cases `.append()`, not general method dispatch (D-105).
        assert_capability_error_message(
            "foo.bar()\n",
            "only the `.append()` method is supported so far, got `.bar(...)`",
        );
    }

    // The five tests below exercise each new arm's own `?`-propagation path
    // specifically (an inner element/base/index/argument/body expression
    // that itself fails to lower), as opposed to every test above, which
    // only ever supplies inner expressions that lower successfully.

    #[test]
    fn a_list_literal_with_an_unsupported_element_propagates_the_element_error() {
        assert_capability_error_message("x = [(1, 2)]\n", "expression kind not supported yet");
    }

    #[test]
    fn a_subscript_with_an_unsupported_base_propagates_the_base_error() {
        assert_capability_error_message("y = (1, 2)[0]\n", "expression kind not supported yet");
    }

    #[test]
    fn a_subscript_with_an_unsupported_index_propagates_the_index_error() {
        assert_capability_error_message("y = x[(1, 2)]\n", "expression kind not supported yet");
    }

    #[test]
    fn an_append_with_an_unsupported_argument_propagates_the_argument_error() {
        assert_capability_error_message("x.append((1, 2))\n", "expression kind not supported yet");
    }

    #[test]
    fn a_for_list_body_with_an_unsupported_statement_propagates_the_body_error() {
        assert_capability_error_message(
            "x = [1, 2, 3]\nfor v in x:\n    (1, 2)\n",
            "expression kind not supported yet",
        );
    }
}

#[cfg(test)]
mod pycc_parser_test_helper {
    pub fn parse(source: &str) -> pycc_ast::ModModule {
        pycc_parser::parse(source).expect("test fixture must parse")
    }
}
