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
use pycc_ast::{Expr, Number, Stmt};
use pycc_diag::Diagnostic;

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
    pub attrs: Vec<(String, Ty)>,
    pub methods: Vec<(String, String)>,
}

/// Lowers a module-level `class Foo: ...` statement (D-154). Returns the
/// class's own declared shape (for `HirModule::class_defs`) alongside every
/// method it defines, already lowered into ordinary mangled
/// `HirItem::Function`s ready to append to `HirModule::items` -- see this
/// module's own doc comment for why a class has no `HirItem` of its own.
///
/// Every check below is a `C0001` capability diagnostic, not a design
/// question this PR resolves: generic classes (`class C[T]:`) and
/// inheritance (`class C(Base):`) are both explicitly Part 3 of #375 (#387)
/// per the plan's Correction 1, and a class decorator is out of scope
/// entirely (dataclasses/`dataclass_transform` are unrelated later PRs' own
/// scope).
pub(crate) fn lower_class(
    def: &pycc_ast::StmtClassDef,
    aliases: &[(String, Ty)],
) -> Result<(HirClassDef, Vec<HirItem>), Diagnostic> {
    if !def.decorator_list.is_empty() {
        return Err(unsupported("class decorators are not supported yet", def.range));
    }
    if def.type_params.is_some() {
        return Err(unsupported(
            "a generic class (`class C[T]:`) is not supported yet",
            def.range,
        ));
    }
    if let Some(arguments) = def.arguments.as_deref()
        && (!arguments.args.is_empty() || !arguments.keywords.is_empty())
    {
        return Err(unsupported(
            "class inheritance (`class C(Base):`) is not supported yet",
            def.range,
        ));
    }
    let class_name = def.name.to_string();
    let mut methods: Vec<(String, String)> = Vec::new();
    let mut items: Vec<HirItem> = Vec::new();
    let mut attrs: Vec<(String, Ty)> = Vec::new();
    let mut init_seen = false;
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
        let (item, params) = lower_method(method_def, &class_name, aliases)?;
        if method_name == "__init__" {
            init_seen = true;
            attrs = collect_init_attrs(&method_def.body, &params)?;
        }
        let mangled = format!("{class_name}.{method_name}");
        // #386: rebind semantics for non-`__init__` method redefinition.
        // Both definitions share the same mangled `<ClassName>.<method>`
        // name, so PR #358's function-pointer slot infrastructure already
        // handles the actual rebind at the codegen level (the second `def`'s
        // source-order execution stores the new function's address into the
        // slot). Here, replacing the method table entry on redefinition
        // rather than appending a duplicate keeps the table clean -- the
        // mangled name is the same either way, so `resolve_method_call` and
        // MIR lowering's `.methods.iter().find(..)` resolve identically.
        if let Some(entry) = methods.iter_mut().find(|(name, _)| name == &method_name) {
            *entry = (method_name.clone(), mangled.clone());
        } else {
            methods.push((method_name.clone(), mangled));
        }
        items.push(item);
    }
    if !methods.iter().any(|(name, _)| name == "__init__") {
        return Err(unsupported(
            "a class without an `__init__` method is not supported yet",
            def.range,
        ));
    }
    Ok((
        HirClassDef {
            name: class_name,
            attrs,
            methods,
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
    aliases: &[(String, Ty)],
) -> Result<(HirItem, Vec<(String, Ty)>), Diagnostic> {
    if def.is_async {
        return Err(unsupported("an async method is not supported yet", def.range));
    }
    if !def.decorator_list.is_empty() {
        return Err(unsupported("method decorators are not supported yet", def.range));
    }
    if def.type_params.is_some() {
        return Err(unsupported("a generic method is not supported yet", def.range));
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
    let method_name = def.name.as_str();
    let is_public = !method_name.starts_with('_'); // D-038
    // See this function's own doc comment: `__init__`'s own parameters
    // always require an annotation, regardless of D-038's usual
    // public-name-only rule.
    let params_is_public = is_public || method_name == "__init__";
    let self_ty = Ty::Instance(Box::new(class_name.to_string()));
    let mut params = vec![("self".to_string(), self_ty)];
    params.extend(lower_arg_list(
        rest,
        params_is_public,
        method_name,
        None,
        aliases,
    )?);
    let return_ty = crate::lower_return_annotation(
        def.returns.as_deref(),
        is_public,
        method_name,
        None,
        aliases,
    )?;
    let body = crate::stmt::lower_body(&def.body, aliases, false, true)?;
    let mangled_name = format!("{class_name}.{method_name}");
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
                Some(ty @ (Ty::Int | Ty::Float | Ty::Bool | Ty::Str)) => Ok(ty),
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
    fn a_generic_class_is_unsupported() {
        assert_c0001("class C[T]:\n    def __init__(self) -> None:\n        return\n");
    }

    #[test]
    fn a_class_with_a_positional_base_is_unsupported() {
        assert_c0001("class C(Base):\n    def __init__(self) -> None:\n        return\n");
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
            .filter(|item| {
                matches!(item, HirItem::Function { name, .. } if name == "C.foo")
            })
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
            diagnostic
                .message
                .contains("method name `get` collides with the compiler's built-in container-method syntax"),
            "unexpected message: {}",
            diagnostic.message
        );
        assert!(
            !diagnostic.message.contains("dict.get() takes exactly two arguments"),
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
            ("append", "c.append()", "list.append() takes exactly one argument, got 0"),
            ("pop", "c.pop(1)", "list.pop() takes no arguments, got 1"),
            ("add", "c.add()", "set.add() takes exactly one argument, got 0"),
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
                attrs: vec![
                    ("x".to_string(), Ty::Int),
                    ("y".to_string(), Ty::Int),
                ],
                methods: vec![("__init__".to_string(), "Point.__init__".to_string())],
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

    // -- collect_init_attrs / slot_ty_from_init_rhs -------------------------

    #[test]
    fn an_init_attr_assigned_from_an_unrelated_name_is_unsupported() {
        assert_c0001(
            "class C:\n    def __init__(self, x: int) -> None:\n        self.y = z\n",
        );
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
        assert_c0001(
            "class C:\n    def __init__(self) -> None:\n        self.link = self\n",
        );
    }

    #[test]
    fn an_init_attr_assigned_an_int_literal_establishes_an_int_slot() {
        let hir = lower_ok("class C:\n    def __init__(self) -> None:\n        self.x = 5\n");
        assert_eq!(hir.class_defs[0].1.attrs, vec![("x".to_string(), Ty::Int)]);
    }

    #[test]
    fn an_init_attr_assigned_a_float_literal_establishes_a_float_slot() {
        let hir = lower_ok("class C:\n    def __init__(self) -> None:\n        self.x = 1.5\n");
        assert_eq!(hir.class_defs[0].1.attrs, vec![("x".to_string(), Ty::Float)]);
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
        let hir =
            lower_ok("class C:\n    def __init__(self) -> None:\n        self.x = \"hi\"\n");
        assert_eq!(hir.class_defs[0].1.attrs, vec![("x".to_string(), Ty::Str)]);
    }

    #[test]
    fn an_init_attr_assigned_an_arithmetic_expression_is_unsupported() {
        assert_c0001(
            "class C:\n    def __init__(self, x: int) -> None:\n        self.y = x + 1\n",
        );
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
        assert_c0001(
            "class C:\n    def __init__(self) -> None:\n        self.x = self.y = 0\n",
        );
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
}
